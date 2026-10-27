//! The shell's side of reminders on date mentions (ADR-0024): a timer that ticks the core's
//! scheduler and the OS notification each reminder gets.
//!
//! The core owns the scheduling (`kb_core::Core::reminders_tick`: which reminders fell due since
//! the last tick, once each, within seven days back); the shell only supplies the clock and the
//! thread. [`reminders_starter`] is background work like the sync driver's: it starts when the
//! core becomes ready (at launch, after an unlock), ticks at once to catch up on what fell due
//! while the app was closed or locked, then every [`TICK_INTERVAL`] (30 seconds), and stops when
//! the core locks (nothing opens a document while locked, so nothing fires). A reminder is no
//! more than the `Event::ReminderDue` the tick emits: the event reaches the webview like every
//! event (the UI shows its toast), and a Rust listener registered here hands it to the
//! [`Notifier`], which shows an OS notification.
//!
//! **Trash expiry rides on the same thread** (ADR-0040): about once an hour, while the core is ready,
//! the loop also calls `kb_core::Core::trash_expire`, which purges the pages that have been in the
//! trash for longer than the device setting allows. The core runs the same step at every open and
//! unlock, which is when this thread starts, so the first run here is an hour after that. An
//! [`HourlyGate`] decides when; there is no other timer and no other thread. The events of the
//! purges are forwarded like any other, so an open Trash panel and the tree refresh.
//!
//! **Comments and mentions ride on the same path** (M13 slice 2, ADR-0048): [`notifications_starter`] hears the
//! `Event::NotificationRaised` the core emits when a change of another person concerns this account (a reply in a
//! thread it wrote in, a mention of it in a comment or a page, being added to a Person property) and hands it to a
//! [`NotificationNotifier`], which shows an OS notification: the page's title, and a phrase with the actor's name in the
//! words the UI sent ([`notice_of_notification`]). The row holds no comment text and the notification shows none; the
//! event reaches the webview as well (its toast and its inbox).
//!
//! **What leaves the process.** Only the local OS notification centre, through
//! `tauri-plugin-notification`, called from this Rust code: the title is the page's title and
//! the body the text of the block trimmed to [`BODY_CHARS`] characters (for a comment or a mention, a phrase that
//! names the actor). The webview is granted no `notification:*` permission, so the page cannot show one itself.
//! Nothing here logs a title, a text, a name or a date: counts and error kinds only.
//!
//! **No click handling.** On desktop the plugin shows a notification and hands back no hook for
//! its click (the callbacks exist on mobile only), so a click does what the OS does by default
//! and the window is not brought to the front. On Windows an app run from the build directory
//! shows the notification under PowerShell's name and icon (the plugin sets the app id only for
//! an installed app).

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use kb_types::{ApiError, Event, NotificationInfo, NotificationKind, ReminderDue};
use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::shell_labels::{Labels, fill};
use crate::state::{AppState, BackgroundStarter, BackgroundTask, CoreApi};

/// How often the scheduler of the core is ticked while the app is ready.
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// How often the same loop checks the trash for expired pages (ADR-0040): at most once an hour.
pub const TRASH_EXPIRY_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The longest body of a notification, in characters (the ellipsis of a cut one included).
pub const BODY_CHARS: usize = 120;

/// The core's reminder scheduler, as the shell calls it under its lock. A separate seam from
/// [`CoreApi`], like the sync methods, so the fakes that have no reminders need not implement it.
pub trait RemindersCoreApi {
    /// One tick of the scheduler: emits an `Event::ReminderDue` for every reminder that fell due
    /// since the last tick, and returns how many.
    fn reminders_tick(&mut self) -> Result<usize, ApiError>;

    /// Purges the trashed pages that are past the trash limit (ADR-0040), and returns how many
    /// top-level pages it purged. The events are those of a purge.
    fn trash_expire(&mut self) -> Result<usize, ApiError>;
}

impl RemindersCoreApi for kb_core::Core {
    fn reminders_tick(&mut self) -> Result<usize, ApiError> {
        kb_core::Core::reminders_tick(self)
    }

    fn trash_expire(&mut self) -> Result<usize, ApiError> {
        kb_core::Core::trash_expire(self)
    }
}

impl<C: CoreApi + RemindersCoreApi> AppState<C> {
    /// Ticks the core's reminder scheduler under the single-writer lock, and forwards the events
    /// it emitted (the `ReminderDue`s) like any other call does. A core that is not ready answers
    /// `Locked` or `Internal` without being touched.
    ///
    /// # Errors
    ///
    /// Whatever the core answers, or why it is not ready.
    pub fn reminders_tick(&self) -> Result<usize, ApiError> {
        self.with_core(
            |core| core.reminders_tick(),
            // Counts only: what a reminder says is the user's content.
            |result, events| {
                tracing::debug!(
                    ok = result.is_ok(),
                    fired = result.as_ref().map_or(0, |fired| *fired),
                    events,
                    "reminders tick"
                );
            },
        )
    }

    /// Expires the trash of the core under the single-writer lock (see
    /// [`RemindersCoreApi::trash_expire`]) and forwards the events of the purges like any other
    /// call does. A core that is not ready (a locked one included) answers `Locked` or `Internal`
    /// without being touched, so nothing expires while the workspace is locked.
    ///
    /// # Errors
    ///
    /// Whatever the core answers, or why it is not ready.
    pub fn trash_expire(&self) -> Result<usize, ApiError> {
        self.with_core(
            |core| core.trash_expire(),
            // Counts only: the pages' titles are the user's content.
            |result, events| {
                tracing::debug!(
                    ok = result.is_ok(),
                    pages = result.as_ref().map_or(0, |pages| *pages),
                    events,
                    "trash expiry"
                );
            },
        )
    }
}

/// Opens at most once per interval: the loop asks it on every tick and it says yes when the
/// interval has passed since it last said yes (or since it was made). It keeps no clock of its own
/// (the `Instant`s come in), so a test can drive it.
#[derive(Debug, Clone, Copy)]
pub struct HourlyGate {
    every: Duration,
    last: Instant,
}

impl HourlyGate {
    /// A gate that first opens `every` after `now`.
    #[must_use]
    pub fn starting(now: Instant, every: Duration) -> Self {
        Self { every, last: now }
    }

    /// Whether the gate opens at `now`; when it does, the next opening is `every` after `now`.
    pub fn opens(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.last) >= self.every {
            self.last = now;
            true
        } else {
            false
        }
    }
}

/// What an OS notification says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The page's title (what the UI calls an untitled page, "Untitled" in English, for a page with none).
    pub title: String,
    /// The block's text, at most [`BODY_CHARS`] characters; the mention's date and time when the
    /// block has no text.
    pub body: String,
}

/// The notification of a reminder: the page's title (`untitled` for a page with none, in the language of the UI,
/// `shell_labels`), and the block's text trimmed to [`BODY_CHARS`] characters (cut on a character, with an
/// ellipsis that counts among them).
#[must_use]
pub fn notice_of(reminder: &ReminderDue, untitled: &str) -> Notice {
    let title = reminder.page_title.trim();
    let text = reminder.text.trim();
    let body = if text.is_empty() {
        match &reminder.time {
            Some(time) => format!("{} {time}", reminder.date),
            None => reminder.date.clone(),
        }
    } else {
        shorten(text, BODY_CHARS)
    };
    Notice {
        title: if title.is_empty() { untitled } else { title }.to_owned(),
        body,
    }
}

/// `text` as it is when it has at most `max` characters, else its first `max - 1` characters and
/// an ellipsis (`max` characters in all).
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    // A cut that ends in a space reads better without it.
    cut.truncate(cut.trim_end().len());
    cut.push('\u{2026}');
    cut
}

/// Shows the notification of a reminder.
pub type Notifier = Arc<dyn Fn(&ReminderDue) + Send + Sync>;

/// A [`Notifier`] that shows each reminder in the OS notification centre through
/// `tauri-plugin-notification`, from this Rust code (the webview has no permission to). A
/// notification that cannot be shown is logged by its error and nothing else.
pub fn os_notifier<R: Runtime>(app: AppHandle<R>) -> Notifier {
    Arc::new(move |reminder| {
        let notice = notice_of(reminder, &crate::dialogs::labels_of(&app).dialog_untitled);
        if let Err(error) = app
            .notification()
            .builder()
            .title(notice.title)
            .body(notice.body)
            .show()
        {
            tracing::warn!(%error, "the OS could not show a reminder notification");
        }
    })
}

/// Shows the OS notification of a row of the inbox (a comment, a mention, an assignment).
pub type NotificationNotifier = Arc<dyn Fn(&NotificationInfo) + Send + Sync>;

/// The notification of a row of the inbox (ADR-0048): the page's title as its title (`untitled` of the labels for a page
/// with none, or that is gone), and as its body the phrase of the row's kind with the actor's name, in the words of the UI
/// (`labels`, `dialogNotify*`: `{actor}` and `{page}`), trimmed to [`BODY_CHARS`] characters. **Never the words of a comment**: the row holds none.
#[must_use]
pub fn notice_of_notification(info: &NotificationInfo, labels: &Labels) -> Notice {
    let template = match info.kind {
        NotificationKind::ThreadReply => &labels.dialog_notify_thread_reply,
        NotificationKind::CommentMention => &labels.dialog_notify_comment_mention,
        NotificationKind::PageMention => &labels.dialog_notify_page_mention,
        NotificationKind::PersonAssigned => &labels.dialog_notify_person_assigned,
    };
    let title = info.page_title.as_deref().map_or("", str::trim);
    let title = if title.is_empty() {
        labels.dialog_untitled.as_str()
    } else {
        title
    };
    Notice {
        title: title.to_owned(),
        body: shorten(
            &fill(
                template,
                &[("actor", info.actor_name.trim()), ("page", title)],
            ),
            BODY_CHARS,
        ),
    }
}

/// A [`NotificationNotifier`] that shows each row in the OS notification centre through `tauri-plugin-notification`,
/// from this Rust code. A notification that cannot be shown is logged by its error and nothing else.
pub fn os_notification_notifier<R: Runtime>(app: AppHandle<R>) -> NotificationNotifier {
    Arc::new(move |info| {
        let notice = notice_of_notification(info, &crate::dialogs::labels_of(&app));
        if let Err(error) = app
            .notification()
            .builder()
            .title(notice.title)
            .body(notice.body)
            .show()
        {
            tracing::warn!(%error, "the OS could not show a notification");
        }
    })
}

/// What stops the ticking thread: a flag and the condition the thread sleeps on. (The scheduled
/// backups' timer, `backup_scheduler`, stops the same way.)
#[derive(Default)]
pub(crate) struct Stop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Stop {
    /// Wakes the thread and tells it to end.
    pub(crate) fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.wake.notify_all();
    }

    /// Sleeps for `interval` or until stopped; `true` when stopped.
    pub(crate) fn sleep(&self, interval: Duration) -> bool {
        let stopped = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (stopped, _) = self
            .wake
            .wait_timeout_while(stopped, interval, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        *stopped
    }
}

/// The running scheduler: dropping it stops the thread (which finishes its sleep or its tick on
/// its own: dropping never waits) and takes its listener off the state.
struct Ticker {
    stop: Arc<Stop>,
    unlisten: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(unlisten) = self.unlisten.take() {
            unlisten();
        }
    }
}

/// The background work `lib.rs` registers next to the sync driver: ticks the core's scheduler
/// every `interval` (at once the first time) while the core is ready and shows each reminder it
/// emits with `notify`. It holds the state only weakly: when the state is gone the thread ends.
pub fn reminders_starter<C: CoreApi + RemindersCoreApi>(
    state: Weak<AppState<C>>,
    notify: Notifier,
    interval: Duration,
) -> BackgroundStarter {
    reminders_starter_with(state, notify, interval, TRASH_EXPIRY_INTERVAL)
}

/// [`reminders_starter`] with the interval of the trash expiry given (the tests use a short one).
pub fn reminders_starter_with<C: CoreApi + RemindersCoreApi>(
    state: Weak<AppState<C>>,
    notify: Notifier,
    interval: Duration,
    expiry_interval: Duration,
) -> BackgroundStarter {
    Box::new(move || {
        let host = state.upgrade()?;
        let notify = Arc::clone(&notify);
        let (sender, receiver) = mpsc::channel::<ReminderDue>();
        // Called under the core's lock: it only hands the reminder over to the thread.
        let unlisten = host.add_listener(Arc::new(move |event: &Event| {
            if let Event::ReminderDue(reminder) = event {
                // The thread is gone when the receiver is: nothing to show then.
                let _ = sender.send(reminder.clone());
            }
        }));
        drop(host);
        let stop = Arc::new(Stop::default());
        let thread_stop = Arc::clone(&stop);
        let thread_state = state.clone();
        let spawned = std::thread::Builder::new()
            .name("reminders".to_owned())
            .spawn(move || {
                run(
                    &thread_state,
                    &receiver,
                    &notify,
                    &thread_stop,
                    interval,
                    expiry_interval,
                );
            });
        match spawned {
            Ok(_) => {
                let task: BackgroundTask = Box::new(Ticker {
                    stop,
                    unlisten: Some(unlisten),
                });
                Some(task)
            }
            Err(error) => {
                tracing::error!(%error, "the reminders thread could not be started");
                unlisten();
                None
            }
        }
    })
}

/// The running listener of [`notifications_starter`]: dropping it takes the listener off the state, which ends the thread
/// (its channel closes) once it has shown what it already holds.
struct Listening {
    unlisten: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        if let Some(unlisten) = self.unlisten.take() {
            unlisten();
        }
    }
}

/// The background work `lib.rs` registers next to the reminders' timer: shows an OS notification with `notify` for each
/// `Event::NotificationRaised` the core emits while it is ready (a reply, a mention, an assignment from another person's
/// change, ADR-0048). The listener runs under the core's lock and only hands the row over to a thread of its own, so
/// showing a notification never holds the core; the thread ends when the core locks (the listener goes) or the state is
/// gone. Nothing it holds is logged.
pub fn notifications_starter<C: CoreApi>(
    state: Weak<AppState<C>>,
    notify: NotificationNotifier,
) -> BackgroundStarter {
    Box::new(move || {
        let host = state.upgrade()?;
        let notify = Arc::clone(&notify);
        let (sender, receiver) = mpsc::channel::<NotificationInfo>();
        let unlisten = host.add_listener(Arc::new(move |event: &Event| {
            if let Event::NotificationRaised(info) = event {
                // The thread is gone when the receiver is: nothing to show then.
                let _ = sender.send(info.clone());
            }
        }));
        drop(host);
        let spawned = std::thread::Builder::new()
            .name("notifications".to_owned())
            .spawn(move || {
                // Ends when the listener (and the sender in it) is gone.
                for info in receiver {
                    notify(&info);
                }
            });
        match spawned {
            Ok(_) => {
                let task: BackgroundTask = Box::new(Listening {
                    unlisten: Some(unlisten),
                });
                Some(task)
            }
            Err(error) => {
                tracing::error!(%error, "the notifications thread could not be started");
                unlisten();
                None
            }
        }
    })
}

/// The thread: tick, expire the trash when the hourly gate opens, show what the tick emitted,
/// sleep; until stopped or the state is gone.
fn run<C: CoreApi + RemindersCoreApi>(
    state: &Weak<AppState<C>>,
    receiver: &Receiver<ReminderDue>,
    notify: &Notifier,
    stop: &Stop,
    interval: Duration,
    expiry_interval: Duration,
) {
    // The core has just expired the trash itself: this thread starts once it is open or unlocked.
    let mut expiry_gate = HourlyGate::starting(Instant::now(), expiry_interval);
    loop {
        let Some(host) = state.upgrade() else {
            return;
        };
        if let Err(error) = host.reminders_tick() {
            // The error code only: the message is the core's, and it never has content, but the
            // code is all anyone needs (a locked core stops this thread soon after).
            tracing::debug!(code = ?error.code, "the reminders tick did not run");
        }
        if expiry_gate.opens(Instant::now())
            && let Err(error) = host.trash_expire()
        {
            // Tried again when the gate next opens; the code only, as above.
            tracing::debug!(code = ?error.code, "the trash expiry did not run");
        }
        drop(host);
        // The events of the tick were forwarded under the lock, so they are in the channel now.
        for reminder in receiver.try_iter() {
            notify(&reminder);
        }
        if stop.sleep(interval) {
            return;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use kb_types::{AccountId, BlockId, ErrorCode, PageId, SpaceId};

    use super::*;
    use crate::state::CoreHost;
    use crate::test_support::{SharedSpy, captured_logs, ready_attachment_state};

    fn reminder(title: &str, text: &str, time: Option<&str>) -> ReminderDue {
        ReminderDue {
            page_id: PageId::from_bytes([1; 16]),
            space_id: SpaceId::from_bytes([2; 16]),
            block_id: BlockId::from_bytes([3; 16]),
            node_id: BlockId::from_bytes([4; 16]),
            page_title: title.to_owned(),
            text: text.to_owned(),
            date: "2026-10-07".to_owned(),
            time: time.map(str::to_owned),
            reminder_minutes: 15,
            fire_at: 1_791_382_500,
        }
    }

    /// The notification in English, the language of a shell that has not heard from the UI.
    fn notice(reminder: &ReminderDue) -> Notice {
        notice_of(reminder, "Untitled")
    }

    #[test]
    fn the_notification_is_the_page_title_and_the_block_text() {
        let notice = notice(&reminder(
            "  Plan  ",
            "  Call the dentist 2026-10-07 14:30  ",
            None,
        ));
        assert_eq!(notice.title, "Plan");
        assert_eq!(notice.body, "Call the dentist 2026-10-07 14:30");
    }

    #[test]
    fn an_untitled_page_says_what_the_ui_calls_one() {
        let blank = reminder("", "x", None);
        assert_eq!(notice_of(&blank, "Sans titre").title, "Sans titre");
        assert_eq!(notice_of(&blank, "Untitled").title, "Untitled");
        // A page with a title is not renamed.
        let named = reminder("Plan", "x", None);
        assert_eq!(notice_of(&named, "Sans titre").title, "Plan");
    }

    #[test]
    fn an_untitled_page_says_untitled_and_a_block_without_text_says_the_date() {
        let untitled = notice(&reminder("", "x", None));
        assert_eq!(untitled.title, "Untitled");
        assert_eq!(notice(&reminder("   ", "x", None)).title, "Untitled");

        let empty = notice(&reminder("Plan", "   ", Some("14:30")));
        assert_eq!(empty.body, "2026-10-07 14:30");
        assert_eq!(notice(&reminder("Plan", "", None)).body, "2026-10-07");
    }

    #[test]
    fn the_body_is_trimmed_to_120_characters_with_the_ellipsis_among_them() {
        let exactly = "a".repeat(BODY_CHARS);
        assert_eq!(notice(&reminder("P", &exactly, None)).body, exactly);

        let long = "b".repeat(BODY_CHARS + 1);
        let body = notice(&reminder("P", &long, None)).body;
        assert_eq!(body.chars().count(), BODY_CHARS);
        assert!(body.ends_with('\u{2026}'));
        assert!(body.starts_with(&"b".repeat(BODY_CHARS - 1)));

        // Characters, not bytes: a text of 3-byte characters is cut on one, never inside it.
        let wide = "日".repeat(300);
        let body = notice(&reminder("P", &wide, None)).body;
        assert_eq!(body.chars().count(), BODY_CHARS);
        assert!(body.starts_with("日日日"));

        // A cut that falls after a space does not leave the space before the ellipsis.
        let spaced = format!("{} {}", "c".repeat(BODY_CHARS - 2), "d".repeat(50));
        let body = notice(&reminder("P", &spaced, None)).body;
        assert_eq!(body, format!("{}\u{2026}", "c".repeat(BODY_CHARS - 2)));
    }

    #[test]
    fn the_title_is_not_cut() {
        let title = "T".repeat(500);
        assert_eq!(notice(&reminder(&title, "x", None)).title, title);
    }

    /// Waits until `check` holds, for at most five seconds.
    fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
        let started = Instant::now();
        while !check() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ticks(spy: &SharedSpy) -> usize {
        spy.lock().unwrap_or_else(PoisonError::into_inner).ticks
    }

    fn queue(spy: &SharedSpy, reminders: Vec<ReminderDue>) {
        spy.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .due
            .extend(reminders);
    }

    /// A notifier that records what it was given.
    fn recording() -> (Notifier, Arc<Mutex<Vec<ReminderDue>>>) {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&shown);
        let notifier: Notifier = Arc::new(move |reminder| {
            log.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(reminder.clone());
        });
        (notifier, shown)
    }

    #[test]
    fn it_ticks_at_once_and_then_on_its_interval_and_shows_each_reminder_once() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let (notifier, shown) = recording();
        queue(&spy, vec![reminder("Plan", "Call", Some("14:30"))]);
        state.set_background(reminders_starter(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(10),
        ));
        assert!(state.background_running());

        wait_for("the first tick to show the reminder", || {
            !shown.lock().unwrap().is_empty()
        });
        wait_for("more ticks", || ticks(&spy) >= 4);
        // The fake core emitted one reminder, once: it is shown once, however often it ticks.
        assert_eq!(shown.lock().unwrap().len(), 1);
        assert_eq!(shown.lock().unwrap()[0].page_title, "Plan");

        // A later reminder is shown at a later tick.
        queue(&spy, vec![reminder("Standup", "Daily", None)]);
        wait_for("the second reminder", || shown.lock().unwrap().len() == 2);
        assert_eq!(shown.lock().unwrap()[1].page_title, "Standup");
    }

    #[test]
    fn the_events_reach_the_other_listeners_too_so_the_webview_shows_its_toast() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let _unlisten = state.add_listener(Arc::new(move |event: &Event| {
            if matches!(event, Event::ReminderDue(_)) {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }));
        queue(&spy, vec![reminder("Plan", "Call", None)]);
        let (notifier, shown) = recording();
        state.set_background(reminders_starter(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(10),
        ));
        wait_for("the notification", || !shown.lock().unwrap().is_empty());
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn locking_stops_the_thread_and_takes_its_listener_off() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let (notifier, shown) = recording();
        let listeners = state.listener_count();
        state.set_background(reminders_starter(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(10),
        ));
        assert_eq!(state.listener_count(), listeners + 1);
        wait_for("a tick", || ticks(&spy) >= 2);

        state.lock().unwrap();
        assert!(!state.background_running());
        assert_eq!(
            state.listener_count(),
            listeners,
            "the listener went with it"
        );
        // The thread ends its current sleep or tick and does not start another.
        std::thread::sleep(Duration::from_millis(60));
        let after = ticks(&spy);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(ticks(&spy), after, "nothing ticks while the core is locked");
        assert!(shown.lock().unwrap().is_empty());
    }

    #[test]
    fn a_core_that_cannot_tick_is_left_alone_and_nothing_is_shown() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        spy.lock().unwrap().fail_tick = Some(ApiError::storage("the database failed"));
        queue(&spy, vec![reminder("Plan", "Call", None)]);
        let (notifier, shown) = recording();
        state.set_background(reminders_starter(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(10),
        ));
        wait_for("a few failed ticks", || ticks(&spy) >= 3);
        assert!(shown.lock().unwrap().is_empty());
        // It keeps trying: when the core recovers the reminder comes.
        spy.lock().unwrap().fail_tick = None;
        wait_for("the reminder after the recovery", || {
            !shown.lock().unwrap().is_empty()
        });
    }

    #[test]
    fn the_thread_ends_when_the_state_is_gone() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let (notifier, _) = recording();
        let starter =
            reminders_starter(Arc::downgrade(&state), notifier, Duration::from_millis(10));
        let task = starter();
        assert!(task.is_some());
        wait_for("a tick", || ticks(&spy) >= 1);
        // The task holds nothing of the state: dropping the state ends the thread.
        drop(state);
        std::thread::sleep(Duration::from_millis(60));
        let after = ticks(&spy);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(ticks(&spy), after);
        drop(task);
    }

    #[test]
    fn a_state_that_is_gone_starts_nothing() {
        let state = Arc::new(ready_attachment_state().0);
        let (notifier, _) = recording();
        let starter =
            reminders_starter(Arc::downgrade(&state), notifier, Duration::from_millis(10));
        drop(state);
        assert!(starter().is_none());
    }

    #[test]
    fn the_tick_of_a_state_that_is_not_ready_is_refused_without_touching_the_core() {
        let state: AppState<crate::test_support::AttachCore> = AppState::new();
        let error = state.reminders_tick().unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        let (ready, spy) = ready_attachment_state();
        assert_eq!(ready.reminders_tick().unwrap(), 0);
        assert_eq!(ticks(&spy), 1);
        ready.lock().unwrap();
        assert_eq!(ready.reminders_tick().unwrap_err().code, ErrorCode::Locked);
        assert_eq!(ticks(&spy), 1, "a locked core is not ticked");
    }

    #[test]
    fn nothing_a_reminder_says_is_logged() {
        let (state, spy) = ready_attachment_state();
        // A date that is not today, so that the timestamp of a log line cannot be mistaken for it.
        let secret = ReminderDue {
            date: "2031-02-03".to_owned(),
            ..reminder("Secret title", "Secret text 2031-02-03", Some("04:05"))
        };
        queue(&spy, vec![secret]);
        let logs = captured_logs(|| {
            state.reminders_tick().unwrap();
        });
        assert!(logs.contains("reminders tick"), "{logs}");
        for hidden in ["Secret", "title", "2031-02-03"] {
            assert!(!logs.contains(hidden), "{hidden} in {logs}");
        }
    }

    // --- the trash expiry that rides on the loop (ADR-0040) ---

    fn expiries(spy: &SharedSpy) -> usize {
        spy.lock().unwrap_or_else(PoisonError::into_inner).expiries
    }

    #[test]
    fn the_gate_opens_once_per_interval_and_not_at_the_start() {
        let start = Instant::now();
        let hour = Duration::from_secs(3600);
        let mut gate = HourlyGate::starting(start, hour);
        assert!(
            !gate.opens(start),
            "the core has just expired the trash itself"
        );
        assert!(!gate.opens(start + Duration::from_secs(30)));
        assert!(!gate.opens(start + hour - Duration::from_millis(1)));
        assert!(gate.opens(start + hour));
        // The next opening is an hour after this one, not after the start.
        assert!(!gate.opens(start + hour + Duration::from_secs(30)));
        assert!(!gate.opens(start + 2 * hour - Duration::from_millis(1)));
        assert!(gate.opens(start + 2 * hour));
        // A late tick (the machine slept) opens it once, not once per hour it missed.
        assert!(gate.opens(start + 10 * hour));
        assert!(!gate.opens(start + 10 * hour + Duration::from_secs(30)));
        // An instant before the last opening is no reason to open.
        assert!(!gate.opens(start));
    }

    #[test]
    fn the_trash_is_not_expired_at_every_tick_and_not_at_the_start() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let (notifier, _) = recording();
        state.set_background(reminders_starter_with(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(5),
            TRASH_EXPIRY_INTERVAL,
        ));
        wait_for("several ticks", || ticks(&spy) >= 6);
        assert_eq!(expiries(&spy), 0, "an hour has not passed");
    }

    #[test]
    fn the_hourly_expiry_runs_every_interval_among_the_ticks_and_its_events_are_forwarded() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let structure_changes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&structure_changes);
        let _unlisten = state.add_listener(Arc::new(move |event: &Event| {
            if matches!(event, Event::StructureChanged { .. }) {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }));
        spy.lock().unwrap().expiring = 3;
        let (notifier, _) = recording();
        state.set_background(reminders_starter_with(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(5),
            Duration::from_millis(40),
        ));
        wait_for("the first expiry", || expiries(&spy) >= 1);
        wait_for("a second expiry", || expiries(&spy) >= 2);
        assert!(ticks(&spy) > expiries(&spy), "the ticks are more frequent");
        // The purge of the fake core emitted its event once, and the webview's listener heard it.
        assert_eq!(structure_changes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_expiry_neither_stops_the_ticks_nor_the_next_attempt() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        spy.lock().unwrap().fail_expire = Some(ApiError::storage("the database failed"));
        let (notifier, shown) = recording();
        queue(&spy, vec![reminder("Plan", "Call", None)]);
        state.set_background(reminders_starter_with(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(5),
            Duration::from_millis(30),
        ));
        wait_for("two failed expiries", || expiries(&spy) >= 2);
        // The reminder of the same loop was shown all the same.
        wait_for("the reminder", || !shown.lock().unwrap().is_empty());
        let before = ticks(&spy);
        wait_for("more ticks", || ticks(&spy) > before + 2);
        spy.lock().unwrap().fail_expire = None;
        spy.lock().unwrap().expiring = 1;
        let failed = expiries(&spy);
        wait_for("an expiry after the recovery", || expiries(&spy) > failed);
    }

    #[test]
    fn nothing_is_expired_while_the_workspace_is_locked() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let (notifier, _) = recording();
        state.set_background(reminders_starter_with(
            Arc::downgrade(&state),
            notifier,
            Duration::from_millis(5),
            Duration::from_millis(20),
        ));
        wait_for("an expiry", || expiries(&spy) >= 1);

        state.lock().unwrap();
        assert!(!state.background_running());
        // The thread ends its current sleep or call and does not start another.
        std::thread::sleep(Duration::from_millis(80));
        let after = expiries(&spy);
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(expiries(&spy), after, "a locked workspace expires nothing");
    }

    #[test]
    fn the_expiry_of_a_state_that_is_not_ready_is_refused_without_touching_the_core() {
        let state: AppState<crate::test_support::AttachCore> = AppState::new();
        assert_eq!(state.trash_expire().unwrap_err().code, ErrorCode::Internal);
        let (ready, spy) = ready_attachment_state();
        spy.lock().unwrap().expiring = 2;
        assert_eq!(ready.trash_expire().unwrap(), 2);
        assert_eq!(expiries(&spy), 1);
        ready.lock().unwrap();
        assert_eq!(ready.trash_expire().unwrap_err().code, ErrorCode::Locked);
        assert_eq!(expiries(&spy), 1, "a locked core is not asked");
    }

    #[test]
    fn the_expiry_logs_counts_only() {
        let (state, spy) = ready_attachment_state();
        spy.lock().unwrap().expiring = 4;
        let logs = captured_logs(|| {
            state.trash_expire().unwrap();
        });
        assert!(logs.contains("trash expiry"), "{logs}");
        assert!(logs.contains("pages=4"), "{logs}");
    }

    // --- the notifications of comments and mentions (M13 slice 2, ADR-0048) ---

    fn row(kind: NotificationKind, actor: &str, title: Option<&str>) -> NotificationInfo {
        NotificationInfo {
            id: 1,
            kind,
            space_id: SpaceId::from_bytes([2; 16]),
            page_id: Some(PageId::from_bytes([1; 16])),
            block_id: None,
            comment_id: None,
            actor: AccountId::from_bytes([5; 16]),
            actor_name: actor.to_owned(),
            page_title: title.map(str::to_owned),
            at: 1_791_382_500_000,
            read: false,
        }
    }

    #[test]
    fn a_notification_is_the_page_title_and_a_phrase_with_the_actors_name() {
        let labels = Labels::default();
        let cases = [
            (NotificationKind::ThreadReply, "Bob replied to your thread"),
            (
                NotificationKind::CommentMention,
                "Bob mentioned you in a comment",
            ),
            (NotificationKind::PageMention, "Bob mentioned you"),
            (NotificationKind::PersonAssigned, "Bob assigned you"),
        ];
        for (kind, phrase) in cases {
            let notice = notice_of_notification(&row(kind, "  Bob ", Some("  Plan ")), &labels);
            assert_eq!(notice.title, "Plan");
            assert_eq!(notice.body, phrase);
        }
    }

    #[test]
    fn a_notification_speaks_the_language_of_the_ui_and_names_an_untitled_page_as_it_does() {
        let french = Labels {
            dialog_notify_thread_reply: "{actor} a r\u{e9}pondu \u{e0} votre fil sur {page}"
                .to_owned(),
            dialog_untitled: "Sans titre".to_owned(),
            ..Labels::default()
        };
        let notice = notice_of_notification(
            &row(NotificationKind::ThreadReply, "Ana\u{ef}s", None),
            &french,
        );
        assert_eq!(notice.title, "Sans titre");
        // `{page}` is the title the notification shows: the label of an untitled page when there is none.
        assert_eq!(
            notice.body,
            "Ana\u{ef}s a r\u{e9}pondu \u{e0} votre fil sur Sans titre"
        );
        // A blank title is no title either.
        let blank = notice_of_notification(
            &row(NotificationKind::PageMention, "Bob", Some("   ")),
            &french,
        );
        assert_eq!(blank.title, "Sans titre");
    }

    #[test]
    fn a_notification_body_is_cut_to_120_characters_and_shows_no_comment() {
        let long_name = "N".repeat(300);
        let notice = notice_of_notification(
            &row(NotificationKind::CommentMention, &long_name, Some("Plan")),
            &Labels::default(),
        );
        assert_eq!(notice.body.chars().count(), BODY_CHARS);
        assert!(notice.body.ends_with('\u{2026}'));
        // The row has no field for the words of a comment, and the notice has nothing else to show.
        let info = row(NotificationKind::ThreadReply, "Bob", Some("Plan"));
        let rendered = format!("{:?}", notice_of_notification(&info, &Labels::default()));
        assert_eq!(rendered.matches("Bob").count(), 1, "{rendered}");
    }

    /// A notifier that records what it was given.
    fn recording_notifications() -> (NotificationNotifier, Arc<Mutex<Vec<NotificationInfo>>>) {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&shown);
        let notifier: NotificationNotifier = Arc::new(move |info| {
            log.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(info.clone());
        });
        (notifier, shown)
    }

    fn raise(spy: &SharedSpy, rows: Vec<NotificationInfo>) {
        spy.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .raised
            .extend(rows);
    }

    #[test]
    fn a_raised_notification_is_shown_once_and_still_reaches_the_other_listeners() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let webview = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&webview);
        let _unlisten = state.add_listener(Arc::new(move |event: &Event| {
            if matches!(event, Event::NotificationRaised(_)) {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let (notifier, shown) = recording_notifications();
        state.set_background(notifications_starter(Arc::downgrade(&state), notifier));
        assert!(state.background_running());

        raise(
            &spy,
            vec![row(NotificationKind::ThreadReply, "Bob", Some("Plan"))],
        );
        // The fake core raises it with its next tick; the event is forwarded under the lock.
        state.reminders_tick().unwrap();
        wait_for("the notification", || !shown.lock().unwrap().is_empty());
        assert_eq!(shown.lock().unwrap().len(), 1);
        assert_eq!(shown.lock().unwrap()[0].actor_name, "Bob");
        assert_eq!(webview.load(Ordering::SeqCst), 1, "the toast of the page");

        // Another one is shown at a later time; a reminder is not a notification.
        raise(
            &spy,
            vec![row(NotificationKind::PersonAssigned, "Carol", None)],
        );
        queue(&spy, vec![reminder("Plan", "Call", None)]);
        state.reminders_tick().unwrap();
        wait_for("the second notification", || {
            shown.lock().unwrap().len() == 2
        });
        assert_eq!(
            shown.lock().unwrap()[1].kind,
            NotificationKind::PersonAssigned
        );
    }

    #[test]
    fn locking_takes_the_notifications_listener_off_and_nothing_is_shown_after() {
        let (state, spy) = ready_attachment_state();
        let state = Arc::new(state);
        let (notifier, shown) = recording_notifications();
        let listeners = state.listener_count();
        state.set_background(notifications_starter(Arc::downgrade(&state), notifier));
        assert_eq!(state.listener_count(), listeners + 1);

        state.lock().unwrap();
        assert!(!state.background_running());
        assert_eq!(
            state.listener_count(),
            listeners,
            "the listener went with it"
        );
        raise(
            &spy,
            vec![row(NotificationKind::PageMention, "Bob", Some("Plan"))],
        );
        std::thread::sleep(Duration::from_millis(60));
        assert!(shown.lock().unwrap().is_empty());
    }

    #[test]
    fn a_state_that_is_gone_starts_no_notifications_thread() {
        let state = Arc::new(ready_attachment_state().0);
        let (notifier, _) = recording_notifications();
        let starter = notifications_starter(Arc::downgrade(&state), notifier);
        drop(state);
        assert!(starter().is_none());
    }

    #[test]
    fn nothing_a_notification_says_is_logged() {
        let (state, spy) = ready_attachment_state();
        let (notifier, shown) = recording_notifications();
        let state = Arc::new(state);
        state.set_background(notifications_starter(Arc::downgrade(&state), notifier));
        raise(
            &spy,
            vec![row(
                NotificationKind::CommentMention,
                "Secret person",
                Some("Secret page"),
            )],
        );
        let logs = captured_logs(|| {
            state.reminders_tick().unwrap();
            wait_for("the notification", || !shown.lock().unwrap().is_empty());
        });
        for hidden in ["Secret", "person", "page title"] {
            assert!(!logs.contains(hidden), "{hidden} in {logs}");
        }
    }
}
