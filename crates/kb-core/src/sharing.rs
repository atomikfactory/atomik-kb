//! Sharing a space (M13 slice 1, ADR-0041, ADR-0043, ADR-0044, ADR-0045 and ADR-0047,
//! `docs/development/milestone-13.md` sections 1.1 and 1.5): `SpaceShare` and its plan, the
//! members, role and settings changes, removal, leaving, closing, the per-account AI switch, and the
//! role gate every write request goes through.
//!
//! The requests mint membership entries (`Core::membership_mint`, `membership/mint.rs`) validated
//! against the tentative timeline; the driver posts them by compare-and-swap, and what they mean
//! (events, archives, keys) happens when they come back through the feed and fold
//! (`membership/ingest.rs`). The answers of the requests read the tentative state, so the panel
//! shows the change at once.
//!
//! **Leaving** keeps the space hidden but registered until the `Leave` is on the server
//! (`leaving` in the feed's settings): only then the local data goes (`space_drop_local`), so a
//! crash or an offline period never loses the entry.

use std::collections::HashMap;

use kb_keys::membership::{EntryBody, InviteRecordStatus, Private};
use kb_model::StructureDoc;
use kb_storage::{
    AttachmentQueries, ChunkSyncQueries, CommentQueries, DateMentionQueries, DocKindCode,
    DocQueries, FeedQueries, IconRefQueries, NotificationQueries, PagePropertyQueries,
    PersonMentionQueries, SyncFeedRow, SyncQueries, TextIndexQueries,
};
use kb_types::sharing::{MAX_MEMBERS_PER_FEED, MAX_OPEN_INVITES, SpaceSettings};
use kb_types::{
    AccountId, ApiError, ArchiveReason, DocId, ErrorCode, Event, MemberRole, MembersInfo, PageId,
    Request, Response, SpaceId, SpaceInfo, SpaceSharePlan, SpaceShareState,
};

use crate::documents::map_model_error;
use crate::{Core, map_storage_error};

/// The longest a space's trash may be kept, in days (the mirror's limit).
const MAX_TRASH_RETENTION_DAYS: u32 = 3650;

impl Core {
    /// Routes the sharing requests: `SpaceSharePlan`, `SpaceShare`, `SpaceMembers`, `MemberRoleSet`,
    /// `MemberRemove`, `SpaceLeave`, `SpaceClose`, `SpaceSettingsSet` and `SpaceAiSet`.
    pub(crate) fn dispatch_sharing(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::SpaceSharePlan { space_id } => {
                Ok(Response::SpaceSharePlan(self.space_share_plan(space_id)?))
            }
            Request::SpaceShare { space_id } => Ok(Response::Space(self.space_share(space_id)?)),
            Request::SpaceMembers { space_id } => {
                Ok(Response::Members(self.space_members(space_id)?))
            }
            Request::MemberRoleSet {
                space_id,
                account_id,
                role,
            } => Ok(Response::Members(
                self.member_role_set(space_id, account_id, role)?,
            )),
            Request::MemberRemove {
                space_id,
                account_id,
            } => Ok(Response::Members(self.member_remove(space_id, account_id)?)),
            Request::SpaceLeave { space_id } => {
                self.space_leave(space_id)?;
                Ok(Response::Ok)
            }
            Request::SpaceClose { space_id } => Ok(Response::Space(self.space_close(space_id)?)),
            Request::SpaceSettingsSet {
                space_id,
                trash_retention_days,
                editors_may_link,
            } => Ok(Response::Members(self.space_settings_set(
                space_id,
                trash_retention_days,
                editors_may_link,
            )?)),
            Request::SpaceAiSet { space_id, enabled } => {
                Ok(Response::Space(self.space_ai_set(space_id, enabled)?))
            }
            other => Err(ApiError::internal(format!(
                "{} is not a sharing request",
                other.kind()
            ))),
        }
    }

    // ----- what the lists show --------------------------------------------------------------

    /// The shared spaces of this device by space id (archives included): the rows of `sync_feeds`.
    pub(crate) fn shared_rows(&self) -> HashMap<SpaceId, SyncFeedRow> {
        self.db
            .sync_feed_list()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.kind == kb_types::sync::FeedKind::Shared)
            .filter_map(|row| row.space_id.map(|space| (space, row)))
            .collect()
    }

    /// Whether the space is going: this account left it and the `Leave` is not on the server yet.
    pub(crate) fn space_is_leaving(&self, row: &SyncFeedRow) -> bool {
        self.fcfg(row.feed, "leaving").ok().flatten().is_some()
    }

    /// Adds what sharing says to a listed space (`shareState`, `role`, `archiveReason`,
    /// `memberCount`, `aiEnabled`), from the folded timeline; a space whose `Create` the server has
    /// not taken is `Pending`.
    pub(crate) fn decorate_space(
        &self,
        rows: &HashMap<SpaceId, SyncFeedRow>,
        mut info: SpaceInfo,
    ) -> SpaceInfo {
        let account = self.docs.account.as_ref().map(|open| &open.model);
        let ai = account.and_then(|a| a.space_ai(info.id));
        let Some(row) = rows.get(&info.id) else {
            info.ai_enabled = ai.unwrap_or(true);
            return info;
        };
        info.ai_enabled = ai.unwrap_or(false);
        let archived = self.feed_archived(row.feed).ok().flatten();
        let state = self.shared.timelines.get(&row.feed).map(|t| t.state());
        let folded = state.is_some_and(|state| !state.members.is_empty());
        info.share_state = if archived.is_some() {
            SpaceShareState::Archived
        } else if folded {
            SpaceShareState::Shared
        } else {
            SpaceShareState::Pending
        };
        info.archive_reason = archived;
        let me = self.sync_identity().ok().flatten().map(|(me, _)| me);
        let cached = self
            .fcfg(row.feed, "role")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u8>().ok())
            .and_then(MemberRole::from_byte);
        info.role = match (state.zip(me).and_then(|(s, me)| s.role_of(me)), archived) {
            (_, Some(_)) => cached.or(Some(MemberRole::Viewer)),
            (Some(role), None) => Some(role),
            (None, None) if folded => cached.or(Some(MemberRole::Viewer)),
            // Not folded yet: this account made the space (`Pending`), or is joining it.
            (None, None) => cached.or(Some(MemberRole::Owner)),
        };
        info.member_count = match state {
            Some(state) if !state.members.is_empty() => {
                u32::try_from(state.members.len()).unwrap_or(u32::MAX)
            }
            _ => 1,
        };
        info
    }

    /// [`Self::decorate_space`] for one space.
    pub(crate) fn decorated_space(&self, info: SpaceInfo) -> SpaceInfo {
        let rows = self.shared_rows();
        self.decorate_space(&rows, info)
    }

    /// Whether local AI and MCP may use the space on this account's devices (ADR-0047): the account
    /// document's switch, default `true` for a personal space and `false` for a shared one.
    pub(crate) fn ai_space_enabled(&self, space_id: SpaceId) -> bool {
        let chosen = self
            .docs
            .account
            .as_ref()
            .and_then(|open| open.model.space_ai(space_id));
        match chosen {
            Some(enabled) => enabled,
            None => !self.is_shared_space(space_id),
        }
    }

    /// Whether this device fires a reminder of a date mention in a space (ADR-0047). A personal
    /// space's reminders always fire here. A shared space's fire while the space is live and the
    /// account is a member: on the devices of `reminder_for` when the mention names an account (every
    /// role, a Viewer included: it is theirs), and on an Owner's device when it names nobody.
    pub(crate) fn fires_reminder(
        &self,
        space_id: SpaceId,
        reminder_for: Option<kb_types::AccountId>,
    ) -> bool {
        let rows = self.shared_rows();
        let Some(row) = rows.get(&space_id) else {
            return true;
        };
        if self.feed_archived(row.feed).ok().flatten().is_some() || self.space_is_leaving(row) {
            return false;
        }
        let Some((me, _)) = self.sync_identity().ok().flatten() else {
            return false;
        };
        let folded = self
            .shared
            .timelines
            .get(&row.feed)
            .and_then(|t| t.state().role_of(me));
        match reminder_for {
            Some(account) => account == me && folded.is_some(),
            None => folded.is_some_and(MemberRole::is_owner),
        }
    }

    /// Whether the space has a feed on this device (an archive too).
    pub(crate) fn is_shared_space(&self, space_id: SpaceId) -> bool {
        self.db
            .sync_feed_list()
            .unwrap_or_default()
            .iter()
            .any(|row| {
                row.kind == kb_types::sync::FeedKind::Shared && row.space_id == Some(space_id)
            })
    }

    // ----- the plan and the share -----------------------------------------------------------

    /// What `SpaceShare` would send; writes nothing. Any space of the workspace (`NotFound`
    /// otherwise).
    pub(crate) fn space_share_plan(
        &mut self,
        space_id: SpaceId,
    ) -> Result<SpaceSharePlan, ApiError> {
        let open = self.structure(space_id)?;
        let page_count = open.model.live_page_count();
        let trashed_page_count = open
            .model
            .trashed_pages()
            .iter()
            .map(|entry| 1 + entry.descendant_count)
            .sum();
        let structure_doc = open.doc_id;
        let attachments = self
            .db
            .attachments_by_space(space_id)
            .map_err(map_storage_error)?;
        let attachment_bytes: u64 = attachments.iter().map(|row| row.size).sum();
        // The sealed size of the full-history snapshots: the stored history of every document of the
        // space (what a snapshot holds is at most that), plus the framing of each.
        let mut docs = vec![structure_doc];
        for row in self
            .db
            .docs_by_kind(DocKindCode::Page)
            .map_err(map_storage_error)?
        {
            if row.space_id == Some(space_id) {
                docs.push(row.doc_id);
            }
        }
        let mut history_bytes: u64 = 0;
        for doc in &docs {
            let stats = self.db.update_stats(*doc).map_err(map_storage_error)?;
            let snapshot = self
                .db
                .snapshot_get(*doc)
                .map_err(map_storage_error)?
                .map_or(0, |(blob, _)| blob.len() as u64);
            history_bytes = history_bytes
                .saturating_add(stats.bytes)
                .saturating_add(snapshot)
                .saturating_add(128);
        }
        Ok(SpaceSharePlan {
            space_id,
            page_count,
            trashed_page_count,
            attachment_count: u32::try_from(attachments.len()).unwrap_or(u32::MAX),
            attachment_bytes,
            history_bytes,
            server_configured: self.sync_server_url()?.is_some(),
            has_account: self.sync_identity()?.is_some(),
            has_profile_name: self.profile_name_of_account().is_some(),
            already_shared: self.is_shared_space(space_id),
        })
    }

    /// This account's display name (the account document's `profile`).
    pub(crate) fn profile_name_of_account(&self) -> Option<String> {
        self.docs
            .account
            .as_ref()
            .and_then(|open| open.model.profile_name())
    }

    /// Shares a personal space (the five steps of ADR-0045, section 10); answers the space with
    /// `share_state: Pending`.
    pub(crate) fn space_share(&mut self, space_id: SpaceId) -> Result<SpaceInfo, ApiError> {
        let space_name = self.structure(space_id)?.model.space().name;
        if self.is_shared_space(space_id) {
            return Err(ApiError::new(
                ErrorCode::AlreadyExists,
                "the space is already shared",
            ));
        }
        if self.sync_server_url()?.is_none() {
            return Err(ApiError::invalid(
                "serverUrl",
                "turn sync on before sharing a space",
            ));
        }
        if self.sync_identity()?.is_none() {
            return Err(ApiError::invalid(
                "account",
                "create your recovery kit before sharing a space",
            ));
        }
        let Some(profile) = self.profile_name_of_account() else {
            return Err(ApiError::invalid(
                "displayName",
                "set the name your co-members see before sharing a space",
            ));
        };
        // 1. SK[e + 1] and the `Create` (which registers the feed and writes `sharedSpaces`).
        self.mint_create(space_id, &space_name, SpaceSettings::default(), &profile)?;
        // 2. The documents of the space go into the feed at the next cycles (the routing follows the
        //    space); 3. the attachment keys are wrapped under the new epoch and the chunks copied
        //    server-side from the personal namespace.
        self.share_rewrap_attachment_keys(space_id)?;
        self.chunks_follow_space_to_feed(space_id)?;
        // The owner's favourites move to the account document, and the space's map is cleared.
        self.adopt_favorites_of(space_id)?;
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::StructureChanged { space_id });
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        tracing::info!("a space was shared");
        self.space_info_now(space_id)
    }

    /// The share step's favourites: `favorites/<space>` of the account document takes the structure
    /// document's favourites, which are then cleared.
    fn adopt_favorites_of(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        self.ensure_account()?;
        let moved = {
            let Some(open) = self.docs.structures.get_mut(&space_id) else {
                return Ok(());
            };
            let Some(account) = self.docs.account.as_ref() else {
                return Ok(());
            };
            account
                .model
                .adopt_favorites(space_id, &mut open.model)
                .map_err(map_model_error)?
        };
        if moved > 0 {
            self.persist_structure(space_id, Some(&[]), kb_storage::UpdateOrigin::Local)?;
        }
        self.persist_account()
    }

    /// The space as a request answers it: the tentative state (a pending `Close` already shows as an
    /// archive).
    pub(crate) fn space_info_now(&mut self, space_id: SpaceId) -> Result<SpaceInfo, ApiError> {
        let model = &self.structure(space_id)?.model;
        let meta = model.space();
        let info = SpaceInfo {
            id: space_id,
            name: meta.name,
            icon: meta.icon,
            created_at: meta.created_at,
            page_count: model.live_page_count(),
            share_state: SpaceShareState::Personal,
            role: None,
            archive_reason: None,
            member_count: 0,
            ai_enabled: true,
        };
        let mut info = self.decorated_space(info);
        if let Some(row) = self.shared_feed_of(space_id)?
            && info.share_state != SpaceShareState::Archived
            && let Some(state) = self.tentative_state(row.feed)?
        {
            if state.closed.is_some() {
                info.share_state = SpaceShareState::Archived;
                info.archive_reason = Some(ArchiveReason::Closed);
            }
            let count = u32::try_from(state.members.len()).unwrap_or(u32::MAX);
            if count > 0 {
                info.member_count = count;
            }
            if let Some(role) = self.me().ok().and_then(|me| state.role_of(me)) {
                info.role = Some(role);
            }
        }
        Ok(info)
    }

    // ----- members --------------------------------------------------------------------------

    /// The membership of a shared space as the panel shows it; `InvalidInput` for `spaceId` on a
    /// personal space.
    pub(crate) fn space_members(&mut self, space_id: SpaceId) -> Result<MembersInfo, ApiError> {
        self.shared_space_for_request(space_id)?;
        self.members_info(space_id)
    }

    /// The shared space of a request: `NotFound` for an unknown space, `InvalidInput` (`spaceId`) for
    /// a personal one.
    pub(crate) fn shared_space_for_request(
        &mut self,
        space_id: SpaceId,
    ) -> Result<SyncFeedRow, ApiError> {
        if !self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::not_found("space not found"));
        }
        match self.shared_feed_of(space_id)? {
            Some(row) if !self.space_is_leaving(&row) => Ok(row),
            Some(_) => Err(ApiError::not_found("space not found")),
            None => Err(ApiError::invalid("spaceId", "this space is not shared")),
        }
    }

    /// `RoleSet` (Owner); refused when it would leave no Owner.
    pub(crate) fn member_role_set(
        &mut self,
        space_id: SpaceId,
        account_id: AccountId,
        role: MemberRole,
    ) -> Result<MembersInfo, ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        let state = self
            .tentative_state(row.feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        let Some(previous) = state.role_of(account_id) else {
            return Err(ApiError::invalid(
                "accountId",
                "this person is not a member of the space",
            ));
        };
        if previous == role {
            return self.members_info(space_id);
        }
        if previous.is_owner() && state.owners().len() <= 1 {
            return Err(ApiError::invalid(
                "role",
                "a space needs at least one Owner: promote someone else first",
            ));
        }
        self.membership_mint(
            row.feed,
            EntryBody::RoleSet {
                target: account_id,
                role,
            },
            Private::None,
            None,
        )?;
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        if self.me().is_ok_and(|me| me == account_id) {
            self.push_event_once(Event::SpacesChanged);
        }
        self.members_info(space_id)
    }

    /// `Remove` with its Rekey (Owner, target is not this account).
    pub(crate) fn member_remove(
        &mut self,
        space_id: SpaceId,
        account_id: AccountId,
    ) -> Result<MembersInfo, ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        let me = self.me()?;
        let state = self
            .tentative_state(row.feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        if state.role_of(account_id).is_none() {
            return Err(ApiError::invalid(
                "accountId",
                "this person is not a member of the space",
            ));
        }
        if account_id == me {
            return Err(ApiError::invalid(
                "accountId",
                "you cannot remove yourself: leave the space instead",
            ));
        }
        let remaining: Vec<AccountId> = state
            .members
            .iter()
            .map(|member| member.account)
            .filter(|account| *account != account_id)
            .collect();
        let Some((fields, key)) = self.build_rekey_fields(row.feed, &remaining)? else {
            return Err(ApiError::invalid(
                "membership",
                "a member's device log is not known here yet; sync and try again",
            ));
        };
        let epoch = fields.new_epoch;
        self.membership_mint(
            row.feed,
            EntryBody::Remove {
                target: account_id,
                rekey: fields,
            },
            Private::None,
            Some((epoch, key)),
        )?;
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.members_info(space_id)
    }

    /// `Leave` (a member, not the last Owner), then the local data goes once the entry is on the
    /// server; on an archive it only deletes the local copy.
    pub(crate) fn space_leave(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        if self.feed_archived(row.feed)?.is_some() {
            return self.space_drop_local(space_id);
        }
        // A space whose `Create` never reached the server is only undone locally.
        let on_server = self.fcfg_u64(row.feed, "posted")?.is_some()
            || self
                .shared
                .timelines
                .get(&row.feed)
                .is_some_and(|t| !t.is_empty());
        if !on_server {
            return self.space_drop_local(space_id);
        }
        let me = self.me()?;
        let state = self
            .tentative_state(row.feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        match state.role_of(me) {
            Some(role) if role.is_owner() && state.owners().len() <= 1 => {
                return Err(ApiError::invalid(
                    "role",
                    "you are the last Owner: promote someone first, or close the space",
                ));
            }
            Some(_) => {}
            // Not a member any more: only the archive is left (the removal has not folded here yet).
            None => return self.space_drop_local(space_id),
        }
        let entry = self.membership_mint(row.feed, EntryBody::Leave, Private::None, None)?;
        self.fcfg_set(row.feed, "leaving", &entry.header.seq.to_string())?;
        self.push_event_once(Event::SpacesChanged);
        self.push_event_once(Event::SyncRequested);
        Ok(())
    }

    /// The spaces this account left whose `Leave` the server has taken lose their local data. Runs at
    /// the start of a sync cycle (`sync_feeds`), when no step of a feed is under way.
    pub(crate) fn leave_finalize_all(&mut self) -> Result<(), ApiError> {
        let rows: Vec<SyncFeedRow> = self.shared_rows().into_values().collect();
        for row in rows {
            let Some(leaving) = self.fcfg_u64(row.feed, "leaving")? else {
                continue;
            };
            let posted = self.fcfg_u64(row.feed, "posted")?;
            if posted.is_some_and(|posted| posted >= leaving)
                && let Some(space_id) = row.space_id
            {
                self.space_drop_local(space_id)?;
            }
        }
        Ok(())
    }

    /// `Close` (Owner): the space becomes an archive for everyone once the entry folds.
    pub(crate) fn space_close(&mut self, space_id: SpaceId) -> Result<SpaceInfo, ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        self.membership_mint(row.feed, EntryBody::Close, Private::None, None)?;
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        self.space_info_now(space_id)
    }

    /// `Settings` (Owner); a field left out stays.
    pub(crate) fn space_settings_set(
        &mut self,
        space_id: SpaceId,
        trash_retention_days: Option<u32>,
        editors_may_link: Option<bool>,
    ) -> Result<MembersInfo, ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        if let Some(days) = trash_retention_days
            && days > MAX_TRASH_RETENTION_DAYS
        {
            return Err(ApiError::invalid(
                "trashRetentionDays",
                format!("must be 0 (keep) or a number of days up to {MAX_TRASH_RETENTION_DAYS}"),
            ));
        }
        let current = self
            .shared
            .metas
            .get(&row.feed)
            .map(|meta| meta.settings)
            .unwrap_or_default();
        let settings = SpaceSettings {
            trash_retention_days: trash_retention_days.unwrap_or(current.trash_retention_days),
            editors_may_link: editors_may_link.unwrap_or(current.editors_may_link),
        };
        if settings == current {
            return self.members_info(space_id);
        }
        self.membership_mint(
            row.feed,
            EntryBody::Settings,
            Private::Settings { settings },
            None,
        )?;
        // The panel shows the change at once; the entry sets the same value when it folds.
        self.shared.metas.entry(row.feed).or_default().settings = settings;
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.members_info(space_id)
    }

    /// The account's AI switch for a space (any member): `spaceSettings` of the account document.
    /// Turning it off revokes the MCP grants of the space and clears its rows of the semantic index.
    pub(crate) fn space_ai_set(
        &mut self,
        space_id: SpaceId,
        enabled: bool,
    ) -> Result<SpaceInfo, ApiError> {
        self.structure(space_id)?;
        let before = self.ai_space_enabled(space_id);
        self.ensure_account()?;
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .set_space_ai(space_id, enabled)
                .map_err(map_model_error)?;
        }
        self.persist_account()?;
        if before != enabled {
            if !enabled {
                self.mcp_revoke_space(space_id)?;
                let pages: Vec<PageId> = self
                    .db
                    .pages_index_by_space(space_id)
                    .map_err(map_storage_error)?
                    .into_iter()
                    .map(|row| row.page_id)
                    .collect();
                self.ai_forget_pages(&pages)?;
            } else {
                self.events.push(Event::AiIndexChanged);
            }
            self.push_event_once(Event::SpacesChanged);
        }
        self.space_info_now(space_id)
    }

    // ----- the gate -------------------------------------------------------------------------

    /// The role gate of a request that writes in a space (the table "Role gating of requests" of
    /// milestone-13.md, 1.5): `Ok` when this account's role in the space is at least `required`,
    /// otherwise `Forbidden` with `ApiError::forbidden_role(required)`; any write to an archive is
    /// `ApiError::forbidden_archived()`. A personal space always passes (its owner is its only
    /// member). Every request of that table calls it first.
    pub(crate) fn require_role(
        &mut self,
        space_id: SpaceId,
        required: MemberRole,
    ) -> Result<(), ApiError> {
        // A hostile or out-of-date device of the tests writes whatever the role says.
        #[cfg(test)]
        if self.shared.force_push {
            return Ok(());
        }
        let Some(row) = self.shared_feed_of(space_id)? else {
            return Ok(());
        };
        if self.feed_archived(row.feed)?.is_some() || self.space_is_leaving(&row) {
            return Err(ApiError::forbidden_archived());
        }
        self.shared_load()?;
        // The hot path (every content save) reads the folded timeline; a feed whose entries have not
        // folded yet (a space just shared, a join in flight) takes the tentative state.
        let folded = self.sync_identity()?.and_then(|(me, _)| {
            self.shared
                .timelines
                .get(&row.feed)
                .and_then(|t| t.state().role_of(me))
        });
        let role = match folded {
            Some(role) => Some(role),
            None => self.role_in_space(space_id)?,
        };
        match role {
            Some(role) if role >= required => Ok(()),
            _ => Err(ApiError::forbidden_role(required)),
        }
    }

    /// Whether the open invites and the members of a feed leave room for one more (the spec's
    /// limits): `InvalidInput` (`invites` or `spaceId`) otherwise.
    pub(crate) fn check_invite_room(&mut self, feed: kb_types::FeedId) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let state = self
            .tentative_state(feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        let open = state
            .invites
            .iter()
            .filter(|invite| {
                matches!(invite.status, InviteRecordStatus::Open) && !invite.is_expired(now)
            })
            .count();
        if open >= MAX_OPEN_INVITES {
            return Err(ApiError::invalid(
                "invites",
                format!("a space has at most {MAX_OPEN_INVITES} open invites: revoke one first"),
            ));
        }
        if state.members.len() >= MAX_MEMBERS_PER_FEED {
            return Err(ApiError::invalid(
                "spaceId",
                format!("a space has at most {MAX_MEMBERS_PER_FEED} members"),
            ));
        }
        Ok(())
    }

    // ----- the local copy -------------------------------------------------------------------

    /// Deletes the local copy of a space with everything that belongs to it, and tells nobody: no
    /// tombstone, no chunk delete (leaving, deleting an archive, undoing a share the server never
    /// took). The pages, the structure document, the derived rows, the attachments' chunks, the keys,
    /// the feed's bookkeeping, the account document's entries of the space (`sharedSpaces`,
    /// `favorites`, `spaceSettings`, the order), the MCP grants and the semantic index rows.
    pub(crate) fn space_drop_local(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        // The device-local state of the space's view links goes first (it is read from the structure).
        self.view_links_forget_space(space_id);
        let feed = self.shared_feed_of(space_id)?.map(|row| row.feed);
        let Some(open) = self.docs.structures.get(&space_id) else {
            return Err(ApiError::not_found("space not found"));
        };
        let structure_doc = open.doc_id;
        let manifests = open.model.attachments();
        let pages: Vec<PageId> = self
            .db
            .pages_index_by_space(space_id)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|row| row.page_id)
            .collect();
        let mut doc_ids = vec![structure_doc];
        for kind in [DocKindCode::Page, DocKindCode::Comments] {
            for row in self.db.docs_by_kind(kind).map_err(map_storage_error)? {
                if row.space_id == Some(space_id) {
                    doc_ids.push(row.doc_id);
                }
            }
        }
        // The attachments: chunks, transfer rows, index rows (their keys go with the keyring below).
        let rows = self
            .db
            .attachments_by_space(space_id)
            .map_err(map_storage_error)?;
        for manifest in &manifests {
            for chunk in &manifest.chunks {
                if let Err(error) = self.blob_store.delete(chunk) {
                    tracing::warn!(%error, "a chunk of a dropped space could not be deleted");
                }
            }
        }
        for row in &rows {
            self.db
                .chunk_sync_delete_attachment(row.attachment_id)
                .map_err(map_storage_error)?;
            self.db
                .attachment_delete(row.attachment_id)
                .map_err(map_storage_error)?;
        }
        self.db
            .transaction(|tx| {
                for page in &pages {
                    tx.fts_delete_page(*page)?;
                    tx.links_delete_page(*page)?;
                    tx.date_mentions_delete_page(*page)?;
                    tx.person_mentions_delete(DocId::for_page(*page))?;
                    tx.comment_records_delete_page(*page)?;
                    tx.comment_fts_delete_page(*page)?;
                }
                tx.notification_delete_space(space_id)?;
                tx.page_properties_delete_pages(&pages)?;
                tx.attachment_refs_delete_pages(&pages)?;
                tx.pages_index_clear_space(space_id)?;
                tx.page_properties_clear_space(space_id)?;
                tx.icon_refs_replace_space(space_id, &[])?;
                for doc in &doc_ids {
                    tx.doc_delete(*doc)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        for page in &pages {
            self.docs.pages.remove(page);
        }
        self.docs.structures.remove(&space_id);
        self.ai_forget_pages(&pages)?;
        self.mcp_revoke_space(space_id)?;
        // The account document: the order, the feed, the favourites, the settings.
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .remove_space(space_id)
                .map_err(map_model_error)?;
            account
                .model
                .remove_shared_space(space_id)
                .map_err(map_model_error)?;
            account
                .model
                .remove_favorites(space_id)
                .map_err(map_model_error)?;
            account
                .model
                .remove_space_settings(space_id)
                .map_err(map_model_error)?;
            self.persist_account()?;
        }
        self.keyring_forget_space(space_id)?;
        if let Some(feed) = feed {
            self.feed_forget(feed)?;
        }
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::SyncChanged);
        self.events.push(Event::AttachmentsChanged { space_id });
        tracing::info!("the local copy of a space was deleted");
        Ok(())
    }

    /// Forgets a shared feed on this device: its rows, its settings and the memory of its timeline.
    pub(crate) fn feed_forget(&mut self, feed: kb_types::FeedId) -> Result<(), ApiError> {
        self.db.sync_feed_delete(feed).map_err(map_storage_error)?;
        let prefix = crate::membership::feed_key(feed, "");
        for (key, _) in self.db.sync_config_all().map_err(map_storage_error)? {
            if key.starts_with(&prefix) {
                self.db
                    .sync_config_delete(&key)
                    .map_err(map_storage_error)?;
            }
        }
        self.shared.timelines.remove(&feed);
        self.shared.metas.remove(&feed);
        self.shared.keyring_in_flight.remove(&feed);
        self.shared.snapshots_checked_at.remove(&feed);
        self.shared.forkout_due.remove(&feed);
        self.shared.stalled.remove(&feed);
        Ok(())
    }

    /// Whether this device may write tombstones and purge in the space: a personal space always; a
    /// shared one only on an Owner's device while it is live (ADR-0047).
    pub(crate) fn may_purge_in(&mut self, space_id: SpaceId) -> bool {
        self.require_role(space_id, MemberRole::Owner).is_ok()
    }

    /// The structure document of a space, for the code that needs a read-only look at it.
    #[allow(dead_code)]
    pub(crate) fn structure_model(&self, space_id: SpaceId) -> Option<&StructureDoc> {
        self.docs.structures.get(&space_id).map(|open| &open.model)
    }
}
