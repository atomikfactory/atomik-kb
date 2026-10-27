//! Duplicating a page with its sub-pages (ADR-0040): the request end to end on one device, the
//! copied content, links, drawings, values and relations, the attachments the copies keep alive,
//! undo, the keys and the derived rows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use kb_storage::{
    AttachmentQueries, DateMentionQueries, DocQueries, SyncQueries, TextIndexQueries, UpdateOrigin,
};
use kb_types::attachments::CHUNK_SIZE;
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, BlockId, ChangeSource, DocId, ErrorCode, Event,
    OptionColor, PageFont, PageId, PageInfo, PropertyConfig, PropertyDef, PropertyFilter,
    PropertyId, PropertyKind, PropertyOption, PropertyValue, Request, Response, SpaceId,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_workspace, kit_create, outline, page, page_content, spaces, tree,
    write_text,
};

/// A core with a workspace and one space.
fn world() -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    core.drain_events();
    (fx, core, space)
}

fn call(core: &mut Core, request: Request) -> Response {
    core.dispatch(request).unwrap()
}

fn duplicate(core: &mut Core, page_id: PageId) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageDuplicate { page_id })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

/// The field an `InvalidInput` names.
fn field(err: ApiError) -> String {
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
    err.details.unwrap()["field"].as_str().unwrap().to_owned()
}

fn search(core: &mut Core, query: &str) -> BTreeSet<PageId> {
    match call(
        core,
        Request::Search {
            query: query.into(),
            space_id: None,
            scope: None,
            limit: Some(50),
            edited_after: None,
        },
    ) {
        Response::Search(hits) => hits.into_iter().map(|hit| hit.page_id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// The live pages of a space by title, depth first (titles repeat after a duplication).
fn ids_by_title(core: &mut Core, space: SpaceId) -> BTreeMap<String, Vec<PageId>> {
    fn walk(nodes: &[kb_types::PageNode], out: &mut BTreeMap<String, Vec<PageId>>) {
        for node in nodes {
            out.entry(node.page.title.clone())
                .or_default()
                .push(node.page.id);
            walk(&node.children, out);
        }
    }
    let mut out = BTreeMap::new();
    walk(&tree(core, space).roots, &mut out);
    out
}

/// The live subtree of a page, pre-order, from the tree.
fn subtree(core: &mut Core, space: SpaceId, top: PageId) -> Vec<PageId> {
    fn find(nodes: &[kb_types::PageNode], top: PageId) -> Option<&kb_types::PageNode> {
        nodes.iter().find_map(|node| {
            if node.page.id == top {
                Some(node)
            } else {
                find(&node.children, top)
            }
        })
    }
    fn collect(node: &kb_types::PageNode, out: &mut Vec<PageId>) {
        out.push(node.page.id);
        for child in &node.children {
            collect(child, out);
        }
    }
    let view = tree(core, space);
    let mut out = Vec::new();
    collect(find(&view.roots, top).unwrap(), &mut out);
    out
}

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

/// Appends portable blocks to a page's document, like an import or the MCP server does.
fn append_blocks(core: &mut Core, page_id: PageId, blocks: Value) {
    let appended = core
        .open_page(page_id)
        .unwrap()
        .model
        .append_portable(&json!({ "type": "doc", "content": blocks }))
        .unwrap();
    assert!(appended > 0);
    core.persist_page(page_id, None, UpdateOrigin::Core, ChangeSource::Core)
        .unwrap();
    core.page_doc_close(page_id).unwrap();
}

/// The text of the paragraphs of portable content, in order.
fn texts_of(content: &Value) -> Vec<String> {
    fn visit(node: &Value, out: &mut Vec<String>) {
        if node["type"] == "paragraph" || node["type"] == "heading" {
            let mut text = String::new();
            for child in node["content"].as_array().into_iter().flatten() {
                if let Some(part) = child["text"].as_str() {
                    text.push_str(part);
                }
            }
            if !text.is_empty() {
                out.push(text);
            }
        }
        for child in node["content"].as_array().into_iter().flatten() {
            visit(child, out);
        }
    }
    let mut out = Vec::new();
    visit(content, &mut out);
    out
}

fn block_ids_of(content: &Value) -> BTreeSet<BlockId> {
    kb_model::portable_block_ids(content).into_iter().collect()
}

/// Every `pageRef` and `pageLink` of portable content: its kind, page and block.
fn links_of(content: &Value) -> Vec<(String, String, Option<String>)> {
    fn visit(node: &Value, out: &mut Vec<(String, String, Option<String>)>) {
        let kind = node["type"].as_str().unwrap_or_default();
        if kind == "pageRef" || kind == "pageLink" {
            out.push((
                kind.to_owned(),
                node["attrs"]["pageId"].as_str().unwrap().to_owned(),
                node["attrs"]["blockId"].as_str().map(str::to_owned),
            ));
        }
        for child in node["content"].as_array().into_iter().flatten() {
            visit(child, out);
        }
    }
    let mut out = Vec::new();
    visit(content, &mut out);
    out
}

fn linked_pages(core: &Core, page_id: PageId) -> BTreeSet<PageId> {
    core.db
        .links_from(page_id)
        .unwrap()
        .into_iter()
        .map(|link| link.to_page)
        .collect()
}

fn backlink_sources(core: &mut Core, page_id: PageId) -> BTreeSet<PageId> {
    match call(core, Request::BacklinksGet { page_id }) {
        Response::Backlinks(links) => links.into_iter().map(|link| link.page_id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

// ----- properties --------------------------------------------------------------------------

fn prop_create(
    core: &mut Core,
    space_id: SpaceId,
    name: &str,
    kind: PropertyKind,
    config: Option<PropertyConfig>,
) -> PropertyDef {
    match call(
        core,
        Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config,
        },
    ) {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    }
}

fn option_add(
    core: &mut Core,
    space_id: SpaceId,
    property_id: PropertyId,
    name: &str,
) -> PropertyOption {
    match call(
        core,
        Request::PropertyOptionAdd {
            space_id,
            property_id,
            name: name.into(),
            color: OptionColor::Green,
            group: None,
        },
    ) {
        Response::PropertyOption(option) => option,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_value(core: &mut Core, page_id: PageId, property_id: PropertyId, value: PropertyValue) {
    call(
        core,
        Request::PagePropertySet {
            page_id,
            property_id,
            value: Some(value),
        },
    );
}

fn value_of(core: &mut Core, page_id: PageId, property_id: PropertyId) -> Option<PropertyValue> {
    match call(core, Request::PagePropertiesGet { page_id }) {
        Response::PageProperties(values) => values
            .values
            .into_iter()
            .find(|(id, _)| *id == property_id)
            .map(|(_, value)| value),
        other => panic!("unexpected {other:?}"),
    }
}

fn relation(pages: &[PageId]) -> PropertyValue {
    PropertyValue::Relation(pages.to_vec())
}

fn by_property(
    core: &mut Core,
    space_id: SpaceId,
    property_id: PropertyId,
    filter: PropertyFilter,
) -> BTreeSet<PageId> {
    match call(
        core,
        Request::PagesByProperty {
            space_id,
            property_id,
            filter,
        },
    ) {
        Response::PageIds(ids) => ids.into_iter().collect(),
        other => panic!("unexpected {other:?}"),
    }
}

// ----- attachments -------------------------------------------------------------------------

fn upload(core: &mut Core, space_id: SpaceId, name: &str, bytes: &[u8]) -> AttachmentInfo {
    let id = match call(
        core,
        Request::AttachmentBegin {
            space_id,
            name: name.into(),
            mime: "image/png".into(),
        },
    ) {
        Response::AttachmentStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    };
    for piece in bytes.chunks(CHUNK_SIZE / 3 + 1) {
        core.attachment_write(id, piece).unwrap();
    }
    match call(core, Request::AttachmentFinish { attachment_id: id }) {
        Response::Attachment(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn attachment(core: &mut Core, id: AttachmentId) -> Result<AttachmentInfo, ApiError> {
    match core.dispatch(Request::AttachmentGet { attachment_id: id })? {
        Response::Attachment(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn read_attachment(core: &mut Core, id: AttachmentId) -> Vec<u8> {
    let mut out = Vec::new();
    core.attachment_read(id, None, &mut |piece| {
        out.extend_from_slice(piece);
        Ok(())
    })
    .unwrap();
    out
}

/// The attributes of the first `image` block of portable content.
fn image_attrs(content: &Value) -> Value {
    content["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "image")
        .unwrap()["attrs"]
        .clone()
}

fn image_block(id: u8, attachment: AttachmentId) -> Value {
    json!({ "type": "image", "attrs": {
        "id": bid(id).to_string(),
        "attachmentId": attachment.to_string(),
        "alt": "a cat"
    }})
}

// ----- the copy ----------------------------------------------------------------------------

#[test]
fn a_page_is_copied_with_its_live_subpages_after_it_with_new_ids_and_the_same_content() {
    let (_fx, mut core, space) = world();
    let before = create_page(&mut core, space, None, "Before");
    let top = create_page(&mut core, space, None, "Roadmap");
    let q1 = create_page(&mut core, space, Some(top.id), "Quarter one");
    let week = create_page(&mut core, space, Some(q1.id), "Week two");
    let q2 = create_page(&mut core, space, Some(top.id), "Quarter two");
    let old = create_page(&mut core, space, Some(top.id), "Old");
    let after = create_page(&mut core, space, None, "After");
    write_text(&mut core, top.id, "quarterly roadmap zebra", 1);
    write_text(&mut core, q1.id, "milestones giraffe", 2);
    write_text(&mut core, week.id, "tiny tasks", 3);
    call(
        &mut core,
        Request::PageSetIcon {
            page_id: top.id,
            icon: Some("\u{1F680}".into()),
        },
    );
    call(
        &mut core,
        Request::PageSetCover {
            page_id: top.id,
            cover: Some("gradient:sunset".into()),
            cover_y: Some(30),
        },
    );
    call(
        &mut core,
        Request::PageSetCover {
            page_id: q1.id,
            cover: Some("color:blue".into()),
            cover_y: None,
        },
    );
    call(
        &mut core,
        Request::PageSetAppearance {
            page_id: top.id,
            font: Some(PageFont::Serif),
            small_text: Some(true),
            locked: Some(true),
        },
    );
    call(
        &mut core,
        Request::PageSetAppearance {
            page_id: q1.id,
            font: Some(PageFont::Mono),
            small_text: None,
            locked: None,
        },
    );
    call(
        &mut core,
        Request::PageFavoriteSet {
            page_id: top.id,
            favorite: true,
        },
    );
    call(&mut core, Request::PageTrash { page_id: old.id });
    let live_before = spaces(&mut core)[0].page_count;
    core.drain_events();

    let copy = duplicate(&mut core, top.id).unwrap();

    // The answer: the copy of the page, right after it, named as a copy, with its icon.
    assert_ne!(copy.id, top.id);
    assert_eq!(copy.title, "Roadmap (copy)");
    assert_eq!(copy.icon.as_deref(), Some("\u{1F680}"));
    assert_eq!(
        (copy.cover.as_deref(), copy.cover_y),
        (Some("gradient:sunset"), Some(30)),
        "the cover and its position are copied"
    );
    assert_eq!(
        (copy.font, copy.small_text, copy.locked),
        (Some(PageFont::Serif), true, true),
        "the font, the small text and the lock are copied"
    );
    assert_eq!(copy.parent_id, None);
    assert_eq!(copy.space_id, space);
    assert_eq!(page(&mut core, copy.id), copy);
    assert_eq!(
        outline(&tree(&mut core, space)),
        "Before\nRoadmap\n  Quarter one\n    Week two\n  Quarter two\nRoadmap (copy)\n  \
         Quarter one\n    Week two\n  Quarter two\nAfter\n"
    );
    let view = tree(&mut core, space);
    assert_eq!(
        view.trashed.len(),
        1,
        "the trashed page stays in the trash, once"
    );
    assert_eq!(view.trashed[0].page.id, old.id);
    assert_eq!(view.favorites, vec![top.id], "favourites are not copied");
    assert_eq!(spaces(&mut core)[0].page_count, live_before + 4);

    // The sub-pages: new ids, the same titles, under the copies of their parents.
    let copies = subtree(&mut core, space, copy.id);
    let originals = subtree(&mut core, space, top.id);
    assert_eq!(copies.len(), 4);
    assert_eq!(originals, vec![top.id, q1.id, week.id, q2.id]);
    for (original, copied) in originals.iter().zip(&copies) {
        assert_ne!(original, copied);
        let (o, c) = (page(&mut core, *original), page(&mut core, *copied));
        if *original != top.id {
            assert_eq!(o.title, c.title);
        }
        assert_eq!(o.icon.is_some(), c.icon.is_some());
        assert_eq!((o.cover, o.cover_y), (c.cover, c.cover_y), "sub-pages too");
        assert_eq!(
            (o.font, o.small_text, o.locked),
            (c.font, c.small_text, c.locked),
            "sub-pages too"
        );
    }
    assert_eq!(page(&mut core, copies[1]).parent_id, Some(copies[0]));
    assert_eq!(page(&mut core, copies[2]).parent_id, Some(copies[1]));
    assert_eq!(page(&mut core, copies[3]).parent_id, Some(copies[0]));
    assert_eq!(page(&mut core, before.id).parent_id, None);
    assert_eq!(page(&mut core, after.id).parent_id, None);

    // The content: the same text, new block ids, nothing shared.
    for (original, copied) in originals.iter().zip(&copies) {
        let (o, c) = (
            page_content(&mut core, *original),
            page_content(&mut core, *copied),
        );
        assert_eq!(texts_of(&o), texts_of(&c));
        assert_eq!(block_ids_of(&o).len(), block_ids_of(&c).len());
        assert!(
            block_ids_of(&o).is_disjoint(&block_ids_of(&c)),
            "a copy shares no block id with its original"
        );
    }
    assert_eq!(
        texts_of(&page_content(&mut core, copies[0]))[0],
        "quarterly roadmap zebra"
    );

    // The derived rows are written as for any saved page: the search finds both, and a rebuild from
    // the documents changes nothing.
    let hits = |core: &mut Core| {
        (
            search(core, "zebra"),
            search(core, "giraffe"),
            search(core, "tiny"),
            search(core, "Roadmap"),
        )
    };
    let found = hits(&mut core);
    assert_eq!(found.0, BTreeSet::from([top.id, copies[0]]));
    assert_eq!(found.1, BTreeSet::from([q1.id, copies[1]]));
    assert_eq!(found.2, BTreeSet::from([week.id, copies[2]]));
    assert_eq!(found.3, BTreeSet::from([top.id, copies[0]]));
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(hits(&mut core), found);
    assert_eq!(
        kb_storage::DocQueries::pages_index_live_count(&core.db, space).unwrap(),
        live_before + 4
    );
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );

    // The events.
    let events = core.drain_events();
    assert!(events.contains(&Event::StructureChanged { space_id: space }));
    assert!(events.contains(&Event::SpacesChanged));
    for id in &copies {
        assert!(
            events.contains(&Event::PageContentChanged {
                page_id: *id,
                source: ChangeSource::Core
            }),
            "{events:?}"
        );
    }
}

#[test]
fn a_copy_goes_after_its_page_among_the_siblings_of_any_parent() {
    let (_fx, mut core, space) = world();
    let parent = create_page(&mut core, space, None, "Parent");
    let a = create_page(&mut core, space, Some(parent.id), "A");
    create_page(&mut core, space, Some(parent.id), "B");
    create_page(&mut core, space, None, "Last");
    duplicate(&mut core, a.id).unwrap();
    assert_eq!(
        outline(&tree(&mut core, space)),
        "Parent\n  A\n  A (copy)\n  B\nLast\n"
    );
    // The copy of a copy.
    let copy = ids_by_title(&mut core, space)["A (copy)"][0];
    let again = duplicate(&mut core, copy).unwrap();
    assert_eq!(again.title, "A (copy) (copy)");
    assert_eq!(
        outline(&tree(&mut core, space)),
        "Parent\n  A\n  A (copy)\n  A (copy) (copy)\n  B\nLast\n"
    );
    assert_eq!(again.parent_id, Some(parent.id));
}

#[test]
fn the_copys_title_fits_the_limit_and_an_untitled_page_becomes_the_copy() {
    let (_fx, mut core, space) = world();
    let long = create_page(&mut core, space, None, &"x".repeat(497));
    let copy = duplicate(&mut core, long.id).unwrap();
    assert_eq!(copy.title.chars().count(), 500);
    assert!(
        copy.title.ends_with("xxx (copy)"),
        "the title is cut, the suffix stays"
    );
    let untitled = create_page(&mut core, space, None, "");
    assert_eq!(duplicate(&mut core, untitled.id).unwrap().title, "(copy)");
    let exact = create_page(&mut core, space, None, &"y".repeat(493));
    assert_eq!(
        duplicate(&mut core, exact.id).unwrap().title,
        format!("{} (copy)", "y".repeat(493)),
        "a title that still fits is kept whole"
    );
}

#[test]
fn a_page_without_a_document_is_copied_as_an_empty_page_and_creates_nothing_for_the_original() {
    let (_fx, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Top");
    // A page an import has not reached has no document yet.
    let child = PageId::from_bytes([0x31; 16]);
    core.structure_mut(space)
        .unwrap()
        .model
        .create_pages(
            &[kb_model::NewPage {
                page_id: child,
                parent: Some(top.id),
                title: "Child".into(),
                icon: None,
                cover: None,
                cover_y: None,
            }],
            5,
        )
        .unwrap();
    core.persist_structure(space, None, UpdateOrigin::Local)
        .unwrap();
    assert!(core.db.doc_get(DocId::for_page(child)).unwrap().is_none());

    let copy = duplicate(&mut core, top.id).unwrap();
    let copies = subtree(&mut core, space, copy.id);
    assert_eq!(copies.len(), 2);
    for id in &copies {
        assert!(
            core.db.doc_get(DocId::for_page(*id)).unwrap().is_some(),
            "every copy has its document"
        );
        assert_eq!(
            texts_of(&page_content(&mut core, *id)),
            Vec::<String>::new()
        );
    }
    assert!(
        core.db.doc_get(DocId::for_page(child)).unwrap().is_none(),
        "the original still has none"
    );
}

#[test]
fn date_mentions_and_their_reminders_are_copied_with_new_node_ids() {
    let (_fx, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Plan");
    append_blocks(
        &mut core,
        top.id,
        json!([{ "type": "paragraph", "attrs": { "id": bid(1).to_string() }, "content": [
            { "type": "text", "text": "Call " },
            { "type": "dateMention", "attrs": { "id": bid(2).to_string(), "date": "2026-10-20",
                "time": "14:30", "reminder": 15 } },
            { "type": "dateMention", "attrs": { "id": bid(3).to_string(), "date": "2026-10-21" } },
        ]}]),
    );
    let original = core.db.date_mentions_of(top.id).unwrap();
    assert_eq!(original.len(), 2);

    let copy = duplicate(&mut core, top.id).unwrap();
    let copied = core.db.date_mentions_of(copy.id).unwrap();
    assert_eq!(copied.len(), 2);
    for (o, c) in original.iter().zip(&copied) {
        assert_ne!(o.block_id, c.block_id);
        assert_ne!(o.node_id, c.node_id);
        assert_eq!(
            (&o.date, &o.time, o.reminder_minutes, o.fire_at_local),
            (&c.date, &c.time, c.reminder_minutes, c.fire_at_local),
            "the day, the time and the reminder are the original's"
        );
    }
    assert_eq!(copied[0].reminder_minutes, Some(15));
    assert_eq!(copied[1].reminder_minutes, None);
    assert_eq!(core.db.date_mentions_of(top.id).unwrap(), original);
}

// ----- links, drawings ---------------------------------------------------------------------

#[test]
fn links_into_the_copied_subtree_follow_the_copies_and_the_others_stay() {
    let (_fx, mut core, space) = world();
    let outside = create_page(&mut core, space, None, "Outside");
    let a = create_page(&mut core, space, None, "A");
    let b = create_page(&mut core, space, Some(a.id), "B");
    // A: a paragraph with a chip to B and to Outside, a card to B at B's block, a card to A itself.
    // B: a paragraph with a chip to A at A's block.
    append_blocks(
        &mut core,
        b.id,
        json!([{ "type": "paragraph", "attrs": { "id": bid(21).to_string() }, "content": [
            { "type": "text", "text": "back to A " },
            { "type": "pageRef", "attrs": { "id": bid(22).to_string(), "pageId": a.id.to_string(),
                "blockId": bid(11).to_string() } },
        ]}]),
    );
    append_blocks(
        &mut core,
        a.id,
        json!([
            { "type": "paragraph", "attrs": { "id": bid(11).to_string() }, "content": [
                { "type": "text", "text": "see " },
                { "type": "pageRef", "attrs": { "id": bid(12).to_string(), "pageId": b.id.to_string() } },
                { "type": "pageRef", "attrs": { "id": bid(13).to_string(), "pageId": outside.id.to_string() } },
            ]},
            { "type": "pageLink", "attrs": { "id": bid(14).to_string(), "pageId": b.id.to_string(),
                "blockId": bid(21).to_string() } },
            { "type": "pageLink", "attrs": { "id": bid(15).to_string(), "pageId": a.id.to_string() } },
        ]),
    );
    assert_eq!(
        linked_pages(&core, a.id),
        BTreeSet::from([b.id, outside.id, a.id])
    );
    core.drain_events();

    let copy_a = duplicate(&mut core, a.id).unwrap().id;
    let copy_b = subtree(&mut core, space, copy_a)[1];

    // A's copy: the chip and the card point at B's copy (the card at the copy of B's block), the
    // chip to Outside is kept, the card to A points at the copy of A.
    let content_a = page_content(&mut core, copy_a);
    let content_b = page_content(&mut core, copy_b);
    let links = links_of(&content_a);
    assert_eq!(links.len(), 4);
    assert_eq!(links[0], ("pageRef".into(), copy_b.to_string(), None));
    assert_eq!(links[1], ("pageRef".into(), outside.id.to_string(), None));
    assert_eq!(links[2].0, "pageLink");
    assert_eq!(links[2].1, copy_b.to_string());
    let named = links[2].2.clone().unwrap();
    assert_ne!(
        named,
        bid(21).to_string(),
        "the card names the copy of the block"
    );
    assert!(
        block_ids_of(&content_b)
            .iter()
            .any(|id| id.to_string() == named),
        "and that block is in the copy of B"
    );
    assert_eq!(links[3], ("pageLink".into(), copy_a.to_string(), None));
    // B's copy: the chip points at A's copy, at the copy of A's block.
    let links = links_of(&content_b);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].1, copy_a.to_string());
    let named = links[0].2.clone().unwrap();
    assert_ne!(named, bid(11).to_string());
    assert!(
        block_ids_of(&content_a)
            .iter()
            .any(|id| id.to_string() == named)
    );

    // The originals are untouched.
    let original = links_of(&page_content(&mut core, a.id));
    assert_eq!(original[0].1, b.id.to_string());
    assert_eq!(original[2].2, Some(bid(21).to_string()));
    assert_eq!(
        linked_pages(&core, a.id),
        BTreeSet::from([b.id, outside.id, a.id])
    );

    // The link rows and the backlinks follow the copies.
    assert_eq!(
        linked_pages(&core, copy_a),
        BTreeSet::from([copy_b, outside.id, copy_a])
    );
    assert_eq!(linked_pages(&core, copy_b), BTreeSet::from([copy_a]));
    assert_eq!(
        backlink_sources(&mut core, outside.id),
        BTreeSet::from([a.id, copy_a])
    );
    assert_eq!(backlink_sources(&mut core, b.id), BTreeSet::from([a.id]));
    assert_eq!(
        backlink_sources(&mut core, copy_b),
        BTreeSet::from([copy_a])
    );
    assert_eq!(
        backlink_sources(&mut core, copy_a),
        BTreeSet::from([copy_a, copy_b])
    );
    let events = core.drain_events();
    assert!(
        events.contains(&Event::BacklinksChanged {
            page_id: outside.id
        }),
        "{events:?}"
    );
}

#[test]
fn a_drawing_follows_its_block_and_stays_the_originals() {
    let (_fx, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Sketch");
    append_blocks(
        &mut core,
        top.id,
        json!([{ "type": "whiteboard", "attrs": { "id": bid(7).to_string(), "height": 400 } }]),
    );
    {
        let open = core.open_page(top.id).unwrap();
        for (id, text) in [("t1", "zebra drawing"), ("r1", "")] {
            let kind = if text.is_empty() { "rectangle" } else { "text" };
            let mut element = json!({
                "id": id, "type": kind, "index": "a0", "version": 1, "versionNonce": 5,
                "isDeleted": false, "x": 1.5, "y": 2
            });
            if !text.is_empty() {
                element["text"] = json!(text);
                element["originalText"] = json!(text);
            }
            open.model.set_whiteboard_element(bid(7), &element).unwrap();
        }
    }
    core.persist_page(top.id, None, UpdateOrigin::Local, ChangeSource::Editor)
        .unwrap();
    core.page_doc_close(top.id).unwrap();
    assert_eq!(search(&mut core, "zebra"), BTreeSet::from([top.id]));

    let copy = duplicate(&mut core, top.id).unwrap();
    let original = page_content(&mut core, top.id);
    let copied = page_content(&mut core, copy.id);
    let boards = copied["whiteboards"].as_object().unwrap();
    assert_eq!(boards.len(), 1);
    let (key, elements) = boards.iter().next().unwrap();
    assert_ne!(
        key,
        &bid(7).to_string(),
        "the elements are under the copy's block"
    );
    assert!(
        block_ids_of(&copied)
            .iter()
            .any(|id| &id.to_string() == key)
    );
    assert_eq!(
        elements,
        &original["whiteboards"][bid(7).to_string()],
        "the same elements"
    );
    assert_eq!(original["whiteboards"].as_object().unwrap().len(), 1);
    // The text of the drawing is indexed for the copy as well.
    assert_eq!(
        search(&mut core, "zebra"),
        BTreeSet::from([top.id, copy.id])
    );
}

// ----- values ------------------------------------------------------------------------------

#[test]
fn values_and_two_way_relations_are_copied_through_the_value_path() {
    let (_fx, mut core, space) = world();
    let status = prop_create(&mut core, space, "Status", PropertyKind::Select, None);
    let doing = option_add(&mut core, space, status.id, "Doing");
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number, None);
    let blocks = prop_create(
        &mut core,
        space,
        "Blocks",
        PropertyKind::Relation,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: None,
        }),
    );
    let blocked_by = match blocks.config {
        Some(PropertyConfig::Relation {
            paired_property_id: Some(back),
            ..
        }) => back,
        ref other => panic!("{other:?}"),
    };
    let outside = create_page(&mut core, space, None, "Outside");
    let top = create_page(&mut core, space, None, "Project");
    let child = create_page(&mut core, space, Some(top.id), "Task");
    set_value(
        &mut core,
        top.id,
        status.id,
        PropertyValue::Select(doing.id),
    );
    set_value(&mut core, top.id, cost.id, PropertyValue::Number(3.5));
    set_value(&mut core, child.id, cost.id, PropertyValue::Number(1.0));
    // Project blocks Outside; Task blocks Project (both inside the copied subtree).
    set_value(&mut core, top.id, blocks.id, relation(&[outside.id]));
    set_value(&mut core, child.id, blocks.id, relation(&[top.id]));
    assert_eq!(
        value_of(&mut core, top.id, blocked_by),
        Some(relation(&[child.id]))
    );
    core.drain_events();

    let copy = duplicate(&mut core, top.id).unwrap();
    let copy_child = subtree(&mut core, space, copy.id)[1];

    // Plain values are copied (the page answer carries them too).
    assert_eq!(
        value_of(&mut core, copy.id, status.id),
        Some(PropertyValue::Select(doing.id))
    );
    assert!(
        copy.properties
            .contains(&(cost.id, PropertyValue::Number(3.5)))
    );
    assert_eq!(
        value_of(&mut core, copy_child, cost.id),
        Some(PropertyValue::Number(1.0))
    );
    // A relation to a page outside is kept, and that page lists both sides of the pair.
    assert_eq!(
        value_of(&mut core, copy.id, blocks.id),
        Some(relation(&[outside.id]))
    );
    assert_eq!(
        value_of(&mut core, outside.id, blocked_by),
        Some(relation(&[top.id, copy.id]))
    );
    // A relation inside the subtree relates the copies among themselves, from both sides.
    assert_eq!(
        value_of(&mut core, copy_child, blocks.id),
        Some(relation(&[copy.id]))
    );
    assert_eq!(
        value_of(&mut core, copy.id, blocked_by),
        Some(relation(&[copy_child]))
    );
    // The originals keep theirs, and are not related to the copies.
    assert_eq!(
        value_of(&mut core, top.id, blocked_by),
        Some(relation(&[child.id]))
    );
    assert_eq!(
        value_of(&mut core, child.id, blocks.id),
        Some(relation(&[top.id]))
    );

    // The derived rows: queries by value find the copies, the page answer and a rebuild agree.
    let by_cost = |core: &mut Core| by_property(core, space, cost.id, PropertyFilter::HasValue);
    assert_eq!(
        by_cost(&mut core),
        BTreeSet::from([top.id, child.id, copy.id, copy_child])
    );
    call(
        &mut core,
        Request::IndexRebuild {
            space_id: Some(space),
        },
    );
    assert_eq!(
        by_cost(&mut core),
        BTreeSet::from([top.id, child.id, copy.id, copy_child])
    );
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
}

// ----- refusals ----------------------------------------------------------------------------

#[test]
fn a_page_in_the_trash_unknown_or_too_large_is_refused_and_nothing_is_written() {
    let (_fx, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Top");
    let trashed = create_page(&mut core, space, None, "Trashed");
    let below = create_page(&mut core, space, Some(trashed.id), "Below");
    call(
        &mut core,
        Request::PageTrash {
            page_id: trashed.id,
        },
    );
    core.drain_events();
    let before = outline(&tree(&mut core, space));

    assert_eq!(
        field(duplicate(&mut core, trashed.id).unwrap_err()),
        "pageId"
    );
    assert_eq!(
        field(duplicate(&mut core, below.id).unwrap_err()),
        "pageId",
        "a page below a trashed one is in the trash too"
    );
    assert_eq!(
        duplicate(&mut core, PageId::from_bytes([9; 16]))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    // 1,001 pages (the page and 1,000 sub-pages) are one too many.
    let batch: Vec<kb_model::NewPage> = (0..1_000u32)
        .map(|n| {
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&n.to_le_bytes());
            bytes[15] = 0x77;
            kb_model::NewPage {
                page_id: PageId::from_random_bytes(bytes),
                parent: Some(top.id),
                title: format!("p{n}"),
                icon: None,
                cover: None,
                cover_y: None,
            }
        })
        .collect();
    core.structure_mut(space)
        .unwrap()
        .model
        .create_pages(&batch, 5)
        .unwrap();
    core.persist_structure(space, None, UpdateOrigin::Local)
        .unwrap();
    let full = outline(&tree(&mut core, space));
    assert_eq!(field(duplicate(&mut core, top.id).unwrap_err()), "pageId");
    assert_eq!(
        outline(&tree(&mut core, space)),
        full,
        "nothing was written"
    );
    assert!(core.drain_events().is_empty());
    assert_ne!(before, full);

    // One page fewer fits: 1,000 pages are copied.
    core.structure_mut(space)
        .unwrap()
        .model
        .trash_page(batch[0].page_id, 6)
        .unwrap();
    core.persist_structure(space, None, UpdateOrigin::Local)
        .unwrap();
    let copy = duplicate(&mut core, top.id).unwrap();
    assert_eq!(subtree(&mut core, space, copy.id).len(), 1_000);
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
}

// ----- attachments -------------------------------------------------------------------------

#[test]
fn attachments_stay_shared_and_the_copys_references_keep_them_alive() {
    let (fx, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Photos");
    let child = create_page(&mut core, space, Some(top.id), "More");
    let photo = upload(&mut core, space, "cat.png", &[7u8; 40]).attachment_id;
    let logo = upload(&mut core, space, "logo.png", &[9u8; 40]).attachment_id;
    let unused = upload(&mut core, space, "unused.png", &[1u8; 40]).attachment_id;
    let banner = upload(&mut core, space, "banner.png", &[5u8; 40]).attachment_id;
    append_blocks(&mut core, top.id, json!([image_block(1, photo)]));
    append_blocks(&mut core, child.id, json!([image_block(2, photo)]));
    call(
        &mut core,
        Request::PageSetIcon {
            page_id: top.id,
            icon: Some(format!("attachment:{logo}")),
        },
    );
    call(
        &mut core,
        Request::PageSetCover {
            page_id: top.id,
            cover: Some(format!("attachment:{banner}")),
            cover_y: Some(75),
        },
    );
    assert_eq!(attachment(&mut core, photo).unwrap().references, 2);
    assert_eq!(attachment(&mut core, logo).unwrap().references, 1);
    assert_eq!(attachment(&mut core, banner).unwrap().references, 1);

    let copy = duplicate(&mut core, top.id).unwrap();

    // The copies reference the same attachments: nothing is uploaded or duplicated.
    assert_eq!(copy.icon, Some(format!("attachment:{logo}")));
    assert_eq!(copy.cover, Some(format!("attachment:{banner}")));
    assert_eq!(copy.cover_y, Some(75));
    assert_eq!(attachment(&mut core, banner).unwrap().references, 2);
    assert_eq!(attachment(&mut core, photo).unwrap().references, 4);
    assert_eq!(attachment(&mut core, logo).unwrap().references, 2);
    let copies = subtree(&mut core, space, copy.id);
    let shown = image_attrs(&page_content(&mut core, copies[1]));
    assert_eq!(shown["attachmentId"], photo.to_string());
    assert_ne!(
        shown["id"],
        bid(2).to_string(),
        "the block has a new id, the attachment the same"
    );
    assert_eq!(
        core.db.attachment_referencing_pages(photo).unwrap().len(),
        4
    );
    assert_eq!(
        crate::test_support::spaces(&mut core).len(),
        1,
        "no space was created"
    );

    // The original goes away for good: the copies keep both attachments alive.
    call(&mut core, Request::PageTrash { page_id: top.id });
    call(&mut core, Request::PagePurge { page_id: top.id });
    let kept = attachment(&mut core, photo).unwrap();
    assert_eq!(kept.references, 2);
    assert_eq!(kept.unreferenced_since, None);
    assert_eq!(attachment(&mut core, logo).unwrap().references, 1);
    assert_eq!(
        attachment(&mut core, logo).unwrap().unreferenced_since,
        None
    );
    assert!(
        attachment(&mut core, unused)
            .unwrap()
            .unreferenced_since
            .is_some()
    );

    // A month later (past the default retention) the sweep removes the unreferenced file only.
    drop(core);
    let mut core = fx.open(2, 1_000 + 40 * 86_400_000);
    call(&mut core, Request::AttachmentGc);
    assert_eq!(
        attachment(&mut core, unused).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(read_attachment(&mut core, photo), vec![7u8; 40]);
    assert_eq!(read_attachment(&mut core, logo), vec![9u8; 40]);
    assert_eq!(
        image_attrs(&page_content(&mut core, copies[1]))["attachmentId"],
        photo.to_string(),
        "the copy still shows the picture"
    );
}

// ----- keys, sync, restart -----------------------------------------------------------------

#[test]
fn every_new_document_has_its_key_and_its_updates_reach_the_sync_outbox() {
    let (_fx, mut core, space) = world();
    kit_create(&mut core);
    core.sync_configure(Some("https://sync.example.test"), None)
        .unwrap();
    let top = create_page(&mut core, space, None, "Top");
    let child = create_page(&mut core, space, Some(top.id), "Child");
    write_text(&mut core, top.id, "some text", 1);
    write_text(&mut core, child.id, "more text", 2);
    core.drain_events();

    let copy = duplicate(&mut core, top.id).unwrap();
    let copies = subtree(&mut core, space, copy.id);

    for id in &copies {
        let doc = DocId::for_page(*id);
        let row = core.db.doc_get(doc).unwrap().unwrap();
        assert_eq!(row.space_id, Some(space), "the document is under the space");
        assert_eq!(
            core.security.keyring.doc_spaces(doc),
            vec![space],
            "its key record is under the space, as PageCreate leaves it"
        );
        let updates = core.db.updates_from(doc, 0).unwrap();
        assert!(
            updates
                .iter()
                .any(|(_, _, origin)| *origin != UpdateOrigin::Remote),
            "a local update the outbox can push"
        );
    }
    core.sync_feeds().unwrap();
    let personal = core.personal_feed().unwrap().unwrap();
    let outbox: BTreeSet<DocId> = core
        .sync_feed_outbox(personal)
        .unwrap()
        .into_iter()
        .map(|entry| entry.doc_id)
        .collect();
    for id in &copies {
        assert!(
            outbox.contains(&DocId::for_page(*id)),
            "the new document is in the outbox"
        );
    }
}

#[test]
fn the_copy_survives_a_restart_and_a_rebuild_of_the_indexes() {
    let (fx, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Notes");
    create_page(&mut core, space, Some(top.id), "Sub");
    write_text(&mut core, top.id, "remember the zebra", 1);
    let copy = duplicate(&mut core, top.id).unwrap();
    let outline_before = outline(&tree(&mut core, space));
    drop(core);

    let mut core = fx.open(2, 5_000);
    assert_eq!(outline(&tree(&mut core, space)), outline_before);
    assert_eq!(
        search(&mut core, "zebra"),
        BTreeSet::from([top.id, copy.id])
    );
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(
        search(&mut core, "zebra"),
        BTreeSet::from([top.id, copy.id])
    );
    assert_eq!(
        texts_of(&page_content(&mut core, copy.id)),
        ["remember the zebra"]
    );
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
}

// ----- undo --------------------------------------------------------------------------------

#[test]
fn a_duplication_is_one_undo_step_and_a_redo_brings_the_pages_and_their_content_back() {
    let (_fx, mut core, space) = world();
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number, None);
    let top = create_page(&mut core, space, None, "Top");
    let child = create_page(&mut core, space, Some(top.id), "Child");
    write_text(&mut core, top.id, "undo zebra", 1);
    set_value(&mut core, child.id, cost.id, PropertyValue::Number(4.0));
    let original = outline(&tree(&mut core, space));
    let copy = duplicate(&mut core, top.id).unwrap();
    let copies = subtree(&mut core, space, copy.id);
    let copied = outline(&tree(&mut core, space));
    assert_ne!(original, copied);
    let undo = |core: &mut Core, redo: bool| -> kb_types::TreeView {
        let request = if redo {
            Request::StructureRedo { space_id: space }
        } else {
            Request::StructureUndo { space_id: space }
        };
        match core.dispatch(request).unwrap() {
            Response::Tree(view) => view,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert!(tree(&mut core, space).undo.can_undo);

    // One undo removes the copy of the page and of its sub-page.
    let view = undo(&mut core, false);
    assert_eq!(outline(&view), original);
    assert!(
        view.undo.can_undo,
        "the steps before the duplication are still there"
    );
    assert!(view.undo.can_redo);
    for id in &copies {
        assert_eq!(
            core.dispatch(Request::PageGet { page_id: *id })
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }
    assert_eq!(search(&mut core, "zebra"), BTreeSet::from([top.id]));
    assert_eq!(search(&mut core, "Top"), BTreeSet::from([top.id]));
    assert_eq!(
        by_property(&mut core, space, cost.id, PropertyFilter::HasValue),
        BTreeSet::from([child.id])
    );

    // One redo brings both back, with the content and the values.
    let view = undo(&mut core, true);
    assert_eq!(outline(&view), copied);
    assert_eq!(
        search(&mut core, "zebra"),
        BTreeSet::from([top.id, copy.id])
    );
    assert_eq!(texts_of(&page_content(&mut core, copy.id)), ["undo zebra"]);
    assert_eq!(
        value_of(&mut core, copies[1], cost.id),
        Some(PropertyValue::Number(4.0))
    );
    assert_eq!(
        by_property(&mut core, space, cost.id, PropertyFilter::HasValue),
        BTreeSet::from([child.id, copies[1]])
    );
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
}
