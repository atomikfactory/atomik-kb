//! Trash expiry (ADR-0040 item 3, open question 9): a page in the trash is purged for good once it
//! has been there longer than the device setting `app.trash.retention_days`.
//!
//! **The setting** is a number of days, read at each run like the attachments' retention
//! (`app.attachments.retention_days`, ADR-0020): fractions are allowed, a value that is not a
//! number is as good as none and means [`DEFAULT_TRASH_RETENTION_DAYS`] (30), and `0` (or any
//! value that is not positive or not finite) keeps trashed pages forever. The setting is local
//! like every setting, but its effect is not: the purge is the ordinary one
//! ([`Core::page_purge`]), a structure update that reaches every synced device, so a device with
//! a shorter limit purges for all.
//!
//! **What expires.** The top-level entries of each space's trash, the ones `TreeGet` lists as
//! `trashed`, whose `trashed_at` is strictly older than the limit (`now - trashed_at > limit`, so a
//! page is still there at exactly the limit and goes one millisecond later). A page that was
//! trashed with its parent has the parent's `trashed_at` and is no entry of its own: it goes with
//! its subtree. A page restored and trashed again has a new `trashed_at` (a restore removes it).
//!
//! **When.** [`Core::trash_expire`] runs at every open and unlock (`finish_open`), and the shell
//! calls it from its reminders tick, gated to once an hour while the app runs and is unlocked.
//! There is no timer or thread here. A locked workspace has no `Core`, so nothing expires while
//! it is locked; the unlock catches up.

use kb_types::{ApiError, Event, MemberRole, PageId, SpaceId};

use crate::{Core, map_storage_error};

/// Setting key of the number of days a page stays in the trash (a device setting the core reads
/// when it expires pages).
pub const TRASH_RETENTION_SETTING: &str = "app.trash.retention_days";

/// Days a page stays in the trash unless the setting says otherwise (`0` keeps it forever).
pub const DEFAULT_TRASH_RETENTION_DAYS: u32 = 30;

const MS_PER_DAY: f64 = 86_400_000.0;

impl Core {
    /// The limit in milliseconds from the setting; `None` keeps trashed pages forever.
    fn trash_retention_ms(&self) -> Result<Option<i64>, ApiError> {
        let days = self
            .db
            .setting_get(TRASH_RETENTION_SETTING)
            .map_err(map_storage_error)?
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::from(DEFAULT_TRASH_RETENTION_DAYS));
        if days <= 0.0 || !days.is_finite() {
            return Ok(None);
        }
        // A float-to-integer cast saturates, so an enormous limit is simply never reached.
        Ok(Some((days * MS_PER_DAY) as i64))
    }

    /// Purges every top-level trashed page (and with it its subtree) that has been in the trash
    /// for longer than the limit, in every space. Returns how many pages it purged (the entries,
    /// not their sub-pages).
    ///
    /// Each purge is [`Core::page_purge`]'s: the page and its sub-pages leave the structure
    /// document with their tombstones (which sync carries to the other devices), their keys are
    /// erased, and their derived rows, attachment references and relations are cleaned up. The
    /// events are those of a purge, with the repeats of one run dropped, so an open trash panel
    /// and the tree refresh once.
    ///
    /// Logs a count at `info` when something expired, nothing else (no titles, no ids).
    ///
    /// # Errors
    ///
    /// `Storage` (or whatever a purge answers) when a purge fails; the pages purged before it
    /// stay purged and the next run tries the rest.
    pub fn trash_expire(&mut self) -> Result<usize, ApiError> {
        let device = self.trash_retention_ms()?;
        let now = self.clock.now_ms();
        // Collected first: a purge changes the documents this reads. Each space has its own limit: the
        // device setting for a personal one, the Owners' setting for a shared one, which only an
        // Owner's device purges (ADR-0047).
        let space_ids: Vec<SpaceId> = self.docs.structures.keys().copied().collect();
        let mut expired: Vec<PageId> = Vec::new();
        for space_id in space_ids {
            let Some(Some(retention)) = self.trash_policy(space_id, device)? else {
                continue;
            };
            let cutoff = now.saturating_sub(retention);
            if let Some(open) = self.docs.structures.get(&space_id) {
                expired.extend(
                    open.model
                        .trashed_pages()
                        .into_iter()
                        .filter(|entry| entry.page.trashed_at.is_some_and(|at| at < cutoff))
                        .map(|entry| entry.page.id),
                );
            }
        }
        // The spaces come in no particular order: the purges do, so that a run is repeatable.
        expired.sort_unstable();

        let first_event = self.events.len();
        let mut purged = 0;
        let mut failure = None;
        for page_id in expired {
            match self.page_purge(page_id) {
                Ok(()) => purged += 1,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        dedupe_events(&mut self.events, first_event);
        if purged > 0 {
            tracing::info!(pages = purged, "expired pages purged from the trash");
        }
        failure.map_or(Ok(purged), Err)
    }

    /// How a space's trash expires: `None` when this device does not purge it (a shared space whose
    /// Owner this account is not, or an archive); `Some(None)` keeps it for ever; `Some(Some(ms))` is
    /// the limit. A personal space follows the device setting `device`; a shared one follows the
    /// `Settings` entry of its log (30 days until an Owner says otherwise, 0 keeps).
    fn trash_policy(
        &mut self,
        space_id: SpaceId,
        device: Option<i64>,
    ) -> Result<Option<Option<i64>>, ApiError> {
        let Some(row) = self.shared_feed_of(space_id)? else {
            return Ok(Some(device));
        };
        if self.require_role(space_id, MemberRole::Owner).is_err() {
            return Ok(None);
        }
        let days = self.shared.metas.get(&row.feed).map_or(
            kb_types::sharing::DEFAULT_SPACE_TRASH_RETENTION_DAYS,
            |meta| meta.settings.trash_retention_days,
        );
        Ok(Some(if days == 0 {
            None
        } else {
            Some(i64::from(days).saturating_mul(86_400_000))
        }))
    }

    /// [`Core::trash_expire`] at open and unlock. Housekeeping must not stop a workspace from
    /// opening, so a failure is logged (its code only) and the next run tries again.
    pub(crate) fn trash_expire_at_open(&mut self) {
        if let Err(error) = self.trash_expire() {
            tracing::warn!(code = ?error.code, "expiring the trash failed");
        }
    }
}

/// Drops the repeats among `events[from..]`, keeping the first of each.
fn dedupe_events(events: &mut Vec<Event>, from: usize) {
    let mut kept: Vec<Event> = Vec::new();
    for event in events.drain(from..) {
        if !kept.contains(&event) {
            kept.push(event);
        }
    }
    events.extend(kept);
}
