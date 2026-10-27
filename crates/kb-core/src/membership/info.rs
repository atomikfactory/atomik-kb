//! The member panel's view of a shared space (`MembersInfo`): who is a member with which role, the
//! safety numbers, the invites an Owner sees, the settings, the open snapshot disputes and how many
//! changes were refused on this device.

use std::collections::{BTreeMap, HashMap, HashSet};

use kb_keys::log::LogEntryBody;
use kb_keys::membership::{InviteRecordStatus, MembershipState};
use kb_keys::safety::{genesis_hash, safety_number};
use kb_storage::FeedQueries;
use kb_types::{
    AccountId, ApiError, DisputeInfo, FormerMemberInfo, InviteInfo, InviteKind, InviteStatus,
    MemberInfo, MemberRole, MembersInfo, SpaceId,
};

use crate::{Core, map_storage_error};

impl Core {
    /// The membership of a shared space as the panel shows it (any member); `InvalidInput` for
    /// `spaceId` on a personal space.
    pub(crate) fn members_info(&mut self, space_id: SpaceId) -> Result<MembersInfo, ApiError> {
        let Some(row) = self.shared_feed_of(space_id)? else {
            return Err(ApiError::invalid("spaceId", "this space is not shared"));
        };
        let feed = row.feed;
        self.shared_load()?;
        let me = self.me()?;
        let now = self.clock.now_ms();
        let state = self
            .tentative_state(feed)?
            .unwrap_or_else(|| MembershipState::empty(feed));
        let meta = self.shared.metas.get(&feed).cloned().unwrap_or_default();
        let my_role = self.role_in_space(space_id)?.unwrap_or(MemberRole::Viewer);
        let refused: HashMap<AccountId, u32> = self
            .db
            .rejected_count_by_account(feed)
            .map_err(map_storage_error)?
            .into_iter()
            .collect();
        let own_genesis = self.security.log.as_ref().map(genesis_hash);
        let verified_at = |core: &Self, account: AccountId| -> bool {
            core.docs
                .account
                .as_ref()
                .is_some_and(|a| a.model.verified_at(account).is_some())
        };

        let names: Vec<String> = state
            .members
            .iter()
            .map(|member| self.member_name(feed, member.account))
            .collect();
        let mut members = Vec::with_capacity(state.members.len());
        for (member, name) in state.members.iter().zip(&names) {
            let is_me = member.account == me;
            let log = if is_me {
                self.security.log.as_ref()
            } else {
                self.shared.logs.get(&member.account)
            };
            let (device_count, via_kit_at) = match log {
                Some(log) => (
                    log.verify().map_or(0, |s| {
                        u32::try_from(s.devices.iter().filter(|d| d.revoked_at.is_none()).count())
                            .unwrap_or(u32::MAX)
                    }),
                    log.entries()
                        .iter()
                        .filter_map(|entry| match &entry.body {
                            LogEntryBody::DeviceAdded {
                                via_kit: true, at, ..
                            } => Some(*at),
                            _ => None,
                        })
                        .max(),
                ),
                None => (0, None),
            };
            let safety = if is_me {
                None
            } else {
                match (own_genesis, log.map(genesis_hash)) {
                    (Some(a), Some(b)) => Some(safety_number(&a, &b)),
                    _ => None,
                }
            };
            members.push(MemberInfo {
                account_id: member.account,
                name: name.clone(),
                role: member.role,
                joined_at: member.joined_at,
                is_me,
                verified: verified_at(self, member.account),
                safety_number: safety,
                device_count,
                via_kit_at,
                refused_changes: refused.get(&member.account).copied().unwrap_or(0),
                // The same name up to case: "alice" next to "Alice" is the lookalike the mark is for.
                name_shared: names
                    .iter()
                    .filter(|other| other.to_lowercase() == name.to_lowercase())
                    .count()
                    > 1,
            });
        }
        let invites = if my_role.is_owner() {
            state
                .invites
                .iter()
                .map(|invite| InviteInfo {
                    invite_id: invite.invite_id,
                    kind: if invite.bound_account.is_some() {
                        InviteKind::Code
                    } else {
                        InviteKind::Link
                    },
                    role: invite.role,
                    created_by: invite.created_by,
                    created_at: invite.created_at,
                    expires_at: invite.expires_at,
                    bound_account: invite.bound_account,
                    status: match invite.status {
                        InviteRecordStatus::Open if invite.is_expired(now) => InviteStatus::Expired,
                        InviteRecordStatus::Open => InviteStatus::Open,
                        InviteRecordStatus::Used { .. } => InviteStatus::Used,
                        InviteRecordStatus::Revoked => InviteStatus::Revoked,
                    },
                    used_by: match invite.status {
                        InviteRecordStatus::Used { by } => Some(by),
                        _ => None,
                    },
                })
                .collect()
        } else {
            Vec::new()
        };
        let disputes: Vec<DisputeInfo> = if my_role.is_owner() {
            self.disputes_of(feed)?
                .into_iter()
                .map(|(doc_id, account_id, at)| DisputeInfo {
                    page_id: self.page_of_doc(doc_id),
                    account_id,
                    at,
                })
                .collect()
        } else {
            Vec::new()
        };
        let refused_total = refused.values().copied().fold(0u32, u32::saturating_add);
        // Who was a member and is not, with the last name the space knew them by: a removed member's
        // (`former_names`) and the name a member who left gave (`names` keeps it). A person who joined
        // again is a member, so not listed.
        let current: HashSet<AccountId> =
            state.members.iter().map(|member| member.account).collect();
        let mut former: BTreeMap<AccountId, String> = meta
            .names
            .iter()
            .filter(|(account, _)| !current.contains(account))
            .map(|(account, name)| (*account, name.clone()))
            .collect();
        for (account, name) in &meta.former_names {
            if !current.contains(account) {
                former.insert(*account, name.clone());
            }
        }
        let former_members: Vec<FormerMemberInfo> = former
            .into_iter()
            .map(|(account_id, name)| FormerMemberInfo { account_id, name })
            .collect();
        Ok(MembersInfo {
            space_id,
            feed_id: feed,
            my_role,
            epoch: state.epoch,
            pending_rekey: state.pending_rekey || self.fcfg(feed, "rekey")?.is_some(),
            closed_at: meta.closed_at,
            members,
            former_members,
            invites,
            settings: meta.settings,
            disputes,
            refused_total,
            server_flagged: self.fcfg(feed, "flagged")?.as_deref() == Some("1"),
        })
    }
}
