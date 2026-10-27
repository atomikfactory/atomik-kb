//! The membership log: encodings, entries, every rule of the fold, the timeline and the checks of
//! a recipient. Property tests are in `prop.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod encoding;
mod entries;
mod recipients;
mod rules_fold;
mod rules_members;
mod scenario;
mod timeline;

use super::testkit::*;
use super::*;
use kb_types::testing::SeededRng;

fn account(byte: u8) -> AccountId {
    AccountId::from_bytes([byte; 16])
}

fn device(byte: u8) -> DeviceId {
    DeviceId::from_bytes([byte; 16])
}

fn invite_id_of(byte: u8) -> InviteId {
    InviteId::from_bytes([byte; 16])
}

fn rng(seed: u64) -> SeededRng {
    SeededRng::new(seed)
}

/// `result` is a refusal of the entry at `index` for `rule`.
#[track_caller]
fn assert_refused(result: Result<Applied, FoldError>, index: usize, rule: FoldRule) {
    match result {
        Err(error) => assert_eq!((error.index, error.rule), (index, rule), "{error}"),
        Ok(applied) => panic!("accepted {applied:?}, expected {rule:?} at {index}"),
    }
}

/// Offers `entry` and checks that it is refused for `rule` with the timeline untouched.
#[track_caller]
fn refuse(space: &mut Space, entry: MembershipEntry, rule: FoldRule) {
    let before = space.timeline.clone();
    let index = space.timeline.len();
    assert_refused(space.submit(entry), index, rule);
    assert_eq!(space.timeline, before, "a refused entry changes nothing");
}

/// The three roles an invite may grant.
const ROLES: [MemberRole; 3] = [
    MemberRole::Editor,
    MemberRole::Commenter,
    MemberRole::Viewer,
];

#[test]
fn the_rule_names_are_distinct_snake_case_identifiers() {
    use rules::*;
    let all = [
        CLOSED,
        SIGNER_NOT_MEMBER,
        SIGNER_NOT_OWNER,
        META_EPOCH,
        INVITE_GRANTS_OWNER,
        INVITE_EXPIRY_TOO_FAR,
        INVITE_ID_EXISTS,
        TOO_MANY_OPEN_INVITES,
        INVITE_UNKNOWN,
        INVITE_USED,
        INVITE_REVOKED,
        INVITE_BOUND_TO_OTHER_ACCOUNT,
        JOIN_ACCOUNT_MISMATCH,
        JOIN_PROOF_INVALID,
        ALREADY_MEMBER,
        SPACE_FULL,
        TARGET_NOT_MEMBER,
        LAST_OWNER,
        REMOVE_SELF,
        GRANT_NOT_CURRENT_EPOCH,
        GRANT_EXISTS,
        GRANT_BOXES_INVALID,
    ];
    let distinct: std::collections::BTreeSet<&str> = all.iter().copied().collect();
    assert_eq!(distinct.len(), all.len(), "two rules share a name");
    for name in all {
        assert!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{name}"
        );
    }
}
