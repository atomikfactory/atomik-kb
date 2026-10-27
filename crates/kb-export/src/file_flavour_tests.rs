//! The file flavour of the Markdown renderer (`kb_model::render_markdown_file`), read back by a
//! CommonMark parser: what is escaped, linked and annotated must come out as the page said it, in
//! the parser the importer uses (`pulldown-cmark` with tables, strikethrough, task lists, math,
//! footnotes and wiki links). The rules themselves are tested in `kb-model`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};

use crate::test_support::{TestLinks, attachment, block, page};

/// The golden of the sample page (`fixtures/doc/page-v1.file.md`).
const GOLDEN: &str = include_str!("../../../fixtures/doc/page-v1.file.md");

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS
}

fn events(markdown: &str) -> Vec<Event<'_>> {
    Parser::new_ext(markdown, options()).collect()
}

fn doc(blocks: Value) -> Value {
    json!({ "type": "doc", "content": blocks })
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn render(blocks: Value) -> String {
    kb_model::render_markdown_file(&doc(blocks), &TestLinks::sample())
}

/// The plain text of the events (soft and hard breaks as newlines), panicking on any markup.
fn only_text(events: &[Event<'_>], context: &str) -> String {
    events
        .iter()
        .map(|event| match event {
            Event::Text(text) => text.to_string(),
            Event::SoftBreak | Event::HardBreak => "\n".to_owned(),
            other => panic!("markup {other:?} in {context:?}"),
        })
        .collect()
}

/// A paragraph of one text node as the parser reads it back.
fn read_paragraph(words: &str) -> String {
    let markdown = render(json!([{ "type": "paragraph", "content": [text(words)] }]));
    let events = events(&markdown);
    assert!(
        matches!(events.first(), Some(Event::Start(Tag::Paragraph))),
        "{words:?} wrote {markdown:?}, read as {events:?}"
    );
    assert!(matches!(events.last(), Some(Event::End(TagEnd::Paragraph))));
    only_text(&events[1..events.len() - 1], &markdown)
}

const TEXTS: [&str; 60] = [
    "plain words",
    "# heading",
    "## two",
    "######## seven",
    "#hashtag",
    "+ plus",
    "- minus",
    "-- two dashes",
    "---",
    "===",
    "= equals",
    "1. one",
    "12) twelve",
    "007. zeros",
    "1.5 apples",
    "> quote",
    ">> nested",
    "* star",
    "*emphasis* and **strong**",
    "_under_ and __double__",
    "snake_case_word and 2*3*4",
    "`tick` and ``double``",
    "[link](http://x.example)",
    "[ref]: http://x.example",
    "[^1]: footnote",
    "text[^1]",
    "![img](x.png)",
    "<b>bold</b>",
    "<!-- comment -->",
    "<http://auto.example>",
    "&amp; &copy; &#35; & &",
    "$x$ and $$y$$ and $",
    "~~strike~~ and ~sub~",
    "| a | b |",
    "a \\ b \\* c \\\\ d",
    "ends with a backslash \\",
    "{{braces}} (parens) [[wiki]] [[Title|alias]]",
    "- [ ] task",
    "- [x] done",
    "* * *",
    "___",
    "***",
    "|",
    "!",
    "\"quoted\" and 'single'",
    "tab\there",
    "emoji 👩‍💻 日本語 café",
    "<script>alert(1)</script>",
    "<img src=x onerror=alert(1)>",
    "1) paren",
    "2026-10-07 and a:b",
    "http://example.com/a_b_c?x=1&y=2",
    "```fence```",
    "~~~ tilde fence",
    "    four spaces inside",
    "  - indented dash",
    "- a\n- b",
    "# a\n## b",
    "a\n1. b",
    "a\n===",
];

#[test]
fn escaped_text_reads_back_as_the_same_text() {
    for words in TEXTS {
        // Leading and trailing white space is not part of a paragraph.
        assert_eq!(read_paragraph(words).trim(), words.trim(), "{words:?}");
    }
}

#[test]
fn text_after_a_hard_break_reads_back_as_text() {
    let paragraph = json!({ "type": "paragraph", "content": [
        text("first"), { "type": "hardBreak" }, text("- second"), { "type": "hardBreak" }, text("1. third"),
    ] });
    let markdown = render(json!([paragraph]));
    let events = events(&markdown);
    assert_eq!(
        only_text(&events[1..events.len() - 1], &markdown),
        "first\n- second\n1. third"
    );
}

#[test]
fn text_in_a_list_item_and_a_quote_stays_text() {
    let item = |words: &str| json!({ "type": "listItem", "content": [{ "type": "paragraph", "content": [text(words)] }] });
    for words in ["- x", "1. x", "# x", "> x", "+ x", "=== x"] {
        let list = json!({ "type": "bulletList", "content": [item(words)] });
        let markdown = render(json!([list]));
        let read = events(&markdown);
        let starts = read
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::List(_))))
            .count();
        assert_eq!(starts, 1, "{words:?} wrote {markdown:?}");
        let texts: String = read
            .iter()
            .filter_map(|e| {
                if let Event::Text(t) = e {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(texts, words, "{markdown:?}");

        let quote = json!({ "type": "blockquote", "content": [{ "type": "paragraph", "content": [text(words)] }] });
        let markdown = render(json!([quote]));
        let read = events(&markdown);
        let quotes = read
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::BlockQuote(_))))
            .count();
        let headings = read
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::Heading { .. })))
            .count();
        let lists = read
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::List(_))))
            .count();
        assert_eq!(
            (quotes, headings, lists),
            (1, 0, 0),
            "{words:?} wrote {markdown:?}"
        );
    }
}

#[test]
fn a_heading_keeps_its_text_and_its_closing_hashes() {
    for words in [
        "Plain", "Issue #", "Issue ##", "#", "# first", "C# and #", "a #b #",
    ] {
        let heading =
            json!({ "type": "heading", "attrs": { "level": 2 }, "content": [text(words)] });
        let markdown = render(json!([heading]));
        let read = events(&markdown);
        assert!(
            matches!(
                read.first(),
                Some(Event::Start(Tag::Heading {
                    level: HeadingLevel::H2,
                    ..
                }))
            ),
            "{words:?} wrote {markdown:?}, read as {read:?}"
        );
        assert_eq!(
            only_text(&read[1..read.len() - 1], &markdown),
            words,
            "{markdown:?}"
        );
    }
}

#[test]
fn code_survives_whatever_it_holds() {
    for code in [
        "a`b",
        "a``b",
        "`x",
        "x`",
        " x ",
        "  ",
        "a*b_c [d] $e$",
        "<b>",
        "```",
        "| pipe |",
    ] {
        let run = json!({ "type": "text", "text": code, "marks": [{ "type": "code" }] });
        let markdown = render(json!([{ "type": "paragraph", "content": [text("see "), run] }]));
        let read = events(&markdown);
        let spans: Vec<String> = read
            .iter()
            .filter_map(|e| {
                if let Event::Code(c) = e {
                    Some(c.to_string())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(spans.len(), 1, "{code:?} wrote {markdown:?}");
        // CommonMark strips one space on each side of a span that has one on both and is not all
        // spaces: the writer pads such code, so it comes back whole.
        assert_eq!(spans[0], code, "{markdown:?}");
    }
    for code in [
        "plain",
        "a\n```\nb",
        "`````",
        "```rust",
        "~~~",
        "<!-- x -->",
    ] {
        let block = json!({ "type": "codeBlock", "attrs": { "language": "rust" }, "content": [text(code)] });
        let markdown = render(json!([block, { "type": "paragraph", "content": [text("after")] }]));
        let read = events(&markdown);
        let body: String = read
            .iter()
            .filter_map(|e| {
                if let Event::Text(t) = e {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(body, format!("{code}\nafter"), "wrote {markdown:?}");
    }
}

#[test]
fn links_and_images_come_back_with_their_destinations() {
    let link = |label: &str, href: &str| {
        let run = json!({ "type": "text", "text": label, "marks": [{ "type": "link", "attrs": { "href": href } }] });
        let markdown = render(json!([{ "type": "paragraph", "content": [run] }]));
        let read = events(&markdown);
        let Some(Event::Start(Tag::Link { dest_url, .. })) = read.get(1) else {
            panic!("{href:?} wrote {markdown:?}, read as {read:?}");
        };
        (
            dest_url.to_string(),
            only_text(&read[2..read.len() - 2], &markdown),
        )
    };
    for href in [
        "https://a.example/x",
        "https://a.example/a_(b)",
        "https://a.example/x y",
        "https://a.example/a)b",
        "https://a.example/<b>",
        "mailto:a@b.example",
    ] {
        assert_eq!(
            link("a [b] *c*", href),
            (href.to_owned(), "a [b] *c*".to_owned()),
            "{href}"
        );
    }

    // A page of the export is a link to its file, one outside it a wiki link.
    let reference =
        |n: u8| json!({ "type": "pageRef", "attrs": { "pageId": page(n).to_string() } });
    let markdown = render(
        json!([{ "type": "paragraph", "content": [text("see "), reference(1), text(" and "), reference(2)] }]),
    );
    let read = events(&markdown);
    assert!(read.iter().any(|e| matches!(e, Event::Start(Tag::Link { dest_url, .. }) if dest_url.as_ref() == "Alpha.md")), "{read:?}");
    assert!(markdown.ends_with("and [[Beta]]\n"), "{markdown}");

    // An image: alt text with brackets and quotes, a caption with quotes and a backslash.
    let image = json!({ "type": "image", "attrs": {
        "attachmentId": attachment(1).to_string(),
        "alt": "a [b] \"c\" \\d",
        "caption": "Figure \"1\": C:\\sync",
    } });
    let markdown = render(json!([image]));
    let read = events(&markdown);
    let Some(Event::Start(Tag::Image {
        dest_url, title, ..
    })) = read.get(1)
    else {
        panic!("{markdown:?} read as {read:?}");
    };
    assert_eq!(dest_url.as_ref(), "dir/diagram.png");
    assert_eq!(title.as_ref(), "Figure \"1\": C:\\sync");
    assert_eq!(
        only_text(&read[2..read.len() - 2], &markdown),
        "a [b] \"c\" \\d"
    );
}

#[test]
fn table_cells_keep_their_pipes() {
    let cell = |blocks: Value| json!({ "type": "tableCell", "content": blocks });
    let code = json!({ "type": "text", "text": "a|b", "marks": [{ "type": "code" }] });
    let table = json!({ "type": "table", "content": [
        { "type": "tableRow", "content": [
            cell(json!([{ "type": "paragraph", "content": [text("a | b")] }])),
            cell(json!([{ "type": "paragraph", "content": [code] }])),
        ] },
        { "type": "tableRow", "content": [
            cell(json!([{ "type": "paragraph", "content": [text("1. x")] }])),
            cell(json!([{ "type": "paragraph", "content": [text("- y")] }])),
        ] },
    ] });
    let markdown = render(json!([table]));
    let read = events(&markdown);
    let cells: Vec<String> = {
        let mut cells = Vec::new();
        let mut current = String::new();
        for event in &read {
            match event {
                Event::Start(Tag::TableCell) => current.clear(),
                Event::End(TagEnd::TableCell) => cells.push(std::mem::take(&mut current)),
                Event::Text(t) | Event::Code(t) => current.push_str(t),
                _ => {}
            }
        }
        cells
    };
    // The header row is the first row: two cells, then the second row's two.
    assert_eq!(cells, ["a | b", "a|b", "1. x", "- y"], "{markdown}");
}

#[test]
fn a_date_mention_is_inline_html() {
    let mention = json!({ "type": "dateMention", "attrs": { "date": "2026-10-08", "time": "14:30", "reminder": 15 } });
    let markdown = render(json!([{ "type": "paragraph", "content": [text("due "), mention] }]));
    let read = events(&markdown);
    let inline: Vec<String> = read
        .iter()
        .filter_map(|e| {
            if let Event::InlineHtml(h) = e {
                Some(h.to_string())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        inline,
        [
            "<time datetime=\"2026-10-08T14:30\" data-reminder=\"15\">",
            "</time>"
        ]
    );
}

/// A person mention as the editor stores it (ADR-0049).
fn mention(label: &str) -> Value {
    json!({ "type": "personMention", "attrs": {
        "accountId": "0b7e5d3a-91c4-4f68-8a2d-6c1f9e4b7a30", "label": label
    } })
}

/// Names a member can give themselves (self-asserted, 1 to 100 characters): whatever they hold
/// must come back as the text `@label`, never as markup, a link, a table or an annotation.
const LABELS: [&str; 24] = [
    "Ann Lee",
    "Ann *the* Great",
    "**Ann**",
    "a_b_c",
    "__Ann__",
    "`Ann`",
    "[Ann](http://x.example)",
    "[[Ann]]",
    "<b>Ann</b>",
    "<script>alert(1)</script>",
    "<img src=x onerror=alert(1)>",
    "<!-- atomik:{\"node\":\"tableOfContents\"} -->",
    "Ann & Co &amp; &copy;",
    "$Ann$ and $$Bo$$",
    "~~Ann~~",
    "a|b",
    "| a | b |",
    "a \\ b \\* c \\\\ d",
    "ends with a backslash \\",
    "# Ann #",
    "- Ann",
    "1. Ann",
    "emoji 👩‍💻 日本語 café",
    "http://example.com/a_b_c?x=1&y=2",
];

#[test]
fn a_person_mention_reads_back_as_its_label() {
    for label in LABELS {
        let markdown =
            render(json!([{ "type": "paragraph", "content": [text("cc "), mention(label)] }]));
        let read = events(&markdown);
        assert!(
            matches!(read.first(), Some(Event::Start(Tag::Paragraph))),
            "{label:?} wrote {markdown:?}, read as {read:?}"
        );
        assert!(matches!(read.last(), Some(Event::End(TagEnd::Paragraph))));
        assert_eq!(
            only_text(&read[1..read.len() - 1], &markdown),
            format!("cc @{label}"),
            "{label:?} wrote {markdown:?}"
        );
        // The account id is not in the file.
        assert!(!markdown.contains("0b7e5d3a"), "{markdown}");
    }
}

#[test]
fn a_person_mention_keeps_its_label_in_a_heading_a_list_and_a_table() {
    for label in LABELS {
        // A heading: the `#`s at the end of the text are escaped, so the label cannot close it.
        let heading = json!({ "type": "heading", "attrs": { "level": 2 }, "content": [text("by "), mention(label)] });
        let markdown = render(json!([heading]));
        let read = events(&markdown);
        assert!(
            matches!(
                read.first(),
                Some(Event::Start(Tag::Heading {
                    level: HeadingLevel::H2,
                    ..
                }))
            ),
            "{label:?} wrote {markdown:?}, read as {read:?}"
        );
        assert_eq!(
            only_text(&read[1..read.len() - 1], &markdown),
            format!("by @{label}"),
            "{label:?} wrote {markdown:?}"
        );

        // A list item that starts with the mention.
        let item = json!({ "type": "listItem", "content": [{ "type": "paragraph", "content": [mention(label), text(" owns it")] }] });
        let markdown = render(json!([{ "type": "bulletList", "content": [item] }]));
        let read = events(&markdown);
        let lists = read
            .iter()
            .filter(|e| matches!(e, Event::Start(Tag::List(_))))
            .count();
        assert_eq!(lists, 1, "{label:?} wrote {markdown:?}");
        let texts: String = read
            .iter()
            .filter_map(|e| {
                if let Event::Text(t) = e {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(texts, format!("@{label} owns it"), "{markdown:?}");

        // A table cell: the label keeps its pipes and does not add a cell.
        let cell = |blocks: Value| json!({ "type": "tableCell", "content": blocks });
        let table = json!({ "type": "table", "content": [
            { "type": "tableRow", "content": [
                cell(json!([{ "type": "paragraph", "content": [mention(label)] }])),
                cell(json!([{ "type": "paragraph", "content": [text("x")] }])),
            ] },
        ] });
        let markdown = render(json!([table]));
        let read = events(&markdown);
        let mut cells = Vec::new();
        let mut current = String::new();
        for event in &read {
            match event {
                Event::Start(Tag::TableCell) => current.clear(),
                Event::End(TagEnd::TableCell) => cells.push(std::mem::take(&mut current)),
                Event::Text(t) | Event::Code(t) => current.push_str(t),
                Event::SoftBreak | Event::HardBreak => current.push('\n'),
                _ => {}
            }
        }
        assert_eq!(
            cells,
            [format!("@{label}"), "x".to_owned()],
            "{label:?} wrote {markdown:?}"
        );
    }
}

#[test]
fn a_mention_without_a_label_is_nothing() {
    for attrs in [
        json!({}),
        json!({ "label": "" }),
        json!({ "label": " \n " }),
        json!({ "label": 3 }),
        json!({ "accountId": "x" }),
    ] {
        let node = json!({ "type": "personMention", "attrs": attrs });
        let markdown = render(json!([{ "type": "paragraph", "content": [text("cc "), node] }]));
        assert_eq!(markdown.trim_end(), "cc", "{markdown:?}");
    }
}

// ----- annotations ---------------------------------------------------------------------------------

/// The HTML blocks of the file that are annotations, parsed.
fn annotations_of(markdown: &str) -> Vec<Value> {
    let mut found = Vec::new();
    for event in events(markdown) {
        if let Event::Html(html) = event
            && html.contains("<!--")
        {
            // One comment per HTML block, and it is the annotation line alone.
            let line = html.trim_end();
            assert_eq!(line.matches("<!--").count(), 1, "{html:?}");
            assert_eq!(line.matches("-->").count(), 1, "{html:?}");
            let json = line
                .strip_prefix(kb_model::ANNOTATION_OPEN)
                .and_then(|rest| rest.strip_suffix(kb_model::ANNOTATION_CLOSE))
                .unwrap_or_else(|| panic!("not an annotation: {html:?}"));
            found.push(serde_json::from_str(json).unwrap());
        }
    }
    found
}

#[test]
fn the_golden_reads_back_as_markdown_with_annotations_of_their_own() {
    let read = events(GOLDEN);
    let in_source = GOLDEN
        .lines()
        .filter(|line| line.starts_with(kb_model::ANNOTATION_OPEN))
        .count();
    let annotations = annotations_of(GOLDEN);
    assert_eq!(annotations.len(), in_source);
    assert!(
        in_source >= 10,
        "the sample has an annotation of nearly every kind"
    );
    // Annotations never end up inside text.
    for event in &read {
        if let Event::Text(t) = event {
            assert!(!t.contains("atomik:"), "{t:?}");
        }
    }
    // The kinds that were written, in order.
    let kinds: Vec<String> = annotations
        .iter()
        .map(|a| {
            a.get("node")
                .or_else(|| a.get("end"))
                .and_then(Value::as_str)
                .map_or_else(|| "attrs".to_owned(), str::to_owned)
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "callout",
            "attrs",
            "attrs",
            "gallery",
            "whiteboard",
            "columnList",
            "column",
            "column",
            "columnList",
            // The template button (ADR-0056): its opening and its end.
            "templateButton",
            "templateButton",
            "tableOfContents",
            "breadcrumb",
            "pageList"
        ]
    );
    // The images and links of the sample are real ones, with the caption unescaped.
    let images: Vec<(String, String)> = read
        .iter()
        .filter_map(|e| match e {
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => Some((dest_url.to_string(), title.to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 4);
    assert_eq!(
        images[0],
        (
            "Welcome/diagram.png".to_owned(),
            "Figure 1: the \"sync\" flow, C:\\sync".to_owned()
        )
    );
    assert_eq!(
        images[3].0,
        "attachment://8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708"
    );
    let links: Vec<String> = read
        .iter()
        .filter_map(|e| {
            if let Event::Start(Tag::Link { dest_url, .. }) = e {
                Some(dest_url.to_string())
            } else {
                None
            }
        })
        .collect();
    assert!(links.contains(&"Linked%20page.md".to_owned()));
    assert!(links.contains(&"Welcome/Quarterly%20report.pdf".to_owned()));
}

#[test]
fn annotations_in_quotes_and_lists_are_html_blocks_too() {
    let callout = |icon: &str| {
        json!({ "type": "callout", "attrs": { "icon": icon }, "content": [
            { "type": "paragraph", "content": [text("outer")] },
            { "type": "callout", "attrs": { "icon": "B" }, "content": [{ "type": "paragraph", "content": [text("inner")] }] },
        ] })
    };
    let markdown = render(json!([callout("A")]));
    let found = annotations_of(&markdown);
    assert_eq!(
        found,
        [
            json!({ "node": "callout", "attrs": { "icon": "A" } }),
            json!({ "node": "callout", "attrs": { "icon": "B" } }),
        ]
    );

    let list = json!({ "type": "bulletList", "content": [{ "type": "listItem", "content": [
        { "type": "paragraph", "content": [text("item")] },
        { "type": "image", "attrs": { "attachmentId": attachment(1).to_string(), "align": "left" } },
    ] }] });
    let markdown = render(json!([list]));
    assert_eq!(
        annotations_of(&markdown),
        [json!({ "attrs": { "align": "left" } })]
    );
    // The image is the item's, after the annotation.
    let read = events(&markdown);
    assert!(
        read.iter()
            .any(|e| matches!(e, Event::Start(Tag::Image { .. }))),
        "{read:?}"
    );
}

#[test]
fn an_annotation_with_hostile_names_stays_one_comment() {
    // A property name that tries to close the comment and start markup.
    let mut links = TestLinks::sample();
    let property = kb_types::PropertyId::from_random_bytes([9; 16]);
    links.properties.insert(
        property,
        "--> <script>alert(1)</script> <!-- ---".to_owned(),
    );
    let list = json!({ "type": "pageList", "attrs": { "properties": [property.to_string()] } });
    let markdown = kb_model::render_markdown_file(
        &doc(json!([list, { "type": "paragraph", "content": [text("after")] }])),
        &links,
    );
    let found = annotations_of(&markdown);
    assert_eq!(
        found,
        [
            json!({ "node": "pageList", "attrs": { "properties": ["--> <script>alert(1)</script> <!-- ---"] } })
        ]
    );
    // The paragraph after it is still a paragraph, and no stray HTML came out.
    let read = events(&markdown);
    assert_eq!(
        read.iter().filter(|e| matches!(e, Event::Html(_))).count(),
        1,
        "{read:?}"
    );
    assert!(
        read.iter()
            .any(|e| matches!(e, Event::Text(t) if t.as_ref() == "after"))
    );
}

#[test]
fn the_views_of_a_list_are_one_annotation_that_cannot_close_its_comment() {
    // Names are the user's words: a view's name, a property's name, a filter's option all go into the
    // annotation's JSON, where `--` is escaped, so none of them can end the comment or start markup.
    let mut links = TestLinks::sample();
    let status = kb_types::PropertyId::from_random_bytes([9; 16]);
    let done = kb_types::OptionId::from_random_bytes([8; 16]);
    links
        .properties
        .insert(status, "--> <b>Status</b> <!-- ---".to_owned());
    links
        .options
        .insert((status, done), "--> done <script>".to_owned());
    let (status_id, done_id) = (status.to_string(), done.to_string());
    let list = json!({ "type": "pageList", "attrs": {
        "layout": "table",
        "views": [
            { "id": "k3f9a2bc", "name": "--> <script>alert(1)</script> <!--", "layout": "table",
              "properties": [status_id], "sort": { "by": status_id, "dir": "asc" },
              "filter": { "op": "or", "items": [
                  { "propertyId": status_id, "op": "hasOption", "value": done_id },
                  { "op": "and", "items": [{ "propertyId": status_id, "op": "isSet" }] } ] } },
            { "id": "7c0a55d1", "name": "Calendar", "layout": "calendar", "properties": [],
              "sort": { "by": "title", "dir": "asc" }, "dateBy": status_id, "endBy": status_id }
        ]
    }});
    let markdown = kb_model::render_markdown_file(
        &doc(json!([list, { "type": "paragraph", "content": [text("after")] }])),
        &links,
    );
    let found = annotations_of(&markdown);
    assert_eq!(
        found.len(),
        1,
        "one annotation for the block and all its views: {markdown:?}"
    );
    let views = &found[0]["attrs"]["views"];
    assert_eq!(views[0]["name"], "--> <script>alert(1)</script> <!--");
    assert_eq!(
        views[0]["properties"],
        json!(["--> <b>Status</b> <!-- ---"])
    );
    assert_eq!(
        views[0]["filter"],
        json!({ "op": "or", "items": [
            { "property": "--> <b>Status</b> <!-- ---", "op": "hasOption", "value": "--> done <script>" },
            { "op": "and", "items": [{ "property": "--> <b>Status</b> <!-- ---", "op": "isSet" }] } ] })
    );
    assert_eq!(views[1]["dateBy"], "--> <b>Status</b> <!-- ---");
    // No id of the schema is left, and the paragraph after it is still a paragraph.
    assert!(!markdown.contains(&status_id) && !markdown.contains(&done_id));
    let read = events(&markdown);
    assert_eq!(
        read.iter().filter(|e| matches!(e, Event::Html(_))).count(),
        1,
        "{read:?}"
    );
    assert!(
        read.iter()
            .any(|e| matches!(e, Event::Text(t) if t.as_ref() == "after"))
    );
}

#[test]
fn the_widgets_of_a_dashboard_are_named_in_the_annotation_and_keep_what_they_do_not_know() {
    // The ids of the schema never leave: a widget's filter, the column of a number, the grouping and the measure
    // of a chart are written with names; what has no name falls back as a move does, and a name that tries to
    // close the comment stays inside it.
    let mut links = TestLinks::sample();
    let hours = kb_types::PropertyId::from_random_bytes([9; 16]);
    let stage = kb_types::PropertyId::from_random_bytes([10; 16]);
    let gone = kb_types::PropertyId::from_random_bytes([11; 16]);
    let done = kb_types::OptionId::from_random_bytes([8; 16]);
    links.properties.insert(hours, "Hours --> <b>".to_owned());
    links.properties.insert(stage, "Stage".to_owned());
    links.options.insert((stage, done), "Done".to_owned());
    let (hours_id, stage_id, gone_id, done_id) = (
        hours.to_string(),
        stage.to_string(),
        gone.to_string(),
        done.to_string(),
    );
    let list = json!({ "type": "pageList", "attrs": {
        "layout": "dashboard",
        "views": [
            { "id": "d", "name": "Overview", "layout": "dashboard", "properties": [],
              "widgets": [
                { "id": "w0000001", "kind": "number", "title": "", "w": 1, "h": "s",
                  "calc": { "by": hours_id, "fn": "sum" } },
                { "id": "w0000002", "kind": "number", "title": "", "w": 1, "h": "s",
                  "calc": { "by": "title", "fn": "count_all" },
                  "filter": { "op": "and", "items": [
                      { "propertyId": stage_id, "op": "hasOption", "value": done_id },
                      { "propertyId": gone_id, "op": "isSet" } ] } },
                { "id": "w0000003", "kind": "chart", "title": "", "w": 2, "h": "m",
                  "chart": { "type": "bar", "groupBy": stage_id, "bucket": "week",
                             "measure": { "fn": "sum", "propertyId": hours_id }, "future": [1] } },
                { "id": "w0000004", "kind": "chart",
                  "chart": { "type": "line", "groupBy": "created", "measure": { "fn": "count" } } },
                { "id": "w0000005", "kind": "number", "calc": { "by": gone_id, "fn": "sum" },
                  "filter": { "op": "or", "items": [{ "propertyId": gone_id, "op": "isSet" }] } },
                { "id": "w0000006", "kind": "chart",
                  "chart": { "type": "donut", "groupBy": gone_id,
                             "measure": { "fn": "average", "propertyId": gone_id } } },
                { "id": "w0000007", "kind": "view", "viewId": "k3f9a2bc" },
                { "id": "w0000008", "kind": "newer", "unknown": { "ids": [hours_id] } },
                "not a widget"
              ] }
        ]
    }});
    let markdown = kb_model::render_markdown_file(
        &doc(json!([list, { "type": "paragraph", "content": [text("after")] }])),
        &links,
    );
    let found = annotations_of(&markdown);
    assert_eq!(found.len(), 1, "one annotation for the block: {markdown:?}");
    let widgets = found[0]["attrs"]["views"][0]["widgets"].as_array().unwrap();
    assert_eq!(widgets.len(), 9);
    assert_eq!(
        widgets[0]["calc"],
        json!({ "by": "Hours --> <b>", "fn": "sum" })
    );
    assert_eq!(
        widgets[1]["calc"],
        json!({ "by": "title", "fn": "count_all" })
    );
    assert_eq!(
        widgets[1]["filter"],
        json!({ "op": "and", "items": [{ "property": "Stage", "op": "hasOption", "value": "Done" }] })
    );
    assert_eq!(
        widgets[2]["chart"],
        json!({ "type": "bar", "groupBy": "Stage", "bucket": "week",
                "measure": { "fn": "sum", "propertyId": "Hours --> <b>" }, "future": [1] })
    );
    assert_eq!(
        widgets[3]["chart"],
        json!({ "type": "line", "groupBy": "created", "measure": { "fn": "count" } })
    );
    // No name for the property: the number counts all pages, its emptied filter goes, the chart asks and counts.
    assert_eq!(
        widgets[4]["calc"],
        json!({ "by": "title", "fn": "count_all" })
    );
    assert!(widgets[4].get("filter").is_none());
    assert_eq!(
        widgets[5]["chart"],
        json!({ "type": "donut", "groupBy": null, "measure": { "fn": "count" } })
    );
    assert_eq!(
        widgets[6],
        json!({ "id": "w0000007", "kind": "view", "viewId": "k3f9a2bc" })
    );
    // A widget of a newer version keeps its keys as they are (here an id inside is its own business).
    assert_eq!(widgets[7]["kind"], "newer");
    assert_eq!(widgets[8], json!("not a widget"));
    assert!(
        !markdown.contains(&stage_id)
            && !markdown.contains(&done_id)
            && !markdown.contains(&gone_id)
    );
    let read = events(&markdown);
    assert_eq!(
        read.iter().filter(|e| matches!(e, Event::Html(_))).count(),
        1,
        "{read:?}"
    );
}

#[test]
fn a_whiteboard_annotation_points_at_its_file_and_keeps_its_summary() {
    let content = json!({
        "type": "doc",
        "content": [{ "type": "whiteboard", "attrs": { "id": block(1).to_string(), "height": 480 } }],
        "whiteboards": { block(1).to_string(): { "a": { "id": "a", "type": "text", "index": "a0", "originalText": "# Plan" } } }
    });
    let markdown = kb_model::render_markdown_file(&content, &TestLinks::sample());
    assert_eq!(
        annotations_of(&markdown),
        [
            json!({ "node": "whiteboard", "attrs": { "height": 480 }, "file": "dir/Whiteboard.excalidraw" })
        ]
    );
    let read = events(&markdown);
    assert!(
        read.iter()
            .any(|e| matches!(e, Event::Start(Tag::BlockQuote(_))))
    );
    // The summary's text is text, not a heading.
    assert!(
        !read
            .iter()
            .any(|e| matches!(e, Event::Start(Tag::Heading { .. })))
    );
}
