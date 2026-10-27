//! The profile and the people (M13 slice 1, ADR-0042, `docs/development/milestone-13.md` section
//! 1.1): this account's display name (`profile` of the account document), the people it knows from
//! shared spaces, their safety numbers and "Mark verified" (`verifiedPeople`).
//!
//! Setting the name appends a `Profile` entry to every shared space this account is a member of
//! (sealed under the space's membership-meta key, so the server never sees the name). A person is
//! "known" when a shared space lists them (or the verified list does); their name is whatever the
//! spaces say, preferring a live one.

use kb_keys::safety::{genesis_hash, safety_number};
use kb_storage::FeedQueries;
use kb_types::sharing::PROFILE_NAME_MAX_CHARS;
use kb_types::{
    AccountId, ApiError, Event, FeedId, PersonInfo, ProfileInfo, Request, Response, SpaceId,
};

use crate::documents::map_model_error;
use crate::membership::fallback_name;
use crate::{Core, validate};
use kb_keys::membership::{EntryBody, Private};

impl Core {
    /// Routes `ProfileGet`, `ProfileSet`, `PersonGet` and `PersonVerifiedSet`.
    pub(crate) fn dispatch_profile(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::ProfileGet => Ok(Response::Profile(self.profile_get()?)),
            Request::ProfileSet { display_name } => {
                Ok(Response::Profile(self.profile_set(&display_name)?))
            }
            Request::PersonGet { account_id } => Ok(Response::Person(self.person_get(account_id)?)),
            Request::PersonVerifiedSet {
                account_id,
                verified,
            } => Ok(Response::Person(
                self.person_verified_set(account_id, verified)?,
            )),
            other => Err(ApiError::internal(format!(
                "{} is not a profile request",
                other.kind()
            ))),
        }
    }

    /// This account's profile.
    pub(crate) fn profile_get(&mut self) -> Result<ProfileInfo, ApiError> {
        Ok(ProfileInfo {
            display_name: self.profile_name_of_account(),
        })
    }

    /// Sets the display name (1 to `PROFILE_NAME_MAX_CHARS` characters after trimming) and appends
    /// a `Profile` entry to every shared space. `InvalidInput` for `displayName`.
    pub(crate) fn profile_set(&mut self, display_name: &str) -> Result<ProfileInfo, ApiError> {
        let name = validate::display_name(display_name, PROFILE_NAME_MAX_CHARS, "displayName")?;
        self.ensure_account()?;
        let changed = self.profile_name_of_account().as_deref() != Some(name.as_str());
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .set_profile_name(&name)
                .map_err(map_model_error)?;
        }
        if changed {
            self.persist_account()?;
            self.profile_publish(&name)?;
        }
        self.push_event_once(Event::PeopleChanged);
        Ok(ProfileInfo {
            display_name: Some(name),
        })
    }

    /// Appends a `Profile` entry with `name` to each live shared space this account is a member of.
    /// A space that cannot take it now (a co-member's device log is not here yet) is skipped: the
    /// name is in the account document and the next `ProfileSet` or join carries it.
    fn profile_publish(&mut self, name: &str) -> Result<(), ApiError> {
        let me = self.me()?;
        let rows: Vec<(FeedId, SpaceId)> = self
            .db
            .sync_feed_list()
            .map_err(crate::map_storage_error)?
            .into_iter()
            .filter(|row| row.kind == kb_types::sync::FeedKind::Shared)
            .filter_map(|row| row.space_id.map(|space| (row.feed, space)))
            .collect();
        for (feed, space_id) in rows {
            let leaving = self.fcfg(feed, "leaving")?.is_some();
            let live = !leaving
                && self.feed_archived(feed)?.is_none()
                && self
                    .shared
                    .timelines
                    .get(&feed)
                    .is_none_or(|t| t.state().closed.is_none());
            let member = self
                .tentative_state(feed)?
                .is_some_and(|state| state.role_of(me).is_some());
            if !live || !member {
                continue;
            }
            match self.membership_mint(
                feed,
                EntryBody::Profile,
                Private::Profile {
                    display_name: name.to_owned(),
                },
                None,
            ) {
                Ok(_) => {
                    // The panel shows the name at once; the entry sets the same value when it folds.
                    self.shared
                        .metas
                        .entry(feed)
                        .or_default()
                        .names
                        .insert(me, name.to_owned());
                    self.push_event_once(Event::SpaceMembersChanged { space_id });
                    if let Err(error) = self.person_names_changed(feed) {
                        tracing::warn!(code = ?error.code, "the people's search text could not be refreshed");
                    }
                }
                Err(error) => {
                    tracing::warn!(code = ?error.code, "a profile entry could not be written");
                }
            }
        }
        Ok(())
    }

    /// A person known from a shared space or the verified list. `NotFound` for an unknown account.
    pub(crate) fn person_get(&mut self, account_id: AccountId) -> Result<PersonInfo, ApiError> {
        let me = self.me().ok();
        if Some(account_id) == me {
            return Ok(PersonInfo {
                account_id,
                name: self
                    .profile_name_of_account()
                    .unwrap_or_else(|| fallback_name(account_id)),
                verified: false,
                safety_number: None,
                is_me: true,
            });
        }
        let verified = self
            .docs
            .account
            .as_ref()
            .is_some_and(|open| open.model.verified_at(account_id).is_some());
        // The name: a live space first, then an archive; the members of the folded timelines.
        let mut name: Option<String> = None;
        let mut archived_name: Option<String> = None;
        for (feed, meta) in &self.shared.metas {
            let Some(found) = meta.names.get(&account_id) else {
                continue;
            };
            let listed = self
                .shared
                .timelines
                .get(feed)
                .is_some_and(|t| t.state().member(account_id).is_some());
            if !listed {
                continue;
            }
            if self.feed_archived(*feed)?.is_none() {
                name = Some(found.clone());
                break;
            }
            archived_name.get_or_insert_with(|| found.clone());
        }
        let known_member = name.is_some()
            || archived_name.is_some()
            || self
                .shared
                .timelines
                .values()
                .any(|t| t.state().member(account_id).is_some());
        if !known_member && !verified {
            return Err(ApiError::not_found("person not found"));
        }
        let safety = match (
            self.security.log.as_ref().map(genesis_hash),
            self.shared.logs.get(&account_id).map(genesis_hash),
        ) {
            (Some(a), Some(b)) => Some(safety_number(&a, &b)),
            _ => None,
        };
        Ok(PersonInfo {
            account_id,
            name: name
                .or(archived_name)
                .unwrap_or_else(|| fallback_name(account_id)),
            verified,
            safety_number: safety,
            is_me: false,
        })
    }

    /// Writes `verifiedPeople`.
    pub(crate) fn person_verified_set(
        &mut self,
        account_id: AccountId,
        verified: bool,
    ) -> Result<PersonInfo, ApiError> {
        let person = self.person_get(account_id)?;
        if person.is_me {
            return Err(ApiError::invalid("accountId", "you cannot verify yourself"));
        }
        let now = self.clock.now_ms();
        self.ensure_account()?;
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .set_verified(account_id, verified.then_some(now))
                .map_err(map_model_error)?;
        }
        self.persist_account()?;
        self.push_event_once(Event::PeopleChanged);
        // The panels of the spaces this person is in show the mark.
        let spaces: Vec<SpaceId> = self
            .shared
            .timelines
            .iter()
            .filter(|(_, t)| t.state().member(account_id).is_some())
            .filter_map(|(feed, _)| self.feed_space(*feed).ok().flatten())
            .collect();
        for space_id in spaces {
            self.push_event_once(Event::SpaceMembersChanged { space_id });
        }
        self.person_get(account_id)
    }
}
