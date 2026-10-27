//! Import sessions (ADR-0036, `docs/architecture/import-export.md`, "Import"): the core collects
//! the source's text files, plans with `kb-import`, matches or creates the properties, creates
//! the pages and writes their content.
//!
//! The protocol is `ImportBegin`, `ImportAddFile` per text file, `ImportPlan`, `ImportAddAttachment`
//! per uploaded file, then `ImportCommit` until it answers `done`. The first commit creates the
//! properties the pages need and **every page in the structure** (one persisted update), so that
//! all links can resolve; every commit then writes up to `maxPages` page documents
//! (`kb_import::resolve`, a fresh document from the portable JSON, its derived rows in one
//! transaction) and their property values (coerced to each property's kind, one persisted
//! update). A session lives in memory only; a failure drops it and what was written stays.
//!
//! `ImportBeginSpaces` (ADR-0039, `docs/architecture/appflowy-import.md` section 5) opens a session
//! whose destination is new spaces. `ImportPlan` then creates one space per space root of the plan
//! (a source without spaces gets one, named after the source), every planned page belongs to the
//! space of its root ancestor, the roots themselves are not pages, and the first commit creates
//! the properties and the pages **per space**. The resolver works across the session's spaces.
//! Aborting or failing deletes the session's spaces that hold no page.

use std::collections::{BTreeSet, HashMap, HashSet};

use kb_doc::{Doc, DocKind};
use kb_import::{
    ImportOptions, ImportValue, PageTarget, Plan, PlannedConfig, PlannedCover, PlannedOption,
    PlannedPage, PlannedProperty, PlannedRollupTarget, Resolver, SourceFile,
};
use kb_keys::KeyringItem;
use kb_model::formula;
use kb_model::{ImportedTemplate, ModelError, NewPage, PageDoc, StructureDoc};
use kb_storage::{
    AttachmentQueries, DateMentionQueries, DocKindCode, DocQueries, JournalOp, SecurityQueries,
    TextIndexQueries, UpdateOrigin,
};
use kb_types::properties::{
    FORMULA_TITLE_ID, MAX_CHECKLIST_ITEMS, MAX_CHECKLIST_TEXT_CHARS, MAX_FILES, MAX_FORMULA_CHARS,
    MAX_NAME_CHARS, MAX_OPTIONS, MAX_PLACE_NAME_CHARS, MAX_PROPERTIES, MAX_RELATED_PAGES,
    MAX_SELECTED_OPTIONS, MAX_TEXT_CHARS,
};
use kb_types::{
    ApiError, AttachmentAvailability, AttachmentId, BlockId, ChangeSource, ChecklistItem,
    ChecklistItemId, DateValue, DocId, Event, ImportPlanInfo, ImportProgress, ImportRemoteFile,
    ImportReport, ImportSpaceInfo, ImportWarning, ImportWarningCode, MemberRole, OptionColor,
    OptionId, PageId, PlaceValue, PropertyConfig, PropertyDef, PropertyId, PropertyKind,
    PropertyValue, Request, Response, Rng, RollupTarget, SpaceId, StatusGroup,
};
use serde_json::Value;

use crate::documents::{map_doc_error, map_model_error, new_doc_row, new_peer};
use crate::pages::{TextEntry, text_entry_of};
use crate::properties::{PropsScope, back_property_name};
use crate::{Core, map_storage_error, validate};

/// Most text files of one import.
const MAX_FILES_PER_IMPORT: usize = 10_000;
/// Largest text file, in bytes.
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
/// Most bytes of text files in all.
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;
/// Longest source path, in bytes.
const MAX_PATH_BYTES: usize = 1_024;
/// Most pages one import creates.
const MAX_PLANNED_PAGES: usize = 10_000;
/// Most binary paths one plan takes.
const MAX_BINARY_PATHS: usize = 100_000;
/// Most pages one `ImportCommit` writes.
const MAX_COMMIT_PAGES: u32 = 500;
/// Most warnings the report lists.
const MAX_REPORT_WARNINGS: usize = 500;
/// Longest page title, in characters.
const MAX_TITLE_CHARS: usize = 500;

/// A running import (`ImportBegin` to the last `ImportCommit` or `ImportAbort`).
pub(crate) struct ImportSession {
    id: u32,
    dest: Destination,
    source_name: Option<String>,
    stage: Stage,
}

/// Where a session's pages go.
#[derive(Clone, Copy)]
enum Destination {
    /// Under a page, or the root, of an existing space (`ImportBegin`).
    Page {
        space_id: SpaceId,
        parent_id: Option<PageId>,
    },
    /// Into spaces the plan creates (`ImportBeginSpaces`).
    NewSpaces,
}

enum Stage {
    /// Text files are being added.
    Collecting(Collected),
    /// The plan is made; the texts are gone.
    Planned(Box<Planned>),
}

#[derive(Default)]
struct Collected {
    files: Vec<SourceFile>,
    paths: HashSet<String>,
    bytes: usize,
}

struct Planned {
    plan: Plan,
    /// Which space each page and each file belongs to.
    layout: Layout,
    /// The attachment reported for each needed path or remote key, with the space it is in.
    attachments: HashMap<String, Reported>,
    plan_warnings: WarningList,
    resolve_warnings: WarningList,
    coerce_warnings: WarningList,
    /// Set by the first `ImportCommit`.
    started: Option<Started>,
}

/// An attachment the shell reported for a needed path or a remote key.
#[derive(Clone, Copy)]
struct Reported {
    id: AttachmentId,
    space_id: SpaceId,
}

/// How the plan's pages and files are spread over the session's spaces. For an `ImportBegin`
/// session everything is in its one space.
struct Layout {
    /// The spaces the pages go into, in order.
    spaces: Vec<SpaceId>,
    /// The spaces this session created (`ImportBeginSpaces`), in order; none otherwise.
    created: Vec<SpaceId>,
    /// The space of each planned page (of a space root: its own space), parallel to `Plan::pages`.
    page_space: Vec<SpaceId>,
    /// Whether each planned page is a space root, which is not created as a page.
    is_root: Vec<bool>,
    /// The planned pages that are created, in plan order.
    order: Vec<usize>,
    /// The space each needed path and each remote key is uploaded into.
    file_space: HashMap<String, SpaceId>,
    /// The space whose live pages wiki links and relation values may name by title (`ImportBegin`
    /// only: a new space holds nothing yet).
    titles_from: Option<SpaceId>,
    /// The planned property names each space's pages use, when the pages are spread over several
    /// spaces (a space gets the properties its own pages use); `None`: every planned property.
    used_properties: Option<HashMap<SpaceId, HashSet<String>>>,
}

impl Layout {
    /// Everything in one existing space.
    fn single(space_id: SpaceId, plan: &Plan) -> Self {
        let mut file_space = HashMap::new();
        for path in &plan.needed_files {
            file_space.insert(path.clone(), space_id);
        }
        for remote in &plan.remote_files {
            file_space.insert(remote.key.clone(), space_id);
        }
        Self {
            spaces: vec![space_id],
            created: Vec::new(),
            page_space: vec![space_id; plan.pages.len()],
            is_root: vec![false; plan.pages.len()],
            order: (0..plan.pages.len()).collect(),
            file_space,
            titles_from: Some(space_id),
            used_properties: None,
        }
    }

    /// Pages created in the session's spaces, per space.
    fn pages_in(&self, space_id: SpaceId) -> u32 {
        let count = self
            .order
            .iter()
            .filter(|index| self.page_space.get(**index) == Some(&space_id))
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }
}

/// What the first commit made, and how far the writing got.
struct Started {
    /// The id of each planned page, parallel to `Plan::pages`; `None` for a space root.
    page_ids: Vec<Option<PageId>>,
    /// The cleaned titles the pages were created with.
    page_titles: Vec<String>,
    /// Live pages of the destination space before the import: title to the page, `None` when
    /// several have it (wiki links and relation values by title resolve against these).
    existing_titles: HashMap<String, Option<PageId>>,
    /// The properties of each space, matched or created.
    props: HashMap<SpaceId, ImportProps>,
    properties_created: u32,
    /// Pages of `Layout::order` whose documents are written.
    next: usize,
    /// Attachments the written pages use.
    used_attachments: HashSet<AttachmentId>,
}

/// Warnings of one stage: the first ones are kept, the rest counted.
#[derive(Default)]
struct WarningList {
    kept: Vec<ImportWarning>,
    more: u32,
}

impl WarningList {
    fn push(&mut self, warning: ImportWarning) {
        if self.kept.len() < MAX_REPORT_WARNINGS {
            self.kept.push(warning);
        } else {
            self.more = self.more.saturating_add(1);
        }
    }

    fn extend(&mut self, warnings: impl IntoIterator<Item = ImportWarning>) {
        for warning in warnings {
            self.push(warning);
        }
    }

    fn note(&mut self, path: &str, code: ImportWarningCode, detail: Option<&str>) {
        self.push(ImportWarning {
            path: path.to_owned(),
            code,
            detail: detail.map(str::to_owned),
        });
    }
}

// ----- properties ----------------------------------------------------------------------------

/// The space's properties as the import sees them: matched by name (Unicode lower-casing),
/// created as planned, with their options.
struct ImportProps {
    by_name: HashMap<String, PropertyId>,
    info: HashMap<PropertyId, PropInfo>,
    /// Planned name to the property that serves it (`None`: left out, the space is full).
    planned: HashMap<String, Option<PropertyId>>,
}

struct PropInfo {
    kind: PropertyKind,
    /// Folded name and id of each option, in order.
    options: Vec<(String, OptionId)>,
    /// Colour of the last option (the next one gets the one after it).
    last_color: Option<OptionColor>,
}

impl ImportProps {
    fn from_schema(defs: &[PropertyDef]) -> Self {
        let mut props = Self {
            by_name: HashMap::new(),
            info: HashMap::new(),
            planned: HashMap::new(),
        };
        for def in defs {
            props.add_property(def.id, &def.name, def.kind);
            for option in &def.options {
                props.add_option(def.id, &option.name, option.id, option.color);
            }
        }
        props
    }

    fn add_property(&mut self, id: PropertyId, name: &str, kind: PropertyKind) {
        // The first property of a name (in schema order) serves it.
        self.by_name.entry(fold(name)).or_insert(id);
        self.info.insert(
            id,
            PropInfo {
                kind,
                options: Vec::new(),
                last_color: None,
            },
        );
    }

    fn add_option(&mut self, property: PropertyId, name: &str, id: OptionId, color: OptionColor) {
        if let Some(info) = self.info.get_mut(&property) {
            info.options.push((fold(name), id));
            info.last_color = Some(color);
        }
    }

    fn find(&self, name: &str) -> Option<PropertyId> {
        self.by_name.get(&fold(name)).copied()
    }

    fn kind(&self, property: PropertyId) -> Option<PropertyKind> {
        self.info.get(&property).map(|info| info.kind)
    }

    fn option(&self, property: PropertyId, name: &str) -> Option<OptionId> {
        let folded = fold(name);
        self.info
            .get(&property)?
            .options
            .iter()
            .find(|(known, _)| *known == folded)
            .map(|(_, id)| *id)
    }

    /// The palette colour after the last option's (gray for the first), as the editor picks it.
    fn next_color(&self, property: PropertyId) -> OptionColor {
        let Some(last) = self.info.get(&property).and_then(|info| info.last_color) else {
            return OptionColor::Gray;
        };
        let at = OptionColor::ALL
            .iter()
            .position(|color| *color == last)
            .unwrap_or(0);
        OptionColor::ALL[(at + 1) % OptionColor::ALL.len()]
    }
}

/// Lower-cases a name for matching (Unicode lower-casing: the simple case folding the design
/// asks for, without a table of its own).
fn fold(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

/// Whether a formula's `prop("...")` names the page title: the language-independent `title` the export
/// writes (the core has no translations; a property of the space called so wins, as the caller looks
/// properties up first; ADR-0055, `formulas.md` section 6).
fn is_title_name(name: &str) -> bool {
    name == FORMULA_TITLE_ID
}

/// The planned properties the import makes, in the order it makes them: every kind that reads no
/// other property first, then the rollups (they read a relation and its target), then the formulas
/// (they read any property, formulas included, so one follows the formulas it reads; a loop, or a
/// formula that reads itself, is left in the file's order and stays broken).
fn import_order<'a>(
    properties: &'a [PlannedProperty],
    uses: &dyn Fn(&str) -> bool,
) -> Vec<&'a PlannedProperty> {
    let wanted: Vec<&PlannedProperty> = properties.iter().filter(|p| uses(&p.name)).collect();
    let mut ordered: Vec<&PlannedProperty> = wanted
        .iter()
        .copied()
        .filter(|p| !matches!(p.kind, PropertyKind::Rollup | PropertyKind::Formula))
        .collect();
    ordered.extend(
        wanted
            .iter()
            .copied()
            .filter(|p| p.kind == PropertyKind::Rollup),
    );
    let mut pending: Vec<&PlannedProperty> = wanted
        .iter()
        .copied()
        .filter(|p| p.kind == PropertyKind::Formula)
        .collect();
    while !pending.is_empty() {
        let waiting: HashSet<String> = pending.iter().map(|p| fold(&p.name)).collect();
        let (ready, rest): (Vec<&PlannedProperty>, Vec<&PlannedProperty>) =
            pending.into_iter().partition(|p| {
                let Some(PlannedConfig::Formula { expression }) = &p.config else {
                    return true;
                };
                let own = fold(&p.name);
                formula::referenced_ids(expression)
                    .iter()
                    .map(|name| fold(name))
                    .all(|name| name == own || !waiting.contains(&name))
            });
        if ready.is_empty() {
            ordered.extend(rest);
            break;
        }
        ordered.extend(ready);
        pending = rest;
    }
    ordered
}

/// Control characters become spaces.
fn spaced(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A property or option name made valid: control characters become spaces, the ends are
/// trimmed and the length is cut to the limit.
fn clean_name(raw: &str) -> String {
    let cut: String = spaced(raw).trim().chars().take(MAX_NAME_CHARS).collect();
    cut.trim().to_owned()
}

/// A page title made valid: control characters become spaces, trimmed, at most 500 characters.
fn clean_title(raw: &str) -> String {
    let cut: String = spaced(raw).trim().chars().take(MAX_TITLE_CHARS).collect();
    cut.trim().to_owned()
}

/// Text on one line, at most `max` characters (a value's own limit).
fn one_line(raw: &str, max: usize) -> String {
    spaced(raw).trim().chars().take(max).collect()
}

// ----- the resolver ---------------------------------------------------------------------------

/// What `kb_import::resolve` asks the core, for one page: `props` are the properties and options
/// of the page's space, `spaces` the session's spaces (an existing page or attachment may be in
/// any of them, and a file uploaded into one of them is used from any other).
struct CoreResolver<'a> {
    db: &'a kb_storage::Db,
    spaces: &'a [SpaceId],
    page_ids: &'a [Option<PageId>],
    existing_titles: &'a HashMap<String, Option<PageId>>,
    attachments: &'a HashMap<String, Reported>,
    props: &'a ImportProps,
}

impl Resolver for CoreResolver<'_> {
    fn page(&self, index: usize) -> Option<PageId> {
        self.page_ids.get(index).copied().flatten()
    }

    fn page_by_title(&self, title: &str) -> Option<PageId> {
        self.existing_titles.get(title).copied().flatten()
    }

    fn existing_page(&self, page: PageId) -> bool {
        self.db
            .pages_index_get(page)
            .ok()
            .flatten()
            .is_some_and(|row| self.spaces.contains(&row.space_id) && row.trashed_at.is_none())
    }

    fn attachment(&self, path: &str) -> Option<AttachmentId> {
        // A file is uploaded once, into the space of the first page that uses it; pages of the
        // session's other spaces reference it across spaces, as a cross-space paste does (ADR-0020,
        // ADR-0039). Moving such a page later leaves the attachment where it is (ADR-0038).
        self.attachments.get(path).map(|reported| reported.id)
    }

    fn existing_attachment(&self, attachment: AttachmentId) -> bool {
        self.db
            .attachment_get(attachment)
            .ok()
            .flatten()
            .is_some_and(|row| self.spaces.contains(&row.space_id))
    }

    fn property(&self, name: &str) -> Option<PropertyId> {
        self.props.find(name)
    }

    fn option(&self, property: &str, option: &str) -> Option<OptionId> {
        self.props.option(self.props.find(property)?, option)
    }
}

// ----- requests --------------------------------------------------------------------------------

impl Core {
    /// Dispatch entry for the `Import*` requests.
    pub(crate) fn dispatch_import(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::ImportBegin {
                space_id,
                parent_id,
                source_name,
            } => Ok(Response::ImportStarted(self.import_begin(
                space_id,
                parent_id,
                source_name,
            )?)),
            Request::ImportBeginSpaces { source_name } => Ok(Response::ImportStarted(
                self.import_begin_spaces(source_name)?,
            )),
            Request::ImportAddFile {
                import_id,
                path,
                text,
            } => {
                self.import_add_file(import_id, path, text)?;
                Ok(Response::Ok)
            }
            Request::ImportPlan {
                import_id,
                binary_paths,
            } => Ok(Response::ImportPlanned(
                self.import_plan(import_id, &binary_paths)?,
            )),
            Request::ImportAddAttachment {
                import_id,
                path,
                attachment_id,
            } => {
                self.import_add_attachment(import_id, &path, attachment_id)?;
                Ok(Response::Ok)
            }
            Request::ImportCommit {
                import_id,
                max_pages,
            } => Ok(Response::ImportProgress(
                self.import_commit(import_id, max_pages)?,
            )),
            Request::ImportAbort { import_id } => {
                if self.import.as_ref().is_some_and(|s| s.id == import_id)
                    && let Some(session) = self.import.take()
                {
                    self.import_discard(&session);
                }
                Ok(Response::Ok)
            }
            other => Err(ApiError::internal(format!(
                "request {} is not an import request",
                other.kind()
            ))),
        }
    }

    /// `ImportBeginSpaces` (ADR-0039, `docs/architecture/appflowy-import.md` section 5): a session
    /// whose destination is new spaces, created by `ImportPlan`.
    fn import_begin_spaces(&mut self, source_name: Option<String>) -> Result<u32, ApiError> {
        self.import_open(Destination::NewSpaces, source_name)
    }

    fn import_begin(
        &mut self,
        space_id: SpaceId,
        parent_id: Option<PageId>,
        source_name: Option<String>,
    ) -> Result<u32, ApiError> {
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let model = &self.structure(space_id)?.model;
        if let Some(parent) = parent_id {
            let info = model
                .page(parent)
                .ok_or_else(|| ApiError::not_found("page not found"))?;
            if info.trashed_at.is_some() {
                return Err(ApiError::invalid("parentId", "the page is in the trash"));
            }
        }
        self.import_open(
            Destination::Page {
                space_id,
                parent_id,
            },
            source_name,
        )
    }

    /// Starts a session, replacing (and cleaning up after) the one that was running.
    fn import_open(
        &mut self,
        dest: Destination,
        source_name: Option<String>,
    ) -> Result<u32, ApiError> {
        let source_name = source_name
            .map(|name| clean_title(&name))
            .filter(|name| !name.is_empty());
        if let Some(old) = self.import.take() {
            self.import_discard(&old);
        }
        self.import_counter = self.import_counter.saturating_add(1);
        self.import = Some(ImportSession {
            id: self.import_counter,
            dest,
            source_name,
            stage: Stage::Collecting(Collected::default()),
        });
        Ok(self.import_counter)
    }

    /// Drops what an ended session leaves behind: the spaces it created that hold no page (live
    /// or in the trash), with their attachments. Spaces with pages stay, as do pages already
    /// written. Best effort: a space that cannot be deleted stays, and nothing is logged about
    /// its name.
    fn import_discard(&mut self, session: &ImportSession) {
        let Stage::Planned(planned) = &session.stage else {
            return;
        };
        for space_id in planned.layout.created.clone() {
            let empty = self.docs.structures.get(&space_id).is_some_and(|open| {
                open.model.live_page_count() == 0 && open.model.trashed_page_count() == 0
            });
            if empty && let Err(error) = self.space_delete(space_id) {
                tracing::debug!(code = ?error.code, "an empty space of an import could not be deleted");
            }
        }
    }

    fn import_session_mut(&mut self, import_id: u32) -> Result<&mut ImportSession, ApiError> {
        self.import
            .as_mut()
            .filter(|session| session.id == import_id)
            .ok_or_else(|| ApiError::not_found("import session not found"))
    }

    fn import_add_file(
        &mut self,
        import_id: u32,
        path: String,
        text: String,
    ) -> Result<(), ApiError> {
        let session = self.import_session_mut(import_id)?;
        let Stage::Collecting(collected) = &mut session.stage else {
            return Err(ApiError::invalid(
                "importId",
                "the import is already planned; no more files can be added",
            ));
        };
        if !valid_source_path(&path) {
            return Err(ApiError::invalid(
                "path",
                "must be a relative path of at most 1,024 bytes with no empty, . or .. segment",
            ));
        }
        if text.len() > MAX_FILE_BYTES {
            return Err(ApiError::invalid(
                "text",
                format!("a file is at most {MAX_FILE_BYTES} bytes"),
            ));
        }
        if collected.files.len() >= MAX_FILES_PER_IMPORT {
            return Err(ApiError::invalid(
                "path",
                format!("an import takes at most {MAX_FILES_PER_IMPORT} text files"),
            ));
        }
        if collected.bytes.saturating_add(text.len()) > MAX_TOTAL_BYTES {
            return Err(ApiError::invalid(
                "text",
                format!("an import takes at most {MAX_TOTAL_BYTES} bytes of text"),
            ));
        }
        if !collected.paths.insert(path.clone()) {
            return Err(ApiError::invalid("path", "this file was already added"));
        }
        collected.bytes += text.len();
        collected.files.push(SourceFile { path, text });
        Ok(())
    }

    fn import_plan(
        &mut self,
        import_id: u32,
        binary_paths: &[String],
    ) -> Result<ImportPlanInfo, ApiError> {
        if binary_paths.len() > MAX_BINARY_PATHS {
            return Err(ApiError::invalid(
                "binaryPaths",
                format!("at most {MAX_BINARY_PATHS} paths"),
            ));
        }
        if !binary_paths.iter().all(|path| valid_source_path(path)) {
            return Err(ApiError::invalid(
                "binaryPaths",
                "every path must be relative with no empty, . or .. segment",
            ));
        }
        let session = self.import_session_mut(import_id)?;
        let Stage::Collecting(collected) = &mut session.stage else {
            return Err(ApiError::invalid(
                "importId",
                "the import is already planned",
            ));
        };
        let files = std::mem::take(&mut collected.files);
        let options = ImportOptions {
            source_name: session.source_name.clone(),
            max_pages: MAX_PLANNED_PAGES,
        };
        // The texts are not kept: the plan holds what the pages need.
        let plan = kb_import::plan(files, binary_paths, &options);
        self.import_install_plan(import_id, plan)
    }

    /// Keeps a plan in the session and answers what it needs. For an `ImportBeginSpaces` session
    /// the spaces are created here.
    fn import_install_plan(
        &mut self,
        import_id: u32,
        plan: Plan,
    ) -> Result<ImportPlanInfo, ApiError> {
        if !plan_is_consistent(&plan) {
            self.import = None;
            return Err(ApiError::internal("the import plan is inconsistent"));
        }
        let (dest, source_name) = {
            let session = self.import_session_mut(import_id)?;
            (session.dest, session.source_name.clone())
        };
        let (layout, spaces) = match dest {
            Destination::Page { space_id, .. } => (Layout::single(space_id, &plan), Vec::new()),
            Destination::NewSpaces => {
                match self.import_create_spaces(&plan, source_name.as_deref()) {
                    Ok(made) => made,
                    Err(error) => {
                        self.import = None;
                        return Err(error);
                    }
                }
            }
        };
        let spread = matches!(dest, Destination::NewSpaces);

        let mut needed_files: Vec<String> = Vec::with_capacity(plan.needed_files.len());
        let mut file_spaces: Vec<SpaceId> = Vec::new();
        let mut file_names: Vec<String> = Vec::new();
        for (at, path) in plan.needed_files.iter().enumerate() {
            if let Some(space_id) = layout.file_space.get(path) {
                needed_files.push(path.clone());
                file_spaces.push(*space_id);
                // The source's own name for the file where it gives one, else the last segment of
                // the path; none at all for a source that names no file (the plan's list is empty).
                if !plan.needed_file_names.is_empty() {
                    file_names.push(
                        plan.needed_file_names
                            .get(at)
                            .cloned()
                            .flatten()
                            .filter(|name| !name.trim().is_empty())
                            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned()),
                    );
                }
            }
        }
        let remote_files: Vec<ImportRemoteFile> = plan
            .remote_files
            .iter()
            .filter_map(|remote| {
                layout
                    .file_space
                    .get(&remote.key)
                    .map(|space_id| ImportRemoteFile {
                        key: remote.key.clone(),
                        url: remote.url.clone(),
                        host: remote.host.clone(),
                        space_id: *space_id,
                    })
            })
            .collect();
        let info = ImportPlanInfo {
            pages: u32::try_from(layout.order.len()).unwrap_or(u32::MAX),
            needed_files,
            warnings: plan.warnings.clone(),
            spaces,
            file_spaces: if spread { file_spaces } else { Vec::new() },
            file_names,
            remote_files,
        };
        let mut plan_warnings = WarningList::default();
        plan_warnings.extend(plan.warnings.iter().cloned());
        let session = self.import_session_mut(import_id)?;
        session.stage = Stage::Planned(Box::new(Planned {
            plan,
            layout,
            attachments: HashMap::new(),
            plan_warnings,
            resolve_warnings: WarningList::default(),
            coerce_warnings: WarningList::default(),
            started: None,
        }));
        Ok(info)
    }

    /// Creates the spaces of an `ImportBeginSpaces` plan (ADR-0039): one per space root, named
    /// and iconed after it, in order, appended to the account's spaces, and one more named after
    /// the source (or "Imported") for pages that have no space root above them, which is the only
    /// space of a plan without roots. A plan without pages makes none. On a failure the spaces
    /// made so far are deleted (they are empty).
    fn import_create_spaces(
        &mut self,
        plan: &Plan,
        source_name: Option<&str>,
    ) -> Result<(Layout, Vec<ImportSpaceInfo>), ApiError> {
        let count = plan.pages.len();
        let mut is_root = vec![false; count];
        for &index in &plan.space_roots {
            is_root[index] = true;
        }
        // The group (space) of each page: its own for a root, else its parent's; a page with
        // none above it is in the default group, after the roots'.
        let mut group_of: Vec<usize> = vec![0; count];
        for (group, &index) in plan.space_roots.iter().enumerate() {
            group_of[index] = group;
        }
        let default_group = plan.space_roots.len();
        let mut orphans = false;
        for (index, page) in plan.pages.iter().enumerate() {
            if is_root[index] {
                continue;
            }
            group_of[index] = match page.parent {
                Some(parent) => group_of[parent],
                None => default_group,
            };
            orphans |= group_of[index] == default_group;
        }
        let mut groups: Vec<(String, Option<String>)> = plan
            .space_roots
            .iter()
            .map(|&root| {
                let page = &plan.pages[root];
                let icon = page
                    .icon
                    .as_deref()
                    .and_then(|icon| validate::space_icon(Some(icon)).ok().flatten());
                (space_name(&page.title), icon)
            })
            .collect();
        if orphans {
            groups.push((space_name(source_name.unwrap_or("Imported")), None));
        }

        let mut created: Vec<SpaceId> = Vec::with_capacity(groups.len());
        let mut names: Vec<String> = Vec::with_capacity(groups.len());
        for (name, icon) in &groups {
            match self.space_create(name, icon.as_deref()) {
                Ok(info) => {
                    created.push(info.id);
                    names.push(info.name);
                }
                Err(error) => {
                    for space_id in created {
                        if let Err(cleanup) = self.space_delete(space_id) {
                            tracing::debug!(
                                code = ?cleanup.code,
                                "an empty space of an import could not be deleted"
                            );
                        }
                    }
                    return Err(error);
                }
            }
        }

        let page_space: Vec<SpaceId> = group_of.iter().map(|group| created[*group]).collect();
        let order: Vec<usize> = (0..count).filter(|index| !is_root[*index]).collect();
        let mut layout = Layout {
            spaces: created.clone(),
            created: created.clone(),
            page_space,
            is_root,
            order,
            file_space: HashMap::new(),
            titles_from: None,
            used_properties: None,
        };
        layout.place_files(plan);
        if created.len() > 1 {
            layout.used_properties = Some(used_properties(plan, &layout));
        }
        let spaces = created
            .iter()
            .zip(names)
            .map(|(space_id, name)| ImportSpaceInfo {
                space_id: *space_id,
                name,
                pages: layout.pages_in(*space_id),
            })
            .collect();
        Ok((layout, spaces))
    }

    fn import_add_attachment(
        &mut self,
        import_id: u32,
        path: &str,
        attachment_id: AttachmentId,
    ) -> Result<(), ApiError> {
        self.import_session_mut(import_id)?;
        let info = self.attachment_info(attachment_id)?;
        let session = self.import_session_mut(import_id)?;
        let Stage::Planned(planned) = &mut session.stage else {
            return Err(ApiError::invalid("importId", "plan the import first"));
        };
        // A needed path or a remote key, and the space the plan gave it.
        let Some(space_id) = planned.layout.file_space.get(path).copied() else {
            return Err(ApiError::invalid(
                "path",
                "the plan does not need this file",
            ));
        };
        if info.space_id != space_id {
            return Err(ApiError::invalid(
                "attachmentId",
                "the attachment belongs to another space",
            ));
        }
        if info.availability != AttachmentAvailability::Complete {
            return Err(ApiError::invalid(
                "attachmentId",
                "the attachment is not complete",
            ));
        }
        planned.attachments.insert(
            path.to_owned(),
            Reported {
                id: attachment_id,
                space_id,
            },
        );
        Ok(())
    }

    fn import_commit(
        &mut self,
        import_id: u32,
        max_pages: u32,
    ) -> Result<ImportProgress, ApiError> {
        if !(1..=MAX_COMMIT_PAGES).contains(&max_pages) {
            return Err(ApiError::invalid(
                "maxPages",
                format!("must be from 1 to {MAX_COMMIT_PAGES}"),
            ));
        }
        if matches!(
            self.import_session_mut(import_id)?.stage,
            Stage::Collecting(_)
        ) {
            return Err(ApiError::invalid("importId", "plan the import first"));
        }
        let Some(mut session) = self.import.take() else {
            return Err(ApiError::not_found("import session not found"));
        };
        // A failure drops the session: what was written stays (and so do the spaces that hold a
        // page of it), and a retry would not know which pages are done.
        let progress = match self.import_step(&mut session, max_pages as usize) {
            Ok(progress) => progress,
            Err(error) => {
                self.import_discard(&session);
                return Err(error);
            }
        };
        self.events.push(Event::ImportProgress {
            import_id,
            done: progress.created,
            total: progress.total,
        });
        if !progress.done {
            self.import = Some(session);
        }
        Ok(progress)
    }

    fn import_step(
        &mut self,
        session: &mut ImportSession,
        max_pages: usize,
    ) -> Result<ImportProgress, ApiError> {
        let dest = session.dest;
        // The role may have changed since the session began (ADR-0043).
        if let Destination::Page { space_id, .. } = dest {
            self.require_role(space_id, MemberRole::Editor)?;
        }
        let Stage::Planned(planned) = &mut session.stage else {
            return Err(ApiError::invalid("importId", "plan the import first"));
        };
        if planned.started.is_none() {
            self.import_start(dest, planned)?;
        }
        self.import_batch(planned, max_pages)?;

        let total = planned.layout.order.len();
        let Some(started) = planned.started.as_ref() else {
            return Err(ApiError::internal("the import did not start"));
        };
        let created = u32::try_from(started.next).unwrap_or(u32::MAX);
        let total_u32 = u32::try_from(total).unwrap_or(u32::MAX);
        if started.next < total {
            return Ok(ImportProgress {
                created,
                total: total_u32,
                done: false,
                report: None,
            });
        }
        // The pages directly under the destination: no parent, or (new spaces) a space root.
        let root_page_ids = planned
            .layout
            .order
            .iter()
            .filter(|index| {
                planned.plan.pages.get(**index).is_some_and(|page| {
                    page.parent
                        .is_none_or(|parent| planned.layout.is_root[parent])
                })
            })
            .filter_map(|index| started.page_ids.get(*index).copied().flatten())
            .collect();
        let mut warnings: Vec<ImportWarning> = Vec::new();
        let mut more = 0u32;
        for list in [
            &mut planned.plan_warnings,
            &mut planned.resolve_warnings,
            &mut planned.coerce_warnings,
        ] {
            more = more.saturating_add(list.more);
            for warning in std::mem::take(&mut list.kept) {
                if warnings.len() < MAX_REPORT_WARNINGS {
                    warnings.push(warning);
                } else {
                    more = more.saturating_add(1);
                }
            }
        }
        Ok(ImportProgress {
            created,
            total: total_u32,
            done: true,
            report: Some(ImportReport {
                root_page_ids,
                pages: created,
                attachments: u32::try_from(started.used_attachments.len()).unwrap_or(u32::MAX),
                properties_created: started.properties_created,
                warnings,
                more_warnings: more,
                space_ids: planned.layout.created.clone(),
            }),
        })
    }

    // ----- the first commit: properties and the pages in the structure ------------------------

    fn import_start(&mut self, dest: Destination, planned: &mut Planned) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        // Pages without a parent go under the destination page (or the space root); in new
        // spaces they are root pages.
        let parent_id = match dest {
            Destination::Page { parent_id, .. } => parent_id,
            Destination::NewSpaces => None,
        };
        // Titles of the live pages that exist before the import (an existing destination only).
        let mut existing_titles: HashMap<String, Option<PageId>> = HashMap::new();
        if let Some(space_id) = planned.layout.titles_from {
            for row in self
                .db
                .pages_index_by_space(space_id)
                .map_err(map_storage_error)?
            {
                if row.trashed_at.is_none() {
                    existing_titles
                        .entry(row.title)
                        .and_modify(|known| *known = None)
                        .or_insert(Some(row.page_id));
                }
            }
        }

        // Properties, per space: matched by name or created; relations first, rollups after them,
        // formulas after every property they read.
        let mut props_by_space: HashMap<SpaceId, ImportProps> = HashMap::new();
        let mut properties_created = 0u32;
        let mut schema_changed: Vec<SpaceId> = Vec::new();
        for space_id in planned.layout.spaces.clone() {
            let defs = self.structure(space_id)?.model.property_schema();
            let mut props = ImportProps::from_schema(&defs);
            let wanted = planned
                .layout
                .used_properties
                .as_ref()
                .map(|used| used.get(&space_id));
            let uses = |name: &str| match wanted {
                None => true,
                Some(names) => names.is_some_and(|names| names.contains(name)),
            };
            let ordered = import_order(&planned.plan.properties, &uses);
            let mut changed = false;
            for property in ordered {
                self.import_property(
                    space_id,
                    property,
                    &mut props,
                    &mut planned.coerce_warnings,
                    &mut properties_created,
                    &mut changed,
                    now,
                )?;
            }
            if changed {
                schema_changed.push(space_id);
            }
            props_by_space.insert(space_id, props);
        }

        // Every page, in plan order, appended after the destination's existing children (a space
        // root is not a page: its children are root pages of its space): one bulk creation per
        // space, committed with the import origin (not an undo step).
        let author = self.device.device_id;
        let rng = self.rng.as_ref();
        let count = planned.plan.pages.len();
        let mut page_ids: Vec<Option<PageId>> = vec![None; count];
        let mut page_titles: Vec<String> = vec![String::new(); count];
        // The images the covers of the new pages use (ADR-0040).
        let mut cover_attachments: HashSet<AttachmentId> = HashSet::new();
        let mut per_space: Vec<(SpaceId, Vec<NewPage>)> = planned
            .layout
            .spaces
            .iter()
            .map(|space_id| (*space_id, Vec::new()))
            .collect();
        for &index in &planned.layout.order {
            let page = &planned.plan.pages[index];
            let page_id = PageId::generate(rng);
            let parent = match page.parent {
                Some(parent) if planned.layout.is_root[parent] => None,
                Some(parent) => Some(
                    page_ids
                        .get(parent)
                        .copied()
                        .flatten()
                        .ok_or_else(|| ApiError::internal("the import plan is inconsistent"))?,
                ),
                None => parent_id,
            };
            let title = clean_title(&page.title);
            // An icon that does not fit its grammar is left out; the import never makes an
            // uploaded-image icon.
            let icon = page
                .icon
                .as_deref()
                .and_then(|icon| validate::icon(Some(icon)).ok().flatten())
                .filter(|icon| validate::icon_attachment(icon).is_none());
            let space_id = planned.layout.page_space[index];
            let (cover, cover_y) = self.import_cover(
                &planned.plan.pages[index],
                space_id,
                &planned.attachments,
                &mut cover_attachments,
                &mut planned.resolve_warnings,
            );
            let Some((_, new_pages)) = per_space.iter_mut().find(|(known, _)| *known == space_id)
            else {
                return Err(ApiError::internal("the import plan is inconsistent"));
            };
            new_pages.push(NewPage {
                page_id,
                parent,
                title: title.clone(),
                icon,
                cover,
                cover_y,
            });
            page_ids[index] = Some(page_id);
            page_titles[index] = title;
        }
        for (space_id, new_pages) in &per_space {
            if new_pages.is_empty() {
                continue;
            }
            let open = self
                .docs
                .structures
                .get_mut(space_id)
                .ok_or_else(|| ApiError::not_found("space not found"))?;
            open.model.set_author(Some(author));
            open.model
                .create_pages(new_pages, now)
                .map_err(map_model_error)?;
            let ids: Vec<PageId> = new_pages.iter().map(|page| page.page_id).collect();
            self.persist_structure_with(
                *space_id,
                Some(&ids),
                UpdateOrigin::Local,
                PropsScope::Nothing,
            )?;
            // The titles are searchable at once, whatever happens to the documents.
            self.db
                .transaction(|tx| {
                    for page in new_pages {
                        tx.fts_set_title(page.page_id, &page.title)?;
                    }
                    Ok(())
                })
                .map_err(map_storage_error)?;
        }
        for space_id in &schema_changed {
            self.events.push(Event::PropertySchemaChanged {
                space_id: *space_id,
            });
        }
        for space_id in &planned.layout.spaces {
            self.events.push(Event::StructureChanged {
                space_id: *space_id,
            });
        }
        self.events.push(Event::SpacesChanged);
        self.sync_note_local_change();

        planned.started = Some(Started {
            page_ids,
            page_titles,
            existing_titles,
            props: props_by_space,
            properties_created,
            next: 0,
            used_attachments: cover_attachments,
        });
        Ok(())
    }

    /// The cover a planned page gets and its vertical position (ADR-0040): a named cover as it is
    /// (it is checked like `PageSetCover` checks one), a file cover as `attachment:<id>` when the
    /// shell uploaded it into the page's space and it is an image. Anything else leaves the page
    /// without a cover and is reported: `MissingFile` for a file that was not uploaded (or went to
    /// another space), `UnsupportedContent` (detail `cover`) for a cover the core refuses.
    fn import_cover(
        &self,
        page: &PlannedPage,
        space_id: SpaceId,
        attachments: &HashMap<String, Reported>,
        used: &mut HashSet<AttachmentId>,
        warnings: &mut WarningList,
    ) -> (Option<String>, Option<u8>) {
        let Some(cover) = &page.cover else {
            return (None, None);
        };
        let (text, attachment) = match cover {
            PlannedCover::Named(name) => (name.clone(), None),
            PlannedCover::File(path) => {
                match attachments
                    .get(path)
                    .filter(|reported| reported.space_id == space_id)
                {
                    Some(reported) => (format!("attachment:{}", reported.id), Some(reported.id)),
                    None => {
                        warnings.note(
                            &page.source_path,
                            ImportWarningCode::MissingFile,
                            Some(path),
                        );
                        return (None, None);
                    }
                }
            }
        };
        let valid = validate::cover(Some(&text)).ok().flatten().filter(|cover| {
            self.check_page_cover_attachment(space_id, Some(cover))
                .is_ok()
        });
        let Some(cover) = valid else {
            warnings.note(
                &page.source_path,
                ImportWarningCode::UnsupportedContent,
                Some("cover"),
            );
            return (None, None);
        };
        used.extend(attachment);
        let cover_y = validate::cover_y(page.cover_y).ok().flatten();
        (Some(cover), cover_y)
    }

    /// Matches or creates one planned property.
    #[allow(clippy::too_many_arguments)]
    fn import_property(
        &mut self,
        space_id: SpaceId,
        planned: &PlannedProperty,
        props: &mut ImportProps,
        warnings: &mut WarningList,
        created: &mut u32,
        schema_changed: &mut bool,
        now: i64,
    ) -> Result<(), ApiError> {
        let name = clean_name(&planned.name);
        if name.is_empty() {
            props.planned.insert(planned.name.clone(), None);
            return Ok(());
        }
        if let Some(existing) = props.find(&name) {
            // Matched by name, whatever its kind; options the source has and it lacks are added.
            props.planned.insert(planned.name.clone(), Some(existing));
            if props.kind(existing).is_some_and(PropertyKind::has_options) {
                for option in &planned.options {
                    *schema_changed |=
                        self.import_option(space_id, props, existing, option, now)?;
                }
            }
            return Ok(());
        }

        match self.import_create_property(space_id, planned, &name, props, created, now) {
            Ok(Some(id)) => {
                *schema_changed = true;
                props.planned.insert(planned.name.clone(), Some(id));
                if props.kind(id).is_some_and(PropertyKind::has_options) {
                    self.import_new_options(space_id, props, id, &planned.options, now)?;
                }
                Ok(())
            }
            Ok(None) | Err(PropertyIssue::Invalid) => {
                props.planned.insert(planned.name.clone(), None);
                warnings.note("", ImportWarningCode::UnsupportedContent, Some(&name));
                Ok(())
            }
            Err(PropertyIssue::Full) => {
                props.planned.insert(planned.name.clone(), None);
                warnings.note("", ImportWarningCode::PropertyLimit, Some(&name));
                Ok(())
            }
            Err(PropertyIssue::Failed(err)) => Err(err),
        }
    }

    /// Creates the property in the structure document. `Ok(None)`: a rollup the plan does not
    /// configure, or the schema refuses.
    fn import_create_property(
        &mut self,
        space_id: SpaceId,
        planned: &PlannedProperty,
        name: &str,
        props: &mut ImportProps,
        created: &mut u32,
        now: i64,
    ) -> Result<Option<PropertyId>, PropertyIssue> {
        if props.info.len() >= MAX_PROPERTIES {
            return Err(PropertyIssue::Full);
        }
        let rng = self.rng.as_ref();
        let id = PropertyId::generate(rng);
        let author = self.device.device_id;
        let open = self
            .docs
            .structures
            .get_mut(&space_id)
            .ok_or_else(|| PropertyIssue::Failed(ApiError::not_found("space not found")))?;
        open.model.set_author(Some(author));

        match (&planned.kind, &planned.config) {
            // People are members of a space: nothing in a file names them (the importer sends such a
            // column as text, and a plan that still says Person is left out with the usual warning).
            (PropertyKind::Person, _) => return Ok(None),
            (
                PropertyKind::Relation,
                Some(PlannedConfig::Relation {
                    two_way: true,
                    pair_name,
                }),
            ) => {
                if props.info.len() + 2 > MAX_PROPERTIES {
                    return Err(PropertyIssue::Full);
                }
                let pair_name = pair_name
                    .as_deref()
                    .map(clean_name)
                    .filter(|pair| !pair.is_empty());
                let back_name = pair_name.unwrap_or_else(|| back_property_name(name));
                let back_id = PropertyId::generate(rng);
                open.model
                    .create_relation_pair(id, back_id, name, &back_name, now)
                    .map_err(property_issue)?;
                props.add_property(id, name, PropertyKind::Relation);
                props.add_property(back_id, &back_name, PropertyKind::Relation);
                *created += 2;
            }
            (PropertyKind::Rollup, config) => {
                let Some(PlannedConfig::Rollup {
                    relation,
                    target,
                    function,
                }) = config
                else {
                    return Ok(None);
                };
                let Some(relation_id) = props
                    .find(relation)
                    .filter(|found| props.kind(*found) == Some(PropertyKind::Relation))
                else {
                    return Ok(None);
                };
                let target = match target {
                    PlannedRollupTarget::Title => RollupTarget::Title,
                    PlannedRollupTarget::Property(target) => match props.find(target) {
                        Some(target_id) => RollupTarget::Property(target_id),
                        None => return Ok(None),
                    },
                };
                let config = PropertyConfig::Rollup {
                    relation_property_id: relation_id,
                    target,
                    function: *function,
                };
                match open.model.create_property_with(
                    id,
                    name,
                    PropertyKind::Rollup,
                    Some(&config),
                    &mut || OptionId::generate(rng),
                    now,
                ) {
                    Ok(_) => {}
                    // A configuration the schema refuses (a function that does not fit the target).
                    Err(ModelError::InvalidProperty {
                        field: "config", ..
                    }) => return Ok(None),
                    Err(err) => return Err(property_issue(err)),
                }
                props.add_property(id, name, PropertyKind::Rollup);
                *created += 1;
            }
            (PropertyKind::Formula, config) => {
                let Some(PlannedConfig::Formula { expression }) = config else {
                    return Ok(None);
                };
                if expression.trim().is_empty() || expression.chars().count() > MAX_FORMULA_CHARS {
                    return Ok(None);
                }
                // The file names the properties it reads; they are the properties of this space by
                // name now (the title as `title`).
                let names = |name: &str| {
                    props
                        .find(name)
                        .map(|found| found.to_string())
                        .or_else(|| is_title_name(name).then(|| FORMULA_TITLE_ID.to_owned()))
                };
                let made = formula::to_stored(expression, &names)
                    .ok()
                    .and_then(|stored| {
                        let config = PropertyConfig::Formula {
                            expression: stored,
                            result_type: None,
                        };
                        open.model
                            .create_property_with(
                                id,
                                name,
                                PropertyKind::Formula,
                                Some(&config),
                                &mut || OptionId::generate(rng),
                                now,
                            )
                            .ok()
                    });
                if made.is_none() {
                    // An expression that does not resolve or check here is kept as written and stays
                    // broken (no value) until the person fixes it in the settings.
                    open.model
                        .create_formula_unchecked(id, name, expression, now)
                        .map_err(property_issue)?;
                }
                props.add_property(id, name, PropertyKind::Formula);
                *created += 1;
            }
            (kind, _) => {
                let def = open
                    .model
                    .create_property_with(
                        id,
                        name,
                        *kind,
                        None,
                        &mut || OptionId::generate(rng),
                        now,
                    )
                    .map_err(property_issue)?;
                props.add_property(id, name, *kind);
                for option in &def.options {
                    props.add_option(id, &option.name, option.id, option.color);
                }
                *created += 1;
            }
        }
        Ok(Some(id))
    }

    /// The options of a property this import just created. A status property starts with three
    /// default options; when the plan lists options they replace the defaults, in the plan's order,
    /// with the planned colours and groups.
    fn import_new_options(
        &mut self,
        space_id: SpaceId,
        props: &mut ImportProps,
        property: PropertyId,
        options: &[PlannedOption],
        now: i64,
    ) -> Result<(), ApiError> {
        if props.kind(property) == Some(PropertyKind::Status) && !options.is_empty() {
            let defaults: Vec<OptionId> = props
                .info
                .get(&property)
                .map(|info| info.options.iter().map(|(_, id)| *id).collect())
                .unwrap_or_default();
            for id in defaults {
                self.structure_mut(space_id)?
                    .model
                    .delete_option(property, id, now)
                    .map_err(map_model_error)?;
            }
            if let Some(info) = props.info.get_mut(&property) {
                info.options.clear();
                info.last_color = None;
            }
        }
        for option in options {
            self.import_option(space_id, props, property, option, now)?;
        }
        Ok(())
    }

    /// Adds a planned option unless the property has one of that name (the colour after the last
    /// option's in the palette when the plan gives none, to do for a status option without a
    /// group). Answers whether the schema changed.
    fn import_option(
        &mut self,
        space_id: SpaceId,
        props: &mut ImportProps,
        property: PropertyId,
        option: &PlannedOption,
        now: i64,
    ) -> Result<bool, ApiError> {
        let name = clean_name(&option.name);
        if name.is_empty() || props.option(property, &name).is_some() {
            return Ok(false);
        }
        if props
            .info
            .get(&property)
            .is_some_and(|info| info.options.len() >= MAX_OPTIONS)
        {
            return Ok(false);
        }
        let status = props.kind(property) == Some(PropertyKind::Status);
        let color = option.color.unwrap_or_else(|| props.next_color(property));
        let group = status.then(|| option.group.unwrap_or(StatusGroup::Todo));
        let id = OptionId::generate(self.rng.as_ref());
        self.structure_mut(space_id)?
            .model
            .add_option(property, id, &name, color, group, now)
            .map_err(map_model_error)?;
        props.add_option(property, &name, id, color);
        Ok(true)
    }

    // ----- every commit: documents and values -------------------------------------------------

    /// Writes the documents and the property values of the next `max_pages` pages.
    fn import_batch(&mut self, planned: &mut Planned, max_pages: usize) -> Result<(), ApiError> {
        let Planned {
            plan,
            layout,
            attachments,
            resolve_warnings,
            coerce_warnings,
            started,
            ..
        } = planned;
        let Some(started) = started.as_mut() else {
            return Err(ApiError::internal("the import did not start"));
        };
        let first = started.next;
        let last = layout.order.len().min(first.saturating_add(max_pages));
        if first >= last {
            return Ok(());
        }
        let now = self.clock.now_ms();
        let batch: &[usize] = &layout.order[first..last];

        // Placeholders to ids and fresh block ids, for each page of the batch, against the
        // schema of the page's own space.
        let mut contents: Vec<Value> = Vec::with_capacity(batch.len());
        for &index in batch {
            let space_id = layout.page_space[index];
            let Some(props) = started.props.get(&space_id) else {
                return Err(ApiError::internal("the import did not start"));
            };
            let resolver = CoreResolver {
                db: &self.db,
                spaces: &layout.spaces,
                page_ids: &started.page_ids,
                existing_titles: &started.existing_titles,
                attachments,
                props,
            };
            let (content, problems) =
                kb_import::resolve(&plan.pages[index], &resolver, self.rng.as_ref());
            resolve_warnings.extend(problems);
            contents.push(content);
        }

        // The documents and the values, a space at a time.
        let mut groups: Vec<(SpaceId, Vec<usize>, Vec<PreparedDoc>)> = Vec::new();
        for (&index, content) in batch.iter().zip(&contents) {
            let page_id = started
                .page_ids
                .get(index)
                .copied()
                .flatten()
                .ok_or_else(|| ApiError::internal("the import plan is inconsistent"))?;
            let title = started.page_titles.get(index).cloned().unwrap_or_default();
            let source_path = plan
                .pages
                .get(index)
                .map_or("", |page| page.source_path.as_str());
            let prepared = self.import_prepare_document(
                page_id,
                title,
                content,
                source_path,
                resolve_warnings,
            )?;
            let space_id = layout.page_space[index];
            match groups.iter_mut().find(|(known, _, _)| *known == space_id) {
                Some((_, indices, docs)) => {
                    indices.push(index);
                    docs.push(prepared);
                }
                None => groups.push((space_id, vec![index], vec![prepared])),
            }
        }
        for (space_id, indices, docs) in groups {
            let used = self.import_write_documents(space_id, docs, now)?;
            started.used_attachments.extend(used);
            self.import_values(
                space_id,
                plan,
                &layout.page_space,
                attachments,
                started,
                &indices,
                coerce_warnings,
                now,
            )?;
            // The template flag and the placeholders go after the content and the values (ADR-0056).
            self.import_templates(space_id, plan, started, &indices)?;
        }
        started.next = last;
        Ok(())
    }

    /// A new page document from resolved portable JSON, with the rows derived from it. Content
    /// the schema cannot hold becomes an empty page (with a warning): no page is left without
    /// a document.
    fn import_prepare_document(
        &self,
        page_id: PageId,
        title: String,
        content: &Value,
        source_path: &str,
        warnings: &mut WarningList,
    ) -> Result<PreparedDoc, ApiError> {
        let rng = self.rng.as_ref();
        let model = match PageDoc::from_portable(Doc::new(DocKind::Page, new_peer(rng)), content) {
            Ok(model) => model,
            Err(ModelError::Shape(_)) => {
                warnings.note(source_path, ImportWarningCode::UnsupportedContent, None);
                PageDoc::new(
                    Doc::new(DocKind::Page, new_peer(rng)),
                    BlockId::generate(rng),
                )
                .map_err(map_model_error)?
            }
            Err(other) => return Err(map_model_error(other)),
        };
        let blob = model
            .doc()
            .export_updates_since(None)
            .map_err(map_doc_error)?
            .ok_or_else(|| ApiError::internal("a new page document exported nothing"))?;
        let entry = text_entry_of(&model, &self.db);
        Ok(PreparedDoc {
            page_id,
            doc_id: DocId::for_page(page_id),
            title,
            blob,
            version: model.doc().frontiers(),
            entry,
        })
    }

    /// Persists the documents of a batch with their derived rows in one transaction, then the
    /// bookkeeping and the events. Answers the attachments the documents use.
    pub(crate) fn import_write_documents(
        &mut self,
        space_id: SpaceId,
        prepared: Vec<PreparedDoc>,
        now: i64,
    ) -> Result<BTreeSet<AttachmentId>, ApiError> {
        // A page someone opened while the import ran already has a document: the imported content
        // is merged into it instead.
        let mut existing: Vec<PreparedDoc> = Vec::new();
        let mut fresh: Vec<PreparedDoc> = Vec::with_capacity(prepared.len());
        for item in prepared {
            if self
                .db
                .doc_get(item.doc_id)
                .map_err(map_storage_error)?
                .is_some()
            {
                existing.push(item);
            } else {
                fresh.push(item);
            }
        }

        // Each document's key record goes in with it.
        let mut keys: Vec<KeyringItem> = Vec::with_capacity(fresh.len());
        for item in &fresh {
            keys.extend(self.doc_key_item(space_id, item.doc_id)?);
        }
        // The page's history begins with each document's content (ADR-0057); the rows ride in the same
        // transaction as the documents, one per document, whole batch in one commit.
        let journals: Vec<Option<JournalOp>> = fresh
            .iter()
            .map(|item| {
                self.journal_plan_new(item.page_id, &item.version, Some(item.title.clone()))
            })
            .collect();
        self.db
            .transaction(|tx| {
                for key in &keys {
                    tx.keyring_put(&crate::security::keyring_row(key))?;
                }
                for (item, journal) in fresh.iter().zip(&journals) {
                    let row = new_doc_row(item.doc_id, DocKindCode::Page, Some(space_id), now);
                    tx.doc_insert(&row)?;
                    tx.update_append(item.doc_id, &item.blob, UpdateOrigin::Core, now)?;
                    tx.fts_replace_page(item.page_id, &item.title, &item.entry.blocks)?;
                    tx.date_mentions_replace(item.page_id, &item.entry.date_mentions)?;
                    tx.links_replace_from(item.page_id, &item.entry.links)?;
                    tx.attachment_refs_replace(item.page_id, &item.entry.refs)?;
                    crate::history::record_journal(tx, item.page_id, journal.as_ref());
                }
                Ok(())
            })
            .map_err(map_storage_error)?;

        let mut touched: BTreeSet<AttachmentId> = BTreeSet::new();
        let mut link_targets: BTreeSet<PageId> = BTreeSet::new();
        self.keyring_merge(keys);
        for item in &fresh {
            self.events.push(Event::PageContentChanged {
                page_id: item.page_id,
                source: ChangeSource::Core,
            });
        }
        for item in fresh.iter().chain(&existing) {
            touched.extend(item.entry.refs.iter().map(|(_, id)| *id));
            link_targets.extend(item.entry.links.iter().map(|link| link.to_page));
        }
        let used = touched.clone();
        for item in existing {
            let page_id = item.page_id;
            self.open_page(page_id)?
                .model
                .doc()
                .import(&item.blob)
                .map_err(map_doc_error)?;
            self.persist_page(
                page_id,
                Some(item.blob),
                UpdateOrigin::Core,
                ChangeSource::Core,
            )?;
        }
        self.attachment_refs_changed(touched, now)?;
        for target in link_targets {
            self.events
                .push(Event::BacklinksChanged { page_id: target });
        }
        self.sync_note_local_change();
        Ok(used)
    }

    /// Coerces and sets the property values of the planned pages `indices`, all of the space
    /// `space_id`.
    #[allow(clippy::too_many_arguments)]
    fn import_values(
        &mut self,
        space_id: SpaceId,
        plan: &Plan,
        page_spaces: &[SpaceId],
        attachments: &HashMap<String, Reported>,
        started: &mut Started,
        indices: &[usize],
        warnings: &mut WarningList,
        now: i64,
    ) -> Result<(), ApiError> {
        if indices.iter().all(|index| {
            plan.pages
                .get(*index)
                .is_none_or(|page| page.properties.is_empty())
        }) {
            return Ok(());
        }
        let author = self.device.device_id;
        let rng = self.rng.as_ref();
        let open = self
            .docs
            .structures
            .get_mut(&space_id)
            .ok_or_else(|| ApiError::not_found("space not found"))?;
        open.model.set_author(Some(author));

        let Started {
            page_ids,
            existing_titles,
            props,
            used_attachments,
            ..
        } = started;
        let Some(props) = props.get_mut(&space_id) else {
            return Err(ApiError::internal("the import did not start"));
        };
        let mut ctx = ValueContext {
            model: &mut open.model,
            rng,
            now,
            space_id,
            props,
            plan_pages: &plan.pages,
            page_spaces,
            page_ids,
            existing_titles,
            attachments,
            used_attachments,
            schema_changed: false,
        };
        let mut affected: Vec<PageId> = Vec::new();
        for &index in indices {
            let Some(page) = plan.pages.get(index) else {
                continue;
            };
            let Some(page_id) = ctx.page_ids.get(index).copied().flatten() else {
                continue;
            };
            for (name, value) in &page.properties {
                let Some(Some(property)) = ctx.props.planned.get(name).copied() else {
                    // Left out when the property was matched or created (a warning was given).
                    continue;
                };
                let Some(kind) = ctx.props.kind(property) else {
                    continue;
                };
                let coerced = match ctx.coerce(property, kind, value, &page.source_path, warnings) {
                    Coerced::Value(value) => value,
                    Coerced::Empty => continue,
                    Coerced::Invalid => {
                        warnings.note(
                            &page.source_path,
                            ImportWarningCode::InvalidValue,
                            Some(name.as_str()),
                        );
                        continue;
                    }
                };
                match ctx
                    .model
                    .set_page_property(page_id, property, Some(coerced), now)
                {
                    Ok(pages) => {
                        for page in pages {
                            if !affected.contains(&page) {
                                affected.push(page);
                            }
                        }
                    }
                    // A value the property's own rules refuse (a URL that is not absolute, a
                    // date that is not a real day).
                    Err(ModelError::InvalidProperty { .. }) => warnings.note(
                        &page.source_path,
                        ImportWarningCode::InvalidValue,
                        Some(name.as_str()),
                    ),
                    Err(err) => return Err(map_model_error(err)),
                }
            }
        }
        let schema_changed = ctx.schema_changed;
        if affected.is_empty() && !schema_changed {
            return Ok(());
        }
        self.persist_structure_with(
            space_id,
            Some(&affected),
            UpdateOrigin::Local,
            PropsScope::Pages(&affected),
        )?;
        if schema_changed {
            self.events.push(Event::PropertySchemaChanged { space_id });
        }
        self.events.push(Event::StructureChanged { space_id });
        Ok(())
    }

    /// Sets the template flag and the placeholders of the planned pages `indices` that have them (a
    /// template exported by atomik-kb, ADR-0056), all of the space `space_id`, after their content and
    /// values are in. A placeholder is matched to a property of the space by name (the properties the
    /// import matched or created, and the ones the space had); one whose name matches none, or whose
    /// property does not take it, is left out. The write is not an undo step (the import is undone by
    /// trashing its container); the page index rows of the flagged subtrees are rewritten (they say the
    /// pages are in a template) and so are the property rows of the pages whose value a placeholder
    /// replaced.
    fn import_templates(
        &mut self,
        space_id: SpaceId,
        plan: &Plan,
        started: &Started,
        indices: &[usize],
    ) -> Result<(), ApiError> {
        let Some(props) = started.props.get(&space_id) else {
            return Ok(());
        };
        let mut wanted: Vec<ImportedTemplate> = Vec::new();
        for &index in indices {
            let Some(page) = plan.pages.get(index) else {
                continue;
            };
            if !page.template && page.template_placeholders.is_empty() {
                continue;
            }
            let Some(page_id) = started.page_ids.get(index).copied().flatten() else {
                continue;
            };
            wanted.push(ImportedTemplate {
                page_id,
                template: page.template,
                placeholders: page
                    .template_placeholders
                    .iter()
                    .filter_map(|(name, placeholder)| Some((props.find(name)?, *placeholder)))
                    .collect(),
            });
        }
        if wanted.is_empty() {
            return Ok(());
        }
        let author = self.device.device_id;
        let open = self
            .docs
            .structures
            .get_mut(&space_id)
            .ok_or_else(|| ApiError::not_found("space not found"))?;
        open.model.set_author(Some(author));
        let affected = open
            .model
            .import_templates(&wanted)
            .map_err(map_model_error)?;
        if affected.is_empty() {
            return Ok(());
        }
        self.persist_structure_with(
            space_id,
            Some(&affected),
            UpdateOrigin::Local,
            PropsScope::Pages(&affected),
        )?;
        self.events.push(Event::StructureChanged { space_id });
        Ok(())
    }

    /// Test hook: plans a session with a ready-made plan (the converters are tested on their own).
    #[cfg(test)]
    pub(crate) fn import_install_plan_for_test(
        &mut self,
        import_id: u32,
        plan: Plan,
    ) -> Result<ImportPlanInfo, ApiError> {
        self.import_install_plan(import_id, plan)
    }

    /// Test hook: gives a page the document built from portable JSON, replacing the one it has
    /// (the golden sample and whiteboards cannot be typed through the editor operations).
    #[cfg(test)]
    pub(crate) fn replace_page_document_for_test(
        &mut self,
        page_id: PageId,
        content: &Value,
    ) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        let title = crate::pages::title_of(&self.db, page_id).unwrap_or_default();
        self.docs.pages.remove(&page_id);
        self.db
            .transaction(|tx| tx.doc_delete(DocId::for_page(page_id)))
            .map_err(map_storage_error)?;
        let mut warnings = WarningList::default();
        let item = self.import_prepare_document(page_id, title, content, "", &mut warnings)?;
        let now = self.clock.now_ms();
        self.import_write_documents(space_id, vec![item], now)?;
        Ok(())
    }
}

/// A new page document ready to be written (by an import, and by a duplication, ADR-0040).
pub(crate) struct PreparedDoc {
    pub(crate) page_id: PageId,
    pub(crate) doc_id: DocId,
    pub(crate) title: String,
    pub(crate) blob: Vec<u8>,
    /// The version of the new document (its frontiers), for the page history journal.
    pub(crate) version: kb_doc::Frontiers,
    pub(crate) entry: TextEntry,
}

/// Why a property could not be created.
enum PropertyIssue {
    /// The schema has no room.
    Full,
    /// The name or configuration was refused.
    Invalid,
    /// A failure that ends the import.
    Failed(ApiError),
}

fn property_issue(err: ModelError) -> PropertyIssue {
    match err {
        ModelError::InvalidProperty {
            field: "spaceId", ..
        } => PropertyIssue::Full,
        ModelError::InvalidProperty { .. } => PropertyIssue::Invalid,
        other => PropertyIssue::Failed(map_model_error(other)),
    }
}

// ----- values ----------------------------------------------------------------------------------

/// The outcome of coercing one imported value to its property's kind.
enum Coerced {
    Value(PropertyValue),
    /// Nothing to store (an empty list, a blank text).
    Empty,
    /// The value does not fit the kind.
    Invalid,
}

struct ValueContext<'a> {
    model: &'a mut StructureDoc,
    rng: &'a dyn Rng,
    now: i64,
    /// The space of the pages whose values are coerced.
    space_id: SpaceId,
    props: &'a mut ImportProps,
    plan_pages: &'a [PlannedPage],
    /// The space of each planned page.
    page_spaces: &'a [SpaceId],
    page_ids: &'a [Option<PageId>],
    existing_titles: &'a HashMap<String, Option<PageId>>,
    attachments: &'a HashMap<String, Reported>,
    used_attachments: &'a mut HashSet<AttachmentId>,
    /// An option was added to a property's list.
    schema_changed: bool,
}

impl ValueContext<'_> {
    fn coerce(
        &mut self,
        property: PropertyId,
        kind: PropertyKind,
        value: &ImportValue,
        path: &str,
        warnings: &mut WarningList,
    ) -> Coerced {
        match kind {
            PropertyKind::Text => {
                let text = self.text_of(value);
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Coerced::Invalid;
                }
                let text: String = text
                    .chars()
                    .map(|c| {
                        if c.is_control() && !matches!(c, '\t' | '\n' | '\r') {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect();
                if text.trim().is_empty() {
                    Coerced::Empty
                } else {
                    Coerced::Value(PropertyValue::Text(text))
                }
            }
            PropertyKind::Url | PropertyKind::Email | PropertyKind::Phone => {
                let text = self.text_of(value);
                let text = text.trim().to_owned();
                if text.is_empty() {
                    Coerced::Empty
                } else if kind == PropertyKind::Url {
                    Coerced::Value(PropertyValue::Url(text))
                } else if kind == PropertyKind::Email {
                    Coerced::Value(PropertyValue::Email(text))
                } else {
                    Coerced::Value(PropertyValue::Phone(text))
                }
            }
            PropertyKind::Number => match value {
                ImportValue::Number(number) if number.is_finite() => {
                    Coerced::Value(PropertyValue::Number(*number))
                }
                ImportValue::Text(text) => match kb_import::values::parse_number(text) {
                    Some(number) => Coerced::Value(PropertyValue::Number(number)),
                    None if text.trim().is_empty() => Coerced::Empty,
                    None => Coerced::Invalid,
                },
                _ => Coerced::Invalid,
            },
            PropertyKind::Checkbox => match value {
                ImportValue::Bool(on) => Coerced::Value(PropertyValue::Checkbox(*on)),
                ImportValue::Text(text) => match kb_import::values::parse_bool(text) {
                    Some(on) => Coerced::Value(PropertyValue::Checkbox(on)),
                    None if text.trim().is_empty() => Coerced::Empty,
                    None => Coerced::Invalid,
                },
                ImportValue::Number(number) if *number == 0.0 || *number == 1.0 => {
                    Coerced::Value(PropertyValue::Checkbox(*number == 1.0))
                }
                _ => Coerced::Invalid,
            },
            PropertyKind::Date => match value {
                ImportValue::Date { date, time } => {
                    Coerced::Value(PropertyValue::Date(DateValue {
                        date: date.clone(),
                        time: time.clone(),
                    }))
                }
                ImportValue::Text(text) => match kb_import::values::parse_date(text) {
                    Some((date, time)) => {
                        Coerced::Value(PropertyValue::Date(DateValue { date, time }))
                    }
                    None if text.trim().is_empty() => Coerced::Empty,
                    None => Coerced::Invalid,
                },
                _ => Coerced::Invalid,
            },
            PropertyKind::Select | PropertyKind::Status => {
                let names = self.names_of(value);
                let Some(first) = names.first() else {
                    return Coerced::Empty;
                };
                if names.len() > 1 {
                    // A select holds one option: the first is kept, the rest are reported.
                    warnings.note(path, ImportWarningCode::InvalidValue, None);
                }
                match self.option_for(property, first) {
                    Some(option) if kind == PropertyKind::Select => {
                        Coerced::Value(PropertyValue::Select(option))
                    }
                    Some(option) => Coerced::Value(PropertyValue::Status(option)),
                    None => Coerced::Invalid,
                }
            }
            PropertyKind::MultiSelect => {
                let mut options: Vec<OptionId> = Vec::new();
                for name in self.names_of(value) {
                    match self.option_for(property, &name) {
                        Some(option) if !options.contains(&option) => options.push(option),
                        Some(_) => {}
                        None => return Coerced::Invalid,
                    }
                    if options.len() >= MAX_SELECTED_OPTIONS {
                        break;
                    }
                }
                if options.is_empty() {
                    Coerced::Empty
                } else {
                    Coerced::Value(PropertyValue::MultiSelect(options))
                }
            }
            PropertyKind::Files => {
                let paths: Vec<&str> = match value {
                    ImportValue::Files(paths) => paths.iter().map(String::as_str).collect(),
                    ImportValue::Text(text) => vec![text.as_str()],
                    ImportValue::List(items) => items.iter().map(String::as_str).collect(),
                    _ => return Coerced::Invalid,
                };
                let mut files: Vec<AttachmentId> = Vec::new();
                for path in paths {
                    // Only files of the page's own space (the space of the first page that uses
                    // a file is where it was uploaded).
                    if let Some(reported) = self.attachments.get(path)
                        && reported.space_id == self.space_id
                        && !files.contains(&reported.id)
                        && files.len() < MAX_FILES
                    {
                        files.push(reported.id);
                        self.used_attachments.insert(reported.id);
                    }
                }
                if files.is_empty() {
                    Coerced::Empty
                } else {
                    Coerced::Value(PropertyValue::Files(files))
                }
            }
            PropertyKind::Checklist => {
                let items: Vec<(bool, String)> = match value {
                    ImportValue::Checklist(items) => items.clone(),
                    ImportValue::List(items) => {
                        items.iter().map(|text| (false, text.clone())).collect()
                    }
                    ImportValue::Text(text) => vec![(false, text.clone())],
                    _ => return Coerced::Invalid,
                };
                let items: Vec<ChecklistItem> = items
                    .into_iter()
                    .take(MAX_CHECKLIST_ITEMS)
                    .map(|(done, text)| ChecklistItem {
                        id: ChecklistItemId::generate(self.rng),
                        text: one_line(&text, MAX_CHECKLIST_TEXT_CHARS),
                        done,
                    })
                    .collect();
                if items.is_empty() {
                    Coerced::Empty
                } else {
                    Coerced::Value(PropertyValue::Checklist(items))
                }
            }
            PropertyKind::Place => {
                let (name, lat, lon) = match value {
                    ImportValue::Place { name, lat, lon } => (name.clone(), *lat, *lon),
                    ImportValue::Text(text) => (text.clone(), None, None),
                    _ => return Coerced::Invalid,
                };
                let name = one_line(&name, MAX_PLACE_NAME_CHARS);
                if name.is_empty() {
                    return Coerced::Empty;
                }
                let (lat, lon) = match (lat, lon) {
                    (Some(lat), Some(lon)) => (Some(lat), Some(lon)),
                    _ => (None, None),
                };
                Coerced::Value(PropertyValue::Place(PlaceValue { name, lat, lon }))
            }
            PropertyKind::Relation => self.relation(value, path, warnings),
            // An import cannot name members of the space (ADR-0049): the importer sends the people of a
            // column as text, so a Person property that exists already (matched by name, whatever its
            // kind) gets no value from it, with the warning of any value its kind does not take, and
            // a column is never created as a Person property (`import_create_property`).
            // A property the page cannot store a value for.
            PropertyKind::Person
            | PropertyKind::Rollup
            | PropertyKind::CreatedTime
            | PropertyKind::LastEditedTime
            | PropertyKind::CreatedBy
            | PropertyKind::LastEditedBy
            | PropertyKind::Formula => Coerced::Invalid,
        }
    }

    /// Pages by planned index or by the unique title of an existing page.
    fn relation(&self, value: &ImportValue, path: &str, warnings: &mut WarningList) -> Coerced {
        let targets: Vec<PageTarget> = match value {
            ImportValue::Pages(targets) => targets.clone(),
            ImportValue::List(items) => items.iter().cloned().map(PageTarget::Title).collect(),
            ImportValue::Text(text) if !text.trim().is_empty() => {
                vec![PageTarget::Title(text.trim().to_owned())]
            }
            ImportValue::Text(_) => return Coerced::Empty,
            _ => return Coerced::Invalid,
        };
        let mut pages: Vec<PageId> = Vec::new();
        for target in &targets {
            let page = match target {
                // A relation holds pages of its own space: a page planned into another space
                // (an AppFlowy relation across spaces) is left out, with a warning.
                PageTarget::Planned(index) => self
                    .page_ids
                    .get(*index)
                    .copied()
                    .flatten()
                    .filter(|_| self.page_spaces.get(*index) == Some(&self.space_id)),
                PageTarget::Title(title) => self.existing_titles.get(title).copied().flatten(),
            };
            match page {
                Some(page) if !pages.contains(&page) && pages.len() < MAX_RELATED_PAGES => {
                    pages.push(page);
                }
                Some(_) => {}
                None => warnings.note(
                    path,
                    ImportWarningCode::UnresolvedLink,
                    match target {
                        PageTarget::Title(title) => Some(title.as_str()),
                        PageTarget::Planned(_) => None,
                    },
                ),
            }
        }
        if pages.is_empty() {
            Coerced::Empty
        } else {
            Coerced::Value(PropertyValue::Relation(pages))
        }
    }

    /// The option of a property with this name, added when the property has none.
    fn option_for(&mut self, property: PropertyId, name: &str) -> Option<OptionId> {
        let name = clean_name(name);
        if name.is_empty() {
            return None;
        }
        if let Some(option) = self.props.option(property, &name) {
            return Some(option);
        }
        let info = self.props.info.get(&property)?;
        if info.options.len() >= MAX_OPTIONS {
            return None;
        }
        let color = self.props.next_color(property);
        let group = (info.kind == PropertyKind::Status).then_some(StatusGroup::Todo);
        let id = OptionId::generate(self.rng);
        self.model
            .add_option(property, id, &name, color, group, self.now)
            .ok()?;
        self.props.add_option(property, &name, id, color);
        self.schema_changed = true;
        Some(id)
    }

    /// Option names of a value: a list, or one text.
    fn names_of(&self, value: &ImportValue) -> Vec<String> {
        match value {
            ImportValue::List(items) => items
                .iter()
                .map(|item| clean_name(item))
                .filter(|item| !item.is_empty())
                .collect(),
            other => {
                let text = clean_name(&self.text_of(other));
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![text]
                }
            }
        }
    }

    /// A value as text, for a text property or an option name.
    fn text_of(&self, value: &ImportValue) -> String {
        match value {
            ImportValue::Text(text) => text.clone(),
            ImportValue::Number(number) => number.to_string(),
            ImportValue::Bool(true) => "Yes".to_owned(),
            ImportValue::Bool(false) => "No".to_owned(),
            ImportValue::Date { date, time } => match time {
                Some(time) => format!("{date} {time}"),
                None => date.clone(),
            },
            ImportValue::List(items) => items.join(", "),
            ImportValue::Pages(targets) => targets
                .iter()
                .map(|target| match target {
                    PageTarget::Planned(index) => self
                        .plan_pages
                        .get(*index)
                        .map(|page| page.title.clone())
                        .unwrap_or_default(),
                    PageTarget::Title(title) => title.clone(),
                })
                .collect::<Vec<_>>()
                .join(", "),
            ImportValue::Files(paths) => paths
                .iter()
                .map(|path| path.rsplit('/').next().unwrap_or(path).to_owned())
                .collect::<Vec<_>>()
                .join(", "),
            ImportValue::Checklist(items) => items
                .iter()
                .map(|(done, text)| format!("[{}] {text}", if *done { 'x' } else { ' ' }))
                .collect::<Vec<_>>()
                .join("; "),
            ImportValue::Place { name, .. } => name.clone(),
        }
    }
}

// ----- the plan's layout ---------------------------------------------------------------------------

/// A plan the session can use: at most [`MAX_PLANNED_PAGES`] pages, every page's parent before
/// it, and space roots in range, each listed once.
fn plan_is_consistent(plan: &Plan) -> bool {
    let count = plan.pages.len();
    if count > MAX_PLANNED_PAGES
        || plan
            .pages
            .iter()
            .enumerate()
            .any(|(index, page)| page.parent.is_some_and(|parent| parent >= index))
    {
        return false;
    }
    let mut seen: HashSet<usize> = HashSet::new();
    plan.space_roots
        .iter()
        .all(|index| *index < count && seen.insert(*index))
}

/// The name of a space made from a page title or the source's name: control characters become
/// spaces, at most 100 characters, "Untitled" when nothing is left.
fn space_name(title: &str) -> String {
    let cut: String = clean_title(title).chars().take(100).collect();
    let cut = cut.trim();
    if cut.is_empty() {
        "Untitled".to_owned()
    } else {
        cut.to_owned()
    }
}

impl Layout {
    /// Gives each needed file and each remote file the space of the first page that uses it
    /// (a file no page names goes to the first space).
    fn place_files(&mut self, plan: &Plan) {
        let known: HashSet<&str> = plan
            .needed_files
            .iter()
            .map(String::as_str)
            .chain(plan.remote_files.iter().map(|remote| remote.key.as_str()))
            .collect();
        for (index, page) in plan.pages.iter().enumerate() {
            let Some(space_id) = self.page_space.get(index).copied() else {
                continue;
            };
            let mut paths: Vec<&str> = page.file_refs();
            paths.extend(page.cover_file());
            for (_, value) in &page.properties {
                if let ImportValue::Files(files) = value {
                    paths.extend(files.iter().map(String::as_str));
                }
            }
            for path in paths {
                if known.contains(path) {
                    self.file_space.entry(path.to_owned()).or_insert(space_id);
                }
            }
        }
        if let Some(first) = self.spaces.first().copied() {
            for path in known {
                self.file_space.entry(path.to_owned()).or_insert(first);
            }
        }
    }
}

/// The names of the planned properties each space's pages use: in their values and in the saved
/// views of their content (a `pageList` with `$names: true` holds names where ids will go).
fn used_properties(plan: &Plan, layout: &Layout) -> HashMap<SpaceId, HashSet<String>> {
    let mut used: HashMap<SpaceId, HashSet<String>> = layout
        .spaces
        .iter()
        .map(|space_id| (*space_id, HashSet::new()))
        .collect();
    for &index in &layout.order {
        let Some(page) = plan.pages.get(index) else {
            continue;
        };
        let Some(names) = layout
            .page_space
            .get(index)
            .and_then(|space_id| used.get_mut(space_id))
        else {
            continue;
        };
        names.extend(page.properties.iter().map(|(name, _)| name.clone()));
        names.extend(page.view_property_names());
    }
    // A rollup needs its relation and its target in the space too, a formula the properties it
    // reads (by the names the plan has them under).
    for names in used.values_mut() {
        for _ in 0..3 {
            let before = names.len();
            for property in &plan.properties {
                if !names.contains(&property.name) {
                    continue;
                }
                match &property.config {
                    Some(PlannedConfig::Rollup {
                        relation, target, ..
                    }) => {
                        names.insert(relation.clone());
                        if let PlannedRollupTarget::Property(target) = target {
                            names.insert(target.clone());
                        }
                    }
                    Some(PlannedConfig::Formula { expression }) => {
                        for read in formula::referenced_ids(expression) {
                            let read = fold(&read);
                            names.extend(
                                plan.properties
                                    .iter()
                                    .filter(|other| fold(&other.name) == read)
                                    .map(|other| other.name.clone()),
                            );
                        }
                    }
                    _ => {}
                }
            }
            if names.len() == before {
                break;
            }
        }
    }
    used
}

// ----- paths -------------------------------------------------------------------------------------

/// A source path as the shell gives it: relative, `/`-separated, at most 1,024 bytes, no empty,
/// `.` or `..` segment, no backslash, drive letter or control character.
fn valid_source_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return false;
    }
    if path.starts_with('/') || path.contains('\\') || path.chars().any(char::is_control) {
        return false;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    path.split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}
