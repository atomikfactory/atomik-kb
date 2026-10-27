//! A world for testing the membership fold: people with devices and device logs, the space keys, a
//! sequencer that plays the server's compare-and-swap, and builders for every kind of entry.
//!
//! Two modes. The **real** mode signs, seals and verifies with the real cryptography (the unit
//! tests). The **fast** mode (the property tests) swaps the three expensive things for stand-ins
//! that keep every rule of the fold in play: signatures become a keyed checksum that a flipped
//! byte breaks, boxes become opaque bytes (the fold never opens one) and the device logs come from
//! a template built once. The debug build of the curve crates takes about 8 ms per signature and
//! 18 ms per box, which would make ten thousand cases take hours.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use kb_crypto::{CryptoError, Secret32, Signature, SigningKey, VerifyingKey, WrappedKey};
use kb_types::sharing::SpaceSettings;
use kb_types::testing::SeededRng;
use kb_types::{AccountId, DeviceId, FeedId, InviteId, MemberRole};

use super::{
    Applied, ChainHead, EntryBody, FoldError, KeyBox, LogSource, MemberHead, MembershipEntry,
    MembershipHeader, Private, RekeyFields, Timeline, rekey_recipients,
};
use crate::KeysError;
use crate::device::DeviceKeys;
use crate::invite::{
    InviteSecret, invite_id, invite_signing_key, sign_join_proof, verify_join_proof,
};
use crate::log::{DeviceLog, LogSigner, account_id_of};
use crate::space_keys::{SpaceKey, membership_meta_key, seal_space_key_for, wrap_prev};

pub(super) const T0: i64 = 1_760_000_000_000;
pub(super) const DAY: i64 = 86_400_000;

/// `(person, device)`: who signs.
pub(super) type Who = (usize, usize);

/// One device of a person. The public keys are kept: deriving them costs a scalar multiplication
/// each time (8 ms in the debug build).
#[derive(Clone)]
pub(super) struct Dev {
    pub id: DeviceId,
    pub keys: DeviceKeys,
    pub public: crate::DevicePublicKeys,
}

impl Dev {
    pub fn new(id: DeviceId, keys: DeviceKeys) -> Self {
        let public = keys.public();
        Self { id, keys, public }
    }
}

/// A person: an account with the devices it ever had and the log after each change (the source
/// serves `versions[version]`, which stands for "how much of the log has been fetched").
#[derive(Clone)]
pub(super) struct Person {
    pub account: AccountId,
    pub devices: Vec<Dev>,
    pub versions: Vec<DeviceLog>,
    pub version: usize,
}

impl Person {
    /// A new account with one device.
    pub fn new(seed: u64) -> Self {
        let rng = SeededRng::new(seed);
        let first = Dev::new(DeviceId::generate(&rng), DeviceKeys::generate(&rng));
        let kit = SigningKey::from_seed(&Secret32::generate(&rng));
        let log = DeviceLog::genesis(first.id, &first.keys, kit.verifying_key(), vec![1; 12], T0)
            .unwrap();
        Self {
            account: account_id_of(&log.entries()[0]),
            devices: vec![first],
            versions: vec![log],
            version: 0,
        }
    }

    /// The log as the source serves it.
    pub fn log(&self) -> &DeviceLog {
        &self.versions[self.version]
    }

    /// The whole log.
    pub fn latest(&self) -> &DeviceLog {
        self.versions.last().unwrap()
    }

    /// The first device that is not revoked in the latest log.
    pub fn active_device(&self) -> usize {
        let log = self.latest();
        let seq = u64::try_from(log.entries().len() - 1).unwrap();
        let active = log.active_devices_at(seq).unwrap();
        self.devices
            .iter()
            .position(|d| active.iter().any(|(id, _)| *id == d.id))
            .unwrap()
    }

    /// Adds a device (signed by an active one); the source serves it at once.
    pub fn add_device(&mut self, seed: u64) -> usize {
        let rng = SeededRng::new(seed);
        let new = Dev::new(DeviceId::generate(&rng), DeviceKeys::generate(&rng));
        let signer = self.active_device();
        let mut log = self.latest().clone();
        log.add_device(
            new.id,
            new.public,
            vec![2; 12],
            LogSigner::Device(&self.devices[signer].keys, self.devices[signer].id),
            T0 + 10,
        )
        .unwrap();
        self.devices.push(new);
        self.versions.push(log);
        self.version = self.versions.len() - 1;
        self.devices.len() - 1
    }

    /// Revokes a device (by another active one); the source serves it at once.
    pub fn revoke_device(&mut self, target: usize) {
        let target_id = self.devices[target].id;
        let log = self.latest();
        let seq = u64::try_from(log.entries().len() - 1).unwrap();
        let active = log.active_devices_at(seq).unwrap();
        let by = self
            .devices
            .iter()
            .find(|d| d.id != target_id && active.iter().any(|(id, _)| *id == d.id))
            .unwrap()
            .clone();
        let mut log = log.clone();
        log.revoke_device(target_id, (&by.keys, by.id), T0 + 20)
            .unwrap();
        self.versions.push(log);
        self.version = self.versions.len() - 1;
    }

    /// The head of the log as the source serves it.
    pub fn head(&self) -> ChainHead {
        let log = self.log();
        let seq = u64::try_from(log.entries().len() - 1).unwrap();
        ChainHead {
            seq,
            hash: log.entry_hash(seq).unwrap(),
        }
    }

    /// The devices active after entry `seq` of the log, worked out with the log's own verifier
    /// (not with `active_devices_at`).
    pub fn active_by_verifier(&self, seq: u64) -> BTreeSet<DeviceId> {
        let entries = self.latest().entries()[..=usize::try_from(seq).unwrap()].to_vec();
        DeviceLog::from_entries(entries)
            .unwrap()
            .verify()
            .unwrap()
            .devices
            .iter()
            .filter(|d| d.revoked_at.is_none())
            .map(|d| d.device_id)
            .collect()
    }
}

/// The people the property tests share, built once: five accounts, each with the log versions
/// `Genesis(d0)`, `+d1`, `+d2`, `-d0`, `+d3`, `-d2` (so the active devices change at every step).
fn template() -> &'static Vec<Person> {
    static TEMPLATE: OnceLock<Vec<Person>> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        (0..5u64)
            .map(|i| {
                let mut p = Person::new(100 + i);
                p.add_device(200 + i);
                p.add_device(300 + i);
                p.revoke_device(0);
                p.add_device(400 + i);
                p.revoke_device(2);
                p.version = 0;
                p
            })
            .collect()
    })
}

/// The devices active at each version of each template person, by the log's verifier.
fn template_active() -> &'static Vec<Vec<BTreeSet<DeviceId>>> {
    static ACTIVE: OnceLock<Vec<Vec<BTreeSet<DeviceId>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| {
        template()
            .iter()
            .map(|p| {
                (0..p.versions.len())
                    .map(|v| p.active_by_verifier(u64::try_from(v).unwrap()))
                    .collect()
            })
            .collect()
    })
}

/// The people of a space.
#[derive(Clone)]
pub(super) struct World {
    pub people: Vec<Person>,
}

impl LogSource for World {
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog> {
        self.people
            .iter()
            .find(|p| p.account == account)
            .map(Person::log)
    }
}

/// A source that holds nothing.
pub(super) struct NoLogs;

impl LogSource for NoLogs {
    fn device_log(&self, _: AccountId) -> Option<&DeviceLog> {
        None
    }
}

/// A source with an explicit list of logs.
pub(super) struct Logs(pub Vec<(AccountId, DeviceLog)>);

impl LogSource for Logs {
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog> {
        self.0.iter().find(|(a, _)| *a == account).map(|(_, l)| l)
    }
}

/// The stand-in signature of the fast mode: a 64-byte checksum of the signer's public key and the
/// signed bytes (FNV-1a with four seeds). Any changed byte changes it.
pub(super) fn fake_signature(key: &VerifyingKey, signed: &[u8]) -> Signature {
    let mut out = [0u8; 64];
    for (lane, chunk) in out.chunks_mut(8).enumerate() {
        let mut state: u64 = 0xcbf2_9ce4_8422_2325 ^ (u64::try_from(lane).unwrap() * 0x9e37_79b9);
        for byte in key.to_bytes().iter().chain(signed.iter()) {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    Signature::from_bytes(&out).unwrap()
}

/// The fast mode's signature check.
pub(super) fn fake_verify(entry: &MembershipEntry, key: &VerifyingKey) -> Result<(), KeysError> {
    let bytes = entry.encode();
    let signed = &bytes[..bytes.len().saturating_sub(64)];
    if entry.envelope.signature == Some(fake_signature(key, signed)) {
        Ok(())
    } else {
        Err(KeysError::Crypto(CryptoError::Authentication))
    }
}

/// The stand-in for a join proof: a checksum of the invite key and the proven fields.
pub(super) fn fake_proof(
    pubkey: &VerifyingKey,
    feed: FeedId,
    id: InviteId,
    account: AccountId,
    head: &ChainHead,
) -> Signature {
    let mut message = pubkey.to_bytes().to_vec();
    message.extend_from_slice(feed.as_bytes());
    message.extend_from_slice(id.as_bytes());
    message.extend_from_slice(account.as_bytes());
    message.extend_from_slice(&head.seq.to_le_bytes());
    message.extend_from_slice(&head.hash);
    fake_signature(pubkey, &message)
}

/// The fast mode's join proof check.
pub(super) fn fake_proof_verify(
    pubkey: &VerifyingKey,
    feed: FeedId,
    id: InviteId,
    account: AccountId,
    head: &ChainHead,
    proof: &Signature,
) -> Result<(), KeysError> {
    if *proof == fake_proof(pubkey, feed, id, account, head) {
        Ok(())
    } else {
        Err(KeysError::Crypto(CryptoError::Authentication))
    }
}

/// The checks of the fast mode.
pub(super) const FAST: super::fold::Checks<'static> = super::fold::Checks {
    signature: &fake_verify,
    join_proof: &fake_proof_verify,
};

/// A valid Ed25519 key that belongs to no invite (RFC 8032 section 7.1, test 1).
fn dummy_pub() -> VerifyingKey {
    VerifyingKey::from_bytes(&[
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ])
    .unwrap()
}

/// Invite secrets with their keys, derived once: the fast mode draws its invites from here
/// instead of deriving an Ed25519 key per invite (8 ms in the debug build).
fn invite_pool() -> &'static Vec<(InviteSecret, VerifyingKey)> {
    static POOL: OnceLock<Vec<(InviteSecret, VerifyingKey)>> = OnceLock::new();
    POOL.get_or_init(|| {
        (0..160u64)
            .map(|i| {
                let secret = InviteSecret::generate(&SeededRng::new(0x1117_0000 + i));
                let key = invite_signing_key(&secret).verifying_key();
                (secret, key)
            })
            .collect()
    })
}

fn pool_pub(secret: &InviteSecret) -> Option<VerifyingKey> {
    invite_pool()
        .iter()
        .find(|(s, _)| s == secret)
        .map(|(_, key)| *key)
}

/// A space and the sequencer in front of it.
pub(super) struct Space {
    pub feed: FeedId,
    pub world: World,
    pub fast: bool,
    pub entries: Vec<(u64, MembershipEntry)>,
    pub timeline: Timeline,
    /// The space key of each epoch.
    pub keys: BTreeMap<u32, SpaceKey>,
    pub secrets: BTreeMap<InviteId, InviteSecret>,
    /// The feed cursor of the last accepted entry.
    pub cursor: u64,
    /// How far the feed cursor jumps between two entries (content items in between).
    pub gap: u64,
    /// The `at` of the next entry.
    pub clock: i64,
    pub rng: SeededRng,
    /// Secrets drawn so far, for fresh ones.
    draws: u64,
    /// Entries built so far (each gets its own nonce).
    built: std::cell::Cell<u64>,
}

impl Space {
    /// A space with `people` accounts: real keys and logs, or the shared template when `fast`.
    pub fn new(people: usize, fast: bool, seed: u64) -> Self {
        let people = if fast {
            template()[..people].to_vec()
        } else {
            (0..people)
                .map(|i| Person::new(seed * 1000 + u64::try_from(i).unwrap()))
                .collect()
        };
        let rng = SeededRng::new(seed);
        let feed = FeedId::generate(&rng);
        Self {
            feed,
            world: World { people },
            fast,
            entries: Vec::new(),
            timeline: Timeline::new(feed),
            keys: BTreeMap::new(),
            secrets: BTreeMap::new(),
            cursor: 0,
            gap: 1,
            clock: T0 + 1_000,
            rng,
            draws: 0,
            built: std::cell::Cell::new(0),
        }
    }

    /// A fast space with `people` fresh accounts that only have their genesis device (for the
    /// limits that need many members).
    pub fn large(people: usize, seed: u64) -> Self {
        let mut space = Self::new(1, true, seed);
        space.world.people = (0..people)
            .map(|i| Person::new(seed * 10_000 + u64::try_from(i).unwrap()))
            .collect();
        space
    }

    pub fn account(&self, person: usize) -> AccountId {
        self.world.people[person].account
    }

    pub fn state(&self) -> &super::MembershipState {
        self.timeline.state()
    }

    /// Who signs for a person: its first device.
    pub fn who(person: usize) -> Who {
        (person, 0)
    }

    // ----- low-level building ------------------------------------------------------------------

    /// `(seq, prev)` of the next entry of the log.
    pub fn next_position(&self) -> (u64, [u8; 32]) {
        match self.timeline.state().head {
            None => (0, [0u8; 32]),
            Some(head) => (head.seq + 1, head.hash),
        }
    }

    /// The key an entry's private part is sealed under (a stand-in for epochs nobody holds).
    fn meta_key(&self, meta_epoch: u32) -> Secret32 {
        match self.keys.get(&meta_epoch) {
            Some(sk) => membership_meta_key(sk, self.feed),
            None => Secret32::from_bytes([0xee; 32]),
        }
    }

    /// An entry of any shape signed by `who`, at an explicit position.
    #[allow(clippy::too_many_arguments)]
    pub fn entry_at(
        &self,
        who: Who,
        seq: u64,
        prev: [u8; 32],
        at: i64,
        meta_epoch: u32,
        body: EntryBody,
        private: &Private,
    ) -> MembershipEntry {
        let person = &self.world.people[who.0];
        let header = MembershipHeader {
            feed: self.feed,
            seq,
            prev,
            account: person.account,
            device: person.devices[who.1].id,
            at,
            meta_epoch,
            body,
        };
        self.sign_header(header, private, who)
    }

    /// An entry with exactly this header, sealed and signed by `signer` (which need not be the
    /// account the header names: that is how forgeries are built).
    pub fn sign_header(
        &self,
        header: MembershipHeader,
        private: &Private,
        signer: Who,
    ) -> MembershipEntry {
        let device = &self.world.people[signer.0].devices[signer.1];
        let meta = self.meta_key(header.meta_epoch);
        let n = self.built.get();
        self.built.set(n + 1);
        let rng = SeededRng::new(0x00ab_cdef_0000 + n);
        if self.fast {
            let key = device.public.signing;
            MembershipEntry::build_with(header, private, &meta, &rng, &|signed: &[u8]| {
                fake_signature(&key, signed)
            })
        } else {
            MembershipEntry::build(header, private, &meta, &device.keys, &rng)
        }
    }

    /// Who signed an entry (the person and device its header names).
    pub fn who_of(&self, header: &MembershipHeader) -> Who {
        let person = self
            .world
            .people
            .iter()
            .position(|p| p.account == header.account)
            .unwrap();
        let device = self.world.people[person]
            .devices
            .iter()
            .position(|d| d.id == header.device)
            .unwrap();
        (person, device)
    }

    /// The same entry with its header edited, signed again by the same device.
    pub fn rebuild(
        &self,
        entry: &MembershipEntry,
        edit: impl FnOnce(&mut MembershipHeader),
    ) -> MembershipEntry {
        let who = self.who_of(&entry.header);
        let mut header = entry.header.clone();
        edit(&mut header);
        self.sign_header(header, &Private::None, who)
    }

    /// An entry at the next position of the log.
    pub fn entry(
        &self,
        who: Who,
        meta_epoch: u32,
        body: EntryBody,
        private: &Private,
    ) -> MembershipEntry {
        let (seq, prev) = self.next_position();
        self.entry_at(who, seq, prev, self.clock, meta_epoch, body, private)
    }

    /// Offers an entry to the sequencer: the fold judges it at the next cursor.
    pub fn submit(&mut self, entry: MembershipEntry) -> Result<Applied, FoldError> {
        let cursor = self.cursor + self.gap;
        let result = if self.fast {
            self.timeline
                .apply_checked(cursor, &entry, &self.world, &FAST)
        } else {
            self.timeline.apply(cursor, &entry, &self.world)
        };
        if result.is_ok() {
            self.entries.push((cursor, entry));
            self.cursor = cursor;
            self.clock += 1_000;
        }
        result
    }

    /// Applies one entry to `timeline` the way `submit` does (same mode, no sequencer).
    pub fn apply_to(
        &self,
        timeline: &mut Timeline,
        cursor: u64,
        entry: &MembershipEntry,
    ) -> Result<Applied, FoldError> {
        if self.fast {
            timeline.apply_checked(cursor, entry, &self.world, &FAST)
        } else {
            timeline.apply(cursor, entry, &self.world)
        }
    }

    /// `fold` in this space's mode: the whole log at once.
    pub fn fold(&self, entries: &[(u64, MembershipEntry)]) -> Result<Timeline, FoldError> {
        let mut timeline = Timeline::new(self.feed);
        for (cursor, entry) in entries {
            self.apply_to(&mut timeline, *cursor, entry)?;
        }
        Ok(timeline)
    }

    /// An entry that must be accepted.
    pub fn push(&mut self, entry: MembershipEntry) -> Applied {
        self.submit(entry).unwrap()
    }

    /// The feed cursor the next accepted entry will get.
    pub fn next_cursor(&self) -> u64 {
        self.cursor + self.gap
    }

    /// The public key of an invite secret (from the pool in the fast mode).
    pub fn invite_pub(&self, secret: &InviteSecret) -> VerifyingKey {
        if self.fast {
            pool_pub(secret).unwrap_or_else(dummy_pub)
        } else {
            invite_signing_key(secret).verifying_key()
        }
    }

    /// A join proof by `secret` over any fields (a forgery when they are not the entry's).
    pub fn proof_raw(
        &self,
        secret: &InviteSecret,
        feed: FeedId,
        id: InviteId,
        account: AccountId,
        head: &ChainHead,
    ) -> Signature {
        if self.fast {
            fake_proof(&self.invite_pub(secret), feed, id, account, head)
        } else {
            sign_join_proof(&invite_signing_key(secret), feed, id, account, head)
        }
    }

    /// Checks a join proof in this space's mode.
    pub fn check_proof(
        &self,
        pubkey: &VerifyingKey,
        feed: FeedId,
        id: InviteId,
        account: AccountId,
        head: &ChainHead,
        proof: &Signature,
    ) -> Result<(), KeysError> {
        if self.fast {
            fake_proof_verify(pubkey, feed, id, account, head, proof)
        } else {
            verify_join_proof(pubkey, feed, id, account, head, proof)
        }
    }

    // ----- keys and boxes ---------------------------------------------------------------------

    fn fresh_key(&mut self) -> SpaceKey {
        self.draws += 1;
        SpaceKey::generate(&SeededRng::new(0x5eed_0000 + self.draws))
    }

    /// A box of `sk` for a device: real, or opaque bytes in the fast mode.
    fn seal_box(&self, sk: &SpaceKey, epoch: u32, keys: &crate::DevicePublicKeys) -> Vec<u8> {
        if self.fast {
            vec![0u8; 104]
        } else {
            seal_space_key_for(sk, self.feed, epoch, keys, &self.rng)
        }
    }

    fn wrap(&self, prev: &SpaceKey, sk: &SpaceKey, epoch_prev: u32) -> WrappedKey {
        if self.fast {
            WrappedKey::from_bytes(&[0u8; 72]).unwrap()
        } else {
            wrap_prev(prev, sk, self.feed, epoch_prev, &self.rng)
        }
    }

    /// The boxes and heads of an entry that gives `sk` (of `epoch`) to the devices of `members`.
    pub fn boxes_for(
        &self,
        sk: &SpaceKey,
        epoch: u32,
        members: &[AccountId],
    ) -> (Vec<KeyBox>, Vec<MemberHead>) {
        let plan = rekey_recipients(members, &self.world).unwrap();
        let boxes = plan
            .devices
            .iter()
            .map(|(_, device, keys)| KeyBox {
                device: *device,
                bytes: self.seal_box(sk, epoch, keys),
            })
            .collect();
        (boxes, plan.member_heads)
    }

    /// The fields of a `Rekey` to the next epoch for `members`, with a fresh key (stored).
    pub fn rekey_fields(&mut self, members: &[AccountId]) -> RekeyFields {
        let epoch = self.timeline.state().epoch;
        let new_epoch = epoch + 1;
        let sk = self.fresh_key();
        let (boxes, member_heads) = self.boxes_for(&sk, new_epoch, members);
        let prev_wrapped = self.wrap(&self.keys[&epoch].clone(), &sk, epoch);
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

    // ----- one builder per kind -----------------------------------------------------------------

    /// A `Create` by `creator` at epoch 1.
    pub fn create(&mut self, creator: usize) -> MembershipEntry {
        let sk = self.fresh_key();
        let epoch = 1;
        let account = self.account(creator);
        let (boxes, member_heads) = self.boxes_for(&sk, epoch, &[account]);
        let commitment = sk.commitment(self.feed, epoch);
        self.keys.insert(epoch, sk);
        self.entry(
            Self::who(creator),
            epoch,
            EntryBody::Create {
                epoch,
                sk_commitment: commitment,
                boxes,
                member_heads,
            },
            &Private::Create {
                space_name: "Space".to_owned(),
                settings: SpaceSettings::default(),
                profile_name: format!("Person {creator}"),
            },
        )
    }

    /// The `Invite` of a new secret by `by`: a link invite when `bound` is `None`.
    pub fn invite(
        &mut self,
        by: usize,
        role: MemberRole,
        bound: Option<usize>,
        expires_in: i64,
    ) -> (MembershipEntry, InviteSecret) {
        self.draws += 1;
        let secret = if self.fast {
            let pool = invite_pool();
            pool[usize::try_from(self.draws - 1).unwrap() % pool.len()]
                .0
                .clone()
        } else {
            InviteSecret::generate(&SeededRng::new(0x1117_0000 + self.draws))
        };
        let entry = self.invite_with(by, &secret, role, bound, expires_in);
        (entry, secret)
    }

    /// The `Invite` of a given secret.
    pub fn invite_with(
        &mut self,
        by: usize,
        secret: &InviteSecret,
        role: MemberRole,
        bound: Option<usize>,
        expires_in: i64,
    ) -> MembershipEntry {
        let id = invite_id(secret);
        self.secrets.insert(id, secret.clone());
        let epoch = self.timeline.state().epoch;
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::Invite {
                invite_id: id,
                invite_pub: self.invite_pub(secret),
                role,
                expires_at: self.clock + expires_in,
                bound_account: bound.map(|p| self.account(p)),
                verified: bound.is_some(),
                enc_bundle: bound.is_none().then(|| vec![9; 40]),
            },
            &Private::None,
        )
    }

    pub fn revoke(&self, by: usize, id: InviteId) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::InviteRevoke { invite_id: id },
            &Private::None,
        )
    }

    /// The `Join` of `who` with `secret`'s proof, at the head of its log.
    pub fn join(&self, who: usize, secret: &InviteSecret) -> MembershipEntry {
        let person = &self.world.people[who];
        let id = invite_id(secret);
        let head = person.head();
        let proof = self.proof_raw(secret, self.feed, id, person.account, &head);
        let epoch = self.timeline.state().epoch;
        self.entry(
            Self::who(who),
            epoch,
            EntryBody::Join {
                invite_id: id,
                proof,
                account: person.account,
                log: head,
            },
            &Private::Join {
                profile_name: format!("Person {who}"),
            },
        )
    }

    pub fn role_set(&self, by: usize, target: usize, role: MemberRole) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::RoleSet {
                target: self.account(target),
                role,
            },
            &Private::None,
        )
    }

    /// A `Remove` of `target` by `by` with its `Rekey` for the members that remain.
    pub fn remove(&mut self, by: usize, target: usize) -> MembershipEntry {
        let target_account = self.account(target);
        let remaining: Vec<AccountId> = self
            .timeline
            .state()
            .members
            .iter()
            .map(|m| m.account)
            .filter(|a| *a != target_account)
            .collect();
        let rekey = self.rekey_fields(&remaining);
        let epoch = rekey.new_epoch;
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::Remove {
                target: target_account,
                rekey,
            },
            &Private::None,
        )
    }

    pub fn leave(&self, who: usize) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        self.entry(Self::who(who), epoch, EntryBody::Leave, &Private::None)
    }

    /// A `Rekey` by `by` for every current member.
    pub fn rekey(&mut self, by: usize) -> MembershipEntry {
        let members: Vec<AccountId> = self
            .timeline
            .state()
            .members
            .iter()
            .map(|m| m.account)
            .collect();
        let rekey = self.rekey_fields(&members);
        let epoch = rekey.new_epoch;
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::Rekey(rekey),
            &Private::None,
        )
    }

    /// A `Grant` of the current key to the devices of `member`.
    pub fn grant(&self, by: usize, member: usize) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        let sk = self.keys[&epoch].clone();
        let (boxes, _) = self.boxes_for(&sk, epoch, &[self.account(member)]);
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::Grant {
                member: self.account(member),
                epoch,
                boxes,
            },
            &Private::None,
        )
    }

    pub fn profile(&self, who: usize, name: &str) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        self.entry(
            Self::who(who),
            epoch,
            EntryBody::Profile,
            &Private::Profile {
                display_name: name.to_owned(),
            },
        )
    }

    pub fn settings(&self, by: usize, settings: SpaceSettings) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        self.entry(
            Self::who(by),
            epoch,
            EntryBody::Settings,
            &Private::Settings { settings },
        )
    }

    pub fn close(&self, by: usize) -> MembershipEntry {
        let epoch = self.timeline.state().epoch;
        self.entry(Self::who(by), epoch, EntryBody::Close, &Private::None)
    }

    // ----- scenarios -----------------------------------------------------------------------------

    /// `creator` creates the space; `joiners` join in order with the given roles (link invites by
    /// the creator).
    pub fn start(&mut self, creator: usize, joiners: &[(usize, MemberRole)]) {
        let create = self.create(creator);
        self.push(create);
        for (person, role) in joiners {
            let (invite, secret) = self.invite(creator, *role, None, 7 * DAY);
            self.push(invite);
            let join = self.join(*person, &secret);
            self.push(join);
        }
    }

    /// The members as `(person index, role)`, in join order.
    pub fn roster(&self) -> Vec<(usize, MemberRole)> {
        self.timeline
            .state()
            .members
            .iter()
            .map(|m| {
                let person = self
                    .world
                    .people
                    .iter()
                    .position(|p| p.account == m.account)
                    .unwrap();
                (person, m.role)
            })
            .collect()
    }

    /// The devices a `Rekey` of the current members must box: the active devices of each member as
    /// of the head of the log the source serves (by the log's own verifier).
    pub fn expected_devices(&self, accounts: &[AccountId]) -> BTreeSet<DeviceId> {
        let mut out = BTreeSet::new();
        for account in accounts {
            let (index, person) = self
                .world
                .people
                .iter()
                .enumerate()
                .find(|(_, p)| p.account == *account)
                .unwrap();
            let seq = person.head().seq;
            if self.fast {
                out.extend(
                    template_active()[index][usize::try_from(seq).unwrap()]
                        .iter()
                        .copied(),
                );
            } else {
                out.extend(person.active_by_verifier(seq));
            }
        }
        out
    }

    /// The active devices of `account` as of log position `seq` (independent of the fold).
    pub fn active_at(&self, account: AccountId, seq: u64) -> BTreeSet<DeviceId> {
        let (index, person) = self
            .world
            .people
            .iter()
            .enumerate()
            .find(|(_, p)| p.account == account)
            .unwrap();
        if self.fast {
            template_active()[index][usize::try_from(seq).unwrap()].clone()
        } else {
            person.active_by_verifier(seq)
        }
    }
}
