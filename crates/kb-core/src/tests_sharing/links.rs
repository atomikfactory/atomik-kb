//! Milestone 13, slice 4: view links, on the people of the simulator in `super` (accounts with devices on
//! one `MemoryRemote`, whose cycle includes the driver's link step). The scenarios drive the three requests
//! the way the UI does and read what a viewer would: the publication on the server, opened with the key in
//! the URL. They check who may make a link, that the publication is a history-free page without comments,
//! that member devices refresh it after a minute (and Viewers and Commenters never do), revocation, the
//! Owner's tombstone of an abandoned record, closing a space, and a personal space's links.

use kb_doc::{Doc, DocKind};
use kb_keys::KeySubject;
use kb_keys::link::LinkKey;
use kb_model::{Inline, PageDoc};
use kb_sync::SyncError;
use kb_sync::v2::{LinkFetch, Publication, open_publication};
use kb_types::sync::PULL_INTERVAL_MS;
use kb_types::{CommentBodyItem, LinkId, Request, Response, ViewLinkInfo, ViewLinkStatus};
use serde_json::json;

use super::requests::*;
use super::scenarios::{forbidden_archived, forbidden_role, invalid_field};
use super::*;

const MINUTE_MS: i64 = 60_000;

// ----- helpers ----------------------------------------------------------------------------------

fn create(
    dev: &mut Dev,
    space: SpaceId,
    page: PageId,
    days: Option<u32>,
) -> Result<ViewLinkInfo, ApiError> {
    match call(
        dev,
        Request::ViewLinkCreate {
            space_id: space,
            page_id: page,
            expires_in_days: days,
        },
    )? {
        Response::ViewLink(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn list(dev: &mut Dev, space: SpaceId) -> Vec<ViewLinkInfo> {
    match call(dev, Request::ViewLinkList { space_id: space }).unwrap() {
        Response::ViewLinks(links) => links,
        other => panic!("unexpected {other:?}"),
    }
}

fn revoke(dev: &mut Dev, space: SpaceId, link: LinkId) -> Result<Vec<ViewLinkInfo>, ApiError> {
    match call(
        dev,
        Request::ViewLinkRevoke {
            space_id: space,
            link_id: link,
        },
    )? {
        Response::ViewLinks(links) => Ok(links),
        other => panic!("unexpected {other:?}"),
    }
}

/// The link key of a URL, as a viewer reads it from the fragment.
fn key_of(info: &ViewLinkInfo) -> LinkKey {
    let (_, fragment) = info.url.split_once("/view#").expect("a view URL");
    assert_eq!(fragment.len(), 43, "the key is 43 characters");
    LinkKey::from_fragment(fragment).unwrap()
}

/// What a viewer gets from the server: the `pub_seq` and the opened publication, `None` for the one
/// answer a revoked, expired or unknown link has.
fn published(dev: &Dev, key: &LinkKey) -> Option<(u64, Publication)> {
    match dev.remote.link_get_sync(key.link_id(), None) {
        Ok(LinkFetch::Publication { pub_seq, bytes }) => {
            Some((pub_seq, open_publication(key, &bytes).unwrap()))
        }
        Ok(LinkFetch::NotModified) => panic!("no If-None-Match was sent"),
        Err(SyncError::NotFound) => None,
        Err(other) => panic!("unexpected {other:?}"),
    }
}

/// The text of the blocks of a publication's snapshot, as a viewer's `loro-crdt` reads them.
fn texts_of(publication: &Publication) -> Vec<String> {
    let doc = Doc::load(DocKind::Page, 0x7100, Some(&publication.snapshot), &[]).unwrap();
    let model = PageDoc::from_doc(doc).unwrap();
    model
        .blocks()
        .into_iter()
        .map(|block| {
            block
                .inlines
                .iter()
                .filter_map(|inline| match inline {
                    Inline::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn later(dev: &Dev, ms: i64) {
    dev.clock.set(dev.clock.now_ms() + ms);
}

fn add_third(alice: &mut Dev, space: SpaceId, feed: FeedId, role: MemberRole) -> Dev {
    let server = alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    add_member(alice, &mut carol, feed, space, role, "Carol");
    carol
}

fn allow_editors(alice: &mut Dev, space: SpaceId) {
    call(
        alice,
        Request::SpaceSettingsSet {
            space_id: space,
            trash_retention_days: None,
            editors_may_link: Some(true),
        },
    )
    .unwrap();
}

/// Appends portable blocks to a page's document the way an editor's save does.
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
        kb_types::ChangeSource::Editor,
    )
    .unwrap();
    core.page_doc_close(page).unwrap();
}

/// Deletes the block whose text is `needle`, as an editor's save does.
fn delete_block_with(dev: &mut Dev, page: PageId, needle: &str) {
    let core = &mut dev.core;
    let snapshot = core.page_doc_open(page).unwrap();
    let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
    let doc =
        PageDoc::from_doc(Doc::load(DocKind::Page, 0x7200, Some(&blob), &[]).unwrap()).unwrap();
    let before = doc.doc().version();
    let target = doc
        .blocks()
        .into_iter()
        .find(|block| {
            block
                .inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Text(text) if text.contains(needle)))
        })
        .expect("a block with that text")
        .id;
    doc.delete_block(target).unwrap();
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page, payload).unwrap();
    core.page_doc_close(page).unwrap();
}

fn subject(link: LinkId) -> KeySubject {
    KeySubject::Link { link_id: link }
}

fn tombstoned(dev: &Dev, space: SpaceId, link: LinkId) -> bool {
    dev.core
        .security
        .keyring
        .is_tombstoned(space, &subject(link))
}

// ----- tests ------------------------------------------------------------------------------------

#[test]
fn an_owner_makes_a_link_an_editor_needs_the_setting_and_nobody_below_editor_may() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        notes,
    } = pair(MemberRole::Editor);
    let mut carol = add_third(&mut alice, space, feed, MemberRole::Commenter);
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);

    // The setting is off: an Editor is told to ask an Owner, a Commenter that an Editor is the least.
    forbidden_role(&create(&mut bob, space, plan, None).unwrap_err(), "owner");
    forbidden_role(
        &create(&mut carol, space, plan, None).unwrap_err(),
        "editor",
    );

    // The Owner's input is checked: the days, and one answer for a page that cannot be linked.
    for days in [0, 366] {
        invalid_field(
            &create(&mut alice, space, plan, Some(days)).unwrap_err(),
            "expiresInDays",
        );
    }
    invalid_field(
        &create(&mut alice, space, PageId::from_bytes([9; 16]), None).unwrap_err(),
        "pageId",
    );
    call(&mut alice, Request::PageTrash { page_id: notes }).unwrap();
    invalid_field(
        &create(&mut alice, space, notes, None).unwrap_err(),
        "pageId",
    );

    // The Owner's link: unpublished until the next cycle, the key in the URL fragment, the name of its maker.
    alice.events();
    let info = create(&mut alice, space, plan, Some(7)).unwrap();
    assert_eq!(info.status, ViewLinkStatus::Publishing);
    assert_eq!(info.page_id, plan);
    assert_eq!(info.created_by_name, "Alice");
    assert_eq!(info.created_by, alice.account());
    assert_eq!(info.expires_at, Some(alice.clock.now_ms() + 7 * DAY_MS));
    assert_eq!(info.last_published_at, None);
    assert!(info.url.starts_with(&format!("{SERVER_URL}/view#")));
    assert_eq!(key_of(&info).link_id(), info.link_id);
    let events = alice.events();
    assert!(events.contains(&Event::ViewLinksChanged { space_id: space }));
    assert!(events.contains(&Event::SyncRequested));
    assert!(
        published(&alice, &key_of(&info)).is_none(),
        "nothing is uploaded yet"
    );

    // The setting on: the same Editor may.
    allow_editors(&mut alice, space);
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    later(&bob, 1_000);
    let mine = create(&mut bob, space, plan, None).unwrap();
    assert_eq!(mine.created_by_name, "Bob");
    assert_eq!(mine.expires_at, None);
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    // Every member lists both, newest first. The URL was shown once, in the answer of the request that made
    // the link: no list shows it, not even to the member who made it.
    for dev in [&mut alice, &mut bob, &mut carol] {
        let links = list(dev, space);
        assert_eq!(
            links.iter().map(|l| l.link_id).collect::<Vec<_>>(),
            [mine.link_id, info.link_id],
            "{}",
            dev.name
        );
        assert!(
            links.iter().all(|link| link.url.is_empty()),
            "{} sees no URL",
            dev.name
        );
    }
}

#[test]
fn the_first_upload_makes_the_link_live_and_the_publication_opens_with_the_key() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let info = create(&mut alice, space, plan, None).unwrap();
    let key = key_of(&info);
    alice.cycle().unwrap();

    let (pub_seq, publication) = published(&alice, &key).expect("the link is live");
    assert_eq!(pub_seq, 1);
    assert_eq!(publication.page_id, plan);
    assert_eq!(publication.title, "Plan");
    assert!(texts_of(&publication).contains(&"base line".to_owned()));
    assert!(publication.attachments.is_empty() && publication.mentions.is_empty());
    let listed = list(&mut alice, space);
    assert_eq!(listed[0].status, ViewLinkStatus::Live);
    assert_eq!(listed[0].last_published_at, Some(alice.clock.now_ms()));
    assert_eq!(
        listed[0].url, "",
        "the URL is shown once, by the request that made the link"
    );

    // The server learned nothing it can read: another key opens nothing.
    let other = LinkKey::generate(&kb_types::testing::SeededRng::new(5));
    let Ok(LinkFetch::Publication { bytes, .. }) = alice.remote.link_get_sync(key.link_id(), None)
    else {
        panic!("the link is served");
    };
    assert!(open_publication(&other, &bytes).is_err());

    // Another member lists it as live too (the creator's upload is not theirs to know), without the URL.
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    let seen = list(&mut bob, space);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url, "");
    assert_eq!(seen[0].status, ViewLinkStatus::Live);
    assert_eq!(seen[0].last_published_at, None);
}

#[test]
fn a_publication_has_no_deleted_text_no_comments_and_carries_attachments_and_the_names_mentioned() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let mut carol = add_third(&mut alice, space, feed, MemberRole::Commenter);
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    alice.write(plan, "TOPSECRET words");
    delete_block_with(&mut alice, plan, "TOPSECRET");
    alice.write(plan, "kept words");
    // A picture, and a mention of Bob.
    let picture = alice.upload_attachment(space, "diagram.png", &[7u8; 5_000]);
    append_blocks(
        &mut alice,
        plan,
        json!([
            { "type": "image", "attrs": {
                "id": kb_types::BlockId::from_random_bytes([41; 16]).to_string(),
                "attachmentId": picture.attachment_id.to_string(), "alt": "" } },
            { "type": "paragraph",
              "attrs": { "id": kb_types::BlockId::from_random_bytes([42; 16]).to_string() },
              "content": [
                { "type": "text", "text": "cc " },
                { "type": "personMention",
                  "attrs": { "accountId": bob.account().to_string(), "label": "someone" } }
              ] }
        ]),
    );
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    // Carol comments on the page.
    call(
        &mut carol,
        Request::CommentPost {
            page_id: plan,
            thread_id: None,
            anchor: None,
            body: vec![CommentBodyItem::Text {
                text: "COMMENTWORDS".into(),
            }],
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);

    let info = create(&mut alice, space, plan, None).unwrap();
    let key = key_of(&info);
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    let (_, publication) = published(&alice, &key).unwrap();

    let texts = texts_of(&publication);
    assert!(texts.contains(&"kept words".to_owned()), "{texts:?}");
    assert!(texts.iter().all(|text| !text.contains("TOPSECRET")));
    assert!(!contains(&publication.snapshot, b"TOPSECRET"), "no history");
    assert!(
        !contains(&publication.snapshot, b"COMMENTWORDS"),
        "no comments"
    );
    // The attachment travels with its key and manifest; its chunks are the ones the server serves.
    assert_eq!(publication.attachments.len(), 1);
    let attached = &publication.attachments[0];
    assert_eq!(attached.attachment_id, picture.attachment_id);
    assert_eq!(
        (attached.name.as_str(), attached.size),
        ("diagram.png", 5_000)
    );
    for chunk in &attached.chunks {
        alice
            .remote
            .link_chunk_get_sync(key.link_id(), *chunk)
            .expect("a declared chunk is served");
    }
    // The name of the person mentioned is the one the space knows, not the label in the text.
    assert_eq!(publication.mentions.len(), 1);
    assert_eq!(publication.mentions[0].account_id, bob.account());
    assert_eq!(publication.mentions[0].name, "Bob");
}

#[test]
fn an_edit_on_any_editors_device_refreshes_the_link_after_a_minute_and_a_burst_publishes_once() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let info = create(&mut alice, space, plan, None).unwrap();
    let key = key_of(&info);
    alice.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert_eq!(published(&alice, &key).unwrap().0, 1);

    // Bob edits twice: nothing goes out within the debounce, and nothing before a minute has passed.
    bob.write(plan, "bob wrote this");
    bob.write(plan, "and this");
    bob.cycle().unwrap();
    later(&bob, MINUTE_MS - 1_000);
    bob.cycle().unwrap();
    assert_eq!(
        published(&alice, &key).unwrap().0,
        1,
        "a minute has not passed"
    );
    later(&bob, 2_000);
    bob.cycle().unwrap();
    let (pub_seq, publication) = published(&alice, &key).unwrap();
    assert_eq!(pub_seq, 2, "the burst made one publication");
    let texts = texts_of(&publication);
    assert!(texts.contains(&"bob wrote this".to_owned()) && texts.contains(&"and this".to_owned()));
    bob.cycle().unwrap();
    assert_eq!(
        published(&alice, &key).unwrap().0,
        2,
        "nothing is left to publish"
    );

    // Alice applied Bob's entries: her device refreshes too, after her own minute.
    alice.cycle().unwrap();
    alice.cycle().unwrap();
    assert_eq!(published(&alice, &key).unwrap().0, 2);
    later(&alice, MINUTE_MS + 1_000);
    alice.cycle().unwrap();
    assert_eq!(published(&alice, &key).unwrap().0, 3);

    // A change that arrives while an upload is under way is not lost: the work is made, the edit
    // comes, the upload is reported, and a note stays for a minute later.
    alice.write(plan, "while uploading");
    later(&alice, MINUTE_MS + 1_000);
    let work = alice.core.sync_link_work().unwrap();
    assert_eq!(work.len(), 1);
    alice.write(plan, "after the work was made");
    let kb_sync::v2::LinkWork::Put { link_id, put } = work.into_iter().next().unwrap() else {
        panic!("an upload");
    };
    let pub_seq = alice.remote.link_put_sync(link_id, &put).unwrap();
    alice
        .core
        .sync_link_done(link_id, kb_sync::v2::LinkOutcome::Published { pub_seq })
        .unwrap();
    assert!(alice.core.sync_link_work().unwrap().is_empty());
    later(&alice, MINUTE_MS + 1_000);
    assert_eq!(alice.core.sync_link_work().unwrap().len(), 1);
}

#[test]
fn a_viewer_and_a_commenter_never_refresh_a_link() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Viewer);
    let mut carol = add_third(&mut alice, space, feed, MemberRole::Commenter);
    let info = create(&mut alice, space, plan, None).unwrap();
    alice.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    alice.write(plan, "news");
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol], 3);
    assert!(bob.texts(plan).contains(&"news".to_owned()));

    for dev in [&mut bob, &mut carol] {
        later(dev, 10 * MINUTE_MS);
        assert!(
            dev.core.sync_link_work().unwrap().is_empty(),
            "{}",
            dev.name
        );
        let links = list(dev, space);
        assert_eq!(
            (links.len(), links[0].url.as_str(), links[0].status),
            (1, "", ViewLinkStatus::Live),
            "{} lists the link, not its URL",
            dev.name
        );
    }
    let key = key_of(&info);
    assert_eq!(published(&alice, &key).unwrap().0, 1);
    // They may not revoke it either.
    forbidden_role(&revoke(&mut bob, space, info.link_id).unwrap_err(), "owner");
}

#[test]
fn revoking_deletes_the_publication_tombstones_the_record_and_empties_every_members_list() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let info = create(&mut alice, space, plan, None).unwrap();
    let key = key_of(&info);
    alice.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(published(&alice, &key).is_some());

    // An Editor who did not make the link, and a link the space does not have.
    forbidden_role(&revoke(&mut bob, space, info.link_id).unwrap_err(), "owner");
    invalid_field(
        &revoke(&mut alice, space, LinkId::from_bytes([7; 16])).unwrap_err(),
        "linkId",
    );

    bob.events();
    assert!(revoke(&mut alice, space, info.link_id).unwrap().is_empty());
    assert!(tombstoned(&alice, space, info.link_id));
    alice.cycle().unwrap();
    assert!(
        published(&alice, &key).is_none(),
        "the server forgot the link"
    );
    assert!(
        alice.core.sync_link_work().unwrap().is_empty(),
        "the deletion is done"
    );
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(list(&mut bob, space).is_empty());
    assert!(tombstoned(&bob, space, info.link_id));
    assert!(
        bob.events()
            .contains(&Event::ViewLinksChanged { space_id: space })
    );
}

#[test]
fn an_editor_who_revokes_their_link_leaves_the_tombstone_to_an_owners_device() {
    let Pair {
        mut alice,
        mut bob,
        space,
        notes,
        ..
    } = pair(MemberRole::Editor);
    allow_editors(&mut alice, space);
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    let info = create(&mut bob, space, notes, None).unwrap();
    let key = key_of(&info);
    bob.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(published(&bob, &key).is_some());

    // The creator revokes: the server row and the entry go; the record stays (his tombstone would be refused).
    assert!(revoke(&mut bob, space, info.link_id).unwrap().is_empty());
    assert!(!tombstoned(&bob, space, info.link_id));
    bob.cycle().unwrap();
    assert!(published(&bob, &key).is_none());

    // Alice's device sees the record without its entry, waits a pull interval, and tombstones it.
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(list(&mut alice, space).is_empty());
    assert!(
        !tombstoned(&alice, space, info.link_id),
        "it may be in flight"
    );
    later(&alice, PULL_INTERVAL_MS as i64 + 1_000);
    alice.cycle().unwrap();
    alice.cycle().unwrap();
    assert!(tombstoned(&alice, space, info.link_id));
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(tombstoned(&bob, space, info.link_id));
}

#[test]
fn closing_a_space_deletes_its_links() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let info = create(&mut alice, space, plan, None).unwrap();
    let key = key_of(&info);
    alice.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert!(published(&alice, &key).is_some());

    call(&mut alice, Request::SpaceClose { space_id: space }).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(
        published(&alice, &key).is_none(),
        "the server deleted the link with the feed"
    );
    // The archive lists the link as revoked and nobody can make or revoke one in it.
    for dev in [&mut alice, &mut bob] {
        let links = list(dev, space);
        assert_eq!(links.len(), 1, "{}", dev.name);
        assert_eq!(links[0].status, ViewLinkStatus::Revoked, "{}", dev.name);
        assert!(dev.core.sync_link_work().unwrap().is_empty());
    }
    forbidden_archived(&create(&mut alice, space, plan, None).unwrap_err());
    forbidden_archived(&revoke(&mut alice, space, info.link_id).unwrap_err());
}

#[test]
fn a_personal_space_has_links_and_an_expired_one_is_reported() {
    let server = server();
    let (mut solo, _) = Dev::first("solo", 1, &server);
    let space = crate::test_support::spaces(&mut solo.core)[0].id;
    let page = create_page(&mut solo.core, space, None, "Diary").id;
    solo.write(page, "dear diary");
    // Without a server there is nobody to publish to.
    invalid_field(
        &create(&mut solo, space, page, None).unwrap_err(),
        "serverUrl",
    );
    solo.configure();
    solo.cycle().unwrap();

    let info = create(&mut solo, space, page, Some(1)).unwrap();
    let key = key_of(&info);
    solo.cycle().unwrap();
    let (_, publication) = published(&solo, &key).expect("a personal page can be linked");
    assert_eq!(publication.title, "Diary");
    assert!(texts_of(&publication).contains(&"dear diary".to_owned()));
    assert_eq!(list(&mut solo, space)[0].status, ViewLinkStatus::Live);

    // A day later the link has expired: the core says so (the server's clock is its own).
    solo.events();
    later(&solo, DAY_MS + 1_000);
    solo.cycle().unwrap();
    assert_eq!(list(&mut solo, space)[0].status, ViewLinkStatus::Expired);
    assert!(
        solo.events()
            .contains(&Event::ViewLinksChanged { space_id: space })
    );
    assert!(
        solo.core.sync_link_work().unwrap().is_empty(),
        "no upload for an expired link"
    );

    // Revoking works there too, and the deletion survives a restart.
    assert!(revoke(&mut solo, space, info.link_id).unwrap().is_empty());
    let now = solo.clock.now_ms();
    solo.reopen(11, now);
    assert_eq!(
        solo.core.sync_link_work().unwrap().len(),
        1,
        "the deletion is still queued"
    );
    solo.cycle().unwrap();
    assert!(published(&solo, &key).is_none());
    assert!(solo.core.sync_link_work().unwrap().is_empty());
}

#[test]
fn a_rename_on_another_device_refreshes_the_link_and_purging_the_page_revokes_it() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        notes,
        ..
    } = pair(MemberRole::Editor);
    let info = create(&mut alice, space, plan, None).unwrap();
    let doomed = create(&mut alice, space, notes, None).unwrap();
    let (key, doomed_key) = (key_of(&info), key_of(&doomed));
    alice.cycle().unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert_eq!(published(&alice, &key).unwrap().0, 1);

    // Bob renames the page: a structure edit, which the publication shows too.
    call(
        &mut bob,
        Request::PageRename {
            page_id: plan,
            title: "Plan v2".into(),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    later(&alice, MINUTE_MS + 1_000);
    alice.cycle().unwrap();
    let (pub_seq, publication) = published(&alice, &key).unwrap();
    assert_eq!((pub_seq, publication.title.as_str()), (2, "Plan v2"));

    // Purging the other page takes its link with it, and the record is tombstoned by the Owner.
    assert!(published(&alice, &doomed_key).is_some());
    call(&mut alice, Request::PageTrash { page_id: notes }).unwrap();
    call(&mut alice, Request::PagePurge { page_id: notes }).unwrap();
    assert!(tombstoned(&alice, space, doomed.link_id));
    alice.cycle().unwrap();
    assert!(published(&alice, &doomed_key).is_none());
    assert_eq!(list(&mut alice, space).len(), 1);
}
