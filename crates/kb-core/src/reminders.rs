//! Reminders on date mentions (ADR-0024, `docs/product/feature-gap-analysis.md` section 3.1).
//!
//! A date mention (`dateMention`, ADR-0016) may carry a `reminder`: minutes before its moment,
//! `date` at `time` (09:00 for a whole day). The derived `date_mentions` table (schema v9,
//! rebuilt from the page documents with every save exactly like `links`, see `pages.rs`) holds
//! each mention with its fire time, and this module answers two questions from it:
//!
//! - [`Request::RemindersDue`]: which reminders fell due since this device last looked, a pure
//!   query;
//! - [`Core::reminders_tick`]: the scheduler. The shell calls it every 30 to 60 seconds while
//!   the app runs and when the core becomes ready (at launch, after an unlock); it emits one
//!   [`Event::ReminderDue`] per reminder that fell due and moves the per-device mark
//!   `app.reminders.last_checked`, so every reminder fires once.
//!
//! **Time.** A mention is a floating local date (ADR-0016): "7 October 14:30" is that wall clock
//! time wherever its reader is, and the core has no time zone data (it builds for wasm32). So
//! a reminder is due when this device's *wall clock* reaches its moment, and every time here,
//! the stored `fire_at_local`, the `until` of the request, the mark and the `fire_at` of an
//! event, is on that clock: seconds since 1970-01-01T00:00:00 as the local clock reads them,
//! that is UTC plus the device's offset ([`Clock::local_offset_seconds`], told by the
//! platform). This is also what keeps daylight saving right without a zone database: the
//! clock jumps forward and the reminders that were skipped fire (they are inside the window);
//! it falls back and nothing fires twice (the mark never moves back).
//!
//! **Catch-up.** A reminder only fires while the app runs and is unlocked (no tray, no
//! background process, nothing reads documents while locked). What fell due meanwhile fires at
//! the first tick afterwards, within [`CATCH_UP_SECONDS`] (seven days) back; with no mark yet
//! (the first tick of this device) the same seven days apply. A reminder set on a moment that
//! already passed before the last tick never fires: only moments the clock crosses after the
//! mark do, which is what makes "set a reminder on yesterday's date" silent.

use kb_model::PageDoc;
use kb_storage::{
    DateMentionQueries, DateMentionRow, DocQueries, DueReminderRow, TextIndexQueries,
};
use kb_types::{ApiError, Event, Json, ReminderDue, Request, Response};

use crate::pages::truncate_chars;
use crate::{Core, map_storage_error};

/// Setting key of the per-device mark: the fire time (on the local wall clock, in seconds) up
/// to which reminders have been fired. Local like every setting; internal (the UI neither
/// reads nor writes it).
pub const LAST_CHECKED_SETTING: &str = "app.reminders.last_checked";

/// How far back a tick looks for reminders it missed, in seconds: seven days.
pub const CATCH_UP_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Most reminders [`Request::RemindersDue`] returns.
pub const MAX_REMINDERS_DUE: u32 = 100;

/// Most reminders one tick fires (more of them due at once, after a long absence, come one
/// batch per tick, so neither the notification centre nor the toast stack floods). The ones
/// that fire at the same moment as the last of a batch go with it.
pub const FIRES_PER_TICK: usize = 25;

/// Longest block text carried by a reminder, in characters (as for backlinks).
const REMINDER_TEXT_CHARS: usize = 200;

/// The most rows one query of due reminders reads while it looks past the ones this device does not
/// fire.
const REMINDER_WINDOW_MAX: u32 = 4_096;

/// `meta` key that says the `date_mentions` table was filled from the documents (a database
/// that was migrated from before schema v9 has the table empty).
const INDEXED_MARKER: &str = "date_mentions_indexed";

/// The `date_mentions` rows of a page: its date mentions with their fire times. A reminder
/// that does not fit the table (more minutes than a `u32`, or an arithmetic overflow of the
/// fire time) is no reminder.
pub(crate) fn date_mention_rows(model: &PageDoc) -> Vec<DateMentionRow> {
    model
        .date_mentions()
        .into_iter()
        .map(|mention| {
            let (reminder_minutes, fire_at_local) = match (
                mention
                    .reminder_minutes
                    .and_then(|minutes| u32::try_from(minutes).ok()),
                mention.fire_at_local,
            ) {
                (Some(minutes), Some(fire)) => (Some(minutes), Some(fire)),
                _ => (None, None),
            };
            DateMentionRow {
                block_id: mention.block_id,
                node_id: mention.node_id,
                date: mention.date,
                time: mention.time,
                reminder_minutes,
                fire_at_local,
                reminder_for: mention.reminder_for,
            }
        })
        .collect()
}

impl Core {
    /// Dispatch entry for the reminder requests.
    pub(crate) fn dispatch_reminders(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::RemindersDue { until } => Ok(Response::Reminders(self.reminders_due(until)?)),
            other => Err(ApiError::internal(format!(
                "request {} is not a reminders request",
                other.kind()
            ))),
        }
    }

    /// The scheduler tick: fires the reminders that fell due since the last one.
    ///
    /// Reads the clock (UTC plus the platform's offset, so the device's wall clock), takes the
    /// reminders whose fire time is after the mark `app.reminders.last_checked` (at most seven
    /// days back, see the module docs) and not after now, emits an [`Event::ReminderDue`] for
    /// each (oldest first, at most [`FIRES_PER_TICK`], the rest at the next tick), and moves the
    /// mark to now (to the fire time of the last one fired when some were left for later). It
    /// never moves the mark back, so nothing fires twice. Returns how many reminders fired.
    ///
    /// The shell calls it every 30 to 60 seconds and when the core becomes ready; there is no
    /// tick while the workspace is locked (nothing is open), and the first one after the
    /// unlock catches up. It does no I/O but the database's.
    ///
    /// # Errors
    ///
    /// `Storage` when the database fails; nothing fires and the mark does not move then, so
    /// the next tick tries again.
    pub fn reminders_tick(&mut self) -> Result<usize, ApiError> {
        let until = self.local_now_seconds();
        let last = self.reminders_last_checked()?;
        let after = Self::reminders_window_start(last, until);
        let mut batch = self.reminders_in(after, until, FIRES_PER_TICK as u32 + 1)?;
        let mark = if batch.len() > FIRES_PER_TICK {
            // More than one tick fires: the oldest ones now (with whatever fires at the same
            // moment as the last of them), the rest at the next tick.
            let cut = batch[FIRES_PER_TICK - 1].fire_at;
            batch = self.reminders_in(after, cut, MAX_REMINDERS_DUE)?;
            cut
        } else {
            last.map_or(until, |last| last.max(until))
        };
        if last != Some(mark) {
            let now_ms = self.clock.now_ms();
            self.db
                .setting_set(LAST_CHECKED_SETTING, &Json::from(mark), now_ms)
                .map_err(map_storage_error)?;
        }
        let fired = batch.len();
        // Counts only: what a reminder says is the user's content.
        tracing::debug!(fired, "reminders tick");
        self.events
            .extend(batch.into_iter().map(Event::ReminderDue));
        Ok(fired)
    }

    /// The reminders that fell due before `until` and since the mark (see
    /// [`Request::RemindersDue`]), oldest first, at most [`MAX_REMINDERS_DUE`].
    fn reminders_due(&self, until: i64) -> Result<Vec<ReminderDue>, ApiError> {
        let after = Self::reminders_window_start(self.reminders_last_checked()?, until);
        self.reminders_in(after, until, MAX_REMINDERS_DUE)
    }

    /// The device's wall clock now: seconds since 1970-01-01T00:00:00 as the local clock
    /// reads it.
    fn local_now_seconds(&self) -> i64 {
        self.clock
            .now_ms()
            .div_euclid(1000)
            .saturating_add(i64::from(self.clock.local_offset_seconds()))
    }

    /// The mark, `None` while this device has never ticked (a stored value that is not an
    /// integer is as good as none).
    fn reminders_last_checked(&self) -> Result<Option<i64>, ApiError> {
        Ok(self
            .db
            .setting_get(LAST_CHECKED_SETTING)
            .map_err(map_storage_error)?
            .and_then(|value| value.as_i64()))
    }

    /// Where the window of due reminders starts (exclusive): the mark, but never more than
    /// [`CATCH_UP_SECONDS`] before `until`.
    fn reminders_window_start(last: Option<i64>, until: i64) -> i64 {
        let floor = until.saturating_sub(CATCH_UP_SECONDS);
        last.map_or(floor, |last| last.max(floor))
    }

    /// The reminders with a fire time after `after` and not after `until`, as the API shows
    /// them: the page's title and the block's text come from the indexes.
    fn reminders_in(
        &self,
        after: i64,
        until: i64,
        limit: u32,
    ) -> Result<Vec<ReminderDue>, ApiError> {
        // A reminder of a shared space fires on the devices of the account its mention names
        // (`reminderFor`), and, when the mention names nobody, on the devices of the space's Owners
        // (ADR-0047): the rest of the members do not hear a reminder somebody else set. Rows that do
        // not fire here are dropped, and the window grows when they left too few.
        let mut fetch = limit;
        loop {
            let rows = self
                .db
                .date_mentions_due(after, until, fetch)
                .map_err(map_storage_error)?;
            let exhausted = u32::try_from(rows.len()).unwrap_or(u32::MAX) < fetch;
            let mut kept: Vec<DueReminderRow> = rows
                .into_iter()
                .filter(|row| self.fires_reminder(row.space_id, row.reminder_for))
                .collect();
            if kept.len() >= limit as usize || exhausted || fetch >= REMINDER_WINDOW_MAX {
                kept.truncate(limit as usize);
                return kept.into_iter().map(|row| self.reminder_of(row)).collect();
            }
            fetch = fetch.saturating_mul(4).min(REMINDER_WINDOW_MAX);
        }
    }

    fn reminder_of(&self, row: DueReminderRow) -> Result<ReminderDue, ApiError> {
        let page_title = self
            .db
            .pages_index_get(row.page_id)
            .map_err(map_storage_error)?
            .map(|page| page.title)
            .unwrap_or_default();
        let text = self
            .db
            .fts_block_text(row.page_id, row.block_id)
            .map_err(map_storage_error)?
            .unwrap_or_default();
        Ok(ReminderDue {
            page_id: row.page_id,
            space_id: row.space_id,
            block_id: row.block_id,
            node_id: row.node_id,
            page_title,
            text: truncate_chars(&text, REMINDER_TEXT_CHARS),
            date: row.date,
            time: row.time,
            reminder_minutes: row.reminder_minutes,
            fire_at: row.fire_at_local,
        })
    }

    /// Fills the `date_mentions` table from the documents once, for a database that was
    /// migrated from before schema v9 (its pages hold mentions the table does not know yet).
    /// Runs at open, from `verify_text_index`; a fresh database has no pages and just gets the
    /// marker.
    pub(crate) fn verify_date_mentions(&mut self) -> Result<(), ApiError> {
        if self
            .db
            .meta_get(INDEXED_MARKER)
            .map_err(map_storage_error)?
            .is_some()
        {
            return Ok(());
        }
        let spaces: Vec<_> = self.docs.structures.keys().copied().collect();
        for space_id in spaces {
            let rows = self
                .db
                .pages_index_by_space(space_id)
                .map_err(map_storage_error)?;
            for row in rows {
                let entry = self.text_entry(row.page_id, &row.title)?;
                self.db
                    .date_mentions_replace(row.page_id, &entry.date_mentions)
                    .map_err(map_storage_error)?;
            }
        }
        self.db
            .meta_set(INDEXED_MARKER, "1")
            .map_err(map_storage_error)
    }
}
