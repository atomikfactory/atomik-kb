//! The CSV of a view (`docs/architecture/import-export.md`, "CSV export").
//!
//! RFC 4180 with a UTF-8 byte order mark, so that spreadsheets read the encoding right, and CRLF
//! line ends (every record ends with one, the last included). A field is quoted when it holds a
//! comma, a double quote, a CR or an LF, or starts or ends with a space; quotes are doubled. A
//! text cell that starts with `=`, `+`, `-`, `@`, a TAB or a CR would be run as a formula by a
//! spreadsheet: it is prefixed with `'` (the importer takes the quote off again). Number cells are
//! never prefixed (`-5` is a number), so a negative number stays a number.

use crate::value::ExportValue;

/// The first characters of a text that a spreadsheet may run as a formula.
const FORMULA_STARTS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// RFC 4180 text with a UTF-8 byte order mark and CRLF line ends: the header row, then one row
/// per entry of `rows` (a `None` cell is empty). Cells are quoted when needed; text that a
/// spreadsheet would run as a formula is prefixed with `'`. A row with fewer cells than the
/// header is padded with empty ones. Without columns there is nothing to write but the mark.
#[must_use]
pub fn csv(
    header: &[String],
    rows: &[Vec<Option<ExportValue>>],
    local_offset_seconds: i32,
) -> String {
    let mut out = String::from('\u{feff}');
    if header.is_empty() {
        return out;
    }
    record(&mut out, header.iter().map(|name| text_cell(name)));
    for row in rows {
        let cells = row.iter().map(|cell| match cell {
            None => String::new(),
            // A number is never a formula, whatever its sign.
            Some(ExportValue::Number(number)) => number.to_string(),
            Some(value) => text_cell(&value.text(local_offset_seconds)),
        });
        let padding = header.len().saturating_sub(row.len());
        record(
            &mut out,
            cells.chain(std::iter::repeat_n(String::new(), padding)),
        );
    }
    out
}

/// `text` as a cell of text: prefixed with `'` when a spreadsheet could take it for a formula.
fn text_cell(text: &str) -> String {
    if text.starts_with(FORMULA_STARTS) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

/// One record: the fields quoted as needed, separated by commas, ended by CRLF.
fn record(out: &mut String, cells: impl Iterator<Item = String>) {
    for (index, cell) in cells.enumerate() {
        if index > 0 {
            out.push(',');
        }
        field(out, &cell);
    }
    out.push_str("\r\n");
}

/// One field, in quotes when it needs them.
fn field(out: &mut String, cell: &str) {
    let quoted =
        cell.contains([',', '"', '\r', '\n']) || cell.starts_with(' ') || cell.ends_with(' ');
    if quoted {
        out.push('"');
        out.push_str(&cell.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(cell);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::value::{LinkedFile, LinkedPage};

    fn header(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn text(value: &str) -> Option<ExportValue> {
        Some(ExportValue::Text(value.to_owned()))
    }

    /// The text after the byte order mark.
    fn body(csv: &str) -> &str {
        csv.strip_prefix('\u{feff}').unwrap()
    }

    #[test]
    fn a_csv_has_a_byte_order_mark_a_header_and_crlf() {
        let out = csv(
            &header(&["Name", "Status"]),
            &[vec![text("Alpha"), text("Done")], vec![text("Beta"), None]],
            0,
        );
        assert!(out.starts_with('\u{feff}'));
        assert_eq!(body(&out), "Name,Status\r\nAlpha,Done\r\nBeta,\r\n");
        // Never a bare LF.
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    }

    #[test]
    fn no_rows_is_just_the_header_and_no_columns_is_just_the_mark() {
        assert_eq!(body(&csv(&header(&["Name"]), &[], 0)), "Name\r\n");
        assert_eq!(csv(&[], &[], 0), "\u{feff}");
        assert_eq!(csv(&[], &[vec![text("x")]], 0), "\u{feff}");
    }

    #[test]
    fn fields_are_quoted_when_they_need_it() {
        let cases = [
            ("plain", "plain"),
            ("a,b", "\"a,b\""),
            ("say \"hi\"", "\"say \"\"hi\"\"\""),
            ("two\nlines", "\"two\nlines\""),
            ("two\r\nlines", "\"two\r\nlines\""),
            ("lone\rCR", "\"lone\rCR\""),
            (" leading", "\" leading\""),
            ("trailing ", "\"trailing \""),
            ("in side", "in side"),
            ("semi;colon", "semi;colon"),
            ("tab\there", "tab\there"),
            ("", ""),
            ("日本語, café", "\"日本語, café\""),
        ];
        for (cell, expected) in cases {
            let out = csv(&header(&["H"]), &[vec![text(cell)]], 0);
            assert_eq!(body(&out), format!("H\r\n{expected}\r\n"), "{cell:?}");
        }
    }

    #[test]
    fn the_header_is_quoted_like_any_row() {
        let out = csv(&header(&["Name", "Due, date", "Say \"x\""]), &[], 0);
        assert_eq!(body(&out), "Name,\"Due, date\",\"Say \"\"x\"\"\"\r\n");
    }

    #[test]
    fn text_that_looks_like_a_formula_gets_a_quote() {
        for start in ["=", "+", "-", "@", "\t", "\r"] {
            let cell = format!("{start}SUM(A1:A2)");
            let out = csv(&header(&["H"]), &[vec![text(&cell)]], 0);
            let expected_cell = format!("'{cell}");
            let written = body(&out).strip_prefix("H\r\n").unwrap();
            // A CR starts a quoted field; the quote of the guard comes first inside it.
            let written = written.strip_suffix("\r\n").unwrap();
            let unquoted = written
                .strip_prefix('"')
                .and_then(|w| w.strip_suffix('"'))
                .unwrap_or(written);
            assert_eq!(unquoted, expected_cell, "{cell:?}");
        }
        let out = csv(
            &header(&["H"]),
            &[vec![text("=HYPERLINK(\"http://x\",\"y\")")]],
            0,
        );
        assert_eq!(
            body(&out),
            "H\r\n\"'=HYPERLINK(\"\"http://x\"\",\"\"y\"\")\"\r\n"
        );
        // Elsewhere in the text, or after a space, nothing to guard (a leading space is quoted instead).
        let out = csv(&header(&["H"]), &[vec![text("a=b"), text("1+1")]], 0);
        assert_eq!(body(&out), "H\r\na=b,1+1\r\n");
        let out = csv(&header(&["H"]), &[vec![text(" =x")]], 0);
        assert_eq!(body(&out), "H\r\n\" =x\"\r\n");
    }

    #[test]
    fn numbers_are_never_guarded_but_their_text_forms_are() {
        let numbers = vec![
            Some(ExportValue::Number(-5.0)),
            Some(ExportValue::Number(3.5)),
            Some(ExportValue::Number(0.1)),
            Some(ExportValue::Number(-0.25)),
            Some(ExportValue::Number(1e21)),
        ];
        let out = csv(&header(&["a", "b", "c", "d", "e"]), &[numbers], 0);
        assert_eq!(
            body(&out),
            "a,b,c,d,e\r\n-5,3.5,0.1,-0.25,1000000000000000000000\r\n"
        );
        // The same digits as text are text.
        let out = csv(&header(&["H"]), &[vec![text("-5")]], 0);
        assert_eq!(body(&out), "H\r\n'-5\r\n");
        // An option or a page title that starts with a dash is text too.
        let option = Some(ExportValue::Options(vec!["-x".to_owned(), "y".to_owned()]));
        let out = csv(&header(&["H"]), &[vec![option]], 0);
        assert_eq!(body(&out), "H\r\n\"'-x, y\"\r\n");
    }

    #[test]
    fn the_header_is_guarded_too() {
        let out = csv(&header(&["=cmd|' /C calc'!A0", "-x", "+1 week"]), &[], 0);
        assert_eq!(body(&out), "'=cmd|' /C calc'!A0,'-x,'+1 week\r\n");
    }

    #[test]
    fn every_kind_of_value_is_its_text() {
        let row = vec![
            Some(ExportValue::Checkbox(true)),
            Some(ExportValue::Checkbox(false)),
            Some(ExportValue::Date {
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
            }),
            Some(ExportValue::Date {
                date: "2026-10-07".into(),
                time: None,
            }),
            Some(ExportValue::Pages(vec![
                LinkedPage {
                    title: "A".into(),
                    href: Some("A.md".into()),
                },
                LinkedPage {
                    title: "B".into(),
                    href: None,
                },
            ])),
            Some(ExportValue::Files(vec![LinkedFile {
                name: "x.pdf".into(),
                href: None,
            }])),
            Some(ExportValue::Checklist(vec![
                (true, "a".into()),
                (false, "b".into()),
            ])),
            Some(ExportValue::Place {
                name: "Paris".into(),
                lat: Some(48.8566),
                lon: Some(2.3522),
            }),
            // 2025-10-09T08:53:20Z at UTC+2.
            Some(ExportValue::Instant(1_760_000_000_000)),
        ];
        let out = csv(
            &header(&["1", "2", "3", "4", "5", "6", "7", "8", "9"]),
            &[row],
            7200,
        );
        assert_eq!(
            body(&out),
            "1,2,3,4,5,6,7,8,9\r\nYes,No,2026-10-07 14:30,2026-10-07,\"A, B\",x.pdf,[x] a; [ ] b,\"Paris (48.8566, 2.3522)\",2025-10-09 10:53\r\n"
        );
    }

    #[test]
    fn short_rows_are_padded_and_long_ones_are_kept() {
        let out = csv(
            &header(&["A", "B", "C"]),
            &[
                vec![text("1")],
                vec![],
                vec![text("1"), text("2"), text("3"), text("4")],
            ],
            0,
        );
        assert_eq!(body(&out), "A,B,C\r\n1,,\r\n,,\r\n1,2,3,4\r\n");
    }

    #[test]
    fn a_csv_reads_back_with_a_standard_reader() {
        // A small RFC 4180 reader, to check that what is written is what the rules say.
        fn read(text: &str) -> Vec<Vec<String>> {
            let mut rows = Vec::new();
            let mut row = Vec::new();
            let mut cell = String::new();
            let mut quoted = false;
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                match (c, quoted) {
                    ('"', true) if chars.peek() == Some(&'"') => {
                        chars.next();
                        cell.push('"');
                    }
                    ('"', true) => quoted = false,
                    ('"', false) if cell.is_empty() => quoted = true,
                    (',', false) => row.push(std::mem::take(&mut cell)),
                    ('\r', false) if chars.peek() == Some(&'\n') => {
                        chars.next();
                        row.push(std::mem::take(&mut cell));
                        rows.push(std::mem::take(&mut row));
                    }
                    (other, _) => cell.push(other),
                }
            }
            rows
        }
        let cells = [
            "plain",
            "a,b",
            "q\"uote",
            "line\nbreak",
            " pad ",
            "",
            "'=x",
            "日本",
        ];
        let row: Vec<Option<ExportValue>> = cells.iter().map(|c| text(c)).collect();
        let head: Vec<String> = (0..cells.len()).map(|n| format!("c{n}")).collect();
        let out = csv(&head, &[row], 0);
        let parsed = read(body(&out));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1], cells);
    }
}
