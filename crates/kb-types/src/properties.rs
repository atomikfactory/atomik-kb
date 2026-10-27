//! Typed page properties (ADR-0026): the kinds, values, schema entries and filters of the
//! API, and the limits the core enforces and the TypeScript client mirrors. Not glob
//! re-exported as a module (the limits are `kb_types::properties::...`); the types are
//! re-exported at the crate root.
//!
//! Every page can carry properties. A space owns the schema (`PropertyDef`s, in its structure
//! document), a page owns its values (one per property). Kinds are fixed at creation.
//!
//! JSON shapes (camelCase fields, as everywhere in the API):
//!
//! ```text
//! PropertyValue  {"kind": "text",         "value": "Quarterly plan"}
//!                {"kind": "number",       "value": 3.5}
//!                {"kind": "checkbox",     "value": true}
//!                {"kind": "date",         "value": {"date": "2026-10-07", "time": "14:30"}}   (time optional)
//!                {"kind": "select",       "value": "<option id>"}
//!                {"kind": "status",       "value": "<option id>"}
//!                {"kind": "multi_select", "value": ["<option id>", "<option id>"]}
//!                {"kind": "url",          "value": "https://example.com"}
//!                {"kind": "email",        "value": "ada@example.com"}
//!                {"kind": "phone",        "value": "+1 555 0100"}
//!                {"kind": "files",        "value": ["<attachment id>", "<attachment id>"]}
//!                {"kind": "checklist",    "value": [{"id": "<item id>", "text": "Pack", "done": false}]}
//!                {"kind": "place",        "value": {"name": "Café de Flore", "lat": 48.854, "lon": 2.333}}   (lat and lon optional, both or neither)
//!                {"kind": "person",       "value": ["<account id>", "<account id>"]}   (members of the space when written, at most 20)
//!                {"kind": "relation",     "value": ["<page id>", "<page id>"]}   (same space, at most 100)
//!                {"kind": "rollup",       "value": {"function": "sum", "result": {"type": "number", "data": 12.5}}}   (computed, read-only)
//!                {"kind": "created_time" | "last_edited_time", "value": 1760000000000}   (Unix ms, computed, read-only)
//!                {"kind": "created_by" | "last_edited_by",     "value": "<device id>"}    (computed, read-only)
//!                {"kind": "formula",      "value": {"type": "number", "data": 6}}   (computed, read-only, ADR-0055)
//! PropertyFilter {"type": "HasValue"}  {"type": "Equals", "data": <PropertyValue>}
//!                {"type": "ContainsOption", "data": "<option id>"}  {"type": "TextContains", "data": "abc"}
//!                {"type": "DateBetween", "data": {"from": "2026-10-01", "to": "2026-10-31"}}
//!                {"type": "Checked", "data": true}  {"type": "ChecklistComplete", "data": false}
//!                {"type": "InstantBetween", "data": {"from": 1760000000000, "to": 1760086400000}}
//!                {"type": "RelatesTo", "data": "<page id>"}
//!                {"type": "ContainsPerson", "data": "<account id>"}  {"type": "ContainsMe"}
//! PropertyConfig {"kind": "relation", "twoWay": true, "pairedPropertyId": "<property id>"}
//!                {"kind": "rollup", "relationPropertyId": "<property id>", "target": "title" | "<property id>", "function": "sum"}
//!                {"kind": "formula", "expression": "prop(\"<property id>\") * 2", "resultType": {"base": "number", "list": false}}
//! ```

use serde::{Deserialize, Serialize};

use crate::ids::{
    AccountId, AttachmentId, ChecklistItemId, DeviceId, OptionId, PageId, PropertyId,
};

/// The fixed id of the built-in Tags property (`0000000a-0000-4000-8000-000000000001`): a
/// multi-select that exists in every space. It is listed first by `PropertySchemaGet` even
/// before anything is written, is written to the structure document on the first change
/// that needs it, and cannot be deleted.
pub const TAGS_PROPERTY_ID: PropertyId = PropertyId::from_bytes([
    0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
]);
/// The name the Tags property starts with.
pub const TAGS_NAME: &str = "Tags";

/// Most properties a space's schema may hold, Tags included.
pub const MAX_PROPERTIES: usize = 100;
/// Most options one select, multi-select or status property may hold.
pub const MAX_OPTIONS: usize = 500;
/// Longest property or option name, in characters (after trimming).
pub const MAX_NAME_CHARS: usize = 100;
/// Longest `text` value, in characters.
pub const MAX_TEXT_CHARS: usize = 10_000;
/// Longest `url` value, in characters.
pub const MAX_URL_CHARS: usize = 2_048;
/// Longest `email` value, in characters.
pub const MAX_EMAIL_CHARS: usize = 254;
/// Longest `phone` value, in characters.
pub const MAX_PHONE_CHARS: usize = 64;
/// Most options one `multi_select` value may list.
pub const MAX_SELECTED_OPTIONS: usize = 100;
/// Longest filter text of [`PropertyFilter::TextContains`], in characters.
pub const MAX_FILTER_CHARS: usize = 200;
/// Most attachments one `files` value may list.
pub const MAX_FILES: usize = 50;
/// Most items one `checklist` value may hold.
pub const MAX_CHECKLIST_ITEMS: usize = 200;
/// Longest text of one checklist item, in characters.
pub const MAX_CHECKLIST_TEXT_CHARS: usize = 500;
/// Longest name (label or address) of a `place` value, in characters.
pub const MAX_PLACE_NAME_CHARS: usize = 500;
/// Most pages one `relation` value may list.
pub const MAX_RELATED_PAGES: usize = 100;
/// Most people one `person` value may list (M13 slice 2, ADR-0049).
pub const MAX_PEOPLE: usize = 20;
/// The text a rollup's `target` has when it is the related pages' title.
pub const ROLLUP_TITLE_TARGET: &str = "title";
/// Longest formula expression, in characters (ADR-0055, `docs/architecture/formulas.md`).
pub const MAX_FORMULA_CHARS: usize = 4_000;
/// Deepest nesting of a formula (parentheses, calls, lists).
pub const MAX_FORMULA_DEPTH: usize = 64;
/// Most items of a list inside a formula or in its result.
pub const MAX_FORMULA_LIST_ITEMS: usize = 10_000;
/// Most evaluation steps of one formula on one page, nested formulas included.
pub const MAX_FORMULA_STEPS: usize = 100_000;
/// The id a stored formula expression uses for the page title: `prop("title")`.
pub const FORMULA_TITLE_ID: &str = "title";

/// The kind of a property. Fixed when the property is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PropertyKind {
    /// Free text.
    Text,
    /// A finite number.
    Number,
    /// One option of the property's list.
    Select,
    /// Several options of the property's list.
    MultiSelect,
    /// A floating local date, optionally with a time of day.
    Date,
    /// On or off.
    Checkbox,
    /// An absolute web, mail or phone link.
    Url,
    /// One option of the property's list, each belonging to a group (to do, in progress, done).
    Status,
    /// An email address, stored as typed.
    Email,
    /// A phone number, stored as typed.
    Phone,
    /// An ordered list of attachments (files and media).
    Files,
    /// A list of items, each with a text and a done flag.
    Checklist,
    /// A named place, optionally with coordinates.
    Place,
    /// A list of people: accounts that are members of the space (in a personal space, the person's
    /// own account), at most [`MAX_PEOPLE`] (M13 slice 2, ADR-0049).
    Person,
    /// An ordered list of pages of the same space, optionally two-way (ADR-0027).
    Relation,
    /// A summary of a property of the related pages (computed, read-only, ADR-0027).
    Rollup,
    /// When the page was created (computed, read-only).
    CreatedTime,
    /// When the page was last edited (computed, read-only).
    LastEditedTime,
    /// The device that created the page (computed, read-only).
    CreatedBy,
    /// The device that last edited the page (computed, read-only).
    LastEditedBy,
    /// An expression over the page's properties and its related pages (computed, read-only,
    /// ADR-0055).
    Formula,
}

impl PropertyKind {
    /// Every kind, in the order the UI lists them.
    pub const ALL: [PropertyKind; 21] = [
        PropertyKind::Text,
        PropertyKind::Number,
        PropertyKind::Select,
        PropertyKind::MultiSelect,
        PropertyKind::Date,
        PropertyKind::Checkbox,
        PropertyKind::Url,
        PropertyKind::Status,
        PropertyKind::Email,
        PropertyKind::Phone,
        PropertyKind::Files,
        PropertyKind::Checklist,
        PropertyKind::Place,
        PropertyKind::Person,
        PropertyKind::Relation,
        PropertyKind::Rollup,
        PropertyKind::CreatedTime,
        PropertyKind::LastEditedTime,
        PropertyKind::CreatedBy,
        PropertyKind::LastEditedBy,
        PropertyKind::Formula,
    ];

    /// The name stored in documents and used in JSON (`snake_case`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            PropertyKind::Text => "text",
            PropertyKind::Number => "number",
            PropertyKind::Select => "select",
            PropertyKind::MultiSelect => "multi_select",
            PropertyKind::Date => "date",
            PropertyKind::Checkbox => "checkbox",
            PropertyKind::Url => "url",
            PropertyKind::Status => "status",
            PropertyKind::Email => "email",
            PropertyKind::Phone => "phone",
            PropertyKind::Files => "files",
            PropertyKind::Checklist => "checklist",
            PropertyKind::Place => "place",
            PropertyKind::Person => "person",
            PropertyKind::Relation => "relation",
            PropertyKind::Rollup => "rollup",
            PropertyKind::CreatedTime => "created_time",
            PropertyKind::LastEditedTime => "last_edited_time",
            PropertyKind::CreatedBy => "created_by",
            PropertyKind::LastEditedBy => "last_edited_by",
            PropertyKind::Formula => "formula",
        }
    }

    /// The kind with this name; `None` for a name this version does not know.
    #[must_use]
    pub fn parse(name: &str) -> Option<PropertyKind> {
        PropertyKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == name)
    }

    /// Whether the value is computed (from the page itself, or for a rollup from its related
    /// pages): such a property can be added to the schema, but a page stores no value for it and
    /// `PagePropertySet` refuses it.
    #[must_use]
    pub const fn is_computed(self) -> bool {
        matches!(
            self,
            PropertyKind::Rollup
                | PropertyKind::CreatedTime
                | PropertyKind::LastEditedTime
                | PropertyKind::CreatedBy
                | PropertyKind::LastEditedBy
                | PropertyKind::Formula
        )
    }

    /// Whether a rollup may summarise a property of this kind (ADR-0027): not files, relations,
    /// rollups or the computed kinds (a rollup of a rollup would need a dependency order).
    #[must_use]
    pub const fn is_rollup_target(self) -> bool {
        !self.is_computed() && !matches!(self, PropertyKind::Files | PropertyKind::Relation)
    }

    /// Whether the property keeps a list of options (`select`, `multi_select`, `status`).
    #[must_use]
    pub const fn has_options(self) -> bool {
        matches!(
            self,
            PropertyKind::Select | PropertyKind::MultiSelect | PropertyKind::Status
        )
    }
}

/// The named colours of an option: the editor's palette (ADR-0016, amendments of 2026-10-06 and
/// 2026-10-07), a theme-aware name and not a fixed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum OptionColor {
    /// Gray.
    Gray,
    /// Brown.
    Brown,
    /// Orange.
    Orange,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Purple.
    Purple,
    /// Pink.
    Pink,
    /// Red.
    Red,
}

impl OptionColor {
    /// The nine colours, in the order the pickers list them.
    pub const ALL: [OptionColor; 9] = [
        OptionColor::Gray,
        OptionColor::Brown,
        OptionColor::Orange,
        OptionColor::Yellow,
        OptionColor::Green,
        OptionColor::Blue,
        OptionColor::Purple,
        OptionColor::Pink,
        OptionColor::Red,
    ];

    /// The name stored in documents and used in JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            OptionColor::Gray => "gray",
            OptionColor::Brown => "brown",
            OptionColor::Orange => "orange",
            OptionColor::Yellow => "yellow",
            OptionColor::Green => "green",
            OptionColor::Blue => "blue",
            OptionColor::Purple => "purple",
            OptionColor::Pink => "pink",
            OptionColor::Red => "red",
        }
    }

    /// The colour with this name; `None` for a name this version does not know.
    #[must_use]
    pub fn parse(name: &str) -> Option<OptionColor> {
        OptionColor::ALL
            .into_iter()
            .find(|color| color.as_str() == name)
    }
}

/// The group of a `status` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum StatusGroup {
    /// Not started.
    Todo,
    /// Under way.
    InProgress,
    /// Finished.
    Done,
}

impl StatusGroup {
    /// The three groups, in order.
    pub const ALL: [StatusGroup; 3] = [
        StatusGroup::Todo,
        StatusGroup::InProgress,
        StatusGroup::Done,
    ];

    /// The name stored in documents and used in JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            StatusGroup::Todo => "todo",
            StatusGroup::InProgress => "in_progress",
            StatusGroup::Done => "done",
        }
    }

    /// The group with this name; `None` for a name this version does not know.
    #[must_use]
    pub fn parse(name: &str) -> Option<StatusGroup> {
        StatusGroup::ALL
            .into_iter()
            .find(|group| group.as_str() == name)
    }
}

/// The three options a new `status` property starts with: name, group and colour.
pub const DEFAULT_STATUS_OPTIONS: [(&str, StatusGroup, OptionColor); 3] = [
    ("Not started", StatusGroup::Todo, OptionColor::Gray),
    ("In progress", StatusGroup::InProgress, OptionColor::Blue),
    ("Done", StatusGroup::Done, OptionColor::Green),
];

/// The value of a `date` property: a floating local calendar day (`YYYY-MM-DD`), optionally
/// with a time of day (`HH:mm`), checked like a date mention (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DateValue {
    /// The day, `YYYY-MM-DD`.
    pub date: String,
    /// The time of day, `HH:mm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub time: Option<String>,
}

/// The value of a `place` property: a label or address, optionally with coordinates. Nothing is
/// looked up or fetched: the name is what the user typed, and the coordinates are typed too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PlaceValue {
    /// The label or address: not blank, one line, at most [`MAX_PLACE_NAME_CHARS`] characters.
    pub name: String,
    /// Latitude in degrees, -90 to 90 (present together with `lon`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub lat: Option<f64>,
    /// Longitude in degrees, -180 to 180 (present together with `lat`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub lon: Option<f64>,
}

/// One item of a `checklist` value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ChecklistItem {
    /// Stable id of the item (random, made by the device that adds it).
    pub id: ChecklistItemId,
    /// The text, at most [`MAX_CHECKLIST_TEXT_CHARS`] characters on one line (it may be empty).
    pub text: String,
    /// Whether the item is done.
    pub done: bool,
}

/// The value of one property of one page. The `kind` is the property's kind
/// (`PagePropertySet` refuses a value of another kind).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PropertyValue {
    /// Free text, at most [`MAX_TEXT_CHARS`] characters. An empty text is no value.
    Text(String),
    /// A finite number.
    Number(f64),
    /// The id of one option of the property.
    Select(OptionId),
    /// Distinct option ids of the property, in the order shown (at most
    /// [`MAX_SELECTED_OPTIONS`]). An empty list is no value.
    MultiSelect(Vec<OptionId>),
    /// A day, optionally with a time.
    Date(DateValue),
    /// On or off.
    Checkbox(bool),
    /// An absolute `http`, `https`, `mailto` or `tel` URL without spaces, at most
    /// [`MAX_URL_CHARS`] characters. An empty URL is no value.
    Url(String),
    /// The id of one option of the property.
    Status(OptionId),
    /// A non-blank string, stored as typed, at most [`MAX_EMAIL_CHARS`] characters (the UI
    /// checks the format).
    Email(String),
    /// A non-blank string, stored as typed, at most [`MAX_PHONE_CHARS`] characters.
    Phone(String),
    /// Distinct attachments, in the order shown (at most [`MAX_FILES`]). An empty list is no
    /// value. Whether an attachment exists is not checked when a value is written.
    Files(Vec<AttachmentId>),
    /// Items with distinct ids, in the order shown (at most [`MAX_CHECKLIST_ITEMS`]). An empty
    /// list is no value.
    Checklist(Vec<ChecklistItem>),
    /// A named place, optionally with coordinates.
    Place(PlaceValue),
    /// Distinct accounts that were members of the space when the value was written, in the order
    /// shown (at most [`MAX_PEOPLE`]; in a personal space only the person's own account). An empty
    /// list is no value. Readers keep and show an account that is no longer a member as "Former
    /// member" (ADR-0049).
    Person(Vec<AccountId>),
    /// Distinct pages of the same space, in the order shown (at most [`MAX_RELATED_PAGES`]). An
    /// empty list is no value. Whether a page exists is not checked when a value is written;
    /// pages that are unknown, purged or in another space read as nothing.
    Relation(Vec<PageId>),
    /// Computed from the related pages (ADR-0027). Never stored, never accepted.
    Rollup(RollupValue),
    /// Computed: when the page was created, Unix ms. Never stored, never accepted.
    CreatedTime(i64),
    /// Computed: when the page was last edited, Unix ms. Never stored, never accepted.
    LastEditedTime(i64),
    /// Computed: the device that created the page. Never stored, never accepted.
    CreatedBy(DeviceId),
    /// Computed: the device that last edited the page. Never stored, never accepted.
    LastEditedBy(DeviceId),
    /// Computed from the page's formula (ADR-0055). Never stored, never accepted.
    Formula(FormulaResult),
}

// Numbers are finite (the core refuses anything else), so equality is reflexive.
impl Eq for PropertyValue {}

impl PropertyValue {
    /// The kind of property this value belongs to.
    #[must_use]
    pub const fn kind(&self) -> PropertyKind {
        match self {
            PropertyValue::Text(_) => PropertyKind::Text,
            PropertyValue::Number(_) => PropertyKind::Number,
            PropertyValue::Select(_) => PropertyKind::Select,
            PropertyValue::MultiSelect(_) => PropertyKind::MultiSelect,
            PropertyValue::Date(_) => PropertyKind::Date,
            PropertyValue::Checkbox(_) => PropertyKind::Checkbox,
            PropertyValue::Url(_) => PropertyKind::Url,
            PropertyValue::Status(_) => PropertyKind::Status,
            PropertyValue::Email(_) => PropertyKind::Email,
            PropertyValue::Phone(_) => PropertyKind::Phone,
            PropertyValue::Files(_) => PropertyKind::Files,
            PropertyValue::Checklist(_) => PropertyKind::Checklist,
            PropertyValue::Place(_) => PropertyKind::Place,
            PropertyValue::Person(_) => PropertyKind::Person,
            PropertyValue::Relation(_) => PropertyKind::Relation,
            PropertyValue::Rollup(_) => PropertyKind::Rollup,
            PropertyValue::CreatedTime(_) => PropertyKind::CreatedTime,
            PropertyValue::LastEditedTime(_) => PropertyKind::LastEditedTime,
            PropertyValue::CreatedBy(_) => PropertyKind::CreatedBy,
            PropertyValue::LastEditedBy(_) => PropertyKind::LastEditedBy,
            PropertyValue::Formula(_) => PropertyKind::Formula,
        }
    }
}

/// One option of a select, multi-select or status property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PropertyOption {
    /// Stable id (random, never reused).
    pub id: OptionId,
    /// Display name, 1 to [`MAX_NAME_CHARS`] characters. Two options may share a name.
    pub name: String,
    /// Colour from the palette.
    pub color: OptionColor,
    /// The group of a `status` option; absent for the other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub group: Option<StatusGroup>,
}

/// One property of a space's schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PropertyDef {
    /// Property id ([`TAGS_PROPERTY_ID`] for Tags).
    pub id: PropertyId,
    /// Display name, 1 to [`MAX_NAME_CHARS`] characters.
    pub name: String,
    /// Kind, fixed at creation.
    pub kind: PropertyKind,
    /// The options in display order; empty for the kinds without a list.
    pub options: Vec<PropertyOption>,
    /// Creation time, Unix ms (0 for Tags before anything wrote it).
    pub created_at: i64,
    /// The configuration of a `relation` (always present for one) or a `rollup` (absent when
    /// it is damaged); absent for the other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub config: Option<PropertyConfig>,
}

/// The answer of `PropertySchemaGet`: Tags first, then the other properties in the order of
/// their positions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PropertySchema {
    /// The properties of the space.
    pub properties: Vec<PropertyDef>,
}

/// The values of one page, in schema order (Tags first). Unset properties, values of
/// properties the schema no longer has and options it no longer lists are left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageProperties {
    /// `(property id, value)` pairs.
    pub values: Vec<(PropertyId, PropertyValue)>,
}

/// What `PagesByProperty` looks for. Each filter applies to some kinds only; another kind is
/// `InvalidInput`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all_fields = "camelCase")]
pub enum PropertyFilter {
    /// The page has a value (any kind; a checkbox set to off counts).
    HasValue,
    /// The page's value equals this one (same kind as the property). Text-like values compare
    /// exactly (case-sensitive); a `multi_select` compares as a set; a `date` without a time
    /// matches only values without one.
    Equals(PropertyValue),
    /// The page's `select`, `multi_select` or `status` value includes this option.
    ContainsOption(OptionId),
    /// The page's `text`, `url`, `email`, `phone` or `place` (name) value contains this text, ignoring
    /// case (at most [`MAX_FILTER_CHARS`] characters, not empty).
    TextContains(String),
    /// The page's `date` value is on or after `from` and on or before `to` (either end may be
    /// left out; the time of day is ignored).
    DateBetween {
        /// First day, `YYYY-MM-DD`, inclusive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        from: Option<String>,
        /// Last day, `YYYY-MM-DD`, inclusive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        to: Option<String>,
    },
    /// A `checkbox` is on (`true`), or is off or unset (`false`: every live page of the space
    /// that is not on).
    Checked(bool),
    /// A `checklist` has every item done (`true`), or at least one item not done (`false`). A
    /// page without a checklist is in neither answer (unlike [`PropertyFilter::Checked`], a
    /// missing checklist is not "all done" and not "open items"; use `HasValue` for it).
    ChecklistComplete(bool),
    /// A `relation` lists this page (both directions of a two-way relation count). Trashed
    /// pages are never in the answer, as everywhere.
    RelatesTo(PageId),
    /// A `person` value lists this account (M13 slice 2, ADR-0049). Refused for every other kind.
    ContainsPerson(AccountId),
    /// A `person` value lists the account asking: the core substitutes this account for "me" when it
    /// answers. Its own filter because a saved view is shared and "me" differs per viewer
    /// (ADR-0049). Refused for every other kind.
    ContainsMe,
    /// A `created_time` or `last_edited_time` is on or after `from` and on or before `to`
    /// (Unix ms; either end may be left out). Those values are instants, not floating days, so
    /// the caller turns local days into instants. `DateBetween` is refused for them.
    InstantBetween {
        /// First instant, Unix ms, inclusive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        from: Option<i64>,
        /// Last instant, Unix ms, inclusive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        to: Option<i64>,
    },
}

/// A summary function of a rollup (ADR-0027). Which target kinds each accepts:
/// [`RollupFunction::accepts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum RollupFunction {
    /// The number of related pages.
    CountAll,
    /// The number of related pages that have a value.
    CountValues,
    /// The number of distinct values.
    CountUnique,
    /// The number of related pages without a value.
    CountEmpty,
    /// Percent (0 to 100) of the related pages without a value.
    PercentEmpty,
    /// Percent (0 to 100) of the related pages with a value.
    PercentNotEmpty,
    /// Sum of numbers.
    Sum,
    /// Mean of numbers.
    Average,
    /// Median of numbers.
    Median,
    /// Smallest number.
    Min,
    /// Largest number.
    Max,
    /// Largest minus smallest number.
    Range,
    /// Earliest date.
    Earliest,
    /// Latest date.
    Latest,
    /// Percent (0 to 100) of the related pages whose checkbox is on.
    PercentChecked,
    /// The number of related pages whose checkbox is on.
    CountChecked,
    /// The values themselves, in the order of the relation.
    ShowOriginal,
    /// Items done and items in all, summed over the checklists.
    ChecklistProgress,
}

impl RollupFunction {
    /// Every function, in the order the UI lists them.
    pub const ALL: [RollupFunction; 18] = [
        RollupFunction::CountAll,
        RollupFunction::CountValues,
        RollupFunction::CountUnique,
        RollupFunction::CountEmpty,
        RollupFunction::PercentEmpty,
        RollupFunction::PercentNotEmpty,
        RollupFunction::Sum,
        RollupFunction::Average,
        RollupFunction::Median,
        RollupFunction::Min,
        RollupFunction::Max,
        RollupFunction::Range,
        RollupFunction::Earliest,
        RollupFunction::Latest,
        RollupFunction::PercentChecked,
        RollupFunction::CountChecked,
        RollupFunction::ShowOriginal,
        RollupFunction::ChecklistProgress,
    ];

    /// The name stored in documents and used in JSON (`snake_case`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RollupFunction::CountAll => "count_all",
            RollupFunction::CountValues => "count_values",
            RollupFunction::CountUnique => "count_unique",
            RollupFunction::CountEmpty => "count_empty",
            RollupFunction::PercentEmpty => "percent_empty",
            RollupFunction::PercentNotEmpty => "percent_not_empty",
            RollupFunction::Sum => "sum",
            RollupFunction::Average => "average",
            RollupFunction::Median => "median",
            RollupFunction::Min => "min",
            RollupFunction::Max => "max",
            RollupFunction::Range => "range",
            RollupFunction::Earliest => "earliest",
            RollupFunction::Latest => "latest",
            RollupFunction::PercentChecked => "percent_checked",
            RollupFunction::CountChecked => "count_checked",
            RollupFunction::ShowOriginal => "show_original",
            RollupFunction::ChecklistProgress => "checklist_progress",
        }
    }

    /// The function with this name; `None` for a name this version does not know.
    #[must_use]
    pub fn parse(name: &str) -> Option<RollupFunction> {
        RollupFunction::ALL
            .into_iter()
            .find(|function| function.as_str() == name)
    }

    /// Whether the function can summarise a target of this kind (`None` is the title of the
    /// related pages). The counting functions and `show_original` take any target; the number
    /// functions a `number`; `earliest` and `latest` a `date`; the checked functions a
    /// `checkbox`; `checklist_progress` a `checklist`. A target kind that [cannot be rolled
    /// up](PropertyKind::is_rollup_target) is refused by every function.
    #[must_use]
    pub fn accepts(self, target: Option<PropertyKind>) -> bool {
        if target.is_some_and(|kind| !kind.is_rollup_target()) {
            return false;
        }
        match self {
            RollupFunction::CountAll
            | RollupFunction::CountValues
            | RollupFunction::CountUnique
            | RollupFunction::CountEmpty
            | RollupFunction::PercentEmpty
            | RollupFunction::PercentNotEmpty
            | RollupFunction::ShowOriginal => true,
            RollupFunction::Sum
            | RollupFunction::Average
            | RollupFunction::Median
            | RollupFunction::Min
            | RollupFunction::Max
            | RollupFunction::Range => target == Some(PropertyKind::Number),
            RollupFunction::Earliest | RollupFunction::Latest => target == Some(PropertyKind::Date),
            RollupFunction::PercentChecked | RollupFunction::CountChecked => {
                target == Some(PropertyKind::Checkbox)
            }
            RollupFunction::ChecklistProgress => target == Some(PropertyKind::Checklist),
        }
    }
}

/// What a rollup summarises: the title of the related pages, or one of their properties.
/// JSON: the text `"title"` or the property id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, type = "string"))]
pub enum RollupTarget {
    /// The title of the related pages.
    Title,
    /// A property of the space.
    Property(PropertyId),
}

impl Serialize for RollupTarget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RollupTarget::Title => serializer.serialize_str(ROLLUP_TITLE_TARGET),
            RollupTarget::Property(id) => id.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RollupTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == ROLLUP_TITLE_TARGET {
            return Ok(RollupTarget::Title);
        }
        text.parse::<PropertyId>()
            .map(RollupTarget::Property)
            .map_err(|_| serde::de::Error::custom("a rollup target is \"title\" or a property id"))
    }
}

/// The configuration of a `relation` or a `rollup` property (ADR-0027), or of a `formula` (ADR-0055).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PropertyConfig {
    /// A relation: `two_way` makes a paired property on the same pages that shows the links
    /// backwards. `paired_property_id` is absent when creating one (the core makes the paired
    /// property) and when the relation is one-way.
    Relation {
        /// Whether the relation has a paired property.
        two_way: bool,
        /// The paired property, when there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        paired_property_id: Option<PropertyId>,
    },
    /// A rollup: summarise `target` of the pages related through `relation_property_id`.
    Rollup {
        /// The relation property of the same space.
        relation_property_id: PropertyId,
        /// What to summarise.
        target: RollupTarget,
        /// How.
        function: RollupFunction,
    },
    /// A formula (ADR-0055, `docs/architecture/formulas.md`).
    Formula {
        /// The stored expression: properties referenced by id, `prop("<property id>")`, the title
        /// as `prop("title")`.
        expression: String,
        /// Derived on read: the type of the result; absent when the expression does not check
        /// against the schema (then no page has a value). Ignored on write.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        result_type: Option<FormulaType>,
    },
}

/// The result of a rollup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RollupResult {
    /// A count, a percent (0 to 100) or a number summary.
    Number(f64),
    /// `earliest` or `latest`.
    Date(DateValue),
    /// `show_original`: the values of the related pages (one per page that has one).
    Values(Vec<PropertyValue>),
    /// `checklist_progress`: items done and items in all.
    Progress {
        /// Items done.
        done: u32,
        /// Items.
        total: u32,
    },
}

/// The value of a rollup property: the function and its result. A rollup that has no result
/// (an average of nothing, a broken configuration) has no value at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RollupValue {
    /// The function that produced the result.
    pub function: RollupFunction,
    /// The result.
    pub result: RollupResult,
}

/// The base of a formula's static type (ADR-0055).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FormulaBase {
    /// A finite number.
    Number,
    /// A text.
    Text,
    /// True or false.
    Boolean,
    /// A floating local date, optionally with a time.
    Date,
    /// A page of the same space.
    Page,
    /// An account.
    Person,
}

/// The static type of a formula's result: a base, or a list of it (never a list of lists).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct FormulaType {
    /// The type of the value, or of each item of a list.
    pub base: FormulaBase,
    /// Whether the result is a list.
    pub list: bool,
}

/// The value of a formula for one page (ADR-0055). Numbers are finite; a list holds items of one
/// base type and never a list. An empty result is no value at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum FormulaResult {
    /// A number.
    Number(f64),
    /// A text (never empty).
    Text(String),
    /// True or false.
    Boolean(bool),
    /// A date, with a time when it has one.
    Date(DateValue),
    /// A page.
    Page(PageId),
    /// An account.
    Person(AccountId),
    /// A list (never empty).
    List(Vec<FormulaResult>),
}
