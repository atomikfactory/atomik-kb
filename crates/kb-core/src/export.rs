//! Export sessions and the CSV export of a view (ADR-0036, `docs/architecture/import-export.md`,
//! "Export"): the core gathers the pages, their metadata, attachments and content, and renders
//! them with `kb-export`; the shell writes the archive.
//!
//! A session is planned by `ExportBegin` (the live pages in tree order with their metadata and the
//! attachments and whiteboards they use, laid out by `kb_export::layout`) and then walked by
//! `ExportNext`, which renders one page at a time into a small queue of entries and hands out at
//! most `maxEntries` of them. Nothing of the content is kept between calls, and the export never
//! creates a page document: a page without a stored one exports as an empty page.

use std::collections::{HashMap, HashSet, VecDeque};

use kb_doc::DocKind;
use kb_export::{
    ExportLinks, ExportProperty, ExportValue, Layout, LayoutAttachment, LayoutPage, LinkedFile,
    LinkedPage, PageMeta,
};
use kb_model::PageDoc;
use kb_storage::{AttachmentQueries, Db, DocQueries};
use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{
    AccountId, ApiError, AttachmentAvailability, AttachmentId, BlockId, CsvColumn, DeviceId, DocId,
    Event, ExportBatch, ExportContent, ExportEntry, ExportFormat, ExportPlan, ExportWarning,
    ExportWarningCode, FormulaResult, OptionId, PageId, PageInfo, PageNode, PropertyDef,
    PropertyId, PropertySchema, PropertyValue, Request, Response, RollupResult, SpaceId,
    TemplatePlaceholder,
};
use serde_json::json;

use crate::documents::{load_doc, map_model_error};
use crate::{Core, map_storage_error, validate};

/// Most pages one export holds.
pub(crate) const MAX_EXPORT_PAGES: usize = 50_000;
/// Most entries `ExportNext` hands out per call.
const MAX_BATCH_ENTRIES: u32 = 500;
/// What an export names a person who is no longer a member of the space (the UI's "Former member").
const FORMER_MEMBER: &str = "Former member";
/// Most rows of a CSV export (the same bound as the pages of an export).
const MAX_CSV_ROWS: usize = MAX_EXPORT_PAGES;
/// Most columns of a CSV export: the title and every property of a full schema.
const MAX_CSV_COLUMNS: usize = 101;
/// Below this many rows a CSV export reads its pages one by one; above, from one pass over the tree.
const CSV_DIRECT_LOOKUPS: usize = 500;

/// A running export (`ExportBegin` to `ExportEnd`).
pub(crate) struct ExportSession {
    id: u32,
    space_id: SpaceId,
    format: ExportFormat,
    export_name: String,
    /// When the session was planned, Unix ms (the sidecar's `exportedAt`).
    started_at: i64,
    layout: Layout,
    /// The pages in tree order, as they were when the session was planned.
    pages: Vec<GatheredPage>,
    /// Titles of the pages of the export.
    titles: HashMap<PageId, String>,
    /// The space's schema when the session was planned.
    names: Names,
    /// The attachments the pages use, with what the device holds of each.
    attachments: HashMap<AttachmentId, UsedAttachment>,
    /// Pages rendered so far.
    next_page: usize,
    /// Entries rendered and not handed out yet (at most one page's group).
    pending: VecDeque<ExportEntry>,
    /// Attachments already written or warned about: each is handled once, with the first page
    /// that uses it.
    handled: HashSet<AttachmentId>,
    /// The sidecar was queued.
    sidecar_queued: bool,
}

/// A page of an export, as read from the structure document.
struct GatheredPage {
    page_id: PageId,
    parent: Option<PageId>,
    title: String,
    icon: Option<String>,
    cover: Option<String>,
    cover_y: Option<u8>,
    /// The page is a template (ADR-0056).
    template: bool,
    /// Its placeholders, by property id, in schema order.
    template_placeholders: Vec<(PropertyId, TemplatePlaceholder)>,
    created_at: i64,
    updated_at: i64,
    values: Vec<(PropertyId, PropertyValue)>,
    /// Attachments the page uses, in order, without duplicates.
    attachments: Vec<AttachmentId>,
    /// Whiteboard blocks, in document order.
    whiteboards: Vec<BlockId>,
}

/// What the device holds of an attachment an export names.
#[derive(Clone)]
struct UsedAttachment {
    name: String,
    state: AttachmentState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttachmentState {
    /// Every chunk is here: the file goes into the archive.
    Available { size: u64 },
    /// Known, but its bytes are not on this device.
    NotDownloaded,
    /// Unknown, deleted, or another space's.
    Missing,
}

/// The names of a space's properties and options.
struct Names {
    schema: PropertySchema,
    by_id: HashMap<PropertyId, usize>,
    options: HashMap<OptionId, String>,
}

impl Names {
    fn new(defs: Vec<PropertyDef>) -> Self {
        let by_id = defs
            .iter()
            .enumerate()
            .map(|(at, def)| (def.id, at))
            .collect();
        let options = defs
            .iter()
            .flat_map(|def| def.options.iter().map(|o| (o.id, o.name.clone())))
            .collect();
        Self {
            schema: PropertySchema { properties: defs },
            by_id,
            options,
        }
    }

    fn property(&self, id: PropertyId) -> Option<&PropertyDef> {
        self.by_id
            .get(&id)
            .and_then(|at| self.schema.properties.get(*at))
    }

    fn option(&self, id: OptionId) -> Option<&str> {
        self.options.get(&id).map(String::as_str)
    }
}

/// The links of the page being written: everything is relative to that page's file.
struct PageLinks<'a> {
    page_id: Option<PageId>,
    /// The archive path of the file being written (empty outside an archive).
    from: &'a str,
    layout: &'a Layout,
    db: &'a Db,
    space_id: SpaceId,
    names: &'a Names,
    titles: &'a HashMap<PageId, String>,
    attachments: &'a HashMap<AttachmentId, UsedAttachment>,
}

impl PageLinks<'_> {
    fn title(&self, page: PageId) -> Option<String> {
        if let Some(title) = self.titles.get(&page) {
            return Some(title.clone());
        }
        crate::pages::title_of(self.db, page)
    }
}

impl ExportLinks for PageLinks<'_> {
    fn page_href(&self, page: PageId) -> Option<String> {
        let to = self.layout.page_files.get(&page)?;
        Some(kb_export::relative_href(self.from, to))
    }

    fn page_title(&self, page: PageId) -> Option<String> {
        self.title(page)
    }

    fn attachment_href(&self, attachment: AttachmentId) -> Option<String> {
        let to = self.layout.attachment_files.get(&attachment)?;
        Some(kb_export::relative_href(self.from, to))
    }

    fn attachment_name(&self, attachment: AttachmentId) -> Option<String> {
        if let Some(used) = self.attachments.get(&attachment) {
            return (used.state != AttachmentState::Missing).then(|| used.name.clone());
        }
        // An attachment the page named since the session was planned.
        let row = self.db.attachment_get(attachment).ok().flatten()?;
        (row.space_id == self.space_id).then_some(row.name)
    }

    fn whiteboard_href(&self, block: BlockId) -> Option<String> {
        let to = self.layout.whiteboard_files.get(&(self.page_id?, block))?;
        Some(kb_export::relative_href(self.from, to))
    }

    fn property_name(&self, property: PropertyId) -> Option<String> {
        self.names.property(property).map(|def| def.name.clone())
    }

    fn option_name(&self, property: PropertyId, option: OptionId) -> Option<String> {
        self.names
            .property(property)?
            .options
            .iter()
            .find(|o| o.id == option)
            .map(|o| o.name.clone())
    }
}

impl Core {
    /// Dispatch entry for `ExportBegin`, `ExportNext`, `ExportEnd` and `ExportCsv`.
    pub(crate) fn dispatch_export(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::ExportBegin {
                space_id,
                page_id,
                include_children,
                format,
            } => Ok(Response::ExportPlanned(self.export_begin(
                space_id,
                page_id,
                include_children,
                format,
            )?)),
            Request::ExportNext {
                export_id,
                max_entries,
            } => Ok(Response::ExportBatch(
                self.export_next(export_id, max_entries)?,
            )),
            Request::ExportEnd { export_id } => {
                if self.export.as_ref().is_some_and(|s| s.id == export_id) {
                    self.export = None;
                }
                Ok(Response::Ok)
            }
            Request::ExportCsv {
                space_id,
                page_ids,
                columns,
            } => Ok(Response::Csv(
                self.export_csv(space_id, &page_ids, &columns)?,
            )),
            other => Err(ApiError::internal(format!(
                "request {} is not an export request",
                other.kind()
            ))),
        }
    }

    // ----- the session ------------------------------------------------------------------

    fn export_begin(
        &mut self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        include_children: bool,
        format: ExportFormat,
    ) -> Result<ExportPlan, ApiError> {
        // The membership names the people of Person values.
        self.shared_load()?;
        let (mut pages, export_name) = self.export_gather(space_id, page_id, include_children)?;
        if pages.len() > MAX_EXPORT_PAGES {
            return Err(ApiError::invalid(
                if page_id.is_some() {
                    "pageId"
                } else {
                    "spaceId"
                },
                format!("an export holds at most {MAX_EXPORT_PAGES} pages"),
            ));
        }
        let names = Names::new(self.structure(space_id)?.model.property_schema());

        // What each page uses (the documents are read, never created).
        for page in &mut pages {
            let (blocks, whiteboards) = self.with_stored_page_model(page.page_id, |model| {
                model.map_or_else(
                    || (Vec::new(), Vec::new()),
                    |model| {
                        let boards = model
                            .blocks()
                            .into_iter()
                            .filter(|block| block.kind == "whiteboard")
                            .map(|block| block.id)
                            .collect();
                        let attachments = model
                            .attachment_refs()
                            .into_iter()
                            .map(|(_, attachment)| attachment)
                            .collect();
                        (attachments, boards)
                    },
                )
            })?;
            let mut used = blocks;
            for (_, value) in &page.values {
                if let PropertyValue::Files(files) = value {
                    used.extend(files.iter().copied());
                }
            }
            if let Some(icon) = page.icon.as_deref().and_then(validate::icon_attachment) {
                used.push(icon);
            }
            if let Some(cover) = page.cover.as_deref().and_then(validate::cover_attachment) {
                used.push(cover);
            }
            let mut seen = HashSet::new();
            used.retain(|attachment| seen.insert(*attachment));
            page.attachments = used;
            page.whiteboards = whiteboards;
        }

        let mut attachments: HashMap<AttachmentId, UsedAttachment> = HashMap::new();
        for page in &pages {
            for attachment in &page.attachments {
                if !attachments.contains_key(attachment) {
                    let used = self.export_attachment_state(space_id, *attachment)?;
                    attachments.insert(*attachment, used);
                }
            }
        }

        let layout_pages: Vec<LayoutPage> = pages
            .iter()
            .map(|page| LayoutPage {
                page_id: page.page_id,
                parent: page.parent,
                title: page.title.clone(),
                attachments: page
                    .attachments
                    .iter()
                    .filter_map(|id| {
                        let used = attachments.get(id)?;
                        Some(LayoutAttachment {
                            attachment_id: *id,
                            name: used.name.clone(),
                            available: matches!(used.state, AttachmentState::Available { .. }),
                        })
                    })
                    .collect(),
                whiteboards: page.whiteboards.clone(),
            })
            .collect();
        let layout = kb_export::layout(&export_name, &layout_pages, format);

        self.export_counter = self.export_counter.saturating_add(1);
        let plan = ExportPlan {
            export_id: self.export_counter,
            pages: count(pages.len()),
            attachments: count(layout.attachment_files.len()),
            single_file: layout.single_file,
            suggested_name: layout.suggested_name.clone(),
        };
        let titles = pages
            .iter()
            .map(|page| (page.page_id, page.title.clone()))
            .collect();
        self.export = Some(ExportSession {
            id: plan.export_id,
            space_id,
            format,
            export_name,
            started_at: self.clock.now_ms(),
            layout,
            pages,
            titles,
            names,
            attachments,
            next_page: 0,
            pending: VecDeque::new(),
            handled: HashSet::new(),
            sidecar_queued: false,
        });
        Ok(plan)
    }

    /// The live pages of the export in tree order, and the export's name (the root page's title,
    /// or the space's name).
    fn export_gather(
        &self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        include_children: bool,
    ) -> Result<(Vec<GatheredPage>, String), ApiError> {
        let model = &self.structure(space_id)?.model;
        let Some(root) = page_id else {
            return Ok((flatten(model.live_tree()), model.space().name));
        };
        let info = model
            .page(root)
            .filter(|info| info.trashed_at.is_none())
            .ok_or_else(|| ApiError::not_found("page not found"))?;
        let name = info.title.clone();
        if !include_children {
            let node = PageNode {
                page: info,
                children: Vec::new(),
            };
            return Ok((flatten(vec![node]), name));
        }
        let mut stack = model.live_tree();
        while let Some(node) = stack.pop() {
            if node.page.id == root {
                return Ok((flatten(vec![node]), name));
            }
            stack.extend(node.children);
        }
        Err(ApiError::not_found("page not found"))
    }

    /// What this device holds of an attachment the export names. Attachments of another space
    /// count as missing: an export never carries them.
    fn export_attachment_state(
        &self,
        space_id: SpaceId,
        attachment: AttachmentId,
    ) -> Result<UsedAttachment, ApiError> {
        match self.attachment_info(attachment) {
            Ok(info) if info.space_id == space_id => Ok(UsedAttachment {
                name: info.name,
                state: if info.availability == AttachmentAvailability::Complete {
                    AttachmentState::Available { size: info.size }
                } else {
                    AttachmentState::NotDownloaded
                },
            }),
            Ok(_) => Ok(UsedAttachment {
                name: String::new(),
                state: AttachmentState::Missing,
            }),
            Err(err) if err.code == kb_types::ErrorCode::NotFound => Ok(UsedAttachment {
                name: String::new(),
                state: AttachmentState::Missing,
            }),
            Err(err) => Err(err),
        }
    }

    fn export_next(&mut self, export_id: u32, max_entries: u32) -> Result<ExportBatch, ApiError> {
        if !(1..=MAX_BATCH_ENTRIES).contains(&max_entries) {
            return Err(ApiError::invalid(
                "maxEntries",
                format!("must be from 1 to {MAX_BATCH_ENTRIES}"),
            ));
        }
        if self.export.as_ref().map(|s| s.id) != Some(export_id) {
            return Err(ApiError::not_found("export session not found"));
        }
        let Some(mut session) = self.export.take() else {
            return Err(ApiError::not_found("export session not found"));
        };
        // The session goes back whatever happens: the shell may retry or end it.
        let result = self.export_step(&mut session, max_entries as usize);
        let total = count(session.pages.len());
        let done = count(session.next_page);
        self.export = Some(session);
        if result.is_ok() {
            self.events.push(Event::ExportProgress {
                export_id,
                done,
                total,
            });
        }
        result
    }

    /// Renders pages until `max` entries are ready, and hands them out.
    fn export_step(
        &self,
        session: &mut ExportSession,
        max: usize,
    ) -> Result<ExportBatch, ApiError> {
        let mut entries: Vec<ExportEntry> = Vec::new();
        let mut warnings: Vec<ExportWarning> = Vec::new();
        while entries.len() < max {
            if let Some(entry) = session.pending.pop_front() {
                entries.push(entry);
                continue;
            }
            if session.next_page < session.pages.len() {
                let (group, problems) = self.export_render_page(session, session.next_page)?;
                session.next_page += 1;
                session.pending.extend(group);
                warnings.extend(problems);
                continue;
            }
            if !session.sidecar_queued
                && let Some(path) = session.layout.sidecar.clone()
            {
                session.sidecar_queued = true;
                let page_files: Vec<String> = session
                    .pages
                    .iter()
                    .filter_map(|page| session.layout.page_files.get(&page.page_id).cloned())
                    .collect();
                let text = kb_export::sidecar::sidecar(
                    &session.export_name,
                    session.started_at,
                    &session.names.schema,
                    &page_files,
                );
                session.pending.push_back(ExportEntry {
                    path,
                    content: ExportContent::Text { text },
                });
                continue;
            }
            break;
        }
        let done = session.pending.is_empty()
            && session.next_page >= session.pages.len()
            && (session.sidecar_queued || session.layout.sidecar.is_none());
        Ok(ExportBatch {
            entries,
            done,
            warnings,
        })
    }

    /// One page as its group of entries: the page file, its whiteboards, then the attachments this
    /// page is the first to use (available ones as entries, the others as warnings).
    fn export_render_page(
        &self,
        session: &mut ExportSession,
        index: usize,
    ) -> Result<(Vec<ExportEntry>, Vec<ExportWarning>), ApiError> {
        let page = session
            .pages
            .get(index)
            .ok_or_else(|| ApiError::internal("export page out of range"))?;
        let page_path = session
            .layout
            .page_files
            .get(&page.page_id)
            .cloned()
            .ok_or_else(|| ApiError::internal("the export layout lacks a page"))?;
        let links = PageLinks {
            page_id: Some(page.page_id),
            from: &page_path,
            layout: &session.layout,
            db: &self.db,
            space_id: session.space_id,
            names: &session.names,
            titles: &session.titles,
            attachments: &session.attachments,
        };
        let devices = self.device_names();
        let mut properties: Vec<ExportProperty> = Vec::new();
        for (property_id, value) in &page.values {
            let Some(def) = session.names.property(*property_id) else {
                continue;
            };
            if def.kind.is_computed() {
                continue;
            }
            if let Some(value) = self.export_value(value, &links, &devices) {
                properties.push(ExportProperty {
                    name: def.name.clone(),
                    tags: *property_id == TAGS_PROPERTY_ID,
                    value,
                });
            }
        }
        let icon = match page.icon.as_deref() {
            Some(icon) => match validate::icon_attachment(icon) {
                // An uploaded icon is written as the link to its file, or left out.
                Some(attachment) => links.attachment_href(attachment),
                None => Some(icon.to_owned()),
            },
            None => None,
        };
        // The cover is written the same way (ADR-0040): a colour or a gradient as stored, an uploaded
        // one as the link to its file, or left out (and its position with it).
        let cover = match page.cover.as_deref() {
            Some(cover) => match validate::cover_attachment(cover) {
                Some(attachment) => links.attachment_href(attachment),
                None => Some(cover.to_owned()),
            },
            None => None,
        };
        let cover_y = cover.as_ref().and(page.cover_y);
        let meta = PageMeta {
            page_id: Some(page.page_id),
            title: page.title.clone(),
            icon,
            cover,
            cover_y,
            template: page.template,
            // By the names of the properties (ids mean nothing in another space), in schema order.
            template_placeholders: page
                .template_placeholders
                .iter()
                .filter_map(|(id, placeholder)| {
                    Some((session.names.property(*id)?.name.clone(), *placeholder))
                })
                .collect(),
            created_at: page.created_at,
            updated_at: page.updated_at,
            properties,
        };
        let content = self.with_stored_page_model(page.page_id, |model| {
            model.map_or_else(|| json!({ "type": "doc" }), PageDoc::to_portable)
        })?;
        let text = match session.format {
            ExportFormat::Markdown => kb_export::markdown::page_markdown(&meta, &content, &links),
            ExportFormat::Html => kb_export::html::page_html(&meta, &content, &links),
        };

        let mut group = vec![ExportEntry {
            path: page_path.clone(),
            content: ExportContent::Text { text },
        }];
        for (block, scene) in kb_export::excalidraw::whiteboard_files(&content) {
            if let Some(path) = session.layout.whiteboard_files.get(&(page.page_id, block)) {
                group.push(ExportEntry {
                    path: path.clone(),
                    content: ExportContent::Text { text: scene },
                });
            }
        }
        let mut warnings = Vec::new();
        for attachment in &page.attachments {
            if !session.handled.insert(*attachment) {
                continue;
            }
            let state = session
                .attachments
                .get(attachment)
                .map_or(AttachmentState::Missing, |used| used.state);
            match state {
                AttachmentState::Available { size } => {
                    if let Some(path) = session.layout.attachment_files.get(attachment) {
                        group.push(ExportEntry {
                            path: path.clone(),
                            content: ExportContent::Attachment {
                                attachment_id: *attachment,
                                size,
                            },
                        });
                    }
                }
                AttachmentState::NotDownloaded => warnings.push(ExportWarning {
                    code: ExportWarningCode::AttachmentNotDownloaded,
                    page_id: Some(page.page_id),
                    attachment_id: Some(*attachment),
                }),
                AttachmentState::Missing => warnings.push(ExportWarning {
                    code: ExportWarningCode::AttachmentMissing,
                    page_id: Some(page.page_id),
                    attachment_id: Some(*attachment),
                }),
            }
        }
        Ok((group, warnings))
    }

    // ----- CSV ----------------------------------------------------------------------------

    /// The rows of a view as CSV (`ExportCsv`, section 2.8).
    fn export_csv(
        &self,
        space_id: SpaceId,
        page_ids: &[PageId],
        columns: &[CsvColumn],
    ) -> Result<String, ApiError> {
        let model = &self.structure(space_id)?.model;
        if page_ids.len() > MAX_CSV_ROWS {
            return Err(ApiError::invalid(
                "pageIds",
                format!("an export holds at most {MAX_CSV_ROWS} rows"),
            ));
        }
        if columns.len() > MAX_CSV_COLUMNS {
            return Err(ApiError::invalid(
                "columns",
                format!("an export holds at most {MAX_CSV_COLUMNS} columns"),
            ));
        }
        let names = Names::new(model.property_schema());
        let mut header = Vec::with_capacity(columns.len());
        for column in columns {
            match column {
                CsvColumn::Title => header.push("Name".to_owned()),
                CsvColumn::Property { property_id } => {
                    let def = names.property(*property_id).ok_or_else(|| {
                        ApiError::invalid("columns", "a column names a property the space lacks")
                    })?;
                    header.push(def.name.clone());
                }
            }
        }

        // The pages as the structure document has them: live ones of this space only.
        let mut tree: HashMap<PageId, PageInfo> = HashMap::new();
        if page_ids.len() > CSV_DIRECT_LOOKUPS {
            let mut stack = model.live_tree();
            while let Some(node) = stack.pop() {
                stack.extend(node.children);
                tree.insert(node.page.id, node.page);
            }
        }
        let devices = self.device_names();
        let titles: HashMap<PageId, String> = HashMap::new();
        let attachments: HashMap<AttachmentId, UsedAttachment> = HashMap::new();
        let layout = Layout::default();
        let links = PageLinks {
            page_id: None,
            from: "",
            layout: &layout,
            db: &self.db,
            space_id,
            names: &names,
            titles: &titles,
            attachments: &attachments,
        };
        let mut rows: Vec<Vec<Option<ExportValue>>> = Vec::with_capacity(page_ids.len());
        for page_id in page_ids {
            let info = if page_ids.len() > CSV_DIRECT_LOOKUPS {
                tree.get(page_id).cloned()
            } else {
                model.page(*page_id)
            };
            let Some(info) = info.filter(|info| info.trashed_at.is_none()) else {
                continue;
            };
            let row = columns
                .iter()
                .map(|column| match column {
                    CsvColumn::Title => Some(ExportValue::Text(info.title.clone())),
                    CsvColumn::Property { property_id } => info
                        .properties
                        .iter()
                        .find(|(id, _)| id == property_id)
                        .and_then(|(_, value)| self.export_value(value, &links, &devices)),
                })
                .collect();
            rows.push(row);
        }
        Ok(kb_export::csv::csv(
            &header,
            &rows,
            self.clock.local_offset_seconds(),
        ))
    }

    // ----- values ---------------------------------------------------------------------------

    /// A property value in the form the exporters write: option ids as names, related pages as
    /// titles with the link to their file when they are part of the export, attachments as names.
    /// `None` for a value that is empty once resolved.
    fn export_value(
        &self,
        value: &PropertyValue,
        links: &PageLinks<'_>,
        devices: &[(DeviceId, String)],
    ) -> Option<ExportValue> {
        Some(match value {
            PropertyValue::Text(text)
            | PropertyValue::Url(text)
            | PropertyValue::Email(text)
            | PropertyValue::Phone(text) => ExportValue::Text(text.clone()),
            PropertyValue::Number(number) => ExportValue::Number(*number),
            PropertyValue::Checkbox(on) => ExportValue::Checkbox(*on),
            PropertyValue::Date(date) => ExportValue::Date {
                date: date.date.clone(),
                time: date.time.clone(),
            },
            PropertyValue::Select(option) | PropertyValue::Status(option) => {
                ExportValue::Options(vec![links.names.option(*option)?.to_owned()])
            }
            PropertyValue::MultiSelect(options) => {
                let names: Vec<String> = options
                    .iter()
                    .filter_map(|option| links.names.option(*option).map(str::to_owned))
                    .collect();
                if names.is_empty() {
                    return None;
                }
                ExportValue::Options(names)
            }
            // The people's names, as the member panel shows them ("Former member" for an account
            // that left), never an account id: the file leaves the space.
            PropertyValue::Person(accounts) => {
                let names: Vec<String> = accounts
                    .iter()
                    .map(|account| {
                        let (name, former) = self.person_in_space(links.space_id, *account);
                        if former {
                            FORMER_MEMBER.to_owned()
                        } else {
                            name
                        }
                    })
                    .collect();
                if names.is_empty() {
                    return None;
                }
                ExportValue::Options(names)
            }
            PropertyValue::Relation(pages) => {
                let linked: Vec<LinkedPage> = pages
                    .iter()
                    .filter_map(|page| {
                        Some(LinkedPage {
                            title: links.title(*page)?,
                            href: links.page_href(*page),
                        })
                    })
                    .collect();
                if linked.is_empty() {
                    return None;
                }
                ExportValue::Pages(linked)
            }
            PropertyValue::Files(files) => {
                let linked: Vec<LinkedFile> = files
                    .iter()
                    .filter_map(|attachment| {
                        Some(LinkedFile {
                            name: links.attachment_name(*attachment)?,
                            href: links.attachment_href(*attachment),
                        })
                    })
                    .collect();
                if linked.is_empty() {
                    return None;
                }
                ExportValue::Files(linked)
            }
            PropertyValue::Checklist(items) => {
                if items.is_empty() {
                    return None;
                }
                ExportValue::Checklist(
                    items
                        .iter()
                        .map(|item| (item.done, item.text.clone()))
                        .collect(),
                )
            }
            PropertyValue::Place(place) => ExportValue::Place {
                name: place.name.clone(),
                lat: place.lat,
                lon: place.lon,
            },
            PropertyValue::Rollup(rollup) => match &rollup.result {
                RollupResult::Number(number) => ExportValue::Number(*number),
                RollupResult::Date(date) => ExportValue::Date {
                    date: date.date.clone(),
                    time: date.time.clone(),
                },
                RollupResult::Values(values) => {
                    let offset = self.clock.local_offset_seconds();
                    let texts: Vec<String> = values
                        .iter()
                        .filter_map(|value| self.export_value(value, links, devices))
                        .map(|value| value.text(offset))
                        .collect();
                    if texts.is_empty() {
                        return None;
                    }
                    ExportValue::Text(texts.join(", "))
                }
                RollupResult::Progress { done, total } => {
                    ExportValue::Text(format!("{done}/{total}"))
                }
            },
            PropertyValue::Formula(result) => return self.export_formula(result, links),
            PropertyValue::CreatedTime(at) | PropertyValue::LastEditedTime(at) => {
                ExportValue::Instant(*at)
            }
            PropertyValue::CreatedBy(device) | PropertyValue::LastEditedBy(device) => {
                ExportValue::Text(
                    devices
                        .iter()
                        .find(|(id, _)| id == device)
                        .map(|(_, name)| name.clone())?,
                )
            }
        })
    }

    /// A formula's result in the form the exporters write (ADR-0055, `formulas.md` section 3.4.2): a
    /// number, text, boolean (as a checkbox) or date as the kind it is, a page by its title (with
    /// its link when it is part of the export), a person by name ("Former member" for one the space
    /// no longer knows); a list of pages or people as those kinds' lists, any other list joined with
    /// `, ` like the rest of the CSV cell texts. `None` for nothing to show.
    fn export_formula(&self, result: &FormulaResult, links: &PageLinks<'_>) -> Option<ExportValue> {
        let person = |account: AccountId| {
            let (name, former) = self.person_in_space(links.space_id, account);
            if former {
                FORMER_MEMBER.to_owned()
            } else {
                name
            }
        };
        let page = |page: PageId| {
            Some(LinkedPage {
                title: links.title(page)?,
                href: links.page_href(page),
            })
        };
        Some(match result {
            FormulaResult::Number(number) => ExportValue::Number(*number),
            FormulaResult::Text(text) => ExportValue::Text(text.clone()),
            FormulaResult::Boolean(on) => ExportValue::Checkbox(*on),
            FormulaResult::Date(date) => ExportValue::Date {
                date: date.date.clone(),
                time: date.time.clone(),
            },
            FormulaResult::Page(id) => ExportValue::Pages(vec![page(*id)?]),
            FormulaResult::Person(account) => ExportValue::Options(vec![person(*account)]),
            FormulaResult::List(items) => match items.first()? {
                FormulaResult::Page(_) => {
                    let linked: Vec<LinkedPage> = items
                        .iter()
                        .filter_map(|item| match item {
                            FormulaResult::Page(id) => page(*id),
                            _ => None,
                        })
                        .collect();
                    if linked.is_empty() {
                        return None;
                    }
                    ExportValue::Pages(linked)
                }
                FormulaResult::Person(_) => ExportValue::Options(
                    items
                        .iter()
                        .filter_map(|item| match item {
                            FormulaResult::Person(account) => Some(person(*account)),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => {
                    let offset = self.clock.local_offset_seconds();
                    let texts: Vec<String> = items
                        .iter()
                        .filter_map(|item| self.export_formula(item, links))
                        .map(|value| value.text(offset))
                        .collect();
                    if texts.is_empty() {
                        return None;
                    }
                    ExportValue::Text(texts.join(", "))
                }
            },
        })
    }

    /// The names of the devices the account knows, this one included.
    fn device_names(&self) -> Vec<(DeviceId, String)> {
        let mut names = self
            .docs
            .account
            .as_ref()
            .map(|account| account.model.device_names())
            .unwrap_or_default();
        if !names.iter().any(|(id, _)| *id == self.device.device_id) {
            names.push((self.device.device_id, self.device.name.clone()));
        }
        names
    }

    // ----- reading content without creating it ------------------------------------------

    /// Runs `f` on a page's stored document: the open replica, or a transient load; `None` when
    /// the page has no stored document (it is never created here).
    fn with_stored_page_model<T>(
        &self,
        page_id: PageId,
        f: impl FnOnce(Option<&PageDoc>) -> T,
    ) -> Result<T, ApiError> {
        if let Some(open) = self.docs.pages.get(&page_id) {
            return Ok(f(Some(&open.model)));
        }
        let Some(row) = self
            .db
            .doc_get(DocId::for_page(page_id))
            .map_err(map_storage_error)?
        else {
            return Ok(f(None));
        };
        let doc = load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
        let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
        Ok(f(Some(&model)))
    }
}

/// The pages of the trees in pre-order, each with its parent in the export.
fn flatten(roots: Vec<PageNode>) -> Vec<GatheredPage> {
    let mut out = Vec::new();
    let mut stack: Vec<(PageNode, Option<PageId>)> =
        roots.into_iter().rev().map(|node| (node, None)).collect();
    while let Some((node, parent)) = stack.pop() {
        let id = node.page.id;
        stack.extend(
            node.children
                .into_iter()
                .rev()
                .map(|child| (child, Some(id))),
        );
        let info = node.page;
        out.push(GatheredPage {
            page_id: info.id,
            parent,
            title: info.title,
            icon: info.icon,
            cover: info.cover,
            cover_y: info.cover_y,
            template: info.template,
            template_placeholders: info.template_placeholders,
            created_at: info.created_at,
            updated_at: info.updated_at,
            values: info.properties,
            attachments: Vec::new(),
            whiteboards: Vec::new(),
        });
    }
    out
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
