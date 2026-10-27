//! The structure document's `links` map (M13 slice 4, ADR-0051, `docs/development/milestone-13.md`
//! sections 4.1 and 4.2): the view links of a space, beside the server's rows and the `Link` records
//! of the keyring. The map is what members list ("the links of this space"), what an Owner's device
//! reads to find a `Link` record whose entry is gone (an abandoned link whose creator could not
//! tombstone it), and what revocation removes.
//!
//! ```text
//! root map "links": "<link id>": {"v": 1, "page": "<page id>", "created_by": "<account id>",
//!                                 "created_at": i64, "expires_at": i64 (absent: never)}
//! ```
//!
//! One plain Loro map value per link (not a container): it is written as a whole and removed as a
//! whole, like an attachment manifest; the link key never appears here (it is a keyring record). Readers
//! ignore keys they do not know and entries that do not parse (another version, damaged data), so an
//! entry a later version wrote is not a reason to refuse the document. The map is an additive root map of
//! the structure document, merged per key; writes are committed with [`LINKS_ORIGIN`], which the undo
//! manager excludes (undo and redo never create or revoke a link).
//!
//! A link whose page was purged is a finding of `check_invariants`, not an error (the entry stays until an
//! Owner revokes the link, which also deletes the server's row). The shape is frozen by
//! `fixtures/doc/structure-v1-links.*`.

use std::collections::HashMap;

use kb_types::{AccountId, LinkId, PageId};
use loro::{CommitOptions, LoroValue};

use crate::meta;
use crate::{ModelError, StructureDoc};

/// Name of the structure document's root map of view links.
pub const LINKS_MAP: &str = "links";
/// Commit origin of the writes of this module; the undo manager excludes it.
pub const LINKS_ORIGIN: &str = "kb-model/links";
/// The version of a link entry this code writes and the only one it reads.
pub const LINK_ENTRY_VERSION: u8 = 1;

/// Keys of a link entry value.
mod key {
    pub(super) const V: &str = "v";
    pub(super) const PAGE: &str = "page";
    pub(super) const CREATED_BY: &str = "created_by";
    pub(super) const CREATED_AT: &str = "created_at";
    pub(super) const EXPIRES_AT: &str = "expires_at";
}

/// One view link of a space, as the `links` map stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewLinkRecord {
    /// The link (`HKDF(K_link, "link-id")[..16]`; the map key).
    pub link_id: LinkId,
    /// The page the link shows.
    pub page_id: PageId,
    /// The member who created it.
    pub created_by: AccountId,
    /// When, Unix ms.
    pub created_at: i64,
    /// When it stops working, Unix ms; `None` for a link that never expires.
    pub expires_at: Option<i64>,
}

impl ViewLinkRecord {
    /// The Loro value stored under the link id (see the module documentation).
    fn to_loro(&self) -> LoroValue {
        let mut map: HashMap<&str, LoroValue> = HashMap::with_capacity(5);
        map.insert(key::V, LoroValue::I64(i64::from(LINK_ENTRY_VERSION)));
        map.insert(key::PAGE, LoroValue::from(self.page_id.to_string()));
        map.insert(
            key::CREATED_BY,
            LoroValue::from(self.created_by.to_string()),
        );
        map.insert(key::CREATED_AT, LoroValue::I64(self.created_at));
        if let Some(expires_at) = self.expires_at {
            map.insert(key::EXPIRES_AT, LoroValue::I64(expires_at));
        }
        LoroValue::from(map)
    }

    /// Reads the value stored under `link_id`; `None` for anything that is not an entry of this
    /// version (another version, a missing or mistyped field, a malformed id).
    fn from_loro(link_id: LinkId, value: &LoroValue) -> Option<Self> {
        if !matches!(value, LoroValue::Map(_)) {
            return None;
        }
        // The version first: another version may have other fields.
        if int(value, key::V)? != i64::from(LINK_ENTRY_VERSION) {
            return None;
        }
        let expires_at = match value.get_by_key(key::EXPIRES_AT) {
            None => None,
            Some(LoroValue::I64(at)) => Some(*at),
            Some(_) => return None,
        };
        Some(Self {
            link_id,
            page_id: string(value, key::PAGE)?.parse().ok()?,
            created_by: string(value, key::CREATED_BY)?.parse().ok()?,
            created_at: int(value, key::CREATED_AT)?,
            expires_at,
        })
    }
}

fn int(value: &LoroValue, key: &str) -> Option<i64> {
    match value.get_by_key(key) {
        Some(LoroValue::I64(v)) => Some(*v),
        _ => None,
    }
}

fn string(value: &LoroValue, key: &str) -> Option<String> {
    match value.get_by_key(key) {
        Some(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

impl StructureDoc {
    /// Writes (or replaces) the entry of `record.link_id`. Not recorded on the undo stack.
    pub fn link_put(&self, record: &ViewLinkRecord) -> Result<(), ModelError> {
        let value = record.to_loro();
        let key = record.link_id.to_string();
        self.links_write(|| self.links.insert(&key, value).map_err(meta::engine))
    }

    /// The entry of a link; `None` when absent or unreadable.
    #[must_use]
    pub fn link(&self, link_id: LinkId) -> Option<ViewLinkRecord> {
        let value = meta::value(&self.links, &link_id.to_string())?;
        ViewLinkRecord::from_loro(link_id, &value)
    }

    /// Every readable entry, newest `created_at` first, then by link id.
    #[must_use]
    pub fn links(&self) -> Vec<ViewLinkRecord> {
        let mut out: Vec<ViewLinkRecord> = self
            .link_entries()
            .into_iter()
            .filter_map(|(_, record)| record)
            .collect();
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.link_id.cmp(&b.link_id))
        });
        out
    }

    /// Removes the entry of a link (a no-op when absent). Not recorded on the undo stack.
    pub fn link_remove(&self, link_id: LinkId) -> Result<(), ModelError> {
        let key = link_id.to_string();
        self.links_write(|| meta::remove(&self.links, &key))
    }

    /// A write to the `links` map, committed on its own with [`LINKS_ORIGIN`] so that the undo
    /// manager leaves it out (anything pending is committed first).
    fn links_write(
        &self,
        write: impl FnOnce() -> Result<(), ModelError>,
    ) -> Result<(), ModelError> {
        self.commit();
        let result = write();
        self.doc
            .inner()
            .commit_with(CommitOptions::new().origin(LINKS_ORIGIN));
        result
    }

    /// Every entry of the `links` map by key: the link id (`None` when the key is not one) and the
    /// record read from the value (`None` when it does not parse).
    fn link_entries(&self) -> Vec<(Option<LinkId>, Option<ViewLinkRecord>)> {
        let LoroValue::Map(map) = self.links.get_value() else {
            return Vec::new();
        };
        map.iter()
            .map(|(key, value)| match key.parse::<LinkId>() {
                Ok(id) => (Some(id), ViewLinkRecord::from_loro(id, value)),
                Err(_) => (None, None),
            })
            .collect()
    }

    /// Findings of `check_invariants` about the `links` map: entries that are malformed, and
    /// entries of a page that was purged. Link and page ids only, never a name or a key; a
    /// stale entry is a finding, not an error (an Owner revokes the link). Sorted.
    pub(crate) fn link_problems(&self) -> Vec<String> {
        let purged = crate::index::tombstoned(&self.tombstones);
        let mut problems = Vec::new();
        for (id, record) in self.link_entries() {
            match (id, record) {
                (None, _) => problems.push("a links entry has a malformed key".to_owned()),
                (Some(id), None) => {
                    problems.push(format!("link {} has no readable entry", id.short_hex()));
                }
                (Some(id), Some(record)) => {
                    if purged.contains(&record.page_id) {
                        problems.push(format!(
                            "link {} names the purged page {}",
                            id.short_hex(),
                            record.page_id
                        ));
                    }
                }
            }
        }
        problems.sort_unstable();
        problems
    }
}
