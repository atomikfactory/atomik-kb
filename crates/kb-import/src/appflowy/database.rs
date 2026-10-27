//! An AppFlowy database (`collab_jsons/databases/<view id>.json`): fields to planned properties,
//! cells to values, the view to a `pageList` block (ADR-0039, section 4.3 of the design).

use std::collections::{HashMap, HashSet};

use kb_types::properties::TAGS_NAME;
use kb_types::{ImportWarningCode, OptionColor, PropertyKind};
use serde_json::{Map, Value};

use super::archive::{array_of, flag, int, object_of, text_of};
use super::colors;
use super::delta::civil;
use super::env::Env;
use crate::frontmatter::MAX_NAME_CHARS;
use crate::json;
use crate::paths::truncate_chars;
use crate::placeholder;
use crate::values::{number_text, parse_bool, parse_number};
use crate::{ImportValue, PageTarget, PlannedConfig, PlannedOption, PlannedProperty};

/// Most options kept for one property.
const MAX_OPTIONS: usize = 500;
/// Most fields kept of one database.
const MAX_FIELDS: usize = 200;

/// A select option.
#[derive(Debug, Clone)]
pub(super) struct Opt {
    pub id: String,
    pub name: String,
    pub color: Option<OptionColor>,
}

/// A field (a column of the database).
#[derive(Debug, Clone)]
pub(super) struct Field {
    pub id: String,
    pub name: String,
    pub ty: i64,
    pub primary: bool,
    pub options: Vec<Opt>,
}

/// A row: its id and its cells by field id.
#[derive(Debug, Clone)]
pub(super) struct Row {
    pub id: String,
    pub cells: Map<String, Value>,
}

/// What the row's metadata says.
#[derive(Debug, Clone, Default)]
pub(super) struct Meta {
    /// `icon_url` when it is set.
    pub icon: Option<String>,
    /// `is_document_empty`.
    pub empty_document: bool,
}

/// The view of the database that is shown.
#[derive(Debug, Clone, Default)]
pub(super) struct Settings {
    pub field_order: Vec<String>,
    pub hidden: HashSet<String>,
    pub group_field: Option<String>,
    pub hidden_groups: Vec<String>,
    pub hide_ungrouped: bool,
    /// The view has filters or sorts, which are dropped.
    pub filtered: bool,
}

/// A database as read.
#[derive(Debug, Clone)]
pub(super) struct Database {
    pub id: String,
    pub fields: Vec<Field>,
    /// The rows, in the view's order (rows it does not list follow in file order).
    pub rows: Vec<Row>,
    pub metas: HashMap<String, Meta>,
    pub settings: Settings,
    /// The layout the database's own view entry says, in the view table's numbering (1 grid,
    /// 2 board, 3 calendar). The database file counts from 0 (grid, board, calendar), which is
    /// not the view table's numbering: it only serves when the view table has no known layout.
    pub layout_hint: Option<i64>,
}

impl Database {
    /// Reads the database file of the view `view_id`; `None` when it is not JSON of the shape.
    pub(super) fn parse(text: &str, view_id: &str) -> Option<Self> {
        let top: Value = serde_json::from_str(text).ok()?;
        let top = top.as_object()?;
        let id = text_of(top.get("database_id")).unwrap_or_default();

        let mut fields: Vec<Field> = Vec::new();
        for value in top.get("fields").and_then(Value::as_array)? {
            if fields.len() >= MAX_FIELDS {
                break;
            }
            if let Some(field) = parse_field(value) {
                fields.push(field);
            }
        }

        let mut by_id: HashMap<String, Row> = HashMap::new();
        let mut file_order: Vec<String> = Vec::new();
        for value in top
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(row) = value.as_object() else {
                continue;
            };
            let Some(row_id) = text_of(row.get("id")).filter(|id| !id.is_empty()) else {
                continue;
            };
            if by_id.contains_key(&row_id) {
                continue;
            }
            let cells = row.get("cells").and_then(object_of).unwrap_or_default();
            file_order.push(row_id.clone());
            by_id.insert(row_id.clone(), Row { id: row_id, cells });
        }

        let views = top.get("views").and_then(Value::as_array);
        let view = views
            .and_then(|list| {
                list.iter()
                    .find(|v| text_of(v.get("id")).as_deref() == Some(view_id))
                    .or_else(|| list.first())
            })
            .and_then(Value::as_object);
        let settings = view.map(read_settings).unwrap_or_default();
        let layout_hint = view
            .and_then(|v| int(v.get("layout")))
            .filter(|layout| (0..=2).contains(layout))
            .map(|layout| layout + 1);

        let mut rows: Vec<Row> = Vec::with_capacity(by_id.len());
        let listed: Vec<String> = view
            .and_then(|v| v.get("row_orders"))
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(|r| text_of(r.get("id"))).collect())
            .unwrap_or_default();
        for row_id in listed.into_iter().chain(file_order) {
            if let Some(row) = by_id.remove(&row_id) {
                rows.push(row);
            }
        }

        let mut metas = HashMap::new();
        if let Some(Value::Object(list)) = top.get("row_metas") {
            for (row_id, meta) in list {
                let icon = meta
                    .get("icon_url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|icon| !icon.is_empty())
                    .map(str::to_owned);
                metas.insert(
                    row_id.clone(),
                    Meta {
                        icon,
                        empty_document: flag(meta.get("is_document_empty")),
                    },
                );
            }
        }
        Some(Self {
            id,
            fields,
            rows,
            metas,
            settings,
            layout_hint,
        })
    }

    /// The primary field: the one flagged, else the first text field, else the first.
    pub(super) fn primary(&self) -> Option<&Field> {
        self.fields
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.fields.iter().find(|f| f.ty == 0))
            .or_else(|| self.fields.first())
    }

    fn field(&self, id: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// The fields that become properties, in field order: everything but the primary field and
    /// the computed kinds.
    pub(super) fn property_fields(&self) -> Vec<&Field> {
        let primary = self.primary().map(|f| f.id.as_str());
        self.fields
            .iter()
            .filter(|f| Some(f.id.as_str()) != primary && property_kind(f.ty).is_some())
            .collect()
    }

    /// The title of a row: the primary field's text.
    pub(super) fn title(&self, row: &Row) -> String {
        let Some(field) = self.primary() else {
            return String::new();
        };
        row.cells
            .get(&field.id)
            .map(|cell| cell_text(field, cell))
            .unwrap_or_default()
    }

    /// Whether every row a relation field points to is a planned page.
    pub(super) fn relation_complete(&self, field: &Field, env: &Env) -> bool {
        self.rows.iter().all(|row| {
            row.cells
                .get(&field.id)
                .map(relation_ids)
                .unwrap_or_default()
                .iter()
                .all(|id| env.rows.contains_key(id))
        })
    }

    /// The fields the view shows as columns, in its order.
    pub(super) fn visible_fields(&self) -> Vec<&Field> {
        let mut ordered: Vec<&Field> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for id in &self.settings.field_order {
            if let Some(field) = self.field(id)
                && seen.insert(field.id.as_str())
            {
                ordered.push(field);
            }
        }
        for field in &self.fields {
            if seen.insert(field.id.as_str()) {
                ordered.push(field);
            }
        }
        ordered
            .into_iter()
            .filter(|f| !self.settings.hidden.contains(&f.id))
            .collect()
    }
}

fn parse_field(value: &Value) -> Option<Field> {
    let field = value.as_object()?;
    let id = text_of(field.get("id")).filter(|id| !id.is_empty())?;
    let ty = int(field.get("field_type")).unwrap_or(0);
    let options = field
        .get("type_options")
        .and_then(object_of)
        .and_then(|all| all.get(&ty.to_string()).and_then(object_of))
        .map(|own| options_of(&own))
        .unwrap_or_default();
    Some(Field {
        id,
        name: text_of(field.get("name"))
            .unwrap_or_default()
            .trim()
            .to_owned(),
        ty,
        primary: flag(field.get("is_primary")),
        options,
    })
}

/// The options of a select-like field: `content` (a JSON string of `{options, disable_color}`)
/// or an `options` list right in the type options.
fn options_of(type_options: &Map<String, Value>) -> Vec<Opt> {
    let content = type_options.get("content").and_then(object_of);
    let list = content
        .as_ref()
        .and_then(|c| c.get("options"))
        .or_else(|| type_options.get("options"))
        .and_then(array_of)
        .unwrap_or_default();
    list.iter()
        .filter_map(|option| {
            let option = option.as_object()?;
            Some(Opt {
                id: text_of(option.get("id"))?,
                name: text_of(option.get("name"))
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                color: text_of(option.get("color")).and_then(|c| colors::option_color(&c)),
            })
        })
        .take(MAX_OPTIONS)
        .collect()
}

fn read_settings(view: &Map<String, Value>) -> Settings {
    let mut settings = Settings::default();
    if let Some(orders) = view.get("field_orders").and_then(Value::as_array) {
        settings.field_order = orders.iter().filter_map(|o| text_of(o.get("id"))).collect();
    }
    if let Some(Value::Object(per_field)) = view.get("field_settings") {
        for (id, field) in per_field {
            if int(field.get("visibility")) == Some(2) {
                settings.hidden.insert(id.clone());
            }
        }
    }
    if let Some(group) = view
        .get("group_settings")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
    {
        settings.group_field = text_of(group.get("field_id"));
        if let Some(groups) = group.get("groups").and_then(Value::as_array) {
            for entry in groups {
                if matches!(entry.get("visible"), Some(Value::Bool(false)))
                    && let Some(id) = text_of(entry.get("id"))
                {
                    settings.hidden_groups.push(id);
                }
            }
        }
    }
    if let Some(Value::Object(layouts)) = view.get("layout_settings") {
        settings.hide_ungrouped = layouts
            .values()
            .any(|layout| flag(layout.get("hide_ungrouped_column")));
    }
    let non_empty = |key: &str| {
        view.get(key)
            .and_then(array_of)
            .is_some_and(|list| !list.is_empty())
    };
    settings.filtered = non_empty("filters") || non_empty("sorts");
    settings
}

/// The property kind of a field type; `None` for the computed ones that are not imported.
pub(super) fn property_kind(ty: i64) -> Option<PropertyKind> {
    Some(match ty {
        1 | 13 => PropertyKind::Number,
        2 => PropertyKind::Date,
        3 => PropertyKind::Select,
        4 => PropertyKind::MultiSelect,
        5 => PropertyKind::Checkbox,
        6 => PropertyKind::Url,
        7 => PropertyKind::Checklist,
        8 | 9 => return None,
        10 => PropertyKind::Relation,
        14 => PropertyKind::Files,
        // Text, summary, translation and the kinds this reader does not know.
        _ => PropertyKind::Text,
    })
}

// ----- cells -----------------------------------------------------------------------------------

/// The cell's `data` as text: a string, a number, or an object's `data`.
fn data_text(cell: &Value) -> String {
    match cell.get("data") {
        Some(Value::Object(inner)) => text_of(inner.get("data")).unwrap_or_default(),
        other => text_of(other).unwrap_or_default(),
    }
}

/// The text a cell shows (the title of a row, the text of a text property).
pub(super) fn cell_text(field: &Field, cell: &Value) -> String {
    let data = data_text(cell);
    match field.ty {
        3 | 4 => select_names(field, &data).join(", "),
        5 => match parse_bool(&data) {
            Some(true) if !data.trim().is_empty() => "Yes".to_owned(),
            Some(false) if !data.trim().is_empty() => "No".to_owned(),
            _ => String::new(),
        },
        _ => data.trim().to_owned(),
    }
}

/// The names of the options a select cell lists (ids separated by commas).
fn select_names(field: &Field, data: &str) -> Vec<String> {
    data.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .filter_map(|id| field.options.iter().find(|o| o.id == id))
        .map(|o| o.name.clone())
        .collect()
}

/// The row ids a relation cell lists: a JSON list, or ids separated by commas.
pub(super) fn relation_ids(cell: &Value) -> Vec<String> {
    let raw = cell.get("data");
    let list: Vec<String> = match raw {
        Some(Value::Array(list)) => list.iter().filter_map(|v| text_of(Some(v))).collect(),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.starts_with('[') {
                array_of(&Value::String(text.to_owned()))
                    .map(|list| list.iter().filter_map(|v| text_of(Some(v))).collect())
                    .unwrap_or_default()
            } else {
                text.split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
        }
        _ => Vec::new(),
    };
    list
}

/// A number as AppFlowy stores it: a decimal string, perhaps with formatting around it.
fn number_of(data: &str) -> Option<f64> {
    parse_number(data).or_else(|| {
        let digits: String = data
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | 'E'))
            .collect();
        parse_number(&digits)
    })
}

/// The value of a cell for the property of its field. `relation` says the field is a relation
/// property (every target is planned). A value that does not fit is dropped with a warning.
pub(super) fn cell_value(
    field: &Field,
    cell: &Value,
    relation: bool,
    env: &mut Env,
    path: &str,
) -> Option<ImportValue> {
    let data = data_text(cell);
    let trimmed = data.trim();
    let invalid = |env: &mut Env| {
        env.warn(path, ImportWarningCode::InvalidValue, Some(&field.name));
        None
    };
    match field.ty {
        1 => {
            if trimmed.is_empty() {
                return None;
            }
            match number_of(trimmed) {
                Some(n) => Some(ImportValue::Number(n)),
                None => invalid(env),
            }
        }
        13 => {
            if trimmed.is_empty() {
                return None;
            }
            match number_of(trimmed) {
                Some(n) => Some(ImportValue::Number(n)),
                None => invalid(env),
            }
        }
        2 => {
            if trimmed.is_empty() {
                return None;
            }
            let with_time = flag(cell.get("include_time"));
            match trimmed
                .parse::<i64>()
                .ok()
                .and_then(|seconds| civil(seconds, with_time))
            {
                Some((date, time)) => Some(ImportValue::Date { date, time }),
                None => invalid(env),
            }
        }
        3 => {
            let names = select_names(field, trimmed);
            if trimmed.is_empty() {
                return None;
            }
            match names.into_iter().next() {
                Some(name) => Some(ImportValue::Text(name)),
                None => invalid(env),
            }
        }
        4 => {
            if trimmed.is_empty() {
                return None;
            }
            let names = select_names(field, trimmed);
            let listed = trimmed
                .split(',')
                .filter(|id| !id.trim().is_empty())
                .count();
            if names.len() < listed {
                env.warn(path, ImportWarningCode::InvalidValue, Some(&field.name));
            }
            (!names.is_empty()).then_some(ImportValue::List(names))
        }
        5 => {
            if trimmed.is_empty() {
                return None;
            }
            match parse_bool(trimmed) {
                Some(checked) => Some(ImportValue::Bool(checked)),
                None => invalid(env),
            }
        }
        7 => {
            let checklist = cell
                .get("data")
                .and_then(object_of)
                .or_else(|| object_of(&Value::String(data.clone())))?;
            let selected: HashSet<String> = checklist
                .get("selected_option_ids")
                .and_then(array_of)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| text_of(Some(v)))
                .collect();
            let items: Vec<(bool, String)> = checklist
                .get("options")
                .and_then(array_of)
                .unwrap_or_default()
                .iter()
                .filter_map(|option| {
                    let name = text_of(option.get("name"))?;
                    let id = text_of(option.get("id")).unwrap_or_default();
                    Some((selected.contains(&id), name))
                })
                .collect();
            (!items.is_empty()).then_some(ImportValue::Checklist(items))
        }
        10 => {
            let ids = relation_ids(cell);
            if ids.is_empty() {
                return None;
            }
            let mut targets: Vec<PageTarget> = Vec::new();
            for id in &ids {
                if let Some(index) = env.rows.get(id).copied() {
                    let target = PageTarget::Planned(index);
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
            if relation {
                (!targets.is_empty()).then_some(ImportValue::Pages(targets))
            } else {
                // A relation that does not stay one: the titles of the rows it does reach.
                let titles: Vec<&str> = targets
                    .iter()
                    .filter_map(|t| match t {
                        PageTarget::Planned(i) => Some(env.title(*i)),
                        PageTarget::Title(_) => None,
                    })
                    .collect();
                (!titles.is_empty()).then(|| ImportValue::Text(titles.join(", ")))
            }
        }
        14 => {
            let list = cell
                .get("data")
                .and_then(array_of)
                .or_else(|| array_of(&Value::String(data.clone())))?;
            let mut files: Vec<String> = Vec::new();
            let mut remote = false;
            for item in &list {
                let url = text_of(item.get("url")).unwrap_or_default();
                let name = text_of(item.get("name"));
                match env.local_file(&url, name.as_deref()) {
                    Some(path) => {
                        if !files.contains(&path) {
                            files.push(path);
                        }
                    }
                    None => remote = true,
                }
            }
            if remote {
                env.warn(
                    path,
                    ImportWarningCode::UnsupportedContent,
                    Some(&field.name),
                );
            }
            (!files.is_empty()).then_some(ImportValue::Files(files))
        }
        // Text, URL, summary, translation, unknown kinds.
        _ => (!trimmed.is_empty()).then(|| ImportValue::Text(trimmed.to_owned())),
    }
}

/// A value as text, for a property that several databases made text.
pub(super) fn value_text(value: &ImportValue, env: &Env) -> String {
    match value {
        ImportValue::Text(text) => text.clone(),
        ImportValue::Number(n) => number_text(*n),
        ImportValue::Bool(b) => if *b { "Yes" } else { "No" }.to_owned(),
        ImportValue::Date { date, time } => match time {
            Some(time) => format!("{date} {time}"),
            None => date.clone(),
        },
        ImportValue::List(items) => items.join(", "),
        ImportValue::Pages(targets) => targets
            .iter()
            .map(|t| match t {
                PageTarget::Planned(i) => env.title(*i).to_owned(),
                PageTarget::Title(title) => title.clone(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        ImportValue::Files(files) => files
            .iter()
            .map(|f| crate::paths::file_name(f))
            .collect::<Vec<_>>()
            .join(", "),
        ImportValue::Checklist(items) => items
            .iter()
            .map(|(done, text)| format!("[{}] {text}", if *done { "x" } else { " " }))
            .collect::<Vec<_>>()
            .join("; "),
        ImportValue::Place { name, .. } => name.clone(),
    }
}

// ----- properties ------------------------------------------------------------------------------

/// The name a property is planned under: `Tags` for any spelling of it, else the name cut to the
/// schema's limit; a field without a name is `Property <n>`.
pub(super) fn property_name(field: &Field, position: usize) -> String {
    let name = field.name.trim();
    if name.is_empty() {
        return format!("Property {}", position + 1);
    }
    if name.eq_ignore_ascii_case(TAGS_NAME) {
        TAGS_NAME.to_owned()
    } else {
        truncate_chars(name, MAX_NAME_CHARS)
    }
}

struct Prop {
    name: String,
    kind: Option<PropertyKind>,
    options: Vec<PlannedOption>,
    seen: HashSet<String>,
}

/// The planned properties: one per name over the whole import, with one kind each (a name met
/// with different kinds is text).
#[derive(Default)]
pub(super) struct Props {
    by_name: HashMap<String, usize>,
    list: Vec<Prop>,
}

impl Props {
    /// Records a field of a database.
    pub(super) fn observe(&mut self, name: &str, kind: PropertyKind, options: &[Opt]) {
        let key = name.to_lowercase();
        let index = match self.by_name.get(&key) {
            Some(index) => *index,
            None => {
                self.list.push(Prop {
                    name: name.to_owned(),
                    kind: None,
                    options: Vec::new(),
                    seen: HashSet::new(),
                });
                self.by_name.insert(key, self.list.len() - 1);
                self.list.len() - 1
            }
        };
        let prop = &mut self.list[index];
        prop.kind = Some(match prop.kind {
            None => kind,
            Some(seen) if seen == kind => kind,
            Some(_) => PropertyKind::Text,
        });
        for option in options {
            if option.name.is_empty()
                || prop.options.len() >= MAX_OPTIONS
                || !prop.seen.insert(option.name.to_lowercase())
            {
                continue;
            }
            prop.options.push(PlannedOption {
                name: truncate_chars(&option.name, MAX_NAME_CHARS),
                color: option.color,
                group: None,
            });
        }
    }

    /// The name a property is planned under, for a name as a field spells it (names match
    /// case-insensitively; the first spelling wins).
    pub(super) fn canonical(&self, name: &str) -> Option<&str> {
        self.by_name
            .get(&name.to_lowercase())
            .map(|index| self.list[*index].name.as_str())
    }

    /// The final kind of every property by lowercase name (what several databases that disagree
    /// made of it).
    pub(super) fn kinds(&self) -> HashMap<String, PropertyKind> {
        self.list
            .iter()
            .map(|prop| {
                (
                    prop.name.to_lowercase(),
                    prop.kind.unwrap_or(PropertyKind::Text),
                )
            })
            .collect()
    }

    /// The planned properties, in order of first appearance.
    pub(super) fn finish(self) -> Vec<PlannedProperty> {
        self.list
            .into_iter()
            .map(|prop| {
                let kind = prop.kind.unwrap_or(PropertyKind::Text);
                PlannedProperty {
                    name: prop.name,
                    kind,
                    options: if kind.has_options() {
                        prop.options
                    } else {
                        Vec::new()
                    },
                    config: (kind == PropertyKind::Relation).then_some(PlannedConfig::Relation {
                        two_way: false,
                        pair_name: None,
                    }),
                }
            })
            .collect()
    }
}

// ----- the view --------------------------------------------------------------------------------

/// The `pageList` block of a database page. `layout` is the view's layout in the view table
/// (1 grid, 2 board, 3 calendar, anything else unknown). A board groups by the view's group field
/// when that is a single select, else it is a table.
pub(super) fn page_list(
    db: &Database,
    layout: i64,
    props: &HashMap<String, String>,
    env: &mut Env,
    path: &str,
) -> Value {
    let mut group: Option<&Field> = None;
    let mut table = true;
    match layout {
        1 => {}
        2 => {
            let grouped = db
                .settings
                .group_field
                .as_deref()
                .and_then(|id| db.field(id))
                .filter(|f| f.ty == 3)
                .or_else(|| db.fields.iter().find(|f| f.ty == 3));
            match grouped {
                Some(field) => {
                    group = Some(field);
                    table = false;
                }
                None => env.warn(path, ImportWarningCode::UnsupportedContent, Some("board")),
            }
        }
        3 => env.warn(
            path,
            ImportWarningCode::UnsupportedContent,
            Some("calendar"),
        ),
        other => env.warn(
            path,
            ImportWarningCode::UnsupportedContent,
            Some(&format!("layout {other}")),
        ),
    }
    if db.settings.filtered {
        env.warn(
            path,
            ImportWarningCode::UnsupportedContent,
            Some("filters and sorts"),
        );
    }

    let primary = db.primary().map(|f| f.id.clone());
    let columns: Vec<Value> = db
        .visible_fields()
        .into_iter()
        .filter(|f| Some(&f.id) != primary.as_ref() && property_kind(f.ty).is_some())
        .filter_map(|f| props.get(&f.id))
        .map(|name| Value::String(name.clone()))
        .collect();
    let mut view = json::attrs([
        (placeholder::NAMES, Value::Bool(true)),
        ("source", serde_json::json!({"kind": "children"})),
        (
            "layout",
            Value::String(if table { "table" } else { "board" }.to_owned()),
        ),
        ("properties", Value::Array(columns)),
    ]);
    if let Some(field) = group
        && let Some(name) = props.get(&field.id)
    {
        view.insert("groupBy".into(), Value::String(name.clone()));
        let mut hidden: Vec<Value> = Vec::new();
        for id in &db.settings.hidden_groups {
            let entry = if *id == field.id {
                Some("none".to_owned())
            } else {
                field
                    .options
                    .iter()
                    .find(|o| o.id == *id)
                    .map(|o| o.name.clone())
            };
            if let Some(entry) = entry
                && !hidden.contains(&Value::String(entry.clone()))
            {
                hidden.push(Value::String(entry));
            }
        }
        if db.settings.hide_ungrouped && !hidden.contains(&Value::String("none".into())) {
            hidden.push(Value::String("none".into()));
        }
        if !hidden.is_empty() {
            view.insert("hiddenGroups".into(), Value::Array(hidden));
        }
    }
    json::element("pageList", view, Vec::new())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn field(ty: i64, options: &[(&str, &str)]) -> Field {
        Field {
            id: "f".into(),
            name: "F".into(),
            ty,
            primary: false,
            options: options
                .iter()
                .map(|(id, name)| Opt {
                    id: (*id).into(),
                    name: (*name).into(),
                    color: None,
                })
                .collect(),
        }
    }

    fn cell(data: Value) -> Value {
        serde_json::json!({"field_type": 0, "data": data})
    }

    #[test]
    fn numbers_tolerate_formatting() {
        assert_eq!(number_of("12.5"), Some(12.5));
        assert_eq!(number_of("$1,234.50"), Some(1234.5));
        assert_eq!(number_of("-3"), Some(-3.0));
        assert_eq!(number_of("abc"), None);
    }

    #[test]
    fn relation_ids_in_every_form() {
        assert_eq!(relation_ids(&cell("a, b".into())), ["a", "b"]);
        assert_eq!(relation_ids(&cell(r#"["a","b"]"#.into())), ["a", "b"]);
        assert_eq!(relation_ids(&cell(serde_json::json!(["a"]))), ["a"]);
        assert!(relation_ids(&cell(Value::Null)).is_empty());
    }

    #[test]
    fn select_cells_by_option_id() {
        let f = field(4, &[("o1", "One"), ("o2", "Two")]);
        assert_eq!(select_names(&f, "o2,o1,zz"), ["Two", "One"]);
        assert_eq!(cell_text(&f, &cell("o1,o2".into())), "One, Two");
    }

    #[test]
    fn properties_merge_by_name_and_conflicts_are_text() {
        let mut props = Props::default();
        let a = [Opt {
            id: "1".into(),
            name: "Open".into(),
            color: Some(OptionColor::Green),
        }];
        props.observe("Status", PropertyKind::Select, &a);
        props.observe("status", PropertyKind::Select, &a);
        props.observe("Size", PropertyKind::Number, &[]);
        props.observe("size", PropertyKind::Text, &[]);
        let list = props.finish();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].options.len(), 1);
        assert_eq!(list[0].options[0].color, Some(OptionColor::Green));
        assert_eq!(list[1].kind, PropertyKind::Text);
    }

    #[test]
    fn names_are_cut_and_tags_is_one_name() {
        let mut f = field(0, &[]);
        f.name = "tags".into();
        assert_eq!(property_name(&f, 0), TAGS_NAME);
        f.name = "x".repeat(300);
        assert_eq!(property_name(&f, 0).chars().count(), MAX_NAME_CHARS);
        f.name = "  ".into();
        assert_eq!(property_name(&f, 2), "Property 3");
    }
}
