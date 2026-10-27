//! Helpers shared by the import and export tests (ADR-0036): a clock with a UTC offset, property
//! and attachment shortcuts, drivers that run a whole export or import session, and the
//! normalisation the fidelity comparison uses.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use kb_storage::{ChunkSyncQueries, ChunkSyncRow};
use kb_types::{
    AttachmentId, AttachmentInfo, ByteRange, Clock, ExportBatch, ExportContent, ExportEntry,
    ExportFormat, ExportPlan, ExportWarning, ImportPlanInfo, ImportProgress, ImportReport,
    OptionColor, PageId, PageProperties, PropertyConfig, PropertyDef, PropertyId, PropertyKind,
    PropertyOption, PropertyValue, Request, Response, SpaceId,
};
use serde_json::{Map, Value, json};

use crate::Core;
use crate::test_support::Fixture;

/// A clock that stands still and tells the device's UTC offset.
pub struct OffsetClock {
    pub now_ms: i64,
    pub offset_seconds: i32,
}

impl Clock for OffsetClock {
    fn now_ms(&self) -> i64 {
        self.now_ms
    }

    fn local_offset_seconds(&self) -> i32 {
        self.offset_seconds
    }
}

/// An open core with a workspace and its first space (the fixture's clock stands at 1,000 ms).
pub fn setup() -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = first_space_of(&mut core);
    (fx, core, space)
}

/// Like [`setup`], for a device `offset_seconds` ahead of UTC, with the clock at `now_ms`.
pub fn setup_with_offset(now_ms: i64, offset_seconds: i32) -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut deps = fx.deps(1, now_ms);
    deps.clock = Box::new(OffsetClock {
        now_ms,
        offset_seconds,
    });
    let mut core = Core::open(deps).unwrap();
    let space = first_space_of(&mut core);
    (fx, core, space)
}

/// Another core in its own data directory (a second device or workspace).
pub fn another_core(seed: u64) -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(seed, 2_000);
    let space = first_space_of(&mut core);
    (fx, core, space)
}

fn first_space_of(core: &mut Core) -> SpaceId {
    crate::test_support::create_workspace(core, "Home", Some("Personal"));
    let space = crate::test_support::spaces(core)[0].id;
    core.drain_events();
    space
}

// ----- properties ------------------------------------------------------------------------------

pub fn schema(core: &mut Core, space_id: SpaceId) -> Vec<PropertyDef> {
    match core
        .dispatch(Request::PropertySchemaGet { space_id })
        .unwrap()
    {
        Response::PropertySchema(schema) => schema.properties,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn prop_create(
    core: &mut Core,
    space_id: SpaceId,
    name: &str,
    kind: PropertyKind,
) -> PropertyDef {
    prop_create_with(core, space_id, name, kind, None)
}

pub fn prop_create_with(
    core: &mut Core,
    space_id: SpaceId,
    name: &str,
    kind: PropertyKind,
    config: Option<PropertyConfig>,
) -> PropertyDef {
    match core
        .dispatch(Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config,
        })
        .unwrap()
    {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn option_add(
    core: &mut Core,
    space_id: SpaceId,
    property_id: PropertyId,
    name: &str,
    color: OptionColor,
) -> PropertyOption {
    match core
        .dispatch(Request::PropertyOptionAdd {
            space_id,
            property_id,
            name: name.into(),
            color,
            group: None,
        })
        .unwrap()
    {
        Response::PropertyOption(option) => option,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn set_value(core: &mut Core, page_id: PageId, property_id: PropertyId, value: PropertyValue) {
    core.dispatch(Request::PagePropertySet {
        page_id,
        property_id,
        value: Some(value),
    })
    .unwrap();
}

pub fn values_of(core: &mut Core, page_id: PageId) -> Vec<(PropertyId, PropertyValue)> {
    match core
        .dispatch(Request::PagePropertiesGet { page_id })
        .unwrap()
    {
        Response::PageProperties(PageProperties { values }) => values,
        other => panic!("unexpected {other:?}"),
    }
}

/// The property of a space by name (the first one, Tags included).
pub fn property_named(core: &mut Core, space_id: SpaceId, name: &str) -> PropertyDef {
    schema(core, space_id)
        .into_iter()
        .find(|def| def.name == name)
        .unwrap_or_else(|| panic!("no property named {name}"))
}

/// The value of the property named `name` on a page.
pub fn value_named(
    core: &mut Core,
    space_id: SpaceId,
    page_id: PageId,
    name: &str,
) -> Option<PropertyValue> {
    let property = property_named(core, space_id, name);
    values_of(core, page_id)
        .into_iter()
        .find(|(id, _)| *id == property.id)
        .map(|(_, value)| value)
}

// ----- attachments -------------------------------------------------------------------------------

pub fn upload(
    core: &mut Core,
    space_id: SpaceId,
    name: &str,
    mime: &str,
    data: &[u8],
) -> AttachmentInfo {
    let id = match core
        .dispatch(Request::AttachmentBegin {
            space_id,
            name: name.into(),
            mime: mime.into(),
        })
        .unwrap()
    {
        Response::AttachmentStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    };
    core.attachment_write(id, data).unwrap();
    match core
        .dispatch(Request::AttachmentFinish { attachment_id: id })
        .unwrap()
    {
        Response::Attachment(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn read_attachment(core: &mut Core, id: AttachmentId) -> Vec<u8> {
    let mut out = Vec::new();
    core.attachment_read(id, None::<ByteRange>, &mut |piece| {
        out.extend_from_slice(piece);
        Ok(())
    })
    .unwrap();
    out
}

/// Makes the device lose the bytes of an attachment, as an on-demand device has not fetched them:
/// its chunks are wanted but not here.
pub fn make_not_downloaded(core: &mut Core, id: AttachmentId) {
    let rows = core.db.chunk_sync_by_attachment(id).unwrap();
    assert!(!rows.is_empty(), "an uploaded attachment has transfer rows");
    for row in rows {
        core.db
            .chunk_sync_put(&ChunkSyncRow {
                hash: row.hash,
                attachment_id: id,
                state: crate::transfers::DOWNLOAD,
                attempts: 0,
                next_at: None,
            })
            .unwrap();
    }
}

pub fn info_of(core: &mut Core, id: AttachmentId) -> AttachmentInfo {
    match core
        .dispatch(Request::AttachmentGet { attachment_id: id })
        .unwrap()
    {
        Response::Attachment(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

// ----- content -----------------------------------------------------------------------------------

/// A portable document of paragraphs with fixed block ids (`seed` keeps them apart).
pub fn doc_of(seed: u8, paragraphs: &[&str]) -> Value {
    let content: Vec<Value> = paragraphs
        .iter()
        .enumerate()
        .map(|(at, text)| {
            json!({
                "type": "paragraph",
                "attrs": { "id": kb_types::BlockId::from_random_bytes([seed.wrapping_add(at as u8); 16]).to_string() },
                "content": [{ "type": "text", "text": text }]
            })
        })
        .collect();
    json!({ "type": "doc", "content": content })
}

/// The portable content of a page.
pub fn content_of(core: &mut Core, page_id: PageId) -> Value {
    crate::test_support::page_content(core, page_id)
}

/// The text of every `text` node of a portable document, in order.
pub fn texts_of(content: &Value) -> Vec<String> {
    fn walk(node: &Value, out: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                if map.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(text) = map.get("text").and_then(Value::as_str)
                {
                    out.push(text.to_owned());
                }
                for (key, value) in map {
                    if key != "whiteboards" {
                        walk(value, out);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(content, &mut out);
    out
}

// ----- export drivers --------------------------------------------------------------------------

/// Everything an export session produced.
pub struct Exported {
    pub plan: ExportPlan,
    pub entries: Vec<ExportEntry>,
    pub warnings: Vec<ExportWarning>,
    pub calls: usize,
}

impl Exported {
    pub fn paths(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }

    pub fn text(&self, path: &str) -> &str {
        match self.entries.iter().find(|entry| entry.path == path) {
            Some(ExportEntry {
                content: ExportContent::Text { text },
                ..
            }) => text,
            Some(_) => panic!("{path} is not a text entry"),
            None => panic!("no entry {path} in {:?}", self.paths()),
        }
    }
}

pub fn export_begin(
    core: &mut Core,
    space_id: SpaceId,
    page_id: Option<PageId>,
    include_children: bool,
    format: ExportFormat,
) -> ExportPlan {
    match core
        .dispatch(Request::ExportBegin {
            space_id,
            page_id,
            include_children,
            format,
        })
        .unwrap()
    {
        Response::ExportPlanned(plan) => plan,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn export_next(core: &mut Core, export_id: u32, max_entries: u32) -> ExportBatch {
    match core
        .dispatch(Request::ExportNext {
            export_id,
            max_entries,
        })
        .unwrap()
    {
        Response::ExportBatch(batch) => batch,
        other => panic!("unexpected {other:?}"),
    }
}

/// Plans an export and walks it to the end, `max_entries` at a time, then ends the session.
pub fn export_all(
    core: &mut Core,
    space_id: SpaceId,
    page_id: Option<PageId>,
    include_children: bool,
    format: ExportFormat,
    max_entries: u32,
) -> Exported {
    let plan = export_begin(core, space_id, page_id, include_children, format);
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut calls = 0;
    loop {
        let batch = export_next(core, plan.export_id, max_entries);
        assert!(batch.entries.len() <= max_entries as usize);
        calls += 1;
        entries.extend(batch.entries);
        warnings.extend(batch.warnings);
        if batch.done {
            break;
        }
        assert!(calls < 200_000, "the export never ends");
    }
    core.dispatch(Request::ExportEnd {
        export_id: plan.export_id,
    })
    .unwrap();
    Exported {
        plan,
        entries,
        warnings,
        calls,
    }
}

/// A text file of an import source: its path and its text.
pub type SourceText = (String, String);

/// A binary file of an import source: its path and its bytes.
pub type SourceBinary = (String, Vec<u8>);

/// The files of an export as an import source: the text entries (`.md`, `.html`, `.excalidraw`,
/// the sidecar) with their text, and the attachment entries with their bytes read from `core`.
pub fn as_source(core: &mut Core, exported: &Exported) -> (Vec<SourceText>, Vec<SourceBinary>) {
    let mut texts = Vec::new();
    let mut binaries = Vec::new();
    for entry in &exported.entries {
        match &entry.content {
            ExportContent::Text { text } => texts.push((entry.path.clone(), text.clone())),
            ExportContent::Attachment {
                attachment_id,
                size,
            } => {
                let bytes = read_attachment(core, *attachment_id);
                assert_eq!(bytes.len() as u64, *size);
                binaries.push((entry.path.clone(), bytes));
            }
        }
    }
    (texts, binaries)
}

// ----- import drivers --------------------------------------------------------------------------

/// What an import session answered.
pub struct Imported {
    pub import_id: u32,
    pub plan: ImportPlanInfo,
    pub report: ImportReport,
}

pub fn import_begin(
    core: &mut Core,
    space_id: SpaceId,
    parent_id: Option<PageId>,
    source_name: Option<&str>,
) -> u32 {
    match core
        .dispatch(Request::ImportBegin {
            space_id,
            parent_id,
            source_name: source_name.map(str::to_owned),
        })
        .unwrap()
    {
        Response::ImportStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn import_commit(core: &mut Core, import_id: u32, max_pages: u32) -> ImportProgress {
    match core
        .dispatch(Request::ImportCommit {
            import_id,
            max_pages,
        })
        .unwrap()
    {
        Response::ImportProgress(progress) => progress,
        other => panic!("unexpected {other:?}"),
    }
}

/// Commits until the import is done; answers the report and the number of calls.
pub fn import_commit_all(core: &mut Core, import_id: u32, max_pages: u32) -> (ImportReport, usize) {
    let mut calls = 0;
    loop {
        let progress = import_commit(core, import_id, max_pages);
        calls += 1;
        if progress.done {
            return (progress.report.expect("a done import has a report"), calls);
        }
        assert!(calls < 100_000, "the import never ends");
    }
}

/// The type the shell gives an uploaded file by its extension: an image type for the usual
/// extensions of pictures, a generic one otherwise.
fn mime_of(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Runs a whole import of text files and binary files into `space_id` under `parent_id`: files,
/// plan, the upload of every needed binary, attachments, commits of `max_pages`.
pub fn run_import(
    core: &mut Core,
    space_id: SpaceId,
    parent_id: Option<PageId>,
    source_name: Option<&str>,
    files: &[(String, String)],
    binaries: &[(String, Vec<u8>)],
    max_pages: u32,
) -> Imported {
    let import_id = import_begin(core, space_id, parent_id, source_name);
    for (path, text) in files {
        core.dispatch(Request::ImportAddFile {
            import_id,
            path: path.clone(),
            text: text.clone(),
        })
        .unwrap();
    }
    let binary_paths: Vec<String> = binaries.iter().map(|(path, _)| path.clone()).collect();
    let plan = match core
        .dispatch(Request::ImportPlan {
            import_id,
            binary_paths,
        })
        .unwrap()
    {
        Response::ImportPlanned(plan) => plan,
        other => panic!("unexpected {other:?}"),
    };
    for path in &plan.needed_files {
        let (_, bytes) = binaries
            .iter()
            .find(|(known, _)| known == path)
            .unwrap_or_else(|| {
                panic!("the plan needs {path}, which is not a binary of the source")
            });
        let name = path.rsplit('/').next().unwrap();
        let info = upload(core, space_id, name, mime_of(name), bytes);
        core.dispatch(Request::ImportAddAttachment {
            import_id,
            path: path.clone(),
            attachment_id: info.attachment_id,
        })
        .unwrap();
    }
    let (report, _) = import_commit_all(core, import_id, max_pages);
    Imported {
        import_id,
        plan,
        report,
    }
}

// ----- fidelity comparison ---------------------------------------------------------------------

/// A portable document with every UUID replaced by `#n`, numbered in order of first appearance in
/// the content, with what a round trip documents as lost taken out: text colours and highlights,
/// the `blockId` of page references and cards, deleted whiteboard elements, person mentions (they
/// come back as their `@name` text). The `whiteboards` map is keyed by the same numbers.
pub fn normalised(content: &Value) -> Value {
    fn is_uuid(text: &str) -> bool {
        text.len() == 36
            && text.bytes().enumerate().all(|(at, b)| match at {
                8 | 13 | 18 | 23 => b == b'-',
                _ => b.is_ascii_hexdigit(),
            })
    }
    fn walk(node: &Value, numbers: &mut BTreeMap<String, usize>) -> Value {
        match node {
            Value::String(text) if is_uuid(text) => {
                let next = numbers.len();
                let n = *numbers.entry(text.clone()).or_insert(next);
                Value::String(format!("#{n}"))
            }
            Value::Object(map) => {
                let mut out = Map::new();
                let node_type = map.get("type").and_then(Value::as_str);
                // A person mention is its `@name` as text in a file and is not read back as a mention (ADR-0049).
                if node_type == Some("personMention") {
                    let label = map
                        .get("attrs")
                        .and_then(|attrs| attrs.get("label"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    return json!({ "type": "text", "text": format!("@{label}") });
                }
                for (key, value) in map {
                    // A reference to a block of another page is not kept by a file link.
                    if key == "attrs"
                        && matches!(node_type, Some("pageRef" | "pageLink"))
                        && let Value::Object(attrs) = value
                    {
                        let mut kept = attrs.clone();
                        kept.remove("blockId");
                        out.insert(key.clone(), walk(&Value::Object(kept), numbers));
                        continue;
                    }
                    out.insert(key.clone(), walk(value, numbers));
                }
                // The colour marks are lost, by design.
                if let Some(Value::Array(marks)) = out.get_mut("marks") {
                    marks.retain(|mark| {
                        !matches!(
                            mark.get("type").and_then(Value::as_str),
                            Some("textColor" | "highlight")
                        )
                    });
                }
                if matches!(out.get("marks"), Some(Value::Array(marks)) if marks.is_empty()) {
                    out.remove("marks");
                }
                Value::Object(out)
            }
            Value::Array(items) => {
                let mut out: Vec<Value> = Vec::with_capacity(items.len());
                for item in items {
                    let item = walk(item, numbers);
                    // Runs of text that only differed by a colour are one run again.
                    if let (Some(Value::Object(last)), Value::Object(next)) =
                        (out.last_mut(), &item)
                        && last.get("type").and_then(Value::as_str) == Some("text")
                        && next.get("type").and_then(Value::as_str) == Some("text")
                        && last.get("marks") == next.get("marks")
                        && let (Some(Value::String(a)), Some(Value::String(b))) =
                            (last.get("text").cloned(), next.get("text"))
                    {
                        last.insert("text".into(), Value::String(format!("{a}{b}")));
                        continue;
                    }
                    out.push(item);
                }
                Value::Array(out)
            }
            other => other.clone(),
        }
    }
    let mut numbers = BTreeMap::new();
    let mut doc = Map::new();
    if let Value::Object(map) = content {
        for (key, value) in map {
            if key != "whiteboards" {
                doc.insert(key.clone(), walk(value, &mut numbers));
            }
        }
        if let Some(Value::Object(boards)) = map.get("whiteboards") {
            let mut mapped = Map::new();
            for (block, elements) in boards {
                let next = numbers.len();
                let n = *numbers.entry(block.clone()).or_insert(next);
                // Deleted elements are not exported, by design.
                let live: Map<String, Value> = match elements {
                    Value::Object(map) => map
                        .iter()
                        .filter(|(_, element)| element.get("isDeleted") != Some(&Value::Bool(true)))
                        .map(|(id, element)| (id.clone(), element.clone()))
                        .collect(),
                    _ => Map::new(),
                };
                mapped.insert(format!("#{n}"), Value::Object(live));
            }
            doc.insert("whiteboards".into(), Value::Object(mapped));
        }
    }
    Value::Object(doc)
}

/// Where two JSON values differ: a JSON pointer and both sides, at most `limit` entries.
pub fn json_differences(before: &Value, after: &Value, limit: usize) -> Vec<String> {
    fn short(value: &Value) -> String {
        let text = value.to_string();
        if text.chars().count() > 160 {
            format!("{}...", text.chars().take(160).collect::<String>())
        } else {
            text
        }
    }
    fn walk(path: &str, before: &Value, after: &Value, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit || before == after {
            return;
        }
        match (before, after) {
            (Value::Object(a), Value::Object(b)) => {
                let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
                keys.sort();
                keys.dedup();
                for key in keys {
                    let child = format!("{path}/{key}");
                    match (a.get(key), b.get(key)) {
                        (Some(x), Some(y)) => walk(&child, x, y, out, limit),
                        (Some(x), None) => out.push(format!("{child}: only before: {}", short(x))),
                        (None, Some(y)) => out.push(format!("{child}: only after: {}", short(y))),
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                for at in 0..a.len().max(b.len()) {
                    let child = format!("{path}/{at}");
                    match (a.get(at), b.get(at)) {
                        (Some(x), Some(y)) => walk(&child, x, y, out, limit),
                        (Some(x), None) => out.push(format!("{child}: only before: {}", short(x))),
                        (None, Some(y)) => out.push(format!("{child}: only after: {}", short(y))),
                        (None, None) => {}
                    }
                }
            }
            _ => out.push(format!(
                "{path}: before {} / after {}",
                short(before),
                short(after)
            )),
        }
    }
    let mut out = Vec::new();
    walk("", before, after, &mut out, limit);
    out
}
