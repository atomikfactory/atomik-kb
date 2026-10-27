//! Making a page from a template (ADR-0056, `docs/architecture/templates.md` section 5): the
//! copy under another parent at an index, no flag on the copies, the title tokens, the
//! placeholders resolved into values, the request's values, the refusals, one undo step, the
//! duplicate that keeps the flag; and the pure content pass (relative mentions, `defaultTemplate`).
//!
//! The flag and the placeholders are written with `set_page_template` and
//! `set_template_placeholder`; the raw map is written to the node meta where a test needs entries
//! the writers refuse.

use std::collections::{BTreeMap, HashMap};

use kb_doc::{Doc, DocKind};
use kb_types::{
    AccountId, DateValue, DeviceId, OptionColor, OptionId, PageId, PropertyId, PropertyKind,
    PropertyValue, TemplatePlaceholder,
};
use loro::{LoroMap, LoroValue};
use serde_json::{Value, json};

use super::{add, assert_healthy, outline, pid, replica, structure, sync};
use crate::meta;
use crate::{
    CopyIds, Duplicated, LocalMoment, ModelError, PageDoc, StructureDoc, TemplateCopy,
    TemplateFill, TemplateMe, fill_template_content, remap_portable,
};

fn device() -> DeviceId {
    DeviceId::from_random_bytes([0xe1, 7, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 7])
}

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xe2, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn opt(n: u8) -> OptionId {
    OptionId::from_random_bytes([0xe3, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn account(n: u8) -> AccountId {
    AccountId::from_bytes([0xe4, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

/// The id of the copy of `pid(n)`: `pid(n + 100)`.
fn copy(n: u8) -> PageId {
    pid(n + 100)
}

fn number_of(id: PageId) -> u8 {
    id.to_bytes()[0]
}

fn date(day: &str, time: Option<&str>) -> PropertyValue {
    PropertyValue::Date(DateValue {
        date: day.to_owned(),
        time: time.map(str::to_owned),
    })
}

/// 2026-10-18 13:05, the moment of the tests.
fn moment() -> LocalMoment {
    LocalMoment {
        date: "2026-10-18".into(),
        time: "13:05".into(),
    }
}

// ----- the node meta ---------------------------------------------------------------------------

fn meta_of(doc: &StructureDoc, page: PageId) -> LoroMap {
    doc.tree.get_meta(doc.index.node(page).unwrap()).unwrap()
}

fn mark_template(doc: &mut StructureDoc, page: PageId) {
    doc.set_page_template(page, true, 35).unwrap();
}

fn set_placeholder(
    doc: &mut StructureDoc,
    page: PageId,
    property: PropertyId,
    placeholder: TemplatePlaceholder,
) {
    doc.set_template_placeholder(page, property, Some(placeholder), 36)
        .unwrap();
}

fn is_flagged(doc: &StructureDoc, page: PageId) -> bool {
    meta::flag(&meta_of(doc, page), meta::TEMPLATE)
}

/// The placeholder map of a page as stored, entries the readers would skip included.
fn raw_placeholders(doc: &StructureDoc, page: PageId) -> BTreeMap<String, String> {
    match meta::value(&meta_of(doc, page), meta::TEMPLATE_VALUES) {
        Some(LoroValue::Map(map)) => map
            .iter()
            .filter_map(|(key, value)| match value {
                LoroValue::String(name) => Some((key.clone(), name.to_string())),
                _ => None,
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// Stores a placeholder map on a page as it is, whatever it holds.
fn put_raw_placeholders(doc: &mut StructureDoc, page: PageId, entries: &[(String, &str)]) {
    let map: HashMap<String, LoroValue> = entries
        .iter()
        .map(|(key, name)| (key.clone(), LoroValue::from(*name)))
        .collect();
    meta_of(doc, page)
        .insert(meta::TEMPLATE_VALUES, LoroValue::from(map))
        .unwrap();
    doc.commit();
}

// ----- the world -------------------------------------------------------------------------------

/// Inbox(1); Tasks(2) that holds the template Standup(3) (flagged) with Notes(4) that holds
/// Deep(5), and Gone(6) in the trash; Archive(7). Properties: Status (select, "Doing"), Due (date),
/// Owner (person), Cost (number), a two-way relation Blocks (`prop(5)`, back side `prop(6)`).
/// Standup: Status "Doing", the placeholder `Due: today`; Notes: `Due: now`, `Owner: me`; Deep:
/// Cost 7.
fn world() -> StructureDoc {
    let mut doc = structure(1);
    doc.set_author(Some(device()));
    let mut next = 0;
    doc.create_property(
        prop(1),
        "Status",
        PropertyKind::Select,
        &mut || {
            next += 1;
            opt(100 + next)
        },
        1_000,
    )
    .unwrap();
    doc.add_option(prop(1), opt(1), "Doing", OptionColor::Blue, None, 1_001)
        .unwrap();
    doc.create_property(prop(2), "Due", PropertyKind::Date, &mut || opt(200), 1_002)
        .unwrap();
    doc.create_property(
        prop(3),
        "Owner",
        PropertyKind::Person,
        &mut || opt(201),
        1_003,
    )
    .unwrap();
    doc.create_property(
        prop(4),
        "Cost",
        PropertyKind::Number,
        &mut || opt(202),
        1_004,
    )
    .unwrap();
    doc.create_relation_pair(prop(5), prop(6), "Blocks", "Blocked by", 1_005)
        .unwrap();
    add(&mut doc, 1, None, "Inbox");
    add(&mut doc, 2, None, "Tasks");
    add(&mut doc, 3, Some(2), "Standup {{date}} {{time}}");
    add(&mut doc, 4, Some(3), "Notes of {{date}}");
    add(&mut doc, 5, Some(4), "Deep");
    add(&mut doc, 6, Some(3), "Gone");
    add(&mut doc, 7, None, "Archive");
    doc.set_page_icon(pid(3), Some("📋"), 20).unwrap();
    doc.set_page_cover(pid(3), Some("gradient:sunset"), Some(30), 21)
        .unwrap();
    doc.set_page_appearance(
        pid(3),
        Some(kb_types::PageFont::Serif),
        Some(true),
        Some(true),
        22,
    )
    .unwrap();
    doc.set_page_property(pid(3), prop(1), Some(PropertyValue::Select(opt(1))), 30)
        .unwrap();
    doc.set_page_property(pid(5), prop(4), Some(PropertyValue::Number(7.0)), 31)
        .unwrap();
    doc.trash_page(pid(6), 40).unwrap();
    mark_template(&mut doc, pid(3));
    set_placeholder(&mut doc, pid(3), prop(2), TemplatePlaceholder::Today);
    set_placeholder(&mut doc, pid(4), prop(2), TemplatePlaceholder::Now);
    set_placeholder(&mut doc, pid(4), prop(3), TemplatePlaceholder::Me);
    doc
}

fn ids_of(doc: &StructureDoc, top: PageId) -> HashMap<PageId, PageId> {
    doc.subtree(top)
        .unwrap()
        .into_iter()
        .map(|id| (id, pid(number_of(id) + 100)))
        .collect()
}

struct Request<'a> {
    parent: Option<u8>,
    index: Option<usize>,
    me: Option<AccountId>,
    values: &'a [(PropertyId, PropertyValue)],
}

impl Default for Request<'_> {
    fn default() -> Self {
        Self {
            parent: Some(2),
            index: None,
            me: Some(account(1)),
            values: &[],
        }
    }
}

fn try_create(
    doc: &mut StructureDoc,
    request: &Request<'_>,
    now: i64,
) -> Result<Duplicated, ModelError> {
    let ids = ids_of(doc, pid(3));
    let moment = moment();
    let spec = TemplateCopy {
        parent: request.parent.map(pid),
        index: request.index,
        moment: &moment,
        me: request.me,
        values: request.values,
    };
    doc.create_from_template(pid(3), &ids, &spec, now)
}

fn create(doc: &mut StructureDoc, request: &Request<'_>, now: i64) -> Duplicated {
    try_create(doc, request, now).unwrap()
}

fn value(doc: &StructureDoc, page: PageId, property: u8) -> Option<PropertyValue> {
    doc.page_properties(page)
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == prop(property))
        .map(|(_, value)| value)
}

fn related(doc: &StructureDoc, page: PageId, property: u8) -> Vec<u8> {
    match value(doc, page, property) {
        Some(PropertyValue::Relation(pages)) => pages.into_iter().map(number_of).collect(),
        None => Vec::new(),
        other => panic!("not a relation: {other:?}"),
    }
}

/// The titles of the live children of `page`, in order.
fn children_of(doc: &StructureDoc, page: u8) -> Vec<String> {
    doc.live_tree()
        .into_iter()
        .find(|node| node.page.id == pid(page))
        .unwrap()
        .children
        .into_iter()
        .map(|node| node.page.title)
        .collect()
}

// ----- the copies --------------------------------------------------------------------------------

#[test]
fn the_subtree_is_copied_under_the_parent_at_the_index_as_pages_with_resolved_titles() {
    let mut doc = world();
    let original = doc.page(pid(3)).unwrap();

    let done = create(
        &mut doc,
        &Request {
            index: Some(0),
            ..Request::default()
        },
        500,
    );

    assert_eq!(
        done.pages,
        vec![(pid(3), copy(3)), (pid(4), copy(4)), (pid(5), copy(5))],
        "pre-order, the top page first; the trashed page is not copied"
    );
    assert_eq!(
        outline(&doc),
        "Inbox\nTasks\n  Standup 2026-10-18 13:05\n    Notes of 2026-10-18\n      Deep\n  Standup {{date}} {{time}}\n    Notes of {{date}}\n      Deep\nArchive\n",
        "the copy is the first child of Tasks, before the template; no ' (copy)'; the tokens of every title are resolved"
    );
    assert_eq!(
        doc.page(pid(3)).unwrap(),
        original,
        "the template is untouched"
    );
    assert_eq!(doc.trashed_pages().len(), 1);

    // The top copy: icon, cover, look and lock as the duplicate keeps them; new times and creator.
    let top = doc.page(copy(3)).unwrap();
    assert_eq!(top.parent_id, Some(pid(2)));
    assert_eq!(top.icon.as_deref(), Some("📋"));
    assert_eq!(
        (top.cover.as_deref(), top.cover_y),
        (Some("gradient:sunset"), Some(30))
    );
    assert_eq!(
        (top.font, top.small_text, top.locked),
        (Some(kb_types::PageFont::Serif), true, true)
    );
    assert_eq!((top.created_at, top.updated_at), (500, 500));
    assert_eq!(top.created_by, Some(device()));
    assert_eq!(doc.page(copy(5)).unwrap().parent_id, Some(copy(4)));
    assert_eq!(doc.page(copy(4)).unwrap().parent_id, Some(copy(3)));
    assert_healthy(&doc);
}

#[test]
fn the_top_copy_goes_to_the_top_level_or_last_or_is_clamped() {
    let mut doc = world();
    // No parent: the top level, last.
    create(
        &mut doc,
        &Request {
            parent: None,
            ..Request::default()
        },
        500,
    );
    assert_eq!(
        outline(&doc)
            .lines()
            .filter(|line| !line.starts_with(' '))
            .collect::<Vec<_>>(),
        ["Inbox", "Tasks", "Archive", "Standup 2026-10-18 13:05"]
    );
    assert_eq!(doc.page(copy(3)).unwrap().parent_id, None);

    // Under a parent, no index: last, after the template that is its only live child.
    let mut doc = world();
    create(&mut doc, &Request::default(), 500);
    assert_eq!(
        children_of(&doc, 2),
        ["Standup {{date}} {{time}}", "Standup 2026-10-18 13:05"]
    );

    // An index past the end is clamped to last; an index in the middle goes there.
    let mut doc = world();
    create(
        &mut doc,
        &Request {
            index: Some(99),
            ..Request::default()
        },
        500,
    );
    assert_eq!(
        children_of(&doc, 2),
        ["Standup {{date}} {{time}}", "Standup 2026-10-18 13:05"]
    );
    let mut doc = world();
    create(
        &mut doc,
        &Request {
            parent: None,
            index: Some(1),
            ..Request::default()
        },
        500,
    );
    assert_eq!(
        outline(&doc)
            .lines()
            .filter(|line| !line.starts_with(' '))
            .collect::<Vec<_>>(),
        ["Inbox", "Standup 2026-10-18 13:05", "Tasks", "Archive"]
    );
    assert_healthy(&doc);
}

#[test]
fn the_copies_are_pages_not_templates_and_carry_no_placeholders() {
    let mut doc = world();
    create(&mut doc, &Request::default(), 500);
    for page in [copy(3), copy(4), copy(5)] {
        assert!(!is_flagged(&doc, page), "{page} is a page");
        assert!(
            raw_placeholders(&doc, page).is_empty(),
            "{page} has no placeholders"
        );
        assert!(doc.page(page).unwrap().properties.len() <= 3);
    }
    // The template keeps its flag and its placeholders.
    assert!(is_flagged(&doc, pid(3)));
    assert_eq!(
        raw_placeholders(&doc, pid(3)),
        BTreeMap::from([(prop(2).to_string(), "today".to_owned())])
    );
    assert_eq!(raw_placeholders(&doc, pid(4)).len(), 2);
}

#[test]
fn a_title_that_the_tokens_make_too_long_is_cut_and_other_tokens_are_left_alone() {
    let mut doc = world();
    let long = format!("{}{{{{date}}}}", "x".repeat(495));
    doc.rename_page(pid(3), &long, 50).unwrap();
    doc.rename_page(pid(5), "{{Date}} {{ date }} {{now}} {{date}}{{time}}", 51)
        .unwrap();
    create(&mut doc, &Request::default(), 500);
    let title = doc.page(copy(3)).unwrap().title;
    assert_eq!(title.chars().count(), 500, "cut to the limit of a title");
    assert!(title.starts_with(&"x".repeat(495)) && title.ends_with("2026-"));
    assert_eq!(
        doc.page(copy(5)).unwrap().title,
        "{{Date}} {{ date }} {{now}} 2026-10-1813:05",
        "case-sensitive, no other token, repeated ones all resolve"
    );
}

// ----- the values ---------------------------------------------------------------------------------

#[test]
fn values_are_copied_and_the_placeholders_become_values() {
    let mut doc = world();
    let done = create(&mut doc, &Request::default(), 500);

    assert_eq!(
        value(&doc, copy(3), 1),
        Some(PropertyValue::Select(opt(1))),
        "a stored value is copied"
    );
    assert_eq!(value(&doc, copy(5), 4), Some(PropertyValue::Number(7.0)));
    assert_eq!(
        value(&doc, copy(3), 2),
        Some(date("2026-10-18", None)),
        "today: the local day"
    );
    assert_eq!(
        value(&doc, copy(4), 2),
        Some(date("2026-10-18", Some("13:05"))),
        "now: the day and the time, on a sub-page too"
    );
    assert_eq!(
        value(&doc, copy(4), 3),
        Some(PropertyValue::Person(vec![account(1)])),
        "me: the account of the request"
    );
    assert_eq!(value(&doc, copy(5), 2), None);
    // The template is unchanged: its own value of Status, no value for the placeholders.
    assert_eq!(value(&doc, pid(3), 2), None);
    assert!(done.touched.contains(&copy(3)) && done.touched.contains(&copy(4)));
    assert!(
        !done.touched.contains(&pid(3)),
        "an original is not touched"
    );
    assert_healthy(&doc);
}

#[test]
fn a_placeholder_that_cannot_be_resolved_gives_no_value() {
    let mut doc = world();
    // `me` without an account; a placeholder on a property of another kind, one that is gone, one
    // that is not an id and a name this version does not know.
    put_raw_placeholders(
        &mut doc,
        pid(5),
        &[
            (prop(4).to_string(), "today"),
            (prop(9).to_string(), "now"),
            ("not-an-id".to_owned(), "today"),
            (prop(2).to_string(), "tomorrow"),
        ],
    );

    create(
        &mut doc,
        &Request {
            me: None,
            ..Request::default()
        },
        500,
    );

    assert_eq!(value(&doc, copy(4), 3), None, "no account, no value");
    assert_eq!(
        value(&doc, copy(4), 2),
        Some(date("2026-10-18", Some("13:05")))
    );
    assert_eq!(
        value(&doc, copy(5), 4),
        Some(PropertyValue::Number(7.0)),
        "a date placeholder on a number property is skipped, the stored value stays"
    );
    assert_eq!(value(&doc, copy(5), 2), None, "an unknown name is skipped");
    assert_healthy(&doc);
}

#[test]
fn the_values_of_the_request_win_over_the_placeholders_and_go_on_the_top_copy_only() {
    let mut doc = world();
    let values = [
        (prop(2), date("2030-01-02", None)),
        (prop(1), PropertyValue::Select(opt(1))),
        (prop(4), PropertyValue::Number(12.0)),
        (prop(3), PropertyValue::Person(vec![account(2)])),
    ];
    create(
        &mut doc,
        &Request {
            values: &values,
            ..Request::default()
        },
        500,
    );
    assert_eq!(value(&doc, copy(3), 2), Some(date("2030-01-02", None)));
    assert_eq!(value(&doc, copy(3), 4), Some(PropertyValue::Number(12.0)));
    assert_eq!(
        value(&doc, copy(3), 3),
        Some(PropertyValue::Person(vec![account(2)])),
        "the model leaves the membership of a person to the core"
    );
    // The sub-pages are not touched by the request's values: their placeholders still apply.
    assert_eq!(
        value(&doc, copy(4), 2),
        Some(date("2026-10-18", Some("13:05")))
    );
    assert_eq!(
        value(&doc, copy(4), 3),
        Some(PropertyValue::Person(vec![account(1)]))
    );
    assert_eq!(value(&doc, copy(4), 4), None);
    assert_healthy(&doc);
}

#[test]
fn relations_inside_the_subtree_follow_the_copies_and_the_others_are_kept_paired() {
    let mut doc = world();
    let relation = |pages: &[u8]| PropertyValue::Relation(pages.iter().map(|n| pid(*n)).collect());
    // Standup blocks Archive (outside); Notes blocks Deep (inside).
    doc.set_page_property(pid(3), prop(5), Some(relation(&[7])), 50)
        .unwrap();
    doc.set_page_property(pid(4), prop(5), Some(relation(&[5])), 51)
        .unwrap();

    let done = create(&mut doc, &Request::default(), 500);

    assert_eq!(related(&doc, copy(3), 5), [7]);
    assert_eq!(
        related(&doc, pid(7), 6),
        [3, 103],
        "Archive is blocked by both"
    );
    assert_eq!(related(&doc, copy(4), 5), [105], "inside: the copy of Deep");
    assert_eq!(related(&doc, copy(5), 6), [104]);
    assert_eq!(
        related(&doc, pid(5), 6),
        [4],
        "the original Deep: the original Notes only"
    );
    assert!(
        done.touched.contains(&pid(7)),
        "the outside page's side was written"
    );
    assert!(!done.touched.contains(&pid(5)));

    // A relation among the request's values pairs the other side as well.
    let mut doc = world();
    let values = [(prop(5), relation(&[1]))];
    create(
        &mut doc,
        &Request {
            values: &values,
            ..Request::default()
        },
        500,
    );
    assert_eq!(related(&doc, copy(3), 5), [1]);
    assert_eq!(related(&doc, pid(1), 6), [103]);
    assert_healthy(&doc);
}

// ----- the refusals -------------------------------------------------------------------------------

#[test]
fn refusals_write_nothing() {
    let mut doc = world();
    let before = outline(&doc);
    let version = doc.doc().version();

    // A parent inside the template (itself or below it), in the trash, unknown.
    for parent in [3, 4, 5] {
        assert!(
            matches!(
                try_create(
                    &mut doc,
                    &Request {
                        parent: Some(parent),
                        ..Request::default()
                    },
                    500
                ),
                Err(ModelError::CyclicMove)
            ),
            "parent {parent}"
        );
    }
    for parent in [6, 99] {
        assert!(
            matches!(
                try_create(
                    &mut doc,
                    &Request {
                        parent: Some(parent),
                        ..Request::default()
                    },
                    500
                ),
                Err(ModelError::ParentUnavailable)
            ),
            "parent {parent}"
        );
    }
    // Values that PagePropertySet would refuse: the wrong kind, an unknown option, an unknown
    // property, a computed one, a value that fails its own rules; the good ones before are not kept.
    let good = (prop(4), PropertyValue::Number(1.0));
    let bad: [(PropertyId, PropertyValue); 5] = [
        (prop(4), PropertyValue::Text("x".into())),
        (prop(1), PropertyValue::Select(opt(77))),
        (prop(9), PropertyValue::Number(1.0)),
        (prop(2), date("2026-13-40", None)),
        (prop(4), PropertyValue::Number(f64::NAN)),
    ];
    for refused in bad {
        let values = [good.clone(), refused];
        let result = try_create(
            &mut doc,
            &Request {
                values: &values,
                ..Request::default()
            },
            500,
        );
        assert!(
            matches!(
                result,
                Err(ModelError::InvalidProperty { .. } | ModelError::PropertyNotFound)
            ),
            "{result:?}"
        );
    }
    // A page that is unknown, in the trash, and a copy id that is taken.
    let ids = ids_of(&doc, pid(3));
    let moment = moment();
    let spec = TemplateCopy {
        parent: Some(pid(2)),
        index: None,
        moment: &moment,
        me: None,
        values: &[],
    };
    assert!(matches!(
        doc.create_from_template(pid(99), &ids, &spec, 500),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.create_from_template(pid(6), &ids_of(&doc, pid(6)), &spec, 500),
        Err(ModelError::PageTrashed)
    ));
    let mut taken = ids;
    taken.insert(pid(4), pid(7));
    assert!(matches!(
        doc.create_from_template(pid(3), &taken, &spec, 500),
        Err(ModelError::DuplicatePage)
    ));

    assert_eq!(outline(&doc), before);
    assert_eq!(doc.doc().version(), version, "nothing was written");
    assert_healthy(&doc);
}

// ----- undo, replicas -----------------------------------------------------------------------------

#[test]
fn a_creation_is_one_undo_step_and_redo_brings_the_pages_and_their_values_back() {
    let mut doc = world();
    let before = outline(&doc);
    create(&mut doc, &Request::default(), 500);
    let after = outline(&doc);
    assert_ne!(before, after);
    let values = doc.page_properties(copy(4)).unwrap();
    assert_eq!(values.len(), 2);

    // One undo removes the three copies together, and the template is still a template.
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(outline(&doc), before);
    assert!(doc.page(copy(3)).is_none() && doc.page(copy(5)).is_none());
    assert!(is_flagged(&doc, pid(3)), "the flag is an earlier step");
    assert_healthy(&doc);

    // One redo brings them all back, with the resolved values, and still no flag on a copy.
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(outline(&doc), after);
    assert_eq!(doc.page_properties(copy(4)).unwrap(), values);
    assert!(!is_flagged(&doc, copy(3)));
    assert!(doc.can_undo() && !doc.can_redo());
    assert_healthy(&doc);
}

#[test]
fn a_copy_made_from_a_template_is_an_ordinary_page_for_a_replica() {
    let mut a = world();
    let mut b = replica(&a, 2);
    create(&mut a, &Request::default(), 500);
    sync(&mut a, &mut b);
    assert_eq!(outline(&a), outline(&b));
    assert_eq!(
        value(&b, copy(4), 2),
        Some(date("2026-10-18", Some("13:05")))
    );
    assert!(!is_flagged(&b, copy(3)));
    assert_healthy(&b);
}

// ----- the duplicate keeps the flag ----------------------------------------------------------------

#[test]
fn the_duplicate_of_a_template_is_a_template_with_its_placeholders() {
    let mut doc = world();
    let ids = ids_of(&doc, pid(3));
    doc.duplicate_subtree(pid(3), &ids, "Standup (copy)", 500)
        .unwrap();

    assert!(
        is_flagged(&doc, copy(3)),
        "the copy of a template is a template"
    );
    assert!(
        !is_flagged(&doc, copy(4)),
        "a sub-page carries no flag of its own"
    );
    assert_eq!(
        raw_placeholders(&doc, copy(3)),
        raw_placeholders(&doc, pid(3))
    );
    assert_eq!(
        raw_placeholders(&doc, copy(4)),
        raw_placeholders(&doc, pid(4))
    );
    assert_eq!(raw_placeholders(&doc, copy(4)).len(), 2);
    assert_eq!(
        doc.page(copy(3)).unwrap().title,
        "Standup (copy)",
        "the duplicate keeps its own title rule"
    );
    assert_eq!(
        doc.page(copy(4)).unwrap().title,
        "Notes of {{date}}",
        "the duplicate does not resolve tokens"
    );
    // The placeholders were not resolved into values by a duplicate.
    assert_eq!(value(&doc, copy(3), 2), None);
    // A replica sees the same.
    let mut b = replica(&doc, 2);
    sync(&mut doc, &mut b);
    assert!(is_flagged(&b, copy(3)));
    assert_eq!(raw_placeholders(&b, copy(4)).len(), 2);
}

// ----- the local moment ---------------------------------------------------------------------------

#[test]
fn the_local_moment_follows_the_offset_across_midnight_and_leap_days() {
    let at = |seconds: i64, offset: i32| {
        let moment = LocalMoment::new(seconds * 1000, offset);
        (moment.date, moment.time)
    };
    let pair = |date: &str, time: &str| (date.to_owned(), time.to_owned());
    // 2026-10-07 14:30 UTC.
    let utc = 1_791_383_400;
    assert_eq!(at(utc, 0), pair("2026-10-07", "14:30"));
    assert_eq!(at(utc, 14 * 3_600), pair("2026-10-08", "04:30"));
    assert_eq!(at(utc, -15 * 3_600), pair("2026-10-06", "23:30"));
    assert_eq!(at(utc, 30 * 60), pair("2026-10-07", "15:00"));
    // The epoch, and just before it.
    assert_eq!(at(0, 0), pair("1970-01-01", "00:00"));
    assert_eq!(at(0, -1), pair("1969-12-31", "23:59"));
    assert_eq!(
        LocalMoment::new(-1, 0),
        LocalMoment {
            date: "1969-12-31".into(),
            time: "23:59".into()
        }
    );
    // A leap day, and the day after.
    let leap = 19_782 * 86_400 + 12 * 3_600;
    assert_eq!(at(leap, 0), pair("2024-02-29", "12:00"));
    assert_eq!(at(leap + 86_400, 0), pair("2024-03-01", "12:00"));
    assert_eq!(at(leap - 86_400, 0), pair("2024-02-28", "12:00"));
    // The end of a year.
    let new_year = 20_454 * 86_400 - 60;
    assert_eq!(at(new_year, 0), pair("2025-12-31", "23:59"));
    assert_eq!(at(new_year + 60, 0), pair("2026-01-01", "00:00"));
}

#[test]
fn title_tokens_resolve_every_occurrence_and_nothing_else() {
    let moment = moment();
    assert_eq!(moment.resolve_title("Plain", 500), "Plain");
    assert_eq!(moment.resolve_title("", 500), "");
    assert_eq!(
        moment.resolve_title("{{date}} at {{time}} ({{date}})", 500),
        "2026-10-18 at 13:05 (2026-10-18)"
    );
    assert_eq!(
        moment.resolve_title("{{DATE}} {{date }} {date}", 500),
        "{{DATE}} {{date }} {date}"
    );
    assert_eq!(moment.resolve_title("{{date}}", 4), "2026");
    assert_eq!(
        moment.resolve_title("ab {{date}}", 3),
        "ab",
        "a cut leaves no trailing space"
    );
}

// ----- the content pass --------------------------------------------------------------------------

fn bid(n: u8) -> String {
    kb_types::BlockId::from_random_bytes([n; 16]).to_string()
}

fn doc_of(content: Value) -> Value {
    json!({ "type": "doc", "content": content })
}

fn para(n: u8, content: Value) -> Value {
    json!({ "type": "paragraph", "attrs": { "id": bid(n) }, "content": content })
}

fn date_mention(n: u8, attrs: Value) -> Value {
    let mut attrs_map = attrs.as_object().unwrap().clone();
    attrs_map.insert("id".into(), json!(bid(n)));
    json!({ "type": "dateMention", "attrs": Value::Object(attrs_map) })
}

fn person_mention(attrs: Value) -> Value {
    json!({ "type": "personMention", "attrs": attrs })
}

fn template_button(n: u8, content: Value) -> Value {
    json!({ "type": "templateButton", "attrs": { "id": bid(n), "label": "Add" }, "content": content })
}

fn fill_with(json: &Value, me: Option<&TemplateMe>) -> Value {
    let moment = moment();
    fill_template_content(
        json,
        &TemplateFill {
            moment: &moment,
            me,
        },
    )
}

fn mention_attrs(json: &Value, path: &[usize]) -> Value {
    let mut node = json;
    for at in path {
        node = &node["content"][*at];
    }
    node["attrs"].clone()
}

#[test]
fn relative_date_mentions_get_the_day_and_the_time_and_lose_relative() {
    let content = doc_of(json!([para(
        1,
        json!([
            date_mention(2, json!({ "date": "2020-01-01", "relative": "today" })),
            date_mention(
                3,
                json!({ "date": "2020-01-01", "time": "09:00", "relative": "today" })
            ),
            date_mention(
                4,
                json!({ "date": "2020-01-01", "time": "09:00", "relative": "now" })
            ),
            date_mention(
                5,
                json!({ "date": "2020-01-01", "relative": "now", "reminder": 15 })
            ),
            date_mention(6, json!({ "date": "2020-01-01" })),
            date_mention(7, json!({ "date": "2020-01-01", "relative": "tomorrow" })),
        ])
    )]));
    let filled = fill_with(&content, None);

    let attrs = |at: usize| mention_attrs(&filled, &[0, at]);
    assert_eq!(attrs(0)["date"], "2026-10-18");
    assert!(attrs(0).get("relative").is_none() && attrs(0).get("time").is_none());
    assert_eq!(
        (attrs(1)["date"].as_str(), attrs(1)["time"].as_str()),
        (Some("2026-10-18"), Some("09:00")),
        "today keeps a time that was written"
    );
    assert_eq!(
        (attrs(2)["date"].as_str(), attrs(2)["time"].as_str()),
        (Some("2026-10-18"), Some("13:05")),
        "now sets the time"
    );
    assert!(attrs(2).get("relative").is_none());
    assert_eq!(attrs(3)["reminder"], 15, "the reminder stays");
    assert_eq!(attrs(3)["time"], "13:05");
    assert_eq!(
        attrs(4),
        json!({ "id": bid(6), "date": "2020-01-01" }),
        "not relative: untouched"
    );
    assert_eq!(
        attrs(5)["relative"],
        "tomorrow",
        "a value of a newer build is left as it is"
    );
    assert_eq!(attrs(5)["date"], "2020-01-01");
    assert_eq!(attrs(0)["id"], bid(2), "ids are not this pass's business");
}

#[test]
fn relative_person_mentions_become_me() {
    let content = doc_of(json!([para(
        1,
        json!([
            person_mention(
                json!({ "accountId": account(9).to_string(), "label": "Ada", "relative": "me" })
            ),
            person_mention(json!({ "accountId": account(9).to_string(), "label": "Ada" })),
            person_mention(
                json!({ "accountId": account(9).to_string(), "label": "Ada", "relative": "you" })
            ),
        ])
    )]));
    let me = TemplateMe {
        account: account(1),
        label: Some("Grace".into()),
    };
    let filled = fill_with(&content, Some(&me));
    assert_eq!(
        mention_attrs(&filled, &[0, 0]),
        json!({ "accountId": account(1).to_string(), "label": "Grace" })
    );
    assert_eq!(
        mention_attrs(&filled, &[0, 1]),
        json!({ "accountId": account(9).to_string(), "label": "Ada" }),
        "a plain mention is untouched"
    );
    assert_eq!(mention_attrs(&filled, &[0, 2])["relative"], "you");

    // The name is not known: the label that was written stays; a long name is cut to 100.
    let unnamed = TemplateMe {
        account: account(1),
        label: None,
    };
    assert_eq!(
        mention_attrs(&fill_with(&content, Some(&unnamed)), &[0, 0]),
        json!({ "accountId": account(1).to_string(), "label": "Ada" })
    );
    let long = TemplateMe {
        account: account(1),
        label: Some("é".repeat(150)),
    };
    assert_eq!(
        mention_attrs(&fill_with(&content, Some(&long)), &[0, 0])["label"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        100
    );
    // No account at all: `relative` goes and the mention stays as it was written.
    assert_eq!(
        mention_attrs(&fill_with(&content, None), &[0, 0]),
        json!({ "accountId": account(9).to_string(), "label": "Ada" })
    );
}

#[test]
fn a_template_buttons_content_is_left_as_it_is_and_everything_else_is_reached() {
    let relative = date_mention(3, json!({ "date": "2020-01-01", "relative": "today" }));
    let me = person_mention(
        json!({ "accountId": account(9).to_string(), "label": "Ada", "relative": "me" }),
    );
    let content = doc_of(json!([
        template_button(
            1,
            json!([para(2, json!([relative.clone(), me.clone()]))])
        ),
        {
            "type": "callout",
            "attrs": { "id": bid(10) },
            "content": [{
                "type": "bulletList",
                "attrs": { "id": bid(11) },
                "content": [{
                    "type": "listItem",
                    "attrs": { "id": bid(12) },
                    "content": [para(13, json!([date_mention(14, json!({ "date": "2020-01-01", "relative": "now" }))]))]
                }]
            }]
        },
    ]));
    let me_fill = TemplateMe {
        account: account(1),
        label: Some("Grace".into()),
    };
    let filled = fill_with(&content, Some(&me_fill));

    assert_eq!(
        filled["content"][0], content["content"][0],
        "the button and its content are exactly what they were"
    );
    let nested = &filled["content"][1]["content"][0]["content"][0]["content"][0]["content"][0];
    assert_eq!(nested["attrs"]["date"], "2026-10-18");
    assert_eq!(nested["attrs"]["time"], "13:05");
    assert!(nested["attrs"].get("relative").is_none());
}

#[test]
fn the_input_of_the_content_pass_is_not_modified() {
    let content = doc_of(json!([para(
        1,
        json!([date_mention(
            2,
            json!({ "date": "2020-01-01", "relative": "today" })
        )])
    )]));
    let snapshot = content.clone();
    let _ = fill_with(&content, None);
    assert_eq!(content, snapshot);
}

#[test]
fn a_view_that_names_a_template_inside_the_subtree_names_its_copy() {
    let page =
        |n: u8| PageId::from_random_bytes([0x70, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n]);
    let page_copy =
        |n: u8| PageId::from_random_bytes([0x71, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n]);
    let mut ids = CopyIds::default();
    ids.pages.insert(page(1), page_copy(1));
    let content = doc_of(json!([
        {
            "type": "pageList",
            "attrs": {
                "id": bid(1),
                "views": [
                    { "id": "default", "layout": "list", "defaultTemplate": page(1).to_string() },
                    { "id": "v2", "layout": "table", "defaultTemplate": page(2).to_string(), "name": "Outside" },
                    { "id": "v3", "layout": "board" },
                    { "id": "v4", "layout": "board", "defaultTemplate": "not an id" }
                ]
            }
        },
        // Not a pageList: a key of that name is not looked at.
        { "type": "callout", "attrs": { "id": bid(2), "views": [{ "defaultTemplate": page(1).to_string() }] } }
    ]));
    let remapped = remap_portable(&content, &ids);

    let views = &remapped["content"][0]["attrs"]["views"];
    assert_eq!(views[0]["defaultTemplate"], page_copy(1).to_string());
    assert_eq!(views[0]["layout"], "list", "the rest of the view is kept");
    assert_eq!(
        views[1]["defaultTemplate"],
        page(2).to_string(),
        "outside: kept"
    );
    assert_eq!(views[1]["name"], "Outside");
    assert!(views[2].get("defaultTemplate").is_none());
    assert_eq!(views[3]["defaultTemplate"], "not an id");
    assert_eq!(
        remapped["content"][1]["attrs"]["views"][0]["defaultTemplate"],
        page(1).to_string()
    );
    assert_eq!(
        remapped["content"][0]["attrs"]["id"],
        bid(1),
        "block ids this map does not know stay"
    );
    // The page document rebuilt from it keeps the views.
    let doc = PageDoc::from_portable(Doc::new(DocKind::Page, 9), &remapped).unwrap();
    assert_eq!(
        doc.to_portable()["content"][0]["attrs"]["views"][0]["defaultTemplate"],
        page_copy(1).to_string()
    );
}
