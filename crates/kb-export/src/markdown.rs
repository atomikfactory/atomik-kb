//! A page as a Markdown file: YAML front matter, then the body in the file flavour of
//! `kb_model::render_markdown_file` (`docs/architecture/import-export.md`, "Front matter").
//!
//! ```yaml
//! ---
//! title: "Weekly review"
//! icon: "📝"
//! cover: "gradient:sunset"
//! cover_y: 30
//! created: "2026-10-01T09:30:00Z"
//! updated: "2026-10-05T18:02:11Z"
//! tags:
//!   - "work"
//!   - "review"
//! "Status": "Done"
//! "Due": "2026-10-07 14:30"
//! "Done": true
//! "Estimate": 3.5
//! "Related":
//!   - "[[Other page]]"
//! "Attachments":
//!   - "Weekly review/report.pdf"
//! "Checklist":
//!   - "[x] Buy milk"
//!   - "[ ] Call Ann"
//! "Place":
//!   name: "Paris"
//!   lat: 48.8566
//!   lon: 2.3522
//! ---
//! ```
//!
//! It is always written, in this key order: `title` (always, as stored, even empty), `icon` (when
//! set; an uploaded icon as the relative path of its file, which the core leaves out when the file
//! is not in the archive), `cover` (when set, ADR-0040: `color:<name>` or `gradient:<name>` as
//! stored, an uploaded cover as the relative path of its file, left out like an icon when the file
//! is not in the archive) and `cover_y` (the vertical position in percent, a number, only when it
//! is not the default 50), `template: true` (only on a template, ADR-0056) and `templatePlaceholders`
//! (only when there is one: a mapping from the property's name, double-quoted, to `"today"`, `"now"` or
//! `"me"`), `created` and `updated` (RFC 3339, UTC, quoted), `tags` (the built-in
//! Tags property: lower case, Obsidian's convention), then the other properties in the order they
//! come, each under its name, always double-quoted. Strings are double-quoted YAML scalars with JSON
//! escapes (plus `\uXXXX` for the characters YAML does not allow raw); numbers are JSON numbers
//! (an integral one without `.0`); checkboxes `true` or `false`; select and status the option name;
//! multi-select a block list of names; dates `"YYYY-MM-DD"` or `"YYYY-MM-DD HH:mm"`; relations a
//! list of `"[[Title]]"`; files a list of relative paths (not percent-encoded; the file's name when
//! it is not in the archive); checklists a list of `"[x] text"` and `"[ ] text"`; places a map of
//! `name`, `lat` and `lon`; people (a Person property, ADR-0049) the names of the people, which the
//! core hands over as `ExportValue::Options` (one name a scalar, several a list; never an account
//! id). Empty values and the computed kinds are not written.
//!
//! A property whose name is, in any case, one of the keys above (`title`, `icon`, `cover`,
//! `cover_y`, `template`, `templatePlaceholders`, `created`, `updated`, `tags`) or of the keys the importer ignores (`aliases`, `cssclasses`, `cssclass`,
//! `publish`, `permalink`, `layout`), or the name of a property written before it, is written as
//! `<name> (2)`, `<name> (3)`, ...: a mapping with a repeated key is not YAML, and the importer
//! reads keys in lower case, so a `Title` would become the title and a `Layout` would be dropped.
//!
//! `ExportValue::Options` does not say whether it comes from a select or from a multi-select, so a
//! list of one name (other than the Tags') is written as a scalar, as a select's.

use std::collections::HashSet;

use percent_encoding::percent_decode_str;
use serde_json::Value;

use crate::ExportLinks;
use crate::time::rfc3339_utc;
use crate::value::{ExportProperty, ExportValue, PageMeta};

/// The keys of the front matter that are not properties (and the ones the importer ignores): a
/// property of the same name is written under another one.
const RESERVED_KEYS: [&str; 15] = [
    "title",
    "icon",
    "cover",
    "cover_y",
    "template",
    "templateplaceholders",
    "created",
    "updated",
    "tags",
    "aliases",
    "cssclasses",
    "cssclass",
    "publish",
    "permalink",
    "layout",
];

/// The whole Markdown file of a page: front matter (always, it holds at least the title) and
/// the body, ending with exactly one newline. An empty page is its front matter alone.
#[must_use]
pub fn page_markdown(meta: &PageMeta, content: &Value, links: &dyn ExportLinks) -> String {
    let body = kb_model::render_markdown_file(content, links);
    let mut out = front_matter(meta);
    if !body.trim().is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

/// The front matter block, `---` to `---`, each line ended with a newline.
fn front_matter(meta: &PageMeta) -> String {
    let mut lines: Vec<String> = vec!["---".to_owned()];
    lines.push(format!("title: {}", quote(&meta.title)));
    if let Some(icon) = meta.icon.as_deref().filter(|icon| !icon.is_empty()) {
        lines.push(format!("icon: {}", quote(&icon_text(icon))));
    }
    if let Some(cover) = meta.cover.as_deref().filter(|cover| !cover.is_empty()) {
        lines.push(format!("cover: {}", quote(&icon_text(cover))));
        if let Some(y) = meta.cover_y.filter(|y| *y <= 100 && *y != 50) {
            lines.push(format!("cover_y: {y}"));
        }
    }
    if meta.template {
        lines.push("template: true".to_owned());
    }
    template_placeholders(&mut lines, &meta.template_placeholders);
    lines.push(format!("created: {}", quote(&rfc3339_utc(meta.created_at))));
    lines.push(format!("updated: {}", quote(&rfc3339_utc(meta.updated_at))));

    for tags in meta.properties.iter().filter(|property| property.tags) {
        if let ExportValue::Options(names) = &tags.value {
            list(
                &mut lines,
                "tags",
                names
                    .iter()
                    .filter(|name| !name.is_empty())
                    .map(|name| quote(name)),
            );
        }
    }

    let mut used: HashSet<String> = RESERVED_KEYS.iter().map(|key| (*key).to_owned()).collect();
    for property in meta.properties.iter().filter(|property| !property.tags) {
        let key = quote(&unique_key(&mut used, &property.name));
        write_property(&mut lines, &key, property);
    }
    lines.push("---".to_owned());
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The `templatePlaceholders` mapping of a page (ADR-0056): each property's name, double-quoted, to its
/// placeholder (`"today"`, `"now"` or `"me"`), in the order given. Nothing when there is none; a name
/// that repeats (in any case, like the keys of the other properties) keeps its first entry, because a
/// mapping with a repeated key is not YAML.
fn template_placeholders(
    lines: &mut Vec<String>,
    placeholders: &[(String, kb_types::TemplatePlaceholder)],
) {
    let mut seen: HashSet<String> = HashSet::new();
    let entries: Vec<String> = placeholders
        .iter()
        .filter(|(name, _)| !name.is_empty() && seen.insert(name.to_lowercase()))
        .map(|(name, placeholder)| format!("  {}: {}", quote(name), quote(placeholder.as_str())))
        .collect();
    if entries.is_empty() {
        return;
    }
    lines.push("templatePlaceholders:".to_owned());
    lines.extend(entries);
}

/// `name` or, when it is taken (in any case: the importer reads keys in lower case), `name (2)`,
/// `name (3)`, ...; the key is claimed.
fn unique_key(used: &mut HashSet<String>, name: &str) -> String {
    let mut key = name.to_owned();
    let mut number = 2_u32;
    while !used.insert(key.to_lowercase()) {
        key = format!("{name} ({number})");
        number += 1;
    }
    key
}

/// The lines of one property (none when its value is empty or computed).
fn write_property(lines: &mut Vec<String>, key: &str, property: &ExportProperty) {
    match &property.value {
        ExportValue::Text(text) if !text.is_empty() => {
            lines.push(format!("{key}: {}", quote(text)))
        }
        ExportValue::Number(number) if number.is_finite() => {
            lines.push(format!("{key}: {number}"));
        }
        ExportValue::Checkbox(on) => lines.push(format!("{key}: {on}")),
        ExportValue::Date { date, time } => {
            let text = match time {
                Some(time) => format!("{date} {time}"),
                None => date.clone(),
            };
            lines.push(format!("{key}: {}", quote(&text)));
        }
        ExportValue::Options(names) => {
            let names: Vec<&String> = names.iter().filter(|name| !name.is_empty()).collect();
            match names.as_slice() {
                [] => {}
                [one] => lines.push(format!("{key}: {}", quote(one))),
                many => list(lines, key, many.iter().map(|name| quote(name))),
            }
        }
        ExportValue::Pages(pages) => list(
            lines,
            key,
            pages
                .iter()
                .map(|page| quote(&format!("[[{}]]", page.title))),
        ),
        ExportValue::Files(files) => list(
            lines,
            key,
            files.iter().map(|file| {
                quote(&match &file.href {
                    Some(href) => percent_decode_str(href).decode_utf8_lossy().into_owned(),
                    None => file.name.clone(),
                })
            }),
        ),
        ExportValue::Checklist(items) => list(
            lines,
            key,
            items
                .iter()
                .map(|(done, text)| quote(&format!("[{}] {text}", if *done { 'x' } else { ' ' }))),
        ),
        ExportValue::Place { name, lat, lon } => {
            let mut fields: Vec<String> = Vec::new();
            if !name.is_empty() {
                fields.push(format!("  name: {}", quote(name)));
            }
            if let (Some(lat), Some(lon)) = (lat, lon)
                && lat.is_finite()
                && lon.is_finite()
            {
                fields.push(format!("  lat: {lat}"));
                fields.push(format!("  lon: {lon}"));
            }
            if !fields.is_empty() {
                lines.push(format!("{key}:"));
                lines.extend(fields);
            }
        }
        // Empty text, a number that is not one, an instant: nothing to say.
        ExportValue::Text(_) | ExportValue::Number(_) | ExportValue::Instant(_) => {}
    }
}

/// A block list under `key` (nothing when there is no item).
fn list(lines: &mut Vec<String>, key: &str, items: impl Iterator<Item = String>) {
    let items: Vec<String> = items.collect();
    if items.is_empty() {
        return;
    }
    lines.push(format!("{key}:"));
    lines.extend(items.into_iter().map(|item| format!("  - {item}")));
}

/// `text` as a double-quoted YAML scalar: JSON escapes, and `\uXXXX` for the characters of the
/// rest of the C0 and C1 controls, the line and paragraph separators and the byte order mark,
/// which a YAML stream cannot carry raw (or reads as line breaks).
fn quote(text: &str) -> String {
    let json = Value::String(text.to_owned()).to_string();
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') {
            out.push_str(&format!("\\u{:04x}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// The icon or cover as the front matter says it: an emoji, `icon:<name>`, `color:<name>` or
/// `gradient:<name>` as stored, an uploaded one (the relative link the core gives) as the path of its
/// file, not percent-encoded.
fn icon_text(icon: &str) -> String {
    let is_link = !icon.starts_with("icon:")
        && !icon.starts_with("color:")
        && !icon.starts_with("gradient:")
        && !icon.contains(':')
        && (icon.contains('/') || icon.contains('.'));
    if is_link {
        percent_decode_str(icon).decode_utf8_lossy().into_owned()
    } else {
        icon.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use serde_json::json;
    use yaml_rust2::{Yaml, YamlLoader};

    use super::*;
    use crate::test_support::TestLinks;
    use crate::value::{LinkedFile, LinkedPage};

    fn meta(properties: Vec<ExportProperty>) -> PageMeta {
        PageMeta {
            page_id: None,
            title: "Weekly review".to_owned(),
            icon: Some("📝".to_owned()),
            cover: None,
            cover_y: None,
            template: false,
            template_placeholders: Vec::new(),
            // 2025-10-09T08:53:20Z and 2025-10-12T09:30:00Z.
            created_at: 1_760_000_000_000,
            updated_at: 1_760_261_400_999,
            properties,
        }
    }

    fn prop(name: &str, value: ExportValue) -> ExportProperty {
        ExportProperty {
            name: name.to_owned(),
            tags: false,
            value,
        }
    }

    fn tags(names: &[&str]) -> ExportProperty {
        ExportProperty {
            name: "Tags".to_owned(),
            tags: true,
            value: ExportValue::Options(names.iter().map(|n| (*n).to_owned()).collect()),
        }
    }

    fn names(names: &[&str]) -> ExportValue {
        ExportValue::Options(names.iter().map(|n| (*n).to_owned()).collect())
    }

    /// The front matter parsed back by a YAML parser.
    fn parsed(front_matter: &str) -> Yaml {
        let inner = front_matter
            .strip_prefix("---\n")
            .and_then(|rest| rest.strip_suffix("---\n"))
            .unwrap();
        YamlLoader::load_from_str(inner).unwrap().remove(0)
    }

    #[test]
    fn the_documented_example_is_written_exactly() {
        let page = meta(vec![
            tags(&["work", "review"]),
            prop("Status", names(&["Done"])),
            prop(
                "Due",
                ExportValue::Date {
                    date: "2026-10-07".into(),
                    time: Some("14:30".into()),
                },
            ),
            prop("Done", ExportValue::Checkbox(true)),
            prop("Estimate", ExportValue::Number(3.5)),
            prop(
                "Related",
                ExportValue::Pages(vec![LinkedPage {
                    title: "Other page".into(),
                    href: Some("Other%20page.md".into()),
                }]),
            ),
            prop(
                "Attachments",
                ExportValue::Files(vec![LinkedFile {
                    name: "report.pdf".into(),
                    href: Some("Weekly%20review/report.pdf".into()),
                }]),
            ),
            prop(
                "Checklist",
                ExportValue::Checklist(vec![(true, "Buy milk".into()), (false, "Call Ann".into())]),
            ),
            prop(
                "Place",
                ExportValue::Place {
                    name: "Paris".into(),
                    lat: Some(48.8566),
                    lon: Some(2.3522),
                },
            ),
        ]);
        let out = page_markdown(&page, &json!({ "type": "doc" }), &TestLinks::default());
        assert_eq!(
            out,
            "---\n\
             title: \"Weekly review\"\n\
             icon: \"📝\"\n\
             created: \"2025-10-09T08:53:20Z\"\n\
             updated: \"2025-10-12T09:30:00Z\"\n\
             tags:\n  - \"work\"\n  - \"review\"\n\
             \"Status\": \"Done\"\n\
             \"Due\": \"2026-10-07 14:30\"\n\
             \"Done\": true\n\
             \"Estimate\": 3.5\n\
             \"Related\":\n  - \"[[Other page]]\"\n\
             \"Attachments\":\n  - \"Weekly review/report.pdf\"\n\
             \"Checklist\":\n  - \"[x] Buy milk\"\n  - \"[ ] Call Ann\"\n\
             \"Place\":\n  name: \"Paris\"\n  lat: 48.8566\n  lon: 2.3522\n\
             ---\n"
        );
        // And it is YAML that says the same.
        let yaml = parsed(&out);
        assert_eq!(yaml["title"].as_str(), Some("Weekly review"));
        assert_eq!(yaml["icon"].as_str(), Some("📝"));
        assert_eq!(yaml["tags"][1].as_str(), Some("review"));
        assert_eq!(yaml["Status"].as_str(), Some("Done"));
        assert_eq!(yaml["Due"].as_str(), Some("2026-10-07 14:30"));
        assert_eq!(yaml["Done"].as_bool(), Some(true));
        assert_eq!(yaml["Estimate"].as_f64(), Some(3.5));
        assert_eq!(yaml["Related"][0].as_str(), Some("[[Other page]]"));
        assert_eq!(
            yaml["Attachments"][0].as_str(),
            Some("Weekly review/report.pdf")
        );
        assert_eq!(yaml["Checklist"][1].as_str(), Some("[ ] Call Ann"));
        assert_eq!(yaml["Place"]["name"].as_str(), Some("Paris"));
        assert_eq!(yaml["Place"]["lat"].as_f64(), Some(48.8566));
        assert_eq!(yaml["Place"]["lon"].as_f64(), Some(2.3522));
    }

    #[test]
    fn an_empty_page_is_its_front_matter_alone() {
        let out = page_markdown(
            &meta(vec![]),
            &json!({ "type": "doc" }),
            &TestLinks::default(),
        );
        assert_eq!(
            out,
            "---\ntitle: \"Weekly review\"\nicon: \"📝\"\ncreated: \"2025-10-09T08:53:20Z\"\nupdated: \"2025-10-12T09:30:00Z\"\n---\n"
        );
        // Empty paragraphs are empty too.
        let blank =
            json!({ "type": "doc", "content": [{ "type": "paragraph" }, { "type": "paragraph" }] });
        assert_eq!(
            page_markdown(&meta(vec![]), &blank, &TestLinks::default()),
            out
        );
    }

    #[test]
    fn the_body_follows_after_a_blank_line_and_ends_with_one_newline() {
        let content = json!({
            "type": "doc",
            "content": [
                { "type": "heading", "attrs": { "level": 1 }, "content": [{ "type": "text", "text": "Hello" }] },
                { "type": "paragraph", "content": [{ "type": "text", "text": "A *star*" }] },
            ]
        });
        let out = page_markdown(&meta(vec![]), &content, &TestLinks::default());
        assert!(out.ends_with("---\n\n# Hello\n\nA \\*star\\*\n"), "{out}");
        assert!(!out.ends_with("\n\n"));
        // The body is the file flavour of the renderer, byte for byte.
        let body = kb_model::render_markdown_file(&content, &TestLinks::default());
        assert!(out.ends_with(&body));
    }

    #[test]
    fn a_minimal_page_still_has_a_title() {
        let bare = PageMeta {
            title: String::new(),
            ..PageMeta::default()
        };
        let out = page_markdown(&bare, &json!({ "type": "doc" }), &TestLinks::default());
        assert_eq!(
            out,
            "---\ntitle: \"\"\ncreated: \"1970-01-01T00:00:00Z\"\nupdated: \"1970-01-01T00:00:00Z\"\n---\n"
        );
        assert_eq!(parsed(&out)["title"].as_str(), Some(""));
    }

    #[test]
    fn the_icon_is_an_emoji_a_name_or_the_path_of_a_file() {
        let with_icon = |icon: Option<&str>| {
            let mut page = meta(vec![]);
            page.icon = icon.map(str::to_owned);
            front_matter(&page)
        };
        assert!(with_icon(Some("🚀")).contains("icon: \"🚀\"\n"));
        assert!(with_icon(Some("icon:rocket:blue")).contains("icon: \"icon:rocket:blue\"\n"));
        // An uploaded icon arrives as a relative link and is written as a path.
        assert!(with_icon(Some("Notes/my%20icon.png")).contains("icon: \"Notes/my icon.png\"\n"));
        assert!(with_icon(Some("../Notes/icon.png")).contains("icon: \"../Notes/icon.png\"\n"));
        // Left out when there is none (or the file is not in the archive).
        assert!(!with_icon(None).contains("icon"));
        assert!(!with_icon(Some("")).contains("icon"));
    }

    #[test]
    fn the_cover_is_a_named_one_or_the_path_of_a_file_and_the_position_is_written_unless_default() {
        let with_cover = |cover: Option<&str>, y: Option<u8>| {
            let mut page = meta(vec![]);
            page.cover = cover.map(str::to_owned);
            page.cover_y = y;
            front_matter(&page)
        };
        // Named covers as stored; the position as a number, only when it is not the default.
        let out = with_cover(Some("gradient:sunset"), Some(30));
        assert!(
            out.contains("icon: \"📝\"\ncover: \"gradient:sunset\"\ncover_y: 30\ncreated:"),
            "{out}"
        );
        assert!(with_cover(Some("color:blue"), None).contains("cover: \"color:blue\"\ncreated:"));
        for default in [None, Some(50)] {
            let out = with_cover(Some("color:blue"), default);
            assert!(!out.contains("cover_y"), "{out}");
        }
        assert!(with_cover(Some("color:red"), Some(0)).contains("cover_y: 0\n"));
        assert!(with_cover(Some("color:red"), Some(100)).contains("cover_y: 100\n"));
        // An uploaded cover arrives as a relative link and is written as a path, like an icon.
        let out = with_cover(Some("Notes/my%20banner.png"), Some(80));
        assert!(
            out.contains("cover: \"Notes/my banner.png\"\ncover_y: 80\n"),
            "{out}"
        );
        assert!(
            with_cover(Some("../Notes/banner.png"), None)
                .contains("cover: \"../Notes/banner.png\"\n")
        );
        // Left out when there is none (or the file is not in the archive); a position needs a cover.
        for out in [
            with_cover(None, None),
            with_cover(None, Some(30)),
            with_cover(Some(""), Some(30)),
        ] {
            assert!(!out.contains("cover"), "{out}");
        }
        // The result is YAML a reader gets the same values from, numbers as numbers.
        let yaml = parsed(&with_cover(Some("gradient:dawn"), Some(25)));
        assert_eq!(yaml["cover"].as_str(), Some("gradient:dawn"));
        assert_eq!(yaml["cover_y"].as_i64(), Some(25));
    }

    #[test]
    fn a_property_called_cover_does_not_take_the_covers_key() {
        let mut page = meta(vec![
            prop("Cover", ExportValue::Text("mine".into())),
            prop("cover_y", ExportValue::Number(7.0)),
        ]);
        page.cover = Some("color:green".to_owned());
        page.cover_y = Some(10);
        let out = front_matter(&page);
        assert!(
            out.contains("cover: \"color:green\"\ncover_y: 10\n"),
            "{out}"
        );
        assert!(out.contains("\"Cover (2)\": \"mine\"\n"), "{out}");
        assert!(out.contains("\"cover_y (2)\": 7\n"), "{out}");
        let yaml = parsed(&out);
        assert_eq!(yaml["cover"].as_str(), Some("color:green"));
        assert_eq!(yaml["cover_y"].as_i64(), Some(10));
    }

    #[test]
    fn tags_are_a_lower_case_list_whatever_their_count() {
        let one = front_matter(&meta(vec![tags(&["solo"])]));
        assert!(one.contains("tags:\n  - \"solo\"\n"), "{one}");
        let none = front_matter(&meta(vec![tags(&[])]));
        assert!(!none.contains("tags"), "{none}");
        // They come before the other properties, wherever they are in the list.
        let late = front_matter(&meta(vec![
            prop("Zed", ExportValue::Number(1.0)),
            tags(&["a", "b"]),
        ]));
        assert!(late.find("tags:").unwrap() < late.find("\"Zed\"").unwrap());
    }

    #[test]
    fn numbers_are_json_numbers() {
        let number = |n: f64| {
            front_matter(&meta(vec![prop("N", ExportValue::Number(n))]))
                .lines()
                .find(|line| line.starts_with("\"N\""))
                .unwrap()
                .to_owned()
        };
        assert_eq!(number(3.0), "\"N\": 3");
        assert_eq!(number(-3.0), "\"N\": -3");
        assert_eq!(number(0.0), "\"N\": 0");
        assert_eq!(number(3.5), "\"N\": 3.5");
        assert_eq!(number(0.1), "\"N\": 0.1");
        assert_eq!(number(-0.000_25), "\"N\": -0.00025");
        assert_eq!(number(1e21), "\"N\": 1000000000000000000000");
        // Not finite: not a value.
        let none = front_matter(&meta(vec![
            prop("N", ExportValue::Number(f64::NAN)),
            prop("M", ExportValue::Number(f64::INFINITY)),
        ]));
        assert!(!none.contains("\"N\"") && !none.contains("\"M\""));
    }

    #[test]
    fn options_are_a_name_or_a_list() {
        let out = front_matter(&meta(vec![
            prop("Status", names(&["Done"])),
            prop("Labels", names(&["a", "b", "c"])),
            prop("Nothing", names(&[])),
            prop("Blank", names(&[""])),
        ]));
        assert!(out.contains("\"Status\": \"Done\"\n"), "{out}");
        assert!(
            out.contains("\"Labels\":\n  - \"a\"\n  - \"b\"\n  - \"c\"\n"),
            "{out}"
        );
        assert!(!out.contains("Nothing") && !out.contains("Blank"), "{out}");
    }

    #[test]
    fn dates_files_checklists_and_places() {
        let out = front_matter(&meta(vec![
            prop(
                "Day",
                ExportValue::Date {
                    date: "2026-10-07".into(),
                    time: None,
                },
            ),
            prop(
                "Files",
                ExportValue::Files(vec![
                    LinkedFile {
                        name: "in archive.pdf".into(),
                        href: Some("Page/in%20archive%20%281%29.pdf".into()),
                    },
                    LinkedFile {
                        name: "elsewhere.pdf".into(),
                        href: None,
                    },
                ]),
            ),
            prop(
                "Items",
                ExportValue::Checklist(vec![(false, "".into()), (true, "a \"quoted\" one".into())]),
            ),
            prop(
                "Named",
                ExportValue::Place {
                    name: "Home".into(),
                    lat: None,
                    lon: None,
                },
            ),
            prop(
                "Half",
                ExportValue::Place {
                    name: "Half".into(),
                    lat: Some(1.0),
                    lon: None,
                },
            ),
            prop(
                "Bare",
                ExportValue::Place {
                    name: String::new(),
                    lat: Some(-12.5),
                    lon: Some(100.0),
                },
            ),
            prop(
                "Void",
                ExportValue::Place {
                    name: String::new(),
                    lat: None,
                    lon: None,
                },
            ),
        ]));
        assert!(out.contains("\"Day\": \"2026-10-07\"\n"), "{out}");
        assert!(
            out.contains("\"Files\":\n  - \"Page/in archive (1).pdf\"\n  - \"elsewhere.pdf\"\n"),
            "{out}"
        );
        assert!(
            out.contains("\"Items\":\n  - \"[ ] \"\n  - \"[x] a \\\"quoted\\\" one\"\n"),
            "{out}"
        );
        assert!(out.contains("\"Named\":\n  name: \"Home\"\n"), "{out}");
        assert!(out.contains("\"Half\":\n  name: \"Half\"\n"), "{out}");
        assert!(!out.contains("\"Half\":\n  name: \"Half\"\n  lat"), "{out}");
        assert!(
            out.contains("\"Bare\":\n  lat: -12.5\n  lon: 100\n"),
            "{out}"
        );
        assert!(!out.contains("Void"), "{out}");
        assert!(parsed(&out)["Items"][1].as_str().is_some());
    }

    #[test]
    fn empty_values_and_instants_are_not_written() {
        let out = front_matter(&meta(vec![
            prop("Text", ExportValue::Text(String::new())),
            prop("Pages", ExportValue::Pages(vec![])),
            prop("Files", ExportValue::Files(vec![])),
            prop("Checklist", ExportValue::Checklist(vec![])),
            prop("Created", ExportValue::Instant(5)),
        ]));
        for name in ["Text", "Pages", "Files", "Checklist", "Created"] {
            assert!(!out.contains(&format!("\"{name}\"")), "{name} in {out}");
        }
        // A box that is off is a value.
        let off = front_matter(&meta(vec![prop("Off", ExportValue::Checkbox(false))]));
        assert!(off.contains("\"Off\": false\n"));
    }

    #[test]
    fn strings_are_valid_yaml_whatever_they_hold() {
        let hostile = [
            "plain",
            "with \"quotes\" and \\ backslash",
            "line one\nline two\r\nthree\ttab",
            "---",
            "...",
            "- list: item # comment",
            "key: value",
            "*alias &anchor !tag | > %",
            "null",
            "true",
            "123",
            "0x1F",
            "~",
            "  padded  ",
            "[[Wiki]] {flow: map}",
            "\u{0}\u{1}\u{7f}\u{85}\u{2028}\u{2029}\u{feff}",
            "café 日本語 👩‍💻 𝔘𝔫𝔦",
            "",
        ];
        for text in hostile {
            // As the title, a text value and a property name.
            let mut page = meta(vec![
                prop(text, ExportValue::Text(format!("v{text}"))),
                prop("Opt", names(&[text, "x"])),
            ]);
            page.title = text.to_owned();
            let out = front_matter(&page);
            let yaml = parsed(&out);
            assert_eq!(yaml["title"].as_str(), Some(text), "{text:?} in {out}");
            if !text.is_empty() {
                // The key is the name (a name that is a reserved key or repeated is renamed).
                assert_eq!(
                    yaml[text].as_str(),
                    Some(format!("v{text}").as_str()),
                    "{text:?} in {out}"
                );
            }
            if text.is_empty() {
                assert_eq!(yaml["Opt"].as_str(), Some("x"), "{out}");
            } else {
                assert_eq!(yaml["Opt"][0].as_str(), Some(text), "{text:?} in {out}");
                assert_eq!(yaml["Opt"][1].as_str(), Some("x"), "{text:?} in {out}");
            }
        }
    }

    #[test]
    fn no_raw_control_character_reaches_the_yaml() {
        let page = PageMeta {
            title: "a\u{0}b\u{1f}c\u{7f}d\u{80}e\u{9f}f\u{2028}g\u{feff}h".to_owned(),
            ..PageMeta::default()
        };
        let out = front_matter(&page);
        assert!(
            out.contains("title: \"a\\u0000b\\u001fc\\u007fd\\u0080e\\u009ff\\u2028g\\ufeffh\"\n"),
            "{out}"
        );
        assert!(out.chars().all(|c| !c.is_control() || c == '\n'), "{out:?}");
    }

    #[test]
    fn a_property_cannot_repeat_or_replace_a_key() {
        let out = front_matter(&meta(vec![
            prop("title", ExportValue::Text("mine".into())),
            prop("tags", ExportValue::Text("mine".into())),
            prop("layout", ExportValue::Text("mine".into())),
            prop("Same", ExportValue::Number(1.0)),
            prop("Same", ExportValue::Number(2.0)),
            prop("Same (2)", ExportValue::Number(3.0)),
            // The importer reads keys in lower case: the case does not make another key.
            prop("Title", ExportValue::Text("capital".into())),
            prop("SAME", ExportValue::Number(4.0)),
        ]));
        assert!(out.contains("\"title (2)\": \"mine\"\n"), "{out}");
        assert!(out.contains("\"tags (2)\": \"mine\"\n"), "{out}");
        assert!(out.contains("\"layout (2)\": \"mine\"\n"), "{out}");
        assert!(
            out.contains("\"Same\": 1\n")
                && out.contains("\"Same (2)\": 2\n")
                && out.contains("\"Same (2) (2)\": 3\n"),
            "{out}"
        );
        assert!(out.contains("\"Title (3)\": \"capital\"\n"), "{out}");
        assert!(out.contains("\"SAME (3)\": 4\n"), "{out}");
        // No key twice: the parser would refuse a repeated one.
        let yaml = parsed(&out);
        let Yaml::Hash(map) = &yaml else {
            panic!("not a map")
        };
        assert_eq!(map.len(), 1 + 1 + 2 + 8);
        assert_eq!(yaml["title"].as_str(), Some("Weekly review"));
    }

    #[test]
    fn a_template_says_so_and_lists_its_placeholders_by_property_name() {
        let mut page = meta(Vec::new());
        page.template = true;
        page.template_placeholders = vec![
            ("Due".to_owned(), kb_types::TemplatePlaceholder::Today),
            ("Who \"me\"".to_owned(), kb_types::TemplatePlaceholder::Me),
            // The same name in another case repeats a key: only the first is written.
            ("due".to_owned(), kb_types::TemplatePlaceholder::Now),
        ];
        let out = front_matter(&page);
        assert!(
            out.contains(
                "icon: \"📝\"\ntemplate: true\ntemplatePlaceholders:\n  \"Due\": \"today\"\n  \"Who \\\"me\\\"\": \"me\"\ncreated:"
            ),
            "{out}"
        );
        let yaml = parsed(&out);
        assert_eq!(yaml["template"].as_bool(), Some(true));
        assert_eq!(yaml["templatePlaceholders"]["Due"].as_str(), Some("today"));
        assert_eq!(
            yaml["templatePlaceholders"]["Who \"me\""].as_str(),
            Some("me")
        );
        assert!(yaml["templatePlaceholders"]["due"].is_badvalue());
    }

    #[test]
    fn a_page_in_a_template_lists_placeholders_without_the_flag_and_other_pages_neither() {
        // A sub-page of a template holds placeholders but is not flagged.
        let mut inside = meta(Vec::new());
        inside.template_placeholders =
            vec![("Owner".to_owned(), kb_types::TemplatePlaceholder::Me)];
        let out = front_matter(&inside);
        assert!(!out.contains("template: true"), "{out}");
        assert!(
            out.contains("templatePlaceholders:\n  \"Owner\": \"me\"\n"),
            "{out}"
        );
        // A page that is neither writes no key.
        let plain = front_matter(&meta(Vec::new()));
        assert!(!plain.contains("template"), "{plain}");
        // A flagged page without placeholders writes the flag alone.
        let mut flagged = meta(Vec::new());
        flagged.template = true;
        let out = front_matter(&flagged);
        assert!(out.contains("template: true\n"), "{out}");
        assert!(!out.contains("templatePlaceholders"), "{out}");
    }

    #[test]
    fn a_property_named_like_a_template_key_is_written_under_another_name() {
        let mut page = meta(vec![
            prop("Template", ExportValue::Checkbox(true)),
            prop("templatePlaceholders", ExportValue::Text("x".to_owned())),
        ]);
        page.template = true;
        page.template_placeholders =
            vec![("Template".to_owned(), kb_types::TemplatePlaceholder::Now)];
        let out = front_matter(&page);
        assert!(out.contains("\"Template (2)\": true\n"), "{out}");
        assert!(
            out.contains("\"templatePlaceholders (2)\": \"x\"\n"),
            "{out}"
        );
        let yaml = parsed(&out);
        assert_eq!(yaml["template"].as_bool(), Some(true));
        assert_eq!(
            yaml["templatePlaceholders"]["Template"].as_str(),
            Some("now")
        );
    }

    #[test]
    fn a_title_cannot_close_the_front_matter() {
        let mut page = meta(vec![]);
        page.title = "x\n---\ninjected: true".to_owned();
        let out = page_markdown(&page, &json!({ "type": "doc" }), &TestLinks::default());
        assert_eq!(
            out.matches("\n---\n").count() + usize::from(out.starts_with("---\n")),
            2,
            "{out}"
        );
        let yaml = parsed(&out);
        assert!(yaml["injected"].is_badvalue());
        assert_eq!(yaml["title"].as_str(), Some("x\n---\ninjected: true"));
    }
}
