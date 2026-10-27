//! The two [`SyncRemote`] implementations of protocol v2: [`HttpRemote`] (the `/v2` routes of
//! `kb-server`, through its `kb_sync::v2::RemoteStore` implementation) and the reference
//! [`MemoryRemote`] (its synchronous `*_sync` twins, wrapped in ready futures), which the tests
//! drive the driver and the invite task against.
//!
//! Every call below names the `RemoteStore` trait explicitly: the inherent and the trait methods of
//! `HttpRemote` and `MemoryRemote` share names.

use std::sync::Arc;

use kb_crypto::{Secret32, Signature, SigningKey};
use kb_sync::v2::{
    FeedCreated, FeedListing, InviteOpened, LinkPut, LogChunk, LogRequest, MarksPage,
    MembershipHead, MembershipPage, MemoryRemote, RemoteStore, ServerInfo,
};
use kb_sync::{ChangesPage, PairingPage, SnapshotsPage};
use kb_types::{AccountId, ChunkId, DeviceId, DocId, FeedId, InviteId, LinkId};

use super::{RemoteFuture, SyncRemote};
use crate::http::{AuthSigner, HttpRemote};
use crate::realtime::{Realtime, memory_socket};

impl SyncRemote for HttpRemote {
    fn server_info(&self) -> RemoteFuture<'_, ServerInfo> {
        Box::pin(RemoteStore::server_info(self))
    }
    fn create_account<'a>(
        &'a self,
        invite_token: &'a str,
        log: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::create_account(self, invite_token, log))
    }
    fn account_create_share<'a>(
        &'a self,
        log: &'a [Vec<u8>],
        invite_id: InviteId,
        sig: &'a Signature,
    ) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::account_create_share(self, log, invite_id, sig))
    }
    fn append_log<'a>(
        &'a self,
        account: AccountId,
        entries: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::append_log(self, account, entries))
    }
    fn fetch_log(&self, account: AccountId, after: Option<u64>) -> RemoteFuture<'_, Vec<Vec<u8>>> {
        Box::pin(RemoteStore::fetch_log(self, account, after))
    }
    fn logs_batch<'a>(&'a self, requests: &'a [LogRequest]) -> RemoteFuture<'a, Vec<LogChunk>> {
        Box::pin(RemoteStore::logs_batch(self, requests))
    }
    fn needs_authentication(&self, account: AccountId, device: DeviceId) -> bool {
        !self.is_authenticated(account, device)
    }
    fn authenticate(
        &self,
        account: AccountId,
        device: DeviceId,
        signer: AuthSigner,
    ) -> RemoteFuture<'_, ()> {
        Box::pin(self.authenticate_with(account, device, signer))
    }
    fn feeds_create<'a>(&'a self, create: &'a [u8]) -> RemoteFuture<'a, FeedCreated> {
        Box::pin(RemoteStore::feeds_create(self, create))
    }
    fn feeds_list(&self) -> RemoteFuture<'_, Vec<FeedListing>> {
        Box::pin(RemoteStore::feeds_list(self))
    }
    fn membership_append<'a>(
        &'a self,
        feed: FeedId,
        entries: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, MembershipHead> {
        Box::pin(RemoteStore::membership_append(self, feed, entries))
    }
    fn membership_get(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> RemoteFuture<'_, MembershipPage> {
        Box::pin(RemoteStore::membership_get(self, feed, after_seq))
    }
    fn append<'a>(&'a self, feed: FeedId, entry: &'a [u8]) -> RemoteFuture<'a, (u64, [u8; 32])> {
        Box::pin(RemoteStore::append(self, feed, entry))
    }
    fn changes(&self, feed: FeedId, after: u64, limit: u32) -> RemoteFuture<'_, ChangesPage> {
        Box::pin(RemoteStore::changes(self, feed, after, limit))
    }
    fn put_snapshot<'a>(&'a self, feed: FeedId, snapshot: &'a [u8]) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::put_snapshot(self, feed, snapshot))
    }
    fn get_snapshot(&self, feed: FeedId, doc: DocId) -> RemoteFuture<'_, Option<Vec<u8>>> {
        Box::pin(RemoteStore::get_snapshot(self, feed, doc))
    }
    fn snapshots(&self, feed: FeedId) -> RemoteFuture<'_, SnapshotsPage> {
        Box::pin(RemoteStore::snapshots(self, feed))
    }
    fn snapshot_marks(&self, feed: FeedId, after: u64) -> RemoteFuture<'_, MarksPage> {
        Box::pin(RemoteStore::snapshot_marks(self, feed, after))
    }
    fn snapshot_dispute(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> RemoteFuture<'_, ()> {
        Box::pin(RemoteStore::snapshot_dispute(
            self,
            feed,
            doc,
            covers_cursor,
        ))
    }
    fn put_head<'a>(&'a self, feed: FeedId, head: &'a [u8]) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::put_head(self, feed, head))
    }
    fn heads(&self, feed: FeedId) -> RemoteFuture<'_, Vec<Vec<u8>>> {
        Box::pin(RemoteStore::heads(self, feed))
    }
    fn put_chunk<'a>(
        &'a self,
        feed: FeedId,
        hash: ChunkId,
        bytes: &'a [u8],
    ) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::put_chunk(self, feed, hash, bytes))
    }
    fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, Option<Vec<u8>>> {
        Box::pin(RemoteStore::get_chunk(self, feed, hash))
    }
    fn delete_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, ()> {
        Box::pin(RemoteStore::delete_chunk(self, feed, hash))
    }
    fn copy_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, ()> {
        Box::pin(RemoteStore::copy_chunk(self, feed, hash))
    }
    fn invite_open<'a>(
        &'a self,
        invite_id: InviteId,
        sig: &'a Signature,
    ) -> RemoteFuture<'a, InviteOpened> {
        Box::pin(RemoteStore::invite_open(self, invite_id, sig))
    }
    fn link_put<'a>(&'a self, link_id: LinkId, put: &'a LinkPut) -> RemoteFuture<'a, u64> {
        Box::pin(RemoteStore::link_put(self, link_id, put))
    }
    fn link_delete(&self, link_id: LinkId) -> RemoteFuture<'_, ()> {
        Box::pin(RemoteStore::link_delete(self, link_id))
    }
    fn realtime(&self) -> Option<Realtime> {
        match self.open_realtime() {
            Ok(realtime) => Some(realtime),
            Err(_) => {
                tracing::debug!("the realtime socket is not available");
                None
            }
        }
    }
    fn pair_create<'a>(&'a self, offer: &'a [u8]) -> RemoteFuture<'a, ([u8; 6], i64)> {
        Box::pin(RemoteStore::pair_create(self, offer))
    }
    fn pair_messages(
        &self,
        id: [u8; 6],
        after: u32,
        wait_seconds: u32,
    ) -> RemoteFuture<'_, PairingPage> {
        Box::pin(RemoteStore::pair_messages(self, id, after, wait_seconds))
    }
    fn pair_post<'a>(&'a self, id: [u8; 6], role: u8, bytes: &'a [u8]) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::pair_post(self, id, role, bytes))
    }
    fn pair_close(&self, id: [u8; 6]) -> RemoteFuture<'_, ()> {
        Box::pin(RemoteStore::pair_close(self, id))
    }
    fn recovery_put<'a>(&'a self, id: [u8; 32], bytes: &'a [u8]) -> RemoteFuture<'a, ()> {
        Box::pin(RemoteStore::recovery_put(self, id, bytes))
    }
    fn recovery_get(&self, id: [u8; 32]) -> RemoteFuture<'_, Option<Vec<u8>>> {
        Box::pin(RemoteStore::recovery_get(self, id))
    }
    fn recovery_delete(&self, id: [u8; 32]) -> RemoteFuture<'_, ()> {
        Box::pin(RemoteStore::recovery_delete(self, id))
    }
}

/// What a signer that cannot sign hands to a remote whose signature callback must answer: a
/// signature by a throwaway key, which no server accepts.
fn refusal(message: &[u8]) -> Signature {
    let key = SigningKey::from_seed(&Secret32::from_bytes([0u8; 32]));
    kb_crypto::sign(&key, "refused", message)
}

fn ready<'a, T: Send + 'a>(result: Result<T, kb_sync::SyncError>) -> RemoteFuture<'a, T> {
    Box::pin(std::future::ready(result))
}

/// The reference remote answers synchronously (its `*_sync` twins); tests drive the driver against it.
impl SyncRemote for MemoryRemote {
    fn server_info(&self) -> RemoteFuture<'_, ServerInfo> {
        ready(self.server_info_sync())
    }
    fn create_account<'a>(
        &'a self,
        invite_token: &'a str,
        log: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, ()> {
        ready(self.create_account_sync(invite_token, log))
    }
    fn account_create_share<'a>(
        &'a self,
        log: &'a [Vec<u8>],
        invite_id: InviteId,
        sig: &'a Signature,
    ) -> RemoteFuture<'a, ()> {
        ready(self.account_create_share_sync(log, invite_id, sig))
    }
    fn append_log<'a>(
        &'a self,
        account: AccountId,
        entries: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, ()> {
        ready(self.append_log_sync(account, entries))
    }
    fn fetch_log(&self, account: AccountId, after: Option<u64>) -> RemoteFuture<'_, Vec<Vec<u8>>> {
        ready(self.fetch_log_sync(account, after))
    }
    fn logs_batch<'a>(&'a self, requests: &'a [LogRequest]) -> RemoteFuture<'a, Vec<LogChunk>> {
        ready(self.logs_batch_sync(requests))
    }
    /// Whether the connection's session is gone (or another account's): a listing needs one and has no
    /// effect. The default `true` would open a new session every cycle, which closes the realtime
    /// socket (a socket lives as long as its session), as a real server's session outlives a cycle.
    fn needs_authentication(&self, account: AccountId, _device: DeviceId) -> bool {
        match self.feeds_list_sync() {
            Ok(listing) => listing
                .first()
                .is_none_or(|personal| personal.feed != FeedId::from_account(account)),
            Err(_) => true,
        }
    }
    fn authenticate(
        &self,
        account: AccountId,
        device: DeviceId,
        signer: AuthSigner,
    ) -> RemoteFuture<'_, ()> {
        let signer = Arc::clone(&signer);
        ready(
            self.authenticate_sync(account, device, &move |message: &[u8]| {
                signer(message).unwrap_or_else(|| refusal(message))
            }),
        )
    }
    fn feeds_create<'a>(&'a self, create: &'a [u8]) -> RemoteFuture<'a, FeedCreated> {
        ready(self.feeds_create_sync(create))
    }
    fn feeds_list(&self) -> RemoteFuture<'_, Vec<FeedListing>> {
        ready(self.feeds_list_sync())
    }
    fn membership_append<'a>(
        &'a self,
        feed: FeedId,
        entries: &'a [Vec<u8>],
    ) -> RemoteFuture<'a, MembershipHead> {
        ready(self.membership_append_sync(feed, entries))
    }
    fn membership_get(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> RemoteFuture<'_, MembershipPage> {
        ready(self.membership_get_sync(feed, after_seq))
    }
    fn append<'a>(&'a self, feed: FeedId, entry: &'a [u8]) -> RemoteFuture<'a, (u64, [u8; 32])> {
        ready(self.append_sync(feed, entry))
    }
    fn changes(&self, feed: FeedId, after: u64, limit: u32) -> RemoteFuture<'_, ChangesPage> {
        ready(self.changes_sync(feed, after, limit))
    }
    fn put_snapshot<'a>(&'a self, feed: FeedId, snapshot: &'a [u8]) -> RemoteFuture<'a, ()> {
        ready(self.put_snapshot_sync(feed, snapshot))
    }
    fn get_snapshot(&self, feed: FeedId, doc: DocId) -> RemoteFuture<'_, Option<Vec<u8>>> {
        ready(self.get_snapshot_sync(feed, doc))
    }
    fn snapshots(&self, feed: FeedId) -> RemoteFuture<'_, SnapshotsPage> {
        ready(self.snapshots_sync(feed))
    }
    fn snapshot_marks(&self, feed: FeedId, after: u64) -> RemoteFuture<'_, MarksPage> {
        ready(self.snapshot_marks_sync(feed, after))
    }
    fn snapshot_dispute(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> RemoteFuture<'_, ()> {
        ready(self.snapshot_dispute_sync(feed, doc, covers_cursor))
    }
    fn put_head<'a>(&'a self, feed: FeedId, head: &'a [u8]) -> RemoteFuture<'a, ()> {
        ready(self.put_head_sync(feed, head))
    }
    fn heads(&self, feed: FeedId) -> RemoteFuture<'_, Vec<Vec<u8>>> {
        ready(self.heads_sync(feed))
    }
    fn put_chunk<'a>(
        &'a self,
        feed: FeedId,
        hash: ChunkId,
        bytes: &'a [u8],
    ) -> RemoteFuture<'a, ()> {
        ready(self.put_chunk_sync(feed, hash, bytes))
    }
    fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, Option<Vec<u8>>> {
        ready(self.get_chunk_sync(feed, hash))
    }
    fn delete_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, ()> {
        ready(self.delete_chunk_sync(feed, hash))
    }
    fn copy_chunk(&self, feed: FeedId, hash: ChunkId) -> RemoteFuture<'_, ()> {
        ready(self.copy_chunk_sync(feed, hash))
    }
    fn invite_open<'a>(
        &'a self,
        invite_id: InviteId,
        sig: &'a Signature,
    ) -> RemoteFuture<'a, InviteOpened> {
        ready(self.invite_open_sync(invite_id, sig))
    }
    fn link_put<'a>(&'a self, link_id: LinkId, put: &'a LinkPut) -> RemoteFuture<'a, u64> {
        ready(self.link_put_sync(link_id, put))
    }
    fn link_delete(&self, link_id: LinkId) -> RemoteFuture<'_, ()> {
        ready(self.link_delete_sync(link_id))
    }
    fn realtime(&self) -> Option<Realtime> {
        // A socket belongs to the session of this connection; without one there is none.
        memory_socket(self.ws_open().ok()?)
    }
    fn pair_create<'a>(&'a self, offer: &'a [u8]) -> RemoteFuture<'a, ([u8; 6], i64)> {
        ready(self.pair_create_sync(offer))
    }
    fn pair_messages(
        &self,
        id: [u8; 6],
        after: u32,
        wait_seconds: u32,
    ) -> RemoteFuture<'_, PairingPage> {
        ready(self.pair_messages_sync(id, after, wait_seconds))
    }
    fn pair_post<'a>(&'a self, id: [u8; 6], role: u8, bytes: &'a [u8]) -> RemoteFuture<'a, ()> {
        ready(self.pair_post_sync(id, role, bytes))
    }
    fn pair_close(&self, id: [u8; 6]) -> RemoteFuture<'_, ()> {
        ready(self.pair_close_sync(id))
    }
    fn recovery_put<'a>(&'a self, id: [u8; 32], bytes: &'a [u8]) -> RemoteFuture<'a, ()> {
        ready(self.recovery_put_sync(id, bytes))
    }
    fn recovery_get(&self, id: [u8; 32]) -> RemoteFuture<'_, Option<Vec<u8>>> {
        ready(self.recovery_get_sync(id))
    }
    fn recovery_delete(&self, id: [u8; 32]) -> RemoteFuture<'_, ()> {
        ready(self.recovery_delete_sync(id))
    }
}
