//! Tests of the Markdown reader: each mapping of section 3.4 of the design with inline sources.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};

use super::*;
use crate::testing::{node_depth, sample_index};

fn md(source: &str) -> Value {
    read(source).0
}

fn read(source: &str) -> (Value, Vec<kb_types::ImportWarning>) {
    let index = sample_index();
    let mut warnings = crate::ctx::Warnings::default();
    let mut ctx = PageCtx::new(&index, &mut warnings, "Export/Notes/Page.md", "Notes", 1);
    let blocks = convert(&mut ctx, source);
    (Value::Array(blocks), warnings.into_vec())
}

fn para(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

#[test]
fn headings_paragraphs_and_breaks() {
    let got = md("# One\n\n###### Six\n\nline one\nline two  \nline three\\\nline four\n\n---\n");
    assert_eq!(
        got[0],
        json!({"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "One"}]})
    );
    assert_eq!(got[1]["attrs"]["level"], 3);
    assert_eq!(
        got[2],
        json!({"type": "paragraph", "content": [
            {"type": "text", "text": "line one line two"},
            {"type": "hardBreak"},
            {"type": "text", "text": "line three"},
            {"type": "hardBreak"},
            {"type": "text", "text": "line four"}
        ]})
    );
    assert_eq!(got[3], json!({"type": "horizontalRule"}));
}

#[test]
fn inline_marks() {
    let got = md(
        "**bold** *italic* ~~strike~~ `code` ***both*** [link](https://example.com) <u>under</u> <mark>marked</mark>",
    );
    let content = got[0]["content"].as_array().unwrap();
    let marks_of = |text: &str| -> Vec<String> {
        content
            .iter()
            .find(|n| n["text"] == text)
            .map(|n| {
                n["marks"]
                    .as_array()
                    .map(|m| {
                        m.iter()
                            .map(|x| x["type"].as_str().unwrap().to_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| panic!("no text {text:?} in {content:?}"))
    };
    assert_eq!(marks_of("bold"), ["bold"]);
    assert_eq!(marks_of("italic"), ["italic"]);
    assert_eq!(marks_of("strike"), ["strike"]);
    assert_eq!(marks_of("code"), ["code"]);
    assert_eq!(marks_of("both"), ["bold", "italic"]);
    assert_eq!(marks_of("link"), ["link"]);
    assert_eq!(marks_of("under"), ["underline"]);
    assert_eq!(marks_of("marked"), ["highlight"]);
    let link = content.iter().find(|n| n["text"] == "link").unwrap();
    assert_eq!(link["marks"][0]["attrs"]["href"], "https://example.com");
}

#[test]
fn escapes_entities_and_special_characters_are_text() {
    let got =
        md("1\\. not a list \\*stars\\* \\[brackets\\] a &amp; b \\<tag\\> \\$5 \\| pipe \\# hash");
    assert_eq!(
        got[0]["content"][0]["text"],
        "1. not a list *stars* [brackets] a & b <tag> $5 | pipe # hash"
    );
}

#[test]
fn code_blocks_and_math() {
    let got = md(
        "```rust ignore\nfn main() {}\n```\n\n```\nplain\n```\n\n    indented\n\n```math\nE = mc^2\n```\n\n$$\na^2 + b^2\n$$\n\nInline $x^2$ and $$y$$ here.",
    );
    assert_eq!(
        got[0],
        json!({"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn main() {}"}]})
    );
    assert_eq!(
        got[1],
        json!({"type": "codeBlock", "content": [{"type": "text", "text": "plain"}]})
    );
    assert_eq!(got[2]["type"], "codeBlock");
    assert_eq!(
        got[3],
        json!({"type": "mathBlock", "content": [{"type": "text", "text": "E = mc^2"}]})
    );
    assert_eq!(
        got[4],
        json!({"type": "mathBlock", "content": [{"type": "text", "text": "a^2 + b^2"}]})
    );
    let content = got[5]["content"].as_array().unwrap();
    assert_eq!(
        content[1],
        json!({"type": "mathInline", "attrs": {"latex": "x^2"}})
    );
    // `$$y$$` in the middle of a line is a block of its own: the paragraph splits around it.
    assert!(
        got.as_array()
            .unwrap()
            .iter()
            .any(|b| b["type"] == "mathBlock" && b["content"][0]["text"] == "y")
    );
}

#[test]
fn quotes_lists_and_tasks() {
    let got = md(
        "> quoted\n> more\n\n- a\n- b\n  - nested\n\n3. three\n4. four\n\n- [x] done\n- [ ] todo\n\n* [x] mixed\n* plain\n",
    );
    assert_eq!(got[0]["type"], "blockquote");
    assert_eq!(got[0]["content"][0]["content"][0]["text"], "quoted more");
    let bullets = &got[1];
    assert_eq!(bullets["type"], "bulletList");
    assert_eq!(
        bullets["content"][1]["content"][0]["content"][0]["text"],
        "b"
    );
    assert_eq!(bullets["content"][1]["content"][1]["type"], "bulletList");
    assert_eq!(got[2]["type"], "orderedList");
    assert_eq!(got[2]["attrs"]["start"], 3);
    assert_eq!(got[3]["type"], "taskList");
    assert_eq!(got[3]["content"][0]["attrs"]["checked"], true);
    assert_eq!(got[3]["content"][1]["attrs"]["checked"], false);
    // In a mixed list the marker stays text.
    assert_eq!(got[4]["type"], "bulletList");
    assert_eq!(
        got[4]["content"][0]["content"][0]["content"][0]["text"],
        "[x] mixed"
    );
}

#[test]
fn list_items_start_with_a_paragraph() {
    let got = md("- ```\n  code\n  ```\n- - nested first\n-\n");
    for item in got[0]["content"].as_array().unwrap() {
        assert_eq!(item["content"][0]["type"], "paragraph", "{item}");
    }
}

#[test]
fn tables() {
    let got = md("| A | B |\n| --- | --- |\n| 1 | a \\| b |\n| 2 |\n| x<br>y | z<br><br>w |\n");
    let table = &got[0];
    assert_eq!(table["type"], "table");
    let rows = table["content"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["content"][0]["type"], "tableHeader");
    assert_eq!(
        rows[0]["content"][0]["content"][0]["content"][0]["text"],
        "A"
    );
    assert_eq!(rows[1]["content"][0]["type"], "tableCell");
    assert_eq!(
        rows[1]["content"][1]["content"][0]["content"][0]["text"],
        "a | b"
    );
    // A short row is padded.
    assert_eq!(rows[2]["content"].as_array().map(Vec::len), Some(2));
    // `<br>` is a hard break, two of them end the paragraph.
    assert_eq!(
        rows[3]["content"][0]["content"][0]["content"][1]["type"],
        "hardBreak"
    );
    assert_eq!(
        rows[3]["content"][1]["content"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        rows[3]["content"][1]["content"][1]["content"][0]["text"],
        "w"
    );
    assert_eq!(
        rows[0]["content"][0]["attrs"],
        json!({"colspan": 1, "rowspan": 1})
    );
}

#[test]
fn callouts_by_annotation_alert_emoji_and_notion_aside() {
    let got = md(
        "<!-- atomik:{\"attrs\":{\"color\":\"blue\",\"icon\":\"📌\"},\"node\":\"callout\"} -->\n> 📌 Pinned text\n\n\
         > [!WARNING]\n> Careful\n\n\
         > 💡 A tip\n> second line\n\n\
         > 🎉\n>\n> Party time\n\n\
         > Not a callout: plain\n\n\
         > - item\n\n\
         <aside>\n💡 Notion callout\n</aside>\n\n\
         <aside>\n🔥\n\nOn fire\n\n</aside>\n\n\
         <aside>\nNo icon here\n</aside>\n",
    );
    assert_eq!(got[0]["type"], "callout");
    assert_eq!(got[0]["attrs"], json!({"icon": "📌", "color": "blue"}));
    assert_eq!(got[0]["content"][0]["content"][0]["text"], "Pinned text");
    assert_eq!(got[1]["attrs"]["icon"], "⚠️");
    assert_eq!(got[1]["content"][0]["content"][0]["text"], "Careful");
    assert_eq!(got[2]["attrs"]["icon"], "💡");
    assert_eq!(
        got[2]["content"][0]["content"][0]["text"],
        "A tip second line"
    );
    assert_eq!(got[3]["attrs"]["icon"], "🎉");
    assert_eq!(got[3]["content"].as_array().map(Vec::len), Some(1));
    assert_eq!(got[3]["content"][0]["content"][0]["text"], "Party time");
    assert_eq!(got[4]["type"], "blockquote");
    assert_eq!(got[5]["type"], "blockquote");
    assert_eq!(got[6]["type"], "callout");
    assert_eq!(got[6]["attrs"], json!({"icon": "💡"}));
    assert_eq!(got[6]["content"][0]["content"][0]["text"], "Notion callout");
    assert_eq!(got[7]["attrs"]["icon"], "🔥");
    assert_eq!(got[7]["content"].as_array().map(Vec::len), Some(1));
    assert_eq!(got[7]["content"][0]["content"][0]["text"], "On fire");
    assert_eq!(got[8]["attrs"]["icon"], "💡");
    assert_eq!(got[8]["content"][0]["content"][0]["text"], "No icon here");
}

#[test]
fn the_alert_icons() {
    let got = md("> [!NOTE]\n> a\n\n> [!TIP]\n> b\n\n> [!IMPORTANT]\n> c\n\n> [!CAUTION]\n> d\n");
    let icons: Vec<_> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["attrs"]["icon"].as_str().unwrap())
        .collect();
    assert_eq!(icons, ["ℹ️", "💡", "❗", "🛑"]);
}

#[test]
fn details_across_blocks() {
    let got = md(
        "<details open>\n<summary>Toggle <b>summary</b></summary>\n\nFirst **para**\n\n- item\n\n</details>\n\n\
         <details>\n<summary>Closed</summary>\n\nbody\n\n<details>\n<summary>Inner</summary>\n\ninside\n\n</details>\n\n</details>\n\nafter",
    );
    let first = &got[0];
    assert_eq!(first["type"], "details");
    assert_eq!(first["attrs"]["open"], true);
    assert_eq!(first["content"][0]["content"][0]["text"], "Toggle summary");
    let content = &first["content"][1];
    assert_eq!(content["type"], "detailsContent");
    assert_eq!(content["content"].as_array().map(Vec::len), Some(2));
    assert_eq!(content["content"][1]["type"], "bulletList");
    let second = &got[1];
    assert!(second["attrs"].is_null());
    assert_eq!(second["content"][1]["content"][1]["type"], "details");
    assert_eq!(
        second["content"][1]["content"][1]["content"][0]["content"][0]["text"],
        "Inner"
    );
    assert_eq!(got[2], para("after"));
    assert_eq!(got.as_array().map(Vec::len), Some(3));
}

#[test]
fn a_heading_inside_the_summary_makes_a_toggle_heading() {
    // What atomik-kb's own export writes (ADR-0040), in both flavours: the text, entities and
    // `<time>` elements decode as they do in a plain summary.
    for level in 1..=3 {
        let got = md(&format!(
            "<details open>\n<summary><h{level}>Part &amp; <b>one</b></h{level}></summary>\n\nbody\n\n</details>"
        ));
        assert_eq!(got[0]["type"], "details");
        assert_eq!(got[0]["attrs"]["open"], true);
        assert_eq!(
            got[0]["content"][0],
            json!({"type": "detailsSummary", "attrs": {"level": level},
                   "content": [{"type": "text", "text": "Part & one"}]}),
            "h{level}"
        );
        assert_eq!(
            got[0]["content"][1]["content"][0]["content"][0]["text"],
            "body"
        );
    }
    // An empty one keeps its level; h4 to h6 clamp to 3 as headings do; the tag may carry
    // attributes and any case (other tools write them).
    let got = md("<details>\n<summary><h1></h1></summary>\n\nx\n\n</details>");
    assert_eq!(
        got[0]["content"][0],
        json!({"type": "detailsSummary", "attrs": {"level": 1}})
    );
    let got = md("<details>\n<summary><H5 id=\"a\">Deep</H5></summary>\n\nx\n\n</details>");
    assert_eq!(got[0]["content"][0]["attrs"]["level"], 3);
    assert_eq!(got[0]["content"][0]["content"][0]["text"], "Deep");
    // A summary that is not one heading is a plain toggle's: text, no level.
    for source in [
        "<summary>a <h2>b</h2></summary>",
        "<summary>Plain</summary>",
        "<summary><h2>a</h2><h2>b</h2></summary>",
    ] {
        let got = md(&format!("<details>\n{source}\n\nx\n\n</details>"));
        assert_eq!(got[0]["type"], "details", "{source}");
        assert!(got[0]["content"][0]["attrs"].is_null(), "{source}");
    }
}

#[test]
fn an_unterminated_details_is_closed_at_the_end() {
    let got = md("<details>\n<summary>Open ended</summary>\n\nbody text");
    assert_eq!(got[0]["type"], "details");
    assert_eq!(
        got[0]["content"][1]["content"][0]["content"][0]["text"],
        "body text"
    );
}

#[test]
fn inline_html() {
    let got = md(
        "a<br>b <time datetime=\"2026-10-07T14:30\" data-reminder=\"15\">2026-10-07 14:30</time> c <sub>sub</sub> <span>span</span> <blink>text</blink> <time datetime=\"nope\">kept</time>",
    );
    let content = got[0]["content"].as_array().unwrap();
    assert_eq!(content[1]["type"], "hardBreak");
    let mention = content.iter().find(|n| n["type"] == "dateMention").unwrap();
    assert_eq!(
        mention["attrs"],
        json!({"date": "2026-10-07", "time": "14:30", "reminder": 15})
    );
    let text: String = content.iter().filter_map(|n| n["text"].as_str()).collect();
    // The mention's own text is not repeated; other tags are dropped and their text kept.
    assert_eq!(text, "ab  c sub span text kept");
    // A mention is only valid with a real date.
    assert_eq!(
        content
            .iter()
            .filter(|n| n["type"] == "dateMention")
            .count(),
        1
    );
}

#[test]
fn html_blocks_go_through_the_html_reader() {
    let got = md(
        "<table><tr><th>H</th></tr><tr><td>c</td></tr></table>\n\n<div align=\"center\"><p>centred</p></div>\n",
    );
    assert_eq!(got[0]["type"], "table");
    assert_eq!(got[1], para("centred"));
}

#[test]
fn links_to_pages_by_path_and_url() {
    let got = md(
        "See [other](../Other.md) and [web](https://example.com) and [mail](mailto:a@b.c).\n\n\
         [Sub page](Sub%20Page%200123456789abcdef0123456789abcdef.md)\n\n\
         [notion](https://www.notion.so/Sub-Page-0123456789abcdef0123456789abcdef)\n\n\
         <https://auto.link/x>\n\n[anchor](#top) [empty]()",
    );
    let first = got[0]["content"].as_array().unwrap();
    assert_eq!(first[1]["type"], "pageRef");
    assert_eq!(first[1]["attrs"]["$page"], json!({"planned": 2}));
    assert_eq!(first[3]["marks"][0]["attrs"]["href"], "https://example.com");
    assert_eq!(first[5]["marks"][0]["attrs"]["href"], "mailto:a@b.c");
    assert_eq!(got[1]["type"], "pageLink");
    assert_eq!(got[1]["attrs"]["$page"], json!({"planned": 3}));
    assert_eq!(got[2]["type"], "pageLink");
    assert_eq!(got[2]["attrs"]["$page"], json!({"planned": 3}));
    assert_eq!(
        got[3]["content"][0]["marks"][0]["attrs"]["href"],
        "https://auto.link/x"
    );
    assert_eq!(
        got[4]["content"]
            .as_array()
            .map(|c| c.iter().all(|n| n.get("marks").is_none())),
        Some(true)
    );
}

#[test]
fn files_and_images_of_the_source() {
    let (got, warnings) = read(
        "![A diagram](diagram.png \"Figure 1\")\n\n[report](report.pdf)\n\n[photo](photo.jpg)\n\nRead the [report](report.pdf) first.\n\n\
         text ![inline](diagram.png) more text\n\n![](Missing.png)\n\n[gone](gone.pdf) [gone page](Gone.md)\n",
    );
    assert_eq!(got[0]["type"], "image");
    assert_eq!(
        got[0]["attrs"],
        json!({"$file": "Export/Notes/diagram.png", "alt": "A diagram", "caption": "Figure 1"})
    );
    assert_eq!(got[1]["type"], "file");
    assert_eq!(got[1]["attrs"]["$file"], "Export/Notes/report.pdf");
    assert_eq!(got[1]["attrs"]["$label"], "report");
    // A picture linked alone is an image.
    assert_eq!(got[2]["type"], "image");
    assert_eq!(got[2]["attrs"]["alt"], "photo");
    // A link in text keeps its text and adds the file after.
    assert_eq!(got[3]["content"][0]["text"], "Read the report first.");
    assert_eq!(got[4]["type"], "file");
    // An image in the middle of text splits the paragraph.
    assert_eq!(got[5], para("text"));
    assert_eq!(got[6]["type"], "image");
    assert_eq!(got[7], para("more text"));
    // Missing files stay text.
    assert_eq!(got[8], para("Missing.png"));
    assert_eq!(got[9]["content"][0]["text"], "gone gone page");
    let codes: Vec<_> = warnings
        .iter()
        .map(|w| (w.code, w.detail.clone().unwrap()))
        .collect();
    assert_eq!(
        codes,
        vec![
            (
                ImportWarningCode::MissingFile,
                "Notes/Missing.png".to_owned()
            ),
            (ImportWarningCode::MissingFile, "Notes/gone.pdf".to_owned()),
            (
                ImportWarningCode::UnresolvedLink,
                "Notes/Gone.md".to_owned()
            ),
        ]
    );
}

#[test]
fn remote_and_embedded_images_are_never_fetched() {
    let (got, warnings) = read(
        "![remote](https://example.com/i.png)\n\n![](https://example.com/j.png)\n\n![embedded](data:image/png;base64,AAAA)\n",
    );
    assert_eq!(got[0]["content"][0]["text"], "remote");
    assert_eq!(
        got[0]["content"][0]["marks"][0]["attrs"]["href"],
        "https://example.com/i.png"
    );
    assert_eq!(got[1]["content"][0]["text"], "https://example.com/j.png");
    assert_eq!(got[2]["content"][0]["text"], "embedded");
    assert!(got[2]["content"][0].get("marks").is_none());
    let codes: Vec<_> = warnings.iter().map(|w| w.code).collect();
    assert_eq!(
        codes,
        [
            ImportWarningCode::RemoteImage,
            ImportWarningCode::RemoteImage,
            ImportWarningCode::EmbeddedImage
        ]
    );
}

#[test]
fn attachment_and_atomik_links_wait_for_the_core() {
    let id = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
    let page = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
    let block = "8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7";
    let got = md(&format!(
        "![alt](attachment://{id} \"cap\")\n\n[name](attachment://{id})\n\nsee [there](atomik-kb://page/{page}#{block}) now\n\n[bad](attachment://nope)"
    ));
    assert_eq!(
        got[0]["attrs"],
        json!({"$attachment": id, "alt": "alt", "caption": "cap"})
    );
    assert_eq!(got[1]["type"], "file");
    assert_eq!(got[1]["attrs"]["$attachment"], id);
    let reference = &got[2]["content"][1];
    assert_eq!(reference["type"], "pageRef");
    assert_eq!(
        reference["attrs"]["$page"],
        json!({"existing": page, "block": block})
    );
    assert_eq!(reference["attrs"]["$label"], "there");
    assert_eq!(got[3], para("bad"));
}

#[test]
fn wiki_links_and_embeds() {
    let (got, _) = read(
        "A [[Other]] and [[Folder/Deep Note#Heading|shown]] and ![[diagram.png]] and ![[Embedded page]].",
    );
    let content = got[0]["content"].as_array().unwrap();
    assert_eq!(
        content[1]["attrs"]["$page"],
        json!({"wiki": "Other", "from": 1})
    );
    assert_eq!(content[1]["attrs"]["$label"], "[[Other]]");
    assert_eq!(
        content[3]["attrs"]["$page"],
        json!({"wiki": "Folder/Deep Note", "from": 1})
    );
    assert_eq!(content[3]["attrs"]["$label"], "[[Folder/Deep Note|shown]]");
    // The embed of a file of the source is an image block, the paragraph splits around it.
    let blocks = got.as_array().unwrap();
    assert!(
        blocks
            .iter()
            .any(|b| b["type"] == "image" && b["attrs"]["$file"] == "Export/Notes/diagram.png")
    );
    assert!(
        blocks
            .iter()
            .flat_map(|b| b["content"].as_array().cloned().unwrap_or_default())
            .any(|n| n["type"] == "pageRef" && n["attrs"]["$page"]["wiki"] == "Embedded page")
    );
}

#[test]
fn footnotes_are_text_and_trailing_paragraphs() {
    let got = md("Claim[^1] and another[^note].\n\n[^1]: First note.\n[^note]: Second note.\n");
    assert_eq!(got[0]["content"][0]["text"], "Claim[1] and another[note].");
    assert_eq!(got[1], para("[1] First note."));
    assert_eq!(got[2], para("[note] Second note."));
}

#[test]
fn annotations_restore_layout() {
    let id = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
    let got = md(&format!(
        "<!-- atomik:{{\"attrs\":{{\"align\":\"right\",\"widthPercent\":60}}}} -->\n![alt](attachment://{id} \"caption\")\n\n\
         <!-- atomik:{{\"attrs\":{{\"view\":\"preview\"}}}} -->\n[doc.pdf](report.pdf)\n\n\
         <!-- atomik:{{\"attrs\":{{\"columns\":2}},\"items\":2,\"node\":\"gallery\"}} -->\n![](diagram.png \"one\")\n\n![](attachment://{id})\n\nafter gallery\n\n\
         <!-- atomik:{{\"node\":\"tableOfContents\"}} -->\n\n<!-- atomik:{{\"node\":\"breadcrumb\"}} -->\n\n\
         <!-- atomik:{{\"node\":\"mystery\"}} -->\n\n<!-- plain comment -->\n\nlast"
    ));
    assert_eq!(got[0]["attrs"]["widthPercent"], 60);
    assert_eq!(got[0]["attrs"]["align"], "right");
    assert_eq!(got[0]["attrs"]["caption"], "caption");
    assert_eq!(got[1]["attrs"]["view"], "preview");
    assert_eq!(got[2]["type"], "gallery");
    assert_eq!(got[2]["attrs"]["columns"], 2);
    assert_eq!(
        got[2]["attrs"]["items"],
        json!([{"$file": "Export/Notes/diagram.png", "caption": "one"}, {"$attachment": id}])
    );
    assert_eq!(got[3], para("after gallery"));
    assert_eq!(got[4], json!({"type": "tableOfContents"}));
    assert_eq!(got[5], json!({"type": "breadcrumb"}));
    assert_eq!(got[6], para("last"));
    assert_eq!(got.as_array().map(Vec::len), Some(7));
}

#[test]
fn annotated_whiteboards_columns_and_views() {
    let got = md(
        "<!-- atomik:{\"attrs\":{\"height\":480},\"file\":\"Board.excalidraw\",\"node\":\"whiteboard\"} -->\n> **Whiteboard**\n>\n> - text\n\n\
         <!-- atomik:{\"node\":\"columnList\"} -->\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nLeft\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nRight\n\nTwo\n\n<!-- atomik:{\"end\":\"columnList\"} -->\n\n\
         <!-- atomik:{\"attrs\":{\"layout\":\"table\",\"properties\":[\"Status\"],\"sort\":{\"by\":\"Due\",\"dir\":\"desc\"},\"source\":{\"kind\":\"children\"}},\"node\":\"pageList\"} -->\n\n\
         <!-- atomik:{\"node\":\"whiteboard\",\"file\":\"Gone.excalidraw\"} -->\n> **Whiteboard**\n\nend",
    );
    assert_eq!(got[0]["type"], "whiteboard");
    assert_eq!(got[0]["attrs"]["height"], 480);
    assert!(got[0]["attrs"]["$elements"]["a"].is_object());
    assert_eq!(got[1]["type"], "columnList");
    assert_eq!(got[1]["content"].as_array().map(Vec::len), Some(2));
    assert_eq!(got[1]["content"][0]["content"][0], para("Left"));
    assert_eq!(
        got[1]["content"][1]["content"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(got[2]["type"], "pageList");
    assert_eq!(got[2]["attrs"]["$names"], true);
    assert_eq!(got[2]["attrs"]["properties"], json!(["Status"]));
    // A whiteboard whose file is missing is an empty one, with a warning; its summary quote is
    // skipped like the others.
    assert_eq!(got[3]["type"], "whiteboard");
    assert_eq!(got[4], para("end"));
    assert_eq!(got.as_array().map(Vec::len), Some(5));
}

#[test]
fn columns_that_never_close_or_are_too_few_are_flattened() {
    let got = md(
        "<!-- atomik:{\"node\":\"columnList\"} -->\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nonly one\n\n<!-- atomik:{\"end\":\"columnList\"} -->\n\nnext",
    );
    assert_eq!(got[0], para("only one"));
    assert_eq!(got[1], para("next"));
    let got = md(
        "<!-- atomik:{\"node\":\"columnList\"} -->\n\n<!-- atomik:{\"node\":\"column\"} -->\n\na\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nb",
    );
    assert_eq!(got[0]["type"], "columnList");
    assert_eq!(got[0]["content"].as_array().map(Vec::len), Some(2));
    // More than five columns: the rest joins the fifth.
    let mut source = String::from("<!-- atomik:{\"node\":\"columnList\"} -->\n\n");
    for n in 0..7 {
        source.push_str(&format!(
            "<!-- atomik:{{\"node\":\"column\"}} -->\n\nc{n}\n\n"
        ));
    }
    source.push_str("<!-- atomik:{\"end\":\"columnList\"} -->\n");
    let got = md(&source);
    assert_eq!(got[0]["content"].as_array().map(Vec::len), Some(5));
    assert_eq!(
        got[0]["content"][4]["content"].as_array().map(Vec::len),
        Some(3)
    );
}

#[test]
fn columns_closed_by_their_container() {
    // The list item ends before the end annotation: the columns close with it.
    let got = md(
        "- item\n\n  <!-- atomik:{\"node\":\"columnList\"} -->\n\n  <!-- atomik:{\"node\":\"column\"} -->\n\n  a\n\n  <!-- atomik:{\"node\":\"column\"} -->\n\n  b\n\nafter",
    );
    assert_eq!(got[0]["type"], "bulletList");
    assert_eq!(got[1], para("after"));
    let item = &got[0]["content"][0]["content"];
    assert!(
        item.as_array()
            .unwrap()
            .iter()
            .any(|b| b["type"] == "columnList")
    );
}

#[test]
fn annotations_apply_to_the_next_block_only() {
    // An image annotation before a paragraph is dropped, and does not leak to a later image.
    let got = md("<!-- atomik:{\"attrs\":{\"align\":\"left\"}} -->\n\ntext\n\n![](diagram.png)\n");
    assert_eq!(got[1]["type"], "image");
    assert!(got[1]["attrs"].get("align").is_none());
    // A callout annotation before a paragraph does not turn the next quote into a callout.
    let got = md(
        "<!-- atomik:{\"node\":\"callout\",\"attrs\":{\"icon\":\"💡\"}} -->\n\ntext\n\n> plain quote\n",
    );
    assert_eq!(got[1]["type"], "blockquote");
}

#[test]
fn nesting_is_flattened_beyond_the_budget() {
    let quotes = format!("{}deep text", "> ".repeat(500));
    let got = md(&quotes);
    assert!(node_depth(&got) <= 62);
    assert!(text_of(&got).contains("deep text"));
    let mut lists = String::new();
    for level in 0..300 {
        lists.push_str(&"  ".repeat(level));
        lists.push_str("- item\n");
    }
    let got = md(&lists);
    assert!(node_depth(&got) <= 62);
    assert_eq!(text_of(&got).matches("item").count(), 300);
    let details = format!(
        "{}x{}",
        "<details>\n<summary>s</summary>\n\n".repeat(100),
        "\n\n</details>".repeat(100)
    );
    let got = md(&details);
    assert!(node_depth(&got) <= 62);
}

#[test]
fn huge_tables_are_cut_and_reported() {
    let mut source = String::from("| A |\n| --- |\n");
    for n in 0..(crate::html::MAX_TABLE_ROWS + 20) {
        source.push_str(&format!("| {n} |\n"));
    }
    let (got, warnings) = read(&source);
    assert_eq!(
        got[0]["content"].as_array().map(Vec::len),
        Some(crate::html::MAX_TABLE_ROWS)
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.detail.as_deref() == Some("table rows"))
    );
}

#[test]
fn hostile_inputs_never_panic() {
    let samples = [
        "\u{0}\u{1}\u{2}",
        "[[",
        "]]",
        "[[]]",
        "![[",
        "<",
        "<!--",
        "<!-- atomik:",
        "<!-- atomik:{ -->",
        "<details>",
        "</details></details></aside>",
        "<summary>",
        "<aside><aside><aside>",
        "<!-- atomik:{\"node\":\"column\"} -->",
        "<!-- atomik:{\"end\":\"columnList\"} -->",
        "| a |\n|---|\n| <br><br><br> |",
        "[^1]",
        "[^1]: [^1]: [^1]:",
        "$$",
        "$$$$$$",
        "```",
        "~~~~\n",
        "> > > > > ",
        "1. 2. 3. - - -",
        "![](",
        "[](",
        "[a](<b c>)",
        "<time datetime=\"x\">",
        "<img src=\"\">",
        "<a href=\"\">x</a>",
    ];
    for sample in samples {
        let got = md(sample);
        check_shape(&got);
    }
    // Pseudo-random mixtures of markup characters.
    let alphabet: Vec<&str> = vec![
        "#",
        " ",
        "\n",
        "\n\n",
        "-",
        "*",
        "_",
        "`",
        "~",
        ">",
        "[",
        "]",
        "(",
        ")",
        "!",
        "<",
        ">",
        "|",
        ":",
        "$",
        "\\",
        "&",
        "1.",
        "- [ ]",
        "<br>",
        "<details>",
        "</details>",
        "<summary>",
        "</summary>",
        "<aside>",
        "</aside>",
        "<!-- atomik:{\"node\":\"columnList\"} -->",
        "<!-- atomik:{\"node\":\"column\"} -->",
        "<!-- atomik:{\"end\":\"columnList\"} -->",
        "<!-- atomik:{\"node\":\"callout\"} -->",
        "<u>",
        "</u>",
        "<time datetime=\"2026-10-07\">",
        "</time>",
        "a",
        "word",
        "💡",
        "x.md",
        "[[",
        "]]",
        "![",
        "](",
        "pic.png",
        "attachment://",
        "|---|",
        "\t",
        "    ",
    ];
    let mut state: u64 = 0x1234_5678_9abc_def0;
    for _ in 0..300 {
        let mut source = String::new();
        for _ in 0..60 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            source.push_str(
                alphabet[usize::try_from((state >> 33) % alphabet.len() as u64).unwrap()],
            );
        }
        let got = md(&source);
        check_shape(&got);
    }
}

fn text_of(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

/// Every node is one of the schema's, nothing carries an id, nesting is within the budget.
fn check_shape(blocks: &Value) {
    fn visit(node: &Value, depth: usize) {
        assert!(depth <= 62, "too deep");
        if let Some(items) = node.as_array() {
            for item in items {
                visit(item, depth);
            }
            return;
        }
        let name = node["type"].as_str().expect("a node has a type");
        assert!(
            kb_model::schema::NODE_NAMES.contains(&name),
            "unknown node {name}"
        );
        assert!(node["attrs"].get("id").is_none(), "no ids at plan time");
        if name == "text" {
            assert!(node["text"].is_string());
            return;
        }
        if let Some(children) = node.get("content") {
            visit(children, depth + 1);
        }
    }
    visit(blocks, 0);
}

#[test]
fn html_marks_and_links_do_not_outlive_their_paragraph() {
    let got = md(
        "<b>bold <i>both <u>all <a href=\"https://example.com\">linked\n\nnext paragraph\n\n# head <b>x\n\nlast",
    );
    let text_marks = |block: &Value| -> Vec<Vec<String>> {
        block["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                n.get("marks")
                    .and_then(Value::as_array)
                    .map(|m| {
                        m.iter()
                            .map(|x| x["type"].as_str().unwrap().to_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    assert!(!text_marks(&got[0]).iter().all(Vec::is_empty));
    assert_eq!(text_marks(&got[1]), vec![Vec::<String>::new()]);
    assert_eq!(got[2]["type"], "heading");
    assert_eq!(text_marks(&got[3]), vec![Vec::<String>::new()]);
    // A `</a>` that no `<a>` opened does not end a Markdown link.
    let got = md("[a *b* </a> c](https://example.com)");
    let content = got[0]["content"].as_array().unwrap();
    assert!(content.iter().all(|n| {
        n["marks"]
            .as_array()
            .is_some_and(|m| m.iter().any(|x| x["type"] == "link"))
    }));
}

#[test]
fn dollar_signs_that_are_not_equations_stay_text() {
    for text in [
        "costs $5 and $10 today",
        "$5-$10",
        "a $ b $ c",
        "price: $9.99",
        "pay $x to $ y",
    ] {
        let got = md(text);
        assert_eq!(
            got[0]["content"].as_array().map(Vec::len),
            Some(1),
            "{text}: {got}"
        );
        assert_eq!(got[0]["content"][0]["text"], text);
    }
    let got = md("an equation $a*b*c$ here");
    assert_eq!(
        got[0]["content"][1],
        json!({"type": "mathInline", "attrs": {"latex": "a*b*c"}})
    );
}

#[test]
fn a_heading_that_is_only_a_picture_is_the_picture() {
    let got = md("# ![logo](diagram.png)\n\n## Title ![](photo.jpg)\n\n#\n");
    assert_eq!(got[0]["type"], "image");
    assert_eq!(got[1]["type"], "heading");
    assert_eq!(got[2]["type"], "image");
    // A lone `#` stays an empty heading.
    assert_eq!(got[3], json!({"type": "heading", "attrs": {"level": 1}}));
}

#[test]
fn wiki_links_inside_the_page_keep_their_text() {
    let got = md("See [[#A heading]] and [[#^block|that]] and ![[#x]].");
    let text: String = got[0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["text"].as_str())
        .collect();
    assert_eq!(text, "See #A heading and that and .");
    assert!(
        got[0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["type"] == "text")
    );
}

// ----- template buttons (ADR-0056) ----------------------------------------------------------

#[test]
fn an_annotated_template_button_holds_the_blocks_up_to_its_end() {
    let got = md("before\n\n\
         <!-- atomik:{\"attrs\":{\"label\":\"Add task\",\"position\":\"above\"},\"node\":\"templateButton\"} -->\n\n\
         Task one\n\n- [ ] sub\n\n\
         <!-- atomik:{\"end\":\"templateButton\"} -->\n\nafter");
    assert_eq!(got[0], para("before"));
    assert_eq!(got[1]["type"], "templateButton");
    assert_eq!(
        got[1]["attrs"],
        json!({"label": "Add task", "position": "above"})
    );
    assert_eq!(got[1]["content"][0], para("Task one"));
    assert_eq!(got[1]["content"][1]["type"], "taskList");
    assert_eq!(got[1]["content"].as_array().map(Vec::len), Some(2));
    assert_eq!(got[2], para("after"));
    assert_eq!(got.as_array().map(Vec::len), Some(3));
}

#[test]
fn a_template_button_without_attributes_or_content_is_an_empty_one() {
    let got = md(
        "<!-- atomik:{\"node\":\"templateButton\"} -->\n\n<!-- atomik:{\"end\":\"templateButton\"} -->\n\nnext",
    );
    assert_eq!(got[0]["type"], "templateButton");
    assert!(got[0].get("attrs").is_none() || got[0]["attrs"] == json!({}));
    // The schema wants at least one block in it.
    assert_eq!(got[0]["content"].as_array().map(Vec::len), Some(1));
    assert_eq!(got[0]["content"][0]["type"], "paragraph");
    assert_eq!(got[1], para("next"));
}

#[test]
fn a_template_button_that_never_closes_ends_with_the_file() {
    let got = md("<!-- atomik:{\"node\":\"templateButton\"} -->\n\nonly this\n");
    assert_eq!(got[0]["type"], "templateButton");
    assert_eq!(got[0]["content"][0], para("only this"));
    assert_eq!(got.as_array().map(Vec::len), Some(1));
}

#[test]
fn a_template_button_keeps_the_annotated_blocks_inside_it() {
    let got = md("<!-- atomik:{\"node\":\"templateButton\"} -->\n\n\
         <!-- atomik:{\"attrs\":{\"icon\":\"💡\"},\"node\":\"callout\"} -->\n\n> 💡 Note\n\n\
         <!-- atomik:{\"node\":\"columnList\"} -->\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nL\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nR\n\n<!-- atomik:{\"end\":\"columnList\"} -->\n\n\
         <!-- atomik:{\"end\":\"templateButton\"} -->\n\nafter");
    assert_eq!(got[0]["type"], "templateButton");
    assert_eq!(got[0]["content"][0]["type"], "callout");
    assert_eq!(got[0]["content"][1]["type"], "columnList");
    assert_eq!(got[1], para("after"));
}

#[test]
fn an_oversized_label_or_a_bad_position_is_cut_down() {
    let label = "x".repeat(200);
    let got = md(&format!(
        "<!-- atomik:{{\"attrs\":{{\"label\":\"{label}\",\"position\":\"middle\"}},\"node\":\"templateButton\"}} -->\n\nbody\n\n<!-- atomik:{{\"end\":\"templateButton\"}} -->\n"
    ));
    assert_eq!(got[0]["attrs"]["label"].as_str().map(str::len), Some(80));
    assert!(got[0]["attrs"].get("position").is_none());
}

#[test]
fn a_time_element_reads_its_relative_attribute() {
    let got = md(
        "due <time datetime=\"2026-10-18\" data-reminder=\"30\" data-relative=\"today\">2026-10-18</time> and \
         <time datetime=\"2026-10-18T09:30\" data-relative=\"now\">x</time> and \
         <time datetime=\"2026-10-18\" data-relative=\"yesterday\">y</time>",
    );
    let mentions: Vec<&Value> = got[0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["type"] == "dateMention")
        .collect();
    assert_eq!(mentions.len(), 3);
    assert_eq!(mentions[0]["attrs"]["relative"], "today");
    assert_eq!(mentions[0]["attrs"]["reminder"], 30);
    assert_eq!(mentions[1]["attrs"]["relative"], "now");
    assert_eq!(mentions[1]["attrs"]["time"], "09:30");
    assert!(mentions[2]["attrs"].get("relative").is_none());
}
