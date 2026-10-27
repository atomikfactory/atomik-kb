//! Unit tests of the page document (`PageDoc`, Milestone 3).

use std::time::{Duration, Instant};

use kb_doc::{Doc, DocKind};
use kb_types::{AttachmentId, BlockId, PageId};
use loro::{
    Container, ContainerTrait, LoroList, LoroMap, LoroText, LoroValue, ToJson, ValueOrContainer,
};
use serde_json::{Value, json};

use crate::{BlockKind, BlockText, Inline, ModelError, PageDoc, PageLink, schema};

// ----- helpers ------------------------------------------------------------------------------

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn page_id(n: u8) -> PageId {
    PageId::from_random_bytes([0x70, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn s(id: BlockId) -> String {
    id.to_string()
}

fn new_page(peer: u64) -> PageDoc {
    PageDoc::new(Doc::new(DocKind::Page, peer), bid(1)).unwrap()
}

fn from_json(json: &Value) -> PageDoc {
    PageDoc::from_portable(Doc::new(DocKind::Page, 3), json).unwrap()
}

fn doc_of(content: Value) -> Value {
    json!({ "type": "doc", "content": content })
}

fn para(id: BlockId, text: &str) -> Value {
    if text.is_empty() {
        json!({ "type": "paragraph", "attrs": { "id": s(id) } })
    } else {
        json!({ "type": "paragraph", "attrs": { "id": s(id) }, "content": [{ "type": "text", "text": text }] })
    }
}

fn no_titles(_: PageId) -> Option<String> {
    None
}

fn is_not_found(err: &ModelError) -> bool {
    matches!(err, ModelError::BlockNotFound)
}

fn is_duplicate(err: &ModelError) -> bool {
    matches!(err, ModelError::DuplicateBlock)
}

fn is_invalid(err: &ModelError) -> bool {
    matches!(err, ModelError::InvalidBlock(_))
}

fn assert_sound(page: &PageDoc) {
    let problems = page.check_invariants();
    assert!(problems.is_empty(), "invariant violations: {problems:#?}");
}

fn texts(page: &PageDoc) -> Vec<String> {
    page.blocks()
        .into_iter()
        .filter(|block| !block.inlines.is_empty())
        .map(|block| {
            block
                .inlines
                .iter()
                .map(|inline| match inline {
                    Inline::Text(text) => text.clone(),
                    Inline::PageRef { page_id, .. } => format!("[[{page_id}]]"),
                    Inline::Break => "\n".to_owned(),
                })
                .collect()
        })
        .collect()
}

fn top_ids(page: &PageDoc) -> Vec<String> {
    page.to_portable()["content"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| node["attrs"]["id"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

// Raw engine access, to build what the binding (or damage) can produce.

fn root(page: &PageDoc) -> LoroMap {
    page.doc().inner().get_map("doc")
}

fn list_at(map: &LoroMap, key: &str) -> LoroList {
    match map.get(key) {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        other => panic!("{key} is not a list: {other:?}"),
    }
}

fn map_at(map: &LoroMap, key: &str) -> LoroMap {
    match map.get(key) {
        Some(ValueOrContainer::Container(Container::Map(child))) => child,
        other => panic!("{key} is not a map: {other:?}"),
    }
}

fn child_map(list: &LoroList, pos: usize) -> LoroMap {
    match list.get(pos) {
        Some(ValueOrContainer::Container(Container::Map(map))) => map,
        other => panic!("entry {pos} is not a map: {other:?}"),
    }
}

fn child_text(list: &LoroList, pos: usize) -> LoroText {
    match list.get(pos) {
        Some(ValueOrContainer::Container(Container::Text(text))) => text,
        other => panic!("entry {pos} is not a text: {other:?}"),
    }
}

/// A node in the binding's shape, inserted at `pos` of `list`.
fn raw_node(list: &LoroList, pos: usize, name: &str, attrs: &[(&str, LoroValue)]) -> LoroMap {
    let map = list.insert_container(pos, LoroMap::new()).unwrap();
    map.insert("nodeName", name).unwrap();
    let attributes = map.insert_container("attributes", LoroMap::new()).unwrap();
    for (key, value) in attrs {
        attributes.insert(key, value.clone()).unwrap();
    }
    map.insert_container("children", LoroList::new()).unwrap();
    map
}

fn raw_text(list: &LoroList, pos: usize, text: &str) -> LoroText {
    let container = list.insert_container(pos, LoroText::new()).unwrap();
    container.insert(0, text).unwrap();
    container
}

fn id_value(id: BlockId) -> LoroValue {
    LoroValue::from(id.to_string())
}

fn empty_map() -> LoroValue {
    LoroValue::Map(Vec::<(String, LoroValue)>::new().into())
}

// ----- creation and wrapping ----------------------------------------------------------------

#[test]
fn new_writes_the_binding_shape_with_one_empty_paragraph() {
    let page = new_page(1);
    assert_eq!(
        page.doc().inner().get_deep_value().to_json_value(),
        json!({ "doc": {
            "nodeName": "doc",
            "attributes": {},
            "children": [{ "nodeName": "paragraph", "attributes": { "id": s(bid(1)) }, "children": [] }]
        }})
    );
    assert_eq!(page.to_portable(), doc_of(json!([para(bid(1), "")])));
    assert_eq!(
        page.blocks(),
        vec![BlockText {
            id: bid(1),
            kind: "paragraph".into(),
            inlines: Vec::new()
        }]
    );
    assert_eq!(page.block_ids(), vec![bid(1)]);
    assert!(page.is_empty());
    assert!(page.links().is_empty());
    assert_eq!(page.to_markdown(&no_titles), "\n");
    assert_sound(&page);
    // Committed: an export right away carries the creation.
    assert!(page.doc().export_updates_since(None).unwrap().is_some());

    // Loaded elsewhere, the same content.
    let snapshot = page.doc().export_snapshot().unwrap();
    let loaded =
        PageDoc::from_doc(Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap()).unwrap();
    assert_eq!(loaded.to_portable(), page.to_portable());
}

#[test]
fn the_root_triple_is_mergeable_and_element_triples_are_regular() {
    // Two documents created independently, by each way of creating a root.
    let empty = || PageDoc::from_doc(Doc::new(DocKind::Page, 7)).unwrap();
    let inserted = empty();
    inserted
        .insert_block(None, BlockKind::Paragraph, "x", bid(1))
        .unwrap();
    let pages = [
        new_page(1),
        new_page(2),
        from_json(&doc_of(json!([para(bid(1), "x")]))),
        inserted,
    ];
    let first = &pages[0];
    for page in &pages[1..] {
        // Mergeable: the container ids derive from the root and the key.
        assert_eq!(
            list_at(&root(page), "children").id(),
            list_at(&root(first), "children").id()
        );
        assert_eq!(
            map_at(&root(page), "attributes").id(),
            map_at(&root(first), "attributes").id()
        );
        // Regular: element containers are created by operations, per document.
        let node = child_map(&list_at(&root(page), "children"), 0);
        let first_node = child_map(&list_at(&root(first), "children"), 0);
        assert_ne!(
            list_at(&node, "children").id(),
            list_at(&first_node, "children").id()
        );
        assert_ne!(
            map_at(&node, "attributes").id(),
            map_at(&first_node, "attributes").id()
        );
    }
}

#[test]
fn a_root_with_regular_containers_still_reads_and_edits() {
    // What the binding's `getOrCreateContainer` and older TypeScript mirrors write.
    let doc = Doc::new(DocKind::Page, 1);
    let old_root = doc.inner().get_map("doc");
    old_root.insert("nodeName", "doc").unwrap();
    old_root
        .insert_container("attributes", LoroMap::new())
        .unwrap();
    let children = old_root
        .insert_container("children", LoroList::new())
        .unwrap();
    let paragraph = raw_node(&children, 0, "paragraph", &[("id", id_value(bid(1)))]);
    raw_text(&list_at(&paragraph, "children"), 0, "written the old way");
    doc.inner().commit();
    let regular_id = children.id();

    let page = PageDoc::from_doc(doc).unwrap();
    assert_sound(&page);
    assert_eq!(
        page.to_portable(),
        doc_of(json!([para(bid(1), "written the old way")]))
    );
    assert_eq!(page.block_ids(), vec![bid(1)]);
    // Every operation works on it and keeps the regular list (no conversion).
    page.insert_block(Some(bid(1)), BlockKind::Heading(2), "added", bid(2))
        .unwrap();
    page.insert_page_ref(bid(1), bid(3), page_id(1), None)
        .unwrap();
    page.set_block_text(bid(2), "edited").unwrap();
    page.delete_block(bid(2)).unwrap();
    page.insert_block(None, BlockKind::Divider, "", bid(4))
        .unwrap();
    assert_eq!(list_at(&root(&page), "children").id(), regular_id);
    assert_eq!(top_ids(&page), [s(bid(4)), s(bid(1))]);
    assert_eq!(page.links().len(), 1);
    assert_sound(&page);

    // Replicas of it converge and rebuild into the current shape.
    let snapshot = page.doc().export_snapshot().unwrap();
    let other =
        PageDoc::from_doc(Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap()).unwrap();
    other
        .insert_block(Some(bid(4)), BlockKind::Paragraph, "from the other", bid(5))
        .unwrap();
    let to_page = other
        .doc()
        .export_updates_since(Some(&page.doc().version()))
        .unwrap()
        .unwrap();
    page.doc().import(&to_page).unwrap();
    assert_eq!(page.to_portable(), other.to_portable());
    let rebuilt = from_json(&page.to_portable());
    assert_eq!(rebuilt.to_portable(), page.to_portable());
}

#[test]
fn new_refuses_other_kinds_and_initialised_documents() {
    for kind in [DocKind::Account, DocKind::Structure] {
        assert!(matches!(
            PageDoc::new(Doc::new(kind, 1), bid(1)),
            Err(ModelError::Shape(_))
        ));
        assert!(matches!(
            PageDoc::from_doc(Doc::new(kind, 1)),
            Err(ModelError::Shape(_))
        ));
        assert!(matches!(
            PageDoc::from_portable(Doc::new(kind, 1), &doc_of(json!([]))),
            Err(ModelError::Shape(_))
        ));
    }
    let snapshot = new_page(1).doc().export_snapshot().unwrap();
    let loaded = Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap();
    assert!(matches!(
        PageDoc::new(loaded, bid(2)),
        Err(ModelError::Shape(_))
    ));
}

#[test]
fn an_empty_engine_document_reads_as_a_page_without_blocks() {
    let page = PageDoc::from_doc(Doc::new(DocKind::Page, 1)).unwrap();
    assert!(page.blocks().is_empty());
    assert!(page.block_ids().is_empty());
    assert!(page.links().is_empty());
    assert!(page.is_empty());
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert_eq!(page.to_portable(), json!({ "type": "doc" }));
    assert_eq!(page.to_markdown(&no_titles), "\n");
    assert_sound(&page);
    // `{"type": "doc"}` round-trips.
    assert_eq!(
        from_json(&json!({ "type": "doc" })).to_portable(),
        json!({ "type": "doc" })
    );

    // Semantic operations create the root on demand.
    page.insert_block(None, BlockKind::Paragraph, "first", bid(5))
        .unwrap();
    assert_eq!(page.to_portable(), doc_of(json!([para(bid(5), "first")])));
    assert_sound(&page);
}

#[test]
fn page_documents_can_move_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PageDoc>();
}

#[test]
fn from_doc_rejects_a_root_that_is_not_a_doc() {
    for name in [LoroValue::from("paragraph"), LoroValue::I64(1)] {
        let doc = Doc::new(DocKind::Page, 1);
        doc.inner().get_map("doc").insert("nodeName", name).unwrap();
        doc.inner().commit();
        assert!(matches!(PageDoc::from_doc(doc), Err(ModelError::Shape(_))));
    }
}

// ----- portable JSON ------------------------------------------------------------------------

#[test]
fn portable_json_round_trips_every_node_kind() {
    let json: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/doc/page-v1.portable.json"
    ))
    .unwrap();
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).to_portable(), json);
    assert_sound(&page);
}

#[test]
fn from_portable_accepts_raw_prosemirror_json() {
    let raw = json!({
        "type": "doc",
        "attrs": {},
        "content": [
            {
                "type": "codeBlock",
                "attrs": { "id": s(bid(1)), "language": null },
                "marks": [],
                "content": [{ "type": "text", "text": "x = 1" }]
            },
            {
                "type": "paragraph",
                "attrs": { "id": s(bid(2)), "textAlign": null },
                "content": [
                    { "type": "text", "text": "" },
                    { "type": "text", "marks": [], "text": "plain " },
                    {
                        "type": "text",
                        "marks": [{ "type": "link", "attrs": { "href": "https://a.example", "target": null, "rel": null, "class": null } }],
                        "text": "link"
                    },
                    {
                        "type": "pageRef",
                        "attrs": { "id": s(bid(3)), "pageId": page_id(1).to_string(), "blockId": null },
                        "marks": [{ "type": "bold", "attrs": {} }]
                    }
                ]
            },
            { "type": "paragraph", "attrs": { "id": s(bid(4)) }, "content": [] },
            { "type": "horizontalRule", "attrs": { "id": s(bid(5)) }, "content": null, "marks": null }
        ]
    });
    let page = from_json(&raw);
    let expected = doc_of(json!([
        { "type": "codeBlock", "attrs": { "id": s(bid(1)) }, "content": [{ "type": "text", "text": "x = 1" }] },
        {
            "type": "paragraph",
            "attrs": { "id": s(bid(2)) },
            "content": [
                { "type": "text", "text": "plain " },
                { "type": "text", "marks": [{ "type": "link", "attrs": { "href": "https://a.example" } }], "text": "link" },
                { "type": "pageRef", "attrs": { "id": s(bid(3)), "pageId": page_id(1).to_string() } }
            ]
        },
        para(bid(4), ""),
        { "type": "horizontalRule", "attrs": { "id": s(bid(5)) } }
    ]));
    assert_eq!(page.to_portable(), expected);
    // Stored as the binding stores it: no null attributes, mark attrs as a map.
    let attributes = map_at(
        &child_map(&list_at(&root(&page), "children"), 0),
        "attributes",
    );
    assert_eq!(
        attributes.len(),
        1,
        "only the id; language null is not stored"
    );
    let paragraph = child_map(&list_at(&root(&page), "children"), 1);
    let delta = child_text(&list_at(&paragraph, "children"), 0).get_richtext_value();
    assert_eq!(
        delta.to_json_value(),
        json!([
            { "insert": "plain " },
            { "insert": "link", "attributes": { "link": { "href": "https://a.example" } } }
        ])
    );
}

#[test]
fn from_portable_rejects_anything_but_a_node_tree_and_writes_nothing() {
    let id = s(bid(1));
    let bad = [
        json!("doc"),
        json!([]),
        json!({ "type": "paragraph" }),
        json!({ "content": [] }),
        json!({ "type": "" }),
        json!({ "type": 5 }),
        json!({ "type": "doc", "content": {} }),
        json!({ "type": "doc", "content": ["text"] }),
        json!({ "type": "doc", "attrs": [] }),
        json!({ "type": "doc", "extra": 1 }),
        json!({ "type": "doc", "text": "x" }),
        json!({ "type": "doc", "content": [{ "type": "text" }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": 5 }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "attrs": { "a": 1 } }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "content": [{ "type": "text", "text": "y" }] }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "marks": {} }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "marks": ["bold"] }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "marks": [{ "attrs": {} }] }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "marks": [{ "type": "bold", "x": 1 }] }] }),
        json!({ "type": "doc", "content": [{ "type": "text", "text": "x", "marks": [{ "type": "link", "attrs": "h" }] }] }),
        json!({ "type": "doc", "content": [{ "type": "paragraph", "attrs": { "id": id }, "text": "x" }] }),
    ];
    for json in bad {
        let doc = Doc::new(DocKind::Page, 1);
        match PageDoc::from_portable(doc, &json) {
            Err(ModelError::Shape(msg)) => assert!(!msg.contains("doc\""), "{msg}"),
            other => panic!("{json} was accepted: {other:?}"),
        }
    }
    // Deeper than the readers follow.
    let mut deep = json!({ "type": "paragraph" });
    for _ in 0..200 {
        deep = json!({ "type": "blockquote", "content": [deep] });
    }
    assert!(matches!(
        PageDoc::from_portable(Doc::new(DocKind::Page, 1), &doc_of(json!([deep]))),
        Err(ModelError::Shape(_))
    ));
    // Only into an empty document.
    let snapshot = new_page(1).doc().export_snapshot().unwrap();
    let loaded = Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap();
    assert!(matches!(
        PageDoc::from_portable(loaded, &doc_of(json!([]))),
        Err(ModelError::Shape(_))
    ));
}

#[test]
fn text_runs_are_merged_and_null_marks_are_no_marks() {
    let page = new_page(1);
    let paragraph = child_map(&list_at(&root(&page), "children"), 0);
    let children = list_at(&paragraph, "children");
    // Adjacent texts (as concurrent edits leave them), an empty one, an atom.
    let first = raw_text(&children, 0, "Hello");
    raw_text(&children, 1, "");
    let third = raw_text(&children, 2, " world");
    raw_node(
        &children,
        3,
        "pageRef",
        &[
            ("id", id_value(bid(2))),
            ("pageId", LoroValue::from(page_id(1).to_string())),
        ],
    );
    raw_text(&children, 4, "!");
    first.mark(0..5, "bold", empty_map()).unwrap();
    first.unmark(2..5, "bold").unwrap(); // leaves bold: null on "llo"
    third.mark(0..6, "italic", empty_map()).unwrap();
    page.doc().inner().commit();

    assert_eq!(
        page.to_portable()["content"][0]["content"],
        json!([
            { "type": "text", "marks": [{ "type": "bold" }], "text": "He" },
            { "type": "text", "text": "llo" },
            { "type": "text", "marks": [{ "type": "italic" }], "text": " world" },
            { "type": "pageRef", "attrs": { "id": s(bid(2)), "pageId": page_id(1).to_string() } },
            { "type": "text", "text": "!" }
        ])
    );
    assert_eq!(
        page.blocks()[0].inlines,
        vec![
            Inline::Text("Hello world".into()),
            Inline::PageRef {
                page_id: page_id(1),
                block_id: None
            },
            Inline::Text("!".into()),
        ]
    );
    assert_eq!(
        page.to_markdown(&no_titles),
        format!("**He**llo* world*[[{}]]!\n", page_id(1))
    );
    assert_sound(&page);
    // The portable form rebuilds into one text per run of text.
    let rebuilt = from_json(&page.to_portable());
    assert_eq!(rebuilt.to_portable(), page.to_portable());
}

#[test]
fn marks_are_listed_in_canonical_order_with_non_null_attributes() {
    let page = new_page(1);
    let paragraph = child_map(&list_at(&root(&page), "children"), 0);
    let text = raw_text(&list_at(&paragraph, "children"), 0, "abc");
    text.mark(0..3, "zeta", LoroValue::Bool(true)).unwrap();
    text.mark(
        0..3,
        "link",
        LoroValue::Map(
            vec![
                ("href".to_owned(), LoroValue::from("https://x.example")),
                ("target".to_owned(), LoroValue::Null),
            ]
            .into(),
        ),
    )
    .unwrap();
    text.mark(
        0..3,
        "spoiler",
        LoroValue::Map(vec![("color".to_owned(), LoroValue::from("red"))].into()),
    )
    .unwrap();
    text.mark(0..3, "italic", empty_map()).unwrap();
    text.mark(0..3, "bold", empty_map()).unwrap();
    page.doc().inner().commit();
    assert_eq!(
        page.to_portable()["content"][0]["content"][0]["marks"],
        json!([
            { "type": "bold" },
            { "type": "italic" },
            { "type": "link", "attrs": { "href": "https://x.example" } },
            { "type": "spoiler", "attrs": { "color": "red" } },
            { "type": "zeta" }
        ])
    );
    assert_eq!(
        page.to_markdown(&no_titles),
        "[***abc***](https://x.example)\n"
    );
    let rebuilt = from_json(&page.to_portable());
    assert_eq!(rebuilt.to_portable(), page.to_portable());
}

#[test]
fn attribute_values_are_normalised() {
    let page = new_page(1);
    let children = list_at(&root(&page), "children");
    let heading = raw_node(
        &children,
        1,
        "heading",
        &[
            ("id", id_value(bid(2))),
            ("level", LoroValue::Double(2.0)),
            ("ratio", LoroValue::Double(0.5)),
            ("gone", LoroValue::Null),
            (
                "nested",
                LoroValue::Map(
                    vec![
                        ("a".to_owned(), LoroValue::Null),
                        ("b".to_owned(), LoroValue::I64(1)),
                    ]
                    .into(),
                ),
            ),
        ],
    );
    let attributes = map_at(&heading, "attributes");
    let list = attributes
        .insert_container("list", LoroList::new())
        .unwrap();
    list.push("x").unwrap();
    page.doc().inner().commit();
    assert_eq!(
        page.to_portable()["content"][1]["attrs"],
        json!({ "id": s(bid(2)), "level": 2, "ratio": 0.5, "nested": { "a": null, "b": 1 }, "list": ["x"] })
    );
    assert_eq!(page.to_markdown(&no_titles), "\n\n## \n");
}

#[test]
fn unknown_nodes_and_marks_pass_through() {
    let json = doc_of(json!([
        {
            "type": "fancyWidget",
            "attrs": { "id": s(bid(1)), "size": 3, "options": { "legend": true } },
            "content": [
                para(bid(2), "inside"),
                { "type": "text", "marks": [{ "type": "spoiler", "attrs": { "color": "yellow" } }], "text": "loose" }
            ]
        },
        para(bid(3), "after")
    ]));
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
    assert_eq!(
        page.unknown_node_names(schema::NODE_NAMES),
        vec!["fancyWidget".to_owned()]
    );
    assert_eq!(
        page.unknown_node_names(&["doc"]),
        vec!["fancyWidget".to_owned(), "paragraph".to_owned()]
    );
    assert_eq!(page.block_ids(), vec![bid(1), bid(2), bid(3)]);
    assert_eq!(
        page.blocks()[0],
        BlockText {
            id: bid(1),
            kind: "fancyWidget".into(),
            inlines: vec![Inline::Text("loose".into())]
        }
    );
    assert_eq!(page.to_markdown(&no_titles), "inside\n\nloose\n\nafter\n");
    assert_sound(&page);
}

// ----- blocks, links, emptiness -------------------------------------------------------------

#[test]
fn links_belong_to_the_nearest_enclosing_block() {
    let reference = |id: u8, page: u8| json!({ "type": "pageRef", "attrs": { "id": s(bid(id)), "pageId": page_id(page).to_string() } });
    let json = doc_of(json!([
        { "type": "bulletList", "attrs": { "id": s(bid(1)) }, "content": [
            { "type": "listItem", "attrs": { "id": s(bid(2)) }, "content": [
                { "type": "paragraph", "attrs": { "id": s(bid(3)) }, "content": [reference(4, 1)] },
                { "type": "paragraph", "content": [reference(5, 2)] }
            ]}
        ]},
        { "type": "paragraph", "attrs": { "id": s(bid(6)) }, "content": [
            { "type": "pageRef", "attrs": { "id": s(bid(7)), "pageId": "not-a-uuid" } },
            { "type": "pageRef", "attrs": { "id": s(bid(8)), "pageId": page_id(3).to_string(), "blockId": "nope" } },
            { "type": "pageRef", "attrs": { "id": s(bid(9)), "pageId": page_id(3).to_string(), "blockId": s(bid(42)) } }
        ]},
        { "type": "paragraph", "content": [reference(10, 4)] }
    ]));
    let page = from_json(&json);
    assert_eq!(
        page.links(),
        vec![
            PageLink {
                from_block: bid(3),
                to_page: page_id(1),
                to_block: None
            },
            PageLink {
                from_block: bid(2),
                to_page: page_id(2),
                to_block: None
            },
            PageLink {
                from_block: bid(6),
                to_page: page_id(3),
                to_block: None
            },
            PageLink {
                from_block: bid(6),
                to_page: page_id(3),
                to_block: Some(bid(42))
            },
        ]
    );
    let problems = page.check_invariants();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("without a valid pageId")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("malformed blockId")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("paragraph without an id")),
        "{problems:?}"
    );
    // The unresolvable reference renders with what is stored.
    assert!(page.to_markdown(&no_titles).contains("[[not-a-uuid]]"));
}

#[test]
fn emptiness_counts_text_references_and_breaks_only() {
    let empty_cases = [
        doc_of(json!([para(bid(1), "")])),
        doc_of(
            json!([{ "type": "horizontalRule", "attrs": { "id": s(bid(1)) } }, para(bid(2), "")]),
        ),
        doc_of(
            json!([{ "type": "bulletList", "attrs": { "id": s(bid(1)) }, "content": [
                { "type": "listItem", "attrs": { "id": s(bid(2)) }, "content": [para(bid(3), "")] }
            ]}]),
        ),
    ];
    for json in empty_cases {
        assert!(from_json(&json).is_empty(), "{json}");
    }
    let full_cases = [
        doc_of(json!([para(bid(1), " ")])),
        doc_of(
            json!([{ "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [{ "type": "hardBreak" }] }]),
        ),
        doc_of(
            json!([{ "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
                { "type": "pageRef", "attrs": { "id": s(bid(2)), "pageId": page_id(1).to_string() } }
            ]}]),
        ),
        doc_of(
            json!([{ "type": "codeBlock", "attrs": { "id": s(bid(1)) }, "content": [{ "type": "text", "text": "x" }] }]),
        ),
    ];
    for json in full_cases {
        assert!(!from_json(&json).is_empty(), "{json}");
    }
    // An emptied text container leaves the page empty again.
    let page = new_page(1);
    page.set_block_text(bid(1), "text").unwrap();
    assert!(!page.is_empty());
    page.set_block_text(bid(1), "").unwrap();
    assert!(page.is_empty());
}

// ----- semantic operations ------------------------------------------------------------------

#[test]
fn insert_block_creates_every_kind_in_the_editor_shape() {
    let page = new_page(1);
    page.insert_block(Some(bid(1)), BlockKind::Paragraph, "Hello", bid(2))
        .unwrap();
    page.insert_block(Some(bid(2)), BlockKind::Heading(2), "Title", bid(3))
        .unwrap();
    page.insert_block(
        Some(bid(3)),
        BlockKind::CodeBlock {
            language: Some("rust".into()),
        },
        "fn x() {}",
        bid(4),
    )
    .unwrap();
    page.insert_block(
        Some(bid(4)),
        BlockKind::CodeBlock { language: None },
        "",
        bid(5),
    )
    .unwrap();
    page.insert_block(Some(bid(5)), BlockKind::Quote, "Quoted", bid(6))
        .unwrap();
    page.insert_block(Some(bid(6)), BlockKind::Divider, "", bid(7))
        .unwrap();
    let quoted = PageDoc::quote_paragraph_id(bid(6));
    assert_eq!(
        page.to_portable(),
        doc_of(json!([
            para(bid(1), ""),
            para(bid(2), "Hello"),
            { "type": "heading", "attrs": { "id": s(bid(3)), "level": 2 }, "content": [{ "type": "text", "text": "Title" }] },
            { "type": "codeBlock", "attrs": { "id": s(bid(4)), "language": "rust" }, "content": [{ "type": "text", "text": "fn x() {}" }] },
            { "type": "codeBlock", "attrs": { "id": s(bid(5)) } },
            { "type": "blockquote", "attrs": { "id": s(bid(6)) }, "content": [para(quoted, "Quoted")] },
            { "type": "horizontalRule", "attrs": { "id": s(bid(7)) } }
        ]))
    );
    assert_eq!(
        page.to_markdown(&no_titles),
        "\n\nHello\n\n## Title\n\n```rust\nfn x() {}\n```\n\n```\n\n```\n\n> Quoted\n\n---\n"
    );
    assert_eq!(
        page.block_ids(),
        vec![
            bid(1),
            bid(2),
            bid(3),
            bid(4),
            bid(5),
            bid(6),
            quoted,
            bid(7)
        ]
    );
    assert_sound(&page);

    // The derived id keeps the UUIDv4 bits and differs from the block's.
    assert_ne!(quoted, bid(6));
    assert_eq!(quoted.as_bytes()[6] >> 4, 4);
    assert_eq!(quoted.as_bytes()[8] >> 6, 0b10);
    assert_eq!(PageDoc::quote_paragraph_id(quoted), bid(6));
}

#[test]
fn insert_block_positions_relative_to_top_level_blocks() {
    let page = new_page(1);
    page.insert_block(None, BlockKind::Paragraph, "zero", bid(2))
        .unwrap();
    assert_eq!(top_ids(&page), [s(bid(2)), s(bid(1))]);
    page.insert_block(Some(bid(2)), BlockKind::Quote, "q", bid(3))
        .unwrap();
    assert_eq!(top_ids(&page), [s(bid(2)), s(bid(3)), s(bid(1))]);
    // A nested anchor (the quote's paragraph) places after its top-level block.
    page.insert_block(
        Some(PageDoc::quote_paragraph_id(bid(3))),
        BlockKind::Paragraph,
        "after quote",
        bid(4),
    )
    .unwrap();
    assert_eq!(top_ids(&page), [s(bid(2)), s(bid(3)), s(bid(4)), s(bid(1))]);
    // Appending with the last id, even when it is a page reference.
    page.insert_page_ref(bid(1), bid(5), page_id(1), None)
        .unwrap();
    assert_eq!(page.block_ids().last(), Some(&bid(5)));
    let last = page.block_ids().last().copied();
    page.insert_block(last, BlockKind::Paragraph, "end", bid(6))
        .unwrap();
    assert_eq!(
        top_ids(&page),
        [s(bid(2)), s(bid(3)), s(bid(4)), s(bid(1)), s(bid(6))]
    );
    assert_sound(&page);
}

#[test]
fn insert_block_refuses_bad_requests_without_writing() {
    let page = new_page(1);
    page.insert_block(Some(bid(1)), BlockKind::Quote, "q", bid(2))
        .unwrap();
    page.insert_page_ref(bid(1), bid(3), page_id(1), None)
        .unwrap();
    let before = page.doc().version();
    let quoted = PageDoc::quote_paragraph_id(bid(2));
    for (after, kind, text, id) in [
        (None, BlockKind::Paragraph, "", bid(1)),
        (None, BlockKind::Paragraph, "", quoted),
        (None, BlockKind::Paragraph, "", bid(3)),
        (
            None,
            BlockKind::Quote,
            "",
            PageDoc::quote_paragraph_id(bid(3)),
        ),
    ] {
        let err = page.insert_block(after, kind, text, id).unwrap_err();
        assert!(is_duplicate(&err), "{err}");
    }
    let err = page
        .insert_block(Some(bid(99)), BlockKind::Paragraph, "", bid(9))
        .unwrap_err();
    assert!(is_not_found(&err), "{err}");
    for level in [0, 4, 7] {
        let err = page
            .insert_block(None, BlockKind::Heading(level), "h", bid(9))
            .unwrap_err();
        assert!(is_invalid(&err), "{err}");
    }
    let err = page
        .insert_block(None, BlockKind::Divider, "text", bid(9))
        .unwrap_err();
    assert!(is_invalid(&err), "{err}");
    assert_eq!(page.doc().version(), before, "nothing written");
    assert_sound(&page);
}

#[test]
fn set_block_text_replaces_the_inline_content_in_place() {
    let json = doc_of(json!([
        { "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
            { "type": "text", "marks": [{ "type": "bold" }], "text": "Hello" },
            { "type": "text", "text": " old " },
            { "type": "hardBreak" },
            { "type": "text", "text": "tail" },
            { "type": "pageRef", "attrs": { "id": s(bid(2)), "pageId": page_id(1).to_string() } }
        ]},
        { "type": "heading", "attrs": { "id": s(bid(3)), "level": 1 } },
        { "type": "codeBlock", "attrs": { "id": s(bid(4)) }, "content": [{ "type": "text", "text": "a" }] },
        { "type": "details", "attrs": { "id": s(bid(5)), "open": false }, "content": [
            { "type": "detailsSummary", "attrs": { "id": s(bid(6)) }, "content": [{ "type": "text", "text": "S" }] },
            { "type": "detailsContent", "attrs": { "id": s(bid(7)) }, "content": [para(bid(8), "inner")] }
        ]},
        { "type": "horizontalRule", "attrs": { "id": s(bid(9)) } }
    ]));
    let page = from_json(&json);
    let paragraph = child_map(&list_at(&root(&page), "children"), 0);
    let text_id = child_text(&list_at(&paragraph, "children"), 0).id();

    page.set_block_text(bid(1), "Hello new").unwrap();
    assert_eq!(
        page.blocks()[0].inlines,
        vec![Inline::Text("Hello new".into())]
    );
    assert_eq!(
        page.to_portable()["content"][0]["content"],
        json!([{ "type": "text", "text": "Hello new" }]),
        "marks, the break, the second text and the reference are gone"
    );
    let children = list_at(&paragraph, "children");
    assert_eq!(children.len(), 1);
    assert_eq!(
        child_text(&children, 0).id(),
        text_id,
        "the first text is kept"
    );
    assert!(page.links().is_empty());

    page.set_block_text(bid(3), "Heading").unwrap();
    page.set_block_text(bid(4), "b").unwrap();
    page.set_block_text(bid(6), "Summary").unwrap();
    page.set_block_text(bid(8), "nested").unwrap();
    assert_eq!(
        texts(&page),
        ["Hello new", "Heading", "b", "Summary", "nested"]
    );
    page.set_block_text(bid(8), "").unwrap();
    assert_eq!(
        page.to_portable()["content"][3]["content"][1]["content"][0],
        para(bid(8), "")
    );

    for id in [bid(5), bid(7), bid(9), bid(2)] {
        let err = page.set_block_text(id, "x").unwrap_err();
        if id == bid(2) {
            assert!(is_not_found(&err), "the reference was removed: {err}");
        } else {
            assert!(is_invalid(&err), "{err}");
        }
    }
    assert!(is_not_found(
        &page.set_block_text(bid(77), "x").unwrap_err()
    ));
    assert_sound(&page);
}

#[test]
fn set_block_text_drops_marks_that_typing_would_inherit() {
    let page = from_json(&doc_of(
        json!([{ "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
            { "type": "text", "text": "plain " },
            { "type": "text", "marks": [{ "type": "bold" }], "text": "bold" }
        ]}]),
    ));
    // Appending after a bold run (bold expands after) and replacing the start.
    page.set_block_text(bid(1), "Plain bold and more").unwrap();
    assert_eq!(
        page.to_portable()["content"][0]["content"],
        json!([{ "type": "text", "text": "Plain bold and more" }])
    );
    // A text without marks records no mark operations.
    let version = page.doc().version();
    page.set_block_text(bid(1), "Plain bold and more").unwrap();
    assert_eq!(page.doc().version(), version, "same text, nothing written");
}

#[test]
fn set_block_text_merges_with_concurrent_typing() {
    let a = from_json(&doc_of(json!([para(bid(1), "The quick fox")])));
    let snapshot = a.doc().export_snapshot().unwrap();
    let b = PageDoc::from_doc(Doc::load(DocKind::Page, 9, Some(&snapshot), &[]).unwrap()).unwrap();
    a.set_block_text(bid(1), "A quick fox").unwrap();
    // `b` types at the end of the same text at the same time.
    let paragraph = child_map(&list_at(&root(&b), "children"), 0);
    let text = child_text(&list_at(&paragraph, "children"), 0);
    text.insert(text.len_unicode(), " jumps").unwrap();
    b.doc().inner().commit();
    let to_b = a
        .doc()
        .export_updates_since(Some(&b.doc().version()))
        .unwrap()
        .unwrap();
    let to_a = b
        .doc()
        .export_updates_since(Some(&a.doc().version()))
        .unwrap()
        .unwrap();
    a.doc().import(&to_a).unwrap();
    b.doc().import(&to_b).unwrap();
    assert_eq!(texts(&a), ["A quick fox jumps"]);
    assert_eq!(a.to_portable(), b.to_portable());
}

#[test]
fn delete_block_removes_top_level_blocks_only() {
    let page = new_page(1);
    page.insert_block(Some(bid(1)), BlockKind::Quote, "q", bid(2))
        .unwrap();
    page.insert_block(Some(bid(2)), BlockKind::Paragraph, "p", bid(3))
        .unwrap();
    page.insert_page_ref(bid(3), bid(4), page_id(1), None)
        .unwrap();

    let err = page
        .delete_block(PageDoc::quote_paragraph_id(bid(2)))
        .unwrap_err();
    assert!(is_invalid(&err), "nested: {err}");
    let err = page.delete_block(bid(4)).unwrap_err();
    assert!(is_invalid(&err), "a reference: {err}");
    assert!(is_not_found(&page.delete_block(bid(50)).unwrap_err()));

    page.delete_block(bid(2)).unwrap();
    assert_eq!(top_ids(&page), [s(bid(1)), s(bid(3))]);
    page.delete_block(bid(3)).unwrap();
    assert!(page.links().is_empty());
    assert_eq!(page.block_ids(), vec![bid(1)]);
    let err = page.delete_block(bid(1)).unwrap_err();
    assert!(is_invalid(&err), "the last block stays: {err}");
    // The index follows: the deleted ids are free again.
    page.insert_block(Some(bid(1)), BlockKind::Paragraph, "again", bid(3))
        .unwrap();
    assert_eq!(top_ids(&page), [s(bid(1)), s(bid(3))]);
    assert_sound(&page);
}

#[test]
fn insert_page_ref_appends_a_reference_to_a_text_block() {
    let page = new_page(1);
    page.set_block_text(bid(1), "See ").unwrap();
    page.insert_page_ref(bid(1), bid(2), page_id(1), None)
        .unwrap();
    page.insert_block(Some(bid(1)), BlockKind::Heading(1), "Also ", bid(3))
        .unwrap();
    page.insert_page_ref(bid(3), bid(4), page_id(2), Some(bid(9)))
        .unwrap();

    assert_eq!(
        page.links(),
        vec![
            PageLink {
                from_block: bid(1),
                to_page: page_id(1),
                to_block: None
            },
            PageLink {
                from_block: bid(3),
                to_page: page_id(2),
                to_block: Some(bid(9))
            },
        ]
    );
    assert_eq!(
        page.blocks()[0].inlines,
        vec![
            Inline::Text("See ".into()),
            Inline::PageRef {
                page_id: page_id(1),
                block_id: None
            }
        ]
    );
    assert_eq!(
        page.to_portable(),
        doc_of(json!([
            { "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
                { "type": "text", "text": "See " },
                { "type": "pageRef", "attrs": { "id": s(bid(2)), "pageId": page_id(1).to_string() } }
            ]},
            { "type": "heading", "attrs": { "id": s(bid(3)), "level": 1 }, "content": [
                { "type": "text", "text": "Also " },
                { "type": "pageRef", "attrs": { "id": s(bid(4)), "pageId": page_id(2).to_string(), "blockId": s(bid(9)) } }
            ]}
        ]))
    );
    let titles = |id: PageId| (id == page_id(1)).then(|| "Target".to_owned());
    assert_eq!(
        page.to_markdown(&titles),
        format!("See [[Target]]\n\n# Also [[{}]]\n", page_id(2))
    );
    // Stored like the binding: empty children, no null attributes.
    let reference = child_map(
        &list_at(
            &child_map(&list_at(&root(&page), "children"), 0),
            "children",
        ),
        1,
    );
    assert_eq!(
        reference.get_deep_value().to_json_value(),
        json!({ "nodeName": "pageRef", "attributes": { "id": s(bid(2)), "pageId": page_id(1).to_string() }, "children": [] })
    );
    assert_sound(&page);

    // Errors.
    assert!(is_not_found(
        &page
            .insert_page_ref(bid(50), bid(51), page_id(1), None)
            .unwrap_err()
    ));
    assert!(is_duplicate(
        &page
            .insert_page_ref(bid(1), bid(2), page_id(1), None)
            .unwrap_err()
    ));
    assert!(is_duplicate(
        &page
            .insert_page_ref(bid(1), bid(3), page_id(1), None)
            .unwrap_err()
    ));
    page.insert_block(None, BlockKind::CodeBlock { language: None }, "c", bid(10))
        .unwrap();
    page.insert_block(None, BlockKind::Quote, "q", bid(11))
        .unwrap();
    for block in [bid(10), bid(11), bid(2)] {
        let err = page
            .insert_page_ref(block, bid(60), page_id(1), None)
            .unwrap_err();
        assert!(is_invalid(&err), "{err}");
    }
    page.insert_page_ref(
        PageDoc::quote_paragraph_id(bid(11)),
        bid(60),
        page_id(1),
        None,
    )
    .unwrap();
    assert_eq!(
        page.links()[0].from_block,
        PageDoc::quote_paragraph_id(bid(11))
    );
}

#[test]
fn the_block_index_follows_imports_and_raw_edits() {
    let a = new_page(1);
    let snapshot = a.doc().export_snapshot().unwrap();
    let b = PageDoc::from_doc(Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap()).unwrap();
    assert_eq!(b.block_ids(), vec![bid(1)]);

    a.insert_block(Some(bid(1)), BlockKind::Paragraph, "from a", bid(2))
        .unwrap();
    let update = a
        .doc()
        .export_updates_since(Some(&b.doc().version()))
        .unwrap()
        .unwrap();
    b.doc().import(&update).unwrap();
    assert_eq!(b.block_ids(), vec![bid(1), bid(2)]);
    b.insert_block(Some(bid(2)), BlockKind::Paragraph, "from b", bid(3))
        .unwrap();
    assert!(is_duplicate(
        &b.insert_block(None, BlockKind::Paragraph, "", bid(2))
            .unwrap_err()
    ));

    // A raw edit (what an editor update does) is seen as well.
    list_at(&root(&b), "children").delete(0, 1).unwrap();
    b.doc().inner().commit();
    assert_eq!(b.block_ids(), vec![bid(2), bid(3)]);
    b.insert_block(None, BlockKind::Paragraph, "first again", bid(1))
        .unwrap();
    assert_eq!(b.block_ids(), vec![bid(1), bid(2), bid(3)]);
    assert_eq!(texts(&b), ["first again", "from a", "from b"]);
    assert_sound(&b);
}

// ----- Markdown -----------------------------------------------------------------------------

fn md(content: Value) -> String {
    from_json(&doc_of(content)).to_markdown(&no_titles)
}

fn text_with(marks: Value, text: &str) -> Value {
    json!({ "type": "text", "marks": marks, "text": text })
}

#[test]
fn markdown_wraps_each_run_in_its_marks_outermost_first() {
    let content = json!([{ "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
        text_with(json!([{ "type": "bold" }, { "type": "italic" }]), "both"),
        { "type": "text", "text": " " },
        text_with(json!([{ "type": "bold" }, { "type": "link", "attrs": { "href": "https://l.example" } }]), "link"),
        { "type": "text", "text": " " },
        text_with(json!([{ "type": "strike" }, { "type": "code" }]), "gone"),
        { "type": "text", "text": " " },
        text_with(json!([{ "type": "underline" }]), "under"),
        { "type": "text", "text": " " },
        text_with(json!([{ "type": "bold" }]), "a"),
        text_with(json!([{ "type": "bold" }, { "type": "spoiler" }]), "b"),
        { "type": "text", "text": " " },
        text_with(json!([{ "type": "link" }]), "no href"),
        { "type": "hardBreak" },
        text_with(json!([{ "type": "italic" }]), "x"),
    ]}]);
    assert_eq!(
        md(content),
        "***both*** [**link**](https://l.example) ~~`gone`~~ <u>under</u> **ab** [no href]()  \n*x*\n"
    );
}

#[test]
fn colour_marks_render_as_plain_text_and_do_not_split_a_run() {
    // `textColor` and `highlight` (added 2026-10-07): Markdown has no colour, so the text
    // appears as if it had no mark, and a mark it cannot show never breaks a run of
    // bold or italic text into several (`**a****b**`).
    let colour = |name: &str, color: &str| json!({ "type": name, "attrs": { "color": color } });
    let content = json!([
        { "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
            { "type": "text", "text": "plain " },
            text_with(json!([colour("textColor", "red")]), "red"),
            { "type": "text", "text": " " },
            text_with(json!([colour("highlight", "yellow")]), "marked"),
            { "type": "text", "text": " " },
            text_with(json!([colour("textColor", "blue"), colour("highlight", "pink")]), "both"),
            { "type": "text", "text": " " },
            text_with(json!([colour("textColor", "chartreuse")]), "unknown"),
            { "type": "text", "text": " " },
            text_with(json!([{ "type": "textColor" }]), "none")
        ]},
        { "type": "paragraph", "attrs": { "id": s(bid(2)) }, "content": [
            text_with(json!([{ "type": "bold" }]), "one "),
            text_with(json!([{ "type": "bold" }, colour("textColor", "red")]), "red"),
            text_with(json!([{ "type": "bold" }, colour("highlight", "red")]), " two"),
            { "type": "text", "text": " " },
            text_with(json!([{ "type": "italic" }]), "thr"),
            text_with(json!([{ "type": "italic" }, colour("highlight", "green")]), "ee")
        ]},
        { "type": "heading", "attrs": { "id": s(bid(3)), "level": 2 }, "content": [
            text_with(json!([colour("textColor", "green"), { "type": "link", "attrs": { "href": "https://l.example" } }]), "linked "),
            text_with(json!([colour("textColor", "red"), { "type": "link", "attrs": { "href": "https://l.example" } }]), "twice"),
            text_with(json!([{ "type": "link", "attrs": { "href": "https://m.example" } }]), "other")
        ]},
        { "type": "codeBlock", "attrs": { "id": s(bid(4)) }, "content": [
            text_with(json!([colour("textColor", "red")]), "plain code")
        ]}
    ]);
    assert_eq!(
        md(content),
        "plain red marked both unknown none\n\n**one red two** *three*\n\n\
         ## [linked twice](https://l.example)[other](https://m.example)\n\n```\nplain code\n```\n"
    );
}

#[test]
fn colour_marks_keep_their_attribute_and_sort_after_link() {
    let colour = |name: &str, color: &str| json!({ "type": name, "attrs": { "color": color } });
    let json = doc_of(
        json!([{ "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
            { "type": "text", "text": "a " },
            text_with(json!([colour("textColor", "red")]), "red"),
            { "type": "text", "text": " " },
            text_with(json!([colour("highlight", "yellow")]), "marked"),
            { "type": "text", "text": " " },
            text_with(json!([{ "type": "bold" }, { "type": "link", "attrs": { "href": "https://l.example" } }, colour("textColor", "blue"), colour("highlight", "pink")]), "all"),
            { "type": "text", "text": " " },
            text_with(json!([colour("textColor", "chartreuse")]), "newer"),
            { "type": "text", "text": " " },
            text_with(json!([{ "type": "textColor" }]), "bare")
        ]}]),
    );
    // Given in the reverse order, the marks come out in the order of the table: the six
    // marks of the first table, `textColor`, `highlight`, then unknown marks by name.
    let mut shuffled = json.clone();
    shuffled["content"][0]["content"][5]["marks"] = json!([
        { "type": "zeta" },
        colour("highlight", "pink"),
        colour("textColor", "blue"),
        { "type": "link", "attrs": { "href": "https://l.example" } },
        { "type": "bold" }
    ]);
    let page = from_json(&shuffled);
    assert_eq!(
        page.to_portable()["content"][0]["content"][5]["marks"],
        json!([
            { "type": "bold" },
            { "type": "link", "attrs": { "href": "https://l.example" } },
            colour("textColor", "blue"),
            colour("highlight", "pink"),
            { "type": "zeta" }
        ])
    );
    // Every colour, a name this version does not know and a mark without attributes survive.
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).to_portable(), json);
    assert_sound(&page);
    assert_eq!(
        page.blocks()[0].inlines,
        vec![Inline::Text("a red marked all newer bare".into())]
    );
}

#[test]
fn the_mark_table_ends_with_the_two_colour_marks() {
    let names: Vec<&str> = schema::MARKS.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        [
            "bold",
            "italic",
            "underline",
            "strike",
            "code",
            "link",
            "textColor",
            "highlight"
        ]
    );
    for (name, expand) in schema::MARKS {
        let expected = if matches!(*name, "code" | "link") {
            kb_doc::MarkExpand::None
        } else {
            kb_doc::MarkExpand::After
        };
        assert_eq!(*expand, expected, "{name}");
    }
}

#[test]
fn markdown_lists_quotes_details_and_unknown_blocks() {
    let item = |id: u8, text: &str| json!({ "type": "listItem", "attrs": { "id": s(bid(id)) }, "content": [para(bid(id + 100), text)] });
    let content = json!([
        { "type": "orderedList", "attrs": { "id": s(bid(1)), "start": 9 }, "content": [
            item(2, "nine"),
            { "type": "listItem", "attrs": { "id": s(bid(3)) }, "content": [
                para(bid(103), "ten"),
                para(bid(104), "second paragraph"),
                { "type": "bulletList", "attrs": { "id": s(bid(5)) }, "content": [item(6, "nested")] }
            ]}
        ]},
        { "type": "taskList", "attrs": { "id": s(bid(7)) }, "content": [
            { "type": "taskItem", "attrs": { "id": s(bid(8)), "checked": true }, "content": [
                para(bid(108), "done"),
                { "type": "taskList", "attrs": { "id": s(bid(9)) }, "content": [
                    { "type": "taskItem", "attrs": { "id": s(bid(10)), "checked": false }, "content": [para(bid(110), "sub")] }
                ]}
            ]},
            { "type": "taskItem", "attrs": { "id": s(bid(11)) }, "content": [para(bid(111), "")] }
        ]},
        { "type": "blockquote", "attrs": { "id": s(bid(12)) }, "content": [
            para(bid(112), "one"),
            para(bid(113), "two"),
            { "type": "codeBlock", "attrs": { "id": s(bid(14)) }, "content": [{ "type": "text", "text": "a\n\nb" }] }
        ]},
        { "type": "callout", "attrs": { "id": s(bid(15)) }, "content": [para(bid(115), "no icon"), para(bid(116), "more")] },
        { "type": "details", "attrs": { "id": s(bid(17)) }, "content": [
            { "type": "detailsSummary", "attrs": { "id": s(bid(18)) }, "content": [
                text_with(json!([{ "type": "bold" }]), "Sum"),
                { "type": "text", "text": "mary" }
            ]},
            { "type": "detailsContent", "attrs": { "id": s(bid(19)) } }
        ]},
        { "type": "heading", "attrs": { "id": s(bid(20)), "level": 9 }, "content": [{ "type": "text", "text": "Deep" }] },
        { "type": "heading", "attrs": { "id": s(bid(21)) }, "content": [{ "type": "text", "text": "Default" }] },
        { "type": "mystery", "attrs": { "id": s(bid(22)) }, "content": [para(bid(122), "a"), para(bid(123), "b")] },
        para(bid(24), "")
    ]);
    let expected = "\
9. nine
10. ten
    second paragraph
    - nested

- [x] done
  - [ ] sub
- [ ]\x20

> one
>
> two
>
> ```
> a
>
> b
> ```

> 💡 no icon
>
> more

<details>
<summary>Summary</summary>



</details>

# Deep

# Default

a

b
";
    assert_eq!(md(content), expected);
}

#[test]
fn markdown_ignores_the_colour_of_a_callout() {
    // `color` (ADR-0016, amendment of 2026-10-06) is a background name: a palette name, a
    // name this version does not know and a stored non-string all export like no colour.
    let callout = |id: u8, attrs: Value| {
        let mut attrs = attrs;
        attrs["id"] = json!(s(bid(id)));
        json!({ "type": "callout", "attrs": attrs, "content": [para(bid(id + 100), "note")] })
    };
    let plain = md(json!([callout(1, json!({ "icon": "⚠️" }))]));
    assert_eq!(plain, "> ⚠️ note\n");
    for color in [json!("blue"), json!("red"), json!("chartreuse"), json!(7)] {
        assert_eq!(
            md(json!([callout(1, json!({ "icon": "⚠️", "color": color }))])),
            plain,
            "{color}"
        );
    }
    assert_eq!(
        md(json!([callout(1, json!({ "color": "green" }))])),
        "> 💡 note\n",
        "the default icon still applies"
    );
}

#[test]
fn markdown_resolves_page_titles() {
    let content = json!([{ "type": "paragraph", "attrs": { "id": s(bid(1)) }, "content": [
        { "type": "pageRef", "attrs": { "id": s(bid(2)), "pageId": page_id(1).to_string() } },
        { "type": "text", "text": " and " },
        { "type": "pageRef", "attrs": { "id": s(bid(3)), "pageId": page_id(2).to_string(), "blockId": s(bid(4)) } }
    ]}]);
    let page = from_json(&doc_of(content));
    let titles = |id: PageId| (id == page_id(2)).then(|| "Known".to_owned());
    assert_eq!(
        page.to_markdown(&titles),
        format!("[[{}]] and [[Known]]\n", page_id(1))
    );
}

// ----- invariants ---------------------------------------------------------------------------

#[test]
fn check_invariants_reports_damaged_documents_without_panicking() {
    let page = new_page(1);
    let children = list_at(&root(&page), "children");
    // Duplicate, malformed and non-string ids; a schema node without id.
    raw_node(&children, 1, "paragraph", &[("id", id_value(bid(1)))]);
    raw_node(&children, 2, "paragraph", &[("id", LoroValue::from("xyz"))]);
    raw_node(&children, 3, "paragraph", &[("id", LoroValue::I64(5))]);
    raw_node(&children, 4, "heading", &[]);
    // Broken triples.
    let no_name = children.insert_container(5, LoroMap::new()).unwrap();
    no_name.insert("attributes", 1).unwrap();
    let bad_name = children.insert_container(6, LoroMap::new()).unwrap();
    bad_name.insert("nodeName", 7).unwrap();
    let bare = children.insert_container(7, LoroMap::new()).unwrap();
    bare.insert("nodeName", "callout").unwrap();
    bare.insert("attributes", "not a map").unwrap();
    bare.insert("children", true).unwrap();
    // Entries that are not nodes, and names no element may have.
    children.insert(8, "a plain value").unwrap();
    children.insert_container(9, LoroList::new()).unwrap();
    raw_node(&children, 10, "", &[("id", id_value(bid(20)))]);
    raw_node(&children, 11, "text", &[("id", id_value(bid(21)))]);
    page.doc().inner().commit();

    let problems = page.check_invariants();
    let expect = [
        format!("block id {} appears 2 times", bid(1)),
        "node /2: malformed id".to_owned(),
        "node /3: id is not a string".to_owned(),
        "node /4: heading without an id".to_owned(),
        "node /5: no nodeName".to_owned(),
        "node /6: nodeName is not a string".to_owned(),
        "node /7: attributes is not a map".to_owned(),
        "node /7: children is not a list".to_owned(),
        "node /7: callout without an id".to_owned(),
        "node /8: children entry is neither a node nor a text".to_owned(),
        "node /9: children entry is neither a node nor a text".to_owned(),
        "node /10: empty nodeName".to_owned(),
        "node /11: element node named text".to_owned(),
    ];
    for message in &expect {
        assert!(
            problems.contains(message),
            "missing {message:?} in {problems:#?}"
        );
    }
    assert_eq!(problems.len(), expect.len(), "{problems:#?}");
    // Readers skip what they cannot read.
    assert_eq!(page.block_ids(), vec![bid(1), bid(1)]);
    assert_eq!(page.blocks().len(), 2);
    assert_eq!(
        top_ids(&page),
        [
            s(bid(1)),
            s(bid(1)),
            "xyz".to_owned(),
            String::new(),
            String::new(),
            String::new()
        ]
    );
    let _ = page.to_markdown(&no_titles);
    // Semantic operations still work around the damage.
    page.insert_block(None, BlockKind::Paragraph, "ok", bid(30))
        .unwrap();
    assert_eq!(page.block_ids()[0], bid(30));

    // A root that became something else after wrapping, and missing triple parts.
    let doc = Doc::new(DocKind::Page, 1);
    let page = PageDoc::from_doc(doc).unwrap();
    root(&page).insert("nodeName", "paragraph").unwrap();
    page.doc().inner().commit();
    assert_eq!(
        page.check_invariants(),
        [
            "root: nodeName is not \"doc\"",
            "root: no attributes map",
            "root: no children list"
        ]
    );
}

#[test]
fn nesting_beyond_the_reader_limit_is_cut_off_and_reported() {
    let page = new_page(1);
    let mut list = list_at(&root(&page), "children");
    for n in 0..200u32 {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&n.to_le_bytes());
        bytes[15] = 0xdd;
        let node = raw_node(
            &list,
            0,
            "blockquote",
            &[("id", id_value(BlockId::from_random_bytes(bytes)))],
        );
        list = list_at(&node, "children");
    }
    page.doc().inner().commit();
    let problems = page.check_invariants();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("nested deeper than 128 levels"),
        "{problems:?}"
    );
    // The paragraph plus the 128 readable levels.
    assert_eq!(page.block_ids().len(), 1 + 128);
    let portable = page.to_portable();
    assert_eq!(portable["content"].as_array().map(Vec::len), Some(2));
    let _ = page.to_markdown(&no_titles);
}

// ----- performance --------------------------------------------------------------------------

/// About 1 MB of varied text (2,000 paragraphs of 500 characters), so the snapshot
/// cannot compress it away.
fn big_page() -> PageDoc {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let letters: Vec<char> = "abcdefghijklmnopqrstuvwxyz     éàü".chars().collect();
    let content: Vec<Value> = (0..2_000u32)
        .map(|n| {
            let text: String = (0..500)
                .map(|_| letters[usize::try_from(next() % letters.len() as u64).unwrap()])
                .collect();
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&n.to_le_bytes());
            bytes[15] = 0xbb;
            para(BlockId::from_random_bytes(bytes), &text)
        })
        .collect();
    from_json(&doc_of(Value::Array(content)))
}

#[test]
fn a_one_megabyte_page_loads_and_lists_its_blocks_within_budget() {
    let page = big_page();
    let snapshot = page.doc().export_snapshot().unwrap();
    assert!(snapshot.len() > 900_000, "{} bytes", snapshot.len());

    let started = Instant::now();
    let loaded =
        PageDoc::from_doc(Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap()).unwrap();
    let blocks = loaded.blocks();
    let elapsed = started.elapsed();
    assert_eq!(blocks.len(), 2_000);
    assert_eq!(
        blocks
            .iter()
            .map(|b| match b.inlines.as_slice() {
                [Inline::Text(text)] => text.chars().count(),
                _ => 0,
            })
            .sum::<usize>(),
        1_000_000
    );
    let budget = if cfg!(debug_assertions) {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(200)
    };
    assert!(
        elapsed < budget,
        "load + blocks took {elapsed:?}, budget {budget:?}"
    );
}

#[test]
fn appending_blocks_one_by_one_stays_linear() {
    // The core's tests append with `block_ids().last()` before every insert.
    let page = new_page(1);
    let started = Instant::now();
    for n in 0..1_000u32 {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&n.to_le_bytes());
        bytes[15] = 0xcc;
        let last = page.block_ids().last().copied();
        page.insert_block(
            last,
            BlockKind::Paragraph,
            "line",
            BlockId::from_random_bytes(bytes),
        )
        .unwrap();
    }
    let elapsed = started.elapsed();
    assert_eq!(page.block_ids().len(), 1_001);
    // Well under a second in release; the assertion only guards against blow-ups.
    assert!(
        elapsed < Duration::from_secs(20),
        "1,000 appends took {elapsed:?}"
    );
}

// ----- image and file blocks (Milestone 5) --------------------------------------------------

fn aid(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([
        0xa0 | (n & 0x0f),
        n,
        9,
        8,
        7,
        6,
        5,
        4,
        3,
        2,
        1,
        0,
        1,
        2,
        3,
        n,
    ])
}

fn image(id: BlockId, attachment: &str, alt: &str) -> Value {
    json!({ "type": "image", "attrs": { "id": s(id), "attachmentId": attachment, "alt": alt } })
}

fn file(id: BlockId, attachment: &str) -> Value {
    json!({ "type": "file", "attrs": { "id": s(id), "attachmentId": attachment } })
}

#[test]
fn insert_block_writes_image_and_file_leaves_in_the_editor_shape() {
    let page = new_page(1);
    page.insert_block(
        Some(bid(1)),
        BlockKind::Image {
            attachment_id: aid(1),
            width: Some(480),
        },
        "A cat",
        bid(2),
    )
    .unwrap();
    page.insert_block(
        Some(bid(2)),
        BlockKind::Image {
            attachment_id: aid(2),
            width: None,
        },
        "",
        bid(3),
    )
    .unwrap();
    page.insert_block(
        Some(bid(3)),
        BlockKind::File {
            attachment_id: aid(3),
        },
        "",
        bid(4),
    )
    .unwrap();
    assert_eq!(
        page.to_portable(),
        doc_of(json!([
            para(bid(1), ""),
            { "type": "image", "attrs": { "alt": "A cat", "attachmentId": aid(1).to_string(), "id": s(bid(2)), "width": 480 } },
            { "type": "image", "attrs": { "alt": "", "attachmentId": aid(2).to_string(), "id": s(bid(3)) } },
            { "type": "file", "attrs": { "attachmentId": aid(3).to_string(), "id": s(bid(4)) } }
        ]))
    );
    // Leaves in the binding's shape: an empty children list, `width` absent when unset,
    // stored as an integer otherwise; the alt text is stored even when empty.
    let blocks = list_at(&root(&page), "children");
    for pos in 1..=3 {
        assert_eq!(list_at(&child_map(&blocks, pos), "children").len(), 0);
    }
    let first = map_at(&child_map(&blocks, 1), "attributes");
    assert_eq!(
        first.get("width").map(|v| v.get_deep_value()),
        Some(LoroValue::I64(480))
    );
    let second = map_at(&child_map(&blocks, 2), "attributes");
    assert!(second.get("width").is_none());
    assert_eq!(
        second.get("alt").map(|v| v.get_deep_value()),
        Some(LoroValue::from(""))
    );
    assert_eq!(page.block_ids(), vec![bid(1), bid(2), bid(3), bid(4)]);
    assert_sound(&page);

    // A file block takes no text; ids stay unique; nothing is written on refusal.
    let version = page.doc().version();
    let file_with_text = page.insert_block(
        None,
        BlockKind::File {
            attachment_id: aid(4),
        },
        "name",
        bid(5),
    );
    assert!(is_invalid(&file_with_text.unwrap_err()));
    let duplicate = page.insert_block(
        None,
        BlockKind::Image {
            attachment_id: aid(4),
            width: None,
        },
        "",
        bid(2),
    );
    assert!(is_duplicate(&duplicate.unwrap_err()));
    assert_eq!(page.doc().version(), version);

    // They hold no text of their own, are deleted like any top-level block.
    assert!(is_invalid(&page.set_block_text(bid(2), "x").unwrap_err()));
    page.delete_block(bid(4)).unwrap();
    assert_eq!(
        page.attachment_refs(),
        vec![(bid(2), aid(1)), (bid(3), aid(2))]
    );
}

#[test]
fn image_blocks_read_as_their_alt_text_and_file_blocks_as_nothing() {
    let page = from_json(&doc_of(json!([
        image(bid(1), &aid(1).to_string(), "Sunset over the bay"),
        image(bid(2), &aid(2).to_string(), ""),
        file(bid(3), &aid(3).to_string()),
        { "type": "image", "attrs": { "id": s(bid(4)), "attachmentId": aid(4).to_string(), "alt": 7 } },
    ])));
    let blocks = page.blocks();
    assert_eq!(
        blocks,
        vec![
            BlockText {
                id: bid(1),
                kind: "image".into(),
                inlines: vec![Inline::Text("Sunset over the bay".into())],
            },
            BlockText {
                id: bid(2),
                kind: "image".into(),
                inlines: Vec::new(),
            },
            BlockText {
                id: bid(3),
                kind: "file".into(),
                inlines: Vec::new(),
            },
            BlockText {
                id: bid(4),
                kind: "image".into(),
                inlines: Vec::new(),
            },
        ]
    );
    assert_eq!(texts(&page), vec!["Sunset over the bay".to_owned()]);

    // A page holding only an attachment block is not empty.
    for json in [
        doc_of(json!([image(bid(1), &aid(1).to_string(), "")])),
        doc_of(json!([file(bid(1), &aid(1).to_string())])),
    ] {
        assert!(!from_json(&json).is_empty(), "{json}");
    }
}

#[test]
fn attachment_refs_list_every_reference_in_document_order() {
    let page = from_json(&doc_of(json!([
        file(bid(1), &aid(1).to_string()),
        { "type": "bulletList", "attrs": { "id": s(bid(2)) }, "content": [
            { "type": "listItem", "attrs": { "id": s(bid(3)) }, "content": [
                para(bid(4), "with a picture"),
                image(bid(5), &aid(2).to_string(), "nested")
            ]}
        ]},
        { "type": "callout", "attrs": { "id": s(bid(6)) }, "content": [
            image(bid(7), &aid(1).to_string(), "the same file again")
        ]},
        // Left out: a malformed or missing attachment id, a block without a valid id,
        // and attachment ids on nodes that are not attachment blocks.
        image(bid(8), "not-a-uuid", ""),
        { "type": "file", "attrs": { "id": s(bid(9)) } },
        { "type": "file", "attrs": { "attachmentId": aid(3).to_string() } },
        { "type": "image", "attrs": { "id": "xyz", "attachmentId": aid(3).to_string() } },
        { "type": "fancyWidget", "attrs": { "id": s(bid(10)), "attachmentId": aid(4).to_string() } },
        file(bid(11), &aid(2).to_string()),
    ])));
    assert_eq!(
        page.attachment_refs(),
        vec![
            (bid(1), aid(1)),
            (bid(5), aid(2)),
            (bid(7), aid(1)),
            (bid(11), aid(2)),
        ]
    );
    // An empty document has none.
    let empty = PageDoc::from_doc(Doc::new(DocKind::Page, 2)).unwrap();
    assert!(empty.attachment_refs().is_empty());
}

#[test]
fn first_image_is_the_first_picture_in_document_order_at_any_depth() {
    // A file is no picture; the image inside a callout in a toggle comes before the later ones.
    let page = from_json(&doc_of(json!([
        file(bid(1), &aid(1).to_string()),
        para(bid(2), "words"),
        { "type": "bulletList", "attrs": { "id": s(bid(3)) }, "content": [
            { "type": "listItem", "attrs": { "id": s(bid(4)) }, "content": [
                { "type": "callout", "attrs": { "id": s(bid(5)) }, "content": [
                    image(bid(6), &aid(2).to_string(), "nested")
                ]}
            ]}
        ]},
        image(bid(7), &aid(3).to_string(), "later"),
    ])));
    assert_eq!(page.first_image(), Some(aid(2)));

    // The first item of a gallery counts, and the gallery wins when it comes first.
    let page = from_json(&doc_of(json!([
        gallery_with(1, &json!({ "items": [item(4, Some("a")), item(5, None)] })),
        image(bid(7), &aid(3).to_string(), "later"),
    ])));
    assert_eq!(page.first_image(), Some(aid(4)));

    // ... and an image first beats the gallery after it.
    let page = from_json(&doc_of(json!([
        image(bid(7), &aid(3).to_string(), ""),
        gallery_with(1, &json!({ "items": [item(4, None)] })),
    ])));
    assert_eq!(page.first_image(), Some(aid(3)));
}

#[test]
fn first_image_skips_what_has_no_usable_attachment_or_block_and_what_is_no_picture() {
    let page = from_json(&doc_of(json!([
        // Skipped: a malformed or missing attachment id, a block without a valid id, a gallery with no
        // list or only damaged entries, a widget that merely carries an attachment id.
        image(bid(1), "not-a-uuid", ""),
        { "type": "image", "attrs": { "id": s(bid(2)) } },
        { "type": "image", "attrs": { "attachmentId": aid(1).to_string() } },
        { "type": "image", "attrs": { "id": "xyz", "attachmentId": aid(1).to_string() } },
        gallery_with(3, &json!({})),
        gallery_with(4, &json!({ "items": [5, { "caption": "no picture" }, { "attachmentId": "nope" }] })),
        { "type": "whiteboard", "attrs": { "id": s(bid(5)) } },
        { "type": "fancyWidget", "attrs": { "id": s(bid(6)), "attachmentId": aid(2).to_string() } },
        // The first damaged entry of a gallery is passed over for the next usable one.
        gallery_with(7, &json!({ "items": [{ "caption": "x" }, { "attachmentId": aid(6).to_string() }] })),
    ])));
    assert_eq!(page.first_image(), Some(aid(6)));

    let none = from_json(&doc_of(json!([
        para(bid(1), "no picture here"),
        file(bid(2), &aid(1).to_string()),
    ])));
    assert_eq!(none.first_image(), None);
    let empty = PageDoc::from_doc(Doc::new(DocKind::Page, 2)).unwrap();
    assert_eq!(empty.first_image(), None);
}

#[test]
fn markdown_renders_images_and_files_as_attachment_links() {
    let one = aid(1).to_string();
    let two = aid(2).to_string();
    let page = from_json(&doc_of(json!([
        image(bid(1), &one, "Alt text"),
        { "type": "image", "attrs": { "id": s(bid(2)), "attachmentId": two.clone(), "width": 300 } },
        file(bid(3), &one),
        file(bid(4), &two),
        file(bid(5), &one),
        { "type": "bulletList", "attrs": { "id": s(bid(6)) }, "content": [
            { "type": "listItem", "attrs": { "id": s(bid(7)) }, "content": [
                para(bid(8), "item"),
                file(bid(9), &two)
            ]}
        ]},
        file(bid(10), "broken"),
    ])));
    let mut asked = Vec::new();
    let markdown = page.to_markdown_with(&no_titles, &mut |id| {
        asked.push(id);
        (id == aid(1)).then(|| "report.pdf".to_owned())
    });
    assert_eq!(
        markdown,
        format!(
            "![Alt text](attachment://{one})\n\n![](attachment://{two})\n\n\
             [report.pdf](attachment://{one})\n\n[{two}](attachment://{two})\n\n\
             [report.pdf](attachment://{one})\n\n- item\n  [{two}](attachment://{two})\n\n\
             [broken](attachment://broken)\n"
        )
    );
    // Asked once per distinct valid id of the file blocks, in document order.
    assert_eq!(asked, vec![aid(1), aid(2)]);
    // Without a resolver, files show their attachment id.
    assert!(
        page.to_markdown(&no_titles)
            .contains(&format!("[{one}](attachment://{one})"))
    );
}

#[test]
fn check_invariants_reports_attachment_blocks_without_a_valid_attachment() {
    let page = from_json(&doc_of(json!([
        image(bid(1), &aid(1).to_string(), "fine"),
        image(bid(2), "nope", "secret alt"),
        { "type": "file", "attrs": { "id": s(bid(3)) } },
        { "type": "file", "attrs": { "attachmentId": aid(2).to_string() } },
    ])));
    let problems = page.check_invariants();
    assert_eq!(
        problems,
        vec![
            "node /1: image without a valid attachmentId".to_owned(),
            "node /2: file without a valid attachmentId".to_owned(),
            "node /3: file without an id".to_owned(),
        ]
    );
    assert!(problems.iter().all(|p| !p.contains("secret")));
}

// ----- image width in percent, alignment and caption (ADR-0016 amendment of 2026-10-09) ------

/// An `image` with a valid attachment and the given extra attributes (`alt` is left to them).
fn image_with(id: u8, attrs: &Value) -> Value {
    let mut all = json!({ "id": s(bid(id)), "attachmentId": aid(1).to_string() });
    if let (Some(all), Some(extra)) = (all.as_object_mut(), attrs.as_object()) {
        all.extend(extra.clone());
    }
    json!({ "type": "image", "attrs": all })
}

fn image_texts(page: &PageDoc) -> Vec<Vec<Inline>> {
    page.blocks()
        .into_iter()
        .map(|block| block.inlines)
        .collect()
}

#[test]
fn an_image_reads_as_its_caption_then_its_alt_text() {
    let page = from_json(&doc_of(json!([
        image_with(1, &json!({ "alt": "A cat", "caption": "Our cat, asleep" })),
        image_with(2, &json!({ "alt": "", "caption": "Only a caption" })),
        image_with(3, &json!({ "alt": "Only alt text" })),
        image_with(4, &json!({ "alt": "Alt text", "caption": "" })),
        image_with(5, &json!({ "alt": "", "caption": "" })),
        image_with(6, &json!({})),
        // Layout is not text, and a damaged caption or alt reads as absent.
        image_with(
            7,
            &json!({ "alt": "A", "widthPercent": 50, "align": "right", "width": 640 })
        ),
        image_with(8, &json!({ "alt": "Kept", "caption": 7 })),
        image_with(9, &json!({ "alt": 7, "caption": "Kept too" })),
    ])));
    assert_eq!(
        image_texts(&page),
        vec![
            vec![Inline::Text("Our cat, asleep A cat".into())],
            vec![Inline::Text("Only a caption".into())],
            vec![Inline::Text("Only alt text".into())],
            vec![Inline::Text("Alt text".into())],
            Vec::new(),
            Vec::new(),
            vec![Inline::Text("A".into())],
            vec![Inline::Text("Kept".into())],
            vec![Inline::Text("Kept too".into())],
        ]
    );
    // A caption is text: the page is not empty because of it.
    let only = from_json(&doc_of(json!([image_with(1, &json!({ "caption": "Hi" }))])));
    assert!(!only.is_empty());
}

#[test]
fn markdown_writes_an_image_caption_as_the_link_title() {
    let id = aid(1).to_string();
    let page = from_json(&doc_of(json!([
        image_with(1, &json!({ "alt": "A cat", "caption": "Our cat, asleep" })),
        image_with(2, &json!({ "alt": "", "caption": "No alt" })),
        // Quotes and backslashes are escaped; every kind of line break is one space.
        image_with(
            3,
            &json!({ "alt": "x", "caption": "She said \"hi\" at C:\\temp\\" })
        ),
        image_with(
            4,
            &json!({ "alt": "x", "caption": "one\ntwo\r\nthree\rfour\n\nfive" })
        ),
        // No caption (absent, empty, damaged): the line is as before; layout is not written.
        image_with(5, &json!({ "alt": "plain" })),
        image_with(6, &json!({ "alt": "plain", "caption": "" })),
        image_with(7, &json!({ "alt": "plain", "caption": 3 })),
        image_with(
            8,
            &json!({ "alt": "plain", "widthPercent": 40, "align": "left", "width": 200 })
        ),
    ])));
    assert_eq!(
        page.to_markdown(&no_titles),
        format!(
            "![A cat](attachment://{id} \"Our cat, asleep\")\n\n\
             ![](attachment://{id} \"No alt\")\n\n\
             ![x](attachment://{id} \"She said \\\"hi\\\" at C:\\\\temp\\\\\")\n\n\
             ![x](attachment://{id} \"one two three four  five\")\n\n\
             ![plain](attachment://{id})\n\n\
             ![plain](attachment://{id})\n\n\
             ![plain](attachment://{id})\n\n\
             ![plain](attachment://{id})\n"
        )
    );
}

#[test]
fn check_invariants_rejects_a_malformed_image_width_percent_align_or_caption() {
    let page = from_json(&doc_of(json!([
        // Sound: the bounds, a whole number written as a float, every alignment, any string.
        image_with(
            1,
            &json!({ "widthPercent": 10, "align": "left", "caption": "" })
        ),
        image_with(
            2,
            &json!({ "widthPercent": 100, "align": "center", "caption": "x" })
        ),
        image_with(3, &json!({ "widthPercent": 50.0, "align": "right" })),
        image_with(4, &json!({ "widthPercent": 60 })),
        // Damaged widthPercent: out of range, fractional, not a number.
        image_with(5, &json!({ "widthPercent": 9 })),
        image_with(6, &json!({ "widthPercent": 101 })),
        image_with(7, &json!({ "widthPercent": "x" })),
        image_with(8, &json!({ "widthPercent": 1.5 })),
        image_with(9, &json!({ "widthPercent": 0 })),
        image_with(10, &json!({ "widthPercent": -50 })),
        image_with(11, &json!({ "widthPercent": 50.5 })),
        image_with(12, &json!({ "widthPercent": true })),
        // Damaged align and caption.
        image_with(13, &json!({ "align": "middle" })),
        image_with(14, &json!({ "align": "Left" })),
        image_with(15, &json!({ "align": 1 })),
        image_with(16, &json!({ "caption": 5 })),
        image_with(17, &json!({ "caption": false })),
        // All three at once.
        image_with(
            18,
            &json!({ "widthPercent": 5, "align": "x", "caption": [] })
        ),
    ])));
    let rejected = |name: &str| format!("image with a malformed {name}");
    assert_eq!(
        page.check_invariants(),
        vec![
            format!("node /4: {}", rejected("widthPercent")),
            format!("node /5: {}", rejected("widthPercent")),
            format!("node /6: {}", rejected("widthPercent")),
            format!("node /7: {}", rejected("widthPercent")),
            format!("node /8: {}", rejected("widthPercent")),
            format!("node /9: {}", rejected("widthPercent")),
            format!("node /10: {}", rejected("widthPercent")),
            format!("node /11: {}", rejected("widthPercent")),
            format!("node /12: {}", rejected("align")),
            format!("node /13: {}", rejected("align")),
            format!("node /14: {}", rejected("align")),
            format!("node /15: {}", rejected("caption")),
            format!("node /16: {}", rejected("caption")),
            format!("node /17: {}", rejected("widthPercent")),
            format!("node /17: {}", rejected("align")),
            format!("node /17: {}", rejected("caption")),
        ]
    );
    // The messages hold positions and names, never the damaged values.
    assert!(
        page.check_invariants()
            .iter()
            .all(|p| !p.contains("middle"))
    );
}

// ----- photo gallery (ADR-0016 amendment of 2026-10-09, ADR-0030) ----------------------------

/// A `gallery` with the given attributes besides its id.
fn gallery_with(id: u8, attrs: &Value) -> Value {
    let mut all = json!({ "id": s(bid(id)) });
    if let (Some(all), Some(extra)) = (all.as_object_mut(), attrs.as_object()) {
        all.extend(extra.clone());
    }
    json!({ "type": "gallery", "attrs": all })
}

fn item(n: u8, caption: Option<&str>) -> Value {
    match caption {
        Some(caption) => json!({ "attachmentId": aid(n).to_string(), "caption": caption }),
        None => json!({ "attachmentId": aid(n).to_string() }),
    }
}

#[test]
fn a_gallery_reads_as_its_captions_and_references_each_of_its_attachments() {
    let page = from_json(&doc_of(json!([
        gallery_with(
            1,
            &json!({ "columns": 4, "items": [
                item(1, Some("Sunrise")), item(2, None), item(3, Some("")), item(1, Some("Again"))
            ] })
        ),
        // No items, items damaged: no text, no reference.
        gallery_with(2, &json!({})),
        gallery_with(3, &json!({ "items": "x" })),
        gallery_with(
            4,
            &json!({ "items": [ 5, { "caption": "no picture" }, { "attachmentId": "nope" },
                                { "attachmentId": aid(4).to_string(), "caption": 7 } ] })
        ),
    ])));
    assert_eq!(
        image_texts(&page),
        vec![
            vec![Inline::Text("Sunrise Again".into())],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ]
    );
    // Document order; a repeated picture is listed twice here and is one row in the index; a
    // damaged entry is left out, and so is a gallery that has no list.
    assert_eq!(
        page.attachment_refs(),
        vec![
            (bid(1), aid(1)),
            (bid(1), aid(2)),
            (bid(1), aid(3)),
            (bid(1), aid(1)),
            (bid(4), aid(4)),
        ]
    );
    // A gallery is content: the page is not empty because of it.
    assert!(!from_json(&doc_of(json!([gallery_with(1, &json!({ "items": [] }))]))).is_empty());
}

#[test]
fn markdown_writes_a_gallery_as_one_image_line_per_item() {
    let a = aid(1).to_string();
    let b = aid(2).to_string();
    let page = from_json(&doc_of(json!([
        gallery_with(
            1,
            &json!({ "columns": 2, "items": [
                item(1, Some("She said \"hi\" at C:\\temp\\")),
                item(2, None),
                item(1, Some("one\ntwo")),
                item(2, Some("")),
                { "caption": "no picture" },
                7
            ] })
        ),
        gallery_with(2, &json!({ "items": [] })),
        gallery_with(3, &json!({})),
    ])));
    assert_eq!(
        page.to_markdown(&no_titles),
        format!(
            "![](attachment://{a} \"She said \\\"hi\\\" at C:\\\\temp\\\\\")\n\n\
             ![](attachment://{b})\n\n\
             ![](attachment://{a} \"one two\")\n\n\
             ![](attachment://{b})\n"
        )
    );
}

#[test]
fn check_invariants_rejects_a_malformed_gallery() {
    let page = from_json(&doc_of(json!([
        // Sound: any columns from 2 to 4 (a whole float too), no items at all, an empty list,
        // captions of any text, the same picture twice.
        gallery_with(
            1,
            &json!({ "columns": 2, "items": [item(1, Some("x")), item(1, None)] })
        ),
        gallery_with(2, &json!({ "columns": 4.0, "items": [] })),
        gallery_with(3, &json!({})),
        // Damaged columns.
        gallery_with(4, &json!({ "columns": 1 })),
        gallery_with(5, &json!({ "columns": 5 })),
        gallery_with(6, &json!({ "columns": 2.5 })),
        gallery_with(7, &json!({ "columns": "3" })),
        // Damaged items: not a list, then entries that are not maps, without a valid attachment
        // or with a caption that is not a string.
        gallery_with(8, &json!({ "items": "secret" })),
        gallery_with(
            9,
            &json!({ "items": { "attachmentId": aid(1).to_string() } })
        ),
        gallery_with(
            10,
            &json!({ "items": [item(1, None), "secret", {}, { "attachmentId": "secret" },
                               { "attachmentId": aid(2).to_string(), "caption": 3 }] })
        ),
    ])));
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /3: gallery with a malformed columns".to_owned(),
            "node /4: gallery with a malformed columns".to_owned(),
            "node /5: gallery with a malformed columns".to_owned(),
            "node /6: gallery with a malformed columns".to_owned(),
            "node /7: gallery with a malformed items".to_owned(),
            "node /8: gallery with a malformed items".to_owned(),
            "node /9: gallery item 1 is malformed".to_owned(),
            "node /9: gallery item 2 is malformed".to_owned(),
            "node /9: gallery item 3 is malformed".to_owned(),
            "node /9: gallery item 4 is malformed".to_owned(),
        ]
    );
    // The messages hold positions and names, never the damaged values.
    assert!(
        page.check_invariants()
            .iter()
            .all(|p| !p.contains("secret"))
    );
}

#[test]
fn the_gallery_is_a_schema_node_with_an_id() {
    assert!(schema::NODE_NAMES.contains(&"gallery"));
    assert!(schema::ID_BEARING.contains(&"gallery"));
    assert!(!schema::TEXT_NODES.contains(&"gallery"));
}

// ----- the preview view of a file block (ADR-0016 amendment of 2026-10-09, ADR-0031) ----------

fn file_with(id: u8, attrs: &Value) -> Value {
    let mut node = json!({ "type": "file", "attrs": { "id": s(bid(id)), "attachmentId": aid(id).to_string() } });
    if let (Some(target), Some(extra)) = (node["attrs"].as_object_mut(), attrs.as_object()) {
        target.extend(extra.clone());
    }
    node
}

#[test]
fn check_invariants_checks_the_view_of_a_file() {
    let page = from_json(&doc_of(json!([
        // Sound: no view (the card) and the preview.
        file_with(1, &json!({})),
        file_with(2, &json!({ "view": "preview" })),
        // Damaged: another word, another type, a casing, an empty string.
        file_with(3, &json!({ "view": "secret" })),
        file_with(4, &json!({ "view": 1 })),
        file_with(5, &json!({ "view": "Preview" })),
        file_with(6, &json!({ "view": "" })),
    ])));
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /2: file with a malformed view".to_owned(),
            "node /3: file with a malformed view".to_owned(),
            "node /4: file with a malformed view".to_owned(),
            "node /5: file with a malformed view".to_owned(),
        ]
    );
}

#[test]
fn the_view_of_a_file_is_layout_only() {
    // The preview changes nothing the core derives: no text, the same reference, the same Markdown
    // (the file link), and a file with a view keeps its attachment for retention.
    let card = from_json(&doc_of(json!([file_with(1, &json!({}))])));
    let preview = from_json(&doc_of(json!([file_with(
        1,
        &json!({ "view": "preview" })
    )])));
    assert_eq!(preview.attachment_refs(), card.attachment_refs());
    assert_eq!(preview.attachment_refs(), vec![(bid(1), aid(1))]);
    assert!(
        preview
            .blocks()
            .iter()
            .all(|block| block.inlines.is_empty())
    );
    assert_eq!(
        preview.to_markdown(&no_titles),
        card.to_markdown(&no_titles)
    );
    assert!(preview.check_invariants().is_empty());
    assert!(!preview.is_empty());
}

// ----- tables (additive nodes, ADR-0016 amendment of 2026-10-05) -----------------------------

fn table(id: u8, rows: Vec<Value>) -> Value {
    json!({ "type": "table", "attrs": { "id": s(bid(id)) }, "content": rows })
}

fn row(id: u8, cells: Vec<Value>) -> Value {
    json!({ "type": "tableRow", "attrs": { "id": s(bid(id)) }, "content": cells })
}

/// A `tableCell` or `tableHeader` as the editor writes it (default span attributes).
fn cell(kind: &str, id: u8, blocks: Vec<Value>) -> Value {
    json!({ "type": kind, "attrs": { "id": s(bid(id)), "colspan": 1, "rowspan": 1 }, "content": blocks })
}

/// A `tableCell` holding one paragraph of `text`.
fn one(id: u8, text: &str) -> Value {
    cell("tableCell", id, vec![para(bid(id + 100), text)])
}

#[test]
fn tables_are_ordinary_nodes_whose_text_lives_in_their_cell_paragraphs() {
    let reference = json!({ "type": "pageRef", "attrs": { "id": s(bid(50)), "pageId": page_id(1).to_string() } });
    let json = doc_of(json!([
        table(
            1,
            vec![
                row(
                    2,
                    vec![
                        cell("tableHeader", 3, vec![para(bid(103), "Name")]),
                        cell("tableHeader", 4, vec![para(bid(104), "Link")]),
                    ]
                ),
                row(
                    5,
                    vec![
                        one(6, "plain"),
                        cell(
                            "tableCell",
                            7,
                            vec![
                                json!({ "type": "paragraph", "attrs": { "id": s(bid(107)) }, "content": [
                                    { "type": "text", "text": "see " }, reference
                                ]}),
                                para(bid(108), "second"),
                            ]
                        ),
                    ]
                ),
            ]
        ),
        para(bid(9), "after"),
    ]));
    let page = from_json(&json);
    // The names are in the schema module, and none is unknown to the core.
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    for name in schema::TABLE_NODES {
        assert!(schema::NODE_NAMES.contains(name), "{name}");
        assert!(schema::ID_BEARING.contains(name), "{name}");
    }
    // Every node with an id is a block, in document order; only the paragraphs have text.
    let blocks = page.blocks();
    let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "table",
            "tableRow",
            "tableHeader",
            "paragraph",
            "tableHeader",
            "paragraph",
            "tableRow",
            "tableCell",
            "paragraph",
            "tableCell",
            "paragraph",
            "pageRef",
            "paragraph",
            "paragraph",
        ]
    );
    assert!(
        blocks
            .iter()
            .filter(|b| schema::TABLE_NODES.contains(&b.kind.as_str()))
            .all(|b| b.inlines.is_empty())
    );
    assert_eq!(
        texts(&page),
        [
            "Name".to_owned(),
            "Link".to_owned(),
            "plain".to_owned(),
            format!("see [[{}]]", page_id(1)),
            "second".to_owned(),
            "after".to_owned(),
        ]
    );
    // A reference in a cell belongs to its paragraph.
    assert_eq!(
        page.links(),
        vec![PageLink {
            from_block: bid(107),
            to_page: page_id(1),
            to_block: None
        }]
    );
    assert_sound(&page);
    assert!(!page.is_empty());
    // Portable JSON keeps the table (including the default span attributes) as it is.
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).to_portable(), json);
    // Editing around a table keeps it.
    page.insert_block(Some(bid(9)), BlockKind::Paragraph, "appended", bid(60))
        .unwrap();
    page.set_block_text(bid(9), "changed").unwrap();
    assert_eq!(page.to_portable()["content"][0], json["content"][0]);
    assert_sound(&page);
}

#[test]
fn markdown_renders_a_table_as_gfm_with_one_line_per_row() {
    let bold_break = json!({ "type": "paragraph", "attrs": { "id": s(bid(109)) }, "content": [
        text_with(json!([{ "type": "bold" }]), "bold"),
        { "type": "hardBreak" },
        { "type": "text", "text": "after" }
    ]});
    let list = json!({ "type": "bulletList", "attrs": { "id": s(bid(12)) }, "content": [
        { "type": "listItem", "attrs": { "id": s(bid(13)) }, "content": [para(bid(113), "x")] },
        { "type": "listItem", "attrs": { "id": s(bid(14)) }, "content": [para(bid(114), "y")] }
    ]});
    let content = json!([
        para(bid(20), "before"),
        table(
            1,
            vec![
                row(
                    2,
                    vec![
                        cell("tableHeader", 3, vec![para(bid(103), "Name")]),
                        cell("tableHeader", 4, vec![para(bid(104), "Note")]),
                    ]
                ),
                row(
                    5,
                    vec![
                        one(6, "a | b"),
                        cell(
                            "tableCell",
                            7,
                            vec![para(bid(107), "one"), para(bid(108), "two")]
                        ),
                    ]
                ),
                row(
                    9,
                    vec![
                        cell("tableCell", 10, vec![bold_break]),
                        cell("tableCell", 11, vec![list]),
                    ]
                ),
            ]
        ),
        para(bid(21), "after"),
    ]);
    assert_eq!(
        md(content),
        "\
before

| Name | Note |
| --- | --- |
| a \\| b | one<br><br>two |
| **bold**  <br>after | - x<br>- y |

after
"
    );
}

#[test]
fn markdown_of_tables_is_literal_in_the_odd_cases() {
    // No rows: nothing, like an empty paragraph.
    let empty = json!({ "type": "table", "attrs": { "id": s(bid(1)) } });
    assert_eq!(md(json!([empty])), "\n");
    assert_eq!(
        md(json!([para(bid(2), "a"), empty, para(bid(3), "b")])),
        "a\n\n\n\nb\n"
    );

    // The separator has as many columns as the first row; other rows keep their own cells.
    let ragged = table(
        1,
        vec![
            row(2, vec![one(3, "a"), one(4, "b")]),
            row(5, vec![one(6, "c"), one(7, "d"), one(8, "e")]),
            row(9, vec![one(10, "f")]),
        ],
    );
    assert_eq!(
        md(json!([ragged])),
        "| a | b |\n| --- | --- |\n| c | d | e |\n| f |\n"
    );

    // Empty cells and rows without cells.
    let blank = table(
        1,
        vec![
            row(2, vec![one(3, ""), one(4, "")]),
            row(5, vec![]),
            row(6, vec![cell("tableCell", 7, vec![])]),
        ],
    );
    assert_eq!(md(json!([blank])), "|  |  |\n| --- | --- |\n|  |\n|  |\n");
    let no_cells = table(1, vec![row(2, vec![]), row(3, vec![one(4, "x")])]);
    assert_eq!(md(json!([no_cells])), "|  |\n|  |\n| x |\n");

    // Spans and widths are ignored; cells and rows are whatever the children are called.
    let spanned = json!({ "type": "tableCell", "attrs": { "id": s(bid(3)), "colspan": 2, "rowspan": 3, "colwidth": [120, 80] },
        "content": [para(bid(103), "wide")] });
    let odd = table(
        1,
        vec![
            row(
                2,
                vec![
                    spanned,
                    json!({ "type": "gridCell", "content": [para(bid(104), "odd")] }),
                ],
            ),
            json!({ "type": "line", "content": [json!({ "type": "tableCell", "content": [para(bid(105), "other")] })] }),
        ],
    );
    assert_eq!(
        md(json!([odd])),
        "| wide | odd |\n| --- | --- |\n| other |\n"
    );

    // Nothing but `|` and newlines is touched: marks, links, backslashes and code stay.
    let busy = table(
        1,
        vec![row(
            2,
            vec![cell(
                "tableCell",
                3,
                vec![
                    json!({ "type": "paragraph", "attrs": { "id": s(bid(103)) }, "content": [
                        text_with(json!([{ "type": "link", "attrs": { "href": "https://a.example/?q=a|b" } }]), "x|y"),
                        { "type": "text", "text": " back\\slash" }
                    ]}),
                    json!({ "type": "codeBlock", "attrs": { "id": s(bid(104)), "language": "rs" }, "content": [{ "type": "text", "text": "a || b\nc" }] }),
                ],
            )],
        )],
    );
    assert_eq!(
        md(json!([busy])),
        "| [x\\|y](https://a.example/?q=a\\|b) back\\slash<br><br>```rs<br>a \\|\\| b<br>c<br>``` |\n| --- |\n"
    );

    // A table in a cell is one more block of that cell; a table in a quote or an item
    // takes the container's prefix on each line.
    let inner = table(20, vec![row(21, vec![one(22, "i")])]);
    let nested = table(
        1,
        vec![row(
            2,
            vec![cell(
                "tableCell",
                3,
                vec![para(bid(103), "o"), inner.clone()],
            )],
        )],
    );
    assert_eq!(
        md(json!([nested])),
        "| o<br><br>\\| i \\|<br>\\| --- \\| |\n| --- |\n"
    );
    let quoted =
        json!({ "type": "blockquote", "attrs": { "id": s(bid(30)) }, "content": [inner.clone()] });
    assert_eq!(md(json!([quoted])), "> | i |\n> | --- |\n");
    let listed = json!({ "type": "bulletList", "attrs": { "id": s(bid(31)) }, "content": [
        { "type": "listItem", "attrs": { "id": s(bid(32)) }, "content": [para(bid(132), "item"), inner] }
    ]});
    assert_eq!(md(json!([listed])), "- item\n  | i |\n  | --- |\n");
}

#[test]
fn markdown_names_files_inside_table_cells() {
    let id = aid(1).to_string();
    let page = from_json(&doc_of(json!([table(
        1,
        vec![row(2, vec![cell("tableCell", 3, vec![file(bid(4), &id)])]),]
    )])));
    let mut asked = Vec::new();
    let markdown = page.to_markdown_with(&no_titles, &mut |attachment| {
        asked.push(attachment);
        Some("report.pdf".to_owned())
    });
    assert_eq!(
        markdown,
        format!("| [report.pdf](attachment://{id}) |\n| --- |\n")
    );
    assert_eq!(asked, vec![aid(1)]);
    assert_eq!(page.attachment_refs(), vec![(bid(4), aid(1))]);
}

// ----- math blocks (additive node, ADR-0016 amendment of 2026-10-06) --------------------------

/// A `mathBlock` holding `latex` as one plain text run (an empty one has no children).
fn math(id: u8, latex: &str) -> Value {
    if latex.is_empty() {
        json!({ "type": "mathBlock", "attrs": { "id": s(bid(id)) } })
    } else {
        json!({ "type": "mathBlock", "attrs": { "id": s(bid(id)) }, "content": [{ "type": "text", "text": latex }] })
    }
}

#[test]
fn markdown_renders_a_math_block_as_fenced_dollars() {
    // The same inputs and outputs as the tests of core-client (`src/markdown.test.ts`): both
    // runtimes must write the same bytes.
    assert_eq!(md(json!([math(1, "E = mc^2")])), "$$\nE = mc^2\n$$\n");
    // Marks inside are ignored (the editor allows none, a hand-built document may hold them).
    assert_eq!(
        md(
            json!([{ "type": "mathBlock", "attrs": { "id": s(bid(1)) }, "content": [
                text_with(json!([{ "type": "bold" }]), "x")
            ]}])
        ),
        "$$\nx\n$$\n"
    );
    // Empty: an empty line between the fences, like an empty code block.
    assert_eq!(md(json!([math(1, "")])), "$$\n\n$$\n");
    // Several lines keep their newlines; nothing is escaped, `$` and backslashes included.
    assert_eq!(
        md(json!([math(
            1,
            "\\begin{aligned}\na &= $b$ \\\\\nc &= d\n\\end{aligned}"
        )])),
        "$$\n\\begin{aligned}\na &= $b$ \\\\\nc &= d\n\\end{aligned}\n$$\n"
    );
    // Between other blocks, and inside a container, like any block.
    assert_eq!(
        md(json!([
            para(bid(10), "before"),
            math(1, "a^2 + b^2 = c^2"),
            para(bid(11), "after")
        ])),
        "before\n\n$$\na^2 + b^2 = c^2\n$$\n\nafter\n"
    );
    let quoted = json!({ "type": "blockquote", "attrs": { "id": s(bid(20)) }, "content": [math(21, "x = 1")] });
    assert_eq!(md(json!([quoted])), "> $$\n> x = 1\n> $$\n");
}

#[test]
fn math_blocks_are_text_blocks_whose_latex_is_indexed_like_code() {
    let page = from_json(&doc_of(json!([
        para(bid(1), "Intro"),
        math(2, "\\int_0^1 x^2\\,dx = \\frac{1}{3}"),
        math(3, ""),
        math(4, "a\nb"),
    ])));
    // The name is in the schema module, id-bearing and a text node; the core knows it.
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert!(schema::NODE_NAMES.contains(&schema::MATH_BLOCK));
    assert!(schema::ID_BEARING.contains(&schema::MATH_BLOCK));
    assert!(schema::TEXT_NODES.contains(&schema::MATH_BLOCK));
    assert_sound(&page);

    // Every math block is a block with an id, in document order; its LaTeX is its text.
    let blocks = page.blocks();
    let kinds: Vec<(&str, BlockId)> = blocks.iter().map(|b| (b.kind.as_str(), b.id)).collect();
    assert_eq!(
        kinds,
        [
            ("paragraph", bid(1)),
            ("mathBlock", bid(2)),
            ("mathBlock", bid(3)),
            ("mathBlock", bid(4)),
        ]
    );
    assert_eq!(
        blocks[1].inlines,
        vec![Inline::Text("\\int_0^1 x^2\\,dx = \\frac{1}{3}".into())]
    );
    assert!(
        blocks[2].inlines.is_empty(),
        "an empty equation has no text"
    );
    assert_eq!(
        texts(&page),
        ["Intro", "\\int_0^1 x^2\\,dx = \\frac{1}{3}", "a\nb"]
    );

    // The semantic edit that targets text blocks works on it and keeps node, id and kind.
    page.set_block_text(bid(3), "x^2").unwrap();
    page.set_block_text(bid(2), "E = mc^2").unwrap();
    assert_sound(&page);
    let portable = page.to_portable();
    assert_eq!(portable["content"][1], math(2, "E = mc^2"));
    assert_eq!(portable["content"][2], math(3, "x^2"));
    page.set_block_text(bid(2), "").unwrap();
    assert_eq!(page.to_portable()["content"][1], math(2, ""));
    assert_eq!(
        page.to_markdown(&no_titles),
        "Intro\n\n$$\n\n$$\n\n$$\nx^2\n$$\n\n$$\na\nb\n$$\n"
    );
}

// ----- inline equations (additive inline node, ADR-0016 amendment of 2026-10-07) ----------------

/// A `mathInline` holding `latex`.
fn math_inline(id: u8, latex: &str) -> Value {
    json!({ "type": "mathInline", "attrs": { "id": s(bid(id)), "latex": latex } })
}

#[test]
fn an_inline_equation_reads_as_its_latex_inside_the_text_run_around_it() {
    let json = doc_of(json!([
        paragraph_of(
            1,
            vec![
                text_node("Energy "),
                math_inline(2, "E=mc^2"),
                text_node(" is "),
                math_inline(3, "\\frac{a}{b}"),
                text_node(" of mass"),
            ]
        ),
        paragraph_of(4, vec![math_inline(5, "x^2")]),
        paragraph_of(6, vec![text_node("a"), math_inline(7, ""), text_node("b")]),
    ]));
    let page = from_json(&json);
    assert_sound(&page);
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert!(schema::NODE_NAMES.contains(&"mathInline"));
    assert!(schema::ID_BEARING.contains(&"mathInline"));

    // The paragraph's inline content is one text: the equation does not split the run, so the text
    // index and the backlinks' text hold the LaTeX as written (a search for `E=mc^2` finds it).
    let blocks = page.blocks();
    assert_eq!(
        blocks.iter().map(|b| b.kind.as_str()).collect::<Vec<_>>(),
        [
            "paragraph",
            "mathInline",
            "mathInline",
            "paragraph",
            "mathInline",
            "paragraph",
            "mathInline"
        ]
    );
    assert_eq!(
        blocks[0].inlines,
        vec![Inline::Text("Energy E=mc^2 is \\frac{a}{b} of mass".into())]
    );
    assert_eq!(
        blocks[3].inlines,
        vec![Inline::Text("x^2".into())],
        "a paragraph of nothing but an equation is that text"
    );
    assert_eq!(
        blocks[5].inlines,
        vec![Inline::Text("ab".into())],
        "an empty equation reads as nothing"
    );
    // The equation is a node with an id of its own and no inline content, like a `pageRef`.
    for equation in blocks.iter().filter(|b| b.kind == "mathInline") {
        assert!(equation.inlines.is_empty());
    }
    assert_eq!(
        texts(&page),
        [
            "Energy E=mc^2 is \\frac{a}{b} of mass".to_owned(),
            "x^2".to_owned(),
            "ab".to_owned()
        ]
    );
    // An equation is no link.
    assert!(page.links().is_empty());
    assert_eq!(
        page.block_ids(),
        blocks.iter().map(|b| b.id).collect::<Vec<_>>()
    );
}

#[test]
fn an_inline_equation_next_to_a_break_or_a_reference_splits_the_text_only_there() {
    let reference = json!({ "type": "pageRef", "attrs": { "id": s(bid(9)), "pageId": page_id(1).to_string() } });
    let json = doc_of(json!([paragraph_of(
        1,
        vec![
            text_node("a "),
            math_inline(2, "x"),
            json!({ "type": "hardBreak" }),
            math_inline(3, "y"),
            text_node(" b "),
            reference,
            math_inline(4, "z"),
        ]
    )]));
    let page = from_json(&json);
    assert_sound(&page);
    assert_eq!(
        page.blocks()[0].inlines,
        vec![
            Inline::Text("a x".into()),
            Inline::Break,
            Inline::Text("y b ".into()),
            Inline::PageRef {
                page_id: page_id(1),
                block_id: None
            },
            Inline::Text("z".into()),
        ]
    );
}

#[test]
fn an_inline_equation_counts_as_content_even_while_empty() {
    for latex in ["x", ""] {
        let json = doc_of(json!([paragraph_of(1, vec![math_inline(2, latex)])]));
        assert!(!from_json(&json).is_empty(), "{latex:?}");
    }
}

#[test]
fn an_inline_equation_survives_the_portable_round_trip_and_keeps_an_empty_source() {
    let json = doc_of(json!([paragraph_of(
        1,
        vec![
            text_node("a "),
            math_inline(2, "E=mc^2"),
            text_node(" "),
            math_inline(3, ""),
            text_node(" "),
            math_inline(4, "a$b \\ c"),
        ]
    )]));
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
    // An empty source is stored (the attribute is required): it is not a null that vanishes.
    assert_eq!(
        page.to_portable()["content"][0]["content"][3]["attrs"]["latex"],
        ""
    );
    let again = PageDoc::from_portable(Doc::new(DocKind::Page, 4), &page.to_portable()).unwrap();
    assert_eq!(again.to_portable(), json);
    assert_sound(&again);
}

#[test]
fn markdown_renders_an_inline_equation_as_dollars_around_its_latex() {
    // The same inputs and outputs as the tests of core-client (`src/markdown.test.ts`): both
    // runtimes must write the same bytes.
    let md = |content: Vec<Value>| from_json(&doc_of(json!(content))).to_markdown(&no_titles);
    assert_eq!(
        md(vec![paragraph_of(
            1,
            vec![
                text_node("Energy "),
                math_inline(2, "E=mc^2"),
                text_node(" is "),
                math_inline(3, "\\frac{a}{b}"),
                text_node("."),
            ]
        )]),
        "Energy $E=mc^2$ is $\\frac{a}{b}$.\n"
    );
    // The LaTeX is written as stored: nothing is escaped or trimmed, `$` and spaces included.
    assert_eq!(
        md(vec![paragraph_of(
            1,
            vec![
                math_inline(2, "a$b"),
                text_node(" "),
                math_inline(3, " x "),
                text_node(" "),
                math_inline(4, "a\\b")
            ]
        )]),
        "$a$b$ $ x $ $a\\b$\n"
    );
    // Nothing (not `$$`, the block's fence) for an equation with no source, and for one whose
    // `latex` is no string.
    assert_eq!(
        md(vec![paragraph_of(
            1,
            vec![
                text_node("a "),
                math_inline(2, ""),
                text_node(" b "),
                json!({ "type": "mathInline", "attrs": { "id": s(bid(3)) } }),
                text_node(" c "),
                json!({ "type": "mathInline", "attrs": { "id": s(bid(4)), "latex": 5 } }),
                text_node(" d"),
            ]
        )]),
        "a  b  c  d\n"
    );
    // The text keeps its marks around it; the equation does not carry any.
    assert_eq!(
        md(vec![paragraph_of(
            1,
            vec![
                json!({ "type": "text", "text": "bold ", "marks": [{ "type": "bold" }] }),
                math_inline(2, "x"),
                json!({ "type": "text", "text": " bold", "marks": [{ "type": "bold" }] }),
            ]
        )]),
        "**bold **$x$** bold**\n"
    );
    // An equation among blocks (not in a paragraph) joins the inline nodes around it as one block.
    assert_eq!(
        md(vec![
            para(bid(1), "x"),
            math_inline(2, "y"),
            para(bid(3), "z")
        ]),
        "x\n\n$y$\n\nz\n"
    );
}

#[test]
fn markdown_renders_an_inline_equation_inside_blocks() {
    let md = |content: Vec<Value>| from_json(&doc_of(json!(content))).to_markdown(&no_titles);
    // In a heading, a list item, a quote, a table cell (where the cell's own `|` escape applies)
    // and a toggle's summary.
    let heading = json!({ "type": "heading", "attrs": { "id": s(bid(1)), "level": 2 }, "content": [
        text_node("Proof of "), math_inline(2, "a^2+b^2=c^2")
    ]});
    let list = json!({ "type": "bulletList", "attrs": { "id": s(bid(3)) }, "content": [
        { "type": "listItem", "attrs": { "id": s(bid(4)) }, "content": [
            paragraph_of(5, vec![text_node("Let "), math_inline(6, "n>0")])
        ]}
    ]});
    let quote = json!({ "type": "blockquote", "attrs": { "id": s(bid(7)) }, "content": [
        paragraph_of(8, vec![math_inline(9, "x")])
    ]});
    let table = json!({ "type": "table", "attrs": { "id": s(bid(20)) }, "content": [
        { "type": "tableRow", "attrs": { "id": s(bid(21)) }, "content": [
            { "type": "tableCell", "attrs": { "id": s(bid(22)) }, "content": [
                paragraph_of(23, vec![math_inline(24, "|x|")])
            ]}
        ]}
    ]});
    let details = json!({ "type": "details", "attrs": { "id": s(bid(10)), "open": false }, "content": [
        { "type": "detailsSummary", "attrs": { "id": s(bid(11)) }, "content": [
            text_node("About "), math_inline(12, "e")
        ]},
        { "type": "detailsContent", "attrs": { "id": s(bid(13)) }, "content": [para(bid(14), "body")] }
    ]});
    assert_eq!(
        md(vec![heading, list, quote, table, details]),
        "## Proof of $a^2+b^2=c^2$\n\n- Let $n>0$\n\n> $x$\n\n| $\\|x\\|$ |\n| --- |\n\n\
         <details>\n<summary>About $e$</summary>\n\nbody\n\n</details>\n"
    );
}

#[test]
fn check_invariants_reports_an_inline_equation_without_a_string_latex_or_an_id() {
    let json = doc_of(json!([paragraph_of(
        1,
        vec![
            json!({ "type": "mathInline", "attrs": { "id": s(bid(2)) } }),
            json!({ "type": "mathInline", "attrs": { "id": s(bid(3)), "latex": 5 } }),
            json!({ "type": "mathInline", "attrs": { "latex": "x" } }),
            math_inline(4, ""),
            math_inline(5, "ok"),
        ]
    )]));
    let page = from_json(&json);
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /0/0: mathInline without a string latex".to_owned(),
            "node /0/1: mathInline without a string latex".to_owned(),
            "node /0/2: mathInline without an id".to_owned(),
        ]
    );
    // Reading is lenient: the two that are not strings read as nothing, nothing is rewritten.
    assert_eq!(page.blocks()[0].inlines, vec![Inline::Text("xok".into())]);
}

// ----- columns (additive nodes, ADR-0016 amendment of 2026-10-06) -----------------------------

fn column_list(id: u8, columns: Vec<Value>) -> Value {
    json!({ "type": "columnList", "attrs": { "id": s(bid(id)) }, "content": columns })
}

fn column(id: u8, blocks: Vec<Value>) -> Value {
    json!({ "type": "column", "attrs": { "id": s(bid(id)) }, "content": blocks })
}

#[test]
fn columns_are_ordinary_nodes_whose_text_lives_in_their_blocks() {
    let reference = json!({ "type": "pageRef", "attrs": { "id": s(bid(50)), "pageId": page_id(1).to_string() } });
    let json = doc_of(json!([
        para(bid(9), "before"),
        column_list(
            1,
            vec![
                column(2, vec![para(bid(3), "left"), para(bid(4), "left two")]),
                column(
                    5,
                    vec![
                        json!({ "type": "paragraph", "attrs": { "id": s(bid(6)) }, "content": [
                            { "type": "text", "text": "see " }, reference
                        ]})
                    ]
                ),
            ]
        ),
        para(bid(10), "after"),
    ]));
    let page = from_json(&json);
    // The names are in the schema module, and none is unknown to the core.
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    for name in schema::COLUMN_NODES {
        assert!(schema::NODE_NAMES.contains(name), "{name}");
        assert!(schema::ID_BEARING.contains(name), "{name}");
    }
    // Every node with an id is a block, in document order; only the paragraphs have text.
    let blocks = page.blocks();
    let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "paragraph",
            "columnList",
            "column",
            "paragraph",
            "paragraph",
            "column",
            "paragraph",
            "pageRef",
            "paragraph",
        ]
    );
    assert!(
        blocks
            .iter()
            .filter(|b| schema::COLUMN_NODES.contains(&b.kind.as_str()))
            .all(|b| b.inlines.is_empty())
    );
    assert_eq!(
        texts(&page),
        [
            "before".to_owned(),
            "left".to_owned(),
            "left two".to_owned(),
            format!("see [[{}]]", page_id(1)),
            "after".to_owned(),
        ]
    );
    // A reference in a column belongs to its paragraph.
    assert_eq!(
        page.links(),
        vec![PageLink {
            from_block: bid(6),
            to_page: page_id(1),
            to_block: None
        }]
    );
    assert_sound(&page);
    // Portable JSON keeps the layout as it is.
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).to_portable(), json);
    // Editing around and inside the columns keeps them.
    page.insert_block(Some(bid(10)), BlockKind::Paragraph, "appended", bid(60))
        .unwrap();
    page.set_block_text(bid(4), "changed").unwrap();
    assert_eq!(
        page.to_portable()["content"][1]["content"][0]["content"][1],
        para(bid(4), "changed")
    );
    assert_eq!(page.to_portable()["content"][1]["attrs"]["id"], s(bid(1)));
    assert_sound(&page);
}

#[test]
fn markdown_renders_a_column_list_one_column_after_another() {
    let quote = json!({ "type": "blockquote", "attrs": { "id": s(bid(30)) }, "content": [para(bid(31), "q")] });
    let list = json!({ "type": "bulletList", "attrs": { "id": s(bid(32)) }, "content": [
        { "type": "listItem", "attrs": { "id": s(bid(33)) }, "content": [para(bid(34), "x")] },
        { "type": "listItem", "attrs": { "id": s(bid(35)) }, "content": [para(bid(36), "y")] }
    ]});
    // The normal case: each column's blocks joined by blank lines, the columns joined by
    // blank lines, the layout among the other blocks of the page.
    assert_eq!(
        md(json!([
            para(bid(20), "before"),
            column_list(
                1,
                vec![
                    column(2, vec![para(bid(3), "one"), para(bid(4), "two")]),
                    column(5, vec![para(bid(6), "three")]),
                ]
            ),
            para(bid(21), "after"),
        ])),
        "before\n\none\n\ntwo\n\nthree\n\nafter\n"
    );
    // A list and a quote: each block renders as it does anywhere else.
    assert_eq!(
        md(json!([column_list(
            7,
            vec![
                column(8, vec![list]),
                column(9, vec![quote.clone(), para(bid(37), "tail")]),
            ]
        )])),
        "- x\n- y\n\n> q\n\ntail\n"
    );
    // The layout takes the prefix of the container it is in, as any block does.
    assert_eq!(
        md(json!([
            json!({ "type": "blockquote", "attrs": { "id": s(bid(40)) }, "content": [
                column_list(41, vec![column(42, vec![para(bid(43), "a")]), column(44, vec![para(bid(45), "b")])])
            ]})
        ])),
        "> a\n>\n> b\n"
    );
    // The number of columns and the nesting are not looked at.
    assert_eq!(
        md(json!([column_list(
            50,
            vec![
                column(
                    51,
                    vec![column_list(
                        52,
                        vec![
                            column(53, vec![para(bid(54), "i1")]),
                            column(55, vec![para(bid(56), "i2")]),
                        ]
                    )]
                ),
                column(57, vec![para(bid(58), "o")]),
            ]
        )])),
        "i1\n\ni2\n\no\n"
    );
}

#[test]
fn markdown_of_an_empty_column_takes_its_place_in_the_join() {
    // An empty paragraph in a column, and a column without blocks: both render as the empty
    // string, and the blank lines pile up where the column was (as for an empty top-level
    // paragraph).
    let layout = |middle: Value| {
        json!([
            para(bid(20), "x"),
            column_list(
                1,
                vec![
                    column(2, vec![para(bid(3), "a")]),
                    middle,
                    column(5, vec![para(bid(6), "b")]),
                ]
            ),
            para(bid(21), "y"),
        ])
    };
    let expected = "x\n\na\n\n\n\nb\n\ny\n";
    assert_eq!(md(layout(column(4, vec![para(bid(7), "")]))), expected);
    assert_eq!(md(layout(column(4, vec![]))), expected);
    // At the very end of the page the blank lines are trimmed.
    let last = column_list(
        1,
        vec![
            column(2, vec![para(bid(3), "a")]),
            column(4, vec![para(bid(5), "")]),
        ],
    );
    assert_eq!(md(json!([last])), "a\n");
    // A column list without columns is nothing, like an empty paragraph.
    let none = json!({ "type": "columnList", "attrs": { "id": s(bid(1)) } });
    assert_eq!(
        md(json!([para(bid(2), "a"), none, para(bid(3), "b")])),
        "a\n\n\n\nb\n"
    );
}

#[test]
fn markdown_names_files_inside_columns() {
    let id = aid(1).to_string();
    let page = from_json(&doc_of(json!([column_list(
        1,
        vec![
            column(2, vec![para(bid(3), "see")]),
            column(4, vec![file(bid(5), &id)]),
        ]
    )])));
    let mut asked = Vec::new();
    let markdown = page.to_markdown_with(&no_titles, &mut |attachment| {
        asked.push(attachment);
        Some("report.pdf".to_owned())
    });
    assert_eq!(
        markdown,
        format!("see\n\n[report.pdf](attachment://{id})\n")
    );
    assert_eq!(asked, vec![aid(1)]);
    assert_eq!(page.attachment_refs(), vec![(bid(5), aid(1))]);
}

// ----- page links: the card block (ADR-0016, amendment of 2026-10-06) ------------------------

fn page_link(id: u8, page: u8, block: Option<u8>) -> Value {
    let mut attrs = json!({ "id": s(bid(id)), "pageId": page_id(page).to_string() });
    if let Some(block) = block {
        attrs["blockId"] = json!(s(bid(block)));
    }
    json!({ "type": "pageLink", "attrs": attrs })
}

#[test]
fn a_page_link_block_counts_as_a_link_of_its_own() {
    let json = doc_of(json!([
        page_link(1, 1, None),
        { "type": "paragraph", "attrs": { "id": s(bid(2)) }, "content": [
            { "type": "pageRef", "attrs": { "id": s(bid(3)), "pageId": page_id(2).to_string() } }
        ]},
        // In a column and in a list item: the card is the block, whatever holds it.
        { "type": "columnList", "attrs": { "id": s(bid(4)) }, "content": [
            { "type": "column", "attrs": { "id": s(bid(5)) }, "content": [page_link(6, 3, Some(42))] },
            { "type": "column", "attrs": { "id": s(bid(7)) }, "content": [para(bid(8), "x")] }
        ]},
        // Without an id of its own it belongs to the nearest block that has one, like a chip.
        { "type": "callout", "attrs": { "id": s(bid(9)) }, "content": [
            { "type": "pageLink", "attrs": { "pageId": page_id(4).to_string() } }
        ]},
        // Left out: no valid page, and no block to belong to.
        { "type": "pageLink", "attrs": { "id": s(bid(10)), "pageId": "not-a-uuid" } },
        { "type": "pageLink", "attrs": { "pageId": page_id(5).to_string() } },
        // A malformed blockId reads as none.
        { "type": "pageLink", "attrs": { "id": s(bid(11)), "pageId": page_id(6).to_string(), "blockId": "nope" } }
    ]));
    let page = from_json(&json);
    let link = |from: u8, to: u8, to_block: Option<u8>| PageLink {
        from_block: bid(from),
        to_page: page_id(to),
        to_block: to_block.map(bid),
    };
    assert_eq!(
        page.links(),
        vec![
            link(1, 1, None),
            link(2, 2, None),
            link(6, 3, Some(42)),
            link(9, 4, None),
            link(11, 6, None),
        ]
    );
    // Cards are blocks with ids, listed with the others.
    assert!(page.block_ids().contains(&bid(1)));
    assert!(page.block_ids().contains(&bid(6)));
}

#[test]
fn a_page_link_reads_as_the_reference_the_text_index_resolves() {
    let page = from_json(&doc_of(json!([
        page_link(1, 1, None),
        page_link(2, 2, Some(7)),
        { "type": "pageLink", "attrs": { "id": s(bid(3)), "pageId": "not-a-uuid" } },
        para(bid(4), "after")
    ])));
    let blocks = page.blocks();
    assert_eq!(blocks[0].kind, "pageLink");
    assert_eq!(
        blocks[0].inlines,
        vec![Inline::PageRef {
            page_id: page_id(1),
            block_id: None
        }]
    );
    assert_eq!(
        blocks[1].inlines,
        vec![Inline::PageRef {
            page_id: page_id(2),
            block_id: Some(bid(7))
        }]
    );
    assert!(blocks[2].inlines.is_empty(), "no valid page, no text");
    // The card is no text block: `set_block_text` refuses it, like any other leaf.
    assert!(is_invalid(&page.set_block_text(bid(1), "x").unwrap_err()));
    assert!(schema::NODE_NAMES.contains(&"pageLink"));
    assert!(schema::ID_BEARING.contains(&"pageLink"));
    assert!(!schema::TEXT_NODES.contains(&"pageLink"));
}

#[test]
fn a_page_link_is_content_and_its_damage_is_reported() {
    let only_card = doc_of(json!([page_link(1, 1, None), para(bid(2), "")]));
    let page = from_json(&only_card);
    assert!(!page.is_empty(), "a card is content, like a chip");
    assert_sound(&page);

    let damaged = from_json(&doc_of(json!([
        { "type": "pageLink", "attrs": { "id": s(bid(1)), "pageId": "nope" } },
        { "type": "pageLink", "attrs": { "id": s(bid(2)), "pageId": page_id(1).to_string(), "blockId": "nope" } },
        { "type": "pageLink", "attrs": { "pageId": page_id(2).to_string() } }
    ])));
    let problems = damaged.check_invariants();
    for message in [
        "node /0: pageLink without a valid pageId",
        "node /1: pageLink with a malformed blockId",
        "node /2: pageLink without an id",
    ] {
        assert!(
            problems.iter().any(|p| p == message),
            "missing {message:?} in {problems:#?}"
        );
    }
    assert_eq!(problems.len(), 3, "{problems:#?}");
}

#[test]
fn markdown_renders_a_page_link_like_a_reference_on_a_line_of_its_own() {
    let titles = |id: PageId| (id == page_id(2)).then(|| "Known".to_owned());
    let page = from_json(&doc_of(json!([
        para(bid(1), "before"),
        page_link(2, 2, None),
        page_link(3, 2, Some(9)),
        page_link(4, 1, None),
        { "type": "pageLink", "attrs": { "id": s(bid(5)), "pageId": "not-a-uuid" } },
        { "type": "callout", "attrs": { "id": s(bid(6)) }, "content": [page_link(7, 2, None)] },
        { "type": "columnList", "attrs": { "id": s(bid(8)) }, "content": [
            { "type": "column", "attrs": { "id": s(bid(9)) }, "content": [page_link(10, 2, None)] },
            { "type": "column", "attrs": { "id": s(bid(11)) }, "content": [para(bid(12), "right")] }
        ]},
        para(bid(13), "after")
    ])));
    assert_eq!(
        page.to_markdown(&titles),
        format!(
            "before\n\n[[Known]]\n\n[[Known]]\n\n[[{}]]\n\n[[not-a-uuid]]\n\n> 💡 [[Known]]\n\n[[Known]]\n\nright\n\nafter\n",
            page_id(1)
        )
    );
}

// ----- derived blocks (additive nodes, ADR-0016 amendment of 2026-10-06) ---------------------

/// A `tableOfContents` or `breadcrumb`: a leaf block with an `id` and nothing else.
fn derived(kind: &str, id: u8) -> Value {
    json!({ "type": kind, "attrs": { "id": s(bid(id)) } })
}

#[test]
fn derived_blocks_are_ordinary_leaf_blocks_with_an_id_and_no_text() {
    let json = doc_of(json!([
        para(bid(1), "before"),
        derived("tableOfContents", 2),
        column_list(
            3,
            vec![
                column(
                    4,
                    vec![derived("breadcrumb", 5), para(bid(6), "in a column")]
                ),
                column(7, vec![para(bid(8), "right")]),
            ]
        ),
        json!({ "type": "callout", "attrs": { "id": s(bid(9)) }, "content": [
            derived("tableOfContents", 10)
        ]}),
        para(bid(11), "after"),
    ]));
    let page = from_json(&json);
    // The names are in the schema module, and none is unknown to the core.
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert_eq!(
        schema::DERIVED_NODES,
        ["tableOfContents", "breadcrumb", "pageList"]
    );
    for name in schema::DERIVED_NODES {
        assert!(schema::NODE_NAMES.contains(name), "{name}");
        assert!(schema::ID_BEARING.contains(name), "{name}");
        assert!(!schema::TEXT_NODES.contains(name), "{name}");
    }
    // Every node with an id is a block, in document order, and a derived block has no text.
    let blocks = page.blocks();
    let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "paragraph",
            "tableOfContents",
            "columnList",
            "column",
            "breadcrumb",
            "paragraph",
            "column",
            "paragraph",
            "callout",
            "tableOfContents",
            "paragraph",
        ]
    );
    assert!(
        blocks
            .iter()
            .filter(|b| schema::DERIVED_NODES.contains(&b.kind.as_str()))
            .all(|b| b.inlines.is_empty())
    );
    assert_eq!(
        texts(&page),
        ["before", "in a column", "right", "after"].map(str::to_owned)
    );
    assert!(page.links().is_empty());
    assert_sound(&page);
    // Portable JSON keeps them exactly as they are, ids included.
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).to_portable(), json);
    // They hold no text to set, and editing around them keeps them.
    assert!(is_invalid(&page.set_block_text(bid(2), "x").unwrap_err()));
    assert!(is_invalid(&page.set_block_text(bid(5), "x").unwrap_err()));
    page.set_block_text(bid(6), "changed").unwrap();
    page.insert_block(Some(bid(11)), BlockKind::Paragraph, "appended", bid(60))
        .unwrap();
    let portable = page.to_portable();
    assert_eq!(portable["content"][1], derived("tableOfContents", 2));
    assert_eq!(
        portable["content"][2]["content"][0]["content"][0],
        derived("breadcrumb", 5)
    );
    assert_sound(&page);
}

#[test]
fn a_page_of_derived_blocks_alone_is_empty() {
    // They show what the page and the tree hold and store nothing: no text, no reference,
    // no attachment, as for a page of only a divider or an empty table.
    let page = from_json(&doc_of(json!([
        derived("tableOfContents", 1),
        derived("breadcrumb", 2)
    ])));
    assert!(page.is_empty());
    assert_eq!(page.blocks().len(), 2);
    assert_sound(&page);
}

#[test]
fn markdown_of_a_derived_block_is_empty_and_takes_its_place_in_the_join() {
    let toc = |id| derived("tableOfContents", id);
    let crumbs = |id| derived("breadcrumb", id);
    // Between two blocks the empty chunk and its two joins leave three blank lines.
    assert_eq!(
        md(json!([para(bid(1), "a"), toc(2), para(bid(3), "b")])),
        "a\n\n\n\nb\n"
    );
    assert_eq!(
        md(json!([para(bid(1), "a"), crumbs(2), para(bid(3), "b")])),
        "a\n\n\n\nb\n"
    );
    // Each one adds two newlines: two in a row leave five blank lines.
    assert_eq!(
        md(json!([
            para(bid(1), "a"),
            toc(2),
            crumbs(4),
            para(bid(3), "b")
        ])),
        "a\n\n\n\n\n\nb\n"
    );
    // As the first block of the page: two empty lines before the first line (leading
    // newlines are not trimmed).
    assert_eq!(md(json!([toc(2), para(bid(1), "a")])), "\n\na\n");
    // As the last blocks of the page: trimmed with the trailing newlines.
    assert_eq!(md(json!([para(bid(1), "a"), toc(2), crumbs(3)])), "a\n");
    // Alone, or with nothing else: an empty page.
    assert_eq!(md(json!([toc(2)])), "\n");
    assert_eq!(md(json!([toc(2), crumbs(3)])), "\n");
    // In a container the empty block is the empty string among its blocks, as anywhere.
    assert_eq!(
        md(json!([
            json!({ "type": "blockquote", "attrs": { "id": s(bid(10)) }, "content": [
                para(bid(11), "a"), toc(12), para(bid(13), "b")
            ]})
        ])),
        "> a\n>\n>\n>\n> b\n"
    );
}

// ----- the list view (additive node, ADR-0016 amendment of 2026-10-07) --------------------------

/// A `pageList`: a leaf block with an `id` and the configuration of its view.
fn page_list(id: u8, source: Value, properties: Value, sort: Value) -> Value {
    json!({ "type": "pageList", "attrs": {
        "id": s(bid(id)), "source": source, "layout": "list", "properties": properties, "sort": sort
    }})
}

/// The list view as the editor inserts it: the children of the page, nothing shown, by title.
fn default_page_list(id: u8) -> Value {
    page_list(
        id,
        json!({ "kind": "children" }),
        json!([]),
        json!({ "by": "title", "dir": "asc" }),
    )
}

/// A list view of the pages that carry a tag, showing two properties, sorted by one of them.
fn tagged_page_list(id: u8) -> Value {
    page_list(
        id,
        json!({ "kind": "tag", "optionId": s(bid(200)) }),
        json!([s(bid(201)), s(bid(202))]),
        json!({ "by": s(bid(202)), "dir": "desc" }),
    )
}

#[test]
fn a_page_list_is_a_derived_leaf_block_whose_configuration_passes_through() {
    let json = doc_of(json!([
        para(bid(1), "before"),
        default_page_list(2),
        column_list(
            3,
            vec![
                column(4, vec![tagged_page_list(5), para(bid(6), "in a column")]),
                column(7, vec![para(bid(8), "right")]),
            ]
        ),
        json!({ "type": "callout", "attrs": { "id": s(bid(9)) }, "content": [
            default_page_list(10)
        ]}),
        para(bid(11), "after"),
    ]));
    let page = from_json(&json);
    // The name is in the schema module and in the derived list, and the core knows it.
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert!(schema::NODE_NAMES.contains(&"pageList"));
    assert!(schema::ID_BEARING.contains(&"pageList"));
    assert!(schema::DERIVED_NODES.contains(&"pageList"));
    assert!(!schema::TEXT_NODES.contains(&"pageList"));
    // It is a block with an id and no text, in document order, wherever blocks are.
    let blocks = page.blocks();
    let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "paragraph",
            "pageList",
            "columnList",
            "column",
            "pageList",
            "paragraph",
            "column",
            "paragraph",
            "callout",
            "pageList",
            "paragraph",
        ]
    );
    assert!(
        blocks
            .iter()
            .filter(|b| b.kind == "pageList")
            .all(|b| b.inlines.is_empty())
    );
    assert_eq!(
        texts(&page),
        ["before", "in a column", "right", "after"].map(str::to_owned)
    );
    // It holds no reference: the tag and the properties it names are ids of the space's
    // schema, not links between pages.
    assert!(page.links().is_empty());
    assert_sound(&page);
    // Portable JSON keeps the configuration exactly as it is, objects and lists included.
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).to_portable(), json);
    let portable = page.to_portable();
    assert_eq!(portable["content"][1], default_page_list(2));
    assert_eq!(
        portable["content"][2]["content"][0]["content"][0],
        tagged_page_list(5)
    );
    assert_eq!(
        portable["content"][2]["content"][0]["content"][0]["attrs"]["sort"]["dir"],
        "desc"
    );
    // It holds no text to set, and editing around it keeps it.
    assert!(is_invalid(&page.set_block_text(bid(2), "x").unwrap_err()));
    assert!(is_invalid(&page.set_block_text(bid(5), "x").unwrap_err()));
    page.set_block_text(bid(6), "changed").unwrap();
    page.insert_block(Some(bid(11)), BlockKind::Paragraph, "appended", bid(60))
        .unwrap();
    let portable = page.to_portable();
    assert_eq!(portable["content"][1], default_page_list(2));
    assert_eq!(
        portable["content"][2]["content"][0]["content"][0],
        tagged_page_list(5)
    );
    assert_sound(&page);
}

/// A table view (ADR-0016 amendments of 2026-10-08 and 2026-10-14): the list's configuration with
/// `layout: "table"` and the optional `filters`, `columnWidths` and `calculations` (the footer's function
/// of the title column and of two properties, the last one a function of a newer version).
fn table_page_list(id: u8) -> Value {
    json!({ "type": "pageList", "attrs": {
        "id": s(bid(id)),
        "source": { "kind": "children" },
        "layout": "table",
        "properties": [s(bid(201)), s(bid(202))],
        "sort": { "by": s(bid(202)), "dir": "desc" },
        "filters": [
            { "propertyId": s(bid(201)), "op": "isSet" },
            { "propertyId": s(bid(202)), "op": "between", "value": { "from": "2026-10-01", "to": "2026-10-31" } },
            { "propertyId": s(bid(203)), "op": "an-operator-of-a-newer-version", "value": [1, "two"] }
        ],
        "columnWidths": { "title": 320, (s(bid(201))): 120.5 },
        "calculations": {
            "title": "count_all",
            (s(bid(201))): "sum",
            (s(bid(203))): "a-function-of-a-newer-version"
        }
    }})
}

#[test]
fn a_table_page_list_keeps_its_filters_widths_and_calculations_whatever_they_hold() {
    // The layout, the filters, the widths and the calculations are plain attributes: the core reads
    // none of them, so a configuration of a newer version (an operator or a function this version
    // does not know) is kept as it is, and the block still holds no text, no reference and no export.
    let json = doc_of(json!([
        para(bid(1), "before"),
        table_page_list(2),
        default_page_list(3)
    ]));
    let page = from_json(&json);
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert_eq!(page.to_portable(), json);
    assert_eq!(page.to_portable()["content"][1], table_page_list(2));
    assert_eq!(
        page.to_portable()["content"][1]["attrs"]["filters"][2]["op"],
        "an-operator-of-a-newer-version"
    );
    assert_eq!(
        page.to_portable()["content"][1]["attrs"]["calculations"],
        json!({
            "title": "count_all",
            (s(bid(201))): "sum",
            (s(bid(203))): "a-function-of-a-newer-version"
        })
    );
    assert!(page.links().is_empty());
    assert!(is_invalid(&page.set_block_text(bid(2), "x").unwrap_err()));
    page.set_block_text(bid(1), "changed").unwrap();
    assert_eq!(page.to_portable()["content"][1], table_page_list(2));
    assert_eq!(page.to_portable()["content"][2], default_page_list(3));
    assert_sound(&page);
    // The block without them stays without: the new attributes are never added by the core.
    assert!(
        page.to_portable()["content"][2]["attrs"]
            .get("filters")
            .is_none()
    );
    assert!(
        page.to_portable()["content"][2]["attrs"]
            .get("columnWidths")
            .is_none()
    );
    assert!(
        page.to_portable()["content"][2]["attrs"]
            .get("calculations")
            .is_none()
    );
}

#[test]
fn a_page_of_page_lists_alone_is_empty() {
    // The pages a list view shows are the app's business at run time: the document stores no
    // text, no reference and no attachment for it, as for a table of contents.
    let page = from_json(&doc_of(json!([
        default_page_list(1),
        tagged_page_list(2),
        derived("tableOfContents", 3)
    ])));
    assert!(page.is_empty());
    assert_eq!(page.blocks().len(), 3);
    assert_sound(&page);
}

#[test]
fn markdown_of_a_page_list_is_empty_and_takes_its_place_in_the_join() {
    // The page set is derived at run time, so the export writes nothing for it, by the same
    // generic rule as the other derived blocks: between two blocks it leaves three blank lines.
    assert_eq!(
        md(json!([
            para(bid(1), "a"),
            default_page_list(2),
            para(bid(3), "b")
        ])),
        "a\n\n\n\nb\n"
    );
    assert_eq!(
        md(json!([
            para(bid(1), "a"),
            tagged_page_list(2),
            para(bid(3), "b")
        ])),
        "a\n\n\n\nb\n"
    );
    // Next to another derived block: five blank lines, as two of them in a row.
    assert_eq!(
        md(json!([
            para(bid(1), "a"),
            default_page_list(2),
            derived("breadcrumb", 4),
            para(bid(3), "b")
        ])),
        "a\n\n\n\n\n\nb\n"
    );
    // First on the page: two empty lines before the first line. Last: trimmed. Alone: an empty page.
    assert_eq!(
        md(json!([default_page_list(2), para(bid(1), "a")])),
        "\n\na\n"
    );
    assert_eq!(md(json!([para(bid(1), "a"), tagged_page_list(2)])), "a\n");
    assert_eq!(md(json!([default_page_list(2)])), "\n");
    // Nothing of the configuration reaches the text: no id, no kind, no property id.
    let text = md(json!([
        para(bid(1), "a"),
        tagged_page_list(2),
        para(bid(3), "b")
    ]));
    assert!(!text.contains("tag") && !text.contains("title") && !text.contains(&s(bid(201))));
    // In a container it is the empty string among its blocks, as anywhere.
    assert_eq!(
        md(json!([
            json!({ "type": "blockquote", "attrs": { "id": s(bid(10)) }, "content": [
                para(bid(11), "a"), default_page_list(12), para(bid(13), "b")
            ]})
        ])),
        "> a\n>\n>\n>\n> b\n"
    );
}

/// A page with a list view at the top, one in a column, one in a toggle and one in a callout.
fn page_with_nested_lists() -> Value {
    doc_of(json!([
        para(bid(1), "before"),
        table_page_list(2),
        column_list(
            3,
            vec![
                column(4, vec![tagged_page_list(5), para(bid(6), "in a column")]),
                column(7, vec![para(bid(8), "right")]),
            ]
        ),
        json!({ "type": "details", "attrs": { "id": s(bid(9)) }, "content": [
            { "type": "detailsSummary", "attrs": { "id": s(bid(10)) }, "content": [text_node("toggle")] },
            { "type": "detailsContent", "attrs": { "id": s(bid(11)) }, "content": [default_page_list(12)] }
        ]}),
        json!({ "type": "callout", "attrs": { "id": s(bid(13)) }, "content": [
            default_page_list(14)
        ]}),
        para(bid(15), "after"),
    ]))
}

fn attrs_of(value: &Value) -> serde_json::Map<String, Value> {
    value["attrs"].as_object().unwrap().clone()
}

#[test]
fn page_lists_lists_every_list_view_in_document_order_with_its_attributes() {
    let json = page_with_nested_lists();
    let page = from_json(&json);
    let lists = page.page_lists();
    assert_eq!(
        lists.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [2, 5, 12, 14].map(bid)
    );
    assert_eq!(lists[0].1, attrs_of(&table_page_list(2)));
    assert_eq!(lists[1].1, attrs_of(&tagged_page_list(5)));
    assert_eq!(lists[2].1, attrs_of(&default_page_list(12)));
    assert_eq!(lists[3].1, attrs_of(&default_page_list(14)));
    // Every attribute the configuration holds, nested values as they are, the id included.
    assert_eq!(lists[0].1["id"], s(bid(2)));
    assert_eq!(
        lists[0].1["filters"][2]["op"],
        "an-operator-of-a-newer-version"
    );
    assert_eq!(lists[0].1["columnWidths"]["title"], 320);
    assert_eq!(lists[0].1["columnWidths"][s(bid(201))], 120.5);
    assert_eq!(lists[0].1["calculations"]["title"], "count_all");
    assert_eq!(lists[0].1["calculations"][s(bid(201))], "sum");
    assert_eq!(
        lists[1].1["source"],
        json!({ "kind": "tag", "optionId": s(bid(200)) })
    );
    // Reading writes nothing, and a round trip through the portable JSON gives the same lists.
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).page_lists(), lists);
    // Other blocks are not listed; a page without any gives nothing.
    assert!(
        from_json(&doc_of(json!([para(bid(1), "a")])))
            .page_lists()
            .is_empty()
    );
    assert!(new_page(1).page_lists().is_empty());
    assert!(
        PageDoc::from_doc(Doc::new(DocKind::Page, 1))
            .unwrap()
            .page_lists()
            .is_empty()
    );
}

#[test]
fn a_page_list_keeps_its_views_as_one_plain_attribute_that_reads_back_typed() {
    let views = json!([
        { "id": "k3f9a2bc", "name": "", "layout": "table", "properties": [s(bid(201))],
          "sort": { "by": s(bid(201)), "dir": "asc" },
          "filter": { "op": "and", "items": [
              { "propertyId": s(bid(201)), "op": "hasOption", "value": s(bid(210)) },
              { "op": "or", "items": [
                  { "propertyId": s(bid(202)), "op": "hasOption", "value": s(bid(211)) },
                  { "propertyId": s(bid(201)), "op": "isSet" } ] } ] },
          "columnWidths": { "title": 320 } },
        { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": [],
          "sort": { "by": "title", "dir": "asc" }, "dateBy": s(bid(201)), "endBy": s(bid(202)),
          "openIn": "center", "cardSize": "l", "someKeyOfANewerVersion": [1, { "a": null }] }
    ]);
    let list = json!({ "type": "pageList", "attrs": {
        "id": s(bid(2)), "source": { "kind": "children" }, "layout": "table",
        "properties": [s(bid(201))], "sort": { "by": s(bid(201)), "dir": "asc" },
        "filters": [{ "propertyId": s(bid(201)), "op": "hasOption", "value": s(bid(210)) }],
        "columnWidths": { "title": 320 },
        "views": views
    }});
    let json = doc_of(json!([para(bid(1), "a"), list]));
    let page = from_json(&json);
    let lists = page.page_lists();
    assert_eq!(
        lists[0].1["views"], views,
        "the list is stored whole, as plain JSON"
    );
    // Reading changes nothing, and a round trip through the portable JSON gives the same attribute.
    assert_eq!(page.to_portable(), json);
    assert_eq!(from_json(&page.to_portable()).page_lists(), lists);
    let typed = crate::views::read_views(&lists[0].1).unwrap();
    assert_eq!(typed.len(), 2);
    assert_eq!(typed[1].end_by, Some(s(bid(202))));
    assert_eq!(serde_json::to_value(&typed).unwrap(), views);

    // The attribute is replaced whole by `set_page_list_attrs`, and removed like any other.
    let newer = json!([{ "id": "only", "name": "One", "layout": "list", "properties": [] }]);
    assert!(
        page.set_page_list_attrs(bid(2), &[("views", Some(newer.clone()))])
            .unwrap()
    );
    assert_eq!(page.page_lists()[0].1["views"], newer);
    assert!(
        !page
            .set_page_list_attrs(bid(2), &[("views", Some(newer))])
            .unwrap()
    );
    assert!(
        page.set_page_list_attrs(bid(2), &[("views", None)])
            .unwrap()
    );
    assert!(page.page_lists()[0].1.get("views").is_none());
    assert_eq!(
        page.page_lists()[0].1["layout"],
        "table",
        "the mirror stays"
    );
    assert_sound(&page);
}

#[test]
fn set_page_list_attrs_sets_removes_and_reports_whether_anything_changed() {
    let page = from_json(&page_with_nested_lists());
    let before = page.to_portable();

    // Setting whole values, removing one, in one commit.
    let changed = page
        .set_page_list_attrs(
            bid(2),
            &[
                ("properties", Some(json!([s(bid(230))]))),
                ("sort", Some(json!({ "by": s(bid(230)), "dir": "asc" }))),
                ("filters", None),
                ("groupBy", Some(json!({ "propertyId": s(bid(230)) }))),
                ("columnWidths", Some(Value::Null)),
            ],
        )
        .unwrap();
    assert!(changed);
    let after = page.page_lists();
    assert_eq!(after[0].1["properties"], json!([s(bid(230))]));
    assert_eq!(
        after[0].1["sort"],
        json!({ "by": s(bid(230)), "dir": "asc" })
    );
    assert_eq!(after[0].1["groupBy"], json!({ "propertyId": s(bid(230)) }));
    assert!(after[0].1.get("filters").is_none() && after[0].1.get("columnWidths").is_none());
    assert_eq!(
        after[0].1["layout"], "table",
        "what the call did not name stays"
    );
    assert_eq!(after[0].1["id"], s(bid(2)));
    // The other lists and the rest of the page are untouched.
    let portable = page.to_portable();
    assert_eq!(portable["content"][0], before["content"][0]);
    assert_eq!(portable["content"][2], before["content"][2]);
    assert_eq!(portable["content"][3], before["content"][3]);
    assert_eq!(portable["content"][4], before["content"][4]);
    assert_eq!(from_json(&portable).page_lists(), after);
    assert_sound(&page);

    // The same call again changes nothing: no answer of "changed", no operation.
    let version = page.doc().version();
    assert!(
        !page
            .set_page_list_attrs(
                bid(2),
                &[
                    ("properties", Some(json!([s(bid(230))]))),
                    ("filters", None),
                    ("columnWidths", None),
                    ("never-there", Some(Value::Null)),
                ],
            )
            .unwrap()
    );
    assert_eq!(page.doc().version(), version);
    assert!(!page.set_page_list_attrs(bid(2), &[]).unwrap());
    assert_eq!(page.doc().version(), version);

    // `100` and `100.0` are one value; a new attribute is added whole.
    assert!(
        page.set_page_list_attrs(bid(2), &[("columnWidths", Some(json!({ "title": 100.0 })))])
            .unwrap()
    );
    assert!(
        !page
            .set_page_list_attrs(bid(2), &[("columnWidths", Some(json!({ "title": 100 })))])
            .unwrap()
    );
    assert_eq!(
        page.page_lists()[0].1["columnWidths"],
        json!({ "title": 100 })
    );
}

#[test]
fn set_page_list_attrs_writes_the_calculations_whole_and_removes_them() {
    let page = from_json(&page_with_nested_lists());
    let before = page.to_portable();
    // The footer's functions of a table are one plain value: set whole, changed whole, removed.
    let calculations = json!({ "title": "count_not_empty", (s(bid(202))): "date_range" });
    assert!(
        page.set_page_list_attrs(bid(2), &[("calculations", Some(calculations.clone()))])
            .unwrap()
    );
    assert_eq!(page.page_lists()[0].1["calculations"], calculations);
    // The same value again writes nothing; a list without any gets one added.
    let version = page.doc().version();
    assert!(
        !page
            .set_page_list_attrs(bid(2), &[("calculations", Some(calculations.clone()))])
            .unwrap()
    );
    assert_eq!(page.doc().version(), version);
    assert!(
        page.set_page_list_attrs(bid(12), &[("calculations", Some(calculations.clone()))])
            .unwrap()
    );
    assert_eq!(page.page_lists()[2].1["calculations"], calculations);
    // Removing the last one takes the key away, as if it had never been set; the rest stays.
    assert!(
        page.set_page_list_attrs(bid(12), &[("calculations", None)])
            .unwrap()
    );
    assert!(page.page_lists()[2].1.get("calculations").is_none());
    assert!(
        page.set_page_list_attrs(bid(2), &[("calculations", Some(Value::Null))])
            .unwrap()
    );
    assert!(page.page_lists()[0].1.get("calculations").is_none());
    assert_eq!(page.page_lists()[0].1["columnWidths"]["title"], 320);
    let mut expected = before;
    expected["content"][1]["attrs"]
        .as_object_mut()
        .unwrap()
        .remove("calculations");
    assert_eq!(page.to_portable(), expected);
    assert_sound(&page);
}

#[test]
fn set_page_list_attrs_reaches_lists_nested_in_columns_toggles_and_callouts() {
    let page = from_json(&page_with_nested_lists());
    for (n, source) in [(5, "tag"), (12, "children"), (14, "children")] {
        let new_properties = json!([s(bid(230 + n))]);
        assert!(
            page.set_page_list_attrs(bid(n), &[("properties", Some(new_properties.clone()))])
                .unwrap(),
            "list {n} ({source})"
        );
        let found = page
            .page_lists()
            .into_iter()
            .find(|(id, _)| *id == bid(n))
            .unwrap();
        assert_eq!(found.1["properties"], new_properties);
        assert_eq!(found.1["source"]["kind"], source);
    }
    // The first list never changed.
    assert_eq!(page.page_lists()[0].1, attrs_of(&table_page_list(2)));
    // The edits survive a merge with a concurrent edit of the text around them.
    let other = PageDoc::from_doc(
        Doc::load(
            DocKind::Page,
            9,
            Some(&page.doc().export_snapshot().unwrap()),
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    other
        .set_block_text(bid(6), "typed on the other device")
        .unwrap();
    page.set_page_list_attrs(bid(5), &[("layout", Some(json!("table")))])
        .unwrap();
    let blob = other
        .doc()
        .export_updates_since(Some(&page.doc().version()))
        .unwrap()
        .unwrap();
    page.doc().import(&blob).unwrap();
    let list = page
        .page_lists()
        .into_iter()
        .find(|(id, _)| *id == bid(5))
        .unwrap();
    assert_eq!(list.1["layout"], "table");
    assert_eq!(list.1["properties"], json!([s(bid(235))]));
    assert!(texts(&page).contains(&"typed on the other device".to_owned()));
    assert_sound(&page);
}

#[test]
fn set_page_list_attrs_refuses_what_is_not_a_list_view_and_writes_nothing() {
    let page = from_json(&page_with_nested_lists());
    let before = page.to_portable();
    let version = page.doc().version();
    let change = [("properties", Some(json!([])))];
    // An unknown block, a block that is not a list view (a paragraph, a column, the page's own
    // root id), a block whose id is a list view's neighbour.
    for id in [bid(99), bid(1), bid(4), bid(9), bid(10)] {
        assert!(
            matches!(
                page.set_page_list_attrs(id, &change),
                Err(ModelError::BlockNotFound)
            ),
            "{id}"
        );
    }
    // The id cannot be changed, an unnamed attribute cannot be set; one bad change in a call
    // keeps the good ones out too.
    for bad in [
        ("id", Some(json!(s(bid(77))))),
        ("id", None),
        ("", Some(json!(1))),
    ] {
        let err = page
            .set_page_list_attrs(bid(2), &[("layout", Some(json!("list"))), bad])
            .unwrap_err();
        assert!(is_invalid(&err), "{err:?}");
    }
    // A value nested too deeply is a shape error.
    let mut deep = json!(1);
    for _ in 0..200 {
        deep = json!([deep]);
    }
    assert!(matches!(
        page.set_page_list_attrs(
            bid(2),
            &[("layout", Some(json!("list"))), ("deep", Some(deep))]
        ),
        Err(ModelError::Shape(_))
    ));
    assert_eq!(page.to_portable(), before);
    assert_eq!(page.doc().version(), version);
    assert_sound(&page);
}

// ----- date mentions (additive inline node, ADR-0016 amendment of 2026-10-07) -------------------

fn date_mention(id: u8, date: &str, time: Option<&str>) -> Value {
    match time {
        Some(time) => {
            json!({ "type": "dateMention", "attrs": { "id": s(bid(id)), "date": date, "time": time } })
        }
        None => json!({ "type": "dateMention", "attrs": { "id": s(bid(id)), "date": date } }),
    }
}

fn text_node(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn paragraph_of(id: u8, content: Vec<Value>) -> Value {
    json!({ "type": "paragraph", "attrs": { "id": s(bid(id)) }, "content": content })
}

#[test]
fn a_date_mention_reads_as_its_iso_text_inside_the_text_run_around_it() {
    let json = doc_of(json!([
        paragraph_of(
            1,
            vec![
                text_node("Meet on "),
                date_mention(2, "2026-10-07", None),
                text_node(" and again at "),
                date_mention(3, "2026-10-08", Some("14:30")),
                text_node(" sharp"),
            ]
        ),
        paragraph_of(4, vec![date_mention(5, "2024-02-29", None)]),
    ]));
    let page = from_json(&json);
    assert_sound(&page);
    assert!(page.unknown_node_names(schema::NODE_NAMES).is_empty());
    assert!(schema::NODE_NAMES.contains(&"dateMention"));
    assert!(schema::ID_BEARING.contains(&"dateMention"));

    // The paragraph's inline content is one text: the mention does not split the run, so the
    // text index and the backlinks' text hold the date as typed.
    let blocks = page.blocks();
    assert_eq!(
        blocks.iter().map(|b| b.kind.as_str()).collect::<Vec<_>>(),
        [
            "paragraph",
            "dateMention",
            "dateMention",
            "paragraph",
            "dateMention"
        ]
    );
    assert_eq!(
        blocks[0].inlines,
        vec![Inline::Text(
            "Meet on 2026-10-07 and again at 2026-10-08 14:30 sharp".into()
        )]
    );
    assert_eq!(
        blocks[3].inlines,
        vec![Inline::Text("2024-02-29".into())],
        "a paragraph of nothing but a mention is that text"
    );
    // The mention is a node with an id of its own and no inline content, like a `pageRef`.
    for mention in blocks.iter().filter(|b| b.kind == "dateMention") {
        assert!(mention.inlines.is_empty());
    }
    assert_eq!(
        texts(&page),
        [
            "Meet on 2026-10-07 and again at 2026-10-08 14:30 sharp".to_owned(),
            "2024-02-29".to_owned()
        ]
    );
    // A date is no link: backlinks have nothing to list.
    assert!(page.links().is_empty());
    assert_eq!(
        page.block_ids(),
        blocks.iter().map(|b| b.id).collect::<Vec<_>>()
    );
}

#[test]
fn a_date_mention_next_to_a_break_or_a_reference_splits_the_text_only_there() {
    let reference = json!({ "type": "pageRef", "attrs": { "id": s(bid(9)), "pageId": page_id(1).to_string() } });
    let json = doc_of(json!([paragraph_of(
        1,
        vec![
            text_node("a "),
            date_mention(2, "2026-10-07", None),
            json!({ "type": "hardBreak" }),
            date_mention(3, "2026-10-08", Some("09:05")),
            text_node(" b "),
            reference,
            date_mention(4, "2026-10-09", None),
        ]
    )]));
    let page = from_json(&json);
    assert_sound(&page);
    assert_eq!(
        page.blocks()[0].inlines,
        vec![
            Inline::Text("a 2026-10-07".into()),
            Inline::Break,
            Inline::Text("2026-10-08 09:05 b ".into()),
            Inline::PageRef {
                page_id: page_id(1),
                block_id: None
            },
            Inline::Text("2026-10-09".into()),
        ]
    );
}

#[test]
fn a_date_mention_counts_as_content() {
    let json = doc_of(json!([paragraph_of(
        1,
        vec![date_mention(2, "2026-10-07", None)]
    )]));
    assert!(!from_json(&json).is_empty());
}

#[test]
fn only_real_days_are_valid_dates() {
    let valid = [
        "2026-10-07",
        "2026-01-31",
        "2026-12-31",
        "2024-02-29", // a leap year
        "2000-02-29", // a century divisible by 400
        "0001-01-01",
        "9999-12-31",
        "2026-04-30",
    ];
    let invalid = [
        "",
        "2026-10-7",
        "26-10-07",
        "2026/10/07",
        "20261007",
        "2026-10-07 ",
        " 2026-10-07",
        "2026-10-07T00:00",
        "2026-13-01",
        "2026-00-10",
        "2026-10-00",
        "2026-10-32",
        "2026-04-31",
        "2026-02-29", // not a leap year
        "1900-02-29", // a century not divisible by 400
        "0000-01-01",
        "+026-10-07",
        "2026-1o-07",
        "tomorrow",
    ];
    let content = |dates: &[&str]| -> Value {
        doc_of(json!([paragraph_of(
            1,
            dates
                .iter()
                .enumerate()
                .map(|(i, date)| date_mention(u8::try_from(10 + i).unwrap(), date, None))
                .collect()
        )]))
    };
    assert_sound(&from_json(&content(&valid)));
    assert_eq!(
        from_json(&content(&valid)).blocks()[0].inlines,
        vec![Inline::Text(valid.join(""))]
    );
    for date in invalid {
        let page = from_json(&content(&[date]));
        let problems = page.check_invariants();
        assert_eq!(
            problems,
            vec!["node /0/0: dateMention without a valid date".to_owned()],
            "{date:?}"
        );
        // It reads as nothing: no text, no markdown.
        assert!(page.blocks()[0].inlines.is_empty(), "{date:?}");
        assert_eq!(page.to_markdown(&no_titles), "\n", "{date:?}");
    }
}

#[test]
fn only_hours_and_minutes_are_a_valid_time() {
    for time in ["00:00", "09:05", "12:00", "23:59"] {
        let page = from_json(&doc_of(json!([paragraph_of(
            1,
            vec![date_mention(2, "2026-10-07", Some(time))]
        )])));
        assert_sound(&page);
        assert_eq!(
            page.blocks()[0].inlines,
            vec![Inline::Text(format!("2026-10-07 {time}"))]
        );
    }
    for time in [
        "", "9:05", "24:00", "12:60", "12:5", "1205", "12-05", "12:05:00", " 12:05", "ab:cd",
    ] {
        let page = from_json(&doc_of(json!([paragraph_of(
            1,
            vec![date_mention(2, "2026-10-07", Some(time))]
        )])));
        // A malformed time is reported and left out: the date still reads.
        assert_eq!(
            page.check_invariants(),
            vec!["node /0/0: dateMention with a malformed time".to_owned()],
            "{time:?}"
        );
        assert_eq!(
            page.blocks()[0].inlines,
            vec![Inline::Text("2026-10-07".into())],
            "{time:?}"
        );
        assert_eq!(page.to_markdown(&no_titles), "2026-10-07\n", "{time:?}");
    }
}

#[test]
fn check_invariants_reports_a_date_mention_without_a_date_or_an_id_or_with_other_types() {
    let json = doc_of(json!([paragraph_of(
        1,
        vec![
            json!({ "type": "dateMention", "attrs": { "id": s(bid(2)) } }),
            json!({ "type": "dateMention", "attrs": { "id": s(bid(3)), "date": 20_261_007 } }),
            json!({ "type": "dateMention", "attrs": { "date": "2026-10-07" } }),
            json!({ "type": "dateMention", "attrs": { "id": s(bid(4)), "date": "2026-10-07", "time": 930 } }),
        ]
    )]));
    let page = from_json(&json);
    let problems = page.check_invariants();
    assert_eq!(
        problems,
        vec![
            "node /0/0: dateMention without a valid date".to_owned(),
            "node /0/1: dateMention without a valid date".to_owned(),
            "node /0/2: dateMention without an id".to_owned(),
            "node /0/3: dateMention with a malformed time".to_owned(),
        ]
    );
    // Reading is lenient: the only text is the two valid dates, with the bad time left out.
    assert_eq!(
        page.blocks()
            .iter()
            .flat_map(|b| b.inlines.iter())
            .collect::<Vec<_>>(),
        [&Inline::Text("2026-10-072026-10-07".into())]
    );
}

#[test]
fn a_date_mention_survives_the_portable_round_trip_and_stores_no_empty_time() {
    let json = doc_of(json!([paragraph_of(
        1,
        vec![
            text_node("a "),
            date_mention(2, "2026-10-07", None),
            text_node(" "),
            date_mention(3, "2026-10-08", Some("14:30")),
        ]
    )]));
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
    // No `time` key without a time.
    assert!(
        page.to_portable()["content"][0]["content"][1]["attrs"]
            .get("time")
            .is_none()
    );
    let again = PageDoc::from_portable(Doc::new(DocKind::Page, 4), &page.to_portable()).unwrap();
    assert_eq!(again.to_portable(), json);
    assert_sound(&again);
}

#[test]
fn markdown_renders_a_date_mention_as_its_iso_text() {
    let md = |content: Vec<Value>| from_json(&doc_of(json!(content))).to_markdown(&no_titles);
    assert_eq!(
        md(vec![paragraph_of(
            1,
            vec![
                text_node("Due "),
                date_mention(2, "2026-10-07", None),
                text_node(", review "),
                date_mention(3, "2026-10-08", Some("14:30")),
                text_node("."),
            ]
        )]),
        "Due 2026-10-07, review 2026-10-08 14:30.\n"
    );
    // The text keeps its marks around it; the date does not carry any.
    assert_eq!(
        md(vec![paragraph_of(
            1,
            vec![
                json!({ "type": "text", "text": "bold ", "marks": [{ "type": "bold" }] }),
                date_mention(2, "2026-10-07", None),
                json!({ "type": "text", "text": " bold", "marks": [{ "type": "bold" }] }),
            ]
        )]),
        "**bold **2026-10-07** bold**\n"
    );
    // In a heading, a list item, a quote and a toggle's summary.
    let heading = json!({ "type": "heading", "attrs": { "id": s(bid(1)), "level": 2 }, "content": [
        text_node("Plan for "), date_mention(2, "2026-10-07", None)
    ]});
    let list = json!({ "type": "bulletList", "attrs": { "id": s(bid(3)) }, "content": [
        { "type": "listItem", "attrs": { "id": s(bid(4)) }, "content": [
            paragraph_of(5, vec![text_node("Call "), date_mention(6, "2026-10-09", Some("08:00"))])
        ]}
    ]});
    let quote = json!({ "type": "blockquote", "attrs": { "id": s(bid(7)) }, "content": [
        paragraph_of(8, vec![date_mention(9, "2026-10-10", None)])
    ]});
    let details = json!({ "type": "details", "attrs": { "id": s(bid(10)), "open": false }, "content": [
        { "type": "detailsSummary", "attrs": { "id": s(bid(11)) }, "content": [
            text_node("Until "), date_mention(12, "2026-10-11", None)
        ]},
        { "type": "detailsContent", "attrs": { "id": s(bid(13)) }, "content": [para(bid(14), "body")] }
    ]});
    assert_eq!(
        md(vec![heading, list, quote, details]),
        "## Plan for 2026-10-07\n\n- Call 2026-10-09 08:00\n\n> 2026-10-10\n\n\
         <details>\n<summary>Until 2026-10-11</summary>\n\nbody\n\n</details>\n"
    );
    // A mention among blocks (not in a paragraph) joins the inline nodes around it as one block.
    assert_eq!(
        md(vec![
            para(bid(1), "x"),
            date_mention(2, "2026-10-07", None),
            para(bid(3), "y")
        ]),
        "x\n\n2026-10-07\n\ny\n"
    );
}

// ----- whiteboard (ADR-0016 amendment of 2026-10-10, ADR-0033) -------------------------------

fn whiteboard_node(id: u8, attrs: &Value) -> Value {
    let mut all = json!({ "id": s(bid(id)) });
    if let (Some(all), Some(extra)) = (all.as_object_mut(), attrs.as_object()) {
        all.extend(extra.clone());
    }
    json!({ "type": "whiteboard", "attrs": all })
}

fn shape_element(id: &str, kind: &str, index: &str, version: u32) -> Value {
    json!({
        "id": id, "type": kind, "index": index, "version": version, "versionNonce": 7,
        "isDeleted": false, "x": 1.5, "y": 2, "frameId": null, "groupIds": []
    })
}

fn text_element(id: &str, index: &str, text: &str) -> Value {
    let mut element = shape_element(id, "text", index, 1);
    element["originalText"] = json!(text);
    element["text"] = json!(text);
    element
}

fn page_with_whiteboard() -> PageDoc {
    from_json(&doc_of(json!([
        para(bid(1), "intro"),
        whiteboard_node(2, &json!({ "height": 400 })),
    ])))
}

#[test]
fn a_whiteboard_reads_as_its_live_text_elements_in_z_order() {
    let page = page_with_whiteboard();
    // Written out of order, with a tombstone, a shape, a multi-line text and a blank one.
    for element in [
        text_element("t2", "a2", "second\n  line"),
        shape_element("r1", "rectangle", "a0", 1),
        text_element("t1", "a1", "first"),
        text_element("t3", "a3", "   "),
        {
            let mut gone = text_element("t4", "a4", "deleted text");
            gone["isDeleted"] = json!(true);
            gone
        },
    ] {
        page.set_whiteboard_element(bid(2), &element).unwrap();
    }
    assert_eq!(
        page.whiteboard_elements(bid(2))
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["r1", "t1", "t2", "t3", "t4"],
        "z-order by the fractional index; the tombstone stays"
    );
    assert!(page.whiteboard_elements(bid(2))[4].is_deleted());
    assert_eq!(
        texts(&page),
        ["intro", "first second line"],
        "live text elements only, whitespace collapsed"
    );
    assert_sound(&page);
    // A drawing is content: a page holding only one is not empty.
    assert!(!from_json(&doc_of(json!([whiteboard_node(2, &json!({}))]))).is_empty());
}

#[test]
fn the_portable_form_carries_the_elements_of_the_blocks_that_exist() {
    let page = page_with_whiteboard();
    page.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 3))
        .unwrap();
    // An orphan: its block (3) has no node.
    page.set_whiteboard_element(bid(3), &shape_element("o1", "ellipse", "a0", 1))
        .unwrap();
    let portable = page.to_portable();
    let boards = portable["whiteboards"].as_object().unwrap();
    assert_eq!(boards.len(), 1);
    assert_eq!(boards[&s(bid(2))]["r1"]["x"], 1.5);
    assert_eq!(boards[&s(bid(2))]["r1"]["y"], 2);
    assert_eq!(boards[&s(bid(2))]["r1"]["frameId"], Value::Null);
    // It rebuilds into the same page (the orphan is not exported).
    let copy = from_json(&portable);
    assert_eq!(copy.to_portable(), portable);
    assert_eq!(copy.whiteboard_elements(bid(2)).len(), 1);
    assert_sound(&copy);
    // A page without drawings has no `whiteboards` key at all.
    assert!(
        from_json(&doc_of(json!([para(bid(1), "x")])))
            .to_portable()
            .get("whiteboards")
            .is_none()
    );
}

#[test]
fn from_portable_rejects_damaged_whiteboards() {
    let id = s(bid(2));
    let bad = [
        json!("x"),
        json!({ "not a uuid": {} }),
        json!({ id.clone(): [] }),
        json!({ id.clone(): { "r1": { "id": "other" } } }),
        json!({ id.clone(): { "r1": 5 } }),
    ];
    for whiteboards in bad {
        let mut json = doc_of(json!([whiteboard_node(2, &json!({}))]));
        json["whiteboards"] = whiteboards;
        assert!(
            PageDoc::from_portable(Doc::new(DocKind::Page, 3), &json).is_err(),
            "{json}"
        );
    }
}

#[test]
fn orphans_stay_until_pruned_and_come_back_with_their_block() {
    let page = page_with_whiteboard();
    page.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 1))
        .unwrap();
    page.delete_block(bid(2)).unwrap();
    assert_eq!(
        page.orphan_whiteboard_keys(),
        vec![format!("{}/r1", s(bid(2)))]
    );
    assert!(page.to_portable().get("whiteboards").is_none());
    // Undo of the deletion re-inserts a node with the same id: the drawing is back.
    let json = page.to_portable();
    let mut content = json["content"].as_array().unwrap().clone();
    content.push(whiteboard_node(2, &json!({ "height": 400 })));
    let restored = from_json(&doc_of(Value::Array(content)));
    assert!(restored.orphan_whiteboard_keys().is_empty());
    // Pruning removes exactly the orphans.
    assert_eq!(page.prune_orphan_whiteboards().unwrap(), 1);
    assert!(page.orphan_whiteboard_keys().is_empty());
    assert_eq!(page.prune_orphan_whiteboards().unwrap(), 0);
}

#[test]
fn markdown_writes_a_whiteboard_as_a_quote_of_its_texts() {
    let page = from_json(&doc_of(json!([
        whiteboard_node(2, &json!({})),
        whiteboard_node(3, &json!({})),
    ])));
    page.set_whiteboard_element(bid(2), &text_element("t2", "a1", "Beta | two\nlines"))
        .unwrap();
    page.set_whiteboard_element(bid(2), &text_element("t1", "a0", "Alpha"))
        .unwrap();
    page.set_whiteboard_element(bid(2), &shape_element("r", "rectangle", "a2", 1))
        .unwrap();
    assert_eq!(
        page.to_markdown(&no_titles),
        "> **Whiteboard**\n>\n> - Alpha\n> - Beta | two lines\n\n> **Whiteboard**\n"
    );
}

#[test]
fn check_invariants_reports_a_malformed_whiteboard_without_quoting_it() {
    let page = from_json(&doc_of(json!([
        whiteboard_node(1, &json!({ "height": 160 })),
        whiteboard_node(2, &json!({ "height": 1200.0 })),
        whiteboard_node(3, &json!({ "height": 10 })),
        whiteboard_node(4, &json!({ "height": "secret" })),
        whiteboard_node(5, &json!({})),
    ])));
    let root = page.doc().inner().get_map("whiteboards");
    root.insert("secret-key", "secret").unwrap();
    root.insert(&format!("{}/r1", s(bid(1))), "secret").unwrap();
    root.insert(
        &format!("{}/r2", s(bid(1))),
        loro::LoroValue::from(json!({ "id": "secret" }).to_string()),
    )
    .unwrap();
    let problems = page.check_invariants();
    assert_eq!(
        problems,
        vec![
            "node /2: whiteboard with a malformed height".to_owned(),
            "node /3: whiteboard with a malformed height".to_owned(),
            format!("whiteboard {} has a malformed element", s(bid(1))),
            format!("whiteboard {} has a malformed element", s(bid(1))),
            "whiteboards has 1 entries with a malformed key".to_owned(),
        ]
    );
    assert!(problems.iter().all(|p| !p.contains("secret")));
    assert!(schema::NODE_NAMES.contains(&"whiteboard"));
    assert!(schema::ID_BEARING.contains(&"whiteboard"));
    assert!(!schema::TEXT_NODES.contains(&"whiteboard"));
}

/// Two replicas of one page, each editing the drawing offline.
fn two_whiteboard_replicas() -> (PageDoc, PageDoc) {
    let a = page_with_whiteboard();
    for element in [
        shape_element("r1", "rectangle", "a0", 1),
        shape_element("r2", "ellipse", "a1", 1),
    ] {
        a.set_whiteboard_element(bid(2), &element).unwrap();
    }
    let snapshot = a.doc().export_snapshot().unwrap();
    let b = PageDoc::from_doc(Doc::load(DocKind::Page, 9, Some(&snapshot), &[]).unwrap()).unwrap();
    (a, b)
}

fn exchange(a: &PageDoc, b: &PageDoc) {
    let to_b = a
        .doc()
        .export_updates_since(Some(&b.doc().version()))
        .unwrap();
    let to_a = b
        .doc()
        .export_updates_since(Some(&a.doc().version()))
        .unwrap();
    if let Some(to_a) = to_a {
        a.doc().import(&to_a).unwrap();
    }
    if let Some(to_b) = to_b {
        b.doc().import(&to_b).unwrap();
    }
}

fn version_of(page: &PageDoc, id: &str) -> Option<i64> {
    page.whiteboard_elements(bid(2))
        .iter()
        .find(|e| e.id == id)
        .and_then(|e| e.value["version"].as_i64())
}

#[test]
fn two_devices_editing_different_elements_keep_both_edits() {
    let (a, b) = two_whiteboard_replicas();
    a.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 2))
        .unwrap();
    b.set_whiteboard_element(bid(2), &shape_element("r2", "ellipse", "a1", 2))
        .unwrap();
    // Each one also adds an element of its own.
    a.set_whiteboard_element(bid(2), &shape_element("n-a", "diamond", "a2", 1))
        .unwrap();
    b.set_whiteboard_element(bid(2), &text_element("n-b", "a3", "from b"))
        .unwrap();
    exchange(&a, &b);
    for page in [&a, &b] {
        assert_eq!(version_of(page, "r1"), Some(2));
        assert_eq!(version_of(page, "r2"), Some(2));
        assert_eq!(page.whiteboard_elements(bid(2)).len(), 4);
        assert_sound(page);
    }
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(texts(&a), ["intro", "from b"]);
}

#[test]
fn two_devices_editing_the_same_element_converge_on_one_value() {
    let (a, b) = two_whiteboard_replicas();
    a.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 2))
        .unwrap();
    b.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 3))
        .unwrap();
    exchange(&a, &b);
    let (va, vb) = (version_of(&a, "r1"), version_of(&b, "r1"));
    assert_eq!(va, vb, "last writer wins per element, the same on both");
    assert!(va == Some(2) || va == Some(3));
    assert_eq!(a.to_portable(), b.to_portable());
    // The other element is untouched.
    assert_eq!(version_of(&a, "r2"), Some(1));
}

#[test]
fn a_delete_and_an_edit_of_the_same_element_converge_and_the_tombstone_is_kept() {
    let (a, b) = two_whiteboard_replicas();
    let mut tombstone = shape_element("r1", "rectangle", "a0", 5);
    tombstone["isDeleted"] = json!(true);
    a.set_whiteboard_element(bid(2), &tombstone).unwrap();
    b.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 4))
        .unwrap();
    exchange(&a, &b);
    // The CRDT keeps one of the two writes, the same on both devices. Which one is the
    // editor's business to repair: the device whose scene holds version 5 rewrites it when the
    // document shows an older one (ADR-0033).
    assert_eq!(a.to_portable(), b.to_portable());
    let kept = a
        .whiteboard_elements(bid(2))
        .into_iter()
        .find(|e| e.id == "r1")
        .unwrap();
    assert!(
        (kept.is_deleted() && kept.value["version"] == 5)
            || (!kept.is_deleted() && kept.value["version"] == 4)
    );
    // A tombstone written later is never dropped by the model: the key stays.
    a.set_whiteboard_element(bid(2), &tombstone).unwrap();
    assert!(
        a.whiteboard_elements(bid(2))
            .iter()
            .any(|e| e.id == "r1" && e.is_deleted())
    );
}

#[test]
fn concurrent_first_elements_of_a_new_whiteboard_both_survive() {
    // Both devices create the whiteboard's first element at the same time: separate keys of the
    // always-existing root map, no container is created, nothing is lost.
    let a = page_with_whiteboard();
    let snapshot = a.doc().export_snapshot().unwrap();
    let b = PageDoc::from_doc(Doc::load(DocKind::Page, 9, Some(&snapshot), &[]).unwrap()).unwrap();
    a.set_whiteboard_element(bid(2), &shape_element("x", "rectangle", "a0", 1))
        .unwrap();
    b.set_whiteboard_element(bid(2), &shape_element("y", "ellipse", "a1", 1))
        .unwrap();
    exchange(&a, &b);
    assert_eq!(a.whiteboard_elements(bid(2)).len(), 2);
    assert_eq!(a.to_portable(), b.to_portable());
}

#[test]
fn the_elements_are_not_part_of_the_tree_an_older_client_reads() {
    // An older client only knows the `doc` root: its tree has the node and no element, and it
    // never touches the other root.
    let page = page_with_whiteboard();
    page.set_whiteboard_element(bid(2), &shape_element("r1", "rectangle", "a0", 1))
        .unwrap();
    let tree = page
        .doc()
        .inner()
        .get_map("doc")
        .get_deep_value()
        .to_json_value()
        .to_string();
    assert!(tree.contains("whiteboard"));
    assert!(!tree.contains("rectangle"));
}

// ----- toggle headings: the optional `level` of `detailsSummary` (ADR-0016 amendment of 2026-10-14, ADR-0040) -----

/// A toggle whose summary holds `title` and the given extra attributes (`level` and the like).
fn toggle_with(id: u8, summary_attrs: &Value, title: &str) -> Value {
    let mut attrs = json!({ "id": s(bid(id + 1)) });
    if let (Some(target), Some(extra)) = (attrs.as_object_mut(), summary_attrs.as_object()) {
        target.extend(extra.clone());
    }
    json!({ "type": "details", "attrs": { "id": s(bid(id)), "open": false }, "content": [
        { "type": "detailsSummary", "attrs": attrs, "content": [text_node(title)] },
        { "type": "detailsContent", "attrs": { "id": s(bid(id + 2)) }, "content": [para(bid(id + 3), "body")] }
    ]})
}

#[test]
fn a_toggle_heading_is_a_known_node_with_no_new_name() {
    // `level` is an attribute of an existing node: the node lists do not change, and the
    // summary is still a text node whose text the core reads as it did.
    assert!(schema::NODE_NAMES.contains(&"detailsSummary"));
    assert!(schema::ID_BEARING.contains(&"detailsSummary"));
    assert!(schema::TEXT_NODES.contains(&"detailsSummary"));
    let plain = from_json(&doc_of(json!([toggle_with(1, &json!({}), "Title")])));
    let heading = from_json(&doc_of(json!([toggle_with(
        1,
        &json!({ "level": 2 }),
        "Title"
    )])));
    // The level is layout: the blocks (ids, kinds, text) are the same with or without it.
    assert_eq!(heading.blocks(), plain.blocks());
    assert_sound(&heading);
    // And it survives the portable round trip like any attribute.
    assert_eq!(
        heading.to_portable()["content"][0]["content"][0]["attrs"]["level"],
        2
    );
}

#[test]
fn check_invariants_checks_the_level_of_a_toggle_heading() {
    let page = from_json(&doc_of(json!([
        // Sound: no level (a plain toggle), 1, 2, 3 and an integral float.
        toggle_with(1, &json!({}), "a"),
        toggle_with(5, &json!({ "level": 1 }), "b"),
        toggle_with(9, &json!({ "level": 2 }), "c"),
        toggle_with(13, &json!({ "level": 3 }), "d"),
        toggle_with(17, &json!({ "level": 3.0 }), "e"),
        // Damaged: out of range, fractional, a string, a boolean.
        toggle_with(21, &json!({ "level": 0 }), "secret"),
        toggle_with(25, &json!({ "level": 4 }), "secret"),
        toggle_with(29, &json!({ "level": 1.5 }), "secret"),
        toggle_with(33, &json!({ "level": "2" }), "secret"),
        toggle_with(37, &json!({ "level": true }), "secret"),
    ])));
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /5/0: detailsSummary with a malformed level".to_owned(),
            "node /6/0: detailsSummary with a malformed level".to_owned(),
            "node /7/0: detailsSummary with a malformed level".to_owned(),
            "node /8/0: detailsSummary with a malformed level".to_owned(),
            "node /9/0: detailsSummary with a malformed level".to_owned(),
        ]
    );
}

#[test]
fn markdown_writes_a_toggle_heading_as_a_heading_in_the_summary() {
    // The clipboard flavour: the summary's text goes inside `<h1>` to `<h3>`; a toggle without a
    // level, or with a damaged one, is the plain toggle it was.
    let render = |level: Value| md(json!([toggle_with(1, &json!({ "level": level }), "Title")]));
    for level in 1..=3 {
        assert_eq!(
            render(json!(level)),
            format!(
                "<details>\n<summary><h{level}>Title</h{level}></summary>\n\nbody\n\n</details>\n"
            )
        );
    }
    // An integral float is the integer (the rule every integer attribute follows).
    assert_eq!(
        render(json!(2.0)),
        "<details>\n<summary><h2>Title</h2></summary>\n\nbody\n\n</details>\n"
    );
    let plain = "<details>\n<summary>Title</summary>\n\nbody\n\n</details>\n";
    assert_eq!(md(json!([toggle_with(1, &json!({}), "Title")])), plain);
    for damaged in [json!(0), json!(4), json!(1.5), json!("2"), json!(null)] {
        assert_eq!(render(damaged.clone()), plain, "{damaged}");
    }
}

#[test]
fn markdown_of_an_empty_toggle_heading_keeps_its_level() {
    let toggle = json!({ "type": "details", "attrs": { "id": s(bid(1)), "open": true }, "content": [
        { "type": "detailsSummary", "attrs": { "id": s(bid(2)), "level": 1 } },
        { "type": "detailsContent", "attrs": { "id": s(bid(3)) }, "content": [para(bid(4), "x")] }
    ]});
    assert_eq!(
        md(json!([toggle])),
        "<details open>\n<summary><h1></h1></summary>\n\nx\n\n</details>\n"
    );
}

#[test]
fn replace_attachment_points_blocks_and_gallery_entries_at_the_copy() {
    let page = from_json(&doc_of(json!([
        file(bid(1), &aid(1).to_string()),
        { "type": "bulletList", "attrs": { "id": s(bid(2)) }, "content": [
            { "type": "listItem", "attrs": { "id": s(bid(3)) }, "content": [
                para(bid(4), "with a picture"),
                image(bid(5), &aid(1).to_string(), "nested")
            ]}
        ]},
        image(bid(6), &aid(2).to_string(), "another file"),
        gallery_with(
            7,
            &json!({ "columns": 3, "items": [item(1, Some("Sunrise")), item(2, None), item(1, None)] })
        ),
    ])));
    let before = page.doc().version();
    assert_eq!(page.replace_attachment(aid(1), aid(9)).unwrap(), 3);
    assert_ne!(page.doc().version(), before);
    assert_eq!(
        page.attachment_refs(),
        vec![
            (bid(1), aid(9)),
            (bid(5), aid(9)),
            (bid(6), aid(2)),
            (bid(7), aid(9)),
            (bid(7), aid(2)),
            (bid(7), aid(9)),
        ],
        "every reference moved, the others stayed, the order held"
    );
    // The captions and the other attributes of the blocks are untouched.
    let json = page.to_portable();
    assert!(json.to_string().contains("Sunrise"));
    assert!(json.to_string().contains("another file"));
    // Nothing refers to the old one any more: a second run changes nothing and writes nothing.
    let version = page.doc().version();
    assert_eq!(page.replace_attachment(aid(1), aid(8)).unwrap(), 0);
    assert_eq!(page.doc().version(), version);
    // And the change survives an export and an import of the document.
    let blob = page.doc().export_snapshot().unwrap();
    let again = PageDoc::from_doc(Doc::load(DocKind::Page, 4, Some(&blob), &[]).unwrap()).unwrap();
    assert_eq!(again.attachment_refs(), page.attachment_refs());
}
