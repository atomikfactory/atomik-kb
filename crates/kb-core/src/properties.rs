//! Typed page properties (ADR-0026): the handlers of the property requests, and the derived
//! `page_properties` rows and property rows of the text index.
//!
//! The schema and the values live in the space's structure document (`kb_model`'s `properties`
//! module), so they sync and are encrypted like the page tree and need nothing of their own in
//! the sync protocol. This module validates requests, applies them to the document, persists
//! the update together with the derived rows in one transaction
//! ([`Core::persist_structure_with`](crate::Core)), and answers the queries over the derived
//! table. Every change emits `StructureChanged` (page values travel in `PageInfo::properties`);
//! a change of the schema emits `PropertySchemaChanged` first.
//!
//! The derived rows of a page are rebuilt from the document whenever its values change, for a
//! whole space when the schema changes in a way the rows depend on (a property deleted, an option
//! renamed), when a remote update changes the schema, by `IndexRebuild`, and once at open for a
//! database that was migrated to schema v10.
//!
//! **Attachment references.** The rows of a `files` value (`page_properties.attachment_id`) count
//! as references of the attachment, next to the `image` and `file` blocks of the page documents
//! (`kb_storage::AttachmentQueries::attachment_reference_count`). Whenever rows are replaced or
//! removed, the attachments they attached before and attach after go through the same bookkeeping
//! as a document change (`Core::attachment_refs_changed`): an attachment left without a reference
//! becomes collectable by the retention GC, one that is referenced again is kept. So a file that
//! only a property holds is not collected; removing it from the value, deleting the property or
//! purging the page makes it collectable, and `IndexRebuild` restores the references with the rows.

use std::collections::{BTreeSet, HashMap};

use kb_model::{PageIndexEntry, PropertyRow, StructureDoc};
use kb_storage::{
    DocQueries, FtsBlock, PagePropertyQueries, PagePropertyRow, PropertyMatch, StorageError,
    TextIndexQueries, UpdateOrigin,
};
use kb_types::properties::{MAX_FILTER_CHARS, MAX_NAME_CHARS};
use kb_types::{
    AccountId, ApiError, AttachmentId, BlockId, DateValue, Event, MemberRole, OptionColor,
    OptionId, PageId, PageNode, PageProperties, PropertyConfig, PropertyFilter, PropertyId,
    PropertyKind, PropertySchema, PropertyValue, Request, Response, SpaceId, StatusGroup,
};

use crate::documents::map_model_error;
use crate::{Core, map_storage_error, validate};

/// `meta` key that marks a database whose derived property rows were built from the documents.
const INDEXED_MARKER: &str = "page_properties_indexed";

/// Which derived property rows a structure write rewrites in its own transaction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PropsScope<'a> {
    /// None (the rows of pages that left the document are removed anyway).
    Nothing,
    /// The rows of these pages.
    Pages(&'a [PageId]),
    /// Every row of the space, rebuilt.
    Space,
}

/// What a write of derived property rows consists of, computed from the model before the
/// transaction opens.
pub(crate) struct PropertyWrites {
    /// Clear the property rows of the whole space first (`Space` scope).
    clear_space: bool,
    /// `(page, derived rows)`; a page with an empty entry has its rows cleared.
    entries: Vec<(PageId, PageIndexEntry)>,
    /// Pages the document does not know any more: their rows go.
    gone: Vec<PageId>,
}

impl PropertyWrites {
    /// Pages the document does not know any more whose rows belong to another space now (they
    /// moved there, ADR-0038) are not "gone": their rows stay. Pages without any row, or with
    /// rows of this space, are gone as before.
    pub(crate) fn keep_rows_of_moved_pages<Q: DocQueries>(
        &mut self,
        q: &Q,
        space_id: SpaceId,
    ) -> Result<(), StorageError> {
        let mut kept = Vec::with_capacity(self.gone.len());
        for page in &self.gone {
            let elsewhere = q
                .pages_index_get(*page)?
                .is_some_and(|row| row.space_id != space_id);
            if !elsewhere {
                kept.push(*page);
            }
        }
        self.gone = kept;
        Ok(())
    }

    /// The attachments the rows of this write attach.
    pub(crate) fn attachments(&self) -> BTreeSet<AttachmentId> {
        self.entries
            .iter()
            .flat_map(|(_, entry)| entry.rows.iter().filter_map(|row| row.attachment_id))
            .collect()
    }

    /// The attachments the stored rows attach that this write replaces or removes: read before
    /// the write, together with [`Self::attachments`] they are the attachments whose reference
    /// count the write may change. `also_gone` are pages the same transaction removes.
    pub(crate) fn replaced_attachments<Q: PagePropertyQueries>(
        &self,
        q: &Q,
        space_id: SpaceId,
        also_gone: &[PageId],
    ) -> Result<Vec<AttachmentId>, StorageError> {
        if self.clear_space {
            return q.page_properties_attachments_in_space(space_id);
        }
        let mut pages: Vec<PageId> = self.entries.iter().map(|(page, _)| *page).collect();
        pages.extend(self.gone.iter().copied());
        pages.extend(also_gone.iter().copied());
        q.page_properties_attachments_of(&pages)
    }
}

/// The rows to write for `scope`. `people` are the names the space's membership gives, which the
/// text rows of Person values are made of (the model knows only account ids: its search text for a
/// Person value is empty, [`kb_model::person_text`] joins the names the core knows).
pub(crate) fn property_writes(
    model: &StructureDoc,
    scope: PropsScope<'_>,
    people: &HashMap<AccountId, String>,
) -> PropertyWrites {
    let mut writes = property_writes_of(model, scope);
    for (_, entry) in &mut writes.entries {
        let mut properties: Vec<PropertyId> = Vec::new();
        for row in &entry.rows {
            if row.kind == PropertyKind::Person && !properties.contains(&row.property_id) {
                properties.push(row.property_id);
            }
        }
        for property in properties {
            let accounts: Vec<AccountId> = entry
                .rows
                .iter()
                .filter(|row| row.property_id == property)
                .filter_map(|row| row.account_id)
                .collect();
            if let Some(text) =
                kb_model::person_text(&accounts, &|account| people.get(&account).cloned())
            {
                entry.texts.push((property, text));
            }
        }
    }
    writes
}

fn property_writes_of(model: &StructureDoc, scope: PropsScope<'_>) -> PropertyWrites {
    match scope {
        PropsScope::Nothing => PropertyWrites {
            clear_space: false,
            entries: Vec::new(),
            gone: Vec::new(),
        },
        PropsScope::Pages(pages) => {
            let entries = model.property_index(pages);
            let gone = pages
                .iter()
                .filter(|page| !entries.iter().any(|(known, _)| known == *page))
                .copied()
                .collect();
            PropertyWrites {
                clear_space: false,
                entries,
                gone,
            }
        }
        PropsScope::Space => PropertyWrites {
            clear_space: true,
            entries: model.property_index_all(),
            gone: Vec::new(),
        },
    }
}

/// Writes the derived property rows of `writes` through a transaction or connection.
pub(crate) fn write_property_rows<Q>(
    q: &Q,
    space_id: SpaceId,
    writes: &PropertyWrites,
) -> Result<(), StorageError>
where
    Q: PagePropertyQueries + TextIndexQueries,
{
    if writes.clear_space {
        q.page_properties_clear_space(space_id)?;
        q.fts_clear_properties(space_id)?;
    }
    for page in &writes.gone {
        q.page_properties_delete_pages(&[*page])?;
        q.fts_replace_properties(*page, &[])?;
    }
    for (page, entry) in &writes.entries {
        let rows: Vec<PagePropertyRow> = entry.rows.iter().map(storage_row).collect();
        q.page_properties_replace(space_id, *page, &rows)?;
        let texts: Vec<FtsBlock> = entry
            .texts
            .iter()
            .map(|(property, text)| FtsBlock {
                // The property's id stands where a block's would.
                block_id: BlockId::from_bytes(property.to_bytes()),
                text: text.clone(),
            })
            .collect();
        q.fts_replace_properties(*page, &texts)?;
    }
    Ok(())
}

fn storage_row(row: &PropertyRow) -> PagePropertyRow {
    PagePropertyRow {
        property_id: row.property_id,
        kind: row.kind,
        value_text: row.value_text.clone(),
        value_number: row.value_number,
        value_bool: row.value_bool,
        value_date: row.value_date.clone(),
        value_time: row.value_time.clone(),
        option_id: row.option_id,
        attachment_id: row.attachment_id,
        value_total: row.value_total,
        related_page_id: row.related_page_id,
        account_id: row.account_id,
    }
}

/// What a property change leaves to refresh, besides the document.
struct Change {
    /// Pages whose `updated_at` the change set (their page index rows are refreshed).
    pages: Vec<PageId>,
    /// The derived rows to rewrite.
    rows: Rows,
    /// The schema changed (`PropertySchemaChanged`).
    schema: bool,
}

enum Rows {
    Nothing,
    Pages(Vec<PageId>),
    Space,
}

impl Core {
    /// Dispatch entry for the property requests (ADR-0026).
    pub(crate) fn dispatch_properties(&mut self, request: Request) -> Result<Response, ApiError> {
        use Request as R;
        match request {
            R::PropertySchemaGet { space_id } => Ok(Response::PropertySchema(PropertySchema {
                properties: self.structure(space_id)?.model.property_schema(),
            })),
            R::PropertyCreate {
                space_id,
                name,
                kind,
                config,
            } => self.property_create(space_id, &name, kind, config.as_ref()),
            R::PropertyConfigSet {
                space_id,
                property_id,
                config,
            } => {
                let def = self.change_properties(space_id, |model, _| {
                    let def = model.set_property_config(property_id, &config)?;
                    Ok((def, schema_change()))
                })?;
                Ok(Response::Property(def))
            }
            R::PropertyRename {
                space_id,
                property_id,
                name,
            } => {
                let name = validate::display_name(&name, MAX_NAME_CHARS, "name")?;
                let def = self.change_properties(space_id, |model, now| {
                    let def = model.rename_property(property_id, &name, now)?;
                    Ok((def, schema_change()))
                })?;
                Ok(Response::Property(def))
            }
            R::PropertyDelete {
                space_id,
                property_id,
            } => {
                self.change_properties(space_id, |model, _| {
                    model.delete_property(property_id)?;
                    // Its values leave the index at once (they stay in the documents).
                    Ok((
                        (),
                        Change {
                            pages: Vec::new(),
                            rows: Rows::Space,
                            schema: true,
                        },
                    ))
                })?;
                Ok(Response::Ok)
            }
            R::PropertyMove {
                space_id,
                property_id,
                before,
            } => {
                self.change_properties(space_id, |model, _| {
                    model.move_property(property_id, before)?;
                    Ok(((), schema_change()))
                })?;
                Ok(Response::Ok)
            }
            R::PropertyOptionAdd {
                space_id,
                property_id,
                name,
                color,
                group,
            } => self.option_add(space_id, property_id, &name, color, group),
            R::PropertyOptionUpdate {
                space_id,
                property_id,
                option_id,
                name,
                color,
                group,
            } => {
                let name = name
                    .map(|name| validate::display_name(&name, MAX_NAME_CHARS, "name"))
                    .transpose()?;
                let option = self.change_properties(space_id, |model, _| {
                    let option = model.update_option(
                        property_id,
                        option_id,
                        name.as_deref(),
                        color,
                        group,
                    )?;
                    // A renamed option changes the text of the pages that have it.
                    Ok((
                        option,
                        Change {
                            pages: Vec::new(),
                            rows: if name.is_some() {
                                Rows::Space
                            } else {
                                Rows::Nothing
                            },
                            schema: true,
                        },
                    ))
                })?;
                Ok(Response::PropertyOption(option))
            }
            R::PropertyOptionDelete {
                space_id,
                property_id,
                option_id,
            } => {
                self.change_properties(space_id, |model, now| {
                    let pages = model.delete_option(property_id, option_id, now)?;
                    Ok((
                        (),
                        Change {
                            rows: Rows::Pages(pages.clone()),
                            pages,
                            schema: true,
                        },
                    ))
                })?;
                Ok(Response::Ok)
            }
            R::PagePropertySet {
                page_id,
                property_id,
                value,
            } => {
                let space_id = self.space_of_page(page_id)?;
                if let Some(PropertyValue::Person(accounts)) = &value {
                    // Members of the space when the value is written (ADR-0049), the ones the value
                    // listed already excepted (one may have left since); the structural rules (20 at
                    // most, distinct) are the model's.
                    let kept: Vec<AccountId> = self
                        .structure(space_id)?
                        .model
                        .page_properties(page_id)
                        .map_err(map_model_error)?
                        .into_iter()
                        .find_map(|(id, current)| match current {
                            PropertyValue::Person(listed) if id == property_id => Some(listed),
                            _ => None,
                        })
                        .unwrap_or_default();
                    self.person_value_check(space_id, accounts, &kept)?;
                }
                self.change_properties(space_id, |model, now| {
                    let pages = model.set_page_property(page_id, property_id, value, now)?;
                    // The pages written: this one, and for a two-way relation the paired ones.
                    let mut rows = vec![page_id];
                    rows.extend(pages.iter().copied().filter(|page| *page != page_id));
                    Ok((
                        (),
                        Change {
                            rows: Rows::Pages(rows),
                            pages,
                            schema: false,
                        },
                    ))
                })?;
                Ok(Response::PageProperties(self.page_properties(page_id)?))
            }
            R::PagePropertiesGet { page_id } => {
                Ok(Response::PageProperties(self.page_properties(page_id)?))
            }
            R::PagesByProperty {
                space_id,
                property_id,
                filter,
            } => Ok(Response::PageIds(self.pages_by_property(
                space_id,
                property_id,
                &filter,
            )?)),
            other => Err(ApiError::internal(format!(
                "request {} is not a property request",
                other.kind()
            ))),
        }
    }

    fn property_create(
        &mut self,
        space_id: SpaceId,
        name: &str,
        kind: PropertyKind,
        config: Option<&PropertyConfig>,
    ) -> Result<Response, ApiError> {
        let name = validate::display_name(name, MAX_NAME_CHARS, "name")?;
        let property_id = PropertyId::generate(self.rng.as_ref());
        // A two-way relation is two properties made in one commit (ADR-0027).
        if kind == PropertyKind::Relation
            && let Some(PropertyConfig::Relation {
                two_way: true,
                paired_property_id: None,
            }) = config
        {
            let back_id = PropertyId::generate(self.rng.as_ref());
            let back_name = back_property_name(&name);
            let def = self.change_properties(space_id, |model, now| {
                let def =
                    model.create_relation_pair(property_id, back_id, &name, &back_name, now)?;
                Ok((def, schema_change()))
            })?;
            return Ok(Response::Property(def));
        }
        // The three options of a new status property get their ids from the core's randomness.
        let mut spare: Vec<OptionId> = (0..3)
            .map(|_| OptionId::generate(self.rng.as_ref()))
            .collect();
        let def = self.change_properties(space_id, |model, now| {
            let def = model.create_property_with(
                property_id,
                &name,
                kind,
                config,
                &mut || spare.pop().unwrap_or_else(|| OptionId::from_bytes([0; 16])),
                now,
            )?;
            Ok((def, schema_change()))
        })?;
        Ok(Response::Property(def))
    }

    fn option_add(
        &mut self,
        space_id: SpaceId,
        property_id: PropertyId,
        name: &str,
        color: OptionColor,
        group: Option<StatusGroup>,
    ) -> Result<Response, ApiError> {
        let name = validate::display_name(name, MAX_NAME_CHARS, "name")?;
        let option_id = OptionId::generate(self.rng.as_ref());
        let option = self.change_properties(space_id, |model, now| {
            let option = model.add_option(property_id, option_id, &name, color, group, now)?;
            Ok((option, schema_change()))
        })?;
        Ok(Response::PropertyOption(option))
    }

    /// Applies `op` to the space's structure document, persists the update with the derived
    /// rows the change calls for, and emits the events: `PropertySchemaChanged` for a schema
    /// change, then `StructureChanged`.
    fn change_properties<T>(
        &mut self,
        space_id: SpaceId,
        op: impl FnOnce(&mut StructureDoc, i64) -> Result<(T, Change), kb_model::ModelError>,
    ) -> Result<T, ApiError> {
        self.structure(space_id)?;
        // The schema and the values are Editor writes in a shared space (ADR-0043).
        self.require_role(space_id, MemberRole::Editor)?;
        let now = self.clock.now_ms();
        let open = self.structure_mut(space_id)?;
        let (out, change) = op(&mut open.model, now).map_err(map_model_error)?;
        let rows = match &change.rows {
            Rows::Nothing => PropsScope::Nothing,
            Rows::Pages(pages) => PropsScope::Pages(pages),
            Rows::Space => PropsScope::Space,
        };
        self.persist_structure_with(space_id, Some(&change.pages), UpdateOrigin::Local, rows)?;
        if change.schema {
            self.events.push(Event::PropertySchemaChanged { space_id });
        }
        self.events.push(Event::StructureChanged { space_id });
        Ok(out)
    }

    /// `PageBulk` `SetValue` and `SetValues` (ADR-0058): one property of many live pages of a space, in one
    /// properties change like a single `PagePropertySet`: the same checks (the property, the value, the
    /// members of a Person value) for every entry before anything is written, then one commit, one persist
    /// with the derived rows of the pages written, and one `StructureChanged`. The caller has checked the
    /// pages and the role.
    pub(crate) fn pages_property_set(
        &mut self,
        space_id: SpaceId,
        property_id: PropertyId,
        entries: &[(PageId, Option<PropertyValue>)],
    ) -> Result<(), ApiError> {
        if entries
            .iter()
            .any(|(_, value)| matches!(value, Some(PropertyValue::Person(_))))
        {
            // Members of the space when the values are written (ADR-0049), the ones a page lists
            // already excepted (one may have left since); the structural rules are the model's.
            let members = self.space_member_accounts(space_id)?;
            for (page_id, value) in entries {
                let Some(PropertyValue::Person(accounts)) = value else {
                    continue;
                };
                if accounts.iter().all(|account| members.contains(account)) {
                    continue;
                }
                let kept: Vec<AccountId> = self
                    .structure(space_id)?
                    .model
                    .page_properties(*page_id)
                    .map_err(map_model_error)?
                    .into_iter()
                    .find_map(|(id, current)| match current {
                        PropertyValue::Person(listed) if id == property_id => Some(listed),
                        _ => None,
                    })
                    .unwrap_or_default();
                self.person_value_check(space_id, accounts, &kept)?;
            }
        }
        self.change_properties(space_id, |model, now| {
            let pages = model.set_pages_property(property_id, entries, now)?;
            Ok((
                (),
                Change {
                    rows: Rows::Pages(pages.clone()),
                    pages,
                    schema: false,
                },
            ))
        })
    }

    fn page_properties(&self, page_id: PageId) -> Result<PageProperties, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        let values = self
            .structure(space_id)?
            .model
            .page_properties(page_id)
            .map_err(map_model_error)?;
        Ok(PageProperties { values })
    }

    /// The live pages of a space whose value of a property matches `filter`, by title.
    fn pages_by_property(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        filter: &PropertyFilter,
    ) -> Result<Vec<PageId>, ApiError> {
        let def = self
            .structure(space_id)?
            .model
            .property(property_id)
            .ok_or_else(|| ApiError::not_found("property not found"))?;
        let kind = def.kind;
        if kind.is_computed() {
            return self.computed_pages(space_id, property_id, kind, filter);
        }
        if kind == PropertyKind::Relation {
            return self.relation_pages(space_id, property_id, def.config.as_ref(), filter);
        }
        if kind == PropertyKind::Person {
            return self.person_pages(space_id, property_id, filter);
        }
        let options: Vec<OptionId>;
        let day: DateValue;
        let matching = match filter {
            PropertyFilter::HasValue => PropertyMatch::HasValue,
            PropertyFilter::Equals(value) => {
                if value.kind() != kind {
                    return Err(ApiError::invalid(
                        "filter",
                        "the value does not match the kind of the property",
                    ));
                }
                let Some(value) = kb_model::normalise_value(value).map_err(map_filter_error)?
                else {
                    // An empty text, URL or list is no value: nothing equals it.
                    return Ok(Vec::new());
                };
                match value {
                    PropertyValue::Relation(_) => {
                        return Err(ApiError::invalid(
                            "filter",
                            "a list of pages cannot be compared; use HasValue or RelatesTo",
                        ));
                    }
                    PropertyValue::Files(_) | PropertyValue::Checklist(_) => {
                        return Err(ApiError::invalid(
                            "filter",
                            "a list of files or items cannot be compared; use HasValue \
                             (or ChecklistComplete for a checklist)",
                        ));
                    }
                    PropertyValue::Place(_) => {
                        return Err(ApiError::invalid(
                            "filter",
                            "a place cannot be compared; use TextContains or HasValue",
                        ));
                    }
                    // Not reachable: a Person property is answered by `person_pages` above.
                    PropertyValue::Person(_) => return Ok(Vec::new()),
                    // Not reachable: the kind of the value is the property's, which is not computed.
                    PropertyValue::Rollup(_)
                    | PropertyValue::Formula(_)
                    | PropertyValue::CreatedTime(_)
                    | PropertyValue::LastEditedTime(_)
                    | PropertyValue::CreatedBy(_)
                    | PropertyValue::LastEditedBy(_) => return Ok(Vec::new()),
                    PropertyValue::Text(text)
                    | PropertyValue::Url(text)
                    | PropertyValue::Email(text)
                    | PropertyValue::Phone(text) => {
                        return self.pages_matching(
                            space_id,
                            property_id,
                            &PropertyMatch::Text(&text),
                        );
                    }
                    PropertyValue::Number(number) => PropertyMatch::Number(number),
                    PropertyValue::Checkbox(on) => PropertyMatch::Bool(on),
                    PropertyValue::Select(option) | PropertyValue::Status(option) => {
                        PropertyMatch::Option(option)
                    }
                    PropertyValue::MultiSelect(ids) => {
                        options = ids;
                        PropertyMatch::OptionSet(&options)
                    }
                    PropertyValue::Date(value) => {
                        day = value;
                        PropertyMatch::Date {
                            date: &day.date,
                            time: day.time.as_deref(),
                        }
                    }
                }
            }
            PropertyFilter::ContainsOption(option) => {
                if !kind.has_options() {
                    return Err(ApiError::invalid(
                        "filter",
                        "this kind of property has no options",
                    ));
                }
                PropertyMatch::Option(*option)
            }
            PropertyFilter::TextContains(text) => {
                if !matches!(
                    kind,
                    PropertyKind::Text
                        | PropertyKind::Url
                        | PropertyKind::Email
                        | PropertyKind::Phone
                        | PropertyKind::Place
                ) {
                    return Err(ApiError::invalid(
                        "filter",
                        "text can only be searched in a text, URL, email, phone or place property",
                    ));
                }
                if text.is_empty() {
                    return Err(ApiError::invalid("filter", "must not be empty"));
                }
                if text.chars().count() > MAX_FILTER_CHARS {
                    return Err(ApiError::invalid(
                        "filter",
                        format!("must be at most {MAX_FILTER_CHARS} characters"),
                    ));
                }
                if text.chars().any(char::is_control) {
                    return Err(ApiError::invalid(
                        "filter",
                        "must not contain control characters",
                    ));
                }
                PropertyMatch::TextContains(text)
            }
            PropertyFilter::DateBetween { from, to } => {
                if kind != PropertyKind::Date {
                    return Err(ApiError::invalid(
                        "filter",
                        "dates can only be searched in a date property",
                    ));
                }
                for end in [from, to].into_iter().flatten() {
                    if !kb_model::valid_date(end) {
                        return Err(ApiError::invalid(
                            "filter",
                            "a date must be a real day as YYYY-MM-DD",
                        ));
                    }
                }
                if let (Some(from), Some(to)) = (from, to)
                    && from > to
                {
                    return Err(ApiError::invalid("filter", "from must not be after to"));
                }
                PropertyMatch::DateBetween {
                    from: from.as_deref(),
                    to: to.as_deref(),
                }
            }
            PropertyFilter::Checked(on) => {
                if kind != PropertyKind::Checkbox {
                    return Err(ApiError::invalid(
                        "filter",
                        "a checked state can only be searched in a checkbox property",
                    ));
                }
                if *on {
                    PropertyMatch::Bool(true)
                } else {
                    PropertyMatch::NotChecked
                }
            }
            PropertyFilter::ChecklistComplete(complete) => {
                if kind != PropertyKind::Checklist {
                    return Err(ApiError::invalid(
                        "filter",
                        "completion can only be searched in a checklist property",
                    ));
                }
                PropertyMatch::ChecklistComplete(*complete)
            }
            PropertyFilter::InstantBetween { .. } => {
                return Err(ApiError::invalid(
                    "filter",
                    "instants can only be searched in a created or last edited time property",
                ));
            }
            PropertyFilter::RelatesTo(_) => {
                return Err(ApiError::invalid(
                    "filter",
                    "related pages can only be searched in a relation property",
                ));
            }
            PropertyFilter::ContainsPerson(_) | PropertyFilter::ContainsMe => {
                return Err(ApiError::invalid(
                    "filter",
                    "people can only be searched in a person property",
                ));
            }
        };
        self.pages_matching(space_id, property_id, &matching)
    }

    /// The live pages of a space whose relation matches `filter`: `HasValue` (a related page that
    /// exists) or `RelatesTo` a page. A two-way relation answers for both directions (the symmetric
    /// view of ADR-0027): a page also matches when the other page stores it in the paired property.
    fn relation_pages(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        config: Option<&PropertyConfig>,
        filter: &PropertyFilter,
    ) -> Result<Vec<PageId>, ApiError> {
        let paired = match config {
            Some(PropertyConfig::Relation {
                two_way: true,
                paired_property_id,
            }) => *paired_property_id,
            _ => None,
        };
        let page = match filter {
            PropertyFilter::HasValue => None,
            PropertyFilter::RelatesTo(page) => Some(*page),
            PropertyFilter::Equals(_) => {
                return Err(ApiError::invalid(
                    "filter",
                    "a list of pages cannot be compared; use HasValue or RelatesTo",
                ));
            }
            PropertyFilter::ContainsOption(_) => {
                return Err(ApiError::invalid(
                    "filter",
                    "this kind of property has no options",
                ));
            }
            PropertyFilter::TextContains(_) => {
                return Err(ApiError::invalid(
                    "filter",
                    "text can only be searched in a text, URL, email, phone or place property",
                ));
            }
            PropertyFilter::DateBetween { .. } => {
                return Err(ApiError::invalid(
                    "filter",
                    "dates can only be searched in a date property",
                ));
            }
            PropertyFilter::Checked(_) => {
                return Err(ApiError::invalid(
                    "filter",
                    "a checked state can only be searched in a checkbox property",
                ));
            }
            PropertyFilter::ChecklistComplete(_) => {
                return Err(ApiError::invalid(
                    "filter",
                    "completion can only be searched in a checklist property",
                ));
            }
            PropertyFilter::InstantBetween { .. } => {
                return Err(ApiError::invalid(
                    "filter",
                    "instants can only be searched in a created or last edited time property",
                ));
            }
            PropertyFilter::ContainsPerson(_) | PropertyFilter::ContainsMe => {
                return Err(ApiError::invalid(
                    "filter",
                    "people can only be searched in a person property",
                ));
            }
        };
        self.pages_matching(
            space_id,
            property_id,
            &PropertyMatch::Related { page, paired },
        )
    }

    /// The live pages of a space whose computed value (created or last edited time or device)
    /// matches `filter`, by title like the other queries. Computed values have no derived rows:
    /// they are read from the pages in the document, which costs one pass over the space.
    fn computed_pages(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        kind: PropertyKind,
        filter: &PropertyFilter,
    ) -> Result<Vec<PageId>, ApiError> {
        let refused = |message: &str| Err(ApiError::invalid("filter", message));
        // A formula is computed on every read and has no derived rows: nothing is searched by it
        // here, the views filter it in the client like a rollup (ADR-0055 item 4).
        if kind == PropertyKind::Formula {
            return refused("a formula cannot be searched; its pages are filtered in the views");
        }
        enum Test<'a> {
            Any,
            Equals(&'a PropertyValue),
            Between(Option<i64>, Option<i64>),
        }
        let test = match filter {
            PropertyFilter::HasValue => Test::Any,
            PropertyFilter::Equals(value) => {
                if value.kind() != kind {
                    return refused("the value does not match the kind of the property");
                }
                Test::Equals(value)
            }
            PropertyFilter::InstantBetween { from, to } => {
                if !matches!(
                    kind,
                    PropertyKind::CreatedTime | PropertyKind::LastEditedTime
                ) {
                    return refused(
                        "instants can only be searched in a created or last edited time property",
                    );
                }
                if let (Some(from), Some(to)) = (from, to)
                    && from > to
                {
                    return refused("from must not be after to");
                }
                Test::Between(*from, *to)
            }
            PropertyFilter::ContainsOption(_) => {
                return refused("this kind of property has no options");
            }
            PropertyFilter::TextContains(_) => {
                return refused(
                    "text can only be searched in a text, URL, email, phone or place property",
                );
            }
            PropertyFilter::DateBetween { .. } => {
                return refused("dates can only be searched in a date property");
            }
            PropertyFilter::Checked(_) => {
                return refused("a checked state can only be searched in a checkbox property");
            }
            PropertyFilter::ChecklistComplete(_) => {
                return refused("completion can only be searched in a checklist property");
            }
            PropertyFilter::RelatesTo(_) => {
                return refused("related pages can only be searched in a relation property");
            }
            PropertyFilter::ContainsPerson(_) | PropertyFilter::ContainsMe => {
                return refused("people can only be searched in a person property");
            }
        };
        let mut found = Vec::new();
        let mut stack: Vec<PageNode> = self.structure(space_id)?.model.live_tree();
        while let Some(node) = stack.pop() {
            stack.extend(node.children);
            let info = node.page;
            let Some(value) = info
                .properties
                .iter()
                .find(|(id, _)| *id == property_id)
                .map(|(_, value)| value)
            else {
                continue;
            };
            let keep = match (&test, value) {
                (Test::Any, _) => true,
                (Test::Equals(wanted), value) => value == *wanted,
                (
                    Test::Between(from, to),
                    PropertyValue::CreatedTime(at) | PropertyValue::LastEditedTime(at),
                ) => from.is_none_or(|from| *at >= from) && to.is_none_or(|to| *at <= to),
                (Test::Between(..), _) => false,
            };
            if keep {
                found.push((info.title, info.id));
            }
        }
        // As in SQL: by title (ASCII case-insensitive, then exact), then by id.
        found.sort_by(|(a_title, a_id), (b_title, b_id)| {
            a_title
                .to_ascii_lowercase()
                .cmp(&b_title.to_ascii_lowercase())
                .then_with(|| a_title.cmp(b_title))
                .then_with(|| a_id.to_bytes().cmp(&b_id.to_bytes()))
        });
        Ok(found.into_iter().map(|(_, id)| id).collect())
    }

    fn pages_matching(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        matching: &PropertyMatch<'_>,
    ) -> Result<Vec<PageId>, ApiError> {
        self.db
            .page_properties_pages(space_id, property_id, matching)
            .map_err(map_storage_error)
    }

    // ----- derived rows --------------------------------------------------------------------

    /// Rewrites the derived property rows of `scope` from the structure document, in a
    /// transaction of their own (after an undo, at open, by `IndexRebuild`).
    pub(crate) fn refresh_property_rows(
        &mut self,
        space_id: SpaceId,
        scope: PropsScope<'_>,
    ) -> Result<(), ApiError> {
        let people = self.space_people_names(space_id);
        let writes = property_writes(&self.structure(space_id)?.model, scope, &people);
        let mut touched: BTreeSet<AttachmentId> = writes.attachments();
        touched.extend(
            writes
                .replaced_attachments(&self.db, space_id, &[])
                .map_err(map_storage_error)?,
        );
        self.db
            .transaction(|tx| write_property_rows(tx, space_id, &writes))
            .map_err(map_storage_error)?;
        // Files that were attached and no longer are (or the other way round) change their
        // references: the retention bookkeeping of the attachments follows.
        let now = self.clock.now_ms();
        self.attachment_refs_changed(touched, now)
    }

    /// Fills the derived property rows from the documents once, for a database that was
    /// migrated to schema v10 (its structure documents may hold properties that came by sync
    /// before). Runs at open from `verify_text_index`; a fresh database has no spaces and just
    /// gets the marker.
    pub(crate) fn verify_page_properties(&mut self) -> Result<(), ApiError> {
        if self
            .db
            .meta_get(INDEXED_MARKER)
            .map_err(map_storage_error)?
            .is_some()
        {
            return Ok(());
        }
        let spaces: Vec<SpaceId> = self.docs.structures.keys().copied().collect();
        for space_id in spaces {
            self.refresh_property_rows(space_id, PropsScope::Space)?;
        }
        self.db
            .meta_set(INDEXED_MARKER, "1")
            .map_err(map_storage_error)
    }
}

/// The name of the back property of a new two-way relation: "<name> (back)", the name cut short
/// to fit the limit.
pub(crate) fn back_property_name(name: &str) -> String {
    const SUFFIX: &str = " (back)";
    let room = MAX_NAME_CHARS.saturating_sub(SUFFIX.chars().count());
    let base: String = name.chars().take(room).collect();
    format!("{}{SUFFIX}", base.trim_end())
}

/// A change of the schema alone: no value and no derived row changes.
fn schema_change() -> Change {
    Change {
        pages: Vec::new(),
        rows: Rows::Nothing,
        schema: true,
    }
}

/// A filter value that fails its own rules.
fn map_filter_error(err: kb_model::ModelError) -> ApiError {
    match err {
        kb_model::ModelError::InvalidProperty { message, .. } => {
            ApiError::invalid("filter", message)
        }
        other => map_model_error(other),
    }
}
