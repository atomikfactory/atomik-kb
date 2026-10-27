//! The shell's side of sync (ADR-0021 section 7 and ADR-0045 section 9, `docs/development/milestone-06.md`
//! and `milestone-13.md` sections 1.5 and 1.6, "Shell"), of device pairing and recovery (ADR-0022,
//! `milestone-07.md`) and of invites (ADR-0046): [`AppState`] is the `kb_net::v2::SyncHost` the driver
//! runs against (one session, every feed: the personal one and one per shared space), the
//! `kb_net::invite::InviteHost` the invite task runs against, and the `PairingHost` and `RecoveryHost`
//! the device tasks run against. [`driver_starter`] is the background work `lib.rs` registers: the v2
//! sync driver, `kb_net::DeviceTasks` (the pairing task on `PairingRequested`, the recovery task on
//! `RecoveryRequested`) and `kb_net::invite::InviteTasks` (the invite task on `InviteRequested`). All of
//! it runs while the core is ready and stops when it locks; a task also ends with its session (done,
//! failed or cancelled).
//!
//! Every host method takes the single-writer lock and makes one call into the core, exactly like
//! `dispatch`: events the core emits (`SyncChanged`, the `Remote` changes of an applied page,
//! `InviteChanged`) go to the webview and to the driver and the tasks before the lock is released, and a
//! locked or starting core answers `Locked` or `Internal` without being touched. The driver calls these
//! from tokio's blocking pool. Nothing here logs a page, an entry, a signature, a link, a code or an
//! error message: method names, outcomes and event counts only.
//!
//! View links (M13 slice 4, ADR-0051) run inside the driver's cycle through `sync_link_work` and
//! `sync_link_done`, under the same lock: a publication is sealed by the core and uploaded by the
//! driver; neither a link id (past its first 8 hex characters), a key nor a URL is logged here.
//!
//! Attachment chunk transfers (ADR-0023, `milestone-08.md`) run inside the driver's cycle through the
//! `sync_feed_chunk_*` host methods, under the same lock; chunk bytes and hashes are never logged.
//!
//! The IPC surface does not change: `SyncConfigure`, `SyncStatus`, `SyncNow`, the `Pairing*`, the
//! `Recovery*` and the `Invite*` requests are ordinary `dispatch` requests; the invite requests start
//! their task through the core's `InviteRequested` event, and `InviteCancel` and `InviteStatus` work
//! through the core's session (`InviteChanged` wakes the task).
//!
//! **An invitee's first account.** A person who joins with an invitation and has no account on the
//! server gets one from the invitation (the share signup, in the invite task). Adopting the space
//! points sync at that server, so the driver may meet the server before the task has created the
//! account; its "the server does not know this account" is not recorded (no invite token is needed)
//! while an invitee's session runs, and the next cycle, which the join asks for, finds the account.

use std::sync::{Arc, Weak};

use kb_crypto::Signature;
use kb_net::invite::{InviteHost, InviteTasks};
use kb_net::v2::{SyncDriver, SyncHost};
use kb_net::{
    DeviceTasks, DriverHandle, HttpRemote, PairingHost, RecoveryHost, RecoveryUpload,
    RemoteFactory, TaskHandle,
};
use kb_sync::v2::{
    FeedChunkWork, FeedWork, InlineEntry, InlineOutcome, InviteOpenRequest, InviteOpened,
    InviteSignup, LinkOutcome, LinkWork, LogRequest, MembershipHead, MembershipPage, OutboundEntry,
    PendingMembership, SnapshotMark, SnapshotsApplied, SyncApplied,
};
use kb_sync::{ChangesPage, SnapshotsPage};
use kb_types::devices::MAILBOX_ID_LEN;
use kb_types::{
    AccountId, ApiError, ChunkId, DeviceId, DocId, Event, FeedId, InviteSessionInfo,
    InviteSessionState, InviteSide, LinkId, PairingInfo, PairingRole, PresenceMessage,
    RecoveryInfo, SyncInfo, SyncState,
};

use crate::state::{AppState, BackgroundStarter, BackgroundTask, CoreApi};

/// The core's sync methods (`kb_core::Core`'s, one to one), as the shell calls them under its lock. A
/// separate seam from [`CoreApi`] so the fakes that do not sync need not implement it.
pub trait SyncCoreApi {
    fn sync_info(&mut self) -> Result<SyncInfo, ApiError>;
    fn sync_identity(&mut self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
    fn sync_take_invite_token(&mut self) -> Result<Option<String>, ApiError>;
    fn sync_account_unknown(&mut self) -> Result<(), ApiError>;
    fn sync_log_received(&mut self, entries: &[Vec<u8>]) -> Result<bool, ApiError>;
    fn sync_account_log(&mut self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
    fn sync_log_published(&mut self, up_to_seq: u64) -> Result<(), ApiError>;
    fn sync_sign(&mut self, message: &[u8]) -> Result<Signature, ApiError>;
    fn sync_remote_epoch(&mut self, epoch: [u8; 16]) -> Result<bool, ApiError>;
    fn sync_set_state(&mut self, state: SyncState, error: Option<String>) -> Result<(), ApiError>;
    fn sync_recovery_blob(&mut self) -> Result<Option<RecoveryUpload>, ApiError>;
    fn sync_recovery_published(&mut self) -> Result<(), ApiError>;
    fn sync_recovery_obsolete(&mut self) -> Result<Vec<[u8; 32]>, ApiError>;
    fn sync_recovery_deleted(&mut self, id: [u8; 32]) -> Result<(), ApiError>;

    // ----- feeds and co-members -----
    fn sync_feeds(&mut self) -> Result<Vec<FeedWork>, ApiError>;
    fn sync_needed_logs(&mut self) -> Result<Vec<LogRequest>, ApiError>;
    fn sync_foreign_log_received(
        &mut self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError>;

    // ----- membership -----
    fn sync_membership_pending(&mut self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError>;
    fn sync_membership_posted(
        &mut self,
        feed: FeedId,
        head: MembershipHead,
    ) -> Result<(), ApiError>;
    fn sync_membership_conflict(&mut self, feed: FeedId) -> Result<(), ApiError>;
    fn sync_membership_wanted(&mut self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError>;
    fn sync_membership_fetched(
        &mut self,
        feed: FeedId,
        page: &MembershipPage,
    ) -> Result<(), ApiError>;
    fn sync_feed_lost(&mut self, feed: FeedId) -> Result<(), ApiError>;
    fn sync_feed_refused(&mut self, feed: FeedId, message: &str) -> Result<(), ApiError>;
    fn sync_feed_rekey_needed(&mut self, feed: FeedId) -> Result<(), ApiError>;

    // ----- entries, snapshots, heads, chunks of one feed -----
    fn sync_feed_outbox(&mut self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError>;
    fn sync_feed_pushed(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        up_to_seq: i64,
    ) -> Result<(), ApiError>;
    fn sync_feed_apply(
        &mut self,
        feed: FeedId,
        page: &ChangesPage,
    ) -> Result<SyncApplied, ApiError>;
    fn sync_feed_snapshot_candidates(&mut self, feed: FeedId) -> Result<Vec<DocId>, ApiError>;
    fn sync_feed_make_snapshot(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(Vec<u8>, u64), ApiError>;
    fn sync_feed_snapshot_published(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError>;
    fn sync_feed_apply_snapshots(
        &mut self,
        feed: FeedId,
        page: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError>;
    fn sync_snapshot_marks_wanted(&mut self, feed: FeedId) -> Result<Option<u64>, ApiError>;
    fn sync_snapshot_marks_received(
        &mut self,
        feed: FeedId,
        marks: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError>;
    fn sync_snapshot_verified(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        ok: bool,
    ) -> Result<(), ApiError>;
    fn sync_feed_head(&mut self, feed: FeedId) -> Result<Vec<u8>, ApiError>;
    fn sync_feed_heads_received(
        &mut self,
        feed: FeedId,
        heads: &[Vec<u8>],
    ) -> Result<bool, ApiError>;
    fn sync_feed_chunk_work(
        &mut self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError>;
    fn sync_feed_chunk_bytes(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError>;
    fn sync_feed_chunk_uploaded(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
    fn sync_feed_chunk_received(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError>;
    fn sync_feed_chunk_unavailable(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
    fn sync_feed_chunk_deleted(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;

    // ----- realtime and presence (M13 slice 3, `kb_core::Core`'s, see `kb_net::realtime`) -----
    fn sync_feed_apply_inline(
        &mut self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError>;
    fn sync_push_debounce_ms(&mut self) -> Result<u64, ApiError>;
    fn presence_open(
        &mut self,
        feed: FeedId,
        envelope: &[u8],
    ) -> Result<Option<PresenceMessage>, ApiError>;
    fn presence_expire(&mut self) -> Result<(), ApiError>;

    // ----- view links (M13 slice 4, `kb_core::Core`'s, see `kb_net::v2` `links.rs`) -----
    fn sync_link_work(&mut self) -> Result<Vec<LinkWork>, ApiError>;
    fn sync_link_done(&mut self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError>;
}

/// The core's pairing and recovery methods (ADR-0022), as the shell calls them under its lock: the seam
/// of the device tasks.
pub trait DevicesCoreApi {
    fn pairing_info(&mut self) -> Result<PairingInfo, ApiError>;
    fn pairing_mailbox_id(&mut self) -> Result<Option<[u8; MAILBOX_ID_LEN]>, ApiError>;
    fn pairing_mailbox_opened(
        &mut self,
        mailbox_id: [u8; MAILBOX_ID_LEN],
        expires_at: i64,
    ) -> Result<(), ApiError>;
    fn pairing_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError>;
    fn pairing_posted(&mut self, seq: u32) -> Result<(), ApiError>;
    fn pairing_receive(
        &mut self,
        seq: u32,
        role: PairingRole,
        bytes: &[u8],
    ) -> Result<(), ApiError>;
    fn pairing_failed(&mut self, message: &str) -> Result<(), ApiError>;
    fn recovery_info(&mut self) -> Result<RecoveryInfo, ApiError>;
    fn recovery_id(&mut self) -> Result<Option<[u8; 32]>, ApiError>;
    fn recovery_blob_received(&mut self, bytes: &[u8]) -> Result<(), ApiError>;
    fn recovery_log_received(&mut self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError>;
    fn recovery_entry_published(&mut self) -> Result<(), ApiError>;
    fn recovery_failed(&mut self, message: &str) -> Result<(), ApiError>;
    fn recovery_account_id(&mut self) -> Result<Option<AccountId>, ApiError>;
}

/// The core's invite-session methods (ADR-0046, `kb_core::Core`'s `invite_*`), as the shell calls them
/// under its lock: the seam of the invite task. The sync methods the task shares with the driver
/// (`sync_identity`, `sync_sign`, `sync_account_log`, `sync_log_published`, `sync_needed_logs`,
/// `sync_foreign_log_received`) come from [`SyncCoreApi`].
pub trait InviteCoreApi {
    fn invite_info(&mut self) -> Result<InviteSessionInfo, ApiError>;
    fn invite_mailbox_id(&mut self) -> Result<Option<[u8; 6]>, ApiError>;
    fn invite_mailbox_opened(
        &mut self,
        mailbox_id: [u8; 6],
        expires_at: i64,
    ) -> Result<(), ApiError>;
    fn invite_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError>;
    fn invite_posted(&mut self, seq: u32) -> Result<(), ApiError>;
    fn invite_receive(&mut self, seq: u32, from: InviteSide, bytes: &[u8]) -> Result<(), ApiError>;
    fn invite_failed(&mut self, message: &str) -> Result<(), ApiError>;
    fn invite_membership_pending(&mut self) -> Result<Option<PendingMembership>, ApiError>;
    fn invite_membership_posted(&mut self, head: MembershipHead) -> Result<(), ApiError>;
    fn invite_signup(&mut self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError>;
    fn invite_open_request(
        &mut self,
        server_epoch: [u8; 16],
    ) -> Result<Option<InviteOpenRequest>, ApiError>;
    fn invite_opened(&mut self, opened: &InviteOpened) -> Result<(), ApiError>;
    fn invite_join(&mut self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError>;
    fn invite_joined(&mut self, head: MembershipHead) -> Result<(), ApiError>;
}

/// Whether a session of the invitee side is running: the person is about to get an account on the
/// server from the invitation, or has just got it.
fn invitee_session_running(info: &InviteSessionInfo) -> bool {
    info.side == InviteSide::Invitee
        && !matches!(
            info.state,
            InviteSessionState::Idle
                | InviteSessionState::Done
                | InviteSessionState::Failed
                | InviteSessionState::Cancelled
        )
}

impl SyncCoreApi for kb_core::Core {
    fn sync_info(&mut self) -> Result<SyncInfo, ApiError> {
        kb_core::Core::sync_info(self)
    }

    fn sync_identity(&mut self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        kb_core::Core::sync_identity(self)
    }

    fn sync_take_invite_token(&mut self) -> Result<Option<String>, ApiError> {
        kb_core::Core::sync_take_invite_token(self)
    }

    fn sync_account_unknown(&mut self) -> Result<(), ApiError> {
        // An invitee that is about to get its account from the invitation does not need a token.
        if kb_core::Core::invite_info(self).is_ok_and(|info| invitee_session_running(&info)) {
            return Ok(());
        }
        kb_core::Core::sync_account_unknown(self)
    }

    fn sync_log_received(&mut self, entries: &[Vec<u8>]) -> Result<bool, ApiError> {
        kb_core::Core::sync_log_received(self, entries)
    }

    fn sync_account_log(&mut self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        kb_core::Core::sync_account_log(self)
    }

    fn sync_log_published(&mut self, up_to_seq: u64) -> Result<(), ApiError> {
        kb_core::Core::sync_log_published(self, up_to_seq)
    }

    fn sync_sign(&mut self, message: &[u8]) -> Result<Signature, ApiError> {
        kb_core::Core::sync_sign(self, message)
    }

    fn sync_remote_epoch(&mut self, epoch: [u8; 16]) -> Result<bool, ApiError> {
        kb_core::Core::sync_remote_epoch(self, epoch)
    }

    fn sync_set_state(&mut self, state: SyncState, error: Option<String>) -> Result<(), ApiError> {
        kb_core::Core::sync_set_state(self, state, error)
    }

    fn sync_recovery_blob(&mut self) -> Result<Option<RecoveryUpload>, ApiError> {
        kb_core::Core::sync_recovery_blob(self)
    }

    fn sync_recovery_published(&mut self) -> Result<(), ApiError> {
        kb_core::Core::sync_recovery_published(self)
    }

    fn sync_recovery_obsolete(&mut self) -> Result<Vec<[u8; 32]>, ApiError> {
        kb_core::Core::sync_recovery_obsolete(self)
    }

    fn sync_recovery_deleted(&mut self, id: [u8; 32]) -> Result<(), ApiError> {
        kb_core::Core::sync_recovery_deleted(self, id)
    }

    fn sync_feeds(&mut self) -> Result<Vec<FeedWork>, ApiError> {
        kb_core::Core::sync_feeds(self)
    }

    fn sync_needed_logs(&mut self) -> Result<Vec<LogRequest>, ApiError> {
        kb_core::Core::sync_needed_logs(self)
    }

    fn sync_foreign_log_received(
        &mut self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        kb_core::Core::sync_foreign_log_received(self, account, entries)
    }

    fn sync_membership_pending(&mut self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError> {
        kb_core::Core::sync_membership_pending(self, feed)
    }

    fn sync_membership_posted(
        &mut self,
        feed: FeedId,
        head: MembershipHead,
    ) -> Result<(), ApiError> {
        kb_core::Core::sync_membership_posted(self, feed, head)
    }

    fn sync_membership_conflict(&mut self, feed: FeedId) -> Result<(), ApiError> {
        kb_core::Core::sync_membership_conflict(self, feed)
    }

    fn sync_membership_wanted(&mut self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError> {
        kb_core::Core::sync_membership_wanted(self, feed)
    }

    fn sync_membership_fetched(
        &mut self,
        feed: FeedId,
        page: &MembershipPage,
    ) -> Result<(), ApiError> {
        kb_core::Core::sync_membership_fetched(self, feed, page)
    }

    fn sync_feed_lost(&mut self, feed: FeedId) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_lost(self, feed)
    }

    fn sync_feed_refused(&mut self, feed: FeedId, message: &str) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_refused(self, feed, message)
    }

    fn sync_feed_rekey_needed(&mut self, feed: FeedId) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_rekey_needed(self, feed)
    }

    fn sync_feed_outbox(&mut self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError> {
        kb_core::Core::sync_feed_outbox(self, feed)
    }

    fn sync_feed_pushed(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        up_to_seq: i64,
    ) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_pushed(self, feed, doc_id, device_seq, cursor, up_to_seq)
    }

    fn sync_feed_apply(
        &mut self,
        feed: FeedId,
        page: &ChangesPage,
    ) -> Result<SyncApplied, ApiError> {
        kb_core::Core::sync_feed_apply(self, feed, page)
    }

    fn sync_feed_snapshot_candidates(&mut self, feed: FeedId) -> Result<Vec<DocId>, ApiError> {
        kb_core::Core::sync_feed_snapshot_candidates(self, feed)
    }

    fn sync_feed_make_snapshot(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(Vec<u8>, u64), ApiError> {
        kb_core::Core::sync_feed_make_snapshot(self, feed, doc_id)
    }

    fn sync_feed_snapshot_published(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_snapshot_published(self, feed, doc_id, covers)
    }

    fn sync_feed_apply_snapshots(
        &mut self,
        feed: FeedId,
        page: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError> {
        kb_core::Core::sync_feed_apply_snapshots(self, feed, page)
    }

    fn sync_snapshot_marks_wanted(&mut self, feed: FeedId) -> Result<Option<u64>, ApiError> {
        kb_core::Core::sync_snapshot_marks_wanted(self, feed)
    }

    fn sync_snapshot_marks_received(
        &mut self,
        feed: FeedId,
        marks: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError> {
        kb_core::Core::sync_snapshot_marks_received(self, feed, marks)
    }

    fn sync_snapshot_verified(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        ok: bool,
    ) -> Result<(), ApiError> {
        kb_core::Core::sync_snapshot_verified(self, feed, doc_id, ok)
    }

    fn sync_feed_head(&mut self, feed: FeedId) -> Result<Vec<u8>, ApiError> {
        kb_core::Core::sync_feed_head(self, feed)
    }

    fn sync_feed_heads_received(
        &mut self,
        feed: FeedId,
        heads: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        kb_core::Core::sync_feed_heads_received(self, feed, heads)
    }

    fn sync_feed_chunk_work(
        &mut self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError> {
        kb_core::Core::sync_feed_chunk_work(self, feed, limit)
    }

    fn sync_feed_chunk_bytes(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        kb_core::Core::sync_feed_chunk_bytes(self, feed, hash)
    }

    fn sync_feed_chunk_uploaded(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_chunk_uploaded(self, feed, hash)
    }

    fn sync_feed_chunk_received(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_chunk_received(self, feed, hash, bytes)
    }

    fn sync_feed_chunk_unavailable(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_chunk_unavailable(self, feed, hash)
    }

    fn sync_feed_chunk_deleted(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        kb_core::Core::sync_feed_chunk_deleted(self, feed, hash)
    }

    fn sync_feed_apply_inline(
        &mut self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError> {
        kb_core::Core::sync_feed_apply_inline(self, feed, inline)
    }

    fn sync_push_debounce_ms(&mut self) -> Result<u64, ApiError> {
        kb_core::Core::sync_push_debounce_ms(self)
    }

    fn presence_open(
        &mut self,
        feed: FeedId,
        envelope: &[u8],
    ) -> Result<Option<PresenceMessage>, ApiError> {
        kb_core::Core::presence_open(self, feed, envelope)
    }

    fn presence_expire(&mut self) -> Result<(), ApiError> {
        kb_core::Core::presence_expire(self)
    }

    fn sync_link_work(&mut self) -> Result<Vec<LinkWork>, ApiError> {
        kb_core::Core::sync_link_work(self)
    }

    fn sync_link_done(&mut self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError> {
        kb_core::Core::sync_link_done(self, link_id, outcome)
    }
}

impl DevicesCoreApi for kb_core::Core {
    fn pairing_info(&mut self) -> Result<PairingInfo, ApiError> {
        Ok(kb_core::Core::pairing_info(self))
    }

    fn pairing_mailbox_id(&mut self) -> Result<Option<[u8; MAILBOX_ID_LEN]>, ApiError> {
        kb_core::Core::pairing_mailbox_id(self)
    }

    fn pairing_mailbox_opened(
        &mut self,
        mailbox_id: [u8; MAILBOX_ID_LEN],
        expires_at: i64,
    ) -> Result<(), ApiError> {
        kb_core::Core::pairing_mailbox_opened(self, mailbox_id, expires_at)
    }

    fn pairing_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        kb_core::Core::pairing_outbox(self)
    }

    fn pairing_posted(&mut self, seq: u32) -> Result<(), ApiError> {
        kb_core::Core::pairing_posted(self, seq)
    }

    fn pairing_receive(
        &mut self,
        seq: u32,
        role: PairingRole,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        kb_core::Core::pairing_receive(self, seq, role, bytes)
    }

    fn pairing_failed(&mut self, message: &str) -> Result<(), ApiError> {
        kb_core::Core::pairing_failed(self, message)
    }

    fn recovery_info(&mut self) -> Result<RecoveryInfo, ApiError> {
        Ok(kb_core::Core::recovery_info(self))
    }

    fn recovery_id(&mut self) -> Result<Option<[u8; 32]>, ApiError> {
        kb_core::Core::recovery_id(self)
    }

    fn recovery_blob_received(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        kb_core::Core::recovery_blob_received(self, bytes)
    }

    fn recovery_log_received(&mut self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError> {
        kb_core::Core::recovery_log_received(self, entries)
    }

    fn recovery_entry_published(&mut self) -> Result<(), ApiError> {
        kb_core::Core::recovery_entry_published(self)
    }

    fn recovery_failed(&mut self, message: &str) -> Result<(), ApiError> {
        kb_core::Core::recovery_failed(self, message)
    }

    fn recovery_account_id(&mut self) -> Result<Option<AccountId>, ApiError> {
        kb_core::Core::recovery_account_id(self)
    }
}

impl InviteCoreApi for kb_core::Core {
    fn invite_info(&mut self) -> Result<InviteSessionInfo, ApiError> {
        kb_core::Core::invite_info(self)
    }

    fn invite_mailbox_id(&mut self) -> Result<Option<[u8; 6]>, ApiError> {
        kb_core::Core::invite_mailbox_id(self)
    }

    fn invite_mailbox_opened(
        &mut self,
        mailbox_id: [u8; 6],
        expires_at: i64,
    ) -> Result<(), ApiError> {
        kb_core::Core::invite_mailbox_opened(self, mailbox_id, expires_at)
    }

    fn invite_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        kb_core::Core::invite_outbox(self)
    }

    fn invite_posted(&mut self, seq: u32) -> Result<(), ApiError> {
        kb_core::Core::invite_posted(self, seq)
    }

    fn invite_receive(&mut self, seq: u32, from: InviteSide, bytes: &[u8]) -> Result<(), ApiError> {
        kb_core::Core::invite_receive(self, seq, from, bytes)
    }

    fn invite_failed(&mut self, message: &str) -> Result<(), ApiError> {
        kb_core::Core::invite_failed(self, message)
    }

    fn invite_membership_pending(&mut self) -> Result<Option<PendingMembership>, ApiError> {
        kb_core::Core::invite_membership_pending(self)
    }

    fn invite_membership_posted(&mut self, head: MembershipHead) -> Result<(), ApiError> {
        kb_core::Core::invite_membership_posted(self, head)
    }

    fn invite_signup(&mut self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError> {
        kb_core::Core::invite_signup(self, server_epoch)
    }

    fn invite_open_request(
        &mut self,
        server_epoch: [u8; 16],
    ) -> Result<Option<InviteOpenRequest>, ApiError> {
        kb_core::Core::invite_open_request(self, server_epoch)
    }

    fn invite_opened(&mut self, opened: &InviteOpened) -> Result<(), ApiError> {
        kb_core::Core::invite_opened(self, opened)
    }

    fn invite_join(&mut self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError> {
        kb_core::Core::invite_join(self)
    }

    fn invite_joined(&mut self, head: MembershipHead) -> Result<(), ApiError> {
        kb_core::Core::invite_joined(self, head)
    }
}

impl<C: CoreApi> AppState<C> {
    /// One sync call under the lock: logs the method, the outcome (the error code, never its message)
    /// and the event count.
    fn sync_call<T>(
        &self,
        kind: &'static str,
        call: impl FnOnce(&mut C) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        self.with_core(call, |result, events| match result {
            Ok(_) => tracing::trace!(kind, events, "sync"),
            Err(error) => tracing::debug!(kind, events, code = ?error.code, "sync call refused"),
        })
    }
}

impl<C: CoreApi + SyncCoreApi> SyncHost for AppState<C> {
    fn sync_info(&self) -> Result<SyncInfo, ApiError> {
        self.sync_call("sync_info", SyncCoreApi::sync_info)
    }

    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.sync_call("sync_identity", SyncCoreApi::sync_identity)
    }

    fn sync_take_invite_token(&self) -> Result<Option<String>, ApiError> {
        self.sync_call(
            "sync_take_invite_token",
            SyncCoreApi::sync_take_invite_token,
        )
    }

    fn sync_account_unknown(&self) -> Result<(), ApiError> {
        self.sync_call("sync_account_unknown", SyncCoreApi::sync_account_unknown)
    }

    fn sync_log_received(&self, entries: &[Vec<u8>]) -> Result<bool, ApiError> {
        self.sync_call("sync_log_received", |core| core.sync_log_received(entries))
    }

    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.sync_call("sync_account_log", SyncCoreApi::sync_account_log)
    }

    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.sync_call("sync_log_published", |core| {
            core.sync_log_published(up_to_seq)
        })
    }

    fn sync_sign(&self, message: &[u8]) -> Result<Signature, ApiError> {
        self.sync_call("sync_sign", |core| core.sync_sign(message))
    }

    fn sync_remote_epoch(&self, epoch: [u8; 16]) -> Result<bool, ApiError> {
        self.sync_call("sync_remote_epoch", |core| core.sync_remote_epoch(epoch))
    }

    fn sync_set_state(&self, state: SyncState, error: Option<String>) -> Result<(), ApiError> {
        self.sync_call("sync_set_state", |core| core.sync_set_state(state, error))
    }

    fn sync_recovery_blob(&self) -> Result<Option<RecoveryUpload>, ApiError> {
        self.sync_call("sync_recovery_blob", SyncCoreApi::sync_recovery_blob)
    }

    fn sync_recovery_published(&self) -> Result<(), ApiError> {
        self.sync_call(
            "sync_recovery_published",
            SyncCoreApi::sync_recovery_published,
        )
    }

    fn sync_recovery_obsolete(&self) -> Result<Vec<[u8; 32]>, ApiError> {
        self.sync_call(
            "sync_recovery_obsolete",
            SyncCoreApi::sync_recovery_obsolete,
        )
    }

    fn sync_recovery_deleted(&self, id: [u8; 32]) -> Result<(), ApiError> {
        self.sync_call("sync_recovery_deleted", |core| {
            core.sync_recovery_deleted(id)
        })
    }

    fn sync_feeds(&self) -> Result<Vec<FeedWork>, ApiError> {
        self.sync_call("sync_feeds", SyncCoreApi::sync_feeds)
    }

    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError> {
        self.sync_call("sync_needed_logs", SyncCoreApi::sync_needed_logs)
    }

    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.sync_call("sync_foreign_log_received", |core| {
            core.sync_foreign_log_received(account, entries)
        })
    }

    fn sync_membership_pending(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError> {
        self.sync_call("sync_membership_pending", |core| {
            core.sync_membership_pending(feed)
        })
    }

    fn sync_membership_posted(&self, feed: FeedId, head: MembershipHead) -> Result<(), ApiError> {
        self.sync_call("sync_membership_posted", |core| {
            core.sync_membership_posted(feed, head)
        })
    }

    fn sync_membership_conflict(&self, feed: FeedId) -> Result<(), ApiError> {
        self.sync_call("sync_membership_conflict", |core| {
            core.sync_membership_conflict(feed)
        })
    }

    fn sync_membership_wanted(&self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError> {
        self.sync_call("sync_membership_wanted", |core| {
            core.sync_membership_wanted(feed)
        })
    }

    fn sync_membership_fetched(&self, feed: FeedId, page: &MembershipPage) -> Result<(), ApiError> {
        self.sync_call("sync_membership_fetched", |core| {
            core.sync_membership_fetched(feed, page)
        })
    }

    fn sync_feed_lost(&self, feed: FeedId) -> Result<(), ApiError> {
        self.sync_call("sync_feed_lost", |core| core.sync_feed_lost(feed))
    }

    fn sync_feed_refused(&self, feed: FeedId, message: &str) -> Result<(), ApiError> {
        self.sync_call("sync_feed_refused", |core| {
            core.sync_feed_refused(feed, message)
        })
    }

    fn sync_feed_rekey_needed(&self, feed: FeedId) -> Result<(), ApiError> {
        self.sync_call("sync_feed_rekey_needed", |core| {
            core.sync_feed_rekey_needed(feed)
        })
    }

    fn sync_feed_outbox(&self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError> {
        self.sync_call("sync_feed_outbox", |core| core.sync_feed_outbox(feed))
    }

    fn sync_feed_pushed(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        up_to_seq: i64,
    ) -> Result<(), ApiError> {
        self.sync_call("sync_feed_pushed", |core| {
            core.sync_feed_pushed(feed, doc_id, device_seq, cursor, up_to_seq)
        })
    }

    fn sync_feed_apply(&self, feed: FeedId, page: &ChangesPage) -> Result<SyncApplied, ApiError> {
        self.sync_call("sync_feed_apply", |core| core.sync_feed_apply(feed, page))
    }

    fn sync_feed_snapshot_candidates(&self, feed: FeedId) -> Result<Vec<DocId>, ApiError> {
        self.sync_call("sync_feed_snapshot_candidates", |core| {
            core.sync_feed_snapshot_candidates(feed)
        })
    }

    fn sync_feed_make_snapshot(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(Vec<u8>, u64), ApiError> {
        self.sync_call("sync_feed_make_snapshot", |core| {
            core.sync_feed_make_snapshot(feed, doc_id)
        })
    }

    fn sync_feed_snapshot_published(
        &self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError> {
        self.sync_call("sync_feed_snapshot_published", |core| {
            core.sync_feed_snapshot_published(feed, doc_id, covers)
        })
    }

    fn sync_feed_apply_snapshots(
        &self,
        feed: FeedId,
        page: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError> {
        self.sync_call("sync_feed_apply_snapshots", |core| {
            core.sync_feed_apply_snapshots(feed, page)
        })
    }

    fn sync_snapshot_marks_wanted(&self, feed: FeedId) -> Result<Option<u64>, ApiError> {
        self.sync_call("sync_snapshot_marks_wanted", |core| {
            core.sync_snapshot_marks_wanted(feed)
        })
    }

    fn sync_snapshot_marks_received(
        &self,
        feed: FeedId,
        marks: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError> {
        self.sync_call("sync_snapshot_marks_received", |core| {
            core.sync_snapshot_marks_received(feed, marks)
        })
    }

    fn sync_snapshot_verified(
        &self,
        feed: FeedId,
        doc_id: DocId,
        ok: bool,
    ) -> Result<(), ApiError> {
        self.sync_call("sync_snapshot_verified", |core| {
            core.sync_snapshot_verified(feed, doc_id, ok)
        })
    }

    fn sync_feed_head(&self, feed: FeedId) -> Result<Vec<u8>, ApiError> {
        self.sync_call("sync_feed_head", |core| core.sync_feed_head(feed))
    }

    fn sync_feed_heads_received(&self, feed: FeedId, heads: &[Vec<u8>]) -> Result<bool, ApiError> {
        self.sync_call("sync_feed_heads_received", |core| {
            core.sync_feed_heads_received(feed, heads)
        })
    }

    fn sync_feed_chunk_work(
        &self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError> {
        self.sync_call("sync_feed_chunk_work", |core| {
            core.sync_feed_chunk_work(feed, limit)
        })
    }

    fn sync_feed_chunk_bytes(
        &self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        self.sync_call("sync_feed_chunk_bytes", |core| {
            core.sync_feed_chunk_bytes(feed, hash)
        })
    }

    fn sync_feed_chunk_uploaded(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.sync_call("sync_feed_chunk_uploaded", |core| {
            core.sync_feed_chunk_uploaded(feed, hash)
        })
    }

    fn sync_feed_chunk_received(
        &self,
        feed: FeedId,
        hash: ChunkId,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError> {
        self.sync_call("sync_feed_chunk_received", move |core| {
            core.sync_feed_chunk_received(feed, hash, bytes)
        })
    }

    fn sync_feed_chunk_unavailable(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.sync_call("sync_feed_chunk_unavailable", |core| {
            core.sync_feed_chunk_unavailable(feed, hash)
        })
    }

    fn sync_feed_chunk_deleted(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.sync_call("sync_feed_chunk_deleted", |core| {
            core.sync_feed_chunk_deleted(feed, hash)
        })
    }

    fn sync_feed_apply_inline(
        &self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError> {
        self.sync_call("sync_feed_apply_inline", |core| {
            core.sync_feed_apply_inline(feed, inline)
        })
    }

    fn sync_push_debounce_ms(&self) -> Result<u64, ApiError> {
        self.sync_call("sync_push_debounce_ms", SyncCoreApi::sync_push_debounce_ms)
    }

    fn presence_received(&self, feed: FeedId, envelope: &[u8]) -> Result<(), ApiError> {
        // The core opens the frame under its lock (and emits `PresenceChanged` when someone appeared, moved or
        // left); an accepted one goes to the webview's channel after the lock is released.
        let accepted =
            self.sync_call("presence_open", |core| core.presence_open(feed, envelope))?;
        if let Some(message) = accepted {
            let heard = self.presence_deliver(&message);
            tracing::trace!(heard, "presence frame delivered");
        }
        Ok(())
    }

    fn presence_expire(&self) -> Result<(), ApiError> {
        self.sync_call("presence_expire", SyncCoreApi::presence_expire)
    }

    fn sync_link_work(&self) -> Result<Vec<LinkWork>, ApiError> {
        self.sync_call("sync_link_work", SyncCoreApi::sync_link_work)
    }

    fn sync_link_done(&self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError> {
        self.sync_call("sync_link_done", |core| {
            core.sync_link_done(link_id, outcome)
        })
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl<C: CoreApi + SyncCoreApi + InviteCoreApi> InviteHost for AppState<C> {
    fn invite_info(&self) -> Result<InviteSessionInfo, ApiError> {
        self.sync_call("invite_info", InviteCoreApi::invite_info)
    }

    fn invite_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError> {
        self.sync_call("invite_mailbox_id", InviteCoreApi::invite_mailbox_id)
    }

    fn invite_mailbox_opened(&self, mailbox_id: [u8; 6], expires_at: i64) -> Result<(), ApiError> {
        self.sync_call("invite_mailbox_opened", |core| {
            core.invite_mailbox_opened(mailbox_id, expires_at)
        })
    }

    fn invite_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        self.sync_call("invite_outbox", InviteCoreApi::invite_outbox)
    }

    fn invite_posted(&self, seq: u32) -> Result<(), ApiError> {
        self.sync_call("invite_posted", |core| core.invite_posted(seq))
    }

    fn invite_receive(&self, seq: u32, from: InviteSide, bytes: &[u8]) -> Result<(), ApiError> {
        self.sync_call("invite_receive", |core| {
            core.invite_receive(seq, from, bytes)
        })
    }

    fn invite_failed(&self, message: &str) -> Result<(), ApiError> {
        self.sync_call("invite_failed", |core| core.invite_failed(message))
    }

    fn invite_membership_pending(&self) -> Result<Option<PendingMembership>, ApiError> {
        self.sync_call(
            "invite_membership_pending",
            InviteCoreApi::invite_membership_pending,
        )
    }

    fn invite_membership_posted(&self, head: MembershipHead) -> Result<(), ApiError> {
        self.sync_call("invite_membership_posted", |core| {
            core.invite_membership_posted(head)
        })
    }

    fn invite_signup(&self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError> {
        self.sync_call("invite_signup", |core| core.invite_signup(server_epoch))
    }

    fn invite_open_request(
        &self,
        server_epoch: [u8; 16],
    ) -> Result<Option<InviteOpenRequest>, ApiError> {
        self.sync_call("invite_open_request", |core| {
            core.invite_open_request(server_epoch)
        })
    }

    fn invite_opened(&self, opened: &InviteOpened) -> Result<(), ApiError> {
        self.sync_call("invite_opened", |core| core.invite_opened(opened))
    }

    fn invite_join(&self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError> {
        self.sync_call("invite_join", InviteCoreApi::invite_join)
    }

    fn invite_joined(&self, head: MembershipHead) -> Result<(), ApiError> {
        self.sync_call("invite_joined", |core| core.invite_joined(head))
    }

    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.sync_call("sync_identity", SyncCoreApi::sync_identity)
    }

    fn sync_sign(&self, message: &[u8]) -> Result<Signature, ApiError> {
        self.sync_call("sync_sign", |core| core.sync_sign(message))
    }

    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.sync_call("sync_account_log", SyncCoreApi::sync_account_log)
    }

    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.sync_call("sync_log_published", |core| {
            core.sync_log_published(up_to_seq)
        })
    }

    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError> {
        self.sync_call("sync_needed_logs", SyncCoreApi::sync_needed_logs)
    }

    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.sync_call("sync_foreign_log_received", |core| {
            core.sync_foreign_log_received(account, entries)
        })
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl<C: CoreApi + SyncCoreApi + DevicesCoreApi> PairingHost for AppState<C> {
    fn pairing_info(&self) -> Result<PairingInfo, ApiError> {
        self.sync_call("pairing_info", DevicesCoreApi::pairing_info)
    }

    fn pairing_mailbox_id(&self) -> Result<Option<[u8; MAILBOX_ID_LEN]>, ApiError> {
        self.sync_call("pairing_mailbox_id", DevicesCoreApi::pairing_mailbox_id)
    }

    fn pairing_mailbox_opened(
        &self,
        mailbox_id: [u8; MAILBOX_ID_LEN],
        expires_at: i64,
    ) -> Result<(), ApiError> {
        self.sync_call("pairing_mailbox_opened", |core| {
            core.pairing_mailbox_opened(mailbox_id, expires_at)
        })
    }

    fn pairing_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        self.sync_call("pairing_outbox", DevicesCoreApi::pairing_outbox)
    }

    fn pairing_posted(&self, seq: u32) -> Result<(), ApiError> {
        self.sync_call("pairing_posted", |core| core.pairing_posted(seq))
    }

    fn pairing_receive(&self, seq: u32, role: PairingRole, bytes: &[u8]) -> Result<(), ApiError> {
        self.sync_call("pairing_receive", |core| {
            core.pairing_receive(seq, role, bytes)
        })
    }

    fn pairing_failed(&self, message: &str) -> Result<(), ApiError> {
        self.sync_call("pairing_failed", |core| core.pairing_failed(message))
    }

    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.sync_call("sync_identity", SyncCoreApi::sync_identity)
    }

    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.sync_call("sync_account_log", SyncCoreApi::sync_account_log)
    }

    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.sync_call("sync_log_published", |core| {
            core.sync_log_published(up_to_seq)
        })
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl<C: CoreApi + SyncCoreApi + DevicesCoreApi> RecoveryHost for AppState<C> {
    fn recovery_info(&self) -> Result<RecoveryInfo, ApiError> {
        self.sync_call("recovery_info", DevicesCoreApi::recovery_info)
    }

    fn recovery_id(&self) -> Result<Option<[u8; 32]>, ApiError> {
        self.sync_call("recovery_id", DevicesCoreApi::recovery_id)
    }

    fn recovery_blob_received(&self, bytes: &[u8]) -> Result<(), ApiError> {
        self.sync_call("recovery_blob_received", |core| {
            core.recovery_blob_received(bytes)
        })
    }

    fn recovery_log_received(&self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError> {
        self.sync_call("recovery_log_received", |core| {
            core.recovery_log_received(entries)
        })
    }

    fn recovery_entry_published(&self) -> Result<(), ApiError> {
        self.sync_call(
            "recovery_entry_published",
            DevicesCoreApi::recovery_entry_published,
        )
    }

    fn recovery_failed(&self, message: &str) -> Result<(), ApiError> {
        self.sync_call("recovery_failed", |core| core.recovery_failed(message))
    }

    fn recovery_account_id(&self) -> Result<Option<AccountId>, ApiError> {
        self.sync_call("recovery_account_id", DevicesCoreApi::recovery_account_id)
    }
}

/// What runs in the background while the core is ready: dropping it stops all three.
struct NetworkWork {
    _driver: DriverHandle,
    _devices: TaskHandle,
    _invites: TaskHandle,
}

/// The background work `lib.rs` registers: the v2 [`SyncDriver`] over this state (every feed), talking to
/// the configured server with [`HttpRemote`], on Tauri's tokio runtime, and the listeners that start the
/// device and invite tasks. It holds only a weak reference to the state until it starts (the running
/// driver holds a strong one, released when the core locks and the driver is dropped).
pub fn driver_starter<C: CoreApi + SyncCoreApi + DevicesCoreApi + InviteCoreApi>(
    state: Weak<AppState<C>>,
) -> BackgroundStarter {
    Box::new(move || {
        let state = state.upgrade()?;
        let factory: RemoteFactory<HttpRemote> =
            Arc::new(|url: &str| HttpRemote::new(url).map(Arc::new));
        let runtime = tauri::async_runtime::handle().inner().clone();
        let devices = DeviceTasks::spawn(Arc::clone(&state), Arc::clone(&factory), runtime.clone());
        let invites = InviteTasks::spawn(Arc::clone(&state), Arc::clone(&factory), runtime.clone());
        let presence = Arc::clone(&state);
        let host: Arc<dyn SyncHost> = state;
        let driver = SyncDriver::spawn(host, factory, runtime);
        // The shell's presence commands send through the driver's socket for as long as it runs; the handle
        // goes dead when the driver is dropped (the core locks), and the next start replaces it.
        presence.presence_attach(driver.realtime());
        let work: BackgroundTask = Box::new(NetworkWork {
            _driver: driver,
            _devices: devices,
            _invites: invites,
        });
        Some(work)
    })
}
