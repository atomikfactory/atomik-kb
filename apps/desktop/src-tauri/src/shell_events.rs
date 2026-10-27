//! What the shell tells the webview on its own account (ADR-0040): the one channel behind the command
//! `shell_events_subscribe`, which carries the deep links of the operating system (`deep_link`), the quick
//! capture (the global shortcut and the tray's "New page"), the tray's "Lock", the state of the capture
//! shortcut and what the native menu bar asks of the UI (`menu_bar`). Each message is
//! `{ "type": ..., "data"?: ... }`:
//!
//! | `type`      | `data`                                | when                                           |
//! |-------------|---------------------------------------|------------------------------------------------|
//! | `deepLink`  | `{ pageId, blockId? }`                | the OS opened an `atomik-kb://page/...` link   |
//! | `capture`   | none                                  | the shortcut or the tray asked for a new page  |
//! | `lock`      | none                                  | the tray asked to lock the app                 |
//! | `shortcut`  | `{ state: "disabled" \| "registered" \| "taken" \| "invalid" }` | the capture shortcut was (re)registered |
//! | `menu`      | `{ id: "import" \| "export" \| "print" \| "undo" \| "redo" \| "find" \| "toggleSidebar" \| "back" \| "forward" \| "about" }` | a menu item the UI runs was chosen |
//! | `menuBar`   | `{ visible: boolean }`                | the menu bar was shown or hidden               |
//! | `inviteLink`| `{ link }`                            | the OS opened an `atomik-kb://invite?...` link |
//!
//! **Delivery.** A message goes to every subscriber that can still be reached (one window; a reloaded page
//! re-subscribes after the old subscribers were cleared), and a subscriber that cannot be reached is
//! forgotten. What happens with nobody to hear it depends on the message:
//!
//! - `deepLink`, `inviteLink` and `capture` are *held* and go to the next subscriber at once, in the order they
//!   came. A newer link replaces an older one of its kind that was held (the last link wins), and a second
//!   capture while one is held is the same capture. The shell does not look at the app lock: the UI keeps them
//!   until a workspace is open (after an unlock when locked), so a locked app opens and creates nothing until
//!   the passphrase has been given.
//! - `lock` is *not* held: [`ShellEvents::lock`] answers whether anyone heard it, and the shell locks the
//!   core itself when nobody did. The UI runs its own lock action (which sends the editor's unsaved text
//!   first), so a lock the UI heard is a lock the UI makes.
//! - `shortcut` is *sticky*: the last state is sent to every new subscriber, whenever it subscribes.
//! - `menu` and `menuBar` are *not* held: a menu item is chosen in a window that is showing a page, and a page
//!   that reloads starts with the bar hidden (the shell hides it at the page load), so a message nobody can hear
//!   is dropped.
//!
//! Nothing here logs a link (an invitation's carries a secret), an id or an accelerator.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::deep_link::{DeepLink, InviteLink};
use crate::menu_bar::UiCommand;
use crate::shortcut::ShortcutState;

/// One message of the channel, as the webview reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum ShellEvent {
    /// A link of the operating system that the shell accepted.
    DeepLink(DeepLink),
    /// Make a page at the top level of the space in use, with its title ready to type.
    Capture,
    /// Lock the app the way the UI's lock action does.
    Lock,
    /// The state of the capture shortcut after it was (re)registered.
    Shortcut {
        /// Registered, off, refused by the system or invalid.
        state: ShortcutState,
    },
    /// A menu item the UI runs was chosen in the native menu bar.
    Menu {
        /// What to run.
        id: UiCommand,
    },
    /// The native menu bar was shown or hidden (by Alt, a choice, a loss of focus or a page load).
    MenuBar {
        /// Whether it is on the window now.
        visible: bool,
    },
    /// An invitation link of the operating system that the shell accepted (M13, ADR-0046); the UI hands it
    /// to the core once a workspace is open.
    InviteLink {
        /// The link as it came (`atomik-kb://invite?server=<url>#<secret>`); never logged.
        link: InviteLink,
    },
}

/// Hands a message to one subscriber; `false` when it cannot be reached any more (its page is gone), and the
/// subscriber is dropped.
pub type Sink = Box<dyn Fn(&ShellEvent) -> bool + Send + Sync>;

#[derive(Default)]
struct Hub {
    /// Links and captures nobody was there to receive, oldest first. Not empty only while there is no sink.
    held: VecDeque<ShellEvent>,
    /// The last state of the capture shortcut.
    shortcut: Option<ShortcutState>,
    sinks: Vec<Sink>,
}

impl Hub {
    /// Sends `event` to every subscriber and forgets those that cannot be reached; whether any heard it.
    fn deliver(&mut self, event: &ShellEvent) -> bool {
        let mut delivered = false;
        self.sinks.retain(|sink| {
            let reached = sink(event);
            delivered |= reached;
            reached
        });
        delivered
    }

    /// Keeps a link or a capture for the next subscriber (the other kinds are not held).
    fn hold(&mut self, event: ShellEvent) {
        match event {
            ShellEvent::DeepLink(_) => {
                self.held
                    .retain(|held| !matches!(held, ShellEvent::DeepLink(_)));
                self.held.push_back(event);
            }
            ShellEvent::InviteLink { .. } => {
                self.held
                    .retain(|held| !matches!(held, ShellEvent::InviteLink { .. }));
                self.held.push_back(event);
            }
            ShellEvent::Capture => {
                if !self.held.contains(&ShellEvent::Capture) {
                    self.held.push_back(event);
                }
            }
            ShellEvent::Lock
            | ShellEvent::Shortcut { .. }
            | ShellEvent::Menu { .. }
            | ShellEvent::MenuBar { .. } => {}
        }
    }
}

/// Where the messages wait for the webview and are sent to it. Managed by Tauri inside an `Arc`.
#[derive(Default)]
pub struct ShellEvents {
    hub: Mutex<Hub>,
}

impl ShellEvents {
    /// No subscriber, nothing held, no shortcut state yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn hub(&self) -> MutexGuard<'_, Hub> {
        self.hub.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A link of the operating system, already accepted by `deep_link::parse`: sent to the subscribers, or
    /// held when none can be reached (the last one wins).
    pub fn deep_link(&self, link: DeepLink) {
        self.send_or_hold(ShellEvent::DeepLink(link));
    }

    /// An invitation link of the operating system, already accepted by `deep_link::parse_invite`: sent to
    /// the subscribers, or held when none can be reached (the last one wins).
    pub fn invite_link(&self, link: InviteLink) {
        self.send_or_hold(ShellEvent::InviteLink { link });
    }

    /// A request for a quick capture: sent to the subscribers, or held when none can be reached.
    pub fn capture(&self) {
        self.send_or_hold(ShellEvent::Capture);
    }

    fn send_or_hold(&self, event: ShellEvent) {
        let mut hub = self.hub();
        if !hub.deliver(&event) {
            hub.hold(event);
        }
    }

    /// A request to lock the app, which is not held. `true` when a subscriber heard it (the UI locks), `false`
    /// when nobody could be reached (the caller locks the core itself).
    pub fn lock(&self) -> bool {
        self.hub().deliver(&ShellEvent::Lock)
    }

    /// A menu item the UI runs was chosen: sent to the subscribers and not held. `true` when one heard it.
    pub fn menu(&self, id: UiCommand) -> bool {
        self.hub().deliver(&ShellEvent::Menu { id })
    }

    /// The menu bar was shown or hidden: sent to the subscribers and not held.
    pub fn menu_bar(&self, visible: bool) {
        self.hub().deliver(&ShellEvent::MenuBar { visible });
    }

    /// The capture shortcut was (re)registered: remembered for the subscribers to come and sent to those there.
    pub fn set_shortcut_state(&self, state: ShortcutState) {
        let mut hub = self.hub();
        hub.shortcut = Some(state);
        hub.deliver(&ShellEvent::Shortcut { state });
    }

    /// The last state of the capture shortcut, if it was ever registered.
    #[must_use]
    pub fn shortcut_state(&self) -> Option<ShortcutState> {
        self.hub().shortcut
    }

    /// Registers a subscriber for the messages from now on, and gives it at once the state of the shortcut and
    /// what was held, in the order it came. A subscriber that cannot be reached is not kept, and what it did
    /// not receive stays held for the next.
    pub fn subscribe(&self, sink: Sink) {
        let mut hub = self.hub();
        if let Some(state) = hub.shortcut
            && !sink(&ShellEvent::Shortcut { state })
        {
            return;
        }
        while let Some(event) = hub.held.pop_front() {
            if !sink(&event) {
                hub.held.push_front(event);
                return;
            }
        }
        hub.sinks.push(sink);
    }

    /// Forgets every subscriber: a page that starts loading has lost the callbacks of its channels.
    pub fn clear_subscribers(&self) {
        self.hub().sinks.clear();
    }

    /// Whether a link or a capture is held for the next subscriber.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.hub().held.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    const PAGE: &str = "12345678-9abc-4def-8123-456789abcdef";
    const BLOCK: &str = "0a1b2c3d-4e5f-4a6b-8c7d-8e9f0a1b2c3d";
    const OTHER: &str = "87654321-cba9-4fed-8321-fedcba987654";

    fn link(page: &str, block: Option<&str>) -> DeepLink {
        DeepLink {
            page_id: page.parse().unwrap(),
            block_id: block.map(str::to_owned),
        }
    }

    fn deep(page: &str, block: Option<&str>) -> ShellEvent {
        ShellEvent::DeepLink(link(page, block))
    }

    fn shortcut(state: ShortcutState) -> ShellEvent {
        ShellEvent::Shortcut { state }
    }

    // ----- the wire --------------------------------------------------------------------------------------------

    #[test]
    fn every_message_serialises_as_the_webview_reads_it() {
        let wire = |event: ShellEvent| serde_json::to_value(event).unwrap();
        assert_eq!(
            wire(deep(PAGE, Some(BLOCK))),
            json!({ "type": "deepLink", "data": { "pageId": PAGE, "blockId": BLOCK } })
        );
        assert_eq!(
            wire(deep(PAGE, None)),
            json!({ "type": "deepLink", "data": { "pageId": PAGE } })
        );
        assert_eq!(wire(ShellEvent::Capture), json!({ "type": "capture" }));
        assert_eq!(wire(ShellEvent::Lock), json!({ "type": "lock" }));
        assert_eq!(
            wire(ShellEvent::Menu {
                id: UiCommand::ToggleSidebar
            }),
            json!({ "type": "menu", "data": { "id": "toggleSidebar" } })
        );
        assert_eq!(
            wire(ShellEvent::MenuBar { visible: true }),
            json!({ "type": "menuBar", "data": { "visible": true } })
        );
        for (state, text) in [
            (ShortcutState::Disabled, "disabled"),
            (ShortcutState::Registered, "registered"),
            (ShortcutState::Taken, "taken"),
            (ShortcutState::Invalid, "invalid"),
        ] {
            assert_eq!(
                wire(shortcut(state)),
                json!({ "type": "shortcut", "data": { "state": text } })
            );
        }
    }

    // ----- delivery --------------------------------------------------------------------------------------------

    /// A subscriber that records what it was sent and can be made unreachable.
    struct Listener {
        seen: Arc<Mutex<Vec<ShellEvent>>>,
        reachable: Arc<Mutex<bool>>,
    }

    impl Listener {
        fn new() -> Self {
            Self {
                seen: Arc::default(),
                reachable: Arc::new(Mutex::new(true)),
            }
        }

        fn sink(&self) -> Sink {
            let seen = Arc::clone(&self.seen);
            let reachable = Arc::clone(&self.reachable);
            Box::new(move |event| {
                if !*reachable.lock().unwrap() {
                    return false;
                }
                seen.lock().unwrap().push(event.clone());
                true
            })
        }

        fn seen(&self) -> Vec<ShellEvent> {
            self.seen.lock().unwrap().clone()
        }

        fn disconnect(&self) {
            *self.reachable.lock().unwrap() = false;
        }
    }

    #[test]
    fn a_message_reaches_a_subscriber_and_is_not_kept() {
        let events = ShellEvents::new();
        let listener = Listener::new();
        events.subscribe(listener.sink());
        events.deep_link(link(PAGE, None));
        events.deep_link(link(OTHER, Some(BLOCK)));
        events.capture();
        assert_eq!(
            listener.seen(),
            vec![
                deep(PAGE, None),
                deep(OTHER, Some(BLOCK)),
                ShellEvent::Capture
            ]
        );
        assert!(!events.has_pending());
    }

    #[test]
    fn a_link_before_anyone_listens_is_held_and_delivered_at_the_subscription() {
        // The cold start: the OS hands the link over while the page is still loading.
        let events = ShellEvents::new();
        events.deep_link(link(PAGE, None));
        assert!(events.has_pending());
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert_eq!(listener.seen(), vec![deep(PAGE, None)]);
        assert!(!events.has_pending());
        // And it is delivered once: a second subscriber gets nothing old.
        let later = Listener::new();
        events.subscribe(later.sink());
        assert!(later.seen().is_empty());
    }

    #[test]
    fn only_the_last_link_is_held() {
        let events = ShellEvents::new();
        events.deep_link(link(PAGE, None));
        events.deep_link(link(OTHER, Some(BLOCK)));
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert_eq!(listener.seen(), vec![deep(OTHER, Some(BLOCK))]);
    }

    #[test]
    fn a_capture_before_anyone_listens_is_held_once() {
        let events = ShellEvents::new();
        events.capture();
        events.capture();
        events.capture();
        assert!(events.has_pending());
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert_eq!(listener.seen(), vec![ShellEvent::Capture]);
        assert!(!events.has_pending());
    }

    #[test]
    fn what_was_held_arrives_in_the_order_it_came() {
        let events = ShellEvents::new();
        events.deep_link(link(PAGE, None));
        events.capture();
        events.deep_link(link(OTHER, None));
        let listener = Listener::new();
        events.subscribe(listener.sink());
        // The newer link replaced the older one and stands where it came.
        assert_eq!(
            listener.seen(),
            vec![ShellEvent::Capture, deep(OTHER, None)]
        );
    }

    #[test]
    fn a_subscriber_that_cannot_be_reached_is_dropped_and_the_message_is_held() {
        let events = ShellEvents::new();
        let gone = Listener::new();
        events.subscribe(gone.sink());
        gone.disconnect();
        events.capture();
        events.deep_link(link(PAGE, None));
        assert!(gone.seen().is_empty());
        assert!(events.has_pending());
        let next = Listener::new();
        events.subscribe(next.sink());
        assert_eq!(next.seen(), vec![ShellEvent::Capture, deep(PAGE, None)]);
    }

    #[test]
    fn a_subscription_that_cannot_be_reached_leaves_what_is_held_for_the_next() {
        let events = ShellEvents::new();
        events.deep_link(link(PAGE, None));
        events.capture();
        let dead = Listener::new();
        dead.disconnect();
        events.subscribe(dead.sink());
        assert!(events.has_pending());
        let next = Listener::new();
        events.subscribe(next.sink());
        assert_eq!(next.seen(), vec![deep(PAGE, None), ShellEvent::Capture]);
        assert!(!events.has_pending());
    }

    #[test]
    fn a_page_that_reloads_loses_its_subscribers_and_messages_wait_for_the_next() {
        let events = ShellEvents::new();
        let before = Listener::new();
        events.subscribe(before.sink());
        events.clear_subscribers();
        events.deep_link(link(PAGE, None));
        events.capture();
        assert!(before.seen().is_empty());
        let after = Listener::new();
        events.subscribe(after.sink());
        assert_eq!(after.seen(), vec![deep(PAGE, None), ShellEvent::Capture]);
    }

    #[test]
    fn every_reachable_subscriber_hears_a_message() {
        let events = ShellEvents::new();
        let (first, second) = (Listener::new(), Listener::new());
        events.subscribe(first.sink());
        events.subscribe(second.sink());
        events.capture();
        assert_eq!(first.seen(), vec![ShellEvent::Capture]);
        assert_eq!(second.seen(), vec![ShellEvent::Capture]);
        // One of them going away does not keep the other from the next one.
        first.disconnect();
        events.deep_link(link(OTHER, None));
        assert_eq!(second.seen().len(), 2);
        assert!(!events.has_pending());
    }

    // ----- invitation links ------------------------------------------------------------------------------------

    const INVITE: &str = "atomik-kb://invite?server=http%3A%2F%2F127.0.0.1%3A8787#8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB";
    const OTHER_INVITE: &str = "atomik-kb://invite?server=https%3A%2F%2Fsync.example.com#0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJK";

    fn invite_link(raw: &str) -> ShellEvent {
        ShellEvent::InviteLink {
            link: crate::deep_link::parse_invite(raw).unwrap(),
        }
    }

    #[test]
    fn an_invitation_serialises_as_the_webview_reads_it() {
        assert_eq!(
            serde_json::to_value(invite_link(INVITE)).unwrap(),
            json!({ "type": "inviteLink", "data": { "link": INVITE } })
        );
    }

    #[test]
    fn an_invitation_never_shows_in_debug_output() {
        let shown = format!("{:?}", invite_link(INVITE));
        assert!(!shown.contains("8H2XG7MQ"), "{shown}");
        assert!(!shown.contains("127.0.0.1"), "{shown}");
    }

    #[test]
    fn an_invitation_reaches_every_subscriber_and_is_not_kept() {
        let events = ShellEvents::new();
        let (first, second) = (Listener::new(), Listener::new());
        events.subscribe(first.sink());
        events.subscribe(second.sink());
        events.invite_link(crate::deep_link::parse_invite(INVITE).unwrap());
        assert_eq!(first.seen(), vec![invite_link(INVITE)]);
        assert_eq!(second.seen(), first.seen());
        assert!(!events.has_pending());
    }

    #[test]
    fn an_invitation_before_anyone_listens_is_held_and_the_last_one_wins() {
        let events = ShellEvents::new();
        events.invite_link(crate::deep_link::parse_invite(INVITE).unwrap());
        events.invite_link(crate::deep_link::parse_invite(OTHER_INVITE).unwrap());
        assert!(events.has_pending());
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert_eq!(listener.seen(), vec![invite_link(OTHER_INVITE)]);
        assert!(!events.has_pending());
        // Delivered once: a second subscriber gets nothing old.
        let later = Listener::new();
        events.subscribe(later.sink());
        assert!(later.seen().is_empty());
    }

    #[test]
    fn an_invitation_and_a_page_link_are_held_apart_in_the_order_they_came() {
        let events = ShellEvents::new();
        events.deep_link(link(PAGE, None));
        events.invite_link(crate::deep_link::parse_invite(INVITE).unwrap());
        events.deep_link(link(OTHER, Some(BLOCK)));
        let listener = Listener::new();
        events.subscribe(listener.sink());
        // The newer page link replaced the older one and stands where it came.
        assert_eq!(
            listener.seen(),
            vec![invite_link(INVITE), deep(OTHER, Some(BLOCK))]
        );
    }

    #[test]
    fn an_invitation_that_could_not_be_delivered_waits_for_the_next_subscriber() {
        let events = ShellEvents::new();
        let gone = Listener::new();
        events.subscribe(gone.sink());
        gone.disconnect();
        events.invite_link(crate::deep_link::parse_invite(INVITE).unwrap());
        assert!(gone.seen().is_empty());
        assert!(events.has_pending());
        let next = Listener::new();
        events.subscribe(next.sink());
        assert_eq!(next.seen(), vec![invite_link(INVITE)]);
    }

    // ----- lock ------------------------------------------------------------------------------------------------

    #[test]
    fn a_lock_request_is_heard_by_the_subscribers_and_never_held() {
        let events = ShellEvents::new();
        // Nobody there: the caller must lock the core itself, and nothing waits for later.
        assert!(!events.lock());
        assert!(!events.has_pending());
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert!(listener.seen().is_empty());

        assert!(events.lock());
        assert_eq!(listener.seen(), vec![ShellEvent::Lock]);

        // A subscriber that went away does not count as having heard it.
        listener.disconnect();
        assert!(!events.lock());
        assert!(!events.has_pending());
    }

    // ----- the menu bar ----------------------------------------------------------------------------------------

    #[test]
    fn a_menu_choice_and_the_bar_s_visibility_reach_the_subscribers_and_are_never_held() {
        let events = ShellEvents::new();
        // Nobody there: nothing is kept for later, and the caller is told.
        assert!(!events.menu(UiCommand::Print));
        events.menu_bar(true);
        assert!(!events.has_pending());
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert!(listener.seen().is_empty());

        assert!(events.menu(UiCommand::Print));
        events.menu_bar(false);
        assert_eq!(
            listener.seen(),
            vec![
                ShellEvent::Menu {
                    id: UiCommand::Print
                },
                ShellEvent::MenuBar { visible: false }
            ]
        );
        assert!(!events.has_pending());

        // A subscriber that went away does not count as having heard it.
        listener.disconnect();
        assert!(!events.menu(UiCommand::About));
        assert!(!events.has_pending());
    }

    #[test]
    fn a_page_that_reloads_hears_nothing_old_of_the_menu() {
        let events = ShellEvents::new();
        events.menu_bar(true);
        events.menu(UiCommand::Find);
        events.clear_subscribers();
        let after = Listener::new();
        events.subscribe(after.sink());
        assert!(after.seen().is_empty());
    }

    // ----- the shortcut's state --------------------------------------------------------------------------------

    #[test]
    fn the_shortcut_state_goes_to_the_subscribers_and_to_every_later_one() {
        let events = ShellEvents::new();
        assert_eq!(events.shortcut_state(), None);
        let first = Listener::new();
        events.subscribe(first.sink());
        // Nothing to say before the shortcut was ever registered.
        assert!(first.seen().is_empty());

        events.set_shortcut_state(ShortcutState::Registered);
        events.set_shortcut_state(ShortcutState::Taken);
        assert_eq!(
            first.seen(),
            vec![
                shortcut(ShortcutState::Registered),
                shortcut(ShortcutState::Taken)
            ]
        );
        assert_eq!(events.shortcut_state(), Some(ShortcutState::Taken));
        // Not pending: it is a state, not something to deliver once.
        assert!(!events.has_pending());

        // A page that loads later (or again) is told the state it missed, every time it subscribes.
        for _ in 0..2 {
            events.clear_subscribers();
            let later = Listener::new();
            events.subscribe(later.sink());
            assert_eq!(later.seen(), vec![shortcut(ShortcutState::Taken)]);
        }
    }

    #[test]
    fn the_shortcut_state_comes_before_what_was_held() {
        let events = ShellEvents::new();
        events.set_shortcut_state(ShortcutState::Registered);
        events.capture();
        let listener = Listener::new();
        events.subscribe(listener.sink());
        assert_eq!(
            listener.seen(),
            vec![shortcut(ShortcutState::Registered), ShellEvent::Capture]
        );
    }

    #[test]
    fn a_subscriber_that_cannot_be_reached_does_not_use_up_the_held_messages() {
        let events = ShellEvents::new();
        events.set_shortcut_state(ShortcutState::Registered);
        events.capture();
        let dead = Listener::new();
        dead.disconnect();
        events.subscribe(dead.sink());
        assert!(events.has_pending());
    }
}
