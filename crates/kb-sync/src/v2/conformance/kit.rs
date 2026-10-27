//! The building blocks of the v2 conformance suite and of the adversarial tests of `kb-sync`,
//! `kb-server` and `kb-net`: people (accounts with devices and verified device logs), spaces
//! (membership logs built entry by entry with the real cryptography), and sealed feed items.
//!
//! Everything is deterministic for a seed. The cryptography is the real one, so a debug build pays
//! about 8 ms per signature and 18 ms per key box: a suite builds a handful of people and entries,
//! never hundreds.

// A test kit: failing loudly is its job.
#![allow(clippy::panic)]

use std::cell::Cell;
use std::collections::BTreeMap;

use kb_crypto::{Secret32, Signature, SigningKey};
use kb_keys::invite::{
    InviteSecret, LinkBundle, invite_id, invite_signing_key, seal_link_bundle, sign_join_proof,
    sign_open, sign_signup,
};
use kb_keys::keyring::new_space_key;
use kb_keys::log::{account_id_of, encrypt_name};
use kb_keys::membership::{
    ChainHead, EntryBody, KeyBox, LogSource, MembershipEntry, MembershipHeader, Private,
    RekeyFields, rekey_recipients,
};
use kb_keys::space_keys::{
    SpaceKey, membership_meta_key, presence_key, seal_space_key_for, wrap_prev,
};
use kb_keys::{Ark, DeviceKeys, DeviceLog, DevicePublicKeys, KeyringItem, LogSigner, RecoveryKey};
use kb_types::sharing::SpaceSettings;
use kb_types::testing::SeededRng;
use kb_types::{
    AccountId, ChunkId, DeviceId, DocId, FeedId, InviteId, MemberRole, RngExt, SpaceId,
};

use crate::v2::ws::{PresenceBody, PresenceFrame, PresenceHeader, seal_presence};
use crate::v2::{
    EntryHeader, Head, KeyringHeader, SnapshotHeader, seal_entry, seal_keyring, seal_snapshot,
    sign_head,
};
use crate::{EntryBody as DocBody, SnapshotBody, sign_auth};

/// Unix ms of the fixture's first moment: shortly before the date of the milestone, so a server
/// clock set to it (`MemoryRemote::set_now_ms`, the `FixedClock` of the server tests) agrees.
pub const FIXTURE_TIME_MS: i64 = 1_790_000_000_000;

/// Milliseconds per hour.
pub const HOUR_MS: i64 = 3_600_000;
/// Milliseconds per day.
pub const DAY_MS: i64 = 86_400_000;

/// One device of a person.
#[derive(Clone)]
pub struct Dev {
    /// The device id.
    pub id: DeviceId,
    /// Its keys.
    pub keys: DeviceKeys,
    /// Its public keys (deriving them costs a scalar multiplication each time).
    pub public: DevicePublicKeys,
}

/// A person: an account with the devices it ever had and its verified device log.
#[derive(Clone)]
pub struct Person {
    /// A display name for the profile fields.
    pub name: String,
    /// The account id (derived from the genesis).
    pub account: AccountId,
    /// Every device the person ever had, in the order they were added.
    pub devices: Vec<Dev>,
    /// The device log as the person holds it now (genesis first).
    pub log: DeviceLog,
    /// The account root key (keyring entries are sealed under it).
    pub ark: Ark,
    rng: SeededRngBox,
}

impl core::fmt::Debug for Person {
    /// Names and ids only: never a key.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Person")
            .field("name", &self.name)
            .field("account", &self.account)
            .field("devices", &self.devices.len())
            .finish_non_exhaustive()
    }
}

/// A seeded generator that can be cloned with its position.
struct SeededRngBox {
    seed: u64,
    draws: Cell<u64>,
}

impl Clone for SeededRngBox {
    fn clone(&self) -> Self {
        Self {
            seed: self.seed,
            draws: Cell::new(self.draws.get()),
        }
    }
}

impl SeededRngBox {
    /// A generator for the next thing that needs randomness.
    fn next(&self) -> SeededRng {
        let n = self.draws.get();
        self.draws.set(n + 1);
        SeededRng::new(self.seed ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }
}

impl Person {
    /// A person with one device, from `seed`.
    ///
    /// # Panics
    ///
    /// When `kb-keys` refuses the genesis it is asked to build (a bug there).
    #[must_use]
    pub fn generate(seed: u64, name: &str, at_ms: i64) -> Self {
        Self::generate_with_device_id(seed, name, at_ms, None)
    }

    /// [`Self::generate`] whose first device has the given id (a device id another account uses:
    /// the server refuses it).
    ///
    /// # Panics
    ///
    /// When `kb-keys` refuses the genesis it is asked to build (a bug there).
    #[must_use]
    pub fn generate_with_device_id(
        seed: u64,
        name: &str,
        at_ms: i64,
        device_id: Option<DeviceId>,
    ) -> Self {
        let rng = SeededRng::new(seed);
        let ark = Ark::from_secret(Secret32::generate(&rng));
        let keys = DeviceKeys::generate(&rng);
        let id = device_id.unwrap_or_else(|| DeviceId::generate(&rng));
        let recovery = RecoveryKey::generate(&rng);
        let log = DeviceLog::genesis(
            id,
            &keys,
            recovery.verifying_key(),
            encrypt_name(&ark, id, "first device", &rng),
            at_ms,
        )
        .unwrap_or_else(|e| panic!("person genesis: {e}"));
        let account = account_id_of(&log.entries()[0]);
        let public = keys.public();
        Self {
            name: name.to_owned(),
            account,
            devices: vec![Dev { id, keys, public }],
            log,
            ark,
            rng: SeededRngBox {
                seed: seed ^ 0xA5A5_A5A5,
                draws: Cell::new(0),
            },
        }
    }

    /// The device log as `create_account` takes it.
    #[must_use]
    pub fn encoded_log(&self) -> Vec<Vec<u8>> {
        self.log
            .entries()
            .iter()
            .map(kb_keys::LogEntry::encode)
            .collect()
    }

    /// The encoded entries after sequence `after` (the new tail an `append_log` publishes).
    #[must_use]
    pub fn encoded_log_after(&self, after: u64) -> Vec<Vec<u8>> {
        self.log
            .entries()
            .iter()
            .filter(|entry| entry.seq > after)
            .map(kb_keys::LogEntry::encode)
            .collect()
    }

    /// The sequence number of the log's last entry.
    #[must_use]
    pub fn log_seq(&self) -> u64 {
        self.log.entries().last().map_or(0, |entry| entry.seq)
    }

    /// The head of the log: the last sequence number and its hash.
    #[must_use]
    pub fn head(&self) -> ChainHead {
        let seq = self.log_seq();
        ChainHead {
            seq,
            hash: self.log.entry_hash(seq).unwrap_or([0u8; 32]),
        }
    }

    /// The index of the first device that is active in the log.
    ///
    /// # Panics
    ///
    /// When no device is active (never, a log keeps one).
    #[must_use]
    pub fn active_device(&self) -> usize {
        let active = self
            .log
            .active_devices_at(self.log_seq())
            .unwrap_or_else(|| panic!("person log has no head"));
        self.devices
            .iter()
            .position(|dev| active.iter().any(|(id, _)| *id == dev.id))
            .unwrap_or_else(|| panic!("person has no active device"))
    }

    /// Adds a device (signed by the first active one); answers its index. The new log entry is
    /// the last of [`Self::encoded_log_after`].
    ///
    /// # Panics
    ///
    /// When `kb-keys` refuses the entry (a bug there).
    pub fn add_device(&mut self, at_ms: i64) -> usize {
        let rng = self.rng.next();
        let keys = DeviceKeys::generate(&rng);
        let id = DeviceId::generate(&rng);
        let signer = self.active_device();
        let encrypted = encrypt_name(&self.ark, id, "another device", &rng);
        let public = keys.public();
        if let Err(e) = self.log.add_device(
            id,
            public,
            encrypted,
            LogSigner::Device(&self.devices[signer].keys, self.devices[signer].id),
            at_ms,
        ) {
            panic!("person add_device: {e}");
        }
        self.devices.push(Dev { id, keys, public });
        self.devices.len() - 1
    }

    /// Revokes device `target` (by another active device).
    ///
    /// # Panics
    ///
    /// When no other device is active or `kb-keys` refuses the entry.
    pub fn revoke_device(&mut self, target: usize, at_ms: i64) {
        let active = self
            .log
            .active_devices_at(self.log_seq())
            .unwrap_or_else(|| panic!("person log has no head"));
        let target_id = self.devices[target].id;
        let by = self
            .devices
            .iter()
            .find(|dev| dev.id != target_id && active.iter().any(|(id, _)| *id == dev.id))
            .unwrap_or_else(|| panic!("person has no other active device"))
            .clone();
        if let Err(e) = self.log.revoke_device(target_id, (&by.keys, by.id), at_ms) {
            panic!("person revoke_device: {e}");
        }
    }

    /// Signs authentication messages as device `device`.
    pub fn signer(&self, device: usize) -> impl Fn(&[u8]) -> Signature + '_ {
        move |message: &[u8]| sign_auth(message, self.devices[device].keys.signing())
    }

    /// The signing key of device `device`.
    #[must_use]
    pub fn signing_key(&self, device: usize) -> &SigningKey {
        self.devices[device].keys.signing()
    }

    /// The id of device `device`.
    #[must_use]
    pub fn device_id(&self, device: usize) -> DeviceId {
        self.devices[device].id
    }
}

/// The device logs of some people, as the fold reads them.
pub struct World<'a>(pub &'a [&'a Person]);

impl LogSource for World<'_> {
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog> {
        self.0
            .iter()
            .find(|person| person.account == account)
            .map(|person| &person.log)
    }
}

/// A shared space as its members build its membership log, entry by entry with the real
/// cryptography. It tracks the head of the log it has seen accepted ([`Space::accept`]), the space
/// key of every epoch and the invite secrets it made. Builders return entries; posting them is
/// the caller's.
pub struct Space {
    /// The feed.
    pub feed: FeedId,
    /// The space key of each epoch the builder knows.
    pub keys: BTreeMap<u32, SpaceKey>,
    /// The epoch in force as far as accepted entries say.
    pub epoch: u32,
    /// The head of the accepted log; `None` before the `Create`.
    pub head: Option<ChainHead>,
    /// Secrets of the invites made, by id.
    pub secrets: BTreeMap<InviteId, InviteSecret>,
    /// The `at` of the entries it builds.
    pub now: i64,
    name: String,
    rng: SeededRng,
}

impl Space {
    /// A space with a random feed id drawn from `seed`.
    #[must_use]
    pub fn new(seed: u64, name: &str, now: i64) -> Self {
        let rng = SeededRng::new(seed);
        Self {
            feed: FeedId::generate(&rng),
            keys: BTreeMap::new(),
            epoch: 0,
            head: None,
            secrets: BTreeMap::new(),
            now,
            name: name.to_owned(),
            rng,
        }
    }

    /// A random 16-byte feed-like id (for unknown feeds).
    #[must_use]
    pub fn random_feed(&self) -> FeedId {
        FeedId::generate(&self.rng)
    }

    /// Records that the server accepted `entry`: the head moves, and a `Rekey` or `Remove` brings
    /// the new epoch into force.
    pub fn accept(&mut self, entry: &MembershipEntry) {
        self.head = Some(entry.head());
        match &entry.header.body {
            EntryBody::Rekey(rekey) | EntryBody::Remove { rekey, .. } => {
                self.epoch = rekey.new_epoch;
            }
            EntryBody::Create { epoch, .. } => self.epoch = *epoch,
            _ => {}
        }
    }

    /// The `(seq, prev)` of the next entry.
    #[must_use]
    pub fn next_position(&self) -> (u64, [u8; 32]) {
        self.head.map_or((0, [0u8; 32]), |h| (h.seq + 1, h.hash))
    }

    /// The key an entry's private part is sealed under (a stand-in for an epoch nobody holds).
    fn meta_key(&self, meta_epoch: u32) -> Secret32 {
        self.keys.get(&meta_epoch).map_or_else(
            || Secret32::from_bytes([0xee; 32]),
            |sk| membership_meta_key(sk, self.feed),
        )
    }

    /// An entry of any shape signed by `who`'s device `dev` at an explicit position.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn entry_at(
        &self,
        who: &Person,
        dev: usize,
        seq: u64,
        prev: [u8; 32],
        meta_epoch: u32,
        body: EntryBody,
        private: &Private,
    ) -> MembershipEntry {
        let header = MembershipHeader {
            feed: self.feed,
            seq,
            prev,
            account: who.account,
            device: who.devices[dev].id,
            at: self.now,
            meta_epoch,
            body,
        };
        MembershipEntry::build(
            header,
            private,
            &self.meta_key(meta_epoch),
            &who.devices[dev].keys,
            &self.rng,
        )
    }

    /// An entry at the next position of the accepted log, signed by `who`'s first active device.
    #[must_use]
    pub fn entry(&self, who: &Person, body: EntryBody, private: &Private) -> MembershipEntry {
        let (seq, prev) = self.next_position();
        self.entry_at(
            who,
            who.active_device(),
            seq,
            prev,
            self.epoch,
            body,
            private,
        )
    }

    /// The boxes and member heads of a key `sk` of `epoch` for every active device of `members`
    /// (the logs they hold now).
    ///
    /// # Panics
    ///
    /// When a member's log is empty or the boxes would exceed the limit.
    #[must_use]
    pub fn boxes_for(
        &self,
        sk: &SpaceKey,
        epoch: u32,
        members: &[&Person],
    ) -> (Vec<KeyBox>, Vec<kb_keys::membership::MemberHead>) {
        let accounts: Vec<AccountId> = members.iter().map(|m| m.account).collect();
        let plan = rekey_recipients(&accounts, &World(members))
            .unwrap_or_else(|e| panic!("rekey recipients: {e}"));
        let boxes = plan
            .devices
            .iter()
            .map(|(_, device, keys)| KeyBox {
                device: *device,
                bytes: seal_space_key_for(sk, self.feed, epoch, keys, &self.rng),
            })
            .collect();
        (boxes, plan.member_heads)
    }

    /// The fields of a `Rekey` to the next epoch for `members` with a fresh key (kept in
    /// [`Self::keys`]).
    ///
    /// # Panics
    ///
    /// When the builder does not hold the current key.
    pub fn rekey_fields(&mut self, members: &[&Person]) -> RekeyFields {
        let new_epoch = self.epoch + 1;
        let sk = SpaceKey::generate(&self.rng);
        let (boxes, member_heads) = self.boxes_for(&sk, new_epoch, members);
        let previous = self
            .keys
            .get(&self.epoch)
            .unwrap_or_else(|| panic!("the builder holds no key for the current epoch"));
        let prev_wrapped = wrap_prev(previous, &sk, self.feed, self.epoch, &self.rng);
        let fields = RekeyFields {
            new_epoch,
            sk_commitment: sk.commitment(self.feed, new_epoch),
            prev_wrapped,
            boxes,
            member_heads,
        };
        self.keys.insert(new_epoch, sk);
        fields
    }

    /// A `Create` by `creator` at epoch `epoch` with a fresh key.
    pub fn create(&mut self, creator: &Person, epoch: u32) -> MembershipEntry {
        let sk = SpaceKey::generate(&self.rng);
        let (boxes, member_heads) = self.boxes_for(&sk, epoch, &[creator]);
        let sk_commitment = sk.commitment(self.feed, epoch);
        self.keys.insert(epoch, sk);
        let (seq, prev) = self.next_position();
        self.entry_at(
            creator,
            creator.active_device(),
            seq,
            prev,
            epoch,
            EntryBody::Create {
                epoch,
                sk_commitment,
                boxes,
                member_heads,
            },
            &Private::Create {
                space_name: self.name.clone(),
                settings: SpaceSettings::default(),
                profile_name: creator.name.clone(),
            },
        )
    }

    /// The `Invite` of a new secret by `owner`: a link invite (with its sealed bundle) when
    /// `bound` is `None`, a code invite bound to the account otherwise. `expires_at` is absolute
    /// (Unix ms).
    ///
    /// # Panics
    ///
    /// When the builder does not hold the current key (link invites).
    pub fn invite(
        &mut self,
        owner: &Person,
        role: MemberRole,
        expires_at: i64,
        bound: Option<AccountId>,
    ) -> (MembershipEntry, InviteSecret) {
        let secret = InviteSecret::generate(&self.rng);
        let entry = self.invite_with(owner, &secret, role, expires_at, bound);
        (entry, secret)
    }

    /// [`Self::invite`] for a secret the caller made.
    #[must_use]
    pub fn invite_with(
        &mut self,
        owner: &Person,
        secret: &InviteSecret,
        role: MemberRole,
        expires_at: i64,
        bound: Option<AccountId>,
    ) -> MembershipEntry {
        let id = invite_id(secret);
        self.secrets.insert(id, secret.clone());
        let enc_bundle = if bound.is_none() {
            let space_key = self
                .keys
                .get(&self.epoch)
                .cloned()
                .unwrap_or_else(|| panic!("the builder holds no key for the current epoch"));
            Some(seal_link_bundle(
                &LinkBundle {
                    epoch: self.epoch,
                    space_key,
                    space_name: self.name.clone(),
                    inviter_name: owner.name.clone(),
                },
                secret,
                &self.rng,
            ))
        } else {
            None
        };
        self.entry(
            owner,
            EntryBody::Invite {
                invite_id: id,
                invite_pub: invite_signing_key(secret).verifying_key(),
                role,
                expires_at,
                bound_account: bound,
                verified: bound.is_some(),
                enc_bundle,
            },
            &Private::None,
        )
    }

    /// An `InviteRevoke` by `owner`.
    #[must_use]
    pub fn invite_revoke(&self, owner: &Person, id: InviteId) -> MembershipEntry {
        self.entry(
            owner,
            EntryBody::InviteRevoke { invite_id: id },
            &Private::None,
        )
    }

    /// A `Join` by `joiner` with the invite's secret.
    #[must_use]
    pub fn join(&self, joiner: &Person, secret: &InviteSecret) -> MembershipEntry {
        self.join_forged(joiner, invite_id(secret), secret)
    }

    /// A `Join` of the invite `id` whose proof is made with another secret (a forgery).
    #[must_use]
    pub fn join_forged(
        &self,
        joiner: &Person,
        id: InviteId,
        proof_secret: &InviteSecret,
    ) -> MembershipEntry {
        let head = joiner.head();
        let proof = sign_join_proof(
            &invite_signing_key(proof_secret),
            self.feed,
            id,
            joiner.account,
            &head,
        );
        self.entry(
            joiner,
            EntryBody::Join {
                invite_id: id,
                proof,
                account: joiner.account,
                log: head,
            },
            &Private::Join {
                profile_name: joiner.name.clone(),
            },
        )
    }

    /// A `RoleSet` by `owner`.
    #[must_use]
    pub fn role_set(&self, owner: &Person, target: AccountId, role: MemberRole) -> MembershipEntry {
        self.entry(owner, EntryBody::RoleSet { target, role }, &Private::None)
    }

    /// A `Remove` of `target` by `owner` with the Rekey for `remaining` (the members that stay).
    pub fn remove(
        &mut self,
        owner: &Person,
        target: AccountId,
        remaining: &[&Person],
    ) -> MembershipEntry {
        let rekey = self.rekey_fields(remaining);
        self.entry(owner, EntryBody::Remove { target, rekey }, &Private::None)
    }

    /// A `Leave` by `member`.
    #[must_use]
    pub fn leave(&self, member: &Person) -> MembershipEntry {
        self.entry(member, EntryBody::Leave, &Private::None)
    }

    /// A `Rekey` by `by` for `members` (every current member, with the logs the caller holds).
    pub fn rekey(&mut self, by: &Person, members: &[&Person]) -> MembershipEntry {
        let fields = self.rekey_fields(members);
        self.entry(by, EntryBody::Rekey(fields), &Private::None)
    }

    /// A `Profile` by `member`.
    #[must_use]
    pub fn profile(&self, member: &Person, name: &str) -> MembershipEntry {
        self.entry(
            member,
            EntryBody::Profile,
            &Private::Profile {
                display_name: name.to_owned(),
            },
        )
    }

    /// A `Settings` by `owner`.
    #[must_use]
    pub fn settings(&self, owner: &Person, settings: SpaceSettings) -> MembershipEntry {
        self.entry(owner, EntryBody::Settings, &Private::Settings { settings })
    }

    /// A `Close` by `owner`.
    #[must_use]
    pub fn close(&self, owner: &Person) -> MembershipEntry {
        self.entry(owner, EntryBody::Close, &Private::None)
    }

    /// A `Grant` by `by` of the current key to `member`'s devices.
    ///
    /// # Panics
    ///
    /// When the builder does not hold the current key.
    #[must_use]
    pub fn grant(&self, by: &Person, member: &Person) -> MembershipEntry {
        let sk = self
            .keys
            .get(&self.epoch)
            .unwrap_or_else(|| panic!("the builder holds no key for the current epoch"));
        let (boxes, _) = self.boxes_for(sk, self.epoch, &[member]);
        self.entry(
            by,
            EntryBody::Grant {
                member: member.account,
                epoch: self.epoch,
                boxes,
            },
            &Private::None,
        )
    }

    /// A sealed document entry of `doc` by `who`'s device `dev` (the feed and epoch are the
    /// space's; `key_epoch` is the document key epoch the header claims).
    #[must_use]
    pub fn doc_entry(
        &self,
        who: &Person,
        dev: usize,
        doc: DocId,
        device_seq: u64,
        key_epoch: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        sealed_entry(
            self.feed, who, dev, doc, device_seq, key_epoch, payload, &self.rng,
        )
    }

    /// A sealed keyring entry of the shared feed by `who`'s device `dev` (sealed under the space
    /// keyring key of `key_epoch`, one fresh space key record inside).
    #[must_use]
    pub fn keyring_entry(
        &self,
        who: &Person,
        dev: usize,
        device_seq: u64,
        key_epoch: u32,
    ) -> Vec<u8> {
        let (_, record) = new_space_key(
            &who.ark,
            SpaceId::generate(&self.rng),
            0,
            &who.devices[dev].keys,
            who.devices[dev].id,
            self.now,
            &self.rng,
        );
        let key = self.keys.get(&key_epoch).map_or_else(
            || Secret32::from_bytes([0xdd; 32]),
            |sk| kb_keys::space_keys::space_keyring_key(sk, self.feed),
        );
        seal_keyring(
            &KeyringHeader {
                feed: self.feed,
                account_id: who.account,
                device_id: who.devices[dev].id,
                device_seq,
                key_epoch,
            },
            &[KeyringItem::Record(record)],
            &key,
            who.devices[dev].keys.signing(),
            &self.rng,
        )
    }

    /// A sealed snapshot of `doc` by `who`'s device `dev` covering `covers`.
    #[must_use]
    pub fn doc_snapshot(
        &self,
        who: &Person,
        dev: usize,
        doc: DocId,
        covers: u64,
        key_epoch: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        sealed_snapshot(
            self.feed, who, dev, doc, covers, key_epoch, payload, &self.rng,
        )
    }

    /// The header of a presence frame of `who`'s device `dev` in this feed.
    #[must_use]
    pub fn presence_header(
        &self,
        who: &Person,
        dev: usize,
        epoch: u32,
        counter: u64,
    ) -> PresenceHeader {
        PresenceHeader {
            feed: self.feed,
            account_id: who.account,
            device_id: who.devices[dev].id,
            epoch,
            counter,
        }
    }

    /// A presence frame of `who`'s device `dev` for this feed (its page `doc`, `update` as the
    /// editor's bytes), sealed under the presence key of `epoch` (a stand-in key when the builder holds
    /// none for it) and signed by the device. The envelope fits a frame while `update` is at most
    /// `PRESENCE_UPDATE_MAX_BYTES`.
    #[must_use]
    pub fn presence_frame(
        &self,
        who: &Person,
        dev: usize,
        epoch: u32,
        counter: u64,
        update: &[u8],
    ) -> PresenceFrame {
        self.presence_frame_with(
            &self.presence_header(who, dev, epoch, counter),
            who,
            dev,
            update,
        )
    }

    /// [`Self::presence_frame`] with an explicit header (a forged account or feed), still signed by
    /// `who`'s device `dev`.
    #[must_use]
    pub fn presence_frame_with(
        &self,
        header: &PresenceHeader,
        who: &Person,
        dev: usize,
        update: &[u8],
    ) -> PresenceFrame {
        let key = self.keys.get(&header.epoch).map_or_else(
            || Secret32::from_bytes([0xcc; 32]),
            |sk| presence_key(sk, self.feed),
        );
        let body = PresenceBody {
            doc_id: DocId::from_bytes([0x0d; 16]),
            state: kb_types::PresenceState::Editing,
            update: update.to_vec(),
        };
        PresenceFrame {
            feed: header.feed,
            envelope: seal_presence(
                header,
                &body,
                &key,
                who.devices[dev].keys.signing(),
                &self.rng,
            ),
        }
    }

    /// A signed head of `who`'s device `dev` in this feed.
    #[must_use]
    pub fn head_of(
        &self,
        who: &Person,
        dev: usize,
        cursor: u64,
        chain_hash: [u8; 32],
        server_epoch: [u8; 16],
    ) -> Vec<u8> {
        signed_head(
            self.feed,
            who,
            dev,
            cursor,
            chain_hash,
            server_epoch,
            self.now,
        )
    }

    /// `len` random bytes and their chunk id.
    #[must_use]
    pub fn random_chunk(&self, len: usize) -> (ChunkId, Vec<u8>) {
        random_chunk(&self.rng, len)
    }

    /// The signup claim of the invite's secret for `account` on a server of `server_epoch`.
    #[must_use]
    pub fn signup_sig(
        secret: &InviteSecret,
        account: AccountId,
        server_epoch: [u8; 16],
    ) -> Signature {
        sign_signup(&invite_signing_key(secret), account, server_epoch)
    }

    /// The open request's signature of the invite's secret on a server of `server_epoch`.
    #[must_use]
    pub fn open_sig(secret: &InviteSecret, server_epoch: [u8; 16]) -> Signature {
        sign_open(&invite_signing_key(secret), invite_id(secret), server_epoch)
    }
}

/// A sealed document entry in `feed` (the body is one update of `payload`).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn sealed_entry(
    feed: FeedId,
    who: &Person,
    dev: usize,
    doc: DocId,
    device_seq: u64,
    key_epoch: u32,
    payload: &[u8],
    rng: &SeededRng,
) -> Vec<u8> {
    let header = EntryHeader {
        feed,
        account_id: who.account,
        doc_id: doc,
        device_id: who.devices[dev].id,
        device_seq,
        key_epoch,
    };
    let body = DocBody {
        engine: 1,
        format_version: 1,
        min_reader_version: 1,
        doc_kind: 2,
        updates: vec![payload.to_vec()],
        version_vector: device_seq.to_le_bytes().to_vec(),
    };
    seal_entry(
        &header,
        &body,
        &Secret32::from_bytes([0x42; 32]),
        who.devices[dev].keys.signing(),
        rng,
    )
}

/// A sealed snapshot in `feed`.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn sealed_snapshot(
    feed: FeedId,
    who: &Person,
    dev: usize,
    doc: DocId,
    covers: u64,
    key_epoch: u32,
    payload: &[u8],
    rng: &SeededRng,
) -> Vec<u8> {
    let header = SnapshotHeader {
        feed,
        account_id: who.account,
        doc_id: doc,
        device_id: who.devices[dev].id,
        covers_cursor: covers,
        key_epoch,
    };
    let body = SnapshotBody {
        engine: 1,
        format_version: 1,
        min_reader_version: 1,
        doc_kind: 2,
        snapshot: payload.to_vec(),
        version_vector: covers.to_le_bytes().to_vec(),
    };
    seal_snapshot(
        &header,
        &body,
        &Secret32::from_bytes([0x42; 32]),
        who.devices[dev].keys.signing(),
        rng,
    )
}

/// A signed head in `feed`.
#[must_use]
pub fn signed_head(
    feed: FeedId,
    who: &Person,
    dev: usize,
    cursor: u64,
    chain_hash: [u8; 32],
    server_epoch: [u8; 16],
    at_ms: i64,
) -> Vec<u8> {
    sign_head(
        &Head {
            feed,
            account_id: who.account,
            device_id: who.devices[dev].id,
            cursor,
            chain_hash,
            server_epoch,
            at_ms,
        },
        who.devices[dev].keys.signing(),
    )
}

/// `len` random bytes and their chunk id.
#[must_use]
pub fn random_chunk(rng: &SeededRng, len: usize) -> (ChunkId, Vec<u8>) {
    let mut bytes = vec![0u8; len];
    kb_types::Rng::fill_bytes(rng, &mut bytes);
    (ChunkId::from_bytes(kb_crypto::hash(&bytes)), bytes)
}

/// A random document id.
#[must_use]
pub fn random_doc(rng: &SeededRng) -> DocId {
    DocId::from_bytes(rng.random_array::<16>())
}
