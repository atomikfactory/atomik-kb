//! Comments (M13 slice 2, ADR-0048, `docs/security/sharing.md` section 8,
//! `docs/development/milestone-13.md` sections 2.1 and 2.2): the page's comment stream, its signed
//! records, the five requests that read and write it, and the hooks the sync paths call.
//!
//! A page's comments are a **stream of signed records** of their own (document kind
//! `DocKind::Comments` / `DocKindCode::Comments`, engine `kb_doc::ENGINE_RECORDS`, one update blob per
//! record, a snapshot is the set of valid records), so a Commenter writes comments without being able
//! to write the page. The document's id derives from the page id ([`comment_stream_doc_id`]), so two
//! Commenters never make two streams; the first Commenter makes the stream's `Doc` record under the
//! first-in-feed rule (the first one the feed holds wins, the loser re-sends its record, which is
//! idempotent). `kb-model` knows the records and the fold ([`kb_model::CommentStream`]); this module
//! signs, checks and applies them, with the roles of the membership timeline:
//!
//! - **Out** (`CommentPost`, `CommentEdit`, `CommentDelete`, `CommentResolve`): the role gate
//!   ([`Core::require_role`] at Commenter, or the op's own rule), body and anchor checks
//!   (`kb_model::validate_body`, `validate_anchor`: `InvalidInput` for `body`, `anchor`, `threadId`), a
//!   [`kb_model::CommentRecord`] signed by this device under `COMMENT_RECORD_CONTEXT`, one update entry
//!   of the stream, the `comments` row and the `comment_fts` rows, `CommentsChanged` and
//!   `SyncRequested`. Never a notification: a change this device made itself does not notify.
//! - **In** (the pull path of `feeds/pull.rs`, [`Core::apply_comment_updates`], in `apply.rs`): every
//!   record is decoded, its signature checked against the key of the device that signed it, its
//!   `author` against the account of the entry, its `page_id` against the stream's page, and its role
//!   taken from the membership timeline at the item's cursor; an item with a record that fails is
//!   quarantined (`sync_rejected`), never partially applied. What the fold ignores stays in
//!   `comments` (it may count when an earlier record arrives). A snapshot of the stream is accepted
//!   when its record set includes every valid record id seen at cursors up to what it covers
//!   (ADR-0045).
//!
//! **Where things are.** `comments` holds one row per verified record (`kb-storage`), the threads are
//! the fold of those rows, `comment_fts` the words of the comments that count. The stream's document
//! row (`docs`, kind 3) exists for the sync paths (the key records, the outbox, the snapshots); its
//! update rows are the records' blobs (`Local` for this device's, `Remote` for the others').

mod apply;

use kb_doc::BlobKind;
use kb_model::{
    CommentOp, CommentRecord, CommentStream, FoldedThread, SIGNATURE_LEN, VerifiedRecord,
    validate_anchor, validate_body,
};
use kb_storage::{
    CommentQueries, CommentRow, CommentTextRow, DocKindCode, DocQueries, UpdateOrigin,
};
use kb_types::comments::COMMENT_RECORD_CONTEXT;
use kb_types::comments::COMMENT_STREAM_DOMAIN;
use kb_types::{
    AccountId, ApiError, CommentAnchor, CommentBodyItem, CommentId, CommentInfo, CommentRecordId,
    DocId, ErrorCode, Event, FeedId, MemberRole, PageId, Request, Response, SpaceId, ThreadId,
    ThreadInfo,
};

use crate::documents::new_doc_row;
use crate::membership::fallback_name;
use crate::{Core, map_storage_error};

pub(crate) use apply::{Carried, CommentApply, StreamStatus};

/// The id of the comment stream of a page (ADR-0048): `BLAKE3("atomik-kb/v1/comments" ‖ page id)[..16]`,
/// the page id as its 16 bytes. The same on every device, so concurrent first comments land in one
/// stream. (`feeds/pull.rs` recognises a stream by this derivation.)
#[must_use]
pub(crate) fn comment_stream_doc_id(page_id: PageId) -> DocId {
    let mut input = Vec::with_capacity(COMMENT_STREAM_DOMAIN.len() + 16);
    input.extend_from_slice(COMMENT_STREAM_DOMAIN.as_bytes());
    input.extend_from_slice(page_id.as_bytes());
    let digest = kb_crypto::hash(&input);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    DocId::from_bytes(id)
}

/// The words of a comment body: its text items joined with one space (a mention or a page reference
/// adds no word).
pub(crate) fn body_text(body: &[CommentBodyItem]) -> String {
    let texts: Vec<&str> = body
        .iter()
        .filter_map(|item| match item {
            CommentBodyItem::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    texts.join(" ")
}

/// Whether a body mentions the account.
pub(crate) fn body_mentions(body: &[CommentBodyItem], account: AccountId) -> bool {
    body.iter().any(
        |item| matches!(item, CommentBodyItem::Mention { account_id } if *account_id == account),
    )
}

/// The `InvalidInput` of a record check that failed: `body` or `anchor`, whatever the record error says.
fn record_error(error: &kb_model::CommentRecordError) -> ApiError {
    let field = match error {
        kb_model::CommentRecordError::Anchor(_) => "anchor",
        _ => "body",
    };
    ApiError::invalid(field, error.to_string())
}

/// `Forbidden` for a change only the author may make (`details`: `{reason: "author"}`).
fn forbidden_author() -> ApiError {
    ApiError::new(
        ErrorCode::Forbidden,
        "only the author of a comment may change it",
    )
    .with_details(serde_json::json!({ "reason": "author" }))
}

impl Core {
    /// Routes `CommentList`, `CommentPost`, `CommentEdit`, `CommentDelete` and `CommentResolve`.
    pub(crate) fn dispatch_comments(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::CommentList { page_id } => Ok(Response::Comments(self.comment_list(page_id)?)),
            Request::CommentPost {
                page_id,
                thread_id,
                anchor,
                body,
            } => Ok(Response::Thread(
                self.comment_post(page_id, thread_id, anchor, body)?,
            )),
            Request::CommentEdit {
                page_id,
                comment_id,
                body,
            } => Ok(Response::Thread(
                self.comment_edit(page_id, comment_id, body)?,
            )),
            Request::CommentDelete {
                page_id,
                comment_id,
            } => Ok(Response::Thread(self.comment_delete(page_id, comment_id)?)),
            Request::CommentResolve {
                page_id,
                thread_id,
                resolved,
            } => Ok(Response::Thread(
                self.comment_resolve(page_id, thread_id, resolved)?,
            )),
            other => Err(ApiError::internal(format!(
                "{} is not a comment request",
                other.kind()
            ))),
        }
    }

    // ----- the requests ---------------------------------------------------------------------

    /// The threads of a page, oldest root first, with names and `isMine` (any member; a personal
    /// space answers its own comments, there is no one else). `NotFound` for the page. Threads whose
    /// comments are all deleted are left out.
    pub(crate) fn comment_list(&mut self, page_id: PageId) -> Result<Vec<ThreadInfo>, ApiError> {
        let space = self.space_of_page(page_id)?;
        self.shared_load()?;
        let stream = self.comment_stream_of(page_id)?;
        let mut out = Vec::new();
        for thread in stream.threads() {
            out.push(self.thread_info(space, page_id, &thread));
        }
        Ok(out)
    }

    /// Posts a comment (Commenter and up): a new thread when `thread_id` is absent, a reply when it
    /// names a thread of the page. Answers the thread. `InvalidInput` for `body`, `anchor` (also
    /// when given with a `thread_id`) or `threadId`; `NotFound` for the page; `Forbidden` for a
    /// Viewer or an archive.
    pub(crate) fn comment_post(
        &mut self,
        page_id: PageId,
        thread_id: Option<ThreadId>,
        anchor: Option<CommentAnchor>,
        body: Vec<CommentBodyItem>,
    ) -> Result<ThreadInfo, ApiError> {
        let space = self.space_of_page(page_id)?;
        self.require_role(space, MemberRole::Commenter)?;
        validate_body(&body).map_err(|error| record_error(&error))?;
        if thread_id.is_some() && anchor.is_some() {
            return Err(ApiError::invalid(
                "anchor",
                "a reply has no anchor: only a new thread points at the page",
            ));
        }
        if let Some(anchor) = &anchor {
            validate_anchor(anchor).map_err(|error| record_error(&error))?;
        }
        let stream = self.comment_stream_of(page_id)?;
        let comment_id = CommentId::generate(self.rng.as_ref());
        let thread = match thread_id {
            Some(thread) => {
                let live = stream
                    .thread(thread)
                    .is_some_and(|folded| folded.comments.iter().any(|c| !c.deleted));
                if !live {
                    return Err(ApiError::invalid(
                        "threadId",
                        "this is not a thread of the page",
                    ));
                }
                thread
            }
            None => ThreadId::of(comment_id),
        };
        let record = self.comment_record(
            page_id,
            thread,
            comment_id,
            CommentOp::Post { anchor, body },
            0,
        )?;
        self.comment_store_local(space, page_id, &record)?;
        self.comment_thread_after(space, page_id, thread)
    }

    /// Replaces the body of a comment (the author only). `NotFound` for the page or the comment;
    /// `Forbidden` for anyone else; `InvalidInput` for `body`.
    pub(crate) fn comment_edit(
        &mut self,
        page_id: PageId,
        comment_id: CommentId,
        body: Vec<CommentBodyItem>,
    ) -> Result<ThreadInfo, ApiError> {
        let space = self.space_of_page(page_id)?;
        self.require_role(space, MemberRole::Commenter)?;
        validate_body(&body).map_err(|error| record_error(&error))?;
        let stream = self.comment_stream_of(page_id)?;
        let (thread, author) = Self::live_comment(&stream, comment_id)?;
        if author != self.comment_account()? {
            return Err(forbidden_author());
        }
        let rev = next_rev(&stream, comment_id);
        let record =
            self.comment_record(page_id, thread, comment_id, CommentOp::Edit { body }, rev)?;
        self.comment_store_local(space, page_id, &record)?;
        self.comment_thread_after(space, page_id, thread)
    }

    /// Deletes a comment (its author, or an Owner): final. `NotFound` for the page or the comment;
    /// `Forbidden` for anyone else. Answers the thread, with no comments when this was the last one.
    pub(crate) fn comment_delete(
        &mut self,
        page_id: PageId,
        comment_id: CommentId,
    ) -> Result<ThreadInfo, ApiError> {
        let space = self.space_of_page(page_id)?;
        self.require_role(space, MemberRole::Commenter)?;
        let stream = self.comment_stream_of(page_id)?;
        let (thread, author) = Self::live_comment(&stream, comment_id)?;
        if author != self.comment_account()? {
            // An Owner may delete anybody's comment.
            self.require_role(space, MemberRole::Owner)?;
        }
        let rev = next_rev(&stream, comment_id);
        let record = self.comment_record(page_id, thread, comment_id, CommentOp::Delete, rev)?;
        self.comment_store_local(space, page_id, &record)?;
        self.comment_thread_after(space, page_id, thread)
    }

    /// Resolves or reopens a thread (the thread's author, or Editor and up). `NotFound` for the
    /// page or the thread; `Forbidden` for anyone else.
    pub(crate) fn comment_resolve(
        &mut self,
        page_id: PageId,
        thread_id: ThreadId,
        resolved: bool,
    ) -> Result<ThreadInfo, ApiError> {
        let space = self.space_of_page(page_id)?;
        self.require_role(space, MemberRole::Commenter)?;
        let stream = self.comment_stream_of(page_id)?;
        let root = thread_id.root_comment();
        let live = stream
            .thread(thread_id)
            .is_some_and(|folded| folded.comments.iter().any(|c| !c.deleted));
        let Some(author) = stream.comment_author(root).filter(|_| live) else {
            return Err(ApiError::not_found("thread not found"));
        };
        if author != self.comment_account()? {
            self.require_role(space, MemberRole::Editor)?;
        }
        let rev = next_rev(&stream, root);
        let record = self.comment_record(
            page_id,
            thread_id,
            root,
            CommentOp::Resolve { resolved },
            rev,
        )?;
        self.comment_store_local(space, page_id, &record)?;
        self.comment_thread_after(space, page_id, thread_id)
    }

    // ----- the pieces the requests share -----------------------------------------------------

    /// The thread and the author of a comment that is there and not deleted (`NotFound` otherwise).
    fn live_comment(
        stream: &CommentStream,
        comment_id: CommentId,
    ) -> Result<(ThreadId, AccountId), ApiError> {
        stream
            .threads()
            .into_iter()
            .find_map(|thread| {
                thread
                    .comments
                    .iter()
                    .find(|comment| comment.comment_id == comment_id && !comment.deleted)
                    .map(|comment| (thread.thread_id, comment.author))
            })
            .ok_or_else(|| ApiError::not_found("comment not found"))
    }

    /// This account's id, for a request that needs to name the author.
    fn comment_account(&self) -> Result<AccountId, ApiError> {
        self.sync_identity()?
            .map(|(account, _)| account)
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before commenting"))
    }

    /// The fold's input for a page: every record this device holds, with the role its author had
    /// when it was verified.
    pub(crate) fn comment_stream_of(&self, page_id: PageId) -> Result<CommentStream, ApiError> {
        let rows = self
            .db
            .comment_records_of_page(page_id)
            .map_err(map_storage_error)?;
        let mut stream = CommentStream::new(page_id);
        for row in rows {
            // A damaged record is left out, never an error: the rest of the page still reads.
            if let Ok(record) = CommentRecord::decode(&row.record) {
                stream.apply(VerifiedRecord {
                    record,
                    author_role: row.author_role,
                });
            }
        }
        Ok(stream)
    }

    /// A signed record of this device (the fields are the request's, `at` is now).
    pub(crate) fn comment_record(
        &mut self,
        page_id: PageId,
        thread_id: ThreadId,
        comment_id: CommentId,
        op: CommentOp,
        rev: u32,
    ) -> Result<CommentRecord, ApiError> {
        let (author, device) = self.sync_identity()?.ok_or_else(|| {
            ApiError::invalid("kit", "create your recovery kit before commenting")
        })?;
        let mut record = CommentRecord {
            record_id: CommentRecordId::generate(self.rng.as_ref()),
            page_id,
            thread_id,
            comment_id,
            op,
            author,
            device,
            rev,
            at: self.clock.now_ms(),
            sig: [0; SIGNATURE_LEN],
        };
        let signature = kb_crypto::sign(
            self.signer(),
            COMMENT_RECORD_CONTEXT,
            &record.signing_bytes(),
        );
        record.sig = signature.to_bytes();
        Ok(record)
    }

    /// This account's role in the space, as the fold reads it (a personal space: Owner).
    fn comment_my_role(&mut self, space: SpaceId) -> Result<MemberRole, ApiError> {
        Ok(self.role_in_space(space)?.unwrap_or(MemberRole::Owner))
    }

    /// Stores a record this device made: the stream's document row and key, the update row (`Local`,
    /// so the outbox carries it), the `comments` row (no cursor until the push gets one) and the words.
    pub(crate) fn comment_store_local(
        &mut self,
        space: SpaceId,
        page_id: PageId,
        record: &CommentRecord,
    ) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let doc_id = comment_stream_doc_id(page_id);
        let role = self.comment_my_role(space)?;
        self.comment_stream_row(space, doc_id, now)?;
        let encoded = record.encode();
        let blob = kb_doc::encode_records_update(&encoded);
        let row = comment_row(space, record, None, role, encoded);
        self.db
            .transaction(|tx| {
                tx.update_append(doc_id, &blob, UpdateOrigin::Local, now)?;
                tx.comment_record_put(&row)?;
                Ok::<(), kb_storage::StorageError>(())
            })
            .map_err(map_storage_error)?;
        self.comment_fts_refresh_page(space, page_id)?;
        self.events.push(Event::CommentsChanged { page_id });
        self.sync_note_local_change();
        Ok(())
    }

    /// The document row of a page's stream (created when this is its first record) and its `Doc`
    /// key record (a Commenter makes it too).
    pub(crate) fn comment_stream_row(
        &mut self,
        space: SpaceId,
        doc_id: DocId,
        now: i64,
    ) -> Result<(), ApiError> {
        if self
            .db
            .doc_get(doc_id)
            .map_err(map_storage_error)?
            .is_none()
        {
            self.db
                .doc_insert(&new_doc_row(
                    doc_id,
                    DocKindCode::Comments,
                    Some(space),
                    now,
                ))
                .map_err(map_storage_error)?;
        }
        self.ensure_comment_stream_key(space, doc_id)
    }

    /// The thread as the requests answer it, after a change.
    fn comment_thread_after(
        &mut self,
        space: SpaceId,
        page_id: PageId,
        thread: ThreadId,
    ) -> Result<ThreadInfo, ApiError> {
        self.shared_load()?;
        let stream = self.comment_stream_of(page_id)?;
        let folded = stream
            .thread(thread)
            .ok_or_else(|| ApiError::not_found("thread not found"))?;
        Ok(self.thread_info(space, page_id, &folded))
    }

    /// A folded thread with the names, the former members and `isMine` the UI shows.
    fn thread_info(
        &mut self,
        space: SpaceId,
        page_id: PageId,
        thread: &FoldedThread,
    ) -> ThreadInfo {
        let me = self.me().ok();
        let comments = thread
            .comments
            .iter()
            .map(|comment| {
                let (author_name, author_former) = self.person_in_space(space, comment.author);
                CommentInfo {
                    comment_id: comment.comment_id,
                    thread_id: comment.thread_id,
                    page_id,
                    author: comment.author,
                    author_name,
                    author_former,
                    body: comment.body.clone(),
                    created_at: comment.created_at,
                    edited_at: comment.edited_at,
                    deleted: comment.deleted,
                    is_mine: Some(comment.author) == me,
                }
            })
            .collect();
        ThreadInfo {
            thread_id: thread.thread_id,
            page_id,
            anchor: thread.anchor.clone(),
            resolved: thread.resolved,
            resolved_by: thread.resolved_by,
            comments,
        }
    }

    /// The name of an account in a space and whether it is no longer a member there (a personal
    /// space has no one but this account). A name the membership log never gave is the short hex of
    /// the account id; a former member keeps the last name known.
    pub(crate) fn person_in_space(&self, space: SpaceId, account: AccountId) -> (String, bool) {
        match self.shared_feed_of(space).ok().flatten() {
            Some(row) => {
                let member = self
                    .shared
                    .timelines
                    .get(&row.feed)
                    .is_some_and(|timeline| timeline.state().member(account).is_some());
                (self.member_name(row.feed, account), !member)
            }
            None => {
                let name = if self.me().ok() == Some(account) {
                    self.profile_name_of_account()
                } else {
                    None
                };
                (name.unwrap_or_else(|| fallback_name(account)), false)
            }
        }
    }

    // ----- the words -------------------------------------------------------------------------

    /// Rewrites the `comment_fts` rows of a page from the fold: one per comment that counts and is
    /// not deleted, its latest valid body's text.
    pub(crate) fn comment_fts_refresh_page(
        &mut self,
        space: SpaceId,
        page_id: PageId,
    ) -> Result<(), ApiError> {
        let stream = self.comment_stream_of(page_id)?;
        let rows: Vec<CommentTextRow> = stream
            .threads()
            .into_iter()
            .flat_map(|thread| thread.comments)
            .filter(|comment| !comment.deleted)
            .map(|comment| CommentTextRow {
                comment_id: comment.comment_id,
                text: body_text(&comment.body),
            })
            .collect();
        self.db
            .comment_fts_replace(space, page_id, &rows)
            .map_err(map_storage_error)
    }

    /// Rewrites the words of every page of a space that has comments (`IndexRebuild`).
    pub(crate) fn comment_fts_rebuild_space(&mut self, space: SpaceId) -> Result<(), ApiError> {
        let pages: Vec<PageId> = self
            .db
            .comment_pages()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|(page_space, _)| *page_space == space)
            .map(|(_, page)| page)
            .collect();
        for page in pages {
            self.comment_fts_refresh_page(space, page)?;
        }
        Ok(())
    }

    // ----- pages that leave, move and come back -----------------------------------------------

    /// Forgets the comment streams of pages that are gone (purged, here or on another device): the
    /// records, the words and the stream's document. Answers the streams that had a document, with
    /// their space, for the crypto-erase of their key records.
    pub(crate) fn comments_forget_pages(
        &mut self,
        pages: &[PageId],
    ) -> Result<Vec<(SpaceId, DocId)>, ApiError> {
        let mut streams = Vec::new();
        for page in pages {
            let doc_id = comment_stream_doc_id(*page);
            let row = self.db.doc_get(doc_id).map_err(map_storage_error)?;
            self.db
                .transaction(|tx| {
                    tx.comment_records_delete_page(*page)?;
                    tx.comment_fts_delete_page(*page)?;
                    tx.doc_delete(doc_id)
                })
                .map_err(map_storage_error)?;
            if let Some(space) = row.and_then(|row| row.space_id) {
                streams.push((space, doc_id));
            }
        }
        Ok(streams)
    }

    /// The comment streams of the moved pages follow them (ADR-0048, ADR-0038): the stream's
    /// document row, its records and its words point at the target space. Called inside the move's
    /// own transaction for the rows; answers the streams that have a document.
    pub(crate) fn comment_streams_of_pages(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<DocId>, ApiError> {
        let mut docs = Vec::new();
        for page in pages {
            let doc_id = comment_stream_doc_id(*page);
            if self
                .db
                .doc_get(doc_id)
                .map_err(map_storage_error)?
                .is_some()
            {
                docs.push(doc_id);
            }
        }
        Ok(docs)
    }

    /// After a move: the records and the words of the pages name the target space.
    pub(crate) fn comments_after_move(
        &mut self,
        pages: &[PageId],
        target: SpaceId,
    ) -> Result<(), ApiError> {
        for page in pages {
            let changed = self
                .db
                .comment_records_set_space(*page, target)
                .map_err(map_storage_error)?;
            if changed > 0 {
                self.comment_fts_refresh_page(target, *page)?;
                self.events.push(Event::CommentsChanged { page_id: *page });
            }
        }
        Ok(())
    }

    /// Makes the space of every comment stream agree with its page's (a crash between the steps of
    /// a move, a backup): the page index follows the structure documents.
    pub(crate) fn comments_verify_spaces(&mut self) -> Result<(), ApiError> {
        let rows = self
            .db
            .docs_by_kind(DocKindCode::Comments)
            .map_err(map_storage_error)?;
        for row in rows {
            let Some((_, page)) = self.comment_stream_page(None, row.doc_id) else {
                continue;
            };
            let Some(indexed) = self.db.pages_index_get(page).map_err(map_storage_error)? else {
                continue;
            };
            let space = indexed.space_id;
            if row.space_id != Some(space) {
                self.db
                    .doc_set_space(row.doc_id, space)
                    .map_err(map_storage_error)?;
                self.db
                    .comment_records_set_space(page, space)
                    .map_err(map_storage_error)?;
                self.comment_fts_refresh_page(space, page)?;
            }
        }
        Ok(())
    }

    // ----- finding streams -------------------------------------------------------------------

    /// The page a comment stream belongs to and its space: the page of `hint`'s structure (or of any
    /// structure) whose derived stream id is `doc_id`. `None` when no page this device knows has it
    /// (the structure that creates the page has not arrived yet).
    pub(crate) fn comment_stream_page(
        &self,
        hint: Option<SpaceId>,
        doc_id: DocId,
    ) -> Option<(SpaceId, PageId)> {
        let find = |space: SpaceId| -> Option<PageId> {
            self.docs
                .structures
                .get(&space)?
                .model
                .all_page_ids()
                .into_iter()
                .find(|page| comment_stream_doc_id(*page) == doc_id)
        };
        let (space, page) = match hint {
            Some(space) => (space, find(space)?),
            None => self
                .docs
                .structures
                .keys()
                .find_map(|space| find(*space).map(|page| (*space, page)))?,
        };
        Some((self.space_of_page(page).unwrap_or(space), page))
    }

    // ----- the snapshots ---------------------------------------------------------------------

    /// The snapshot blob of a page's comment stream: `kb_doc::encode_records_snapshot` of the records
    /// the fold counts (`CommentStream::valid_records`), by record id. The pushing path offers it when
    /// the stream's entries pile up, like any other document's snapshot, and a backup keeps it.
    pub(crate) fn comment_stream_snapshot(&self, page_id: PageId) -> Result<Vec<u8>, ApiError> {
        let stream = self.comment_stream_of(page_id)?;
        let encoded: Vec<Vec<u8>> = stream
            .valid_records()
            .into_iter()
            .map(CommentRecord::encode)
            .collect();
        let records: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
        Ok(kb_doc::encode_records_snapshot(&records))
    }

    /// [`Self::comment_stream_snapshot`] for a stream's document row (its page is looked up; a
    /// stream whose page is not known here snapshots as an empty set).
    pub(crate) fn comment_stream_snapshot_of_doc(
        &self,
        row: &kb_storage::DocRow,
    ) -> Result<Vec<u8>, ApiError> {
        match self.comment_stream_page(None, row.doc_id) {
            Some((_, page)) => self.comment_stream_snapshot(page),
            None => Ok(kb_doc::encode_records_snapshot(&[])),
        }
    }

    /// The records this device made and the server has now taken in an entry at `cursor`: their
    /// feed cursor is known (what snapshot dominance counts from).
    pub(crate) fn comment_records_pushed(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        cursor: u64,
    ) -> Result<(), ApiError> {
        let _ = feed;
        let Some((_, page)) = self.comment_stream_page(None, doc_id) else {
            return Ok(());
        };
        let device = self.device.device_id;
        for row in self
            .db
            .comment_records_of_page(page)
            .map_err(map_storage_error)?
        {
            if row.cursor.is_none() && row.device == device {
                self.db
                    .comment_record_set_cursor(row.record_id, cursor)
                    .map_err(map_storage_error)?;
            }
        }
        Ok(())
    }

    /// Reads a comment stream's snapshot back out of a backup (the records a device held when it was
    /// made, which its fold counted) and stores them for `page`: no cursor, and the highest role (the
    /// backup lists only records that counted, and the roles they counted under are not in it).
    pub(crate) fn comments_restore(
        &mut self,
        row: &kb_storage::DocRow,
        snapshot: &[u8],
    ) -> Result<(), ApiError> {
        let Some((space, page)) = self.comment_stream_page(row.space_id, row.doc_id) else {
            return Ok(());
        };
        let Ok(blob) = kb_doc::decode_records(snapshot) else {
            return Ok(());
        };
        if blob.kind != BlobKind::Snapshot {
            return Ok(());
        }
        let mut rows = Vec::new();
        for bytes in blob.records {
            if let Ok(record) = CommentRecord::decode(bytes)
                && record.page_id == page
            {
                rows.push(comment_row(
                    space,
                    &record,
                    None,
                    MemberRole::Owner,
                    bytes.to_vec(),
                ));
            }
        }
        self.db
            .transaction(|tx| {
                for row in &rows {
                    tx.comment_record_put(row)?;
                }
                Ok::<(), kb_storage::StorageError>(())
            })
            .map_err(map_storage_error)?;
        self.comment_fts_refresh_page(space, page)
    }
}

/// The `comments` row of a verified record.
pub(crate) fn comment_row(
    space: SpaceId,
    record: &CommentRecord,
    cursor: Option<u64>,
    role: MemberRole,
    encoded: Vec<u8>,
) -> CommentRow {
    CommentRow {
        record_id: record.record_id,
        space_id: space,
        page_id: record.page_id,
        thread_id: record.thread_id,
        comment_id: record.comment_id,
        op: record.op.code(),
        author: record.author,
        device: record.device,
        rev: record.rev,
        at: record.at,
        cursor,
        author_role: role,
        record: encoded,
    }
}

/// The revision of a record that changes a comment (or resolves its thread): one more than the
/// highest the device has seen for that comment id.
fn next_rev(stream: &CommentStream, comment_id: CommentId) -> u32 {
    stream
        .records()
        .filter(|entry| entry.record.comment_id == comment_id)
        .map(|entry| entry.record.rev)
        .max()
        .map_or(1, |highest| highest.saturating_add(1))
}

#[cfg(test)]
impl Core {
    /// Stores a record exactly as given, signed by this device, with no role or rule check: what a
    /// hostile or out-of-date client does (the adversarial tests send it through the outbox).
    pub(crate) fn test_comment_forge(
        &mut self,
        page_id: PageId,
        mut record: CommentRecord,
    ) -> Result<(), ApiError> {
        let space = self.space_of_page(page_id)?;
        let signature = kb_crypto::sign(
            self.signer(),
            COMMENT_RECORD_CONTEXT,
            &record.signing_bytes(),
        );
        record.sig = signature.to_bytes();
        self.comment_store_local(space, page_id, &record)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use kb_types::{NotificationKind, SpaceId};

    use super::*;
    use crate::test_support::{Fixture, create_page, create_workspace, spaces};

    fn personal() -> (Fixture, PageId, SpaceId) {
        let fixture = Fixture::new();
        let mut core = fixture.open(1, 1_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space: SpaceId = spaces(&mut core)[0].id;
        let page = create_page(&mut core, space, None, "Plan").id;
        drop(core);
        (fixture, page, space)
    }

    #[test]
    fn a_personal_space_without_a_kit_refuses_to_comment_and_lists_nothing() {
        let (fixture, page, _) = personal();
        let mut core = fixture.open(2, 2_000);
        let body = vec![CommentBodyItem::Text { text: "Hi".into() }];
        let error = core
            .dispatch(Request::CommentPost {
                page_id: page,
                thread_id: None,
                anchor: None,
                body,
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(
            core.dispatch(Request::CommentList { page_id: page })
                .unwrap(),
            Response::Comments(Vec::new())
        );
        // An unknown page is NotFound for every request.
        let missing = PageId::from_bytes([9; 16]);
        let error = core
            .dispatch(Request::CommentList { page_id: missing })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        // The notification kinds are the four of ADR-0048 and ADR-0049.
        assert_eq!(
            [
                NotificationKind::ThreadReply,
                NotificationKind::CommentMention,
                NotificationKind::PageMention,
                NotificationKind::PersonAssigned,
            ]
            .len(),
            4
        );
    }

    #[test]
    fn the_stream_id_of_a_page_is_the_first_half_of_blake3_of_domain_and_page() {
        let page = PageId::from_bytes([0x11; 16]);
        let mut input = b"atomik-kb/v1/comments".to_vec();
        input.extend_from_slice(&[0x11; 16]);
        let expected = blake3_prefix(&input);
        assert_eq!(comment_stream_doc_id(page).to_bytes(), expected);
        // Deterministic, and different for another page (and not the page's own document id).
        assert_eq!(comment_stream_doc_id(page), comment_stream_doc_id(page));
        assert_ne!(
            comment_stream_doc_id(page),
            comment_stream_doc_id(PageId::from_bytes([0x12; 16]))
        );
        assert_ne!(comment_stream_doc_id(page), DocId::for_page(page));
    }

    fn blake3_prefix(input: &[u8]) -> [u8; 16] {
        let digest = kb_crypto::hash(input);
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest[..16]);
        out
    }

    #[test]
    fn a_body_reads_as_its_text_and_names_the_accounts_it_mentions() {
        let me = AccountId::from_bytes([1; 16]);
        let body = vec![
            CommentBodyItem::Text {
                text: "Thanks".into(),
            },
            CommentBodyItem::Mention { account_id: me },
            CommentBodyItem::Page {
                id: PageId::from_bytes([2; 16]),
            },
            CommentBodyItem::Text {
                text: "again".into(),
            },
        ];
        assert_eq!(body_text(&body), "Thanks again");
        assert!(body_mentions(&body, me));
        assert!(!body_mentions(&body, AccountId::from_bytes([3; 16])));
    }
}
