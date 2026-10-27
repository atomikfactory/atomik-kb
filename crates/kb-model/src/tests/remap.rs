//! The content of a page copy (ADR-0040): new block ids, links to the copied pages remapped,
//! whiteboards that follow their blocks.

use std::collections::HashSet;

use kb_doc::{Doc, DocKind};
use kb_types::{BlockId, PageId};
use serde_json::{Value, json};

use crate::{CopyIds, PageDoc, portable_block_ids, remap_portable};

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

/// The copy's block id of `bid(n)`.
fn copy_bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n + 100; 16])
}

fn page_id(n: u8) -> PageId {
    PageId::from_random_bytes([0x70, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn copy_page(n: u8) -> PageId {
    PageId::from_random_bytes([0x71, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn s(id: impl ToString) -> String {
    id.to_string()
}

fn para(id: BlockId, content: Value) -> Value {
    json!({ "type": "paragraph", "attrs": { "id": s(id) }, "content": content })
}

fn doc_of(content: Value) -> Value {
    json!({ "type": "doc", "content": content })
}

fn page_ref(id: u8, page: PageId, block: Option<BlockId>) -> Value {
    let mut attrs = json!({ "id": s(bid(id)), "pageId": s(page) });
    if let Some(block) = block {
        attrs["blockId"] = json!(s(block));
    }
    json!({ "type": "pageRef", "attrs": attrs })
}

/// The ids of blocks 1 to 30 and of pages 1 and 2 are copied.
fn ids() -> CopyIds {
    let mut ids = CopyIds::default();
    for n in 1..=30 {
        ids.blocks.insert(bid(n), copy_bid(n));
    }
    ids.pages.insert(page_id(1), copy_page(1));
    ids.pages.insert(page_id(2), copy_page(2));
    ids
}

fn from_json(json: &Value) -> PageDoc {
    PageDoc::from_portable(Doc::new(DocKind::Page, 9), json).unwrap()
}

#[test]
fn every_node_with_an_id_gets_its_copy_and_nothing_else_changes() {
    let json = doc_of(json!([
        para(
            bid(1),
            json!([
                { "type": "text", "text": "Hello ", "marks": [{ "type": "bold" }] },
                { "type": "dateMention", "attrs": { "id": s(bid(2)), "date": "2026-10-13" } },
                { "type": "mathInline", "attrs": { "id": s(bid(3)), "latex": "E=mc^2" } },
            ])
        ),
        { "type": "bulletList", "attrs": { "id": s(bid(4)) }, "content": [
            { "type": "listItem", "attrs": { "id": s(bid(5)) }, "content": [
                para(bid(6), json!([{ "type": "text", "text": "item" }]))
            ]}
        ]},
        { "type": "heading", "attrs": { "id": s(bid(7)), "level": 2 }, "content": [
            { "type": "text", "text": "Title" }
        ]},
        // A node without an id, one with an id that is not a block id and one the copy does not know.
        { "type": "horizontalRule" },
        { "type": "paragraph", "attrs": { "id": "not-an-id" } },
        { "type": "paragraph", "attrs": { "id": s(bid(99)) } },
    ]));
    let copy = remap_portable(&json, &ids());

    assert_eq!(
        copy["content"][0]["attrs"]["id"],
        json!(s(copy_bid(1))),
        "a paragraph"
    );
    assert_eq!(
        copy["content"][0]["content"][1]["attrs"],
        json!({ "id": s(copy_bid(2)), "date": "2026-10-13" }),
        "an inline node keeps its other attributes"
    );
    assert_eq!(
        copy["content"][0]["content"][0], json["content"][0]["content"][0],
        "text and marks are untouched"
    );
    assert_eq!(
        copy["content"][1]["content"][0]["content"][0]["attrs"]["id"],
        json!(s(copy_bid(6))),
        "nested blocks"
    );
    assert_eq!(copy["content"][2]["attrs"]["level"], json!(2));
    assert_eq!(copy["content"][3], json!({ "type": "horizontalRule" }));
    assert_eq!(copy["content"][4]["attrs"]["id"], json!("not-an-id"));
    assert_eq!(
        copy["content"][5]["attrs"]["id"],
        json!(s(bid(99))),
        "an id the copy has no replacement for is left alone"
    );
    // The original is not modified, and the copy shares no mapped id with it.
    assert_eq!(json["content"][0]["attrs"]["id"], json!(s(bid(1))));
    let old: HashSet<BlockId> = portable_block_ids(&json).into_iter().collect();
    let new: HashSet<BlockId> = portable_block_ids(&copy).into_iter().collect();
    assert!(old.contains(&bid(1)) && !new.contains(&bid(1)));
    assert!(new.contains(&copy_bid(1)) && new.contains(&bid(99)));
}

#[test]
fn links_to_copied_pages_follow_them_and_other_links_stay() {
    let outside = page_id(9);
    let json = doc_of(json!([
        para(
            bid(1),
            json!([
                page_ref(10, page_id(1), None),
                page_ref(11, page_id(2), Some(bid(5))),
                // A link into the subtree to a block that was not copied: the block id stays.
                page_ref(12, page_id(2), Some(bid(77))),
                // A link to a page outside the subtree, with a block that is among the copied ones
                // (a block id is only meaningful with its page): nothing changes but the ref's own id.
                page_ref(13, outside, Some(bid(5))),
            ])
        ),
        { "type": "pageLink", "attrs": { "id": s(bid(14)), "pageId": s(page_id(1)), "blockId": s(bid(6)) } },
        { "type": "pageLink", "attrs": { "id": s(bid(15)), "pageId": s(outside) } },
    ]));
    let copy = remap_portable(&json, &ids());
    let refs = &copy["content"][0]["content"];
    assert_eq!(
        refs[0]["attrs"],
        json!({ "id": s(copy_bid(10)), "pageId": s(copy_page(1)) })
    );
    assert_eq!(
        refs[1]["attrs"],
        json!({ "id": s(copy_bid(11)), "pageId": s(copy_page(2)), "blockId": s(copy_bid(5)) }),
        "the page and the block it names"
    );
    assert_eq!(
        refs[2]["attrs"],
        json!({ "id": s(copy_bid(12)), "pageId": s(copy_page(2)), "blockId": s(bid(77)) }),
        "a block that was not copied keeps its id"
    );
    assert_eq!(
        refs[3]["attrs"],
        json!({ "id": s(copy_bid(13)), "pageId": s(outside), "blockId": s(bid(5)) }),
        "a link to another page is kept as it was"
    );
    assert_eq!(
        copy["content"][1]["attrs"],
        json!({ "id": s(copy_bid(14)), "pageId": s(copy_page(1)), "blockId": s(copy_bid(6)) }),
        "a page card"
    );
    assert_eq!(
        copy["content"][2]["attrs"],
        json!({ "id": s(copy_bid(15)), "pageId": s(outside) })
    );
}

#[test]
fn a_whiteboard_follows_its_block() {
    let element = |id: &str| json!({ "id": id, "type": "rectangle", "index": "a0", "version": 1, "isDeleted": false });
    let mut json = doc_of(json!([
        para(bid(1), json!([{ "type": "text", "text": "x" }])),
        { "type": "whiteboard", "attrs": { "id": s(bid(2)), "height": 400 } },
        { "type": "whiteboard", "attrs": { "id": s(bid(3)) } },
    ]));
    json["whiteboards"] = json!({
        s(bid(2)): { "r1": element("r1"), "r2": element("r2") },
        s(bid(3)): { "r1": element("r1") },
    });
    let copy = remap_portable(&json, &ids());
    let boards = copy["whiteboards"].as_object().unwrap();
    assert_eq!(boards.len(), 2);
    assert_eq!(
        boards[&s(copy_bid(2))],
        json!({ "r1": element("r1"), "r2": element("r2") }),
        "the elements and their own ids are unchanged"
    );
    assert!(boards.contains_key(&s(copy_bid(3))));
    assert!(!boards.contains_key(&s(bid(2))));

    // It builds into a page whose drawings are under the new block ids, and the original's
    // drawings are not shared with it.
    let original = from_json(&json);
    let page = from_json(&copy);
    assert_eq!(page.whiteboard_elements(copy_bid(2)).len(), 2);
    assert_eq!(page.whiteboard_elements(copy_bid(3)).len(), 1);
    assert!(page.whiteboard_elements(bid(2)).is_empty());
    assert_eq!(original.whiteboard_elements(bid(2)).len(), 2);
    assert_eq!(page.to_portable(), copy, "the portable form is stable");
    assert!(page.check_invariants().is_empty());
}

#[test]
fn portable_block_ids_lists_each_id_once_in_document_order() {
    let json = doc_of(json!([
        para(bid(3), json!([page_ref(4, page_id(1), None)])),
        { "type": "horizontalRule" },
        para(bid(3), json!([])),
        para(bid(1), json!([])),
    ]));
    assert_eq!(portable_block_ids(&json), vec![bid(3), bid(4), bid(1)]);
    assert!(portable_block_ids(&json!({ "type": "doc" })).is_empty());
}

#[test]
fn a_duplicated_id_stays_one_id_in_the_copy() {
    // Concurrent edits can leave two nodes with one id; the copy keeps them equal (it is the same
    // shape), under a new id.
    let json = doc_of(json!([para(bid(3), json!([])), para(bid(3), json!([]))]));
    let copy = remap_portable(&json, &ids());
    assert_eq!(copy["content"][0]["attrs"]["id"], json!(s(copy_bid(3))));
    assert_eq!(copy["content"][1]["attrs"]["id"], json!(s(copy_bid(3))));
}

fn page_list(id: u8, source: Value) -> Value {
    json!({
        "type": "pageList",
        "attrs": {
            "id": s(bid(id)),
            "source": source,
            "views": [{
                "id": "default", "layout": "list",
                "sorts": [{ "by": "title", "dir": "asc" }, { "by": "created", "dir": "desc" }],
                "filter": { "op": "and", "items": [
                    { "propertyId": "p", "op": "within", "value": { "relative": "thisWeek" } }
                ] }
            }]
        }
    })
}

#[test]
fn a_page_source_inside_the_copied_subtree_points_to_the_copy() {
    let json = doc_of(json!([
        page_list(1, json!({ "kind": "page", "pageId": s(page_id(1)) })),
        page_list(2, json!({ "kind": "page", "pageId": s(page_id(9)) })),
        page_list(3, json!({ "kind": "children" })),
        page_list(4, json!({ "kind": "space" })),
        page_list(5, json!({ "kind": "tag", "optionId": s(page_id(1)) })),
        page_list(6, json!({ "kind": "page", "pageId": "not an id" })),
        page_list(7, json!({ "kind": "page" })),
        // Not a page list: a `source` of that shape on another node is not looked at.
        { "type": "callout", "attrs": { "id": s(bid(8)), "source": { "kind": "page", "pageId": s(page_id(1)) } } },
    ]));
    let copy = remap_portable(&json, &ids());
    let source = |n: usize| &copy["content"][n]["attrs"]["source"];

    assert_eq!(
        source(0),
        &json!({ "kind": "page", "pageId": s(copy_page(1)) }),
        "inside the subtree: the copy"
    );
    assert_eq!(
        source(1),
        &json!({ "kind": "page", "pageId": s(page_id(9)) }),
        "outside the subtree: kept"
    );
    assert_eq!(source(2), &json!({ "kind": "children" }));
    assert_eq!(source(3), &json!({ "kind": "space" }));
    assert_eq!(
        source(4),
        &json!({ "kind": "tag", "optionId": s(page_id(1)) }),
        "only a page source is followed"
    );
    assert_eq!(source(5), &json!({ "kind": "page", "pageId": "not an id" }));
    assert_eq!(source(6), &json!({ "kind": "page" }));
    assert_eq!(
        copy["content"][7]["attrs"]["source"]["pageId"],
        json!(s(page_id(1)))
    );
    // The views, their sorts and their relative values are untouched; the block id still follows.
    assert_eq!(
        copy["content"][0]["attrs"]["views"],
        json["content"][0]["attrs"]["views"]
    );
    assert_eq!(copy["content"][0]["attrs"]["id"], json!(s(copy_bid(1))));
    // The page document rebuilt from it keeps the source.
    assert_eq!(
        from_json(&copy).to_portable()["content"][0]["attrs"]["source"],
        json!({ "kind": "page", "pageId": s(copy_page(1)) })
    );
}
