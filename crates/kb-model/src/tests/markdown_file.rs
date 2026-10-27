//! The file flavour of the Markdown renderer (`render_markdown_file`, ADR-0036,
//! `docs/architecture/import-export.md`, 2.3), rule by rule. The golden of the whole sample is
//! `tests/page_file_golden.rs`; the clipboard flavour has its own tests and golden.

use std::collections::HashMap;

use kb_doc::{Doc, DocKind};
use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{AttachmentId, BlockId, OptionId, PageId, PropertyId};
use serde_json::{Value, json};

use crate::{ANNOTATION_CLOSE, ANNOTATION_OPEN, ExportLinks, PageDoc, render_markdown_file};

// ----- helpers ------------------------------------------------------------------------------

fn page_id(n: u8) -> PageId {
    PageId::from_random_bytes([n; 16])
}

fn attachment(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([n; 16])
}

fn block(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn property(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([n; 16])
}

fn option(n: u8) -> OptionId {
    OptionId::from_random_bytes([n; 16])
}

/// What the test export knows; everything else is unknown or outside it.
#[derive(Default)]
struct Links {
    /// Page: title, and its href when it is in the export.
    pages: HashMap<PageId, (String, Option<String>)>,
    /// Attachment: name, and its href when it is in the archive.
    attachments: HashMap<AttachmentId, (String, Option<String>)>,
    whiteboards: HashMap<BlockId, String>,
    properties: HashMap<PropertyId, String>,
    options: HashMap<(PropertyId, OptionId), String>,
}

impl ExportLinks for Links {
    fn page_href(&self, page: PageId) -> Option<String> {
        self.pages.get(&page)?.1.clone()
    }

    fn page_title(&self, page: PageId) -> Option<String> {
        Some(self.pages.get(&page)?.0.clone())
    }

    fn attachment_href(&self, attachment: AttachmentId) -> Option<String> {
        self.attachments.get(&attachment)?.1.clone()
    }

    fn attachment_name(&self, attachment: AttachmentId) -> Option<String> {
        Some(self.attachments.get(&attachment)?.0.clone())
    }

    fn whiteboard_href(&self, block: BlockId) -> Option<String> {
        self.whiteboards.get(&block).cloned()
    }

    fn property_name(&self, property: PropertyId) -> Option<String> {
        self.properties.get(&property).cloned()
    }

    fn option_name(&self, property: PropertyId, option: OptionId) -> Option<String> {
        self.options.get(&(property, option)).cloned()
    }
}

/// The export most tests use: page 1 "Alpha" is in it (`Alpha.md`), page 2 "Beta" is not;
/// attachment 1 is `dir/diagram.png`, 2 a file named `my [file].pdf` (`dir/my%20file.pdf`),
/// 3 is named `gone.bin` but is not in the archive; block 1 is a whiteboard; property 10
/// "Status" has option 1 "Done"; property 11 "Estimate"; Tags has option 5 "work".
fn links() -> Links {
    let mut links = Links::default();
    links.pages.insert(
        page_id(1),
        ("Alpha".to_owned(), Some("Alpha.md".to_owned())),
    );
    links.pages.insert(page_id(2), ("Beta".to_owned(), None));
    links.pages.insert(
        page_id(4),
        ("A [b] *c*".to_owned(), Some("A%20%5Bb%5D.md".to_owned())),
    );
    links.attachments.insert(
        attachment(1),
        ("diagram.png".to_owned(), Some("dir/diagram.png".to_owned())),
    );
    links.attachments.insert(
        attachment(2),
        (
            "my [file].pdf".to_owned(),
            Some("dir/my%20file.pdf".to_owned()),
        ),
    );
    links
        .attachments
        .insert(attachment(3), ("gone.bin".to_owned(), None));
    links
        .whiteboards
        .insert(block(1), "dir/Whiteboard.excalidraw".to_owned());
    links.properties.insert(TAGS_PROPERTY_ID, "Tags".to_owned());
    links.properties.insert(property(10), "Status".to_owned());
    links.properties.insert(property(11), "Estimate".to_owned());
    links
        .options
        .insert((property(10), option(1)), "Done".to_owned());
    links
        .options
        .insert((TAGS_PROPERTY_ID, option(5)), "work".to_owned());
    links
}

fn doc(blocks: Value) -> Value {
    json!({ "type": "doc", "content": blocks })
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn marked(text: &str, marks: Value) -> Value {
    json!({ "type": "text", "text": text, "marks": marks })
}

fn para(content: Value) -> Value {
    json!({ "type": "paragraph", "content": content })
}

fn p(words: &str) -> Value {
    para(json!([text(words)]))
}

fn render(blocks: Value) -> String {
    render_markdown_file(&doc(blocks), &links())
}

/// The text of one paragraph.
fn line(words: &str) -> String {
    render(json!([p(words)]))
}

/// The JSON of every annotation line of `markdown`, in order, parsed back.
fn annotations(markdown: &str) -> Vec<Value> {
    markdown
        .lines()
        .filter_map(|line| {
            let line = line.trim_start_matches(['>', ' ']);
            let json = line
                .strip_prefix(ANNOTATION_OPEN)?
                .strip_suffix(ANNOTATION_CLOSE)?;
            Some(serde_json::from_str(json).expect("an annotation is JSON"))
        })
        .collect()
}

fn attachment_ref(n: u8) -> String {
    attachment(n).to_string()
}

// ----- text ---------------------------------------------------------------------------------

#[test]
fn every_special_character_of_a_text_run_is_escaped() {
    assert_eq!(
        line(r"a\`*_[]<>$~|&b"),
        "a\\\\\\`\\*\\_\\[\\]\\<\\>\\$\\~\\|\\&b\n"
    );
}

#[test]
fn plain_text_is_not_touched() {
    assert_eq!(
        line("Hello, world! (1) #1 a-b = c + d."),
        "Hello, world! (1) #1 a-b = c + d.\n"
    );
    assert_eq!(line("2026-10-07 and 3.5"), "2026-10-07 and 3.5\n");
    assert_eq!(line("café 日本語 👩‍💻"), "café 日本語 👩‍💻\n");
}

#[test]
fn the_start_of_a_line_escapes_what_would_open_a_block() {
    assert_eq!(line("# not a heading"), "\\# not a heading\n");
    assert_eq!(line("#hashtag"), "\\#hashtag\n");
    assert_eq!(line("+ not a list"), "\\+ not a list\n");
    assert_eq!(line("- not a list"), "\\- not a list\n");
    assert_eq!(line("= not a setext line"), "\\= not a setext line\n");
    assert_eq!(line("1. not a list"), "1\\. not a list\n");
    assert_eq!(line("12) not a list"), "12\\) not a list\n");
    assert_eq!(line("1.5 apples"), "1\\.5 apples\n");
    assert_eq!(line("  - indented"), "  \\- indented\n");
    // Not at the start of a line: nothing to escape.
    assert_eq!(line("a - b # c = d + e 1. f"), "a - b # c = d + e 1. f\n");
    assert_eq!(line("1 2. 3"), "1 2. 3\n");
}

#[test]
fn a_hard_break_starts_a_new_line() {
    let hard = json!({ "type": "hardBreak" });
    assert_eq!(
        render(json!([para(json!([text("a"), hard, text("- b")]))])),
        "a  \n\\- b\n"
    );
    // A line break inside a text node does too.
    assert_eq!(line("a\n# b"), "a\n\\# b\n");
}

#[test]
fn list_and_quote_text_starts_a_line() {
    let list = json!({
        "type": "bulletList",
        "content": [{ "type": "listItem", "content": [p("- x")] }]
    });
    assert_eq!(render(json!([list])), "- \\- x\n");
    let ordered = json!({
        "type": "orderedList",
        "content": [{ "type": "listItem", "content": [p("1. x")] }]
    });
    assert_eq!(render(json!([ordered])), "1. 1\\. x\n");
    let quote = json!({ "type": "blockquote", "content": [p("> y"), p("# z")] });
    assert_eq!(render(json!([quote])), "> \\> y\n>\n> \\# z\n");
}

#[test]
fn marks_that_write_characters_come_before_the_text_on_the_line() {
    let bold = para(json!([marked("- x", json!([{ "type": "bold" }]))]));
    assert_eq!(render(json!([bold])), "**- x**\n");
    let link = para(json!([marked(
        "1. x",
        json!([{ "type": "link", "attrs": { "href": "https://a.example" } }])
    )]));
    assert_eq!(render(json!([link])), "[1. x](https://a.example)\n");
    // A mark that writes nothing does not hide the start of the line.
    let red = para(json!([marked(
        "- x",
        json!([{ "type": "textColor", "attrs": { "color": "red" } }])
    )]));
    assert_eq!(render(json!([red])), "\\- x\n");
}

#[test]
fn a_heading_does_not_start_a_line_of_its_own() {
    let heading =
        json!({ "type": "heading", "attrs": { "level": 2 }, "content": [text("# - 1. x")] });
    assert_eq!(render(json!([heading])), "## # - 1. x\n");
}

#[test]
fn the_hashes_at_the_end_of_a_heading_are_not_a_closing_sequence() {
    let heading = |words: &str| {
        render(json!([{ "type": "heading", "attrs": { "level": 2 }, "content": [text(words)] }]))
    };
    assert_eq!(heading("Issue #"), "## Issue \\#\n");
    assert_eq!(heading("Issue ##"), "## Issue \\#\\#\n");
    assert_eq!(heading("#"), "## \\#\n");
    // `C#` has no space before it, but is escaped all the same.
    assert_eq!(heading("C#"), "## C\\#\n");
    // Not at the end: nothing to escape.
    assert_eq!(
        heading("# In the middle # of it"),
        "## # In the middle # of it\n"
    );
    assert_eq!(heading("Plain"), "## Plain\n");
    // Marks around it do not matter: the text ends with the closing mark.
    let bold = json!([{ "type": "heading", "attrs": { "level": 1 }, "content": [marked("x #", json!([{ "type": "bold" }]))] }]);
    assert_eq!(render(bold), "# **x #**\n");
}

#[test]
fn four_columns_of_indentation_would_be_code() {
    // Less than four: the white space stays.
    assert_eq!(line("   - x"), "   \\- x\n");
    assert_eq!(line("   x"), "   x\n");
    // Four or more: the first character is a reference, so the line is text.
    assert_eq!(line("    x"), "&#32;   x\n");
    assert_eq!(line("      x"), "&#32;     x\n");
    assert_eq!(line("\tx"), "&#9;x\n");
    assert_eq!(line("  \tx"), "&#32; \tx\n");
    // After a line break in the text, too; and not in the middle of a line.
    assert_eq!(line("a\n    b"), "a\n&#32;   b\n");
    assert_eq!(line("a    b"), "a    b\n");
    // What follows the reference is no longer at the start of a line.
    assert_eq!(line("    - x"), "&#32;   - x\n");
}

#[test]
fn marks_wrap_escaped_text() {
    let styled = para(json!([
        marked("a*b", json!([{ "type": "bold" }])),
        marked("c_d", json!([{ "type": "italic" }])),
        marked("e~f", json!([{ "type": "strike" }])),
        marked("g<h", json!([{ "type": "underline" }])),
    ]));
    assert_eq!(
        render(json!([styled])),
        "**a\\*b***c\\_d*~~e\\~f~~<u>g\\<h</u>\n"
    );
}

#[test]
fn code_is_not_escaped_and_its_fence_fits_it() {
    let code = |text: &str| {
        render(json!([para(json!([marked(
            text,
            json!([{ "type": "code" }])
        )]))]))
    };
    assert_eq!(code("a*b_c [d]"), "`a*b_c [d]`\n");
    assert_eq!(code("a`b"), "``a`b``\n");
    assert_eq!(code("a``b"), "```a``b```\n");
    assert_eq!(code("`x"), "`` `x ``\n");
    assert_eq!(code("x`"), "`` x` ``\n");
    assert_eq!(code(" x "), "`  x  `\n");
    assert_eq!(code("  "), "`  `\n");
}

#[test]
fn code_blocks_keep_their_text_and_get_a_safe_fence() {
    let block_of = |language: Option<&str>, code: &str| {
        let attrs = language.map_or(json!({}), |language| json!({ "language": language }));
        render(json!([{ "type": "codeBlock", "attrs": attrs, "content": [text(code)] }]))
    };
    assert_eq!(
        block_of(Some("rust"), "a*b\n# c"),
        "```rust\na*b\n# c\n```\n"
    );
    assert_eq!(block_of(None, "x"), "```\nx\n```\n");
    assert_eq!(block_of(None, "a\n```\nb"), "````\na\n```\nb\n````\n");
    assert_eq!(block_of(None, "`````"), "``````\n`````\n``````\n");
    // The info string cannot end the fence line or open a second one.
    assert_eq!(block_of(Some("c++ `x`"), "y"), "```c++x\ny\n```\n");
}

#[test]
fn math_is_not_escaped() {
    let inline = para(json!([
        text("see "),
        { "type": "mathInline", "attrs": { "latex": "a_b*c|d" } }
    ]));
    assert_eq!(render(json!([inline])), "see $a_b*c|d$\n");
    let display = json!({ "type": "mathBlock", "content": [text("x_1 * y")] });
    assert_eq!(render(json!([display])), "$$\nx_1 * y\n$$\n");
}

#[test]
fn a_person_mention_is_at_and_its_label_as_text() {
    let account = "0f0f0f0f-0f0f-0f0f-0f0f-0f0f0f0f0f0f";
    let mention = |label: &str| {
        json!({
            "type": "personMention",
            "attrs": { "accountId": account, "label": label }
        })
    };
    // The label is text, so it is escaped like text: it cannot open emphasis, a link or a table
    // cell in the exported file, and the account id is never written.
    assert_eq!(
        render(json!([para(json!([text("cc "), mention("Ada L.")]))])),
        "cc @Ada L.\n"
    );
    assert_eq!(
        render(json!([para(json!([mention("*Ada* [x](y) _z_ | <b>")]))])),
        "@\\*Ada\\* \\[x\\](y) \\_z\\_ \\| \\<b\\>\n"
    );
    // A line break or a control character in a self-asserted name stays on its line.
    assert_eq!(
        render(json!([para(json!([mention("Ada\n# not a heading")]))])),
        "@Ada # not a heading\n"
    );
    // No label: nothing, as the core's plain Markdown writes it.
    let unnamed = json!({ "type": "personMention", "attrs": { "accountId": account } });
    assert_eq!(
        render(json!([para(json!([text("a"), unnamed, text("b")]))])),
        "ab\n"
    );
    // In a toggle's summary, which is HTML there.
    let toggle = json!({
        "type": "details",
        "content": [
            { "type": "detailsSummary", "content": [text("Owner "), mention("A&B <c>")] },
            { "type": "detailsContent", "content": [p("body")] }
        ]
    });
    let rendered = render(json!([toggle]));
    assert!(
        rendered.contains("<summary>Owner @A&amp;B &lt;c&gt;</summary>"),
        "{rendered}"
    );
}

#[test]
fn a_link_destination_is_written_so_that_it_ends_where_it_should() {
    let link = |label: &str, href: &str| {
        render(json!([para(json!([marked(
            label,
            json!([{ "type": "link", "attrs": { "href": href } }])
        )]))]))
    };
    assert_eq!(
        link("see", "https://a.example/x"),
        "[see](https://a.example/x)\n"
    );
    // The label is text: escaped. The destination is not.
    assert_eq!(
        link("a[b]_c", "https://a.example/a_b"),
        "[a\\[b\\]\\_c](https://a.example/a_b)\n"
    );
    assert_eq!(
        link("see", "https://a.example/a_(b)"),
        "[see](https://a.example/a_(b))\n"
    );
    assert_eq!(
        link("see", "https://a.example/x y"),
        "[see](<https://a.example/x y>)\n"
    );
    assert_eq!(
        link("see", "https://a.example/a)b"),
        "[see](<https://a.example/a)b>)\n"
    );
    assert_eq!(
        link("see", "https://a.example/<b>"),
        "[see](<https://a.example/\\<b\\>>)\n"
    );
    assert_eq!(link("see", ""), "[see]()\n");
}

// ----- links --------------------------------------------------------------------------------

fn page_ref(id: PageId) -> Value {
    json!({ "type": "pageRef", "attrs": { "pageId": id.to_string() } })
}

#[test]
fn a_page_reference_is_a_link_when_the_page_is_in_the_export() {
    let inside = para(json!([text("see "), page_ref(page_id(1)), text(" now")]));
    assert_eq!(render(json!([inside])), "see [Alpha](Alpha.md) now\n");
    // Outside the export: the wiki link as in the clipboard flavour.
    let outside = para(json!([page_ref(page_id(2))]));
    assert_eq!(render(json!([outside])), "[[Beta]]\n");
    // A page nobody knows: its stored id.
    let unknown = para(json!([page_ref(page_id(9))]));
    assert_eq!(render(json!([unknown])), format!("[[{}]]\n", page_id(9)));
    // The title is text.
    let odd = para(json!([page_ref(page_id(4))]));
    assert_eq!(
        render(json!([odd])),
        "[A \\[b\\] \\*c\\*](A%20%5Bb%5D.md)\n"
    );
}

#[test]
fn a_page_card_is_the_same_link_alone() {
    let card = |id: PageId| json!({ "type": "pageLink", "attrs": { "pageId": id.to_string(), "blockId": block(3).to_string() } });
    assert_eq!(
        render(json!([p("before"), card(page_id(1)), p("after")])),
        "before\n\n[Alpha](Alpha.md)\n\nafter\n"
    );
    assert_eq!(render(json!([card(page_id(2))])), "[[Beta]]\n");
}

fn image(attrs: Value) -> Value {
    json!({ "type": "image", "attrs": attrs })
}

#[test]
fn an_image_links_its_file_or_stays_an_attachment_link() {
    let alt = "a [b] \"c\" \\d";
    let with_file = image(
        json!({ "attachmentId": attachment_ref(1), "alt": alt, "caption": "Fig \"1\"\nsecond" }),
    );
    assert_eq!(
        render(json!([with_file])),
        "![a \\[b\\] \\\"c\\\" \\\\d](dir/diagram.png \"Fig \\\"1\\\" second\")\n"
    );
    // Not in the archive (unavailable) or unknown: the link stays `attachment://`.
    for n in [3, 9] {
        let id = attachment_ref(n);
        assert_eq!(
            render(json!([image(json!({ "attachmentId": id, "alt": "x" }))])),
            format!("![x](attachment://{id})\n")
        );
    }
    // Nothing stored.
    assert_eq!(render(json!([image(json!({}))])), "![](attachment://)\n");
    // An id that is no id cannot break out of the link.
    assert_eq!(
        render(json!([image(json!({ "attachmentId": "a b)c" }))])),
        "![](<attachment://a b)c>)\n"
    );
}

#[test]
fn a_file_links_its_name_or_the_attachment() {
    let file = |n: u8| json!({ "type": "file", "attrs": { "attachmentId": attachment_ref(n) } });
    assert_eq!(
        render(json!([file(2)])),
        "[my \\[file\\].pdf](dir/my%20file.pdf)\n"
    );
    // In the manifest but not in the archive.
    assert_eq!(
        render(json!([file(3)])),
        format!("[gone.bin](attachment://{})\n", attachment_ref(3))
    );
    // Unknown: the id is the name.
    assert_eq!(
        render(json!([file(9)])),
        format!("[{id}](attachment://{id})\n", id = attachment_ref(9))
    );
}

#[test]
fn a_gallery_is_its_image_lines_and_skips_the_unusable() {
    let gallery = json!({
        "type": "gallery",
        "attrs": {
            "columns": 2,
            "items": [
                { "attachmentId": attachment_ref(1), "caption": "One \"1\"" },
                "not an item",
                { "caption": "no id" },
                { "attachmentId": "" },
                { "attachmentId": attachment_ref(3) }
            ]
        }
    });
    let out = render(json!([gallery]));
    assert_eq!(
        out,
        format!(
            "{}\n\n![](dir/diagram.png \"One \\\"1\\\"\")\n\n![](attachment://{})\n",
            line_of_annotation(&out),
            attachment_ref(3)
        )
    );
    assert_eq!(
        annotations(&out),
        vec![json!({ "node": "gallery", "attrs": { "columns": 2 }, "items": 2 })]
    );
    // Nothing usable: nothing, annotation included.
    let empty = json!({ "type": "gallery", "attrs": { "items": [{ "caption": "x" }] } });
    assert_eq!(render(json!([empty])), "\n");
    let missing = json!({ "type": "gallery" });
    assert_eq!(render(json!([missing])), "\n");
}

/// The first annotation line of `markdown`.
fn line_of_annotation(markdown: &str) -> &str {
    markdown
        .lines()
        .find(|line| line.starts_with(ANNOTATION_OPEN))
        .expect("an annotation line")
}

// ----- dates --------------------------------------------------------------------------------

fn date(attrs: Value) -> Value {
    json!({ "type": "dateMention", "attrs": attrs })
}

#[test]
fn a_date_mention_is_a_time_element() {
    let out = |attrs: Value| render(json!([para(json!([text("on "), date(attrs)]))]));
    assert_eq!(
        out(json!({ "date": "2026-10-07" })),
        "on <time datetime=\"2026-10-07\">2026-10-07</time>\n"
    );
    assert_eq!(
        out(json!({ "date": "2026-10-07", "time": "14:30" })),
        "on <time datetime=\"2026-10-07T14:30\">2026-10-07 14:30</time>\n"
    );
    assert_eq!(
        out(json!({ "date": "2026-10-07", "time": "14:30", "reminder": 15 })),
        "on <time datetime=\"2026-10-07T14:30\" data-reminder=\"15\">2026-10-07 14:30</time>\n"
    );
    assert_eq!(
        out(json!({ "date": "2026-10-07", "reminder": 0 })),
        "on <time datetime=\"2026-10-07\" data-reminder=\"0\">2026-10-07</time>\n"
    );
    // A whole number stored as a float is the same number.
    assert_eq!(
        out(json!({ "date": "2026-10-07", "reminder": 1440.0 })),
        "on <time datetime=\"2026-10-07\" data-reminder=\"1440\">2026-10-07</time>\n"
    );
}

#[test]
fn a_damaged_date_mention_loses_what_is_damaged() {
    let out = |attrs: Value| render(json!([para(json!([text("on "), date(attrs)]))]));
    // No valid date: nothing at all.
    assert_eq!(out(json!({ "date": "2026-02-30" })), "on \n");
    assert_eq!(out(json!({})), "on \n");
    // A malformed time and a damaged reminder are left out.
    assert_eq!(
        out(json!({ "date": "2026-10-07", "time": "25:99", "reminder": -5 })),
        "on <time datetime=\"2026-10-07\">2026-10-07</time>\n"
    );
    assert_eq!(
        out(json!({ "date": "2026-10-07", "reminder": 1.5 })),
        "on <time datetime=\"2026-10-07\">2026-10-07</time>\n"
    );
}

// ----- annotations: the line itself -----------------------------------------------------------

#[test]
fn an_annotation_is_compact_json_with_sorted_keys() {
    let callout = json!({ "type": "callout", "attrs": { "icon": "🔥", "color": "red" }, "content": [p("x")] });
    let out = render(json!([callout]));
    assert_eq!(
        out,
        "<!-- atomik:{\"attrs\":{\"color\":\"red\",\"icon\":\"🔥\"},\"node\":\"callout\"} -->\n\n> 🔥 x\n"
    );
}

#[test]
fn an_annotation_cannot_close_its_comment() {
    // Names the annotation carries (a property name here) may hold any text.
    let mut links = links();
    links
        .properties
        .insert(property(12), "a--b ---c --> <!-- d".to_owned());
    let list = json!({
        "type": "pageList",
        "attrs": { "properties": [property(12).to_string()] }
    });
    let out = render_markdown_file(&doc(json!([list])), &links);
    let comment = line_of_annotation(&out);
    let body = comment
        .strip_prefix(ANNOTATION_OPEN)
        .and_then(|rest| rest.strip_suffix(ANNOTATION_CLOSE))
        .unwrap();
    assert!(!body.contains("--"), "{body}");
    assert!(!body.contains("-->"), "{body}");
    assert!(body.contains("-\\u002d"), "{body}");
    // Read back, it is the name as it was.
    assert_eq!(
        annotations(&out),
        vec![json!({ "node": "pageList", "attrs": { "properties": ["a--b ---c --> <!-- d"] } })]
    );
}

// ----- annotations: blocks --------------------------------------------------------------------

#[test]
fn an_image_annotation_carries_the_layout_that_is_set() {
    let id = attachment_ref(1);
    let none = render(json!([image(json!({ "attachmentId": id }))]));
    assert_eq!(none, "![](dir/diagram.png)\n");
    assert!(annotations(&none).is_empty());

    let all = render(json!([image(json!({
        "attachmentId": id, "width": 640, "widthPercent": 60, "align": "right"
    }))]));
    assert_eq!(
        annotations(&all),
        vec![json!({ "attrs": { "align": "right", "width": 640, "widthPercent": 60 } })]
    );
    assert_eq!(
        all,
        "<!-- atomik:{\"attrs\":{\"align\":\"right\",\"width\":640,\"widthPercent\":60}} -->\n\n![](dir/diagram.png)\n"
    );

    // Only what is set: no empty strings, no nulls.
    let some = render(json!([image(json!({
        "attachmentId": id, "widthPercent": 50, "align": "", "caption": "c", "width": null
    }))]));
    assert_eq!(
        annotations(&some),
        vec![json!({ "attrs": { "widthPercent": 50 } })]
    );
}

#[test]
fn a_file_annotation_carries_its_view() {
    let file = |attrs: Value| render(json!([{ "type": "file", "attrs": attrs }]));
    let id = attachment_ref(2);
    assert!(annotations(&file(json!({ "attachmentId": id }))).is_empty());
    assert_eq!(
        annotations(&file(json!({ "attachmentId": id, "view": "preview" }))),
        vec![json!({ "attrs": { "view": "preview" } })]
    );
}

#[test]
fn every_callout_has_an_annotation() {
    let callout =
        |attrs: Value| json!({ "type": "callout", "attrs": attrs, "content": [p("Body")] });
    let out = render(json!([callout(json!({ "icon": "💡", "color": "blue" }))]));
    assert_eq!(
        out,
        "<!-- atomik:{\"attrs\":{\"color\":\"blue\",\"icon\":\"💡\"},\"node\":\"callout\"} -->\n\n> 💡 Body\n"
    );
    // The icon is always there (the default when unset); the colour only when set.
    for attrs in [json!({}), json!({ "icon": "" }), json!({ "color": "" })] {
        let out = render(json!([callout(attrs)]));
        assert_eq!(
            out,
            "<!-- atomik:{\"attrs\":{\"icon\":\"💡\"},\"node\":\"callout\"} -->\n\n> 💡 Body\n"
        );
    }
    // Two callouts, a callout in a callout: each one has its own.
    let nested = json!({ "type": "callout", "attrs": { "icon": "A" }, "content": [p("x"), callout(json!({ "icon": "B" }))] });
    let out = render(json!([nested]));
    assert_eq!(
        annotations(&out),
        vec![
            json!({ "node": "callout", "attrs": { "icon": "A" } }),
            json!({ "node": "callout", "attrs": { "icon": "B" } }),
        ]
    );
    assert!(out.contains("> <!-- atomik:"), "inside the quote: {out}");
}

#[test]
fn a_whiteboard_annotation_points_at_its_file() {
    let board = |id: BlockId, height: Value| json!({ "type": "whiteboard", "attrs": { "id": id.to_string(), "height": height } });
    let page = json!({
        "type": "doc",
        "content": [board(block(1), json!(480))],
        "whiteboards": {
            block(1).to_string(): {
                "a": { "id": "a", "type": "text", "index": "a1", "isDeleted": false, "originalText": "# Plan\n- one" },
                "b": { "id": "b", "type": "rectangle", "index": "a0", "isDeleted": false },
                "c": { "id": "c", "type": "text", "index": "a2", "isDeleted": true, "originalText": "gone" }
            }
        }
    });
    let out = render_markdown_file(&page, &links());
    assert_eq!(
        out,
        "<!-- atomik:{\"attrs\":{\"height\":480},\"file\":\"dir/Whiteboard.excalidraw\",\"node\":\"whiteboard\"} -->\n\n> **Whiteboard**\n>\n> - \\# Plan - one\n"
    );
    // No height set, and no file for the block: the annotation still marks the whiteboard.
    let other = json!({ "type": "doc", "content": [{ "type": "whiteboard", "attrs": { "id": block(2).to_string() } }] });
    let out = render_markdown_file(&other, &links());
    assert_eq!(annotations(&out), vec![json!({ "node": "whiteboard" })]);
    assert!(out.ends_with("> **Whiteboard**\n"), "{out}");
}

#[test]
fn columns_are_marked_and_stay_linearised() {
    let columns = json!({
        "type": "columnList",
        "content": [
            { "type": "column", "content": [p("Left")] },
            { "type": "column", "content": [p("Right"), p("Two")] },
        ]
    });
    assert_eq!(
        render(json!([p("before"), columns, p("after")])),
        "before\n\n\
         <!-- atomik:{\"node\":\"columnList\"} -->\n\n\
         <!-- atomik:{\"node\":\"column\"} -->\n\nLeft\n\n\
         <!-- atomik:{\"node\":\"column\"} -->\n\nRight\n\nTwo\n\n\
         <!-- atomik:{\"end\":\"columnList\"} -->\n\nafter\n"
    );
    // An empty column is its annotation alone.
    let empty = json!({
        "type": "columnList",
        "content": [
            { "type": "column", "content": [] },
            { "type": "column", "content": [p("x")] },
        ]
    });
    assert_eq!(
        annotations(&render(json!([empty]))),
        vec![
            json!({ "node": "columnList" }),
            json!({ "node": "column" }),
            json!({ "node": "column" }),
            json!({ "end": "columnList" }),
        ]
    );
}

#[test]
fn derived_blocks_are_annotations() {
    let out = render(json!([
        p("a"),
        { "type": "tableOfContents" },
        { "type": "breadcrumb" },
        { "type": "pageList" },
        p("b"),
    ]));
    assert_eq!(
        out,
        "a\n\n<!-- atomik:{\"node\":\"tableOfContents\"} -->\n\n<!-- atomik:{\"node\":\"breadcrumb\"} -->\n\n<!-- atomik:{\"attrs\":{},\"node\":\"pageList\"} -->\n\nb\n"
    );
}

#[test]
fn a_page_list_names_what_the_schema_names() {
    let list = json!({
        "type": "pageList",
        "attrs": {
            "id": block(7).to_string(),
            "source": { "kind": "tag", "optionId": option(5).to_string() },
            "layout": "board",
            "properties": [TAGS_PROPERTY_ID.to_string(), property(10).to_string(), property(99).to_string(), "x"],
            "sort": { "by": property(10).to_string(), "dir": "asc" },
            "filters": [
                { "propertyId": property(10).to_string(), "op": "hasOption", "value": option(1).to_string() },
                { "propertyId": property(10).to_string(), "op": "hasOption", "value": option(98).to_string() },
                { "propertyId": property(10).to_string(), "op": "isSet" },
                { "propertyId": property(99).to_string(), "op": "isSet" },
                { "propertyId": property(11).to_string(), "op": "equals", "value": 3 },
                { "propertyId": property(11).to_string(), "op": "contains", "value": option(1).to_string() },
                "not a filter"
            ],
            "columnWidths": { "title": 300, property(10).to_string(): 150, property(99).to_string(): 200 },
            "groupBy": property(10).to_string(),
            "hiddenGroups": [option(1).to_string(), "none", option(98).to_string()],
            "calculations": {
                "title": "count_all",
                property(10).to_string(): "count_unique",
                property(11).to_string(): "a-function-of-a-newer-version",
                property(99).to_string(): "sum",
                property(12).to_string(): 3
            }
        }
    });
    let out = render(json!([list]));
    assert_eq!(
        annotations(&out),
        vec![json!({
            "node": "pageList",
            "attrs": {
                "source": { "kind": "tag", "option": "work" },
                "layout": "board",
                "properties": ["Tags", "Status"],
                "sort": { "by": "Status", "dir": "asc" },
                "filters": [
                    { "property": "Status", "op": "hasOption", "value": "Done" },
                    { "property": "Status", "op": "isSet" },
                    { "property": "Estimate", "op": "equals", "value": 3 },
                    { "property": "Estimate", "op": "contains", "value": option(1).to_string() }
                ],
                "columnWidths": { "title": 300, "Status": 150 },
                "groupBy": "Status",
                "hiddenGroups": ["Done", "none"],
                "calculations": {
                    "title": "count_all",
                    "Status": "count_unique",
                    "Estimate": "a-function-of-a-newer-version"
                }
            }
        })]
    );
    // No id of the schema is left, and the block's own id is never written.
    for id in [
        property(10).to_string(),
        property(11).to_string(),
        block(7).to_string(),
        option(5).to_string(),
    ] {
        assert!(!out.contains(&id), "{id} in {out}");
    }
    assert!(
        out.contains("\"attrs\":{\"calculations\""),
        "keys are sorted: {out}"
    );
}

#[test]
fn a_page_list_drops_what_cannot_be_named() {
    let list = |attrs: Value| render(json!([{ "type": "pageList", "attrs": attrs }]));
    // A tag nobody names, a sort by a property that is gone, a group by one: dropped.
    let out = list(json!({
        "source": { "kind": "tag", "optionId": option(77).to_string() },
        "sort": { "by": property(99).to_string(), "dir": "desc" },
        "groupBy": property(99).to_string(),
        "hiddenGroups": [option(1).to_string(), "none"],
        "layout": "table"
    }));
    assert_eq!(
        annotations(&out),
        vec![
            json!({ "node": "pageList", "attrs": { "layout": "table", "hiddenGroups": ["none"] } })
        ]
    );
    // Built-in sorts stay, a children source stays, null attributes are never written, and
    // what a later version adds is kept as it is.
    let out = list(json!({
        "source": { "kind": "children" },
        "sort": { "by": "updated", "dir": "desc" },
        "filters": null,
        "columnWidths": null,
        "calendarField": "x"
    }));
    assert_eq!(
        annotations(&out),
        vec![json!({
            "node": "pageList",
            "attrs": { "source": { "kind": "children" }, "sort": { "by": "updated", "dir": "desc" }, "calendarField": "x" }
        })]
    );
    // Calculations of properties that are gone, or that name no function, leave none: the key is not written empty.
    let out = list(json!({
        "layout": "table",
        "calculations": { property(99).to_string(): "sum", property(10).to_string(): 4 }
    }));
    assert_eq!(
        annotations(&out),
        vec![json!({ "node": "pageList", "attrs": { "layout": "table" } })]
    );
    // Attributes of the wrong shape read as none.
    let out = list(
        json!({ "properties": "no", "sort": 3, "filters": {}, "source": [], "calculations": ["sum"] }),
    );
    assert_eq!(
        annotations(&out),
        vec![json!({ "node": "pageList", "attrs": {} })]
    );
}

fn views_of(out: &str) -> Value {
    annotations(out)[0]["attrs"]["views"].clone()
}

#[test]
fn the_views_of_a_page_list_are_named_like_its_own_attributes() {
    let status = property(10).to_string();
    let estimate = property(11).to_string();
    let gone = property(99).to_string();
    let list = json!({
        "type": "pageList",
        "attrs": {
            "id": block(7).to_string(),
            "layout": "table",
            "properties": [status, estimate],
            "views": [
                { "id": "k3f9a2bc", "name": "", "layout": "table",
                  "properties": [status, estimate, gone],
                  "sort": { "by": estimate, "dir": "desc" },
                  "filter": { "op": "and", "items": [
                      { "propertyId": status, "op": "hasOption", "value": option(1).to_string() },
                      { "op": "or", "items": [
                          { "propertyId": estimate, "op": "isSet" },
                          { "propertyId": gone, "op": "isSet" } ] },
                      { "op": "or", "items": [{ "propertyId": gone, "op": "isSet" }] },
                      { "propertyId": gone, "op": "isSet" } ] },
                  "columnWidths": { "title": 320, status.clone(): 150, gone.clone(): 5 },
                  "calculations": { "title": "count_all", gone.clone(): "sum" } },
                { "id": "b81d00e4", "name": "By status", "layout": "board", "properties": [estimate],
                  "sort": { "by": "title", "dir": "asc" }, "groupBy": status,
                  "hiddenGroups": [option(1).to_string(), "none", option(98).to_string()], "openIn": "center" },
                { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": [],
                  "sort": { "by": "updated", "dir": "asc" }, "dateBy": estimate, "endBy": gone },
                { "id": "e2a9b6f0", "name": "Photos", "layout": "gallery", "properties": [],
                  "sort": { "by": "created", "dir": "desc" }, "dateBy": "created", "cardSize": "l" },
                { "id": "00000001", "name": "Left alone", "layout": "calendar", "properties": [gone],
                  "sort": { "by": gone, "dir": "asc" }, "groupBy": gone, "dateBy": gone, "endBy": gone,
                  "filter": { "op": "or", "items": [{ "propertyId": gone, "op": "isSet" }] } }
            ]
        }
    });
    let out = render(json!([list]));
    assert_eq!(
        views_of(&out),
        json!([
            { "id": "k3f9a2bc", "name": "", "layout": "table",
              "properties": ["Status", "Estimate"],
              "sort": { "by": "Estimate", "dir": "desc" },
              "filter": { "op": "and", "items": [
                  { "property": "Status", "op": "hasOption", "value": "Done" },
                  { "op": "or", "items": [{ "property": "Estimate", "op": "isSet" }] } ] },
              "columnWidths": { "title": 320, "Status": 150 },
              "calculations": { "title": "count_all" } },
            { "id": "b81d00e4", "name": "By status", "layout": "board", "properties": ["Estimate"],
              "sort": { "by": "title", "dir": "asc" }, "groupBy": "Status",
              "hiddenGroups": ["Done", "none"], "openIn": "center" },
            { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": [],
              "sort": { "by": "updated", "dir": "asc" }, "dateBy": "Estimate" },
            { "id": "e2a9b6f0", "name": "Photos", "layout": "gallery", "properties": [],
              "sort": { "by": "created", "dir": "desc" }, "dateBy": "created", "cardSize": "l" },
            // Nothing is left to say but the id, the name and the layout.
            { "id": "00000001", "name": "Left alone", "layout": "calendar", "properties": [] }
        ])
    );
    // The block's own attributes are carried as before (the mirror of the first view).
    assert_eq!(
        annotations(&out)[0]["attrs"]["properties"],
        json!(["Status", "Estimate"])
    );
    for id in [
        status,
        estimate,
        gone,
        block(7).to_string(),
        option(1).to_string(),
    ] {
        assert!(!out.contains(&id), "{id} in {out}");
    }
}

#[test]
fn the_views_of_a_page_list_are_read_leniently_and_what_is_not_understood_is_kept() {
    let list = |views: Value| {
        render(json!([{ "type": "pageList", "attrs": { "layout": "list", "views": views } }]))
    };
    let many: Vec<Value> = (0..25)
        .map(|n| json!({ "id": format!("v{n}"), "layout": "list" }))
        .collect();
    let out = list(Value::Array(many));
    let views = views_of(&out);
    assert_eq!(
        views.as_array().map(Vec::len),
        Some(20),
        "at most twenty views"
    );
    assert_eq!(
        views[0],
        json!({ "id": "v0", "name": "", "layout": "list", "properties": [] })
    );

    // Entries that are no view, the second of a repeated id and views of a newer version.
    let out = list(json!([
        "nope",
        { "name": "no id" },
        { "id": "a", "name": "first", "layout": "timeline", "futureKey": { "n": 1 }, "openIn": "elsewhere", "__proto__": 1 },
        { "id": "a", "name": "second" },
        { "id": "b", "name": "x".repeat(100) }
    ]));
    let views = views_of(&out);
    assert_eq!(views.as_array().map(Vec::len), Some(2));
    assert_eq!(
        views[0],
        json!({ "id": "a", "name": "first", "layout": "timeline", "properties": [],
                "futureKey": { "n": 1 }, "openIn": "elsewhere" })
    );
    assert_eq!(views[1]["name"], json!("x".repeat(80)));

    // No usable view: the key is not written (the legacy attributes say what the block shows).
    for views in [
        json!([]),
        json!("views"),
        json!([1, null, { "id": "" }]),
        json!({ "id": "a" }),
    ] {
        let out = list(views);
        assert_eq!(
            annotations(&out),
            vec![json!({ "node": "pageList", "attrs": { "layout": "list" } })]
        );
    }
}

#[test]
fn the_sorts_of_a_view_are_named_like_its_sort_and_a_key_without_a_name_is_dropped() {
    let status = property(10).to_string();
    let estimate = property(11).to_string();
    let gone = property(99).to_string();
    let list = |views: Value| {
        render(json!([{ "type": "pageList", "attrs": { "layout": "table", "views": views } }]))
    };
    let out = list(json!([
        { "id": "a", "name": "", "layout": "table", "properties": [],
          "sort": { "by": estimate, "dir": "desc" },
          "sorts": [
              { "by": estimate, "dir": "desc" },
              { "by": gone, "dir": "asc" },
              { "by": "created", "dir": "desc" },
              { "by": status, "dir": "asc" }
          ] },
        // Every key without a name: the attribute is not written, with the `sort` that mirrored it.
        { "id": "b", "name": "", "layout": "table", "properties": [],
          "sort": { "by": gone, "dir": "asc" },
          "sorts": [{ "by": gone, "dir": "asc" }, { "by": "created", "dir": "asc" }] },
        { "id": "c", "name": "", "layout": "table", "properties": [],
          "sort": { "by": gone, "dir": "asc" },
          "sorts": [{ "by": gone, "dir": "asc" }] }
    ]));
    let views = views_of(&out);
    assert_eq!(
        views[0]["sorts"],
        json!([
            { "by": "Estimate", "dir": "desc" },
            { "by": "created", "dir": "desc" },
            { "by": "Status", "dir": "asc" }
        ])
    );
    assert_eq!(views[0]["sort"], json!({ "by": "Estimate", "dir": "desc" }));
    assert_eq!(
        views[1]["sorts"],
        json!([{ "by": "created", "dir": "asc" }])
    );
    assert!(views[1].get("sort").is_none());
    assert!(views[2].get("sorts").is_none() && views[2].get("sort").is_none());
    for id in [status, estimate, gone] {
        assert!(!out.contains(&id), "{id} in {out}");
    }
}

#[test]
fn the_new_filter_operators_and_the_page_sources_are_written_as_the_old_ones_are() {
    let status = property(10).to_string();
    let estimate = property(11).to_string();
    let page = page_id(2).to_string();
    let list = |attrs: Value| render(json!([{ "type": "pageList", "attrs": attrs }]));
    let filters = json!([
        { "propertyId": status, "op": "lacksOption", "value": option(1).to_string() },
        { "propertyId": status, "op": "lacksOption", "value": option(98).to_string() },
        { "propertyId": status, "op": "lacksOption", "value": [option(1).to_string(), option(98).to_string()] },
        { "propertyId": estimate, "op": "notRelatesTo", "value": page },
        { "propertyId": estimate, "op": "onOrBefore", "value": { "relative": "today" } },
        { "propertyId": estimate, "op": "within", "value": { "relative": "pastDays", "n": 7 } },
        { "propertyId": estimate, "op": "notEquals", "value": 3 },
        { "propertyId": property(99).to_string(), "op": "lacksOption", "value": option(1).to_string() }
    ]);
    let expected = json!([
        { "property": "Status", "op": "lacksOption", "value": "Done" },
        { "property": "Status", "op": "lacksOption", "value": ["Done"] },
        { "property": "Estimate", "op": "notRelatesTo", "value": page },
        { "property": "Estimate", "op": "onOrBefore", "value": { "relative": "today" } },
        { "property": "Estimate", "op": "within", "value": { "relative": "pastDays", "n": 7 } },
        { "property": "Estimate", "op": "notEquals", "value": 3 }
    ]);
    // In the flat list and in a view's filter tree.
    let out = list(json!({ "layout": "table", "filters": filters }));
    assert_eq!(annotations(&out)[0]["attrs"]["filters"], expected);
    let out = list(json!({ "layout": "table", "views": [
        { "id": "a", "layout": "table", "filter": { "op": "and", "items": [
            { "op": "or", "items": filters } ] } }
    ] }));
    assert_eq!(
        views_of(&out)[0]["filter"],
        json!({ "op": "and", "items": [{ "op": "or", "items": expected }] })
    );
    assert!(!out.contains(&option(1).to_string()), "{out}");

    // The linked sources are written as they are stored.
    for source in [
        json!({ "kind": "page", "pageId": page }),
        json!({ "kind": "space" }),
    ] {
        let out = list(json!({ "source": source, "layout": "list" }));
        assert_eq!(annotations(&out)[0]["attrs"]["source"], source);
    }
}

// ----- annotations: where they are not written, and where they go -----------------------------

#[test]
fn a_table_cell_has_no_annotations() {
    let cell = |blocks: Value| {
        json!({
            "type": "table",
            "content": [{ "type": "tableRow", "content": [{ "type": "tableCell", "content": blocks }] }]
        })
    };
    let blocks = json!([
        image(json!({ "attachmentId": attachment_ref(1), "widthPercent": 50 })),
        { "type": "tableOfContents" },
        { "type": "callout", "attrs": { "icon": "x" }, "content": [p("y")] },
    ]);
    let out = render(json!([cell(blocks)]));
    assert!(annotations(&out).is_empty(), "{out}");
    assert!(!out.contains("<!--"), "{out}");
    assert!(out.starts_with("| ![](dir/diagram.png)"), "{out}");
}

#[test]
fn annotations_in_a_list_item_follow_the_item() {
    let list = json!({
        "type": "bulletList",
        "content": [{
            "type": "listItem",
            "content": [p("item"), image(json!({ "attachmentId": attachment_ref(1), "align": "left" }))]
        }]
    });
    assert_eq!(
        render(json!([list])),
        "- item\n  <!-- atomik:{\"attrs\":{\"align\":\"left\"}} -->\n\n  ![](dir/diagram.png)\n"
    );
}

// ----- tables, toggles, whiteboards -----------------------------------------------------------

/// A table of rows of cells, each cell a list of blocks.
fn table(rows: Vec<Vec<Vec<Value>>>) -> Value {
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let cells: Vec<Value> = row
                .into_iter()
                .map(|blocks| json!({ "type": "tableCell", "content": blocks }))
                .collect();
            json!({ "type": "tableRow", "content": cells })
        })
        .collect();
    json!({ "type": "table", "content": rows })
}

#[test]
fn a_table_cell_escapes_its_pipes_once_and_has_no_block_syntax() {
    let code = para(json!([marked("a|b", json!([{ "type": "code" }]))]));
    let grid = table(vec![
        vec![vec![p("a | b")], vec![code]],
        vec![vec![p("- x"), p("1. y")], vec![p("*")]],
    ]);
    assert_eq!(
        render(json!([grid])),
        "| a \\| b | `a\\|b` |\n| --- | --- |\n| - x<br><br>1. y | \\* |\n"
    );
    // A pipe in a destination does not split the cell either.
    let link = para(json!([marked(
        "x",
        json!([{ "type": "link", "attrs": { "href": "https://a.example/?a|b" } }])
    )]));
    assert_eq!(
        render(json!([table(vec![vec![vec![link]]])])),
        "| [x](https://a.example/?a\\|b) |\n| --- |\n"
    );
}

#[test]
fn a_toggle_summary_is_html() {
    let toggle = json!({
        "type": "details",
        "attrs": { "open": true },
        "content": [
            { "type": "detailsSummary", "content": [text("A & B <i> *c* "), date(json!({ "date": "2026-10-07" }))] },
            { "type": "detailsContent", "content": [p("a & *b*")] },
        ]
    });
    assert_eq!(
        render(json!([toggle])),
        "<details open>\n<summary>A &amp; B &lt;i&gt; *c* <time datetime=\"2026-10-07\">2026-10-07</time></summary>\n\na \\& \\*b\\*\n\n</details>\n"
    );
}

#[test]
fn a_toggle_heading_summary_is_a_heading_in_html() {
    // ADR-0040: the summary's text goes inside `<h1>` to `<h3>`, escaped as HTML like any summary,
    // so that the importer reads the level back from the element.
    let toggle = |level: Value| {
        json!({
            "type": "details",
            "content": [
                { "type": "detailsSummary", "attrs": { "level": level }, "content": [text("A & B <i> *c* "), date(json!({ "date": "2026-10-07" }))] },
                { "type": "detailsContent", "content": [p("a & *b*")] },
            ]
        })
    };
    for level in 1..=3 {
        assert_eq!(
            render(json!([toggle(json!(level))])),
            format!(
                "<details>\n<summary><h{level}>A &amp; B &lt;i&gt; *c* <time datetime=\"2026-10-07\">2026-10-07</time></h{level}></summary>\n\na \\& \\*b\\*\n\n</details>\n"
            )
        );
    }
    // A level that is not 1 to 3 is a plain toggle.
    assert_eq!(
        render(json!([toggle(json!(4))])),
        "<details>\n<summary>A &amp; B &lt;i&gt; *c* <time datetime=\"2026-10-07\">2026-10-07</time></summary>\n\na \\& \\*b\\*\n\n</details>\n"
    );
}

// ----- what does not change ---------------------------------------------------------------------

#[test]
fn the_clipboard_flavour_is_not_escaped() {
    let id = |n: u8| block(n).to_string();
    let doc_json = json!({
        "type": "doc",
        "content": [
            { "type": "paragraph", "attrs": { "id": id(1) }, "content": [text("a*b_c [d] - e")] },
            { "type": "paragraph", "attrs": { "id": id(2) }, "content": [
                { "type": "dateMention", "attrs": { "id": id(3), "date": "2026-10-07", "reminder": 5 } }
            ] },
            { "type": "callout", "attrs": { "id": id(4), "icon": "💡", "color": "blue" }, "content": [
                { "type": "paragraph", "attrs": { "id": id(5) }, "content": [text("x")] }
            ] },
            { "type": "tableOfContents", "attrs": { "id": id(6) } },
        ]
    });
    let page = PageDoc::from_portable(Doc::new(DocKind::Page, 3), &doc_json).unwrap();
    let clipboard = page.to_markdown(&|_| None);
    assert_eq!(clipboard, "a*b_c [d] - e\n\n2026-10-07\n\n> 💡 x\n");
    let file = render_markdown_file(&page.to_portable(), &links());
    assert!(file.contains("a\\*b\\_c \\[d\\] - e"), "{file}");
    assert!(file.contains("<time"), "{file}");
    assert!(file.contains("<!-- atomik:"), "{file}");
}

#[test]
fn plain_pages_read_the_same_in_both_flavours() {
    let doc_json = json!({
        "type": "doc",
        "content": [
            { "type": "heading", "attrs": { "id": block(1).to_string(), "level": 2 }, "content": [text("Title")] },
            { "type": "paragraph", "attrs": { "id": block(2).to_string() }, "content": [text("Plain words, nothing else.")] },
            { "type": "bulletList", "attrs": { "id": block(3).to_string() }, "content": [
                { "type": "listItem", "attrs": { "id": block(4).to_string() }, "content": [
                    { "type": "paragraph", "attrs": { "id": block(5).to_string() }, "content": [text("one")] }
                ] }
            ] },
        ]
    });
    let page = PageDoc::from_portable(Doc::new(DocKind::Page, 3), &doc_json).unwrap();
    assert_eq!(
        render_markdown_file(&page.to_portable(), &links()),
        page.to_markdown(&|_| None)
    );
}

#[test]
fn an_empty_page_and_a_lone_block() {
    assert_eq!(render_markdown_file(&doc(json!([])), &links()), "\n");
    assert_eq!(
        render_markdown_file(&p("lone *block*"), &links()),
        "lone \\*block\\*\n"
    );
}

// ----- template buttons (ADR-0056) ------------------------------------------------------------

fn template_button(attrs: Value, content: Value) -> Value {
    json!({ "type": "templateButton", "attrs": attrs, "content": content })
}

#[test]
fn a_template_button_is_annotated_around_its_content() {
    let out = render(json!([
        p("Before"),
        template_button(
            json!({ "label": "Add task", "position": "above" }),
            json!([p("Task *one*"), p("Second")])
        ),
        p("After")
    ]));
    assert_eq!(
        out,
        "Before\n\n\
         <!-- atomik:{\"attrs\":{\"label\":\"Add task\",\"position\":\"above\"},\"node\":\"templateButton\"} -->\n\n\
         Task \\*one\\*\n\nSecond\n\n\
         <!-- atomik:{\"end\":\"templateButton\"} -->\n\n\
         After\n"
    );
}

#[test]
fn a_template_button_without_label_or_position_has_no_attributes() {
    let out = render(json!([
        template_button(
            json!({ "label": "", "position": "sideways" }),
            json!([p("x")])
        ),
        template_button(json!({}), json!([]))
    ]));
    assert_eq!(
        annotations(&out),
        vec![
            json!({ "node": "templateButton" }),
            json!({ "end": "templateButton" }),
            json!({ "node": "templateButton" }),
            json!({ "end": "templateButton" })
        ]
    );
    // An empty button is its two annotations alone.
    assert!(out.ends_with("<!-- atomik:{\"node\":\"templateButton\"} -->\n\n<!-- atomik:{\"end\":\"templateButton\"} -->\n"));
}

#[test]
fn a_label_cannot_close_its_annotation() {
    let out = render(json!([template_button(
        json!({ "label": "a--> <b> & \"c\"" }),
        json!([p("x")])
    )]));
    let first = out.lines().next().unwrap();
    assert_eq!(first.matches("-->").count(), 1, "{first}");
    assert!(first.ends_with(ANNOTATION_CLOSE));
    assert_eq!(
        annotations(&out)[0],
        json!({ "attrs": { "label": "a--> <b> & \"c\"" }, "node": "templateButton" })
    );
}

#[test]
fn the_content_of_a_template_button_keeps_its_own_annotations() {
    let out = render(json!([template_button(
        json!({}),
        json!([
            { "type": "callout", "attrs": { "icon": "💡", "color": "blue" }, "content": [p("Note")] },
            { "type": "columnList", "content": [
                { "type": "column", "content": [p("L")] },
                { "type": "column", "content": [p("R")] }
            ] }
        ])
    )]));
    let found = annotations(&out);
    assert_eq!(found.first(), Some(&json!({ "node": "templateButton" })));
    assert_eq!(found.last(), Some(&json!({ "end": "templateButton" })));
    assert!(
        found.contains(&json!({ "attrs": { "color": "blue", "icon": "💡" }, "node": "callout" }))
    );
    assert!(found.contains(&json!({ "end": "columnList" })));
}

#[test]
fn a_template_button_in_a_table_cell_writes_nothing() {
    let out = render(json!([{
        "type": "table",
        "content": [{ "type": "tableRow", "content": [
            { "type": "tableCell", "content": [
                p("cell"),
                template_button(json!({ "label": "x" }), json!([p("hidden")]))
            ] }
        ] }]
    }]));
    assert!(!out.contains("hidden"), "{out}");
    assert!(!out.contains("atomik:"), "{out}");
}

#[test]
fn a_relative_date_mention_is_marked_in_its_time_element() {
    let mention = |attrs: Value| json!({ "type": "dateMention", "attrs": attrs });
    let out = render(json!([para(json!([
        mention(json!({ "date": "2026-10-18", "reminder": 30, "relative": "today" })),
        text(" "),
        mention(json!({ "date": "2026-10-18", "time": "09:30", "relative": "now" })),
        text(" "),
        mention(json!({ "date": "2026-10-18", "relative": "yesterday" })),
    ]))]));
    assert_eq!(
        out,
        "<time datetime=\"2026-10-18\" data-reminder=\"30\" data-relative=\"today\">2026-10-18</time> \
         <time datetime=\"2026-10-18T09:30\" data-relative=\"now\">2026-10-18 09:30</time> \
         <time datetime=\"2026-10-18\">2026-10-18</time>\n"
    );
}
