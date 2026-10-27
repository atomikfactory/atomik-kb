//! The account document's maps for shared spaces (M13, ADR-0041, ADR-0042, ADR-0045 and ADR-0047,
//! `docs/development/milestone-13.md` section 1.4). All are additive root maps of the
//! [`AccountDoc`], merged per key like `devices`, so two devices of one account that write
//! different keys both keep theirs and two writes of the same key resolve last-writer-wins:
//!
//! | Root map | Key -> value |
//! |---|---|
//! | `sharedSpaces` | space id string -> feed id string (the space is shared; its feed) |
//! | `profile` | `name` -> the person's display name |
//! | `spaceSettings` | space id string -> a map with `ai` (bool; absent: the space's default) |
//! | `favorites` | space id string -> a map of page id string -> fractional position (the shape of ADR-0029's structure map): `favorites/<space id>` |
//! | `verifiedPeople` | account id string -> Unix ms of "Mark verified" |
//!
//! `spaceSettings/<space>` and `favorites/<space>` are **mergeable child containers**
//! (`LoroMap::ensure_mergeable_map`): their identity is `(parent map, space id, map)`, not the
//! operation that created them, so two devices that write a space's first favourite (or first
//! setting) at the same moment write into one map and both writes survive. (An ordinary child
//! container made on each device would be two different maps, one of which loses.) Loro does not
//! allow a `/` in a root container's name, so "`favorites/<space>`" is a path, not a root name.
//!
//! The structure document's favourites stay for personal spaces; [`AccountDoc::adopt_favorites`]
//! moves a space's favourites out of its structure document when the space is shared (the share
//! step), because favourites are per person and a shared structure document is shared.
//!
//! Readers never write: asking for a space that has nothing returns nothing and leaves the document
//! as it was. Writers write only what changes (setting a value that is already there, or removing
//! one that is not, writes nothing), so a device that repeats a step does not grow the history.
//! Entries that do not parse (a key that is not an id, a value of another type) are ignored by
//! readers and replaced by writers, as for `devices`. Every write is committed with
//! [`SHARING_ORIGIN`]; nothing in the account document is undoable.

use kb_types::{AccountId, FeedId, PageId, SpaceId};
use loro::{CommitOptions, Container, LoroMap, LoroValue, ValueOrContainer};

use crate::meta::{self, engine};
use crate::properties::order;
use crate::{AccountDoc, ModelError, StructureDoc};

/// Root map of `sharedSpaces`.
pub const SHARED_SPACES_MAP: &str = "sharedSpaces";
/// Root map of `profile`; its only key is [`PROFILE_NAME_KEY`].
pub const PROFILE_MAP: &str = "profile";
/// The key of the display name inside `profile`.
pub const PROFILE_NAME_KEY: &str = "name";
/// Root map of `spaceSettings`.
pub const SPACE_SETTINGS_MAP: &str = "spaceSettings";
/// The key of the AI switch inside a `spaceSettings` entry.
pub const SPACE_AI_KEY: &str = "ai";
/// Root map of the per-space favourites: `favorites/<space id>` is its child under the space id
/// (a path; Loro root names cannot contain `/`).
pub const FAVORITES_MAP: &str = "favorites";
/// Root map of `verifiedPeople`.
pub const VERIFIED_PEOPLE_MAP: &str = "verifiedPeople";
/// Commit origin of the writes of this module; nothing of the account document is undoable.
pub const SHARING_ORIGIN: &str = "kb-model/sharing";

/// Shorthand for a refused favourite operation.
fn refused(field: &'static str, message: &'static str) -> ModelError {
    ModelError::InvalidFavorite { field, message }
}

impl AccountDoc {
    // ----- sharedSpaces ---------------------------------------------------------------------

    /// Every shared space with its feed, ordered by space id. An entry that does not parse is left
    /// out.
    #[must_use]
    pub fn shared_spaces(&self) -> Vec<(SpaceId, FeedId)> {
        let LoroValue::Map(map) = self.root(SHARED_SPACES_MAP).get_value() else {
            return Vec::new();
        };
        let mut out: Vec<(SpaceId, FeedId)> = map
            .iter()
            .filter_map(|(key, value)| match value {
                LoroValue::String(feed) => Some((key.parse().ok()?, feed.parse().ok()?)),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// The feed of a shared space, `None` for a personal space.
    #[must_use]
    pub fn shared_space(&self, space_id: SpaceId) -> Option<FeedId> {
        meta::string(&self.root(SHARED_SPACES_MAP), &space_id.to_string())?
            .parse()
            .ok()
    }

    /// Records that `space_id` is shared through `feed_id` (the share step, and a join). Writing the
    /// same pair again changes nothing; another feed for the space replaces the pair (last writer
    /// wins on two devices).
    pub fn set_shared_space(&self, space_id: SpaceId, feed_id: FeedId) -> Result<(), ModelError> {
        let map = self.root(SHARED_SPACES_MAP);
        let key = space_id.to_string();
        let value = feed_id.to_string();
        if meta::string(&map, &key).as_deref() == Some(value.as_str()) {
            return Ok(());
        }
        map.insert(&key, value.as_str()).map_err(engine)?;
        self.commit_sharing();
        Ok(())
    }

    /// Forgets a shared space (leaving it, deleting an archive). Succeeds when it is absent.
    pub fn remove_shared_space(&self, space_id: SpaceId) -> Result<(), ModelError> {
        let map = self.root(SHARED_SPACES_MAP);
        if map.get(&space_id.to_string()).is_none() {
            return Ok(());
        }
        meta::remove(&map, &space_id.to_string())?;
        self.commit_sharing();
        Ok(())
    }

    // ----- profile --------------------------------------------------------------------------

    /// The person's display name, `None` until set.
    #[must_use]
    pub fn profile_name(&self) -> Option<String> {
        meta::string(&self.root(PROFILE_MAP), PROFILE_NAME_KEY)
    }

    /// Sets the display name. The caller trimmed and validated it (1 to `PROFILE_NAME_MAX_CHARS`
    /// characters).
    pub fn set_profile_name(&self, name: &str) -> Result<(), ModelError> {
        let map = self.root(PROFILE_MAP);
        if meta::string(&map, PROFILE_NAME_KEY).as_deref() == Some(name) {
            return Ok(());
        }
        map.insert(PROFILE_NAME_KEY, name).map_err(engine)?;
        self.commit_sharing();
        Ok(())
    }

    // ----- spaceSettings --------------------------------------------------------------------

    /// This account's AI switch for a space; `None` when it never chose (a personal space then
    /// allows AI, a shared space does not: the core applies the default, ADR-0047). A value that is
    /// not a boolean reads as `None`.
    #[must_use]
    pub fn space_ai(&self, space_id: SpaceId) -> Option<bool> {
        let settings = self.child(SPACE_SETTINGS_MAP, space_id)?;
        match meta::value(&settings, SPACE_AI_KEY)? {
            LoroValue::Bool(enabled) => Some(enabled),
            _ => None,
        }
    }

    /// Sets the AI switch for a space.
    pub fn set_space_ai(&self, space_id: SpaceId, enabled: bool) -> Result<(), ModelError> {
        if self.space_ai(space_id) == Some(enabled) {
            return Ok(());
        }
        let settings = self.child_for_write(SPACE_SETTINGS_MAP, space_id)?;
        settings.insert(SPACE_AI_KEY, enabled).map_err(engine)?;
        self.commit_sharing();
        Ok(())
    }

    /// Forgets everything stored for a space in `spaceSettings`. Succeeds when there is nothing.
    pub fn remove_space_settings(&self, space_id: SpaceId) -> Result<(), ModelError> {
        self.forget_child(SPACE_SETTINGS_MAP, space_id)
    }

    // ----- favorites/<space> ----------------------------------------------------------------

    /// The space's favourite pages in order, by `(position, page id)`. The document does not know
    /// which pages are live: the core hides the trashed and ignores the unknown, as for
    /// `StructureDoc::favorites`. An entry that is not a page id with a text position is left out.
    #[must_use]
    pub fn favorites(&self, space_id: SpaceId) -> Vec<PageId> {
        self.favorite_entries(space_id)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Adds the page to the space's favourites (last) or removes it. Returns whether anything
    /// changed; idempotent like `StructureDoc::set_favorite`: adding a favourite that exists keeps
    /// its place and removing one that does not exist changes nothing.
    pub fn set_favorite(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        favorite: bool,
    ) -> Result<bool, ModelError> {
        let key = page_id.to_string();
        if !favorite {
            let Some(map) = self.child(FAVORITES_MAP, space_id) else {
                return Ok(false);
            };
            if map.get(&key).is_none() {
                return Ok(false);
            }
            meta::remove(&map, &key)?;
            self.commit_sharing();
            return Ok(true);
        }

        let all = self.favorite_entries(space_id);
        if all.iter().any(|(id, _)| *id == page_id) {
            return Ok(false);
        }
        // After every stored entry, so the new favourite is last.
        let position = order::key_between(all.last().map(|(_, position)| position.as_str()), None);
        let map = self.child_for_write(FAVORITES_MAP, space_id)?;
        match position {
            Some(position) => meta::set_string(&map, &key, &position)?,
            None => {
                // The last position is damaged (not a base-62 string): renumber the entries and put
                // the new one after them.
                let mut order_after: Vec<PageId> = all.into_iter().map(|(id, _)| id).collect();
                order_after.push(page_id);
                renumber(&map, &order_after)?;
            }
        }
        self.commit_sharing();
        Ok(true)
    }

    /// Moves a favourite in front of `before` (another favourite of the space), or last when `None`.
    /// Returns whether anything changed (a move to the place it already holds writes nothing).
    /// [`ModelError::InvalidFavorite`] with the field `pageId` or `before` when the page is not a
    /// favourite, as for `StructureDoc::move_favorite`.
    pub fn move_favorite(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        before: Option<PageId>,
    ) -> Result<bool, ModelError> {
        let all = self.favorite_entries(space_id);
        let Some(current) = all.iter().position(|(id, _)| *id == page_id) else {
            return Err(refused("pageId", "the page is not a favourite"));
        };
        if before == Some(page_id) {
            return Ok(false);
        }
        let others: Vec<&(PageId, String)> = all.iter().filter(|(id, _)| *id != page_id).collect();
        let at = match before {
            None => others.len(),
            Some(target) => others
                .iter()
                .position(|(id, _)| *id == target)
                .ok_or_else(|| refused("before", "the page is not a favourite"))?,
        };
        if current == at {
            return Ok(false);
        }
        let prev = at.checked_sub(1).and_then(|i| others.get(i));
        let next = others.get(at);
        let position = order::key_between(
            prev.map(|(_, position)| position.as_str()),
            next.map(|(_, position)| position.as_str()),
        );
        let map = self.child_for_write(FAVORITES_MAP, space_id)?;
        match position {
            Some(position) => meta::set_string(&map, &page_id.to_string(), &position)?,
            None => {
                // Equal or damaged neighbours: renumber the entries in their new order.
                let mut order_after: Vec<PageId> = others.iter().map(|(id, _)| *id).collect();
                order_after.insert(at, page_id);
                renumber(&map, &order_after)?;
            }
        }
        self.commit_sharing();
        Ok(true)
    }

    /// The share step: copies the favourites of `structure` (the space's structure document) into
    /// `favorites/<space id>` with their positions, then clears the structure document's favourites
    /// map. Returns how many moved: the readable entries of the structure document, the hidden
    /// favourites of trashed pages included (this document does not know which pages are live, so
    /// a restore after sharing brings the favourite back in its place). Favourites already in the
    /// account document keep their positions (a second device that shares first wins). The copy is
    /// committed before the clear, so a failure in between leaves the favourite in both places,
    /// never in neither.
    pub fn adopt_favorites(
        &self,
        space_id: SpaceId,
        structure: &mut StructureDoc,
    ) -> Result<usize, ModelError> {
        let entries = structure.all_favorite_entries();
        if !entries.is_empty() {
            let map = self.child_for_write(FAVORITES_MAP, space_id)?;
            for (page_id, position) in &entries {
                let key = page_id.to_string();
                if meta::string(&map, &key).is_none() {
                    meta::set_string(&map, &key, position)?;
                }
            }
            self.commit_sharing();
        }
        // Also clears entries that do not parse; writes nothing when the map is already empty.
        structure.clear_favorites()?;
        Ok(entries.len())
    }

    /// Forgets the space's favourites (leaving, deleting an archive). Succeeds when there are none.
    pub fn remove_favorites(&self, space_id: SpaceId) -> Result<(), ModelError> {
        self.forget_child(FAVORITES_MAP, space_id)
    }

    // ----- verifiedPeople -------------------------------------------------------------------

    /// The accounts the person marked verified, with the Unix ms of the mark, ordered by account id.
    #[must_use]
    pub fn verified_people(&self) -> Vec<(AccountId, i64)> {
        let LoroValue::Map(map) = self.root(VERIFIED_PEOPLE_MAP).get_value() else {
            return Vec::new();
        };
        let mut out: Vec<(AccountId, i64)> = map
            .iter()
            .filter_map(|(key, value)| match value {
                LoroValue::I64(at) => Some((key.parse().ok()?, *at)),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// When the account was marked verified, `None` if it was not.
    #[must_use]
    pub fn verified_at(&self, account_id: AccountId) -> Option<i64> {
        meta::int(&self.root(VERIFIED_PEOPLE_MAP), &account_id.to_string())
    }

    /// Marks the account verified at `at` (Unix ms), or removes the mark with `None`. Marking an
    /// account that is already marked keeps the earlier time: a later mark writes nothing.
    pub fn set_verified(&self, account_id: AccountId, at: Option<i64>) -> Result<(), ModelError> {
        let map = self.root(VERIFIED_PEOPLE_MAP);
        let key = account_id.to_string();
        match at {
            None => {
                if map.get(&key).is_none() {
                    return Ok(());
                }
                meta::remove(&map, &key)?;
            }
            Some(at) => {
                if meta::int(&map, &key).is_some_and(|earlier| earlier <= at) {
                    return Ok(());
                }
                map.insert(&key, at).map_err(engine)?;
            }
        }
        self.commit_sharing();
        Ok(())
    }

    // ----- internals ------------------------------------------------------------------------

    /// A root map of the document (a root container exists by its name; reading one writes nothing).
    fn root(&self, name: &str) -> LoroMap {
        self.doc().inner().get_map(name)
    }

    /// The per-space child map of a root map (`spaceSettings`, `favorites`), if the space has one.
    /// Never creates it.
    fn child(&self, root: &str, space_id: SpaceId) -> Option<LoroMap> {
        match self.root(root).get(&space_id.to_string())? {
            ValueOrContainer::Container(Container::Map(map)) => Some(map),
            _ => None,
        }
    }

    /// The per-space child map, created as a mergeable container when the space has none. A value
    /// at the key that is not such a map (a damaged document) is replaced.
    fn child_for_write(&self, root: &str, space_id: SpaceId) -> Result<LoroMap, ModelError> {
        let parent = self.root(root);
        let key = space_id.to_string();
        if let Ok(map) = parent.ensure_mergeable_map(&key) {
            return Ok(map);
        }
        parent.delete(&key).map_err(engine)?;
        parent.ensure_mergeable_map(&key).map_err(engine)
    }

    /// Forgets a space's child map: its entries go first (Loro keeps a hidden mergeable child's
    /// state under its fixed identity, and a later write would bring it back), then the key.
    fn forget_child(&self, root: &str, space_id: SpaceId) -> Result<(), ModelError> {
        let parent = self.root(root);
        let key = space_id.to_string();
        if parent.get(&key).is_none() {
            return Ok(());
        }
        if let Some(map) = self.child(root, space_id) {
            let entries: Vec<String> = match map.get_value() {
                LoroValue::Map(entries) => entries.keys().map(ToString::to_string).collect(),
                _ => Vec::new(),
            };
            for entry in entries {
                meta::remove(&map, &entry)?;
            }
        }
        meta::remove(&parent, &key)?;
        self.commit_sharing();
        Ok(())
    }

    /// The space's favourites with their positions, by `(position, page id)`.
    fn favorite_entries(&self, space_id: SpaceId) -> Vec<(PageId, String)> {
        let Some(map) = self.child(FAVORITES_MAP, space_id) else {
            return Vec::new();
        };
        let LoroValue::Map(entries) = map.get_value() else {
            return Vec::new();
        };
        let mut out: Vec<(PageId, String)> = entries
            .iter()
            .filter_map(|(key, value)| match value {
                LoroValue::String(position) => {
                    Some((key.parse::<PageId>().ok()?, position.to_string()))
                }
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// Commits the pending writes of this module (the document commits nothing by itself).
    fn commit_sharing(&self) {
        self.doc()
            .inner()
            .commit_with(CommitOptions::new().origin(SHARING_ORIGIN));
    }
}

/// Writes fresh, evenly spread positions for `order` (first to last).
fn renumber(map: &LoroMap, order_of: &[PageId]) -> Result<(), ModelError> {
    for (id, position) in order_of.iter().zip(order::sequence(order_of.len())) {
        meta::set_string(map, &id.to_string(), &position)?;
    }
    Ok(())
}
