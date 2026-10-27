//! The membership log of a shared space (M13, ADR-0043, `docs/security/sharing.md` section 3,
//! `docs/development/milestone-13.md` section 1.2).
//!
//! Membership entries are feed items of the envelope kind `Membership` (9). An entry is one
//! envelope: the **clear header** ([`MembershipHeader`], everything the server must check), the
//! **sealed private part** ([`Private`], under the membership-meta key
//! `MK[e] = HKDF(SK[e], "membership-meta", feed)`, `e` = the header's `meta_epoch`) and a device
//! signature over both (context `envelope/membership`). The log has its own `seq` (the `Create`
//! is 0) and a `prev` hash (BLAKE3 of the previous entry's encoding); the server makes it linear
//! with a compare-and-swap, and the feed cursor orders it against content.
//!
//! [`fold`] and [`Timeline::apply`] run **identically** on the server (over its own copy of the
//! device logs, which it needs no key for) and on every client: they check the chain, the
//! canonical encoding, the signature by a device of the header's account found in that account's
//! log (devices revoked later are accepted: entries written before a revocation stay valid; the
//! server additionally refuses inactive devices at append time), the rules of the table in
//! ADR-0043, the completeness of every `Rekey` (its boxes cover exactly the active devices of
//! every member as of `member_heads`) and that heads are monotone per member. They cannot check
//! what only a recipient can: that its own box opens and matches the commitment, and that
//! `prev_wrapped` opens to the previous committed key. That is
//! [`verify_for_recipient`]. Invite expiry is the one rule only the server evaluates (its clock).
//!
//! # Clear header (version 2, little-endian)
//!
//! ```text
//! u8 2 | [16] feed | u64 seq | [32] prev | [16] account | [16] device | u8 kind | fields |
//! i64 at | u32 meta_epoch
//! ```
//!
//! `kind` is the [`EntryKind`] byte (1 to 12 in the order of the table of ADR-0043). The `fields`
//! are those of [`EntryBody`], length-prefixed where they vary; the layout of each is in the
//! `codec` submodule and frozen by golden vectors in the tests.
//!
//! # What is where
//!
//! The rules of the fold are in `fold` and their stable names in [`rules`]; this file holds the
//! types, the entry (build, decode, verify, open) and the [`Timeline`] that answers "who was what
//! at cursor `c`".
//!
//! # What the fold does not do
//!
//! It never reads a clock: the expiry of an invite ([`InviteRecord::is_expired`]), the rate limit
//! of `Rekey`s, the refusal of a revoked device at append time and the 30-day horizon of a new
//! `Invite` against the server's own time are the server's. Nor does it see a key: whether a
//! recipient's box opens, matches the commitment and chains back to the previous key is
//! [`verify_for_recipient`].

mod codec;
mod fold;
#[cfg(test)]
mod prop;
pub mod rules;
#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use kb_crypto::{
    CryptoError, Envelope, EnvelopeKind, Nonce24, SIGNATURE_LEN, Secret32, Signature, VerifyingKey,
    WrappedKey,
};
use kb_types::sharing::{MAX_BOXES_PER_ENTRY, SpaceSettings};
use kb_types::{AccountId, DeviceId, FeedId, InviteId, MemberRole, Rng};

use crate::KeysError;
use crate::device::{DeviceKeys, DevicePublicKeys};
use crate::log::DeviceLog;
use crate::space_keys::{SpaceKey, open_space_key_box, unwrap_prev};

/// Version byte of the clear header: the protocol version of ADR-0045.
pub const MEMBERSHIP_VERSION: u8 = kb_types::sync::PROTOCOL_VERSION;

/// What a membership entry does: the twelve kinds of ADR-0043, kind bytes 1 to 12 in table order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum EntryKind {
    /// 1: the creator becomes Owner; creates the feed.
    Create,
    /// 2: an Owner writes an invite.
    Invite,
    /// 3: an Owner revokes an open invite.
    InviteRevoke,
    /// 4: the joiner's device redeems an invite.
    Join,
    /// 5: an Owner changes a member's role.
    RoleSet,
    /// 6: an Owner removes a member, with the Rekey inline.
    Remove,
    /// 7: a member leaves; sets `pending_rekey`.
    Leave,
    /// 8: any member re-keys the space (`epoch + 1`).
    Rekey,
    /// 9: a member boxes the current key for a member who joined with an older bundle.
    Grant,
    /// 10: a member publishes its display name.
    Profile,
    /// 11: an Owner changes the space settings.
    Settings,
    /// 12: an Owner closes the space; terminal.
    Close,
}

impl EntryKind {
    /// Every kind, in kind-byte order.
    pub const ALL: [Self; 12] = [
        Self::Create,
        Self::Invite,
        Self::InviteRevoke,
        Self::Join,
        Self::RoleSet,
        Self::Remove,
        Self::Leave,
        Self::Rekey,
        Self::Grant,
        Self::Profile,
        Self::Settings,
        Self::Close,
    ];

    /// The kind byte, 1 to 12.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Create => 1,
            Self::Invite => 2,
            Self::InviteRevoke => 3,
            Self::Join => 4,
            Self::RoleSet => 5,
            Self::Remove => 6,
            Self::Leave => 7,
            Self::Rekey => 8,
            Self::Grant => 9,
            Self::Profile => 10,
            Self::Settings => 11,
            Self::Close => 12,
        }
    }

    /// Parses [`Self::to_byte`]; any other byte is `None`.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Create),
            2 => Some(Self::Invite),
            3 => Some(Self::InviteRevoke),
            4 => Some(Self::Join),
            5 => Some(Self::RoleSet),
            6 => Some(Self::Remove),
            7 => Some(Self::Leave),
            8 => Some(Self::Rekey),
            9 => Some(Self::Grant),
            10 => Some(Self::Profile),
            11 => Some(Self::Settings),
            12 => Some(Self::Close),
            _ => None,
        }
    }
}

/// A position in a hash-chained log: the sequence number and the hash of an entry. Used for the
/// head of a membership log (hash of the entry's encoding), for the head of a device log (hash of
/// `LogEntry::encode`) and for the `log_seq` / `log_hash` of a `Join`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChainHead {
    /// Sequence number of the last entry (0 for the first).
    pub seq: u64,
    /// Hash of that entry.
    pub hash: [u8; 32],
}

/// Where a member's device log stood when a `Create` or a `Rekey` was made: which devices were
/// active is read from the log as of this head. Heads are monotone per member across entries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemberHead {
    /// The member.
    pub account: AccountId,
    /// The head of the member's device log.
    pub head: ChainHead,
}

/// The space key of an epoch sealed to one device (`space_keys::seal_space_key_for`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyBox {
    /// The recipient device (of a member's log, active as of the entry's `member_heads`).
    pub device: DeviceId,
    /// The sealed box.
    pub bytes: Vec<u8>,
}

/// The fields of a `Rekey`, also carried inline by a `Remove` (atomic with it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RekeyFields {
    /// `epoch + 1`.
    pub new_epoch: u32,
    /// `sk_commitment(feed, new_epoch, SK[new_epoch])`.
    pub sk_commitment: [u8; 32],
    /// `SK[new_epoch - 1]` wrapped under `SK[new_epoch]` (`space_keys::wrap_prev`).
    pub prev_wrapped: WrappedKey,
    /// One box per active device of every member (at most `MAX_BOXES_PER_ENTRY`).
    pub boxes: Vec<KeyBox>,
    /// Each member's device log head the boxes were computed from (see [`MemberHead`]).
    pub member_heads: Vec<MemberHead>,
}

/// The clear fields of an entry, by kind (ADR-0043). The private fields are in [`Private`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntryBody {
    /// The creator becomes Owner. `seq` 0; the first epoch's commitment and boxes; the creator's
    /// own head (`member_heads` has one element).
    Create {
        /// The first shared epoch (a space's current personal epoch + 1, ADR-0045).
        epoch: u32,
        /// `sk_commitment(feed, epoch, SK[epoch])`.
        sk_commitment: [u8; 32],
        /// One box per active device of the creator.
        boxes: Vec<KeyBox>,
        /// The creator's head.
        member_heads: Vec<MemberHead>,
    },
    /// An Owner writes an invite (role Editor, Commenter or Viewer; `expires_at` at most
    /// `INVITE_MAX_DAYS` ahead; at most `MAX_OPEN_INVITES` open).
    Invite {
        /// `HKDF(s, "invite-id")[..16]`.
        invite_id: InviteId,
        /// The Ed25519 public key of `HKDF(s, "invite-sign")`.
        invite_pub: VerifyingKey,
        /// The role it grants (never Owner).
        role: MemberRole,
        /// Unix ms; the server's clock decides.
        expires_at: i64,
        /// The account a code invite is bound to.
        bound_account: Option<AccountId>,
        /// The short authentication string was compared (code flow).
        verified: bool,
        /// Link flow: the bundle sealed under `HKDF(s, "invite-bundle")`
        /// (`invite::seal_link_bundle`).
        enc_bundle: Option<Vec<u8>>,
    },
    /// An Owner revokes an open invite.
    InviteRevoke {
        /// The invite.
        invite_id: InviteId,
    },
    /// The joiner's device redeems an invite.
    Join {
        /// The invite (open, unexpired, bound to this account if bound).
        invite_id: InviteId,
        /// An invite-key signature over `feed ‖ invite_id ‖ account ‖ log_seq ‖ log_hash`
        /// (`invite::sign_join_proof`).
        proof: Signature,
        /// The joining account (not a member yet).
        account: AccountId,
        /// The head of the joiner's device log: the server checks the proof, members check the
        /// log hashes to it.
        log: ChainHead,
    },
    /// An Owner changes a member's role (at least one Owner remains).
    RoleSet {
        /// The member.
        target: AccountId,
        /// The new role.
        role: MemberRole,
    },
    /// An Owner removes a member (not itself), with the Rekey that locks the member out.
    Remove {
        /// The member.
        target: AccountId,
        /// The Rekey, atomic with the removal.
        rekey: RekeyFields,
    },
    /// A member leaves (not the last Owner); sets `pending_rekey`.
    Leave,
    /// Any member re-keys the space; clears `pending_rekey`.
    Rekey(RekeyFields),
    /// A member holding `SK[epoch]` boxes it for a member who joined with an older bundle (at most
    /// one per member and epoch).
    Grant {
        /// The member the boxes are for.
        member: AccountId,
        /// The current epoch.
        epoch: u32,
        /// One box per active device of that member.
        boxes: Vec<KeyBox>,
    },
    /// A member publishes its display name (in [`Private`]).
    Profile,
    /// An Owner changes the settings (in [`Private`]).
    Settings,
    /// An Owner closes the space; nothing follows.
    Close,
}

impl EntryBody {
    /// The kind of this body.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        match self {
            Self::Create { .. } => EntryKind::Create,
            Self::Invite { .. } => EntryKind::Invite,
            Self::InviteRevoke { .. } => EntryKind::InviteRevoke,
            Self::Join { .. } => EntryKind::Join,
            Self::RoleSet { .. } => EntryKind::RoleSet,
            Self::Remove { .. } => EntryKind::Remove,
            Self::Leave => EntryKind::Leave,
            Self::Rekey(_) => EntryKind::Rekey,
            Self::Grant { .. } => EntryKind::Grant,
            Self::Profile => EntryKind::Profile,
            Self::Settings => EntryKind::Settings,
            Self::Close => EntryKind::Close,
        }
    }
}

/// The clear header of a membership entry: what the server reads without any key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MembershipHeader {
    /// The feed.
    pub feed: FeedId,
    /// Position in the membership log (0 for the `Create`).
    pub seq: u64,
    /// BLAKE3 of the previous entry's encoding (zeros for the `Create`).
    pub prev: [u8; 32],
    /// The signing account.
    pub account: AccountId,
    /// The signing device (resolved inside `account`'s log).
    pub device: DeviceId,
    /// Unix ms the signer wrote (informational).
    pub at: i64,
    /// The epoch of the space key the private part is sealed under.
    pub meta_epoch: u32,
    /// The kind-specific clear fields.
    pub body: EntryBody,
}

impl MembershipHeader {
    /// The kind byte's meaning.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        self.body.kind()
    }

    /// The canonical header bytes (layout in the [module documentation](self)): the header of the
    /// envelope and what `peek_membership` of `kb-sync` reads.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        codec::encode_header(self)
    }

    /// Parses [`Self::encode`] output, strictly (canonical: re-encoding gives the same bytes; an
    /// unknown version or kind, a truncation, trailing bytes, a role that is not 1 to 4, more than
    /// `MAX_BOXES_PER_ENTRY` boxes are [`KeysError::Corrupted`]).
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        codec::decode_header(bytes)
    }
}

/// The private fields of an entry, sealed under `MK[meta_epoch]` (the server never sees them).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Private {
    /// A `Create`: the space's name, its settings and the creator's display name.
    Create {
        /// The space name.
        space_name: String,
        /// The first settings.
        settings: SpaceSettings,
        /// The creator's display name (1 to `PROFILE_NAME_MAX_CHARS` characters).
        profile_name: String,
    },
    /// A `Join`: the joiner's display name.
    Join {
        /// The display name.
        profile_name: String,
    },
    /// A `Profile`: a member's new display name.
    Profile {
        /// The display name.
        display_name: String,
    },
    /// A `Settings`: the new settings.
    Settings {
        /// The settings.
        settings: SpaceSettings,
    },
    /// The kinds with no private fields (a sealed empty plaintext).
    None,
}

/// One membership entry: the header, and the envelope (kind `Membership`) that carries it, the
/// sealed [`Private`] part and the device signature.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MembershipEntry {
    /// The clear header (the same bytes as `envelope.header`).
    pub header: MembershipHeader,
    /// The envelope as signed.
    pub envelope: Envelope,
}

impl MembershipEntry {
    /// Builds and signs an entry: seals `private` under `meta_key` (`MK[header.meta_epoch]`, see
    /// `space_keys::membership_meta_key`) and signs the envelope with `device`'s signing key. The
    /// caller sets `seq` and `prev` (`prev` is the previous entry's [`Self::hash`]) and passes the
    /// device that is in the log of `header.account`.
    #[must_use]
    pub fn build(
        header: MembershipHeader,
        private: &Private,
        meta_key: &Secret32,
        device: &DeviceKeys,
        rng: &dyn Rng,
    ) -> Self {
        Self::build_with(header, private, meta_key, rng, &|signed: &[u8]| {
            kb_crypto::sign(device.signing(), EnvelopeKind::Membership.context(), signed)
        })
    }

    /// [`Self::build`] with the signing step supplied: `sign` gets the envelope bytes without the
    /// signature. The property tests of the fold use it to sign cheaply.
    #[must_use]
    pub(crate) fn build_with(
        header: MembershipHeader,
        private: &Private,
        meta_key: &Secret32,
        rng: &dyn Rng,
        sign: &dyn Fn(&[u8]) -> Signature,
    ) -> Self {
        let mut envelope = Envelope {
            kind: EnvelopeKind::Membership,
            header: header.encode(),
            nonce: Nonce24::random(rng),
            ciphertext: Vec::new(),
            signature: None,
        };
        envelope.ciphertext = kb_crypto::aead_seal(
            meta_key,
            &envelope.nonce,
            &envelope.aad(),
            &codec::encode_private(private),
        );
        let signature = sign(&envelope.encode());
        envelope.signature = Some(signature);
        Self { header, envelope }
    }

    /// The canonical bytes: the envelope's encoding (this is what the server stores, what the feed
    /// carries and what [`Self::hash`] covers).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.envelope.encode()
    }

    /// Parses an entry strictly: the envelope kind is `Membership`, its signature is present, its
    /// header decodes and re-encodes to the envelope header. Nothing is verified or opened.
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let envelope =
            Envelope::decode(bytes).map_err(|_| codec::malformed("entry", "not an envelope"))?;
        if envelope.kind != EnvelopeKind::Membership {
            return Err(codec::malformed("entry", "not a membership envelope"));
        }
        if envelope.signature.is_none() {
            return Err(codec::malformed("entry", "unsigned"));
        }
        let header = MembershipHeader::decode(&envelope.header)?;
        if header.encode() != envelope.header {
            return Err(codec::malformed("entry", "the header is not canonical"));
        }
        Ok(Self { header, envelope })
    }

    /// Reads only the clear header of an entry (what a server routes on).
    pub fn peek(bytes: &[u8]) -> Result<MembershipHeader, KeysError> {
        Self::decode(bytes).map(|entry| entry.header)
    }

    /// BLAKE3 of [`Self::encode`]: the `prev` of the next entry.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        kb_crypto::hash(&self.encode())
    }

    /// The kind.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        self.header.kind()
    }

    /// This entry's place in the log: `seq` and [`Self::hash`].
    #[must_use]
    pub fn head(&self) -> ChainHead {
        ChainHead {
            seq: self.header.seq,
            hash: self.hash(),
        }
    }

    /// Verifies the device signature over the envelope (context `envelope/membership`) with the
    /// device's signing key; a mismatch is [`KeysError::Crypto`] (`Authentication`). The caller
    /// resolves `header.device` to a key through the device log of `header.account` (the fold
    /// does).
    pub fn verify_signature(&self, key: &VerifyingKey) -> Result<(), KeysError> {
        let authentication = || KeysError::Crypto(CryptoError::Authentication);
        let signature = self
            .envelope
            .signature
            .as_ref()
            .ok_or_else(authentication)?;
        if self.envelope.kind != EnvelopeKind::Membership {
            return Err(authentication());
        }
        // The signature covers every byte of the encoding before it.
        let bytes = self.envelope.encode();
        let signed_len = bytes
            .len()
            .checked_sub(SIGNATURE_LEN)
            .ok_or_else(authentication)?;
        let signed = bytes.get(..signed_len).ok_or_else(authentication)?;
        kb_crypto::verify(key, EnvelopeKind::Membership.context(), signed, signature)?;
        Ok(())
    }

    /// Opens the private part with `MK[header.meta_epoch]`; a wrong key or tampering is
    /// [`KeysError::Crypto`] (`Authentication`); a plaintext that does not match the kind is
    /// [`KeysError::Corrupted`].
    pub fn open_private(&self, meta_key: &Secret32) -> Result<Private, KeysError> {
        let plaintext = kb_crypto::aead_open(
            meta_key,
            &self.envelope.nonce,
            &self.envelope.aad(),
            &self.envelope.ciphertext,
        )?;
        codec::decode_private(self.header.kind(), &plaintext)
    }
}

// ----- the fold ------------------------------------------------------------------------------

/// Resolves the device logs of members for the fold. The server answers from its own copy, a
/// client from `foreign_device_logs` and its own log. Logs are verified by whoever builds them
/// (`DeviceLog::from_entries` replays a log from its genesis).
pub trait LogSource {
    /// The account's verified device log, as far as the source holds it; `None` when unknown (the
    /// fold then cannot judge an entry of that account: [`FoldRule::UnknownLog`]).
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog>;
}

/// The rule an entry broke (the category; `Rule` names the row of the table in ADR-0043).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FoldRule {
    /// The bytes do not decode canonically.
    Encoding,
    /// `seq` or `prev` does not continue the chain, or the first entry is not a `Create`.
    Chain,
    /// The signature does not verify, or the device is not in the header account's log.
    Signature,
    /// The source holds no log for an account the entry names (a signer, a joiner, a member head).
    UnknownLog,
    /// A rule of ADR-0043 (signer role, target, invite state, last Owner, limits, closed, epochs,
    /// grants, commitments of the same epoch ...), named by static text without any data.
    Rule(&'static str),
    /// A `Create`, `Rekey` or `Remove` whose boxes do not cover exactly the active devices of every
    /// member as of its `member_heads`, or whose epoch does not follow.
    Completeness,
    /// A member head that goes backwards, or does not exist in the member's log.
    Heads,
}

/// Why a membership log does not fold: the index of the first entry that breaks a rule (counted
/// from the start of the entries given, or from the entries already folded for
/// [`Timeline::apply`]) and the rule. Carries no data of the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("membership entry {index} breaks a rule: {rule:?}")]
pub struct FoldError {
    /// The entry.
    pub index: usize,
    /// The rule.
    pub rule: FoldRule,
}

/// One member of a folded membership state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MemberRecord {
    /// The account.
    pub account: AccountId,
    /// The current role.
    pub role: MemberRole,
    /// The feed cursor of the `Join` (of the `Create` for the creator).
    pub joined_cursor: u64,
    /// The `at` of that entry.
    pub joined_at: i64,
}

/// Where an invite of the log stands (expiry is the server's: a client never says `Expired`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InviteRecordStatus {
    /// Redeemable (as far as the log says).
    Open,
    /// A `Join` consumed it.
    Used {
        /// The joining account.
        by: AccountId,
    },
    /// An `InviteRevoke`, or its creator lost the Owner role (removed, left or demoted).
    Revoked,
}

/// An invite of the log.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InviteRecord {
    /// The invite.
    pub invite_id: InviteId,
    /// Its public key (verifies the join proof, the signup claim and the open request).
    pub invite_pub: VerifyingKey,
    /// The role it grants.
    pub role: MemberRole,
    /// Unix ms; the server's clock decides.
    pub expires_at: i64,
    /// The account a code invite is bound to.
    pub bound_account: Option<AccountId>,
    /// The short authentication string was compared.
    pub verified: bool,
    /// The sealed bundle of a link invite.
    pub enc_bundle: Option<Vec<u8>>,
    /// The Owner who wrote it.
    pub created_by: AccountId,
    /// The `at` of the `Invite`.
    pub created_at: i64,
    /// The feed cursor of the `Invite`.
    pub cursor: u64,
    /// Where it stands.
    pub status: InviteRecordStatus,
}

impl InviteRecord {
    /// Whether the invite has expired at `now_ms`. The fold never asks: expiry is judged by the
    /// server's clock when it appends a `Join` (clients cannot read that clock and do not
    /// re-check).
    #[must_use]
    pub const fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.expires_at
    }
}

/// The state a membership log describes after its last entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MembershipState {
    /// The feed.
    pub feed: FeedId,
    /// Current members, in join order.
    pub members: Vec<MemberRecord>,
    /// Every invite the log holds (open, used, revoked), in log order.
    pub invites: Vec<InviteRecord>,
    /// The current space key epoch (0 before the `Create`).
    pub epoch: u32,
    /// The commitment of the current epoch's key.
    pub sk_commitment: [u8; 32],
    /// A `Leave` waits for a `Rekey` (a device revocation also sets it on the server, which the log
    /// does not record).
    pub pending_rekey: bool,
    /// The feed cursor of the `Close`, once the space was closed.
    pub closed: Option<u64>,
    /// The last entry folded (`None` before the `Create`).
    pub head: Option<ChainHead>,
}

impl MembershipState {
    /// The state before any entry.
    #[must_use]
    pub fn empty(feed: FeedId) -> Self {
        Self {
            feed,
            members: Vec::new(),
            invites: Vec::new(),
            epoch: 0,
            sk_commitment: [0u8; 32],
            pending_rekey: false,
            closed: None,
            head: None,
        }
    }

    /// The member's record, if it is a member.
    #[must_use]
    pub fn member(&self, account: AccountId) -> Option<&MemberRecord> {
        self.members.iter().find(|m| m.account == account)
    }

    /// The member's role.
    #[must_use]
    pub fn role_of(&self, account: AccountId) -> Option<MemberRole> {
        self.member(account).map(|m| m.role)
    }

    /// The Owners, in join order.
    #[must_use]
    pub fn owners(&self) -> Vec<AccountId> {
        self.members
            .iter()
            .filter(|m| m.role.is_owner())
            .map(|m| m.account)
            .collect()
    }

    /// The invites that are open.
    pub fn open_invites(&self) -> impl Iterator<Item = &InviteRecord> {
        self.invites
            .iter()
            .filter(|i| i.status == InviteRecordStatus::Open)
    }

    /// The invite with this id, in any state.
    #[must_use]
    pub fn invite(&self, invite_id: InviteId) -> Option<&InviteRecord> {
        self.invites.iter().find(|i| i.invite_id == invite_id)
    }

    /// Whether `key` is the space key of the current epoch, by its commitment. A `Grant` carries no
    /// commitment of its own: the recipient checks the key it opened against this one
    /// ([`verify_for_recipient`] answers the key, the timeline says what it must match).
    #[must_use]
    pub fn current_key_matches(&self, key: &SpaceKey) -> bool {
        self.head.is_some() && key.commitment(self.feed, self.epoch) == self.sk_commitment
    }
}

/// What one folded entry did, for the events the core derives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Applied {
    /// The kind.
    pub kind: EntryKind,
    /// The signing account.
    pub signer: AccountId,
    /// The account the entry is about (`RoleSet`, `Remove`, `Join`, `Grant`: the target; `Leave`
    /// and `Profile`: the signer).
    pub subject: Option<AccountId>,
    /// The subject's role before the entry, when it was a member.
    pub previous_role: Option<MemberRole>,
}

/// A membership log folded entry by entry: the final [`MembershipState`] and the means to answer
/// "who was what at feed cursor c". Kept by the core in memory and rebuilt from
/// `membership_entries`; the incremental result equals the batch result (property test 1 of
/// milestone-13.md, 1.8).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Timeline {
    state: MembershipState,
    folded: usize,
    /// The cursor of the last entry; cursors never go back along the log.
    last_cursor: u64,
    /// The highest position of each account's device log a `Create`, `Rekey` or `Remove` boxed for.
    heads: BTreeMap<AccountId, u64>,
    /// The `Grant`s in the log, by member and epoch.
    grants: BTreeSet<(AccountId, u32)>,
    /// Every role change `(cursor, account, role)` in log order; `None` is "not a member".
    role_events: Vec<(u64, AccountId, Option<MemberRole>)>,
    /// Every epoch change `(cursor, epoch)` in log order.
    epoch_events: Vec<(u64, u32)>,
}

impl Timeline {
    /// An empty timeline for `feed`.
    #[must_use]
    pub fn new(feed: FeedId) -> Self {
        Self {
            state: MembershipState::empty(feed),
            folded: 0,
            last_cursor: 0,
            heads: BTreeMap::new(),
            grants: BTreeSet::new(),
            role_events: Vec::new(),
            epoch_events: Vec::new(),
        }
    }

    /// Folds the next entry, found at feed cursor `cursor` (cursors increase along the log; equal
    /// cursors are accepted, a cursor below the previous one is [`FoldRule::Chain`]), and answers
    /// what it did. On error the timeline is unchanged and `FoldError::index` is the number of
    /// entries folded so far.
    pub fn apply(
        &mut self,
        cursor: u64,
        entry: &MembershipEntry,
        logs: &dyn LogSource,
    ) -> Result<Applied, FoldError> {
        self.apply_checked(cursor, entry, logs, &fold::REAL)
    }

    /// [`Self::apply`] with the signature checks supplied (the property tests substitute cheap ones).
    fn apply_checked(
        &mut self,
        cursor: u64,
        entry: &MembershipEntry,
        logs: &dyn LogSource,
        checks: &fold::Checks<'_>,
    ) -> Result<Applied, FoldError> {
        let fail = |rule| FoldError {
            index: self.folded,
            rule,
        };
        if self.folded > 0 && cursor < self.last_cursor {
            return Err(fail(FoldRule::Chain));
        }
        let mut work = fold::Work {
            state: self.state.clone(),
            heads: self.heads.clone(),
            grants: self.grants.clone(),
            role_events: Vec::new(),
            epoch_event: None,
        };
        let applied = fold::step(&mut work, cursor, entry, logs, checks).map_err(fail)?;

        self.state = work.state;
        self.heads = work.heads;
        self.grants = work.grants;
        for (account, role) in work.role_events {
            self.role_events.push((cursor, account, role));
        }
        if let Some(epoch) = work.epoch_event {
            self.epoch_events.push((cursor, epoch));
        }
        self.last_cursor = cursor;
        self.folded += 1;
        Ok(applied)
    }

    /// The state after the last folded entry.
    #[must_use]
    pub fn state(&self) -> &MembershipState {
        &self.state
    }

    /// How many entries were folded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.folded
    }

    /// No entry was folded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folded == 0
    }

    /// The highest position of the account's device log that a `Create`, `Rekey` or `Remove` boxed
    /// the space key for: a new `Rekey` must not name a lower one, so a device whose copy of the
    /// account's log is shorter fetches more before it mints.
    #[must_use]
    pub fn head_seq(&self, account: AccountId) -> Option<u64> {
        self.heads.get(&account).copied()
    }

    /// The account's role after every entry found at a cursor `<= cursor`; `None` when it was not a
    /// member then. A content item at cursor `c` is judged with `role_at(account, c)`.
    #[must_use]
    pub fn role_at(&self, account: AccountId, cursor: u64) -> Option<MemberRole> {
        self.role_events
            .iter()
            .rev()
            .filter(|(at, _, _)| *at <= cursor)
            .find(|(_, who, _)| *who == account)
            .and_then(|(_, _, role)| *role)
    }

    /// Whether the account was a member at `cursor`.
    #[must_use]
    pub fn is_member_at(&self, account: AccountId, cursor: u64) -> bool {
        self.role_at(account, cursor).is_some()
    }

    /// The space key epoch after every entry found at a cursor `<= cursor`; `None` before the
    /// `Create`.
    #[must_use]
    pub fn epoch_at(&self, cursor: u64) -> Option<u32> {
        self.epoch_events
            .iter()
            .rev()
            .find(|(at, _)| *at <= cursor)
            .map(|(_, epoch)| *epoch)
    }

    /// The Owners at `cursor`, in join order.
    #[must_use]
    pub fn owners_at(&self, cursor: u64) -> Vec<AccountId> {
        // Replay the role changes up to the cursor: a member keeps its place in the join order
        // through role changes, and takes a new one when it joins again.
        let mut members: Vec<(AccountId, MemberRole)> = Vec::new();
        for (at, account, role) in &self.role_events {
            if *at > cursor {
                break;
            }
            let position = members.iter().position(|(who, _)| who == account);
            match (role, position) {
                (Some(role), Some(i)) => members[i].1 = *role,
                (Some(role), None) => members.push((*account, *role)),
                (None, Some(i)) => {
                    members.remove(i);
                }
                (None, None) => {}
            }
        }
        members
            .into_iter()
            .filter(|(_, role)| role.is_owner())
            .map(|(account, _)| account)
            .collect()
    }
}

/// Folds a whole membership log: `entries` are `(feed cursor, entry)` in log order, starting at
/// the `Create`. Equals [`Timeline::apply`] over the entries one by one; the error names the index
/// in `entries` of the first entry that breaks a rule.
pub fn fold(
    feed: FeedId,
    entries: &[(u64, MembershipEntry)],
    logs: &dyn LogSource,
) -> Result<Timeline, FoldError> {
    let mut timeline = Timeline::new(feed);
    for (cursor, entry) in entries {
        timeline.apply(*cursor, entry, logs)?;
    }
    Ok(timeline)
}

// ----- what only a recipient can check -------------------------------------------------------

/// The checks of an entry that need a key (the server cannot run them): when the entry carries a
/// box for this device and epoch (`Create`, `Rekey`, `Remove` with its Rekey, `Grant` for this
/// account), the box must open (`space_keys::open_space_key_box`), the opened key must match the
/// entry's commitment (`kb_crypto::sk_commitment`), and for a `Rekey` or `Remove`, `prev_wrapped`
/// must open under the new key to the previous committed key found through `known_sks`.
///
/// Answers `Ok(Some(key))` with the new space key; `Ok(None)` when the entry holds no box for this
/// device (a `Join`, a `Grant` for another member ...). A box that does not open, a commitment
/// that does not match or a `prev_wrapped` that does not open is [`KeysError::Crypto`]: the entry
/// is not applied for this recipient and the space shows "key mismatch" (nobody can give different
/// keys to different members).
///
/// Details the signature leaves open:
///
/// - A `Grant` carries no commitment: the key it opens is compared with `known_sks(epoch)` when
///   the recipient knows that epoch's key, and the caller checks it against the timeline's
///   commitment of the current epoch ([`MembershipState::current_key_matches`]) otherwise.
/// - `known_sks` should answer only keys the recipient verified itself (opened from a box that
///   matched its commitment, or unwrapped down a verified chain). The previous key is compared by
///   commitment with the one `prev_wrapped` opens to. When the recipient does not know the
///   previous epoch's key (it joined with an older bundle), `prev_wrapped` is only required to
///   open under the new key, which authenticates it.
pub fn verify_for_recipient(
    entry: &MembershipEntry,
    device_id: DeviceId,
    device_keys: &DeviceKeys,
    known_sks: &dyn Fn(u32) -> Option<SpaceKey>,
) -> Result<Option<SpaceKey>, KeysError> {
    let feed = entry.header.feed;
    let (epoch, commitment, boxes, prev_wrapped) = match &entry.header.body {
        EntryBody::Create {
            epoch,
            sk_commitment,
            boxes,
            ..
        } => (*epoch, Some(*sk_commitment), boxes, None),
        EntryBody::Rekey(rekey) | EntryBody::Remove { rekey, .. } => (
            rekey.new_epoch,
            Some(rekey.sk_commitment),
            &rekey.boxes,
            Some(&rekey.prev_wrapped),
        ),
        EntryBody::Grant { epoch, boxes, .. } => (*epoch, None, boxes, None),
        _ => return Ok(None),
    };
    let Some(key_box) = boxes.iter().find(|b| b.device == device_id) else {
        return Ok(None);
    };
    let mismatch = || KeysError::Crypto(CryptoError::Authentication);

    let key = open_space_key_box(device_keys, feed, epoch, &key_box.bytes)?;
    let opened = key.commitment(feed, epoch);
    match commitment {
        Some(committed) => {
            if opened != committed {
                return Err(mismatch());
            }
        }
        None => {
            if let Some(known) = known_sks(epoch)
                && opened != known.commitment(feed, epoch)
            {
                return Err(mismatch());
            }
        }
    }
    if let Some(wrapped) = prev_wrapped {
        let prev_epoch = epoch.checked_sub(1).ok_or_else(mismatch)?;
        let prev = unwrap_prev(&key, wrapped, feed, prev_epoch)?;
        if let Some(known) = known_sks(prev_epoch)
            && prev.commitment(feed, prev_epoch) != known.commitment(feed, prev_epoch)
        {
            return Err(mismatch());
        }
    }
    Ok(Some(key))
}

/// The devices a `Rekey`, `Create` or `Remove` must box for: every active device of every given
/// member as of each member's current log head, and the heads they were read at. The fold's
/// completeness rule uses the same computation, so a minted entry always folds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RekeyRecipients {
    /// Each member's head, in the order of the members given.
    pub member_heads: Vec<MemberHead>,
    /// The active devices with their public keys, member by member.
    pub devices: Vec<(AccountId, DeviceId, DevicePublicKeys)>,
}

/// Plans the boxes of a Rekey: `members` are the accounts that remain after the entry; their logs
/// come from `logs`. [`KeysError::Corrupted`] when a log is unknown (the core first asks for it:
/// `sync_needed_logs`), an account is named twice or the devices exceed `MAX_BOXES_PER_ENTRY`.
///
/// The heads are each log's own head, so they are as new as the source's copies: the entry only
/// folds if no member's head goes below one a previous entry named
/// ([`Timeline::head_seq`]).
pub fn rekey_recipients(
    members: &[AccountId],
    logs: &dyn LogSource,
) -> Result<RekeyRecipients, KeysError> {
    let mut member_heads = Vec::with_capacity(members.len());
    let mut devices = Vec::new();
    let mut seen = BTreeSet::new();
    for account in members {
        if !seen.insert(*account) {
            return Err(KeysError::Corrupted(
                "a member is named twice in a rekey".to_owned(),
            ));
        }
        let log = logs.device_log(*account).ok_or_else(|| {
            KeysError::Corrupted("a member's device log is not known yet".to_owned())
        })?;
        let last = log
            .entries()
            .last()
            .ok_or_else(|| KeysError::Corrupted("a member's device log is empty".to_owned()))?;
        let seq = last.seq;
        let active = log
            .active_devices_at(seq)
            .ok_or_else(|| KeysError::Corrupted("a member's device log has no head".to_owned()))?;
        member_heads.push(MemberHead {
            account: *account,
            head: ChainHead {
                seq,
                hash: last.hash(),
            },
        });
        for (device_id, keys) in active {
            devices.push((*account, device_id, keys));
        }
        if devices.len() > MAX_BOXES_PER_ENTRY {
            return Err(KeysError::Corrupted(
                "a rekey would need more boxes than an entry may carry".to_owned(),
            ));
        }
    }
    Ok(RekeyRecipients {
        member_heads,
        devices,
    })
}
