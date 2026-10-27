//! Page history (ADR-0057, `docs/architecture/page-history.md`): the versions of a page from the
//! device's local journal (dated, with an author) and the document's older history (undated), a
//! version's content, and restoring one as a new change.
//!
//! A **version** is a state of the page document: its Loro frontiers written as an id
//! (`kb_doc::Frontiers`). The **journal** (`page_history_journal`, schema v21) keeps one row per
//! editing session this device witnessed: its own writes (the editor, the core: an import, a
//! template, a duplicate, MCP, a restore) and the sync entries it applied. Dates and authors exist
//! nowhere else (the documents record neither, ADR-0011), so the journal is not derived, never
//! synced and not in a backup. Every row also keeps the document's version before the session's
//! first write (`base_version`): the operations of `version` that are not in the base are what the
//! session wrote, so everything the journal does not account for is the history older than it, listed as
//! **undated** versions (one per run of operations of one replica, `kb_doc::Doc::runs_outside`).

use std::collections::HashSet;

use kb_doc::{Doc, DocError, DocKind, Frontiers, Span};
use kb_model::{ModelError, PageDoc};
use kb_storage::{
    DocQueries, JournalKind, JournalOp, JournalRow, NewJournalRow, PageHistoryQueries, UpdateOrigin,
};
use kb_types::{
    AccountId, ApiError, ChangeSource, DeviceId, DocId, MemberRole, PageHistory, PageId, PageInfo,
    PageVersion, PageVersionContent, PageVersionKind, Request, Response, VersionAuthor,
};

use crate::documents::{load_doc, map_doc_error, map_model_error, new_peer};
use crate::{Core, map_storage_error, short_id};

/// A write continues the latest journal row of the same author and kind when it comes at most this
/// long after the row's last change (ten minutes): one editing session.
pub(crate) const SESSION_GAP_MS: i64 = 600_000;

/// How many versions a list answers when the request does not say.
const DEFAULT_LIMIT: u32 = 100;
/// The most versions one list answers.
const MAX_LIMIT: u32 = 500;

/// Who made a write of a page document, for the journal.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Writer {
    /// This device (and this account): the editor, or the core on the user's behalf.
    ThisDevice,
    /// A sync entry applied from a feed: the account and device that wrote it (the entry's header).
    Remote {
        /// The writer's account.
        account: AccountId,
        /// The writer's device.
        device: DeviceId,
    },
}

/// One persisted write of a page document, for the journal.
pub(crate) struct JournalWrite<'a> {
    /// The page.
    pub(crate) page_id: PageId,
    /// The document's version before the write.
    pub(crate) before: &'a Frontiers,
    /// The document's version after it.
    pub(crate) after: &'a Frontiers,
    /// Who made it.
    pub(crate) writer: Writer,
    /// What made it: an edit continues the session of the same author, a restore starts a row.
    pub(crate) kind: JournalKind,
}

impl Core {
    /// Routes the page history requests.
    pub(crate) fn dispatch_history(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::PageHistoryList {
                page_id,
                before,
                limit,
            } => Ok(Response::PageHistory(
                self.page_history_list(page_id, before, limit)?,
            )),
            Request::PageHistoryGet { page_id, version } => Ok(Response::PageVersionContent(
                self.page_history_get(page_id, &version)?,
            )),
            Request::PageHistoryRestore { page_id, version } => Ok(Response::Page(
                self.page_history_restore(page_id, &version)?,
            )),
            _ => Err(ApiError::invalid("request", "not a page history request")),
        }
    }

    // ----- the journal ---------------------------------------------------------------------

    /// Decides what the journal records for a write of a page document that is about to be stored
    /// (`docs/architecture/page-history.md` section 3.2): it continues the document's latest row when
    /// that is an edit by the same author at most [`SESSION_GAP_MS`] before (and this is an edit
    /// too), else it starts a row; a write that leaves the version as it was records nothing.
    ///
    /// This only **reads** (the latest row, the clock, the author): the caller runs the answer with
    /// [`record_journal`] inside the storage transaction of the write itself, so the journal costs no
    /// commit of its own. `title` is the page's title now, read by the caller before its transaction.
    pub(crate) fn journal_plan(
        &self,
        write: &JournalWrite<'_>,
        title: Option<String>,
    ) -> Option<JournalOp> {
        if write.before == write.after {
            return None;
        }
        let doc_id = DocId::for_page(write.page_id);
        let now = self.clock.now_ms();
        let (device_id, account_id, this_device) = self.journal_author(write.writer);
        let latest = match self.db.journal_latest(doc_id) {
            Ok(latest) => latest,
            Err(err) => {
                tracing::warn!(page = %short_id(&write.page_id), error = %err, "page history journal not read");
                return None;
            }
        };
        if let Some(latest) = latest
            && write.kind == JournalKind::Edit
            && latest.kind == JournalKind::Edit
            && same_author(&latest, device_id, account_id)
            && now - latest.ended_at <= SESSION_GAP_MS
        {
            return Some(JournalOp::Extend {
                doc_id,
                seq: latest.seq,
                ended_at: now.max(latest.ended_at),
                version: write.after.encode(),
                title,
            });
        }
        Some(JournalOp::Append(NewJournalRow {
            doc_id,
            kind: write.kind,
            started_at: now,
            ended_at: now,
            base_version: write.before.encode(),
            version: write.after.encode(),
            device_id,
            account_id,
            this_device,
            title,
        }))
    }

    /// The journal's first row of a document that is being created (a new page, an import, a
    /// duplicate, a template): this device's edit that began with the document, with `after` as its
    /// version. No read: a document that does not exist has no rows.
    pub(crate) fn journal_plan_new(
        &self,
        page_id: PageId,
        after: &Frontiers,
        title: Option<String>,
    ) -> Option<JournalOp> {
        if after.is_empty() {
            return None;
        }
        let (device_id, account_id, this_device) = self.journal_author(Writer::ThisDevice);
        let now = self.clock.now_ms();
        Some(JournalOp::Append(NewJournalRow {
            doc_id: DocId::for_page(page_id),
            kind: JournalKind::Edit,
            started_at: now,
            ended_at: now,
            base_version: String::new(),
            version: after.encode(),
            device_id,
            account_id,
            this_device,
            title,
        }))
    }

    /// The journal's author of a write: `(device, account, this device)`.
    fn journal_author(&self, writer: Writer) -> (Option<DeviceId>, Option<AccountId>, bool) {
        match writer {
            Writer::ThisDevice => (
                Some(self.device.device_id),
                self.sync_identity()
                    .ok()
                    .flatten()
                    .map(|(account, _)| account),
                true,
            ),
            Writer::Remote { account, device } => {
                (Some(device), Some(account), device == self.device.device_id)
            }
        }
    }

    /// The journal of purged pages goes with them: the rows are not derived data, so the purge
    /// removes them by name (the documents' rows, and the rows of the journal with them, are
    /// collected at the next open).
    pub(crate) fn history_after_purge(&mut self, pages: &[PageId]) -> Result<(), ApiError> {
        let docs: Vec<DocId> = pages.iter().map(|page| DocId::for_page(*page)).collect();
        self.db
            .journal_delete_docs(&docs)
            .map_err(map_storage_error)
    }

    // ----- the requests --------------------------------------------------------------------

    /// Runs `f` on a page's document: the open one, else a transient load. `None` when the page has
    /// no document yet (nothing is created: asking for history leaves no trace).
    fn history_view<T>(
        &self,
        page_id: PageId,
        f: impl FnOnce(&Doc) -> Result<T, ApiError>,
    ) -> Result<Option<T>, ApiError> {
        if let Some(open) = self.docs.pages.get(&page_id) {
            return f(open.model.doc()).map(Some);
        }
        let Some(row) = self
            .db
            .doc_get(DocId::for_page(page_id))
            .map_err(map_storage_error)?
        else {
            return Ok(None);
        };
        let doc = load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
        f(&doc).map(Some)
    }

    /// `PageHistoryList`: the dated versions (the journal, newest first), then the document's older
    /// history as undated versions (newest first), from `before` on, at most `limit`.
    pub(crate) fn page_history_list(
        &mut self,
        page_id: PageId,
        before: Option<String>,
        limit: Option<u32>,
    ) -> Result<PageHistory, ApiError> {
        self.space_of_page(page_id)?;
        let rows = self
            .db
            .journal_list(DocId::for_page(page_id), None, None)
            .map_err(map_storage_error)?;
        // One look at the document: the whole list, and where `before` stands in it.
        let listed = self.history_view(page_id, |doc| {
            let versions = listed_versions(doc, &rows)?;
            let wanted = before
                .as_deref()
                .map(|before| {
                    doc.resolve_frontiers(before)
                        .map(|version| version.encode())
                        .map_err(|err| version_error("before", err))
                })
                .transpose()?;
            Ok((versions, wanted))
        })?;
        let (versions, wanted) = match listed {
            Some(listed) => listed,
            // No document, no history: nothing to list, and no `before` to stand after.
            None if before.is_none() => (Vec::new(), None),
            None => return Err(ApiError::invalid("before", "the page has no such version")),
        };
        let limit = usize::try_from(limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
            .unwrap_or(usize::MAX);
        let start = match wanted {
            None => 0,
            Some(wanted) => versions
                .iter()
                .position(|version| version.id == wanted)
                .map(|index| index + 1)
                .ok_or_else(|| {
                    ApiError::invalid("before", "that version is not in the page's history")
                })?,
        };
        let rest = versions.into_iter().skip(start).collect::<Vec<_>>();
        let more = rest.len() > limit;
        Ok(PageHistory {
            page_id,
            versions: rest.into_iter().take(limit).collect(),
            more,
        })
    }

    /// `PageHistoryGet`: the content of the page as it was at a version (the document forked there,
    /// read like `PageContentGet` reads it), and the title the journal recorded for that version.
    pub(crate) fn page_history_get(
        &mut self,
        page_id: PageId,
        version: &str,
    ) -> Result<PageVersionContent, ApiError> {
        self.space_of_page(page_id)?;
        let peer = new_peer(self.rng.as_ref());
        let (id, content) = self
            .history_view(page_id, |doc| {
                let at = doc
                    .resolve_frontiers(version)
                    .map_err(|err| version_error("version", err))?;
                let fork = doc
                    .fork_at(&at, peer)
                    .map_err(|err| version_error("version", err))?;
                let content = PageDoc::from_doc(fork)
                    .map_err(map_model_error)?
                    .to_portable();
                Ok((at.encode(), content))
            })?
            .ok_or_else(|| ApiError::invalid("version", "the page has no such version"))?;
        let title = self
            .db
            .journal_find_version(DocId::for_page(page_id), &id)
            .map_err(map_storage_error)?
            .and_then(|row| row.title);
        Ok(PageVersionContent {
            page_id,
            version: id,
            content,
            title,
        })
    }

    /// `PageHistoryRestore`: makes the page's content, and its title when the journal knows it for
    /// that version, what it was, as a new change on top of the current one (see the request).
    pub(crate) fn page_history_restore(
        &mut self,
        page_id: PageId,
        version: &str,
    ) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let info = self.page_get(page_id)?;
        if info.trashed_at.is_some() {
            return Err(map_model_error(ModelError::PageTrashed));
        }
        // Read only, so a version the page does not have creates nothing.
        let (to, current) = self
            .history_view(page_id, |doc| {
                let to = doc
                    .resolve_frontiers(version)
                    .map_err(|err| version_error("version", err))?;
                Ok((to, doc.frontiers()))
            })?
            .ok_or_else(|| ApiError::invalid("version", "the page has no such version"))?;
        if to == current {
            // The version the page has now: nothing to do.
            return Ok(info);
        }
        let target_title = self
            .db
            .journal_find_version(DocId::for_page(page_id), &to.encode())
            .map_err(map_storage_error)?
            .and_then(|row| row.title);

        // The title first, so that the journal row of the restore says what the page is called
        // after it. A normal structure write: persisted, indexed, announced.
        if let Some(title) = target_title
            && title != info.title
        {
            self.page_rename(page_id, &title)?;
        }

        let was_open = self.docs.pages.contains_key(&page_id);
        let result = self.restore_content(page_id, &to);
        if !was_open {
            // The replica was loaded for this; the editor does not hold it.
            self.docs.pages.remove(&page_id);
        }
        result?;
        self.page_get(page_id)
    }

    /// Reverts the core's replica of the page to `to` in one commit and persists it like any core
    /// write to a page (the update is stored, so it syncs; the text, links, date mentions and
    /// attachment rows follow), as a restore row of the journal.
    fn restore_content(&mut self, page_id: PageId, to: &Frontiers) -> Result<(), ApiError> {
        let (before, after) = {
            let open = self.open_page(page_id)?;
            let doc = open.model.doc();
            let before = doc.frontiers();
            doc.revert_to(to)
                .map_err(|err| version_error("version", err))?;
            (before, doc.frontiers())
        };
        if before == after {
            // The content already reads as it did then: no operation, nothing to store.
            return Ok(());
        }
        self.persist_page_as(
            page_id,
            None,
            UpdateOrigin::Local,
            ChangeSource::Core,
            JournalKind::Restore,
        )?;
        // The page's last edit moves with its content, as for any write.
        self.page_content_edited(page_id)
    }
}

/// Whether the latest journal row was written by the author of a new write: the device when both
/// know it, else the account. An unknown author is nobody's session to continue.
fn same_author(row: &JournalRow, device: Option<DeviceId>, account: Option<AccountId>) -> bool {
    match (row.device_id, device) {
        (Some(a), Some(b)) => a == b,
        (None, None) => matches!((row.account_id, account), (Some(a), Some(b)) if a == b),
        _ => false,
    }
}

/// The versions of a document, newest first: one per journal row (`rows` newest first, a version
/// that follows itself listed once), then the undated runs, then (when nothing else is) the current
/// version. The one that is the document's frontiers now is marked `current`.
fn listed_versions(doc: &Doc, rows: &[JournalRow]) -> Result<Vec<PageVersion>, ApiError> {
    let mut versions: Vec<PageVersion> = Vec::with_capacity(rows.len());
    for row in rows {
        if versions.last().is_some_and(|last| last.id == row.version) {
            continue;
        }
        versions.push(PageVersion {
            id: row.version.clone(),
            kind: match row.kind {
                JournalKind::Edit => PageVersionKind::Edit,
                JournalKind::Restore => PageVersionKind::Restore,
            },
            started_at: Some(row.started_at),
            ended_at: Some(row.ended_at),
            author: author_of(row),
            title: row.title.clone(),
            current: false,
        });
    }

    // What no row accounts for: the history before the journal. A row whose versions do not parse
    // is left out of the accounting (its operations stay in their runs).
    let spans: Vec<Span> = rows
        .iter()
        .filter_map(|row| {
            Some(Span {
                base: Frontiers::parse(&row.base_version).ok()?,
                end: Frontiers::parse(&row.version).ok()?,
            })
        })
        .collect();
    let dated: HashSet<String> = versions.iter().map(|v| v.id.clone()).collect();
    for run in doc.runs_outside(&spans).map_err(map_doc_error)? {
        let id = run.version.encode();
        if dated.contains(&id) {
            continue;
        }
        versions.push(PageVersion {
            id,
            kind: PageVersionKind::Edit,
            started_at: None,
            ended_at: None,
            author: None,
            title: None,
            current: false,
        });
    }

    let current = doc.frontiers();
    let current_id = current.encode();
    let mut found = false;
    for version in &mut versions {
        if version.id == current_id {
            version.current = true;
            found = true;
        }
    }
    if !found && !current.is_empty() {
        // The document's state is no version listed (heads merged without a journal row after
        // them): it is listed, first, so that there is always a current version to compare with.
        versions.insert(
            0,
            PageVersion {
                id: current_id,
                kind: PageVersionKind::Edit,
                started_at: None,
                ended_at: None,
                author: None,
                title: None,
                current: true,
            },
        );
    }
    Ok(versions)
}

/// The author a journal row says, `None` when nothing is known.
fn author_of(row: &JournalRow) -> Option<VersionAuthor> {
    if row.device_id.is_none() && row.account_id.is_none() && !row.this_device {
        return None;
    }
    Some(VersionAuthor {
        device_id: row.device_id,
        account_id: row.account_id,
        this_device: row.this_device,
    })
}

/// A version id the engine refused, as the request's `InvalidInput` on `field`.
fn version_error(field: &'static str, err: DocError) -> ApiError {
    match err {
        DocError::InvalidVersion(message) => ApiError::invalid(field, message),
        other => map_doc_error(other),
    }
}

/// Runs a journal write (see [`Core::journal_plan`]) inside the storage transaction of the write it
/// records. **Best effort** under a savepoint: when it fails, only the journal's statements are
/// rolled back, the failure is logged and the write commits as it would have: the journal is a
/// convenience over history that is stored anyway.
pub(crate) fn record_journal(tx: &dyn PageHistoryQueries, page_id: PageId, op: Option<&JournalOp>) {
    if let Some(op) = op
        && let Err(err) = tx.journal_record(op)
    {
        tracing::warn!(page = %short_id(&page_id), error = %err, "page history journal not written");
    }
}
