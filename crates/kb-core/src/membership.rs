//! The membership of shared spaces in the core (M13 slice 1, ADR-0043,
//! `docs/development/milestone-13.md` section 1.5): membership entries arrive as feed items, are folded
//! in cursor order (`kb_keys::membership::Timeline`, incrementally), and produce the derived events
//! `SpaceMembersChanged`, `SpaceRoleChanged`, `SpaceRemoved` and `MemberDeviceViaKit`; role lookups
//! serve the apply path and the role gate; the host methods post the entries this device minted and
//! report what the server answered.
//!
//! What lives where:
//!
//! - this file: the runtime state (timelines, the co-members' device logs), loading it at open,
//!   the lookups (`role_in_space`, `membership_state`), the host methods of `kb-net`'s `SyncHost`
//!   (v2) that concern membership and logs, and the archive transition;
//! - `membership/ingest.rs`: folding one incoming entry, opening the boxes it carries (`Space(epoch)`
//!   records of the personal keyring), the metadata of the private parts, the events;
//! - `membership/mint.rs`: minting entries (`Create`, `Rekey`, `Grant`, and the generic
//!   [`Core::membership_mint`] the request side uses), the automatic Rekey and Grant;
//! - `membership/info.rs`: [`Core::members_info`], the panel's view.
//!
//! Rules the code implements (see the milestone): a device that finds its account removed or demoted
//! while it holds unsent changes runs fork-out (`forkout.rs`); a `Join` whose joiner lacks a box for
//! the current epoch gets a `Grant` from any member device (compare-and-swap keeps one); a `Rekey`
//! that boxes an account's device list older than a revocation in that account's own log makes the
//! account's devices mint a fresh `Rekey` (self-policing); a `Rekey` is built only when every
//! member's log is known as of `member_heads`.
//!
//! **Pending entries.** A membership entry this device minted and the server has not taken is stored
//! in `membership_entries` with the cursor 0. `sync_membership_posted` records the head the server
//! took (`f.<feed>.posted`); the entry comes back in the feed pull, which fills its cursor in and
//! folds it. Until then it is not part of the timeline (cursors order the fold); the *tentative*
//! timeline (the folded one plus the pending entries) validates what the next mint builds on.

mod info;
mod ingest;
mod keys;
mod mint;
mod reintent;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use kb_keys::log::{DeviceLog, LogEntry, LogEntryBody};
use kb_keys::membership::{LogSource, MembershipEntry, MembershipState, Timeline};
use kb_storage::{FeedQueries, ForeignLogRow, KeyringRow, SyncFeedRow, SyncQueries};
use kb_sync::v2::{LogRequest, MembershipHead, MembershipPage};
use kb_types::sharing::SpaceSettings;
use kb_types::sync::FeedKind;
use kb_types::{AccountId, ApiError, ArchiveReason, Event, FeedId, MemberRole, SpaceId};

use crate::{Core, map_storage_error};

pub(crate) use ingest::{Ingest, entry_accounts};

/// Per-feed facts derived from the private parts and the bodies of the folded entries.
#[derive(Debug, Clone, Default)]
pub(crate) struct FeedMeta {
    /// The space name of the `Create`.
    pub(crate) space_name: Option<String>,
    /// The latest settings.
    pub(crate) settings: SpaceSettings,
    /// The names members gave (`Create`, `Join`, `Profile`).
    pub(crate) names: BTreeMap<AccountId, String>,
    /// The last name of each member that was removed (a comment's author who is gone keeps the name
    /// the space last knew them by, ADR-0048).
    pub(crate) former_names: BTreeMap<AccountId, String>,
    /// The highest epoch a member is known to hold a key for (a `Create` or `Rekey` that boxed it, a
    /// `Grant`, the epoch of the bundle it joined with).
    pub(crate) keyed: BTreeMap<AccountId, u32>,
    /// `sk_commitment` of each epoch.
    pub(crate) commitments: BTreeMap<u32, [u8; 32]>,
    /// `prev_wrapped` of the entry that began each epoch (the key of `epoch - 1` under the key of
    /// `epoch`), for the backward key chain.
    pub(crate) prev_wrapped: BTreeMap<u32, kb_crypto::WrappedKey>,
    /// The `at` of the `Close` entry.
    pub(crate) closed_at: Option<i64>,
    /// Where each member's own device log stood when it joined (or created the space): revocations
    /// before that never concerned the space key.
    pub(crate) joined_log: BTreeMap<AccountId, u64>,
    /// Boxes that did not open or did not match their commitment (diagnostics only).
    pub(crate) key_mismatches: u32,
}

/// What the core keeps in memory about shared feeds; the durable parts are in the database.
#[derive(Default)]
pub(crate) struct SharedRuntime {
    /// `shared_load` ran.
    pub(crate) loaded: bool,
    /// Counts the changes of who the members are and of their names (every `PeopleChanged` and
    /// `SpaceMembersChanged`, the replay of the membership entries, the load): the structure documents
    /// rebuild the directory their formulas read (`name(person)`) when theirs is older.
    pub(crate) names_rev: u64,
    /// The fold of each shared feed's found membership entries.
    pub(crate) timelines: HashMap<FeedId, Timeline>,
    /// Derived facts of each shared feed.
    pub(crate) metas: HashMap<FeedId, FeedMeta>,
    /// The verified device logs of co-members (this account's own log is `security.log`).
    pub(crate) logs: HashMap<AccountId, DeviceLog>,
    /// Accounts whose log an item or an entry needs and the core does not hold.
    pub(crate) wanted_logs: BTreeSet<AccountId>,
    /// Keyring rows that went into the last outbox keyring entry of a feed.
    pub(crate) keyring_in_flight: HashMap<FeedId, Vec<KeyringRow>>,
    /// The cursor at which the driver last refreshed the snapshots of a feed because of a gap.
    pub(crate) snapshots_checked_at: HashMap<FeedId, u64>,
    /// Feeds whose account was demoted: fork-out runs at the end of the pull.
    pub(crate) forkout_due: BTreeSet<FeedId>,
    /// Feeds whose membership fold stalled for a log that is not here.
    pub(crate) stalled: BTreeSet<FeedId>,
    /// Accounts whose device log was fetched since the last page of a feed was applied in full: an
    /// item from a device such a log does not list is not a race with a new device any more.
    pub(crate) fresh_logs: BTreeSet<AccountId>,
    /// The feed whose membership log an invitee adopts at the moment (an earlier membership of this
    /// account in it must not archive the space or raise events).
    pub(crate) adopting: Option<FeedId>,
    /// Who is where in the shared spaces (M13 slice 3): in memory only, see `presence.rs`.
    pub(crate) presence: crate::presence::PresenceRuntime,
    /// View links (M13 slice 4): the uploads under way, the orphans seen and the expiries announced, in
    /// memory only (see `links.rs`).
    pub(crate) links: crate::links::LinksRuntime,
    /// Tests push a feed's outbox whatever the role says (a hostile or out-of-date device).
    #[cfg(test)]
    pub(crate) force_push: bool,
}

/// The device logs the fold may consult: this account's own log and the co-members' copies.
pub(crate) struct LogsView<'a> {
    pub(crate) me: AccountId,
    pub(crate) own: Option<&'a DeviceLog>,
    pub(crate) foreign: &'a HashMap<AccountId, DeviceLog>,
}

impl LogSource for LogsView<'_> {
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog> {
        if account == self.me {
            self.own
        } else {
            self.foreign.get(&account)
        }
    }
}

/// The highest `seq` of a `DeviceRevoked` entry of a log: a `Rekey` whose member head is below it
/// may still box the revoked device.
pub(crate) fn latest_revocation_seq(log: &DeviceLog) -> Option<u64> {
    log.entries()
        .iter()
        .rev()
        .find(|entry| matches!(entry.body, LogEntryBody::DeviceRevoked { .. }))
        .map(|entry| entry.seq)
}

/// The key under which a feed keeps one small setting in `sync_config`.
pub(crate) fn feed_key(feed: FeedId, name: &str) -> String {
    format!("f.{}.{name}", feed.to_hex())
}

impl Core {
    // ----- small per-feed settings (sync_config) --------------------------------------------

    pub(crate) fn fcfg(&self, feed: FeedId, name: &str) -> Result<Option<String>, ApiError> {
        self.db
            .sync_config_get(&feed_key(feed, name))
            .map_err(map_storage_error)
    }

    pub(crate) fn fcfg_set(&self, feed: FeedId, name: &str, value: &str) -> Result<(), ApiError> {
        self.db
            .sync_config_set(&feed_key(feed, name), value)
            .map_err(map_storage_error)
    }

    pub(crate) fn fcfg_del(&self, feed: FeedId, name: &str) -> Result<(), ApiError> {
        self.db
            .sync_config_delete(&feed_key(feed, name))
            .map_err(map_storage_error)
    }

    pub(crate) fn fcfg_u64(&self, feed: FeedId, name: &str) -> Result<Option<u64>, ApiError> {
        Ok(self.fcfg(feed, name)?.and_then(|v| v.parse().ok()))
    }

    /// Pushes an event unless the same one is queued already (the panel re-fetches once).
    pub(crate) fn push_event_once(&mut self, event: Event) {
        if matches!(
            event,
            Event::PeopleChanged | Event::SpaceMembersChanged { .. }
        ) {
            self.shared.names_rev += 1;
        }
        if !self.events.contains(&event) {
            self.events.push(event);
        }
    }

    // ----- loading --------------------------------------------------------------------------

    /// Loads the co-members' device logs and replays the membership entries of every shared feed
    /// (idempotent; the first call after the core opened does the work).
    pub(crate) fn shared_load(&mut self) -> Result<(), ApiError> {
        if self.shared.loaded {
            return Ok(());
        }
        self.shared.loaded = true;
        self.shared.names_rev += 1;
        for account in self.db.foreign_log_accounts().map_err(map_storage_error)? {
            let rows = self
                .db
                .foreign_log_after(account, None)
                .map_err(map_storage_error)?;
            let mut entries = Vec::with_capacity(rows.len());
            let mut broken = false;
            for row in rows {
                match LogEntry::decode(&row.entry) {
                    Ok(entry) => entries.push(entry),
                    Err(_) => {
                        broken = true;
                        break;
                    }
                }
            }
            if broken {
                tracing::warn!("a stored co-member device log does not decode; ignored");
                continue;
            }
            match DeviceLog::from_entries(entries) {
                Ok(log)
                    if log.entries().first().map(kb_keys::log::account_id_of) == Some(account) =>
                {
                    self.shared.logs.insert(account, log);
                }
                _ => tracing::warn!("a stored co-member device log does not verify; ignored"),
            }
        }
        let feeds: Vec<FeedId> = self
            .db
            .sync_feed_list()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|row| row.kind == FeedKind::Shared)
            .map(|row| row.feed)
            .collect();
        for feed in feeds {
            self.replay_membership(feed)?;
        }
        Ok(())
    }

    /// Rebuilds a feed's timeline and metadata from the stored entries that were found in the feed.
    pub(crate) fn replay_membership(&mut self, feed: FeedId) -> Result<(), ApiError> {
        self.shared.timelines.remove(&feed);
        self.shared.metas.remove(&feed);
        let rows = self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?;
        for row in rows {
            if row.cursor == 0 {
                break;
            }
            let Ok(entry) = MembershipEntry::decode(&row.bytes) else {
                tracing::warn!(feed = %feed.short_hex(), "a stored membership entry does not decode");
                break;
            };
            match self.fold_entry(feed, row.cursor, &entry) {
                Ok(applied) => {
                    self.membership_meta(feed, &entry, &applied)?;
                }
                Err(error) => {
                    tracing::warn!(feed = %feed.short_hex(), rule = ?error.rule, "the stored membership log stops folding");
                    break;
                }
            }
        }
        Ok(())
    }

    // ----- lookups --------------------------------------------------------------------------

    /// This account's id (`InvalidInput` before the kit exists).
    pub(crate) fn me(&self) -> Result<AccountId, ApiError> {
        self.sync_identity()?
            .map(|(account, _)| account)
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before sharing"))
    }

    /// The row of the shared feed of a space, if the space is shared (an archive too).
    pub(crate) fn shared_feed_of(
        &self,
        space_id: SpaceId,
    ) -> Result<Option<SyncFeedRow>, ApiError> {
        Ok(self
            .db
            .sync_feed_list()
            .map_err(map_storage_error)?
            .into_iter()
            .find(|row| row.kind == FeedKind::Shared && row.space_id == Some(space_id)))
    }

    /// Whether the feed is an archive (this account was removed, it left, or an Owner closed it).
    pub(crate) fn feed_archived(&self, feed: FeedId) -> Result<Option<ArchiveReason>, ApiError> {
        Ok(match self.fcfg(feed, "archived")?.as_deref() {
            Some("removed") => Some(ArchiveReason::Removed),
            Some("closed") => Some(ArchiveReason::Closed),
            _ => None,
        })
    }

    /// The timeline of a feed (the folded entries only), loaded on first use.
    pub(crate) fn feed_timeline(&mut self, feed: FeedId) -> Result<Option<&Timeline>, ApiError> {
        self.shared_load()?;
        Ok(self.shared.timelines.get(&feed))
    }

    /// The folded membership state of a shared feed (a clone of the cache, rebuilt from
    /// `membership_entries` when it is missing); the empty state of a feed with no entry found yet.
    #[allow(dead_code)]
    pub(crate) fn membership_state(&mut self, feed: FeedId) -> Result<MembershipState, ApiError> {
        self.shared_load()?;
        Ok(self
            .shared
            .timelines
            .get(&feed)
            .map_or_else(|| MembershipState::empty(feed), |t| t.state().clone()))
    }

    /// The state after the folded entries and the ones this device minted and the server has not
    /// confirmed (`None` when nothing is known of the feed).
    pub(crate) fn tentative_state(
        &mut self,
        feed: FeedId,
    ) -> Result<Option<MembershipState>, ApiError> {
        Ok(self.tentative_timeline(feed)?.map(|t| t.state().clone()))
    }

    /// This account's role in a space: `None` for a personal space, `Some` for a shared space and for
    /// an archive (the role it had).
    #[allow(dead_code)]
    pub(crate) fn role_in_space(
        &mut self,
        space_id: SpaceId,
    ) -> Result<Option<MemberRole>, ApiError> {
        let Some(row) = self.shared_feed_of(space_id)? else {
            return Ok(None);
        };
        let me = self.me()?;
        if let Some(state) = self.tentative_state(row.feed)?
            && let Some(role) = state.role_of(me)
        {
            return Ok(Some(role));
        }
        // An archive keeps the role it had; a feed whose timeline is rebuilding after a server reset
        // keeps the last role it knew.
        let cached = self
            .fcfg(row.feed, "role")?
            .and_then(|v| v.parse::<u8>().ok())
            .and_then(MemberRole::from_byte);
        Ok(Some(cached.unwrap_or(MemberRole::Viewer)))
    }

    /// Whether the space's feed is syncing (shared, not an archive).
    pub(crate) fn feed_is_live(&self, feed: FeedId) -> Result<bool, ApiError> {
        Ok(self.feed_archived(feed)?.is_none())
    }

    /// The accounts that are members of a live shared feed (from the folded timeline).
    pub(crate) fn members_of(&mut self, feed: FeedId) -> Result<Vec<AccountId>, ApiError> {
        Ok(self
            .feed_timeline(feed)?
            .map(|t| t.state().members.iter().map(|m| m.account).collect())
            .unwrap_or_default())
    }

    // ----- host methods: co-members' logs ---------------------------------------------------

    /// The device logs of co-members the core needs and does not have up to date, as `(account,
    /// last seq held)`: members of shared feeds, the signers of items it could not judge, the members
    /// a Rekey must box for (`member_heads`). The driver fetches them with `logs_batch` before it works
    /// the shared feeds.
    pub fn sync_needed_logs(&mut self) -> Result<Vec<LogRequest>, ApiError> {
        self.shared_load()?;
        let Some((me, _)) = self.sync_identity()? else {
            return Ok(Vec::new());
        };
        let mut accounts: BTreeSet<AccountId> = self.shared.wanted_logs.clone();
        let feeds: Vec<FeedId> = self
            .db
            .sync_feed_list()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|row| row.kind == FeedKind::Shared)
            .map(|row| row.feed)
            .collect();
        for feed in feeds {
            if !self.feed_is_live(feed)? {
                continue;
            }
            accounts.extend(self.members_of(feed)?);
        }
        accounts.remove(&me);
        Ok(accounts
            .into_iter()
            .map(|account| LogRequest {
                account,
                after: self
                    .shared
                    .logs
                    .get(&account)
                    .and_then(|log| log.entries().last())
                    .map(|entry| entry.seq),
            })
            .collect())
    }

    /// A co-member's device log as the server holds it (entries after the `after` that was asked):
    /// the core replays and verifies them with what it has (`DeviceLog::append`) and stores them in
    /// `foreign_device_logs`. `true` when something was added; a log that does not verify is `Crypto`.
    /// Emits `SpaceMembersChanged` when a member's device count or a kit recovery changed
    /// (`MemberDeviceViaKit`).
    pub fn sync_foreign_log_received(
        &mut self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.shared_load()?;
        if self.sync_identity()?.map(|(me, _)| me) == Some(account) {
            // This account's own log is the personal feed's business (`sync_log_received`).
            return Ok(false);
        }
        let mut decoded = Vec::with_capacity(entries.len());
        for bytes in entries {
            decoded.push(
                LogEntry::decode(bytes)
                    .map_err(|_| ApiError::crypto("a co-member's device log does not decode"))?,
            );
        }
        self.shared.fresh_logs.insert(account);
        self.shared.wanted_logs.remove(&account);
        self.shared.stalled.clear();
        let held = self.shared.logs.get(&account).cloned();
        let mut log = held.clone().unwrap_or_default();
        let mut added: Vec<LogEntry> = Vec::new();
        for entry in decoded {
            let seq = usize::try_from(entry.seq).unwrap_or(usize::MAX);
            if let Some(known) = log.entries().get(seq) {
                if *known != entry {
                    return Err(ApiError::crypto(
                        "the server's copy of a co-member's device log forks the one held here",
                    ));
                }
                continue;
            }
            log.append(entry.clone())
                .map_err(|_| ApiError::crypto("a co-member's device log does not verify"))?;
            added.push(entry);
        }
        if added.is_empty() {
            return Ok(false);
        }
        if held.is_none() && log.entries().first().map(kb_keys::log::account_id_of) != Some(account)
        {
            return Err(ApiError::crypto(
                "a device log does not belong to the account it was fetched for",
            ));
        }
        let now = self.clock.now_ms();
        for entry in &added {
            self.db
                .foreign_log_put(&ForeignLogRow {
                    account_id: account,
                    seq: entry.seq,
                    entry: entry.encode(),
                    fetched_at: now,
                })
                .map_err(map_storage_error)?;
        }
        self.shared.logs.insert(account, log);
        // A device the new entries revoked is no longer present (its entry would otherwise wait for the timeout).
        self.presence_prune_devices(account)?;
        // What the new entries mean for the spaces this account is a member of.
        let kit_added = added
            .iter()
            .any(|entry| matches!(entry.body, LogEntryBody::DeviceAdded { via_kit: true, .. }));
        let feeds: Vec<(FeedId, SpaceId)> = self
            .db
            .sync_feed_list()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|row| row.kind == FeedKind::Shared)
            .filter_map(|row| row.space_id.map(|space| (row.feed, space)))
            .collect();
        for (feed, space_id) in feeds {
            if !self.feed_is_live(feed)? || !self.members_of(feed)?.contains(&account) {
                continue;
            }
            self.push_event_once(Event::SpaceMembersChanged { space_id });
            if kit_added {
                let name = self.member_name(feed, account);
                self.events.push(Event::MemberDeviceViaKit {
                    space_id,
                    account_id: account,
                    name,
                });
            }
        }
        Ok(true)
    }

    // ----- host methods: membership entries -------------------------------------------------

    /// The encoded membership entries this device minted for the feed and the server has not taken, in
    /// log order (the next ones to post by compare-and-swap). The core also mints here what the
    /// protocol asks of any member device: a `Rekey` after a `Leave` or a revocation (when this
    /// device writes, or the server said `rekey_needed`, or this account's own revocation is not
    /// covered: self-policing), a `Grant` for a member who joined with an older bundle.
    ///
    /// If the first entry is a `Create` (`seq` 0) the driver posts it with `feeds_create`, the rest
    /// with `membership_append`.
    pub fn sync_membership_pending(&mut self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError> {
        self.shared_load()?;
        if !self.feed_is_live(feed)? {
            return Ok(Vec::new());
        }
        self.reintents_replay(feed)?;
        self.membership_automint(feed)?;
        Ok(self
            .pending_membership_rows(feed)?
            .into_iter()
            .map(|row| row.bytes)
            .collect())
    }

    /// The server took the pending entries; its membership head after them. The entries stay in
    /// `membership_entries`; the feed items come back through `sync_feed_apply` and fold.
    pub fn sync_membership_posted(
        &mut self,
        feed: FeedId,
        head: MembershipHead,
    ) -> Result<(), ApiError> {
        let posted = self
            .fcfg_u64(feed, "posted")?
            .map_or(head.seq, |p| p.max(head.seq));
        self.fcfg_set(feed, "posted", &posted.to_string())?;
        // The entries were taken: a Rekey this device waited for is on its way.
        self.fcfg_del(feed, "rekey")?;
        // An invite whose `Invite` is among them hands its bundle over (a `Leave` among them lets the
        // local data of the space go at the next `sync_feeds`).
        self.invite_entries_posted(feed);
        Ok(())
    }

    /// The compare-and-swap lost (409 `conflict`): the core drops what it minted on a stale head,
    /// re-reads the log at the next pull, re-evaluates its intent (a target already removed is a
    /// no-op; "I am no longer Owner" is refused) and mints again.
    ///
    /// What this device only re-posts after a server reset (entries it had found in the feed
    /// before) stays pending: the server's copy of the log decides at the next `sync_membership_fetched`.
    pub fn sync_membership_conflict(&mut self, feed: FeedId) -> Result<(), ApiError> {
        self.shared_load()?;
        let found = self.fcfg_u64(feed, "found")?;
        // The entries minted here that were not posted (or not accepted) go; the ones the server
        // took come back through the feed, and the ones found in the feed before a server reset
        // are posted again.
        let dropped_rows: Vec<kb_storage::MembershipEntryRow> = self
            .pending_membership_rows(feed)?
            .into_iter()
            .filter(|row| !found.is_some_and(|found| row.seq <= found))
            .collect();
        let dropped: Vec<u64> = dropped_rows.iter().map(|row| row.seq).collect();
        let dropped_entries: Vec<Vec<u8>> = dropped_rows.into_iter().map(|row| row.bytes).collect();
        if !dropped.is_empty() {
            let keep: Vec<_> = self
                .unconfirmed_membership_rows(feed)?
                .into_iter()
                .filter(|row| !dropped.contains(&row.seq))
                .collect();
            self.db
                .membership_entries_discard_pending(feed)
                .map_err(map_storage_error)?;
            if !keep.is_empty() {
                self.db
                    .membership_entries_put(&keep)
                    .map_err(map_storage_error)?;
            }
        }
        // Whatever was posted is the server's now; read its log again before anything else is minted.
        self.fcfg_set(feed, "refetch", "1")?;
        if let Some(space_id) = self.feed_space(feed)? {
            self.push_event_once(Event::SpaceMembersChanged { space_id });
        }
        // What the request side minted is written again once the log has caught up
        // (`reintents_replay`, at the start of `sync_membership_pending`).
        self.reintents_note(feed, &dropped_entries)?;
        self.invite_after_conflict(feed);
        self.sync_note_local_change();
        Ok(())
    }

    /// Whether the core wants the membership log read directly with `membership_get` (a removed
    /// account verifying its removal, a server reset re-post, a lost compare-and-swap, the first
    /// contact of a device that has no timeline for a feed it syncs): `Some(after_seq)` (`None`
    /// inside asks for the whole log).
    pub fn sync_membership_wanted(
        &mut self,
        feed: FeedId,
    ) -> Result<Option<Option<u64>>, ApiError> {
        self.shared_load()?;
        if !self.feed_is_live(feed)? || self.shared.stalled.contains(&feed) {
            return Ok(None);
        }
        let folded = self.shared.timelines.get(&feed).map_or(0, Timeline::len);
        if self.fcfg(feed, "refetch")?.is_some() {
            return Ok(Some(folded.checked_sub(1).map(|seq| seq as u64)));
        }
        let row = self
            .db
            .sync_feed_get(feed)
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::not_found("feed not found"))?;
        let pending = !self.pending_membership_rows(feed)?.is_empty();
        if folded == 0 && row.cursor == 0 && !pending {
            return Ok(Some(None));
        }
        Ok(None)
    }

    /// A page of the membership log read with `membership_get`: verified and folded like feed items.
    pub fn sync_membership_fetched(
        &mut self,
        feed: FeedId,
        page: &MembershipPage,
    ) -> Result<(), ApiError> {
        self.shared_load()?;
        for item in &page.entries {
            match self.membership_ingest(feed, item.cursor, &item.bytes)? {
                Ingest::Applied | Ingest::Already => {}
                Ingest::Stalled => {
                    self.shared.stalled.insert(feed);
                    self.fcfg_del(feed, "refetch")?;
                    return Ok(());
                }
            }
        }
        if page.more {
            self.fcfg_set(feed, "refetch", "1")?;
        } else {
            self.fcfg_del(feed, "refetch")?;
        }
        Ok(())
    }

    /// The server does not know the feed for this account (404 on a feed that was synced before:
    /// removed with nothing left to read, or deleted after a close): the space becomes an archive.
    pub fn sync_feed_lost(&mut self, feed: FeedId) -> Result<(), ApiError> {
        self.shared_load()?;
        let row = self.db.sync_feed_get(feed).map_err(map_storage_error)?;
        let Some(row) = row else {
            return Ok(());
        };
        if row.kind != FeedKind::Shared || !self.feed_is_live(feed)? {
            return Ok(());
        }
        // A feed whose Create the server never took is not lost: it was never there.
        let created = self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?
            .iter()
            .any(|entry| entry.cursor > 0);
        if !created {
            return Ok(());
        }
        let closed = self
            .shared
            .timelines
            .get(&feed)
            .is_some_and(|t| t.state().closed.is_some());
        let reason = if closed {
            ArchiveReason::Closed
        } else {
            ArchiveReason::Removed
        };
        self.feed_archive(feed, reason)
    }

    /// The server refused this account on the feed (403 `forbidden`): the driver ends that feed's
    /// step; the core records `message` for the feed's diagnostics.
    pub fn sync_feed_refused(&mut self, feed: FeedId, message: &str) -> Result<(), ApiError> {
        let message: String = message.chars().take(200).collect();
        self.fcfg_set(feed, "refused", &message)?;
        tracing::warn!(feed = %feed.short_hex(), "the server refused this account on a feed");
        if let Some(space_id) = self.feed_space(feed)? {
            self.push_event_once(Event::SpaceMembersChanged { space_id });
        }
        Ok(())
    }

    /// The driver got 409 `rekey_needed` on an append: the feed waits for a `Rekey` (a `Leave` or a
    /// revocation). The next [`Self::sync_membership_pending`] mints it once the logs it needs are
    /// here (`sync_needed_logs` asks for the members' logs again meanwhile).
    pub fn sync_feed_rekey_needed(&mut self, feed: FeedId) -> Result<(), ApiError> {
        self.shared_load()?;
        self.fcfg_set(feed, "rekey", "1")?;
        // The members' logs may hold the revocation this device has not seen: ask for them again.
        for account in self.members_of(feed)? {
            if self.sync_identity()?.map(|(me, _)| me) != Some(account) {
                self.shared.wanted_logs.insert(account);
            }
        }
        self.sync_note_local_change();
        Ok(())
    }

    // ----- helpers --------------------------------------------------------------------------

    /// The space a feed carries (`None` for the personal feed and for a feed not bound yet).
    pub(crate) fn feed_space(&self, feed: FeedId) -> Result<Option<SpaceId>, ApiError> {
        Ok(self
            .db
            .sync_feed_get(feed)
            .map_err(map_storage_error)?
            .and_then(|row| row.space_id))
    }

    /// The stored entries of the feed that are not in the timeline yet and that the server has not
    /// taken: the ones the core minted (or kept for a re-post), in log order.
    pub(crate) fn pending_membership_rows(
        &mut self,
        feed: FeedId,
    ) -> Result<Vec<kb_storage::MembershipEntryRow>, ApiError> {
        self.shared_load()?;
        let posted = self.fcfg_u64(feed, "posted")?;
        let rows = self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?;
        Ok(rows
            .into_iter()
            .filter(|row| row.cursor == 0 && posted.is_none_or(|posted| row.seq > posted))
            .collect())
    }

    /// Every stored entry of the feed that was minted here and is not in the feed yet, posted or not.
    pub(crate) fn unconfirmed_membership_rows(
        &self,
        feed: FeedId,
    ) -> Result<Vec<kb_storage::MembershipEntryRow>, ApiError> {
        Ok(self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|row| row.cursor == 0)
            .collect())
    }

    /// The name a member gave, or the short hex of its account id.
    pub(crate) fn member_name(&self, feed: FeedId, account: AccountId) -> String {
        self.shared
            .metas
            .get(&feed)
            .and_then(|meta| {
                meta.names
                    .get(&account)
                    .or_else(|| meta.former_names.get(&account))
            })
            .cloned()
            .unwrap_or_else(|| fallback_name(account))
    }

    /// The space becomes an archive: this account was removed or left, or an Owner closed it.
    /// Unsent changes are copied out first (fork-out); the feed is not synced any more.
    pub(crate) fn feed_archive(
        &mut self,
        feed: FeedId,
        reason: ArchiveReason,
    ) -> Result<(), ApiError> {
        if self.feed_archived(feed)?.is_some() {
            return Ok(());
        }
        let space_id = self.feed_space(feed)?;
        if let Some(space_id) = space_id {
            let me = self.me()?;
            if let Some(role) = self
                .shared
                .timelines
                .get(&feed)
                .and_then(|t| t.state().role_of(me))
            {
                self.fcfg_set(feed, "role", &role.to_byte().to_string())?;
            }
            // The unsent changes of a removed device go into pages of a personal space; the archive
            // keeps what it has.
            if let Err(error) = self.forkout_unsent_changes(space_id) {
                tracing::warn!(code = ?error.code, "fork-out of an archive failed");
            }
        }
        self.fcfg_set(
            feed,
            "archived",
            match reason {
                ArchiveReason::Removed => "removed",
                ArchiveReason::Closed => "closed",
            },
        )?;
        if let Some(space_id) = space_id {
            self.events.push(Event::SpaceRemoved { space_id, reason });
            self.push_event_once(Event::SpaceMembersChanged { space_id });
        }
        self.push_event_once(Event::SpacesChanged);
        self.push_event_once(Event::SyncChanged);
        // Nobody is present in an archive.
        self.presence_prune(feed)?;
        tracing::info!(feed = %feed.short_hex(), ?reason, "a shared space became an archive");
        Ok(())
    }
}

/// What a member shows as until a name is known: its account id cut to 8 hex characters.
pub(crate) fn fallback_name(account: AccountId) -> String {
    let mut out = String::from("member ");
    for byte in &account.as_bytes()[..4] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
