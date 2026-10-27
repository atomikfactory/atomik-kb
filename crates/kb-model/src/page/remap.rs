//! The content of a page copy (ADR-0040, "Duplicate a page with its sub-pages").
//!
//! A copy of a page is built from the portable JSON of the original (`PageDoc::to_portable`, then
//! `PageDoc::from_portable` for the copy) after its ids were replaced here:
//!
//! - every node with a valid `attrs.id` gets the new id the [`CopyIds`] hold for it, so the copy
//!   shares no block id with the original (block ids are meant to be unique across the workspace:
//!   backlinks and block links name them);
//! - a `pageRef` or `pageLink` that points to a page of the copied subtree points to the page's
//!   copy, and its `blockId` (the block it names) to the copy of that block, when the block was
//!   copied; a link to any other page is left exactly as it is;
//! - the `whiteboards` root map of the page follows its blocks: the elements of a block are
//!   stored under the block's new id (the elements themselves, and their own ids, are unchanged);
//! - a `pageList` whose view names a `defaultTemplate` inside the copied subtree (ADR-0056) names
//!   the copy of that page;
//! - a `pageList` whose `source` is `{ "kind": "page", "pageId" }` (ADR-0058) with a `pageId` inside
//!   the copied subtree names the copy of that page; any other source (children, tag, space, a page
//!   outside the subtree) is kept.
//!
//! A page made from a template (ADR-0056, `docs/architecture/templates.md` section 5) goes through
//! a second pass, [`fill_template_content`]: a `dateMention` marked `relative` becomes the day (and
//! for `now` the time) of the creation, a `personMention` marked `relative: "me"` the account that
//! creates the page; a `templateButton`'s content is left exactly as it is (its relative mentions
//! are resolved when the button is clicked).
//!
//! The functions are pure: they read and write JSON, never a document, and they never look at
//! text, marks or attributes other than the ones above.

use std::collections::{HashMap, HashSet};

use kb_types::{AccountId, BlockId, PageId};
use serde_json::Value;

use super::read::MAX_DEPTH;
use super::shape;

/// Node name of a saved view over a set of pages (ADR-0054).
const PAGE_LIST: &str = "pageList";
/// Node name of the template button (ADR-0056): a block that holds template material.
const TEMPLATE_BUTTON: &str = "templateButton";
/// The attribute of a `pageList` block that holds its views.
const VIEWS: &str = "views";
/// The key of a view that names the template "New" makes a page from (ADR-0056).
const DEFAULT_TEMPLATE: &str = "defaultTemplate";
/// The attribute of a `pageList` block that holds its source, shared by its views (ADR-0054).
const SOURCE: &str = "source";
/// The `kind` of the source that lists the sub-pages of another page (ADR-0058).
const SOURCE_KIND_PAGE: &str = "page";
/// The attribute of a date or person mention that makes it relative (ADR-0056).
const RELATIVE: &str = "relative";
/// Longest `personMention` label, in characters.
const MAX_LABEL_CHARS: usize = 100;

/// The ids a copy of a subtree gets: the page copies and the block copies, by original id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyIds {
    /// The copy of each page of the copied subtree.
    pub pages: HashMap<PageId, PageId>,
    /// The copy of each block of the copied pages (every node with an id).
    pub blocks: HashMap<BlockId, BlockId>,
}

/// The ids of the nodes of portable page JSON (`attrs.id` that is a valid block id), in document
/// order, each once (a duplicated id, which concurrent edits can leave, is listed once).
#[must_use]
pub fn portable_block_ids(json: &Value) -> Vec<BlockId> {
    fn visit(json: &Value, depth: usize, seen: &mut HashSet<BlockId>, out: &mut Vec<BlockId>) {
        if depth > MAX_DEPTH {
            return;
        }
        if let Some(id) = block_id_of(json)
            && seen.insert(id)
        {
            out.push(id);
        }
        if let Some(Value::Array(content)) = json.get("content") {
            for child in content {
                visit(child, depth + 1, seen, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(json, 0, &mut HashSet::new(), &mut out);
    out
}

/// The portable JSON of the copy of a page: `json` with the ids replaced as the module
/// documentation says. Ids the maps do not know stay as they are.
#[must_use]
pub fn remap_portable(json: &Value, ids: &CopyIds) -> Value {
    let mut out = json.clone();
    remap_node(&mut out, ids, 0);
    if let Some(Value::Object(boards)) = out.get_mut(shape::WHITEBOARDS) {
        let old = std::mem::take(boards);
        for (key, elements) in old {
            let key = key
                .parse::<BlockId>()
                .ok()
                .and_then(|block| ids.blocks.get(&block))
                .map_or(key, ToString::to_string);
            boards.insert(key, elements);
        }
    }
    out
}

/// The block id of a node: `attrs.id` when it is a valid one.
fn block_id_of(node: &Value) -> Option<BlockId> {
    node.get("attrs")?.get(shape::ID)?.as_str()?.parse().ok()
}

/// The string attribute `key` of an attribute map, parsed as an id.
fn id_attr<T: std::str::FromStr>(attrs: &serde_json::Map<String, Value>, key: &str) -> Option<T> {
    attrs.get(key)?.as_str()?.parse().ok()
}

fn remap_node(node: &mut Value, ids: &CopyIds, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Value::Object(obj) = node else {
        return;
    };
    let is_link = matches!(
        obj.get("type").and_then(Value::as_str),
        Some(shape::PAGE_REF | shape::PAGE_LINK)
    );
    let is_page_list = obj.get("type").and_then(Value::as_str) == Some(PAGE_LIST);
    if let Some(Value::Object(attrs)) = obj.get_mut("attrs") {
        if let Some(copy) = id_attr::<BlockId>(attrs, shape::ID).and_then(|id| ids.blocks.get(&id))
        {
            attrs.insert(shape::ID.into(), copy.to_string().into());
        }
        if is_page_list && let Some(Value::Array(views)) = attrs.get_mut(VIEWS) {
            for view in views {
                if let Value::Object(view) = view
                    && let Some(copy) = id_attr::<PageId>(view, DEFAULT_TEMPLATE)
                        .and_then(|page| ids.pages.get(&page).copied())
                {
                    view.insert(DEFAULT_TEMPLATE.into(), copy.to_string().into());
                }
            }
        }
        if is_page_list
            && let Some(Value::Object(source)) = attrs.get_mut(SOURCE)
            && source.get("kind").and_then(Value::as_str) == Some(SOURCE_KIND_PAGE)
            && let Some(copy) = id_attr::<PageId>(source, shape::PAGE_ID)
                .and_then(|page| ids.pages.get(&page).copied())
        {
            source.insert(shape::PAGE_ID.into(), copy.to_string().into());
        }
        if is_link
            && let Some(copy) = id_attr::<PageId>(attrs, shape::PAGE_ID)
                .and_then(|page| ids.pages.get(&page).copied())
        {
            attrs.insert(shape::PAGE_ID.into(), copy.to_string().into());
            if let Some(block) = id_attr::<BlockId>(attrs, shape::BLOCK_ID)
                .and_then(|block| ids.blocks.get(&block).copied())
            {
                attrs.insert(shape::BLOCK_ID.into(), block.to_string().into());
            }
        }
    }
    if let Some(Value::Array(content)) = obj.get_mut("content") {
        for child in content {
            remap_node(child, ids, depth + 1);
        }
    }
}

// ----- a page made from a template (ADR-0056) -------------------------------------------------

/// The local day and time of the moment a page is made from a template: what `{{date}}` and
/// `{{time}}` in a title, a `today` or `now` placeholder and a relative date mention resolve to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMoment {
    /// The local day, `YYYY-MM-DD`.
    pub date: String,
    /// The local time of day, `HH:mm`.
    pub time: String,
}

impl LocalMoment {
    /// The local day and time of `now_ms` (Unix ms) on a device whose offset from UTC is
    /// `offset_seconds`.
    #[must_use]
    pub fn new(now_ms: i64, offset_seconds: i32) -> Self {
        let local = now_ms
            .div_euclid(1000)
            .saturating_add(i64::from(offset_seconds));
        let (year, month, day) = civil_from_days(local.div_euclid(86_400));
        let of_day = local.rem_euclid(86_400);
        Self {
            date: format!("{year:04}-{month:02}-{day:02}"),
            time: format!("{:02}:{:02}", of_day / 3_600, of_day % 3_600 / 60),
        }
    }

    /// `title` with every `{{date}}` replaced by the day and every `{{time}}` by the time (case
    /// sensitive, no other token, no escaping), cut to `max_chars` characters when the tokens
    /// made it longer (a title never grows past the limit).
    #[must_use]
    pub fn resolve_title(&self, title: &str, max_chars: usize) -> String {
        if !title.contains("{{") {
            return title.to_owned();
        }
        let resolved = title
            .replace("{{date}}", &self.date)
            .replace("{{time}}", &self.time);
        if resolved.chars().count() <= max_chars {
            resolved
        } else {
            let cut: String = resolved.chars().take(max_chars).collect();
            cut.trim_end().to_owned()
        }
    }
}

/// The civil date (year, month 1..12, day 1..31) of a day count since 1970-01-01 (Hinnant's
/// algorithm, the proleptic Gregorian calendar).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let y = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// The person `me` stands for in a page made from a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateMe {
    /// The account that makes the page.
    pub account: AccountId,
    /// Its display name, when the core knows it (1 to 100 characters).
    pub label: Option<String>,
}

/// What [`fill_template_content`] resolves relative mentions to.
#[derive(Debug, Clone, Copy)]
pub struct TemplateFill<'a> {
    /// The local day and time of the creation.
    pub moment: &'a LocalMoment,
    /// The account that makes the page; `None` when the core has none (a personal space before the
    /// recovery kit exists): a `me` mention then keeps the account and label it was written with.
    pub me: Option<&'a TemplateMe>,
}

/// The portable JSON of a page made from a template: `json` with its relative mentions resolved
/// (see the module documentation). A `dateMention` whose `relative` is `today` gets `date` = the
/// local day, one whose `relative` is `now` also `time` = the local time, and both lose `relative`;
/// a `personMention` whose `relative` is `me` gets `accountId` = me and `label` = my display name
/// when known, and loses `relative`. Any other value of `relative` (a newer build's) is left as it
/// is, and so is everything inside a `templateButton`. The input is not modified.
#[must_use]
pub fn fill_template_content(json: &Value, fill: &TemplateFill<'_>) -> Value {
    let mut out = json.clone();
    fill_node(&mut out, fill, 0);
    out
}

fn fill_node(node: &mut Value, fill: &TemplateFill<'_>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Value::Object(obj) = node else {
        return;
    };
    match obj.get("type").and_then(Value::as_str) {
        Some(TEMPLATE_BUTTON) => return,
        Some(shape::DATE_MENTION) => {
            if let Some(Value::Object(attrs)) = obj.get_mut("attrs") {
                fill_date_mention(attrs, fill);
            }
        }
        Some(shape::PERSON_MENTION) => {
            if let Some(Value::Object(attrs)) = obj.get_mut("attrs") {
                fill_person_mention(attrs, fill);
            }
        }
        _ => {}
    }
    if let Some(Value::Array(content)) = obj.get_mut("content") {
        for child in content {
            fill_node(child, fill, depth + 1);
        }
    }
}

fn fill_date_mention(attrs: &mut serde_json::Map<String, Value>, fill: &TemplateFill<'_>) {
    let now = match attrs.get(RELATIVE).and_then(Value::as_str) {
        Some("today") => false,
        Some("now") => true,
        _ => return,
    };
    attrs.remove(RELATIVE);
    attrs.insert(shape::DATE.into(), fill.moment.date.clone().into());
    if now {
        attrs.insert(shape::TIME.into(), fill.moment.time.clone().into());
    }
}

fn fill_person_mention(attrs: &mut serde_json::Map<String, Value>, fill: &TemplateFill<'_>) {
    if attrs.get(RELATIVE).and_then(Value::as_str) != Some("me") {
        return;
    }
    attrs.remove(RELATIVE);
    let Some(me) = fill.me else {
        return;
    };
    attrs.insert(shape::ACCOUNT_ID.into(), me.account.to_string().into());
    if let Some(label) = me.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        let label: String = label.chars().take(MAX_LABEL_CHARS).collect();
        attrs.insert(shape::LABEL.into(), label.into());
    }
}
