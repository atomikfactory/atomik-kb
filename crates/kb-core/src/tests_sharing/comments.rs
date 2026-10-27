//! Milestone 13, slice 2: comments, person mentions and the Person property, on the people of the
//! simulator in `super` (accounts with devices on one `MemoryRemote`). The scenarios drive the requests
//! the way the UI does and read what the other people see after the cycles: roles on both sides,
//! forged records, snapshot dominance, the four notifications and their absence for one's own changes,
//! reminders per account, search, moving a page with its stream, backups, and the Person property.

use kb_model::{CommentOp, CommentRecord};
use kb_storage::CommentQueries;
use kb_types::{
    AccountId, CommentAnchor, CommentBodyItem, CommentId, CommentRecordId, NotificationInfo,
    NotificationKind, PersonInfo, PropertyFilter, PropertyId, PropertyKind, PropertyValue, Request,
    Response, SearchHit, SpaceShareState, ThreadId, ThreadInfo,
};
use serde_json::json;

use super::requests::*;
use super::scenarios::{forbidden_role, invalid_field};
use super::*;
use crate::comments::comment_stream_doc_id;

// ----- helpers ----------------------------------------------------------------------------------

/// One clock for every device of every test: a comment's `at` is its author's clock, and comments
/// sort by it, so a test moves the writer's clock to the next tick before it writes.
static WORLD: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(5_000_000);

fn tick(dev: &Dev) {
    dev.clock
        .set(WORLD.fetch_add(1_000, std::sync::atomic::Ordering::SeqCst) + 1_000);
}

fn text(words: &str) -> Vec<CommentBodyItem> {
    vec![CommentBodyItem::Text { text: words.into() }]
}

fn thread_of(response: Response) -> ThreadInfo {
    match response {
        Response::Thread(thread) => thread,
        other => panic!("unexpected {other:?}"),
    }
}

fn threads(dev: &mut Dev, page: PageId) -> Vec<ThreadInfo> {
    match call(dev, Request::CommentList { page_id: page }).unwrap() {
        Response::Comments(threads) => threads,
        other => panic!("{}: unexpected {other:?}", dev.name),
    }
}

fn post(
    dev: &mut Dev,
    page: PageId,
    thread: Option<ThreadId>,
    body: Vec<CommentBodyItem>,
) -> ThreadInfo {
    tick(dev);
    thread_of(
        call(
            dev,
            Request::CommentPost {
                page_id: page,
                thread_id: thread,
                anchor: None,
                body,
            },
        )
        .unwrap(),
    )
}

fn try_post(
    dev: &mut Dev,
    page: PageId,
    thread: Option<ThreadId>,
    anchor: Option<CommentAnchor>,
    body: Vec<CommentBodyItem>,
) -> Result<ThreadInfo, ApiError> {
    tick(dev);
    call(
        dev,
        Request::CommentPost {
            page_id: page,
            thread_id: thread,
            anchor,
            body,
        },
    )
    .map(thread_of)
}

fn edit(
    dev: &mut Dev,
    page: PageId,
    comment: CommentId,
    words: &str,
) -> Result<ThreadInfo, ApiError> {
    tick(dev);
    call(
        dev,
        Request::CommentEdit {
            page_id: page,
            comment_id: comment,
            body: text(words),
        },
    )
    .map(thread_of)
}

fn delete(dev: &mut Dev, page: PageId, comment: CommentId) -> Result<ThreadInfo, ApiError> {
    tick(dev);
    call(
        dev,
        Request::CommentDelete {
            page_id: page,
            comment_id: comment,
        },
    )
    .map(thread_of)
}

fn resolve(
    dev: &mut Dev,
    page: PageId,
    thread: ThreadId,
    resolved: bool,
) -> Result<ThreadInfo, ApiError> {
    tick(dev);
    call(
        dev,
        Request::CommentResolve {
            page_id: page,
            thread_id: thread,
            resolved,
        },
    )
    .map(thread_of)
}

/// The words of a comment (its text items).
fn words(comment: &kb_types::CommentInfo) -> String {
    comment
        .body
        .iter()
        .filter_map(|item| match item {
            CommentBodyItem::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn inbox(dev: &mut Dev) -> Vec<NotificationInfo> {
    match call(
        dev,
        Request::NotificationList {
            limit: None,
            before_id: None,
            unread_only: None,
        },
    )
    .unwrap()
    {
        Response::Notifications(rows) => rows,
        other => panic!("unexpected {other:?}"),
    }
}

fn kinds(rows: &[NotificationInfo]) -> Vec<NotificationKind> {
    rows.iter().map(|row| row.kind).collect()
}

/// Carol joins Alice's space with `role`.
fn add_carol(alice: &mut Dev, space: SpaceId, feed: FeedId, role: MemberRole) -> Dev {
    let server = alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    add_member(alice, &mut carol, feed, space, role, "Carol");
    carol
}

/// Appends portable blocks to a page's document the way an editor's save does, then persists.
fn append_blocks(dev: &mut Dev, page: PageId, blocks: serde_json::Value) {
    let core = &mut dev.core;
    let appended = core
        .open_page(page)
        .unwrap()
        .model
        .append_portable(&json!({ "type": "doc", "content": blocks }))
        .unwrap();
    assert!(appended > 0);
    core.persist_page(
        page,
        None,
        kb_storage::UpdateOrigin::Local,
        kb_types::ChangeSource::Core,
    )
    .unwrap();
    core.page_doc_close(page).unwrap();
}

fn block(n: u8) -> String {
    kb_types::BlockId::from_random_bytes([n; 16]).to_string()
}

fn create_person_property(dev: &mut Dev, space: SpaceId, name: &str) -> PropertyId {
    match call(
        dev,
        Request::PropertyCreate {
            space_id: space,
            name: name.into(),
            kind: PropertyKind::Person,
            config: None,
        },
    )
    .unwrap()
    {
        Response::Property(def) => def.id,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_people(
    dev: &mut Dev,
    page: PageId,
    property: PropertyId,
    people: &[AccountId],
) -> Result<Response, ApiError> {
    call(
        dev,
        Request::PagePropertySet {
            page_id: page,
            property_id: property,
            value: Some(PropertyValue::Person(people.to_vec())),
        },
    )
}

fn pages_by(
    dev: &mut Dev,
    space: SpaceId,
    property: PropertyId,
    filter: PropertyFilter,
) -> Vec<PageId> {
    match call(
        dev,
        Request::PagesByProperty {
            space_id: space,
            property_id: property,
            filter,
        },
    )
    .unwrap()
    {
        Response::PageIds(pages) => pages,
        other => panic!("unexpected {other:?}"),
    }
}

fn search(dev: &mut Dev, query: &str, space: Option<SpaceId>) -> Vec<SearchHit> {
    match call(
        dev,
        Request::Search {
            query: query.into(),
            space_id: space,
            scope: None,
            limit: None,
            edited_after: None,
        },
    )
    .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

/// A record this device signs with the fields the test chooses: what a hostile client sends.
fn forged(
    dev: &Dev,
    page: PageId,
    thread: ThreadId,
    comment: CommentId,
    op: CommentOp,
    rev: u32,
) -> CommentRecord {
    CommentRecord {
        record_id: CommentRecordId::from_bytes([0xF0 ^ rev as u8; 16]),
        page_id: page,
        thread_id: thread,
        comment_id: comment,
        op,
        author: dev.account(),
        device: dev.core.device.device_id,
        rev,
        at: dev.core.clock.now_ms() + 1,
        sig: [0; 64],
    }
}

// ----- roles ------------------------------------------------------------------------------------

#[test]
fn a_commenter_posts_replies_are_read_by_everyone_and_a_viewer_is_refused() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let mut carol = add_carol(&mut alice, space, feed, MemberRole::Viewer);
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);

    // Bob comments on a selection; the anchor travels with the thread.
    let anchor = CommentAnchor {
        block_id: None,
        start: None,
        end: None,
        quote: "base line".into(),
    };
    let posted = try_post(
        &mut bob,
        plan,
        None,
        Some(anchor.clone()),
        text("Looks good to me"),
    )
    .unwrap();
    assert_eq!(
        posted.thread_id,
        ThreadId::of(posted.comments[0].comment_id)
    );
    assert!(posted.comments[0].is_mine);
    Dev::settle(&mut [&mut bob, &mut alice, &mut carol], 3);

    let seen = threads(&mut alice, plan);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].anchor, Some(anchor));
    assert!(!seen[0].resolved);
    let comment = &seen[0].comments[0];
    assert_eq!(words(comment), "Looks good to me");
    assert_eq!(comment.author_name, "Bob");
    assert!(!comment.author_former && !comment.is_mine && !comment.deleted);
    assert_eq!(comment.author, bob.account());

    // Alice replies; Bob and the Viewer read both, with the right `isMine`.
    post(&mut alice, plan, Some(posted.thread_id), text("Thanks Bob"));
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    for dev in [&mut bob, &mut carol] {
        let seen = threads(dev, plan);
        assert_eq!(seen.len(), 1, "{}", dev.name);
        assert_eq!(
            seen[0].comments.iter().map(words).collect::<Vec<_>>(),
            ["Looks good to me", "Thanks Bob"]
        );
        assert_eq!(seen[0].comments[1].author_name, "Alice");
    }
    assert!(threads(&mut bob, plan)[0].comments[0].is_mine);
    assert!(!threads(&mut carol, plan)[0].comments[0].is_mine);

    // The Viewer reads and cannot post; Bob cannot write the page.
    forbidden_role(
        &fails(
            &mut carol,
            Request::CommentPost {
                page_id: plan,
                thread_id: None,
                anchor: None,
                body: text("hello?"),
            },
        ),
        "commenter",
    );
    assert!(bob.core.page_doc_import(plan, &[1]).is_err());
    // A Commenter's device carries no page change, whatever the page holds.
    bob.core.shared.force_push = true;
    bob.write(plan, "a commenter should not type this");
    bob.core.shared.force_push = false;
    let outbox = bob.core.sync_feed_outbox(feed).unwrap();
    assert!(
        outbox
            .iter()
            .all(|entry| entry.doc_id == comment_stream_doc_id(plan)),
        "only the comment stream travels from a Commenter"
    );
}

#[test]
fn the_requests_check_their_input_and_name_what_is_wrong() {
    let Pair {
        mut alice, plan, ..
    } = pair(MemberRole::Commenter);
    let posted = post(&mut alice, plan, None, text("root"));
    // A body: not empty, no empty text item, at most 10 KB.
    invalid_field(
        &try_post(&mut alice, plan, None, None, Vec::new()).unwrap_err(),
        "body",
    );
    invalid_field(
        &try_post(&mut alice, plan, None, None, text("")).unwrap_err(),
        "body",
    );
    invalid_field(
        &try_post(&mut alice, plan, None, None, text(&"x".repeat(11_000))).unwrap_err(),
        "body",
    );
    // An anchor: not with a reply, a quote of at most 200 characters, cursors that are base64.
    let anchor = |quote: &str, start: Option<&str>| CommentAnchor {
        block_id: None,
        start: start.map(str::to_owned),
        end: None,
        quote: quote.into(),
    };
    invalid_field(
        &try_post(
            &mut alice,
            plan,
            Some(posted.thread_id),
            Some(anchor("q", None)),
            text("r"),
        )
        .unwrap_err(),
        "anchor",
    );
    invalid_field(
        &try_post(
            &mut alice,
            plan,
            None,
            Some(anchor(&"q".repeat(201), None)),
            text("r"),
        )
        .unwrap_err(),
        "anchor",
    );
    invalid_field(
        &try_post(
            &mut alice,
            plan,
            None,
            Some(anchor("q", Some("not base64!"))),
            text("r"),
        )
        .unwrap_err(),
        "anchor",
    );
    // A thread of the page; an unknown page; an unknown comment.
    invalid_field(
        &try_post(
            &mut alice,
            plan,
            Some(ThreadId::from_bytes([9; 16])),
            None,
            text("r"),
        )
        .unwrap_err(),
        "threadId",
    );
    let missing = PageId::from_bytes([7; 16]);
    assert_eq!(
        try_post(&mut alice, missing, None, None, text("r"))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        call(&mut alice, Request::CommentList { page_id: missing })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let unknown = CommentId::from_bytes([8; 16]);
    assert_eq!(
        edit(&mut alice, plan, unknown, "e").unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        delete(&mut alice, plan, unknown).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        resolve(&mut alice, plan, ThreadId::of(unknown), true)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    // Nothing of that was stored.
    assert_eq!(threads(&mut alice, plan).len(), 1);
}

#[test]
fn a_viewers_forced_comment_is_refused_by_the_server_and_quarantined_by_the_others() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Viewer);
    bob.core.shared.force_push = true;
    // The hostile client stores its record (its own fold ignores a Viewer's, so the answer is an
    // error) and sends it.
    let _ = bob
        .core
        .comment_post(plan, None, None, text("a viewer should not comment"));
    assert_eq!(bob.core.db.comment_records_of_page(plan).unwrap().len(), 1);
    // The server refuses the append of a Viewer ...
    let outbox = bob.core.sync_feed_outbox(feed).unwrap();
    assert!(!outbox.is_empty());
    assert!(matches!(
        bob.remote.append_sync(feed, &outbox[0].bytes),
        Err(kb_sync::SyncError::Forbidden)
    ));
    // ... and one that accepts anything lets it in: the receivers refuse it, count it, and go on.
    server_adversary(&bob, Some(Adversary::AcceptAll));
    bob.cycle().unwrap();
    server_adversary(&bob, None);
    bob.core.shared.force_push = false;
    alice.events();
    alice.cycle().unwrap();
    assert!(
        threads(&mut alice, plan).is_empty(),
        "the record is not applied"
    );
    assert!(
        alice
            .events()
            .contains(&Event::SpaceMembersChanged { space_id: space })
    );
    let info = alice.core.members_info(space).unwrap();
    // Two items: the key record of the stream (a Viewer makes none) and the comment itself.
    assert_eq!(info.refused_total, 2);
    assert_eq!(member_of(&info, "Bob").refused_changes, 2);
    // Alice carries on and comments; the chain went past the refused items, and a member who joins
    // afterwards reads her comment (the hostile device's own key record never became the stream's).
    post(&mut alice, plan, None, text("alice carries on"));
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let mut carol = add_carol(&mut alice, space, feed, MemberRole::Viewer);
    Dev::settle(&mut [&mut alice, &mut carol, &mut bob], 3);
    for dev in [&mut alice, &mut carol] {
        assert_eq!(
            threads(dev, plan)
                .iter()
                .flat_map(|t| t.comments.iter().map(words))
                .collect::<Vec<_>>(),
            ["alice carries on"],
            "{}",
            dev.name
        );
    }
}

#[test]
fn nobody_edits_deletes_or_resolves_what_is_not_theirs_and_a_forgery_is_ignored_by_the_others() {
    let Pair {
        mut alice,
        mut bob,
        feed,
        space,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let mut carol = add_carol(&mut alice, space, feed, MemberRole::Commenter);
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    let posted = post(&mut alice, plan, None, text("original"));
    let comment = posted.comments[0].comment_id;
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);

    // The requests say no (the author only; a thread's author or an Editor; an Owner).
    let refused = edit(&mut bob, plan, comment, "hacked").unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(refused.details, Some(json!({ "reason": "author" })));
    forbidden_role(&delete(&mut bob, plan, comment).unwrap_err(), "owner");
    forbidden_role(
        &resolve(&mut bob, plan, posted.thread_id, true).unwrap_err(),
        "editor",
    );

    // A hostile client signs the records anyway; a server that accepts anything takes them.
    let mut records = Vec::new();
    for (rev, op) in [
        (
            5,
            CommentOp::Edit {
                body: text("hacked"),
            },
        ),
        (6, CommentOp::Delete),
        (7, CommentOp::Resolve { resolved: true }),
    ] {
        let record = forged(&bob, plan, posted.thread_id, comment, op, rev);
        records.push(record);
    }
    for record in records {
        bob.core.test_comment_forge(plan, record).unwrap();
    }
    server_adversary(&bob, Some(Adversary::AcceptAll));
    Dev::settle(&mut [&mut bob, &mut alice, &mut carol], 3);
    server_adversary(&bob, None);
    for dev in [&mut alice, &mut bob, &mut carol] {
        let seen = threads(dev, plan);
        assert_eq!(seen.len(), 1, "{}", dev.name);
        let comment = &seen[0].comments[0];
        assert_eq!(
            words(comment),
            "original",
            "{}: only the author's Edit counts",
            dev.name
        );
        assert_eq!(comment.edited_at, None, "{}", dev.name);
        assert!(
            !comment.deleted,
            "{}: only the author or an Owner deletes",
            dev.name
        );
        assert!(
            !seen[0].resolved,
            "{}: a Commenter does not resolve another's thread",
            dev.name
        );
    }
    assert_eq!(
        alice.core.members_info(space).unwrap().refused_total,
        0,
        "what the fold ignores is not quarantined: Bob's records are well formed and signed"
    );

    // The author's own edit counts, and the last revision wins over a replay of an older one.
    edit(&mut alice, plan, comment, "first edit").unwrap();
    let second = edit(&mut alice, plan, comment, "second edit").unwrap();
    assert!(second.comments[0].edited_at.is_some());
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    let replay = forged(
        &alice,
        plan,
        posted.thread_id,
        comment,
        CommentOp::Edit {
            body: text("replayed"),
        },
        1,
    );
    alice.core.test_comment_forge(plan, replay).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    for dev in [&mut alice, &mut bob, &mut carol] {
        assert_eq!(
            words(&threads(dev, plan)[0].comments[0]),
            "second edit",
            "{}: an older revision never wins",
            dev.name
        );
    }
}

#[test]
fn an_owner_deletes_any_comment_the_delete_is_final_and_a_placeholder_keeps_the_thread() {
    let Pair {
        mut alice,
        mut bob,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let root = post(&mut bob, plan, None, text("bob's root"));
    let thread = root.thread_id;
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let reply = post(&mut alice, plan, Some(thread), text("alice's reply"));
    let alice_comment = reply.comments[1].comment_id;
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let late = post(&mut bob, plan, Some(thread), text("bob's last word"));
    let bob_last = late.comments[2].comment_id;
    Dev::settle(&mut [&mut alice, &mut bob], 3);

    // Alice, an Owner, deletes Bob's last word: it is gone, the thread stays.
    let after = delete(&mut alice, plan, bob_last).unwrap();
    assert_eq!(after.comments.len(), 2);
    // Bob deletes nothing of Alice's, but his own root: it stays as a placeholder (a reply follows).
    forbidden_role(&delete(&mut bob, plan, alice_comment).unwrap_err(), "owner");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let root_comment = root.comments[0].comment_id;
    let placeholder = delete(&mut bob, plan, root_comment).unwrap();
    assert!(placeholder.comments[0].deleted);
    assert!(placeholder.comments[0].body.is_empty());
    assert_eq!(placeholder.comments.len(), 2);
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    for dev in [&mut alice, &mut bob] {
        let seen = threads(dev, plan);
        assert_eq!(seen[0].comments.len(), 2, "{}", dev.name);
        assert!(seen[0].comments[0].deleted, "{}", dev.name);
        assert_eq!(words(&seen[0].comments[1]), "alice's reply", "{}", dev.name);
    }
    // A deleted comment cannot be edited, and a forged Edit of it is ignored: Delete is final.
    assert_eq!(
        edit(&mut bob, plan, root_comment, "undelete")
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let record = forged(
        &bob,
        plan,
        thread,
        root_comment,
        CommentOp::Edit {
            body: text("undelete"),
        },
        9,
    );
    bob.core.test_comment_forge(plan, record).unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    for dev in [&mut alice, &mut bob] {
        assert!(threads(dev, plan)[0].comments[0].deleted, "{}", dev.name);
    }
    // Deleting the last live comment leaves no thread in the list; the answer has no comments.
    let emptied = delete(&mut alice, plan, alice_comment).unwrap();
    assert!(emptied.comments.is_empty());
    assert!(threads(&mut alice, plan).is_empty());
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(threads(&mut bob, plan).is_empty());
}

#[test]
fn a_thread_is_resolved_by_its_author_or_an_editor_and_reopened_the_same_way() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let mut carol = add_carol(&mut alice, space, feed, MemberRole::Editor);
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    let bobs = post(&mut bob, plan, None, text("bob's question")).thread_id;
    let alices = post(&mut alice, plan, None, text("alice's question")).thread_id;
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);

    // Bob resolves his own thread; he cannot resolve Alice's.
    let done = resolve(&mut bob, plan, bobs, true).unwrap();
    assert!(done.resolved);
    assert_eq!(done.resolved_by, Some(bob.account()));
    forbidden_role(
        &resolve(&mut bob, plan, alices, true).unwrap_err(),
        "editor",
    );
    // Carol, an Editor, resolves Alice's thread and reopens Bob's.
    assert!(resolve(&mut carol, plan, alices, true).unwrap().resolved);
    Dev::settle(&mut [&mut bob, &mut carol, &mut alice], 3);
    let reopened = resolve(&mut carol, plan, bobs, false).unwrap();
    assert!(!reopened.resolved);
    assert_eq!(reopened.resolved_by, None);
    Dev::settle(&mut [&mut carol, &mut alice, &mut bob], 3);
    let carol_account = carol.account();
    for dev in [&mut alice, &mut bob, &mut carol] {
        let seen = threads(dev, plan);
        let state = |id: ThreadId| seen.iter().find(|t| t.thread_id == id).unwrap();
        assert!(!state(bobs).resolved, "{}", dev.name);
        assert!(state(alices).resolved, "{}", dev.name);
        assert_eq!(
            state(alices).resolved_by,
            Some(carol_account),
            "{}",
            dev.name
        );
    }
}

#[test]
fn comments_of_a_personal_space_sync_between_the_devices_of_the_account_and_never_notify() {
    let server = server();
    let (mut first, kit) = Dev::first("first", 1, &server);
    first.configure();
    let personal = crate::test_support::spaces(&mut first.core)
        .into_iter()
        .find(|info| info.share_state == SpaceShareState::Personal)
        .unwrap()
        .id;
    let page = create_page(&mut first.core, personal, None, "Diary").id;
    first.write(page, "a page of my own");
    Dev::settle(&mut [&mut first], 2);
    let mut second = Dev::from_backup("second", 2, &server, &mut first, &kit);
    second.configure();
    Dev::settle(&mut [&mut second, &mut first], 3);

    // A personal space has one person; the comments are theirs on every device.
    let root = post(&mut first, page, None, text("from the first device"));
    Dev::settle(&mut [&mut first, &mut second], 3);
    let seen = threads(&mut second, page);
    assert_eq!(seen.len(), 1);
    assert!(seen[0].comments[0].is_mine);
    assert!(!seen[0].comments[0].author_former);
    post(
        &mut second,
        page,
        Some(root.thread_id),
        text("and the second"),
    );
    resolve(&mut second, page, root.thread_id, true).unwrap();
    Dev::settle(&mut [&mut second, &mut first], 3);
    let back = threads(&mut first, page);
    assert!(back[0].resolved);
    assert_eq!(
        back[0].comments.iter().map(words).collect::<Vec<_>>(),
        ["from the first device", "and the second"]
    );
    // Words are found on both devices; neither is told of the other's comments (it is their own).
    for dev in [&mut first, &mut second] {
        assert_eq!(search(dev, "second", None).len(), 1, "{}", dev.name);
        assert!(inbox(dev).is_empty(), "{}", dev.name);
    }
    // The people to mention: this account alone.
    let people = match call(
        &mut second,
        Request::PeopleMentionable { space_id: personal },
    )
    .unwrap()
    {
        Response::People(people) => people,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(people.len(), 1);
    assert!(people[0].is_me);
}

// ----- forgeries and snapshots -------------------------------------------------------------------

#[test]
fn a_record_that_claims_another_author_or_a_comment_twice_is_quarantined() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let posted = post(&mut alice, plan, None, text("original"));
    let comment = posted.comments[0].comment_id;
    Dev::settle(&mut [&mut alice, &mut bob], 3);

    // Bob signs a record in Alice's name (the entry says Bob wrote it).
    let mut as_alice = forged(
        &bob,
        plan,
        ThreadId::of(CommentId::from_bytes([0x44; 16])),
        CommentId::from_bytes([0x44; 16]),
        CommentOp::Post {
            anchor: None,
            body: text("alice never said this"),
        },
        0,
    );
    as_alice.author = alice.account();
    bob.core.test_comment_forge(plan, as_alice).unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(alice.core.members_info(space).unwrap().refused_total, 1);
    assert_eq!(threads(&mut alice, plan).len(), 1, "nothing was added");

    // A second claim of a comment id that Alice's thread has: a forgery (ids are random).
    let second_claim = forged(
        &bob,
        plan,
        posted.thread_id,
        comment,
        CommentOp::Post {
            anchor: None,
            body: text("this is the real comment"),
        },
        0,
    );
    bob.core.test_comment_forge(plan, second_claim).unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(alice.core.members_info(space).unwrap().refused_total, 2);
    let seen = threads(&mut alice, plan);
    assert_eq!(words(&seen[0].comments[0]), "original");
    assert_eq!(seen[0].comments[0].author, alice.account());

    // An Edit that names another thread than its comment's, and a Resolve of a reply (not the
    // root): contradictions of their target, refused the same way.
    let wrong_thread = forged(
        &bob,
        plan,
        ThreadId::from_bytes([0x55; 16]),
        comment,
        CommentOp::Edit { body: text("x") },
        3,
    );
    bob.core.test_comment_forge(plan, wrong_thread).unwrap();
    let not_root = forged(
        &bob,
        plan,
        posted.thread_id,
        CommentId::from_bytes([0x66; 16]),
        CommentOp::Resolve { resolved: true },
        4,
    );
    bob.core.test_comment_forge(plan, not_root).unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(
        alice.core.members_info(space).unwrap().refused_total,
        3,
        "one item, one count"
    );
    assert_eq!(words(&threads(&mut alice, plan)[0].comments[0]), "original");
}

#[test]
fn a_comment_snapshot_that_omits_a_record_is_refused_disputed_and_republished() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let first = post(&mut bob, plan, None, text("the first comment"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let second = post(&mut bob, plan, None, text("the second comment"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(threads(&mut alice, plan).len(), 2);

    // Bob's device publishes a snapshot of the stream that covers everything and holds one record.
    let rows = bob.core.db.comment_records_of_page(plan).unwrap();
    let first_record = rows
        .iter()
        .find(|row| row.comment_id == first.comments[0].comment_id)
        .unwrap()
        .record
        .clone();
    let truncated = kb_doc::encode_records_snapshot(&[first_record.as_slice()]);
    let covers = bob.core.feed_row(feed).unwrap().cursor;
    let doc = comment_stream_doc_id(plan);
    let sealed = bob
        .core
        .test_seal_snapshot(feed, doc, truncated, covers)
        .unwrap();
    bob.remote.put_snapshot_sync(feed, &sealed).unwrap();
    alice.events();
    alice.cycle().unwrap();
    let events = alice.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::SnapshotDisputed { space_id, page_id, .. } if *space_id == space && *page_id == Some(plan))),
        "{events:?}"
    );
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.disputes.len(), 1);
    assert_eq!(info.disputes[0].page_id, Some(plan));
    assert_eq!(info.refused_total, 1);
    // Both comments are still there, and her next cycle republishes the whole stream.
    assert_eq!(threads(&mut alice, plan).len(), 2);
    alice.cycle().unwrap();
    let latest = alice.remote.get_snapshot_sync(feed, doc).unwrap().unwrap();
    assert_eq!(
        kb_sync::v2::peek_snapshot(&latest).unwrap().account_id,
        alice.account()
    );
    // A member who joins afterwards reads both comments from the stream.
    let mut carol = add_carol(&mut alice, space, feed, MemberRole::Viewer);
    Dev::settle(&mut [&mut alice, &mut carol, &mut bob], 3);
    let seen = threads(&mut carol, plan);
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen.iter()
            .map(|t| words(&t.comments[0]))
            .collect::<Vec<_>>(),
        ["the first comment", "the second comment"]
    );
    assert_eq!(second.comments.len(), 1);
}

#[test]
fn a_member_who_joins_after_the_comments_were_snapshotted_and_pruned_reads_them() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        notes,
    } = pair(MemberRole::Commenter);
    let first = post(&mut bob, plan, None, text("bob before the snapshot"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let reply = post(
        &mut alice,
        plan,
        Some(first.thread_id),
        text("alice before the snapshot"),
    );
    post(&mut bob, notes, None, text("a second page"));
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    edit(&mut bob, plan, first.comments[0].comment_id, "bob, edited").unwrap();
    resolve(&mut alice, plan, first.thread_id, true).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    // Fresh snapshots of the streams (and the pages), which everybody acknowledges; a week later the
    // server prunes what they cover.
    for doc in [
        comment_stream_doc_id(plan),
        comment_stream_doc_id(notes),
        DocId::for_page(plan),
        DocId::for_page(notes),
    ] {
        alice.core.feed_mark_for_snapshot(feed, doc).unwrap();
    }
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let server = alice.remote.clone();
    server.set_now_ms(SERVER_NOW + 8 * DAY_MS);
    assert!(server.prune(SERVER_NOW + 8 * DAY_MS) > 0);
    let mut carol = add_carol(&mut alice, space, feed, MemberRole::Commenter);
    Dev::settle(&mut [&mut carol, &mut alice, &mut bob], 4);
    for page in [plan, notes] {
        let want = threads(&mut alice, page);
        let got = threads(&mut carol, page);
        assert_eq!(got.len(), want.len());
        for (got, want) in got.iter().zip(&want) {
            assert_eq!(got.thread_id, want.thread_id);
            assert_eq!(got.resolved, want.resolved);
            assert_eq!(
                got.comments.iter().map(words).collect::<Vec<_>>(),
                want.comments.iter().map(words).collect::<Vec<_>>()
            );
        }
    }
    let on_plan = threads(&mut carol, plan);
    assert!(on_plan[0].resolved);
    assert_eq!(
        on_plan[0].comments.iter().map(words).collect::<Vec<_>>(),
        ["bob, edited", "alice before the snapshot"]
    );
    let _ = reply;
    assert_eq!(carol.core.sync_info().unwrap().parked_items, 0);
    // And Carol, a Commenter, comments into the stream she bootstrapped.
    post(
        &mut carol,
        plan,
        Some(first.thread_id),
        text("carol joins in"),
    );
    Dev::settle(&mut [&mut carol, &mut alice, &mut bob], 3);
    assert_eq!(threads(&mut bob, plan)[0].comments.len(), 3);
}

#[test]
fn a_demoted_commenters_later_records_are_refused_and_the_earlier_ones_stay() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    post(&mut bob, plan, None, text("written as a commenter"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let bob_account = bob.account();
    set_role(&mut alice, feed, bob_account, MemberRole::Viewer);
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(role_of(&mut bob, space), Some(MemberRole::Viewer));
    forbidden_role(
        &fails(
            &mut bob,
            Request::CommentPost {
                page_id: plan,
                thread_id: None,
                anchor: None,
                body: text("not any more"),
            },
        ),
        "commenter",
    );
    // A hostile device of his keeps going: the server refuses the append (he is a Viewer now).
    bob.core.shared.force_push = true;
    let _ = bob
        .core
        .comment_post(plan, None, None, text("written as a viewer"));
    bob.core.shared.force_push = false;
    server_adversary(&bob, Some(Adversary::AcceptAll));
    bob.core.shared.force_push = true;
    bob.cycle().unwrap();
    bob.core.shared.force_push = false;
    server_adversary(&bob, None);
    alice.cycle().unwrap();
    let seen = threads(&mut alice, plan);
    assert_eq!(seen.len(), 1);
    assert_eq!(words(&seen[0].comments[0]), "written as a commenter");
    assert_eq!(alice.core.members_info(space).unwrap().refused_total, 1);
}

#[test]
fn a_removed_author_reads_as_a_former_member_and_the_comments_stay() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    post(&mut bob, plan, None, text("said while a member"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert!(!threads(&mut alice, plan)[0].comments[0].author_former);
    call(
        &mut alice,
        Request::MemberRemove {
            space_id: space,
            account_id: bob.account(),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice], 3);
    let seen = threads(&mut alice, plan);
    let comment = &seen[0].comments[0];
    assert!(comment.author_former);
    assert_eq!(comment.author_name, "Bob", "the last name known is kept");
    assert_eq!(words(comment), "said while a member");
    // The members read lists the person with the last name too, for a Person value or a mention
    // that names an account the space does not list any more.
    let info = alice.core.members_info(space).unwrap();
    assert!(
        info.members
            .iter()
            .all(|member| member.account_id != bob.account())
    );
    assert_eq!(
        info.former_members
            .iter()
            .map(|former| (former.account_id, former.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(bob.account(), "Bob")]
    );
}

#[test]
fn a_member_who_left_is_listed_as_a_former_member_with_the_name_the_space_knew() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    post(&mut bob, plan, None, text("said before leaving"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let bob_account = bob.account();
    assert!(
        alice
            .core
            .members_info(space)
            .unwrap()
            .former_members
            .is_empty()
    );
    call(&mut bob, Request::SpaceLeave { space_id: space }).unwrap();
    bob.cycle().unwrap();
    alice.cycle().unwrap();
    let info = alice.core.members_info(space).unwrap();
    assert!(
        info.members
            .iter()
            .all(|member| member.account_id != bob_account)
    );
    assert_eq!(
        info.former_members
            .iter()
            .map(|former| (former.account_id, former.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(bob_account, "Bob")]
    );
    assert!(threads(&mut alice, plan)[0].comments[0].author_former);
}

// ----- notifications -----------------------------------------------------------------------------

#[test]
fn a_reply_a_mention_in_a_comment_a_page_mention_and_an_assignment_each_notify_once() {
    let Pair {
        mut alice,
        mut bob,
        space,
        notes,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let bob_account = bob.account();
    let alice_account = alice.account();
    alice.events();
    bob.events();

    // 1. A reply in a thread Alice wrote in.
    let root = post(&mut alice, plan, None, text("Alice's question"));
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(
        inbox(&mut alice).is_empty(),
        "her own comment notifies nobody, herself least"
    );
    assert!(
        inbox(&mut bob).is_empty(),
        "nothing in the thread names Bob yet"
    );
    let reply = post(&mut bob, plan, Some(root.thread_id), text("Bob's answer"));
    let reply_id = reply.comments[1].comment_id;
    alice.events();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let rows = inbox(&mut alice);
    assert_eq!(kinds(&rows), [NotificationKind::ThreadReply]);
    assert_eq!(rows[0].actor, bob_account);
    assert_eq!(rows[0].actor_name, "Bob");
    assert_eq!(rows[0].page_id, Some(plan));
    assert_eq!(rows[0].page_title.as_deref(), Some("Plan"));
    assert_eq!(rows[0].comment_id, Some(reply_id));
    assert_eq!(rows[0].space_id, space);
    assert!(!rows[0].read);
    let events = alice.events();
    assert!(events.iter().any(
        |event| matches!(event, Event::NotificationRaised(info) if info.kind == NotificationKind::ThreadReply && info.actor_name == "Bob")
    ));
    assert!(events.contains(&Event::NotificationsChanged));
    assert!(
        inbox(&mut bob).is_empty(),
        "his own reply does not notify his own device"
    );

    // 2. A mention of Bob in a comment: a mention, not a reply, although he wrote in the thread; an
    // edit notifies only for a mention it adds.
    let mention = vec![
        CommentBodyItem::Text { text: "cc ".into() },
        CommentBodyItem::Mention {
            account_id: bob_account,
        },
    ];
    let with_mention = post(&mut alice, plan, Some(root.thread_id), mention);
    // A comment of a thread of its own that names nobody: silent (Bob never wrote in it).
    let plain = post(&mut alice, plan, None, text("Thanks, no mention yet"));
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let rows = inbox(&mut bob);
    assert_eq!(kinds(&rows), [NotificationKind::CommentMention]);
    assert_eq!(
        rows[0].comment_id,
        Some(with_mention.comments[2].comment_id)
    );
    edit(
        &mut alice,
        plan,
        plain.comments[0].comment_id,
        "Thanks, still no mention",
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(
        inbox(&mut bob).len(),
        1,
        "an edit with no new mention is silent"
    );
    let adding = vec![CommentBodyItem::Mention {
        account_id: bob_account,
    }];
    call(
        &mut alice,
        Request::CommentEdit {
            page_id: plan,
            comment_id: plain.comments[0].comment_id,
            body: adding.clone(),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(
        kinds(&inbox(&mut bob)),
        [
            NotificationKind::CommentMention,
            NotificationKind::CommentMention
        ]
    );
    // Editing again with the mention still in it does not notify again.
    call(
        &mut alice,
        Request::CommentEdit {
            page_id: plan,
            comment_id: plain.comments[0].comment_id,
            body: vec![
                CommentBodyItem::Text {
                    text: "still: ".into(),
                },
                CommentBodyItem::Mention {
                    account_id: bob_account,
                },
            ],
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(inbox(&mut bob).len(), 2);

    // 3. A person mention of Bob in a page of the space, from somebody else's change.
    let mention_block = block(41);
    append_blocks(
        &mut alice,
        notes,
        json!([{
            "type": "paragraph",
            "attrs": { "id": mention_block },
            "content": [
                { "type": "text", "text": "cc " },
                { "type": "personMention", "attrs": { "accountId": bob_account.to_string(), "label": "Bob" } }
            ]
        }]),
    );
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let rows = inbox(&mut bob);
    assert_eq!(rows[0].kind, NotificationKind::PageMention);
    assert_eq!(rows[0].page_id, Some(notes));
    assert_eq!(
        rows[0].block_id.map(|b| b.to_string()),
        Some(mention_block.clone())
    );
    assert_eq!(rows[0].actor, alice_account);
    // A later change to the page that keeps the mention is not a new mention.
    append_blocks(
        &mut alice,
        notes,
        json!([{ "type": "paragraph", "attrs": { "id": block(42) },
                 "content": [{ "type": "text", "text": "more words" }] }]),
    );
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(inbox(&mut bob).len(), 3);
    // A mention of Alice herself, by herself, notifies nobody; and Alice's own inbox holds only the
    // one reply.
    append_blocks(
        &mut alice,
        notes,
        json!([{ "type": "paragraph", "attrs": { "id": block(43) },
                 "content": [{ "type": "personMention", "attrs": { "accountId": alice_account.to_string(), "label": "Alice" } }] }]),
    );
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(kinds(&inbox(&mut alice)), [NotificationKind::ThreadReply]);

    // 4. Being added to a Person property.
    let property = create_person_property(&mut alice, space, "Owner");
    set_people(&mut alice, plan, property, &[alice_account]).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(inbox(&mut bob).len(), 3, "Bob is not on the list yet");
    set_people(&mut alice, plan, property, &[alice_account, bob_account]).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let rows = inbox(&mut bob);
    assert_eq!(rows[0].kind, NotificationKind::PersonAssigned);
    assert_eq!(rows[0].page_id, Some(plan));
    assert_eq!(rows[0].actor, alice_account);
    // Keeping him on the list is silent; taking him off and putting him back is not.
    set_people(&mut alice, plan, property, &[bob_account, alice_account]).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(inbox(&mut bob).len(), 4);
    set_people(&mut alice, plan, property, &[alice_account]).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    set_people(&mut alice, plan, property, &[alice_account, bob_account]).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(inbox(&mut bob).len(), 5);
    // Bob assigning himself on another device of the account would not notify either (not tested here:
    // the actor is the account).
}

#[test]
fn the_inbox_pages_marks_rows_read_and_announces_the_change() {
    let Pair {
        mut alice,
        mut bob,
        plan,
        notes,
        ..
    } = pair(MemberRole::Commenter);
    let root = post(&mut alice, plan, None, text("question"));
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    for i in 0..3 {
        post(
            &mut bob,
            plan,
            Some(root.thread_id),
            text(&format!("answer {i}")),
        );
        Dev::settle(&mut [&mut bob, &mut alice], 2);
    }
    let _ = notes;
    let all = inbox(&mut alice);
    assert_eq!(all.len(), 3);
    assert!(
        all.windows(2).all(|pair| pair[0].id > pair[1].id),
        "newest first"
    );
    // Paging.
    let first = match call(
        &mut alice,
        Request::NotificationList {
            limit: Some(2),
            before_id: None,
            unread_only: None,
        },
    )
    .unwrap()
    {
        Response::Notifications(rows) => rows,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(first.len(), 2);
    let rest = match call(
        &mut alice,
        Request::NotificationList {
            limit: Some(2),
            before_id: Some(first[1].id),
            unread_only: None,
        },
    )
    .unwrap()
    {
        Response::Notifications(rows) => rows,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        rest.iter().map(|row| row.id).collect::<Vec<_>>(),
        [all[2].id]
    );
    // Marking read.
    alice.events();
    call(
        &mut alice,
        Request::NotificationsMarkRead {
            ids: Some(vec![all[0].id]),
        },
    )
    .unwrap();
    assert!(alice.events().contains(&Event::NotificationsChanged));
    let unread = match call(
        &mut alice,
        Request::NotificationList {
            limit: None,
            before_id: None,
            unread_only: Some(true),
        },
    )
    .unwrap()
    {
        Response::Notifications(rows) => rows,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(unread.len(), 2);
    assert!(inbox(&mut alice)[0].read);
    // Marking what is read already changes nothing and says nothing.
    call(
        &mut alice,
        Request::NotificationsMarkRead {
            ids: Some(vec![all[0].id]),
        },
    )
    .unwrap();
    assert!(!alice.events().contains(&Event::NotificationsChanged));
    call(&mut alice, Request::NotificationsMarkRead { ids: None }).unwrap();
    assert!(inbox(&mut alice).iter().all(|row| row.read));
}

#[test]
fn a_new_device_announces_none_of_the_history_it_reads_and_all_of_the_news() {
    let Pair {
        mut alice,
        mut bob,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let bob_account = bob.account();
    let mention = || {
        vec![
            CommentBodyItem::Text { text: "cc ".into() },
            CommentBodyItem::Mention {
                account_id: bob_account,
            },
        ]
    };
    post(&mut alice, plan, None, mention());
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(inbox(&mut bob).len(), 1);
    // A second device of Bob's account reads the whole feed: the mention is history for it.
    let server = alice.remote.clone();
    let mut bob2 = Dev::fresh("bob2", 5, &server, &mut bob);
    bob2.configure();
    Dev::settle(&mut [&mut bob2, &mut bob, &mut alice], 3);
    assert_eq!(threads(&mut bob2, plan).len(), 1, "the comment is there");
    assert!(inbox(&mut bob2).is_empty(), "history is not announced");
    assert_eq!(inbox(&mut bob).len(), 1);
    // The next mention is news on both devices.
    post(&mut alice, plan, None, mention());
    Dev::settle(&mut [&mut alice, &mut bob, &mut bob2], 3);
    assert_eq!(inbox(&mut bob).len(), 2);
    assert_eq!(inbox(&mut bob2).len(), 1);
}

// ----- reminders ----------------------------------------------------------------------------------

#[test]
fn a_shared_reminder_fires_on_the_devices_of_the_account_it_names_and_otherwise_on_the_owners() {
    let Pair {
        mut alice,
        mut bob,
        notes,
        ..
    } = pair(MemberRole::Commenter);
    let (alice_account, bob_account) = (alice.account(), bob.account());
    let date = |n: u8, who: Option<AccountId>| {
        let mut attrs = json!({
            "id": block(n), "date": "2026-10-07", "time": "14:30", "reminder": 15
        });
        if let Some(who) = who {
            attrs["reminderFor"] = json!(who.to_string());
        }
        json!({ "type": "dateMention", "attrs": attrs })
    };
    append_blocks(
        &mut alice,
        notes,
        json!([{
            "type": "paragraph",
            "attrs": { "id": block(50) },
            "content": [
                { "type": "text", "text": "ship " }, date(61, None),
                { "type": "text", "text": " " }, date(62, Some(bob_account)),
                { "type": "text", "text": " " }, date(63, Some(alice_account)),
            ]
        }]),
    );
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    // 2026-10-07 14:30 minus 15 minutes is 1_791_382_500; ask for what is due at the moment itself.
    let until = 1_791_383_400;
    let due = |dev: &mut Dev| -> Vec<kb_types::BlockId> {
        match call(dev, Request::RemindersDue { until }).unwrap() {
            Response::Reminders(list) => {
                let mut nodes: Vec<kb_types::BlockId> = list.iter().map(|d| d.node_id).collect();
                nodes.sort();
                nodes
            }
            other => panic!("unexpected {other:?}"),
        }
    };
    let id = |n: u8| kb_types::BlockId::from_random_bytes([n; 16]);
    let mut owners = vec![id(61), id(63)];
    owners.sort();
    assert_eq!(
        due(&mut alice),
        owners,
        "the Owner hears the unnamed one and her own"
    );
    assert_eq!(
        due(&mut bob),
        vec![id(62)],
        "a Commenter hears the one that names him, and only it"
    );
}

// ----- search -------------------------------------------------------------------------------------

#[test]
fn the_words_of_comments_are_found_labelled_and_follow_edits_and_deletes() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Commenter);
    let posted = post(
        &mut bob,
        plan,
        None,
        text("The quarterly caf\u{e9} roadmap needs review"),
    );
    let comment = posted.comments[0].comment_id;
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    for dev in [&mut alice, &mut bob] {
        let hits = search(dev, "cafe road", Some(space));
        assert_eq!(hits.len(), 1, "{}", dev.name);
        assert_eq!(hits[0].comment_id, Some(comment));
        assert_eq!(hits[0].page_id, plan);
        assert_eq!(hits[0].block_id, None);
        assert_eq!(hits[0].title, "Plan");
        assert!(hits[0].snippet.contains("caf\u{e9}"));
        assert!(!hits[0].highlights.is_empty());
    }
    // A rebuild of the indexes finds the comments again (their words are rebuilt from the records).
    call(&mut alice, Request::IndexRebuild { space_id: None }).unwrap();
    assert_eq!(search(&mut alice, "cafe road", Some(space)).len(), 1);
    call(
        &mut alice,
        Request::IndexRebuild {
            space_id: Some(space),
        },
    )
    .unwrap();
    assert_eq!(search(&mut alice, "cafe road", Some(space)).len(), 1);
    // Page text hits are not comment hits.
    let page_hits = search(&mut alice, "base line", None);
    assert!(page_hits.iter().all(|hit| hit.comment_id.is_none()));
    // Titles only never lists comments.
    let titles = match call(
        &mut alice,
        Request::Search {
            query: "roadmap".into(),
            space_id: None,
            scope: Some(kb_types::SearchScope::Titles),
            limit: None,
            edited_after: None,
        },
    )
    .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    };
    assert!(titles.iter().all(|hit| hit.comment_id.is_none()));
    // An edit moves the words, a delete takes them out.
    edit(&mut bob, plan, comment, "Entirely different sentence").unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    for dev in [&mut alice, &mut bob] {
        assert!(
            search(dev, "roadmap", None)
                .iter()
                .all(|hit| hit.comment_id.is_none()),
            "{}",
            dev.name
        );
        assert_eq!(search(dev, "different", None).len(), 1, "{}", dev.name);
    }
    delete(&mut bob, plan, comment).unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    for dev in [&mut alice, &mut bob] {
        assert!(search(dev, "different", None).is_empty(), "{}", dev.name);
    }
    // A comment on a trashed page is not found (the page is not either).
    let live = post(&mut alice, plan, None, text("anchorword sits here"));
    assert_eq!(search(&mut alice, "anchorword", None).len(), 1);
    let _ = live;
    call(&mut alice, Request::PageTrash { page_id: plan }).unwrap();
    assert!(search(&mut alice, "anchorword", None).is_empty());
}

// ----- moving a page, backups ----------------------------------------------------------------------

#[test]
fn a_page_moved_into_a_shared_space_brings_its_comments_with_it() {
    let Pair {
        mut alice,
        mut bob,
        space,
        ..
    } = pair(MemberRole::Commenter);
    let personal = crate::test_support::spaces(&mut alice.core)
        .into_iter()
        .find(|info| info.share_state == SpaceShareState::Personal)
        .unwrap()
        .id;
    let solo = create_page(&mut alice.core, personal, None, "Solo").id;
    alice.write(solo, "the page that will move");
    let posted = post(&mut alice, solo, None, text("carried along with the page"));
    Dev::settle(&mut [&mut alice], 2);
    call(
        &mut alice,
        Request::PageMoveToSpace {
            page_id: solo,
            space_id: space,
            parent_id: None,
            index: None,
        },
    )
    .unwrap();
    // Her own view follows at once: the same thread, now in the shared space, found there too.
    let own = threads(&mut alice, solo);
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].thread_id, posted.thread_id);
    assert_eq!(search(&mut alice, "carried", Some(space)).len(), 1);
    assert!(search(&mut alice, "carried", Some(personal)).is_empty());
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    // The members of the target space read it, and Bob comments on it.
    let seen = threads(&mut bob, solo);
    assert_eq!(seen.len(), 1);
    assert_eq!(words(&seen[0].comments[0]), "carried along with the page");
    assert_eq!(seen[0].comments[0].author_name, "Alice");
    post(&mut bob, solo, Some(posted.thread_id), text("seen it"));
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert_eq!(threads(&mut alice, solo)[0].comments.len(), 2);
}

#[test]
fn a_backup_keeps_the_comments_and_a_restore_brings_them_back_searchable() {
    let server = server();
    let (mut alice, kit) = Dev::first("alice", 1, &server);
    let space = crate::test_support::spaces(&mut alice.core)[0].id;
    let page = create_page(&mut alice.core, space, None, "Plan").id;
    let root = post(&mut alice, page, None, text("remember the backup"));
    post(
        &mut alice,
        page,
        Some(root.thread_id),
        text("and its replies"),
    );
    resolve(&mut alice, page, root.thread_id, true).unwrap();
    edit(
        &mut alice,
        page,
        root.comments[0].comment_id,
        "remember the backup, edited",
    )
    .unwrap();
    let before = threads(&mut alice, page);
    let bytes = crate::test_support::export_backup(&mut alice.core);

    let fx = Fixture::new();
    let (mut restored, _) = open_core(&fx, 7, 5_000_000);
    crate::test_support::import_backup(&mut restored, &bytes, Some(&kit));
    let after = match restored
        .dispatch(Request::CommentList { page_id: page })
        .unwrap()
    {
        Response::Comments(threads) => threads,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].thread_id, before[0].thread_id);
    assert!(after[0].resolved);
    assert_eq!(
        after[0].comments.iter().map(words).collect::<Vec<_>>(),
        before[0].comments.iter().map(words).collect::<Vec<_>>()
    );
    assert!(
        after[0].comments.iter().all(|c| c.is_mine),
        "the account is the same"
    );
    let hits = match restored
        .dispatch(Request::Search {
            query: "replies".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(hits.len(), 1);
    assert!(hits[0].comment_id.is_some());
    // The restored device carries on in the same stream.
    let more = restored
        .dispatch(Request::CommentPost {
            page_id: page,
            thread_id: Some(root.thread_id),
            anchor: None,
            body: text("after the restore"),
        })
        .unwrap();
    assert_eq!(thread_of(more).comments.len(), 3);
    // A Markdown export has no comments.
    let markdown = match restored
        .dispatch(Request::PageExportMarkdown { page_id: page })
        .unwrap()
    {
        Response::Markdown(markdown) => markdown,
        other => panic!("unexpected {other:?}"),
    };
    assert!(!markdown.contains("remember the backup"));
}

// ----- people and the Person property ----------------------------------------------------------------

#[test]
fn people_are_the_members_a_person_value_lists_members_and_the_filters_follow_the_viewer() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        notes,
    } = pair(MemberRole::Commenter);
    let (alice_account, bob_account) = (alice.account(), bob.account());
    let people = |dev: &mut Dev, space: SpaceId| -> Vec<PersonInfo> {
        match call(dev, Request::PeopleMentionable { space_id: space }).unwrap() {
            Response::People(people) => people,
            other => panic!("unexpected {other:?}"),
        }
    };
    // The members of the space in join order, this account included; a personal space: oneself.
    for (dev, me) in [(&mut alice, "Alice"), (&mut bob, "Bob")] {
        let listed = people(dev, space);
        assert_eq!(
            listed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Alice", "Bob"]
        );
        assert_eq!(
            listed
                .iter()
                .filter(|p| p.is_me)
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            [me]
        );
    }
    let personal = crate::test_support::spaces(&mut alice.core)
        .into_iter()
        .find(|info| info.share_state == SpaceShareState::Personal)
        .unwrap()
        .id;
    let own = people(&mut alice, personal);
    assert_eq!(own.len(), 1);
    assert!(own[0].is_me && own[0].account_id == alice_account);
    assert_eq!(
        call(
            &mut alice,
            Request::PeopleMentionable {
                space_id: SpaceId::from_bytes([3; 16])
            }
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );

    // A Person property: an Editor's write; members only; at most twenty.
    let property = create_person_property(&mut alice, space, "Owner");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    forbidden_role(
        &set_people(&mut bob, plan, property, &[bob_account]).unwrap_err(),
        "editor",
    );
    let stranger = AccountId::from_bytes([0xEE; 16]);
    invalid_field(
        &set_people(&mut alice, plan, property, &[stranger]).unwrap_err(),
        "value",
    );
    let too_many: Vec<AccountId> = (0..21)
        .map(|n| AccountId::from_bytes([n + 1; 16]))
        .collect();
    invalid_field(
        &set_people(&mut alice, plan, property, &too_many).unwrap_err(),
        "value",
    );
    set_people(&mut alice, plan, property, &[alice_account, bob_account]).unwrap();
    set_people(&mut alice, notes, property, &[bob_account]).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);

    // The same filter on two devices: "me" is each viewer's own.
    assert_eq!(
        pages_by(&mut alice, space, property, PropertyFilter::ContainsMe),
        vec![plan]
    );
    assert_eq!(
        pages_by(&mut bob, space, property, PropertyFilter::ContainsMe),
        vec![notes, plan]
    );
    assert_eq!(
        pages_by(
            &mut alice,
            space,
            property,
            PropertyFilter::ContainsPerson(bob_account)
        ),
        vec![notes, plan]
    );
    assert_eq!(
        pages_by(
            &mut alice,
            space,
            property,
            PropertyFilter::ContainsPerson(alice_account)
        ),
        vec![plan]
    );
    assert_eq!(
        pages_by(&mut alice, space, property, PropertyFilter::HasValue),
        vec![notes, plan]
    );
    assert!(
        pages_by(
            &mut alice,
            space,
            property,
            PropertyFilter::ContainsPerson(stranger)
        )
        .is_empty()
    );
    invalid_field(
        &fails(
            &mut alice,
            Request::PagesByProperty {
                space_id: space,
                property_id: property,
                filter: PropertyFilter::Equals(PropertyValue::Person(vec![bob_account])),
            },
        ),
        "filter",
    );
    // The names are searchable (the page, no block); the values read back.
    let hits = search(&mut alice, "Bob", Some(space));
    let found: Vec<PageId> = hits
        .iter()
        .filter(|hit| hit.block_id.is_none())
        .map(|hit| hit.page_id)
        .collect();
    assert!(found.contains(&plan) && found.contains(&notes), "{hits:?}");
    // A rename reaches the search text of the values that list the person.
    set_profile(&mut bob, "Robert");
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let renamed = search(&mut alice, "Robert", Some(space));
    assert!(
        renamed
            .iter()
            .any(|hit| hit.page_id == plan && hit.block_id.is_none()),
        "{renamed:?}"
    );
    assert!(
        search(&mut alice, "Bob", Some(space))
            .iter()
            .all(|hit| hit.page_id != notes),
        "the old name is gone from the text"
    );
    match call(&mut bob, Request::PagePropertiesGet { page_id: plan }).unwrap() {
        Response::PageProperties(values) => {
            assert!(values.values.iter().any(|(id, value)| *id == property
                && *value == PropertyValue::Person(vec![alice_account, bob_account])));
        }
        other => panic!("unexpected {other:?}"),
    }

    // Bob leaves; the values keep his account ("Former member"), the list can be written again with
    // him on it, and a stranger still cannot be added.
    call(
        &mut alice,
        Request::MemberRemove {
            space_id: space,
            account_id: bob_account,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice], 3);
    assert_eq!(
        pages_by(
            &mut alice,
            space,
            property,
            PropertyFilter::ContainsPerson(bob_account)
        ),
        vec![notes, plan]
    );
    set_people(&mut alice, plan, property, &[alice_account, bob_account]).unwrap();
    invalid_field(
        &set_people(
            &mut alice,
            plan,
            property,
            &[alice_account, bob_account, stranger],
        )
        .unwrap_err(),
        "value",
    );
    let listed = people(&mut alice, space);
    assert_eq!(
        listed.iter().map(|p| p.account_id).collect::<Vec<_>>(),
        vec![alice_account]
    );
    let _ = feed;
}
