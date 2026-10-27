//! Helpers shared by the unit tests of the shell.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use kb_types::attachments::CHUNK_SIZE;
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, BackupSummary, ByteRange, Event, NotificationInfo,
    PageId, ReminderDue, Request, Response, SpaceId,
};

use crate::reminders::RemindersCoreApi;
use crate::state::{AppState, AttachmentSink, BackupSink, BackupSource, CoreApi, LockedApi};

/// A log sink the tests can read back.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Installs, once, a global subscriber that accepts every event and keeps none.
///
/// `tracing` decides whether a call site is enabled the first time any thread hits it, from
/// the default subscriber of that thread, and remembers the answer. Tests run on many
/// threads: a test without a subscriber that reaches a call site first would leave it
/// disabled for good, and a later [`captured_logs`] on another thread would never see its
/// events (an `info!` that is hit by several tests failed one run in eight). With this
/// subscriber in place no call site is ever cached as disabled, and the scoped subscriber of
/// [`captured_logs`] still receives its own thread's events.
fn keep_call_sites_enabled() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        // Another global subscriber (there is none in this test binary) would do as well.
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    });
}

/// Runs `work` with logging down to `DEBUG` captured on this thread and returns everything
/// that was logged, so a test can assert on what the shell writes (and never writes).
pub fn captured_logs(work: impl FnOnce()) -> String {
    captured_logs_through(tracing_subscriber::EnvFilter::new("debug"), work)
}

/// [`captured_logs`] through `filter`: what the app's own filter (`crate::log_filter`) would let
/// through, whatever level the events have.
pub fn captured_logs_through(filter: tracing_subscriber::EnvFilter, work: impl FnOnce()) -> String {
    keep_call_sites_enabled();
    let buffer = LogBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, work);
    let bytes = buffer
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

// --- a fake core with attachments, for the tests of the attachment commands ---

/// An attachment (or an upload in progress) of the fake core.
#[derive(Clone)]
pub struct Stored {
    pub space_id: SpaceId,
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    /// What the info says the size is, when it should differ from the bytes: a core that
    /// lies, or an attachment too large to hold in a test.
    pub claimed_size: Option<u64>,
}

/// What the fake core saw, and what it should do wrong; shared with the test.
#[derive(Default)]
pub struct AttachSpy {
    /// `begin`, `write:<bytes>`, `finish`, `abort`, `get`, `read:full`, `read:<start>-<end>`,
    /// in order.
    pub calls: Vec<String>,
    pub uploads: BTreeMap<AttachmentId, Stored>,
    pub stored: BTreeMap<AttachmentId, Stored>,
    next_id: u8,
    writes: usize,
    /// The 1-based number of the `attachment_write` call that fails with `Storage`.
    pub fail_write_on: Option<usize>,
    pub fail_begin: Option<ApiError>,
    pub fail_finish: Option<ApiError>,
    /// Makes a read fail after its first piece.
    pub fail_read: Option<ApiError>,
    /// How many times the reminder scheduler was ticked.
    pub ticks: usize,
    /// Reminders the next tick emits (as `Event::ReminderDue`), then forgets.
    pub due: Vec<ReminderDue>,
    /// Rows of the inbox the next tick raises (as `Event::NotificationRaised`), then forgets.
    pub raised: Vec<NotificationInfo>,
    /// Makes a tick fail with this error.
    pub fail_tick: Option<ApiError>,
    /// How many times the trash was asked to expire (ADR-0040).
    pub expiries: usize,
    /// Pages the next trash expiry reports purged (with one `StructureChanged` event), then forgets.
    pub expiring: usize,
    /// Makes a trash expiry fail with this error.
    pub fail_expire: Option<ApiError>,
    /// Events the core emitted and the shell has not drained yet.
    events: Vec<Event>,
}

pub type SharedSpy = Arc<Mutex<AttachSpy>>;

/// How many bytes the fake streams to a sink at a time: few, so that a test sees several
/// pieces.
const READ_PIECE: usize = 3;

pub struct AttachCore {
    spy: SharedSpy,
}

pub struct AttachLocked(AttachCore);

impl LockedApi for AttachLocked {
    type Core = AttachCore;

    fn unlock(self, _: &str) -> Result<AttachCore, (Self, ApiError)> {
        Ok(self.0)
    }
}

fn info_of(attachment_id: AttachmentId, stored: &Stored) -> AttachmentInfo {
    let size = stored.claimed_size.unwrap_or(stored.bytes.len() as u64);
    AttachmentInfo {
        attachment_id,
        space_id: stored.space_id,
        name: stored.name.clone(),
        mime: stored.mime.clone(),
        size,
        chunk_count: u32::try_from(size.div_ceil(CHUNK_SIZE as u64).max(1)).unwrap_or(u32::MAX),
        created_at: 1_700_000_000_000,
        references: 0,
        unreferenced_since: None,
        local_chunks: u32::try_from(size.div_ceil(CHUNK_SIZE as u64).max(1)).unwrap_or(u32::MAX),
        availability: kb_types::AttachmentAvailability::Complete,
    }
}

impl AttachCore {
    fn spy(&self) -> MutexGuard<'_, AttachSpy> {
        self.spy.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl RemindersCoreApi for AttachCore {
    fn reminders_tick(&mut self) -> Result<usize, ApiError> {
        let mut spy = self.spy();
        spy.ticks += 1;
        if let Some(error) = spy.fail_tick.clone() {
            return Err(error);
        }
        let due = std::mem::take(&mut spy.due);
        let fired = due.len();
        spy.events.extend(due.into_iter().map(Event::ReminderDue));
        let raised = std::mem::take(&mut spy.raised);
        spy.events
            .extend(raised.into_iter().map(Event::NotificationRaised));
        Ok(fired)
    }

    fn trash_expire(&mut self) -> Result<usize, ApiError> {
        let mut spy = self.spy();
        spy.expiries += 1;
        if let Some(error) = spy.fail_expire.clone() {
            return Err(error);
        }
        let pages = std::mem::take(&mut spy.expiring);
        if pages > 0 {
            spy.events.push(Event::StructureChanged {
                space_id: SpaceId::from_bytes([9; 16]),
            });
        }
        Ok(pages)
    }
}

impl CoreApi for AttachCore {
    type Locked = AttachLocked;

    fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
        let mut spy = self.spy();
        match request {
            Request::AttachmentBegin {
                space_id,
                name,
                mime,
            } => {
                spy.calls.push("begin".into());
                if let Some(error) = spy.fail_begin.clone() {
                    return Err(error);
                }
                spy.next_id += 1;
                let id = AttachmentId::from_bytes([spy.next_id; 16]);
                spy.uploads.insert(
                    id,
                    Stored {
                        space_id,
                        name,
                        mime,
                        bytes: Vec::new(),
                        claimed_size: None,
                    },
                );
                Ok(Response::AttachmentStarted(id))
            }
            Request::AttachmentFinish { attachment_id } => {
                spy.calls.push("finish".into());
                if let Some(error) = spy.fail_finish.clone() {
                    return Err(error);
                }
                let upload = spy
                    .uploads
                    .remove(&attachment_id)
                    .ok_or_else(|| ApiError::not_found("no such upload"))?;
                let info = info_of(attachment_id, &upload);
                spy.stored.insert(attachment_id, upload);
                Ok(Response::Attachment(info))
            }
            Request::AttachmentAbort { attachment_id } => {
                spy.calls.push("abort".into());
                spy.uploads.remove(&attachment_id);
                Ok(Response::Ok)
            }
            Request::AttachmentGet { attachment_id } => {
                spy.calls.push("get".into());
                spy.stored
                    .get(&attachment_id)
                    .map(|stored| Response::Attachment(info_of(attachment_id, stored)))
                    .ok_or_else(|| ApiError::not_found("no such attachment"))
            }
            _ => Ok(Response::Ok),
        }
    }

    fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.spy().events)
    }

    fn page_doc_open(&mut self, _: PageId) -> Result<Vec<u8>, ApiError> {
        Err(ApiError::internal("not used by these tests"))
    }

    fn page_doc_import(&mut self, _: PageId, _: &[u8]) -> Result<(), ApiError> {
        Err(ApiError::internal("not used by these tests"))
    }

    fn page_doc_export(&mut self, _: PageId, _: Option<&[u8]>) -> Result<Vec<u8>, ApiError> {
        Err(ApiError::internal("not used by these tests"))
    }

    fn page_doc_close(&mut self, _: PageId) -> Result<(), ApiError> {
        Err(ApiError::internal("not used by these tests"))
    }

    fn lock(self) -> Result<AttachLocked, (Self, ApiError)> {
        Ok(AttachLocked(self))
    }

    fn backup_export(&mut self, _: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
        Err(ApiError::internal("not used by these tests"))
    }

    fn backup_import(
        &mut self,
        _: BackupSource<'_>,
        _: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        Err(ApiError::internal("not used by these tests"))
    }

    fn attachment_write(
        &mut self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let mut spy = self.spy();
        spy.calls.push(format!("write:{}", bytes.len()));
        spy.writes += 1;
        if spy.fail_write_on == Some(spy.writes) {
            return Err(ApiError::storage("the chunk could not be stored"));
        }
        let upload = spy
            .uploads
            .get_mut(&attachment_id)
            .ok_or_else(|| ApiError::not_found("no such upload"))?;
        upload.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn attachment_read(
        &mut self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        let (stored, fail) = {
            let mut spy = self.spy();
            spy.calls.push(match range {
                None => "read:full".to_owned(),
                Some(range) => format!("read:{}-{}", range.start, range.end),
            });
            let stored = spy
                .stored
                .get(&attachment_id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("no such attachment"))?;
            (stored, spy.fail_read.clone())
        };
        let wanted = match range {
            None => &stored.bytes[..],
            Some(range) => {
                let start = usize::try_from(range.start).unwrap_or(usize::MAX);
                let end = usize::try_from(range.end).unwrap_or(usize::MAX);
                if start >= end || end > stored.bytes.len() {
                    return Err(ApiError::invalid("range", "the range is outside the file"));
                }
                &stored.bytes[start..end]
            }
        };
        for (index, piece) in wanted.chunks(READ_PIECE).enumerate() {
            sink(piece)?;
            if index == 0
                && let Some(error) = fail.clone()
            {
                return Err(error);
            }
        }
        if wanted.is_empty()
            && let Some(error) = fail
        {
            return Err(error);
        }
        Ok(info_of(attachment_id, &stored))
    }
}

/// A ready state over a fresh fake core, and the spy to look at and steer it.
pub fn ready_attachment_state() -> (AppState<AttachCore>, SharedSpy) {
    let spy = SharedSpy::default();
    let state = AppState::new();
    state.set_ready(AttachCore {
        spy: Arc::clone(&spy),
    });
    (state, spy)
}

/// Puts a finished attachment straight into the fake core (no call is recorded).
pub fn seed(spy: &SharedSpy, name: &str, mime: &str, bytes: &[u8]) -> AttachmentId {
    let mut spy = spy.lock().unwrap_or_else(PoisonError::into_inner);
    spy.next_id += 1;
    let id = AttachmentId::from_bytes([spy.next_id; 16]);
    spy.stored.insert(
        id,
        Stored {
            space_id: SpaceId::from_bytes([9; 16]),
            name: name.to_owned(),
            mime: mime.to_owned(),
            bytes: bytes.to_vec(),
            claimed_size: None,
        },
    );
    id
}
