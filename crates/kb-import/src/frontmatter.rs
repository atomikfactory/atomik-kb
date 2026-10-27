//! Front matter as page data (ADR-0036, section 3.3): the title, the icon, the cover (ADR-0040),
//! the template flag and placeholders (ADR-0056), the tags and the property values it holds.

use std::collections::HashMap;

use kb_types::properties::TAGS_NAME;
use kb_types::{ImportWarningCode, PropertyKind, TemplatePlaceholder};

use crate::ImportValue;
use crate::PlannedCover;
use crate::cover;
use crate::ctx::{Dest, PageCtx, is_image_path};
use crate::emoji;
use crate::model::{PendingTarget, RawValue};
use crate::paths::{self, truncate_chars};
use crate::yaml::Yv;

/// Keys that are not properties: the page's own data and the settings of other tools.
const IGNORED_KEYS: &[&str] = &[
    "title",
    "icon",
    "cover_y",
    "created",
    "updated",
    "aliases",
    "cssclasses",
    "cssclass",
    "publish",
    "permalink",
    "layout",
];

/// The longest property name, in characters (the schema's limit).
pub(crate) const MAX_NAME_CHARS: usize = 100;

/// What front matter gives a page.
#[derive(Debug, Default)]
pub(crate) struct Front {
    /// `title`.
    pub title: Option<String>,
    /// `icon`.
    pub icon: Option<String>,
    /// `cover`: a named cover, or the relative path of a picture of the source. A `cover` that is
    /// neither (a URL, a wiki link, some other text) stays an ordinary property.
    pub cover: Option<PlannedCover>,
    /// `cover_y`: the vertical position of the cover in percent.
    pub cover_y: Option<u8>,
    /// `template: true` (ADR-0056): the page is a template. Any other value of a key `template` is an
    /// ordinary property.
    pub template: bool,
    /// `templatePlaceholders` (ADR-0056): a mapping from a property name to `today`, `now` or `me`, in
    /// document order; an entry whose placeholder is not one of the three is left out. A key of that
    /// name that is not a mapping is an ordinary property.
    pub template_placeholders: Vec<(String, TemplatePlaceholder)>,
    /// The properties, in document order; the tags first as `Tags`.
    pub props: Vec<(String, RawValue)>,
}

/// Reads the pairs of a front matter mapping. `hints` are the kinds the sidecar of an atomik-kb
/// export gives to property names (lowercase), which decide how a value is read where the YAML
/// alone is ambiguous.
pub(crate) fn extract(
    pairs: Vec<(String, Yv)>,
    ctx: &mut PageCtx<'_>,
    hints: &HashMap<String, PropertyKind>,
) -> Front {
    let mut front = Front::default();
    let mut tags: Vec<String> = Vec::new();
    for (key, value) in pairs {
        let name = key.trim();
        if name.is_empty() {
            continue;
        }
        let lower = name.to_lowercase();
        if lower == "cover"
            && front.cover.is_none()
            && let Some(cover) = cover_of(&value, ctx)
        {
            front.cover = Some(cover);
            continue;
        }
        if lower == "template" && is_true(&value) {
            front.template = true;
            continue;
        }
        if lower == "templateplaceholders"
            && let Yv::Map(entries) = &value
        {
            front.template_placeholders = placeholders_of(entries);
            continue;
        }
        match lower.as_str() {
            "title" => front.title = value.scalar_text().map(|t| t.trim().to_owned()),
            "icon" => front.icon = icon_of(&value, ctx),
            "cover_y" => front.cover_y = value.scalar_text().and_then(|t| cover::position(&t)),
            "tags" => tags.extend(tag_names(&value)),
            other if IGNORED_KEYS.contains(&other) => {}
            _ => {
                let hint = hints.get(&lower).copied();
                if let Some(raw) = convert(&value, hint, ctx) {
                    let name = truncate_chars(name, MAX_NAME_CHARS);
                    front.props.push((name, raw));
                }
            }
        }
    }
    if !tags.is_empty() {
        front.props.insert(
            0,
            (
                TAGS_NAME.to_owned(),
                RawValue::Plain(ImportValue::List(tags)),
            ),
        );
    }
    front.title = front.title.filter(|t| !t.is_empty());
    front
}

/// Whether a value says yes to a flag: the YAML boolean `true` (what an export writes), or the text `true`.
fn is_true(value: &Yv) -> bool {
    match value {
        Yv::Bool(on) => *on,
        Yv::Str(text) => text.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// The placeholders of a `templatePlaceholders` mapping: a property name (trimmed, at most 100
/// characters, no repeat in any case) to `today`, `now` or `me` (any case). Entries that are not a
/// name with one of the three are left out.
fn placeholders_of(entries: &[(String, Yv)]) -> Vec<(String, TemplatePlaceholder)> {
    let mut out: Vec<(String, TemplatePlaceholder)> = Vec::new();
    for (name, value) in entries {
        let name = truncate_chars(name.trim(), MAX_NAME_CHARS);
        let Some(placeholder) = value
            .scalar_text()
            .and_then(|text| TemplatePlaceholder::parse(&text.trim().to_lowercase()))
        else {
            continue;
        };
        if name.is_empty()
            || out
                .iter()
                .any(|(known, _)| known.to_lowercase() == name.to_lowercase())
        {
            continue;
        }
        out.push((name, placeholder));
    }
    out
}

/// The icon of a page: an emoji or `icon:<name>`; anything else (a file path) is not imported.
fn icon_of(value: &Yv, ctx: &mut PageCtx<'_>) -> Option<String> {
    let text = value.scalar_text()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.len() <= 32 && (text.starts_with("icon:") || emoji::is_icon_word(text)) {
        return Some(text.to_owned());
    }
    ctx.warn(ImportWarningCode::UnsupportedContent, Some("icon"));
    None
}

/// The cover of a page (ADR-0040): `color:<palette name>` or `gradient:<name>` as atomik-kb writes
/// them, or the relative path of a picture of the source (an uploaded cover; the file is recorded
/// as needed). `None` for anything else, which leaves the key to be read as a property.
fn cover_of(value: &Yv, ctx: &mut PageCtx<'_>) -> Option<PlannedCover> {
    let text = value.scalar_text()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(named) = cover::named(text) {
        return Some(PlannedCover::Named(named));
    }
    match ctx.classify(text) {
        Dest::Binary { orig, .. } if is_image_path(&orig) => {
            ctx.need(&orig);
            Some(PlannedCover::File(orig))
        }
        _ => None,
    }
}

/// The names of `tags`: a list, or a string split at commas and white space; a leading `#` is
/// removed; no repeats.
fn tag_names(value: &Yv) -> Vec<String> {
    let raw: Vec<String> = match value {
        Yv::Seq(items) => items.iter().filter_map(Yv::scalar_text).collect(),
        Yv::Null | Yv::Map(_) => Vec::new(),
        scalar => scalar
            .scalar_text()
            .map(|text| {
                text.split(|c: char| c == ',' || c.is_whitespace())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut out: Vec<String> = Vec::new();
    for tag in raw {
        let tag = tag.trim().trim_start_matches('#').trim();
        if !tag.is_empty() && !out.iter().any(|seen| seen == tag) {
            out.push(truncate_chars(tag, MAX_NAME_CHARS));
        }
    }
    out
}

/// The wiki target of a `[[Title]]` or `[[Title|alias]]` string, when the whole string is one.
pub(crate) fn wiki_title(text: &str) -> Option<String> {
    let inner = text.trim().strip_prefix("[[")?.strip_suffix("]]")?;
    if inner.contains("[[") || inner.contains("]]") {
        return None;
    }
    let title = inner.split('|').next().unwrap_or_default().trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// `[x] text` or `[ ] text`.
fn checklist_item(text: &str) -> Option<(bool, String)> {
    let rest = text.trim_start().strip_prefix('[')?;
    let (mark, rest) = rest.split_once(']')?;
    let done = match mark {
        "x" | "X" => true,
        " " | "" => false,
        _ => return None,
    };
    Some((done, rest.trim().to_owned()))
}

/// A value of the property kinds that are plain text.
fn is_text_like(kind: PropertyKind) -> bool {
    matches!(
        kind,
        PropertyKind::Text
            | PropertyKind::Url
            | PropertyKind::Email
            | PropertyKind::Phone
            | PropertyKind::Select
            | PropertyKind::Status
    )
}

/// One front matter value as an import value.
fn convert(value: &Yv, hint: Option<PropertyKind>, ctx: &mut PageCtx<'_>) -> Option<RawValue> {
    match value {
        Yv::Null => None,
        Yv::Bool(b) => Some(match hint {
            Some(kind) if is_text_like(kind) => text_value(&b.to_string())?,
            _ => RawValue::Plain(ImportValue::Bool(*b)),
        }),
        Yv::Number(n) => Some(match hint {
            Some(kind) if is_text_like(kind) => text_value(&crate::values::number_text(*n))?,
            _ => RawValue::Plain(ImportValue::Number(*n)),
        }),
        Yv::Str(text) => string_value(text, hint, ctx),
        Yv::Seq(items) => list_value(value, items, hint, ctx),
        Yv::Map(pairs) => Some(place_or_text(value, pairs)),
    }
}

fn text_value(text: &str) -> Option<RawValue> {
    let text = text.trim();
    (!text.is_empty()).then(|| RawValue::Plain(ImportValue::Text(text.to_owned())))
}

fn string_value(text: &str, hint: Option<PropertyKind>, ctx: &mut PageCtx<'_>) -> Option<RawValue> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match hint {
        Some(PropertyKind::Files) => {
            return files_value(&[text.to_owned()], ctx);
        }
        Some(PropertyKind::Relation) => {
            return Some(RawValue::Pages(vec![PendingTarget::Wiki(
                wiki_title(text).unwrap_or_else(|| text.to_owned()),
            )]));
        }
        Some(PropertyKind::MultiSelect) => {
            return Some(RawValue::Plain(ImportValue::List(vec![text.to_owned()])));
        }
        Some(kind) if is_text_like(kind) => return text_value(text),
        _ => {}
    }
    if let Some(title) = wiki_title(text) {
        return Some(RawValue::Pages(vec![PendingTarget::Wiki(title)]));
    }
    if hint.is_none_or(|kind| kind == PropertyKind::Date)
        && let Some((date, time)) = crate::values::parse_date(text)
    {
        return Some(RawValue::Plain(ImportValue::Date { date, time }));
    }
    text_value(text)
}

fn list_value(
    whole: &Yv,
    items: &[Yv],
    hint: Option<PropertyKind>,
    ctx: &mut PageCtx<'_>,
) -> Option<RawValue> {
    if items
        .iter()
        .any(|item| matches!(item, Yv::Seq(_) | Yv::Map(_)))
    {
        return text_value(&whole.to_text());
    }
    let texts: Vec<String> = items
        .iter()
        .filter_map(Yv::scalar_text)
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .collect();
    if texts.is_empty() {
        return None;
    }
    match hint {
        Some(PropertyKind::Files) => return files_value(&texts, ctx),
        Some(PropertyKind::Relation) => {
            return Some(RawValue::Pages(
                texts
                    .iter()
                    .map(|text| {
                        PendingTarget::Wiki(wiki_title(text).unwrap_or_else(|| text.clone()))
                    })
                    .collect(),
            ));
        }
        Some(PropertyKind::Checklist) => {
            let checklist = checklist(&texts);
            return Some(RawValue::Plain(ImportValue::Checklist(checklist)));
        }
        Some(kind) if kind != PropertyKind::MultiSelect && is_text_like(kind) => {
            return text_value(&texts.join(", "));
        }
        Some(_) => return Some(RawValue::Plain(ImportValue::List(options(texts)))),
        None => {}
    }
    let wikis: Vec<String> = texts.iter().filter_map(|text| wiki_title(text)).collect();
    if wikis.len() == texts.len() {
        return Some(RawValue::Pages(
            wikis.into_iter().map(PendingTarget::Wiki).collect(),
        ));
    }
    if texts.iter().all(|text| checklist_item(text).is_some()) {
        return Some(RawValue::Plain(ImportValue::Checklist(checklist(&texts))));
    }
    if texts.iter().any(|text| resolve_file(text, ctx).is_some()) {
        return files_value(&texts, ctx);
    }
    Some(RawValue::Plain(ImportValue::List(options(texts))))
}

/// Option names as the schema keeps them: at most 100 characters.
fn options(texts: Vec<String>) -> Vec<String> {
    texts
        .into_iter()
        .map(|text| truncate_chars(&text, MAX_NAME_CHARS))
        .collect()
}

fn checklist(texts: &[String]) -> Vec<(bool, String)> {
    texts
        .iter()
        .map(|text| checklist_item(text).unwrap_or((false, text.clone())))
        .collect()
}

/// The original path of a binary file of the source named by a front matter path (relative to the
/// page's folder, not percent-encoded).
fn resolve_file(text: &str, ctx: &PageCtx<'_>) -> Option<String> {
    let norm = paths::resolve_plain(ctx.dir, text)?;
    let found = ctx.index.find_binary(&norm)?;
    ctx.index.binary.get(found).cloned()
}

/// A `files` value: the paths that name binary files of the source; the others are reported.
fn files_value(texts: &[String], ctx: &mut PageCtx<'_>) -> Option<RawValue> {
    let mut files = Vec::new();
    for text in texts {
        match resolve_file(text, ctx) {
            Some(orig) => {
                ctx.need(&orig);
                if !files.contains(&orig) {
                    files.push(orig);
                }
            }
            None => ctx.warn(ImportWarningCode::MissingFile, Some(text)),
        }
    }
    (!files.is_empty()).then_some(RawValue::Plain(ImportValue::Files(files)))
}

/// A map with a `name` and optional `lat` and `lon` is a place; any other map is its text.
fn place_or_text(whole: &Yv, pairs: &[(String, Yv)]) -> RawValue {
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    };
    let number = |value: Option<&Yv>| match value? {
        Yv::Number(n) => Some(*n),
        Yv::Str(text) => crate::values::parse_number(text),
        _ => None,
    };
    if let Some(name) = get("name").and_then(Yv::scalar_text)
        && !name.trim().is_empty()
        && pairs.iter().all(|(k, _)| {
            matches!(
                k.to_ascii_lowercase().as_str(),
                "name" | "lat" | "lon" | "lng" | "latitude" | "longitude"
            )
        })
    {
        let lat = number(get("lat").or_else(|| get("latitude")));
        let lon = number(
            get("lon")
                .or_else(|| get("lng"))
                .or_else(|| get("longitude")),
        );
        let (lat, lon) = match (lat, lon) {
            (Some(lat), Some(lon)) => (Some(lat), Some(lon)),
            _ => (None, None),
        };
        return RawValue::Plain(ImportValue::Place {
            name: name.trim().to_owned(),
            lat,
            lon,
        });
    }
    RawValue::Plain(ImportValue::Text(whole.to_text()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ctx::{SourceIndex, Warnings};
    use crate::yaml;

    fn read(
        yaml_text: &str,
        hints: &[(&str, PropertyKind)],
    ) -> (Front, Vec<kb_types::ImportWarning>) {
        let mut index = SourceIndex::default();
        index.binary.insert(
            "Notes/Weekly review/report.pdf".into(),
            "Export/Notes/Weekly review/report.pdf".into(),
        );
        index
            .binary
            .insert("Notes/banner.png".into(), "Export/Notes/banner.png".into());
        index
            .binary
            .insert("Shared/wide.jpg".into(), "Export/Shared/wide.jpg".into());
        let mut warnings = Warnings::default();
        let hints: HashMap<String, PropertyKind> = hints
            .iter()
            .map(|(name, kind)| ((*name).to_lowercase(), *kind))
            .collect();
        let mut ctx = PageCtx::new(
            &index,
            &mut warnings,
            "Export/Notes/Weekly review.md",
            "Notes",
            0,
        );
        let pairs = yaml::parse(yaml_text).expect("valid YAML");
        let front = extract(pairs, &mut ctx, &hints);
        (front, warnings.into_vec())
    }

    fn props(front: &Front) -> HashMap<&str, &RawValue> {
        front
            .props
            .iter()
            .map(|(name, value)| (name.as_str(), value))
            .collect()
    }

    #[test]
    fn page_data_and_ignored_keys() {
        let (front, warnings) = read(
            "title: \"Weekly review\"\nicon: \"📝\"\ncreated: \"2026-10-01T09:30:00Z\"\nupdated: x\naliases: [a]\nlayout: wide\ntags:\n  - work\n  - '#review'\n  - work\n",
            &[],
        );
        assert_eq!(front.title.as_deref(), Some("Weekly review"));
        assert_eq!(front.icon.as_deref(), Some("📝"));
        assert!(warnings.is_empty());
        assert_eq!(front.props.len(), 1);
        assert_eq!(front.props[0].0, "Tags");
        assert_eq!(
            front.props[0].1,
            RawValue::Plain(ImportValue::List(vec!["work".into(), "review".into()]))
        );
    }

    #[test]
    fn a_template_flag_and_its_placeholders_are_page_data_not_properties() {
        let (front, warnings) = read(
            "title: \"Standup\"\ntemplate: true\ntemplatePlaceholders:\n  \"Due\": \"today\"\n  \"Owner\": \"me\"\n  \"Seen\": \"Now\"\n",
            &[],
        );
        assert!(front.template);
        assert_eq!(
            front.template_placeholders,
            vec![
                ("Due".to_owned(), TemplatePlaceholder::Today),
                ("Owner".to_owned(), TemplatePlaceholder::Me),
                ("Seen".to_owned(), TemplatePlaceholder::Now),
            ]
        );
        assert!(front.props.is_empty(), "neither key is a property");
        assert!(warnings.is_empty());

        // A name that repeats (in another case), an unknown placeholder and an empty name are left out.
        let (front, _) = read(
            "templateplaceholders:\n  Due: today\n  due: now\n  Who: everyone\n  \" \": me\n",
            &[],
        );
        assert!(!front.template);
        assert_eq!(
            front.template_placeholders,
            vec![("Due".to_owned(), TemplatePlaceholder::Today)]
        );

        // The text `true` says yes too; any other value of `template` stays an ordinary property.
        let (front, _) = read("Template: \"True\"\n", &[]);
        assert!(front.template);
        for text in ["template: false", "template: daily", "template: [a]"] {
            let (front, _) = read(&format!("{text}\n"), &[]);
            assert!(!front.template, "{text}");
            assert_eq!(front.props.len(), 1, "{text} is a property");
            assert_eq!(front.props[0].0, "template");
        }
        // `templatePlaceholders` that is not a mapping is a property too.
        let (front, _) = read("templatePlaceholders: today\n", &[]);
        assert!(front.template_placeholders.is_empty());
        assert_eq!(front.props.len(), 1);
    }

    #[test]
    fn tags_as_a_string_and_icons_that_are_not_emoji() {
        let (front, warnings) = read("tags: \"a, #b c\"\nicon: images/me.png\n", &[]);
        assert_eq!(
            front.props[0].1,
            RawValue::Plain(ImportValue::List(vec!["a".into(), "b".into(), "c".into()]))
        );
        assert_eq!(front.icon, None);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::UnsupportedContent);
        assert_eq!(warnings[0].detail.as_deref(), Some("icon"));
        let (front, _) = read("icon: \"icon:book:blue\"\n", &[]);
        assert_eq!(front.icon.as_deref(), Some("icon:book:blue"));
    }

    #[test]
    fn a_cover_is_a_named_one_or_a_picture_of_the_source() {
        let (front, warnings) = read(
            "cover: \"gradient:sunset\"
cover_y: 30
",
            &[],
        );
        assert_eq!(
            front.cover,
            Some(PlannedCover::Named("gradient:sunset".into()))
        );
        assert_eq!(front.cover_y, Some(30));
        assert!(front.props.is_empty(), "a cover is not a property");
        assert!(warnings.is_empty());

        let (front, _) = read(
            "Cover: color:blue
",
            &[],
        );
        assert_eq!(front.cover, Some(PlannedCover::Named("color:blue".into())));
        assert_eq!(front.cover_y, None);

        // A picture of the source, by a path relative to the page (as the export writes it), is a
        // needed file; the position is read next to it.
        let (front, warnings) = read(
            "cover: banner.png
cover_y: 100
",
            &[],
        );
        assert_eq!(
            front.cover,
            Some(PlannedCover::File("Export/Notes/banner.png".into()))
        );
        assert_eq!(front.cover_y, Some(100));
        assert!(front.props.is_empty());
        assert!(warnings.is_empty());
        let (front, _) = read(
            "cover: ../Shared/wide.jpg
",
            &[],
        );
        assert_eq!(
            front.cover,
            Some(PlannedCover::File("Export/Shared/wide.jpg".into()))
        );
    }

    #[test]
    fn a_cover_that_is_anything_else_stays_a_property_and_a_bad_position_is_dropped() {
        for text in [
            "https://example.com/banner.png",
            "\"![[banner.png]]\"",
            "missing.png",
            "color:teal",
            "gradient:Sunset",
            "weekly review",
            "Weekly review/report.pdf",
        ] {
            let (front, _) = read(
                &format!(
                    "cover: {text}
"
                ),
                &[],
            );
            assert_eq!(front.cover, None, "{text}");
            assert_eq!(front.props.len(), 1, "{text}: it is read as a property");
            assert_eq!(front.props[0].0, "cover", "{text}");
        }
        for text in ["high", "101", "-4", "30.5", "\"\""] {
            let (front, _) = read(
                &format!(
                    "cover: color:red
cover_y: {text}
"
                ),
                &[],
            );
            assert_eq!(front.cover, Some(PlannedCover::Named("color:red".into())));
            assert_eq!(front.cover_y, None, "{text}");
            assert_eq!(front.props.len(), 0, "{text}: the key is ignored");
        }
    }

    #[test]
    fn values_by_shape() {
        let (front, _) = read(
            "Status: Done\nDue: \"2026-10-07 14:30\"\nDone: true\nEstimate: 3.5\nRelated:\n  - \"[[Other page]]\"\nOne: \"[[Solo|alias]]\"\nChecklist:\n  - \"[x] Buy milk\"\n  - \"[ ] Call Ann\"\nPlace:\n  name: Paris\n  lat: 48.8566\n  lon: 2.3522\nLabels:\n  - a\n  - 2\nNote: plain words\nBlank:\nObj:\n  x: 1\n",
            &[],
        );
        let p = props(&front);
        assert_eq!(
            p["Status"],
            &RawValue::Plain(ImportValue::Text("Done".into()))
        );
        assert_eq!(
            p["Due"],
            &RawValue::Plain(ImportValue::Date {
                date: "2026-10-07".into(),
                time: Some("14:30".into())
            })
        );
        assert_eq!(p["Done"], &RawValue::Plain(ImportValue::Bool(true)));
        assert_eq!(p["Estimate"], &RawValue::Plain(ImportValue::Number(3.5)));
        assert_eq!(
            p["Related"],
            &RawValue::Pages(vec![PendingTarget::Wiki("Other page".into())])
        );
        assert_eq!(
            p["One"],
            &RawValue::Pages(vec![PendingTarget::Wiki("Solo".into())])
        );
        assert_eq!(
            p["Checklist"],
            &RawValue::Plain(ImportValue::Checklist(vec![
                (true, "Buy milk".into()),
                (false, "Call Ann".into())
            ]))
        );
        assert_eq!(
            p["Place"],
            &RawValue::Plain(ImportValue::Place {
                name: "Paris".into(),
                lat: Some(48.8566),
                lon: Some(2.3522)
            })
        );
        assert_eq!(
            p["Labels"],
            &RawValue::Plain(ImportValue::List(vec!["a".into(), "2".into()]))
        );
        assert_eq!(
            p["Note"],
            &RawValue::Plain(ImportValue::Text("plain words".into()))
        );
        assert!(!p.contains_key("Blank"));
        assert_eq!(p["Obj"], &RawValue::Plain(ImportValue::Text("x: 1".into())));
    }

    #[test]
    fn files_are_paths_to_binaries_of_the_source() {
        let (front, warnings) = read(
            "Attachments:\n  - \"Weekly review/report.pdf\"\n  - gone.pdf\n",
            &[],
        );
        assert_eq!(
            props(&front)["Attachments"],
            &RawValue::Plain(ImportValue::Files(vec![
                "Export/Notes/Weekly review/report.pdf".into()
            ]))
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::MissingFile);
    }

    #[test]
    fn the_sidecar_decides_where_yaml_is_ambiguous() {
        let (front, warnings) = read(
            "Code: 12\nWhen: \"2026-10-07\"\nPick: Done\nFiles: report.pdf\nLinks: [A, B]\nTodo:\n  - Milk\n",
            &[
                ("Code", PropertyKind::Text),
                ("When", PropertyKind::Text),
                ("Pick", PropertyKind::MultiSelect),
                ("Files", PropertyKind::Files),
                ("Links", PropertyKind::Relation),
                ("Todo", PropertyKind::Checklist),
            ],
        );
        let p = props(&front);
        assert_eq!(p["Code"], &RawValue::Plain(ImportValue::Text("12".into())));
        assert_eq!(
            p["When"],
            &RawValue::Plain(ImportValue::Text("2026-10-07".into()))
        );
        assert_eq!(
            p["Pick"],
            &RawValue::Plain(ImportValue::List(vec!["Done".into()]))
        );
        assert!(!p.contains_key("Files"));
        assert_eq!(
            p["Links"],
            &RawValue::Pages(vec![
                PendingTarget::Wiki("A".into()),
                PendingTarget::Wiki("B".into())
            ])
        );
        assert_eq!(
            p["Todo"],
            &RawValue::Plain(ImportValue::Checklist(vec![(false, "Milk".into())]))
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::MissingFile);
    }

    #[test]
    fn long_names_are_cut() {
        let long = "n".repeat(300);
        let (front, _) = read(&format!("{long}: 1\n"), &[]);
        assert_eq!(front.props[0].0.chars().count(), MAX_NAME_CHARS);
    }
}
