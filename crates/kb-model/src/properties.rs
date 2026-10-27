//! Typed page properties in the structure document (ADR-0026, `docs/data-model/domain-model.md`,
//! "Properties and tags").
//!
//! A space has a **schema** of properties; every page has one **value** per property it sets.
//! Both live in the structure document, so they sync and are encrypted like the page tree.
//!
//! ```text
//! root map `properties`: <property id> -> map {
//!     name: str, kind: str, position: str, created_at: i64,
//!     options: map <option id> -> map { name: str, color: str, position: str, group?: str },
//!     config?: map  (relation: two_way: bool, paired?: property id;
//!                    rollup: relation: property id, target: "title" | property id, function: str;
//!                    formula: expression: str) }
//! page node metadata, key `props`: map <property id> -> plain value (see `value.rs`)
//! ```
//!
//! Every container is created with `ensure_mergeable_*`, whose ids derive from the parent and
//! the key: two devices that create the same logical container at the same time (the first
//! write of the built-in Tags property, the first value of a page) write into one container
//! and their edits merge, where regular containers would overwrite each other. Values are
//! plain per-key entries, so concurrent edits of different properties of one page merge, and
//! two edits of the same value resolve last-writer-wins. Property and option names are plain
//! fields: renaming and recolouring merge field by field.
//!
//! **Tags** is a multi-select with a fixed id ([`TAGS_PROPERTY_ID`]) that every space has. It
//! is listed first even before it is written, and is written on the first change that needs
//! it. **Positions** are fractional strings (`order.rs`); the schema lists properties by
//! `(position, id)`, Tags first whatever its own position.
//!
//! A value of a property the schema does not have, or an option id the property does not list,
//! stays in the document (a newer client may know it, or a concurrent edit may restore it) but
//! reads as nothing and has no index row; `check_invariants` reports it. Writes are committed
//! with the origin [`PROPERTIES_ORIGIN`], which structural undo excludes, so undo never changes
//! the schema or a value.
//!
//! **Relations (ADR-0027).** A relation value is a plain list of page ids per page, like `files`.
//! A two-way relation is two properties that name each other; a write of one side also writes the
//! other side of every page it adds or removes, in the same commit. Lists are last-writer-wins, so
//! two devices can leave the sides asymmetric; the *reader* therefore shows the union of what a
//! page stored and what the other pages store about it, so every replica reads the same,
//! symmetric view. A **rollup** is computed from the related pages at read time (`rollup.rs`).
//!
//! **People (M13 slice 2, ADR-0049).** A `person` value is a plain list of account ids per page
//! (at most 20, distinct, like a relation's list of pages); an empty list is no value. The model
//! checks the structure only: that the accounts are members of the space when the value is written is
//! the core's rule (`Core::person_value_check`), and readers keep an account that left (the UI shows
//! "Former member"). The derived rows hold one `account_id` row per listed account; the text index
//! holds nothing for the kind, because the model knows ids and the names are the core's to resolve
//! ([`person_text`] joins the names the core gives, [`person_filter_matches`] is the meaning of the
//! filters of the kind). A person property can be rolled up like any property that is not a list of
//! files or pages.
//!
//! **Formulas (ADR-0055, `formula_host.rs`).** A `formula` property stores one string, its
//! expression (`config.expression`, properties referenced by id), and no page stores a value: like a
//! rollup, the value is computed when the page's values are read, by the engine of `crate::formula`
//! over a [`FormulaHost`](crate::formula::FormulaHost) that reads this document. The expression is
//! parsed and checked against the schema when the property is created or its configuration set (a
//! refusal is `InvalidProperty` for `config`), and again on every read of the schema, which gives
//! the property its derived `result_type` and keeps the parsed expression for the page reads; a
//! formula that no longer checks has no result type and no value (`check_invariants` reports it).
//! `now()` is the time the owner set with [`StructureDoc::set_formula_env`].

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use kb_types::properties::{
    DEFAULT_STATUS_OPTIONS, MAX_NAME_CHARS, MAX_OPTIONS, MAX_PROPERTIES, ROLLUP_TITLE_TARGET,
    TAGS_NAME, TAGS_PROPERTY_ID,
};
use kb_types::{
    AccountId, AttachmentId, DeviceId, OptionColor, OptionId, PageId, PropertyConfig, PropertyDef,
    PropertyId, PropertyKind, PropertyOption, PropertyValue, RollupFunction, RollupTarget,
    RollupValue, StatusGroup,
};
use loro::{CommitOptions, Container, LoroMap, LoroValue, TreeID, ValueOrContainer};

use crate::meta::{self, engine};
use crate::{ModelError, StructureDoc};

mod formula_host;
/// Fractional positions; also used by the favourites list (`favorites.rs`).
pub(crate) mod order;
mod rollup;
mod value;

pub(crate) use formula_host::{FormulaPeople, FormulaTime};

pub use value::{person_filter_matches, person_text, valid_url};

/// Whether `text` is a real day as `YYYY-MM-DD` (the rule of a date mention, ADR-0016).
#[must_use]
pub fn valid_date(text: &str) -> bool {
    crate::page::date::valid_date(text)
}

/// Checks a value against its own rules (not against a property's options) and normalises it:
/// `Ok(None)` for the values that mean "no value" (an empty text, URL or list), the value
/// otherwise. Errors are [`ModelError::InvalidProperty`] for the field `value`.
pub fn normalise_value(value: &PropertyValue) -> Result<Option<PropertyValue>, ModelError> {
    Ok(value::check(value)?.cloned())
}

/// Commit origin of schema and value writes; the structure document's undo manager excludes
/// it (see the module documentation).
pub const PROPERTIES_ORIGIN: &str = "kb-model/properties";

/// Keys of the property and option maps and of a page's metadata.
mod key {
    pub(super) const NAME: &str = "name";
    pub(super) const KIND: &str = "kind";
    pub(super) const POSITION: &str = "position";
    pub(super) const CREATED_AT: &str = "created_at";
    pub(super) const OPTIONS: &str = "options";
    /// The configuration map of a relation or rollup.
    pub(super) const CONFIG: &str = "config";
    pub(super) const TWO_WAY: &str = "two_way";
    pub(super) const PAIRED: &str = "paired";
    pub(super) const RELATION: &str = "relation";
    pub(super) const TARGET: &str = "target";
    pub(super) const FUNCTION: &str = "function";
    /// A formula's stored expression (ADR-0055).
    pub(super) const EXPRESSION: &str = "expression";
    pub(super) const COLOR: &str = "color";
    pub(super) const GROUP: &str = "group";
    /// In a page node's metadata: the map of property values.
    pub(super) const PROPS: &str = "props";
}

/// The position stored with Tags (its listing position does not depend on it).
const TAGS_POSITION: &str = "0";

/// One row of the derived `page_properties` table, as the model computes it: a scalar value, one
/// selected option or one attached file of one page (`docs/data-model/sqlite-schema.md`, v10 and
/// v11).
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyRow {
    /// The property.
    pub property_id: PropertyId,
    /// Its kind.
    pub kind: PropertyKind,
    /// `text`, `url`, `email` and `phone` values; the name of a `place`.
    pub value_text: Option<String>,
    /// `number` values; the number of items done of a `checklist`.
    pub value_number: Option<f64>,
    /// `checkbox` values.
    pub value_bool: Option<bool>,
    /// `date` values: the day.
    pub value_date: Option<String>,
    /// `date` values: the time of day, when there is one.
    pub value_time: Option<String>,
    /// `select` and `status` values (one row), `multi_select` values (one row per option).
    pub option_id: Option<OptionId>,
    /// `files` values: one row per attachment.
    pub attachment_id: Option<AttachmentId>,
    /// `checklist` values: the number of items.
    pub value_total: Option<u32>,
    /// `relation` values: one row per related page that exists, in order.
    pub related_page_id: Option<PageId>,
    /// `person` values (M13 slice 2, ADR-0049): one row per listed account, in order.
    pub account_id: Option<AccountId>,
}

/// What the derived tables hold for one page: the `page_properties` rows and the text of the
/// full-text index (one entry per property that has some).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageIndexEntry {
    /// Rows of `page_properties`.
    pub rows: Vec<PropertyRow>,
    /// `(property id, text)` rows of the text index: option names, text-like values, the file
    /// names of `files` values, the titles of related pages and the item texts of checklists.
    pub texts: Vec<(PropertyId, String)>,
}

// ----- the decoded schema ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct OptionEntry {
    id: OptionId,
    name: String,
    color: OptionColor,
    group: Option<StatusGroup>,
    position: String,
}

#[derive(Debug, Clone, PartialEq)]
struct PropertyEntry {
    id: PropertyId,
    name: String,
    kind: PropertyKind,
    position: String,
    created_at: i64,
    /// In display order (position, then id).
    options: Vec<OptionEntry>,
    /// A relation's (always), rollup's or formula's (when readable) configuration; a formula's
    /// `result_type` is derived by [`Schema::derive_formulas`], never stored.
    config: Option<PropertyConfig>,
}

impl PropertyEntry {
    fn option(&self, id: OptionId) -> Option<&OptionEntry> {
        self.options.iter().find(|option| option.id == id)
    }

    fn def(&self) -> PropertyDef {
        PropertyDef {
            id: self.id,
            name: self.name.clone(),
            kind: self.kind,
            options: self.options.iter().map(OptionEntry::api).collect(),
            created_at: self.created_at,
            config: self.config.clone(),
        }
    }
}

impl OptionEntry {
    fn api(&self) -> PropertyOption {
        PropertyOption {
            id: self.id,
            name: self.name.clone(),
            color: self.color,
            group: self.group,
        }
    }
}

/// The schema of a space as read from the document: Tags first (a virtual entry when it is
/// not written yet), then the other properties by `(position, id)`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Schema {
    entries: Vec<PropertyEntry>,
    /// The pages that list each page through the paired side of a two-way relation, built on
    /// first use (see [`StructureDoc::incoming`]).
    incoming: OnceCell<Incoming>,
    /// The formulas that parse and check against this schema, by property id (ADR-0055).
    formulas: formula_host::ParsedFormulas,
    /// The time of the read ([`StructureDoc::set_formula_env`]): Unix ms and the offset in seconds.
    time: (i64, i32),
    /// The members of the space as the owner told them ([`StructureDoc::set_formula_people`]).
    people: formula_host::People,
}

/// `(forward relation property, page)` -> the pages whose stored back value lists `page`, oldest
/// first (creation time, then id).
type Incoming = HashMap<(PropertyId, PageId), Vec<PageId>>;

impl Schema {
    /// No properties at all (not even Tags): reads no values.
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    fn get(&self, id: PropertyId) -> Option<&PropertyEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The kind of a property of the schema (`None` for an id it has not got).
    pub(crate) fn kind_of(&self, id: PropertyId) -> Option<PropertyKind> {
        self.get(id).map(|entry| entry.kind)
    }

    /// The ids of the properties, in schema order (Tags first).
    pub(crate) fn ids(&self) -> impl Iterator<Item = PropertyId> + '_ {
        self.entries.iter().map(|entry| entry.id)
    }

    fn defs(&self) -> Vec<PropertyDef> {
        self.entries.iter().map(PropertyEntry::def).collect()
    }

    /// The other properties (Tags left out), in order.
    fn others(&self) -> impl Iterator<Item = &PropertyEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.id != TAGS_PROPERTY_ID)
    }

    /// The paired property of a two-way relation (after `read_schema` checked that the pair
    /// points both ways, this is just the configured id).
    fn paired_of(&self, id: PropertyId) -> Option<&PropertyEntry> {
        match self.get(id)?.config {
            Some(PropertyConfig::Relation {
                two_way: true,
                paired_property_id: Some(paired),
            }) => self.get(paired),
            _ => None,
        }
    }

    /// Whether any relation property has a paired one.
    fn has_pairs(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry.config,
                Some(PropertyConfig::Relation { two_way: true, .. })
            )
        })
    }

    /// Makes every relation whose partner is missing, is no relation or does not point back a
    /// one-way relation (a concurrent delete can leave such a pair).
    fn settle_pairs(&mut self) {
        let settled: Vec<(PropertyId, bool)> = self
            .entries
            .iter()
            .filter_map(|entry| match entry.config {
                Some(PropertyConfig::Relation {
                    two_way: true,
                    paired_property_id: Some(paired),
                }) => {
                    let points_back = self.get(paired).is_some_and(|other| {
                        other.kind == PropertyKind::Relation
                            && other.id != entry.id
                            && matches!(
                                other.config,
                                Some(PropertyConfig::Relation {
                                    two_way: true,
                                    paired_property_id: Some(back),
                                }) if back == entry.id
                            )
                    });
                    Some((entry.id, points_back))
                }
                _ => None,
            })
            .collect();
        for (id, points_back) in settled {
            if points_back {
                continue;
            }
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                entry.config = Some(one_way());
            }
        }
    }
}

fn one_way() -> PropertyConfig {
    PropertyConfig::Relation {
        two_way: false,
        paired_property_id: None,
    }
}

fn virtual_tags() -> PropertyEntry {
    PropertyEntry {
        id: TAGS_PROPERTY_ID,
        name: TAGS_NAME.to_owned(),
        kind: PropertyKind::MultiSelect,
        position: TAGS_POSITION.to_owned(),
        created_at: 0,
        options: Vec::new(),
        config: None,
    }
}

/// A child map under `key`: `None` when absent or not a map.
fn child_map(parent: &LoroMap, key: &str) -> Option<LoroMap> {
    match parent.get(key)? {
        ValueOrContainer::Container(Container::Map(map)) => Some(map),
        _ => None,
    }
}

/// The map under `key` for writing: the existing one (mergeable, or regular if a foreign writer
/// made it so), or a new mergeable one.
fn ensure_child_map(parent: &LoroMap, key: &str) -> Result<LoroMap, ModelError> {
    match parent.get(key) {
        Some(ValueOrContainer::Container(Container::Map(map))) => Ok(map),
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => {
            parent.ensure_mergeable_map(key).map_err(engine)
        }
        Some(_) => Err(ModelError::Shape("a property field is not a map".into())),
    }
}

fn read_option(id: OptionId, map: &LoroMap, kind: PropertyKind) -> OptionEntry {
    OptionEntry {
        id,
        name: meta::string(map, key::NAME).unwrap_or_default(),
        color: meta::string(map, key::COLOR)
            .and_then(|name| OptionColor::parse(&name))
            .unwrap_or(OptionColor::Gray),
        group: (kind == PropertyKind::Status).then(|| {
            meta::string(map, key::GROUP)
                .and_then(|name| StatusGroup::parse(&name))
                .unwrap_or(StatusGroup::Todo)
        }),
        position: meta::string(map, key::POSITION).unwrap_or_default(),
    }
}

fn read_options(map: &LoroMap, kind: PropertyKind) -> Vec<OptionEntry> {
    if !kind.has_options() {
        return Vec::new();
    }
    let Some(options) = child_map(map, key::OPTIONS) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out: Vec<OptionEntry> = options
        .keys()
        .filter_map(|raw| {
            let id = raw.parse::<OptionId>().ok()?;
            let option = child_map(&options, &raw)?;
            seen.insert(id).then(|| read_option(id, &option, kind))
        })
        .collect();
    out.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
    out
}

fn read_property(id: PropertyId, map: &LoroMap) -> Option<PropertyEntry> {
    let kind = PropertyKind::parse(&meta::string(map, key::KIND)?)?;
    Some(PropertyEntry {
        id,
        name: meta::string(map, key::NAME).unwrap_or_default(),
        kind,
        position: meta::string(map, key::POSITION).unwrap_or_default(),
        created_at: meta::int(map, key::CREATED_AT).unwrap_or(0),
        options: read_options(map, kind),
        config: read_config(kind, map),
    })
}

/// The configuration stored with a property: a relation always has one (one-way when nothing or
/// something unreadable is stored), a rollup only when it reads completely, a formula when it has
/// an expression (its `result_type` is derived later, from the whole schema).
fn read_config(kind: PropertyKind, map: &LoroMap) -> Option<PropertyConfig> {
    let config = child_map(map, key::CONFIG);
    match kind {
        PropertyKind::Relation => {
            let paired = config
                .as_ref()
                .and_then(|config| meta::string(config, key::PAIRED))
                .and_then(|id| id.parse::<PropertyId>().ok());
            let two_way = config
                .as_ref()
                .and_then(|config| match meta::value(config, key::TWO_WAY) {
                    Some(LoroValue::Bool(on)) => Some(on),
                    _ => None,
                })
                .unwrap_or(false);
            Some(match paired {
                Some(paired) if two_way => PropertyConfig::Relation {
                    two_way: true,
                    paired_property_id: Some(paired),
                },
                _ => one_way(),
            })
        }
        PropertyKind::Rollup => {
            let config = config?;
            let relation = meta::string(&config, key::RELATION)?
                .parse::<PropertyId>()
                .ok()?;
            let target = meta::string(&config, key::TARGET)?;
            let target = if target == ROLLUP_TITLE_TARGET {
                RollupTarget::Title
            } else {
                RollupTarget::Property(target.parse::<PropertyId>().ok()?)
            };
            let function = RollupFunction::parse(&meta::string(&config, key::FUNCTION)?)?;
            Some(PropertyConfig::Rollup {
                relation_property_id: relation,
                target,
                function,
            })
        }
        PropertyKind::Formula => Some(PropertyConfig::Formula {
            expression: meta::string(&config?, key::EXPRESSION)?,
            result_type: None,
        }),
        _ => None,
    }
}

/// Writes a configuration into the property's `config` map (a rollup's three fields, a relation's
/// two).
fn write_config(map: &LoroMap, config: &PropertyConfig) -> Result<(), ModelError> {
    let target = ensure_child_map(map, key::CONFIG)?;
    match config {
        PropertyConfig::Relation {
            two_way,
            paired_property_id,
        } => {
            if meta::value(&target, key::TWO_WAY) != Some(LoroValue::Bool(*two_way)) {
                target.insert(key::TWO_WAY, *two_way).map_err(engine)?;
            }
            meta::set_opt_string(
                &target,
                key::PAIRED,
                paired_property_id.map(|id| id.to_string()).as_deref(),
            )
        }
        PropertyConfig::Rollup {
            relation_property_id,
            target: what,
            function,
        } => {
            meta::set_string(&target, key::RELATION, &relation_property_id.to_string())?;
            meta::set_string(
                &target,
                key::TARGET,
                &match what {
                    RollupTarget::Title => ROLLUP_TITLE_TARGET.to_owned(),
                    RollupTarget::Property(id) => id.to_string(),
                },
            )?;
            meta::set_string(&target, key::FUNCTION, function.as_str())
        }
        // `result_type` is derived on read and never stored.
        PropertyConfig::Formula { expression, .. } => {
            meta::set_string(&target, key::EXPRESSION, expression)
        }
    }
}

/// A property or option name: not blank, at most [`MAX_NAME_CHARS`] characters, no control
/// characters. (The core trims first; the model stores what it is given.)
fn check_name(name: &str) -> Result<(), ModelError> {
    if name.trim().is_empty() {
        return Err(ModelError::InvalidProperty {
            field: "name",
            message: "must not be empty".into(),
        });
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ModelError::InvalidProperty {
            field: "name",
            message: format!("must be at most {MAX_NAME_CHARS} characters"),
        });
    }
    if name.chars().any(char::is_control) {
        return Err(ModelError::InvalidProperty {
            field: "name",
            message: "must not contain control characters".into(),
        });
    }
    Ok(())
}

fn refused(field: &'static str, message: &str) -> ModelError {
    ModelError::InvalidProperty {
        field,
        message: message.to_owned(),
    }
}

/// A value write that has been checked and decided but not written (see
/// [`StructureDoc::set_page_property`] and [`StructureDoc::set_pages_property`]).
enum PlannedWrite {
    /// The page already holds the value: nothing to write.
    Nothing,
    /// A value other than a relation.
    Plain {
        page_id: PageId,
        node: TreeID,
        property_id: PropertyId,
        /// The value as stored, `None` to remove it.
        stored: Option<LoroValue>,
        /// The stored value differs from the wanted one.
        changed: bool,
        /// The page holds a template placeholder for the property, which the write clears.
        placeholder: bool,
    },
    /// A relation, with the paired values it changes.
    Relation(RelationWrite),
}

/// A relation write: the page's own list and, for a two-way relation, the paired list of every
/// page it adds or removes.
struct RelationWrite {
    page_id: PageId,
    node: TreeID,
    property_id: PropertyId,
    stored: Option<LoroValue>,
    own_change: bool,
    partner: Option<PropertyId>,
    others: Vec<(PageId, TreeID, Option<LoroValue>)>,
}

/// The rows of one stored value.
fn rows_of(property_id: PropertyId, value: &PropertyValue) -> Vec<PropertyRow> {
    let row = |option_id: Option<OptionId>| PropertyRow {
        property_id,
        kind: value.kind(),
        value_text: None,
        value_number: None,
        value_bool: None,
        value_date: None,
        value_time: None,
        option_id,
        attachment_id: None,
        value_total: None,
        related_page_id: None,
        account_id: None,
    };
    match value {
        PropertyValue::Text(text)
        | PropertyValue::Url(text)
        | PropertyValue::Email(text)
        | PropertyValue::Phone(text) => vec![PropertyRow {
            value_text: Some(text.clone()),
            ..row(None)
        }],
        PropertyValue::Number(number) => vec![PropertyRow {
            value_number: Some(*number),
            ..row(None)
        }],
        PropertyValue::Checkbox(on) => vec![PropertyRow {
            value_bool: Some(*on),
            ..row(None)
        }],
        PropertyValue::Date(date) => vec![PropertyRow {
            value_date: Some(date.date.clone()),
            value_time: date.time.clone(),
            ..row(None)
        }],
        PropertyValue::Select(id) | PropertyValue::Status(id) => vec![row(Some(*id))],
        PropertyValue::MultiSelect(ids) => ids.iter().map(|id| row(Some(*id))).collect(),
        PropertyValue::Files(ids) => ids
            .iter()
            .map(|id| PropertyRow {
                attachment_id: Some(*id),
                ..row(None)
            })
            .collect(),
        PropertyValue::Place(place) => vec![PropertyRow {
            value_text: Some(place.name.clone()),
            ..row(None)
        }],
        PropertyValue::Relation(pages) => pages
            .iter()
            .map(|page| PropertyRow {
                related_page_id: Some(*page),
                ..row(None)
            })
            .collect(),
        PropertyValue::Person(accounts) => accounts
            .iter()
            .map(|account| PropertyRow {
                account_id: Some(*account),
                ..row(None)
            })
            .collect(),
        // Computed values have no rows: filters read them from the page itself.
        PropertyValue::Rollup(_)
        | PropertyValue::Formula(_)
        | PropertyValue::CreatedTime(_)
        | PropertyValue::LastEditedTime(_)
        | PropertyValue::CreatedBy(_)
        | PropertyValue::LastEditedBy(_) => Vec::new(),
        PropertyValue::Checklist(items) => {
            let done = items.iter().filter(|item| item.done).count();
            vec![PropertyRow {
                // Both counts are at most 200, so they are exact in an `f64`.
                value_number: Some(u32::try_from(done).map_or(f64::MAX, f64::from)),
                value_total: Some(u32::try_from(items.len()).unwrap_or(u32::MAX)),
                ..row(None)
            }]
        }
    }
}

// ----- schema and values: reads ---------------------------------------------------------------

impl StructureDoc {
    /// The property schema: Tags first (even before it is written), then the other properties
    /// by position. Options are in display order.
    #[must_use]
    pub fn property_schema(&self) -> Vec<PropertyDef> {
        self.read_schema().defs()
    }

    /// One property of the schema (`None` when it has none with this id).
    #[must_use]
    pub fn property(&self, property_id: PropertyId) -> Option<PropertyDef> {
        self.read_schema().get(property_id).map(PropertyEntry::def)
    }

    /// The values of a page (live or trashed), in schema order. Values of properties the schema
    /// does not have, options it does not list and values of the wrong shape are left out.
    pub fn page_properties(
        &self,
        page_id: PageId,
    ) -> Result<Vec<(PropertyId, PropertyValue)>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        Ok(self.values_of(node, &self.read_schema()))
    }

    /// The pages that list `page_id` in the *stored* value of a relation property (the property
    /// is the stored side: for a two-way pair, the pages that relate to the page through the
    /// paired property are found with the other id). Existing pages, live or trashed, oldest
    /// first; the answer of the index query "what links here".
    #[must_use]
    pub fn pages_listing(&self, property_id: PropertyId, page_id: PageId) -> Vec<PageId> {
        let schema = self.read_schema();
        let mut found: Vec<PageId> = self
            .index
            .pages
            .keys()
            .copied()
            .filter(|other| {
                self.stored_relation(&schema, property_id, *other)
                    .contains(&page_id)
            })
            .collect();
        self.sort_oldest_first(&mut found);
        found
    }

    /// The derived rows of the given pages (those that exist, live or trashed), in the order
    /// given; a page without values has an empty entry, so the owner can replace its old rows.
    #[must_use]
    pub fn property_index(&self, pages: &[PageId]) -> Vec<(PageId, PageIndexEntry)> {
        let schema = self.read_schema();
        pages
            .iter()
            .filter_map(|page| {
                let node = self.index.node(*page)?;
                Some((*page, self.index_entry(node, &schema)))
            })
            .collect()
    }

    /// The derived rows of every page that has values, in page id order.
    #[must_use]
    pub fn property_index_all(&self) -> Vec<(PageId, PageIndexEntry)> {
        let schema = self.read_schema();
        let mut pages: Vec<(PageId, TreeID)> = self
            .index
            .pages
            .iter()
            .map(|(page, node)| (*page, *node))
            .collect();
        pages.sort_unstable_by_key(|(page, _)| *page);
        pages
            .into_iter()
            .filter(|(_, node)| self.props_map(*node).is_some())
            .map(|(page, node)| (page, self.index_entry(node, &schema)))
            .filter(|(_, entry)| !entry.rows.is_empty())
            .collect()
    }

    pub(crate) fn read_schema(&self) -> Schema {
        let mut seen = HashSet::new();
        let mut entries: Vec<PropertyEntry> = self
            .properties
            .keys()
            .filter_map(|raw| {
                let id = raw.parse::<PropertyId>().ok()?;
                let map = child_map(&self.properties, &raw)?;
                let entry = read_property(id, &map)?;
                seen.insert(id).then_some(entry)
            })
            .collect();
        if !seen.contains(&TAGS_PROPERTY_ID) {
            entries.push(virtual_tags());
        }
        entries.sort_by(|a, b| {
            (b.id == TAGS_PROPERTY_ID)
                .cmp(&(a.id == TAGS_PROPERTY_ID))
                .then_with(|| a.position.cmp(&b.position))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut schema = Schema {
            entries,
            incoming: OnceCell::new(),
            formulas: HashMap::new(),
            time: self.formula_time.get(),
            people: self.formula_directory(),
        };
        schema.settle_pairs();
        schema.derive_formulas();
        schema
    }

    /// The pages that list each page through the paired side of a two-way relation (built once
    /// per schema read, on demand).
    fn incoming<'a>(&self, schema: &'a Schema) -> &'a Incoming {
        schema.incoming.get_or_init(|| {
            let mut map: Incoming = HashMap::new();
            if !schema.has_pairs() {
                return map;
            }
            let mut pages: Vec<PageId> = self.index.pages.keys().copied().collect();
            pages.sort_unstable();
            for entry in &schema.entries {
                let Some(forward) = schema.paired_of(entry.id) else {
                    continue;
                };
                for page in &pages {
                    for listed in self.stored_relation(schema, entry.id, *page) {
                        map.entry((forward.id, listed)).or_default().push(*page);
                    }
                }
            }
            for list in map.values_mut() {
                self.sort_oldest_first(list);
                list.dedup();
            }
            map
        })
    }

    /// Orders pages by creation time, then id (the same on every replica).
    fn sort_oldest_first(&self, pages: &mut [PageId]) {
        pages.sort_by_key(|page| {
            let created = self
                .index
                .node(*page)
                .and_then(|node| self.meta_of(node))
                .and_then(|meta_map| meta::int(&meta_map, meta::CREATED_AT))
                .unwrap_or(0);
            (created, *page)
        });
    }

    /// What `page` stores in relation property `property_id`: the pages that exist, in order.
    fn stored_relation(
        &self,
        schema: &Schema,
        property_id: PropertyId,
        page: PageId,
    ) -> Vec<PageId> {
        let Some(entry) = schema.get(property_id) else {
            return Vec::new();
        };
        let Some(node) = self.index.node(page) else {
            return Vec::new();
        };
        match self.stored_value(entry, node) {
            Some(PropertyValue::Relation(pages)) => pages
                .into_iter()
                .filter(|listed| self.index.node(*listed).is_some())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The stored value of a (non-computed) property of a page node, as its kind.
    fn stored_value(&self, entry: &PropertyEntry, node: TreeID) -> Option<PropertyValue> {
        let props = self.props_map(node)?;
        let stored = meta::value(&props, &entry.id.to_string())?;
        read_value(entry, &stored)
    }

    /// The relation value a page shows: what it stores, plus (for a two-way relation) the pages
    /// that store it on the paired side, existing pages only.
    fn effective_relation(
        &self,
        schema: &Schema,
        entry: &PropertyEntry,
        node: TreeID,
    ) -> Vec<PageId> {
        let Some(page) = self.index.page(node) else {
            return Vec::new();
        };
        let mut pages = self.stored_relation(schema, entry.id, page);
        if let Some(more) = self.incoming(schema).get(&(entry.id, page)) {
            for other in more {
                if !pages.contains(other) {
                    pages.push(*other);
                }
            }
        }
        pages
    }

    /// A page node's map of property values.
    pub(crate) fn props_map(&self, node: TreeID) -> Option<LoroMap> {
        let meta = self.tree.get_meta(node).ok()?;
        child_map(&meta, key::PROPS)
    }

    /// The readable values of a page node in schema order (see [`Self::page_properties`]).
    pub(crate) fn values_of(
        &self,
        node: TreeID,
        schema: &Schema,
    ) -> Vec<(PropertyId, PropertyValue)> {
        let meta_map = self.meta_of(node);
        schema
            .entries
            .iter()
            .filter_map(|entry| {
                if entry.kind == PropertyKind::Rollup {
                    return Some((entry.id, self.rollup_value(schema, entry, node)?));
                }
                if entry.kind == PropertyKind::Formula {
                    return Some((entry.id, self.formula_value(schema, entry, node)?));
                }
                if entry.kind.is_computed() {
                    let value = computed_value(entry.kind, meta_map.as_ref()?)?;
                    return Some((entry.id, value));
                }
                if entry.kind == PropertyKind::Relation {
                    let pages = self.effective_relation(schema, entry, node);
                    return (!pages.is_empty())
                        .then_some((entry.id, PropertyValue::Relation(pages)));
                }
                let value = self.stored_value(entry, node)?;
                Some((entry.id, value))
            })
            .collect()
    }

    /// The stored values of a page in schema order, for the derived rows: what the page itself
    /// holds (a relation without the pages that only list it from the other side; no rollups).
    fn stored_values_of(&self, node: TreeID, schema: &Schema) -> Vec<(PropertyId, PropertyValue)> {
        schema
            .entries
            .iter()
            .filter(|entry| !entry.kind.is_computed())
            .filter_map(|entry| {
                let value = self.stored_value(entry, node)?;
                match value {
                    PropertyValue::Relation(pages) => {
                        let pages: Vec<PageId> = pages
                            .into_iter()
                            .filter(|listed| self.index.node(*listed).is_some())
                            .collect();
                        (!pages.is_empty()).then_some((entry.id, PropertyValue::Relation(pages)))
                    }
                    other => Some((entry.id, other)),
                }
            })
            .collect()
    }

    /// The value of a rollup for a page: `None` when its configuration is not valid (any more) or
    /// the function has no result. Related pages in the trash are left out.
    fn rollup_value(
        &self,
        schema: &Schema,
        entry: &PropertyEntry,
        node: TreeID,
    ) -> Option<PropertyValue> {
        let config = entry.config.as_ref()?;
        check_rollup_config(schema, config).ok()?;
        let PropertyConfig::Rollup {
            relation_property_id,
            target,
            function,
        } = config
        else {
            return None;
        };
        let relation = schema.get(*relation_property_id)?;
        let mut values: Vec<Option<PropertyValue>> = Vec::new();
        for related in self.effective_relation(schema, relation, node) {
            let Some(related_node) = self.index.node(related) else {
                continue;
            };
            if self
                .placement(related_node)
                .is_none_or(|placement| placement.trashed_top.is_some())
            {
                continue;
            }
            values.push(match target {
                RollupTarget::Title => self
                    .meta_of(related_node)
                    .and_then(|meta_map| meta::string(&meta_map, meta::TITLE))
                    .filter(|title| !title.trim().is_empty())
                    .map(PropertyValue::Text),
                RollupTarget::Property(id) => {
                    let target_entry = schema.get(*id)?;
                    self.stored_value(target_entry, related_node)
                }
            });
        }
        let result = rollup::evaluate(*function, &values)?;
        Some(PropertyValue::Rollup(RollupValue {
            function: *function,
            result,
        }))
    }

    /// The value of a formula for a page: `None` when the formula does not parse or check against
    /// the schema (then no page has a value) or when its result is empty. Computed on every read
    /// with the time of the schema ([`StructureDoc::set_formula_env`]); nothing is stored or indexed.
    fn formula_value(
        &self,
        schema: &Schema,
        entry: &PropertyEntry,
        node: TreeID,
    ) -> Option<PropertyValue> {
        let parsed = schema.formulas.get(&entry.id)?;
        let page = self.index.page(node)?;
        let host = formula_host::ModelHost { doc: self, schema };
        let result = crate::formula::evaluate(parsed, page, &host, &schema.env())?;
        Some(PropertyValue::Formula(result))
    }

    fn index_entry(&self, node: TreeID, schema: &Schema) -> PageIndexEntry {
        let mut entry = PageIndexEntry::default();
        for (property_id, value) in self.stored_values_of(node, schema) {
            if value.kind().is_computed() {
                continue;
            }
            entry.rows.extend(rows_of(property_id, &value));
            let Some(property) = schema.get(property_id) else {
                continue;
            };
            let name_of = |option: OptionId| property.option(option).map(|o| o.name.clone());
            // A file's name comes from its manifest, which lives in the same document.
            let file_name =
                |file: AttachmentId| self.attachment(file).map(|manifest| manifest.name);
            let page_title = |page: PageId| {
                let related = self.index.node(page)?;
                meta::string(&self.meta_of(related)?, meta::TITLE)
            };
            if let Some(text) = value::search_text(&value, &name_of, &file_name, &page_title) {
                entry.texts.push((property_id, text));
            }
        }
        entry
    }
}

/// The value of a computed property, read from the page's own metadata: the creation time and
/// last edit time always (`updated_at` falls back to `created_at`), the devices when the page
/// recorded them (pages made or edited before the amendment of 2026-10-08 did not).
fn computed_value(kind: PropertyKind, meta_map: &LoroMap) -> Option<PropertyValue> {
    let device = |key: &str| meta::string(meta_map, key)?.parse::<DeviceId>().ok();
    match kind {
        PropertyKind::CreatedTime => {
            meta::int(meta_map, meta::CREATED_AT).map(PropertyValue::CreatedTime)
        }
        PropertyKind::LastEditedTime => meta::int(meta_map, meta::UPDATED_AT)
            .or_else(|| meta::int(meta_map, meta::CREATED_AT))
            .map(PropertyValue::LastEditedTime),
        PropertyKind::CreatedBy => device(meta::CREATED_BY).map(PropertyValue::CreatedBy),
        PropertyKind::LastEditedBy => device(meta::EDITED_BY).map(PropertyValue::LastEditedBy),
        _ => None,
    }
}

/// Checks the configuration of a rollup against the schema; returns the kind of the target
/// (`None` for the title). Errors are `InvalidProperty` for the field `config`.
fn check_rollup_config(
    schema: &Schema,
    config: &PropertyConfig,
) -> Result<Option<PropertyKind>, ModelError> {
    let PropertyConfig::Rollup {
        relation_property_id,
        target,
        function,
    } = config
    else {
        return Err(refused(
            "config",
            "the configuration does not match the kind",
        ));
    };
    if schema
        .get(*relation_property_id)
        .is_none_or(|entry| entry.kind != PropertyKind::Relation)
    {
        return Err(refused("config", "the rollup needs a relation property"));
    }
    let kind = match target {
        RollupTarget::Title => None,
        RollupTarget::Property(id) => {
            let entry = schema
                .get(*id)
                .ok_or_else(|| refused("config", "the rollup target does not exist"))?;
            if !entry.kind.is_rollup_target() {
                return Err(refused(
                    "config",
                    "this kind of property cannot be rolled up",
                ));
            }
            Some(entry.kind)
        }
    };
    if !function.accepts(kind) {
        return Err(refused(
            "config",
            "this function does not apply to the target",
        ));
    }
    Ok(kind)
}

/// A stored value read as the property's kind; an option the property does not list is dropped
/// (a `select` or `status` value that names one reads as nothing).
fn read_value(entry: &PropertyEntry, stored: &LoroValue) -> Option<PropertyValue> {
    match value::decode(entry.kind, stored)? {
        PropertyValue::Select(id) => entry.option(id).map(|_| PropertyValue::Select(id)),
        PropertyValue::Status(id) => entry.option(id).map(|_| PropertyValue::Status(id)),
        PropertyValue::MultiSelect(ids) => {
            let known: Vec<OptionId> = ids
                .into_iter()
                .filter(|id| entry.option(*id).is_some())
                .collect();
            (!known.is_empty()).then_some(PropertyValue::MultiSelect(known))
        }
        other => Some(other),
    }
}

// ----- schema: writes -------------------------------------------------------------------------

impl StructureDoc {
    /// Creates a property of `kind`, appended after the others. A `status` property starts with
    /// the three default options, whose ids come from `new_option`. Returns the new entry.
    ///
    /// Errors: [`ModelError::InvalidProperty`] for a bad name, a used id, or a full schema
    /// (at most [`MAX_PROPERTIES`] properties, Tags included).
    pub fn create_property(
        &mut self,
        property_id: PropertyId,
        name: &str,
        kind: PropertyKind,
        new_option: &mut dyn FnMut() -> OptionId,
        now_ms: i64,
    ) -> Result<PropertyDef, ModelError> {
        self.create_property_with(property_id, name, kind, None, new_option, now_ms)
    }

    /// Creates a property like [`Self::create_property`], with the configuration of a `relation`
    /// (one-way; a two-way pair is made by [`Self::create_relation_pair`]), a `rollup` or a
    /// `formula` (both required, checked against the schema). Any other kind takes none.
    pub fn create_property_with(
        &mut self,
        property_id: PropertyId,
        name: &str,
        kind: PropertyKind,
        config: Option<&PropertyConfig>,
        new_option: &mut dyn FnMut() -> OptionId,
        now_ms: i64,
    ) -> Result<PropertyDef, ModelError> {
        self.create_property_checked(property_id, name, kind, config, true, new_option, now_ms)
    }

    /// Creates a formula property whose expression is stored as given, without checking it: for the
    /// import of a file whose expression does not resolve in this space (a name that matches no
    /// property). It has no result type and no value until a configuration that checks is set
    /// ([`Self::set_property_config`]), and `check_invariants` reports it. The name, the id and the
    /// size of the schema are checked as for any property.
    pub fn create_formula_unchecked(
        &mut self,
        property_id: PropertyId,
        name: &str,
        expression: &str,
        now_ms: i64,
    ) -> Result<PropertyDef, ModelError> {
        let config = PropertyConfig::Formula {
            expression: expression.to_owned(),
            result_type: None,
        };
        self.create_property_checked(
            property_id,
            name,
            PropertyKind::Formula,
            Some(&config),
            false,
            &mut || OptionId::from_bytes([0; 16]),
            now_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_property_checked(
        &mut self,
        property_id: PropertyId,
        name: &str,
        kind: PropertyKind,
        config: Option<&PropertyConfig>,
        check_formula: bool,
        new_option: &mut dyn FnMut() -> OptionId,
        now_ms: i64,
    ) -> Result<PropertyDef, ModelError> {
        check_name(name)?;
        let schema = self.read_schema();
        let config = match (kind, config) {
            (PropertyKind::Rollup, None) => {
                return Err(refused("config", "a rollup needs a configuration"));
            }
            (PropertyKind::Rollup, Some(config)) => {
                check_rollup_config(&schema, config)?;
                Some(config.clone())
            }
            (PropertyKind::Formula, None) => {
                return Err(refused("config", "a formula needs a configuration"));
            }
            (PropertyKind::Formula, Some(config)) => {
                // The new property is not in the schema yet; its id is `self_id`, so a reference
                // to itself through other formulas is a cycle.
                if check_formula {
                    formula_host::check_formula_config(&schema, Some(property_id), config)?;
                } else if !matches!(config, PropertyConfig::Formula { .. }) {
                    return Err(refused(
                        "config",
                        "the configuration does not match the kind",
                    ));
                }
                Some(config.clone())
            }
            (PropertyKind::Relation, None) => Some(one_way()),
            (
                PropertyKind::Relation,
                Some(PropertyConfig::Relation {
                    two_way: false,
                    paired_property_id: None,
                }),
            ) => Some(one_way()),
            (PropertyKind::Relation, Some(PropertyConfig::Relation { .. })) => {
                return Err(refused(
                    "config",
                    "a new two-way relation makes its own paired property",
                ));
            }
            (
                PropertyKind::Relation,
                Some(PropertyConfig::Rollup { .. } | PropertyConfig::Formula { .. }),
            ) => {
                return Err(refused(
                    "config",
                    "the configuration does not match the kind",
                ));
            }
            (_, None) => None,
            (_, Some(_)) => {
                return Err(refused(
                    "config",
                    "only a relation, a rollup or a formula takes a configuration",
                ));
            }
        };
        if schema.entries.len() >= MAX_PROPERTIES {
            return Err(ModelError::InvalidProperty {
                field: "spaceId",
                message: format!("a space holds at most {MAX_PROPERTIES} properties"),
            });
        }
        if property_id == TAGS_PROPERTY_ID
            || self.properties.get(&property_id.to_string()).is_some()
        {
            return Err(refused("propertyId", "a property with this id exists"));
        }
        let position =
            order::key_between(schema.others().last().map(|e| e.position.as_str()), None)
                .ok_or_else(|| ModelError::Shape("property positions are damaged".into()))?;
        let default_ids: Vec<OptionId> = if kind == PropertyKind::Status {
            DEFAULT_STATUS_OPTIONS
                .iter()
                .map(|_| new_option())
                .collect()
        } else {
            Vec::new()
        };

        self.close_group();
        self.props_write(|| {
            let map = ensure_child_map(&self.properties, &property_id.to_string())?;
            meta::set_string(&map, key::NAME, name)?;
            meta::set_string(&map, key::KIND, kind.as_str())?;
            meta::set_string(&map, key::POSITION, &position)?;
            meta::set_int(&map, key::CREATED_AT, now_ms)?;
            if let Some(config) = &config {
                write_config(&map, config)?;
            }
            if kind.has_options() {
                let options = ensure_child_map(&map, key::OPTIONS)?;
                let positions = order::sequence(default_ids.len());
                for ((id, (option_name, group, color)), position) in default_ids
                    .iter()
                    .zip(DEFAULT_STATUS_OPTIONS)
                    .zip(&positions)
                {
                    write_option(&options, *id, option_name, color, Some(group), position)?;
                }
            }
            Ok(())
        })?;
        self.property(property_id)
            .ok_or_else(|| ModelError::Shape("a new property is unreadable".into()))
    }

    /// Creates the two properties of a two-way relation in one commit: `forward_id` named `name`
    /// and `back_id` named `back_name`, each paired with the other, appended in that order.
    /// Returns the forward property.
    ///
    /// Errors: [`ModelError::InvalidProperty`] for a bad name, a used id, or a schema without
    /// room for both (at most [`MAX_PROPERTIES`] properties, Tags included).
    pub fn create_relation_pair(
        &mut self,
        forward_id: PropertyId,
        back_id: PropertyId,
        name: &str,
        back_name: &str,
        now_ms: i64,
    ) -> Result<PropertyDef, ModelError> {
        check_name(name)?;
        check_name(back_name)?;
        let schema = self.read_schema();
        if schema.entries.len() + 2 > MAX_PROPERTIES {
            return Err(ModelError::InvalidProperty {
                field: "spaceId",
                message: format!("a space holds at most {MAX_PROPERTIES} properties"),
            });
        }
        for id in [forward_id, back_id] {
            if id == TAGS_PROPERTY_ID || self.properties.get(&id.to_string()).is_some() {
                return Err(refused("propertyId", "a property with this id exists"));
            }
        }
        if forward_id == back_id {
            return Err(refused("propertyId", "a property with this id exists"));
        }
        let last = schema.others().last().map(|e| e.position.as_str());
        let forward_position = order::key_between(last, None)
            .ok_or_else(|| ModelError::Shape("property positions are damaged".into()))?;
        let back_position = order::key_between(Some(&forward_position), None)
            .ok_or_else(|| ModelError::Shape("property positions are damaged".into()))?;

        self.close_group();
        self.props_write(|| {
            for (id, title, position, paired) in [
                (forward_id, name, &forward_position, back_id),
                (back_id, back_name, &back_position, forward_id),
            ] {
                let map = ensure_child_map(&self.properties, &id.to_string())?;
                meta::set_string(&map, key::NAME, title)?;
                meta::set_string(&map, key::KIND, PropertyKind::Relation.as_str())?;
                meta::set_string(&map, key::POSITION, position)?;
                meta::set_int(&map, key::CREATED_AT, now_ms)?;
                write_config(
                    &map,
                    &PropertyConfig::Relation {
                        two_way: true,
                        paired_property_id: Some(paired),
                    },
                )?;
            }
            Ok(())
        })?;
        self.property(forward_id)
            .ok_or_else(|| ModelError::Shape("a new property is unreadable".into()))
    }

    /// Changes the configuration of a rollup or a formula (checked against the schema; a formula's
    /// expression must parse, check and not depend on itself, `result_type` is ignored). A
    /// relation's configuration is fixed at creation.
    pub fn set_property_config(
        &mut self,
        property_id: PropertyId,
        config: &PropertyConfig,
    ) -> Result<PropertyDef, ModelError> {
        let schema = self.read_schema();
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        match entry.kind {
            PropertyKind::Rollup => {
                check_rollup_config(&schema, config)?;
                if entry.config.as_ref() != Some(config) {
                    self.close_group();
                    self.props_write(|| {
                        let map = self.existing_property_map(property_id)?;
                        write_config(&map, config)
                    })?;
                }
            }
            PropertyKind::Formula => {
                formula_host::check_formula_config(&schema, Some(property_id), config)?;
                let same = match (&entry.config, config) {
                    (
                        Some(PropertyConfig::Formula {
                            expression: old, ..
                        }),
                        PropertyConfig::Formula {
                            expression: new, ..
                        },
                    ) => old == new,
                    _ => false,
                };
                if !same {
                    self.close_group();
                    self.props_write(|| {
                        let map = self.existing_property_map(property_id)?;
                        write_config(&map, config)
                    })?;
                }
            }
            PropertyKind::Relation => {
                return Err(refused(
                    "config",
                    "the configuration of a relation cannot be changed",
                ));
            }
            _ => {
                return Err(refused(
                    "config",
                    "only a relation, a rollup or a formula takes a configuration",
                ));
            }
        }
        self.property(property_id)
            .ok_or(ModelError::PropertyNotFound)
    }

    /// Renames a property (Tags included).
    pub fn rename_property(
        &mut self,
        property_id: PropertyId,
        name: &str,
        now_ms: i64,
    ) -> Result<PropertyDef, ModelError> {
        check_name(name)?;
        let schema = self.read_schema();
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        if entry.name != name || !self.is_stored(property_id) {
            self.close_group();
            self.props_write(|| {
                let map = self.property_map_for_write(property_id, now_ms)?;
                meta::set_string(&map, key::NAME, name)
            })?;
        }
        self.property(property_id)
            .ok_or(ModelError::PropertyNotFound)
    }

    /// Removes a property from the schema. Tags cannot be removed. The values pages hold for it
    /// stay in the documents, unread and unindexed, until the next write of a page's values
    /// removes them.
    pub fn delete_property(&mut self, property_id: PropertyId) -> Result<(), ModelError> {
        if property_id == TAGS_PROPERTY_ID {
            return Err(refused("propertyId", "the Tags property cannot be deleted"));
        }
        let schema = self.read_schema();
        if schema.get(property_id).is_none() {
            return Err(ModelError::PropertyNotFound);
        }
        // The other half of a two-way relation becomes a one-way relation; it keeps its links.
        let partner = schema.paired_of(property_id).map(|entry| entry.id);
        self.close_group();
        self.props_write(|| {
            if let Some(partner) = partner {
                let map = self.existing_property_map(partner)?;
                write_config(&map, &one_way())?;
            }
            meta::remove(&self.properties, &property_id.to_string())
        })
    }

    /// Moves a property in front of `before` (`None` = last). Tags is not movable and always
    /// comes first; `before` may name it (the property then becomes the first of the others).
    pub fn move_property(
        &mut self,
        property_id: PropertyId,
        before: Option<PropertyId>,
    ) -> Result<(), ModelError> {
        if property_id == TAGS_PROPERTY_ID {
            return Err(refused(
                "propertyId",
                "the Tags property always comes first",
            ));
        }
        let schema = self.read_schema();
        if schema.get(property_id).is_none() {
            return Err(ModelError::PropertyNotFound);
        }
        if let Some(target) = before
            && schema.get(target).is_none()
        {
            return Err(ModelError::PropertyNotFound);
        }
        if before == Some(property_id) {
            return Ok(());
        }
        let all: Vec<&PropertyEntry> = schema.others().collect();
        let others: Vec<&PropertyEntry> = all
            .iter()
            .copied()
            .filter(|entry| entry.id != property_id)
            .collect();
        let at = match before {
            None => others.len(),
            Some(target) if target == TAGS_PROPERTY_ID => 0,
            Some(target) => others
                .iter()
                .position(|entry| entry.id == target)
                .ok_or(ModelError::PropertyNotFound)?,
        };
        if all.iter().position(|entry| entry.id == property_id) == Some(at) {
            return Ok(());
        }
        let prev = at.checked_sub(1).and_then(|i| others.get(i));
        let next = others.get(at);
        let key = order::key_between(
            prev.map(|e| e.position.as_str()),
            next.map(|e| e.position.as_str()),
        );

        self.close_group();
        self.props_write(|| {
            let moving = self.existing_property_map(property_id)?;
            if let Some(key) = &key {
                return meta::set_string(&moving, key::POSITION, key);
            }
            // Equal or damaged neighbours (two devices appended at once): renumber them all.
            let mut order_after: Vec<PropertyId> = others.iter().map(|e| e.id).collect();
            order_after.insert(at, property_id);
            for (id, position) in order_after.iter().zip(order::sequence(order_after.len())) {
                let map = self.existing_property_map(*id)?;
                meta::set_string(&map, key::POSITION, &position)?;
            }
            Ok(())
        })
    }

    /// Adds an option to a `select`, `multi_select` or `status` property, after the others. A
    /// `status` option has a group (`todo` unless given); the other kinds take none. Writes Tags
    /// first when it is not written yet.
    pub fn add_option(
        &mut self,
        property_id: PropertyId,
        option_id: OptionId,
        name: &str,
        color: OptionColor,
        group: Option<StatusGroup>,
        now_ms: i64,
    ) -> Result<PropertyOption, ModelError> {
        check_name(name)?;
        let schema = self.read_schema();
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        if !entry.kind.has_options() {
            return Err(refused(
                "propertyId",
                "this kind of property has no options",
            ));
        }
        let group = match (entry.kind, group) {
            (PropertyKind::Status, group) => Some(group.unwrap_or(StatusGroup::Todo)),
            (_, None) => None,
            (_, Some(_)) => return Err(refused("group", "only status options have a group")),
        };
        if entry.options.len() >= MAX_OPTIONS {
            return Err(ModelError::InvalidProperty {
                field: "propertyId",
                message: format!("a property holds at most {MAX_OPTIONS} options"),
            });
        }
        if entry.option(option_id).is_some() {
            return Err(refused("optionId", "an option with this id exists"));
        }
        let position = order::key_between(entry.options.last().map(|o| o.position.as_str()), None)
            .ok_or_else(|| ModelError::Shape("option positions are damaged".into()))?;

        self.close_group();
        self.props_write(|| {
            let map = self.property_map_for_write(property_id, now_ms)?;
            let options = ensure_child_map(&map, key::OPTIONS)?;
            write_option(&options, option_id, name, color, group, &position)
        })?;
        Ok(PropertyOption {
            id: option_id,
            name: name.to_owned(),
            color,
            group,
        })
    }

    /// Changes an option's name, colour or group; what is `None` stays. Returns the option as
    /// it is now.
    pub fn update_option(
        &mut self,
        property_id: PropertyId,
        option_id: OptionId,
        name: Option<&str>,
        color: Option<OptionColor>,
        group: Option<StatusGroup>,
    ) -> Result<PropertyOption, ModelError> {
        if let Some(name) = name {
            check_name(name)?;
        }
        let schema = self.read_schema();
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        let option = entry.option(option_id).ok_or(ModelError::OptionNotFound)?;
        if group.is_some() && entry.kind != PropertyKind::Status {
            return Err(refused("group", "only status options have a group"));
        }
        let changed = name.is_some_and(|name| name != option.name)
            || color.is_some_and(|color| color != option.color)
            || group.is_some_and(|group| Some(group) != option.group);
        if changed {
            self.close_group();
            self.props_write(|| {
                let map = self.existing_property_map(property_id)?;
                let options = ensure_child_map(&map, key::OPTIONS)?;
                let target = ensure_child_map(&options, &option_id.to_string())?;
                if let Some(name) = name {
                    meta::set_string(&target, key::NAME, name)?;
                }
                if let Some(color) = color {
                    meta::set_string(&target, key::COLOR, color.as_str())?;
                }
                if let Some(group) = group {
                    meta::set_string(&target, key::GROUP, group.as_str())?;
                }
                Ok(())
            })?;
        }
        self.property(property_id)
            .and_then(|def| def.options.into_iter().find(|o| o.id == option_id))
            .ok_or(ModelError::OptionNotFound)
    }

    /// Removes an option and takes it out of every page's value of that property, in one
    /// commit. Returns the pages whose value changed (their `updated_at` is `now_ms`); a
    /// `select` or `status` value that named it is cleared, a `multi_select` value loses it
    /// (and is cleared when it was the last).
    pub fn delete_option(
        &mut self,
        property_id: PropertyId,
        option_id: OptionId,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let schema = self.read_schema();
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        entry.option(option_id).ok_or(ModelError::OptionNotFound)?;
        let kind = entry.kind;
        let property_key = property_id.to_string();
        let option_key = option_id.to_string();

        // The pages to change, found before anything is written.
        let mut pages: Vec<(PageId, TreeID)> = self
            .index
            .pages
            .iter()
            .map(|(page, node)| (*page, *node))
            .collect();
        pages.sort_unstable_by_key(|(page, _)| *page);
        let mut rewrites: Vec<(PageId, TreeID, Option<LoroValue>)> = Vec::new();
        for (page, node) in pages {
            let Some(props) = self.props_map(node) else {
                continue;
            };
            match (kind, meta::value(&props, &property_key)) {
                (PropertyKind::Select | PropertyKind::Status, Some(LoroValue::String(id)))
                    if id.as_str() == option_key =>
                {
                    rewrites.push((page, node, None));
                }
                (PropertyKind::MultiSelect, Some(LoroValue::List(items)))
                    if items.iter().any(
                        |item| matches!(item, LoroValue::String(id) if id.as_str() == option_key),
                    ) =>
                {
                    let kept: Vec<LoroValue> = items
                        .iter()
                        .filter(|item| !matches!(item, LoroValue::String(id) if id.as_str() == option_key))
                        .cloned()
                        .collect();
                    rewrites.push((
                        page,
                        node,
                        (!kept.is_empty()).then(|| LoroValue::from(kept)),
                    ));
                }
                _ => {}
            }
        }

        self.close_group();
        self.props_write(|| {
            let map = self.existing_property_map(property_id)?;
            if let Some(options) = child_map(&map, key::OPTIONS) {
                meta::remove(&options, &option_key)?;
            }
            for (_, node, replacement) in &rewrites {
                let meta_map = self.tree.get_meta(*node).map_err(engine)?;
                let props = ensure_child_map(&meta_map, key::PROPS)?;
                match replacement {
                    Some(list) => props.insert(&property_key, list.clone()).map_err(engine)?,
                    None => meta::remove(&props, &property_key)?,
                }
                self.stamp_edit(&meta_map, now_ms)?;
            }
            Ok(())
        })?;
        Ok(rewrites.into_iter().map(|(page, _, _)| page).collect())
    }

    // ----- values: writes -----------------------------------------------------------------------

    /// Whether [`Self::set_page_property`] would accept `value` for `property_id` on a page: the
    /// checks that need no page (the property exists and is not computed, the value's kind is the
    /// property's, the value passes its own rules, the options it names are listed). Writes
    /// nothing; a page being made from a template (ADR-0056) is checked with it before it exists.
    /// [`ModelError::PropertyNotFound`] or [`ModelError::InvalidProperty`] when refused.
    pub fn check_page_property(
        &self,
        property_id: PropertyId,
        value: &PropertyValue,
    ) -> Result<(), ModelError> {
        let schema = self.read_schema();
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        if entry.kind.is_computed() {
            return Err(refused(
                "propertyId",
                "this property is computed from the page and cannot be set",
            ));
        }
        if value.kind() != entry.kind {
            return Err(refused(
                "value",
                "the value does not match the kind of the property",
            ));
        }
        if let Some(checked) = value::check(value)? {
            for option in value::options_of(checked) {
                if entry.option(option).is_none() {
                    return Err(refused("value", "the value names an unknown option"));
                }
            }
        }
        Ok(())
    }

    /// Sets (or, with `None`, clears) a page's value of a property. The page may be live or
    /// trashed. Returns the page when something was written (its `updated_at` becomes
    /// `now_ms`), nothing when the value was already there.
    ///
    /// The value's kind must be the property's, a `select` or `status` option and every option
    /// of a `multi_select` must be one the property lists, and the value must pass its own
    /// rules ([`ModelError::InvalidProperty`], field `value`). An empty text, an empty URL and
    /// an empty list are no value: they clear. A write also removes the values of properties the
    /// schema no longer has from that page, and the template placeholder the page holds for the
    /// property (ADR-0056), in the same commit.
    pub fn set_page_property(
        &mut self,
        page_id: PageId,
        property_id: PropertyId,
        value: Option<PropertyValue>,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let schema = self.read_schema();
        let planned = self.plan_value_write(&schema, page_id, property_id, value)?;
        if matches!(planned, PlannedWrite::Nothing) {
            return Ok(Vec::new());
        }
        self.close_group();
        self.props_write(|| self.apply_value_write(&schema, &planned, now_ms))
    }

    /// Sets (or, with `None`, clears) one property of many pages in **one commit** (one
    /// properties-origin change that structural undo does not record): the writes of
    /// [`Self::set_page_property`] for each `(page, value)` of `entries`, in order, so a two-way
    /// relation pairs up as it would one write at a time. Returns the pages written, without
    /// duplicates, in the order they were written; a value a page already holds writes nothing.
    ///
    /// All or nothing: the property and every page and value are checked first (as
    /// [`Self::set_page_property`] checks them: [`ModelError::PropertyNotFound`],
    /// [`ModelError::PageNotFound`], [`ModelError::InvalidProperty`]) and nothing is written when
    /// one is refused. Pages are not checked for being live: the caller does that.
    pub fn set_pages_property(
        &mut self,
        property_id: PropertyId,
        entries: &[(PageId, Option<PropertyValue>)],
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let schema = self.read_schema();
        let kind = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?
            .kind;
        if kind.is_computed() {
            return Err(refused(
                "propertyId",
                "this property is computed from the page and cannot be set",
            ));
        }
        for (page_id, value) in entries {
            self.index.node(*page_id).ok_or(ModelError::PageNotFound)?;
            if let Some(value) = value {
                self.check_page_property(property_id, value)?;
            }
        }
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        self.close_group();
        let mut written: Vec<PageId> = Vec::new();
        let mut seen: HashSet<PageId> = HashSet::new();
        self.props_write(|| {
            for (page_id, value) in entries {
                // The schema is read again for each page: the pages that list a page through a
                // two-way relation change with every write.
                let schema = self.read_schema();
                let planned =
                    self.plan_value_write(&schema, *page_id, property_id, value.clone())?;
                for page in self.apply_value_write(&schema, &planned, now_ms)? {
                    if seen.insert(page) {
                        written.push(page);
                    }
                }
            }
            Ok(())
        })?;
        Ok(written)
    }

    /// Decides what writing `value` (or, with `None`, clearing) as the page's value of a property
    /// comes to, checking everything [`Self::set_page_property`] checks. Writes nothing.
    fn plan_value_write(
        &self,
        schema: &Schema,
        page_id: PageId,
        property_id: PropertyId,
        value: Option<PropertyValue>,
    ) -> Result<PlannedWrite, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        if entry.kind.is_computed() {
            return Err(refused(
                "propertyId",
                "this property is computed from the page and cannot be set",
            ));
        }
        if entry.kind == PropertyKind::Relation {
            let wanted = match &value {
                None => Vec::new(),
                Some(PropertyValue::Relation(pages)) => {
                    value::check(&PropertyValue::Relation(pages.clone()))?;
                    pages.clone()
                }
                Some(_) => {
                    return Err(refused(
                        "value",
                        "the value does not match the kind of the property",
                    ));
                }
            };
            return self.plan_relation(page_id, node, schema, entry.id, &wanted);
        }
        let stored = match &value {
            None => None,
            Some(value) => {
                if value.kind() != entry.kind {
                    return Err(refused(
                        "value",
                        "the value does not match the kind of the property",
                    ));
                }
                let checked = value::check(value)?;
                if let Some(checked) = checked {
                    for option in value::options_of(checked) {
                        if entry.option(option).is_none() {
                            return Err(refused("value", "the value names an unknown option"));
                        }
                    }
                }
                checked.map(value::encode)
            }
        };
        let property_key = property_id.to_string();
        let current = self
            .props_map(node)
            .and_then(|props| meta::value(&props, &property_key));
        // A page that holds a template placeholder for the property loses it with any write of the
        // value (ADR-0056), even one that leaves the value as it is (a cleared value, say).
        let placeholder = self.has_template_placeholder(node, property_id);
        if current == stored && !placeholder {
            return Ok(PlannedWrite::Nothing);
        }
        let changed = current != stored;
        Ok(PlannedWrite::Plain {
            page_id,
            node,
            property_id,
            stored,
            changed,
            placeholder,
        })
    }

    /// Writes what [`Self::plan_value_write`] decided, without committing (the caller does, with
    /// [`Self::props_write`]). Returns the pages written.
    fn apply_value_write(
        &self,
        schema: &Schema,
        planned: &PlannedWrite,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        match planned {
            PlannedWrite::Nothing => Ok(Vec::new()),
            PlannedWrite::Plain {
                page_id,
                node,
                property_id,
                stored,
                changed,
                placeholder,
            } => {
                let property_key = property_id.to_string();
                let meta_map = self.tree.get_meta(*node).map_err(engine)?;
                if *changed {
                    match stored {
                        Some(raw) => {
                            let props = ensure_child_map(&meta_map, key::PROPS)?;
                            props.insert(&property_key, raw.clone()).map_err(engine)?;
                        }
                        None => {
                            if let Some(props) = child_map(&meta_map, key::PROPS) {
                                meta::remove(&props, &property_key)?;
                            }
                        }
                    }
                }
                if *placeholder {
                    self.clear_template_placeholder(&meta_map, *property_id)?;
                }
                // Lazy clean-up: values of deleted properties go with the next write.
                if let Some(props) = child_map(&meta_map, key::PROPS) {
                    for raw in props.keys() {
                        let known = raw
                            .parse::<PropertyId>()
                            .is_ok_and(|id| schema.get(id).is_some());
                        if !known {
                            meta::remove(&props, &raw)?;
                        }
                    }
                }
                self.stamp_edit(&meta_map, now_ms)?;
                Ok(vec![*page_id])
            }
            PlannedWrite::Relation(plan) => self.apply_relation(schema, plan, now_ms),
        }
    }

    /// Decides a relation write: the value of the page and, for a two-way relation, the paired
    /// value of every page it adds or removes. Writes nothing.
    fn plan_relation(
        &self,
        page_id: PageId,
        node: TreeID,
        schema: &Schema,
        property_id: PropertyId,
        wanted: &[PageId],
    ) -> Result<PlannedWrite, ModelError> {
        let entry = schema
            .get(property_id)
            .ok_or(ModelError::PropertyNotFound)?;
        let current = self.effective_relation(schema, entry, node);
        let added: Vec<PageId> = wanted
            .iter()
            .copied()
            .filter(|page| self.index.node(*page).is_some() && !current.contains(page))
            .collect();
        let removed: Vec<PageId> = current
            .iter()
            .copied()
            .filter(|page| !wanted.contains(page))
            .collect();
        let property_key = property_id.to_string();
        let stored =
            (!wanted.is_empty()).then(|| value::encode(&PropertyValue::Relation(wanted.to_vec())));
        let current_stored = self
            .props_map(node)
            .and_then(|props| meta::value(&props, &property_key));
        let own_change = current_stored != stored;
        let partner = schema.paired_of(property_id).map(|entry| entry.id);
        // The paired value of each affected page after the change: add or remove this page.
        let mut others: Vec<(PageId, TreeID, Option<LoroValue>)> = Vec::new();
        if let Some(partner) = partner {
            let partner_entry = schema.get(partner).ok_or(ModelError::PropertyNotFound)?;
            let partner_key = partner.to_string();
            for (other, add) in added
                .iter()
                .map(|p| (*p, true))
                .chain(removed.iter().map(|p| (*p, false)))
            {
                let Some(other_node) = self.index.node(other) else {
                    continue;
                };
                let listed = match self
                    .props_map(other_node)
                    .and_then(|props| meta::value(&props, &partner_key))
                {
                    Some(raw) => match value::decode(partner_entry.kind, &raw) {
                        Some(PropertyValue::Relation(pages)) => pages,
                        _ => Vec::new(),
                    },
                    None => Vec::new(),
                };
                let mut next = listed.clone();
                if add && !next.contains(&page_id) {
                    next.push(page_id);
                } else if !add {
                    next.retain(|listed| *listed != page_id);
                }
                if next == listed {
                    continue;
                }
                others.push((
                    other,
                    other_node,
                    (!next.is_empty()).then(|| value::encode(&PropertyValue::Relation(next))),
                ));
            }
        }
        if !own_change && others.is_empty() {
            return Ok(PlannedWrite::Nothing);
        }
        Ok(PlannedWrite::Relation(RelationWrite {
            page_id,
            node,
            property_id,
            stored,
            own_change,
            partner,
            others,
        }))
    }

    /// Writes a relation value and, for a two-way relation, the paired value of every page it
    /// adds or removes, without committing. Returns the pages written (the page first).
    fn apply_relation(
        &self,
        schema: &Schema,
        plan: &RelationWrite,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let RelationWrite {
            page_id,
            node,
            property_id,
            stored,
            own_change,
            partner,
            others,
        } = plan;
        let property_key = property_id.to_string();
        let meta_map = self.tree.get_meta(*node).map_err(engine)?;
        if *own_change {
            match stored {
                Some(raw) => {
                    let props = ensure_child_map(&meta_map, key::PROPS)?;
                    props.insert(&property_key, raw.clone()).map_err(engine)?;
                }
                None => {
                    if let Some(props) = child_map(&meta_map, key::PROPS) {
                        meta::remove(&props, &property_key)?;
                    }
                }
            }
            // Lazy clean-up: values of deleted properties go with the next write.
            if let Some(props) = child_map(&meta_map, key::PROPS) {
                for raw in props.keys() {
                    let known = raw
                        .parse::<PropertyId>()
                        .is_ok_and(|id| schema.get(id).is_some());
                    if !known {
                        meta::remove(&props, &raw)?;
                    }
                }
            }
            self.stamp_edit(&meta_map, now_ms)?;
        }
        if let Some(partner) = partner {
            let partner_key = partner.to_string();
            for (_, other_node, replacement) in others {
                let other_meta = self.tree.get_meta(*other_node).map_err(engine)?;
                match replacement {
                    Some(list) => {
                        let props = ensure_child_map(&other_meta, key::PROPS)?;
                        props.insert(&partner_key, list.clone()).map_err(engine)?;
                    }
                    None => {
                        if let Some(props) = child_map(&other_meta, key::PROPS) {
                            meta::remove(&props, &partner_key)?;
                        }
                    }
                }
                self.stamp_edit(&other_meta, now_ms)?;
            }
        }
        let mut changed = Vec::new();
        if *own_change {
            changed.push(*page_id);
        }
        for (other, _, _) in others {
            if !changed.contains(other) {
                changed.push(*other);
            }
        }
        Ok(changed)
    }

    // ----- plumbing ---------------------------------------------------------------------------

    /// A write that structural undo must not record (see [`PROPERTIES_ORIGIN`]): anything pending
    /// is committed first, and the write is committed with the origin.
    pub(crate) fn props_write<T>(
        &self,
        write: impl FnOnce() -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        self.commit();
        let result = write();
        self.doc
            .inner()
            .commit_with(CommitOptions::new().origin(PROPERTIES_ORIGIN));
        result
    }

    /// Whether the property has an entry in the document (Tags may be virtual).
    fn is_stored(&self, property_id: PropertyId) -> bool {
        child_map(&self.properties, &property_id.to_string()).is_some()
    }

    /// The map of a property that is in the document.
    fn existing_property_map(&self, property_id: PropertyId) -> Result<LoroMap, ModelError> {
        child_map(&self.properties, &property_id.to_string()).ok_or(ModelError::PropertyNotFound)
    }

    /// The map of a property for writing: the existing one, or Tags written now.
    fn property_map_for_write(
        &self,
        property_id: PropertyId,
        now_ms: i64,
    ) -> Result<LoroMap, ModelError> {
        if let Some(map) = child_map(&self.properties, &property_id.to_string()) {
            return Ok(map);
        }
        if property_id != TAGS_PROPERTY_ID {
            return Err(ModelError::PropertyNotFound);
        }
        let map = ensure_child_map(&self.properties, &property_id.to_string())?;
        meta::set_string(&map, key::NAME, TAGS_NAME)?;
        meta::set_string(&map, key::KIND, PropertyKind::MultiSelect.as_str())?;
        meta::set_string(&map, key::POSITION, TAGS_POSITION)?;
        meta::set_int(&map, key::CREATED_AT, now_ms)?;
        ensure_child_map(&map, key::OPTIONS)?;
        Ok(map)
    }
}

/// Writes one option map.
fn write_option(
    options: &LoroMap,
    id: OptionId,
    name: &str,
    color: OptionColor,
    group: Option<StatusGroup>,
    position: &str,
) -> Result<(), ModelError> {
    let map = ensure_child_map(options, &id.to_string())?;
    meta::set_string(&map, key::NAME, name)?;
    meta::set_string(&map, key::COLOR, color.as_str())?;
    meta::set_string(&map, key::POSITION, position)?;
    meta::set_opt_string(&map, key::GROUP, group.map(StatusGroup::as_str))
}

// ----- invariants and the portable form --------------------------------------------------------

impl StructureDoc {
    /// Findings about the schema and the values, for `check_invariants` (ids only, never names
    /// or values). A value of a property the schema has not got, or naming an option it does
    /// not list, is reported although it is kept on purpose.
    pub(crate) fn property_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut schema_ids = HashSet::new();
        for raw in self.properties.keys() {
            let Ok(id) = raw.parse::<PropertyId>() else {
                problems.push("a properties entry has a malformed key".into());
                continue;
            };
            let Some(map) = child_map(&self.properties, &raw) else {
                problems.push(format!("property {id} is not a map"));
                continue;
            };
            if !schema_ids.insert(id) {
                problems.push(format!("property {id} appears twice"));
            }
            match meta::string(&map, key::KIND) {
                None => problems.push(format!("property {id} has no kind")),
                Some(kind) if PropertyKind::parse(&kind).is_none() => {
                    problems.push(format!(
                        "property {id} has a kind this version does not know"
                    ));
                }
                Some(_) => {}
            }
            if meta::string(&map, key::NAME).is_none() {
                problems.push(format!("property {id} has no name"));
            }
            if meta::string(&map, key::POSITION).is_none() {
                problems.push(format!("property {id} has no position"));
            }
            if meta::int(&map, key::CREATED_AT).is_none() {
                problems.push(format!("property {id} has no creation time"));
            }
            self.option_problems(id, &map, &mut problems);
        }

        let schema = self.read_schema();
        for entry in &schema.entries {
            if entry.kind == PropertyKind::Rollup
                && entry
                    .config
                    .as_ref()
                    .is_none_or(|config| check_rollup_config(&schema, config).is_err())
            {
                problems.push(format!(
                    "rollup {} has a configuration that is no longer valid",
                    entry.id
                ));
            }
            // A formula that does not parse or check (a property it read was deleted, a loop made
            // by two devices) has no result type and no value; reported like a damaged rollup.
            if entry.kind == PropertyKind::Formula
                && !matches!(
                    entry.config,
                    Some(PropertyConfig::Formula {
                        result_type: Some(_),
                        ..
                    })
                )
            {
                problems.push(format!(
                    "formula {} does not check against the schema any more",
                    entry.id
                ));
            }
        }
        let mut pages: Vec<(PageId, TreeID)> = self
            .index
            .pages
            .iter()
            .map(|(page, node)| (*page, *node))
            .collect();
        pages.sort_unstable_by_key(|(page, _)| *page);
        for (page, node) in pages {
            let Some(props) = self.props_map(node) else {
                continue;
            };
            let mut keys: Vec<String> = props.keys().map(|key| key.to_string()).collect();
            keys.sort_unstable();
            for raw in keys {
                let Some(entry) = raw.parse::<PropertyId>().ok().and_then(|id| schema.get(id))
                else {
                    problems.push(format!("page {page} holds a value of an unknown property"));
                    continue;
                };
                if raw != entry.id.to_string() {
                    problems.push(format!(
                        "page {page} holds a value under a malformed property key"
                    ));
                    continue;
                }
                let Some(stored) = meta::value(&props, &raw) else {
                    problems.push(format!(
                        "page {page} holds a value of property {} that is not a plain value",
                        entry.id
                    ));
                    continue;
                };
                match value::decode(entry.kind, &stored) {
                    None => problems.push(format!(
                        "page {page} holds a value of property {} that does not read as its kind",
                        entry.id
                    )),
                    Some(value) => {
                        let unknown = value::options_of(&value)
                            .into_iter()
                            .any(|option| entry.option(option).is_none());
                        if unknown {
                            problems.push(format!(
                                "page {page} holds a value of property {} that names an unknown option",
                                entry.id
                            ));
                        }
                    }
                }
            }
        }
        problems
    }

    fn option_problems(&self, id: PropertyId, map: &LoroMap, problems: &mut Vec<String>) {
        let kind = meta::string(map, key::KIND).and_then(|kind| PropertyKind::parse(&kind));
        let Some(options) = child_map(map, key::OPTIONS) else {
            return;
        };
        if kind.is_some_and(|kind| !kind.has_options()) && !options.is_empty() {
            problems.push(format!("property {id} has options but its kind takes none"));
        }
        for raw in options.keys() {
            let Ok(option_id) = raw.parse::<OptionId>() else {
                problems.push(format!("property {id} has an option with a malformed key"));
                continue;
            };
            let Some(option) = child_map(&options, &raw) else {
                problems.push(format!("option {option_id} of property {id} is not a map"));
                continue;
            };
            if meta::string(&option, key::NAME).is_none() {
                problems.push(format!("option {option_id} of property {id} has no name"));
            }
            if meta::string(&option, key::POSITION).is_none() {
                problems.push(format!(
                    "option {option_id} of property {id} has no position"
                ));
            }
            match meta::string(&option, key::COLOR) {
                Some(color) if OptionColor::parse(&color).is_some() => {}
                _ => problems.push(format!(
                    "option {option_id} of property {id} has no colour of the palette"
                )),
            }
            let group = meta::string(&option, key::GROUP);
            match (kind, group) {
                (Some(PropertyKind::Status), Some(group))
                    if StatusGroup::parse(&group).is_some() => {}
                (Some(PropertyKind::Status), _) => problems.push(format!(
                    "status option {option_id} of property {id} has no valid group"
                )),
                (_, Some(_)) => problems.push(format!(
                    "option {option_id} of property {id} has a group but is not a status option"
                )),
                (_, None) => {}
            }
        }
    }

    /// The `properties` object of the portable JSON: every property keyed by id with its
    /// stored fields and options (sorted keys), or `None` when the document has none.
    pub(crate) fn portable_properties(&self) -> Option<serde_json::Value> {
        let mut keys: Vec<String> = self.properties.keys().map(|key| key.to_string()).collect();
        if keys.is_empty() {
            return None;
        }
        keys.sort_unstable();
        let mut out = serde_json::Map::new();
        for raw in keys {
            let Some(map) = child_map(&self.properties, &raw) else {
                continue;
            };
            let mut obj = serde_json::Map::new();
            if let Some(created_at) = meta::int(&map, key::CREATED_AT) {
                obj.insert("createdAt".into(), created_at.into());
            }
            if let Some(kind) = meta::string(&map, key::KIND) {
                obj.insert("kind".into(), kind.into());
            }
            if let Some(name) = meta::string(&map, key::NAME) {
                obj.insert("name".into(), name.into());
            }
            if let Some(config) = child_map(&map, key::CONFIG) {
                let mut config_keys: Vec<String> =
                    config.keys().map(|key| key.to_string()).collect();
                config_keys.sort_unstable();
                let mut listed = serde_json::Map::new();
                for config_key in config_keys {
                    if let Some(stored) = meta::value(&config, &config_key) {
                        listed.insert(config_key, loro_json(&stored));
                    }
                }
                obj.insert("config".into(), listed.into());
            }
            if let Some(options) = child_map(&map, key::OPTIONS) {
                let mut option_keys: Vec<String> =
                    options.keys().map(|key| key.to_string()).collect();
                option_keys.sort_unstable();
                let mut listed = serde_json::Map::new();
                for option_key in option_keys {
                    let Some(option) = child_map(&options, &option_key) else {
                        continue;
                    };
                    let mut entry = serde_json::Map::new();
                    for (json_key, field) in [
                        ("color", key::COLOR),
                        ("group", key::GROUP),
                        ("name", key::NAME),
                        ("position", key::POSITION),
                    ] {
                        if let Some(text) = meta::string(&option, field) {
                            entry.insert(json_key.into(), text.into());
                        }
                    }
                    listed.insert(option_key, entry.into());
                }
                obj.insert("options".into(), listed.into());
            }
            if let Some(position) = meta::string(&map, key::POSITION) {
                obj.insert("position".into(), position.into());
            }
            out.insert(raw, obj.into());
        }
        Some(out.into())
    }

    /// The `props` object of a page in the portable JSON: the stored values as they are (sorted
    /// keys), or `None` when the page has none.
    pub(crate) fn portable_props(&self, node: TreeID) -> Option<serde_json::Value> {
        let props = self.props_map(node)?;
        let mut keys: Vec<String> = props.keys().map(|key| key.to_string()).collect();
        if keys.is_empty() {
            return None;
        }
        keys.sort_unstable();
        let mut out = serde_json::Map::new();
        for raw in keys {
            if let Some(stored) = meta::value(&props, &raw) {
                out.insert(raw, loro_json(&stored));
            }
        }
        Some(out.into())
    }

    /// The values a page node stores, raw, as JSON and by property id (sorted by id; `null`
    /// values and keys that are not property ids are left out): what a move between spaces
    /// carries to the target space, where the core maps the ids and options (ADR-0038).
    pub(crate) fn props_json(&self, node: TreeID) -> Vec<(PropertyId, serde_json::Value)> {
        let Some(props) = self.props_map(node) else {
            return Vec::new();
        };
        let mut out: Vec<(PropertyId, serde_json::Value)> = props
            .keys()
            .filter_map(|raw| {
                let id = raw.parse::<PropertyId>().ok()?;
                let stored = meta::value(&props, &raw)?;
                (!matches!(stored, LoroValue::Null)).then(|| (id, loro_json(&stored)))
            })
            .collect();
        out.sort_unstable_by_key(|(id, _)| *id);
        out
    }
}

/// Writes raw values (as [`StructureDoc::props_json`] reads them, `null` ones skipped) into the
/// `props` map of a page node's metadata, creating it when there is anything to write.
pub(crate) fn write_props_json(
    meta_map: &LoroMap,
    values: &[(PropertyId, serde_json::Value)],
) -> Result<(), ModelError> {
    let mut props: Option<LoroMap> = None;
    for (id, value) in values {
        if value.is_null() {
            continue;
        }
        let map = match &props {
            Some(map) => map.clone(),
            None => {
                let map = ensure_child_map(meta_map, key::PROPS)?;
                props = Some(map.clone());
                map
            }
        };
        map.insert(&id.to_string(), json_loro(value, 0))
            .map_err(engine)?;
    }
    Ok(())
}

/// Deepest nesting of a value written back by [`write_props_json`] (stored values nest two
/// levels at most; anything deeper is not a value this crate wrote).
const MAX_JSON_DEPTH: usize = 8;

/// A JSON value as a plain Loro value: integers are `I64`, other numbers `Double` (the reverse
/// of `loro_json`).
fn json_loro(value: &serde_json::Value, depth: usize) -> LoroValue {
    use serde_json::Value;
    if depth > MAX_JSON_DEPTH {
        return LoroValue::Null;
    }
    match value {
        Value::Null => LoroValue::Null,
        Value::Bool(on) => LoroValue::Bool(*on),
        Value::Number(number) => match number.as_i64() {
            Some(whole) => LoroValue::I64(whole),
            None => LoroValue::Double(number.as_f64().unwrap_or(0.0)),
        },
        Value::String(text) => LoroValue::from(text.as_str()),
        Value::Array(items) => LoroValue::from(
            items
                .iter()
                .map(|item| json_loro(item, depth + 1))
                .collect::<Vec<_>>(),
        ),
        Value::Object(map) => {
            let mut out: HashMap<String, LoroValue> = HashMap::with_capacity(map.len());
            for (name, item) in map {
                out.insert(name.clone(), json_loro(item, depth + 1));
            }
            LoroValue::from(out)
        }
    }
}

/// A plain Loro value as JSON (maps with sorted keys; non-finite numbers become `null`).
fn loro_json(value: &LoroValue) -> serde_json::Value {
    match value {
        LoroValue::Null => serde_json::Value::Null,
        LoroValue::Bool(on) => (*on).into(),
        LoroValue::Double(number) => serde_json::Number::from_f64(*number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        LoroValue::I64(number) => (*number).into(),
        LoroValue::String(text) => text.to_string().into(),
        LoroValue::List(items) => items.iter().map(loro_json).collect::<Vec<_>>().into(),
        LoroValue::Map(map) => {
            let mut entries: Vec<(&String, &LoroValue)> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::new();
            for (name, item) in entries {
                out.insert(name.clone(), loro_json(item));
            }
            out.into()
        }
        _ => serde_json::Value::Null,
    }
}
