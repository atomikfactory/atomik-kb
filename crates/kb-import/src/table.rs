//! CSV files as databases (ADR-0036, section 3.6): the table, the kind of each column, and the
//! values of a row.

use std::collections::{HashMap, HashSet};

use kb_types::{ImportWarningCode, PropertyKind};

use crate::ImportValue;
use crate::ctx::SourceIndex;
use crate::frontmatter::MAX_NAME_CHARS;
use crate::model::{PendingTarget, RawValue};
use crate::paths::truncate_chars;
use crate::values::{parse_bool, parse_date, parse_number};

/// Most columns kept.
const MAX_COLUMNS: usize = 200;
/// Most distinct options a select or multi-select column may have.
const MAX_OPTIONS: usize = 50;
/// The longest option, in characters.
const MAX_OPTION_CHARS: usize = 60;
/// The largest number of rows with which a column of free text still reads as a select.
const SELECT_ROWS: usize = 3;

/// What a column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColKind {
    /// Every cell is empty.
    Empty,
    /// The page's title.
    Title,
    /// A checkbox.
    Checkbox,
    /// A number.
    Number,
    /// A date.
    Date,
    /// A URL.
    Url,
    /// An email address.
    Email,
    /// Notion's `Title (path.md)` items.
    Relation,
    /// Several names in a cell.
    MultiSelect,
    /// One name from a short list.
    Select,
    /// Free text.
    Text,
}

impl ColKind {
    /// The property kind, `None` for a column without values.
    pub(crate) fn property_kind(self) -> Option<PropertyKind> {
        Some(match self {
            ColKind::Empty | ColKind::Title => return None,
            ColKind::Checkbox => PropertyKind::Checkbox,
            ColKind::Number => PropertyKind::Number,
            ColKind::Date => PropertyKind::Date,
            ColKind::Url => PropertyKind::Url,
            ColKind::Email => PropertyKind::Email,
            ColKind::Relation => PropertyKind::Relation,
            ColKind::MultiSelect => PropertyKind::MultiSelect,
            ColKind::Select => PropertyKind::Select,
            ColKind::Text => PropertyKind::Text,
        })
    }
}

/// A column.
#[derive(Debug, Clone)]
pub(crate) struct Column {
    /// Its name (made unique and non-empty).
    pub name: String,
    /// What it holds.
    pub kind: ColKind,
    /// The names of a select or multi-select column, in order of appearance.
    pub options: Vec<String>,
}

/// A CSV file read.
#[derive(Debug, Default)]
pub(crate) struct Table {
    /// The columns, the title column included.
    pub columns: Vec<Column>,
    /// The index of the title column.
    pub title_col: usize,
    /// The rows, each as wide as `columns`.
    pub rows: Vec<Vec<String>>,
    /// Columns or rows beyond the limits were left out.
    pub columns_cut: bool,
    /// Rows beyond the limit were left out.
    pub rows_cut: bool,
}

impl Table {
    /// The title of a row.
    pub(crate) fn title(&self, row: usize) -> String {
        let title = self
            .rows
            .get(row)
            .and_then(|r| r.get(self.title_col))
            .map_or("", |t| t.trim());
        if title.is_empty() {
            "Untitled".to_owned()
        } else {
            title.to_owned()
        }
    }

    /// The names of the columns that are properties, in order.
    pub(crate) fn property_names(&self) -> impl Iterator<Item = &str> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.title_col)
            .map(|(_, column)| column.name.as_str())
    }
}

/// Reads CSV text: the delimiter is `,` or `;` (whichever the header line has more of), a header
/// row is required, rows are padded or cut to the header's width, at most `max_rows` rows are
/// kept.
pub(crate) fn parse(text: &str, max_rows: usize) -> Table {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_line = text.lines().next().unwrap_or_default();
    let commas = first_line.matches(',').count();
    let semicolons = first_line.matches(';').count();
    let delimiter = if semicolons > commas { b';' } else { b',' };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let mut table = Table::default();
    let mut records = reader.records();
    let Some(Ok(header)) = records.next() else {
        return table;
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut names = Vec::new();
    for (index, raw) in header.iter().enumerate() {
        if index >= MAX_COLUMNS {
            table.columns_cut = true;
            break;
        }
        let mut name = truncate_chars(raw.trim(), MAX_NAME_CHARS - 6);
        if name.is_empty() {
            name = format!("Column {}", index + 1);
        }
        let mut candidate = name.clone();
        let mut n = 2;
        while !seen.insert(candidate.to_lowercase()) {
            candidate = format!("{name} ({n})");
            n += 1;
        }
        names.push(candidate);
    }
    let width = names.len();
    table.title_col = names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("name") || n.eq_ignore_ascii_case("title"))
        .unwrap_or(0);
    table.columns = names
        .into_iter()
        .map(|name| Column {
            name,
            kind: ColKind::Empty,
            options: Vec::new(),
        })
        .collect();
    for record in records {
        let Ok(record) = record else {
            break;
        };
        if table.rows.len() >= max_rows {
            table.rows_cut = true;
            break;
        }
        let mut row: Vec<String> = record.iter().take(width).map(clean_cell).collect();
        row.resize(width, String::new());
        table.rows.push(row);
    }
    table
}

/// A cell without the guard `'` that the export puts before `=`, `+`, `-`, `@`, TAB or CR.
fn clean_cell(cell: &str) -> String {
    let cell = cell.trim();
    match cell.strip_prefix('\'') {
        Some(rest) if rest.starts_with(['=', '+', '-', '@', '\t', '\r']) => rest.to_owned(),
        _ => cell.to_owned(),
    }
}

/// Decides the kind of every column. `resolve` answers the arena index of the page a relation
/// item's path names (a path relative to the CSV's folder, a Notion URL).
pub(crate) fn infer(table: &mut Table, resolve: &dyn Fn(&str) -> Option<usize>) {
    for index in 0..table.columns.len() {
        if index == table.title_col {
            table.columns[index].kind = ColKind::Title;
            continue;
        }
        let cells: Vec<&str> = table
            .rows
            .iter()
            .filter_map(|row| row.get(index))
            .map(String::as_str)
            .filter(|cell| !cell.is_empty())
            .collect();
        let (kind, options) = infer_column(&cells, table.rows.len(), resolve);
        table.columns[index].kind = kind;
        table.columns[index].options = options;
    }
}

fn infer_column(
    cells: &[&str],
    rows: usize,
    resolve: &dyn Fn(&str) -> Option<usize>,
) -> (ColKind, Vec<String>) {
    if cells.is_empty() {
        return (ColKind::Empty, Vec::new());
    }
    if cells.iter().all(|c| parse_bool(c).is_some()) {
        return (ColKind::Checkbox, Vec::new());
    }
    if cells.iter().all(|c| parse_number(c).is_some()) {
        return (ColKind::Number, Vec::new());
    }
    if cells.iter().all(|c| parse_date(c).is_some()) {
        return (ColKind::Date, Vec::new());
    }
    if cells.iter().all(|c| is_url(c)) {
        return (ColKind::Url, Vec::new());
    }
    if cells.iter().all(|c| is_email(c)) {
        return (ColKind::Email, Vec::new());
    }
    if is_relation(cells, resolve) {
        return (ColKind::Relation, Vec::new());
    }
    if let Some(options) = multi_select_options(cells) {
        return (ColKind::MultiSelect, options);
    }
    if let Some(options) = select_options(cells, rows) {
        return (ColKind::Select, options);
    }
    (ColKind::Text, Vec::new())
}

fn is_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:"))
        && text.len() <= 2_048
        && !text.chars().any(char::is_whitespace)
        && text.len() > 8
}

fn is_email(text: &str) -> bool {
    if text.len() > 254 || text.chars().any(|c| c.is_whitespace() || c == ',') {
        return false;
    }
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

/// The items of a Notion relation cell: `Title (path.md), Title 2 (path%202.md)` as (title, path).
pub(crate) fn relation_items(cell: &str) -> Option<Vec<(String, String)>> {
    let mut items = Vec::new();
    let mut rest = cell.trim();
    while !rest.is_empty() {
        // The item ends at the first `)` followed by a comma or the end.
        let end = rest.char_indices().find_map(|(index, c)| {
            (c == ')' && {
                let after = &rest[index + 1..];
                after.is_empty() || after.starts_with(',')
            })
            .then_some(index)
        })?;
        let item = &rest[..end];
        let open = item.rfind(" (")?;
        let path = item[open + 2..].trim();
        if path.is_empty() || path.chars().any(char::is_whitespace) {
            return None;
        }
        items.push((item[..open].trim().to_owned(), path.to_owned()));
        rest = rest[end + 1..].trim_start_matches(',').trim_start();
    }
    (!items.is_empty()).then_some(items)
}

fn is_relation(cells: &[&str], resolve: &dyn Fn(&str) -> Option<usize>) -> bool {
    let mut any_resolved = false;
    for cell in cells {
        let Some(items) = relation_items(cell) else {
            return false;
        };
        any_resolved |= items.iter().any(|(_, path)| resolve(path).is_some());
    }
    any_resolved
}

/// The distinct items of a column whose cells hold several, when it looks like a multi-select.
fn multi_select_options(cells: &[&str]) -> Option<Vec<String>> {
    let mut options: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut several = false;
    for cell in cells {
        let items: Vec<&str> = cell
            .split(", ")
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect();
        several |= items.len() >= 2;
        for item in items {
            if item.chars().count() > MAX_OPTION_CHARS || item.contains(['\n', '\r']) {
                return None;
            }
            if seen.insert(item) {
                options.push(item.to_owned());
                if options.len() > MAX_OPTIONS {
                    return None;
                }
            }
        }
    }
    (several && !options.is_empty()).then_some(options)
}

/// The distinct values of a column when it looks like a select: few, short, one line, and some
/// repeated (or the table is tiny).
fn select_options(cells: &[&str], rows: usize) -> Option<Vec<String>> {
    let mut options: Vec<String> = Vec::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for cell in cells {
        if cell.chars().count() > MAX_OPTION_CHARS || cell.contains(['\n', '\r']) {
            return None;
        }
        let count = counts.entry(cell).or_default();
        if *count == 0 {
            options.push((*cell).to_owned());
            if options.len() > MAX_OPTIONS {
                return None;
            }
        }
        *count += 1;
    }
    let repeated = counts.values().any(|count| *count > 1);
    (repeated || rows <= SELECT_ROWS).then_some(options)
}

/// The values of a row, by column name. `resolve` answers the arena index of the page a relation
/// item names.
pub(crate) fn row_values(
    table: &Table,
    row: usize,
    resolve: &dyn Fn(&str) -> Option<usize>,
) -> Vec<(String, RawValue)> {
    let Some(cells) = table.rows.get(row) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for (index, column) in table.columns.iter().enumerate() {
        let Some(cell) = cells
            .get(index)
            .map(String::as_str)
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        let value = match column.kind {
            ColKind::Empty | ColKind::Title => continue,
            ColKind::Checkbox => parse_bool(cell).map(|b| RawValue::Plain(ImportValue::Bool(b))),
            ColKind::Number => parse_number(cell).map(|n| RawValue::Plain(ImportValue::Number(n))),
            ColKind::Date => parse_date(cell)
                .map(|(date, time)| RawValue::Plain(ImportValue::Date { date, time })),
            ColKind::Url | ColKind::Email | ColKind::Select | ColKind::Text => {
                Some(RawValue::Plain(ImportValue::Text(cell.to_owned())))
            }
            ColKind::Relation => relation_items(cell).map(|items| {
                RawValue::Pages(
                    items
                        .into_iter()
                        .map(|(title, path)| match resolve(&path) {
                            Some(id) => PendingTarget::Planned(id),
                            None => PendingTarget::Title(title),
                        })
                        .collect(),
                )
            }),
            ColKind::MultiSelect => {
                let items: Vec<String> = cell
                    .split(", ")
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect();
                (!items.is_empty()).then_some(RawValue::Plain(ImportValue::List(items)))
            }
        };
        if let Some(value) = value {
            values.push((column.name.clone(), value));
        }
    }
    values
}

/// The warning for a table that was cut.
pub(crate) fn cut_warnings(table: &Table) -> Vec<(ImportWarningCode, &'static str)> {
    let mut out = Vec::new();
    if table.columns_cut {
        out.push((ImportWarningCode::PropertyLimit, "columns"));
    }
    if table.rows_cut {
        out.push((ImportWarningCode::Truncated, "rows"));
    }
    out
}

/// Whether the source index knows the page of a relation item (for callers that only have the
/// index).
pub(crate) fn resolver<'a>(
    index: &'a SourceIndex,
    dir: &'a str,
) -> impl Fn(&str) -> Option<usize> + 'a {
    move |dest| index.page_for_dest(dir, dest)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<usize> {
        None
    }

    fn table(text: &str) -> Table {
        let mut table = parse(text, 1_000);
        infer(&mut table, &none);
        table
    }

    fn kinds(table: &Table) -> Vec<ColKind> {
        table.columns.iter().map(|c| c.kind).collect()
    }

    #[test]
    fn headers_titles_and_padding() {
        let table = parse("Title,Name,,Name\nA,B\nC,D,E,F,G\n", 100);
        let names: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Title", "Name", "Column 3", "Name (2)"]);
        // The first column named Name or Title.
        assert_eq!(table.title_col, 0);
        assert_eq!(table.rows[0], ["A", "B", "", ""]);
        assert_eq!(table.rows[1], ["C", "D", "E", "F"]);
        assert_eq!(table.title(0), "A");

        let table = parse("Alpha,Name\nx,y\n,z\n", 100);
        assert_eq!(table.title_col, 1);
        assert_eq!(table.title(0), "y");
        let table = parse("Alpha,Beta\n,x\n", 100);
        assert_eq!(table.title_col, 0);
        assert_eq!(table.title(0), "Untitled");
    }

    #[test]
    fn delimiter_quotes_and_formula_guard() {
        let table = parse(
            "Name;Note\n\"a;b\";\"he said \"\"hi\"\"\nand left\"\n'=SUM(A1);'-5\n",
            100,
        );
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.rows[0], ["a;b", "he said \"hi\"\nand left"]);
        assert_eq!(table.rows[1], ["=SUM(A1)", "-5"]);
        // A quote that is not a guard stays.
        assert_eq!(parse("A\n'quoted'\n", 10).rows[0], ["'quoted'"]);
        // The byte order mark is dropped.
        assert_eq!(parse("\u{feff}A,B\n1,2\n", 10).columns[0].name, "A");
    }

    #[test]
    fn empty_and_header_only_files() {
        assert!(parse("", 10).columns.is_empty());
        assert!(parse("\n", 10).columns.is_empty() || parse("\n", 10).rows.is_empty());
        let table = parse("A,B\n", 10);
        assert_eq!(table.columns.len(), 2);
        assert!(table.rows.is_empty());
    }

    #[test]
    fn rows_and_columns_are_capped() {
        let rows = (0..50).map(|n| format!("r{n},x\n")).collect::<String>();
        let table = parse(&format!("Name,V\n{rows}"), 10);
        assert_eq!(table.rows.len(), 10);
        assert!(table.rows_cut);
        let header = (0..300)
            .map(|n| format!("c{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let table = parse(&header, 10);
        assert_eq!(table.columns.len(), MAX_COLUMNS);
        assert!(table.columns_cut);
    }

    #[test]
    fn kinds_follow_the_cells() {
        let table = table(
            "Name,Done,N,When,Link,Mail,Tags,Status,Notes\n\
             a,Yes,1.5,\"October 7, 2026\",https://example.com/a,ann@example.com,\"x, y\",Open,this is a long free text of a note\n\
             b,No,2,2026-10-08,https://example.com/b,bob@example.com,\"y, z\",Closed,another note entirely\n\
             c,,3,,,,,Open,\n\
             d,Yes,4,2026-10-09,https://example.com/c,cy@example.com,x,Open,more\n",
        );
        assert_eq!(
            kinds(&table),
            [
                ColKind::Title,
                ColKind::Checkbox,
                ColKind::Number,
                ColKind::Date,
                ColKind::Url,
                ColKind::Email,
                ColKind::MultiSelect,
                ColKind::Select,
                ColKind::Text,
            ]
        );
        assert_eq!(table.columns[6].options, ["x", "y", "z"]);
        assert_eq!(table.columns[7].options, ["Open", "Closed"]);
    }

    #[test]
    fn empty_columns_and_tiny_tables() {
        let table = table("Name,Empty,Why\na,,because I said so\nb,,it was long ago\n");
        assert_eq!(table.columns[1].kind, ColKind::Empty);
        // Two rows: free text still reads as a select.
        assert_eq!(table.columns[2].kind, ColKind::Select);
        let big = table_of_text(10);
        assert_eq!(big.columns[1].kind, ColKind::Text);
    }

    fn table_of_text(rows: usize) -> Table {
        let body = (0..rows)
            .map(|n| format!("r{n},unique text {n}\n"))
            .collect::<String>();
        table(&format!("Name,Text\n{body}"))
    }

    #[test]
    fn long_or_many_values_are_text() {
        let long = "word ".repeat(30);
        let body = format!("a,{long}\nb,{long}\nc,{long}\nd,{long}\n");
        let table = table(&format!("Name,V\n{body}"));
        assert_eq!(table.columns[1].kind, ColKind::Text);
        let many = (0..60)
            .map(|n| format!("r{n},v{n}\nq{n},v{n}\n"))
            .collect::<String>();
        let table = table_from(&format!("Name,V\n{many}"));
        assert_eq!(table.columns[1].kind, ColKind::Text);
    }

    fn table_from(text: &str) -> Table {
        table(text)
    }

    #[test]
    fn numbers_before_dates_and_identifiers_are_text() {
        let t = table(
            "Name,Year,Zip\na,2026,01234\nb,2025,98000\nc,2024,00501\nd,2023,12345\ne,2022,00700\n",
        );
        assert_eq!(t.columns[1].kind, ColKind::Number);
        assert_eq!(t.columns[2].kind, ColKind::Text);
    }

    #[test]
    fn notion_relations() {
        let cells = "Page A (Page%20A%200123.md), Page B (Sub/Page%20B%204567.md)";
        let items = relation_items(cells).unwrap();
        assert_eq!(
            items,
            [
                ("Page A".to_owned(), "Page%20A%200123.md".to_owned()),
                ("Page B".to_owned(), "Sub/Page%20B%204567.md".to_owned())
            ]
        );
        // A title with parentheses of its own.
        assert_eq!(
            relation_items("One (two) (One%20two.md)").unwrap(),
            [("One (two)".to_owned(), "One%20two.md".to_owned())]
        );
        assert!(relation_items("just text").is_none());
        assert!(relation_items("A (x.md) trailing").is_none());
        assert!(relation_items("").is_none());

        let resolve = |path: &str| (path == "Page%20A%200123.md").then_some(7);
        let mut t = parse(
            "Name,Related\nx,Page A (Page%20A%200123.md)\ny,\"Page A (Page%20A%200123.md), Gone (Gone%209999.md)\"\n",
            10,
        );
        infer(&mut t, &resolve);
        assert_eq!(t.columns[1].kind, ColKind::Relation);
        let values = row_values(&t, 1, &resolve);
        assert_eq!(
            values,
            vec![(
                "Related".to_owned(),
                RawValue::Pages(vec![
                    PendingTarget::Planned(7),
                    PendingTarget::Title("Gone".into())
                ])
            )]
        );
        // Nothing resolves: plain text.
        let mut t = parse("Name,Related\nx,Page A (Page%20A%200123.md)\n", 10);
        infer(&mut t, &none);
        assert_ne!(t.columns[1].kind, ColKind::Relation);
    }

    #[test]
    fn row_values_by_kind() {
        let t = table(
            "Name,Done,N,When,Tags,Status\nrow,Yes,3,\"Oct 7, 2026 2:30 PM\",\"a, b\",Open\nrow2,No,4,2026-10-08,\"b, c\",Open\n",
        );
        let values = row_values(&t, 0, &none);
        assert_eq!(
            values,
            vec![
                ("Done".to_owned(), RawValue::Plain(ImportValue::Bool(true))),
                ("N".to_owned(), RawValue::Plain(ImportValue::Number(3.0))),
                (
                    "When".to_owned(),
                    RawValue::Plain(ImportValue::Date {
                        date: "2026-10-07".into(),
                        time: Some("14:30".into())
                    })
                ),
                (
                    "Tags".to_owned(),
                    RawValue::Plain(ImportValue::List(vec!["a".into(), "b".into()]))
                ),
                (
                    "Status".to_owned(),
                    RawValue::Plain(ImportValue::Text("Open".into()))
                ),
            ]
        );
        assert!(row_values(&t, 9, &none).is_empty());
    }
}
