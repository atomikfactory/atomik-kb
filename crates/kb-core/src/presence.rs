//! Realtime and presence in the core (M13 slice 3, ADR-0050, `docs/security/sharing.md` section 9,
//! `docs/development/milestone-13.md` sections 3.1 and 3.2): the cryptography of presence frames
//! (what the shell's `presence_send` and `presence_events` channel hand through), the inline apply
//! path of entries the WebSocket relays, the debounce hint and `PresenceList`.
//!
//! The core stays sans-IO: the socket, the channel and the timers belong to `kb-net` and the shell,
//! which call these methods under the single-writer lock. Presence is kept in memory only (never
//! stored, never synced) and **correctness never depends on it**: nothing waits for a frame, and a
//! frame that cannot be trusted is dropped silently (a count in a debug log, never an id longer than 8
//! characters, never a name or a cursor).
//!
//! # Sealing (`presence_seal`)
//!
//! A frame is sent for a **shared, live** space the account is a member of, in any role (Viewers
//! included); for a personal space, an archive, a pending or a closed one there is nothing to send
//! (`Ok(None)`). The header is `kb_sync::v2::PresenceHeader`: the feed, this account, this device, the
//! space key epoch in force and a counter that grows by one per frame of this device in this feed
//! (seeded from the clock in milliseconds at the first frame after a start, so a restarted device
//! never falls behind the counter its peers remember). The body is `PresenceBody`: the page's
//! document id (a page id's bytes; all zeros for no page), the state and the editor's update, sealed
//! under `kb_keys::space_keys::presence_key(SK[epoch], feed)` and signed by the device
//! (`kb_sync::v2::seal_presence`). An `update` over `PRESENCE_UPDATE_MAX_BYTES` is `InvalidInput` for
//! `update`; a `page_id` that is not a page of the space is `InvalidInput` for `pageId`; an unknown space
//! is `NotFound`.
//!
//! # Opening (`presence_open`)
//!
//! `Ok(None)` (dropped) when any of these holds, and no other signal: the feed is not a live shared
//! feed of this device; the header's feed is not `feed`; the header's account is not a current member
//! (a removed member's frames stop at once) or the device is not one of that account's devices; the
//! signature does not verify against that device's key; the epoch is not the current epoch of this device's
//! space key (a frame sealed under an older epoch, after a Rekey, or under one this device cannot
//! open); the counter is not above the last accepted from that `(account, device)` in this feed; the
//! frame is this device's own. Otherwise the frame is accepted: the table of who is where is updated
//! (`updated_at` is this device's clock now; **the sender's claims about time and colour are never
//! used**), `Event::PresenceChanged` is emitted when a device appeared, changed page or state, and the
//! [`PresenceMessage`] is returned for the shell's channel. The name is the member's name in the space and the
//! colour its index in join order.
//!
//! # The table
//!
//! One entry per `(feed, account, device)`. [`Core::presence_expire`] (the driver calls it about every five
//! seconds while a socket is open) drops entries older than `PRESENCE_TIMEOUT_SECONDS` on this device's
//! clock and emits `PresenceChanged` for each space that lost one; a member's removal or departure and the
//! space becoming an archive drop the entries concerned at once ([`Core::presence_prune`], called by the
//! membership fold and by the archive transition), and so does a device that a co-member's log, just read,
//! revokes ([`Core::presence_prune_devices`], called when that log changes). A lock needs no hook: it consumes
//! the core, and the table goes with it.
//!
//! # Inline entries (`sync_feed_apply_inline`)
//!
//! The entry goes through the same verifier as the pull path (`Core::sync_feed_apply` with a page of one
//! item: chain, signature, membership at the cursor, role gating, then stored and applied in one
//! transaction); there is no path that applies content without storing it. See `InlineOutcome` for
//! what each answer means. An entry that breaks a rule of the roles or of the membership, with a chain
//! that lines up, is quarantined like a pulled one (`rejected`); a chain that does not line up is `Pull`.
//!
//! # Rules the code keeps beyond the contract
//!
//! - A device that its account's own log **revoked** may not present: its frames are dropped even
//!   before the Rekey a revocation brings.
//! - The counters accepted so far are kept after a device's entry timed out or its member left: a
//!   replay of an old frame never brings an old presence back.
//! - Membership entries are never applied inline (`InlineOutcome::Pull`): the cycle's
//!   compare-and-swap steps mint and confirm them, and the pull path must see them in its order.
//!   A feed that has not finished its first read (a shared feed without `bootstrapped`, a personal feed
//!   at cursor 0), a forked feed and an entry over `INLINE_ENTRY_MAX_BYTES` are `Pull` as well.
//! - A signature that does not verify is `Pull` too: the pull reports it in its usual way (the feed
//!   stops with `Crypto`), and nothing was stored.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use kb_crypto::VerifyingKey;
use kb_keys::space_keys::presence_key;
use kb_storage::{FeedQueries, SyncFeedRow};
use kb_sync::ChangesPage;
use kb_sync::v2::{
    InlineEntry, InlineOutcome, ItemKind, PresenceBody, PresenceHeader, SealedPresence,
    chain_genesis, chain_next, feed_kind, open_presence, peek_presence, seal_presence,
};
use kb_types::sharing::{
    INLINE_ENTRY_MAX_BYTES, PRESENCE_MAX_FRAME_BYTES, PRESENCE_TIMEOUT_SECONDS,
    PRESENCE_UPDATE_MAX_BYTES, PUSH_DEBOUNCE_PRESENCE_MS,
};
use kb_types::sync::{FeedKind, KEYRING_DOC_ID, PUSH_DEBOUNCE_MS};
use kb_types::{
    AccountId, ApiError, DeviceId, DocId, ErrorCode, Event, FeedId, PageId, PresenceInfo,
    PresenceMessage, PresenceState, Request, Response, SpaceId,
};

use crate::{Core, map_storage_error};

/// One device of a member in a feed: what the table is keyed by.
type DeviceKey = (FeedId, AccountId, DeviceId);

/// What the table holds about one remote device.
#[derive(Debug, Clone, Copy)]
struct PresenceEntry {
    /// The page the device is in (a page of the feed's space), if any.
    page_id: Option<PageId>,
    state: PresenceState,
    /// This device's clock when the latest frame was accepted.
    updated_at: i64,
}

/// The presence state of the core: who is where, the counters accepted and sent. Never stored.
#[derive(Debug, Default)]
pub(crate) struct PresenceRuntime {
    entries: BTreeMap<DeviceKey, PresenceEntry>,
    /// The highest counter accepted from each remote device (kept after its entry is gone).
    accepted: HashMap<DeviceKey, u64>,
    /// The counter of this device's latest frame in each feed.
    sent: HashMap<FeedId, u64>,
    /// Why the latest dropped frame was dropped (tests assert the reason, not just the drop).
    #[cfg(test)]
    pub(crate) last_drop: Option<Dropped>,
}

#[cfg(test)]
impl PresenceRuntime {
    /// How many devices the table holds, timed out or not.
    pub(crate) fn held(&self) -> usize {
        self.entries.len()
    }
}

/// Whether an entry accepted at `updated_at` is gone at `now` (both Unix ms on this device's clock).
fn timed_out(now: i64, updated_at: i64) -> bool {
    let timeout = i64::try_from(PRESENCE_TIMEOUT_SECONDS)
        .unwrap_or(i64::MAX)
        .saturating_mul(1000);
    now.saturating_sub(updated_at) >= timeout
}

/// The reasons a frame is dropped (debug log only).
type Dropped = &'static str;

/// A frame that passed every check, before the table takes it.
struct Accepted {
    space_id: SpaceId,
    header: PresenceHeader,
    body: PresenceBody,
    color: u8,
}

impl Core {
    /// Routes `PresenceList`.
    pub(crate) fn dispatch_presence(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::PresenceList { space_id } => {
                Ok(Response::Presence(self.presence_list(space_id)?))
            }
            other => Err(ApiError::internal(format!(
                "{} is not a presence request",
                other.kind()
            ))),
        }
    }

    /// The shared feed of a space when it is live (not an archive): where presence flows.
    fn presence_feed_of(&mut self, space_id: SpaceId) -> Result<Option<FeedId>, ApiError> {
        self.shared_load()?;
        let Some(row) = self.shared_feed_of(space_id)? else {
            return Ok(None);
        };
        Ok(self.feed_is_live(row.feed)?.then_some(row.feed))
    }

    /// The devices of other members (this account's other devices too, never this one) with a
    /// frame accepted in the last `PRESENCE_TIMEOUT_SECONDS`, for a shared, live space; empty for a
    /// personal space and an archive. `NotFound` for the space.
    pub(crate) fn presence_list(
        &mut self,
        space_id: SpaceId,
    ) -> Result<Vec<PresenceInfo>, ApiError> {
        self.structure(space_id)?;
        let Some(feed) = self.presence_feed_of(space_id)? else {
            return Ok(Vec::new());
        };
        if self.shared.presence.entries.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.clock.now_ms();
        let members: Vec<AccountId> = self
            .shared
            .timelines
            .get(&feed)
            .map(|t| t.state().members.iter().map(|m| m.account).collect())
            .unwrap_or_default();
        let mut out = Vec::new();
        for ((entry_feed, account, device), entry) in &self.shared.presence.entries {
            if *entry_feed != feed || timed_out(now, entry.updated_at) {
                continue;
            }
            // The index in join order doubles as the membership check: a removed member is none.
            let Some(color) = members.iter().position(|member| member == account) else {
                continue;
            };
            out.push(PresenceInfo {
                account_id: *account,
                device_id: *device,
                name: self.member_name(feed, *account),
                color: u8::try_from(color).unwrap_or(u8::MAX),
                page_id: entry.page_id,
                state: entry.state,
                updated_at: entry.updated_at,
            });
        }
        out.sort_by_key(|info| (info.color, info.device_id));
        Ok(out)
    }

    /// Seals this device's presence in a space for the socket (see the module documentation): the feed and
    /// the envelope `kb_sync::v2::seal_presence` made. `Ok(None)`: nothing to send (a personal space, an
    /// archive, a space whose key this device does not hold yet). `page_id` is `None` for "in no page"
    /// (an idle device); `update` is the editor's `EphemeralStore` update, opaque here.
    pub fn presence_seal(
        &mut self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> Result<Option<SealedPresence>, ApiError> {
        self.structure(space_id)?;
        if update.len() > PRESENCE_UPDATE_MAX_BYTES {
            return Err(ApiError::invalid(
                "update",
                format!("a presence update is at most {PRESENCE_UPDATE_MAX_BYTES} bytes"),
            ));
        }
        let doc_id = match page_id {
            Some(page) => {
                if self.space_of_page(page).ok() != Some(space_id) {
                    return Err(ApiError::invalid("pageId", "the page is not in this space"));
                }
                DocId::for_page(page)
            }
            None => KEYRING_DOC_ID,
        };
        let Some(feed) = self.presence_feed_of(space_id)? else {
            return Ok(None);
        };
        let Some((me, device)) = self.sync_identity()? else {
            return Ok(None);
        };
        // The folded membership decides: an entry this device minted and the server has not taken
        // yet makes no member (a pending share has nobody to talk to).
        let Some((member, epoch, closed)) = self.shared.timelines.get(&feed).map(|t| {
            (
                t.state().role_of(me).is_some(),
                t.state().epoch,
                t.state().closed.is_some(),
            )
        }) else {
            return Ok(None);
        };
        if !member || closed {
            return Ok(None);
        }
        let Some(space_key) = self.feed_sk(feed, epoch)? else {
            return Ok(None);
        };
        let key = presence_key(&space_key, feed);
        let now = self.clock.now_ms();
        let counter = match self.shared.presence.sent.get(&feed) {
            Some(last) => last.saturating_add(1),
            None => u64::try_from(now).unwrap_or(0),
        };
        let header = PresenceHeader {
            feed,
            account_id: me,
            device_id: device,
            epoch,
            counter,
        };
        let body = PresenceBody {
            doc_id,
            state,
            update: update.to_vec(),
        };
        let envelope = seal_presence(&header, &body, &key, self.signer(), self.rng.as_ref());
        if envelope.is_empty() {
            return Err(ApiError::internal("the presence frame could not be sealed"));
        }
        self.shared.presence.sent.insert(feed, counter);
        Ok(Some(SealedPresence { feed, envelope }))
    }

    /// Opens a presence frame the server relayed for `feed` (the envelope of the binary frame `2`):
    /// `Ok(Some(message))` for an accepted frame, `Ok(None)` for one that is dropped (see the module
    /// documentation for every reason). An error is a failure of the core itself, never a bad frame.
    pub fn presence_open(
        &mut self,
        feed: FeedId,
        envelope: &[u8],
    ) -> Result<Option<PresenceMessage>, ApiError> {
        let accepted = match self.presence_judge(feed, envelope)? {
            Ok(accepted) => accepted,
            Err(reason) => {
                tracing::debug!(feed = %feed.short_hex(), reason, "a presence frame was dropped");
                #[cfg(test)]
                {
                    self.shared.presence.last_drop = Some(reason);
                }
                return Ok(None);
            }
        };
        let Accepted {
            space_id,
            header,
            body,
            color,
        } = accepted;
        let key = (feed, header.account_id, header.device_id);
        self.shared.presence.accepted.insert(key, header.counter);
        let page_id = if body.doc_id == KEYRING_DOC_ID {
            None
        } else {
            // A document that is not a page of this space is no page (the contract's `pageId`).
            let page = PageId::from_bytes(body.doc_id.to_bytes());
            (self.space_of_page(page).ok() == Some(space_id)).then_some(page)
        };
        let now = self.clock.now_ms();
        let entry = PresenceEntry {
            page_id,
            state: body.state,
            updated_at: now,
        };
        let changed = match self.shared.presence.entries.insert(key, entry) {
            None => true,
            Some(old) => {
                old.page_id != page_id || old.state != body.state || timed_out(now, old.updated_at)
            }
        };
        if changed {
            self.push_event_once(Event::PresenceChanged { space_id });
        }
        Ok(Some(PresenceMessage {
            info: PresenceInfo {
                account_id: header.account_id,
                device_id: header.device_id,
                name: self.member_name(feed, header.account_id),
                color,
                page_id,
                state: body.state,
                updated_at: now,
            },
            update: body.update,
        }))
    }

    /// Every check of a presence frame, in the order of the module documentation. The outer error is
    /// the core failing, the inner one the reason the frame is dropped.
    fn presence_judge(
        &mut self,
        feed: FeedId,
        envelope: &[u8],
    ) -> Result<Result<Accepted, Dropped>, ApiError> {
        if envelope.len() > PRESENCE_MAX_FRAME_BYTES {
            return Ok(Err("oversize"));
        }
        self.shared_load()?;
        let Some(row) = self.db.sync_feed_get(feed).map_err(map_storage_error)? else {
            return Ok(Err("unknown feed"));
        };
        let Some(space_id) = row.space_id.filter(|_| row.kind == FeedKind::Shared) else {
            return Ok(Err("not a shared feed"));
        };
        if !self.feed_is_live(feed)? {
            return Ok(Err("not a live feed"));
        }
        let Ok(header) = peek_presence(envelope) else {
            return Ok(Err("malformed"));
        };
        if header.feed != feed {
            return Ok(Err("another feed"));
        }
        let Some((me, my_device)) = self.sync_identity()? else {
            return Ok(Err("no identity"));
        };
        let Some((color, epoch)) = self.shared.timelines.get(&feed).and_then(|t| {
            let state = t.state();
            let color = state
                .members
                .iter()
                .position(|member| member.account == header.account_id)?;
            Some((u8::try_from(color).unwrap_or(u8::MAX), state.epoch))
        }) else {
            return Ok(Err("not a current member"));
        };
        if header.account_id == me && header.device_id == my_device {
            return Ok(Err("own frame"));
        }
        if header.epoch != epoch {
            return Ok(Err("not the current epoch"));
        }
        let device_key = (feed, header.account_id, header.device_id);
        if self
            .shared
            .presence
            .accepted
            .get(&device_key)
            .is_some_and(|last| header.counter <= *last)
        {
            return Ok(Err("counter not above the last accepted"));
        }
        let Some(space_key) = self.feed_sk(feed, epoch)? else {
            return Ok(Err("no key for the epoch"));
        };
        let Some(verifier) = self.presence_device_key(header.account_id, header.device_id) else {
            return Ok(Err("not a device of the member"));
        };
        let Ok((opened, body)) =
            open_presence(envelope, &presence_key(&space_key, feed), &verifier)
        else {
            return Ok(Err("signature or sealing"));
        };
        if opened != header {
            return Ok(Err("header differs"));
        }
        Ok(Ok(Accepted {
            space_id,
            header,
            body,
            color,
        }))
    }

    /// The signing key of a device that its account's log lists and has not revoked. A log that is
    /// not held, or that does not list the device yet, is asked for again (a member's new device
    /// starts sending before this device has read its log entry).
    fn presence_device_key(
        &mut self,
        account: AccountId,
        device: DeviceId,
    ) -> Option<VerifyingKey> {
        let (me, _) = self.sync_identity().ok().flatten()?;
        let log = if account == me {
            self.security.log.as_ref()
        } else {
            self.shared.logs.get(&account)
        };
        let Some(log) = log else {
            self.shared.wanted_logs.insert(account);
            return None;
        };
        let last = u64::try_from(log.entries().len().checked_sub(1)?).ok()?;
        if let Some((_, keys)) = log
            .active_devices_at(last)?
            .into_iter()
            .find(|(id, _)| *id == device)
        {
            return Some(keys.signing);
        }
        if log.device_keys(device).is_none() && !self.shared.fresh_logs.contains(&account) {
            self.shared.wanted_logs.insert(account);
        }
        None
    }

    /// Drops the entries of the presence table that timed out (`PRESENCE_TIMEOUT_SECONDS`) and emits
    /// `PresenceChanged` for each space that lost one. Cheap when nothing is held.
    pub fn presence_expire(&mut self) -> Result<(), ApiError> {
        if self.shared.presence.entries.is_empty() {
            return Ok(());
        }
        let now = self.clock.now_ms();
        let mut lost: BTreeSet<FeedId> = BTreeSet::new();
        self.shared.presence.entries.retain(|(feed, _, _), entry| {
            let gone = timed_out(now, entry.updated_at);
            if gone {
                lost.insert(*feed);
            }
            !gone
        });
        for feed in lost {
            if let Some(space_id) = self.feed_space(feed)? {
                self.push_event_once(Event::PresenceChanged { space_id });
            }
        }
        Ok(())
    }

    /// Drops what the table holds for a feed that is no longer true: every entry of an archive, and
    /// the entries of accounts that are not members of the folded state any more (a removal, a
    /// `Leave`). Emits `PresenceChanged` when it dropped any. Called by the membership fold and the
    /// archive transition.
    pub(crate) fn presence_prune(&mut self, feed: FeedId) -> Result<(), ApiError> {
        if self.shared.presence.entries.is_empty() {
            return Ok(());
        }
        let live = self.feed_is_live(feed)?;
        let members: BTreeSet<AccountId> = self
            .shared
            .timelines
            .get(&feed)
            .map(|t| t.state().members.iter().map(|m| m.account).collect())
            .unwrap_or_default();
        let mut dropped = false;
        self.shared
            .presence
            .entries
            .retain(|(entry_feed, account, _), _| {
                if *entry_feed != feed {
                    return true;
                }
                let keep = live && members.contains(account);
                dropped |= !keep;
                keep
            });
        if dropped && let Some(space_id) = self.feed_space(feed)? {
            self.push_event_once(Event::PresenceChanged { space_id });
        }
        Ok(())
    }

    /// Drops the entries of a co-member's devices that its device log, just read, no longer lists as active: a
    /// revocation takes the device off the others' lists at once, not at the timeout (its frames were already
    /// refused, `presence_device_key`). Emits `PresenceChanged` for each space that lost one. Called when a
    /// co-member's log changes (`sync_foreign_log_received`).
    pub(crate) fn presence_prune_devices(&mut self, account: AccountId) -> Result<(), ApiError> {
        if self.shared.presence.entries.is_empty() {
            return Ok(());
        }
        let Some(log) = self.shared.logs.get(&account) else {
            return Ok(());
        };
        let Some(last) = log
            .entries()
            .len()
            .checked_sub(1)
            .and_then(|n| u64::try_from(n).ok())
        else {
            return Ok(());
        };
        let Some(active) = log.active_devices_at(last) else {
            return Ok(());
        };
        let active: BTreeSet<DeviceId> = active.into_iter().map(|(id, _)| id).collect();
        let mut lost: BTreeSet<FeedId> = BTreeSet::new();
        self.shared
            .presence
            .entries
            .retain(|(feed, entry_account, device), _| {
                let gone = *entry_account == account && !active.contains(device);
                if gone {
                    lost.insert(*feed);
                }
                !gone
            });
        for feed in lost {
            if let Some(space_id) = self.feed_space(feed)? {
                self.push_event_once(Event::PresenceChanged { space_id });
            }
        }
        Ok(())
    }

    /// The push debounce the driver should use now, in milliseconds:
    /// `kb_types::sharing::PUSH_DEBOUNCE_PRESENCE_MS` while some live shared feed has a remote presence
    /// (an entry not timed out), else `kb_types::sync::PUSH_DEBOUNCE_MS`.
    pub fn sync_push_debounce_ms(&mut self) -> Result<u64, ApiError> {
        if self.shared.presence.entries.is_empty() {
            return Ok(PUSH_DEBOUNCE_MS);
        }
        let now = self.clock.now_ms();
        let feeds: BTreeSet<FeedId> = self
            .shared
            .presence
            .entries
            .iter()
            .filter(|(_, entry)| !timed_out(now, entry.updated_at))
            .map(|((feed, _, _), _)| *feed)
            .collect();
        for feed in feeds {
            if self.feed_is_live(feed)? {
                return Ok(PUSH_DEBOUNCE_PRESENCE_MS);
            }
        }
        Ok(PUSH_DEBOUNCE_MS)
    }

    /// Offers one entry the WebSocket relayed inline to the feed's apply path (see the module
    /// documentation and `InlineOutcome`).
    pub fn sync_feed_apply_inline(
        &mut self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError> {
        self.shared_load()?;
        let Some(row) = self.db.sync_feed_get(feed).map_err(map_storage_error)? else {
            return Ok(InlineOutcome::Pull);
        };
        if inline.feed != feed || !self.inline_feed_ready(&row)? {
            return Ok(InlineOutcome::Pull);
        }
        if inline.cursor <= row.cursor {
            return Ok(InlineOutcome::Known);
        }
        if inline.prev_cursor != row.cursor
            || inline.entry.len() > INLINE_ENTRY_MAX_BYTES
            || !matches!(
                feed_kind(&inline.entry),
                Ok(ItemKind::Entry | ItemKind::Keyring)
            )
        {
            return Ok(InlineOutcome::Pull);
        }
        let Some(epoch) = self.server_epoch()? else {
            return Ok(InlineOutcome::Pull);
        };
        // The chain is judged here, before the apply path could treat a mismatch as a gap or a fork:
        // an inline entry is a hint that the pull confirms, never a reason to doubt the server.
        let before = match (row.chain_hash, row.cursor) {
            (Some(hash), _) => hash,
            (None, 0) => chain_genesis(&epoch),
            (None, _) => return Ok(InlineOutcome::Pull),
        };
        if chain_next(&before, &inline.entry) != inline.chain_after {
            return Ok(InlineOutcome::Pull);
        }
        let page = ChangesPage {
            server_epoch: epoch,
            changes: vec![(inline.cursor, inline.entry.clone())],
            chain_hash_after: inline.chain_after,
            more: false,
        };
        match self.sync_feed_apply(feed, &page) {
            Ok(report) if report.new_cursor >= inline.cursor => Ok(InlineOutcome::Applied(report)),
            // Stalled for a device log: nothing was stored, the pull fetches the log first.
            Ok(_) => Ok(InlineOutcome::Pull),
            // A signature that does not verify: the pull reports it the way it always has.
            Err(error) if error.code == ErrorCode::Crypto => Ok(InlineOutcome::Pull),
            Err(error) => Err(error),
        }
    }

    /// Whether the feed may take an entry ahead of a pull: it is live, not forked and its first full
    /// read is over (what a pull would still bring before an entry in the middle of the feed).
    fn inline_feed_ready(&mut self, row: &SyncFeedRow) -> Result<bool, ApiError> {
        if row.fork {
            return Ok(false);
        }
        match row.kind {
            FeedKind::Personal => Ok(row.cursor > 0),
            FeedKind::Shared => Ok(self.feed_is_live(row.feed)?
                && self.fcfg(row.feed, "bootstrapped")?.as_deref() == Some("1")),
        }
    }
}
