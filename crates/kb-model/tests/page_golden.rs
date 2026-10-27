//! Golden fixtures of page document shape v1 (`fixtures/doc/page-v1*`, written by
//! `cargo run -p kb-model --example make_golden` from the portable JSON inputs).
//!
//! These tests freeze the page shape (ADR-0016): snapshots written with shape v1 must
//! keep loading into the same portable JSON and Markdown forever, and the TypeScript
//! side must read and write the same shape (`page-v1.from-js.snapshot.bin`, written by
//! `core-client`'s `portableToLoro`).
//!
//! **If one of these tests fails, the format changed.** Do not regenerate the fixtures
//! to make it pass: bump `kb_doc::FORMAT_VERSION` (and `MIN_READER_VERSION` if old
//! readers cannot read the new shape), keep a reader for v1, and add v2 fixtures next to
//! these (`docs/development/milestone-03.md`, "Security and migration notes").
//!
//! The one exception is an additive node kind (ADR-0016, amendment of 2026-10-04): the
//! containers do not change, older readers pass the node through, and the sample gains
//! the node so that every node of the schema stays in it. Milestone 5 added `image` and
//! `file` that way and regenerated `page-v1.snapshot.bin`, `page-v1.md` and the
//! TypeScript snapshot from the extended `page-v1.portable.json`; the simple table
//! (`table`, `tableRow`, `tableCell`, `tableHeader`, ADR-0016 amendment of 2026-10-05)
//! did the same. So did an additive attribute with a default of `null` (not stored): the
//! callout's `color` (ADR-0016 amendment of 2026-10-06) is set to `blue` on the sample's
//! callout, the two snapshots were regenerated and `page-v1.md` did not change (Markdown
//! ignores the colour). The math block (`mathBlock`, ADR-0016 amendment of 2026-10-06) is
//! one more additive node: the sample gained one and everything was regenerated. So did
//! the columns layout (`columnList` and `column`, ADR-0016 amendment of 2026-10-06): the
//! sample gained a two-column list and everything was regenerated. So did the page link
//! block (`pageLink`, the card that links to a page, ADR-0016 amendment of 2026-10-06). So
//! did the derived blocks (`tableOfContents` and `breadcrumb`, ADR-0016 amendment of
//! 2026-10-06): the sample gained one of each at its end, where their empty Markdown
//! vanishes with the trailing newlines, so `page-v1.md` did not change. So did the two
//! colour marks (`textColor` and `highlight`, each with a `color` attribute, ADR-0016
//! amendment of 2026-10-07): the sample's last paragraph gained a coloured run, a
//! highlighted run and one that is both (the text is the same, so `page-v1.md` did not
//! change: Markdown renders no colour), and the two snapshots were regenerated. So did the
//! date mention (`dateMention { id, date, time? }`, an inline atom, ADR-0016 amendment of
//! 2026-10-07): the sample's second list item gained two (a day, and a day with a time), so
//! `page-v1.md` changed by their ISO text, and the two snapshots and the schema fixture were
//! regenerated. So did the inline equation (`mathInline { id, latex }`, an inline atom, ADR-0016
//! amendment of 2026-10-07): the sample's blockquote paragraph gained one (`E=mc^2`), so
//! `page-v1.md` changed by `$E=mc^2$` and the same three files were regenerated. So did the
//! `reminder` attribute of the date mention (ADR-0016 amendment of 2026-10-07, ADR-0024): the
//! sample's two mentions carry one (1440 and 15 minutes before), `page-v1.md` did not change (the
//! reminder is not text), and the two snapshots and the schema fixture were regenerated. So did the
//! list view (`pageList { id, source, layout, properties, sort }`, a derived leaf block with the
//! configuration of its view, ADR-0016 amendment of 2026-10-07): the sample gained one at its end
//! (a tag source, two properties, sorted descending by the second), where its empty Markdown
//! vanishes with the trailing newlines, so `page-v1.md` did not change, and the same files were
//! regenerated. So did the three optional attributes of `image` (`widthPercent`, `align` and
//! `caption`, ADR-0016 amendment of 2026-10-09, ADR-0028): the sample's image is 60 % wide,
//! aligned right and captioned, `page-v1.md` gained the caption as the link title, and the two
//! snapshots and the schema fixture were regenerated. So did the photo gallery (`gallery { id, items,
//! columns }`, a leaf block, ADR-0016 amendment of 2026-10-09, ADR-0030): the sample gained one
//! after the file block (three pictures, two captioned), `page-v1.md` gained its three image
//! lines, and the same files were regenerated. So did the `view` attribute of `file` (ADR-0016
//! amendment of 2026-10-09, ADR-0031): the sample's file block is shown as a preview, `page-v1.md`
//! did not change (the Markdown is the file link), and the two snapshots and the schema fixture
//! were regenerated. So did the `level` attribute of `detailsSummary` (ADR-0016 amendment of
//! 2026-10-14, ADR-0040): the sample's toggle is a toggle heading of level 2, `page-v1.md` and
//! `page-v1.file.md` gained the `<h2>` inside the summary, and the two snapshots and the schema
//! fixture were regenerated. So did the template button (`templateButton { id, label, position }`, content
//! `block+`, ADR-0016 amendment of 2026-10-18, ADR-0056) and the `relative` attribute of the date and person
//! mentions: the sample gained a button before its three derived blocks (above, labelled, holding one paragraph with a
//! person mention of `me` and a date mention of `today` that has a reminder), where its empty Markdown vanishes with
//! theirs at the end of the page, so `page-v1.md` did not change, `page-v1.file.md` gained the annotated content, and the two snapshots
//! and the schema fixture were regenerated.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

#[path = "support/page_golden.rs"]
mod page_golden;

use std::collections::{BTreeMap, BTreeSet};

use kb_doc::{
    BlobHeader, BlobKind, Doc, DocKind, ENGINE_LORO, MarkExpand, decode_blob, encode_blob,
};
use kb_model::{BlockKind, Inline, PageDoc, PageLink, schema};
use kb_types::{AttachmentId, BlockId, PageId};
use loro::{Container, LoroList, LoroMap, LoroText, LoroValue, TextDelta, ValueOrContainer};
use page_golden::{PAGE_JSON, UNKNOWN_JSON, json, markdown, title_of};

const PAGE_SNAPSHOT: &[u8] = include_bytes!("../../../fixtures/doc/page-v1.snapshot.bin");
const UNKNOWN_SNAPSHOT: &[u8] =
    include_bytes!("../../../fixtures/doc/page-v1-unknown.snapshot.bin");
const PAGE_MARKDOWN: &str = include_str!("../../../fixtures/doc/page-v1.md");
const BLOCK_SCHEMA: &str = include_str!("../../../fixtures/doc/block-schema-v1.json");
/// Written by the TypeScript side; read at run time because it may not exist yet.
const FROM_JS_SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/doc/page-v1.from-js.snapshot.bin"
);

const V1_SNAPSHOT_HEADER: BlobHeader = BlobHeader {
    engine: ENGINE_LORO,
    format_version: 1,
    min_reader_version: 1,
    kind: BlobKind::Snapshot,
};

/// The paragraph of the sample that holds both page references.
const LINKING_PARAGRAPH: &str = "1a9e4c3f-8d2b-4f1c-8b6e-2c3d4e5f6071";

fn load(snapshot: &[u8]) -> PageDoc {
    PageDoc::from_doc(Doc::load(DocKind::Page, 0xfeed, Some(snapshot), &[]).unwrap()).unwrap()
}

fn block(text: &str) -> BlockId {
    text.parse().unwrap()
}

fn page_id(text: &str) -> PageId {
    text.parse().unwrap()
}

fn attachment(text: &str) -> AttachmentId {
    text.parse().unwrap()
}

fn unix(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn assert_sound(page: &PageDoc) {
    let problems = page.check_invariants();
    assert!(problems.is_empty(), "invariant violations: {problems:#?}");
}

/// The TypeScript snapshot as an enveloped blob, or `None` while it does not exist.
fn js_snapshot() -> Option<Vec<u8>> {
    let Ok(bytes) = std::fs::read(FROM_JS_SNAPSHOT) else {
        println!("{FROM_JS_SNAPSHOT} does not exist yet; cross-runtime check skipped");
        return None;
    };
    // The editor side writes raw Loro bytes; the core persists them enveloped.
    Some(if bytes.starts_with(b"KBD1") {
        bytes
    } else {
        encode_blob(BlobKind::Snapshot, &bytes)
    })
}

// ----- the frozen shape ---------------------------------------------------------------------

#[test]
fn page_v1_blob_has_the_v1_envelope() {
    for snapshot in [PAGE_SNAPSHOT, UNKNOWN_SNAPSHOT] {
        let (header, payload) = decode_blob(snapshot).unwrap();
        assert_eq!(header, V1_SNAPSHOT_HEADER);
        assert!(!payload.is_empty());
    }
}

#[test]
fn page_v1_snapshot_loads_into_the_portable_json() {
    let page = load(PAGE_SNAPSHOT);
    assert_eq!(page.to_portable(), json(PAGE_JSON));
    assert_sound(&page);
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert!(!page.is_empty());
}

#[test]
fn page_v1_markdown_is_the_golden() {
    let page = load(PAGE_SNAPSHOT);
    assert_eq!(unix(&markdown(&page)), unix(PAGE_MARKDOWN));
    // The table is a GFM table: separator after the first row, `|` escaped, paragraphs of
    // a cell joined by `<br>`.
    assert!(unix(PAGE_MARKDOWN).contains(
        "\n\n| Feature | Status | Notes |\n| --- | --- | --- |\n\
         | Tables | **Done** | Header row<br><br>Plain cells |\n\
         | Pipes | a \\| b | Escaped in Markdown |\n\n"
    ));
    // The sample's callout is blue (ADR-0016, amendment of 2026-10-06): the colour is in the
    // document and not in the Markdown.
    let portable = page.to_portable();
    let callout = portable["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["type"] == "callout")
        .unwrap();
    assert_eq!(callout["attrs"]["color"], "blue");
    assert_eq!(callout["attrs"]["icon"], "💡");
    assert!(unix(PAGE_MARKDOWN).contains("\n\n> 💡 Callouts carry an icon.\n\n"));
    assert!(!PAGE_MARKDOWN.contains("blue"));
    // The math block (ADR-0016, amendment of 2026-10-06) is `$$`, its LaTeX and `$$`.
    let math = portable["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["type"] == "mathBlock")
        .unwrap();
    assert_eq!(math["attrs"]["id"], page_golden::MATH_BLOCK);
    assert_eq!(math["content"][0]["text"], "E = mc^2");
    assert!(unix(PAGE_MARKDOWN).contains("\n\n$$\nE = mc^2\n$$\n\n"));
    // The columns (ADR-0016, amendment of 2026-10-06) are linearised: each column's blocks
    // joined by blank lines, the columns joined by blank lines too, nothing marking them.
    assert!(unix(PAGE_MARKDOWN).contains(
        "\n\n[Quarterly report.pdf](attachment://6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6)\n\n\
         ![](attachment://5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5 \"Opening night, \\\"north\\\" wing\")\n\n\
         ![](attachment://7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7)\n\n\
         ![](attachment://8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708 \"Backstage\")\n\n\
         > **Whiteboard**\n>\n> - Plan A ship it\n\n\
         Left column\n\nRight column\n\nTwo blocks in this one\n\n"
    ));
    assert!(!PAGE_MARKDOWN.contains("columnList"));
    // The page link card (ADR-0016, amendment of 2026-10-06) is `[[Title]]` on its own line,
    // its block id ignored; unresolved it is the stored page id.
    assert!(
        unix(PAGE_MARKDOWN).contains("\n\nTwo blocks in this one\n\n[[Linked page]]\n\nUnicode: ")
    );
    assert!(!PAGE_MARKDOWN.contains("8e6f3021"));
    // The derived blocks (ADR-0016, amendments of 2026-10-06 and 2026-10-07) are the last three
    // blocks of the sample and render as nothing: the page ends with the Unicode paragraph and
    // one newline.
    let portable_blocks = portable["content"].as_array().unwrap();
    let toc = &portable_blocks[portable_blocks.len() - 3];
    assert_eq!(toc["type"], "tableOfContents");
    assert_eq!(toc["attrs"]["id"], page_golden::TABLE_OF_CONTENTS_BLOCK);
    assert!(toc.get("content").is_none());
    let crumbs = &portable_blocks[portable_blocks.len() - 2];
    assert_eq!(crumbs["type"], "breadcrumb");
    assert_eq!(crumbs["attrs"]["id"], page_golden::BREADCRUMB_BLOCK);
    assert!(crumbs.get("content").is_none());
    // The list view keeps the configuration of its view: a tag source, two properties shown (Tags
    // first), sorted descending by the second, and no content.
    let list = &portable_blocks[portable_blocks.len() - 1];
    assert_eq!(list["type"], "pageList");
    assert_eq!(list["attrs"]["id"], page_golden::PAGE_LIST_BLOCK);
    assert_eq!(
        list["attrs"]["source"],
        serde_json::json!({ "kind": "tag", "optionId": "3c9d1e52-6a47-4b8e-9f20-5d1c7a8b4e61" })
    );
    assert_eq!(list["attrs"]["layout"], "list");
    assert_eq!(
        list["attrs"]["properties"],
        serde_json::json!([
            "0000000a-0000-4000-8000-000000000001",
            "5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74"
        ])
    );
    assert_eq!(
        list["attrs"]["sort"],
        serde_json::json!({ "by": "5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74", "dir": "desc" })
    );
    assert!(list.get("content").is_none());
    assert!(unix(PAGE_MARKDOWN).ends_with("and 𝔘𝔫𝔦𝔠𝔬𝔡𝔢.\n"));
    assert!(!PAGE_MARKDOWN.contains("tableOfContents"));
    assert!(!PAGE_MARKDOWN.contains("breadcrumb"));
    assert!(!PAGE_MARKDOWN.contains("pageList"));
    assert!(!PAGE_MARKDOWN.contains("5b2a7c14"));
    // The date mentions (ADR-0016, amendment of 2026-10-07) are their ISO text, inline: a
    // day, and a day with a time, no brackets.
    assert!(
        unix(PAGE_MARKDOWN)
            .contains("\n- Second item due 2026-10-07, review 2026-10-08 14:30\n\n3. Third step\n")
    );
    assert!(!PAGE_MARKDOWN.contains("dateMention"));
    // The inline equation (ADR-0016, amendment of 2026-10-07) is `$latex$`, inline in its paragraph.
    assert!(unix(PAGE_MARKDOWN).contains("\n\n> A quoted line with $E=mc^2$ inline.\n\n"));
    assert!(!PAGE_MARKDOWN.contains("mathInline"));
    // Unresolved references fall back to the page id, unnamed files to the attachment id.
    let unresolved = page.to_markdown(&|_| None);
    assert!(unresolved.contains("See also [[7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6]] and [["));
    assert!(unresolved.contains(
        "\n\nTwo blocks in this one\n\n[[7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6]]\n\nUnicode: "
    ));
    assert!(unresolved.contains(&format!(
        "\n\n[{id}](attachment://{id})\n\n",
        id = page_golden::FILE_ATTACHMENT
    )));
    // The image has a caption (ADR-0016, amendment of 2026-10-09): the link title, with the `"`
    // and the `\` of the caption escaped.
    assert!(unresolved.contains(&format!(
        "\n\n![Diagram of the sync flow](attachment://{} \"Figure 1: the \\\"sync\\\" flow, C:\\\\sync\")\n\n",
        page_golden::IMAGE_ATTACHMENT
    )));
    // The resolver is asked once, for the file's attachment only.
    let mut asked = Vec::new();
    let named = page.to_markdown_with(&title_of, &mut |id| {
        asked.push(id);
        page_golden::name_of(id)
    });
    assert_eq!(asked, vec![attachment(page_golden::FILE_ATTACHMENT)]);
    assert_eq!(unix(&named), unix(PAGE_MARKDOWN));
}

#[test]
fn portable_json_is_an_identity_through_the_engine() {
    for (text, snapshot) in [(PAGE_JSON, PAGE_SNAPSHOT), (UNKNOWN_JSON, UNKNOWN_SNAPSHOT)] {
        let expected = json(text);
        // JSON -> engine -> JSON.
        let built = PageDoc::from_portable(Doc::new(DocKind::Page, 5), &expected).unwrap();
        assert_eq!(built.to_portable(), expected);
        // Engine -> JSON -> engine -> JSON, from the frozen snapshot.
        let loaded = load(snapshot);
        let rebuilt =
            PageDoc::from_portable(Doc::new(DocKind::Page, 6), &loaded.to_portable()).unwrap();
        assert_eq!(rebuilt.to_portable(), loaded.to_portable());
        assert_sound(&rebuilt);
    }
}

#[test]
fn the_inputs_still_build_the_golden_snapshots() {
    // Semantics check: building the inputs with today's code gives what the frozen
    // snapshots hold (a failure here without a failure above means behaviour changed).
    assert_eq!(
        page_golden::page().to_portable(),
        load(PAGE_SNAPSHOT).to_portable()
    );
    assert_eq!(
        page_golden::unknown_page().to_portable(),
        load(UNKNOWN_SNAPSHOT).to_portable()
    );
}

// ----- what the core derives from the sample ------------------------------------------------

#[test]
fn blocks_links_and_ids_of_the_sample() {
    let page = load(PAGE_SNAPSHOT);
    let blocks = page.blocks();
    assert_eq!(
        blocks.len(),
        75,
        "every node with an id, page references, date mentions and the inline equation, the table's rows and cells, the math block, the columns, the page link card, the three derived blocks and the template button with the paragraph and the date mention inside it included"
    );
    assert_eq!(
        page.block_ids(),
        blocks.iter().map(|b| b.id).collect::<Vec<_>>()
    );
    let count = |kind: &str| blocks.iter().filter(|b| b.kind == kind).count();
    assert_eq!(
        count("paragraph"),
        25,
        "ten of them inside the table's cells, three inside the columns and one inside the template button"
    );
    assert_eq!(count("listItem"), 4);
    assert_eq!(count("table"), 1);
    assert_eq!(count("tableRow"), 3);
    assert_eq!(count("tableHeader"), 3);
    assert_eq!(count("tableCell"), 6);
    assert_eq!(count("columnList"), 1);
    assert_eq!(count("column"), 2);
    assert_eq!(count("pageRef"), 2);
    assert_eq!(
        count("dateMention"),
        3,
        "two in the list item and one inside the template button"
    );
    assert_eq!(count("templateButton"), 1);
    assert_eq!(count("mathInline"), 1);
    assert_eq!(count("pageLink"), 1);
    assert_eq!(count("hardBreak"), 0, "hard breaks carry no id");
    assert_eq!(count("image"), 1);
    assert_eq!(count("file"), 1);
    assert_eq!(count("gallery"), 1);
    assert_eq!(count("whiteboard"), 1);
    assert_eq!(count("mathBlock"), 1);
    assert_eq!(count("tableOfContents"), 1);
    assert_eq!(count("breadcrumb"), 1);
    assert_eq!(count("pageList"), 1);
    for name in schema::ID_BEARING {
        assert!(count(name) > 0, "{name} is in the sample");
    }
    assert_eq!(
        blocks.iter().filter(|b| !b.inlines.is_empty()).count(),
        32,
        "blocks with their own text (the image's caption and alt, the gallery's captions, the whiteboard's text, the table's cell paragraphs, the math block, the columns' paragraphs and the page link card included)"
    );
    // The derived blocks hold no text: they show what the page and the tree hold.
    assert!(
        blocks
            .iter()
            .filter(|b| schema::DERIVED_NODES.contains(&b.kind.as_str()))
            .all(|b| b.inlines.is_empty())
    );
    // A table is no text block itself: its text is the paragraphs of its cells, in
    // reading order (a bold run reads as plain text, a `|` stays), which is what the
    // text index and the backlinks work from.
    let in_table: Vec<String> = {
        let at = blocks
            .iter()
            .position(|b| b.id == block(page_golden::TABLE_BLOCK))
            .unwrap();
        blocks[at..]
            .iter()
            .take_while(|b| b.kind != "image") // the sample's next block after the table
            .filter(|b| b.kind == "paragraph")
            .map(|b| {
                b.inlines
                    .iter()
                    .map(|inline| match inline {
                        Inline::Text(text) => text.clone(),
                        other => panic!("unexpected inline {other:?}"),
                    })
                    .collect()
            })
            .collect()
    };
    assert_eq!(
        in_table,
        [
            "Feature",
            "Status",
            "Notes",
            "Tables",
            "Done",
            "Header row",
            "Plain cells",
            "Pipes",
            "a | b",
            "Escaped in Markdown",
        ]
    );
    assert!(
        blocks
            .iter()
            .filter(|b| schema::TABLE_NODES.contains(&b.kind.as_str()))
            .all(|b| b.inlines.is_empty())
    );
    // The columns hold no text either: their text is the blocks inside them, in reading
    // order (the left column first).
    assert!(
        blocks
            .iter()
            .filter(|b| schema::COLUMN_NODES.contains(&b.kind.as_str()))
            .all(|b| b.inlines.is_empty())
    );
    let at = blocks
        .iter()
        .position(|b| b.id == block(page_golden::COLUMN_LIST_BLOCK))
        .unwrap();
    let layout = &blocks[at..at + 6];
    assert_eq!(
        layout.iter().map(|b| b.kind.as_str()).collect::<Vec<_>>(),
        [
            "columnList",
            "column",
            "paragraph",
            "column",
            "paragraph",
            "paragraph"
        ]
    );
    assert_eq!(
        layout
            .iter()
            .filter_map(|b| match b.inlines.as_slice() {
                [Inline::Text(text)] => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["Left column", "Right column", "Two blocks in this one"]
    );
    let image = blocks
        .iter()
        .find(|b| b.id == block(page_golden::IMAGE_BLOCK))
        .unwrap();
    assert_eq!(image.kind, "image");
    // The caption, then the alt text (ADR-0016, amendment of 2026-10-09); the width and the
    // alignment are not text.
    assert_eq!(
        image.inlines,
        vec![Inline::Text(
            "Figure 1: the \"sync\" flow, C:\\sync Diagram of the sync flow".into()
        )]
    );
    let file = blocks
        .iter()
        .find(|b| b.id == block(page_golden::FILE_BLOCK))
        .unwrap();
    assert_eq!(file.kind, "file");
    assert!(
        file.inlines.is_empty(),
        "a file block has no text of its own"
    );
    assert_eq!(
        page.attachment_refs(),
        vec![
            (
                block(page_golden::IMAGE_BLOCK),
                attachment(page_golden::IMAGE_ATTACHMENT)
            ),
            (
                block(page_golden::FILE_BLOCK),
                attachment(page_golden::FILE_ATTACHMENT)
            ),
            // The gallery lists three pictures, the first of them the image's (ADR-0030).
            (
                block(page_golden::GALLERY_BLOCK),
                attachment(page_golden::IMAGE_ATTACHMENT)
            ),
            (
                block(page_golden::GALLERY_BLOCK),
                attachment(page_golden::GALLERY_SECOND)
            ),
            (
                block(page_golden::GALLERY_BLOCK),
                attachment(page_golden::GALLERY_THIRD)
            ),
        ]
    );

    // The date mentions read as their ISO text inside the text around them, so the text index
    // and the backlinks' text hold them; they are not links.
    let dated = blocks
        .iter()
        .find(|b| b.id == block("c5497f4a-38d6-44c7-9519-d7f8091a2b3c"))
        .unwrap();
    assert_eq!(
        dated.inlines,
        vec![Inline::Text(
            "Second item due 2026-10-07, review 2026-10-08 14:30".into()
        )]
    );
    // The inline equation reads as its LaTeX inside the text around it, so the text index and the
    // backlinks' text hold `E=mc^2`; it is a block of its own with an id and no text, and no link.
    let quoted = blocks
        .iter()
        .find(|b| b.id == block(page_golden::QUOTE_PARAGRAPH))
        .unwrap();
    assert_eq!(
        quoted.inlines,
        vec![Inline::Text("A quoted line with E=mc^2 inline.".into())]
    );
    let equation = blocks
        .iter()
        .find(|b| b.id == block(page_golden::MATH_INLINE_BLOCK))
        .unwrap();
    assert_eq!(equation.kind, "mathInline");
    assert!(equation.inlines.is_empty());
    assert_eq!(blocks[0].kind, "heading");
    assert_eq!(
        blocks[0].inlines,
        vec![Inline::Text("Welcome to atomik-kb".into())]
    );
    let linked = page_id(page_golden::LINKED_PAGE);
    let linking = blocks
        .iter()
        .find(|b| b.id == block(LINKING_PARAGRAPH))
        .unwrap();
    assert_eq!(
        linking.inlines,
        vec![
            Inline::Text("Rich text with italic, underline, strike, code and a link.".into()),
            Inline::Break,
            Inline::Text("See also ".into()),
            Inline::PageRef {
                page_id: linked,
                block_id: None
            },
            Inline::Text(" and ".into()),
            Inline::PageRef {
                page_id: linked,
                block_id: Some(block("8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7"))
            },
            Inline::Text(".".into()),
        ]
    );
    let code = blocks.iter().find(|b| b.kind == "codeBlock").unwrap();
    assert_eq!(
        code.inlines,
        vec![Inline::Text("fn main() {\n    println!(\"hi\");\n}".into())]
    );

    // The card reads as the reference it stands for (the text index resolves its title).
    let card = blocks
        .iter()
        .find(|b| b.id == block(page_golden::PAGE_LINK_BLOCK))
        .unwrap();
    assert_eq!(card.kind, "pageLink");
    assert_eq!(
        card.inlines,
        vec![Inline::PageRef {
            page_id: linked,
            block_id: Some(block("8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7"))
        }]
    );

    // Both references sit in the paragraph, not in the references themselves; the card
    // links from itself.
    assert_eq!(
        page.links(),
        vec![
            PageLink {
                from_block: block(LINKING_PARAGRAPH),
                to_page: linked,
                to_block: None
            },
            PageLink {
                from_block: block(LINKING_PARAGRAPH),
                to_page: linked,
                to_block: Some(block("8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7"))
            },
            PageLink {
                from_block: block(page_golden::PAGE_LINK_BLOCK),
                to_page: linked,
                to_block: Some(block("8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7"))
            },
        ]
    );
}

#[test]
fn unknown_content_passes_through() {
    let page = load(UNKNOWN_SNAPSHOT);
    assert_eq!(page.to_portable(), json(UNKNOWN_JSON));
    assert_eq!(
        page.unknown_node_names(schema::NODE_NAMES),
        vec!["fancyWidget".to_owned()]
    );
    assert_sound(&page);
    assert_eq!(
        page.links(),
        vec![PageLink {
            from_block: block("3d4e5f60-7182-4d9e-bfa0-b1c2d3e4f506"),
            to_page: page_id(page_golden::LINKED_PAGE),
            to_block: None
        }]
    );
    let widget = page
        .blocks()
        .into_iter()
        .find(|b| b.kind == "fancyWidget")
        .unwrap();
    assert_eq!(
        widget.inlines,
        vec![Inline::Text("Loose widget text".into())]
    );
    assert_eq!(
        unix(&page.to_markdown(&title_of)),
        "## Written by a newer client\n\nPlain, highlighted and **both**.\n\n\
         Inside the widget, see [[Linked page]]\n\nLoose widget text\n\nAfter the widget.\n"
    );
    // Editing next to unknown content keeps it.
    page.insert_block(
        Some(block("2c3d4e5f-6071-4c8d-ae9f-a0b1c2d3e4f5")),
        BlockKind::Paragraph,
        "new",
        BlockId::from_random_bytes([0x42; 16]),
    )
    .unwrap();
    let content = page.to_portable()["content"].clone();
    assert_eq!(content[2], json(UNKNOWN_JSON)["content"][2]);
    assert_eq!(content[3]["content"][0]["text"], "new");
}

#[test]
fn page_v1_snapshot_stays_editable() {
    let page = load(PAGE_SNAPSHOT);
    let since = page.doc().version();
    let new = BlockId::from_random_bytes([0x51; 16]);
    page.insert_block(
        Some(block("f87ca27d-6b09-47fa-894c-0a2b3c4d5e70")),
        BlockKind::Paragraph,
        "Appended",
        new,
    )
    .unwrap();
    page.set_block_text(
        block("0f8d3b2e-7c1a-4e0b-9a5d-1b2c3d4e5f60"),
        "Welcome back",
    )
    .unwrap();
    page.insert_page_ref(
        new,
        BlockId::from_random_bytes([0x52; 16]),
        page_id(page_golden::LINKED_PAGE),
        None,
    )
    .unwrap();
    page.delete_block(block("81053b06-f492-4083-92d5-93b4c5d6e7f9"))
        .unwrap();
    let shown_again = BlockId::from_random_bytes([0x53; 16]);
    page.insert_block(
        Some(new),
        BlockKind::Image {
            attachment_id: attachment(page_golden::IMAGE_ATTACHMENT),
            width: None,
        },
        "",
        shown_again,
    )
    .unwrap();
    page.delete_block(block(page_golden::FILE_BLOCK)).unwrap();
    assert_sound(&page);
    // The sample's two chips and its card, and the chip added above.
    assert_eq!(page.links().len(), 4);
    assert_eq!(
        page.attachment_refs(),
        vec![
            (
                block(page_golden::IMAGE_BLOCK),
                attachment(page_golden::IMAGE_ATTACHMENT)
            ),
            (
                block(page_golden::GALLERY_BLOCK),
                attachment(page_golden::IMAGE_ATTACHMENT)
            ),
            (
                block(page_golden::GALLERY_BLOCK),
                attachment(page_golden::GALLERY_SECOND)
            ),
            (
                block(page_golden::GALLERY_BLOCK),
                attachment(page_golden::GALLERY_THIRD)
            ),
            (shown_again, attachment(page_golden::IMAGE_ATTACHMENT)),
        ]
    );

    // Updates written now apply on top of the old snapshot.
    let delta = page
        .doc()
        .export_updates_since(Some(&since))
        .unwrap()
        .unwrap();
    let replay =
        PageDoc::from_doc(Doc::load(DocKind::Page, 2, Some(PAGE_SNAPSHOT), &[delta]).unwrap())
            .unwrap();
    assert_eq!(replay.to_portable(), page.to_portable());
}

// ----- cross-runtime --------------------------------------------------------------------------

#[test]
fn the_typescript_snapshot_decodes_to_the_same_json() {
    let Some(snapshot) = js_snapshot() else {
        return;
    };
    let page = load(&snapshot);
    assert_eq!(page.to_portable(), json(PAGE_JSON));
    assert_sound(&page);
    assert_eq!(unix(&markdown(&page)), unix(PAGE_MARKDOWN));
    assert_eq!(page.links(), load(PAGE_SNAPSHOT).links());
    assert_eq!(page.blocks(), load(PAGE_SNAPSHOT).blocks());
    assert_eq!(
        page.attachment_refs(),
        load(PAGE_SNAPSHOT).attachment_refs()
    );
}

fn children(map: &LoroMap) -> LoroList {
    match map.get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        other => panic!("no children list: {other:?}"),
    }
}

fn entry(list: &LoroList, index: usize) -> ValueOrContainer {
    list.get(index).unwrap()
}

/// The paragraph of the sample that holds the coloured runs (its last paragraph).
const COLOUR_PARAGRAPH: &str = "f87ca27d-6b09-47fa-894c-0a2b3c4d5e70";

/// The position of the top-level block `id` of the sample.
fn top_level_index(id: &str) -> usize {
    json(PAGE_JSON)["content"]
        .as_array()
        .unwrap()
        .iter()
        .position(|block| block["attrs"]["id"] == id)
        .unwrap()
}

/// Marks of a character typed at `pos` of the first text of the top-level block
/// `top`. The expansion is recorded in each mark operation, so this reads the flags
/// the writer of `snapshot` used.
fn marks_typed_at(snapshot: &[u8], top: usize, pos: usize) -> Vec<String> {
    let page = load(snapshot);
    let root = page.doc().inner().get_map("doc");
    let ValueOrContainer::Container(Container::Map(node)) = entry(&children(&root), top) else {
        panic!("block {top} is not a node");
    };
    let ValueOrContainer::Container(Container::Text(text)) = entry(&children(&node), 0) else {
        panic!("block {top} does not start with text");
    };
    let text: LoroText = text;
    text.insert(pos, "X").unwrap();
    let mut at = 0;
    for delta in text.to_delta() {
        if let TextDelta::Insert { insert, attributes } = delta {
            let len = insert.chars().count();
            if pos < at + len {
                let mut names: Vec<String> = attributes
                    .into_iter()
                    .flatten()
                    .filter(|(_, value)| !matches!(value, LoroValue::Null))
                    .map(|(name, _)| name)
                    .collect();
                names.sort();
                return names;
            }
            at += len;
        }
    }
    panic!("position {pos} is outside the text");
}

#[test]
fn marks_written_by_either_runtime_expand_per_the_shape_table() {
    let mut snapshots = vec![("rust", PAGE_SNAPSHOT.to_vec())];
    if let Some(js) = js_snapshot() {
        snapshots.push(("typescript", js));
    }
    for (writer, snapshot) in &snapshots {
        // "Welcome to atomik-kb": typing after the bold run continues it.
        assert_eq!(marks_typed_at(snapshot, 0, 20), ["bold"], "{writer}");
        // "Rich text with italic, underline, strike, code and a link."
        assert_eq!(marks_typed_at(snapshot, 1, 21), ["italic"], "{writer}");
        assert_eq!(marks_typed_at(snapshot, 1, 32), ["underline"], "{writer}");
        assert_eq!(marks_typed_at(snapshot, 1, 40), ["strike"], "{writer}");
        assert!(marks_typed_at(snapshot, 1, 46).is_empty(), "{writer}: code");
        assert!(marks_typed_at(snapshot, 1, 57).is_empty(), "{writer}: link");
        // "Unicode: café, 日本語, 👩‍💻 and 𝔘𝔫𝔦𝔠𝔬𝔡𝔢.": the colour marks expand like bold.
        let colours = top_level_index(COLOUR_PARAGRAPH);
        assert!(
            marks_typed_at(snapshot, colours, 9).is_empty(),
            "{writer}: before a run"
        );
        assert_eq!(
            marks_typed_at(snapshot, colours, 13),
            ["textColor"],
            "{writer}"
        );
        assert_eq!(
            marks_typed_at(snapshot, colours, 18),
            ["highlight"],
            "{writer}"
        );
        assert_eq!(
            marks_typed_at(snapshot, colours, 35),
            ["highlight", "textColor"],
            "{writer}"
        );
    }
}

/// The photo gallery (ADR-0016 amendment of 2026-10-09, ADR-0030): the editor schema lists
/// `items` and `columns`, the sample's gallery keeps three pictures (the first is the image's
/// attachment, two have a caption) in a list of plain maps, both snapshots read it the same, its
/// captions are its text, and every attachment it lists is a reference of its block.
#[test]
fn the_sample_gallery_lists_three_pictures() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    assert_eq!(
        fixture["nodes"]["gallery"]["attrs"],
        serde_json::json!(["id", "items", "columns"])
    );
    assert_eq!(kb_model::shape::GALLERY, "gallery");
    assert_eq!(kb_model::shape::ITEMS, "items");
    assert_eq!(kb_model::shape::COLUMNS, "columns");
    assert!(schema::ID_BEARING.contains(&"gallery"));

    let sample = json(PAGE_JSON);
    let gallery = sample["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["attrs"]["id"] == page_golden::GALLERY_BLOCK)
        .unwrap();
    assert_eq!(gallery["type"], "gallery");
    assert_eq!(gallery["attrs"]["columns"], 3);
    assert_eq!(gallery["attrs"]["items"].as_array().unwrap().len(), 3);

    let mut snapshots = vec![load(PAGE_SNAPSHOT)];
    if let Some(js) = js_snapshot() {
        snapshots.push(load(&js));
    }
    for page in &snapshots {
        assert_sound(page);
        assert_eq!(page.to_portable(), sample);
        let shown = page
            .blocks()
            .into_iter()
            .find(|b| b.id == block(page_golden::GALLERY_BLOCK))
            .unwrap();
        assert_eq!(
            shown.inlines,
            vec![Inline::Text(
                "Opening night, \"north\" wing Backstage".into()
            )]
        );
    }
}

/// The whiteboard block (ADR-0016 amendment of 2026-10-10, ADR-0033): the editor schema lists `id`
/// and `height`, the sample keeps three elements (a rectangle, its bound text and a deleted
/// ellipse) in the root map `whiteboards`, both snapshots read them the same, only the live text
/// element is the block's text, and the tombstone is kept in the portable form.
#[test]
fn the_sample_whiteboard_keeps_its_elements_in_a_separate_root_map() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    assert_eq!(
        fixture["nodes"]["whiteboard"]["attrs"],
        serde_json::json!(["id", "height"])
    );
    assert_eq!(kb_model::shape::WHITEBOARD, "whiteboard");
    assert_eq!(kb_model::shape::HEIGHT, "height");
    assert_eq!(kb_model::shape::WHITEBOARDS, "whiteboards");
    assert!(schema::ID_BEARING.contains(&"whiteboard"));

    let sample = json(PAGE_JSON);
    let elements = &sample["whiteboards"][page_golden::WHITEBOARD_BLOCK];
    assert_eq!(elements.as_object().unwrap().len(), 3);
    assert_eq!(elements["gone1"]["isDeleted"], true);

    let mut snapshots = vec![load(PAGE_SNAPSHOT)];
    if let Some(js) = js_snapshot() {
        snapshots.push(load(&js));
    }
    for page in &snapshots {
        assert_sound(page);
        assert_eq!(page.to_portable(), sample);
        let id = block(page_golden::WHITEBOARD_BLOCK);
        let listed = page.whiteboard_elements(id);
        assert_eq!(
            listed.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["frame1", "label1", "gone1"],
            "z-order by fractional index"
        );
        assert!(listed[2].is_deleted());
        let shown = page.blocks().into_iter().find(|b| b.id == id).unwrap();
        assert_eq!(shown.inlines, vec![Inline::Text("Plan A ship it".into())]);
        assert!(page.orphan_whiteboard_keys().is_empty());
        assert!(page.attachment_refs().iter().all(|(b, _)| *b != id));
    }
}

/// The `view` attribute of `file` (ADR-0016 amendment of 2026-10-09, ADR-0031): the editor schema
/// lists it after `attachmentId`, the sample's file block is shown as a preview, both snapshots
/// read it the same, and it is layout only (no text, the same Markdown file link, still a
/// reference).
#[test]
fn the_sample_file_is_shown_as_a_preview() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    assert_eq!(
        fixture["nodes"]["file"]["attrs"],
        serde_json::json!(["id", "attachmentId", "view"])
    );
    assert_eq!(kb_model::shape::VIEW, "view");

    let sample = json(PAGE_JSON);
    let file = sample["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["attrs"]["id"] == page_golden::FILE_BLOCK)
        .unwrap();
    assert_eq!(file["attrs"]["view"], "preview");
    assert!(!PAGE_MARKDOWN.contains("preview"));

    let mut snapshots = vec![load(PAGE_SNAPSHOT)];
    if let Some(js) = js_snapshot() {
        snapshots.push(load(&js));
    }
    for page in &snapshots {
        assert_sound(page);
        assert_eq!(page.to_portable(), sample);
        let shown = page
            .blocks()
            .into_iter()
            .find(|b| b.id == block(page_golden::FILE_BLOCK))
            .unwrap();
        assert!(shown.inlines.is_empty());
        assert!(page.attachment_refs().contains(&(
            block(page_golden::FILE_BLOCK),
            attachment(page_golden::FILE_ATTACHMENT)
        )));
    }
}

/// The template button (ADR-0016 amendment of 2026-10-18, ADR-0056): the editor schema lists its content
/// (`block+`) and attributes, the sample's button is labelled and inserts above, both snapshots read it the same, and
/// what it holds is template material: its paragraph is a block with no text, the date mention inside it (a reminder,
/// `relative: today`) is no date mention of the page, the person mention (`relative: me`) is no person mention, and
/// `page-v1.md` has none of it.
#[test]
fn the_sample_template_button_holds_template_material() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    let node = &fixture["nodes"]["templateButton"];
    assert_eq!(node["content"], "block+");
    assert_eq!(node["group"], "block");
    assert_eq!(
        node["attrs"],
        serde_json::json!(["id", "label", "position"])
    );
    assert!(schema::NODE_NAMES.contains(&"templateButton"));
    assert!(schema::ID_BEARING.contains(&"templateButton"));
    assert_eq!(kb_model::shape::TEMPLATE_BUTTON, "templateButton");
    assert_eq!(kb_model::shape::POSITION, "position");
    assert_eq!(kb_model::shape::RELATIVE, "relative");
    assert!(!PAGE_MARKDOWN.contains("Add task"));
    assert!(!PAGE_MARKDOWN.contains("Task for"));

    let sample = json(PAGE_JSON);
    let button = sample["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["attrs"]["id"] == page_golden::TEMPLATE_BUTTON_BLOCK)
        .unwrap();
    assert_eq!(button["type"], "templateButton");
    assert_eq!(button["attrs"]["label"], "Add task");
    assert_eq!(button["attrs"]["position"], "above");
    let inside = button["content"][0]["content"].as_array().unwrap();
    assert_eq!(inside[1]["attrs"]["relative"], "me");
    assert_eq!(inside[3]["attrs"]["relative"], "today");

    let mut snapshots = vec![load(PAGE_SNAPSHOT)];
    if let Some(js) = js_snapshot() {
        snapshots.push(load(&js));
    }
    for page in &snapshots {
        assert_sound(page);
        assert_eq!(page.to_portable(), sample);
        let blocks = page.blocks();
        let shown = blocks
            .iter()
            .find(|b| b.id == block(page_golden::TEMPLATE_BUTTON_BLOCK))
            .unwrap();
        assert_eq!(shown.kind, "templateButton");
        assert!(shown.inlines.is_empty());
        let paragraph = blocks
            .iter()
            .find(|b| b.id == block(page_golden::TEMPLATE_BUTTON_PARAGRAPH))
            .unwrap();
        assert!(
            paragraph.inlines.is_empty(),
            "template material has no text"
        );
        // The two mentions of the list item are the only ones: the reminder inside the button never fires.
        assert_eq!(page.date_mentions().len(), 2);
        assert!(page.person_mentions().is_empty());
    }
}

// ----- schema parity --------------------------------------------------------------------------

#[test]
fn the_schema_module_matches_the_editor_schema() {
    let fixture = json(BLOCK_SCHEMA);
    let marks: BTreeMap<String, String> = fixture["marks"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, spec)| (name.clone(), spec["expand"].as_str().unwrap().to_owned()))
        .collect();
    let ours: BTreeMap<String, String> = schema::MARKS
        .iter()
        .map(|(name, expand)| {
            let expand = match expand {
                MarkExpand::After => "after",
                MarkExpand::None => "none",
            };
            ((*name).to_owned(), expand.to_owned())
        })
        .collect();
    assert_eq!(marks, ours);
    assert_eq!(ours.len(), schema::MARKS.len(), "no mark listed twice");

    let nodes: BTreeSet<String> = fixture["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let ours: BTreeSet<String> = schema::NODE_NAMES.iter().map(|n| (*n).to_owned()).collect();
    assert_eq!(nodes, ours);
    assert_eq!(ours.len(), schema::NODE_NAMES.len());

    let id_bearing: BTreeSet<String> = fixture["idBearing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    let ours: BTreeSet<String> = schema::ID_BEARING.iter().map(|n| (*n).to_owned()).collect();
    assert_eq!(id_bearing, ours);
    assert_eq!(ours.len(), schema::ID_BEARING.len());
    // Every id-bearing node declares the attribute in the editor schema.
    for name in schema::ID_BEARING {
        let attrs = fixture["nodes"][*name]["attrs"].as_array().unwrap();
        assert!(attrs.iter().any(|a| a == "id"), "{name}");
    }
}

/// The attributes of the list view (ADR-0016 amendments of 2026-10-07 and 2026-10-08): the editor
/// schema lists the table layout's `filters`, `columnWidths`, `groupBy`, `hiddenGroups`, (2026-10-14) `calculations` and (ADR-0054) `views` after the four
/// of the list, and the core declares no more than the node itself (it reads none of the attributes).
#[test]
fn the_editor_schema_lists_the_attributes_of_the_list_view() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    assert_eq!(
        fixture["nodes"]["pageList"]["attrs"],
        serde_json::json!([
            "id",
            "source",
            "layout",
            "properties",
            "sort",
            "filters",
            "columnWidths",
            "groupBy",
            "hiddenGroups",
            "calculations",
            "views"
        ])
    );
    assert!(schema::DERIVED_NODES.contains(&"pageList"));
}

/// The person mention (M13 slice 2, ADR-0049): the editor schema lists it as an inline atom with the
/// attributes `accountId` and `label` and **no block id** (it is not in the id-bearing list), and the
/// core's schema module names it.
#[test]
fn the_editor_schema_lists_the_person_mention_as_an_inline_atom_without_an_id() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    let node = &fixture["nodes"]["personMention"];
    assert_eq!(node["group"], "inline");
    assert_eq!(node["inline"], true);
    assert_eq!(node["atom"], true);
    assert_eq!(node["content"], "");
    // `relative` is `me` on a mention filled in with whoever makes a page from a template (ADR-0056).
    assert_eq!(
        node["attrs"],
        serde_json::json!(["accountId", "label", "relative"])
    );
    assert!(schema::NODE_NAMES.contains(&"personMention"));
    assert!(!schema::ID_BEARING.contains(&"personMention"));
    assert_eq!(kb_model::shape::PERSON_MENTION, "personMention");
    assert_eq!(kb_model::shape::ACCOUNT_ID, "accountId");
    assert_eq!(kb_model::shape::LABEL, "label");
}

/// The reminder attribute of the date mention (ADR-0016 amendment of 2026-10-07, ADR-0024): the
/// editor schema lists it, the sample holds two (one on a whole day, one on a day with a time),
/// and the Rust snapshot and the one the TypeScript side wrote read them the same, with the same
/// fire times on the local wall clock. `page-v1.md` has no trace of them.
#[test]
fn the_sample_holds_two_reminders_and_both_snapshots_read_them_alike() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    assert_eq!(
        fixture["nodes"]["dateMention"]["attrs"],
        // `reminderFor` is the account whose devices fire it in a shared space (M13 slice 2, ADR-0047).
        // `relative` is `today` or `now` on a mention filled in when a page is made from a template (ADR-0056).
        serde_json::json!(["id", "date", "time", "reminder", "reminderFor", "relative"])
    );
    assert_eq!(kb_model::shape::REMINDER, "reminder");
    assert_eq!(kb_model::shape::REMINDER_FOR, "reminderFor");
    assert!(!PAGE_MARKDOWN.contains("reminder"));
    assert!(!PAGE_MARKDOWN.contains("1440"));

    let mut snapshots = vec![load(PAGE_SNAPSHOT)];
    if let Some(js) = js_snapshot() {
        snapshots.push(load(&js));
    }
    for page in &snapshots {
        assert_sound(page);
        let mentions = page.date_mentions();
        assert_eq!(mentions.len(), 2);
        // A whole day, a day before: 2026-10-07 at 09:00 minus 1440 minutes.
        assert_eq!(mentions[0].date, "2026-10-07");
        assert_eq!(mentions[0].time, None);
        assert_eq!(mentions[0].reminder_minutes, Some(1440));
        assert_eq!(mentions[0].fire_at_local, Some(1_791_277_200));
        // A day with a time, 15 minutes before: 2026-10-08 at 14:30 minus 15 minutes.
        assert_eq!(mentions[1].date, "2026-10-08");
        assert_eq!(mentions[1].time.as_deref(), Some("14:30"));
        assert_eq!(mentions[1].reminder_minutes, Some(15));
        assert_eq!(mentions[1].fire_at_local, Some(1_791_468_900));
        // The paragraph of the second list item holds both.
        assert_eq!(mentions[0].block_id, mentions[1].block_id);
        assert_eq!(
            mentions[0].block_id,
            block("c5497f4a-38d6-44c7-9519-d7f8091a2b3c")
        );
        assert!(
            mentions[0].block_id != mentions[0].node_id
                && mentions[0].node_id != mentions[1].node_id
        );
        // The reminder is not text: the block reads as its ISO text only.
        assert_eq!(page.to_portable(), json(PAGE_JSON));
    }
}

/// The three optional attributes of `image` (ADR-0016 amendment of 2026-10-09, ADR-0028): the
/// editor schema lists them after `width`, the sample's image is 60 % wide, aligned right and
/// captioned (a caption with a `"` and a `\`), both snapshots read it the same, and the caption is
/// the only one of the three that is text (it is in `blocks()` and in `page-v1.md` as the link
/// title; `widthPercent` and `align` are in neither).
#[test]
fn the_sample_image_has_a_width_a_side_and_a_caption() {
    let fixture: serde_json::Value = serde_json::from_str(BLOCK_SCHEMA).unwrap();
    assert_eq!(
        fixture["nodes"]["image"]["attrs"],
        serde_json::json!([
            "id",
            "attachmentId",
            "alt",
            "width",
            "widthPercent",
            "align",
            "caption"
        ])
    );
    assert_eq!(kb_model::shape::WIDTH_PERCENT, "widthPercent");
    assert_eq!(kb_model::shape::ALIGN, "align");
    assert_eq!(kb_model::shape::CAPTION, "caption");

    let caption = "Figure 1: the \"sync\" flow, C:\\sync";
    let sample = json(PAGE_JSON);
    let image = sample["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["attrs"]["id"] == page_golden::IMAGE_BLOCK)
        .unwrap();
    assert_eq!(
        image["attrs"],
        serde_json::json!({
            "id": page_golden::IMAGE_BLOCK,
            "attachmentId": page_golden::IMAGE_ATTACHMENT,
            "alt": "Diagram of the sync flow",
            "width": 640,
            "widthPercent": 60,
            "align": "right",
            "caption": caption
        })
    );

    // `page-v1.md` has the caption as the link title, escaped, and nothing of the layout.
    assert!(unix(PAGE_MARKDOWN).contains(&format!(
        "\n\n![Diagram of the sync flow](attachment://{} \"Figure 1: the \\\"sync\\\" flow, C:\\\\sync\")\n\n",
        page_golden::IMAGE_ATTACHMENT
    )));
    assert!(!PAGE_MARKDOWN.contains("widthPercent"));
    assert!(!PAGE_MARKDOWN.contains("right"));
    assert!(!PAGE_MARKDOWN.contains("640"));

    let mut snapshots = vec![load(PAGE_SNAPSHOT)];
    if let Some(js) = js_snapshot() {
        snapshots.push(load(&js));
    }
    for page in &snapshots {
        assert_sound(page);
        assert_eq!(page.to_portable(), sample);
        let blocks = page.blocks();
        let shown = blocks
            .iter()
            .find(|b| b.id == block(page_golden::IMAGE_BLOCK))
            .unwrap();
        assert_eq!(
            shown.inlines,
            vec![Inline::Text(format!("{caption} Diagram of the sync flow"))]
        );
    }
}
