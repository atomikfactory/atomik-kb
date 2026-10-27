//! Plain text (ADR-0036, section 3.8): blank lines separate paragraphs, single line breaks are
//! hard breaks.

use serde_json::Value;

use crate::json::{self, MarkSet};

/// Reads plain text into paragraphs.
pub(crate) fn convert(text: &str) -> Vec<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut blocks = Vec::new();
    let mut lines: Vec<&str> = Vec::new();
    // `lines()` takes `\n` and `\r\n`; a lone `\r` is a line end too.
    for line in text.lines().flat_map(|line| line.split('\r')) {
        let line = line.trim_end();
        if line.trim().is_empty() {
            push_paragraph(&mut blocks, &mut lines);
        } else {
            lines.push(line);
        }
    }
    push_paragraph(&mut blocks, &mut lines);
    blocks
}

fn push_paragraph(blocks: &mut Vec<Value>, lines: &mut Vec<&str>) {
    if lines.is_empty() {
        return;
    }
    let marks = MarkSet::default();
    let mut content = Vec::new();
    for (index, line) in lines.drain(..).enumerate() {
        if index > 0 {
            content.push(json::hard_break());
        }
        content.push(json::text_node(line, &marks));
    }
    blocks.push(json::paragraph(content));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn paragraphs_and_hard_breaks() {
        let blocks = convert("first line\nsecond line\r\n\r\n\n  third   \n");
        assert_eq!(
            Value::Array(blocks),
            json!([
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "first line"},
                    {"type": "hardBreak"},
                    {"type": "text", "text": "second line"}
                ]},
                {"type": "paragraph", "content": [{"type": "text", "text": "  third"}]}
            ])
        );
    }

    #[test]
    fn empty_and_odd_inputs() {
        assert!(convert("").is_empty());
        assert!(convert(" \n\t\n\r\n").is_empty());
        assert_eq!(convert("a\rb").len(), 1);
        assert_eq!(convert("\u{feff}x").len(), 1);
        assert_eq!(convert("a\n\n\n\nb").len(), 2);
    }
}
