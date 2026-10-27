//! Favourites of a space (ADR-0029, section 2).
//!
//! ```text
//! root map `favorites`: <page id> -> position (a fractional string, see `properties/order.rs`)
//! ```
//!
//! A favourite is a page the user pinned at the top of the sidebar. The list lives in the
//! structure document, so it syncs, merges and is encrypted like the page tree: one key per page,
//! so two devices that favourite different pages both keep theirs, and two moves of the same
//! favourite resolve last-writer-wins. Readers list the favourites by `(position, page id)`.
//!
//! - A favourite whose page is in the trash is kept and **hidden** (it comes back with a
//!   restore); an entry that names no page at all (purged, or never known here) is ignored on read.
//! - Purging a page deletes its entry (`forget_favorites`, right after the purge's own commit).
//! - Writes are committed with [`FAVORITES_ORIGIN`], which the structure document's undo manager
//!   excludes: favourites are not on the sidebar's undo stack, and undo and redo never touch
//!   them (undoing a purge brings the page back, not its favourite).
//! - Adding puts the favourite last; a move computes a position between the new neighbours and
//!   renumbers the visible favourites when the neighbours are tied or unreadable (two devices
//!   appended at the same moment), as `move_property` does.

use kb_types::PageId;
use loro::{CommitOptions, LoroValue};

use crate::meta;
use crate::properties::order;
use crate::{ModelError, StructureDoc};

/// Commit origin of favourite writes; the structure document's undo manager excludes it (see the
/// module documentation).
pub const FAVORITES_ORIGIN: &str = "kb-model/favorites";

/// Shorthand for a refused favourite operation.
fn refused(field: &'static str, message: &'static str) -> ModelError {
    ModelError::InvalidFavorite { field, message }
}

impl StructureDoc {
    /// The space's live favourites, in order: pages that are in the tree and not in the trash,
    /// by `(position, page id)`. Entries of trashed pages (hidden) and of unknown pages (ignored)
    /// are left out.
    #[must_use]
    pub fn favorites(&self) -> Vec<PageId> {
        self.live_favorites()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Whether the page is one of the live favourites (see [`Self::favorites`]).
    #[must_use]
    pub fn is_favorite(&self, page_id: PageId) -> bool {
        self.live_favorites().iter().any(|(id, _)| *id == page_id)
    }

    /// Adds the page to the favourites (last) or removes it. Returns whether anything changed.
    ///
    /// Idempotent: adding a favourite that exists keeps its place, removing one that does not
    /// exist changes nothing. Adding needs a live page ([`ModelError::PageNotFound`] for an
    /// unknown or purged one, [`ModelError::PageTrashed`] for a page in the trash). Removing
    /// needs nothing: it also deletes the entry of a trashed or unknown page.
    ///
    /// Not recorded on the undo stack (committed with [`FAVORITES_ORIGIN`]).
    pub fn set_favorite(&mut self, page_id: PageId, favorite: bool) -> Result<bool, ModelError> {
        let key = page_id.to_string();
        if !favorite {
            if self.favorites.get(&key).is_none() {
                return Ok(false);
            }
            self.close_group();
            self.favorites_write(|| meta::remove(&self.favorites, &key))?;
            return Ok(true);
        }

        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        if !self.is_live(node) {
            return Err(ModelError::PageTrashed);
        }
        if self.stored_position(&key).is_some() {
            return Ok(false);
        }
        // After every stored entry (hidden ones included), so the new favourite is last.
        let all = self.favorite_entries();
        let position = order::key_between(all.last().map(|(_, position)| position.as_str()), None);

        self.close_group();
        self.favorites_write(|| match &position {
            Some(position) => meta::set_string(&self.favorites, &key, position),
            None => {
                // The last position is damaged (not a base-62 string): renumber the visible
                // favourites and put the new one after them.
                let mut order_after: Vec<PageId> = self
                    .live_favorites()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                order_after.push(page_id);
                self.renumber(&order_after)
            }
        })?;
        Ok(true)
    }

    /// Moves a favourite in front of `before` (another live favourite), or last when `None`.
    /// Returns whether anything changed (a move to the place it already holds writes nothing).
    ///
    /// [`ModelError::InvalidFavorite`] with the field `pageId` when the page is not a live
    /// favourite, with the field `before` when `before` is not one.
    ///
    /// Not recorded on the undo stack (committed with [`FAVORITES_ORIGIN`]).
    pub fn move_favorite(
        &mut self,
        page_id: PageId,
        before: Option<PageId>,
    ) -> Result<bool, ModelError> {
        let live = self.live_favorites();
        let Some(current) = live.iter().position(|(id, _)| *id == page_id) else {
            return Err(refused("pageId", "the page is not a favourite"));
        };
        if before == Some(page_id) {
            return Ok(false);
        }
        let others: Vec<&(PageId, String)> = live.iter().filter(|(id, _)| *id != page_id).collect();
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

        self.close_group();
        let key = page_id.to_string();
        self.favorites_write(|| {
            if let Some(position) = &position {
                return meta::set_string(&self.favorites, &key, position);
            }
            // Equal or damaged neighbours: renumber the visible favourites in their new order.
            let mut order_after: Vec<PageId> = others.iter().map(|(id, _)| *id).collect();
            order_after.insert(at, page_id);
            self.renumber(&order_after)
        })?;
        Ok(true)
    }

    /// Deletes the entries of purged pages (see [`Self::purge_page`]); writes nothing when there
    /// are none. Committed with [`FAVORITES_ORIGIN`].
    pub(crate) fn forget_favorites(&self, pages: &[PageId]) -> Result<(), ModelError> {
        let stale: Vec<String> = pages
            .iter()
            .map(ToString::to_string)
            .filter(|key| self.favorites.get(key).is_some())
            .collect();
        if stale.is_empty() {
            return Ok(());
        }
        self.favorites_write(|| {
            for key in &stale {
                meta::remove(&self.favorites, key)?;
            }
            Ok(())
        })
    }

    /// The share step's read: every readable entry of the `favorites` map, live or not (hidden
    /// favourites of trashed pages included), by `(position, page id)`. See
    /// `AccountDoc::adopt_favorites`.
    pub(crate) fn all_favorite_entries(&self) -> Vec<(PageId, String)> {
        self.favorite_entries()
    }

    /// The share step's clear: deletes **every** entry of the `favorites` map, readable or not, and
    /// writes nothing when the map is already empty. Committed with [`FAVORITES_ORIGIN`], so it is
    /// not an undo step. See `AccountDoc::adopt_favorites`.
    pub(crate) fn clear_favorites(&mut self) -> Result<(), ModelError> {
        let LoroValue::Map(map) = self.favorites.get_value() else {
            return Ok(());
        };
        let keys: Vec<String> = map.keys().map(ToString::to_string).collect();
        if keys.is_empty() {
            return Ok(());
        }
        self.close_group();
        self.favorites_write(|| {
            for key in &keys {
                meta::remove(&self.favorites, key)?;
            }
            Ok(())
        })
    }

    // ----- internals --------------------------------------------------------------------------

    /// The live favourites with their positions, by `(position, page id)`.
    fn live_favorites(&self) -> Vec<(PageId, String)> {
        self.favorite_entries()
            .into_iter()
            .filter(|(id, _)| self.live_node(*id).is_some())
            .collect()
    }

    /// Every readable entry (a page id key with a string position), live or not, by
    /// `(position, page id)`.
    fn favorite_entries(&self) -> Vec<(PageId, String)> {
        let LoroValue::Map(map) = self.favorites.get_value() else {
            return Vec::new();
        };
        let mut out: Vec<(PageId, String)> = map
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

    /// The stored position of an entry; `None` when absent or not a string.
    fn stored_position(&self, key: &str) -> Option<String> {
        meta::string(&self.favorites, key)
    }

    /// Writes fresh, evenly spread positions for `order` (first to last).
    fn renumber(&self, order: &[PageId]) -> Result<(), ModelError> {
        for (id, position) in order.iter().zip(order::sequence(order.len())) {
            meta::set_string(&self.favorites, &id.to_string(), &position)?;
        }
        Ok(())
    }

    /// A favourite write, committed on its own with [`FAVORITES_ORIGIN`] so that the undo manager
    /// leaves it out (anything pending is committed first, normally).
    fn favorites_write<T>(
        &self,
        write: impl FnOnce() -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        self.commit();
        let result = write();
        self.doc
            .inner()
            .commit_with(CommitOptions::new().origin(FAVORITES_ORIGIN));
        result
    }

    /// The `favorites` entry of the portable JSON: page id -> position of every readable entry,
    /// hidden and unknown ones included (the file is the whole document); `None` when the map is
    /// empty, so documents without favourites export exactly as before.
    pub(crate) fn portable_favorites(&self) -> Option<serde_json::Value> {
        let entries = self.favorite_entries();
        if entries.is_empty() {
            return None;
        }
        let mut sorted: Vec<(String, String)> = entries
            .into_iter()
            .map(|(id, position)| (id.to_string(), position))
            .collect();
        sorted.sort_unstable();
        let mut out = serde_json::Map::new();
        for (id, position) in sorted {
            out.insert(id, position.into());
        }
        Some(out.into())
    }

    /// Problems with the `favorites` map for `check_invariants`: entries that are not a page id
    /// with a string position. (An entry of a page that is gone is not a problem: it is ignored.)
    pub(crate) fn favorite_problems(&self) -> Vec<String> {
        let LoroValue::Map(map) = self.favorites.get_value() else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        for (key, value) in map.iter() {
            if key.parse::<PageId>().is_err() {
                problems.push("a favorites entry has a malformed key".into());
                continue;
            }
            match value {
                // A position is a base-62 string (`order::key_between` reads any such one).
                LoroValue::String(position)
                    if order::key_between(Some(position.as_str()), None).is_some() => {}
                _ => problems.push(format!("favorite {key} has no readable position")),
            }
        }
        problems.sort_unstable();
        problems
    }
}
