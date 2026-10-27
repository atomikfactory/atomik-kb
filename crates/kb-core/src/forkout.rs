//! Fork-out (M13 slice 1, ADR-0043 section 10, `docs/security/sharing.md` section 3): a device that
//! learns it was demoted or removed while the feed holds (or refused) its later entries copies the
//! unsent changes of each page into a page "Unsent changes – <title>" of a personal space, then
//! re-bootstraps the shared documents from the feed. Emits `UnsentChangesCopied`.
//!
//! What counts as unsent: the `Local` updates of a page document the feed does not hold (written
//! here, never pushed or pushed after the device lost its role) and the page documents in which
//! another device's pull found an entry of this device that its role at that cursor did not allow
//! (`forkout_note_refused_own`).
//!
//! **Demotion** (the account is still a member): after the last page of a pull, for each such page
//! the content as this device has it is copied out, the local document is dropped, the feed's cursor
//! goes back to 0 and the entries and snapshots are applied again; this device's own entries of the
//! page that were valid when written come back through the feed (the `rebuild` list lets the apply
//! path take them although they are its own), the refused ones are quarantined like any other
//! member's. The structure document is not rebuilt (its changes by a demoted device stay local: a
//! documented limit). **Removal**: the unsent changes are copied out and the archive keeps what it
//! has (it cannot read the feed any more).

use kb_doc::{Doc, DocKind};
use kb_model::PageDoc;
use kb_storage::{DocKindCode, DocQueries, SyncQueries, UpdateOrigin};
use kb_types::{ApiError, DocId, Event, FeedId, MemberRole, PageId, SpaceId};

use crate::documents::{map_doc_error, map_model_error, new_peer};
use crate::import::PreparedDoc;
use crate::pages::text_entry_of;
use crate::security::{decode_hex, hex};
use crate::validate::MAX_PAGE_TITLE_CHARS;
use crate::{Core, map_storage_error};

/// What the copy of a page's unsent changes is titled before the page's own title (English only: the
/// core has no translations).
const UNSENT_TITLE: &str = "Unsent changes \u{2013} ";
/// The personal space created when the account has none to put the copies in.
const UNSENT_SPACE: &str = "Unsent changes";

/// The title of the copy of a page: the constant, then the title cut to fit the limit.
pub(crate) fn unsent_title(title: &str) -> String {
    let room = MAX_PAGE_TITLE_CHARS - UNSENT_TITLE.chars().count();
    let kept: String = title.chars().take(room).collect();
    format!("{UNSENT_TITLE}{}", kept.trim())
}

impl Core {
    /// Notes that an entry of this device was found in the feed at a cursor where its role did not
    /// allow it: the document is a fork-out candidate.
    pub(crate) fn forkout_note_refused_own(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(), ApiError> {
        if self.rebuilding(feed, doc_id)? {
            // The rebuild brings this entry in again: it was handled by the fork-out that began it.
            return Ok(());
        }
        let mut docs = self.forkout_docs(feed)?;
        if !docs.contains(&doc_id) {
            docs.push(doc_id);
            self.forkout_docs_set(feed, &docs)?;
        }
        self.shared.forkout_due.insert(feed);
        Ok(())
    }

    fn forkout_docs(&self, feed: FeedId) -> Result<Vec<DocId>, ApiError> {
        Ok(self
            .fcfg(feed, "forkdocs")?
            .unwrap_or_default()
            .split(',')
            .filter_map(|item| {
                let bytes: [u8; 16] = decode_hex(item)?.try_into().ok()?;
                Some(DocId::from_bytes(bytes))
            })
            .collect())
    }

    fn forkout_docs_set(&self, feed: FeedId, docs: &[DocId]) -> Result<(), ApiError> {
        if docs.is_empty() {
            return self.fcfg_del(feed, "forkdocs");
        }
        let list: Vec<String> = docs.iter().map(|doc| hex(doc.as_bytes())).collect();
        self.fcfg_set(feed, "forkdocs", &list.join(","))
    }

    /// Runs at the end of a pull: a demoted device with unsent or refused changes forks out; a
    /// rebuild that has caught up with the feed is over.
    pub(crate) fn forkout_check(&mut self, feed: FeedId) -> Result<(), ApiError> {
        if let Some(until) = self.fcfg_u64(feed, "rebuild_until")?
            && self.feed_row(feed)?.cursor >= until
        {
            self.fcfg_del(feed, "rebuild")?;
            self.fcfg_del(feed, "rebuild_until")?;
        }
        let due = self.shared.forkout_due.remove(&feed);
        let refused = self.forkout_docs(feed)?;
        if !due && refused.is_empty() {
            return Ok(());
        }
        let Some(space_id) = self.feed_space(feed)? else {
            return Ok(());
        };
        if self
            .feed_role(feed)?
            .is_some_and(|role| role >= MemberRole::Editor)
        {
            // Promoted again meanwhile: nothing was refused that matters any more.
            self.forkout_docs_set(feed, &[])?;
            return Ok(());
        }
        let pages = self.forkout_pages(feed, space_id, &refused)?;
        self.forkout_docs_set(feed, &[])?;
        if pages.is_empty() {
            return Ok(());
        }
        self.forkout_copy_pages(space_id, &pages)?;
        self.forkout_rebuild(feed, &pages)?;
        Ok(())
    }

    /// Copies the unsent changes of the pages of a shared space into new pages of a personal space
    /// and answers the new pages. Called by the membership code when the account's role stops
    /// allowing writes (`SpaceRemoved`) while local changes are unsent.
    pub(crate) fn forkout_unsent_changes(
        &mut self,
        space_id: SpaceId,
    ) -> Result<Vec<PageId>, ApiError> {
        let Some(row) = self.shared_feed_of(space_id)? else {
            return Ok(Vec::new());
        };
        let refused = self.forkout_docs(row.feed)?;
        let pages = self.forkout_pages(row.feed, space_id, &refused)?;
        self.forkout_docs_set(row.feed, &[])?;
        self.forkout_copy_pages(space_id, &pages)
    }

    /// The pages of the space with changes the feed does not hold or refused.
    fn forkout_pages(
        &self,
        feed: FeedId,
        space_id: SpaceId,
        refused: &[DocId],
    ) -> Result<Vec<PageId>, ApiError> {
        let mut pages = Vec::new();
        for row in self
            .db
            .docs_by_kind(DocKindCode::Page)
            .map_err(map_storage_error)?
        {
            if row.space_id != Some(space_id) {
                continue;
            }
            let unsent = self
                .doc_unsent_updates(feed, row.doc_id)?
                .iter()
                .any(|(_, _, origin)| *origin == UpdateOrigin::Local);
            if (unsent || refused.contains(&row.doc_id))
                && let Some(page) = self.page_of_doc(row.doc_id)
            {
                pages.push(page);
            }
        }
        Ok(pages)
    }

    /// A personal space to put the copies in: the first space of the account that is not shared, or a
    /// new one.
    fn forkout_target_space(&mut self) -> Result<SpaceId, ApiError> {
        let shared = self.shared_space_map()?;
        let ordered: Vec<SpaceId> = self.space_list()?.into_iter().map(|info| info.id).collect();
        if let Some(space) = ordered
            .into_iter()
            .find(|space| !shared.contains_key(space))
        {
            return Ok(space);
        }
        Ok(self.space_create(UNSENT_SPACE, None)?.id)
    }

    /// Makes "Unsent changes – <title>" pages of a personal space with the content of the pages as
    /// this device has them. Answers the new pages.
    fn forkout_copy_pages(
        &mut self,
        space_id: SpaceId,
        pages: &[PageId],
    ) -> Result<Vec<PageId>, ApiError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }
        let target = self.forkout_target_space()?;
        let mut copies = Vec::with_capacity(pages.len());
        let now = self.clock.now_ms();
        for page in pages {
            let title = self
                .structure(space_id)?
                .model
                .page(*page)
                .map(|info| info.title)
                .unwrap_or_default();
            let content = self.portable_content(*page)?;
            let copy = PageId::generate(self.rng.as_ref());
            let title = unsent_title(&title);
            self.mutate_structure(target, |model, now| {
                model.create_page(copy, None, None, &title, now)
            })?;
            self.text_after_create(copy, &title)?;
            let doc = Doc::new(DocKind::Page, new_peer(self.rng.as_ref()));
            let model = match &content {
                Some(content) => PageDoc::from_portable(doc, content),
                None => PageDoc::new(doc, kb_types::BlockId::generate(self.rng.as_ref())),
            }
            .map_err(map_model_error)?;
            let blob = model
                .doc()
                .export_updates_since(None)
                .map_err(map_doc_error)?
                .ok_or_else(|| ApiError::internal("a copied page document exported nothing"))?;
            let entry = text_entry_of(&model, &self.db);
            self.import_write_documents(
                target,
                vec![PreparedDoc {
                    page_id: copy,
                    doc_id: DocId::for_page(copy),
                    title,
                    blob,
                    version: model.doc().frontiers(),
                    entry,
                }],
                now,
            )?;
            copies.push(copy);
        }
        self.events.push(Event::UnsentChangesCopied {
            space_id,
            target_space_id: target,
            page_ids: copies.clone(),
        });
        tracing::info!(pages = copies.len(), "unsent changes copied out");
        Ok(copies)
    }

    /// Drops the local copy of the pages and has the feed give them back: the cursor returns to 0,
    /// the documents this device keeps are marked as covered up to where it was, the rebuilt ones
    /// take their own earlier entries again.
    fn forkout_rebuild(&mut self, feed: FeedId, pages: &[PageId]) -> Result<(), ApiError> {
        let mut row = self.feed_row(feed)?;
        let until = row.cursor;
        let docs: Vec<DocId> = pages.iter().map(|page| DocId::for_page(*page)).collect();
        for page in pages {
            self.docs.pages.remove(page);
        }
        self.db
            .transaction(|tx| {
                for doc in &docs {
                    tx.doc_delete(*doc)?;
                }
                tx.sync_seen_clear_in(feed)?;
                tx.sync_pending_clear_in(feed)?;
                for mut state in tx.sync_doc_state_all_in(feed)? {
                    if docs.contains(&state.doc_id) {
                        state.pushed_up_to_seq = 0;
                        state.snapshot_covers = None;
                        state.remote_snapshot_covers = None;
                        state.entries_since_snapshot = 0;
                        state.bytes_since_snapshot = 0;
                    } else if let Some(covers) = state.remote_snapshot_covers {
                        state.remote_snapshot_covers = Some(covers.max(until));
                    }
                    tx.sync_doc_state_put_in(feed, &state)?;
                }
                Ok::<(), kb_storage::StorageError>(())
            })
            .map_err(map_storage_error)?;
        row.cursor = 0;
        row.chain_hash = None;
        row.chain_history = None;
        self.feed_row_put(&row)?;
        let list: Vec<String> = docs.iter().map(|doc| hex(doc.as_bytes())).collect();
        self.fcfg_set(feed, "rebuild", &list.join(","))?;
        self.fcfg_set(feed, "rebuild_until", &until.to_string())?;
        self.shared.snapshots_checked_at.remove(&feed);
        for page in pages {
            self.events.push(Event::PageContentChanged {
                page_id: *page,
                source: kb_types::ChangeSource::Remote,
            });
        }
        self.sync_note_local_change();
        Ok(())
    }
}
