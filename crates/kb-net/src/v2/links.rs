//! The view-link step of a cycle of the v2 driver (M13 slice 4, ADR-0051, `docs/development/milestone-13.md`
//! 4.2 and its contract notes): after the feeds of the cycle (their chunks are uploaded by then, so the
//! attachments a publication declares are on the server), the driver asks the host for the link work that
//! is due and does it.
//!
//! ```text
//! sync_link_work()                 -> [LinkWork::Put { link_id, put } | LinkWork::Delete { link_id }]
//! Put    -> remote.link_put(link_id, &put)      with_session!: a 401 authenticates once and retries
//! Delete -> remote.link_delete(link_id)
//! sync_link_done(link_id, outcome)              one call per piece of work that ended
//! ```
//!
//! Outcomes: `Ok(pub_seq)` of a `Put` is `LinkOutcome::Published { pub_seq }`; `Ok` and `NotFound` of a
//! `Delete` are `LinkOutcome::Deleted`; a `Put` answered `Forbidden`, `NotFound`, `Closed`, `Conflict` (the
//! account is out of the feed, or the id belongs to another feed) or `Quota` (the server's `too_large`: a
//! publication that does not shrink by being sent again) and a `Delete` answered `Forbidden` are
//! `LinkOutcome::Refused` (the core stops offering that work until something changes). A failure that is
//! not the link's (`Unavailable`, `Unauthorized`, a transport error, a failure of the core) ends the cycle
//! like it does in a feed's step and is **not** reported: the work comes back with the next cycle. A
//! failure of the protocol itself (`Protocol`, `Encoding`) is not reported either; its message goes to the
//! cycle's problems like any other step's, and the pieces after it still run. The step runs only when the
//! cycle has a session and the feeds ran (a forked personal feed pauses it with the uploads), at most
//! `TRANSFER_BUDGET` pieces per cycle (`Summary::budget_used` says more is waiting). The core is asked
//! once per cycle, and a piece is sent, reported and dropped before the next, so no more than one
//! publication is held at a time beyond the list. Nothing is logged but counts: never a link id, a
//! publication, a feed or a URL.

use std::sync::Arc;

use kb_sync::SyncError;
use kb_sync::v2::{LinkOutcome, LinkWork};
use kb_types::FeedId;
use kb_types::sync::TRANSFER_BUDGET;

use super::SyncRemote;
use super::driver::{Cx, Driver, Summary, feed_failure};
use crate::NetError;

/// What one piece of work came to.
#[derive(Default)]
struct Done {
    published: usize,
    deleted: usize,
    refused: usize,
    waiting: usize,
}

impl<R: SyncRemote> Driver<R> {
    /// The link step of the cycle (see the module documentation).
    pub(super) async fn work_links(
        &mut self,
        remote: &Arc<R>,
        cx: Cx,
        summary: &mut Summary,
    ) -> Result<(), NetError> {
        // A forked personal feed pauses the uploads (and so the link step), like the feeds' own.
        let personal = FeedId::from(cx.who.0);
        let info = self.host_call(|host| host.sync_info()).await?;
        let forked = if info.feeds.is_empty() {
            info.fork_detected
        } else {
            info.feeds
                .iter()
                .any(|feed| feed.feed_id == personal && feed.forked)
        };
        if forked {
            return Ok(());
        }
        let work = self.host_call(|host| host.sync_link_work()).await?;
        if work.is_empty() {
            return Ok(());
        }
        let handed_out = work.len();
        let mut done = Done::default();
        for (index, piece) in work.into_iter().enumerate() {
            if index >= TRANSFER_BUDGET {
                summary.budget_used = true;
                break;
            }
            let link_id = piece.link_id();
            let result = match piece {
                LinkWork::Put { link_id, put } => {
                    let who = cx.who;
                    with_session!(self, remote, who, remote.link_put(link_id, &put))
                        .map(|pub_seq| LinkOutcome::Published { pub_seq })
                        .or_else(|error| match error {
                            NetError::Sync(
                                SyncError::Forbidden
                                | SyncError::NotFound
                                | SyncError::Closed
                                | SyncError::Conflict(_)
                                | SyncError::Quota,
                            ) => Ok(LinkOutcome::Refused),
                            other => Err(other),
                        })
                }
                LinkWork::Delete { link_id } => {
                    let who = cx.who;
                    with_session!(self, remote, who, remote.link_delete(link_id))
                        .map(|()| LinkOutcome::Deleted)
                        .or_else(|error| match error {
                            NetError::Sync(SyncError::NotFound) => Ok(LinkOutcome::Deleted),
                            NetError::Sync(SyncError::Forbidden) => Ok(LinkOutcome::Refused),
                            other => Err(other),
                        })
                }
            };
            match result {
                Ok(outcome) => {
                    match outcome {
                        LinkOutcome::Published { .. } => done.published += 1,
                        LinkOutcome::Deleted => done.deleted += 1,
                        LinkOutcome::Refused => done.refused += 1,
                    }
                    self.host_call(move |host| host.sync_link_done(link_id, outcome))
                        .await?;
                }
                Err(error) => {
                    // The step's own failures name themselves; anything else ends the cycle.
                    let Some(message) = feed_failure(&error) else {
                        return Err(error);
                    };
                    if message == crate::driver::QUOTA_FULL {
                        summary.quota_full = true;
                    } else {
                        summary.problems.push(message);
                    }
                    done.waiting += 1;
                }
            }
        }
        tracing::debug!(
            handed_out,
            published = done.published,
            deleted = done.deleted,
            refused = done.refused,
            waiting = done.waiting,
            "view links"
        );
        Ok(())
    }
}
