//! Person mentions and the Person property in the core (M13 slice 2, ADR-0049,
//! `docs/development/milestone-13.md` sections 2.1 and 2.2): who the `@` menu and a Person property
//! offer, what a Person value may hold when it is written, and the derived `person_mentions` rows
//! with the diffs that raise notifications.
//!
//! - **People.** [`Core::people_mentionable`] lists a space's members, this account included (in a
//!   personal space this account alone), as `PersonInfo`s with the names the membership log gives
//!   (the same names `PersonGet` shows).
//! - **Person values.** A `PropertyValue::Person` is written through `PagePropertySet` (and
//!   `PropertyCreate` never has one): the structural rules are `kb-model`'s (at most 20, no
//!   duplicates, not computed); [`Core::person_value_check`] adds that every account the value
//!   **newly** lists is a member of the space when it is written (`InvalidInput` for `value`; in a
//!   personal space only this account). An account the value listed already may have left since: it
//!   stays, and shows as "Former member". `PropertyFilter::ContainsMe` is answered as `ContainsPerson`
//!   of this account ([`Core::person_filter_account`]); a space that has no account yet (no kit) has no
//!   "me" and the filter matches nothing.
//! - **Mentions in text.** The `personMention` rows of a page ([`person_mention_rows`]) are rewritten
//!   with the other derived rows of a page on every save; a change that arrived through sync is
//!   diffed against the old rows ([`Core::person_mentions_notify`]), and a mention of this account that
//!   was not there before raises `PageMention`. The same diff over the structure document's Person
//!   values raises `PersonAssigned` ([`Core::person_assignments_notify`]).
//!
//! Only entries notify: a snapshot is a state with no author to name, and a device that reads a feed
//! for the first time announces none of its history (`comments/apply.rs`).

use std::collections::{BTreeSet, HashMap};

use kb_model::{PageDoc, PageIndexEntry};
use kb_storage::{
    NewNotification, PagePropertyQueries, PersonMentionQueries, PersonMentionRow, PropertyMatch,
};
use kb_types::{
    AccountId, ApiError, DocId, FeedId, NotificationKind, PageId, PersonInfo, PropertyFilter,
    PropertyId, PropertyKind, Request, Response, SpaceId,
};

use crate::properties::PropsScope;
use crate::{Core, map_storage_error};

/// The `person_mentions` rows of a page document: one per `(enclosing block, account)`.
pub(crate) fn person_mention_rows(model: &PageDoc) -> Vec<PersonMentionRow> {
    let mut rows: Vec<PersonMentionRow> = model
        .person_mentions()
        .into_iter()
        .map(|mention| PersonMentionRow {
            block_id: mention.block_id,
            account_id: mention.account_id,
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

impl Core {
    /// Routes `PeopleMentionable`.
    pub(crate) fn dispatch_mentions(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::PeopleMentionable { space_id } => {
                Ok(Response::People(self.people_mentionable(space_id)?))
            }
            other => Err(ApiError::internal(format!(
                "{} is not a mention request",
                other.kind()
            ))),
        }
    }

    /// The members the `@` menu and a Person property offer in a space, this account included, in
    /// join order (a personal space: this account alone). `NotFound` for the space.
    pub(crate) fn people_mentionable(
        &mut self,
        space_id: SpaceId,
    ) -> Result<Vec<PersonInfo>, ApiError> {
        self.structure(space_id)?;
        let feed = self.shared_feed_of(space_id)?.map(|row| row.feed);
        let mut people = Vec::new();
        for account in self.space_member_accounts(space_id)? {
            // A member the folded state names always has a name (or the short id).
            if let Ok(mut person) = self.person_get(account) {
                // In a shared space the name is the one this space's log gives.
                if let Some(feed) = feed {
                    person.name = self.member_name(feed, account);
                }
                people.push(person);
            }
        }
        Ok(people)
    }

    /// The accounts that are members of a space now: the folded membership of a shared space (the
    /// archive's last one), this account alone for a personal one (nobody when no kit exists yet).
    pub(crate) fn space_member_accounts(
        &mut self,
        space_id: SpaceId,
    ) -> Result<Vec<AccountId>, ApiError> {
        self.shared_load()?;
        match self.shared_feed_of(space_id)? {
            Some(row) => {
                let state = self.tentative_state(row.feed)?;
                Ok(state
                    .map(|state| state.members.iter().map(|member| member.account).collect())
                    .unwrap_or_default())
            }
            None => Ok(self.me().ok().into_iter().collect()),
        }
    }

    /// The names the space's membership gives, for the text of Person values: every name the
    /// folded log holds for the space (a former member keeps the last one), this account's own, and
    /// for a personal space this account's alone.
    pub(crate) fn space_people_names(&self, space_id: SpaceId) -> HashMap<AccountId, String> {
        let mut names: HashMap<AccountId, String> = HashMap::new();
        if let Some(row) = self.shared_feed_of(space_id).ok().flatten()
            && let Some(meta) = self.shared.metas.get(&row.feed)
        {
            names.extend(
                meta.names
                    .iter()
                    .map(|(account, name)| (*account, name.clone())),
            );
        }
        if let (Ok(me), Some(name)) = (self.me(), self.profile_name_of_account()) {
            names.insert(me, name);
        }
        names
    }

    /// A member's name was given or changed in the space of `feed`: the text rows of the Person values
    /// of that space are written again with the names the log gives now (a space with no Person
    /// property has none to write).
    pub(crate) fn person_names_changed(&mut self, feed: FeedId) -> Result<(), ApiError> {
        let Some(space_id) = self.feed_space(feed)? else {
            return Ok(());
        };
        let has_person = self.docs.structures.get(&space_id).is_some_and(|open| {
            open.model
                .property_schema()
                .iter()
                .any(|def| def.kind == PropertyKind::Person)
        });
        if has_person {
            self.refresh_property_rows(space_id, PropsScope::Space)?;
        }
        Ok(())
    }

    /// The membership rule of a Person value: every account the value lists that `kept` (the value
    /// the property has now) did not is a member of the space now (a personal space: this account).
    /// `InvalidInput` for `value` otherwise. The count and duplicate rules are `kb-model`'s.
    pub(crate) fn person_value_check(
        &mut self,
        space_id: SpaceId,
        accounts: &[AccountId],
        kept: &[AccountId],
    ) -> Result<(), ApiError> {
        let members = self.space_member_accounts(space_id)?;
        if accounts
            .iter()
            .all(|account| kept.contains(account) || members.contains(account))
        {
            Ok(())
        } else {
            Err(ApiError::invalid(
                "value",
                "a person of the list is not a member of this space",
            ))
        }
    }

    /// The pages of a space whose Person property matches `filter`: `HasValue`,
    /// `ContainsPerson(account)` or `ContainsMe` (this account, see [`Self::person_filter_account`];
    /// no account means no page), by title like every `PagesByProperty` answer. `InvalidInput` for
    /// `filter` for any other filter. Reads the `account_id` rows of `page_properties`.
    pub(crate) fn person_pages(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        filter: &PropertyFilter,
    ) -> Result<Vec<PageId>, ApiError> {
        let matching = match filter {
            PropertyFilter::HasValue => PropertyMatch::HasValue,
            PropertyFilter::ContainsPerson(account) => PropertyMatch::Person(*account),
            PropertyFilter::ContainsMe => match self.person_filter_account() {
                Some(account) => PropertyMatch::Person(account),
                None => return Ok(Vec::new()),
            },
            _ => {
                return Err(ApiError::invalid(
                    "filter",
                    "a person property is searched with HasValue, ContainsPerson or ContainsMe",
                ));
            }
        };
        self.db
            .page_properties_pages(space_id, property_id, &matching)
            .map_err(map_storage_error)
    }

    /// The account `PropertyFilter::ContainsMe` stands for: this account, or `None` when there is
    /// none yet (the filter then matches nothing).
    pub(crate) fn person_filter_account(&self) -> Option<AccountId> {
        self.me().ok()
    }

    /// Raises `PageMention` for each mention of this account in the page that the `old` rows did not
    /// have (called after a change to the page arrived in an entry written by `actor`, with the rows
    /// as they were before it; the new rows were written with the page's other derived rows).
    pub(crate) fn person_mentions_notify(
        &mut self,
        space_id: SpaceId,
        page_id: PageId,
        old: &[PersonMentionRow],
        actor: AccountId,
        now: i64,
    ) -> Result<(), ApiError> {
        let Ok(me) = self.me() else {
            return Ok(());
        };
        if actor == me {
            return Ok(());
        }
        let current = self
            .db
            .person_mentions_of(DocId::for_page(page_id))
            .map_err(map_storage_error)?;
        // One notification per page change, at the first new mention of this account.
        if let Some(added) = current
            .iter()
            .find(|row| row.account_id == me && !old.contains(row))
        {
            self.notification_raise(&NewNotification {
                kind: NotificationKind::PageMention,
                space_id,
                page_id: Some(page_id),
                block_id: Some(added.block_id),
                comment_id: None,
                actor,
                at: now,
            })?;
        }
        Ok(())
    }

    /// Raises `PersonAssigned` for each page whose Person values list this account after a change
    /// that arrived in an entry written by `actor` and did not before it (the maps are the pages'
    /// property rows before and after, `PageIndexEntry`).
    pub(crate) fn person_assignments_notify(
        &mut self,
        space_id: SpaceId,
        before: &HashMap<PageId, PageIndexEntry>,
        after: &HashMap<PageId, PageIndexEntry>,
        actor: AccountId,
        now: i64,
    ) -> Result<(), ApiError> {
        let Ok(me) = self.me() else {
            return Ok(());
        };
        if actor == me {
            return Ok(());
        }
        let assigned = |entry: Option<&PageIndexEntry>| -> BTreeSet<PropertyId> {
            entry
                .into_iter()
                .flat_map(|entry| entry.rows.iter())
                .filter(|row| row.kind == PropertyKind::Person && row.account_id == Some(me))
                .map(|row| row.property_id)
                .collect()
        };
        for (page, entry) in after {
            let now_assigned = assigned(Some(entry));
            if now_assigned.is_empty() {
                continue;
            }
            let was_assigned = assigned(before.get(page));
            if now_assigned
                .iter()
                .any(|property| !was_assigned.contains(property))
            {
                self.notification_raise(&NewNotification {
                    kind: NotificationKind::PersonAssigned,
                    space_id,
                    page_id: Some(*page),
                    block_id: None,
                    comment_id: None,
                    actor,
                    at: now,
                })?;
            }
        }
        Ok(())
    }
}
