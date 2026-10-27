//! A view link through the whole stack on the reference remote (tokio's paused clock): a real core makes the
//! link, the v2 driver uploads the publication, a viewer's read with the key from the URL opens it, a rename
//! refreshes it a minute later, and revoking deletes it from the server. The same scenario against
//! `kb-server` over HTTP is C's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use kb_keys::link::LinkKey;
use kb_sync::SyncError;
use kb_sync::v2::{LinkFetch, MemoryRemote, Publication, open_publication};
use kb_types::{Request, Response, ViewLinkInfo, ViewLinkStatus};
use support::{Env, until};

fn listed(peer: &support::Peer, space: kb_types::SpaceId) -> Vec<ViewLinkInfo> {
    match peer
        .request(Request::ViewLinkList { space_id: space })
        .unwrap()
    {
        Response::ViewLinks(links) => links,
        other => panic!("unexpected {other:?}"),
    }
}

/// What a viewer reads: the sealed publication opened with the key, `None` for the one 404.
fn read(server: &MemoryRemote, key: &LinkKey) -> Option<(u64, Publication)> {
    match server.link_get_sync(key.link_id(), None) {
        Ok(LinkFetch::Publication { pub_seq, bytes }) => {
            Some((pub_seq, open_publication(key, &bytes).unwrap()))
        }
        Ok(LinkFetch::NotModified) => panic!("no If-None-Match was sent"),
        Err(SyncError::NotFound) => None,
        Err(other) => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_view_link_is_published_refreshed_and_revoked_through_the_driver() {
    support::init_logging();
    let env = Env::memory();
    let server = env.memory.clone().unwrap();
    let (alice, _network, space, plan) = env.alice_with_a_project();

    // The link, and the one answer that shows its URL.
    let info = match alice
        .request(Request::ViewLinkCreate {
            space_id: space,
            page_id: plan,
            expires_in_days: None,
        })
        .unwrap()
    {
        Response::ViewLink(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    let (_, fragment) = info.url.split_once("/view#").unwrap();
    let key = LinkKey::from_fragment(fragment).unwrap();
    assert_eq!(key.link_id(), info.link_id);
    assert_eq!(info.status, ViewLinkStatus::Publishing);

    // The driver's first cycle that has an account uploads it.
    until("the link is live", || {
        listed(&alice, space)
            .first()
            .is_some_and(|link| link.status == ViewLinkStatus::Live)
    })
    .await;
    let (pub_seq, publication) = read(&server, &key).expect("the server serves the link");
    assert_eq!((pub_seq, publication.title.as_str()), (1, "Plan"));
    assert_eq!(
        listed(&alice, space)[0].url,
        "",
        "the list never shows the URL"
    );

    // A rename changes the publication; a minute later (the debounce) the driver sends it.
    alice.rename(plan, "Plan, second edition").unwrap();
    env.advance(61_000);
    until("the title is refreshed", || {
        read(&server, &key).is_some_and(|(_, publication)| publication.title.ends_with("edition"))
    })
    .await;
    assert_eq!(read(&server, &key).unwrap().0, 2);

    // Revoking deletes the server's row at the next cycle, and nothing is left to do after it.
    alice
        .request(Request::ViewLinkRevoke {
            space_id: space,
            link_id: info.link_id,
        })
        .unwrap();
    until("the server forgot the link", || {
        read(&server, &key).is_none()
    })
    .await;
    assert!(listed(&alice, space).is_empty());
    until("the deletion is done", || {
        alice.with(|core| core.sync_link_work()).unwrap().is_empty()
    })
    .await;
}
