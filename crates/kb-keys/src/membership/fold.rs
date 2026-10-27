//! The rules of the membership log, one entry at a time (ADR-0043, the table in
//! `docs/security/sharing.md` section 3). Pure and deterministic: the result depends on the entries,
//! their cursors and the device logs the [`LogSource`] holds, never on a clock or on a key.
//!
//! [`step`] works on a copy of the fold state ([`Work`]) that the caller drops when it fails, so a
//! refused entry changes nothing.

use std::collections::{BTreeMap, BTreeSet};

use kb_crypto::{EnvelopeKind, Signature, VerifyingKey};
use kb_types::sharing::{
    INVITE_MAX_DAYS, MAX_BOXES_PER_ENTRY, MAX_MEMBERS_PER_FEED, MAX_OPEN_INVITES,
};
use kb_types::{AccountId, DeviceId, FeedId, InviteId, MemberRole};

use super::rules;
use super::{
    Applied, ChainHead, EntryBody, EntryKind, FoldRule, InviteRecord, InviteRecordStatus, KeyBox,
    LogSource, MemberHead, MemberRecord, MembershipEntry, MembershipHeader, MembershipState,
};
use crate::KeysError;
use crate::invite::verify_join_proof;

/// Milliseconds in a day.
const DAY_MS: i64 = 86_400_000;

/// The two signature checks of the fold: an entry against the signing key of its device, and the
/// proof of a `Join` against the invite key. [`REAL`] is what production runs; the property tests
/// of this module substitute cheap stand-ins (the crypto of the debug builds is far too slow for
/// ten thousand cases).
pub(super) struct Checks<'a> {
    pub(super) signature: SignatureCheck<'a>,
    pub(super) join_proof: JoinProofCheck<'a>,
}

/// Verifies an entry's signature against the signing key of its device.
pub(super) type SignatureCheck<'a> =
    &'a dyn Fn(&MembershipEntry, &VerifyingKey) -> Result<(), KeysError>;

/// Verifies the proof of a `Join` against the invite key: `(key, feed, invite, account, head, proof)`.
pub(super) type JoinProofCheck<'a> = &'a dyn Fn(
    &VerifyingKey,
    FeedId,
    InviteId,
    AccountId,
    &ChainHead,
    &Signature,
) -> Result<(), KeysError>;

fn real_signature(entry: &MembershipEntry, key: &VerifyingKey) -> Result<(), KeysError> {
    entry.verify_signature(key)
}

/// The real checks.
pub(super) const REAL: Checks<'static> = Checks {
    signature: &real_signature,
    join_proof: &verify_join_proof,
};

/// The fold state an entry is applied to, and what it leaves behind. Cloned from the timeline
/// before an entry and committed after it.
pub(super) struct Work {
    pub(super) state: MembershipState,
    /// The highest log position each account was boxed for (`Create`, `Rekey`, `Remove`); heads
    /// never go backwards.
    pub(super) heads: BTreeMap<AccountId, u64>,
    /// The `Grant`s the log holds, by member and epoch (one each).
    pub(super) grants: BTreeSet<(AccountId, u32)>,
    /// The role changes of the entry, in order (`None`: not a member any more).
    pub(super) role_events: Vec<(AccountId, Option<MemberRole>)>,
    /// The epoch the entry set, when it set one.
    pub(super) epoch_event: Option<u32>,
}

/// Applies one entry found at feed cursor `cursor` to `work` (the cursor order is the caller's:
/// [`Timeline::apply`](super::Timeline::apply) checks it).
pub(super) fn step(
    work: &mut Work,
    cursor: u64,
    entry: &MembershipEntry,
    logs: &dyn LogSource,
    checks: &Checks<'_>,
) -> Result<Applied, FoldRule> {
    let header = &entry.header;
    check_encoding(entry)?;
    if header.feed != work.state.feed {
        return Err(FoldRule::Chain);
    }
    check_chain(&work.state, header)?;
    if work.state.closed.is_some() {
        return Err(FoldRule::Rule(rules::CLOSED));
    }

    // The signer: a device that is (or was) in the log of the header's account.
    let log = logs
        .device_log(header.account)
        .ok_or(FoldRule::UnknownLog)?;
    let keys = log.device_keys(header.device).ok_or(FoldRule::Signature)?;
    (checks.signature)(entry, &keys.signing).map_err(|_| FoldRule::Signature)?;

    let signer = header.account;
    let applied = match &header.body {
        EntryBody::Create {
            epoch,
            sk_commitment,
            boxes,
            member_heads,
        } => {
            if header.meta_epoch != *epoch {
                return Err(FoldRule::Rule(rules::META_EPOCH));
            }
            check_coverage(logs, &[signer], boxes, member_heads, &mut work.heads)?;
            work.state.members.push(MemberRecord {
                account: signer,
                role: MemberRole::Owner,
                joined_cursor: cursor,
                joined_at: header.at,
            });
            work.role_events.push((signer, Some(MemberRole::Owner)));
            work.state.epoch = *epoch;
            work.state.sk_commitment = *sk_commitment;
            work.epoch_event = Some(*epoch);
            Applied {
                kind: EntryKind::Create,
                signer,
                subject: None,
                previous_role: None,
            }
        }
        EntryBody::Invite {
            invite_id,
            invite_pub,
            role,
            expires_at,
            bound_account,
            verified,
            enc_bundle,
        } => {
            require_owner(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            if role.is_owner() {
                return Err(FoldRule::Rule(rules::INVITE_GRANTS_OWNER));
            }
            if expires_at.saturating_sub(header.at) > i64::from(INVITE_MAX_DAYS) * DAY_MS {
                return Err(FoldRule::Rule(rules::INVITE_EXPIRY_TOO_FAR));
            }
            if work.state.invites.iter().any(|i| i.invite_id == *invite_id) {
                return Err(FoldRule::Rule(rules::INVITE_ID_EXISTS));
            }
            let open = work
                .state
                .invites
                .iter()
                .filter(|i| i.status == InviteRecordStatus::Open && i.expires_at > header.at)
                .count();
            if open >= MAX_OPEN_INVITES {
                return Err(FoldRule::Rule(rules::TOO_MANY_OPEN_INVITES));
            }
            work.state.invites.push(InviteRecord {
                invite_id: *invite_id,
                invite_pub: *invite_pub,
                role: *role,
                expires_at: *expires_at,
                bound_account: *bound_account,
                verified: *verified,
                enc_bundle: enc_bundle.clone(),
                created_by: signer,
                created_at: header.at,
                cursor,
                status: InviteRecordStatus::Open,
            });
            plain(EntryKind::Invite, signer)
        }
        EntryBody::InviteRevoke { invite_id } => {
            require_owner(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            let invite = open_invite_mut(&mut work.state, *invite_id)?;
            invite.status = InviteRecordStatus::Revoked;
            plain(EntryKind::InviteRevoke, signer)
        }
        EntryBody::Join {
            invite_id,
            proof,
            account,
            log: joiner_head,
        } => {
            if *account != signer {
                return Err(FoldRule::Rule(rules::JOIN_ACCOUNT_MISMATCH));
            }
            if work.state.member(signer).is_some() {
                return Err(FoldRule::Rule(rules::ALREADY_MEMBER));
            }
            check_meta_epoch(&work.state, header)?;
            let invite = open_invite_mut(&mut work.state, *invite_id)?;
            if invite.bound_account.is_some_and(|bound| bound != signer) {
                return Err(FoldRule::Rule(rules::INVITE_BOUND_TO_OTHER_ACCOUNT));
            }
            (checks.join_proof)(
                &invite.invite_pub,
                header.feed,
                *invite_id,
                signer,
                joiner_head,
                proof,
            )
            .map_err(|_| FoldRule::Rule(rules::JOIN_PROOF_INVALID))?;
            // The head the proof covers is a position of the joiner's own log.
            check_head_in_log(logs, signer, joiner_head)?;
            let role = invite.role;
            invite.status = InviteRecordStatus::Used { by: signer };
            if work.state.members.len() >= MAX_MEMBERS_PER_FEED {
                return Err(FoldRule::Rule(rules::SPACE_FULL));
            }
            work.state.members.push(MemberRecord {
                account: signer,
                role,
                joined_cursor: cursor,
                joined_at: header.at,
            });
            work.role_events.push((signer, Some(role)));
            Applied {
                kind: EntryKind::Join,
                signer,
                subject: Some(signer),
                previous_role: None,
            }
        }
        EntryBody::RoleSet { target, role } => {
            require_owner(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            let current = work
                .state
                .role_of(*target)
                .ok_or(FoldRule::Rule(rules::TARGET_NOT_MEMBER))?;
            if current.is_owner() && !role.is_owner() && owner_count(&work.state) <= 1 {
                return Err(FoldRule::Rule(rules::LAST_OWNER));
            }
            if current != *role {
                set_role(work, *target, *role);
                if current.is_owner() && !role.is_owner() {
                    void_invites_of(&mut work.state, *target);
                }
            }
            Applied {
                kind: EntryKind::RoleSet,
                signer,
                subject: Some(*target),
                previous_role: Some(current),
            }
        }
        EntryBody::Remove { target, rekey } => {
            require_owner(&work.state, signer)?;
            if *target == signer {
                return Err(FoldRule::Rule(rules::REMOVE_SELF));
            }
            let previous = work
                .state
                .role_of(*target)
                .ok_or(FoldRule::Rule(rules::TARGET_NOT_MEMBER))?;
            let remaining: Vec<AccountId> = work
                .state
                .members
                .iter()
                .map(|m| m.account)
                .filter(|a| a != target)
                .collect();
            check_next_epoch(&work.state, rekey.new_epoch)?;
            if header.meta_epoch > rekey.new_epoch {
                return Err(FoldRule::Rule(rules::META_EPOCH));
            }
            check_coverage(
                logs,
                &remaining,
                &rekey.boxes,
                &rekey.member_heads,
                &mut work.heads,
            )?;
            remove_member(work, *target);
            apply_rekey(work, rekey.new_epoch, rekey.sk_commitment);
            Applied {
                kind: EntryKind::Remove,
                signer,
                subject: Some(*target),
                previous_role: Some(previous),
            }
        }
        EntryBody::Leave => {
            let role = require_member(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            if role.is_owner() && owner_count(&work.state) <= 1 {
                return Err(FoldRule::Rule(rules::LAST_OWNER));
            }
            remove_member(work, signer);
            work.state.pending_rekey = true;
            Applied {
                kind: EntryKind::Leave,
                signer,
                subject: Some(signer),
                previous_role: Some(role),
            }
        }
        EntryBody::Rekey(rekey) => {
            require_member(&work.state, signer)?;
            check_next_epoch(&work.state, rekey.new_epoch)?;
            if header.meta_epoch > rekey.new_epoch {
                return Err(FoldRule::Rule(rules::META_EPOCH));
            }
            let members: Vec<AccountId> = work.state.members.iter().map(|m| m.account).collect();
            check_coverage(
                logs,
                &members,
                &rekey.boxes,
                &rekey.member_heads,
                &mut work.heads,
            )?;
            apply_rekey(work, rekey.new_epoch, rekey.sk_commitment);
            plain(EntryKind::Rekey, signer)
        }
        EntryBody::Grant {
            member,
            epoch,
            boxes,
        } => {
            require_member(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            let previous = work
                .state
                .role_of(*member)
                .ok_or(FoldRule::Rule(rules::TARGET_NOT_MEMBER))?;
            if *epoch != work.state.epoch {
                return Err(FoldRule::Rule(rules::GRANT_NOT_CURRENT_EPOCH));
            }
            if work.grants.contains(&(*member, *epoch)) {
                return Err(FoldRule::Rule(rules::GRANT_EXISTS));
            }
            let devices: BTreeSet<DeviceId> = boxes.iter().map(|b| b.device).collect();
            if boxes.is_empty() || devices.len() != boxes.len() {
                return Err(FoldRule::Rule(rules::GRANT_BOXES_INVALID));
            }
            work.grants.insert((*member, *epoch));
            Applied {
                kind: EntryKind::Grant,
                signer,
                subject: Some(*member),
                previous_role: Some(previous),
            }
        }
        EntryBody::Profile => {
            let role = require_member(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            Applied {
                kind: EntryKind::Profile,
                signer,
                subject: Some(signer),
                previous_role: Some(role),
            }
        }
        EntryBody::Settings => {
            require_owner(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            plain(EntryKind::Settings, signer)
        }
        EntryBody::Close => {
            require_owner(&work.state, signer)?;
            check_meta_epoch(&work.state, header)?;
            work.state.closed = Some(cursor);
            plain(EntryKind::Close, signer)
        }
    };

    work.state.head = Some(ChainHead {
        seq: header.seq,
        hash: entry.hash(),
    });
    Ok(applied)
}

/// An [`Applied`] about nobody in particular.
fn plain(kind: EntryKind, signer: AccountId) -> Applied {
    Applied {
        kind,
        signer,
        subject: None,
        previous_role: None,
    }
}

// ----- shape and chain -----------------------------------------------------------------------

/// The entry is what its bytes say: an envelope of kind `Membership` with a signature, whose
/// header is the canonical encoding of the parsed header, within the box and head limits.
fn check_encoding(entry: &MembershipEntry) -> Result<(), FoldRule> {
    if entry.envelope.kind != EnvelopeKind::Membership
        || entry.envelope.signature.is_none()
        || entry.header.encode() != entry.envelope.header
    {
        return Err(FoldRule::Encoding);
    }
    let (boxes, heads) = match &entry.header.body {
        EntryBody::Create {
            boxes,
            member_heads,
            ..
        } => (boxes.len(), member_heads.len()),
        EntryBody::Remove { rekey, .. } | EntryBody::Rekey(rekey) => {
            (rekey.boxes.len(), rekey.member_heads.len())
        }
        EntryBody::Grant { boxes, .. } => (boxes.len(), 0),
        _ => (0, 0),
    };
    if boxes > MAX_BOXES_PER_ENTRY || heads > MAX_MEMBERS_PER_FEED {
        return Err(FoldRule::Encoding);
    }
    Ok(())
}

/// `seq` and `prev` continue the chain; the log starts with a `Create` and has one only there.
fn check_chain(state: &MembershipState, header: &MembershipHeader) -> Result<(), FoldRule> {
    let is_create = header.kind() == EntryKind::Create;
    match state.head {
        None => {
            if header.seq != 0 || header.prev != [0u8; 32] || !is_create {
                return Err(FoldRule::Chain);
            }
        }
        Some(head) => {
            if head.seq.checked_add(1) != Some(header.seq) || header.prev != head.hash || is_create
            {
                return Err(FoldRule::Chain);
            }
        }
    }
    Ok(())
}

/// The private part is sealed under a key the entry's signer can hold: an epoch that exists.
fn check_meta_epoch(state: &MembershipState, header: &MembershipHeader) -> Result<(), FoldRule> {
    if header.meta_epoch > state.epoch {
        return Err(FoldRule::Rule(rules::META_EPOCH));
    }
    Ok(())
}

// ----- who may do what -----------------------------------------------------------------------

fn require_member(state: &MembershipState, signer: AccountId) -> Result<MemberRole, FoldRule> {
    state
        .role_of(signer)
        .ok_or(FoldRule::Rule(rules::SIGNER_NOT_MEMBER))
}

fn require_owner(state: &MembershipState, signer: AccountId) -> Result<(), FoldRule> {
    if require_member(state, signer)?.is_owner() {
        Ok(())
    } else {
        Err(FoldRule::Rule(rules::SIGNER_NOT_OWNER))
    }
}

fn owner_count(state: &MembershipState) -> usize {
    state.members.iter().filter(|m| m.role.is_owner()).count()
}

// ----- invites -------------------------------------------------------------------------------

/// The invite, which must exist and be open.
fn open_invite_mut(
    state: &mut MembershipState,
    invite_id: InviteId,
) -> Result<&mut InviteRecord, FoldRule> {
    let invite = state
        .invites
        .iter_mut()
        .find(|i| i.invite_id == invite_id)
        .ok_or(FoldRule::Rule(rules::INVITE_UNKNOWN))?;
    match invite.status {
        InviteRecordStatus::Open => Ok(invite),
        InviteRecordStatus::Used { .. } => Err(FoldRule::Rule(rules::INVITE_USED)),
        InviteRecordStatus::Revoked => Err(FoldRule::Rule(rules::INVITE_REVOKED)),
    }
}

/// Every open invite `creator` wrote becomes revoked: an Owner who is removed, leaves or is demoted
/// takes their invites with them (ADR-0043).
fn void_invites_of(state: &mut MembershipState, creator: AccountId) {
    for invite in &mut state.invites {
        if invite.created_by == creator && invite.status == InviteRecordStatus::Open {
            invite.status = InviteRecordStatus::Revoked;
        }
    }
}

// ----- members -------------------------------------------------------------------------------

fn set_role(work: &mut Work, account: AccountId, role: MemberRole) {
    if let Some(member) = work.state.members.iter_mut().find(|m| m.account == account) {
        member.role = role;
        work.role_events.push((account, Some(role)));
    }
}

/// The member is gone (removed or left): their invites are void and a grant of theirs may be
/// issued again if they return.
fn remove_member(work: &mut Work, account: AccountId) {
    work.state.members.retain(|m| m.account != account);
    work.role_events.push((account, None));
    void_invites_of(&mut work.state, account);
    work.grants.retain(|(member, _)| *member != account);
}

// ----- epochs, boxes and heads ---------------------------------------------------------------

/// A `Rekey` (or the one inside a `Remove`) names the epoch that follows the current one.
fn check_next_epoch(state: &MembershipState, new_epoch: u32) -> Result<(), FoldRule> {
    if state.epoch.checked_add(1) == Some(new_epoch) {
        Ok(())
    } else {
        Err(FoldRule::Completeness)
    }
}

/// The new epoch is in force and nothing waits for a `Rekey` any more.
fn apply_rekey(work: &mut Work, new_epoch: u32, commitment: [u8; 32]) {
    work.state.epoch = new_epoch;
    work.state.sk_commitment = commitment;
    work.state.pending_rekey = false;
    work.epoch_event = Some(new_epoch);
}

/// A head the entry names is a position of the account's own log, as far as the source holds it:
/// a log that is shorter than the head is [`FoldRule::UnknownLog`] (fetch more and retry), a hash
/// that is not the log's own at that position is [`FoldRule::Heads`].
fn check_head_in_log(
    logs: &dyn LogSource,
    account: AccountId,
    head: &ChainHead,
) -> Result<(), FoldRule> {
    let log = logs.device_log(account).ok_or(FoldRule::UnknownLog)?;
    match log.entry_hash(head.seq) {
        None => Err(FoldRule::UnknownLog),
        Some(hash) if hash == head.hash => Ok(()),
        Some(_) => Err(FoldRule::Heads),
    }
}

/// The completeness rule of a `Create`, `Rekey` or `Remove` (ADR-0043): `member_heads` name every
/// account of `members` once, each head is a position of that member's log and never goes below a
/// head named before, and the boxes cover exactly the devices active in those logs as of those
/// heads (no device missing, none extra, none twice). `known_heads` records the heads for the next
/// entry.
fn check_coverage(
    logs: &dyn LogSource,
    members: &[AccountId],
    boxes: &[KeyBox],
    member_heads: &[MemberHead],
    known_heads: &mut BTreeMap<AccountId, u64>,
) -> Result<(), FoldRule> {
    if member_heads.len() != members.len() {
        return Err(FoldRule::Completeness);
    }
    let wanted: BTreeSet<AccountId> = members.iter().copied().collect();
    let mut seen: BTreeSet<AccountId> = BTreeSet::new();
    let mut expected: BTreeSet<DeviceId> = BTreeSet::new();
    for named in member_heads {
        if !wanted.contains(&named.account) || !seen.insert(named.account) {
            return Err(FoldRule::Completeness);
        }
        check_head_in_log(logs, named.account, &named.head)?;
        if known_heads
            .get(&named.account)
            .is_some_and(|before| named.head.seq < *before)
        {
            return Err(FoldRule::Heads);
        }
        let log = logs.device_log(named.account).ok_or(FoldRule::UnknownLog)?;
        let active = log
            .active_devices_at(named.head.seq)
            .ok_or(FoldRule::UnknownLog)?;
        for (device, _) in active {
            if !expected.insert(device) {
                return Err(FoldRule::Completeness);
            }
        }
        known_heads.insert(named.account, named.head.seq);
    }
    let mut boxed: BTreeSet<DeviceId> = BTreeSet::new();
    for key_box in boxes {
        if !boxed.insert(key_box.device) {
            return Err(FoldRule::Completeness);
        }
    }
    if boxed != expected || boxes.len() > MAX_BOXES_PER_ENTRY {
        return Err(FoldRule::Completeness);
    }
    Ok(())
}
