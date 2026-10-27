//! Sync network client and driver (ADR-0045 protocol v2; ADR-0021 and ADR-0022 for the pieces that
//! carry over, `docs/development/milestone-13.md`). Native only: [`HttpRemote`] speaks to
//! `kb-server` over HTTP and a WebSocket; [`SyncDriver`] runs the cycle (epoch check, log
//! publication, authentication, then the personal feed and every shared feed: membership, push,
//! snapshots, pull, attachment chunk transfers, heads) against a core reached through
//! [`SyncHost`], with debounce, backoff and notifications. The core itself stays sans-IO: every
//! call into it is synchronous and happens under the shell's lock.
//!
//! - [`http`]: the HTTP remote, its status mapping and the notification socket.
//! - [`v2`]: [`SyncRemote`], the `Send` view of a remote the driver runs against (implemented for
//!   [`HttpRemote`] and the reference [`kb_sync::v2::MemoryRemote`]), [`SyncHost`], what the driver
//!   needs from the core, and [`SyncDriver`], the loop.
//! - [`driver`]: what the loops share: the handle, the schedule, the messages of a failed cycle.
//! - [`devices`]: the pairing and recovery tasks (ADR-0022) and what starts them.
//! - [`invite`]: the invite task of shared spaces (`InviteTask`, started by `InviteTasks` on the
//!   core's `InviteRequested`).
//! - [`codec`]: the JSON bodies and their hex and base64 fields.
//! - [`realtime`]: the realtime socket (WebSocket v2: inline entries and presence frames) as the driver
//!   and the shell see it (M13 slice 3).

#![forbid(unsafe_code)]

pub mod codec;
pub mod devices;
pub mod driver;
pub mod http;
pub mod invite;
pub mod realtime;
pub mod v2;

pub use devices::{
    DeviceHost, DeviceTasks, PairingHost, PairingTask, RecoveryHost, RecoveryTask, TaskHandle,
};
pub use driver::{DriverHandle, RemoteFactory};
pub use http::{AuthSigner, HttpRemote, status_error};
pub use realtime::{Realtime, RealtimeCommand, RealtimeEvent, RealtimeHandle};
pub use v2::{RemoteFuture, SyncDriver, SyncHost, SyncRemote};

use kb_sync::SyncError;
use kb_types::ApiError;

/// A recovery blob to upload: its lookup id and its bytes.
pub type RecoveryUpload = ([u8; 32], Vec<u8>);

/// Errors of the driver and the HTTP remote.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error(transparent)]
    Core(#[from] ApiError),
    #[error("network error: {0}")]
    Transport(String),
    /// The server refused this device for a reason only the user can resolve (an idle
    /// device must be paired again); the message says what to do.
    #[error("{0}")]
    Refused(String),
}
