//! Templates (ADR-0056, `docs/architecture/templates.md`): the template flag of a page and the
//! placeholders of its properties.
//!
//! ```text
//! page node metadata: template: true            (absent when the page is not a template)
//!                     template_values: map      (absent when empty)
//!                       <property id> -> "today" | "now" | "me"
//! ```
//!
//! A **template** is a page whose metadata has the flag; its sub-pages are part of it without
//! carrying the flag, so "in a template" means the page or an ancestor is flagged
//! ([`StructureDoc::in_template`]). A page that leaves a template (a move out, the flag cleared)
//! keeps its map; it is not reported until the page is in a template again.
//!
//! The map is one plain value, replaced as a whole by every write (placeholders are rare edits, so
//! last writer wins), and is written like the appearance: the edit key merges the changes of one
//! page within [`crate::UNDO_MERGE_WINDOW_MS`] into one undo step, and the page is stamped. A
//! placeholder replaces the property's stored value on the page: setting one removes the value in
//! the same commit, and a write of the value ([`StructureDoc::set_page_property`]) removes the
//! placeholder in its commit.

use std::collections::{HashMap, HashSet};

use kb_types::{PageId, PropertyId, PropertyKind, TemplatePlaceholder};
use loro::{LoroMap, LoroValue, TreeID, TreeParentId};

use crate::meta::{self, engine};
use crate::properties::Schema;
use crate::{EditKey, ModelError, StructureDoc};

/// Whether a property of `kind` takes `placeholder`: `today` and `now` a `date`, `me` a `person`.
#[must_use]
pub fn placeholder_fits(placeholder: TemplatePlaceholder, kind: PropertyKind) -> bool {
    match placeholder {
        TemplatePlaceholder::Today | TemplatePlaceholder::Now => kind == PropertyKind::Date,
        TemplatePlaceholder::Me => kind == PropertyKind::Person,
    }
}

/// The placeholders a page's metadata holds as written (entries that are not a property id with a
/// placeholder this version knows are left out), sorted by property id.
pub(crate) fn read_values(meta_map: &LoroMap) -> Vec<(PropertyId, TemplatePlaceholder)> {
    let Some(LoroValue::Map(entries)) = meta::value(meta_map, meta::TEMPLATE_VALUES) else {
        return Vec::new();
    };
    let mut out: Vec<(PropertyId, TemplatePlaceholder)> = entries
        .iter()
        .filter_map(|(raw, value)| {
            let id = raw.parse::<PropertyId>().ok()?;
            match value {
                LoroValue::String(name) => Some((id, TemplatePlaceholder::parse(name)?)),
                _ => None,
            }
        })
        .collect();
    out.sort_unstable_by_key(|(id, _)| *id);
    out
}

/// Writes the placeholders (the whole map; no entry removes the key).
pub(crate) fn write_values(
    meta_map: &LoroMap,
    entries: &[(PropertyId, TemplatePlaceholder)],
) -> Result<(), ModelError> {
    if entries.is_empty() {
        return meta::remove(meta_map, meta::TEMPLATE_VALUES);
    }
    let map: HashMap<String, LoroValue> = entries
        .iter()
        .map(|(id, placeholder)| (id.to_string(), LoroValue::from(placeholder.as_str())))
        .collect();
    meta_map
        .insert(meta::TEMPLATE_VALUES, LoroValue::from(map))
        .map_err(engine)
}

fn refused(field: &'static str, message: &str) -> ModelError {
    ModelError::InvalidProperty {
        field,
        message: message.to_owned(),
    }
}

impl StructureDoc {
    /// Whether the page is a template or inside one: the page or an ancestor has the template flag
    /// (ADR-0056). `false` for an unknown page. A trashed page is looked at where the tree holds it
    /// (a top-level trashed page is no longer under its former parent).
    #[must_use]
    pub fn in_template(&self, page_id: PageId) -> bool {
        self.index
            .node(page_id)
            .is_some_and(|node| self.node_in_template(node))
    }

    /// Every page of the document, live or trashed, that is a template or inside one.
    #[must_use]
    pub fn pages_in_templates(&self) -> HashSet<PageId> {
        let mut out: HashSet<PageId> = HashSet::new();
        let tops: Vec<(TreeID, PageId)> = self
            .visible_children(TreeParentId::Root)
            .into_iter()
            .chain(self.top_trashed())
            .collect();
        for (top, id) in tops {
            // `(node, page, an ancestor is a template)`, depth first without recursion.
            let mut stack = vec![(top, id, false)];
            while let Some((node, id, inherited)) = stack.pop() {
                let inside = inherited || self.node_is_template(node);
                if inside {
                    out.insert(id);
                }
                for (child, child_id) in self.visible_children(TreeParentId::Node(node)) {
                    stack.push((child, child_id, inside));
                }
            }
        }
        out
    }

    /// Makes a page a template or a page again (ADR-0056). Returns the affected ids: the page and
    /// its whole live subtree (their page index rows say whether they are in a template), or none
    /// when the page already is what is asked.
    ///
    /// The flag is stored as the boolean `true` (turning it off removes the key); the placeholders
    /// of the page stay. Written like the appearance: one undo step, merged with the page's other
    /// template edits within the merge window, and the page is stamped. `PageTrashed` for a page
    /// in the trash.
    pub fn set_page_template(
        &mut self,
        page_id: PageId,
        template: bool,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        if !self.is_live(node) {
            return Err(ModelError::PageTrashed);
        }
        let meta_map = self.tree.get_meta(node).map_err(engine)?;
        if meta::flag(&meta_map, meta::TEMPLATE) == template {
            return Ok(Vec::new());
        }
        self.begin_edit(Some(EditKey::Template(page_id)), now_ms);
        meta::set_flag(&meta_map, meta::TEMPLATE, template)?;
        self.stamp_edit(&meta_map, now_ms)?;
        self.commit();
        Ok(self.preorder(node))
    }

    /// Sets or, with `None`, clears the placeholder of one property on a page (ADR-0056). Returns
    /// the page when something was written, nothing when the page already is as asked.
    ///
    /// Setting a placeholder removes the page's stored value of the property in the same commit
    /// (it replaces the value); clearing writes the map only. Entries of properties the schema no
    /// longer has go with the write. Written like [`Self::set_page_template`] (one undo step, the
    /// same edit key). `PageNotFound`, `PropertyNotFound`; `PageTrashed` for a page in the trash;
    /// `InvalidProperty` for the field `pageId` when the page is not in a template and for
    /// `placeholder` when the property's kind does not take the placeholder
    /// ([`placeholder_fits`]).
    pub fn set_template_placeholder(
        &mut self,
        page_id: PageId,
        property_id: PropertyId,
        placeholder: Option<TemplatePlaceholder>,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let schema = self.read_schema();
        let kind = schema
            .kind_of(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        if !self.is_live(node) {
            return Err(ModelError::PageTrashed);
        }
        if !self.node_in_template(node) {
            return Err(refused(
                "pageId",
                "the page is not a template and not inside one",
            ));
        }
        if let Some(placeholder) = placeholder
            && !placeholder_fits(placeholder, kind)
        {
            return Err(refused(
                "placeholder",
                "the property's kind does not take that placeholder",
            ));
        }

        let meta_map = self.tree.get_meta(node).map_err(engine)?;
        let mut entries = read_values(&meta_map);
        let current = entries
            .iter()
            .find(|(id, _)| *id == property_id)
            .map(|(_, placeholder)| *placeholder);
        let value_key = property_id.to_string();
        let stored_value = self
            .props_map(node)
            .filter(|props| props.get(&value_key).is_some());
        let unchanged = match placeholder {
            // The placeholder is there; a value stored next to it (a concurrent edit) goes now.
            Some(wanted) => current == Some(wanted) && stored_value.is_none(),
            None => current.is_none(),
        };
        if unchanged {
            return Ok(Vec::new());
        }

        entries.retain(|(id, _)| *id != property_id && schema.kind_of(*id).is_some());
        if let Some(placeholder) = placeholder {
            entries.push((property_id, placeholder));
        }
        entries.sort_unstable_by_key(|(id, _)| *id);

        self.begin_edit(Some(EditKey::Template(page_id)), now_ms);
        if placeholder.is_some()
            && let Some(props) = stored_value
        {
            meta::remove(&props, &value_key)?;
        }
        write_values(&meta_map, &entries)?;
        self.stamp_edit(&meta_map, now_ms)?;
        self.commit();
        Ok(vec![page_id])
    }

    /// The placeholders of a page node as the page reports them: in schema order, only while the
    /// page is in a template, only for properties that exist and whose kind takes the placeholder.
    pub(crate) fn placeholders_of(
        &self,
        node: TreeID,
        schema: &Schema,
    ) -> Vec<(PropertyId, TemplatePlaceholder)> {
        let Some(meta_map) = self.meta_of(node) else {
            return Vec::new();
        };
        let stored = read_values(&meta_map);
        if stored.is_empty() || !self.node_in_template(node) {
            return Vec::new();
        }
        schema
            .ids()
            .filter_map(|id| {
                let (_, placeholder) = stored.iter().find(|(stored_id, _)| *stored_id == id)?;
                placeholder_fits(*placeholder, schema.kind_of(id)?).then_some((id, *placeholder))
            })
            .collect()
    }

    /// The placeholders a page node stores, as written (sorted by property id): what a move
    /// between spaces carries, and what the portable JSON lists.
    pub(crate) fn stored_placeholders(
        &self,
        node: TreeID,
    ) -> Vec<(PropertyId, TemplatePlaceholder)> {
        self.meta_of(node)
            .map(|meta_map| read_values(&meta_map))
            .unwrap_or_default()
    }

    /// Whether the page node holds a placeholder for the property (in a template or not).
    pub(crate) fn has_template_placeholder(&self, node: TreeID, property_id: PropertyId) -> bool {
        self.meta_of(node).is_some_and(|meta_map| {
            read_values(&meta_map)
                .iter()
                .any(|(id, _)| *id == property_id)
        })
    }

    /// Removes the placeholder of a property from the page's map (no write when it has none).
    pub(crate) fn clear_template_placeholder(
        &self,
        meta_map: &LoroMap,
        property_id: PropertyId,
    ) -> Result<(), ModelError> {
        let mut entries = read_values(meta_map);
        let before = entries.len();
        entries.retain(|(id, _)| *id != property_id);
        if entries.len() == before {
            return Ok(());
        }
        write_values(meta_map, &entries)
    }

    /// The `templateValues` entry of a page in the portable JSON: the stored map by property id
    /// (sorted keys), or `None` when the page has none.
    pub(crate) fn portable_template_values(&self, node: TreeID) -> Option<serde_json::Value> {
        let entries = self.stored_placeholders(node);
        if entries.is_empty() {
            return None;
        }
        let mut sorted: Vec<(String, &'static str)> = entries
            .iter()
            .map(|(id, placeholder)| (id.to_string(), placeholder.as_str()))
            .collect();
        sorted.sort_unstable();
        let mut out = serde_json::Map::new();
        for (id, name) in sorted {
            out.insert(id, name.into());
        }
        Some(out.into())
    }

    /// Whether the node itself has the template flag.
    fn node_is_template(&self, node: TreeID) -> bool {
        self.meta_of(node)
            .is_some_and(|meta_map| meta::flag(&meta_map, meta::TEMPLATE))
    }

    /// Whether the node or an ancestor has the template flag (a bounded walk up the tree).
    fn node_in_template(&self, node: TreeID) -> bool {
        let mut current = node;
        for _ in 0..=self.index.pages.len() + 1 {
            if self.node_is_template(current) {
                return true;
            }
            match self.tree.parent(current) {
                Some(TreeParentId::Node(up)) => current = up,
                _ => return false,
            }
        }
        false
    }
}

/// A page of an import that is a template or holds placeholders (ADR-0056, ADR-0036), by the ids of
/// the space it is imported into: what [`StructureDoc::import_templates`] writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedTemplate {
    /// The page, created earlier in the import.
    pub page_id: PageId,
    /// The page is a template.
    pub template: bool,
    /// Its placeholders; the ones whose property is not in the schema or does not take the placeholder
    /// are left out.
    pub placeholders: Vec<(PropertyId, TemplatePlaceholder)>,
}

impl StructureDoc {
    /// Sets the template flag and the placeholders of imported pages, after their content was written
    /// (ADR-0036: the person undoes an import by trashing its container, so the write carries
    /// [`crate::IMPORT_ORIGIN`], which structural undo excludes, and does not stamp the pages).
    ///
    /// The flags go first, because a placeholder is written only on a page that is in a template (the
    /// page or an ancestor flagged): other entries are skipped, like a page that is not live or not
    /// known. A placeholder replaces the page's stored value of the property, as
    /// [`Self::set_template_placeholder`] does. Returns the ids to rewrite the page index rows of: the
    /// flagged pages with their live subtrees, and the pages that got placeholders.
    pub fn import_templates(
        &mut self,
        pages: &[ImportedTemplate],
    ) -> Result<Vec<PageId>, ModelError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }
        self.close_group();
        self.commit();
        let written = self.write_imported_templates(pages);
        self.doc
            .inner()
            .commit_with(loro::CommitOptions::new().origin(crate::IMPORT_ORIGIN));
        written
    }

    fn write_imported_templates(
        &self,
        pages: &[ImportedTemplate],
    ) -> Result<Vec<PageId>, ModelError> {
        let schema = self.read_schema();
        let mut affected: Vec<PageId> = Vec::new();
        let mut note = |ids: Vec<PageId>| {
            for id in ids {
                if !affected.contains(&id) {
                    affected.push(id);
                }
            }
        };
        for page in pages.iter().filter(|page| page.template) {
            let Some(node) = self
                .index
                .node(page.page_id)
                .filter(|node| self.is_live(*node))
            else {
                continue;
            };
            let meta_map = self.tree.get_meta(node).map_err(engine)?;
            meta::set_flag(&meta_map, meta::TEMPLATE, true)?;
            note(self.preorder(node));
        }
        for page in pages.iter().filter(|page| !page.placeholders.is_empty()) {
            let Some(node) = self
                .index
                .node(page.page_id)
                .filter(|node| self.is_live(*node) && self.node_in_template(*node))
            else {
                continue;
            };
            let meta_map = self.tree.get_meta(node).map_err(engine)?;
            let mut entries = read_values(&meta_map);
            let mut written = false;
            for (property_id, placeholder) in &page.placeholders {
                let fits = schema
                    .kind_of(*property_id)
                    .is_some_and(|kind| placeholder_fits(*placeholder, kind));
                if !fits {
                    continue;
                }
                entries.retain(|(known, _)| known != property_id);
                entries.push((*property_id, *placeholder));
                if let Some(props) = self.props_map(node) {
                    meta::remove(&props, &property_id.to_string())?;
                }
                written = true;
            }
            if written {
                entries.sort_unstable_by_key(|(id, _)| *id);
                write_values(&meta_map, &entries)?;
                note(vec![page.page_id]);
            }
        }
        Ok(affected)
    }
}
