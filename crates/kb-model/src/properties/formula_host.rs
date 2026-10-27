//! The formula engine's view of a space (ADR-0055, `docs/architecture/formulas.md`): the schema
//! it checks against, the pages and values it reads, and the time of a read.
//!
//! The engine (`crate::formula`) is pure; this module is the glue: [`Schema`] answers
//! [`FormulaSchema`] (properties by id, with the options, a formula's expression and a rollup's
//! function), [`ModelHost`] answers [`FormulaHost`] over the structure document (stored values and
//! rollups as the page's values read them, a relation as the rollup reads it: both directions, live
//! pages only, titles, creation and edit times, file names). The people of the space are the core's
//! to know (the model reads account ids only, ADR-0049): the owner tells the model the names of the
//! members with [`StructureDoc::set_formula_people`] and [`FormulaHost::person`] answers from that
//! directory (an account it lacks, a former member's, has no name: `name(person)` is empty).
//!
//! **People.** Like the time, the directory is replica state of the document, not a parameter of the
//! many readers. It is rebuilt by the owner only when the membership or a name changed: the owner
//! stamps it with a revision of its own ([`StructureDoc::formula_people_revision`] says which one the
//! document holds), so a read costs nothing when nothing changed. [`StructureDoc::read_schema`] copies
//! the directory (an `Arc`) into the [`Schema`] with the time.
//!
//! **Time.** `now()` is the time of the read. The owner of the document (the core) tells the model
//! the time and the device's offset with [`StructureDoc::set_formula_env`] before it reads (a field
//! of the document rather than a parameter, so none of the many readers of the tree, a page or its
//! values changes); [`StructureDoc::read_schema`] copies it into the [`Schema`], so one read sees
//! one time. A document nobody told reads the epoch at offset 0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, PoisonError};

use kb_types::{
    AccountId, AttachmentId, FormulaType, PageId, PropertyConfig, PropertyId, PropertyKind,
    PropertyValue, RollupTarget,
};

use super::{PropertyEntry, Schema, check_rollup_config, refused};
use crate::formula::{
    self, Formula, FormulaEnv, FormulaError, FormulaHost, FormulaProperty, FormulaSchema,
};
use crate::meta;
use crate::{ModelError, StructureDoc};

/// The time formulas read ([`FormulaEnv`] kept in atomics, so a read through `&StructureDoc` can
/// see what the owner set and the document stays `Sync`).
#[derive(Debug, Default)]
pub(crate) struct FormulaTime {
    now_ms: AtomicI64,
    offset_seconds: AtomicI32,
}

impl FormulaTime {
    fn set(&self, env: FormulaEnv) {
        self.now_ms.store(env.now_ms, Ordering::Relaxed);
        self.offset_seconds
            .store(env.offset_seconds, Ordering::Relaxed);
    }

    pub(super) fn get(&self) -> (i64, i32) {
        (
            self.now_ms.load(Ordering::Relaxed),
            self.offset_seconds.load(Ordering::Relaxed),
        )
    }
}

/// The names and e-mails of the members of the space (`name(person)`, `email(person)`), as the owner last
/// told them, and the owner's revision of that knowledge.
#[derive(Debug, Default)]
pub(crate) struct FormulaPeople {
    revision: Option<u64>,
    directory: Arc<HashMap<AccountId, (String, String)>>,
}

/// The people formulas can name: account to (name, e-mail).
pub(super) type People = Arc<HashMap<AccountId, (String, String)>>;

impl StructureDoc {
    /// Tells the document who the members of its space are, for `name(person)` and `email(person)` in
    /// formulas: each account with its display name and e-mail (`""` when unknown). `revision` is the
    /// owner's own counter of the membership and names; the document keeps both until the next call
    /// (replica state, not document state). An account that is not listed (a former member) reads with
    /// no name.
    pub fn set_formula_people(&self, revision: u64, people: HashMap<AccountId, (String, String)>) {
        let mut held = self
            .formula_people
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        held.revision = Some(revision);
        held.directory = Arc::new(people);
    }

    /// The revision the owner stamped the people with (`None`: nobody told the document yet).
    #[must_use]
    pub fn formula_people_revision(&self) -> Option<u64> {
        self.formula_people
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .revision
    }

    /// The people as the schema of a read holds them.
    pub(super) fn formula_directory(&self) -> People {
        Arc::clone(
            &self
                .formula_people
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .directory,
        )
    }

    /// Tells the document the time formulas read as "now" (`Clock::now_ms`) and the device's offset
    /// from UTC (`Clock::local_offset_seconds`, ADR-0024). The owner calls it before it reads the
    /// tree, a page or the values of a page; the document keeps it (replica state, not document
    /// state) until the next call.
    pub fn set_formula_env(&self, env: FormulaEnv) {
        self.formula_time.set(env);
    }

    /// The time formulas read now (see [`Self::set_formula_env`]).
    #[must_use]
    pub fn formula_env(&self) -> FormulaEnv {
        let (now_ms, offset_seconds) = self.formula_time.get();
        FormulaEnv {
            now_ms,
            offset_seconds,
        }
    }
}

impl Schema {
    /// The time of the read this schema was made for.
    pub(super) fn env(&self) -> FormulaEnv {
        FormulaEnv {
            now_ms: self.time.0,
            offset_seconds: self.time.1,
        }
    }

    /// Parses and checks every formula of the schema against the schema itself: a formula that
    /// parses and checks keeps its parsed form and gets its `result_type`; any other has no result
    /// type and no parsed form, so no page has a value for it (`formulas.md` section 4.3, ADR-0055
    /// item 5). Dependencies and cycles are the checker's (`self_id`).
    pub(super) fn derive_formulas(&mut self) {
        let mut derived: Vec<(PropertyId, Option<(Formula, FormulaType)>)> = Vec::new();
        for entry in &self.entries {
            if entry.kind != PropertyKind::Formula {
                continue;
            }
            let outcome = match &entry.config {
                Some(config @ PropertyConfig::Formula { .. }) => {
                    check_formula_config(self, Some(entry.id), config).ok()
                }
                _ => None,
            };
            derived.push((entry.id, outcome));
        }
        for (id, outcome) in derived {
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id)
                && let Some(PropertyConfig::Formula { result_type, .. }) = &mut entry.config
            {
                *result_type = outcome.as_ref().map(|(_, ty)| *ty);
            }
            if let Some((parsed, _)) = outcome {
                self.formulas.insert(id, parsed);
            }
        }
    }

    fn formula_property(&self, entry: &PropertyEntry) -> FormulaProperty {
        let expression = match (&entry.kind, &entry.config) {
            (PropertyKind::Formula, Some(PropertyConfig::Formula { expression, .. })) => {
                Some(expression.clone())
            }
            _ => None,
        };
        let rollup = match (&entry.kind, &entry.config) {
            (PropertyKind::Rollup, Some(config @ PropertyConfig::Rollup { function, .. })) => {
                check_rollup_config(self, config)
                    .ok()
                    .map(|target| (*function, target))
            }
            _ => None,
        };
        // A rollup shows the options of its target (`show_original` of a select, a multi-select or a
        // status: the names of the original values); the other kinds their own.
        let option_source = match (&entry.config, rollup) {
            (Some(PropertyConfig::Rollup { target, .. }), Some(_)) => match target {
                RollupTarget::Property(id) => self.get(*id).unwrap_or(entry),
                RollupTarget::Title => entry,
            },
            _ => entry,
        };
        FormulaProperty {
            id: entry.id.to_string(),
            name: entry.name.clone(),
            kind: entry.kind,
            options: option_source
                .options
                .iter()
                .map(|option| (option.id, option.name.clone()))
                .collect(),
            expression,
            rollup,
        }
    }
}

impl FormulaSchema for Schema {
    fn property(&self, id: &str) -> Option<FormulaProperty> {
        let id = id.parse::<PropertyId>().ok()?;
        self.get(id).map(|entry| self.formula_property(entry))
    }
}

/// Parses and checks the configuration of a formula against the schema; `self_id` is the property
/// itself (a reference to it, directly or through other formulas, is a cycle). Returns the parsed
/// expression and the type of its result. Errors are `InvalidProperty` for the field `config`, with
/// the error's code, position and message.
pub(super) fn check_formula_config(
    schema: &Schema,
    self_id: Option<PropertyId>,
    config: &PropertyConfig,
) -> Result<(Formula, FormulaType), ModelError> {
    let PropertyConfig::Formula { expression, .. } = config else {
        return Err(refused(
            "config",
            "the configuration does not match the kind",
        ));
    };
    let parsed = formula::parse(expression).map_err(formula_refusal)?;
    let self_id = self_id.map(|id| id.to_string());
    let result_type =
        formula::check(&parsed, schema, self_id.as_deref()).map_err(formula_refusal)?;
    Ok((parsed, result_type))
}

fn formula_refusal(err: FormulaError) -> ModelError {
    ModelError::InvalidProperty {
        field: "config",
        message: format!("the formula is refused: {err}"),
    }
}

/// What a formula reads of a structure document (see the module documentation).
pub(super) struct ModelHost<'a> {
    pub(super) doc: &'a StructureDoc,
    pub(super) schema: &'a Schema,
}

impl ModelHost<'_> {
    /// Whether the page exists and is not in the trash.
    fn is_live(&self, page: PageId) -> bool {
        self.doc
            .index
            .node(page)
            .and_then(|node| self.doc.placement(node))
            .is_some_and(|placement| placement.trashed_top.is_none())
    }

    fn meta_int(&self, page: PageId, key: &str) -> Option<i64> {
        let node = self.doc.index.node(page)?;
        meta::int(&self.doc.meta_of(node)?, key)
    }
}

impl FormulaSchema for ModelHost<'_> {
    fn property(&self, id: &str) -> Option<FormulaProperty> {
        self.schema.property(id)
    }
}

impl FormulaHost for ModelHost<'_> {
    fn title(&self, page: PageId) -> Option<String> {
        let node = self.doc.index.node(page)?;
        Some(
            self.doc
                .meta_of(node)
                .and_then(|map| meta::string(&map, meta::TITLE))
                .unwrap_or_default(),
        )
    }

    fn value(&self, page: PageId, property: PropertyId) -> Option<PropertyValue> {
        let node = self.doc.index.node(page)?;
        let entry = self.schema.get(property)?;
        match entry.kind {
            PropertyKind::Rollup => self.doc.rollup_value(self.schema, entry, node),
            PropertyKind::Relation => {
                let pages = self.related(page, property);
                (!pages.is_empty()).then_some(PropertyValue::Relation(pages))
            }
            // A formula is the engine's to evaluate; the times are `created_ms` and `edited_ms`;
            // the devices are not readable in a formula.
            kind if kind.is_computed() => None,
            _ => self.doc.stored_value(entry, node),
        }
    }

    fn related(&self, page: PageId, relation: PropertyId) -> Vec<PageId> {
        let Some(node) = self.doc.index.node(page) else {
            return Vec::new();
        };
        let Some(entry) = self
            .schema
            .get(relation)
            .filter(|entry| entry.kind == PropertyKind::Relation)
        else {
            return Vec::new();
        };
        self.doc
            .effective_relation(self.schema, entry, node)
            .into_iter()
            .filter(|related| self.is_live(*related))
            .collect()
    }

    fn created_ms(&self, page: PageId) -> Option<i64> {
        self.meta_int(page, meta::CREATED_AT)
    }

    fn edited_ms(&self, page: PageId) -> Option<i64> {
        self.meta_int(page, meta::UPDATED_AT)
            .or_else(|| self.meta_int(page, meta::CREATED_AT))
    }

    fn person(&self, account: AccountId) -> Option<(String, String)> {
        self.schema.people.get(&account).cloned()
    }

    fn file_name(&self, attachment: AttachmentId) -> Option<String> {
        self.doc
            .attachment(attachment)
            .map(|manifest| manifest.name)
    }
}

/// The parsed formulas of a schema, by property id (kept in [`Schema`]).
pub(super) type ParsedFormulas = HashMap<PropertyId, Formula>;
