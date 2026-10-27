//! Duplicating a subtree (ADR-0040, "Duplicate a page with its sub-pages").
//!
//! [`StructureDoc::duplicate_subtree`] writes the copies of a live page and its live sub-pages
//! (trashed sub-pages hang under the trash root, so they are not part of the live subtree) in two
//! steps:
//!
//! 1. **The pages**: one commit that creates every copy, the top copy right after the original
//!    among its live siblings and the others under the copies of their parents, with the title
//!    (the given one for the top copy, the original's for the others), a new creation time and
//!    creator, and the node meta of [`COPIED_META`]. The commit carries no special origin, so
//!    structural undo records the whole creation as **one step**: one undo removes every copy,
//!    one redo brings them all back (Loro re-creates the nodes with their metadata).
//! 2. **The values**: each copy gets the property values of its original through
//!    [`StructureDoc::set_page_property`], the normal value path, so a two-way relation writes the
//!    paired side of the page it lists too. A relation value that lists a page of the copied
//!    subtree lists that page's copy (the copies relate among themselves, as the originals do),
//!    any other related page is kept. Values are never an undo step (their origin is excluded
//!    from structural undo), like every value write.
//!
//! Everything that can be checked is checked before anything is written: an unknown or trashed
//! page, a page of the subtree without a copy id, a copy id that is taken (live, trashed, purged
//! or listed twice).
//!
//! **Several pages at once** (the bulk duplicate of ADR-0058): [`StructureDoc::duplicate_subtrees`]
//! creates the copies of several tops in the one commit, each top copy right after its page, so
//! they are one structural undo step; the values are copied after, as for one page.
//!
//! **A page made from a template** (ADR-0056, `docs/architecture/templates.md` section 5) is the
//! same copy under another parent, at an index: [`StructureDoc::create_from_template`]. The copies
//! are never templates (no `template` flag, no `template_values`) and are titled with the tokens
//! `{{date}}` and `{{time}}` of the template's titles resolved (no " (copy)"); after the values are
//! copied, each page's placeholders become values (`today`, `now` a date, `me` the account) and the
//! request's values are written on the top copy, winning over a placeholder of the same property.
//! A duplicate, on the other hand, copies the flag and the placeholders (the duplicate of a
//! template is a template).

use std::collections::{HashMap, HashSet};

use kb_types::{AccountId, DateValue, PageId, PropertyId, PropertyValue, TemplatePlaceholder};
use loro::{TreeID, TreeParentId};

use crate::meta::{self, engine};
use crate::page::LocalMoment;
use crate::templates::{placeholder_fits, read_values};
use crate::{ModelError, StructureDoc};

/// The longest page title, in characters (`kb-core`'s `MAX_PAGE_TITLE_CHARS`).
const MAX_TITLE_CHARS: usize = 500;

/// The node meta keys a copy takes over from its original as they are, besides what the copy sets
/// itself (id, title, creation time, edit stamp, creator) and the property values (written through
/// the value path). The page's look belongs here when it is stored in the node meta: the cover and
/// its position, the font, the small text and the lock (ADR-0040; a copy of a locked page is locked).
const COPIED_META: &[&str] = &[
    meta::ICON,
    meta::COVER,
    meta::COVER_Y,
    meta::FONT,
    meta::SMALL_TEXT,
    meta::LOCKED,
];

/// The node meta keys only a duplicate takes over (ADR-0056): the duplicate of a template is a
/// template, with its placeholders. A page made from a template takes neither.
const DUPLICATED_META: &[&str] = &[meta::TEMPLATE, meta::TEMPLATE_VALUES];

/// What [`StructureDoc::duplicate_subtree`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicated {
    /// `(original, copy)` for every page copied, pre-order, the top page first.
    pub pages: Vec<(PageId, PageId)>,
    /// The pages whose stored values the copy wrote: copies that have values, and the pages
    /// outside the copy whose side of a two-way relation gained a copy (the owner refreshes their
    /// derived rows). Without duplicates, in the order they were written.
    pub touched: Vec<PageId>,
}

/// What a creation from a template adds to the copy of a subtree (ADR-0056).
#[derive(Debug, Clone, Copy)]
pub struct TemplateCopy<'a> {
    /// The parent of the top copy; `None` is the top level of the space. A live page that is not in
    /// the subtree being copied.
    pub parent: Option<PageId>,
    /// The position of the top copy among its parent's live children, clamped; `None` is last.
    pub index: Option<usize>,
    /// The local day and time of the request: the tokens of the titles and the `today` and `now`
    /// placeholders.
    pub moment: &'a LocalMoment,
    /// The account of the device, for the `me` placeholders; `None` (a personal space before the
    /// recovery kit exists) leaves them unresolved: the property has no value.
    pub me: Option<AccountId>,
    /// Values written on the top copy after the placeholders, in order; each is checked like
    /// [`StructureDoc::set_page_property`] does before anything is written.
    pub values: &'a [(PropertyId, PropertyValue)],
}

/// How the copies differ from their originals.
#[derive(Clone, Copy)]
enum Style<'a> {
    /// A duplicate (ADR-0040): the top copy goes right after its original with this title.
    Duplicate { top_title: &'a str },
    /// A page made from a template (ADR-0056).
    Template(&'a TemplateCopy<'a>),
}

/// A page of the live subtree: its node, its id and its parent page (`None` for the top page).
type Member = (TreeID, PageId, Option<PageId>);

/// The pages whose stored values a copy wrote, without duplicates, in the order they were written.
#[derive(Default)]
struct Touched {
    pages: Vec<PageId>,
    seen: HashSet<PageId>,
}

impl Touched {
    fn add(&mut self, written: Vec<PageId>) {
        for page in written {
            if self.seen.insert(page) {
                self.pages.push(page);
            }
        }
    }
}

impl StructureDoc {
    /// Copies the live subtree of `page_id` (see the module documentation). `ids` holds the id of
    /// the copy of every page of the subtree ([`Self::subtree`]); the top copy is titled
    /// `top_title`, the others keep their titles, and all are created at `now_ms`.
    ///
    /// Errors, all before anything is written: [`ModelError::PageNotFound`] (unknown or purged),
    /// [`ModelError::PageTrashed`] (in the trash, itself or below a trashed page),
    /// [`ModelError::Shape`] (a page of the subtree has no id in `ids`),
    /// [`ModelError::DuplicatePage`] (a copy id exists, is trashed, purged, or is listed twice).
    pub fn duplicate_subtree(
        &mut self,
        page_id: PageId,
        ids: &HashMap<PageId, PageId>,
        top_title: &str,
        now_ms: i64,
    ) -> Result<Duplicated, ModelError> {
        self.copy_subtree(page_id, ids, Style::Duplicate { top_title }, now_ms)
    }

    /// Duplicates several live pages with their subtrees (the bulk duplicate of ADR-0058): each
    /// `(page, title of its copy)` of `tops` as [`Self::duplicate_subtree`] does one, the copy
    /// right after its page among its live siblings, but all the copies are created in **one
    /// commit**, so structural undo records them as one step. `ids` holds the id of the copy of
    /// every page of every subtree. The values are copied after, through the value path, as for
    /// one page.
    ///
    /// Errors, all before anything is written: those of [`Self::duplicate_subtree`] for any of the
    /// pages, and [`ModelError::DuplicatePage`] when a page is listed twice or is in the subtree
    /// of another (the caller drops those with [`Self::without_descendants`]). `Duplicated::pages`
    /// holds the pages of each top in pre-order, the tops in the order given.
    pub fn duplicate_subtrees(
        &mut self,
        tops: &[(PageId, String)],
        ids: &HashMap<PageId, PageId>,
        now_ms: i64,
    ) -> Result<Duplicated, ModelError> {
        let mut taken: HashSet<PageId> = HashSet::new();
        let mut groups: Vec<(TreeID, &str, Vec<Member>)> = Vec::with_capacity(tops.len());
        for (page_id, title) in tops {
            let top = self.index.node(*page_id).ok_or(ModelError::PageNotFound)?;
            let placement = self.placement(top).ok_or(ModelError::PageNotFound)?;
            if placement.trashed_top.is_some() {
                return Err(ModelError::PageTrashed);
            }
            let members = self.live_members(top);
            self.check_copy_ids(&members, ids, &mut taken)?;
            groups.push((top, title.as_str(), members));
        }
        if groups.is_empty() {
            return Ok(Duplicated {
                pages: Vec::new(),
                touched: Vec::new(),
            });
        }

        // One commit for every copy, which structural undo records as one step.
        self.close_group();
        self.commit();
        let written = self.write_duplicates(&groups, ids, now_ms);
        self.commit();
        match written {
            Ok(()) => self.settle_index(),
            Err(err) => {
                self.rebuild_index();
                return Err(err);
            }
        }
        let pages: Vec<(PageId, PageId)> = groups
            .iter()
            .flat_map(|(_, _, members)| members.iter())
            .filter_map(|(_, source, _)| Some((*source, *ids.get(source)?)))
            .collect();
        let mut touched = Touched::default();
        self.copy_values(&pages, ids, now_ms, &mut touched)?;
        Ok(Duplicated {
            pages,
            touched: touched.pages,
        })
    }

    /// Creates the copies of each group, the top copy right after its page among the live
    /// siblings as they stand when its turn comes.
    fn write_duplicates(
        &mut self,
        groups: &[(TreeID, &str, Vec<Member>)],
        ids: &HashMap<PageId, PageId>,
        now_ms: i64,
    ) -> Result<(), ModelError> {
        for (top, title, members) in groups {
            let placement = self.placement(*top).ok_or(ModelError::PageNotFound)?;
            let parent = match placement.parent {
                None => TreeParentId::Root,
                Some(parent) => {
                    TreeParentId::Node(self.index.node(parent).ok_or(ModelError::PageNotFound)?)
                }
            };
            let at = self
                .visible_children(parent)
                .iter()
                .position(|(node, _)| node == top)
                .ok_or(ModelError::PageNotFound)?;
            let place = (parent, self.engine_position(parent, None, Some(at + 1)));
            self.write_copies(
                members,
                ids,
                place,
                Style::Duplicate { top_title: title },
                now_ms,
            )?;
        }
        Ok(())
    }

    /// Checks that every page of `members` has a copy id in `ids` that is free: not a page of the
    /// document (live, trashed, residue or purged) and not in `taken`, which gets it.
    fn check_copy_ids(
        &self,
        members: &[Member],
        ids: &HashMap<PageId, PageId>,
        taken: &mut HashSet<PageId>,
    ) -> Result<(), ModelError> {
        for (_, source, _) in members {
            let copy = *ids
                .get(source)
                .ok_or_else(|| ModelError::Shape("a page of the subtree has no copy id".into()))?;
            if self.index.node(copy).is_some()
                || self.index.residue.contains_key(&copy)
                || self.is_tombstoned(copy)
                || !taken.insert(copy)
            {
                return Err(ModelError::DuplicatePage);
            }
        }
        Ok(())
    }

    /// Makes a page from a template: copies the live subtree of `template_id` under
    /// `copy.parent` at `copy.index` as pages (not templates), with the title tokens, the
    /// placeholders and `copy.values` resolved (see the module documentation). `ids` is as for
    /// [`Self::duplicate_subtree`]. The pages are one structural undo step; the values are not an
    /// undo step, as every value write. The caller has checked that `template_id` is a template.
    ///
    /// Errors, all before anything is written: those of [`Self::duplicate_subtree`],
    /// [`ModelError::ParentUnavailable`] (the parent is unknown or not live),
    /// [`ModelError::CyclicMove`] (the parent is in the subtree being copied), and
    /// [`ModelError::PropertyNotFound`] or [`ModelError::InvalidProperty`] for a value of
    /// `copy.values` that [`Self::set_page_property`] would refuse.
    pub fn create_from_template(
        &mut self,
        template_id: PageId,
        ids: &HashMap<PageId, PageId>,
        copy: &TemplateCopy<'_>,
        now_ms: i64,
    ) -> Result<Duplicated, ModelError> {
        self.copy_subtree(template_id, ids, Style::Template(copy), now_ms)
    }

    fn copy_subtree(
        &mut self,
        page_id: PageId,
        ids: &HashMap<PageId, PageId>,
        style: Style<'_>,
        now_ms: i64,
    ) -> Result<Duplicated, ModelError> {
        let top = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let placement = self.placement(top).ok_or(ModelError::PageNotFound)?;
        if placement.trashed_top.is_some() {
            return Err(ModelError::PageTrashed);
        }
        let members = self.live_members(top);
        let mut taken: HashSet<PageId> = HashSet::with_capacity(members.len());
        self.check_copy_ids(&members, ids, &mut taken)?;
        let place = match style {
            Style::Duplicate { .. } => {
                let parent = match placement.parent {
                    None => TreeParentId::Root,
                    Some(parent) => {
                        TreeParentId::Node(self.index.node(parent).ok_or(ModelError::PageNotFound)?)
                    }
                };
                // Right after the original among the live siblings (hidden nodes and the trash
                // root may sit in between: `engine_position` anchors on the live sibling that
                // follows).
                let at = self
                    .visible_children(parent)
                    .iter()
                    .position(|(node, _)| *node == top)
                    .ok_or(ModelError::PageNotFound)?;
                (parent, self.engine_position(parent, None, Some(at + 1)))
            }
            Style::Template(spec) => {
                let parent = match spec.parent {
                    None => TreeParentId::Root,
                    Some(parent) => {
                        if members.iter().any(|(_, source, _)| *source == parent) {
                            return Err(ModelError::CyclicMove);
                        }
                        TreeParentId::Node(
                            self.live_node(parent)
                                .ok_or(ModelError::ParentUnavailable)?,
                        )
                    }
                };
                for (property, value) in spec.values {
                    self.check_page_property(*property, value)?;
                }
                (parent, self.engine_position(parent, None, spec.index))
            }
        };

        // 1. The pages, in one commit that structural undo records as one step.
        self.close_group();
        self.commit();
        let written = self.write_copies(&members, ids, place, style, now_ms);
        self.commit();
        match written {
            Ok(()) => self.settle_index(),
            Err(err) => {
                // Some nodes may be in; the index follows the document.
                self.rebuild_index();
                return Err(err);
            }
        }
        let pages: Vec<(PageId, PageId)> = members
            .iter()
            .filter_map(|(_, source, _)| Some((*source, *ids.get(source)?)))
            .collect();

        // 2. The values, through the value path; for a template, then the placeholders and the
        //    values of the request.
        let mut touched = Touched::default();
        self.copy_values(&pages, ids, now_ms, &mut touched)?;
        if let Style::Template(spec) = style {
            self.resolve_placeholders(&pages, spec, now_ms, &mut touched)?;
            if let Some((_, top_copy)) = pages.first() {
                for (property, value) in spec.values {
                    let written =
                        self.set_page_property(*top_copy, *property, Some(value.clone()), now_ms)?;
                    touched.add(written);
                }
            }
        }
        Ok(Duplicated {
            pages,
            touched: touched.pages,
        })
    }

    /// The live subtree of `top`, pre-order, with each page's parent page (none for `top`).
    fn live_members(&self, top: TreeID) -> Vec<Member> {
        let mut out = Vec::new();
        let Some(id) = self.index.page(top) else {
            return out;
        };
        let mut stack: Vec<Member> = vec![(top, id, None)];
        while let Some((node, id, parent)) = stack.pop() {
            out.push((node, id, parent));
            for (child, child_id) in self
                .visible_children(TreeParentId::Node(node))
                .into_iter()
                .rev()
            {
                stack.push((child, child_id, Some(id)));
            }
        }
        out
    }

    /// Creates the nodes of the copies (already validated). `place` is where the top copy goes: its
    /// parent and the engine position among that parent's children.
    fn write_copies(
        &mut self,
        members: &[Member],
        ids: &HashMap<PageId, PageId>,
        place: (TreeParentId, usize),
        style: Style<'_>,
        now_ms: i64,
    ) -> Result<(), ModelError> {
        let mut nodes: HashMap<PageId, TreeID> = HashMap::with_capacity(members.len());
        // The next position under each copy: they start empty, so the count is the position.
        let mut next: HashMap<PageId, usize> = HashMap::new();
        for (at, (source_node, source, parent)) in members.iter().enumerate() {
            let copy = *ids.get(source).ok_or(ModelError::PageNotFound)?;
            let (under, position) = match parent {
                None => place,
                Some(parent) => {
                    let parent_copy = *ids.get(parent).ok_or(ModelError::PageNotFound)?;
                    let node = *nodes
                        .get(&parent_copy)
                        .ok_or(ModelError::ParentUnavailable)?;
                    let slot = next.entry(parent_copy).or_insert(0);
                    let position = *slot;
                    *slot += 1;
                    (TreeParentId::Node(node), position)
                }
            };
            let node = self.tree.create_at(under, position).map_err(engine)?;
            let new_meta = self.tree.get_meta(node).map_err(engine)?;
            let source_meta = self.tree.get_meta(*source_node).map_err(engine)?;
            meta::set_string(&new_meta, meta::ID, &copy.to_string())?;
            let title = match style {
                Style::Duplicate { top_title } if at == 0 => top_title.to_owned(),
                Style::Duplicate { .. } => {
                    meta::string(&source_meta, meta::TITLE).unwrap_or_default()
                }
                Style::Template(spec) => spec.moment.resolve_title(
                    &meta::string(&source_meta, meta::TITLE).unwrap_or_default(),
                    MAX_TITLE_CHARS,
                ),
            };
            meta::set_string(&new_meta, meta::TITLE, &title)?;
            meta::set_int(&new_meta, meta::CREATED_AT, now_ms)?;
            self.stamp_edit(&new_meta, now_ms)?;
            if let Some(author) = self.author {
                meta::set_string(&new_meta, meta::CREATED_BY, &author.to_string())?;
            }
            for key in COPIED_META {
                if let Some(value) = meta::value(&source_meta, key) {
                    new_meta.insert(key, value).map_err(engine)?;
                }
            }
            if matches!(style, Style::Duplicate { .. }) {
                for key in DUPLICATED_META {
                    if let Some(value) = meta::value(&source_meta, key) {
                        new_meta.insert(key, value).map_err(engine)?;
                    }
                }
            }
            self.index.insert(copy, node);
            nodes.insert(copy, node);
        }
        Ok(())
    }

    /// Gives each copy the values of its original (see the module documentation).
    fn copy_values(
        &mut self,
        pages: &[(PageId, PageId)],
        ids: &HashMap<PageId, PageId>,
        now_ms: i64,
        touched: &mut Touched,
    ) -> Result<(), ModelError> {
        for (source, copy) in pages {
            for (property, value) in self.page_properties(*source)? {
                if value.kind().is_computed() {
                    continue;
                }
                let value = match value {
                    PropertyValue::Relation(related) => PropertyValue::Relation(
                        related
                            .into_iter()
                            .map(|page| ids.get(&page).copied().unwrap_or(page))
                            .collect(),
                    ),
                    other => other,
                };
                match self.set_page_property(*copy, property, Some(value), now_ms) {
                    Ok(written) => touched.add(written),
                    // A value its own rules refuse now (it was readable, so this is a stored value
                    // that has become invalid): the copy goes without it rather than failing half way.
                    Err(ModelError::InvalidProperty { .. }) => {
                        tracing::warn!("a property value could not be copied");
                    }
                    Err(other) => return Err(other),
                }
            }
        }
        Ok(())
    }

    /// Resolves the placeholders of the original pages into values of their copies: `today` a
    /// date, `now` a date with the time, `me` the account (nothing when there is none). A
    /// placeholder whose property is gone or whose kind does not take it is skipped.
    fn resolve_placeholders(
        &mut self,
        pages: &[(PageId, PageId)],
        spec: &TemplateCopy<'_>,
        now_ms: i64,
        touched: &mut Touched,
    ) -> Result<(), ModelError> {
        for (source, copy) in pages {
            let node = self.index.node(*source).ok_or(ModelError::PageNotFound)?;
            let source_meta = self.tree.get_meta(node).map_err(engine)?;
            for (property, placeholder) in read_values(&source_meta) {
                let fits = self
                    .property(property)
                    .is_some_and(|def| placeholder_fits(placeholder, def.kind));
                if !fits {
                    continue;
                }
                let value = match placeholder {
                    TemplatePlaceholder::Today => PropertyValue::Date(DateValue {
                        date: spec.moment.date.clone(),
                        time: None,
                    }),
                    TemplatePlaceholder::Now => PropertyValue::Date(DateValue {
                        date: spec.moment.date.clone(),
                        time: Some(spec.moment.time.clone()),
                    }),
                    TemplatePlaceholder::Me => match spec.me {
                        Some(me) => PropertyValue::Person(vec![me]),
                        None => continue,
                    },
                };
                match self.set_page_property(*copy, property, Some(value), now_ms) {
                    Ok(written) => touched.add(written),
                    Err(ModelError::InvalidProperty { .. }) => {
                        tracing::warn!("a template placeholder could not be resolved");
                    }
                    Err(other) => return Err(other),
                }
            }
        }
        Ok(())
    }
}
