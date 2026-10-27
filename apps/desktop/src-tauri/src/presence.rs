//! Presence in shared spaces, the shell's half (M13 slice 3, ADR-0050, `docs/security/sharing.md` section 9,
//! `docs/development/milestone-13.md` sections 3.1 and 3.2): the two commands `presence_send` and `presence_events`
//! of the IPC contract, which move sealed presence frames between the webview and the realtime socket of
//! `kb-net`. The webview never holds a key: the core seals and opens (`Core::presence_seal`, `Core::presence_open`),
//! the shell only moves the bytes.
//!
//! | command           | arguments                                          | result                                      |
//! |-------------------|----------------------------------------------------|---------------------------------------------|
//! | `presence_send`   | `{ spaceId, pageId?, state, update: number[] }`    | nothing; seals and sends, best effort       |
//! | `presence_events` | `{ onEvent: Channel }`                             | nothing; every accepted frame is then sent as a `PresenceMessage` (`{ info, update: number[] }`) |
//!
//! - **`presence_send`**: `state` is a `PresenceState` (`"Viewing"`, `"Editing"`, `"Idle"`), `pageId` is left out for
//!   "in no page", `update` is the editor's `EphemeralStore` update as an array of byte values (at most
//!   `PRESENCE_UPDATE_MAX_BYTES`: `InvalidInput` for `update`). The shell calls `Core::presence_seal` under its lock and
//!   hands the sealed envelope to `kb_net::RealtimeHandle::send_presence`; a personal space, an archive, a device with
//!   no socket and a full queue send nothing, and that is not an error (`Ok(())`): presence is best effort and nothing
//!   waits for it. **At most `PRESENCE_UPDATES_PER_SECOND` updates a second go through** (a sliding window of one
//!   second over the whole shell: the editor limits itself to the same rate, this is the second line, and the
//!   server's limit the third): the excess is dropped silently, with `Ok(())`, before the core is touched. While the
//!   core is locked or still opening the answer is the usual `Locked` or `Internal`.
//! - **`presence_events`**: registers the channel (once per page load, like `subscribe_events`; the shell forgets it when
//!   the page reloads). The driver hands each frame the server relayed to `SyncHost::presence_received`, which opens
//!   it with `Core::presence_open` and sends an accepted one to the channel; a dropped frame reaches nobody. The
//!   `core_state` of a locked or opening core means no frames. A channel whose page is gone is forgotten at its first
//!   failed send.
//!
//! The commands go through [`SharedState`] (`CoreHost::presence_send`, `CoreHost::presence_subscribe`); the state is
//! [`PresenceShell`] inside `AppState`: the channels, the throttle and the handle onto the sync driver's socket, which
//! the sync driver's starter attaches while the core is ready (`driver_starter`) and which is dead when the driver
//! stops.
//!
//! Nothing here logs an id, a name, an update or an envelope: only counts and whether a frame was handed over.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use kb_net::RealtimeHandle;
use kb_types::sharing::{PRESENCE_UPDATE_MAX_BYTES, PRESENCE_UPDATES_PER_SECOND};
use kb_types::{ApiError, FeedId, PageId, PresenceMessage, PresenceState, SpaceId};
use tauri::ipc::Channel;

use crate::SharedState;

/// `presence_send`: seals this device's presence for the space's feed and hands it to the socket.
pub fn send(
    shared: &SharedState,
    space_id: SpaceId,
    page_id: Option<PageId>,
    state: PresenceState,
    update: Vec<u8>,
) -> Result<(), ApiError> {
    shared.presence_send(space_id, page_id, state, &update)
}

/// `presence_events`: registers the channel the accepted frames are sent to.
pub fn subscribe(shared: &SharedState, on_event: Channel<PresenceMessage>) -> Result<(), ApiError> {
    shared.presence_subscribe(Box::new(move |message| {
        on_event.send(message.clone()).is_ok()
    }));
    Ok(())
}

/// What `presence_send` asks of an update before the core is touched: at most `PRESENCE_UPDATE_MAX_BYTES`.
///
/// # Errors
///
/// `InvalidInput` for `update` when it is longer.
pub fn check_update(update: &[u8]) -> Result<(), ApiError> {
    if update.len() > PRESENCE_UPDATE_MAX_BYTES {
        return Err(ApiError::invalid(
            "update",
            format!("a presence update is at most {PRESENCE_UPDATE_MAX_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Hands a message to one subscriber; `false` when it cannot be reached any more (its page is gone), and the
/// subscriber is dropped.
pub type PresenceSink = Box<dyn Fn(&PresenceMessage) -> bool + Send + Sync>;

/// The window the updates are counted in.
const WINDOW: Duration = Duration::from_secs(1);

/// At most [`PRESENCE_UPDATES_PER_SECOND`] updates in any window of one second (a sliding window: the times of the
/// updates that went through, the oldest forgotten when it is a second old).
#[derive(Debug, Default)]
pub struct Throttle {
    sent: VecDeque<Instant>,
}

impl Throttle {
    /// Whether an update at `now` goes through; when it does it counts. `now` must not go back.
    pub fn allow(&mut self, now: Instant) -> bool {
        while self
            .sent
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= WINDOW)
        {
            self.sent.pop_front();
        }
        let limit = usize::try_from(PRESENCE_UPDATES_PER_SECOND).unwrap_or(usize::MAX);
        if self.sent.len() >= limit {
            return false;
        }
        self.sent.push_back(now);
        true
    }
}

/// What the shell keeps for presence: the channels of the webview, the throttle of its updates and the handle onto
/// the sync driver's socket. `AppState` holds one.
#[derive(Default)]
pub struct PresenceShell {
    sinks: Mutex<Vec<PresenceSink>>,
    throttle: Mutex<Throttle>,
    realtime: Mutex<Option<RealtimeHandle>>,
}

impl PresenceShell {
    /// The sync driver started: its socket is where frames go (a handle that belonged to an earlier driver is
    /// replaced).
    pub fn attach(&self, handle: RealtimeHandle) {
        *self.realtime.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
    }

    /// Registers a channel for the accepted frames.
    pub fn subscribe(&self, sink: PresenceSink) {
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sink);
    }

    /// Forgets the channels (a page that is loading again has lost its callbacks).
    pub fn clear(&self) {
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Whether an update may go through now (see [`Throttle`]).
    pub fn allow(&self) -> bool {
        self.throttle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .allow(Instant::now())
    }

    /// Sends an accepted frame to every channel, forgetting those that cannot be reached; whether any heard it.
    pub fn deliver(&self, message: &PresenceMessage) -> bool {
        let mut delivered = false;
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|sink| {
                let reached = sink(message);
                delivered |= reached;
                reached
            });
        delivered
    }

    /// Hands a sealed envelope to the driver's socket; `false` when there is none or its queue is full.
    pub fn send(&self, feed: FeedId, envelope: Vec<u8>) -> bool {
        let handle = self
            .realtime
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        handle.is_some_and(|handle| handle.send_presence(feed, envelope))
    }

    /// How many channels are registered.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kb_net::RealtimeCommand;
    use kb_sync::v2::PresenceFrame;
    use kb_types::{AccountId, DeviceId, PresenceInfo};
    use tokio::sync::mpsc;

    use super::*;

    fn feed() -> FeedId {
        FeedId::from_bytes([4; 16])
    }

    fn message(update: &[u8]) -> PresenceMessage {
        PresenceMessage {
            info: PresenceInfo {
                account_id: AccountId::from_bytes([1; 16]),
                device_id: DeviceId::from_bytes([2; 16]),
                name: "Ada".to_owned(),
                color: 0,
                page_id: None,
                state: PresenceState::Viewing,
                updated_at: 1,
            },
            update: update.to_vec(),
        }
    }

    #[test]
    fn at_most_ten_updates_go_through_in_any_second() {
        let start = Instant::now();
        let mut throttle = Throttle::default();
        // Ten in a burst, then nothing until the oldest is a second old.
        let allowed = (0..15)
            .filter(|i| throttle.allow(start + Duration::from_millis(*i)))
            .count();
        assert_eq!(allowed, 10);
        assert!(!throttle.allow(start + Duration::from_millis(999)));
        // The first ten were at 0..9 ms: one at 1000 ms (the 0 ms update is a second old), the next at 1001...
        assert!(throttle.allow(start + Duration::from_millis(1000)));
        assert!(throttle.allow(start + Duration::from_millis(1001)));
        assert!(!throttle.allow(start + Duration::from_millis(1001)));
    }

    #[test]
    fn a_steady_rate_of_ten_a_second_is_never_refused_and_eleven_is() {
        let start = Instant::now();
        let mut throttle = Throttle::default();
        for i in 0..100 {
            assert!(
                throttle.allow(start + Duration::from_millis(i * 100)),
                "update {i}"
            );
        }
        // Eleven in a second: the last is refused, whichever way they are spread.
        let mut throttle = Throttle::default();
        let refused = (0..11)
            .filter(|i| !throttle.allow(start + Duration::from_millis(i * 90)))
            .count();
        assert_eq!(refused, 1);
    }

    #[test]
    fn a_refused_update_does_not_count_against_the_ones_after_it() {
        let start = Instant::now();
        let mut throttle = Throttle::default();
        for _ in 0..10 {
            assert!(throttle.allow(start));
        }
        // Hammering while refused changes nothing: a second after the burst there is room again.
        for ms in 1..1000 {
            assert!(!throttle.allow(start + Duration::from_millis(ms)));
        }
        assert!(throttle.allow(start + Duration::from_millis(1000)));
    }

    #[test]
    fn an_update_over_three_kib_is_invalid_input_for_update() {
        assert!(check_update(&vec![0; PRESENCE_UPDATE_MAX_BYTES]).is_ok());
        let error = check_update(&vec![0; PRESENCE_UPDATE_MAX_BYTES + 1]).unwrap_err();
        assert_eq!(error.code, kb_types::ErrorCode::InvalidInput);
        assert_eq!(error.details.as_ref().unwrap()["field"], "update");
    }

    #[test]
    fn a_frame_goes_to_every_channel_and_a_channel_that_cannot_be_reached_is_forgotten() {
        let shell = PresenceShell::default();
        assert!(!shell.deliver(&message(b"nobody")), "nobody to hear it");
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&heard);
        shell.subscribe(Box::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            true
        }));
        shell.subscribe(Box::new(|_| false));
        assert_eq!(shell.subscribers(), 2);
        assert!(shell.deliver(&message(b"a")));
        assert_eq!(shell.subscribers(), 1, "the one that failed is gone");
        assert!(shell.deliver(&message(b"b")));
        assert_eq!(heard.load(Ordering::SeqCst), 2);
        shell.clear();
        assert_eq!(shell.subscribers(), 0);
        assert!(!shell.deliver(&message(b"c")));
    }

    #[test]
    fn a_frame_is_handed_to_the_socket_while_one_is_attached_and_dropped_otherwise() {
        let shell = PresenceShell::default();
        assert!(!shell.send(feed(), vec![1; 10]), "no driver yet");
        let (commands, mut socket) = mpsc::channel(4);
        shell.attach(RealtimeHandle::connected_to(commands));
        assert!(shell.send(feed(), vec![1; 10]));
        assert_eq!(
            socket.try_recv().unwrap(),
            RealtimeCommand::Presence(PresenceFrame {
                feed: feed(),
                envelope: vec![1; 10]
            })
        );
        // A frame over the server's limit is not handed over, and a full queue drops the excess.
        assert!(!shell.send(feed(), vec![0; 5000]));
        let accepted = (0..10).filter(|_| shell.send(feed(), vec![2; 10])).count();
        assert_eq!(accepted, 4);
        // The socket is gone (the driver stopped): nothing is sent, and nothing fails.
        drop(socket);
        assert!(!shell.send(feed(), vec![3; 10]));
    }
}
