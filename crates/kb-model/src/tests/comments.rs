//! The fold of a page's comment stream (M13 slice 2, ADR-0048), rule by rule. The property tests
//! of the fold (any arrival order, random authors and roles) are `tests/comment_fold.rs`; the codec
//! tests are in `comments.rs`.
//!
//! Every record here is a `VerifiedRecord`: the core has checked its signature, its author account
//! and its page, and read the role its author held at the carrying item's cursor. Records are named
//! by small numbers (`rec`, the record id; comment, thread and account numbers).

#![allow(clippy::too_many_arguments)]

use std::collections::BTreeSet;

use kb_types::comments::{COMMENT_BODY_MAX_BYTES, COMMENT_QUOTE_MAX_CHARS};
use kb_types::{
    AccountId, CommentAnchor, CommentBodyItem, CommentId, CommentRecordId, DeviceId, MemberRole,
    PageId, ThreadId,
};

use crate::{
    CommentOp, CommentRecord, CommentStream, FoldedComment, FoldedThread, SIGNATURE_LEN,
    VerifiedRecord, snapshot_dominates, snapshot_record_ids,
};

const V: MemberRole = MemberRole::Viewer;
const C: MemberRole = MemberRole::Commenter;
const E: MemberRole = MemberRole::Editor;
const O: MemberRole = MemberRole::Owner;

fn page() -> PageId {
    PageId::from_bytes([0xaa; 16])
}

fn who(n: u8) -> AccountId {
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

fn rid(n: u8) -> CommentRecordId {
    CommentRecordId::from_bytes([0xe0, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n])
}

fn body(text: &str) -> Vec<CommentBodyItem> {
    vec![CommentBodyItem::Text { text: text.into() }]
}

fn make(
    rec: u8,
    comment_n: u8,
    thread_n: u8,
    op: CommentOp,
    author: u8,
    role: MemberRole,
    rev: u32,
    at: i64,
) -> VerifiedRecord {
    VerifiedRecord {
        record: CommentRecord {
            record_id: rid(rec),
            page_id: page(),
            thread_id: thread(thread_n),
            comment_id: comment(comment_n),
            op,
            author: who(author),
            device: device(author),
            rev,
            at,
            sig: [0; SIGNATURE_LEN],
        },
        author_role: role,
    }
}

/// A root comment: thread and comment are `comment_n`.
fn root(
    rec: u8,
    comment_n: u8,
    author: u8,
    role: MemberRole,
    at: i64,
    text: &str,
) -> VerifiedRecord {
    make(
        rec,
        comment_n,
        comment_n,
        CommentOp::Post {
            anchor: None,
            body: body(text),
        },
        author,
        role,
        0,
        at,
    )
}

/// A reply `comment_n` in the thread of root `thread_n`.
fn reply(
    rec: u8,
    comment_n: u8,
    thread_n: u8,
    author: u8,
    role: MemberRole,
    at: i64,
    text: &str,
) -> VerifiedRecord {
    make(
        rec,
        comment_n,
        thread_n,
        CommentOp::Post {
            anchor: None,
            body: body(text),
        },
        author,
        role,
        0,
        at,
    )
}

/// An edit of `comment_n` (in the thread of `thread_n`).
fn edit(
    rec: u8,
    comment_n: u8,
    thread_n: u8,
    author: u8,
    role: MemberRole,
    rev: u32,
    at: i64,
    text: &str,
) -> VerifiedRecord {
    make(
        rec,
        comment_n,
        thread_n,
        CommentOp::Edit { body: body(text) },
        author,
        role,
        rev,
        at,
    )
}

fn delete(
    rec: u8,
    comment_n: u8,
    thread_n: u8,
    author: u8,
    role: MemberRole,
    at: i64,
) -> VerifiedRecord {
    make(
        rec,
        comment_n,
        thread_n,
        CommentOp::Delete,
        author,
        role,
        1,
        at,
    )
}

/// A resolve (or reopen) of the thread of root `thread_n`; its comment is the root.
fn resolve(
    rec: u8,
    thread_n: u8,
    author: u8,
    role: MemberRole,
    rev: u32,
    at: i64,
    resolved: bool,
) -> VerifiedRecord {
    make(
        rec,
        thread_n,
        thread_n,
        CommentOp::Resolve { resolved },
        author,
        role,
        rev,
        at,
    )
}

fn with_device(mut verified: VerifiedRecord, n: u8) -> VerifiedRecord {
    verified.record.device = device(n);
    verified
}

fn stream(records: &[VerifiedRecord]) -> CommentStream {
    CommentStream::from_records(page(), records.iter().cloned())
}

fn text_of(items: &[CommentBodyItem]) -> String {
    items
        .iter()
        .map(|item| match item {
            CommentBodyItem::Text { text } => text.clone(),
            other => format!("<{other:?}>"),
        })
        .collect()
}

/// `(comment number, body text, deleted)` of a thread's comments, in order.
fn shown(thread: &FoldedThread) -> Vec<(u8, String, bool)> {
    thread
        .comments
        .iter()
        .map(|c| (number(c.comment_id), text_of(&c.body), c.deleted))
        .collect()
}

fn number(id: CommentId) -> u8 {
    id.to_bytes()[1]
}

fn thread_numbers(stream: &CommentStream) -> Vec<u8> {
    stream
        .threads()
        .iter()
        .map(|t| number(t.thread_id.root_comment()))
        .collect()
}

fn ids(stream: &CommentStream) -> Vec<u8> {
    stream
        .valid_record_ids()
        .iter()
        .map(|id| id.to_bytes()[1])
        .collect()
}

fn comment_of(stream: &CommentStream, thread_n: u8, comment_n: u8) -> FoldedComment {
    stream
        .thread(thread(thread_n))
        .unwrap()
        .comments
        .into_iter()
        .find(|c| c.comment_id == comment(comment_n))
        .unwrap()
}

/// Applies the records in every order of a few fixed permutations and asserts the answers agree.
fn assert_any_order(records: &[VerifiedRecord]) -> CommentStream {
    let base = stream(records);
    let mut reversed = records.to_vec();
    reversed.reverse();
    let mut rotated = records.to_vec();
    rotated.rotate_left(records.len() / 2);
    let mut interleaved: Vec<VerifiedRecord> = records.iter().step_by(2).cloned().collect();
    interleaved.extend(records.iter().skip(1).step_by(2).cloned());
    for other in [reversed, rotated, interleaved] {
        let other = stream(&other);
        assert_eq!(other.threads(), base.threads());
        assert_eq!(other.valid_record_ids(), base.valid_record_ids());
        assert_eq!(other.valid_records(), base.valid_records());
    }
    base
}

// ----- posts ---------------------------------------------------------------------------------

#[test]
fn a_post_needs_commenter_or_above_at_its_cursor() {
    for (role, counts) in [(V, false), (C, true), (E, true), (O, true)] {
        let s = stream(&[root(1, 1, 1, role, 100, "hello")]);
        assert_eq!(s.threads().len(), usize::from(counts), "{role:?}");
        assert_eq!(s.valid_record_ids().len(), usize::from(counts), "{role:?}");
        assert_eq!(s.comment_author(comment(1)).is_some(), counts, "{role:?}");
        assert_eq!(s.len(), 1, "the set holds it either way");
    }
}

#[test]
fn a_thread_is_built_from_its_root_and_its_replies_in_order() {
    let s = assert_any_order(&[
        reply(3, 3, 1, 3, C, 300, "third"),
        root(1, 1, 1, C, 100, "first"),
        reply(2, 2, 1, 2, E, 200, "second"),
    ]);
    let threads = s.threads();
    assert_eq!(threads.len(), 1);
    let t = &threads[0];
    assert_eq!(t.thread_id, thread(1));
    assert_eq!(t.page_id, page());
    assert!(!t.resolved);
    assert_eq!(t.resolved_by, None);
    assert_eq!(
        shown(t),
        [
            (1, "first".to_owned(), false),
            (2, "second".to_owned(), false),
            (3, "third".to_owned(), false),
        ]
    );
    assert_eq!(t.comments[0].author, who(1));
    assert_eq!(t.comments[0].created_at, 100);
    assert_eq!(t.comments[0].edited_at, None);
    assert_eq!(s.thread(thread(1)).as_ref(), Some(t));
}

#[test]
fn a_reply_without_a_valid_root_is_ignored_until_the_root_arrives() {
    let orphan = reply(2, 2, 1, 2, C, 200, "reply");
    let alone = stream(std::slice::from_ref(&orphan));
    assert!(alone.threads().is_empty());
    assert!(alone.thread(thread(1)).is_none());
    assert!(alone.valid_record_ids().is_empty());
    assert_eq!(alone.len(), 1, "kept in the set");

    // The root arrives later (or never counts: it was a Viewer's).
    let mut later = alone.clone();
    assert!(later.apply(root(1, 1, 1, C, 100, "root")));
    assert_eq!(ids(&later), [1, 2]);
    assert_eq!(later.threads()[0].comments.len(), 2);

    let mut viewer_root = alone;
    assert!(viewer_root.apply(root(1, 1, 1, V, 100, "root")));
    assert!(viewer_root.threads().is_empty());
    assert!(viewer_root.valid_record_ids().is_empty());
}

#[test]
fn a_reply_by_a_viewer_is_ignored_and_a_reply_may_not_open_a_thread_of_its_own() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, V, 200, "viewer"),
        // Names a thread that no root opened.
        reply(3, 3, 9, 3, C, 300, "lost"),
    ]);
    assert_eq!(ids(&s), [1]);
    assert_eq!(shown(&s.threads()[0]), [(1, "root".to_owned(), false)]);
    assert!(s.thread(thread(9)).is_none());
}

#[test]
fn a_reply_cannot_reach_a_comment_that_is_not_a_thread_root() {
    // Comment 2 is a reply in thread 1; a reply naming thread 2 would need comment 2 to be a root.
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "reply"),
        reply(3, 3, 2, 3, C, 300, "nested"),
    ]);
    assert_eq!(ids(&s), [1, 2]);
    assert!(s.thread(thread(2)).is_none());
}

#[test]
fn bodies_are_checked_again_the_fold_never_counts_an_empty_or_oversized_one() {
    let big = "x".repeat(COMMENT_BODY_MAX_BYTES);
    let s = stream(&[
        root(1, 1, 1, C, 100, "fine"),
        root(2, 2, 2, C, 100, ""),
        root(3, 3, 3, C, 100, &big),
        edit(4, 1, 1, 1, C, 1, 200, ""),
        edit(5, 1, 1, 1, C, 2, 300, &big),
    ]);
    assert_eq!(ids(&s), [1]);
    assert_eq!(shown(&s.threads()[0]), [(1, "fine".to_owned(), false)]);
    let no_items = make(
        6,
        4,
        4,
        CommentOp::Post {
            anchor: None,
            body: Vec::new(),
        },
        1,
        C,
        0,
        10,
    );
    assert!(stream(&[no_items]).threads().is_empty());
}

#[test]
fn a_second_post_for_a_comment_id_loses_to_the_earliest_and_never_reaches_a_snapshot() {
    // The same comment id from two authors: the earliest claim by (at, record id) is the comment.
    let honest = root(1, 1, 1, C, 100, "mine");
    let late = root(2, 1, 2, C, 150, "taken over");
    let same_instant = root(3, 1, 3, C, 100, "same instant, higher record id");
    let s = assert_any_order(&[late.clone(), honest.clone()]);
    assert_eq!(s.comment_author(comment(1)), Some(who(1)));
    assert_eq!(shown(&s.threads()[0]), [(1, "mine".to_owned(), false)]);
    assert_eq!(ids(&s), [1], "the loser is not a valid record");
    assert!(s.contains(rid(2)), "but it stays in the set");

    // At the same instant the lower record id wins, whatever the arrival order.
    let tie = assert_any_order(&[same_instant, honest]);
    assert_eq!(tie.comment_author(comment(1)), Some(who(1)));
    assert_eq!(ids(&tie), [1]);
}

#[test]
fn a_reply_never_predates_its_root_and_an_edit_never_predates_its_comment() {
    let s = stream(&[
        root(1, 1, 1, C, 1_000, "root"),
        // Backdated: it claims to be older than the thread it answers.
        reply(2, 2, 1, 2, C, 10, "early bird"),
        reply(3, 3, 1, 3, C, 1_500, "later"),
        edit(4, 2, 1, 2, C, 1, 5, "still early"),
    ]);
    let t = &s.threads()[0];
    assert_eq!(
        shown(t).iter().map(|c| c.0).collect::<Vec<_>>(),
        [1, 2, 3],
        "the root first, then oldest first"
    );
    assert_eq!(t.comments[1].created_at, 1_000, "clamped to the root's");
    assert_eq!(t.comments[1].edited_at, Some(1_000), "and so is the edit");
    assert_eq!(t.comments[2].created_at, 1_500);
    // The record itself is untouched: it still counts, as it was written.
    assert_eq!(ids(&s), [1, 2, 3, 4]);
    assert_eq!(s.valid_records()[1].at, 10);
}

#[test]
fn threads_are_listed_oldest_root_first_then_by_thread_id() {
    let s = assert_any_order(&[
        root(1, 3, 1, C, 300, "c"),
        root(2, 1, 1, C, 100, "a"),
        root(3, 5, 1, C, 300, "d"),
        root(4, 2, 1, C, 200, "b"),
    ]);
    assert_eq!(thread_numbers(&s), [1, 2, 3, 5]);
}

// ----- edits ---------------------------------------------------------------------------------

#[test]
fn only_the_author_edits_a_comment() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "mine"),
        edit(2, 1, 1, 2, C, 1, 200, "a commenter's"),
        edit(3, 1, 1, 3, E, 1, 210, "an editor's"),
        edit(4, 1, 1, 4, O, 1, 220, "an owner's"),
    ]);
    assert_eq!(ids(&s), [1]);
    assert_eq!(shown(&s.threads()[0]), [(1, "mine".to_owned(), false)]);
    assert_eq!(s.threads()[0].comments[0].edited_at, None);
}

#[test]
fn an_edit_takes_the_comment_s_body_and_time() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "first draft"),
        edit(2, 1, 1, 1, C, 1, 250, "second draft"),
    ]);
    let c = comment_of(&s, 1, 1);
    assert_eq!(text_of(&c.body), "second draft");
    assert_eq!(c.created_at, 100);
    assert_eq!(c.edited_at, Some(250));
    assert_eq!(ids(&s), [1, 2]);
}

#[test]
fn the_last_rev_at_device_wins() {
    // A higher rev wins even when it claims an earlier time.
    let s = assert_any_order(&[
        root(1, 1, 1, C, 100, "v0"),
        edit(2, 1, 1, 1, C, 1, 900, "rev 1, late"),
        edit(3, 1, 1, 1, C, 2, 300, "rev 2, early"),
    ]);
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "rev 2, early");

    // The same rev: the later time wins.
    let s = assert_any_order(&[
        root(1, 1, 1, C, 100, "v0"),
        edit(2, 1, 1, 1, C, 1, 300, "earlier"),
        edit(3, 1, 1, 1, C, 1, 400, "later"),
    ]);
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "later");

    // The same rev and time on two devices of the author: the greater device id wins.
    let s = assert_any_order(&[
        root(1, 1, 1, C, 100, "v0"),
        with_device(edit(2, 1, 1, 1, C, 1, 300, "from device 2"), 2),
        with_device(edit(3, 1, 1, 1, C, 1, 300, "from device 9"), 9),
    ]);
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "from device 9");

    // Down to the record id when even the device is the same.
    let s = assert_any_order(&[
        root(1, 1, 1, C, 100, "v0"),
        edit(2, 1, 1, 1, C, 1, 300, "record 2"),
        edit(3, 1, 1, 1, C, 1, 300, "record 3"),
    ]);
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "record 3");
    // Every valid edit stays in the set a snapshot lists, the superseded ones too.
    assert_eq!(ids(&s), [1, 2, 3]);
}

#[test]
fn an_edit_that_arrives_before_its_post_counts_once_the_post_is_there() {
    let mut s = CommentStream::new(page());
    assert!(s.apply(edit(2, 1, 1, 1, C, 1, 200, "edited")));
    assert!(s.threads().is_empty());
    assert!(s.valid_record_ids().is_empty());
    assert!(s.apply(root(1, 1, 1, C, 100, "original")));
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "edited");
    assert_eq!(ids(&s), [1, 2]);
}

#[test]
fn an_edit_names_its_comment_s_thread() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        root(2, 2, 2, C, 100, "other"),
        // Right comment, wrong thread.
        edit(3, 1, 2, 1, C, 1, 200, "wrong thread"),
        // The author as a Viewer (demoted) can no longer write.
        edit(4, 1, 1, 1, V, 2, 300, "demoted"),
    ]);
    assert_eq!(ids(&s), [1, 2]);
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "root");
}

#[test]
fn records_written_while_the_role_allowed_them_stay_valid_after_a_demotion() {
    // Records carry the role at their own cursor: the post and first edit were a Commenter's,
    // the later edit a Viewer's (refused), and the first two still count.
    let s = stream(&[
        root(1, 1, 1, C, 100, "written as a commenter"),
        edit(2, 1, 1, 1, C, 1, 200, "edited as a commenter"),
        edit(3, 1, 1, 1, V, 2, 300, "edited as a viewer"),
    ]);
    assert_eq!(ids(&s), [1, 2]);
    assert_eq!(text_of(&comment_of(&s, 1, 1).body), "edited as a commenter");
}

// ----- deletes -------------------------------------------------------------------------------

#[test]
fn the_author_and_an_owner_delete_a_comment_and_nobody_else() {
    for (author, role, counts) in [
        (1, C, true),  // the author
        (1, E, true),  // the author, promoted since
        (2, C, false), // another commenter
        (2, E, false), // an editor
        (2, O, true),  // an owner
        (2, V, false), // a viewer
    ] {
        let s = stream(&[
            root(1, 1, 1, C, 100, "doomed"),
            reply(2, 2, 1, 3, C, 150, "stays"),
            delete(3, 1, 1, author, role, 200),
        ]);
        let t = s.thread(thread(1)).unwrap();
        assert_eq!(t.comments[0].deleted, counts, "author {author} as {role:?}");
        assert_eq!(ids(&s).contains(&3), counts, "author {author} as {role:?}");
    }
}

#[test]
fn a_deleted_comment_reads_as_a_placeholder_while_a_later_comment_stands() {
    let s = assert_any_order(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "reply"),
        delete(3, 1, 1, 1, C, 300),
    ]);
    let t = &s.threads()[0];
    assert_eq!(
        shown(t),
        [(1, String::new(), true), (2, "reply".to_owned(), false)],
        "the root is a placeholder with no body"
    );
    assert_eq!(t.comments[0].author, who(1), "still known");
    assert_eq!(s.comment_author(comment(1)), Some(who(1)));
    assert_eq!(t.comments[0].edited_at, None);
}

#[test]
fn a_deleted_comment_with_no_later_comment_standing_is_dropped() {
    // A deleted reply at the end goes; the root stays.
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "reply"),
        delete(3, 2, 1, 2, C, 300),
    ]);
    assert_eq!(shown(&s.threads()[0]), [(1, "root".to_owned(), false)]);

    // A deleted comment in the middle stays while a later one is live.
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "gone"),
        reply(4, 3, 1, 3, C, 250, "after"),
        delete(3, 2, 1, 2, C, 300),
    ]);
    assert_eq!(
        shown(&s.threads()[0]),
        [
            (1, "root".to_owned(), false),
            (2, String::new(), true),
            (3, "after".to_owned(), false)
        ]
    );

    // Everything deleted: the thread leaves the list, and `thread` answers with no comments.
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "reply"),
        delete(3, 1, 1, 1, C, 300),
        delete(4, 2, 1, 2, C, 310),
    ]);
    assert!(s.threads().is_empty());
    let answer = s.thread(thread(1)).unwrap();
    assert!(answer.comments.is_empty());
    assert_eq!(answer.thread_id, thread(1));
}

#[test]
fn a_delete_is_final_edits_of_the_comment_are_ignored_whatever_their_order() {
    let records = [
        root(1, 1, 1, C, 100, "original"),
        edit(2, 1, 1, 1, C, 1, 150, "before the delete"),
        delete(3, 1, 1, 9, O, 200),
        edit(4, 1, 1, 1, C, 2, 300, "after the delete"),
        edit(5, 1, 1, 1, C, u32::MAX, i64::MAX, "much later"),
        reply(6, 2, 1, 2, C, 250, "keeps the thread"),
    ];
    let s = assert_any_order(&records);
    let c = comment_of(&s, 1, 1);
    assert!(c.deleted);
    assert!(c.body.is_empty());
    assert_eq!(c.edited_at, None);
    assert_eq!(
        ids(&s),
        [1, 3, 6],
        "the edits of a deleted comment are not valid records"
    );

    // Delivering the delete first changes nothing.
    let mut late = CommentStream::new(page());
    for record in records.iter().rev() {
        late.apply(record.clone());
    }
    assert_eq!(late.threads(), s.threads());
}

#[test]
fn deleting_twice_is_still_a_delete() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "doomed"),
        reply(2, 2, 1, 2, C, 110, "stays"),
        delete(3, 1, 1, 1, C, 200),
        delete(4, 1, 1, 9, O, 210),
    ]);
    assert!(comment_of(&s, 1, 1).deleted);
    assert_eq!(ids(&s), [1, 2, 3, 4]);
}

#[test]
fn a_delete_names_the_right_thread_and_an_existing_comment() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        root(2, 2, 2, C, 100, "other"),
        delete(3, 1, 2, 1, C, 200),
        delete(4, 7, 7, 1, O, 200),
    ]);
    assert_eq!(ids(&s), [1, 2]);
    assert!(!comment_of(&s, 1, 1).deleted);
}

// ----- resolve -------------------------------------------------------------------------------

#[test]
fn the_thread_author_and_editors_and_up_resolve_a_thread() {
    for (author, role, counts) in [
        (1, C, true),  // the thread's author
        (2, C, false), // another commenter
        (2, E, true),
        (2, O, true),
        (2, V, false),
        (1, V, false), // the thread's author, as a viewer now
    ] {
        let s = stream(&[
            root(1, 1, 1, C, 100, "root"),
            resolve(2, 1, author, role, 1, 200, true),
        ]);
        let t = s.thread(thread(1)).unwrap();
        assert_eq!(t.resolved, counts, "author {author} as {role:?}");
        assert_eq!(
            t.resolved_by,
            counts.then(|| who(author)),
            "author {author} as {role:?}"
        );
        assert_eq!(ids(&s).contains(&2), counts);
    }
}

#[test]
fn the_last_resolve_wins_and_reopening_clears_who_resolved() {
    let records = [
        root(1, 1, 1, C, 100, "root"),
        resolve(2, 1, 5, E, 1, 200, true),
        resolve(3, 1, 1, C, 2, 300, false),
        resolve(4, 1, 6, O, 3, 400, true),
        resolve(5, 1, 1, C, 3, 390, false),
    ];
    let s = assert_any_order(&records);
    let t = s.thread(thread(1)).unwrap();
    assert!(t.resolved);
    assert_eq!(t.resolved_by, Some(who(6)));

    // Without the last two the thread is open again.
    let s = assert_any_order(&records[..3]);
    let t = s.thread(thread(1)).unwrap();
    assert!(!t.resolved);
    assert_eq!(t.resolved_by, None);
    assert_eq!(ids(&s), [1, 2, 3], "every valid resolve is kept");
}

#[test]
fn a_resolve_names_the_root_comment_of_its_thread() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 150, "reply"),
        // The thread is right, the comment is not the root.
        make(3, 2, 1, CommentOp::Resolve { resolved: true }, 1, C, 1, 200),
        // No such thread.
        resolve(4, 8, 1, E, 1, 200, true),
    ]);
    assert!(!s.thread(thread(1)).unwrap().resolved);
    assert_eq!(ids(&s), [1, 2]);
}

#[test]
fn a_deleted_root_does_not_freeze_its_thread() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 150, "reply"),
        delete(3, 1, 1, 1, C, 200),
        resolve(4, 1, 5, E, 1, 300, true),
    ]);
    let t = s.threads().remove(0);
    assert!(t.resolved);
    assert_eq!(t.resolved_by, Some(who(5)));
    assert!(t.comments[0].deleted);
}

// ----- viewers and the set ---------------------------------------------------------------

#[test]
fn a_viewer_s_records_never_count_whatever_the_op() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 7, V, 110, "viewer reply"),
        edit(3, 1, 1, 1, V, 1, 120, "viewer edit"),
        delete(4, 1, 1, 1, V, 130),
        resolve(5, 1, 1, V, 1, 140, true),
        // A Viewer cannot delete even as the Owner's namesake: the role is what counts.
        delete(6, 1, 1, 9, V, 150),
    ]);
    assert_eq!(ids(&s), [1]);
    let t = s.thread(thread(1)).unwrap();
    assert!(!t.resolved);
    assert_eq!(shown(&t), [(1, "root".to_owned(), false)]);
}

#[test]
fn the_set_keeps_what_the_rules_ignore_and_lists_only_what_counts() {
    let s = stream(&[
        edit(5, 1, 1, 1, C, 1, 200, "early edit"),
        root(1, 1, 1, C, 100, "root"),
        root(2, 2, 2, V, 100, "viewer"),
        delete(4, 1, 1, 2, C, 300),
    ]);
    assert_eq!(s.len(), 4);
    assert_eq!(ids(&s), [1, 5]);
    let listed: Vec<CommentRecordId> = s.valid_records().iter().map(|r| r.record_id).collect();
    assert_eq!(listed, [rid(1), rid(5)], "by record id");
    // Every record of the set is reachable, valid or not.
    assert_eq!(s.records().count(), 4);
    assert!(s.contains(rid(2)) && s.contains(rid(4)));
}

#[test]
fn a_snapshot_of_the_valid_records_folds_to_the_same_threads() {
    let records = [
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, E, 200, "reply"),
        edit(3, 2, 1, 2, E, 1, 250, "reply, edited"),
        edit(4, 2, 1, 1, C, 1, 260, "not the author"),
        root(5, 3, 3, V, 300, "viewer thread"),
        delete(6, 1, 1, 1, C, 400),
        resolve(7, 1, 4, O, 1, 500, true),
        resolve(8, 1, 2, C, 2, 600, false),
    ];
    let full = stream(&records);
    assert_eq!(ids(&full), [1, 2, 3, 6, 7]);

    // What a snapshot carries: the valid records; the receiver reads each author's role from its own
    // timeline, which here is the role the record was checked with.
    let valid: Vec<VerifiedRecord> = full
        .valid_records()
        .iter()
        .map(|r| {
            records
                .iter()
                .find(|v| v.record.record_id == r.record_id)
                .unwrap()
                .clone()
        })
        .collect();
    let from_snapshot = stream(&valid);
    assert_eq!(from_snapshot.threads(), full.threads());
    assert_eq!(from_snapshot.valid_record_ids(), full.valid_record_ids());
    assert_eq!(from_snapshot.len(), 5, "nothing ignored is left in it");
}

#[test]
fn only_this_page_s_records_are_added_and_a_record_id_counts_once() {
    let mut s = CommentStream::new(page());
    let one = root(1, 1, 1, C, 100, "one");
    assert!(s.apply(one.clone()));
    // The same record id with other content does not replace the first.
    let mut impostor = root(1, 1, 2, C, 50, "impostor");
    impostor.record.record_id = one.record.record_id;
    assert!(!s.apply(impostor));
    assert_eq!(s.comment_author(comment(1)), Some(who(1)));
    let mut elsewhere = root(2, 2, 2, C, 100, "elsewhere");
    elsewhere.record.page_id = PageId::from_bytes([0xbb; 16]);
    assert!(!s.apply(elsewhere));
    assert_eq!(s.len(), 1);
}

#[test]
fn a_comment_id_already_defined_is_visible_to_the_core_before_it_applies_a_duplicate() {
    let mut s = CommentStream::new(page());
    assert!(!s.defines_comment(comment(1)));
    s.apply(root(1, 1, 1, C, 100, "one"));
    assert!(s.defines_comment(comment(1)));
    assert!(!s.defines_comment(comment(2)));
    // Records that are not posts do not define one, and a post counts even while it is ignored.
    s.apply(edit(2, 2, 2, 1, C, 1, 100, "edit"));
    s.apply(root(3, 3, 3, V, 100, "viewer"));
    assert!(!s.defines_comment(comment(2)));
    assert!(s.defines_comment(comment(3)));
}

// ----- anchors -------------------------------------------------------------------------------

fn anchored(rec: u8, comment_n: u8, anchor: CommentAnchor) -> VerifiedRecord {
    make(
        rec,
        comment_n,
        comment_n,
        CommentOp::Post {
            anchor: Some(anchor),
            body: body("anchored"),
        },
        1,
        C,
        0,
        100,
    )
}

#[test]
fn a_thread_keeps_the_anchor_of_its_root() {
    let anchor = CommentAnchor {
        block_id: Some(kb_types::BlockId::from_bytes([6; 16])),
        start: Some("AQID".into()),
        end: Some("BAUG".into()),
        quote: "the plan".into(),
    };
    let s = stream(&[anchored(1, 1, anchor.clone())]);
    assert_eq!(s.threads()[0].anchor, Some(anchor));
    let s = stream(&[root(1, 1, 1, C, 100, "page level")]);
    assert_eq!(s.threads()[0].anchor, None);
}

#[test]
fn an_anchor_that_cannot_be_resolved_stays_a_page_level_thread_with_its_quote() {
    let long: String = "q".repeat(COMMENT_QUOTE_MAX_CHARS + 50);
    let bad_cursor = CommentAnchor {
        block_id: Some(kb_types::BlockId::from_bytes([6; 16])),
        start: Some("not base64!".into()),
        end: None,
        quote: "the words".into(),
    };
    let too_long = CommentAnchor {
        block_id: Some(kb_types::BlockId::from_bytes([6; 16])),
        start: None,
        end: None,
        quote: long.clone(),
    };
    let s = stream(&[anchored(1, 1, bad_cursor), anchored(2, 2, too_long)]);
    let threads = s.threads();
    assert_eq!(threads.len(), 2, "the threads stay");
    assert_eq!(
        threads[0].anchor,
        Some(CommentAnchor {
            block_id: None,
            start: None,
            end: None,
            quote: "the words".into(),
        })
    );
    assert_eq!(
        threads[1].anchor.as_ref().map(|a| a.quote.chars().count()),
        Some(COMMENT_QUOTE_MAX_CHARS)
    );
    assert_eq!(threads[1].anchor.as_ref().and_then(|a| a.block_id), None);
    assert_eq!(ids(&s), [1, 2], "and so do their records");
}

#[test]
fn a_reply_shows_no_anchor() {
    let reply_with_anchor = make(
        2,
        2,
        1,
        CommentOp::Post {
            anchor: Some(CommentAnchor {
                block_id: None,
                start: None,
                end: None,
                quote: "ignored".into(),
            }),
            body: body("reply"),
        },
        2,
        C,
        0,
        200,
    );
    let s = stream(&[root(1, 1, 1, C, 100, "root"), reply_with_anchor]);
    assert_eq!(s.threads()[0].anchor, None);
    assert_eq!(s.threads()[0].comments.len(), 2);
}

// ----- the answer views ----------------------------------------------------------------------

#[test]
fn comment_author_names_the_post_s_author_even_after_a_delete() {
    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 150, "reply"),
        delete(3, 2, 1, 2, C, 200),
    ]);
    assert_eq!(s.comment_author(comment(1)), Some(who(1)));
    assert_eq!(s.comment_author(comment(2)), Some(who(2)));
    assert_eq!(s.comment_author(comment(3)), None);
}

#[test]
fn mentions_and_page_items_pass_through_the_fold_untouched() {
    let rich = vec![
        CommentBodyItem::Text {
            text: "ping ".into(),
        },
        CommentBodyItem::Mention { account_id: who(2) },
        CommentBodyItem::Page {
            id: PageId::from_bytes([9; 16]),
        },
    ];
    let record = make(
        1,
        1,
        1,
        CommentOp::Post {
            anchor: None,
            body: rich.clone(),
        },
        1,
        C,
        0,
        100,
    );
    assert_eq!(stream(&[record]).threads()[0].comments[0].body, rich);
}

// ----- snapshot dominance ----------------------------------------------------------------------

#[test]
fn a_snapshot_dominates_when_its_record_set_includes_every_record_seen_valid() {
    let seen: BTreeSet<CommentRecordId> = [rid(1), rid(2), rid(3)].into_iter().collect();
    let all: BTreeSet<CommentRecordId> = [rid(1), rid(2), rid(3), rid(4)].into_iter().collect();
    let truncated: BTreeSet<CommentRecordId> = [rid(1), rid(3), rid(4)].into_iter().collect();
    assert!(snapshot_dominates(&all, &seen));
    assert!(snapshot_dominates(&seen, &seen), "equal sets dominate");
    assert!(snapshot_dominates(&all, &BTreeSet::new()));
    assert!(
        !snapshot_dominates(&truncated, &seen),
        "a missing record id: refused"
    );
    assert!(!snapshot_dominates(&BTreeSet::new(), &seen));

    let s = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "reply"),
    ]);
    let ids_of = |ns: &[u8]| -> BTreeSet<CommentRecordId> { ns.iter().map(|n| rid(*n)).collect() };
    assert!(s.is_dominated_by(&ids_of(&[1, 2, 9])));
    assert!(s.is_dominated_by(&ids_of(&[1, 2])));
    assert!(!s.is_dominated_by(&ids_of(&[1])));
    // Ignored records are not owed to a snapshot.
    let with_viewer = stream(&[
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, V, 200, "ignored"),
    ]);
    assert!(with_viewer.is_dominated_by(&ids_of(&[1])));
}

#[test]
fn the_ids_of_a_snapshot_blob_are_read_from_its_records() {
    let records = [
        root(1, 1, 1, C, 100, "root"),
        reply(2, 2, 1, 2, C, 200, "reply"),
    ];
    let encoded: Vec<Vec<u8>> = records.iter().map(|v| v.record.encode()).collect();
    let slices: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    let blob = kb_doc::encode_records_snapshot(&slices);
    let decoded = kb_doc::decode_records(&blob).unwrap();
    let found = snapshot_record_ids(decoded.records.iter().copied()).unwrap();
    assert_eq!(found, [rid(1), rid(2)].into_iter().collect());

    // A record that is not one refuses the whole snapshot.
    let junk: Vec<&[u8]> = vec![encoded[0].as_slice(), &[1, 2, 3][..]];
    assert!(snapshot_record_ids(junk).is_err());
    // An empty snapshot has no ids.
    assert!(snapshot_record_ids(Vec::<&[u8]>::new()).unwrap().is_empty());
}
