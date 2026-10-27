//! Property tests of the membership fold (milestone-13.md, 1.8, "Property tests of the membership
//! fold"). They run the fold through a simulated compare-and-swap sequencer (the testkit's
//! `Space`): random sequences of honest actions by several accounts (an honest action is one a
//! client would make against the state it sees, so the fold must accept it) mixed with hostile
//! entries (any signer, any target, broken boxes, stale heads, wrong proofs, forged signers) that
//! the fold must judge without ever breaking an invariant.
//!
//! The crypto of the debug build is too slow for ten thousand cases, so these run on the testkit's
//! fast mode: signatures are a keyed checksum, boxes are opaque bytes and the device logs come
//! from a template (see `testkit.rs`). Every rule of the fold is in play all the same; the real
//! signatures, boxes and proofs are covered by the unit tests, the scenario and the real-crypto
//! cases at the end of this file.
//!
//! Set `KB_KEYS_PROPTEST_CASES` to change the number of cases of every property.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use kb_types::sharing::{INVITE_MAX_DAYS, MAX_MEMBERS_PER_FEED, MAX_OPEN_INVITES};
use proptest::prelude::*;

use super::testkit::*;
use super::*;

const PEOPLE: usize = 5;
const HOUR: i64 = 3_600_000;

const ROLES: [MemberRole; 3] = [
    MemberRole::Editor,
    MemberRole::Commenter,
    MemberRole::Viewer,
];
const ALL_ROLES: [MemberRole; 4] = [
    MemberRole::Viewer,
    MemberRole::Commenter,
    MemberRole::Editor,
    MemberRole::Owner,
];

/// The number of cases of a property: `KB_KEYS_PROPTEST_CASES`, else `default`.
fn cases(default: u32) -> u32 {
    std::env::var("KB_KEYS_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ----- what a client does --------------------------------------------------------------------

/// An intent, resolved against the state a client sees (ranks pick among the valid choices).
#[derive(Clone, Debug)]
enum Act {
    Invite {
        by: usize,
        role: usize,
        who: usize,
        link: bool,
        long: bool,
    },
    Revoke {
        by: usize,
        which: usize,
    },
    Join {
        who: usize,
    },
    RoleSet {
        by: usize,
        target: usize,
        role: usize,
    },
    Remove {
        by: usize,
        target: usize,
    },
    Leave {
        who: usize,
    },
    Rekey {
        by: usize,
    },
    Grant {
        by: usize,
        target: usize,
    },
    Profile {
        who: usize,
    },
    Settings {
        by: usize,
    },
    Close {
        by: usize,
    },
    AdvanceLog {
        person: usize,
    },
    Wait {
        hours: i64,
    },
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        6 => (any::<usize>(), any::<usize>(), any::<usize>(), any::<bool>(), any::<bool>())
            .prop_map(|(by, role, who, link, long)| Act::Invite { by, role, who, link, long }),
        2 => (any::<usize>(), any::<usize>()).prop_map(|(by, which)| Act::Revoke { by, which }),
        8 => any::<usize>().prop_map(|who| Act::Join { who }),
        4 => (any::<usize>(), any::<usize>(), any::<usize>())
            .prop_map(|(by, target, role)| Act::RoleSet { by, target, role }),
        3 => (any::<usize>(), any::<usize>()).prop_map(|(by, target)| Act::Remove { by, target }),
        2 => any::<usize>().prop_map(|who| Act::Leave { who }),
        3 => any::<usize>().prop_map(|by| Act::Rekey { by }),
        2 => (any::<usize>(), any::<usize>()).prop_map(|(by, target)| Act::Grant { by, target }),
        2 => any::<usize>().prop_map(|who| Act::Profile { who }),
        1 => any::<usize>().prop_map(|by| Act::Settings { by }),
        1 => any::<usize>().prop_map(|by| Act::Close { by }),
        3 => any::<usize>().prop_map(|person| Act::AdvanceLog { person }),
        2 => (1i64..60).prop_map(|hours| Act::Wait { hours }),
    ]
}

fn pick<T: Copy>(items: &[T], rank: usize) -> Option<T> {
    (!items.is_empty()).then(|| items[rank % items.len()])
}

fn person_of(space: &Space, account: AccountId) -> usize {
    space
        .world
        .people
        .iter()
        .position(|p| p.account == account)
        .unwrap()
}

fn member_people(space: &Space) -> Vec<usize> {
    space
        .state()
        .members
        .iter()
        .map(|m| person_of(space, m.account))
        .collect()
}

fn owner_people(space: &Space) -> Vec<usize> {
    space
        .state()
        .owners()
        .into_iter()
        .map(|a| person_of(space, a))
        .collect()
}

/// What an honest client writes for `act` against the state it sees, if the act makes sense there.
fn resolve(space: &mut Space, act: &Act) -> Option<MembershipEntry> {
    match *act {
        Act::AdvanceLog { person } => {
            let count = space.world.people.len();
            let person = &mut space.world.people[person % count];
            person.version = (person.version + 1).min(person.versions.len() - 1);
            return None;
        }
        Act::Wait { hours } => {
            space.clock += hours * HOUR;
            return None;
        }
        _ => {}
    }
    if space.state().head.is_none() || space.state().closed.is_some() {
        return None;
    }
    let members = member_people(space);
    let owners = owner_people(space);
    let people = space.world.people.len();
    match *act {
        Act::Invite {
            by,
            role,
            who,
            link,
            long,
        } => {
            let by = pick(&owners, by)?;
            let open = space
                .state()
                .invites
                .iter()
                .filter(|i| i.status == InviteRecordStatus::Open && i.expires_at > space.clock)
                .count();
            if open >= MAX_OPEN_INVITES {
                return None;
            }
            let outsiders: Vec<usize> = (0..people).filter(|p| !members.contains(p)).collect();
            let bound = if link { None } else { pick(&outsiders, who) };
            let expiry = if long { 7 * 24 * HOUR } else { HOUR };
            Some(space.invite(by, ROLES[role % 3], bound, expiry).0)
        }
        Act::Revoke { by, which } => {
            let by = pick(&owners, by)?;
            let open: Vec<InviteId> = space.state().open_invites().map(|i| i.invite_id).collect();
            Some(space.revoke(by, pick(&open, which)?))
        }
        Act::Join { who } => {
            let mut options = Vec::new();
            for person in (0..people).filter(|p| !members.contains(p)) {
                let account = space.account(person);
                for invite in space.state().open_invites() {
                    if invite.bound_account.is_none_or(|b| b == account) {
                        options.push((person, invite.invite_id));
                    }
                }
            }
            let (person, id) = pick(&options, who)?;
            let secret = space.secrets[&id].clone();
            Some(space.join(person, &secret))
        }
        Act::RoleSet { by, target, role } => {
            let by = pick(&owners, by)?;
            let target = pick(&members, target)?;
            let role = ALL_ROLES[role % 4];
            let target_is_owner = owners.contains(&target);
            if target_is_owner && !role.is_owner() && owners.len() == 1 {
                return None;
            }
            Some(space.role_set(by, target, role))
        }
        Act::Remove { by, target } => {
            let by = pick(&owners, by)?;
            let others: Vec<usize> = members.iter().copied().filter(|m| *m != by).collect();
            let target = pick(&others, target)?;
            Some(space.remove(by, target))
        }
        Act::Leave { who } => {
            let who = pick(&members, who)?;
            if owners.contains(&who) && owners.len() == 1 {
                return None;
            }
            Some(space.leave(who))
        }
        Act::Rekey { by } => Some(space.rekey(pick(&members, by)?)),
        Act::Grant { by, target } => {
            let by = pick(&members, by)?;
            Some(space.grant(by, pick(&members, target)?))
        }
        Act::Profile { who } => Some(space.profile(pick(&members, who)?, "name")),
        Act::Settings { by } => Some(space.settings(pick(&owners, by)?, SpaceSettings::default())),
        // Rare: a closed space ends the run.
        Act::Close { by } if by % 16 == 0 => Some(space.close(pick(&owners, by)?)),
        Act::Close { .. } => None,
        Act::AdvanceLog { .. } | Act::Wait { .. } => None,
    }
}

// ----- what a hostile party does -------------------------------------------------------------

/// An entry written with no regard for the rules.
#[derive(Clone, Debug)]
struct Hostile {
    kind: usize,
    by: usize,
    target: usize,
    role: usize,
    which: usize,
    fault: usize,
}

fn hostile() -> impl Strategy<Value = Hostile> {
    (
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
        any::<usize>(),
    )
        .prop_map(|(kind, by, target, role, which, fault)| Hostile {
            kind,
            by,
            target,
            role,
            which,
            fault,
        })
}

fn rekey_fields_mut(body: &mut EntryBody) -> Option<&mut RekeyFields> {
    match body {
        EntryBody::Remove { rekey, .. } | EntryBody::Rekey(rekey) => Some(rekey),
        _ => None,
    }
}

fn boxes_mut(body: &mut EntryBody) -> Option<&mut Vec<KeyBox>> {
    match body {
        EntryBody::Create { boxes, .. } | EntryBody::Grant { boxes, .. } => Some(boxes),
        EntryBody::Remove { rekey, .. } | EntryBody::Rekey(rekey) => Some(&mut rekey.boxes),
        _ => None,
    }
}

fn heads_mut(body: &mut EntryBody) -> Option<&mut Vec<MemberHead>> {
    match body {
        EntryBody::Create { member_heads, .. } => Some(member_heads),
        EntryBody::Remove { rekey, .. } | EntryBody::Rekey(rekey) => Some(&mut rekey.member_heads),
        _ => None,
    }
}

/// A well-formed entry of a random kind by a random person, then possibly one thing wrong with it.
fn hostile_entry(space: &mut Space, h: &Hostile) -> MembershipEntry {
    // Three in four are written by someone who may write that kind (so the deeper rules are
    // reached); the others by anybody.
    let members = member_people(space);
    let owners = owner_people(space);
    let outsiders: Vec<usize> = (0..PEOPLE).filter(|p| !members.contains(p)).collect();
    let informed = !h.which.is_multiple_of(4) && space.state().head.is_some();
    let by = if informed {
        match h.kind % 12 {
            1 | 2 | 4 | 5 | 10 | 11 => pick(&owners, h.by),
            6..=9 => pick(&members, h.by),
            3 => pick(&outsiders, h.by),
            _ => None,
        }
    } else {
        None
    }
    .unwrap_or(h.by % PEOPLE);
    let target = h.target % PEOPLE;
    let role = ALL_ROLES[h.role % 4];
    let known: Vec<InviteId> = space.state().invites.iter().map(|i| i.invite_id).collect();
    let entry = match h.kind % 12 {
        0 => space.create(by),
        1 => {
            space
                .invite(
                    by,
                    role,
                    h.which.is_multiple_of(3).then_some(target),
                    i64::try_from(1 + h.which % 40).unwrap() * 24 * HOUR,
                )
                .0
        }
        2 => {
            let id =
                pick(&known, h.which).unwrap_or_else(|| InviteId::from_bytes([h.which as u8; 16]));
            space.revoke(by, id)
        }
        3 => {
            let secret = match pick(&known, h.which) {
                Some(id) if space.secrets.contains_key(&id) => space.secrets[&id].clone(),
                _ => crate::invite::InviteSecret::generate(&kb_types::testing::SeededRng::new(
                    h.which as u64,
                )),
            };
            space.join(by, &secret)
        }
        4 => space.role_set(by, target, role),
        5 => space.remove(by, target),
        6 => space.leave(by),
        7 => space.rekey(by),
        8 => space.grant(by, target),
        9 => space.profile(by, "hostile"),
        10 => space.settings(by, SpaceSettings::default()),
        _ if h.which.is_multiple_of(8) => space.close(by),
        _ => space.profile(by, "hostile"),
    };
    let stranger = (by + 1 + h.which % (PEOPLE - 1)) % PEOPLE;
    let stranger_device = space.world.people[stranger].devices[0].id;
    match h.fault % 18 {
        3 => space.rebuild(&entry, |hd| hd.meta_epoch = hd.meta_epoch.wrapping_add(3)),
        4 => space.rebuild(&entry, |hd| match &mut hd.body {
            EntryBody::Grant { epoch, .. } => *epoch = epoch.wrapping_add(1),
            body => {
                if let Some(r) = rekey_fields_mut(body) {
                    r.new_epoch = r.new_epoch.wrapping_sub(1);
                }
            }
        }),
        5 => space.rebuild(&entry, |hd| {
            if let Some(boxes) = boxes_mut(&mut hd.body) {
                boxes.pop();
            }
        }),
        6 => space.rebuild(&entry, |hd| {
            if let Some(boxes) = boxes_mut(&mut hd.body) {
                boxes.push(KeyBox {
                    device: stranger_device,
                    bytes: vec![0; 104],
                });
            }
        }),
        7 => space.rebuild(&entry, |hd| {
            if let Some(heads) = heads_mut(&mut hd.body) {
                heads.pop();
            }
        }),
        8 => space.rebuild(&entry, |hd| {
            if let Some(heads) = heads_mut(&mut hd.body)
                && let Some(first) = heads.first_mut()
            {
                first.head.hash = [h.which as u8; 32];
            }
        }),
        9 => space.sign_header(entry.header.clone(), &Private::None, (stranger, 0)),
        10 => space.rebuild(&entry, |hd| hd.seq += 1),
        11 => space.rebuild(&entry, |hd| hd.prev[0] ^= 1),
        12 => space.rebuild(&entry, |hd| {
            if let EntryBody::Join { proof, .. } = &mut hd.body {
                *proof = Signature::from_bytes(&[h.which as u8; 64]).unwrap();
            }
        }),
        13 => space.rebuild(&entry, |hd| {
            if let Some(boxes) = boxes_mut(&mut hd.body)
                && let Some(first) = boxes.first_mut()
            {
                first.device = DeviceId::from_bytes([h.which as u8; 16]);
            }
        }),
        14 => {
            // Not signed at all.
            let mut unsigned = entry;
            unsigned.envelope.signature = None;
            unsigned
        }
        15 => {
            // A header that is not the one the envelope carries.
            let mut other = entry;
            other.header.at += 1;
            other
        }
        16 => space.rebuild(&entry, |hd| {
            // A head beyond the log the source holds.
            if let Some(heads) = heads_mut(&mut hd.body)
                && let Some(first) = heads.first_mut()
            {
                first.head.seq += 50;
            }
        }),
        17 => space.rebuild(&entry, |hd| {
            if let Some(boxes) = boxes_mut(&mut hd.body)
                && let Some(first) = boxes.first().cloned()
            {
                boxes.push(first);
            }
        }),
        _ => entry,
    }
}

// ----- the rules, written as necessary conditions of an accepted entry -------------------------

const OWNER_KINDS: [EntryKind; 6] = [
    EntryKind::Invite,
    EntryKind::InviteRevoke,
    EntryKind::RoleSet,
    EntryKind::Remove,
    EntryKind::Settings,
    EntryKind::Close,
];

/// The devices a `Create`, `Rekey` or `Remove` must box for `members`, worked out from the device
/// logs with the log's own verifier: `fields` name exactly these accounts' heads, which are
/// positions of the real logs, and box exactly the devices active as of them.
fn assert_covers(space: &Space, members: &[AccountId], boxes: &[KeyBox], heads: &[MemberHead]) {
    let named: BTreeSet<AccountId> = heads.iter().map(|h| h.account).collect();
    let wanted: BTreeSet<AccountId> = members.iter().copied().collect();
    assert_eq!(heads.len(), members.len(), "one head per member");
    assert_eq!(named, wanted, "heads name exactly the members");
    let mut expected = BTreeSet::new();
    for head in heads {
        let person = &space.world.people[person_of(space, head.account)];
        assert_eq!(
            person.latest().entry_hash(head.head.seq),
            Some(head.head.hash),
            "a head is a position of the log"
        );
        expected.extend(space.active_at(head.account, head.head.seq));
    }
    let boxed: BTreeSet<DeviceId> = boxes.iter().map(|b| b.device).collect();
    assert_eq!(boxed.len(), boxes.len(), "no device twice");
    assert_eq!(
        boxed, expected,
        "the boxes cover exactly the active devices"
    );
}

/// Every condition the rules of ADR-0043 impose on an entry the fold accepted, checked from the
/// state before it, the state after it and the device logs. No rule of the fold is used.
fn assert_sound(space: &Space, pre: &MembershipState, entry: &MembershipEntry) {
    let post = space.state();
    let h = &entry.header;
    let kind = h.kind();
    let signer = h.account;
    let before = pre.role_of(signer);

    // The chain, the feed, the end of the log.
    assert_eq!(h.feed, pre.feed);
    assert_eq!(h.seq, pre.head.map_or(0, |head| head.seq + 1));
    assert_eq!(h.prev, pre.head.map_or([0; 32], |head| head.hash));
    assert!(pre.closed.is_none(), "nothing follows a Close");
    assert_eq!(kind == EntryKind::Create, pre.head.is_none());

    // Who may write what.
    match kind {
        EntryKind::Create => {}
        EntryKind::Join => assert!(before.is_none(), "a member cannot join"),
        _ => assert!(before.is_some(), "{kind:?} by a non-member"),
    }
    if OWNER_KINDS.contains(&kind) {
        assert_eq!(before, Some(MemberRole::Owner), "{kind:?} by a non-owner");
    }
    // The private part is sealed under an epoch that exists.
    assert!(h.meta_epoch <= post.epoch, "a future epoch");

    match &h.body {
        EntryBody::Create { epoch, .. } => {
            assert_eq!(post.members.len(), 1);
            assert_eq!(post.role_of(signer), Some(MemberRole::Owner));
            assert_eq!(post.epoch, *epoch);
        }
        EntryBody::Invite {
            invite_id,
            role,
            expires_at,
            ..
        } => {
            assert!(!role.is_owner(), "an invite grants Owner");
            assert!(expires_at.saturating_sub(h.at) <= i64::from(INVITE_MAX_DAYS) * DAY);
            assert!(pre.invite(*invite_id).is_none(), "an invite id twice");
            let open = pre
                .invites
                .iter()
                .filter(|i| i.status == InviteRecordStatus::Open && i.expires_at > h.at)
                .count();
            assert!(open < MAX_OPEN_INVITES, "more than twenty open invites");
        }
        EntryBody::InviteRevoke { invite_id } => {
            assert_eq!(
                pre.invite(*invite_id).unwrap().status,
                InviteRecordStatus::Open
            );
        }
        EntryBody::Join {
            invite_id,
            account,
            proof,
            log,
        } => {
            let invite = pre.invite(*invite_id).expect("a known invite");
            assert_eq!(invite.status, InviteRecordStatus::Open, "a dead invite");
            assert_eq!(*account, signer);
            if let Some(bound) = invite.bound_account {
                assert_eq!(bound, signer, "a bound invite used by another account");
            }
            space
                .check_proof(&invite.invite_pub, h.feed, *invite_id, signer, log, proof)
                .expect("a genuine proof");
            assert_eq!(post.role_of(signer), Some(invite.role));
            assert!(pre.members.len() < MAX_MEMBERS_PER_FEED);
            assert_eq!(
                post.invite(*invite_id).unwrap().status,
                InviteRecordStatus::Used { by: signer }
            );
        }
        EntryBody::RoleSet { target, role } => {
            assert!(pre.role_of(*target).is_some(), "a target that is no member");
            assert_eq!(post.role_of(*target), Some(*role));
        }
        EntryBody::Remove { target, rekey } => {
            assert_ne!(*target, signer);
            assert!(pre.role_of(*target).is_some());
            assert_eq!(post.role_of(*target), None);
            assert_eq!(rekey.new_epoch, pre.epoch + 1);
            let remaining: Vec<AccountId> = post.members.iter().map(|m| m.account).collect();
            assert_covers(space, &remaining, &rekey.boxes, &rekey.member_heads);
            assert!(!post.pending_rekey);
        }
        EntryBody::Leave => {
            assert_eq!(post.role_of(signer), None);
            assert!(post.pending_rekey);
        }
        EntryBody::Rekey(rekey) => {
            assert_eq!(rekey.new_epoch, pre.epoch + 1);
            let members: Vec<AccountId> = post.members.iter().map(|m| m.account).collect();
            assert_eq!(pre.members.len(), post.members.len());
            assert_covers(space, &members, &rekey.boxes, &rekey.member_heads);
            assert!(!post.pending_rekey);
        }
        EntryBody::Grant {
            member,
            epoch,
            boxes,
        } => {
            assert!(pre.role_of(*member).is_some());
            assert_eq!(*epoch, pre.epoch);
            assert!(!boxes.is_empty());
        }
        EntryBody::Profile | EntryBody::Settings => {}
        EntryBody::Close => assert_eq!(post.closed, Some(space.cursor)),
    }

    // Invariants of every state.
    assert!(!post.owners().is_empty(), "the space has no Owner left");
    let accounts: BTreeSet<AccountId> = post.members.iter().map(|m| m.account).collect();
    assert_eq!(accounts.len(), post.members.len(), "a member twice");
    assert!(post.members.len() <= MAX_MEMBERS_PER_FEED);
    let moved = matches!(kind, EntryKind::Rekey | EntryKind::Remove);
    if kind != EntryKind::Create {
        assert!(post.epoch >= pre.epoch && post.epoch <= pre.epoch + 1);
        assert_eq!(
            post.epoch == pre.epoch + 1,
            moved,
            "the epoch moves with Rekey and Remove only"
        );
    }
    assert_eq!(
        post.pending_rekey,
        match kind {
            EntryKind::Leave => true,
            EntryKind::Rekey | EntryKind::Remove => false,
            _ => pre.pending_rekey,
        }
    );
    // Invites: a state is final once left, and one Join uses one invite.
    for before in &pre.invites {
        let now = post.invite(before.invite_id).unwrap();
        match before.status {
            InviteRecordStatus::Open => {}
            status => assert_eq!(now.status, status, "a used or revoked invite changed"),
        }
    }
    let used = |s: &MembershipState| {
        s.invites
            .iter()
            .filter(|i| matches!(i.status, InviteRecordStatus::Used { .. }))
            .count()
    };
    assert_eq!(used(post), used(pre) + usize::from(kind == EntryKind::Join));
    // An Owner who lost the role (removed, left, demoted) has no open invite left.
    for owner in pre.owners() {
        if post.role_of(owner) != Some(MemberRole::Owner) {
            assert!(
                post.invites
                    .iter()
                    .all(|i| i.created_by != owner || i.status != InviteRecordStatus::Open),
                "a former Owner's invite is still open"
            );
        }
    }
    // Only what the entry is about changes among the members.
    for member in &pre.members {
        let changed = post.role_of(member.account) != Some(member.role);
        let named = match &h.body {
            EntryBody::RoleSet { target, .. } | EntryBody::Remove { target, .. } => {
                *target == member.account
            }
            EntryBody::Leave => signer == member.account,
            _ => false,
        };
        assert!(
            !changed || named,
            "a member changed that the entry does not name"
        );
    }
}

// ----- a run -----------------------------------------------------------------------------------

/// The members and the epoch after an accepted entry, for the replays.
#[derive(Clone, Debug)]
struct Snap {
    cursor: u64,
    members: Vec<(AccountId, MemberRole)>,
    epoch: u32,
    /// Whether an entry was accepted at this cursor that made `account` leave the space.
    gone: Vec<AccountId>,
    joined: Vec<AccountId>,
}

struct Run {
    space: Space,
    history: Vec<Snap>,
    /// The highest head of each member seen so far.
    heads: BTreeMap<AccountId, u64>,
    refused: usize,
    accepted: usize,
    /// How often each kind was accepted and each rule refused (for the coverage test).
    kinds: BTreeMap<EntryKind, usize>,
    refusals: BTreeMap<String, usize>,
}

impl Run {
    fn new(seed: u64) -> Self {
        let mut space = Space::new(PEOPLE, true, seed);
        space.gap = 1 + seed % 3;
        Self {
            space,
            history: Vec::new(),
            heads: BTreeMap::new(),
            refused: 0,
            accepted: 0,
            kinds: BTreeMap::new(),
            refusals: BTreeMap::new(),
        }
    }

    /// A server in front of the fold: it refuses a `Join` whose invite expired by its clock, and
    /// then offers the entry to the fold, checking every rule of an accepted entry.
    fn submit(&mut self, entry: MembershipEntry) -> Result<Applied, FoldError> {
        let (seq, prev) = self.space.next_position();
        let in_position = entry.header.seq == seq && entry.header.prev == prev;
        if let (true, EntryBody::Join { invite_id, .. }) = (in_position, &entry.header.body)
            && let Some(invite) = self.space.state().invite(*invite_id)
            && invite.is_expired(entry.header.at)
        {
            self.refused += 1;
            return Err(FoldError {
                index: self.space.timeline.len(),
                rule: FoldRule::Rule("server: the invite expired"),
            });
        }
        let pre = self.space.state().clone();
        let result = self.space.submit(entry.clone());
        match &result {
            Ok(applied) => {
                self.accepted += 1;
                *self.kinds.entry(applied.kind).or_default() += 1;
                assert_sound(&self.space, &pre, &entry);
                self.record(&pre, applied);
            }
            Err(error) => {
                self.refused += 1;
                *self
                    .refusals
                    .entry(format!("{:?}", error.rule))
                    .or_default() += 1;
            }
        }
        result
    }

    fn record(&mut self, pre: &MembershipState, applied: &Applied) {
        let post = self.space.state();
        let members: Vec<(AccountId, MemberRole)> =
            post.members.iter().map(|m| (m.account, m.role)).collect();
        let gone = pre
            .members
            .iter()
            .map(|m| m.account)
            .filter(|a| post.role_of(*a).is_none())
            .collect();
        let joined = if applied.kind == EntryKind::Join || applied.kind == EntryKind::Create {
            vec![applied.signer]
        } else {
            Vec::new()
        };
        self.history.push(Snap {
            cursor: self.space.cursor,
            members,
            epoch: post.epoch,
            gone,
            joined,
        });
        // Heads never go back.
        for member in &self.space.state().members {
            if let Some(seq) = self.space.timeline.head_seq(member.account) {
                let before = self.heads.insert(member.account, seq);
                assert!(before.is_none_or(|b| b <= seq), "a head went back");
            }
        }
    }

    /// An honest step: the act is resolved against the state and must be accepted.
    fn honest(&mut self, act: &Act) {
        if let Some(entry) = resolve(&mut self.space, act) {
            match self.submit(entry) {
                Ok(_) => {}
                Err(FoldError {
                    rule: FoldRule::Rule(rules::GRANT_EXISTS),
                    ..
                }) => {} // a Grant that someone made before: nothing to do
                Err(FoldError {
                    rule: FoldRule::Rule("server: the invite expired"),
                    ..
                }) => {}
                Err(error) => panic!("an honest {act:?} was refused: {error}"),
            }
        }
    }

    /// Concurrent clients: every act is resolved against the same head, the first submission wins,
    /// the others lose the compare-and-swap (`Chain`) and write their intent again.
    fn concurrent(&mut self, acts: &[Act]) {
        let entries: Vec<Option<MembershipEntry>> =
            acts.iter().map(|a| resolve(&mut self.space, a)).collect();
        let mut won = false;
        for (act, entry) in acts.iter().zip(entries) {
            let Some(entry) = entry else { continue };
            if won {
                // Written against the old head: the sequencer refuses it as stale...
                let stale = self.submit(entry);
                assert!(
                    matches!(
                        stale,
                        Err(FoldError {
                            rule: FoldRule::Chain,
                            ..
                        })
                    ),
                    "a stale entry must lose the compare-and-swap, got {stale:?}"
                );
                // ... and the client re-evaluates its intent against what it now sees.
                self.honest(act);
            } else {
                match self.submit(entry) {
                    Ok(_) => won = true,
                    Err(FoldError {
                        rule: FoldRule::Rule(rules::GRANT_EXISTS),
                        ..
                    })
                    | Err(FoldError {
                        rule: FoldRule::Rule("server: the invite expired"),
                        ..
                    }) => {}
                    Err(error) => panic!("an honest {act:?} was refused: {error}"),
                }
            }
        }
    }

    fn hostile(&mut self, h: &Hostile) {
        let entry = hostile_entry(&mut self.space, h);
        let _ = self.submit(entry);
    }
}

#[derive(Clone, Debug)]
enum Step {
    Honest(Act),
    Hostile(Hostile),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        7 => act().prop_map(Step::Honest),
        3 => hostile().prop_map(Step::Hostile),
    ]
}

/// A run of steps after a Create by a random person.
fn run_of(seed: u64, steps: &[Step]) -> Run {
    let mut run = Run::new(seed);
    let creator = usize::try_from(seed % PEOPLE as u64).unwrap();
    let create = run.space.create(creator);
    run.submit(create).expect("the first Create folds");
    for step in steps {
        match step {
            Step::Honest(act) => run.honest(act),
            Step::Hostile(h) => run.hostile(h),
        }
    }
    run
}

// ----- the properties --------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(10_000)))]

    /// (1) The fold of a valid log does not depend on how the entries are chunked: all at once,
    /// one by one, or in any number of chunks over a timeline that was cloned in between.
    #[test]
    fn folding_is_independent_of_the_chunking(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..40),
        splits in proptest::collection::vec(any::<usize>(), 0..4),
    ) {
        let run = run_of(seed, &steps);
        let log = run.space.entries.clone();
        let batch = run.space.fold(&log).unwrap();
        prop_assert_eq!(&batch, &run.space.timeline);

        let mut cuts: Vec<usize> = splits.iter().map(|s| s % (log.len() + 1)).collect();
        cuts.sort_unstable();
        let mut timeline = Timeline::new(run.space.feed);
        let mut from = 0;
        for cut in cuts.into_iter().chain([log.len()]) {
            // A copy of the timeline taken at the cut goes on with the rest.
            let mut copy = timeline.clone();
            for (cursor, entry) in &log[from..cut] {
                run.space.apply_to(&mut copy, *cursor, entry).unwrap();
            }
            timeline = copy;
            from = cut;
        }
        prop_assert_eq!(&timeline, &batch);
    }

    /// (2) Any sequence of honest actions by several accounts, run through a compare-and-swap
    /// with retries, yields a valid log in which an Owner remains at every cursor.
    #[test]
    fn honest_owner_actions_through_a_compare_and_swap_keep_an_owner_at_every_cursor(
        seed in 0u64..1_000_000,
        rounds in proptest::collection::vec(proptest::collection::vec(act(), 1..4), 1..25),
    ) {
        let mut run = Run::new(seed);
        let creator = usize::try_from(seed % PEOPLE as u64).unwrap();
        let create = run.space.create(creator);
        run.submit(create).unwrap();
        for acts in &rounds {
            run.concurrent(acts);
        }
        let space = &run.space;
        // The log is valid from its first entry.
        prop_assert_eq!(space.fold(&space.entries).unwrap(), space.timeline.clone());
        // An Owner at every cursor of the log, and at every cursor in between.
        let last = space.cursor;
        for (cursor, _) in &space.entries {
            prop_assert!(!space.timeline.owners_at(*cursor).is_empty(), "no Owner at {}", cursor);
            prop_assert!(!space.timeline.owners_at(cursor + 1).is_empty());
        }
        prop_assert!(!space.timeline.owners_at(last + 1000).is_empty());
        prop_assert!(space.timeline.owners_at(space.entries[0].0 - 1).is_empty());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(2_000)))]

    /// (3) A removed (or departed) account is never a member again without a new `Join`.
    #[test]
    fn a_removed_account_is_back_only_through_a_new_join(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..50),
    ) {
        let run = run_of(seed, &steps);
        let mut gone: BTreeSet<AccountId> = BTreeSet::new();
        for snap in &run.history {
            for account in &snap.joined {
                gone.remove(account);
            }
            let now: BTreeSet<AccountId> = snap.members.iter().map(|(a, _)| *a).collect();
            for account in &gone {
                prop_assert!(!now.contains(account), "a former member is a member again");
            }
            gone.extend(snap.gone.iter().copied());
        }
        // The timeline agrees at every cursor of the history.
        let mut gone: BTreeSet<AccountId> = BTreeSet::new();
        for snap in &run.history {
            for account in &snap.joined {
                gone.remove(account);
            }
            for account in &gone {
                prop_assert!(!run.space.timeline.is_member_at(*account, snap.cursor));
            }
            gone.extend(snap.gone.iter().copied());
        }
    }

    /// (4) An invite is used at most once and never after its expiry (the server refuses by its
    /// clock) or its revocation, and a removed, demoted or departed Owner's open invites are void.
    /// (The conditions are asserted for every accepted entry by `assert_sound`; here the end
    /// state is checked as well.)
    #[test]
    fn an_invite_is_used_at_most_once_and_dies_with_its_creators_authority(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..50),
    ) {
        let run = run_of(seed, &steps);
        let state = run.space.state();
        // Used by exactly one account each, and each account joined once per invite.
        let mut users: Vec<AccountId> = Vec::new();
        for invite in &state.invites {
            if let InviteRecordStatus::Used { by } = invite.status {
                users.push(by);
                // A bound invite was used by the bound account.
                if let Some(bound) = invite.bound_account {
                    prop_assert_eq!(bound, by);
                }
            }
        }
        let used = users.len();
        let joins = run
            .space
            .entries
            .iter()
            .filter(|(_, e)| e.kind() == EntryKind::Join)
            .count();
        prop_assert_eq!(used, joins, "one invite per Join");
        // Every open invite belongs to a current Owner.
        for invite in state.open_invites() {
            prop_assert_eq!(state.role_of(invite.created_by), Some(MemberRole::Owner));
        }
        // No Join is accepted after the expiry of its invite by the entry's own time.
        for (_, entry) in &run.space.entries {
            if let EntryBody::Join { invite_id, .. } = &entry.header.body {
                let invite = state.invite(*invite_id).unwrap();
                prop_assert!(!invite.is_expired(entry.header.at));
            }
        }
    }

    /// (5) The epoch only grows, by one, with a Rekey or a Remove, and a Rekey covers exactly the
    /// active devices of the members as of its heads (the conditions are asserted for every
    /// accepted entry; here the history is checked as well).
    #[test]
    fn epochs_grow_by_one_and_rekeys_cover_exactly_the_active_devices(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..50),
    ) {
        let run = run_of(seed, &steps);
        let epochs: Vec<u32> = run.history.iter().map(|s| s.epoch).collect();
        for pair in epochs.windows(2) {
            prop_assert!(pair[1] == pair[0] || pair[1] == pair[0] + 1);
        }
        let moves = run
            .space
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.kind(), EntryKind::Rekey | EntryKind::Remove))
            .count();
        let first = epochs[0];
        prop_assert_eq!(usize::try_from(*epochs.last().unwrap() - first).unwrap(), moves);
        // The commitment is the one the last entry that set an epoch carries.
        let last_commitment = run
            .space
            .entries
            .iter()
            .rev()
            .find_map(|(_, e)| match &e.header.body {
                EntryBody::Create { sk_commitment, .. } => Some(*sk_commitment),
                EntryBody::Rekey(r) | EntryBody::Remove { rekey: r, .. } => Some(r.sk_commitment),
                _ => None,
            })
            .unwrap();
        prop_assert_eq!(run.space.state().sk_commitment, last_commitment);
    }

    /// (6) Any single-byte change, reorder, duplicate or drop in a log makes the fold fail at the
    /// first affected entry.
    #[test]
    fn any_change_to_a_log_is_caught_at_the_first_affected_entry(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(act().prop_map(Step::Honest), 3..30),
        pick_entry in any::<usize>(),
        pick_byte in any::<usize>(),
        flip in 1u8..=255,
    ) {
        let run = run_of(seed, &steps);
        let space = &run.space;
        let log = space.entries.clone();
        prop_assume!(log.len() >= 3);
        let k = pick_entry % (log.len() - 1);

        // One byte of entry k changed: it no longer decodes, or the fold fails at k.
        let mut bytes = log[k].1.encode();
        let at = pick_byte % bytes.len();
        bytes[at] ^= flip;
        if let Ok(changed) = MembershipEntry::decode(&bytes) {
            let mut broken = log.clone();
            broken[k].1 = changed;
            let error = space.fold(&broken).unwrap_err();
            prop_assert_eq!(error.index, k);
        }

        // Entries k and k + 1 swapped (the cursors stay where they were).
        let mut swapped = log.clone();
        swapped.swap(k, k + 1);
        for (i, (cursor, _)) in log.iter().enumerate() {
            swapped[i].0 = *cursor;
        }
        prop_assert_eq!(space.fold(&swapped).unwrap_err().index, k);

        // Entry k twice in a row.
        let mut doubled = log.clone();
        doubled.insert(k + 1, (log[k].0, log[k].1.clone()));
        prop_assert_eq!(space.fold(&doubled).unwrap_err().index, k + 1);

        // Entry k dropped (the last one may go: a shorter log is a valid log).
        let mut dropped = log.clone();
        dropped.remove(k);
        prop_assert_eq!(space.fold(&dropped).unwrap_err().index, k);
        prop_assert!(space.fold(&log[..log.len() - 1]).is_ok());
    }

    /// (7) `role_at`, `epoch_at` and `owners_at` agree with a naive replay of what was accepted.
    #[test]
    fn role_at_agrees_with_a_naive_replay(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..50),
        probes in proptest::collection::vec(any::<u64>(), 1..12),
    ) {
        let run = run_of(seed, &steps);
        let last = run.space.cursor;
        let accounts: Vec<AccountId> = (0..PEOPLE).map(|p| run.space.account(p)).collect();
        let mut cursors: Vec<u64> = probes.iter().map(|p| p % (last + 3)).collect();
        cursors.extend(run.history.iter().map(|s| s.cursor));
        for cursor in cursors {
            // The naive answer: the last snapshot at a cursor <= the probe.
            let snap = run.history.iter().rev().find(|s| s.cursor <= cursor);
            for account in &accounts {
                let expected = snap.and_then(|s| {
                    s.members.iter().find(|(a, _)| a == account).map(|(_, r)| *r)
                });
                prop_assert_eq!(run.space.timeline.role_at(*account, cursor), expected);
            }
            prop_assert_eq!(run.space.timeline.epoch_at(cursor), snap.map(|s| s.epoch));
            let owners: Vec<AccountId> = snap
                .map(|s| {
                    s.members
                        .iter()
                        .filter(|(_, r)| r.is_owner())
                        .map(|(a, _)| *a)
                        .collect()
                })
                .unwrap_or_default();
            prop_assert_eq!(run.space.timeline.owners_at(cursor), owners);
        }
    }

    /// (8) Heads are monotone per member: no accepted `Create`, `Rekey` or `Remove` names a lower
    /// head than an earlier one for the same account (`run.record` asserts it at every step; here
    /// the timeline's own view is compared with the heads of the entries).
    #[test]
    fn heads_are_monotone_per_member(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..50),
    ) {
        let run = run_of(seed, &steps);
        let mut highest: BTreeMap<AccountId, u64> = BTreeMap::new();
        for (_, entry) in &run.space.entries {
            let heads: &[MemberHead] = match &entry.header.body {
                EntryBody::Create { member_heads, .. } => member_heads,
                EntryBody::Rekey(r) | EntryBody::Remove { rekey: r, .. } => &r.member_heads,
                _ => continue,
            };
            for head in heads {
                let before = highest.insert(head.account, head.head.seq);
                prop_assert!(before.is_none_or(|b| b <= head.head.seq), "a head went back");
            }
        }
        for (account, seq) in highest {
            prop_assert_eq!(run.space.timeline.head_seq(account), Some(seq));
        }
    }

    /// Hostile entries of every kind never break an invariant (each accepted one is checked by
    /// `assert_sound`), whatever the signer, the target or the fault.
    #[test]
    fn hostile_entries_never_break_an_invariant(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(hostile().prop_map(Step::Hostile), 1..60),
    ) {
        let run = run_of(seed, &steps);
        prop_assert!(!run.space.state().owners().is_empty());
        prop_assert_eq!(run.space.fold(&run.space.entries).unwrap(), run.space.timeline.clone());
        // The entries decoded from their bytes (what a server stores) fold to the same timeline.
        let decoded: Vec<(u64, MembershipEntry)> = run
            .space
            .entries
            .iter()
            .map(|(c, e)| (*c, MembershipEntry::decode(&e.encode()).unwrap()))
            .collect();
        prop_assert_eq!(run.space.fold(&decoded).unwrap(), run.space.timeline.clone());
    }

    /// A header decodes only from its canonical bytes, and arbitrary bytes never panic a decoder.
    #[test]
    fn header_bytes_decode_canonically_or_not_at_all(
        seed in 0u64..1_000_000,
        steps in proptest::collection::vec(step(), 1..20),
        flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 0..4),
        cut in any::<usize>(),
    ) {
        let run = run_of(seed, &steps);
        for (_, entry) in &run.space.entries {
            let bytes = entry.header.encode();
            prop_assert_eq!(&MembershipHeader::decode(&bytes).unwrap(), &entry.header);
            let mut mangled = bytes.clone();
            for (at, with) in &flips {
                let at = at % mangled.len();
                mangled[at] ^= with;
            }
            if let Ok(header) = MembershipHeader::decode(&mangled) {
                prop_assert_eq!(header.encode(), mangled);
            }
            let cut = cut % (bytes.len() + 1);
            if cut < bytes.len() {
                prop_assert!(MembershipHeader::decode(&bytes[..cut]).is_err());
            }
        }
    }
}

proptest! {
    // The same fold with the real signatures, proofs and AEAD (a few cases: the debug build is slow).
    #![proptest_config(ProptestConfig::with_cases(6))]

    #[test]
    fn the_real_cryptography_agrees_with_the_stand_ins(
        seed in 0u64..1_000,
        acts in proptest::collection::vec(act(), 3..12),
    ) {
        let mut space = Space::new(3, false, seed);
        let create = space.create(usize::try_from(seed % 3).unwrap());
        space.push(create);
        for act in &acts {
            if let Some(entry) = resolve(&mut space, act) {
                // Every honest entry folds with the real verifier too.
                match space.submit(entry) {
                    Ok(_) => {}
                    Err(FoldError {
                        rule: FoldRule::Rule(rules::GRANT_EXISTS),
                        ..
                    }) => {}
                    Err(e) => return Err(TestCaseError::fail(format!("{act:?}: {e}"))),
                }
            }
        }
        let replay = fold(space.feed, &space.entries, &space.world).unwrap();
        prop_assert_eq!(replay, space.timeline.clone());
    }
}

/// The generators are worth something only if they reach the rules: over a few hundred runs every
/// kind of entry is accepted, and most of the rules of the fold refuse something.
#[test]
fn the_generators_reach_every_kind_and_most_rules() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let strategy = proptest::collection::vec(step(), 25..45);
    let mut kinds: BTreeMap<EntryKind, usize> = BTreeMap::new();
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    let (mut accepted, mut refused) = (0, 0);
    for seed in 0..300u64 {
        let steps = strategy.new_tree(&mut runner).unwrap().current();
        let mut run = Run::new(seed);
        let creator = usize::try_from(seed % PEOPLE as u64).unwrap();
        let create = run.space.create(creator);
        run.submit(create).unwrap();
        for step in &steps {
            match step {
                Step::Honest(act) => run.honest(act),
                Step::Hostile(h) => run.hostile(h),
            }
        }
        accepted += run.accepted;
        refused += run.refused;
        for (kind, n) in run.kinds {
            *kinds.entry(kind).or_default() += n;
        }
        for (rule, n) in run.refusals {
            *refusals.entry(rule).or_default() += n;
        }
    }
    for kind in EntryKind::ALL {
        assert!(kinds.contains_key(&kind), "{kind:?} was never accepted");
    }
    let categories = [
        "Encoding",
        "Chain",
        "Signature",
        "UnknownLog",
        "Completeness",
        "Heads",
    ];
    for category in categories {
        assert!(
            refusals.keys().any(|r| r == category),
            "{category} was never refused: {refusals:?}"
        );
    }
    let named: Vec<&String> = refusals.keys().filter(|r| r.starts_with("Rule(")).collect();
    assert!(
        named.len() >= 18,
        "only {} named rules refused: {named:?}",
        named.len()
    );
    assert!(
        refused > accepted / 4 && accepted > refused / 8,
        "{accepted} accepted, {refused} refused"
    );
}
