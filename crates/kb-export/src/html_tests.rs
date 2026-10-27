//! Tests of the HTML export: each mapping of the spec, and that nothing in a page can inject
//! markup (hostile titles, links, ids, names, classes and styles, read back through an HTML parser).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashSet;

use kb_types::{AttachmentId, BlockId, OptionId, PageId, PropertyId};
use scraper::{Html, Selector};
use serde_json::{Value, json};

use super::*;
use crate::test_support::{TestLinks, attachment, block, page};
use crate::value::{ExportProperty, LinkedFile, LinkedPage};

// ----- helpers ------------------------------------------------------------------------------

fn meta() -> PageMeta {
    PageMeta {
        title: "Weekly review".to_owned(),
        icon: Some("📝".to_owned()),
        ..PageMeta::default()
    }
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

fn p(words: &str) -> Value {
    json!({ "type": "paragraph", "content": [text(words)] })
}

fn id(n: u8) -> String {
    block(n).to_string()
}

/// The blocks of a page as HTML: what is between its header and its end.
fn render(blocks: Value) -> String {
    render_doc(&doc(blocks))
}

fn render_doc(content: &Value) -> String {
    let html = page_html(&meta(), content, &TestLinks::sample());
    let (_, after) = html.split_once("</header>\n").unwrap();
    after
        .strip_suffix("</article>\n</body>\n</html>\n")
        .unwrap()
        .to_owned()
}

/// A paragraph around inline nodes, as HTML.
fn inline(nodes: Value) -> String {
    let html = render(json!([{ "type": "paragraph", "content": nodes }]));
    html.strip_prefix("<p>")
        .and_then(|html| html.strip_suffix("</p>\n"))
        .unwrap()
        .to_owned()
}

// ----- the document -----------------------------------------------------------------------------

#[test]
fn a_page_is_a_standalone_document() {
    let html = page_html(&meta(), &doc(json!([p("Hello")])), &TestLinks::sample());
    assert!(html.starts_with(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"generator\" content=\"atomik-kb\">\n<title>Weekly review</title>\n<style>\n"
    ));
    assert!(html.ends_with("</style>\n</head>\n<body>\n<article class=\"page\">\n<header>\n<span class=\"page-icon page-header-icon\">📝</span>\n<h1 class=\"page-title\">Weekly review</h1>\n</header>\n<p>Hello</p>\n</article>\n</body>\n</html>\n"));
    // One stylesheet, with a dark variant and print rules; nothing is loaded from anywhere.
    assert_eq!(html.matches("<style>").count(), 1);
    assert!(html.contains("prefers-color-scheme: dark"));
    assert!(html.contains("@media print"));
    assert!(html.contains("max-width: none"));
    for forbidden in [
        "<script",
        "@import",
        "url(",
        "<link",
        "http://",
        "https://",
        "src=\"http",
    ] {
        assert!(!html.contains(forbidden), "{forbidden}");
    }
    // And a parser agrees on the structure.
    let dom = Html::parse_document(&html);
    let select = |css: &str| dom.select(&Selector::parse(css).unwrap()).count();
    assert_eq!(select("html[lang=en] > head > meta[charset=utf-8]"), 1);
    assert_eq!(select("article.page > header > h1.page-title"), 1);
    assert_eq!(select("article.page > p"), 1);
}

#[test]
fn an_untitled_page_says_so() {
    let bare = PageMeta::default();
    let html = page_html(&bare, &doc(json!([])), &TestLinks::sample());
    assert!(html.contains("<title>Untitled</title>"));
    assert!(html.contains("<h1 class=\"page-title\">Untitled</h1>"));
    assert!(!html.contains("class=\"page-icon"));
    assert!(!html.contains("class=\"properties\""));
}

#[test]
fn the_cover_is_a_banner_of_a_picture_a_colour_or_a_gradient_or_nothing() {
    let with = |cover: Option<&str>, y: Option<u8>| {
        let mut page = meta();
        page.cover = cover.map(str::to_owned);
        page.cover_y = y;
        page_html(&page, &doc(json!([p("x")])), &TestLinks::sample())
    };
    let banner = |html: &str| {
        let (before, _) = html.split_once("<header>\n").unwrap();
        let (_, after) = before.split_once("<article class=\"page\">\n").unwrap();
        after.to_owned()
    };
    // A picture of the archive, with the position of the image in the banner.
    assert_eq!(
        banner(&with(Some("Notes/my%20banner.png"), Some(30))),
        "<img class=\"page-cover\" src=\"Notes/my%20banner.png\" alt=\"\" style=\"object-position:50% 30%\">\n"
    );
    assert!(banner(&with(Some("../a.jpg"), None)).contains("object-position:50% 50%"));
    // A colour or a gradient is a block the stylesheet colours.
    assert_eq!(
        banner(&with(Some("color:blue"), None)),
        "<div class=\"page-cover\" data-color=\"blue\"></div>\n"
    );
    assert_eq!(
        banner(&with(Some("gradient:sunset"), Some(30))),
        "<div class=\"page-cover\" data-gradient=\"sunset\"></div>\n"
    );
    // The stylesheet knows the nine colours and the bundled gradients, and loads nothing.
    let html = with(Some("color:red"), None);
    for name in [
        "gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red",
    ] {
        assert!(
            html.contains(&format!(".page-cover[data-color=\"{name}\"]")),
            "{name}"
        );
    }
    for name in [
        "sunset", "dawn", "ocean", "forest", "lavender", "peach", "mint", "slate", "candy",
        "aurora",
    ] {
        assert!(
            html.contains(&format!(".page-cover[data-gradient=\"{name}\"]")),
            "{name}"
        );
    }
    assert!(!html.contains("url("));
    // Escaped, and nothing at all without a cover or for what is not a link to a file.
    assert!(banner(&with(Some("gradient:a\"b"), None)).contains("data-gradient=\"a&quot;b\""));
    for none in [
        None,
        Some(""),
        Some("attachment://nowhere"),
        Some("javascript:alert(1)"),
    ] {
        let html = with(none, Some(30));
        assert_eq!(banner(&html), "", "{none:?}");
        assert!(!html.contains("class=\"page-cover\""), "{none:?}");
    }
}

#[test]
fn the_icon_is_a_glyph_an_image_or_nothing() {
    let header = |icon: Option<&str>| {
        let mut page = meta();
        page.icon = icon.map(str::to_owned);
        let html = page_html(&page, &doc(json!([])), &TestLinks::sample());
        html.split_once("<header>\n")
            .unwrap()
            .1
            .split_once("<h1")
            .unwrap()
            .0
            .to_owned()
    };
    assert_eq!(
        header(Some("🚀")),
        "<span class=\"page-icon page-header-icon\">🚀</span>\n"
    );
    assert_eq!(
        header(Some("Notes/my%20icon.png")),
        "<img class=\"page-icon\" src=\"Notes/my%20icon.png\" alt=\"\">\n"
    );
    assert_eq!(header(Some("icon:rocket:blue")), "");
    assert_eq!(header(None), "");
    assert_eq!(header(Some("")), "");
    // A "link" with a scheme or off the archive is not an image.
    assert_eq!(header(Some("//evil.example/x.png")), "");
    assert!(!header(Some("javascript:alert(1)")).contains("<img"));
}

#[test]
fn the_properties_are_a_table_of_names_and_text() {
    let mut page = meta();
    let prop = |name: &str, value: ExportValue| ExportProperty {
        name: name.to_owned(),
        tags: false,
        value,
    };
    page.properties = vec![
        ExportProperty {
            name: "Tags".to_owned(),
            tags: true,
            value: ExportValue::Options(vec!["work".into(), "a & b".into()]),
        },
        prop("Status", ExportValue::Options(vec!["Done".into()])),
        prop("Done", ExportValue::Checkbox(true)),
        prop("Estimate", ExportValue::Number(3.5)),
        prop(
            "Due",
            ExportValue::Date {
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
            },
        ),
        prop("Note", ExportValue::Text("<b>bold</b>".into())),
        prop(
            "Items",
            ExportValue::Checklist(vec![(true, "a".into()), (false, "b".into())]),
        ),
        prop(
            "Place",
            ExportValue::Place {
                name: "Paris".into(),
                lat: Some(48.8566),
                lon: Some(2.3522),
            },
        ),
        prop(
            "Related",
            ExportValue::Pages(vec![
                LinkedPage {
                    title: "Other".into(),
                    href: Some("Other.md".into()),
                },
                LinkedPage {
                    title: "Beta <x>".into(),
                    href: None,
                },
                LinkedPage {
                    title: "Evil".into(),
                    href: Some("javascript:alert(1)".into()),
                },
            ]),
        ),
        prop(
            "Files",
            ExportValue::Files(vec![
                LinkedFile {
                    name: "r.pdf".into(),
                    href: Some("Page/r%20x.pdf".into()),
                },
                LinkedFile {
                    name: "gone.bin".into(),
                    href: None,
                },
            ]),
        ),
    ];
    let html = page_html(&page, &doc(json!([])), &TestLinks::sample());
    let table = html
        .split_once("<table class=\"properties\">\n")
        .unwrap()
        .1
        .split_once("</table>\n")
        .unwrap()
        .0;
    assert_eq!(
        table,
        "<tr><th scope=\"row\">Tags</th><td>work, a &amp; b</td></tr>\n\
         <tr><th scope=\"row\">Status</th><td>Done</td></tr>\n\
         <tr><th scope=\"row\">Done</th><td>Yes</td></tr>\n\
         <tr><th scope=\"row\">Estimate</th><td>3.5</td></tr>\n\
         <tr><th scope=\"row\">Due</th><td>2026-10-07 14:30</td></tr>\n\
         <tr><th scope=\"row\">Note</th><td>&lt;b&gt;bold&lt;/b&gt;</td></tr>\n\
         <tr><th scope=\"row\">Items</th><td>[x] a; [ ] b</td></tr>\n\
         <tr><th scope=\"row\">Place</th><td>Paris (48.8566, 2.3522)</td></tr>\n\
         <tr><th scope=\"row\">Related</th><td><a class=\"page-ref\" href=\"Other.md\">Other</a>, <span class=\"page-ref\">Beta &lt;x&gt;</span>, <span class=\"page-ref\">Evil</span></td></tr>\n\
         <tr><th scope=\"row\">Files</th><td><a class=\"file\" href=\"Page/r%20x.pdf\">r.pdf</a>, gone.bin</td></tr>\n"
    );
}

// ----- blocks ---------------------------------------------------------------------------------

#[test]
fn paragraphs_headings_and_rules() {
    assert_eq!(render(json!([p("a & b < c")])), "<p>a &amp; b &lt; c</p>\n");
    assert_eq!(render(json!([{ "type": "paragraph" }])), "<p></p>\n");
    let heading = |level: Value| {
        render(
            json!([{ "type": "heading", "attrs": { "id": id(7), "level": level }, "content": [text("Title")] }]),
        )
    };
    assert_eq!(
        heading(json!(1)),
        format!("<h1 id=\"h-{}\">Title</h1>\n", id(7))
    );
    assert_eq!(
        heading(json!(2)),
        format!("<h2 id=\"h-{}\">Title</h2>\n", id(7))
    );
    assert_eq!(
        heading(json!(3)),
        format!("<h3 id=\"h-{}\">Title</h3>\n", id(7))
    );
    // Deeper levels, and levels that are not integers, stay inside h1-h3.
    assert_eq!(
        heading(json!(6)),
        format!("<h3 id=\"h-{}\">Title</h3>\n", id(7))
    );
    assert_eq!(
        heading(json!("x")),
        format!("<h1 id=\"h-{}\">Title</h1>\n", id(7))
    );
    assert_eq!(
        render(json!([{ "type": "heading", "attrs": { "level": 2 }, "content": [text("No id")] }])),
        "<h2>No id</h2>\n"
    );
    assert_eq!(render(json!([{ "type": "horizontalRule" }])), "<hr>\n");
}

#[test]
fn lists() {
    let item = |words: &str| json!({ "type": "listItem", "content": [p(words)] });
    let nested = json!({
        "type": "listItem",
        "content": [p("two"), { "type": "bulletList", "content": [item("nested")] }]
    });
    assert_eq!(
        render(json!([{ "type": "bulletList", "content": [item("one"), nested] }])),
        "<ul>\n<li>one</li>\n<li>two\n<ul>\n<li>nested</li>\n</ul>\n</li>\n</ul>\n"
    );
    assert_eq!(
        render(json!([{ "type": "orderedList", "attrs": { "start": 3 }, "content": [item("x")] }])),
        "<ol start=\"3\">\n<li>x</li>\n</ol>\n"
    );
    assert_eq!(
        render(json!([{ "type": "orderedList", "attrs": { "start": 1 }, "content": [item("x")] }])),
        "<ol>\n<li>x</li>\n</ol>\n"
    );
    let task = |checked: bool, words: &str| json!({ "type": "taskItem", "attrs": { "checked": checked }, "content": [p(words)] });
    assert_eq!(
        render(
            json!([{ "type": "taskList", "content": [task(true, "Done"), task(false, "To do")] }])
        ),
        "<ul class=\"task-list\">\n\
         <li class=\"task\" data-checked=\"true\"><input type=\"checkbox\" disabled checked> Done</li>\n\
         <li class=\"task\" data-checked=\"false\"><input type=\"checkbox\" disabled> To do</li>\n\
         </ul>\n"
    );
    // An item that is not a list item stands for itself; an item with no paragraph first.
    assert_eq!(
        render(
            json!([{ "type": "bulletList", "content": [p("bare"), { "type": "listItem", "content": [{ "type": "horizontalRule" }] }] }])
        ),
        "<ul>\n<li>bare</li>\n<li>\n<hr>\n</li>\n</ul>\n"
    );
}

#[test]
fn quotes_callouts_and_toggles() {
    assert_eq!(
        render(json!([{ "type": "blockquote", "content": [p("q")] }])),
        "<blockquote>\n<p>q</p>\n</blockquote>\n"
    );
    assert_eq!(
        render(
            json!([{ "type": "callout", "attrs": { "icon": "💡", "color": "blue" }, "content": [p("Body")] }])
        ),
        "<aside class=\"callout\" data-color=\"blue\"><span class=\"callout-icon\">💡</span><div>\n<p>Body</p>\n</div></aside>\n"
    );
    // The default icon, no colour.
    assert_eq!(
        render(json!([{ "type": "callout", "content": [p("Body")] }])),
        "<aside class=\"callout\"><span class=\"callout-icon\">💡</span><div>\n<p>Body</p>\n</div></aside>\n"
    );
    let toggle = |open: Value| {
        json!({
            "type": "details",
            "attrs": { "open": open },
            "content": [
                { "type": "detailsSummary", "content": [text("Sum & more")] },
                { "type": "detailsContent", "content": [p("Hidden")] },
            ]
        })
    };
    assert_eq!(
        render(json!([toggle(json!(true))])),
        "<details open>\n<summary>Sum &amp; more</summary>\n<p>Hidden</p>\n</details>\n"
    );
    assert_eq!(
        render(json!([toggle(json!(false))])),
        "<details>\n<summary>Sum &amp; more</summary>\n<p>Hidden</p>\n</details>\n"
    );
}

#[test]
fn a_toggle_heading_is_a_heading_inside_the_summary() {
    // ADR-0040: the summary's text goes inside `<h1>` to `<h3>` (with the fragment id the table
    // of contents links to, when the summary has an id); a level that is not 1 to 3 is a plain toggle.
    let toggle = |level: Value| {
        json!({
            "type": "details",
            "attrs": { "open": true },
            "content": [
                { "type": "detailsSummary", "attrs": { "id": id(7), "level": level }, "content": [text("Sum & more")] },
                { "type": "detailsContent", "content": [p("Hidden")] },
            ]
        })
    };
    for level in 1..=3 {
        assert_eq!(
            render(json!([toggle(json!(level))])),
            format!(
                "<details open>\n<summary><h{level} id=\"h-{}\">Sum &amp; more</h{level}></summary>\n<p>Hidden</p>\n</details>\n",
                id(7)
            )
        );
    }
    for damaged in [json!(0), json!(4), json!(1.5), json!("2"), json!(null)] {
        assert_eq!(
            render(json!([toggle(damaged.clone())])),
            "<details open>\n<summary>Sum &amp; more</summary>\n<p>Hidden</p>\n</details>\n",
            "{damaged}"
        );
    }
    // Without an id the heading has none; an empty summary is an empty heading.
    let bare = json!({ "type": "details", "content": [
        { "type": "detailsSummary", "attrs": { "level": 2 } },
        { "type": "detailsContent", "content": [p("x")] },
    ] });
    assert_eq!(
        render(json!([bare])),
        "<details>\n<summary><h2></h2></summary>\n<p>x</p>\n</details>\n"
    );
    // The heading sits on the summary's line, next to its marker.
    assert!(STYLE.contains("summary > :is(h1, h2, h3) { display: inline;"));
}

#[test]
fn code_and_math() {
    let code = |language: Value, source: &str| {
        render(
            json!([{ "type": "codeBlock", "attrs": { "language": language }, "content": [text(source)] }]),
        )
    };
    assert_eq!(
        code(json!("rust"), "fn main() {\n    println!(\"hi\");\n}"),
        "<pre><code class=\"language-rust\">fn main() {\n    println!(&quot;hi&quot;);\n}</code></pre>\n"
    );
    assert_eq!(
        code(json!(null), "<x>"),
        "<pre><code>&lt;x&gt;</code></pre>\n"
    );
    // Mermaid is its source, whatever the language is.
    assert_eq!(
        code(json!("mermaid"), "A-->B"),
        "<pre><code class=\"language-mermaid\">A--&gt;B</code></pre>\n"
    );
    // A language that is more than a name is cut down to one.
    assert_eq!(
        code(json!("c++ \"onclick=x"), "y"),
        "<pre><code class=\"language-c++onclickx\">y</code></pre>\n"
    );
    assert_eq!(
        code(json!("\"> <script>"), "y"),
        "<pre><code class=\"language-script\">y</code></pre>\n"
    );
    assert_eq!(
        render(json!([{ "type": "mathBlock", "content": [text("E = mc^2 < \\alpha")] }])),
        "<div class=\"math math-display\">\\[E = mc^2 &lt; \\alpha\\]</div>\n"
    );
    assert_eq!(
        inline(json!([text("see "), { "type": "mathInline", "attrs": { "latex": "a<b&c" } }])),
        "see <span class=\"math math-inline\">\\(a&lt;b&amp;c\\)</span>"
    );
}

#[test]
fn tables_use_header_cells() {
    let cell = |kind: &str, blocks: Value| json!({ "type": kind, "attrs": { "colspan": 1, "rowspan": 1 }, "content": blocks });
    let table = json!({
        "type": "table",
        "content": [
            { "type": "tableRow", "content": [cell("tableHeader", json!([p("Feature")])), cell("tableHeader", json!([p("Notes")]))] },
            { "type": "tableRow", "content": [
                cell("tableCell", json!([{ "type": "paragraph", "content": [marked("Done", json!([{ "type": "bold" }]))] }])),
                cell("tableCell", json!([p("One"), p("Two")])),
            ] },
            { "type": "tableRow", "content": [
                { "type": "tableCell", "attrs": { "colspan": 2, "rowspan": 3 }, "content": [] },
            ] },
        ]
    });
    assert_eq!(
        render(json!([table])),
        "<table>\n\
         <tr><th>Feature</th><th>Notes</th></tr>\n\
         <tr><td><strong>Done</strong></td><td><p>One</p>\n<p>Two</p>\n</td></tr>\n\
         <tr><td colspan=\"2\" rowspan=\"3\"></td></tr>\n\
         </table>\n"
    );
}

#[test]
fn columns() {
    let columns = json!({
        "type": "columnList",
        "content": [
            { "type": "column", "content": [p("L")] },
            { "type": "column", "content": [p("R"), p("R2")] },
        ]
    });
    assert_eq!(
        render(json!([columns])),
        "<div class=\"columns\" style=\"--columns:2\">\n\
         <div class=\"column\">\n<p>L</p>\n</div>\n\
         <div class=\"column\">\n<p>R</p>\n<p>R2</p>\n</div>\n\
         </div>\n"
    );
}

#[test]
fn images_files_and_galleries() {
    let image = |attrs: Value| render(json!([{ "type": "image", "attrs": attrs }]));
    assert_eq!(
        image(
            json!({ "attachmentId": attachment(1).to_string(), "alt": "Diagram \"x\"", "widthPercent": 60, "align": "right", "caption": "Fig <1>", "width": 640 })
        ),
        "<figure class=\"image align-right\" style=\"width:60%\"><img src=\"dir/diagram.png\" alt=\"Diagram &quot;x&quot;\"><figcaption>Fig &lt;1&gt;</figcaption></figure>\n"
    );
    // The legacy width in pixels, no side, no caption.
    assert_eq!(
        image(json!({ "attachmentId": attachment(1).to_string(), "width": 640 })),
        "<figure class=\"image\" style=\"width:640px;max-width:100%\"><img src=\"dir/diagram.png\" alt=\"\"></figure>\n"
    );
    // Out-of-range or unknown layout is ignored.
    assert_eq!(
        image(
            json!({ "attachmentId": attachment(1).to_string(), "widthPercent": 500, "align": "diagonal" })
        ),
        "<figure class=\"image\"><img src=\"dir/diagram.png\" alt=\"\"></figure>\n"
    );
    // Not in the archive, and not known at all: the attachment link.
    let gone = attachment(5).to_string();
    assert_eq!(
        image(json!({ "attachmentId": gone })),
        format!("<figure class=\"image\"><img src=\"attachment://{gone}\" alt=\"\"></figure>\n")
    );

    let file = |view: Option<&str>, n: u8| {
        let attrs = view.map_or(json!({}), |view| json!({ "view": view }));
        let mut attrs = attrs;
        attrs["attachmentId"] = json!(attachment(n).to_string());
        render(json!([{ "type": "file", "attrs": attrs }]))
    };
    assert_eq!(
        file(None, 4),
        "<p><a class=\"file\" href=\"dir/report%20x.pdf\">report.pdf</a></p>\n"
    );
    assert_eq!(
        file(Some("preview"), 4),
        "<p><a class=\"file\" href=\"dir/report%20x.pdf\">report.pdf</a></p>\n"
    );
    assert_eq!(
        file(Some("preview"), 2),
        "<p><video controls src=\"dir/clip.mp4\"></video></p>\n"
    );
    assert_eq!(
        file(Some("preview"), 3),
        "<p><audio controls src=\"dir/song.mp3\"></audio></p>\n"
    );
    // Without the preview view a video is a link.
    assert_eq!(
        file(None, 2),
        "<p><a class=\"file\" href=\"dir/clip.mp4\">clip.mp4</a></p>\n"
    );
    assert_eq!(
        file(None, 5),
        format!("<p><a class=\"file\" href=\"attachment://{gone}\">gone.bin</a></p>\n")
    );

    let gallery = json!({
        "type": "gallery",
        "attrs": { "columns": 4, "items": [
            { "attachmentId": attachment(1).to_string(), "caption": "One" },
            { "caption": "unusable" },
            { "attachmentId": gone },
        ] }
    });
    assert_eq!(
        render(json!([gallery])),
        format!(
            "<div class=\"gallery\" style=\"--columns:4\">\n\
             <figure class=\"image\"><img src=\"dir/diagram.png\" alt=\"\"><figcaption>One</figcaption></figure>\n\
             <figure class=\"image\"><img src=\"attachment://{gone}\" alt=\"\"></figure>\n\
             </div>\n"
        )
    );
    // The default number of columns; nothing at all without a usable item.
    let plain = json!({ "type": "gallery", "attrs": { "items": [{ "attachmentId": attachment(1).to_string() }] } });
    assert!(render(json!([plain])).starts_with("<div class=\"gallery\" style=\"--columns:3\">"));
    assert_eq!(render(json!([{ "type": "gallery" }])), "");
}

#[test]
fn whiteboards_link_their_file_and_list_their_texts() {
    let content = json!({
        "type": "doc",
        "content": [
            { "type": "whiteboard", "attrs": { "id": id(1) } },
            { "type": "whiteboard", "attrs": { "id": id(2) } },
        ],
        "whiteboards": { id(1): {
            "a": { "id": "a", "type": "text", "index": "a1", "originalText": "Plan A\nship <it>" },
            "b": { "id": "b", "type": "rectangle", "index": "a0" },
            "c": { "id": "c", "type": "text", "index": "a2", "isDeleted": true, "text": "gone" },
        } }
    });
    assert_eq!(
        render_doc(&content),
        "<figure class=\"whiteboard\"><a href=\"dir/Whiteboard.excalidraw\">Whiteboard</a><ul><li>Plan A ship &lt;it&gt;</li></ul></figure>\n\
         <figure class=\"whiteboard\"><span>Whiteboard</span></figure>\n"
    );
}

#[test]
fn page_references() {
    let reference =
        |n: u8| json!({ "type": "pageRef", "attrs": { "pageId": page(n).to_string() } });
    assert_eq!(
        inline(json!([
            text("see "),
            reference(1),
            text(" and "),
            reference(2),
            text(" and "),
            reference(9)
        ])),
        format!(
            "see <a class=\"page-ref\" href=\"Alpha.md\">Alpha</a> and <span class=\"page-ref\">Beta</span> and <span class=\"page-ref\">{}</span>",
            page(9)
        )
    );
    let card =
        json!({ "type": "pageLink", "attrs": { "pageId": page(1).to_string(), "blockId": id(3) } });
    assert_eq!(
        render(json!([card])),
        "<p class=\"page-link\"><a class=\"page-ref\" href=\"Alpha.md\">Alpha</a></p>\n"
    );
}

#[test]
fn dates() {
    let date = |attrs: Value| inline(json!([{ "type": "dateMention", "attrs": attrs }]));
    assert_eq!(
        date(json!({ "date": "2026-10-07" })),
        "<time datetime=\"2026-10-07\">2026-10-07</time>"
    );
    assert_eq!(
        date(json!({ "date": "2026-10-08", "time": "14:30", "reminder": 15 })),
        "<time datetime=\"2026-10-08T14:30\" data-reminder=\"15\">2026-10-08 14:30</time>"
    );
    // A damaged mention loses what is damaged, or everything.
    assert_eq!(
        date(json!({ "date": "2026-10-08", "time": "24:00", "reminder": -1 })),
        "<time datetime=\"2026-10-08\">2026-10-08</time>"
    );
    assert_eq!(date(json!({ "date": "2026-02-30" })), "");
    assert_eq!(date(json!({})), "");
}

/// An account id as the editor writes it into a `personMention`.
fn account() -> String {
    kb_types::AccountId::from_random_bytes([9; 16]).to_string()
}

fn person(label: &str) -> Value {
    json!({ "type": "personMention", "attrs": { "accountId": account(), "label": label } })
}

#[test]
fn person_mentions_are_spans_with_the_label_and_no_account_id() {
    let mention = |attrs: Value| inline(json!([{ "type": "personMention", "attrs": attrs }]));
    let ann = "<span class=\"person-mention\">@Ann Lee</span>";
    assert_eq!(
        mention(json!({ "accountId": account(), "label": "Ann Lee" })),
        ann
    );
    // Readers are lenient (ADR-0049): no valid id is still its label.
    assert_eq!(mention(json!({ "label": "Ann Lee" })), ann);
    assert_eq!(
        mention(json!({ "accountId": "not an id", "label": "Ann Lee" })),
        ann
    );
    // The account id means nothing outside the workspace: it is not in the file at all.
    let html =
        render(json!([{ "type": "paragraph", "content": [text("cc "), person("Ann Lee")] }]));
    assert_eq!(html, format!("<p>cc {ann}</p>\n"));
    assert!(!html.contains(&account()));
    // The label is text.
    assert_eq!(
        mention(json!({ "accountId": account(), "label": "<b>Ann</b> & \"Bo\" 'x'" })),
        "<span class=\"person-mention\">@&lt;b&gt;Ann&lt;/b&gt; &amp; &quot;Bo&quot; &#39;x&#39;</span>"
    );
    // A line break or a control character in a self-asserted name is a space, as in the Markdown.
    assert_eq!(
        mention(json!({ "accountId": account(), "label": "Ann\nLee\u{0}\u{7f}Jr" })),
        "<span class=\"person-mention\">@Ann Lee  Jr</span>"
    );
    // Without a label there is nothing to say.
    for attrs in [
        json!({}),
        json!({ "accountId": account() }),
        json!({ "accountId": account(), "label": "" }),
        json!({ "accountId": account(), "label": " \t\n " }),
        json!({ "accountId": account(), "label": null }),
        json!({ "accountId": account(), "label": 7 }),
        json!({ "accountId": account(), "label": ["Ann"] }),
    ] {
        assert_eq!(mention(attrs.clone()), "", "{attrs}");
    }
}

#[test]
fn person_mentions_sit_in_text_marks_lists_and_headings() {
    // Text around a mention keeps its marks; the mention itself has none.
    assert_eq!(
        inline(json!([
            marked("bold ", json!([{ "type": "bold" }])),
            person("Ann"),
            marked(" after", json!([{ "type": "italic" }])),
        ])),
        "<strong>bold </strong><span class=\"person-mention\">@Ann</span><em> after</em>"
    );
    // A paragraph that is only a mention, in a list item and in a quote.
    let only = json!({ "type": "paragraph", "content": [person("Ann")] });
    assert_eq!(
        render(
            json!([{ "type": "bulletList", "content": [{ "type": "listItem", "content": [only.clone()] }] }])
        ),
        "<ul>\n<li><span class=\"person-mention\">@Ann</span></li>\n</ul>\n"
    );
    assert_eq!(
        render(json!([{ "type": "blockquote", "content": [only] }])),
        "<blockquote>\n<p><span class=\"person-mention\">@Ann</span></p>\n</blockquote>\n"
    );
    // Inline nodes without a paragraph around them are grouped into one, like the others.
    assert_eq!(
        render(json!([text("a "), person("Ann")])),
        "<p>a <span class=\"person-mention\">@Ann</span></p>\n"
    );
    // A heading holds it, and the table of contents reads it as text.
    let html = render(json!([
        { "type": "tableOfContents" },
        { "type": "heading", "attrs": { "id": id(1), "level": 2 }, "content": [text("Review by "), person("Ann")] },
    ]));
    assert_eq!(
        html,
        format!(
            "<nav class=\"toc\">\n<ul>\n<li class=\"toc-level-2\"><a href=\"#h-{0}\">Review by @Ann</a></li>\n</ul>\n</nav>\n\
             <h2 id=\"h-{0}\">Review by <span class=\"person-mention\">@Ann</span></h2>\n",
            id(1)
        )
    );
}

#[test]
fn people_in_a_person_property_are_their_names() {
    let mut page = meta();
    page.properties = vec![
        ExportProperty {
            name: "Owner".to_owned(),
            tags: false,
            value: ExportValue::Options(vec!["Ann Lee".into(), "Bo <b> & co".into()]),
        },
        ExportProperty {
            name: "Reviewer".to_owned(),
            tags: false,
            value: ExportValue::Options(vec!["Former member".into()]),
        },
    ];
    let html = page_html(&page, &doc(json!([])), &TestLinks::sample());
    let table = html
        .split_once("<table class=\"properties\">\n")
        .unwrap()
        .1
        .split_once("</table>\n")
        .unwrap()
        .0;
    assert_eq!(
        table,
        "<tr><th scope=\"row\">Owner</th><td>Ann Lee, Bo &lt;b&gt; &amp; co</td></tr>\n\
         <tr><th scope=\"row\">Reviewer</th><td>Former member</td></tr>\n"
    );
}

#[test]
fn derived_blocks_and_the_table_of_contents() {
    let heading = |level: u8, n: u8, words: &str| json!({ "type": "heading", "attrs": { "id": id(n), "level": level }, "content": [text(words)] });
    let content = json!([
        { "type": "tableOfContents" },
        heading(1, 1, "One & only"),
        { "type": "columnList", "content": [{ "type": "column", "content": [heading(2, 2, "Two")] }] },
        { "type": "heading", "attrs": { "level": 2 }, "content": [text("No id")] },
        heading(3, 3, "Three"),
        // A toggle heading is listed like a heading, at its level, with the id of its summary; a
        // plain toggle is not (ADR-0040).
        { "type": "details", "content": [
            { "type": "detailsSummary", "attrs": { "id": id(4), "level": 2 }, "content": [text("Folded")] },
            { "type": "detailsContent", "content": [p("x")] },
        ] },
        { "type": "details", "content": [
            { "type": "detailsSummary", "attrs": { "id": id(5) }, "content": [text("Plain")] },
            { "type": "detailsContent", "content": [p("y")] },
        ] },
        { "type": "breadcrumb" },
        { "type": "pageList", "attrs": { "layout": "table" } },
    ]);
    let html = render(content);
    let toc = format!(
        "<nav class=\"toc\">\n<ul>\n\
         <li class=\"toc-level-1\"><a href=\"#h-{}\">One &amp; only</a></li>\n\
         <li class=\"toc-level-2\"><a href=\"#h-{}\">Two</a></li>\n\
         <li class=\"toc-level-3\"><a href=\"#h-{}\">Three</a></li>\n\
         <li class=\"toc-level-2\"><a href=\"#h-{}\">Folded</a></li>\n\
         </ul>\n</nav>\n",
        id(1),
        id(2),
        id(3),
        id(4)
    );
    assert!(html.starts_with(&toc), "{html}");
    // The breadcrumb and the list view write nothing; the last block is the plain toggle.
    assert!(html.ends_with("</details>\n"), "{html}");
    // The links point at ids that exist.
    let dom = Html::parse_fragment(&html);
    let ids: HashSet<String> = dom
        .select(&Selector::parse("[id]").unwrap())
        .map(|element| element.value().attr("id").unwrap().to_owned())
        .collect();
    for link in dom.select(&Selector::parse("nav.toc a").unwrap()) {
        let target = link.value().attr("href").unwrap().trim_start_matches('#');
        assert!(ids.contains(target), "{target}");
    }
    // No headings, no entries.
    assert_eq!(
        render(json!([{ "type": "tableOfContents" }])),
        "<nav class=\"toc\"></nav>\n"
    );
}

#[test]
fn a_list_with_several_views_writes_nothing_whatever_its_layouts() {
    // The rows are derived (ADR-0016), so the block is empty in the page's HTML: the views are configuration.
    let views = json!([
        { "id": "a", "name": "Table", "layout": "table", "properties": [] },
        { "id": "b", "name": "Schedule", "layout": "calendar", "dateBy": "created" },
        { "id": "c", "name": "Photos", "layout": "gallery", "cardSize": "l" }
    ]);
    let with = render(json!([
        p("before"),
        { "type": "pageList", "attrs": { "layout": "table", "views": views } },
        p("after")
    ]));
    assert_eq!(with, render(json!([p("before"), p("after")])));
    assert!(!with.contains("Schedule") && !with.contains("calendar"));
}

#[test]
fn unknown_nodes_pass_their_children_and_inline_runs_become_paragraphs() {
    assert_eq!(
        render(json!([{ "type": "futureBlock", "attrs": { "x": 1 }, "content": [p("inside")] }])),
        "<p>inside</p>\n"
    );
    assert_eq!(render(json!([{ "type": "futureLeaf" }])), "");
    // Inline nodes among blocks are a paragraph of their own.
    assert_eq!(
        render(
            json!([text("loose "), text("text"), { "type": "hardBreak" }, p("para"), text("tail")])
        ),
        "<p>loose text<br></p>\n<p>para</p>\n<p>tail</p>\n"
    );
}

// ----- marks ------------------------------------------------------------------------------------

#[test]
fn marks_nest_in_a_fixed_order_and_runs_are_merged() {
    let mark = |name: &str| json!({ "type": name });
    assert_eq!(
        inline(json!([
            marked("a", json!([mark("bold")])),
            marked("b", json!([mark("bold")])),
            text(" c")
        ])),
        "<strong>ab</strong> c"
    );
    assert_eq!(
        inline(json!([marked(
            "x",
            json!([
                mark("code"),
                mark("strike"),
                mark("underline"),
                mark("italic"),
                mark("bold")
            ])
        )])),
        "<strong><em><u><s><code>x</code></s></u></em></strong>"
    );
    // The order they are listed in does not matter; marks the exporter does not know are ignored
    // and do not split a run.
    assert_eq!(
        inline(json!([
            marked("a", json!([mark("bold"), mark("sparkle")])),
            marked("b", json!([mark("sparkle"), mark("bold")])),
        ])),
        "<strong>ab</strong>"
    );
    assert_eq!(
        inline(json!([marked(
            "x",
            json!([
                { "type": "textColor", "attrs": { "color": "blue" } },
                { "type": "highlight", "attrs": { "color": "pink" } },
            ])
        )])),
        "<span class=\"text-color\" data-color=\"blue\"><mark data-color=\"pink\">x</mark></span>"
    );
    assert_eq!(
        inline(json!([marked("x", json!([{ "type": "highlight" }]))])),
        "<mark>x</mark>"
    );
    // A colour mark without a colour is nothing.
    assert_eq!(
        inline(json!([marked(
            "x",
            json!([{ "type": "textColor", "attrs": {} }])
        )])),
        "x"
    );
    assert_eq!(
        inline(json!([text("a"), { "type": "hardBreak" }, text("b")])),
        "a<br>b"
    );
}

#[test]
fn only_web_mail_and_phone_links_are_links() {
    let link = |href: &str| {
        inline(json!([marked(
            "text",
            json!([{ "type": "link", "attrs": { "href": href } }])
        )]))
    };
    for href in [
        "https://a.example/?q=1&r=2",
        "http://x.example",
        "HTTPS://X.EXAMPLE",
        "Http://x.example",
        "mailto:a@b.example",
        "MAILTO:a@b.example",
        "tel:+15550100",
    ] {
        assert_eq!(
            link(href),
            format!("<a href=\"{}\">text</a>", escape(href)),
            "{href}"
        );
    }
    assert_eq!(
        link("https://a.example/?q=1&r=2"),
        "<a href=\"https://a.example/?q=1&amp;r=2\">text</a>"
    );
    for href in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        " javascript:alert(1)",
        "\tjavascript:alert(1)",
        "java\nscript:alert(1)",
        "\u{1}javascript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD4=",
        "vbscript:msgbox(1)",
        "file:///etc/passwd",
        "ftp://x.example",
        "//evil.example/x",
        "/absolute",
        "relative.html",
        "#fragment",
        "attachment://5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5",
        " https://leading.space",
        "httpx://x.example",
        "https",
        "",
    ] {
        assert_eq!(link(href), "text", "{href:?}");
    }
    // The link wraps the other marks.
    assert_eq!(
        inline(json!([marked(
            "x",
            json!([
                { "type": "link", "attrs": { "href": "https://a.example" } },
                { "type": "bold" },
            ])
        )])),
        "<a href=\"https://a.example\"><strong>x</strong></a>"
    );
}

#[test]
fn text_and_attributes_are_escaped() {
    assert_eq!(inline(json!([text("<>&\"'")])), "&lt;&gt;&amp;&quot;&#39;");
    assert_eq!(escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&#39;f");
    assert_eq!(escape("plain café 日本語"), "plain café 日本語");
    // Escaping twice does not happen: `&amp;` in the source is shown as it was typed.
    assert_eq!(inline(json!([text("&amp;")])), "&amp;amp;");
}

#[test]
fn local_links_are_relative_or_attachments() {
    for href in [
        "page.md",
        "dir/page.md",
        "../up/page.md",
        "My%20Notes/a.md",
        "a:b",
        "dir/a:b.md",
        "x?q=a:b",
        "#frag",
        "attachment://5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5",
        "",
    ] {
        // `a:b` and `dir/a:b.md`: a colon after a slash is not a scheme; a bare `a:b` is one.
        let expected = href != "a:b";
        assert_eq!(is_local(href), expected, "{href:?}");
    }
    for href in [
        "javascript:alert(1)",
        "http://x.example",
        "mailto:a@b.example",
        "//evil.example",
        "\\\\host\\share",
        "page\n.md",
        "attachment://x\ny",
        "data:text/html,x",
        "x\u{0}y",
    ] {
        assert!(!is_local(href), "{href:?}");
    }
}

// ----- nothing can inject markup ------------------------------------------------------------------

/// Links that answer what a broken or hostile caller might.
struct HostileLinks;

impl ExportLinks for HostileLinks {
    fn page_href(&self, _: PageId) -> Option<String> {
        Some("javascript:alert(1)".to_owned())
    }
    fn page_title(&self, _: PageId) -> Option<String> {
        Some("\"><script>alert(2)</script>".to_owned())
    }
    fn attachment_href(&self, _: AttachmentId) -> Option<String> {
        Some("data:text/html,<script>alert(3)</script>".to_owned())
    }
    fn attachment_name(&self, _: AttachmentId) -> Option<String> {
        Some("x.mp4\" onerror=\"alert(4)".to_owned())
    }
    fn whiteboard_href(&self, _: BlockId) -> Option<String> {
        Some("//evil.example/w.excalidraw".to_owned())
    }
    fn property_name(&self, _: PropertyId) -> Option<String> {
        None
    }
    fn option_name(&self, _: PropertyId, _: OptionId) -> Option<String> {
        None
    }
}

const HOSTILE: [&str; 6] = [
    "</title><script>alert(1)</script>",
    "\"><img src=x onerror=alert(1)>",
    "'><svg/onload=alert(1)>",
    "<!-- --><iframe src=javascript:alert(1)></iframe>",
    "&lt;script&gt;alert(1)&lt;/script&gt; ]]> <![CDATA[ x",
    "javascript:alert(1)",
];

/// The elements and attributes a page may contain: everything else is an injection.
fn assert_inert(html: &str) {
    const ELEMENTS: &[&str] = &[
        "html",
        "head",
        "meta",
        "title",
        "style",
        "body",
        "article",
        "header",
        "h1",
        "h2",
        "h3",
        "table",
        "tbody",
        "tr",
        "th",
        "td",
        "p",
        "ul",
        "ol",
        "li",
        "a",
        "span",
        "div",
        "aside",
        "blockquote",
        "pre",
        "code",
        "hr",
        "details",
        "summary",
        "nav",
        "figure",
        "figcaption",
        "img",
        "video",
        "audio",
        "time",
        "br",
        "strong",
        "em",
        "u",
        "s",
        "mark",
        "input",
    ];
    const ATTRIBUTES: &[&str] = &[
        "lang",
        "charset",
        "name",
        "content",
        "class",
        "id",
        "href",
        "src",
        "alt",
        "style",
        "data-color",
        "data-checked",
        "data-reminder",
        "datetime",
        "start",
        "type",
        "disabled",
        "checked",
        "open",
        "controls",
        "colspan",
        "rowspan",
        "scope",
    ];
    let dom = Html::parse_document(html);
    let all = Selector::parse("*").unwrap();
    let mut styles = 0;
    let mut metas = 0;
    for element in dom.select(&all) {
        let name = element.value().name();
        assert!(ELEMENTS.contains(&name), "unexpected element <{name}>");
        styles += usize::from(name == "style");
        metas += usize::from(name == "meta");
        for (key, value) in element.value().attrs() {
            assert!(
                ATTRIBUTES.contains(&key),
                "unexpected attribute {key}=\"{value}\" on <{name}>"
            );
            match key {
                "href" | "src" => {
                    let local = is_local(value);
                    let web = is_safe_link(value);
                    assert!(local || web, "{key}=\"{value}\" on <{name}>");
                    assert!(
                        !value.trim().to_ascii_lowercase().starts_with("javascript:"),
                        "{value}"
                    );
                }
                "style" => assert!(
                    value
                        .chars()
                        .all(|c| c.is_ascii_digit() || "-:;%wxpidthcolumnsma".contains(c)),
                    "style=\"{value}\""
                ),
                "class" => assert!(
                    value.chars().all(|c| c.is_ascii_alphanumeric()
                        || matches!(c, ' ' | '-' | '_' | '+' | '#' | '.')),
                    "class=\"{value}\""
                ),
                _ => {}
            }
        }
    }
    assert_eq!(styles, 1, "one stylesheet, ours");
    assert_eq!(metas, 3, "charset, viewport, generator");
}

#[test]
fn hostile_content_cannot_inject_markup() {
    for payload in HOSTILE {
        let mut doc_meta = meta();
        doc_meta.title = payload.to_owned();
        doc_meta.icon = Some(payload.to_owned());
        doc_meta.properties = vec![
            ExportProperty {
                name: payload.to_owned(),
                tags: false,
                value: ExportValue::Text(payload.to_owned()),
            },
            ExportProperty {
                name: "Tags".to_owned(),
                tags: true,
                value: ExportValue::Options(vec![payload.to_owned()]),
            },
            ExportProperty {
                name: "Related".to_owned(),
                tags: false,
                value: ExportValue::Pages(vec![LinkedPage {
                    title: payload.to_owned(),
                    href: Some(payload.to_owned()),
                }]),
            },
            ExportProperty {
                name: "Files".to_owned(),
                tags: false,
                value: ExportValue::Files(vec![LinkedFile {
                    name: payload.to_owned(),
                    href: Some(payload.to_owned()),
                }]),
            },
        ];
        let attachment_id = attachment(1).to_string();
        let content = json!({
            "type": "doc",
            "content": [
                { "type": "heading", "attrs": { "id": payload, "level": payload }, "content": [text(payload)] },
                { "type": "paragraph", "content": [
                    text(payload),
                    marked(payload, json!([{ "type": "link", "attrs": { "href": payload } }])),
                    marked("x", json!([{ "type": "link", "attrs": { "href": "javascript:alert(1)" } }])),
                    marked("y", json!([{ "type": "textColor", "attrs": { "color": payload } }, { "type": "highlight", "attrs": { "color": payload } }])),
                    { "type": "pageRef", "attrs": { "pageId": payload } },
                    { "type": "pageRef", "attrs": { "pageId": page(1).to_string() } },
                    { "type": "dateMention", "attrs": { "date": payload, "time": payload, "reminder": payload } },
                    { "type": "personMention", "attrs": { "accountId": payload, "label": payload } },
                    { "type": "mathInline", "attrs": { "latex": payload } },
                ] },
                { "type": "codeBlock", "attrs": { "language": payload }, "content": [text(payload)] },
                { "type": "mathBlock", "content": [text(payload)] },
                { "type": "blockquote", "content": [p(payload)] },
                { "type": "callout", "attrs": { "icon": payload, "color": payload }, "content": [p(payload)] },
                { "type": "details", "attrs": { "open": true }, "content": [
                    { "type": "detailsSummary", "content": [text(payload)] },
                    { "type": "detailsContent", "content": [p(payload)] },
                ] },
                { "type": "bulletList", "content": [{ "type": "listItem", "content": [p(payload)] }] },
                { "type": "orderedList", "attrs": { "start": payload }, "content": [{ "type": "listItem", "content": [p(payload)] }] },
                { "type": "taskList", "content": [{ "type": "taskItem", "attrs": { "checked": payload }, "content": [p(payload)] }] },
                { "type": "table", "content": [{ "type": "tableRow", "content": [
                    { "type": "tableHeader", "attrs": { "colspan": payload, "rowspan": payload }, "content": [p(payload)] },
                ] }] },
                { "type": "image", "attrs": { "attachmentId": payload, "alt": payload, "caption": payload, "align": payload, "widthPercent": payload, "width": payload } },
                { "type": "image", "attrs": { "attachmentId": attachment_id, "alt": payload, "caption": payload } },
                { "type": "file", "attrs": { "attachmentId": payload, "view": "preview" } },
                { "type": "file", "attrs": { "attachmentId": attachment_id, "view": "preview" } },
                { "type": "gallery", "attrs": { "columns": payload, "items": [
                    { "attachmentId": payload, "caption": payload },
                    { "attachmentId": attachment_id, "caption": payload },
                ] } },
                { "type": "whiteboard", "attrs": { "id": id(1) } },
                { "type": "whiteboard", "attrs": { "id": payload } },
                { "type": "pageLink", "attrs": { "pageId": payload } },
                { "type": "columnList", "content": [{ "type": "column", "content": [p(payload)] }] },
                { "type": payload, "content": [p(payload)] },
                { "type": "tableOfContents" },
            ],
            "whiteboards": { id(1): { "a": { "id": "a", "type": "text", "index": "a0", "text": payload } } }
        });
        for links in [&TestLinks::sample() as &dyn ExportLinks, &HostileLinks] {
            let html = page_html(&doc_meta, &content, links);
            assert_inert(&html);
            // The payloads are text, escaped: none of their markup is in the file as markup.
            for raw in ["<script>", "</title><", "<img src=x", "<svg/", "<iframe"] {
                let count = html.matches(raw).count();
                assert_eq!(count, 0, "{raw} in the page for {payload:?}");
            }
        }
    }
}

#[test]
fn links_the_layout_did_not_produce_are_dropped() {
    let content = json!({
        "type": "doc",
        "content": [
            p("x"),
            { "type": "paragraph", "content": [{ "type": "pageRef", "attrs": { "pageId": page(1).to_string() } }] },
            { "type": "image", "attrs": { "attachmentId": attachment(1).to_string() } },
            { "type": "file", "attrs": { "attachmentId": attachment(1).to_string() } },
            { "type": "whiteboard", "attrs": { "id": id(1) } },
        ]
    });
    let html = page_html(&meta(), &content, &HostileLinks);
    assert_inert(&html);
    assert!(!html.contains("javascript:"));
    assert!(!html.contains("data:text"));
    assert!(!html.contains("evil.example"));
    // The page reference is a span, the image falls back to the attachment link.
    assert!(html.contains("<span class=\"page-ref\">"));
    assert!(html.contains(&format!("src=\"attachment://{}\"", attachment(1))));
}

// ----- template buttons (ADR-0056) ----------------------------------------------------------

#[test]
fn a_template_button_writes_nothing_and_its_neighbours_stay() {
    let out = render(json!([
        p("before"),
        {
            "type": "templateButton",
            "attrs": { "label": "Add <task>", "position": "above" },
            "content": [
                p("template words"),
                { "type": "dateMention", "attrs": { "date": "2026-10-18", "relative": "today" } },
                { "type": "image", "attrs": { "attachmentId": "gone", "alt": "hidden alt" } }
            ]
        },
        p("after")
    ]));
    assert_eq!(out, "<p>before</p>\n<p>after</p>\n");
}
