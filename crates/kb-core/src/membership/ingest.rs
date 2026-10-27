//! Folding one incoming membership entry (ADR-0043): the chain and the rules through
//! `kb_keys::membership::Timeline`, the keys the entry carries for this device (ADR-0044), the
//! metadata of its private part, and the derived events and transitions.

use kb_keys::membership::{
    Applied, EntryBody, EntryKind, FoldError, FoldRule, MembershipEntry, Private, Timeline,
    verify_for_recipient,
};
use kb_keys::space_keys::{SpaceKey, membership_meta_key};
use kb_storage::{FeedQueries, MembershipEntryRow, StorageError};
use kb_types::{AccountId, ApiError, ArchiveReason, Event, FeedId, MemberRole};

use super::{LogsView, SharedRuntime};
use crate::{Core, map_storage_error};

/// What folding an incoming entry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ingest {
    /// The entry is new and now part of the timeline.
    Applied,
    /// The timeline had it already (a replayed page): the bytes equal the stored entry.
    Already,
    /// A device log the entry needs is not here: the core asked for it (`sync_needed_logs`) and
    /// nothing was changed; the same entry is offered again later.
    Stalled,
}

/// The accounts a membership entry names: its signer and everyone its body refers to.
pub(crate) fn entry_accounts(entry: &MembershipEntry) -> Vec<AccountId> {
    let mut accounts = vec![entry.header.account];
    match &entry.header.body {
        EntryBody::Create { member_heads, .. } => {
            accounts.extend(member_heads.iter().map(|h| h.account));
        }
        EntryBody::Join { account, .. } => accounts.push(*account),
        EntryBody::RoleSet { target, .. } => accounts.push(*target),
        EntryBody::Remove { target, rekey } => {
            accounts.push(*target);
            accounts.extend(rekey.member_heads.iter().map(|h| h.account));
        }
        EntryBody::Rekey(rekey) => accounts.extend(rekey.member_heads.iter().map(|h| h.account)),
        EntryBody::Grant { member, .. } => accounts.push(*member),
        _ => {}
    }
    accounts
}

impl Core {
    /// Folds one entry into the timeline of `feed`, found at `cursor` (the caller keeps its own
    /// bookkeeping; on error the timeline is unchanged).
    pub(crate) fn fold_entry(
        &mut self,
        feed: FeedId,
        cursor: u64,
        entry: &MembershipEntry,
    ) -> Result<Applied, FoldError> {
        let Ok(Some((me, _))) = self.sync_identity() else {
            return Err(FoldError {
                index: 0,
                rule: FoldRule::UnknownLog,
            });
        };
        let own = self.security.log.as_ref();
        let SharedRuntime {
            timelines, logs, ..
        } = &mut self.shared;
        let view = LogsView {
            me,
            own,
            foreign: logs,
        };
        timelines
            .entry(feed)
            .or_insert_with(|| Timeline::new(feed))
            .apply(cursor, entry, &view)
    }

    /// The stored entry at `seq`.
    fn stored_membership(
        &self,
        feed: FeedId,
        seq: u64,
    ) -> Result<Option<MembershipEntryRow>, ApiError> {
        Ok(self
            .db
            .membership_entries_list(feed, seq.checked_sub(1))
            .map_err(map_storage_error)?
            .into_iter()
            .find(|row| row.seq == seq))
    }

    /// Takes one membership entry the feed (a pull or `membership_get`) delivered at `cursor`:
    /// stores it, folds it, opens the keys it carries and derives the events. An entry that does not
    /// verify, that rewrites what is stored or that breaks a rule of the log is the server
    /// misbehaving (`Crypto`, the feed is flagged).
    pub(crate) fn membership_ingest(
        &mut self,
        feed: FeedId,
        cursor: u64,
        bytes: &[u8],
    ) -> Result<Ingest, ApiError> {
        self.shared_load()?;
        let entry = MembershipEntry::decode(bytes)
            .map_err(|_| self.flag_server(feed, "a membership entry does not decode"))?;
        if entry.header.feed != feed {
            return Err(self.flag_server(feed, "a membership entry names another feed"));
        }
        let seq = entry.header.seq;
        let folded = self.shared.timelines.get(&feed).map_or(0, Timeline::len);
        if usize::try_from(seq).unwrap_or(usize::MAX) < folded {
            return match self.stored_membership(feed, seq)? {
                Some(row) if row.bytes == bytes => Ok(Ingest::Already),
                _ => {
                    self.mark_fork_in(feed, "the membership log was rewritten")?;
                    Err(self.flag_server(feed, "the membership log was rewritten"))
                }
            };
        }
        match self.db.membership_entries_put(&[MembershipEntryRow {
            feed,
            seq,
            cursor,
            bytes: bytes.to_vec(),
        }]) {
            Ok(()) => {}
            Err(StorageError::InvalidData(_)) => {
                self.mark_fork_in(feed, "the membership log was rewritten")?;
                return Err(self.flag_server(feed, "the membership log was rewritten"));
            }
            Err(error) => return Err(map_storage_error(error)),
        }
        let me = self.me()?;
        let before = self
            .shared
            .timelines
            .get(&feed)
            .map(|t| (t.state().role_of(me), t.state().closed.is_some()));
        match self.fold_entry(feed, cursor, &entry) {
            Ok(applied) => {
                self.membership_applied(feed, cursor, &entry, &applied, before)?;
                // A member who is gone (removed, left) is not present any more.
                self.presence_prune(feed)?;
                Ok(Ingest::Applied)
            }
            Err(error)
                if error.rule == FoldRule::UnknownLog
                    || (error.rule == FoldRule::Signature
                        && !self.shared.fresh_logs.contains(&entry.header.account)) =>
            {
                // A log that is not here, or one fetched before the signer's device was added:
                // ask for the logs again before judging.
                self.shared.wanted_logs.insert(entry.header.account);
                for account in entry_accounts(&entry) {
                    let known = account == me || self.shared.logs.contains_key(&account);
                    if !known {
                        self.shared.wanted_logs.insert(account);
                    }
                }
                Ok(Ingest::Stalled)
            }
            Err(error) => {
                tracing::warn!(feed = %feed.short_hex(), rule = ?error.rule, "a membership entry breaks a rule of the log");
                Err(self.flag_server(feed, "the membership log does not verify"))
            }
        }
    }

    /// Marks the feed as one the server misbehaved on and builds the error to stop the feed with.
    fn flag_server(&mut self, feed: FeedId, why: &str) -> ApiError {
        let _ = self.fcfg_set(feed, "flagged", "1");
        tracing::error!(feed = %feed.short_hex(), "{why}");
        ApiError::crypto(why)
    }

    /// Everything that follows a folded entry: metadata, keys, events, transitions.
    fn membership_applied(
        &mut self,
        feed: FeedId,
        cursor: u64,
        entry: &MembershipEntry,
        applied: &Applied,
        before: Option<(Option<MemberRole>, bool)>,
    ) -> Result<(), ApiError> {
        let _ = cursor;
        let seq = entry.header.seq;
        if self
            .fcfg_u64(feed, "found")?
            .is_none_or(|found| found < seq)
        {
            self.fcfg_set(feed, "found", &seq.to_string())?;
        }
        self.membership_meta(feed, entry, applied)?;
        // Names came or went (a member joined, was renamed or removed): formulas read them again.
        self.shared.names_rev += 1;
        if self.shared.adopting == Some(feed) {
            // An invitee replays the log it was served: nothing in it happens to this account now.
            return Ok(());
        }
        // A name was given or changed: the text rows of the Person values that list it follow.
        if matches!(
            entry.kind(),
            EntryKind::Create | EntryKind::Join | EntryKind::Profile
        ) && let Err(error) = self.person_names_changed(feed)
        {
            tracing::warn!(code = ?error.code, "the people's search text could not be refreshed");
        }
        // An inviter's session finishes when the invitee's `Join` of its invite folds.
        self.invite_join_seen(feed, entry);

        let me = self.me()?;
        let Some(state) = self.shared.timelines.get(&feed).map(|t| t.state().clone()) else {
            return Ok(());
        };
        let space_id = self.feed_space(feed)?;
        let after = state.role_of(me);
        let (role_before, closed_before) = before.unwrap_or((None, false));
        if let Some(role) = after {
            self.fcfg_set(feed, "role", &role.to_byte().to_string())?;
        }
        if let Some(space_id) = space_id {
            self.push_event_once(Event::SpaceMembersChanged { space_id });
            match (role_before, after) {
                (Some(previous), Some(role)) if previous != role => {
                    self.events.push(Event::SpaceRoleChanged {
                        space_id,
                        role,
                        previous,
                    });
                    self.push_event_once(Event::SpacesChanged);
                    if role < MemberRole::Editor && previous >= MemberRole::Editor {
                        self.shared.forkout_due.insert(feed);
                    }
                }
                (None, Some(_)) => self.push_event_once(Event::SpacesChanged),
                _ => {}
            }
        }
        if role_before.is_some() && after.is_none() {
            self.feed_archive(feed, ArchiveReason::Removed)?;
        } else if state.closed.is_some() && !closed_before {
            self.feed_archive(feed, ArchiveReason::Closed)?;
        }
        Ok(())
    }

    /// What an entry gives: the clear fields that later minting needs (commitments, the links of the
    /// backward key chain, who holds which epoch, where each member's own device log stood, the time
    /// of a `Close`), the keys its boxes hold for this device, then its private part (names and
    /// settings) with the keys that came from the entry itself.
    pub(crate) fn membership_meta(
        &mut self,
        feed: FeedId,
        entry: &MembershipEntry,
        applied: &Applied,
    ) -> Result<(), ApiError> {
        let _ = applied;
        self.membership_meta_clear(feed, entry);
        self.membership_receive_keys(feed, entry)?;
        self.membership_meta_private(feed, entry)
    }

    /// The private part of an entry (its names and settings) when this device holds the key of its
    /// `meta_epoch`.
    fn membership_meta_private(
        &mut self,
        feed: FeedId,
        entry: &MembershipEntry,
    ) -> Result<(), ApiError> {
        let private = match self.feed_sk(feed, entry.header.meta_epoch)? {
            Some(sk) => entry.open_private(&membership_meta_key(&sk, feed)).ok(),
            None => None,
        };
        let meta = self.shared.metas.entry(feed).or_default();
        let signer = entry.header.account;
        match private {
            Some(Private::Create {
                space_name,
                settings,
                profile_name,
            }) => {
                meta.space_name = Some(space_name);
                meta.settings = settings;
                meta.names.insert(signer, profile_name);
            }
            Some(Private::Join { profile_name }) => {
                if let EntryBody::Join { account, .. } = &entry.header.body {
                    meta.names.insert(*account, profile_name);
                }
            }
            Some(Private::Profile { display_name }) => {
                meta.names.insert(signer, display_name);
            }
            Some(Private::Settings { settings }) => meta.settings = settings,
            Some(Private::None) | None => {}
        }
        if matches!(
            entry.kind(),
            EntryKind::Create | EntryKind::Join | EntryKind::Profile
        ) {
            // A name was given or changed: the people read again.
            self.push_event_once(Event::PeopleChanged);
        }
        Ok(())
    }

    /// The clear fields of an entry that need no key.
    fn membership_meta_clear(&mut self, feed: FeedId, entry: &MembershipEntry) {
        let members: Vec<AccountId> = self
            .shared
            .timelines
            .get(&feed)
            .map(|t| t.state().members.iter().map(|m| m.account).collect())
            .unwrap_or_default();
        let meta = self.shared.metas.entry(feed).or_default();
        let signer = entry.header.account;
        match &entry.header.body {
            EntryBody::Create {
                epoch,
                sk_commitment,
                ..
            } => {
                meta.commitments.insert(*epoch, *sk_commitment);
                meta.keyed.insert(signer, *epoch);
                if let EntryBody::Create { member_heads, .. } = &entry.header.body
                    && let Some(head) = member_heads.iter().find(|h| h.account == signer)
                {
                    meta.joined_log.insert(signer, head.head.seq);
                }
            }
            EntryBody::Rekey(rekey) | EntryBody::Remove { rekey, .. } => {
                meta.commitments
                    .insert(rekey.new_epoch, rekey.sk_commitment);
                meta.prev_wrapped
                    .insert(rekey.new_epoch, rekey.prev_wrapped.clone());
                meta.keyed.retain(|account, _| members.contains(account));
                for account in &members {
                    meta.keyed.insert(*account, rekey.new_epoch);
                }
            }
            EntryBody::Grant { member, epoch, .. } => {
                let held = meta.keyed.get(member).copied().unwrap_or(0);
                meta.keyed.insert(*member, held.max(*epoch));
            }
            EntryBody::Join { account, log, .. } => {
                meta.keyed.insert(*account, entry.header.meta_epoch);
                meta.joined_log.insert(*account, log.seq);
            }
            EntryBody::Close => meta.closed_at = Some(entry.header.at),
            _ => {}
        }
        if entry.kind() == EntryKind::Remove
            && let EntryBody::Remove { target, .. } = &entry.header.body
        {
            meta.keyed.remove(target);
            if let Some(name) = meta.names.remove(target) {
                meta.former_names.insert(*target, name);
            }
        }
    }

    /// Opens the box this entry holds for one of this device's keys (`Create`, `Rekey`, `Remove` with
    /// its Rekey, `Grant`), checks it against the commitment and stores the key as a `Space(epoch)`
    /// record of the personal keyring. A box that does not open or does not match is a "key
    /// mismatch": counted for diagnostics, nothing is stored.
    fn membership_receive_keys(
        &mut self,
        feed: FeedId,
        entry: &MembershipEntry,
    ) -> Result<(), ApiError> {
        let (epoch, grant) = match &entry.header.body {
            EntryBody::Create { epoch, .. } => (*epoch, false),
            EntryBody::Rekey(rekey) | EntryBody::Remove { rekey, .. } => (rekey.new_epoch, false),
            EntryBody::Grant { epoch, .. } => (*epoch, true),
            _ => return Ok(()),
        };
        let mut known = std::collections::HashMap::new();
        for e in [epoch, epoch.saturating_sub(1)] {
            if let Some(key) = self.feed_sk(feed, e)? {
                known.insert(e, key);
            }
        }
        let lookup = |e: u32| -> Option<SpaceKey> { known.get(&e).cloned() };
        let opened = verify_for_recipient(
            entry,
            self.device.device_id,
            &self.security.device_keys,
            &lookup,
        );
        match opened {
            Ok(Some(key)) => {
                let matches = !grant
                    || self.shared.timelines.get(&feed).is_some_and(|t| {
                        t.state().epoch == epoch && t.state().current_key_matches(&key)
                    });
                if matches {
                    self.feed_store_sk(feed, epoch, &key)?;
                    self.feed_derive_older_keys(feed)?;
                } else {
                    self.note_key_mismatch(feed);
                }
            }
            Ok(None) => {}
            Err(_) => self.note_key_mismatch(feed),
        }
        Ok(())
    }

    fn note_key_mismatch(&mut self, feed: FeedId) {
        self.shared.metas.entry(feed).or_default().key_mismatches += 1;
        tracing::warn!(feed = %feed.short_hex(), "a space key box did not open or did not match its commitment");
    }

    /// The timeline with the entries this device minted and the server has not confirmed folded on
    /// top (`None` when nothing is known of the feed). The pending entries are folded at the
    /// highest possible cursor; the clone is thrown away after the caller looked.
    pub(crate) fn tentative_timeline(
        &mut self,
        feed: FeedId,
    ) -> Result<Option<Timeline>, ApiError> {
        self.shared_load()?;
        let mut timeline = self
            .shared
            .timelines
            .get(&feed)
            .cloned()
            .unwrap_or_else(|| Timeline::new(feed));
        let folded = timeline.len();
        let pending: Vec<MembershipEntryRow> = self
            .unconfirmed_membership_rows(feed)?
            .into_iter()
            .filter(|row| usize::try_from(row.seq).unwrap_or(usize::MAX) >= folded)
            .collect();
        if pending.is_empty() {
            return Ok((folded > 0).then_some(timeline));
        }
        let Some((me, _)) = self.sync_identity()? else {
            return Ok(None);
        };
        let own = self.security.log.as_ref();
        let view = LogsView {
            me,
            own,
            foreign: &self.shared.logs,
        };
        for row in pending {
            let Ok(entry) = MembershipEntry::decode(&row.bytes) else {
                break;
            };
            if timeline.apply(u64::MAX, &entry, &view).is_err() {
                break;
            }
        }
        Ok(Some(timeline))
    }

    /// Records that the feed's history does not match this device's (uploads to it stop).
    pub(crate) fn mark_fork_in(&mut self, feed: FeedId, why: &str) -> Result<(), ApiError> {
        tracing::error!(feed = %feed.short_hex(), "{why}");
        if let Some(mut row) = self.db.sync_feed_get(feed).map_err(map_storage_error)? {
            row.fork = true;
            self.db.sync_feed_put(&row).map_err(map_storage_error)?;
        }
        self.fcfg_set(feed, "refused", why)?;
        self.sync.state = kb_types::SyncState::Forked;
        self.sync.last_error = Some(why.to_owned());
        self.events.push(Event::SyncChanged);
        Ok(())
    }
}
