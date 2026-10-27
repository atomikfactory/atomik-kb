//! Container names, metadata keys and typed reads of Loro map values.
//!
//! The document shapes are frozen by the golden fixtures under `fixtures/doc/`:
//! renaming a key here is a breaking change that needs a new `FORMAT_VERSION`.

use kb_types::PageId;
use loro::{LoroError, LoroMap, LoroValue, ValueOrContainer};

use crate::ModelError;

/// Account document: `MovableList<space id string>`.
pub(crate) const SPACES: &str = "spaces";
/// Account document: `Map<device id string, display name>` (Milestone 4).
pub(crate) const DEVICES: &str = "devices";
/// Structure document: `Map{id, name, icon, created_at}`.
pub(crate) const SPACE: &str = "space";
/// Structure document: the page tree.
pub(crate) const PAGES: &str = kb_doc::STRUCTURE_PAGES_TREE;
/// Structure document: `Map<page id string, purged_at>`.
pub(crate) const TOMBSTONES: &str = "tombstones";
/// Structure document: `Map<attachment id string, manifest value>` (Milestone 5,
/// ADR-0020; see `attachment.rs`).
pub(crate) const ATTACHMENTS: &str = "attachments";
/// Structure document: `Map<property id string, property map>`, the space's property schema
/// (ADR-0026; see `properties.rs`).
pub(crate) const PROPERTIES: &str = "properties";
/// Structure document: `Map<page id string, position string>`, the space's favourite pages
/// (ADR-0029; see `favorites.rs`).
pub(crate) const FAVORITES: &str = "favorites";

/// Space id (space map) or page id (node meta), as a UUID string.
pub(crate) const ID: &str = "id";
/// Space display name.
pub(crate) const NAME: &str = "name";
/// Optional icon of the space or of a page; absent when none.
pub(crate) const ICON: &str = "icon";
/// Optional cover of a page (ADR-0040): `attachment:<id>`, `color:<name>` or `gradient:<name>`;
/// absent when none.
pub(crate) const COVER: &str = "cover";
/// Vertical position of the cover image, an integer 0 to 100 (percent from the top); absent means
/// 50 and is always absent without a cover.
pub(crate) const COVER_Y: &str = "coverY";
/// Optional font of a page (ADR-0040): `serif` or `mono`; absent means the default font.
pub(crate) const FONT: &str = "font";
/// Smaller text on a page (ADR-0040): `true`; absent when off.
pub(crate) const SMALL_TEXT: &str = "smallText";
/// The page opens read-only in the UI (ADR-0040): `true`; absent when off.
pub(crate) const LOCKED: &str = "locked";
/// The page is a template (ADR-0056): `true`; absent when it is not. Its sub-pages are inside the
/// template without carrying the flag.
pub(crate) const TEMPLATE: &str = "template";
/// The placeholders of a template page or a page inside one (ADR-0056): a plain map (one value,
/// replaced as a whole) from a property id string to `today`, `now` or `me`; absent when empty.
pub(crate) const TEMPLATE_VALUES: &str = "template_values";
/// Creation time, Unix ms.
pub(crate) const CREATED_AT: &str = "created_at";
/// Page title.
pub(crate) const TITLE: &str = "title";
/// Last metadata change of a page, Unix ms.
pub(crate) const UPDATED_AT: &str = "updated_at";
/// Device that created a page (device id string); absent for pages made before ADR-0026's
/// amendment of 2026-10-08.
pub(crate) const CREATED_BY: &str = "created_by";
/// Device that last edited a page, written with `updated_at`; absent when unknown.
pub(crate) const EDITED_BY: &str = "edited_by";
/// Set on a top-level trashed page; absent when live.
pub(crate) const TRASHED_AT: &str = "trashed_at";
/// Previous parent of a top-level trashed page; absent when it was a root or is live.
pub(crate) const TRASHED_FROM: &str = "trashed_from";
/// `"trash"` on trash roots; absent on pages.
pub(crate) const KIND: &str = "kind";
/// Value of [`KIND`] that marks a trash root.
pub(crate) const TRASH_KIND: &str = "trash";
/// Value of [`KIND`] that marks a moved root: the hidden root that holds the residue of the pages
/// that left the space (ADR-0038, `moves.rs`).
pub(crate) const MOVED_KIND: &str = "moved";
/// On a residue node: the space (id string) the page went to.
pub(crate) const MOVED_TO: &str = "movedTo";
/// On a residue node: when the page left, Unix ms.
pub(crate) const MOVED_AT: &str = "movedAt";
/// On a page that arrived from another space: when it arrived, Unix ms.
pub(crate) const ARRIVED_AT: &str = "arrivedAt";
/// On a page that arrived from another space: the space (id string) it came from.
pub(crate) const ARRIVED_FROM: &str = "arrivedFrom";

/// A plain value (not a child container) stored under `key`.
pub(crate) fn value(map: &LoroMap, key: &str) -> Option<LoroValue> {
    match map.get(key)? {
        ValueOrContainer::Value(value) => Some(value),
        ValueOrContainer::Container(_) => None,
    }
}

/// A string value; `None` when absent or of another type.
pub(crate) fn string(map: &LoroMap, key: &str) -> Option<String> {
    match value(map, key)? {
        LoroValue::String(s) => Some(s.to_string()),
        _ => None,
    }
}

/// An integer value; `None` when absent or of another type.
pub(crate) fn int(map: &LoroMap, key: &str) -> Option<i64> {
    match value(map, key)? {
        LoroValue::I64(v) => Some(v),
        _ => None,
    }
}

/// Whether a flag is on: `true` is stored as the boolean `true`; absent (or any other value, which a
/// foreign writer may have left) is off.
pub(crate) fn flag(map: &LoroMap, key: &str) -> bool {
    matches!(value(map, key), Some(LoroValue::Bool(true)))
}

/// A page id stored as a UUID string.
pub(crate) fn page_id(map: &LoroMap, key: &str) -> Option<PageId> {
    string(map, key)?.parse().ok()
}

/// Whether the map marks a trash root.
pub(crate) fn is_trash_marker(map: &LoroMap) -> bool {
    string(map, KIND).as_deref() == Some(TRASH_KIND)
}

/// Whether the map marks a moved root.
pub(crate) fn is_moved_marker(map: &LoroMap) -> bool {
    string(map, KIND).as_deref() == Some(MOVED_KIND)
}

/// Whether the node is residue: it carries a `movedTo` stamp (a string, valid or not).
pub(crate) fn is_residue(map: &LoroMap) -> bool {
    string(map, MOVED_TO).is_some()
}

/// Sets `key` to a string, skipping the write when the value is already there.
pub(crate) fn set_string(map: &LoroMap, key: &str, value: &str) -> Result<(), ModelError> {
    if string(map, key).as_deref() == Some(value) {
        return Ok(());
    }
    map.insert(key, value).map_err(engine)
}

/// Sets `key` to an integer, skipping the write when the value is already there.
pub(crate) fn set_int(map: &LoroMap, key: &str, value: i64) -> Result<(), ModelError> {
    if int(map, key) == Some(value) {
        return Ok(());
    }
    map.insert(key, value).map_err(engine)
}

/// Turns a flag on (stores `true`, skipping the write when it is there) or off (removes the key).
pub(crate) fn set_flag(map: &LoroMap, key: &str, on: bool) -> Result<(), ModelError> {
    if on {
        if flag(map, key) {
            return Ok(());
        }
        map.insert(key, true).map_err(engine)
    } else {
        remove(map, key)
    }
}

/// Sets or removes an optional string. Removing an absent key writes nothing (Loro
/// would record a delete operation even for a missing key).
pub(crate) fn set_opt_string(
    map: &LoroMap,
    key: &str,
    value: Option<&str>,
) -> Result<(), ModelError> {
    match value {
        Some(value) => set_string(map, key, value),
        None => remove(map, key),
    }
}

/// Removes `key` if present.
pub(crate) fn remove(map: &LoroMap, key: &str) -> Result<(), ModelError> {
    if map.get(key).is_none() {
        return Ok(());
    }
    map.delete(key).map_err(engine)
}

/// An engine failure during a local operation (all inputs are validated first, so
/// this signals a bug or a damaged document).
pub(crate) fn engine(err: LoroError) -> ModelError {
    ModelError::Doc(kb_doc::DocError::Engine(err.to_string()))
}
