//! `PageDoc::append_portable`: appending portable blocks to an existing page (the MCP server's
//! `append_to_page` and the page document of `create_page`).

use kb_doc::{Doc, DocKind};
use kb_types::BlockId;
use serde_json::{Value, json};

use crate::{ModelError, PageDoc};

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn page_with(first: &str) -> PageDoc {
    let doc = json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "attrs": { "id": bid(1).to_string() },
            "content": [{ "type": "text", "text": first }]
        }]
    });
    PageDoc::from_portable(Doc::new(DocKind::Page, 7), &doc).unwrap()
}

fn para(n: u8, text: &str) -> Value {
    json!({
        "type": "paragraph",
        "attrs": { "id": bid(n).to_string() },
        "content": [{ "type": "text", "text": text }]
    })
}

fn doc_of(content: Vec<Value>) -> Value {
    json!({ "type": "doc", "content": content })
}

fn assert_sound(page: &PageDoc) {
    let problems = page.check_invariants();
    assert!(problems.is_empty(), "invariant violations: {problems:#?}");
}

#[test]
fn blocks_are_appended_after_the_existing_ones_and_nothing_else_changes() {
    let page = page_with("first");
    let before = page.to_portable();
    let appended = page
        .append_portable(&doc_of(vec![para(2, "second"), para(3, "third")]))
        .unwrap();
    assert_eq!(appended, 2);
    assert_eq!(page.block_ids(), [bid(1), bid(2), bid(3)]);
    let after = page.to_portable();
    assert_eq!(
        after["content"][0], before["content"][0],
        "existing content is untouched"
    );
    assert_eq!(after["content"][1], para(2, "second"));
    assert_eq!(after["content"][2], para(3, "third"));
    assert_eq!(page.to_markdown(&|_| None), "first\n\nsecond\n\nthird\n");
    assert_sound(&page);

    // A second append goes after the first one.
    page.append_portable(&doc_of(vec![para(4, "fourth")]))
        .unwrap();
    assert_eq!(page.block_ids(), [bid(1), bid(2), bid(3), bid(4)]);
}

#[test]
fn nested_blocks_keep_their_structure_and_their_ids() {
    let page = page_with("intro");
    let list = json!({
        "type": "bulletList",
        "attrs": { "id": bid(10).to_string() },
        "content": [
            {
                "type": "listItem",
                "attrs": { "id": bid(11).to_string() },
                "content": [para(12, "one")]
            },
            {
                "type": "listItem",
                "attrs": { "id": bid(13).to_string() },
                "content": [para(14, "two")]
            }
        ]
    });
    let heading = json!({
        "type": "heading",
        "attrs": { "id": bid(20).to_string(), "level": 2 },
        "content": [{ "type": "text", "text": "Title", "marks": [{ "type": "bold" }] }]
    });
    assert_eq!(
        page.append_portable(&doc_of(vec![heading.clone(), list.clone()]))
            .unwrap(),
        2
    );
    let ids = page.block_ids();
    for n in [1, 10, 11, 12, 13, 14, 20] {
        assert!(ids.contains(&bid(n)), "block {n} is there");
    }
    let portable = page.to_portable();
    assert_eq!(portable["content"][1], heading);
    assert_eq!(portable["content"][2], list);
    assert_sound(&page);
}

#[test]
fn an_id_the_page_already_has_or_that_repeats_is_refused_and_nothing_is_written() {
    let page = page_with("first");
    let before = page.to_portable();
    // The page's own id, at the top level and nested.
    let nested = json!({
        "type": "blockquote",
        "attrs": { "id": bid(5).to_string() },
        "content": [para(1, "clash")]
    });
    for content in [
        vec![para(1, "again")],
        vec![nested],
        // A new id twice in one append.
        vec![para(6, "a"), para(6, "b")],
        // A good block before the bad one is not written either.
        vec![para(7, "good"), para(1, "bad")],
    ] {
        let err = page.append_portable(&doc_of(content)).unwrap_err();
        assert!(matches!(err, ModelError::DuplicateBlock), "{err:?}");
        assert_eq!(page.to_portable(), before);
    }
}

#[test]
fn invalid_json_and_whiteboards_are_refused_and_nothing_is_written() {
    let page = page_with("first");
    let before = page.to_portable();
    for bad in [
        json!({ "type": "paragraph" }),
        json!({ "type": "doc", "content": "x" }),
        json!({ "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text" }] }] }),
        json!([1]),
        json!("doc"),
        json!({ "type": "doc", "extra": 1 }),
        // Whiteboard elements are not appended.
        json!({ "type": "doc", "content": [para(8, "x")], "whiteboards": { "a": [] } }),
        json!({ "type": "doc", "content": [para(8, "x")], "whiteboards": 5 }),
    ] {
        let err = page.append_portable(&bad).unwrap_err();
        assert!(matches!(err, ModelError::Shape(_)), "{bad}: {err:?}");
        assert_eq!(page.to_portable(), before);
    }
    // An empty `whiteboards` object is no whiteboard.
    let ok = json!({ "type": "doc", "content": [para(8, "x")], "whiteboards": {} });
    assert_eq!(page.append_portable(&ok).unwrap(), 1);
}

#[test]
fn nothing_to_append_is_zero_blocks_and_changes_nothing() {
    let page = page_with("first");
    let before = page.to_portable();
    let version = page.doc().version();
    assert_eq!(page.append_portable(&doc_of(Vec::new())).unwrap(), 0);
    assert_eq!(page.append_portable(&json!({ "type": "doc" })).unwrap(), 0);
    assert_eq!(page.to_portable(), before);
    assert_eq!(page.doc().version(), version);
}

#[test]
fn a_document_without_a_root_gets_one() {
    let page = PageDoc::from_doc(Doc::new(DocKind::Page, 3)).unwrap();
    assert_eq!(
        page.append_portable(&doc_of(vec![para(2, "hello")]))
            .unwrap(),
        1
    );
    assert_eq!(page.block_ids(), [bid(2)]);
    assert_eq!(page.to_markdown(&|_| None), "hello\n");
    assert_sound(&page);
}

#[test]
fn the_empty_first_paragraph_of_a_new_page_can_be_dropped_after_an_append() {
    // What the core does for a new page: its empty paragraph, the content, then the paragraph goes.
    let page = PageDoc::new(Doc::new(DocKind::Page, 4), bid(1)).unwrap();
    assert!(page.is_empty());
    page.append_portable(&doc_of(vec![para(2, "content")]))
        .unwrap();
    page.delete_block(bid(1)).unwrap();
    assert_eq!(page.block_ids(), [bid(2)]);
    assert_eq!(page.to_markdown(&|_| None), "content\n");
    assert_sound(&page);
}

#[test]
fn an_append_travels_as_an_update_to_another_replica() {
    let page = page_with("first");
    let replica = PageDoc::from_doc(Doc::new(DocKind::Page, 9)).unwrap();
    let snapshot = page.doc().export_snapshot().unwrap();
    replica.doc().import(&snapshot).unwrap();
    let since = page.doc().version();
    page.append_portable(&doc_of(vec![para(2, "second")]))
        .unwrap();
    let update = page
        .doc()
        .export_updates_since(Some(&since))
        .unwrap()
        .unwrap();
    replica.doc().import(&update).unwrap();
    assert_eq!(replica.to_portable(), page.to_portable());
    assert_eq!(replica.block_ids(), [bid(1), bid(2)]);
    assert_sound(&replica);
}
