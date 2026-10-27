//! The sync side of comments (ADR-0048, ADR-0045): the records of an entry or a snapshot of a page's
//! comment stream are verified one by one, applied together or not at all, and the changes they bring
//! that concern this account raise notifications.
//!
//! **What is checked** (nothing is stored before every record passed):
//!
//! | Check | Fails as |
//! |---|---|
//! | the blob is a records blob of the item's kind (an entry's: updates with one record each; a snapshot's: a record set) and every record decodes | quarantined |
//! | the record's `page_id` is the stream's page | quarantined |
//! | an **entry's** record was written by the entry's account (`author == writer`); a snapshot's records may have other authors, it is the set a member vouches for | quarantined |
//! | the signature of the record verifies under the key of `record.device`, a device of `record.author` as the logs list it (revoked devices count: records outlive them) | quarantined; the item waits when that log is not held yet |
//! | the author's role at the carrying cursor is Commenter or up (a snapshot: the highest role the author held at any membership cursor up to what it covers, because the record's own cursor is not known) | quarantined |
//! | a `Post` does not name a comment id the stream defines already (ids are random: a second claim is a forgery, and `at` is only a claim) | quarantined |
//! | a `Resolve` names the thread's root comment (`thread_id == ThreadId::of(comment_id)`), an `Edit` or a `Delete` the thread its comment's `Post` is in (when that `Post` is held: otherwise the fold decides when it arrives) | quarantined |
//!
//! What the fold ignores (an `Edit` by somebody else, a `Resolve` by a Commenter who is not the
//! thread's author, a reply with no root) is **not** quarantined: it is stored, and it counts when the
//! record it waits for arrives. A record id already held is skipped, so replays are harmless.
//!
//! **Notifications** (remote changes only; `notifications.rs`): for each new record that counts after
//! the fold, by someone else than this account: a `Post` that replies in a thread this account wrote in
//! (`ThreadReply`), a `Post` or an `Edit` whose folded body mentions this account where the body before
//! it did not (`CommentMention`, which wins when a reply also mentions). Nothing is raised while the
//! feed is still being read for the first time (a new device must not announce a space's history), and
//! a snapshot that brings the first records of a stream is that history too.

use std::collections::BTreeSet;

use kb_crypto::Signature;
use kb_doc::BlobKind;
use kb_model::{
    CommentOp, CommentRecord, CommentStream, VerifiedRecord, snapshot_dominates,
    snapshot_record_ids,
};
use kb_storage::{
    CommentQueries, DocKindCode, DocQueries, FeedQueries, NewNotification, UpdateOrigin,
};
use kb_types::comments::COMMENT_RECORD_CONTEXT;
use kb_types::sync::FeedKind;
use kb_types::{
    AccountId, ApiError, CommentId, DeviceId, DocId, ErrorCode, Event, FeedId, MemberRole,
    NotificationKind, PageId, SpaceId,
};

use super::{body_mentions, comment_row};
use crate::documents::new_doc_row;
use crate::{Core, map_storage_error};

/// What applying the records of one item did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentApply {
    /// Every record is stored (or was held already).
    Applied,
    /// A record failed a check: the item is quarantined and nothing of it was applied.
    Quarantined,
    /// The log of a device a record names is not held yet: nothing was applied, the driver fetches
    /// the logs and offers the item again.
    NeedsLog,
    /// No page this device knows has this stream: nothing was applied, the item waits for the
    /// structure that creates the page.
    PageUnknown,
}

/// Who carried the records of an item and how.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Carried {
    /// The account that wrote the item (an entry's header, a snapshot's header).
    pub(crate) writer: AccountId,
    /// The device that wrote it (the same header). Page history records it as the author of the entry.
    pub(crate) device: DeviceId,
    /// A snapshot (a record set, the authors may differ from the writer) or an entry.
    pub(crate) snapshot: bool,
}

/// What looking for the key of a record's signer found.
enum SignerKey {
    Found(kb_crypto::VerifyingKey),
    /// The signer's log is not here yet.
    NeedsLog,
    /// The account has no such device (a forged claim), or is not this account's.
    Unknown,
}

impl Core {
    /// Applies the records of one comment-stream item that arrived through sync (`feeds/pull.rs`
    /// calls it for a `DocKindCode::Comments` item that `shared_doc_acceptable` took, after the
    /// role check of the item): `blobs` are the entry's updates or the snapshot's one blob; `cursor`
    /// is the item's feed cursor (a snapshot's: what it covers). See the module documentation for what
    /// is checked and what the answers mean.
    pub(crate) fn apply_comment_updates(
        &mut self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
        blobs: &[Vec<u8>],
        carried: Carried,
        now: i64,
    ) -> Result<CommentApply, ApiError> {
        let shared = self.feed_row(feed)?.kind == FeedKind::Shared;
        let hint = if shared { self.feed_space(feed)? } else { None };
        let Some((space, page)) = self.comment_stream_page(hint, doc_id) else {
            return Ok(CommentApply::PageUnknown);
        };
        // The records, as the item holds them.
        let wanted = if carried.snapshot {
            BlobKind::Snapshot
        } else {
            BlobKind::Update
        };
        let mut incoming: Vec<(CommentRecord, Vec<u8>)> = Vec::new();
        for blob in blobs {
            let Ok(parsed) = kb_doc::decode_records(blob) else {
                return Ok(CommentApply::Quarantined);
            };
            if parsed.kind != wanted {
                return Ok(CommentApply::Quarantined);
            }
            for bytes in parsed.records {
                let Ok(record) = CommentRecord::decode(bytes) else {
                    return Ok(CommentApply::Quarantined);
                };
                incoming.push((record, bytes.to_vec()));
            }
        }

        let before = self.comment_stream_of(page)?;
        let mut checked: Vec<(CommentRecord, Vec<u8>, MemberRole)> = Vec::new();
        let mut posts_in_item: BTreeSet<CommentId> = BTreeSet::new();
        for (record, bytes) in incoming {
            if record.page_id != page {
                return Ok(CommentApply::Quarantined);
            }
            if before.contains(record.record_id)
                || checked
                    .iter()
                    .any(|(other, _, _)| other.record_id == record.record_id)
            {
                continue;
            }
            if !carried.snapshot && record.author != carried.writer {
                return Ok(CommentApply::Quarantined);
            }
            // The signature, under the key of the device the record names.
            let key = match self.comment_signer_key(shared, record.author, record.device)? {
                SignerKey::Found(key) => key,
                SignerKey::NeedsLog => return Ok(CommentApply::NeedsLog),
                SignerKey::Unknown => return Ok(CommentApply::Quarantined),
            };
            let Ok(signature) = Signature::from_bytes(&record.sig) else {
                return Ok(CommentApply::Quarantined);
            };
            if kb_crypto::verify(
                &key,
                COMMENT_RECORD_CONTEXT,
                &record.signing_bytes(),
                &signature,
            )
            .is_err()
            {
                return Ok(CommentApply::Quarantined);
            }
            // The role the author held, at the item's cursor.
            let role = if !shared {
                Some(MemberRole::Owner)
            } else if carried.snapshot {
                self.author_role_up_to(feed, record.author, cursor)?
            } else {
                self.role_at(feed, record.author, cursor)
            };
            let Some(role) = role.filter(|role| role.can_comment()) else {
                return Ok(CommentApply::Quarantined);
            };
            // A second `Post` for a comment id: ids are random, so it is a forgery (the fold's
            // earliest-claim rule could be steered by a backdated `at`, so it is refused here, in
            // the order every device sees).
            if matches!(record.op, CommentOp::Post { .. })
                && (before.defines_comment(record.comment_id)
                    || !posts_in_item.insert(record.comment_id))
            {
                return Ok(CommentApply::Quarantined);
            }
            // A record that contradicts its target: a `Resolve` names the thread's root comment, an
            // `Edit` or a `Delete` the thread its comment is in (the fold would ignore them; they
            // are refused so that honest and hostile writers are told apart in one place).
            match &record.op {
                CommentOp::Resolve { .. } if !record.is_root() => {
                    return Ok(CommentApply::Quarantined);
                }
                CommentOp::Edit { .. } | CommentOp::Delete => {
                    let post_thread = before
                        .records()
                        .map(|entry| &entry.record)
                        .chain(checked.iter().map(|(other, _, _)| other))
                        .find(|other| {
                            other.comment_id == record.comment_id
                                && matches!(other.op, CommentOp::Post { .. })
                        })
                        .map(|other| other.thread_id);
                    if post_thread.is_some_and(|thread| thread != record.thread_id) {
                        return Ok(CommentApply::Quarantined);
                    }
                }
                _ => {}
            }
            checked.push((record, bytes, role));
        }
        if checked.is_empty() {
            return Ok(CommentApply::Applied);
        }

        // Everything passed: store it together.
        let missing_doc = self
            .db
            .doc_get(doc_id)
            .map_err(map_storage_error)?
            .is_none();
        let stored_cursor = Some(cursor);
        self.db
            .transaction(|tx| {
                if missing_doc {
                    tx.doc_insert(&new_doc_row(
                        doc_id,
                        DocKindCode::Comments,
                        Some(space),
                        now,
                    ))?;
                }
                for (record, bytes, role) in &checked {
                    tx.update_append(
                        doc_id,
                        &kb_doc::encode_records_update(bytes),
                        UpdateOrigin::Remote,
                        now,
                    )?;
                    tx.comment_record_put(&comment_row(
                        space,
                        record,
                        stored_cursor,
                        *role,
                        bytes.clone(),
                    ))?;
                }
                Ok::<(), kb_storage::StorageError>(())
            })
            .map_err(map_storage_error)?;
        let mut after = before.clone();
        let mut new_records: Vec<CommentRecord> = Vec::new();
        for (record, _, role) in checked {
            new_records.push(record.clone());
            after.apply(VerifiedRecord {
                record,
                author_role: role,
            });
        }
        self.comment_fts_refresh_page(space, page)?;
        self.events.push(Event::CommentsChanged { page_id: page });
        if self.comment_notifications_allowed(feed)? && !(carried.snapshot && before.is_empty()) {
            self.comment_notify(space, page, &before, &after, &new_records, now)?;
        }
        Ok(CommentApply::Applied)
    }

    /// The key that verifies a record: the one of `record.device` in `author`'s log (this account's
    /// own for the personal feed, a co-member's copy for a shared one).
    fn comment_signer_key(
        &mut self,
        shared: bool,
        author: AccountId,
        device: DeviceId,
    ) -> Result<SignerKey, ApiError> {
        if !shared {
            if self.me()? != author {
                return Ok(SignerKey::Unknown);
            }
            return Ok(self
                .device_verifier(device)
                .map_or(SignerKey::Unknown, SignerKey::Found));
        }
        match self.member_device_key(author, device) {
            Ok(Some(key)) => Ok(SignerKey::Found(key)),
            Ok(None) => Ok(SignerKey::NeedsLog),
            Err(error) if error.code == ErrorCode::Crypto => Ok(SignerKey::Unknown),
            Err(error) => Err(error),
        }
    }

    /// The highest role the account held at a membership cursor up to `covers` (a snapshot's records
    /// do not say at which cursor they were written, so what they needed is "was a Commenter or more
    /// at some point of the history the snapshot covers"); `None` when it was never a member.
    fn author_role_up_to(
        &self,
        feed: FeedId,
        account: AccountId,
        covers: u64,
    ) -> Result<Option<MemberRole>, ApiError> {
        let Some(timeline) = self.shared.timelines.get(&feed) else {
            return Ok(None);
        };
        let mut best = timeline.role_at(account, covers);
        for entry in self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?
        {
            if entry.cursor == 0 || entry.cursor > covers {
                continue;
            }
            best = best.max(timeline.role_at(account, entry.cursor));
        }
        Ok(best)
    }

    /// Whether the changes of the feed that arrive now may raise notifications: not while the feed
    /// is still being read for the first time (a device that begins with the history of a space must
    /// not announce all of it).
    pub(crate) fn comment_notifications_allowed(&self, feed: FeedId) -> Result<bool, ApiError> {
        if self.feed_row(feed)?.kind == FeedKind::Personal {
            return Ok(true);
        }
        Ok(self.fcfg(feed, "bootstrapped")?.as_deref() == Some("1"))
    }

    /// Raises the notifications the new records bring (see the module documentation).
    fn comment_notify(
        &mut self,
        space: SpaceId,
        page: PageId,
        before: &CommentStream,
        after: &CommentStream,
        new_records: &[CommentRecord],
        now: i64,
    ) -> Result<(), ApiError> {
        let Ok(me) = self.me() else {
            return Ok(());
        };
        let counting = after.valid_record_ids();
        for record in new_records {
            if record.author == me || !counting.contains(&record.record_id) {
                continue;
            }
            let (CommentOp::Post { .. } | CommentOp::Edit { .. }) = &record.op else {
                continue;
            };
            let Some(thread) = after.thread(record.thread_id) else {
                continue;
            };
            let Some(comment) = thread
                .comments
                .iter()
                .find(|comment| comment.comment_id == record.comment_id)
            else {
                continue;
            };
            let mentioned_before = before
                .thread(record.thread_id)
                .and_then(|folded| {
                    folded
                        .comments
                        .iter()
                        .find(|c| c.comment_id == record.comment_id)
                        .map(|c| body_mentions(&c.body, me))
                })
                .unwrap_or(false);
            let mentions_me = body_mentions(&comment.body, me) && !mentioned_before;
            let replies_to_me = matches!(record.op, CommentOp::Post { .. })
                && !record.is_root()
                && thread
                    .comments
                    .iter()
                    .any(|other| other.comment_id != record.comment_id && other.author == me);
            let kind = if mentions_me {
                NotificationKind::CommentMention
            } else if replies_to_me {
                NotificationKind::ThreadReply
            } else {
                continue;
            };
            self.notification_raise(&NewNotification {
                kind,
                space_id: space,
                page_id: Some(page),
                block_id: thread.anchor.as_ref().and_then(|anchor| anchor.block_id),
                comment_id: Some(record.comment_id),
                actor: record.author,
                at: now,
            })?;
        }
        Ok(())
    }

    /// Snapshot dominance for a comment stream (ADR-0045, ADR-0048): `true` when the snapshot's
    /// record set (`kb_doc::decode_records`) includes every valid record id this device has seen
    /// carried at feed cursors up to `covers`. A snapshot that does not (or is not a records
    /// snapshot at all) is refused and disputed like a truncating page snapshot.
    pub(crate) fn comment_snapshot_dominates(
        &mut self,
        page_id: PageId,
        snapshot: &[u8],
        covers: u64,
    ) -> Result<bool, ApiError> {
        let Ok(parsed) = kb_doc::decode_records(snapshot) else {
            return Ok(false);
        };
        if parsed.kind != BlobKind::Snapshot {
            return Ok(false);
        }
        let Ok(listed) = snapshot_record_ids(parsed.records.iter().copied()) else {
            return Ok(false);
        };
        // What this device saw carried up to `covers`, judged as the fold judges those records alone.
        let mut seen = CommentStream::new(page_id);
        for row in self
            .db
            .comment_records_of_page(page_id)
            .map_err(map_storage_error)?
        {
            if row.cursor.is_some_and(|cursor| cursor <= covers)
                && let Ok(record) = CommentRecord::decode(&row.record)
            {
                seen.apply(VerifiedRecord {
                    record,
                    author_role: row.author_role,
                });
            }
        }
        Ok(snapshot_dominates(&listed, &seen.valid_record_ids()))
    }

    /// Whether a feed item's document id is a comment stream this device can place: it is the derived
    /// stream of a page of the feed's space (`Is`), the id of a document that is not a stream
    /// (`Other`), or the id of nothing this device knows yet (`Unknown`: the structure that creates the
    /// page has not arrived).
    pub(crate) fn comment_stream_status(
        &self,
        space: Option<SpaceId>,
        doc_id: DocId,
    ) -> StreamStatus {
        if let Ok(Some(row)) = self.db.doc_get(doc_id) {
            return if row.kind == DocKindCode::Comments {
                StreamStatus::Is
            } else {
                StreamStatus::Other
            };
        }
        let known_document = space
            .and_then(|space| self.docs.structures.get(&space))
            .is_some_and(|open| open.doc_id == doc_id)
            || self
                .space_of_page(PageId::from_bytes(doc_id.to_bytes()))
                .is_ok();
        if known_document {
            return StreamStatus::Other;
        }
        if self.comment_stream_page(space, doc_id).is_some() {
            StreamStatus::Is
        } else {
            StreamStatus::Unknown
        }
    }
}

/// What a document id of a shared feed is, as far as this device can tell
/// ([`Core::comment_stream_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamStatus {
    /// The comment stream of a page this device knows (or a document it holds as one).
    Is,
    /// The structure of the space, a page, the account document: anything but a stream.
    Other,
    /// Nothing this device knows has this id yet.
    Unknown,
}
