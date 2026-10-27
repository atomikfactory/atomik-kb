//! Engine-independent JSON of a structure document (`StructureDoc::to_portable`).
//!
//! Shape (format 1):
//!
//! ```text
//! {"attachments"?: {"<attachment id>": {"chunkSize", "chunks": ["<hex>", ..],
//!                   "createdAt", "createdBy", "hash", "mime", "name", "size", "v"}},
//!  "favorites"?: {"<page id>": "<position>", ..},
//!  "format": 1,
//!  "pages": [{"children": [..], "cover"?, "coverY"?, "createdAt", "font"?, "icon"?, "id",
//!             "locked"?, "smallText"?, "template"?, "templateValues"?, "title", "updatedAt"}],
//!  "space": {"createdAt", "icon"?, "id", "name"},
//!  "tombstones": {"<page id>": purgedAt, ..},
//!  "trash": [{"children": [..], "cover"?, "coverY"?, "createdAt", "font"?, "icon"?, "id",
//!             "locked"?, "smallText"?, "title", "trashedAt", "trashedFrom"?, "updatedAt"}]}
//! ```
//!
//! Object keys are inserted in sorted order, so the serialised text is the same
//! whether or not `serde_json` preserves insertion order. `pages` follows the live
//! sibling order, `trash` lists the most recently trashed first (ties by id), and
//! `tombstones` is keyed by page id. Changing this shape needs a new format number.
//!
//! `attachments` (Milestone 5, ADR-0020) is an additive key: present only when the
//! document holds at least one readable manifest (unreadable ones are left out and
//! reported by `check_invariants`), so documents without attachments export exactly
//! as before. Keyed by attachment id; hex strings are lowercase.
//!
//! `favorites` (ADR-0029) is an additive key too: present only when the favourites map holds an
//! entry (the raw entries, hidden and unknown pages included: the file is the whole document),
//! keyed by page id, each value the fractional position string.
//!
//! `moved` (ADR-0038) is additive as well: present only when the document holds residue (pages
//! that moved to another space), keyed by page id, each value `{"movedAt", "movedTo": "<space id>"}`.
//! A page that arrived from another space lists `arrivedAt` and `arrivedFrom` with its other fields.
//!
//! `cover` and `coverY` (ADR-0040) are additive: present only on a page that has a cover (and
//! `coverY` only when it is not the default 50).
//!
//! `font` (`"serif"` or `"mono"`), `smallText` and `locked` (`true`) are additive as well (ADR-0040):
//! present only on a page that has them.
//!
//! `template` (`true`) and `templateValues` (`{"<property id>": "today" | "now" | "me"}`, sorted keys, the
//! page's placeholders as stored) are additive too (ADR-0056): present only on a page that has them.

use kb_types::PageId;
use loro::{TreeID, TreeParentId};
use serde_json::{Map, Value};

use crate::properties::Schema;
use crate::{PORTABLE_FORMAT, StructureDoc, meta};

/// Trash fields of a top-level trashed page.
struct TrashFields {
    trashed_at: i64,
    trashed_from: Option<PageId>,
}

pub(crate) fn structure(doc: &StructureDoc) -> Value {
    let space = doc.space();
    let mut space_obj = Map::new();
    space_obj.insert("createdAt".into(), space.created_at.into());
    if let Some(icon) = space.icon {
        space_obj.insert("icon".into(), icon.into());
    }
    space_obj.insert("id".into(), space.id.to_string().into());
    space_obj.insert("name".into(), space.name.into());

    let pages = doc.forest(
        doc.visible_children(TreeParentId::Root),
        &mut |node, id, _parent| page_object(doc, node, id, None),
        &attach_child,
    );

    let mut trash = Vec::new();
    for (node, id, trashed_at) in doc.trash_entries() {
        let trashed_from = doc
            .meta_of(node)
            .and_then(|m| meta::page_id(&m, meta::TRASHED_FROM));
        let mut top = Some(TrashFields {
            trashed_at,
            trashed_from,
        });
        trash.extend(doc.forest(
            vec![(node, id)],
            &mut |node, id, _parent| page_object(doc, node, id, top.take()),
            &attach_child,
        ));
    }

    let mut tombstones = Map::new();
    for (id, purged_at) in doc.tombstones() {
        tombstones.insert(id.to_string(), purged_at.into());
    }

    let mut attachments = Map::new();
    for (_, manifest) in doc.manifest_entries() {
        if let Ok(manifest) = manifest {
            attachments.insert(manifest.attachment_id.to_string(), manifest.to_portable());
        }
    }

    let mut out = Map::new();
    if !attachments.is_empty() {
        out.insert("attachments".into(), attachments.into());
    }
    if let Some(favorites) = doc.portable_favorites() {
        out.insert("favorites".into(), favorites);
    }
    out.insert("format".into(), PORTABLE_FORMAT.into());
    if let Some(moved) = doc.portable_moved() {
        out.insert("moved".into(), moved);
    }
    out.insert("pages".into(), pages.into());
    if let Some(properties) = doc.portable_properties() {
        out.insert("properties".into(), properties);
    }
    out.insert("space".into(), space_obj.into());
    out.insert("tombstones".into(), tombstones.into());
    out.insert("trash".into(), trash.into());
    out.into()
}

fn page_object(doc: &StructureDoc, node: TreeID, id: PageId, trash: Option<TrashFields>) -> Value {
    let info = doc.info(id, node, None, None, &Schema::empty());
    let mut obj = Map::new();
    if let Some(meta_map) = doc.meta_of(node) {
        if let Some(at) = crate::meta::int(&meta_map, crate::meta::ARRIVED_AT) {
            obj.insert("arrivedAt".into(), at.into());
        }
        if let Some(from) = crate::meta::string(&meta_map, crate::meta::ARRIVED_FROM) {
            obj.insert("arrivedFrom".into(), from.into());
        }
    }
    obj.insert("children".into(), Value::Array(Vec::new()));
    if let Some(cover) = info.cover {
        obj.insert("cover".into(), cover.into());
    }
    if let Some(cover_y) = info.cover_y {
        obj.insert("coverY".into(), cover_y.into());
    }
    obj.insert("createdAt".into(), info.created_at.into());
    if let Some(meta_map) = doc.meta_of(node) {
        if let Some(device) = crate::meta::string(&meta_map, crate::meta::CREATED_BY) {
            obj.insert("createdBy".into(), device.into());
        }
        if let Some(device) = crate::meta::string(&meta_map, crate::meta::EDITED_BY) {
            obj.insert("editedBy".into(), device.into());
        }
    }
    if let Some(font) = info.font.and_then(kb_types::PageFont::stored) {
        obj.insert("font".into(), font.into());
    }
    if let Some(icon) = info.icon {
        obj.insert("icon".into(), icon.into());
    }
    obj.insert("id".into(), id.to_string().into());
    if info.locked {
        obj.insert("locked".into(), true.into());
    }
    if let Some(props) = doc.portable_props(node) {
        obj.insert("props".into(), props);
    }
    if info.small_text {
        obj.insert("smallText".into(), true.into());
    }
    if info.template {
        obj.insert("template".into(), true.into());
    }
    if let Some(values) = doc.portable_template_values(node) {
        obj.insert("templateValues".into(), values);
    }
    obj.insert("title".into(), info.title.into());
    if let Some(trash) = trash {
        obj.insert("trashedAt".into(), trash.trashed_at.into());
        if let Some(from) = trash.trashed_from {
            obj.insert("trashedFrom".into(), from.to_string().into());
        }
    }
    obj.insert("updatedAt".into(), info.updated_at.into());
    obj.into()
}

fn attach_child(parent: &mut Value, child: Value) {
    if let Some(Value::Array(children)) = parent.get_mut("children") {
        children.push(child);
    }
}
