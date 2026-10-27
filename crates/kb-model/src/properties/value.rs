//! Property values: the checks a value must pass, and how one is stored in the structure
//! document (ADR-0026). The TypeScript mirror (`core-client`, `properties.ts`,
//! `isValidPropertyValue`) applies the same rules.
//!
//! Stored values are plain Loro values keyed by property id in a page's `props` map:
//!
//! ```text
//! text, url, email, phone  string          number   f64 (finite)
//! checkbox                 bool            select, status   option id string
//! date                     {"date": "YYYY-MM-DD", "time"?: "HH:mm"}
//! multi_select             ["<option id>", ..] in order
//! files                    ["<attachment id>", ..] in order
//! checklist                [{"id": "<item id>", "text": str, "done": bool}, ..] in order
//! place                    {"name": str, "lat"?: f64, "lon"?: f64}   (lat and lon both or neither)
//! relation                 ["<page id>", ..] in order
//! person                   ["<account id>", ..] in order (M13 slice 2, ADR-0049)
//! rollup, created_time, last_edited_time, created_by, last_edited_by: computed, never stored
//! ```

use std::collections::{HashMap, HashSet};

use kb_types::properties::{
    MAX_CHECKLIST_ITEMS, MAX_CHECKLIST_TEXT_CHARS, MAX_EMAIL_CHARS, MAX_FILES, MAX_PEOPLE,
    MAX_PHONE_CHARS, MAX_PLACE_NAME_CHARS, MAX_RELATED_PAGES, MAX_SELECTED_OPTIONS, MAX_TEXT_CHARS,
    MAX_URL_CHARS,
};
use kb_types::{
    AccountId, AttachmentId, ChecklistItem, ChecklistItemId, DateValue, OptionId, PageId,
    PlaceValue, PropertyFilter, PropertyKind, PropertyValue,
};
use loro::LoroValue;

use crate::ModelError;
use crate::page::date::{valid_date, valid_time};

fn invalid(message: &str) -> ModelError {
    ModelError::InvalidProperty {
        field: "value",
        message: message.to_owned(),
    }
}

fn too_long(max: usize) -> ModelError {
    ModelError::InvalidProperty {
        field: "value",
        message: format!("must be at most {max} characters"),
    }
}

/// The schemes of a `url` value. Narrower than "any absolute URL" on purpose: a value is shown
/// as a link, and `javascript:` or `file:` links do not belong in notes.
const URL_SCHEMES: [&str; 4] = ["http", "https", "mailto", "tel"];

/// Whether `text` is an absolute URL of an accepted scheme: `http(s)://host...`, or
/// `mailto:` or `tel:` followed by something; no whitespace or control character anywhere,
/// and at most [`MAX_URL_CHARS`] characters.
#[must_use]
pub fn valid_url(text: &str) -> bool {
    if text.chars().count() > MAX_URL_CHARS
        || text.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let scheme_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok || !URL_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return false;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => rest.strip_prefix("//").is_some_and(|after| {
            !after
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .is_empty()
        }),
        _ => !rest.is_empty(),
    }
}

/// Text that may span lines: no control characters but tab, line feed and carriage return.
fn free_text_ok(text: &str) -> bool {
    text.chars()
        .all(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
}

/// Checks a value against its own rules (kind-level; whether an option id belongs to the
/// property is the caller's check, [`options_of`]) and normalises an empty text, URL or
/// multi-select list to `None`, which stands for "no value". `Ok(Some(value))` is what to
/// store (unchanged: text is stored as typed).
pub(crate) fn check(value: &PropertyValue) -> Result<Option<&PropertyValue>, ModelError> {
    match value {
        PropertyValue::Text(text) => {
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(too_long(MAX_TEXT_CHARS));
            }
            if !free_text_ok(text) {
                return Err(invalid(
                    "must not contain control characters other than tabs and line breaks",
                ));
            }
            Ok((!text.is_empty()).then_some(value))
        }
        PropertyValue::Number(number) => {
            if number.is_finite() {
                Ok(Some(value))
            } else {
                Err(invalid("must be a finite number"))
            }
        }
        PropertyValue::Checkbox(_) | PropertyValue::Select(_) | PropertyValue::Status(_) => {
            Ok(Some(value))
        }
        PropertyValue::MultiSelect(ids) => {
            if ids.len() > MAX_SELECTED_OPTIONS {
                return Err(ModelError::InvalidProperty {
                    field: "value",
                    message: format!("must list at most {MAX_SELECTED_OPTIONS} options"),
                });
            }
            let distinct: HashSet<&OptionId> = ids.iter().collect();
            if distinct.len() != ids.len() {
                return Err(invalid("option ids must be distinct"));
            }
            Ok((!ids.is_empty()).then_some(value))
        }
        PropertyValue::Date(DateValue { date, time }) => {
            if !valid_date(date) {
                return Err(invalid("the date must be a real day as YYYY-MM-DD"));
            }
            if time.as_deref().is_some_and(|time| !valid_time(time)) {
                return Err(invalid("the time must be HH:mm (24 hours)"));
            }
            Ok(Some(value))
        }
        PropertyValue::Url(url) => {
            if url.is_empty() {
                return Ok(None);
            }
            if url.chars().count() > MAX_URL_CHARS {
                return Err(too_long(MAX_URL_CHARS));
            }
            if valid_url(url) {
                Ok(Some(value))
            } else {
                Err(invalid(
                    "must be an absolute http, https, mailto or tel URL without spaces",
                ))
            }
        }
        PropertyValue::Email(text) => single_line(text, MAX_EMAIL_CHARS).map(|()| Some(value)),
        PropertyValue::Phone(text) => single_line(text, MAX_PHONE_CHARS).map(|()| Some(value)),
        PropertyValue::Files(ids) => {
            if ids.len() > MAX_FILES {
                return Err(invalid(&format!("must list at most {MAX_FILES} files")));
            }
            let distinct: HashSet<&AttachmentId> = ids.iter().collect();
            if distinct.len() != ids.len() {
                return Err(invalid("attachment ids must be distinct"));
            }
            Ok((!ids.is_empty()).then_some(value))
        }
        PropertyValue::Checklist(items) => {
            if items.len() > MAX_CHECKLIST_ITEMS {
                return Err(invalid(&format!(
                    "must hold at most {MAX_CHECKLIST_ITEMS} items"
                )));
            }
            let distinct: HashSet<&ChecklistItemId> = items.iter().map(|item| &item.id).collect();
            if distinct.len() != items.len() {
                return Err(invalid("item ids must be distinct"));
            }
            for item in items {
                if item.text.chars().count() > MAX_CHECKLIST_TEXT_CHARS {
                    return Err(invalid(&format!(
                        "an item must be at most {MAX_CHECKLIST_TEXT_CHARS} characters"
                    )));
                }
                if item.text.chars().any(char::is_control) {
                    return Err(invalid("an item must not contain control characters"));
                }
            }
            Ok((!items.is_empty()).then_some(value))
        }
        PropertyValue::Place(place) => check_place(place).map(|()| Some(value)),
        PropertyValue::Relation(pages) => {
            if pages.len() > MAX_RELATED_PAGES {
                return Err(invalid(&format!(
                    "must list at most {MAX_RELATED_PAGES} pages"
                )));
            }
            let distinct: HashSet<&PageId> = pages.iter().collect();
            if distinct.len() != pages.len() {
                return Err(invalid("page ids must be distinct"));
            }
            Ok((!pages.is_empty()).then_some(value))
        }
        // The structural rules of a Person value (ADR-0049): at most `MAX_PEOPLE` accounts, distinct;
        // an empty list is no value. Whether the accounts are members of the space is the core's rule
        // (`Core::person_value_check`), and readers keep accounts that left.
        PropertyValue::Person(accounts) => {
            if accounts.len() > MAX_PEOPLE {
                return Err(invalid(&format!("must list at most {MAX_PEOPLE} people")));
            }
            let distinct: HashSet<&AccountId> = accounts.iter().collect();
            if distinct.len() != accounts.len() {
                return Err(invalid("account ids must be distinct"));
            }
            Ok((!accounts.is_empty()).then_some(value))
        }
        PropertyValue::Rollup(_)
        | PropertyValue::Formula(_)
        | PropertyValue::CreatedTime(_)
        | PropertyValue::LastEditedTime(_)
        | PropertyValue::CreatedBy(_)
        | PropertyValue::LastEditedBy(_) => {
            Err(invalid("is computed from the page and cannot be set"))
        }
    }
}

/// The rules of a `place`: a non-blank single-line name of at most [`MAX_PLACE_NAME_CHARS`]
/// characters; coordinates both or neither, finite, latitude in [-90, 90], longitude in
/// [-180, 180].
fn check_place(place: &PlaceValue) -> Result<(), ModelError> {
    single_line(&place.name, MAX_PLACE_NAME_CHARS)?;
    match (place.lat, place.lon) {
        (None, None) => Ok(()),
        (Some(lat), Some(lon)) => {
            if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
                return Err(invalid("the latitude must be between -90 and 90"));
            }
            if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
                return Err(invalid("the longitude must be between -180 and 180"));
            }
            Ok(())
        }
        _ => Err(invalid("the latitude and the longitude go together")),
    }
}

/// A non-blank single line of at most `max` characters, kept as typed.
fn single_line(text: &str, max: usize) -> Result<(), ModelError> {
    if text.trim().is_empty() {
        return Err(invalid("must not be empty"));
    }
    if text.chars().count() > max {
        return Err(too_long(max));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid("must not contain control characters"));
    }
    Ok(())
}

/// The option ids a value refers to, in order (none for the kinds without a list).
pub(crate) fn options_of(value: &PropertyValue) -> Vec<OptionId> {
    match value {
        PropertyValue::Select(id) | PropertyValue::Status(id) => vec![*id],
        PropertyValue::MultiSelect(ids) => ids.clone(),
        _ => Vec::new(),
    }
}

/// The Loro value a property value is stored as.
pub(crate) fn encode(value: &PropertyValue) -> LoroValue {
    match value {
        PropertyValue::Text(text)
        | PropertyValue::Url(text)
        | PropertyValue::Email(text)
        | PropertyValue::Phone(text) => LoroValue::from(text.as_str()),
        PropertyValue::Number(number) => LoroValue::Double(*number),
        PropertyValue::Checkbox(on) => LoroValue::Bool(*on),
        PropertyValue::Select(id) | PropertyValue::Status(id) => LoroValue::from(id.to_string()),
        PropertyValue::MultiSelect(ids) => LoroValue::from(
            ids.iter()
                .map(|id| LoroValue::from(id.to_string()))
                .collect::<Vec<_>>(),
        ),
        PropertyValue::Files(ids) => LoroValue::from(
            ids.iter()
                .map(|id| LoroValue::from(id.to_string()))
                .collect::<Vec<_>>(),
        ),
        PropertyValue::Relation(pages) => LoroValue::from(
            pages
                .iter()
                .map(|id| LoroValue::from(id.to_string()))
                .collect::<Vec<_>>(),
        ),
        PropertyValue::Checklist(items) => LoroValue::from(
            items
                .iter()
                .map(|item| {
                    let mut map: HashMap<String, LoroValue> = HashMap::with_capacity(3);
                    map.insert("id".to_owned(), LoroValue::from(item.id.to_string()));
                    map.insert("text".to_owned(), LoroValue::from(item.text.as_str()));
                    map.insert("done".to_owned(), LoroValue::Bool(item.done));
                    LoroValue::from(map)
                })
                .collect::<Vec<_>>(),
        ),
        PropertyValue::Place(place) => {
            let mut map: HashMap<String, LoroValue> = HashMap::with_capacity(3);
            map.insert("name".to_owned(), LoroValue::from(place.name.as_str()));
            if let (Some(lat), Some(lon)) = (place.lat, place.lon) {
                map.insert("lat".to_owned(), LoroValue::Double(lat));
                map.insert("lon".to_owned(), LoroValue::Double(lon));
            }
            LoroValue::from(map)
        }
        // A list of account id strings, like a relation's list of page id strings.
        PropertyValue::Person(accounts) => LoroValue::from(
            accounts
                .iter()
                .map(|id| LoroValue::from(id.to_string()))
                .collect::<Vec<_>>(),
        ),
        // Computed values are never stored (`check` refuses them).
        PropertyValue::Rollup(_)
        | PropertyValue::Formula(_)
        | PropertyValue::CreatedTime(_)
        | PropertyValue::LastEditedTime(_)
        | PropertyValue::CreatedBy(_)
        | PropertyValue::LastEditedBy(_) => LoroValue::Null,
        PropertyValue::Date(DateValue { date, time }) => {
            let mut map: HashMap<String, LoroValue> = HashMap::with_capacity(2);
            map.insert("date".to_owned(), LoroValue::from(date.as_str()));
            if let Some(time) = time {
                map.insert("time".to_owned(), LoroValue::from(time.as_str()));
            }
            LoroValue::from(map)
        }
    }
}

/// Reads a stored value as `kind`. `None` when it has the wrong shape or breaks the value's
/// own rules (damaged or foreign data); option ids are returned as stored, whether or not the
/// property still lists them.
pub(crate) fn decode(kind: PropertyKind, stored: &LoroValue) -> Option<PropertyValue> {
    let value = match (kind, stored) {
        (PropertyKind::Text, LoroValue::String(text)) => PropertyValue::Text(text.to_string()),
        (PropertyKind::Url, LoroValue::String(text)) => PropertyValue::Url(text.to_string()),
        (PropertyKind::Email, LoroValue::String(text)) => PropertyValue::Email(text.to_string()),
        (PropertyKind::Phone, LoroValue::String(text)) => PropertyValue::Phone(text.to_string()),
        (PropertyKind::Number, LoroValue::Double(number)) => PropertyValue::Number(*number),
        (PropertyKind::Number, LoroValue::I64(number)) => {
            // Integers written by another engine; exact for any |n| below 2^53, rounded above.
            #[allow(clippy::cast_precision_loss)]
            PropertyValue::Number(*number as f64)
        }
        (PropertyKind::Checkbox, LoroValue::Bool(on)) => PropertyValue::Checkbox(*on),
        (PropertyKind::Select, LoroValue::String(id)) => {
            PropertyValue::Select(id.parse::<OptionId>().ok()?)
        }
        (PropertyKind::Status, LoroValue::String(id)) => {
            PropertyValue::Status(id.parse::<OptionId>().ok()?)
        }
        (PropertyKind::MultiSelect, LoroValue::List(items)) => PropertyValue::MultiSelect(
            items
                .iter()
                .map(|item| match item {
                    LoroValue::String(id) => id.parse::<OptionId>().ok(),
                    _ => None,
                })
                .collect::<Option<Vec<OptionId>>>()?,
        ),
        (PropertyKind::Files, LoroValue::List(items)) => PropertyValue::Files(
            items
                .iter()
                .map(|item| match item {
                    LoroValue::String(id) => id.parse::<AttachmentId>().ok(),
                    _ => None,
                })
                .collect::<Option<Vec<AttachmentId>>>()?,
        ),
        (PropertyKind::Relation, LoroValue::List(items)) => PropertyValue::Relation(
            items
                .iter()
                .map(|item| match item {
                    LoroValue::String(id) => id.parse::<PageId>().ok(),
                    _ => None,
                })
                .collect::<Option<Vec<PageId>>>()?,
        ),
        (PropertyKind::Person, LoroValue::List(items)) => PropertyValue::Person(
            items
                .iter()
                .map(|item| match item {
                    LoroValue::String(id) => id.parse::<AccountId>().ok(),
                    _ => None,
                })
                .collect::<Option<Vec<AccountId>>>()?,
        ),
        (PropertyKind::Checklist, LoroValue::List(items)) => PropertyValue::Checklist(
            items
                .iter()
                .map(|item| {
                    let LoroValue::Map(map) = item else {
                        return None;
                    };
                    let (
                        Some(LoroValue::String(id)),
                        Some(LoroValue::String(text)),
                        Some(LoroValue::Bool(done)),
                    ) = (map.get("id"), map.get("text"), map.get("done"))
                    else {
                        return None;
                    };
                    Some(ChecklistItem {
                        id: id.parse::<ChecklistItemId>().ok()?,
                        text: text.to_string(),
                        done: *done,
                    })
                })
                .collect::<Option<Vec<ChecklistItem>>>()?,
        ),
        (PropertyKind::Place, LoroValue::Map(map)) => {
            let name = match map.get("name") {
                Some(LoroValue::String(name)) => name.to_string(),
                _ => return None,
            };
            let coordinate = |key: &str| match map.get(key) {
                None => Some(None),
                Some(LoroValue::Double(number)) => Some(Some(*number)),
                Some(LoroValue::I64(number)) =>
                {
                    #[allow(clippy::cast_precision_loss)]
                    Some(Some(*number as f64))
                }
                Some(_) => None,
            };
            PropertyValue::Place(PlaceValue {
                name,
                lat: coordinate("lat")?,
                lon: coordinate("lon")?,
            })
        }
        (PropertyKind::Date, LoroValue::Map(map)) => {
            let date = match map.get("date") {
                Some(LoroValue::String(date)) => date.to_string(),
                _ => return None,
            };
            let time = match map.get("time") {
                None => None,
                Some(LoroValue::String(time)) => Some(time.to_string()),
                Some(_) => return None,
            };
            PropertyValue::Date(DateValue { date, time })
        }
        _ => return None,
    };
    // A stored value follows the rules of a written one; an empty one is as good as none.
    let keep = check(&value).is_ok_and(|checked| checked.is_some());
    keep.then_some(value)
}

/// The searchable text of a value, given the display name of an option id and the file name of
/// an attachment id: option names (joined with a comma) for the kinds with a list, the string for
/// `text`, `url`, `email`, `phone` and the name of a `place`, the file names of a `files` value, the titles of
/// the pages of a `relation` and the item texts of a `checklist` (each joined with a comma), nothing for the others.
pub(crate) fn search_text(
    value: &PropertyValue,
    option_name: &dyn Fn(OptionId) -> Option<String>,
    file_name: &dyn Fn(AttachmentId) -> Option<String>,
    page_title: &dyn Fn(PageId) -> Option<String>,
) -> Option<String> {
    let text = match value {
        PropertyValue::Text(text)
        | PropertyValue::Url(text)
        | PropertyValue::Email(text)
        | PropertyValue::Phone(text) => text.clone(),
        PropertyValue::Select(id) | PropertyValue::Status(id) => option_name(*id)?,
        PropertyValue::MultiSelect(ids) => ids
            .iter()
            .filter_map(|id| option_name(*id))
            .collect::<Vec<_>>()
            .join(", "),
        PropertyValue::Files(ids) => ids
            .iter()
            .filter_map(|id| file_name(*id))
            .collect::<Vec<_>>()
            .join(", "),
        PropertyValue::Checklist(items) => items
            .iter()
            .map(|item| item.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        PropertyValue::Place(place) => place.name.clone(),
        PropertyValue::Relation(pages) => pages
            .iter()
            .filter_map(|id| page_title(*id))
            .filter(|title| !title.trim().is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        // The people's names are the core's to resolve (they change with a rename, and the model
        // knows only account ids): nothing here, [`person_text`] joins the names the core gives.
        PropertyValue::Person(_)
        | PropertyValue::Number(_)
        | PropertyValue::Rollup(_)
        | PropertyValue::Formula(_)
        | PropertyValue::Checkbox(_)
        | PropertyValue::Date(_)
        | PropertyValue::CreatedTime(_)
        | PropertyValue::LastEditedTime(_)
        | PropertyValue::CreatedBy(_)
        | PropertyValue::LastEditedBy(_) => {
            return None;
        }
    };
    (!text.trim().is_empty()).then_some(text)
}

/// The text of a Person value: the names `name_of` gives for its accounts (the core resolves them
/// from the space's membership, the model knows only ids), blank or unknown ones left out, joined
/// with a comma; `None` when nothing is left. What the core indexes and exports for a Person value
/// (ADR-0049); a person who left reads as "Former member" in the UI, which is not text here.
#[must_use]
pub fn person_text(
    accounts: &[AccountId],
    name_of: &dyn Fn(AccountId) -> Option<String>,
) -> Option<String> {
    let text = accounts
        .iter()
        .filter_map(|account| name_of(*account))
        .filter(|name| !name.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    (!text.is_empty()).then_some(text)
}

/// Whether a page's value of a Person property matches `filter` (ADR-0049), the meaning the core's
/// `PagesByProperty` gives the filters of a person property: `HasValue` (the value lists somebody),
/// `ContainsPerson(account)` and `ContainsMe`, which is `ContainsPerson` of `me`, the account asking
/// (a saved view is shared, so "me" differs per viewer; with no account nobody is "me" and the
/// filter matches nothing). A value that is not a person value, no value, and every other filter do
/// not match.
#[must_use]
pub fn person_filter_matches(
    filter: &PropertyFilter,
    value: Option<&PropertyValue>,
    me: Option<AccountId>,
) -> bool {
    let Some(PropertyValue::Person(accounts)) = value else {
        return false;
    };
    match filter {
        PropertyFilter::HasValue => !accounts.is_empty(),
        PropertyFilter::ContainsPerson(account) => accounts.contains(account),
        PropertyFilter::ContainsMe => me.is_some_and(|me| accounts.contains(&me)),
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn option(n: u8) -> OptionId {
        OptionId::from_bytes([n; 16])
    }

    /// The search text of a value that names no page (the relation tests use the real one).
    fn search_text(
        value: &PropertyValue,
        option_name: &dyn Fn(OptionId) -> Option<String>,
        file_name: &dyn Fn(AttachmentId) -> Option<String>,
    ) -> Option<String> {
        super::search_text(value, option_name, file_name, &|_| None)
    }

    fn page(n: u8) -> PageId {
        PageId::from_bytes([n; 16])
    }

    fn file(n: u8) -> AttachmentId {
        AttachmentId::from_bytes([n; 16])
    }

    fn item(n: u8, text: &str, done: bool) -> ChecklistItem {
        ChecklistItem {
            id: ChecklistItemId::from_bytes([n; 16]),
            text: text.to_owned(),
            done,
        }
    }

    #[test]
    fn urls_are_absolute_with_an_accepted_scheme_and_no_spaces() {
        for ok in [
            "https://example.com",
            "http://localhost:8080/a?b=c#d",
            "HTTPS://Example.com/Path",
            "mailto:ada@example.com",
            "tel:+15550100",
            "https://example.com/ünï",
        ] {
            assert!(valid_url(ok), "{ok}");
        }
        for bad in [
            "",
            "example.com",
            "//example.com",
            "https://",
            "https:///path",
            "https:example.com",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://example.com",
            "data:text/html,x",
            "mailto:",
            "tel:",
            "https://exa mple.com",
            " https://example.com",
            "https://example.com ",
            "https://example.com/\n",
            "1http://example.com",
            "ht tp://example.com",
        ] {
            assert!(!valid_url(bad), "{bad:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert!(!valid_url(&long));
    }

    #[test]
    fn values_follow_their_rules_and_empties_mean_no_value() {
        let ok = |value: PropertyValue| check(&value).map(|kept| kept.is_some());
        assert!(ok(PropertyValue::Text("x".into())).unwrap());
        assert!(ok(PropertyValue::Text("line\nbreak\ttab".into())).unwrap());
        assert!(!ok(PropertyValue::Text(String::new())).unwrap());
        assert!(ok(PropertyValue::Text("a\u{0}".into())).is_err());
        assert!(ok(PropertyValue::Text("x".repeat(MAX_TEXT_CHARS + 1))).is_err());
        assert!(ok(PropertyValue::Text("x".repeat(MAX_TEXT_CHARS))).unwrap());

        assert!(ok(PropertyValue::Number(0.0)).unwrap());
        assert!(ok(PropertyValue::Number(-1.5e300)).unwrap());
        assert!(ok(PropertyValue::Number(f64::NAN)).is_err());
        assert!(ok(PropertyValue::Number(f64::INFINITY)).is_err());

        assert!(ok(PropertyValue::Checkbox(false)).unwrap());
        assert!(ok(PropertyValue::Select(option(1))).unwrap());

        assert!(!ok(PropertyValue::MultiSelect(Vec::new())).unwrap());
        assert!(ok(PropertyValue::MultiSelect(vec![option(1), option(2)])).unwrap());
        assert!(ok(PropertyValue::MultiSelect(vec![option(1), option(1)])).is_err());
        let many: Vec<OptionId> = (0..=u8::try_from(MAX_SELECTED_OPTIONS).unwrap())
            .map(option)
            .collect();
        assert!(ok(PropertyValue::MultiSelect(many)).is_err());

        let date = |date: &str, time: Option<&str>| {
            ok(PropertyValue::Date(DateValue {
                date: date.into(),
                time: time.map(Into::into),
            }))
        };
        assert!(date("2026-10-07", None).unwrap());
        assert!(date("2026-10-07", Some("14:30")).unwrap());
        assert!(date("2024-02-29", Some("00:00")).unwrap());
        assert!(date("2026-02-29", None).is_err());
        assert!(date("2026-13-01", None).is_err());
        assert!(date("26-10-07", None).is_err());
        assert!(date("2026-10-07", Some("24:00")).is_err());
        assert!(date("2026-10-07", Some("9:05")).is_err());
        assert!(date("2026-10-07", Some("")).is_err());

        assert!(!ok(PropertyValue::Url(String::new())).unwrap());
        assert!(ok(PropertyValue::Url("https://example.com".into())).unwrap());
        assert!(ok(PropertyValue::Url("example.com".into())).is_err());

        assert!(ok(PropertyValue::Email("ada@example.com".into())).unwrap());
        assert!(
            ok(PropertyValue::Email("not even an address".into())).unwrap(),
            "the format is the UI's check"
        );
        assert!(ok(PropertyValue::Email(String::new())).is_err());
        assert!(ok(PropertyValue::Email("   ".into())).is_err());
        assert!(ok(PropertyValue::Email("a\nb".into())).is_err());
        assert!(ok(PropertyValue::Email("a".repeat(MAX_EMAIL_CHARS + 1))).is_err());
        assert!(ok(PropertyValue::Phone("+1 (555) 010-0100".into())).unwrap());
        assert!(ok(PropertyValue::Phone(String::new())).is_err());
        assert!(ok(PropertyValue::Phone("1".repeat(MAX_PHONE_CHARS + 1))).is_err());
    }

    #[test]
    fn a_value_is_stored_and_read_back_unchanged_for_every_kind() {
        let cases = [
            PropertyValue::Text("Quarterly plan\nline two".into()),
            PropertyValue::Number(3.5),
            PropertyValue::Number(-0.0),
            PropertyValue::Checkbox(false),
            PropertyValue::Checkbox(true),
            PropertyValue::Date(DateValue {
                date: "2026-10-07".into(),
                time: None,
            }),
            PropertyValue::Date(DateValue {
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
            }),
            PropertyValue::Select(option(1)),
            PropertyValue::Status(option(2)),
            PropertyValue::MultiSelect(vec![option(4), option(3)]),
            PropertyValue::Url("https://example.com".into()),
            PropertyValue::Email("ada@example.com".into()),
            PropertyValue::Phone("+1 555 0100".into()),
            PropertyValue::Files(vec![file(9), file(3), file(5)]),
            PropertyValue::Checklist(vec![
                item(1, "Pack the charger", true),
                item(2, "", false),
                item(3, "Caf\u{e9} run", false),
            ]),
        ];
        for value in cases {
            let back = decode(value.kind(), &encode(&value));
            assert_eq!(back.as_ref(), Some(&value), "{value:?}");
        }
    }

    #[test]
    fn files_are_distinct_attachment_ids_at_most_fifty_and_empty_means_no_value() {
        let ok = |value: PropertyValue| check(&value).map(|kept| kept.is_some());
        assert!(ok(PropertyValue::Files(vec![file(1), file(2)])).unwrap());
        assert!(!ok(PropertyValue::Files(Vec::new())).unwrap());
        assert!(ok(PropertyValue::Files(vec![file(1), file(2), file(1)])).is_err());
        let limit = u8::try_from(MAX_FILES).unwrap();
        let many = |count: u8| PropertyValue::Files((1..=count).map(file).collect());
        assert!(ok(many(limit)).unwrap());
        assert!(ok(many(limit + 1)).is_err());
        let refused = check(&many(limit + 1)).unwrap_err();
        assert!(
            refused.to_string().contains("must list at most 50 files"),
            "{refused}"
        );
    }

    #[test]
    fn checklists_follow_their_rules_and_empty_means_no_value() {
        let ok = |items: Vec<ChecklistItem>| {
            check(&PropertyValue::Checklist(items)).map(|kept| kept.is_some())
        };
        assert!(ok(vec![item(1, "a", false), item(2, "b", true)]).unwrap());
        assert!(!ok(Vec::new()).unwrap());
        assert!(
            ok(vec![item(1, "", false)]).unwrap(),
            "an empty text is a valid item"
        );
        assert!(ok(vec![item(1, "a", false), item(1, "b", false)]).is_err());
        assert!(ok(vec![item(1, &"x".repeat(MAX_CHECKLIST_TEXT_CHARS), false)]).unwrap());
        assert!(
            ok(vec![item(
                1,
                &"x".repeat(MAX_CHECKLIST_TEXT_CHARS + 1),
                false
            )])
            .is_err()
        );
        assert!(
            ok(vec![item(
                1,
                &"\u{e9}".repeat(MAX_CHECKLIST_TEXT_CHARS),
                false
            )])
            .unwrap(),
            "characters, not bytes"
        );
        assert!(ok(vec![item(1, "line\nbreak", false)]).is_err());
        assert!(ok(vec![item(1, "tab\there", false)]).is_err());
        assert!(ok(vec![item(1, "nul\u{0}", false)]).is_err());
        let many = |count: usize| {
            (0..count)
                .map(|n| {
                    let mut bytes = [0_u8; 16];
                    bytes[0] = u8::try_from(n % 256).unwrap();
                    bytes[1] = u8::try_from(n / 256).unwrap();
                    ChecklistItem {
                        id: ChecklistItemId::from_bytes(bytes),
                        text: n.to_string(),
                        done: false,
                    }
                })
                .collect::<Vec<_>>()
        };
        assert!(ok(many(MAX_CHECKLIST_ITEMS)).unwrap());
        assert!(ok(many(MAX_CHECKLIST_ITEMS + 1)).is_err());
    }

    #[test]
    fn a_stored_value_of_the_wrong_shape_reads_as_nothing() {
        assert_eq!(decode(PropertyKind::Number, &LoroValue::from("3")), None);
        assert_eq!(decode(PropertyKind::Text, &LoroValue::Double(1.0)), None);
        assert_eq!(
            decode(PropertyKind::Checkbox, &LoroValue::from("true")),
            None
        );
        assert_eq!(
            decode(PropertyKind::Select, &LoroValue::from("not an id")),
            None
        );
        assert_eq!(
            decode(
                PropertyKind::MultiSelect,
                &LoroValue::from(vec![LoroValue::from(1_i64)])
            ),
            None
        );
        assert_eq!(
            decode(PropertyKind::Date, &LoroValue::from("2026-10-07")),
            None
        );
        assert_eq!(
            decode(
                PropertyKind::Date,
                &LoroValue::from("2026-10-07".to_owned())
            ),
            None
        );
        assert_eq!(
            decode(PropertyKind::Url, &LoroValue::from("javascript:x")),
            None
        );
        assert_eq!(decode(PropertyKind::Text, &LoroValue::from("")), None);
        assert_eq!(
            decode(PropertyKind::Number, &LoroValue::I64(7)),
            Some(PropertyValue::Number(7.0))
        );
        // Files: ids only; checklists: complete items with an id, a text and a flag.
        let list = |items: Vec<LoroValue>| LoroValue::from(items);
        assert_eq!(
            decode(PropertyKind::Files, &list(vec![LoroValue::from("nope")])),
            None
        );
        assert_eq!(
            decode(PropertyKind::Files, &list(vec![LoroValue::from(1_i64)])),
            None
        );
        assert_eq!(
            decode(PropertyKind::Files, &list(Vec::<LoroValue>::new())),
            None
        );
        assert_eq!(decode(PropertyKind::Files, &LoroValue::from("x")), None);
        let stored_item = |id: &str, text: LoroValue, done: LoroValue| {
            let mut map: HashMap<String, LoroValue> = HashMap::new();
            map.insert("id".to_owned(), LoroValue::from(id));
            map.insert("text".to_owned(), text);
            map.insert("done".to_owned(), done);
            LoroValue::from(map)
        };
        let id = ChecklistItemId::from_bytes([1; 16]).to_string();
        let good = stored_item(&id, LoroValue::from("a"), LoroValue::Bool(true));
        assert_eq!(
            decode(PropertyKind::Checklist, &list(vec![good.clone()])),
            Some(PropertyValue::Checklist(vec![item(1, "a", true)]))
        );
        for bad in [
            stored_item("nope", LoroValue::from("a"), LoroValue::Bool(true)),
            stored_item(&id, LoroValue::from(1_i64), LoroValue::Bool(true)),
            stored_item(&id, LoroValue::from("a"), LoroValue::from("yes")),
            stored_item(&id, LoroValue::from("a\nb"), LoroValue::Bool(false)),
            LoroValue::from("not a map"),
        ] {
            assert_eq!(decode(PropertyKind::Checklist, &list(vec![bad])), None);
        }
        // The same id twice breaks the value's own rules.
        assert_eq!(
            decode(PropertyKind::Checklist, &list(vec![good.clone(), good])),
            None
        );
        assert_eq!(
            decode(PropertyKind::Checklist, &list(Vec::<LoroValue>::new())),
            None
        );
    }

    #[test]
    fn search_text_is_option_names_and_text_values_only() {
        let no_file = |_: AttachmentId| -> Option<String> { None };
        let name = |id: OptionId| -> Option<String> {
            match id.to_bytes()[0] {
                1 => Some("urgent".into()),
                2 => Some("work".into()),
                _ => None,
            }
        };
        assert_eq!(
            search_text(
                &PropertyValue::MultiSelect(vec![option(1), option(9), option(2)]),
                &name,
                &no_file
            ),
            Some("urgent, work".into())
        );
        assert_eq!(
            search_text(&PropertyValue::Select(option(2)), &name, &no_file),
            Some("work".into())
        );
        assert_eq!(
            search_text(&PropertyValue::Status(option(9)), &name, &no_file),
            None
        );
        assert_eq!(
            search_text(&PropertyValue::Text("Hello".into()), &name, &no_file),
            Some("Hello".into())
        );
        assert_eq!(
            search_text(&PropertyValue::Email("a@b.c".into()), &name, &no_file),
            Some("a@b.c".into())
        );
        // Files read as the names of the manifests, in order; those without one add nothing.
        let file_name = |id: AttachmentId| -> Option<String> {
            match id.to_bytes()[0] {
                1 => Some("plan.pdf".into()),
                2 => Some("atomik.png".into()),
                _ => None,
            }
        };
        assert_eq!(
            search_text(
                &PropertyValue::Files(vec![file(2), file(9), file(1)]),
                &name,
                &file_name
            ),
            Some("atomik.png, plan.pdf".into())
        );
        assert_eq!(
            search_text(&PropertyValue::Files(vec![file(9)]), &name, &file_name),
            None
        );
        // A checklist reads as its item texts; blank items add nothing.
        assert_eq!(
            search_text(
                &PropertyValue::Checklist(vec![
                    item(1, "Pack", false),
                    item(2, "  ", true),
                    item(3, "Call Ada", true)
                ]),
                &name,
                &no_file
            ),
            Some("Pack, Call Ada".into())
        );
        assert_eq!(
            search_text(
                &PropertyValue::Checklist(vec![item(1, "", false)]),
                &name,
                &no_file
            ),
            None
        );
        assert_eq!(
            search_text(&PropertyValue::Number(1.0), &name, &no_file),
            None
        );
        assert_eq!(
            search_text(&PropertyValue::Checkbox(true), &name, &no_file),
            None
        );
        assert_eq!(
            search_text(
                &PropertyValue::Date(DateValue {
                    date: "2026-10-07".into(),
                    time: None
                }),
                &name,
                &no_file
            ),
            None
        );
    }

    #[test]
    fn relations_are_distinct_page_ids_at_most_one_hundred_and_empty_means_no_value() {
        let ok = |value: PropertyValue| check(&value).map(|kept| kept.is_some());
        assert!(ok(PropertyValue::Relation(vec![page(1), page(2)])).unwrap());
        assert!(!ok(PropertyValue::Relation(Vec::new())).unwrap());
        let refused = check(&PropertyValue::Relation(vec![page(1), page(2), page(1)])).unwrap_err();
        assert!(
            refused.to_string().contains("page ids must be distinct"),
            "{refused}"
        );
        let limit = u8::try_from(MAX_RELATED_PAGES).unwrap();
        let many = |count: u8| PropertyValue::Relation((1..=count).map(page).collect());
        assert!(ok(many(limit)).unwrap());
        let refused = check(&many(limit + 1)).unwrap_err();
        assert!(
            refused.to_string().contains("must list at most 100 pages"),
            "{refused}"
        );
        // Not checked: whether a page exists (like an attachment of a files value).
        assert!(ok(PropertyValue::Relation(vec![page(250)])).unwrap());
    }

    #[test]
    fn a_relation_is_stored_as_a_list_of_ids_and_a_damaged_one_reads_as_nothing() {
        let value = PropertyValue::Relation(vec![page(4), page(3)]);
        assert_eq!(decode(PropertyKind::Relation, &encode(&value)), Some(value));
        for bad in [
            LoroValue::from("x"),
            LoroValue::from(vec![LoroValue::from("nope")]),
            LoroValue::from(vec![LoroValue::from(1_i64)]),
            LoroValue::from(Vec::<LoroValue>::new()),
        ] {
            assert_eq!(decode(PropertyKind::Relation, &bad), None);
        }
    }

    #[test]
    fn a_relation_searches_as_the_titles_of_its_pages() {
        let title = |id: PageId| -> Option<String> {
            match id.to_bytes()[0] {
                1 => Some("Roadmap".into()),
                2 => Some("  ".into()),
                3 => Some("Budget".into()),
                _ => None,
            }
        };
        let none = |_: AttachmentId| -> Option<String> { None };
        let no_option = |_: OptionId| -> Option<String> { None };
        assert_eq!(
            super::search_text(
                &PropertyValue::Relation(vec![page(3), page(9), page(2), page(1)]),
                &no_option,
                &none,
                &title
            ),
            Some("Budget, Roadmap".into())
        );
        assert_eq!(
            super::search_text(
                &PropertyValue::Relation(vec![page(9)]),
                &no_option,
                &none,
                &title
            ),
            None
        );
    }
    fn who(n: u8) -> AccountId {
        AccountId::from_bytes([0xac, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
    }

    fn people(ns: &[u8]) -> PropertyValue {
        PropertyValue::Person(ns.iter().map(|n| who(*n)).collect())
    }

    #[test]
    fn a_person_value_lists_at_most_twenty_distinct_accounts_and_an_empty_list_is_no_value() {
        assert_eq!(
            check(&people(&[1, 2, 3])).unwrap(),
            Some(&people(&[1, 2, 3]))
        );
        let limit = u8::try_from(MAX_PEOPLE).unwrap();
        let twenty: Vec<u8> = (1..=limit).collect();
        assert!(check(&people(&twenty)).unwrap().is_some());
        let twenty_one: Vec<u8> = (1..=limit + 1).collect();
        let refused = check(&people(&twenty_one)).unwrap_err();
        assert!(
            refused.to_string().contains("must list at most 20 people"),
            "{refused}"
        );
        let refused = check(&people(&[1, 2, 1])).unwrap_err();
        assert!(
            refused.to_string().contains("account ids must be distinct"),
            "{refused}"
        );
        assert_eq!(check(&people(&[])).unwrap(), None);
        // Not checked here: that the accounts are members of the space (the core's rule).
        assert!(check(&people(&[250])).unwrap().is_some());
    }

    #[test]
    fn a_person_value_is_stored_as_a_list_of_account_ids_and_a_damaged_one_reads_as_nothing() {
        let value = people(&[4, 3]);
        let stored = encode(&value);
        assert_eq!(
            stored,
            LoroValue::from(vec![
                LoroValue::from(who(4).to_string()),
                LoroValue::from(who(3).to_string())
            ])
        );
        assert_eq!(decode(PropertyKind::Person, &stored), Some(value.clone()));
        let twenty_one: Vec<LoroValue> = (1..=21)
            .map(|n| LoroValue::from(who(n).to_string()))
            .collect();
        let duplicated = vec![
            LoroValue::from(who(1).to_string()),
            LoroValue::from(who(1).to_string()),
        ];
        for bad in [
            LoroValue::from("x"),
            LoroValue::from(who(1).to_string()),
            LoroValue::from(vec![LoroValue::from("nope")]),
            LoroValue::from(vec![LoroValue::from(1_i64)]),
            LoroValue::from(vec![
                LoroValue::from(who(1).to_string()),
                LoroValue::from("x"),
            ]),
            LoroValue::from(Vec::<LoroValue>::new()),
            LoroValue::from(twenty_one),
            LoroValue::from(duplicated),
            LoroValue::Null,
        ] {
            assert_eq!(decode(PropertyKind::Person, &bad), None, "{bad:?}");
        }
        // Ids of every kind are UUID text, so the property's kind alone says what a list holds.
        assert_eq!(
            decode(
                PropertyKind::Person,
                &encode(&PropertyValue::Relation(vec![page(1)]))
            ),
            Some(PropertyValue::Person(vec![AccountId::from_bytes([1; 16])]))
        );
    }

    #[test]
    fn a_person_value_has_no_text_of_its_own() {
        // The names are the core's to resolve (they change with a rename): `person_text` joins them.
        let none = |_: AttachmentId| -> Option<String> { None };
        let no_option = |_: OptionId| -> Option<String> { None };
        let title = |_: PageId| -> Option<String> { Some("not a title".into()) };
        assert_eq!(
            super::search_text(&people(&[1, 2]), &no_option, &none, &title),
            None
        );
    }
}
