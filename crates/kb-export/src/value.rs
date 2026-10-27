//! Page metadata and property values as the exporters write them, resolved by the core (option
//! ids to names, related pages to titles and links, attachments to names and links).

use kb_types::{PageId, TemplatePlaceholder};

/// What the exporters write about a page besides its content.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageMeta {
    /// The page.
    pub page_id: Option<PageId>,
    /// Title as stored (may be empty).
    pub title: String,
    /// Icon as stored (`emoji`, `icon:<name>[:<color>]`); an uploaded icon is given as the
    /// relative link to its exported file, or left out.
    pub icon: Option<String>,
    /// Cover as stored (ADR-0040): `color:<name>` or `gradient:<name>`; an uploaded cover
    /// (`attachment:<id>`) is given as the relative link to its exported file, or left out.
    pub cover: Option<String>,
    /// The cover's vertical position in percent when it is not the default 50 (only with a cover).
    pub cover_y: Option<u8>,
    /// The page is a template (ADR-0056): the front matter says `template: true`.
    pub template: bool,
    /// The placeholders of the page, by the name of the property, in schema order (ADR-0056): the front
    /// matter lists them under `templatePlaceholders`. Only the placeholders of a property that exists
    /// and takes them, of a page in a template.
    pub template_placeholders: Vec<(String, TemplatePlaceholder)>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Last edit, Unix ms.
    pub updated_at: i64,
    /// Property values in schema order (Tags first), only those with a value, computed kinds
    /// (created/edited time and by, rollups) left out.
    pub properties: Vec<ExportProperty>,
}

/// One property value of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportProperty {
    /// The property's name.
    pub name: String,
    /// This is the built-in Tags property (written as `tags` in front matter).
    pub tags: bool,
    /// The value.
    pub value: ExportValue,
}

/// A property value, resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportValue {
    /// Text, URL, email or phone.
    Text(String),
    /// A number.
    Number(f64),
    /// A checkbox.
    Checkbox(bool),
    /// A floating local date (`YYYY-MM-DD`) with an optional time (`HH:mm`).
    Date {
        /// The day.
        date: String,
        /// The time of day.
        time: Option<String>,
    },
    /// Option names: one for select and status, any number for multi-select. Also the names of the
    /// people of a Person property (ADR-0049), in the order shown, which the core resolves from
    /// the account ids ("Former member" for one the space no longer knows): an account id is never
    /// exported.
    Options(Vec<String>),
    /// Related pages.
    Pages(Vec<LinkedPage>),
    /// Attachments of a `files` value.
    Files(Vec<LinkedFile>),
    /// Checklist items (done, text).
    Checklist(Vec<(bool, String)>),
    /// A place.
    Place {
        /// Name.
        name: String,
        /// Latitude.
        lat: Option<f64>,
        /// Longitude.
        lon: Option<f64>,
    },
    /// An instant, Unix ms (created and last edited time; CSV only).
    Instant(i64),
}

/// A related page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedPage {
    /// Its title.
    pub title: String,
    /// Link to its exported file, when it is part of the export.
    pub href: Option<String>,
}

/// An attachment of a `files` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedFile {
    /// Its file name.
    pub name: String,
    /// Link to its exported file, when it is part of the export.
    pub href: Option<String>,
}

impl ExportValue {
    /// The value as one line of text (CSV cells, the HTML properties table). Instants use the
    /// device's UTC offset (`local_offset_seconds`) and read `YYYY-MM-DD HH:mm`.
    ///
    /// Numbers are Rust's shortest `Display` (`3`, `3.5`), checkboxes `Yes` or `No`, dates
    /// `YYYY-MM-DD` with ` HH:mm` when there is a time, options, pages and files their names
    /// joined by `, `, checklists `[x] a; [ ] b`, places `Paris (48.8566, 2.3522)` (the name alone
    /// without coordinates).
    #[must_use]
    pub fn text(&self, local_offset_seconds: i32) -> String {
        match self {
            ExportValue::Text(text) => text.clone(),
            ExportValue::Number(number) => number.to_string(),
            ExportValue::Checkbox(true) => "Yes".to_owned(),
            ExportValue::Checkbox(false) => "No".to_owned(),
            ExportValue::Date { date, time } => match time {
                Some(time) => format!("{date} {time}"),
                None => date.clone(),
            },
            ExportValue::Options(names) => names.join(", "),
            ExportValue::Pages(pages) => join(pages.iter().map(|page| page.title.as_str())),
            ExportValue::Files(files) => join(files.iter().map(|file| file.name.as_str())),
            ExportValue::Checklist(items) => items
                .iter()
                .map(|(done, text)| format!("[{}] {text}", if *done { 'x' } else { ' ' }))
                .collect::<Vec<_>>()
                .join("; "),
            ExportValue::Place { name, lat, lon } => match (lat, lon) {
                (Some(lat), Some(lon)) if name.is_empty() => format!("({lat}, {lon})"),
                (Some(lat), Some(lon)) => format!("{name} ({lat}, {lon})"),
                _ => name.clone(),
            },
            ExportValue::Instant(ms) => crate::time::local_minute(*ms, local_offset_seconds),
        }
    }
}

/// `items` joined by `, `.
fn join<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_rusts_shortest_display() {
        for (number, text) in [
            (3.0, "3"),
            (3.5, "3.5"),
            (-2.0, "-2"),
            (0.0, "0"),
            (0.1, "0.1"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1000000000000000000000"),
            (1.5e-7, "0.00000015"),
            (48.8566, "48.8566"),
        ] {
            assert_eq!(ExportValue::Number(number).text(0), text);
        }
    }

    #[test]
    fn text_checkboxes_and_dates() {
        assert_eq!(
            ExportValue::Text("a, \"b\"\nc".into()).text(0),
            "a, \"b\"\nc"
        );
        assert_eq!(ExportValue::Checkbox(true).text(0), "Yes");
        assert_eq!(ExportValue::Checkbox(false).text(0), "No");
        let date = |time: Option<&str>| ExportValue::Date {
            date: "2026-10-07".into(),
            time: time.map(str::to_owned),
        };
        assert_eq!(date(None).text(0), "2026-10-07");
        assert_eq!(date(Some("14:30")).text(0), "2026-10-07 14:30");
    }

    #[test]
    fn lists_are_joined_by_commas_and_checklists_by_semicolons() {
        assert_eq!(ExportValue::Options(vec![]).text(0), "");
        assert_eq!(ExportValue::Options(vec!["a".into()]).text(0), "a");
        assert_eq!(
            ExportValue::Options(vec!["a".into(), "b c".into()]).text(0),
            "a, b c"
        );
        let pages = vec![
            LinkedPage {
                title: "One".into(),
                href: Some("One.md".into()),
            },
            LinkedPage {
                title: "Two".into(),
                href: None,
            },
        ];
        assert_eq!(ExportValue::Pages(pages).text(0), "One, Two");
        let files = vec![
            LinkedFile {
                name: "a.pdf".into(),
                href: Some("dir/a.pdf".into()),
            },
            LinkedFile {
                name: "b.png".into(),
                href: None,
            },
        ];
        assert_eq!(ExportValue::Files(files).text(0), "a.pdf, b.png");
        assert_eq!(ExportValue::Checklist(vec![]).text(0), "");
        assert_eq!(
            ExportValue::Checklist(vec![
                (true, "a".into()),
                (false, "b".into()),
                (true, "".into())
            ])
            .text(0),
            "[x] a; [ ] b; [x] "
        );
    }

    #[test]
    fn places_show_their_coordinates_when_they_have_both() {
        let place = |name: &str, lat: Option<f64>, lon: Option<f64>| {
            ExportValue::Place {
                name: name.into(),
                lat,
                lon,
            }
            .text(0)
        };
        assert_eq!(
            place("Paris", Some(48.8566), Some(2.3522)),
            "Paris (48.8566, 2.3522)"
        );
        assert_eq!(place("Paris", Some(-48.0), Some(0.0)), "Paris (-48, 0)");
        assert_eq!(place("Paris", None, None), "Paris");
        assert_eq!(place("Paris", Some(1.0), None), "Paris");
        assert_eq!(place("", Some(1.5), Some(2.0)), "(1.5, 2)");
        assert_eq!(place("", None, None), "");
    }

    #[test]
    fn instants_are_local_minutes_with_the_devices_offset() {
        // 2025-10-09T08:53:20Z.
        let instant = ExportValue::Instant(1_760_000_000_000);
        assert_eq!(instant.text(0), "2025-10-09 08:53");
        assert_eq!(instant.text(2 * 3600), "2025-10-09 10:53");
        assert_eq!(instant.text(-8 * 3600), "2025-10-09 00:53");
        assert_eq!(instant.text(-9 * 3600), "2025-10-08 23:53");
        assert_eq!(instant.text(5 * 3600 + 45 * 60), "2025-10-09 14:38");
    }
}
