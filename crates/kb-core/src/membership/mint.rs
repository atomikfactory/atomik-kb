//! Minting membership entries (ADR-0043, ADR-0044): the generic [`Core::membership_mint`] the request
//! side builds `Invite`, `RoleSet`, `Remove`, `Leave`, `Settings`, `Profile`, `Close` and `Join` on, the
//! helpers for the entries that carry keys (`Create`, `Rekey`, `Grant`), and the entries the core mints
//! by itself when the protocol asks any member device to (a `Rekey` after a `Leave` or a revocation,
//! including self-policing, and a `Grant`).
//!
//! A minted entry is stored in `membership_entries` with the cursor 0 (see `membership.rs`) and
//! validated against the tentative timeline before it is stored: an entry the fold would refuse is
//! never stored. The `Rekey` of a removal is built here too ([`Core::build_rekey_fields`]) and
//! carried inline by the `Remove` the request side mints.

use kb_keys::membership::{
    Applied, EntryBody, FoldError, FoldRule, KeyBox, MembershipEntry, MembershipHeader, Private,
    RekeyFields, Timeline, rekey_recipients,
};
use kb_keys::space_keys::{SpaceKey, membership_meta_key, seal_space_key_for, wrap_prev};
use kb_storage::{FeedQueries, MembershipEntryRow};
use kb_types::sharing::SpaceSettings;
use kb_types::{AccountId, ApiError, FeedId, MemberRole, SpaceId};

use super::{LogsView, latest_revocation_seq};
use crate::{Core, map_keys_error, map_storage_error};

impl Core {
    /// Folds `entry` on a clone-free tentative timeline with the logs this device holds.
    fn tentative_apply(
        &self,
        timeline: &mut Timeline,
        entry: &MembershipEntry,
    ) -> Result<Applied, FoldError> {
        let Ok(Some((me, _))) = self.sync_identity() else {
            return Err(FoldError {
                index: 0,
                rule: FoldRule::UnknownLog,
            });
        };
        let view = LogsView {
            me,
            own: self.security.log.as_ref(),
            foreign: &self.shared.logs,
        };
        timeline.apply(u64::MAX, entry, &view)
    }

    /// Builds, signs, validates and stores one membership entry signed by this device, and answers
    /// it. `meta` is the key the private part is sealed under (`(epoch, key)` of an entry that
    /// starts an epoch, or of a joiner's bundle); `None` takes the feed's current epoch. A rule the
    /// log would refuse is `InvalidInput` (`membership`), a device log that is not here yet
    /// `Conflict`-free `InvalidInput` too after the core asked for it.
    pub(crate) fn membership_mint(
        &mut self,
        feed: FeedId,
        body: EntryBody,
        private: Private,
        meta: Option<(u32, SpaceKey)>,
    ) -> Result<MembershipEntry, ApiError> {
        self.shared_load()?;
        let Some((me, device)) = self.sync_identity()? else {
            return Err(ApiError::invalid(
                "kit",
                "create your recovery kit before sharing",
            ));
        };
        let rows = self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?;
        let (seq, prev) = match rows.last() {
            Some(row) => (row.seq + 1, kb_crypto::hash(&row.bytes)),
            None => (0, [0u8; 32]),
        };
        let (meta_epoch, meta_key) = match meta {
            Some(pair) => pair,
            None => {
                let epoch = self
                    .tentative_state(feed)?
                    .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?
                    .epoch;
                let key = self.feed_sk(feed, epoch)?.ok_or_else(|| {
                    ApiError::crypto("this device does not hold the space key of the current epoch")
                })?;
                (epoch, key)
            }
        };
        let header = MembershipHeader {
            feed,
            seq,
            prev,
            account: me,
            device,
            at: self.clock.now_ms(),
            meta_epoch,
            body,
        };
        let entry = MembershipEntry::build(
            header,
            &private,
            &membership_meta_key(&meta_key, feed),
            &self.security.device_keys,
            self.rng.as_ref(),
        );
        let mut tentative = self
            .tentative_timeline(feed)?
            .unwrap_or_else(|| Timeline::new(feed));
        match self.tentative_apply(&mut tentative, &entry) {
            Ok(_) => {}
            Err(error) if error.rule == FoldRule::UnknownLog => {
                for account in super::ingest::entry_accounts(&entry) {
                    if account != me && !self.shared.logs.contains_key(&account) {
                        self.shared.wanted_logs.insert(account);
                    }
                }
                return Err(ApiError::invalid(
                    "membership",
                    "a member's device log is not known here yet; sync and try again",
                ));
            }
            Err(error) => {
                tracing::warn!(rule = ?error.rule, "a membership entry this device built breaks a rule");
                return Err(ApiError::invalid(
                    "membership",
                    format!(
                        "the change is not allowed by the membership log ({:?})",
                        error.rule
                    ),
                ));
            }
        }
        self.db
            .membership_entries_put(&[MembershipEntryRow {
                feed,
                seq,
                cursor: 0,
                bytes: entry.encode(),
            }])
            .map_err(map_storage_error)?;
        self.sync_note_local_change();
        Ok(entry)
    }

    /// Starts the shared feed of a personal space: mints `SK[e + 1]` (`e` is the space's current epoch,
    /// so the old personal keys never reach members), stores it as a `Space` record, registers the
    /// feed, writes `sharedSpaces` and mints the `Create` (pending until the driver posts it with
    /// `feeds_create`). Answers the new feed. `AlreadyExists` when the space is shared.
    #[allow(dead_code)] // the share request (agent D2) starts a feed with it
    pub(crate) fn mint_create(
        &mut self,
        space_id: SpaceId,
        space_name: &str,
        settings: SpaceSettings,
        profile_name: &str,
    ) -> Result<FeedId, ApiError> {
        self.shared_load()?;
        let me = self.me()?;
        if self.shared_feed_of(space_id)?.is_some() {
            return Err(ApiError::new(
                kb_types::ErrorCode::AlreadyExists,
                "the space is shared already",
            ));
        }
        let feed = loop {
            let candidate = FeedId::generate(self.rng.as_ref());
            if candidate != FeedId::from(me) {
                break candidate;
            }
        };
        let epoch = self.space_epoch(space_id) + 1;
        let sk = SpaceKey::generate(self.rng.as_ref());
        let recipients = {
            let view = LogsView {
                me,
                own: self.security.log.as_ref(),
                foreign: &self.shared.logs,
            };
            rekey_recipients(&[me], &view).map_err(map_keys_error)?
        };
        let boxes: Vec<KeyBox> = recipients
            .devices
            .iter()
            .map(|(_, device, keys)| KeyBox {
                device: *device,
                bytes: seal_space_key_for(&sk, feed, epoch, keys, self.rng.as_ref()),
            })
            .collect();
        self.feed_register_shared(feed, Some(space_id))?;
        self.feed_store_sk(feed, epoch, &sk)?;
        let body = EntryBody::Create {
            epoch,
            sk_commitment: sk.commitment(feed, epoch),
            boxes,
            member_heads: recipients.member_heads,
        };
        let private = Private::Create {
            space_name: space_name.to_owned(),
            settings,
            profile_name: profile_name.to_owned(),
        };
        self.membership_mint(feed, body, private, Some((epoch, sk)))?;
        self.ensure_account()?;
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .set_shared_space(space_id, feed)
                .map_err(crate::documents::map_model_error)?;
        }
        self.persist_account()?;
        Ok(feed)
    }

    /// The fields of a `Rekey` for `members` (the accounts that remain after the entry: all current
    /// members for a plain `Rekey`, all but the target for a `Remove`), and the new key. `None` when
    /// something is missing that the core asked for (a member's device log, the key of the current
    /// epoch) or a member's log here is older than the head an earlier entry named (the core asks
    /// for the newer one): nothing is minted then.
    pub(crate) fn build_rekey_fields(
        &mut self,
        feed: FeedId,
        members: &[AccountId],
    ) -> Result<Option<(RekeyFields, SpaceKey)>, ApiError> {
        let me = self.me()?;
        let Some(timeline) = self.tentative_timeline(feed)? else {
            return Ok(None);
        };
        let epoch = timeline.state().epoch;
        let Some(old) = self.feed_sk(feed, epoch)? else {
            return Ok(None);
        };
        let missing: Vec<AccountId> = members
            .iter()
            .copied()
            .filter(|account| *account != me && !self.shared.logs.contains_key(account))
            .collect();
        if !missing.is_empty() {
            self.shared.wanted_logs.extend(missing);
            return Ok(None);
        }
        let recipients = {
            let view = LogsView {
                me,
                own: self.security.log.as_ref(),
                foreign: &self.shared.logs,
            };
            rekey_recipients(members, &view).map_err(map_keys_error)?
        };
        for head in &recipients.member_heads {
            if timeline
                .head_seq(head.account)
                .is_some_and(|named| head.head.seq < named)
            {
                // Another entry named a newer copy of this member's log than this device holds.
                self.shared.wanted_logs.insert(head.account);
                return Ok(None);
            }
        }
        let new_epoch = epoch + 1;
        let new = SpaceKey::generate(self.rng.as_ref());
        let boxes: Vec<KeyBox> = recipients
            .devices
            .iter()
            .map(|(_, device, keys)| KeyBox {
                device: *device,
                bytes: seal_space_key_for(&new, feed, new_epoch, keys, self.rng.as_ref()),
            })
            .collect();
        let fields = RekeyFields {
            new_epoch,
            sk_commitment: new.commitment(feed, new_epoch),
            prev_wrapped: wrap_prev(&old, &new, feed, epoch, self.rng.as_ref()),
            boxes,
            member_heads: recipients.member_heads,
        };
        Ok(Some((fields, new)))
    }

    /// Mints a `Rekey` of all current members. `None` when [`Self::build_rekey_fields`] could not
    /// build it yet.
    pub(crate) fn mint_rekey(&mut self, feed: FeedId) -> Result<Option<MembershipEntry>, ApiError> {
        let Some(state) = self.tentative_state(feed)? else {
            return Ok(None);
        };
        let members: Vec<AccountId> = state.members.iter().map(|m| m.account).collect();
        let Some((fields, key)) = self.build_rekey_fields(feed, &members)? else {
            return Ok(None);
        };
        let epoch = fields.new_epoch;
        let entry = self.membership_mint(
            feed,
            EntryBody::Rekey(fields),
            Private::None,
            Some((epoch, key)),
        )?;
        tracing::info!(feed = %feed.short_hex(), epoch, "a Rekey was minted");
        Ok(Some(entry))
    }

    /// Mints a `Grant` of the current key for a member who joined with an older bundle. `None` when
    /// the member's device log or the key is not here yet.
    pub(crate) fn mint_grant(
        &mut self,
        feed: FeedId,
        member: AccountId,
    ) -> Result<Option<MembershipEntry>, ApiError> {
        let me = self.me()?;
        let Some(state) = self.tentative_state(feed)? else {
            return Ok(None);
        };
        let epoch = state.epoch;
        let Some(key) = self.feed_sk(feed, epoch)? else {
            return Ok(None);
        };
        if member != me && !self.shared.logs.contains_key(&member) {
            self.shared.wanted_logs.insert(member);
            return Ok(None);
        }
        let recipients = {
            let view = LogsView {
                me,
                own: self.security.log.as_ref(),
                foreign: &self.shared.logs,
            };
            rekey_recipients(&[member], &view).map_err(map_keys_error)?
        };
        let boxes: Vec<KeyBox> = recipients
            .devices
            .iter()
            .map(|(_, device, keys)| KeyBox {
                device: *device,
                bytes: seal_space_key_for(&key, feed, epoch, keys, self.rng.as_ref()),
            })
            .collect();
        let body = EntryBody::Grant {
            member,
            epoch,
            boxes,
        };
        Ok(Some(self.membership_mint(
            feed,
            body,
            Private::None,
            None,
        )?))
    }

    /// What `sync_membership_pending` mints by itself (see the module documentation): nothing while
    /// something this device minted is still on its way.
    pub(crate) fn membership_automint(&mut self, feed: FeedId) -> Result<(), ApiError> {
        if !self.unconfirmed_membership_rows(feed)?.is_empty() {
            return Ok(());
        }
        let Some((me, _)) = self.sync_identity()? else {
            return Ok(());
        };
        let Some(timeline) = self.shared.timelines.get(&feed).cloned() else {
            return Ok(());
        };
        let state = timeline.state().clone();
        if state.closed.is_some() || state.role_of(me).is_none() {
            return Ok(());
        }
        if self.feed_sk(feed, state.epoch)?.is_none() {
            return Ok(());
        }
        let joined_log = self
            .shared
            .metas
            .get(&feed)
            .map(|meta| meta.joined_log.clone())
            .unwrap_or_default();
        // A revocation in a member's own log that no `Rekey` covers: a device the member revoked may
        // still hold the key. Revocations from before the member joined do not count.
        let covered = |account: AccountId| -> u64 {
            let named = timeline.head_seq(account);
            let joined = joined_log.get(&account).copied();
            named.max(joined).unwrap_or(0)
        };
        let uncovered = |account: AccountId, log: Option<&kb_keys::log::DeviceLog>| -> bool {
            log.and_then(latest_revocation_seq)
                .is_some_and(|revoked| revoked > covered(account))
        };
        let own_uncovered = uncovered(me, self.security.log.as_ref());
        let others_uncovered = state.members.iter().any(|member| {
            member.account != me && uncovered(member.account, self.shared.logs.get(&member.account))
        });
        if state.pending_rekey || own_uncovered || others_uncovered {
            let asked = self.fcfg(feed, "rekey")?.is_some();
            if own_uncovered || asked || self.feed_has_unsent(feed)? {
                self.mint_rekey(feed)?;
                return Ok(());
            }
        }
        // A member who joined after the current epoch began, with an older bundle, has no box for it.
        let behind: Vec<AccountId> = state
            .members
            .iter()
            .filter(|member| member.account != me)
            .filter(|member| {
                self.shared
                    .metas
                    .get(&feed)
                    .and_then(|meta| meta.keyed.get(&member.account))
                    .is_some_and(|keyed| *keyed < state.epoch)
            })
            .map(|member| member.account)
            .collect();
        for member in behind {
            if self.mint_grant(feed, member)?.is_some() {
                break;
            }
        }
        Ok(())
    }

    /// The role of an account in the feed after the folded entries and the pending ones.
    #[allow(dead_code)]
    pub(crate) fn role_of_in_feed(
        &mut self,
        feed: FeedId,
        account: AccountId,
    ) -> Result<Option<MemberRole>, ApiError> {
        Ok(self
            .tentative_state(feed)?
            .and_then(|state| state.role_of(account)))
    }
}

impl Core {
    /// After this account revoked one of its devices (ADR-0022, ADR-0044): one `Rekey` per shared feed
    /// it is a member of, so that the revoked device is not boxed for the new epoch. A feed whose
    /// members' logs are not all here yet is left to the automatic path of `sync_membership_pending`
    /// (self-policing mints it once they are); a failure never fails the revocation.
    pub(crate) fn rekey_shared_spaces_after_revocation(&mut self) {
        let feeds: Vec<FeedId> = match self.db.sync_feed_list() {
            Ok(rows) => rows
                .into_iter()
                .filter(|row| row.kind == kb_types::sync::FeedKind::Shared)
                .map(|row| row.feed)
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "the shared feeds could not be listed for a rekey");
                return;
            }
        };
        for feed in feeds {
            let live = self.feed_is_live(feed).unwrap_or(false);
            let member = self
                .me()
                .ok()
                .and_then(|me| {
                    self.shared
                        .timelines
                        .get(&feed)
                        .and_then(|t| t.state().role_of(me))
                })
                .is_some();
            if !live || !member {
                continue;
            }
            if self
                .unconfirmed_membership_rows(feed)
                .map(|rows| !rows.is_empty())
                .unwrap_or(true)
            {
                continue;
            }
            match self.mint_rekey(feed) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    tracing::info!(feed = %feed.short_hex(), "the rekey after a revocation waits for the members' device logs")
                }
                Err(error) => {
                    tracing::warn!(feed = %feed.short_hex(), code = ?error.code, "the rekey after a revocation failed")
                }
            }
        }
    }
}
