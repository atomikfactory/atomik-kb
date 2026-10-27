//! What a lost compare-and-swap takes away from the request side (ADR-0043, "Conflicts"): the loser
//! re-reads the log, re-evaluates its intent and tries again.
//!
//! `sync_membership_conflict` drops the entries this device minted on a stale head. The ones a person
//! asked for (an invite, a revocation, a role, a removal, a leave, settings, a name, a close) are
//! noted here as intents and written again once the log has caught up (`refetch` is clear): a role
//! of a member who left meanwhile, a removal of a member already removed, a `Leave` of an account that
//! is no longer a member are refused by the fold's rules and dropped silently, which is the
//! "no-op" of the spec; everything else lands on top of the new head. Entries the protocol asks any
//! member for (`Rekey`, `Grant`) are derived again by `membership_automint`, a `Join` by its session.

use kb_keys::membership::{EntryBody, MembershipEntry, Private};
use kb_keys::space_keys::membership_meta_key;
use kb_types::{AccountId, ApiError, FeedId};

use crate::Core;
use crate::security::{decode_hex, hex};

/// The most intents kept for one feed.
const MAX_INTENTS: usize = 32;

impl Core {
    fn reintents(&self, feed: FeedId) -> Result<Vec<String>, ApiError> {
        Ok(self
            .fcfg(feed, "reintent")?
            .unwrap_or_default()
            .split(';')
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn reintents_set(&self, feed: FeedId, list: &[String]) -> Result<(), ApiError> {
        if list.is_empty() {
            return self.fcfg_del(feed, "reintent");
        }
        self.fcfg_set(feed, "reintent", &list.join(";"))
    }

    /// Notes the entries this device minted for a person's request that the lost compare-and-swap
    /// dropped (`dropped` are their encodings).
    pub(crate) fn reintents_note(
        &mut self,
        feed: FeedId,
        dropped: &[Vec<u8>],
    ) -> Result<(), ApiError> {
        let me = self.me()?;
        // The invite an invite session writes again by itself.
        let session_invite = self
            .invite
            .as_ref()
            .filter(|session| session.feed == Some(feed))
            .and_then(|session| session.secret.as_ref())
            .map(kb_keys::invite::invite_id);
        let mut list = self.reintents(feed)?;
        for bytes in dropped {
            let Ok(entry) = MembershipEntry::decode(bytes) else {
                continue;
            };
            if entry.header.account != me {
                continue;
            }
            match &entry.header.body {
                EntryBody::Invite { invite_id, .. } if Some(*invite_id) == session_invite => {}
                EntryBody::Invite { .. }
                | EntryBody::InviteRevoke { .. }
                | EntryBody::RoleSet { .. }
                | EntryBody::Leave
                | EntryBody::Settings
                | EntryBody::Profile
                | EntryBody::Close => list.push(format!("e:{}", hex(bytes))),
                EntryBody::Remove { target, .. } => {
                    list.push(format!("r:{}", hex(target.as_bytes())));
                }
                _ => {}
            }
        }
        if list.len() > MAX_INTENTS {
            let drop = list.len() - MAX_INTENTS;
            list.drain(..drop);
        }
        self.reintents_set(feed, &list)
    }

    /// Writes the noted intents again, once the membership log is read up to date (nothing is minted
    /// on a log that is being re-read). An intent that waits for a device log stays noted.
    pub(crate) fn reintents_replay(&mut self, feed: FeedId) -> Result<(), ApiError> {
        if self.fcfg(feed, "refetch")?.is_some() || self.shared.stalled.contains(&feed) {
            return Ok(());
        }
        self.invite_remint_pending(feed);
        let list = self.reintents(feed)?;
        if list.is_empty() {
            return Ok(());
        }
        self.reintents_set(feed, &[])?;
        let mut keep = Vec::new();
        for item in list {
            if self.reintent_one(feed, &item) == Retry::Later {
                keep.push(item);
            }
        }
        if !keep.is_empty() {
            let mut list = self.reintents(feed)?;
            keep.append(&mut list);
            self.reintents_set(feed, &keep)?;
        }
        Ok(())
    }

    /// One intent: written again, or dropped (the rules of the log refuse it now), or kept for later
    /// (a device log is missing).
    fn reintent_one(&mut self, feed: FeedId, item: &str) -> Retry {
        let minted = if let Some(rest) = item.strip_prefix("e:") {
            self.reintent_entry(feed, rest)
        } else if let Some(rest) = item.strip_prefix("r:") {
            self.reintent_remove(feed, rest)
        } else {
            Ok(())
        };
        match minted {
            Ok(()) => Retry::No,
            Err(error) => {
                tracing::debug!(code = ?error.code, "an entry dropped by a lost compare-and-swap is not written again");
                if error.message.contains("not known here yet") {
                    Retry::Later
                } else {
                    Retry::No
                }
            }
        }
    }

    fn reintent_entry(&mut self, feed: FeedId, hex_bytes: &str) -> Result<(), ApiError> {
        let bytes = decode_hex(hex_bytes).ok_or_else(|| ApiError::internal("bad intent"))?;
        let entry = MembershipEntry::decode(&bytes)
            .map_err(|_| ApiError::internal("an intent does not decode"))?;
        let private = match entry.header.body {
            EntryBody::Profile | EntryBody::Settings => {
                let key = self
                    .feed_sk(feed, entry.header.meta_epoch)?
                    .ok_or_else(|| ApiError::crypto("the key of an intent is not held"))?;
                entry
                    .open_private(&membership_meta_key(&key, feed))
                    .map_err(crate::map_keys_error)?
            }
            _ => Private::None,
        };
        self.membership_mint(feed, entry.header.body, private, None)
            .map(drop)
    }

    fn reintent_remove(&mut self, feed: FeedId, hex_target: &str) -> Result<(), ApiError> {
        let target: [u8; 16] = decode_hex(hex_target)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| ApiError::internal("bad intent"))?;
        let target = AccountId::from_bytes(target);
        let me = self.me()?;
        let state = self
            .tentative_state(feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        if target == me || state.role_of(target).is_none() {
            return Ok(());
        }
        let remaining: Vec<AccountId> = state
            .members
            .iter()
            .map(|member| member.account)
            .filter(|account| *account != target)
            .collect();
        let Some((fields, key)) = self.build_rekey_fields(feed, &remaining)? else {
            return Err(ApiError::invalid(
                "membership",
                "a member's device log is not known here yet",
            ));
        };
        let epoch = fields.new_epoch;
        self.membership_mint(
            feed,
            EntryBody::Remove {
                target,
                rekey: fields,
            },
            Private::None,
            Some((epoch, key)),
        )
        .map(drop)
    }
}

/// What to do with an intent that could not be written now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// Written, or refused for good.
    No,
    /// A device log is missing: try again at a later cycle.
    Later,
}
