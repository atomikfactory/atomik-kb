//! The inline content of a paragraph being read, with the blocks that come out of it: an image in
//! the middle of text splits the paragraph, a file linked from text adds a block after it, a
//! paragraph that is only a link to a page is a card.

use serde_json::Value;

use crate::json::{self, MarkSet, is_blank_text, kind, plain};

/// How a piece of text came to be there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Ordinary text.
    Normal,
    /// The text of a link to a file: it disappears when the link is alone in its paragraph (the
    /// file block stands for it).
    FileLinkText,
}

#[derive(Debug)]
enum Piece {
    /// An inline node (a text run, a hard break, a page reference, a date, an equation).
    Inline(Value, Role),
    /// A block that stands where it is: the paragraph splits around it.
    Block(Value),
    /// A block that follows the whole run.
    After(Value),
}

/// The inline content of one text block while it is read.
#[derive(Debug, Default)]
pub(crate) struct InlineRun {
    pieces: Vec<Piece>,
}

impl InlineRun {
    /// Whether text added now would start a line or follow a space: HTML drops a space there.
    pub(crate) fn ends_with_space(&self) -> bool {
        match self.pieces.last() {
            None | Some(Piece::Block(_) | Piece::After(_)) => true,
            Some(Piece::Inline(node, _)) => match kind(node) {
                "hardBreak" => true,
                "text" => node
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.ends_with(char::is_whitespace)),
                _ => false,
            },
        }
    }

    /// Adds text; it joins the previous run of text with the same marks and role.
    pub(crate) fn push_text(&mut self, text: &str, marks: &MarkSet, role: Role) {
        if text.is_empty() {
            return;
        }
        let wanted = marks.to_json();
        if let Some(Piece::Inline(last, last_role)) = self.pieces.last_mut()
            && *last_role == role
            && kind(last) == "text"
            && last
                .get("marks")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                == wanted.as_slice()
            && let Some(Value::String(existing)) = last.get_mut("text")
        {
            existing.push_str(text);
            return;
        }
        self.pieces
            .push(Piece::Inline(json::text_node(text, marks), role));
    }

    /// Adds an inline node.
    pub(crate) fn push_node(&mut self, node: Value) {
        self.pieces.push(Piece::Inline(node, Role::Normal));
    }

    /// Adds a hard break.
    pub(crate) fn push_hard_break(&mut self) {
        self.push_node(json::hard_break());
    }

    /// Removes the last piece when it is a hard break.
    pub(crate) fn pop_hard_break(&mut self) -> bool {
        if matches!(self.pieces.last(), Some(Piece::Inline(node, _)) if kind(node) == "hardBreak") {
            self.pieces.pop();
            true
        } else {
            false
        }
    }

    /// Adds a block at this place: the paragraph splits around it.
    pub(crate) fn push_block(&mut self, block: Value) {
        self.pieces.push(Piece::Block(block));
    }

    /// Adds a block after the run (a file linked from the text).
    pub(crate) fn push_after(&mut self, block: Value) {
        self.pieces.push(Piece::After(block));
    }

    /// The blocks the run makes: paragraphs, a page card for a paragraph that is only a page
    /// link, and the blocks that were hoisted out of it.
    pub(crate) fn into_blocks(self) -> Vec<Value> {
        let has_after = self.pieces.iter().any(|p| matches!(p, Piece::After(_)));
        let only_links = has_after
            && self.pieces.iter().all(|piece| match piece {
                Piece::Inline(node, Role::Normal) => is_blank_text(node),
                _ => true,
            });
        let mut out = Vec::new();
        let mut after = Vec::new();
        let mut current: Vec<Value> = Vec::new();
        for piece in self.pieces {
            match piece {
                Piece::Inline(node, role) => {
                    if !(only_links && (role == Role::FileLinkText || is_blank_text(&node))) {
                        current.push(node);
                    }
                }
                Piece::Block(block) => {
                    flush(&mut current, &mut out);
                    out.push(block);
                }
                Piece::After(block) => after.push(block),
            }
        }
        flush(&mut current, &mut out);
        out.extend(after);
        out
    }

    /// The inline nodes of a heading (or any block that cannot be split), trimmed, and the blocks
    /// to put after it.
    pub(crate) fn into_inline(self) -> (Vec<Value>, Vec<Value>) {
        let mut inline = Vec::new();
        let mut blocks = Vec::new();
        for piece in self.pieces {
            match piece {
                Piece::Inline(node, _) => inline.push(node),
                Piece::Block(block) | Piece::After(block) => blocks.push(block),
            }
        }
        trim(&mut inline);
        (inline, blocks)
    }
}

/// Joins adjacent text nodes that carry the same marks.
fn merge_text(nodes: &mut Vec<Value>) {
    let mut merged: Vec<Value> = Vec::with_capacity(nodes.len());
    for node in nodes.drain(..) {
        if let Some(last) = merged.last_mut()
            && kind(last) == "text"
            && kind(&node) == "text"
            && last.get("marks") == node.get("marks")
            && let Some(Value::String(existing)) = last.get_mut("text")
            && let Some(Value::String(more)) = node.get("text")
        {
            existing.push_str(more);
            continue;
        }
        merged.push(node);
    }
    *nodes = merged;
}

/// Removes white space and hard breaks at both ends of inline nodes.
fn trim(nodes: &mut Vec<Value>) {
    merge_text(nodes);
    let mut start = 0;
    while start < nodes.len() {
        let node = &mut nodes[start];
        let gone = match kind(node) {
            "hardBreak" => true,
            "text" => trim_text(node, true),
            _ => break,
        };
        if !gone {
            break;
        }
        start += 1;
    }
    nodes.drain(..start);
    while let Some(node) = nodes.last_mut() {
        let gone = match kind(node) {
            "hardBreak" => true,
            "text" => trim_text(node, false),
            _ => break,
        };
        if !gone {
            break;
        }
        nodes.pop();
    }
}

/// Trims a text node at one end; `true` when nothing is left of it.
fn trim_text(node: &mut Value, start: bool) -> bool {
    let Some(Value::String(text)) = node.get_mut("text") else {
        return true;
    };
    let trimmed = if start {
        text.trim_start()
    } else {
        text.trim_end()
    };
    if trimmed.len() != text.len() {
        *text = trimmed.to_owned();
    }
    text.is_empty()
}

/// Turns the pending inline nodes into a block and clears them.
fn flush(current: &mut Vec<Value>, out: &mut Vec<Value>) {
    let mut nodes = std::mem::take(current);
    trim(&mut nodes);
    match nodes.as_slice() {
        [] => {}
        [only] if kind(only) == "pageRef" => {
            let mut card = only.clone();
            if let Value::Object(object) = &mut card {
                object.insert("type".into(), Value::String("pageLink".into()));
            }
            out.push(card);
        }
        _ => out.push(plain("paragraph", nodes)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::placeholder::{self, Target};

    fn marks() -> MarkSet {
        MarkSet::default()
    }

    fn run_blocks(build: impl FnOnce(&mut InlineRun)) -> Value {
        let mut run = InlineRun::default();
        build(&mut run);
        Value::Array(run.into_blocks())
    }

    #[test]
    fn text_becomes_a_trimmed_paragraph() {
        let blocks = run_blocks(|run| {
            run.push_text("  hello ", &marks(), Role::Normal);
            run.push_text("world  ", &marks(), Role::Normal);
            run.push_hard_break();
        });
        assert_eq!(
            blocks,
            json!([{"type": "paragraph", "content": [{"type": "text", "text": "hello world"}]}])
        );
        assert_eq!(
            run_blocks(|run| run.push_text("   ", &marks(), Role::Normal)),
            json!([])
        );
        assert_eq!(run_blocks(|_| {}), json!([]));
    }

    #[test]
    fn a_block_in_the_middle_splits_the_paragraph() {
        let image = json!({"type": "image"});
        let blocks = run_blocks(|run| {
            run.push_text("before ", &marks(), Role::Normal);
            run.push_block(image.clone());
            run.push_text(" after", &marks(), Role::Normal);
        });
        assert_eq!(blocks[0]["content"][0]["text"], "before");
        assert_eq!(blocks[1], image);
        assert_eq!(blocks[2]["content"][0]["text"], "after");
    }

    #[test]
    fn a_lone_page_reference_is_a_card() {
        let reference = placeholder::page_ref(&Target::Planned(4), "");
        let blocks = run_blocks(|run| {
            run.push_text(" ", &marks(), Role::Normal);
            run.push_node(reference.clone());
        });
        assert_eq!(blocks[0]["type"], "pageLink");
        assert_eq!(blocks[0]["attrs"]["$page"]["planned"], 4);
        // With text around it, it stays inline.
        let blocks = run_blocks(|run| {
            run.push_text("see ", &marks(), Role::Normal);
            run.push_node(reference.clone());
        });
        assert_eq!(blocks[0]["type"], "paragraph");
        assert_eq!(blocks[0]["content"][1]["type"], "pageRef");
    }

    #[test]
    fn a_file_link_alone_is_its_block_and_in_text_follows_it() {
        let file = json!({"type": "file"});
        let alone = run_blocks(|run| {
            run.push_text("report", &marks(), Role::FileLinkText);
            run.push_after(file.clone());
        });
        assert_eq!(alone, json!([file]));

        let in_text = run_blocks(|run| {
            run.push_text("read the ", &marks(), Role::Normal);
            run.push_text("report", &marks(), Role::FileLinkText);
            run.push_after(file.clone());
        });
        assert_eq!(in_text[0]["content"][0]["text"], "read the report");
        assert_eq!(in_text[1], file);

        let two = run_blocks(|run| {
            run.push_text("a", &marks(), Role::FileLinkText);
            run.push_after(file.clone());
            run.push_text(" ", &marks(), Role::Normal);
            run.push_text("b", &marks(), Role::FileLinkText);
            run.push_after(file.clone());
        });
        assert_eq!(two, json!([file, file]));
    }

    #[test]
    fn inline_content_of_a_heading_keeps_blocks_aside() {
        let mut run = InlineRun::default();
        run.push_text(" title ", &marks(), Role::Normal);
        run.push_block(json!({"type": "image"}));
        let (inline, blocks) = run.into_inline();
        assert_eq!(inline, vec![json!({"type": "text", "text": "title"})]);
        assert_eq!(blocks, vec![json!({"type": "image"})]);
    }

    #[test]
    fn marks_split_runs() {
        let bold = MarkSet {
            bold: true,
            ..MarkSet::default()
        };
        let blocks = run_blocks(|run| {
            run.push_text("a", &marks(), Role::Normal);
            run.push_text("b", &bold, Role::Normal);
            run.push_text("c", &bold, Role::Normal);
        });
        assert_eq!(blocks[0]["content"].as_array().map(Vec::len), Some(2));
        assert_eq!(blocks[0]["content"][1]["text"], "bc");
    }
}
