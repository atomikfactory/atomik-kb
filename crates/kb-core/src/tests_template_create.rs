//! Making a page from a template (ADR-0056, `docs/architecture/templates.md` section 5): the request
//! end to end on one device. The copy under a parent at an index, the title tokens, the
//! placeholders of properties and mentions with the local day, time and account of the request, the
//! request's values, the checks that write nothing, relations and links into the subtree, the
//! default template of views, one undo step, the events, and a duplicate that keeps the flag.
//! (The roles in a shared space: `tests_sharing/template_create.rs`.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

use kb_storage::{DateMentionQueries, UpdateOrigin};
use kb_types::{
    ApiError, BlockId, ChangeSource, Clock, DateValue, ErrorCode, Event, OptionColor, PageId,
    PageInfo, PropertyConfig, PropertyDef, PropertyFilter, PropertyId, PropertyKind,
    PropertyOption, PropertyValue, Request, Response, SpaceId, TemplatePlaceholder,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_workspace, kit_create, outline, page, page_content, spaces, tree,
    write_text,
};

/// 2026-10-18 23:40 UTC: with the offset of the tests (+2 hours) it is already 2026-10-19 01:40
/// on the device's wall clock.
const NOW_MS: i64 = 1_792_366_800_000;
const OFFSET_SECONDS: i32 = 2 * 3_600;
const LOCAL_DAY: &str = "2026-10-19";
const LOCAL_TIME: &str = "01:40";

#[derive(Clone)]
struct TestClock {
    now_ms: Arc<AtomicI64>,
    offset_seconds: Arc<AtomicI32>,
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn local_offset_seconds(&self) -> i32 {
        self.offset_seconds.load(Ordering::SeqCst)
    }
}

/// A core whose clock stands at [`NOW_MS`] with the offset of the tests, with a workspace and one
/// space, and (when `kit`) the recovery kit that gives the account its identity.
fn world_with(kit: bool) -> (Fixture, TestClock, Core, SpaceId) {
    let fx = Fixture::new();
    let clock = TestClock {
        now_ms: Arc::new(AtomicI64::new(NOW_MS)),
        offset_seconds: Arc::new(AtomicI32::new(OFFSET_SECONDS)),
    };
    let mut deps = fx.deps(1, NOW_MS);
    deps.clock = Box::new(clock.clone());
    let mut core = Core::open(deps).unwrap();
    create_workspace(&mut core, "Home", Some("Personal"));
    if kit {
        kit_create(&mut core);
    }
    let space = spaces(&mut core)[0].id;
    core.drain_events();
    (fx, clock, core, space)
}

fn world() -> (Fixture, TestClock, Core, SpaceId) {
    world_with(true)
}

fn call(core: &mut Core, request: Request) -> Response {
    core.dispatch(request).unwrap()
}

fn create_from(
    core: &mut Core,
    template: PageId,
    parent: Option<PageId>,
    index: Option<u32>,
    values: Vec<(PropertyId, PropertyValue)>,
) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageCreateFromTemplate {
        template_id: template,
        parent_id: parent,
        index,
        values,
    })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn duplicate(core: &mut Core, page_id: PageId) -> PageInfo {
    match call(core, Request::PageDuplicate { page_id }) {
        Response::Page(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_template(core: &mut Core, page_id: PageId, template: bool) -> PageInfo {
    match call(core, Request::PageSetTemplate { page_id, template }) {
        Response::Page(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_placeholder(
    core: &mut Core,
    page_id: PageId,
    property_id: PropertyId,
    placeholder: TemplatePlaceholder,
) {
    call(
        core,
        Request::PageSetTemplatePlaceholder {
            page_id,
            property_id,
            placeholder: Some(placeholder),
        },
    );
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

/// Every node of portable content that has this type, in document order.
fn nodes_of<'a>(content: &'a Value, kind: &str) -> Vec<&'a Value> {
    fn visit<'a>(node: &'a Value, kind: &str, out: &mut Vec<&'a Value>) {
        if node["type"] == kind {
            out.push(node);
        }
        for child in node["content"].as_array().into_iter().flatten() {
            visit(child, kind, out);
        }
    }
    let mut out = Vec::new();
    visit(content, kind, &mut out);
    out
}

fn block_ids_of(content: &Value) -> BTreeSet<BlockId> {
    kb_model::portable_block_ids(content).into_iter().collect()
}

// ----- properties ----------------------------------------------------------------------------

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

/// The pages of the space that have a value of the property, from the derived rows.
fn with_value(core: &mut Core, space_id: SpaceId, property_id: PropertyId) -> BTreeSet<PageId> {
    match call(
        core,
        Request::PagesByProperty {
            space_id,
            property_id,
            filter: PropertyFilter::HasValue,
        },
    ) {
        Response::PageIds(ids) => ids.into_iter().collect(),
        other => panic!("unexpected {other:?}"),
    }
}

fn day(date: &str, time: Option<&str>) -> PropertyValue {
    PropertyValue::Date(DateValue {
        date: date.into(),
        time: time.map(Into::into),
    })
}

fn relation(pages: &[PageId]) -> PropertyValue {
    PropertyValue::Relation(pages.to_vec())
}

fn paired(def: &PropertyDef) -> PropertyId {
    match def.config {
        Some(PropertyConfig::Relation {
            paired_property_id: Some(back),
            ..
        }) => back,
        ref other => panic!("{other:?}"),
    }
}

/// Tasks(top); the template Standup under it, with Notes (that holds Deep) and Gone (in the
/// trash); Other under Tasks before the template. Standup is a template.
struct Fixture3 {
    tasks: PageId,
    other: PageId,
    template: PageId,
    notes: PageId,
    deep: PageId,
    gone: PageId,
}

fn make_template(core: &mut Core, space: SpaceId) -> Fixture3 {
    let tasks = create_page(core, space, None, "Tasks").id;
    let other = create_page(core, space, Some(tasks), "Other").id;
    let template = create_page(core, space, Some(tasks), "Standup {{date}} {{time}}").id;
    let notes = create_page(core, space, Some(template), "Notes of {{date}}").id;
    let deep = create_page(core, space, Some(notes), "Deep").id;
    let gone = create_page(core, space, Some(template), "Gone").id;
    write_text(core, template, "agenda zebra", 1);
    write_text(core, notes, "notes giraffe", 2);
    call(
        core,
        Request::PageSetIcon {
            page_id: template,
            icon: Some("\u{1F4CB}".into()),
        },
    );
    call(core, Request::PageTrash { page_id: gone });
    set_template(core, template, true);
    core.drain_events();
    Fixture3 {
        tasks,
        other,
        template,
        notes,
        deep,
        gone,
    }
}

// ----- the copy --------------------------------------------------------------------------------

#[test]
fn a_page_is_made_from_a_template_under_a_parent_at_an_index_with_resolved_titles() {
    let (_fx, _clock, mut core, space) = world();
    let t = make_template(&mut core, space);
    let live_before = spaces(&mut core)[0].page_count;
    assert_eq!(
        search(&mut core, "zebra"),
        BTreeSet::new(),
        "a template is hidden from search"
    );

    let made = create_from(&mut core, t.template, Some(t.tasks), Some(0), Vec::new()).unwrap();

    // The answer: a new page, first under Tasks, a page and not a template, titled with the local
    // day and time of the device (not the UTC ones), no " (copy)", the icon kept.
    assert_ne!(made.id, t.template);
    assert_eq!(made.title, format!("Standup {LOCAL_DAY} {LOCAL_TIME}"));
    assert!(!made.template && made.template_placeholders.is_empty());
    assert_eq!(made.parent_id, Some(t.tasks));
    assert_eq!(made.space_id, space);
    assert_eq!(made.icon.as_deref(), Some("\u{1F4CB}"));
    assert_eq!(made.created_at, NOW_MS);
    let copies = subtree(&mut core, space, made.id);
    assert_eq!(copies.len(), 3, "the trashed page is not copied");
    assert_eq!(
        outline(&tree(&mut core, space)),
        format!(
            "Tasks\n  Standup {LOCAL_DAY} {LOCAL_TIME}\n    Notes of {LOCAL_DAY}\n      Deep\n  Other\n  Standup {{{{date}}}} {{{{time}}}}\n    Notes of {{{{date}}}}\n      Deep\n"
        )
    );
    assert_eq!(
        spaces(&mut core)[0].page_count,
        live_before + 3,
        "the copies are live pages"
    );

    // The template is untouched: still a template, its titles with their tokens, its content.
    let original = page(&mut core, t.template);
    assert!(original.template);
    assert_eq!(original.title, "Standup {{date}} {{time}}");
    assert_eq!(page(&mut core, t.notes).title, "Notes of {{date}}");
    assert_eq!(
        texts_of(&page_content(&mut core, t.template)),
        ["agenda zebra"]
    );

    // The content is the template's, in new blocks; the copies are found by search, the template
    // is not.
    let copy_content = page_content(&mut core, copies[0]);
    assert_eq!(texts_of(&copy_content), ["agenda zebra"]);
    assert!(
        block_ids_of(&copy_content)
            .is_disjoint(&block_ids_of(&page_content(&mut core, t.template))),
        "no block id is shared with the template"
    );
    assert_eq!(
        texts_of(&page_content(&mut core, copies[1])),
        ["notes giraffe"]
    );
    assert_eq!(search(&mut core, "zebra"), BTreeSet::from([copies[0]]));
    assert_eq!(search(&mut core, "giraffe"), BTreeSet::from([copies[1]]));
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
    let _ = (t.other, t.deep, t.gone);
}

#[test]
fn the_top_copy_goes_to_the_top_level_last_or_clamped() {
    let (_fx, _clock, mut core, space) = world();
    let t = make_template(&mut core, space);

    // No parent: the top level, last. No index under a parent: last. Past the end: last.
    let top = create_from(&mut core, t.template, None, None, Vec::new()).unwrap();
    assert_eq!(top.parent_id, None);
    let last = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let clamped = create_from(&mut core, t.template, Some(t.tasks), Some(99), Vec::new()).unwrap();
    let view = tree(&mut core, space);
    assert_eq!(
        view.roots.iter().map(|n| n.page.id).collect::<Vec<_>>(),
        [t.tasks, top.id]
    );
    assert_eq!(
        view.roots[0]
            .children
            .iter()
            .map(|n| n.page.id)
            .collect::<Vec<_>>(),
        [t.other, t.template, last.id, clamped.id]
    );
    // In the middle.
    let middle = create_from(&mut core, t.template, Some(t.tasks), Some(1), Vec::new()).unwrap();
    assert_eq!(
        tree(&mut core, space).roots[0]
            .children
            .iter()
            .map(|n| n.page.id)
            .collect::<Vec<_>>(),
        [t.other, middle.id, t.template, last.id, clamped.id]
    );
    // A template can be used under a page of a template-less branch and under another template's
    // sub-page: the parent only has to be live and outside the copied subtree.
    let under_copy = create_from(&mut core, t.template, Some(middle.id), None, Vec::new()).unwrap();
    assert_eq!(under_copy.parent_id, Some(middle.id));
}

// ----- the values ------------------------------------------------------------------------------

#[test]
fn placeholders_resolve_with_the_local_day_the_local_time_and_the_account_of_the_request() {
    let (_fx, clock, mut core, space) = world();
    let status = prop_create(&mut core, space, "Status", PropertyKind::Select, None);
    let doing = option_add(&mut core, space, status.id, "Doing");
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date, None);
    let seen = prop_create(&mut core, space, "Seen", PropertyKind::Date, None);
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person, None);
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number, None);
    let t = make_template(&mut core, space);
    set_value(
        &mut core,
        t.template,
        status.id,
        PropertyValue::Select(doing.id),
    );
    set_value(&mut core, t.deep, cost.id, PropertyValue::Number(7.0));
    set_placeholder(&mut core, t.template, due.id, TemplatePlaceholder::Today);
    set_placeholder(&mut core, t.notes, seen.id, TemplatePlaceholder::Now);
    set_placeholder(&mut core, t.notes, owner.id, TemplatePlaceholder::Me);
    let me = core.me().unwrap();
    core.drain_events();

    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let copies = subtree(&mut core, space, made.id);

    assert_eq!(
        value_of(&mut core, copies[0], status.id),
        Some(PropertyValue::Select(doing.id)),
        "a stored value is copied"
    );
    assert_eq!(
        value_of(&mut core, copies[2], cost.id),
        Some(PropertyValue::Number(7.0))
    );
    assert_eq!(
        value_of(&mut core, copies[0], due.id),
        Some(day(LOCAL_DAY, None)),
        "today: the local day, not the UTC one"
    );
    assert_eq!(
        value_of(&mut core, copies[1], seen.id),
        Some(day(LOCAL_DAY, Some(LOCAL_TIME))),
        "now on a sub-page"
    );
    assert_eq!(
        value_of(&mut core, copies[1], owner.id),
        Some(PropertyValue::Person(vec![me])),
        "me: the account of the device"
    );
    assert!(
        made.properties
            .iter()
            .any(|(id, v)| *id == due.id && *v == day(LOCAL_DAY, None)),
        "the answer carries the resolved values"
    );
    // The derived rows follow: the resolved values are found by the property queries.
    assert_eq!(
        with_value(&mut core, space, due.id),
        BTreeSet::from([copies[0]])
    );
    assert_eq!(
        with_value(&mut core, space, seen.id),
        BTreeSet::from([copies[1]])
    );
    assert_eq!(
        with_value(&mut core, space, owner.id),
        BTreeSet::from([copies[1]])
    );
    // The copies carry no placeholders and the template keeps its own.
    assert!(page(&mut core, copies[1]).template_placeholders.is_empty());
    assert_eq!(page(&mut core, t.notes).template_placeholders.len(), 2);
    assert_eq!(value_of(&mut core, t.template, due.id), None);

    // The moment is the request's: a later one gives later values.
    clock.now_ms.fetch_add(3_600_000, Ordering::SeqCst);
    let later = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    assert_eq!(later.title, format!("Standup {LOCAL_DAY} 02:40"));
    // And the offset is the device's: with another one, the day is another.
    clock.offset_seconds.store(-5 * 3_600, Ordering::SeqCst);
    let west = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    assert_eq!(west.title, "Standup 2026-10-18 19:40");
    let west_copies = subtree(&mut core, space, west.id);
    assert_eq!(
        value_of(&mut core, west_copies[0], due.id),
        Some(day("2026-10-18", None))
    );
    assert_eq!(
        value_of(&mut core, west_copies[1], seen.id),
        Some(day("2026-10-18", Some("19:40")))
    );
}

#[test]
fn without_an_account_a_me_placeholder_gives_no_value_and_the_rest_still_resolves() {
    let (_fx, _clock, mut core, space) = world_with(false);
    assert!(core.me().is_err(), "no recovery kit yet");
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date, None);
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person, None);
    let t = make_template(&mut core, space);
    set_placeholder(&mut core, t.template, due.id, TemplatePlaceholder::Now);
    set_placeholder(&mut core, t.template, owner.id, TemplatePlaceholder::Me);

    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    assert_eq!(
        value_of(&mut core, made.id, due.id),
        Some(day(LOCAL_DAY, Some(LOCAL_TIME)))
    );
    assert_eq!(value_of(&mut core, made.id, owner.id), None);
}

#[test]
fn the_values_of_the_request_win_over_placeholders_and_go_on_the_top_copy_only() {
    let (_fx, _clock, mut core, space) = world();
    let status = prop_create(&mut core, space, "Status", PropertyKind::Select, None);
    let doing = option_add(&mut core, space, status.id, "Doing");
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date, None);
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person, None);
    let t = make_template(&mut core, space);
    set_placeholder(&mut core, t.template, due.id, TemplatePlaceholder::Today);
    set_placeholder(&mut core, t.notes, due.id, TemplatePlaceholder::Today);
    set_placeholder(&mut core, t.template, owner.id, TemplatePlaceholder::Me);
    let me = core.me().unwrap();

    let made = create_from(
        &mut core,
        t.template,
        Some(t.tasks),
        None,
        vec![
            (status.id, PropertyValue::Select(doing.id)),
            (due.id, day("2031-02-03", None)),
        ],
    )
    .unwrap();
    let copies = subtree(&mut core, space, made.id);
    assert_eq!(
        value_of(&mut core, copies[0], status.id),
        Some(PropertyValue::Select(doing.id))
    );
    assert_eq!(
        value_of(&mut core, copies[0], due.id),
        Some(day("2031-02-03", None)),
        "the request's value wins over today"
    );
    assert_eq!(
        value_of(&mut core, copies[0], owner.id),
        Some(PropertyValue::Person(vec![me])),
        "a placeholder the request does not override still resolves"
    );
    assert_eq!(value_of(&mut core, copies[1], status.id), None);
    assert_eq!(
        value_of(&mut core, copies[1], due.id),
        Some(day(LOCAL_DAY, None)),
        "the sub-page's placeholder is not overridden"
    );
}

#[test]
fn relations_inside_the_subtree_follow_the_copies_and_the_others_stay_paired() {
    let (_fx, _clock, mut core, space) = world();
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
    let blocked_by = paired(&blocks);
    let t = make_template(&mut core, space);
    let outside = create_page(&mut core, space, None, "Outside").id;
    // The template blocks Outside; Notes block Deep (inside the subtree).
    set_value(&mut core, t.template, blocks.id, relation(&[outside]));
    set_value(&mut core, t.notes, blocks.id, relation(&[t.deep]));

    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let copies = subtree(&mut core, space, made.id);
    assert_eq!(
        value_of(&mut core, copies[0], blocks.id),
        Some(relation(&[outside]))
    );
    assert_eq!(
        value_of(&mut core, outside, blocked_by),
        Some(relation(&[t.template, copies[0]])),
        "the outside page is blocked by both"
    );
    assert_eq!(
        value_of(&mut core, copies[1], blocks.id),
        Some(relation(&[copies[2]])),
        "inside: the copy of Deep"
    );
    assert_eq!(
        value_of(&mut core, copies[2], blocked_by),
        Some(relation(&[copies[1]]))
    );
    assert_eq!(
        value_of(&mut core, t.deep, blocked_by),
        Some(relation(&[t.notes])),
        "the template's own sides are unchanged"
    );
    // The derived rows of the paired side were written too, for the outside page as well.
    assert_eq!(
        with_value(&mut core, space, blocked_by),
        BTreeSet::from([outside, t.deep, copies[2]])
    );
    assert_eq!(
        with_value(&mut core, space, blocks.id),
        BTreeSet::from([t.template, t.notes, copies[0], copies[1]])
    );

    // A relation among the request's values pairs the other side too.
    let again = create_from(
        &mut core,
        t.template,
        Some(t.tasks),
        None,
        vec![(blocks.id, relation(&[t.other]))],
    )
    .unwrap();
    assert_eq!(
        value_of(&mut core, again.id, blocks.id),
        Some(relation(&[t.other]))
    );
    assert_eq!(
        value_of(&mut core, t.other, blocked_by),
        Some(relation(&[again.id]))
    );
}

// ----- the content -----------------------------------------------------------------------------

#[test]
fn relative_mentions_resolve_links_and_default_templates_follow_and_buttons_are_left_alone() {
    let (_fx, _clock, mut core, space) = world();
    let t = make_template(&mut core, space);
    call(
        &mut core,
        Request::ProfileSet {
            display_name: "Grace".into(),
        },
    );
    let me = core.me().unwrap();
    let other_person = kb_types::AccountId::from_bytes([9; 16]);
    append_blocks(
        &mut core,
        t.template,
        json!([
            { "type": "paragraph", "attrs": { "id": bid(1).to_string() }, "content": [
                { "type": "text", "text": "Due " },
                { "type": "dateMention", "attrs": { "id": bid(2).to_string(), "date": "2020-01-01", "relative": "today", "reminder": 15 } },
                { "type": "dateMention", "attrs": { "id": bid(3).to_string(), "date": "2020-01-01", "time": "09:00", "relative": "now" } },
                { "type": "dateMention", "attrs": { "id": bid(4).to_string(), "date": "2021-05-06" } },
                { "type": "personMention", "attrs": { "accountId": other_person.to_string(), "label": "Ada", "relative": "me" } },
                { "type": "personMention", "attrs": { "accountId": other_person.to_string(), "label": "Ada" } },
                { "type": "pageRef", "attrs": { "id": bid(5).to_string(), "pageId": t.notes.to_string() } },
            ]},
            { "type": "templateButton", "attrs": { "id": bid(6).to_string(), "label": "Add a note" }, "content": [
                { "type": "paragraph", "attrs": { "id": bid(7).to_string() }, "content": [
                    { "type": "dateMention", "attrs": { "id": bid(8).to_string(), "date": "2020-01-01", "relative": "today" } },
                    { "type": "personMention", "attrs": { "accountId": other_person.to_string(), "label": "Ada", "relative": "me" } },
                ]}
            ]},
            { "type": "pageList", "attrs": { "id": bid(9).to_string(),
                "source": { "kind": "page", "pageId": t.notes.to_string() },
                "views": [
                { "id": "default", "layout": "list", "defaultTemplate": t.notes.to_string() },
                { "id": "other", "layout": "table", "defaultTemplate": t.other.to_string() },
            ]}},
            // The other linked sources (ADR-0058): a page outside the subtree and the space are kept.
            { "type": "pageList", "attrs": { "id": bid(10).to_string(),
                "source": { "kind": "page", "pageId": t.other.to_string() } } },
            { "type": "pageList", "attrs": { "id": bid(11).to_string(), "source": { "kind": "space" } } },
        ]),
    );

    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let copies = subtree(&mut core, space, made.id);
    let content = page_content(&mut core, made.id);

    // The relative date mentions: the local day (and time), `relative` gone, the reminder kept.
    let dates = nodes_of(&content, "dateMention");
    assert_eq!(dates.len(), 4, "three in the text and one in the button");
    assert_eq!(dates[0]["attrs"]["date"], LOCAL_DAY);
    assert!(dates[0]["attrs"].get("relative").is_none());
    assert_eq!(dates[0]["attrs"]["reminder"], 15);
    assert_eq!(dates[1]["attrs"]["date"], LOCAL_DAY);
    assert_eq!(dates[1]["attrs"]["time"], LOCAL_TIME);
    assert!(dates[1]["attrs"].get("relative").is_none());
    assert_eq!(
        dates[2]["attrs"]["date"], "2021-05-06",
        "a plain mention stays"
    );
    // The person mentions: me, with my name; a plain one stays.
    let people = nodes_of(&content, "personMention");
    assert_eq!(people.len(), 3);
    assert_eq!(people[0]["attrs"]["accountId"], me.to_string());
    assert_eq!(people[0]["attrs"]["label"], "Grace");
    assert!(people[0]["attrs"].get("relative").is_none());
    assert_eq!(people[1]["attrs"]["accountId"], other_person.to_string());
    assert_eq!(people[1]["attrs"]["label"], "Ada");

    // The button and its content are copied as they are: relative mentions stay relative.
    let buttons = nodes_of(&content, "templateButton");
    assert_eq!(buttons.len(), 1);
    assert_eq!(buttons[0]["attrs"]["label"], "Add a note");
    assert_eq!(dates[3]["attrs"]["relative"], "today");
    assert_eq!(dates[3]["attrs"]["date"], "2020-01-01");
    assert_eq!(people[2]["attrs"]["relative"], "me");
    assert_eq!(people[2]["attrs"]["accountId"], other_person.to_string());
    assert_ne!(
        buttons[0]["attrs"]["id"],
        bid(6).to_string(),
        "new block ids inside it too"
    );

    // The link to Notes points to Notes' copy; the default template of the first view too, and the
    // one that points outside the subtree stays.
    let links = nodes_of(&content, "pageRef");
    assert_eq!(links[0]["attrs"]["pageId"], copies[1].to_string());
    let lists = nodes_of(&content, "pageList");
    let views = lists[0]["attrs"]["views"].as_array().unwrap();
    assert_eq!(views[0]["defaultTemplate"], copies[1].to_string());
    assert_eq!(views[1]["defaultTemplate"], t.other.to_string());
    // The linked source inside the subtree follows too; the ones that point elsewhere stay.
    assert_eq!(
        lists[0]["attrs"]["source"],
        json!({ "kind": "page", "pageId": copies[1].to_string() })
    );
    assert_eq!(
        lists[1]["attrs"]["source"],
        json!({ "kind": "page", "pageId": t.other.to_string() })
    );
    assert_eq!(lists[2]["attrs"]["source"], json!({ "kind": "space" }));

    // The reminders rows follow the resolved mentions: the new page's own, with the local day.
    let rows = core.db.date_mentions_of(made.id).unwrap();
    assert!(
        rows.iter()
            .any(|row| row.date == LOCAL_DAY && row.reminder_minutes == Some(15)),
        "{rows:?}"
    );
    assert!(rows.iter().all(|row| row.date != "2020-01-01"), "{rows:?}");
    assert_eq!(
        rows.iter().filter(|row| row.date == LOCAL_DAY).count(),
        2,
        "the button's mention is not a date mention of the page"
    );

    // The template is exactly as it was.
    let original = page_content(&mut core, t.template);
    let original_dates = nodes_of(&original, "dateMention");
    assert_eq!(original_dates[0]["attrs"]["relative"], "today");
    assert_eq!(original_dates[1]["attrs"]["relative"], "now");
}

// ----- the checks ------------------------------------------------------------------------------

#[test]
fn every_check_comes_first_and_nothing_is_written_when_one_fails() {
    let (_fx, _clock, mut core, space) = world();
    let status = prop_create(&mut core, space, "Status", PropertyKind::Select, None);
    let doing = option_add(&mut core, space, status.id, "Doing");
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number, None);
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person, None);
    let t = make_template(&mut core, space);
    let plain = create_page(&mut core, space, None, "Plain").id;
    let trashed_template = create_page(&mut core, space, None, "Old template").id;
    set_template(&mut core, trashed_template, true);
    call(
        &mut core,
        Request::PageTrash {
            page_id: trashed_template,
        },
    );
    let trashed_parent = create_page(&mut core, space, None, "Trashed parent").id;
    call(
        &mut core,
        Request::PageTrash {
            page_id: trashed_parent,
        },
    );
    let elsewhere = {
        let other_space = crate::test_support::create_space(&mut core, "Work").id;
        create_page(&mut core, other_space, None, "Elsewhere").id
    };
    core.drain_events();
    let before = outline(&tree(&mut core, space));
    let count = spaces(&mut core)[0].page_count;
    let version = core.docs.structures[&space].model.doc().version();

    let some = |at: PageId| Some(at);
    let refused = |label: &str, result: Result<PageInfo, ApiError>, expected: &str| {
        let err = result.unwrap_err();
        assert_eq!(field(err), expected, "{label}");
    };
    // The template: a page that is not one, one in the trash.
    refused(
        "not a template",
        create_from(&mut core, plain, None, None, Vec::new()),
        "templateId",
    );
    refused(
        "trashed template",
        create_from(&mut core, trashed_template, None, None, Vec::new()),
        "templateId",
    );
    refused(
        "a page below a template is not a template",
        create_from(&mut core, t.notes, None, None, Vec::new()),
        "templateId",
    );
    // The parent: unknown, in the trash, in another space, the template and a page inside it.
    refused(
        "unknown parent",
        create_from(
            &mut core,
            t.template,
            some(PageId::from_bytes([9; 16])),
            None,
            Vec::new(),
        ),
        "parentId",
    );
    refused(
        "trashed parent",
        create_from(
            &mut core,
            t.template,
            some(trashed_parent),
            None,
            Vec::new(),
        ),
        "parentId",
    );
    refused(
        "parent in another space",
        create_from(&mut core, t.template, some(elsewhere), None, Vec::new()),
        "parentId",
    );
    for inside in [t.template, t.notes, t.deep] {
        refused(
            "parent inside the template",
            create_from(&mut core, t.template, some(inside), None, Vec::new()),
            "parentId",
        );
    }
    // A page in the trash of the template's own subtree is not "in" it, but is in the trash.
    refused(
        "a trashed page of the template",
        create_from(&mut core, t.template, some(t.gone), None, Vec::new()),
        "parentId",
    );
    // The values: the wrong kind, an unknown option, an unknown property, a person who is not a
    // member; the good value before the bad one is not kept.
    let good = (cost.id, PropertyValue::Number(1.0));
    let bad: Vec<(PropertyId, PropertyValue)> = vec![
        (cost.id, PropertyValue::Text("x".into())),
        (
            status.id,
            PropertyValue::Select(kb_types::OptionId::from_random_bytes([7; 16])),
        ),
        (
            PropertyId::from_random_bytes([8; 16]),
            PropertyValue::Number(1.0),
        ),
        (
            owner.id,
            PropertyValue::Person(vec![kb_types::AccountId::from_bytes([5; 16])]),
        ),
        (cost.id, PropertyValue::Number(f64::NAN)),
    ];
    for refusal in bad {
        refused(
            "a value that PagePropertySet would refuse",
            create_from(
                &mut core,
                t.template,
                Some(t.tasks),
                None,
                vec![good.clone(), refusal],
            ),
            "values",
        );
    }
    let _ = doing;
    // An unknown template.
    assert_eq!(
        create_from(
            &mut core,
            PageId::from_bytes([9; 16]),
            None,
            None,
            Vec::new()
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );

    assert_eq!(
        outline(&tree(&mut core, space)),
        before,
        "nothing was created"
    );
    assert_eq!(spaces(&mut core)[0].page_count, count);
    assert_eq!(
        core.docs.structures[&space].model.doc().version(),
        version,
        "nothing was written"
    );
    assert!(core.drain_events().is_empty(), "and nothing announced");
}

#[test]
fn a_template_of_more_than_a_thousand_pages_is_refused_and_exactly_a_thousand_is_made() {
    let (_fx, _clock, mut core, space) = world();
    let top = create_page(&mut core, space, None, "Big");
    set_template(&mut core, top.id, true);
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
    core.drain_events();
    let full = outline(&tree(&mut core, space));
    assert_eq!(
        field(create_from(&mut core, top.id, None, None, Vec::new()).unwrap_err()),
        "templateId"
    );
    assert_eq!(outline(&tree(&mut core, space)), full);
    assert!(core.drain_events().is_empty());

    // One page fewer fits.
    core.structure_mut(space)
        .unwrap()
        .model
        .trash_page(batch[0].page_id, 6)
        .unwrap();
    core.persist_structure(space, None, UpdateOrigin::Local)
        .unwrap();
    let made = create_from(&mut core, top.id, None, None, Vec::new()).unwrap();
    assert_eq!(subtree(&mut core, space, made.id).len(), 1_000);
}

// ----- events, undo, restart -------------------------------------------------------------------

#[test]
fn the_events_are_those_of_a_duplicate() {
    let (_fx, _clock, mut core, space) = world();
    let t = make_template(&mut core, space);
    let linked = create_page(&mut core, space, None, "Linked").id;
    append_blocks(
        &mut core,
        t.template,
        json!([{ "type": "paragraph", "attrs": { "id": bid(1).to_string() }, "content": [
            { "type": "pageRef", "attrs": { "id": bid(2).to_string(), "pageId": linked.to_string() } },
        ]}]),
    );
    core.drain_events();

    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let copies = subtree(&mut core, space, made.id);
    let events = core.drain_events();

    assert_eq!(
        events[0],
        Event::StructureChanged { space_id: space },
        "the structure first"
    );
    assert!(events.contains(&Event::SpacesChanged));
    for copy in &copies {
        assert!(
            events.contains(&Event::PageContentChanged {
                page_id: *copy,
                source: ChangeSource::Core
            }),
            "a content event for each new page: {events:?}"
        );
    }
    assert!(
        events.contains(&Event::BacklinksChanged { page_id: linked }),
        "the page the copy links to gets a backlink: {events:?}"
    );
    assert_eq!(
        backlink_sources(&mut core, linked),
        BTreeSet::from([made.id]),
        "a link from a template is not a backlink, the copy's is"
    );
}

fn backlink_sources(core: &mut Core, page_id: PageId) -> BTreeSet<PageId> {
    match call(core, Request::BacklinksGet { page_id }) {
        Response::Backlinks(links) => links.into_iter().map(|link| link.page_id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_creation_is_one_undo_step_and_a_redo_brings_the_pages_and_their_values_back() {
    let (_fx, _clock, mut core, space) = world();
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date, None);
    let t = make_template(&mut core, space);
    set_placeholder(&mut core, t.notes, due.id, TemplatePlaceholder::Today);
    let original = outline(&tree(&mut core, space));
    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let copies = subtree(&mut core, space, made.id);
    let created = outline(&tree(&mut core, space));
    assert_ne!(original, created);

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
    // One undo removes the three copies; the template is still a template.
    let view = undo(&mut core, false);
    assert_eq!(outline(&view), original);
    for id in &copies {
        assert_eq!(
            core.dispatch(Request::PageGet { page_id: *id })
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }
    assert!(page(&mut core, t.template).template);
    // One redo brings them back, with their resolved values and content, and no flag.
    let view = undo(&mut core, true);
    assert_eq!(outline(&view), created);
    assert_eq!(
        value_of(&mut core, copies[1], due.id),
        Some(day(LOCAL_DAY, None))
    );
    assert_eq!(
        texts_of(&page_content(&mut core, copies[0])),
        ["agenda zebra"]
    );
    assert!(!page(&mut core, made.id).template);
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
}

#[test]
fn the_page_survives_a_restart_and_a_rebuild_of_the_indexes() {
    let (fx, _clock, mut core, space) = world();
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date, None);
    let t = make_template(&mut core, space);
    set_placeholder(&mut core, t.template, due.id, TemplatePlaceholder::Today);
    let made = create_from(&mut core, t.template, Some(t.tasks), None, Vec::new()).unwrap();
    let before = outline(&tree(&mut core, space));
    drop(core);

    let mut core = fx.open(2, NOW_MS + 5_000);
    assert_eq!(outline(&tree(&mut core, space)), before);
    assert_eq!(search(&mut core, "zebra"), BTreeSet::from([made.id]));
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(
        search(&mut core, "zebra"),
        BTreeSet::from([made.id]),
        "still the copy, never the template"
    );
    assert_eq!(
        value_of(&mut core, made.id, due.id),
        Some(day(LOCAL_DAY, None))
    );
    assert!(page(&mut core, t.template).template);
    assert!(!page(&mut core, made.id).template);
}

// ----- the duplicate keeps the flag ------------------------------------------------------------

#[test]
fn the_duplicate_of_a_template_is_a_template_with_its_placeholders_and_default_templates() {
    let (_fx, _clock, mut core, space) = world();
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date, None);
    let t = make_template(&mut core, space);
    set_placeholder(&mut core, t.template, due.id, TemplatePlaceholder::Today);
    set_placeholder(&mut core, t.notes, due.id, TemplatePlaceholder::Now);
    append_blocks(
        &mut core,
        t.template,
        json!([{ "type": "pageList", "attrs": { "id": bid(9).to_string(), "views": [
            { "id": "default", "layout": "list", "defaultTemplate": t.notes.to_string() },
        ]}}]),
    );

    let copy = duplicate(&mut core, t.template);

    assert!(copy.template, "the copy of a template is a template");
    assert_eq!(copy.title, "Standup {{date}} {{time}} (copy)");
    assert_eq!(
        copy.template_placeholders,
        vec![(due.id, TemplatePlaceholder::Today)]
    );
    let copies = subtree(&mut core, space, copy.id);
    let sub = page(&mut core, copies[1]);
    assert!(!sub.template, "a sub-page has no flag of its own");
    assert_eq!(sub.title, "Notes of {{date}}", "the tokens are kept");
    assert_eq!(
        sub.template_placeholders,
        vec![(due.id, TemplatePlaceholder::Now)]
    );
    assert_eq!(
        value_of(&mut core, copy.id, due.id),
        None,
        "a duplicate resolves nothing"
    );
    let lists = page_content(&mut core, copy.id);
    let views = nodes_of(&lists, "pageList")[0]["attrs"]["views"].clone();
    assert_eq!(
        views[0]["defaultTemplate"],
        copies[1].to_string(),
        "the default template inside the subtree follows the duplicate too"
    );
    // The duplicate of a page that is not a template is not one.
    let tasks_copy = duplicate(&mut core, t.tasks);
    assert!(!tasks_copy.template);
    // A page made from the duplicate of a template works like one from the template.
    let made = create_from(&mut core, copy.id, Some(t.tasks), None, Vec::new()).unwrap();
    assert!(!made.template);
    assert_eq!(
        made.title,
        format!("Standup {LOCAL_DAY} {LOCAL_TIME} (copy)")
    );
}
