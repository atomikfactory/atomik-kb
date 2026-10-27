//! The files of an AppFlowy backup as the reader sees them: the view table of `relation_map.json`,
//! the documents and databases by id, the `local_files` by name, and the small JSON helpers the
//! other modules share (the archive's JSON is lenient: numbers may be strings, objects may be
//! strings that hold JSON).

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::SourceFile;
use crate::paths;

/// The view table.
pub(super) const RELATION_MAP: &str = "relation_map.json";
/// The export's summary; its presence (with the view table) marks a backup.
pub(super) const METADATA: &str = "metadata.json";
/// What the export left out.
const EXPORT_WARNINGS: &str = "EXPORT_WARNINGS.json";
const DOCUMENTS_DIR: &str = "collab_jsons/documents/";
const DATABASES_DIR: &str = "collab_jsons/databases/";
/// Where the documents of database rows may also live:
/// `collab_jsons/databases/<view id>/row_documents/<document id>.json`.
const ROW_DOCUMENTS: &str = "/row_documents/";
const LOCAL_DIR: &str = "local_files/";

/// A view of the workspace: a page, a space or a database.
#[derive(Debug, Clone)]
pub(super) struct View {
    /// Its id (the key of the view table).
    pub id: String,
    /// Its name.
    pub name: String,
    /// 0 document, 1 grid, 2 board, 3 calendar; other values are unknown.
    pub layout: i64,
    /// The id of its parent (a view, the workspace, or nothing known).
    pub parent: String,
    /// The ids of its children, in order.
    pub children: Vec<String>,
    /// The id of its collab object: the document's id for a document.
    pub object: String,
    /// Creation time, seconds.
    pub created: i64,
    /// `icon`: its type (0 emoji) and value.
    pub icon: Option<(i64, String)>,
    /// `extra.is_space`.
    pub is_space: bool,
    /// `extra.database_id`.
    pub database_id: Option<String>,
    /// `extra.space_created_at`, milliseconds.
    pub space_created: Option<i64>,
    /// `extra.space_icon`.
    pub space_icon: Option<String>,
    /// `extra.cover` holds a cover.
    pub has_cover: bool,
    /// `extra.cover` as its type (lower case: a number or a name such as `color`) and its value.
    pub cover: Option<(String, String)>,
    /// `extra.is_database_container`: a database that is shown through one embedded view, which
    /// holds the database file.
    pub is_container: bool,
    /// `extra.embedded`: the view that holds the database of its container.
    pub embedded: bool,
    /// A template of a database's rows (`extra.database_row_template`, `database_view_id` or
    /// `template_id`): not a page.
    pub template: bool,
}

impl View {
    /// Whether it is a database view (not a document, not a space): a layout of AppFlowy's
    /// databases, or any other non-document layout that names a database.
    pub(super) fn is_database(&self) -> bool {
        !self.is_space
            && self.layout != 0
            && (matches!(self.layout, 1..=3) || self.database_id.is_some())
    }
}

/// The archive's files, indexed.
pub(super) struct Archive {
    /// The wrapping folder that was stripped (`""` or `"Name/"`).
    pub prefix: String,
    /// The views by id.
    pub views: HashMap<String, View>,
    /// The text files kept, by index (the normalised path in `names`).
    texts: Vec<String>,
    names: Vec<String>,
    /// Document id to file.
    documents: HashMap<String, usize>,
    /// Database view id to file.
    databases: HashMap<String, usize>,
    /// `local_files` names to the binary path as the shell gave it.
    pub local: HashMap<String, String>,
    /// Row id to document id, from `EXPORT_WARNINGS.json`.
    pub row_document_hints: HashMap<String, String>,
}

/// The wrapping folder around every path, the way `plan` strips it.
fn wrapping_prefix<'a>(all: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut top: Option<&str> = None;
    for path in all {
        match path.split_once('/') {
            Some((folder, _)) if top.is_none_or(|t| t == folder) => top = Some(folder),
            _ => return None,
        }
    }
    top.map(|t| format!("{t}/"))
}

fn usable(path: &str) -> bool {
    paths::is_safe_path(path) && !paths::is_hidden(path)
}

impl Archive {
    /// Recognises a backup: `relation_map.json` and `metadata.json` at the root (after the usual
    /// stripping of one wrapping folder) and a `views` object in the view table. Anything else
    /// gives the files back.
    pub(super) fn open(
        files: Vec<SourceFile>,
        binary_paths: &[String],
    ) -> Result<Self, Vec<SourceFile>> {
        let prefix = wrapping_prefix(
            files
                .iter()
                .map(|f| f.path.as_str())
                .chain(binary_paths.iter().map(String::as_str))
                .filter(|p| usable(p)),
        )
        .unwrap_or_default();
        let norm = |path: &str| {
            path.strip_prefix(prefix.as_str())
                .unwrap_or(path)
                .to_owned()
        };

        let at_root = |name: &str| {
            files
                .iter()
                .position(|f| usable(&f.path) && norm(&f.path) == name)
        };
        let (Some(_), Some(relation)) = (at_root(METADATA), at_root(RELATION_MAP)) else {
            return Err(files);
        };
        let Some(views) = serde_json::from_str::<Value>(&files[relation].text)
            .ok()
            .and_then(|map| map.get("views").and_then(Value::as_object).map(parse_views))
        else {
            return Err(files);
        };

        let mut names: Vec<String> = Vec::new();
        let mut texts: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for file in files {
            if !usable(&file.path) {
                continue;
            }
            let name = norm(&file.path);
            if seen.insert(name.clone()) {
                names.push(name);
                texts.push(file.text);
            }
        }
        let index_of = |wanted: &str| names.iter().position(|n| n == wanted);

        let mut documents = HashMap::new();
        let mut row_documents: Vec<(String, usize)> = Vec::new();
        let mut databases = HashMap::new();
        for (index, name) in names.iter().enumerate() {
            if let Some(id) = name
                .strip_prefix(DOCUMENTS_DIR)
                .and_then(|n| n.strip_suffix(".json"))
                .filter(|id| !id.contains('/'))
            {
                documents.insert(id.to_owned(), index);
            } else if let Some(id) = name
                .strip_prefix(DATABASES_DIR)
                .and_then(|n| n.strip_suffix(".json"))
                .and_then(|n| n.split_once(ROW_DOCUMENTS))
                .map(|(_, id)| id)
                .filter(|id| !id.is_empty() && !id.contains('/'))
            {
                row_documents.push((id.to_owned(), index));
            } else if let Some(id) = name
                .strip_prefix(DATABASES_DIR)
                .and_then(|n| n.strip_suffix(".json"))
                .filter(|id| !id.contains('/'))
            {
                databases.insert(id.to_owned(), index);
            }
        }

        // A document under `documents/` wins over a copy among the row documents.
        for (id, index) in row_documents {
            documents.entry(id).or_insert(index);
        }

        let mut local = HashMap::new();
        for path in binary_paths.iter().filter(|p| usable(p)) {
            if let Some(name) = norm(path).strip_prefix(LOCAL_DIR) {
                local.entry(name.to_owned()).or_insert_with(|| path.clone());
            }
        }

        let row_document_hints = index_of(EXPORT_WARNINGS)
            .and_then(|i| serde_json::from_str::<Value>(&texts[i]).ok())
            .map(|warnings| row_document_hints(&warnings))
            .unwrap_or_default();

        Ok(Self {
            prefix,
            views,
            texts,
            names,
            documents,
            databases,
            local,
            row_document_hints,
        })
    }

    /// The path of a file as the shell gave it (for warnings).
    pub(super) fn orig(&self, norm: &str) -> String {
        format!("{}{norm}", self.prefix)
    }

    /// The text and the path (as given) of a document, when the archive has it.
    pub(super) fn document(&self, id: &str) -> Option<(&str, String)> {
        let index = *self.documents.get(id)?;
        Some((&self.texts[index], self.orig(&self.names[index])))
    }

    /// Whether the archive has the document.
    pub(super) fn has_document(&self, id: &str) -> bool {
        self.documents.contains_key(id)
    }

    /// The path (as given) of the document of `id`: where the archive has it, else where it
    /// would be.
    pub(super) fn document_path(&self, id: &str) -> String {
        match self.documents.get(id) {
            Some(index) => self.orig(&self.names[*index]),
            None => self.orig(&format!("{DOCUMENTS_DIR}{id}.json")),
        }
    }

    /// The text and the path (as given) of the database file of a database view.
    pub(super) fn database(&self, view_id: &str) -> Option<(&str, String)> {
        let index = *self.databases.get(view_id)?;
        Some((&self.texts[index], self.orig(&self.names[index])))
    }

    /// The path (as given) of the view table, where the problems of the tree are reported.
    pub(super) fn tree_path(&self) -> String {
        self.orig(RELATION_MAP)
    }
}

/// The view table. A value that is not an object is not a view.
fn parse_views(views: &Map<String, Value>) -> HashMap<String, View> {
    let mut out = HashMap::with_capacity(views.len());
    for (key, value) in views {
        let Some(object) = value.as_object() else {
            continue;
        };
        let extra = object.get("extra").and_then(object_of).unwrap_or_default();
        let icon = object
            .get("icon")
            .and_then(Value::as_object)
            .and_then(|icon| {
                let value = icon.get("value").and_then(Value::as_str)?;
                Some((int(icon.get("ty")).unwrap_or(-1), value.to_owned()))
            });
        let children = object
            .get("children")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let set = |key: &str| match extra.get(key) {
            None | Some(Value::Null | Value::Bool(false)) => false,
            Some(Value::String(text)) => !text.is_empty(),
            Some(_) => true,
        };
        let template =
            set("database_row_template") || set("database_view_id") || set("template_id");
        let has_cover = match extra.get("cover") {
            None | Some(Value::Null) => false,
            Some(Value::Object(cover)) => !cover.is_empty(),
            Some(Value::String(cover)) => !cover.is_empty(),
            Some(_) => true,
        };
        let cover = extra.get("cover").and_then(object_of).map(|cover| {
            (
                text_of(cover.get("type"))
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase(),
                text_of(cover.get("value")).unwrap_or_default(),
            )
        });
        out.insert(
            key.clone(),
            View {
                id: key.clone(),
                name: text_of(object.get("name")).unwrap_or_default(),
                layout: int(object.get("layout")).unwrap_or(0),
                parent: text_of(object.get("parent_id")).unwrap_or_default(),
                children,
                object: text_of(object.get("collab_object_id")).unwrap_or_else(|| key.clone()),
                created: int(object.get("created_at")).unwrap_or(0),
                icon,
                is_space: flag(extra.get("is_space")),
                database_id: text_of(extra.get("database_id")).filter(|id| !id.is_empty()),
                space_created: int(extra.get("space_created_at")),
                space_icon: text_of(extra.get("space_icon")).filter(|icon| !icon.is_empty()),
                has_cover,
                cover,
                is_container: flag(extra.get("is_database_container")),
                embedded: flag(extra.get("embedded")),
                template,
            },
        );
    }
    out
}

/// `unregistered_row_document` samples: row id to document id.
fn row_document_hints(warnings: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(samples) = warnings.get("samples").and_then(Value::as_array) else {
        return out;
    };
    for sample in samples {
        if text_of(sample.get("kind")).as_deref() == Some("unregistered_row_document")
            && let (Some(row), Some(document)) = (
                text_of(sample.get("row_id")),
                text_of(sample.get("document_id")),
            )
        {
            out.insert(row, document);
        }
    }
    out
}

/// The id of the document of a database row: the UUIDv5 of the name `document_id` in the
/// namespace of the row id. `None` when the row id is not a UUID.
pub(super) fn row_document_id(row_id: &str) -> Option<String> {
    let namespace = Uuid::parse_str(row_id.trim()).ok()?;
    Some(
        Uuid::new_v5(&namespace, b"document_id")
            .hyphenated()
            .to_string(),
    )
}

// ----- lenient JSON readers --------------------------------------------------------------------

/// A string, or a number written out.
pub(super) fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// An integer: a number, or a string holding one.
pub(super) fn int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            // A float that is a whole number reads as one.
            .or_else(|| number.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A boolean: `true`, or the strings `"true"` and `"1"`.
pub(super) fn flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(text)) => matches!(text.trim().to_lowercase().as_str(), "true" | "1"),
        Some(Value::Number(number)) => number.as_i64() == Some(1),
        _ => false,
    }
}

/// An object, or a string that holds one.
pub(super) fn object_of(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) => match serde_json::from_str::<Value>(text).ok()? {
            Value::Object(map) => Some(map),
            _ => None,
        },
        _ => None,
    }
}

/// An array, or a string that holds one.
pub(super) fn array_of(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::Array(list) => Some(list.clone()),
        Value::String(text) => match serde_json::from_str::<Value>(text).ok()? {
            Value::Array(list) => Some(list),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn row_documents_have_a_fixed_derived_id() {
        // UUIDv5 of the name `document_id` in the namespace of the row id (checked against an
        // independent implementation).
        let id = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            row_document_id(id).as_deref(),
            Some("2ecb6d82-736c-542d-a43d-999f32031f0d")
        );
        assert_ne!(
            row_document_id("11111111-2222-4333-8444-555555555556"),
            row_document_id(id)
        );
        assert_eq!(row_document_id("not a uuid"), None);
    }

    #[test]
    fn lenient_readers() {
        assert_eq!(int(Some(&Value::from("12"))), Some(12));
        assert_eq!(int(Some(&Value::from(3.0))), Some(3));
        assert_eq!(int(Some(&Value::Null)), None);
        assert!(flag(Some(&Value::from("true"))));
        assert!(!flag(Some(&Value::from("no"))));
        assert!(object_of(&Value::from("{\"a\":1}")).is_some());
        assert!(object_of(&Value::from("[1]")).is_none());
        assert_eq!(array_of(&Value::from("[1,2]")).unwrap().len(), 2);
    }
}
