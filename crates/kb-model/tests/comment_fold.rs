//! Property tests of the comment-stream fold (M13 slice 2, ADR-0048,
//! `docs/development/milestone-13.md` section 2.4).
//!
//! The fold takes `VerifiedRecord`s (the core has checked signature, author account and page, and
//! read the author's role at the carrying item's cursor) and decides which of them count. Two worlds:
//!
//! - **Hostile.** Random records over a handful of accounts, comment ids, threads and roles, with
//!   forged authors, wrong threads, duplicated comment ids, Viewers, backdated and reordered revisions.
//!   Whatever they are, the invariants hold: the result does not depend on the order the records arrive
//!   in (the tie-breaks are part of the rules), no record counts unless its rule allows it, nobody but
//!   the author changes a body, a delete is final, only the thread's author and Editors resolve, and a
//!   snapshot of the valid records folds to the same answer.
//! - **Honest.** Random sequences of actions of accounts with fixed roles, some of them attempts the
//!   rules refuse. A plain sequential model runs the same actions; the fold of the records, delivered
//!   in any order, must agree with it, and must count exactly the records the model accepted.
//!
//! Deterministic by default (fixed seed, 128 cases). Explore further with
//! `PROPTEST_CASES=5000 PROPTEST_RNG_SEED=<n> cargo test -p kb-model --test comment_fold`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use std::collections::{BTreeMap, BTreeSet};

use kb_model::{
    CommentOp, CommentRecord, CommentStream, FoldedComment, FoldedThread, SIGNATURE_LEN,
    VerifiedRecord,
};
use kb_types::{
    AccountId, CommentBodyItem, CommentId, CommentRecordId, DeviceId, MemberRole, PageId, ThreadId,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.parse().ok()
}

/// The default seed of the generators.
const SEED: u64 = 0xc033_e475_2026;

fn config() -> Config {
    Config {
        cases: env("PROPTEST_CASES").unwrap_or(128),
        rng_seed: RngSeed::Fixed(env("PROPTEST_RNG_SEED").unwrap_or(SEED)),
        failure_persistence: None,
        ..Config::default()
    }
}

fn page() -> PageId {
    PageId::from_bytes([0xaa; 16])
}

fn account(n: u8) -> AccountId {
    AccountId::from_bytes([0xa0, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n])
}

fn device(n: u8) -> DeviceId {
    DeviceId::from_bytes([0xd0, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n])
}

fn comment(n: u8) -> CommentId {
    CommentId::from_bytes([0xc0, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n])
}

fn thread(n: u8) -> ThreadId {
    ThreadId::of(comment(n))
}

fn record_id(i: usize) -> CommentRecordId {
    let [hi, lo] = u16::try_from(i).unwrap().to_be_bytes();
    CommentRecordId::from_bytes([0xe0, hi, lo, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
}

fn text(i: usize) -> Vec<CommentBodyItem> {
    vec![CommentBodyItem::Text {
        text: format!("b{i}"),
    }]
}

fn text_of(items: &[CommentBodyItem]) -> String {
    items
        .iter()
        .map(|item| match item {
            CommentBodyItem::Text { text } => text.as_str(),
            _ => "?",
        })
        .collect()
}

/// A deterministic shuffle (xorshift, Fisher-Yates).
fn shuffled<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
    let mut out = items.to_vec();
    let mut state = seed | 1;
    for i in (1..out.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = usize::try_from(state % (u64::try_from(i).unwrap() + 1)).unwrap();
        out.swap(i, j);
    }
    out
}

fn record(
    i: usize,
    comment_id: CommentId,
    thread_id: ThreadId,
    op: CommentOp,
    author: u8,
    dev: u8,
    role: MemberRole,
    rev: u32,
    at: i64,
) -> VerifiedRecord {
    VerifiedRecord {
        record: CommentRecord {
            record_id: record_id(i),
            page_id: page(),
            thread_id,
            comment_id,
            op,
            author: account(author),
            device: device(dev),
            rev,
            at,
            sig: [0; SIGNATURE_LEN],
        },
        author_role: role,
    }
}

fn fold(records: &[VerifiedRecord]) -> CommentStream {
    CommentStream::from_records(page(), records.iter().cloned())
}

/// Everything a stream answers, for comparing two folds of the same set.
fn answers(
    stream: &CommentStream,
) -> (
    Vec<FoldedThread>,
    Vec<CommentRecord>,
    Vec<Option<AccountId>>,
) {
    (
        stream.threads(),
        stream.valid_records().into_iter().cloned().collect(),
        (0..8).map(|n| stream.comment_author(comment(n))).collect(),
    )
}

// ----- the hostile world -------------------------------------------------------------------------

const ROLES: [MemberRole; 4] = [
    MemberRole::Viewer,
    MemberRole::Commenter,
    MemberRole::Editor,
    MemberRole::Owner,
];

/// One random record, as numbers (kept small so that records collide on comment ids, threads,
/// revisions and instants all the time).
#[derive(Debug, Clone)]
struct Spec {
    kind: u8,
    comment: u8,
    thread: u8,
    author: u8,
    role: u8,
    rev: u8,
    at: u8,
    dev: u8,
    right_thread: bool,
    flag: bool,
    /// An edit, delete or resolve is written in the name of the comment's (or thread's) author, so
    /// that a good share of them are the ones the rules allow.
    mimic: bool,
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        prop_oneof![3 => Just(0u8), 3 => Just(1u8), 3 => Just(2u8), 2 => Just(3u8), 2 => Just(4u8)],
        0u8..8,
        0u8..4,
        0u8..5,
        prop_oneof![1 => Just(0u8), 5 => Just(1u8), 2 => Just(2u8), 2 => Just(3u8)],
        0u8..4,
        0u8..12,
        0u8..3,
        prop_oneof![3 => Just(true), 1 => Just(false)],
        any::<bool>(),
        prop_oneof![3 => Just(true), 1 => Just(false)],
    )
        .prop_map(
            |(kind, comment, thread, author, role, rev, at, dev, right_thread, flag, mimic)| Spec {
                kind,
                comment,
                thread,
                author,
                role,
                rev,
                at,
                dev,
                right_thread,
                flag,
                mimic,
            },
        )
}

/// The records of the specs: the spec's position is the record id.
fn build_all(specs: &[Spec]) -> Vec<VerifiedRecord> {
    // Who posted each comment number first (the spec order stands for nothing but a choice).
    let mut author_of: BTreeMap<u8, u8> = BTreeMap::new();
    for spec in specs.iter().filter(|spec| spec.kind <= 1) {
        author_of.entry(spec.comment % 8).or_insert(spec.author);
    }
    specs
        .iter()
        .enumerate()
        .map(|(i, spec)| build(i, spec, &author_of))
        .collect()
}

/// The record of a spec; the record id is the spec's position, the body text names the record.
fn build(i: usize, spec: &Spec, author_of: &BTreeMap<u8, u8>) -> VerifiedRecord {
    let role = ROLES[usize::from(spec.role)];
    let at = i64::from(spec.at) * 10;
    let rev = u32::from(spec.rev);
    let c = spec.comment % 8;
    // The thread a comment number belongs to when its record says it right.
    let natural = c % 4;
    let owner = |number: u8| {
        spec.mimic
            .then(|| author_of.get(&number).copied())
            .flatten()
            .unwrap_or(spec.author)
    };
    match spec.kind {
        0 => record(
            i,
            comment(c),
            thread(c),
            CommentOp::Post {
                anchor: None,
                body: text(i),
            },
            spec.author,
            spec.dev,
            role,
            0,
            at,
        ),
        1 => record(
            i,
            comment(c),
            thread(if spec.right_thread {
                natural
            } else {
                spec.thread % 4
            }),
            CommentOp::Post {
                anchor: None,
                body: text(i),
            },
            spec.author,
            spec.dev,
            role,
            0,
            at,
        ),
        2 | 3 => {
            let thread_id = thread(if spec.right_thread {
                natural
            } else {
                (natural + 1) % 4
            });
            let op = if spec.kind == 2 {
                CommentOp::Edit { body: text(i) }
            } else {
                CommentOp::Delete
            };
            record(
                i,
                comment(c),
                thread_id,
                op,
                owner(c),
                spec.dev,
                role,
                rev,
                at,
            )
        }
        _ => {
            let t = spec.thread % 4;
            let comment_id = if spec.right_thread {
                comment(t)
            } else {
                comment(c)
            };
            record(
                i,
                comment_id,
                thread(t),
                CommentOp::Resolve {
                    resolved: spec.flag,
                },
                owner(t),
                spec.dev,
                role,
                rev,
                at,
            )
        }
    }
}

type EditKey = (u32, i64, DeviceId, CommentRecordId);

fn key(record: &CommentRecord) -> EditKey {
    (record.rev, record.at, record.device, record.record_id)
}

/// The independent reading of a fold's answers: checks every invariant of the rules against the
/// records the fold says count, and the views against those records.
fn verify(stream: &CommentStream, roles: &BTreeMap<CommentRecordId, MemberRole>) {
    let valid = stream.valid_records();
    let valid_ids = stream.valid_record_ids();
    assert_eq!(
        valid.iter().map(|r| r.record_id).collect::<BTreeSet<_>>(),
        valid_ids,
        "the two views agree"
    );
    assert!(
        valid.windows(2).all(|w| w[0].record_id < w[1].record_id),
        "valid records are listed by record id"
    );
    assert!(valid_ids.iter().all(|id| stream.contains(*id)));

    // Posts that count, by comment id (one each).
    let mut posts: BTreeMap<CommentId, &CommentRecord> = BTreeMap::new();
    for r in valid
        .iter()
        .filter(|r| matches!(r.op, CommentOp::Post { .. }))
    {
        assert!(
            posts.insert(r.comment_id, r).is_none(),
            "a comment id has one post that counts"
        );
    }
    // Of the posts that could count for a comment id, the earliest claim by (at, record id) is the
    // comment (when one counts at all: the earliest may be a reply whose root is missing).
    for entry in stream.records() {
        if matches!(entry.record.op, CommentOp::Post { .. })
            && entry.author_role.can_comment()
            && let Some(chosen) = posts.get(&entry.record.comment_id)
        {
            assert!(
                (chosen.at, chosen.record_id) <= (entry.record.at, entry.record.record_id),
                "a later claim for a comment id was chosen: {chosen:?} against {:?}",
                entry.record
            );
        }
    }
    let deleted: BTreeSet<CommentId> = valid
        .iter()
        .filter(|r| matches!(r.op, CommentOp::Delete))
        .map(|r| r.comment_id)
        .collect();

    // Soundness: every record that counts is allowed by its rule.
    for r in &valid {
        let role = roles[&r.record_id];
        assert!(role.can_comment(), "a Viewer's record counts: {r:?}");
        match &r.op {
            CommentOp::Post { .. } => {
                if !r.is_root() {
                    let root = posts
                        .get(&r.thread_id.root_comment())
                        .unwrap_or_else(|| panic!("a reply without a root counts: {r:?}"));
                    assert!(root.is_root());
                }
            }
            CommentOp::Edit { .. } => {
                let post = posts.get(&r.comment_id).expect("an edit of a missing post");
                assert_eq!(r.author, post.author, "only the author edits");
                assert_eq!(r.thread_id, post.thread_id);
                assert!(
                    !deleted.contains(&r.comment_id),
                    "an edit after a delete counts"
                );
            }
            CommentOp::Delete => {
                let post = posts
                    .get(&r.comment_id)
                    .expect("a delete of a missing post");
                assert_eq!(r.thread_id, post.thread_id);
                assert!(
                    r.author == post.author || role.is_owner(),
                    "a delete by someone else than the author or an Owner counts: {r:?}"
                );
            }
            CommentOp::Resolve { .. } => {
                let root = posts
                    .get(&r.thread_id.root_comment())
                    .expect("a resolve of a missing thread");
                assert!(root.is_root());
                assert_eq!(r.comment_id, r.thread_id.root_comment());
                assert!(
                    r.author == root.author || role.can_edit(),
                    "a resolve by someone else than the thread's author or an Editor counts: {r:?}"
                );
            }
        }
    }

    // The views, against those records.
    let created = |post: &CommentRecord| -> i64 {
        if post.is_root() {
            post.at
        } else {
            post.at.max(posts[&post.thread_id.root_comment()].at)
        }
    };
    let threads = stream.threads();
    let mut listed: BTreeSet<ThreadId> = BTreeSet::new();
    for t in &threads {
        assert!(listed.insert(t.thread_id), "a thread is listed once");
        assert!(
            !t.comments.is_empty(),
            "a thread with nothing left is not listed"
        );
        assert_eq!(t.page_id, page());
        assert_eq!(Some(t), stream.thread(t.thread_id).as_ref());
        assert_eq!(
            t.comments[0].comment_id,
            t.thread_id.root_comment(),
            "the root first"
        );
        let mut previous: Option<(i64, CommentId)> = None;
        for (index, c) in t.comments.iter().enumerate() {
            let post = posts[&c.comment_id];
            assert_eq!(c.thread_id, t.thread_id);
            assert_eq!(post.thread_id, t.thread_id);
            assert_eq!(c.author, post.author);
            assert_eq!(c.created_at, created(post));
            assert_eq!(c.deleted, deleted.contains(&c.comment_id));
            if index > 0 {
                let this = (c.created_at, c.comment_id);
                assert!(previous.is_none_or(|p| p <= this), "replies in order");
                previous = Some(this);
            }
            // A placeholder stands only while a later comment is live.
            if c.deleted {
                assert!(t.comments[index + 1..].iter().any(|later| !later.deleted));
                assert!(c.body.is_empty());
                assert_eq!(c.edited_at, None);
                continue;
            }
            // The body is the post's, or the last valid edit's of the author (rev, at, device, id).
            let last_edit = valid
                .iter()
                .filter(|r| r.comment_id == c.comment_id && matches!(r.op, CommentOp::Edit { .. }))
                .max_by_key(|r| key(r));
            let (expected_body, expected_edited) = match last_edit {
                Some(CommentRecord {
                    op: CommentOp::Edit { body },
                    at,
                    ..
                }) => (body, Some((*at).max(c.created_at))),
                _ => match &post.op {
                    CommentOp::Post { body, .. } => (body, None),
                    _ => unreachable!(),
                },
            };
            assert_eq!(&c.body, expected_body, "the body of {:?}", c.comment_id);
            assert_eq!(c.edited_at, expected_edited);
        }
        // The state: the last valid resolve of the thread.
        let last = valid
            .iter()
            .filter(|r| r.thread_id == t.thread_id && matches!(r.op, CommentOp::Resolve { .. }))
            .max_by_key(|r| key(r));
        match last {
            Some(CommentRecord {
                op: CommentOp::Resolve { resolved: true },
                author,
                ..
            }) => {
                assert!(t.resolved);
                assert_eq!(t.resolved_by, Some(*author));
            }
            _ => {
                assert!(!t.resolved);
                assert_eq!(t.resolved_by, None);
            }
        }
    }
    // Which threads are listed: a root that counts with some comment of the thread not deleted.
    for (id, post) in &posts {
        if !post.is_root() {
            continue;
        }
        let thread_id = ThreadId::of(*id);
        let alive = posts
            .values()
            .any(|p| p.thread_id == thread_id && !deleted.contains(&p.comment_id));
        assert_eq!(listed.contains(&thread_id), alive);
        assert!(stream.thread(thread_id).is_some());
    }
    assert_eq!(
        threads.iter().map(|t| t.thread_id).collect::<BTreeSet<_>>(),
        listed
    );
    let roots: Vec<(i64, ThreadId)> = threads
        .iter()
        .map(|t| (posts[&t.thread_id.root_comment()].at, t.thread_id))
        .collect();
    assert!(roots.windows(2).all(|w| w[0] <= w[1]), "oldest root first");
    for (id, post) in &posts {
        assert_eq!(stream.comment_author(*id), Some(post.author));
    }
    // Nothing else has an author or a thread.
    for n in 0..8 {
        if !posts.contains_key(&comment(n)) {
            assert_eq!(stream.comment_author(comment(n)), None);
            assert!(stream.thread(thread(n)).is_none());
        }
    }
}

/// An extra record that no rule allows must change nothing.
fn assert_changes_nothing(base: &CommentStream, extra: VerifiedRecord, what: &str) {
    let mut with = base.clone();
    assert!(with.apply(extra));
    assert_eq!(with.valid_record_ids(), base.valid_record_ids(), "{what}");
    assert_eq!(answers(&with), answers(base), "{what}");
}

fn check_hostile(specs: &[Spec], seed: u64) {
    let records = build_all(specs);
    let roles: BTreeMap<CommentRecordId, MemberRole> = records
        .iter()
        .map(|v| (v.record.record_id, v.author_role))
        .collect();
    let base = fold(&records);
    assert_eq!(base.len(), records.len());
    verify(&base, &roles);

    // The order the records arrive in changes nothing, and a record that arrives twice neither.
    let expected = answers(&base);
    for round in 0..4u64 {
        let other = fold(&shuffled(&records, seed.wrapping_add(round * 7_919)));
        assert_eq!(answers(&other), expected, "round {round}");
        assert_eq!(other.valid_record_ids(), base.valid_record_ids());
        for id in 0..4 {
            assert_eq!(other.thread(thread(id)), base.thread(thread(id)));
        }
    }
    let mut twice = fold(&records);
    for v in &records {
        assert!(!twice.apply(v.clone()), "a record id counts once");
    }
    assert_eq!(answers(&twice), expected);
    // Records arriving one by one are the same set at every step (spot check the prefixes).
    let mut growing = CommentStream::new(page());
    for (n, v) in shuffled(&records, seed ^ 0xfeed).into_iter().enumerate() {
        growing.apply(v);
        if n % 5 == 4 {
            let prefix = growing.records().cloned().collect::<Vec<_>>();
            assert_eq!(answers(&fold(&prefix)), answers(&growing));
        }
    }

    // A snapshot lists the valid records; folding them gives the same answers.
    let snapshot: Vec<VerifiedRecord> = base
        .valid_records()
        .into_iter()
        .map(|r| VerifiedRecord {
            record: r.clone(),
            author_role: roles[&r.record_id],
        })
        .collect();
    let restored = fold(&snapshot);
    assert_eq!(restored.len(), snapshot.len(), "nothing ignored is in it");
    assert_eq!(restored.valid_record_ids(), base.valid_record_ids());
    assert_eq!(answers(&restored), expected);
    assert!(base.is_dominated_by(&restored.valid_record_ids()));
    if let Some(first) = base.valid_record_ids().into_iter().next() {
        let mut missing = base.valid_record_ids();
        missing.remove(&first);
        assert!(
            !base.is_dominated_by(&missing),
            "a truncated snapshot is refused"
        );
    }

    // No one but the author changes a body, a delete is final, only the thread's author and
    // Editors resolve: records no rule allows, however strong their claim, change nothing.
    let mut next = records.len() + 10;
    let valid = base.valid_records();
    let posts: Vec<&CommentRecord> = valid
        .iter()
        .copied()
        .filter(|r| matches!(r.op, CommentOp::Post { .. }))
        .collect();
    let stranger = 99;
    for post in &posts {
        let post: &CommentRecord = post;
        let edit = |i: usize| CommentOp::Edit { body: text(i) };
        // Edits by anyone else, Owners included.
        for role in [MemberRole::Commenter, MemberRole::Editor, MemberRole::Owner] {
            let extra = forged(&mut next, post, stranger, role, edit);
            assert_changes_nothing(&base, extra, "an edit by another account");
        }
        // Deletes by non-owners who are not the author.
        for role in [MemberRole::Commenter, MemberRole::Editor] {
            let extra = forged(&mut next, post, stranger, role, |_| CommentOp::Delete);
            assert_changes_nothing(
                &base,
                extra,
                "a delete by a non-owner who is not the author",
            );
        }
        // A Viewer's record of any op, even in the author's name.
        let author = account_number(post.author);
        for op in [
            edit as fn(usize) -> CommentOp,
            |_| CommentOp::Delete,
            |_| CommentOp::Resolve { resolved: true },
        ] {
            let extra = forged(&mut next, post, author, MemberRole::Viewer, op);
            assert_changes_nothing(&base, extra, "a viewer's record");
        }
        // Resolves by a Commenter who did not start the thread.
        if post.is_root() {
            let extra = forged(&mut next, post, stranger, MemberRole::Commenter, |_| {
                CommentOp::Resolve { resolved: true }
            });
            assert_changes_nothing(&base, extra, "a resolve by another commenter");
        }
    }
    // A delete is final: whatever follows it, from the author with a revision beyond any other.
    let deleted: BTreeSet<CommentId> = valid
        .iter()
        .filter(|r| matches!(r.op, CommentOp::Delete))
        .map(|r| r.comment_id)
        .collect();
    for post in posts.iter().filter(|p| deleted.contains(&p.comment_id)) {
        let extra = forged(
            &mut next,
            post,
            account_number(post.author),
            MemberRole::Commenter,
            |i| CommentOp::Edit { body: text(i) },
        );
        assert_changes_nothing(&base, extra, "an edit after a delete");
    }
}

/// A record about `post`'s comment (in its thread) that claims everything: the highest revision at
/// the latest instant, by `author` as `role`.
fn forged(
    next: &mut usize,
    post: &CommentRecord,
    author: u8,
    role: MemberRole,
    op: impl FnOnce(usize) -> CommentOp,
) -> VerifiedRecord {
    *next += 1;
    let i = *next;
    record(
        i,
        post.comment_id,
        post.thread_id,
        op(i),
        author,
        0,
        role,
        u32::MAX,
        i64::MAX,
    )
}

/// The small number of an account built by [`account`].
fn account_number(account: AccountId) -> u8 {
    account.to_bytes()[1]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn the_fold_holds_its_invariants_whatever_the_records_and_their_order(
        specs in proptest::collection::vec(spec(), 1..40),
        seed in any::<u64>(),
    ) {
        check_hostile(&specs, seed);
    }
}

/// Guards the strength of the hostile property test: a good share of the random records must
/// count, or the invariants would be checked against nearly empty answers.
#[test]
fn random_records_often_enough_count() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(11),
        failure_persistence: None,
        ..Config::default()
    });
    let (mut total, mut counted, mut with_threads, mut with_edits, mut with_resolves) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut with_deletes, mut with_dropped_ids) = (0usize, 0usize);
    for _ in 0..64 {
        let specs = proptest::collection::vec(spec(), 20..40)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let records = build_all(&specs);
        let stream = fold(&records);
        total += records.len();
        counted += stream.valid_record_ids().len();
        with_threads += usize::from(!stream.threads().is_empty());
        let valid = stream.valid_records();
        with_edits += usize::from(valid.iter().any(|r| matches!(r.op, CommentOp::Edit { .. })));
        with_resolves += usize::from(
            valid
                .iter()
                .any(|r| matches!(r.op, CommentOp::Resolve { .. })),
        );
        with_deletes += usize::from(valid.iter().any(|r| matches!(r.op, CommentOp::Delete)));
        with_dropped_ids += usize::from(stream.valid_record_ids().len() < stream.len());
    }
    assert!(counted * 100 >= total * 20, "{counted} of {total} count");
    assert!(counted * 100 <= total * 90, "{counted} of {total} count");
    assert!(with_threads >= 60, "{with_threads} of 64 have threads");
    assert!(with_edits >= 30, "{with_edits} of 64 have valid edits");
    assert!(
        with_resolves >= 35,
        "{with_resolves} of 64 have valid resolves"
    );
    assert!(
        with_deletes >= 30,
        "{with_deletes} of 64 have valid deletes"
    );
    assert!(
        with_dropped_ids >= 60,
        "{with_dropped_ids} of 64 ignore some record"
    );
}

// ----- the honest world ----------------------------------------------------------------------

/// Account 0 is an Owner, 1 an Editor, 2 and 3 Commenters, 4 a Viewer.
fn role_of(account: u8) -> MemberRole {
    [
        MemberRole::Owner,
        MemberRole::Editor,
        MemberRole::Commenter,
        MemberRole::Commenter,
        MemberRole::Viewer,
    ][usize::from(account)]
}

#[derive(Debug, Clone)]
struct Act {
    kind: u8,
    actor: u8,
    target: u8,
    flag: bool,
}

fn act() -> impl Strategy<Value = Act> {
    (
        prop_oneof![2 => Just(0u8), 3 => Just(1u8), 3 => Just(2u8), 2 => Just(3u8), 2 => Just(4u8)],
        0u8..5,
        any::<u8>(),
        prop_oneof![1 => Just(true), 2 => Just(false)],
    )
        .prop_map(|(kind, actor, target, flag)| Act {
            kind,
            actor,
            target,
            flag,
        })
}

struct SimComment {
    id: u8,
    thread: u8,
    author: u8,
    body: usize,
    at: i64,
    edited_at: Option<i64>,
    deleted: bool,
    edits: u32,
}

struct SimThread {
    root: u8,
    resolved_by: Option<u8>,
    rev: u32,
}

/// The records of the acts, the ids of those the model accepted, and the model's expected threads.
struct Run {
    records: Vec<VerifiedRecord>,
    accepted: BTreeSet<CommentRecordId>,
    expected: Vec<FoldedThread>,
}

fn simulate(acts: &[Act]) -> Run {
    let mut comments: Vec<SimComment> = Vec::new();
    let mut threads: Vec<SimThread> = Vec::new();
    let mut records: Vec<VerifiedRecord> = Vec::new();
    let mut accepted: BTreeSet<CommentRecordId> = BTreeSet::new();
    let mut edits_of: BTreeMap<u8, Vec<CommentRecordId>> = BTreeMap::new();
    let mut next_comment = 0u8;
    for a in acts {
        let i = records.len();
        let at = (i64::try_from(i).unwrap() + 1) * 10;
        let role = role_of(a.actor);
        let mut push = |r: VerifiedRecord, legal: bool| {
            if legal {
                accepted.insert(r.record.record_id);
            }
            records.push(r);
        };
        match a.kind {
            0 => {
                let c = next_comment;
                next_comment += 1;
                let legal = role.can_comment();
                push(
                    record(
                        i,
                        comment(c),
                        thread(c),
                        CommentOp::Post {
                            anchor: None,
                            body: text(i),
                        },
                        a.actor,
                        a.actor,
                        role,
                        0,
                        at,
                    ),
                    legal,
                );
                if legal {
                    comments.push(SimComment {
                        id: c,
                        thread: c,
                        author: a.actor,
                        body: i,
                        at,
                        edited_at: None,
                        deleted: false,
                        edits: 0,
                    });
                    threads.push(SimThread {
                        root: c,
                        resolved_by: None,
                        rev: 0,
                    });
                }
            }
            1 if !threads.is_empty() => {
                let t = threads[usize::from(a.target) % threads.len()].root;
                let c = next_comment;
                next_comment += 1;
                let legal = role.can_comment();
                push(
                    record(
                        i,
                        comment(c),
                        thread(t),
                        CommentOp::Post {
                            anchor: None,
                            body: text(i),
                        },
                        a.actor,
                        a.actor,
                        role,
                        0,
                        at,
                    ),
                    legal,
                );
                if legal {
                    comments.push(SimComment {
                        id: c,
                        thread: t,
                        author: a.actor,
                        body: i,
                        at,
                        edited_at: None,
                        deleted: false,
                        edits: 0,
                    });
                }
            }
            2 if !comments.is_empty() => {
                let index = usize::from(a.target) % comments.len();
                let c = &mut comments[index];
                let actor = if a.flag { a.actor } else { c.author };
                let role = role_of(actor);
                let legal = actor == c.author && role.can_comment() && !c.deleted;
                let rec = record(
                    i,
                    comment(c.id),
                    thread(c.thread),
                    CommentOp::Edit { body: text(i) },
                    actor,
                    actor,
                    role,
                    c.edits + 1,
                    at,
                );
                edits_of.entry(c.id).or_default().push(rec.record.record_id);
                push(rec, legal);
                if legal {
                    c.body = i;
                    c.edited_at = Some(at);
                    c.edits += 1;
                }
            }
            3 if !comments.is_empty() => {
                let index = usize::from(a.target) % comments.len();
                let c = &mut comments[index];
                let actor = if a.flag { a.actor } else { c.author };
                let role = role_of(actor);
                let legal = (actor == c.author || role.is_owner()) && role.can_comment();
                push(
                    record(
                        i,
                        comment(c.id),
                        thread(c.thread),
                        CommentOp::Delete,
                        actor,
                        actor,
                        role,
                        c.edits + 1,
                        at,
                    ),
                    legal,
                );
                if legal {
                    c.deleted = true;
                }
            }
            4 if !threads.is_empty() => {
                let index = usize::from(a.target) % threads.len();
                let author = comments
                    .iter()
                    .find(|c| c.id == threads[index].root)
                    .unwrap()
                    .author;
                let actor = if a.flag { a.actor } else { author };
                let role = role_of(actor);
                let resolved = a.target % 2 == 0;
                let t = &mut threads[index];
                let legal = (actor == author || role.can_edit()) && role.can_comment();
                push(
                    record(
                        i,
                        comment(t.root),
                        thread(t.root),
                        CommentOp::Resolve { resolved },
                        actor,
                        actor,
                        role,
                        t.rev + 1,
                        at,
                    ),
                    legal,
                );
                if legal {
                    t.rev += 1;
                    t.resolved_by = resolved.then_some(actor);
                }
            }
            _ => {}
        }
    }
    // An edit of a comment that ends up deleted is not a record that counts (a delete is final).
    for c in comments.iter().filter(|c| c.deleted) {
        if let Some(ids) = edits_of.get(&c.id) {
            for id in ids {
                accepted.remove(id);
            }
        }
    }
    // The model's threads, as `CommentList` would show them.
    let mut expected = Vec::new();
    for t in &threads {
        let mut shown: Vec<FoldedComment> = comments
            .iter()
            .filter(|c| c.thread == t.root)
            .map(|c| FoldedComment {
                comment_id: comment(c.id),
                thread_id: thread(c.thread),
                author: account(c.author),
                body: if c.deleted { Vec::new() } else { text(c.body) },
                created_at: c.at,
                edited_at: if c.deleted { None } else { c.edited_at },
                deleted: c.deleted,
            })
            .collect();
        let mut later_live = false;
        let mut keep = vec![true; shown.len()];
        for (index, c) in shown.iter().enumerate().rev() {
            keep[index] = !c.deleted || later_live;
            later_live |= !c.deleted;
        }
        let mut keep = keep.into_iter();
        shown.retain(|_| keep.next().unwrap());
        if shown.is_empty() {
            continue;
        }
        expected.push(FoldedThread {
            thread_id: thread(t.root),
            page_id: page(),
            anchor: None,
            resolved: t.resolved_by.is_some(),
            resolved_by: t.resolved_by.map(account),
            comments: shown,
        });
    }
    Run {
        records,
        accepted,
        expected,
    }
}

fn check_honest(acts: &[Act], seed: u64) {
    let run = simulate(acts);
    for round in 0..5u64 {
        let order = if round == 0 {
            run.records.clone()
        } else {
            shuffled(&run.records, seed.wrapping_add(round * 104_729))
        };
        let stream = fold(&order);
        assert_eq!(stream.threads(), run.expected, "round {round}");
        assert_eq!(
            stream.valid_record_ids(),
            run.accepted,
            "the records that count, round {round}"
        );
        // `thread` answers for a thread whose comments are all deleted too.
        let mut roots: BTreeSet<u8> = BTreeSet::new();
        for v in &run.records {
            if v.record.is_root() && v.author_role.can_comment() {
                roots.insert(v.record.comment_id.to_bytes()[1]);
            }
        }
        for n in roots {
            let answer = stream.thread(thread(n)).unwrap();
            match run.expected.iter().find(|t| t.thread_id == thread(n)) {
                Some(listed) => assert_eq!(&answer, listed),
                None => assert!(answer.comments.is_empty()),
            }
        }
        // Reading a body is reading the model's text.
        for t in &stream.threads() {
            for c in &t.comments {
                assert!(c.deleted || text_of(&c.body).starts_with('b'));
            }
        }
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn honest_and_refused_actions_fold_like_the_sequential_model_in_any_arrival_order(
        acts in proptest::collection::vec(act(), 1..60),
        seed in any::<u64>(),
    ) {
        check_honest(&acts, seed);
    }
}

/// Guards the strength of the honest property test: the model's threads are not nearly empty.
#[test]
fn honest_runs_make_threads_with_replies_edits_deletes_and_resolves() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(5),
        failure_persistence: None,
        ..Config::default()
    });
    let (mut threads, mut replies, mut edited, mut deleted, mut resolved, mut refused) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for _ in 0..64 {
        let acts = proptest::collection::vec(act(), 30..60)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let run = simulate(&acts);
        threads += run.expected.len();
        for t in &run.expected {
            replies += t.comments.len() - 1;
            edited += t.comments.iter().filter(|c| c.edited_at.is_some()).count();
            deleted += t.comments.iter().filter(|c| c.deleted).count();
            resolved += usize::from(t.resolved);
        }
        refused += run.records.len() - run.accepted.len();
    }
    assert!(threads >= 150, "{threads} threads");
    assert!(replies >= 150, "{replies} replies");
    assert!(edited >= 50, "{edited} edited comments");
    assert!(deleted >= 10, "{deleted} placeholders");
    assert!(resolved >= 30, "{resolved} resolved threads");
    assert!(refused >= 200, "{refused} refused or dropped records");
}
