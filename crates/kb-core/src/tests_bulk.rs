//! Bulk edit of rows and the excerpts of the feed (ADR-0058): `PageBulk` (`SetValue`, `SetValues`,
//! `Move`, `Duplicate`, `Trash`: the limits and the checks that write nothing, the pages that go
//! with an ancestor, one structural undo step per structural action, the answers, the events, the
//! role gate and persistence) and `PageExcerpts` (the text rule, the pages left out).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use kb_model::NewPage;
use kb_types::{
    AccountId, ApiError, BulkAction, ErrorCode, Event, MemberRole, PageExcerpt, PageId,
    PropertyConfig, PropertyDef, PropertyFilter, PropertyId, PropertyKind, PropertyOption,
    PropertyValue, Request, Response, SpaceId,
};

use crate::Core;
use crate::pages::{cut_excerpt, excerpt_row};
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, outline, page, page_content, spaces,
    tree, write_text,
};
use crate::tests_sharing::{Dev, add_member, server, share};

/// A core with a workspace and one space.
fn world() -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    core.drain_events();
    (fx, core, space)
}

fn bulk(
    core: &mut Core,
    space: SpaceId,
    pages: &[PageId],
    action: BulkAction,
) -> Result<Vec<PageId>, ApiError> {
    match core.dispatch(Request::PageBulk {
        space_id: space,
        page_ids: pages.to_vec(),
        action,
    })? {
        Response::PageBulk(ids) => Ok(ids),
        other => panic!("unexpected {other:?}"),
    }
}

fn excerpts(
    core: &mut Core,
    space: SpaceId,
    pages: &[PageId],
) -> Result<Vec<PageExcerpt>, ApiError> {
    match core.dispatch(Request::PageExcerpts {
        space_id: space,
        page_ids: pages.to_vec(),
    })? {
        Response::PageExcerpts(list) => Ok(list),
        other => panic!("unexpected {other:?}"),
    }
}

/// The text of the excerpt of one page, `None` when the answer leaves it out.
fn excerpt_of(core: &mut Core, space: SpaceId, page_id: PageId) -> Option<String> {
    excerpts(core, space, &[page_id])
        .unwrap()
        .into_iter()
        .next()
        .map(|excerpt| excerpt.text)
}

/// The field an `InvalidInput` names.
fn field(err: ApiError) -> String {
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
    err.details.unwrap()["field"].as_str().unwrap().to_owned()
}

fn undo(core: &mut Core, space: SpaceId) -> String {
    match core
        .dispatch(Request::StructureUndo { space_id: space })
        .unwrap()
    {
        Response::Tree(view) => outline(&view),
        other => panic!("unexpected {other:?}"),
    }
}

fn redo(core: &mut Core, space: SpaceId) -> String {
    match core
        .dispatch(Request::StructureRedo { space_id: space })
        .unwrap()
    {
        Response::Tree(view) => outline(&view),
        other => panic!("unexpected {other:?}"),
    }
}

fn outline_of(core: &mut Core, space: SpaceId) -> String {
    outline(&tree(core, space))
}

fn prop_create(
    core: &mut Core,
    space_id: SpaceId,
    name: &str,
    kind: PropertyKind,
    config: Option<PropertyConfig>,
) -> PropertyDef {
    match core
        .dispatch(Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config,
        })
        .unwrap()
    {
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
    match core
        .dispatch(Request::PropertyOptionAdd {
            space_id,
            property_id,
            name: name.into(),
            color: kb_types::OptionColor::Green,
            group: None,
        })
        .unwrap()
    {
        Response::PropertyOption(option) => option,
        other => panic!("unexpected {other:?}"),
    }
}

fn value_of(core: &mut Core, page_id: PageId, property_id: PropertyId) -> Option<PropertyValue> {
    match core
        .dispatch(Request::PagePropertiesGet { page_id })
        .unwrap()
    {
        Response::PageProperties(values) => values
            .values
            .into_iter()
            .find(|(id, _)| *id == property_id)
            .map(|(_, value)| value),
        other => panic!("unexpected {other:?}"),
    }
}

fn by_property(
    core: &mut Core,
    space_id: SpaceId,
    property_id: PropertyId,
    filter: PropertyFilter,
) -> BTreeSet<PageId> {
    match core
        .dispatch(Request::PagesByProperty {
            space_id,
            property_id,
            filter,
        })
        .unwrap()
    {
        Response::PageIds(ids) => ids.into_iter().collect(),
        other => panic!("unexpected {other:?}"),
    }
}

fn number(n: f64) -> Option<PropertyValue> {
    Some(PropertyValue::Number(n))
}

fn trash(core: &mut Core, page_id: PageId) {
    core.dispatch(Request::PageTrash { page_id }).unwrap();
}

/// A page id that no page has.
fn stranger(n: u16) -> PageId {
    let mut bytes = [0u8; 16];
    bytes[..2].copy_from_slice(&n.to_le_bytes());
    bytes[15] = 0xEE;
    PageId::from_bytes(bytes)
}

/// How many of `events` are `wanted`.
fn count(events: &[Event], wanted: &Event) -> usize {
    events.iter().filter(|event| *event == wanted).count()
}

// ----- the request: limits and checks that write nothing -----------------------------------------

#[test]
fn a_bad_list_of_pages_is_refused_and_nothing_is_written() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    let gone = create_page(&mut core, space, None, "Gone").id;
    trash(&mut core, gone);
    let other = create_space(&mut core, "Other").id;
    let foreign = create_page(&mut core, other, None, "Foreign").id;
    let before = outline_of(&mut core, space);
    core.drain_events();

    let too_many: Vec<PageId> = (0..1_001).map(stranger).collect();
    let cases: Vec<(&str, Vec<PageId>)> = vec![
        ("no page", vec![]),
        ("a page twice", vec![a, b, a]),
        ("a trashed page", vec![a, gone]),
        ("a page of another space", vec![a, foreign]),
        ("an unknown page", vec![a, stranger(1)]),
        ("more than 1,000 pages", too_many),
    ];
    for action in [
        BulkAction::Trash,
        BulkAction::Duplicate,
        BulkAction::Move { parent_id: None },
        BulkAction::SetValue {
            property_id: PropertyId::from_bytes([1; 16]),
            value: None,
        },
    ] {
        for (why, pages) in &cases {
            let err = bulk(&mut core, space, pages, action.clone()).unwrap_err();
            assert_eq!(field(err), "pageIds", "{why}");
        }
    }
    assert_eq!(outline_of(&mut core, space), before);
    assert!(core.drain_events().is_empty(), "nothing was announced");

    let err = bulk(
        &mut core,
        SpaceId::from_bytes([7; 16]),
        &[a],
        BulkAction::Trash,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

// ----- SetValue and SetValues --------------------------------------------------------------------

#[test]
fn set_value_writes_every_page_in_one_change_and_one_event() {
    let (_fx, mut core, space) = world();
    let pages: Vec<PageId> = ["A", "B", "C"]
        .iter()
        .map(|title| create_page(&mut core, space, None, title).id)
        .collect();
    let [a, b, c] = [pages[0], pages[1], pages[2]];
    let amount = prop_create(&mut core, space, "Amount", PropertyKind::Number, None).id;
    core.drain_events();

    let answer = bulk(
        &mut core,
        space,
        &[a, b],
        BulkAction::SetValue {
            property_id: amount,
            value: number(5.0),
        },
    )
    .unwrap();
    assert_eq!(answer, [a, b], "the pages acted on, in the order given");
    assert_eq!(
        core.drain_events(),
        vec![Event::StructureChanged { space_id: space }],
        "one event for the whole change"
    );
    assert_eq!(value_of(&mut core, a, amount), number(5.0));
    assert_eq!(value_of(&mut core, b, amount), number(5.0));
    assert_eq!(value_of(&mut core, c, amount), None);
    let derived = by_property(
        &mut core,
        space,
        amount,
        PropertyFilter::Equals(PropertyValue::Number(5.0)),
    );
    assert_eq!(derived, BTreeSet::from([a, b]), "the derived rows follow");

    // `None` clears.
    bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: amount,
            value: None,
        },
    )
    .unwrap();
    assert_eq!(value_of(&mut core, a, amount), None);
    assert_eq!(value_of(&mut core, b, amount), number(5.0));
}

#[test]
fn set_values_gives_each_page_its_own_value_and_is_all_or_nothing() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    let c = create_page(&mut core, space, None, "C").id;
    let amount = prop_create(&mut core, space, "Amount", PropertyKind::Number, None).id;
    let stage = prop_create(&mut core, space, "Stage", PropertyKind::Select, None).id;
    let todo = option_add(&mut core, space, stage, "Todo").id;
    bulk(
        &mut core,
        space,
        &[b],
        BulkAction::SetValue {
            property_id: amount,
            value: number(9.0),
        },
    )
    .unwrap();
    core.drain_events();

    // One value or `None` for each page, in the order of `pageIds`: the undo of a `SetValue`.
    let answer = bulk(
        &mut core,
        space,
        &[a, b, c],
        BulkAction::SetValues {
            property_id: amount,
            values: vec![number(1.0), None, number(3.0)],
        },
    )
    .unwrap();
    assert_eq!(answer, [a, b, c]);
    assert_eq!(value_of(&mut core, a, amount), number(1.0));
    assert_eq!(value_of(&mut core, b, amount), None);
    assert_eq!(value_of(&mut core, c, amount), number(3.0));
    assert_eq!(
        core.drain_events(),
        vec![Event::StructureChanged { space_id: space }]
    );

    // The lengths differ.
    let err = bulk(
        &mut core,
        space,
        &[a, b, c],
        BulkAction::SetValues {
            property_id: amount,
            values: vec![number(7.0)],
        },
    )
    .unwrap_err();
    assert_eq!(field(err), "values");

    // One value that the property refuses: no page changes (a select option the property does not
    // list, a value of another kind, a computed property, an unknown one).
    let unknown_option = kb_types::OptionId::from_bytes([4; 16]);
    let err = bulk(
        &mut core,
        space,
        &[a, b],
        BulkAction::SetValues {
            property_id: stage,
            values: vec![
                Some(PropertyValue::Select(todo)),
                Some(PropertyValue::Select(unknown_option)),
            ],
        },
    )
    .unwrap_err();
    assert_eq!(field(err), "value");
    assert_eq!(
        value_of(&mut core, a, stage),
        None,
        "the valid one was not written either"
    );
    let err = bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: amount,
            value: Some(PropertyValue::Text("x".into())),
        },
    )
    .unwrap_err();
    assert_eq!(field(err), "value");
    let created = prop_create(&mut core, space, "Created", PropertyKind::CreatedTime, None).id;
    let err = bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: created,
            value: None,
        },
    )
    .unwrap_err();
    assert_eq!(field(err), "propertyId");
    let err = bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: PropertyId::from_bytes([8; 16]),
            value: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(value_of(&mut core, a, amount), number(1.0));
    assert_eq!(value_of(&mut core, c, amount), number(3.0));
}

#[test]
fn a_person_value_is_checked_for_every_page_before_anything_is_written() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person, None).id;
    let err = bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: owner,
            value: Some(PropertyValue::Person(vec![AccountId::from_bytes([5; 16])])),
        },
    )
    .unwrap_err();
    assert_eq!(field(err), "value", "the account is no member of the space");
    assert_eq!(value_of(&mut core, a, owner), None);
}

#[test]
fn set_values_keeps_a_two_way_relation_paired_like_one_write_at_a_time() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    let c = create_page(&mut core, space, None, "C").id;
    let x = create_page(&mut core, space, None, "X").id;
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
    core.drain_events();

    bulk(
        &mut core,
        space,
        &[a, b],
        BulkAction::SetValues {
            property_id: blocks.id,
            values: vec![
                Some(PropertyValue::Relation(vec![x])),
                Some(PropertyValue::Relation(vec![x, c])),
            ],
        },
    )
    .unwrap();
    assert_eq!(
        value_of(&mut core, x, blocked_by),
        Some(PropertyValue::Relation(vec![a, b]))
    );
    assert_eq!(
        value_of(&mut core, c, blocked_by),
        Some(PropertyValue::Relation(vec![b]))
    );
    assert_eq!(
        core.drain_events(),
        vec![Event::StructureChanged { space_id: space }]
    );

    bulk(
        &mut core,
        space,
        &[a, b],
        BulkAction::SetValue {
            property_id: blocks.id,
            value: None,
        },
    )
    .unwrap();
    assert_eq!(value_of(&mut core, x, blocked_by), None);
    assert_eq!(value_of(&mut core, c, blocked_by), None);
}

#[test]
fn a_value_change_is_not_a_structural_undo_step() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    create_page(&mut core, space, None, "B");
    let amount = prop_create(&mut core, space, "Amount", PropertyKind::Number, None).id;
    bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: amount,
            value: number(2.0),
        },
    )
    .unwrap();
    // The undo takes the creation of B, the last structure step; the value stays.
    assert_eq!(undo(&mut core, space), "A\n");
    assert_eq!(value_of(&mut core, a, amount), number(2.0));
}

// ----- Move --------------------------------------------------------------------------------------

#[test]
fn move_puts_the_pages_last_under_the_parent_in_the_order_given_in_one_undo_step() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    let c = create_page(&mut core, space, None, "C").id;
    let d = create_page(&mut core, space, None, "D").id;
    let p = create_page(&mut core, space, None, "P").id;
    let x = create_page(&mut core, space, Some(p), "X").id;
    assert_eq!(outline_of(&mut core, space), "A\nB\nC\nD\nP\n  X\n");
    core.drain_events();

    let answer = bulk(
        &mut core,
        space,
        &[c, a],
        BulkAction::Move { parent_id: Some(p) },
    )
    .unwrap();
    assert_eq!(answer, [c, a]);
    assert_eq!(outline_of(&mut core, space), "B\nD\nP\n  X\n  C\n  A\n");
    assert_eq!(
        page(&mut core, c).parent_id,
        Some(p),
        "the index row follows"
    );
    let events = core.drain_events();
    assert_eq!(
        count(&events, &Event::StructureChanged { space_id: space }),
        1
    );

    // One undo puts both back; the next one is the step before (the creation of X).
    assert_eq!(undo(&mut core, space), "A\nB\nC\nD\nP\n  X\n");
    assert_eq!(undo(&mut core, space), "A\nB\nC\nD\nP\n");
    assert_eq!(redo(&mut core, space), "A\nB\nC\nD\nP\n  X\n");
    assert_eq!(redo(&mut core, space), "B\nD\nP\n  X\n  C\n  A\n");

    // To the top level, last, in the order given; a page already there is moved to its place.
    let answer = bulk(
        &mut core,
        space,
        &[x, c],
        BulkAction::Move { parent_id: None },
    )
    .unwrap();
    assert_eq!(answer, [x, c]);
    assert_eq!(outline_of(&mut core, space), "B\nD\nP\n  A\nX\nC\n");
    let answer = bulk(&mut core, space, &[b], BulkAction::Move { parent_id: None }).unwrap();
    assert_eq!(answer, [b]);
    assert_eq!(outline_of(&mut core, space), "D\nP\n  A\nX\nC\nB\n");
    assert_eq!(undo(&mut core, space), "B\nD\nP\n  A\nX\nC\n");
    assert_eq!(page(&mut core, d).parent_id, None);
}

#[test]
fn a_page_whose_ancestor_is_listed_goes_with_it_and_is_not_moved_alone() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let a1 = create_page(&mut core, space, Some(a), "A1").id;
    let b = create_page(&mut core, space, None, "B").id;
    let d = create_page(&mut core, space, None, "D").id;
    let answer = bulk(
        &mut core,
        space,
        &[a1, a, b],
        BulkAction::Move { parent_id: Some(d) },
    )
    .unwrap();
    assert_eq!(answer, [a, b], "the pages acted on, without the descendant");
    assert_eq!(outline_of(&mut core, space), "D\n  A\n    A1\n  B\n");
    assert_eq!(undo(&mut core, space), "A\n  A1\nB\nD\n");
}

#[test]
fn a_move_under_a_parent_that_cannot_take_the_pages_writes_nothing() {
    let (_fx, mut core, space) = world();
    let p = create_page(&mut core, space, None, "P").id;
    let a = create_page(&mut core, space, Some(p), "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    let gone = create_page(&mut core, space, None, "Gone").id;
    trash(&mut core, gone);
    let other = create_space(&mut core, "Other").id;
    let foreign = create_page(&mut core, other, None, "Foreign").id;
    let before = outline_of(&mut core, space);
    core.drain_events();

    for (why, pages, parent) in [
        ("one of the pages", vec![b, p], p),
        ("below one of the pages", vec![b, p], a),
        ("a trashed page", vec![b], gone),
        ("a page of another space", vec![b], foreign),
        ("an unknown page", vec![b], stranger(2)),
    ] {
        let err = bulk(
            &mut core,
            space,
            &pages,
            BulkAction::Move {
                parent_id: Some(parent),
            },
        )
        .unwrap_err();
        assert_eq!(field(err), "parentId", "{why}");
    }
    assert_eq!(outline_of(&mut core, space), before);
    assert!(core.drain_events().is_empty());
}

#[test]
fn a_move_into_or_out_of_a_template_changes_what_the_search_finds() {
    let (_fx, mut core, space) = world();
    let template = create_page(&mut core, space, None, "Template").id;
    core.dispatch(Request::PageSetTemplate {
        page_id: template,
        template: true,
    })
    .unwrap();
    let a = create_page(&mut core, space, None, "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    write_text(&mut core, a, "a pomegranate plan", 1);
    write_text(&mut core, b, "a pomegranate list", 2);
    let found = |core: &mut Core| -> BTreeSet<PageId> {
        match core
            .dispatch(Request::Search {
                query: "pomegranate".into(),
                space_id: None,
                scope: None,
                limit: Some(50),
                edited_after: None,
            })
            .unwrap()
        {
            Response::Search(hits) => hits.into_iter().map(|hit| hit.page_id).collect(),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(found(&mut core), BTreeSet::from([a, b]));

    // Into the template: the two pages are in a template now, which search leaves out.
    bulk(
        &mut core,
        space,
        &[a, b],
        BulkAction::Move {
            parent_id: Some(template),
        },
    )
    .unwrap();
    assert_eq!(found(&mut core), BTreeSet::new());
    assert_eq!(excerpt_of(&mut core, space, a), None);

    // Out of it: found again.
    bulk(&mut core, space, &[b], BulkAction::Move { parent_id: None }).unwrap();
    assert_eq!(found(&mut core), BTreeSet::from([b]));
    assert_eq!(
        excerpt_of(&mut core, space, b).as_deref(),
        Some("a pomegranate list")
    );
}

// ----- Duplicate ---------------------------------------------------------------------------------

#[test]
fn duplicate_copies_each_page_with_its_subtree_right_after_it_in_one_undo_step() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let a1 = create_page(&mut core, space, Some(a), "A1").id;
    let b = create_page(&mut core, space, None, "B").id;
    create_page(&mut core, space, None, "C");
    let amount = prop_create(&mut core, space, "Amount", PropertyKind::Number, None).id;
    bulk(
        &mut core,
        space,
        &[a],
        BulkAction::SetValue {
            property_id: amount,
            value: number(1.0),
        },
    )
    .unwrap();
    bulk(
        &mut core,
        space,
        &[a1],
        BulkAction::SetValue {
            property_id: amount,
            value: number(2.0),
        },
    )
    .unwrap();
    write_text(&mut core, a, "hello from a", 1);
    write_text(&mut core, a1, "hello from a1", 2);
    assert_eq!(outline_of(&mut core, space), "A\n  A1\nB\nC\n");
    core.drain_events();

    // `a1` goes with `a`: copied once, inside the copy of `a`. The answer is the top copies in the
    // order given.
    let answer = bulk(&mut core, space, &[b, a1, a], BulkAction::Duplicate).unwrap();
    assert_eq!(answer.len(), 2);
    let (b_copy, a_copy) = (answer[0], answer[1]);
    assert_eq!(page(&mut core, b_copy).title, "B (copy)");
    assert_eq!(page(&mut core, a_copy).title, "A (copy)");
    assert_eq!(
        outline_of(&mut core, space),
        "A\n  A1\nA (copy)\n  A1\nB\nB (copy)\nC\n",
        "each copy right after its page"
    );
    // The values and the content of the copies, the subtree included.
    assert_eq!(value_of(&mut core, a_copy, amount), number(1.0));
    let view = tree(&mut core, space);
    let a_copy_node = view
        .roots
        .iter()
        .find(|node| node.page.id == a_copy)
        .unwrap();
    let a1_copy = a_copy_node.children[0].page.id;
    assert_ne!(a1_copy, a1);
    assert_eq!(value_of(&mut core, a1_copy, amount), number(2.0));
    assert!(
        page_content(&mut core, a_copy)
            .to_string()
            .contains("hello from a")
    );
    assert!(
        page_content(&mut core, a1_copy)
            .to_string()
            .contains("hello from a1")
    );
    // The originals are untouched.
    assert_eq!(value_of(&mut core, a, amount), number(1.0));
    assert!(
        page_content(&mut core, a)
            .to_string()
            .contains("hello from a")
    );
    let events = core.drain_events();
    assert_eq!(
        count(&events, &Event::StructureChanged { space_id: space }),
        1
    );
    assert_eq!(count(&events, &Event::SpacesChanged), 1);

    // One undo removes every copy; the next one is the step before (the creation of C).
    assert_eq!(undo(&mut core, space), "A\n  A1\nB\nC\n");
    assert_eq!(undo(&mut core, space), "A\n  A1\nB\n");
    assert_eq!(redo(&mut core, space), "A\n  A1\nB\nC\n");
    assert_eq!(
        redo(&mut core, space),
        "A\n  A1\nA (copy)\n  A1\nB\nB (copy)\nC\n"
    );
}

#[test]
fn duplicate_copies_at_most_1000_pages_in_all() {
    let (_fx, mut core, space) = world();
    let root = PageId::from_bytes([0x11; 16]);
    let mut pages = vec![NewPage {
        page_id: root,
        parent: None,
        title: "Root".into(),
        icon: None,
        cover: None,
        cover_y: None,
    }];
    for n in 0..1_000u16 {
        pages.push(NewPage {
            page_id: stranger(n),
            parent: Some(root),
            title: format!("Child {n}"),
            icon: None,
            cover: None,
            cover_y: None,
        });
    }
    core.mutate_structure(space, |model, now| model.create_pages(&pages, now))
        .unwrap();
    let small = create_page(&mut core, space, None, "Small").id;
    core.drain_events();
    let live = spaces(&mut core)[0].page_count;
    assert_eq!(live, 1_002);

    // 1,001 pages in the subtree of the root; its child goes with it and changes nothing.
    for pages in [vec![root], vec![stranger(5), root], vec![small, root]] {
        let err = bulk(&mut core, space, &pages, BulkAction::Duplicate).unwrap_err();
        assert_eq!(field(err), "pageIds");
    }
    assert_eq!(spaces(&mut core)[0].page_count, live, "nothing was copied");
    assert!(core.drain_events().is_empty());
    // Under the limit it goes through.
    let answer = bulk(&mut core, space, &[small], BulkAction::Duplicate).unwrap();
    assert_eq!(answer.len(), 1);
}

// ----- Trash -------------------------------------------------------------------------------------

#[test]
fn trash_moves_each_page_with_its_subtree_to_the_trash_in_one_undo_step() {
    let (_fx, mut core, space) = world();
    let a = create_page(&mut core, space, None, "A").id;
    let a1 = create_page(&mut core, space, Some(a), "A1").id;
    let b = create_page(&mut core, space, None, "B").id;
    let c = create_page(&mut core, space, None, "C").id;
    core.drain_events();

    let answer = bulk(&mut core, space, &[a1, c, a], BulkAction::Trash).unwrap();
    assert_eq!(answer, [c, a], "the pages acted on, without the descendant");
    assert_eq!(outline_of(&mut core, space), "B\n");
    let view = tree(&mut core, space);
    let trashed: BTreeSet<PageId> = view.trashed.iter().map(|t| t.page.id).collect();
    assert_eq!(
        trashed,
        BTreeSet::from([a, c]),
        "the descendant is not a top of the trash"
    );
    let a_entry = view.trashed.iter().find(|t| t.page.id == a).unwrap();
    assert_eq!(a_entry.descendant_count, 1);
    assert_eq!(page(&mut core, a1).trashed_at, a_entry.page.trashed_at);
    let events = core.drain_events();
    assert_eq!(
        count(&events, &Event::StructureChanged { space_id: space }),
        1
    );
    assert_eq!(count(&events, &Event::SpacesChanged), 1);
    assert_eq!(spaces(&mut core)[0].page_count, 1);

    // One undo brings every page back; the next one is the step before (the creation of C).
    assert_eq!(undo(&mut core, space), "A\n  A1\nB\nC\n");
    assert!(tree(&mut core, space).trashed.is_empty());
    assert_eq!(undo(&mut core, space), "A\n  A1\nB\n");
    assert_eq!(redo(&mut core, space), "A\n  A1\nB\nC\n");
    assert_eq!(redo(&mut core, space), "B\n");

    // A page already in the trash is no live page.
    let err = bulk(&mut core, space, &[b, a], BulkAction::Trash).unwrap_err();
    assert_eq!(field(err), "pageIds");
    assert_eq!(outline_of(&mut core, space), "B\n");
}

#[test]
fn each_action_persists_one_update_of_the_structure_document() {
    use kb_storage::DocQueries;

    let (_fx, mut core, space) = world();
    let pages: Vec<PageId> = ["A", "B", "C", "D"]
        .iter()
        .map(|title| create_page(&mut core, space, None, title).id)
        .collect();
    let amount = prop_create(&mut core, space, "Amount", PropertyKind::Number, None).id;
    let doc_id = core.structure(space).unwrap().doc_id;
    let updates = |core: &Core| core.db.update_stats(doc_id).unwrap().count;

    for (what, action) in [
        (
            "SetValue",
            BulkAction::SetValue {
                property_id: amount,
                value: number(1.0),
            },
        ),
        (
            "SetValues",
            BulkAction::SetValues {
                property_id: amount,
                values: vec![number(1.0), number(2.0), number(3.0), None],
            },
        ),
        (
            "Move",
            BulkAction::Move {
                parent_id: Some(pages[0]),
            },
        ),
        ("Duplicate", BulkAction::Duplicate),
    ] {
        let before = updates(&core);
        let targets: &[PageId] = if what == "Move" { &pages[1..] } else { &pages };
        bulk(&mut core, space, targets, action).unwrap();
        assert_eq!(updates(&core), before + 1, "{what}");
    }
    let before = updates(&core);
    bulk(&mut core, space, &pages[1..], BulkAction::Trash).unwrap();
    assert_eq!(updates(&core), before + 1, "Trash");
}

#[test]
fn every_action_survives_a_reopen() {
    let fx = Fixture::new();
    let (space, amount, copy_of_b, trashed) = {
        let mut core = fx.open(1, 1_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = spaces(&mut core)[0].id;
        let a = create_page(&mut core, space, None, "A").id;
        let b = create_page(&mut core, space, None, "B").id;
        let c = create_page(&mut core, space, None, "C").id;
        let d = create_page(&mut core, space, None, "D").id;
        let amount = prop_create(&mut core, space, "Amount", PropertyKind::Number, None).id;
        bulk(
            &mut core,
            space,
            &[a, b],
            BulkAction::SetValue {
                property_id: amount,
                value: number(4.0),
            },
        )
        .unwrap();
        bulk(
            &mut core,
            space,
            &[b],
            BulkAction::Move { parent_id: Some(a) },
        )
        .unwrap();
        let copy = bulk(&mut core, space, &[c], BulkAction::Duplicate).unwrap()[0];
        bulk(&mut core, space, &[d], BulkAction::Trash).unwrap();
        (space, amount, copy, d)
    };

    let mut core = fx.open(2, 2_000);
    assert_eq!(outline_of(&mut core, space), "A\n  B\nC\nC (copy)\n");
    assert_eq!(tree(&mut core, space).trashed[0].page.id, trashed);
    assert_eq!(page(&mut core, copy_of_b).title, "C (copy)");
    let with_value = by_property(
        &mut core,
        space,
        amount,
        PropertyFilter::Equals(PropertyValue::Number(4.0)),
    );
    assert_eq!(with_value.len(), 2, "the derived rows were persisted");
}

// ----- the role gate -----------------------------------------------------------------------------

#[test]
fn a_viewer_may_read_excerpts_but_no_bulk_action_is_accepted() {
    let server = server();
    let (mut alice, _) = Dev::first("alice", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    alice.configure();
    bob.configure();
    let space = create_space(&mut alice.core, "Project").id;
    let plan = create_page(&mut alice.core, space, None, "Plan").id;
    alice.write(plan, "base line");
    let feed = share(&mut alice, space, "Project", "Alice");
    add_member(&mut alice, &mut bob, feed, space, MemberRole::Viewer, "Bob");
    Dev::settle(&mut [&mut alice, &mut bob], 2);

    for action in [
        BulkAction::Trash,
        BulkAction::Duplicate,
        BulkAction::Move { parent_id: None },
        BulkAction::SetValue {
            property_id: PropertyId::from_bytes([1; 16]),
            value: None,
        },
        BulkAction::SetValues {
            property_id: PropertyId::from_bytes([1; 16]),
            values: vec![None],
        },
    ] {
        let err = bulk(&mut bob.core, space, &[plan], action).unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
    }
    assert_eq!(outline_of(&mut bob.core, space), "Plan\n");

    // Any member may read the excerpts.
    let read = excerpts(&mut bob.core, space, &[plan]).unwrap();
    assert!(read.len() <= 1);

    // The owner may.
    let answer = bulk(&mut alice.core, space, &[plan], BulkAction::Duplicate).unwrap();
    assert_eq!(answer.len(), 1);
    assert_eq!(outline_of(&mut alice.core, space), "Plan\nPlan (copy)\n");
}

// ----- excerpts ----------------------------------------------------------------------------------

#[test]
fn an_excerpt_joins_the_rows_of_the_page_in_document_order() {
    let (_fx, mut core, space) = world();
    let p = create_page(&mut core, space, None, "P").id;
    write_text(&mut core, p, "  first   row \t", 1);
    write_text(&mut core, p, "second\nrow", 2);
    write_text(&mut core, p, "   ", 3);
    write_text(&mut core, p, "third", 4);
    assert_eq!(
        excerpt_of(&mut core, space, p).as_deref(),
        Some("first row\nsecond row\nthird"),
        "whitespace runs are one space, empty rows are skipped, rows are joined with a line break"
    );

    // The private-use highlight markers the index stores as U+FFFD (and the inline atoms) go.
    let q = create_page(&mut core, space, None, "Q").id;
    write_text(&mut core, q, "a\u{E000}b \u{E001} c", 5);
    assert_eq!(excerpt_of(&mut core, space, q).as_deref(), Some("ab c"));
}

#[test]
fn an_excerpt_is_cut_at_280_characters_at_a_word_with_an_ellipsis() {
    let (_fx, mut core, space) = world();
    let words = create_page(&mut core, space, None, "Words").id;
    write_text(&mut core, words, &["word"; 100].join(" "), 1);
    // 56 words are 279 characters; the 280th is the space after them.
    assert_eq!(
        excerpt_of(&mut core, space, words),
        Some(format!("{}…", ["word"; 56].join(" ")))
    );

    let long = create_page(&mut core, space, None, "Long word").id;
    write_text(&mut core, long, &"x".repeat(400), 2);
    assert_eq!(
        excerpt_of(&mut core, space, long),
        Some(format!("{}…", "x".repeat(280))),
        "no whitespace: cut at the limit"
    );

    let exact = create_page(&mut core, space, None, "Exact").id;
    write_text(&mut core, exact, &"y".repeat(280), 3);
    assert_eq!(excerpt_of(&mut core, space, exact), Some("y".repeat(280)));
    let over = create_page(&mut core, space, None, "Over").id;
    write_text(&mut core, over, &"y".repeat(281), 4);
    assert_eq!(
        excerpt_of(&mut core, space, over),
        Some(format!("{}…", "y".repeat(280)))
    );

    // Unicode scalar values, not UTF-16 units or bytes.
    let emoji = create_page(&mut core, space, None, "Emoji").id;
    write_text(&mut core, emoji, &"😀".repeat(300), 5);
    assert_eq!(
        excerpt_of(&mut core, space, emoji),
        Some(format!("{}…", "😀".repeat(280)))
    );

    // Several rows: the line break counts as a character and is a place to cut.
    let rows = create_page(&mut core, space, None, "Rows").id;
    write_text(&mut core, rows, &"a".repeat(100), 6);
    write_text(&mut core, rows, &["bb"; 100].join(" "), 7);
    let text = excerpt_of(&mut core, space, rows).unwrap();
    assert!(
        text.starts_with(&format!("{}\nbb bb", "a".repeat(100))),
        "{text}"
    );
    assert!(text.ends_with("bb…"), "{text}");
    assert!(text.chars().count() <= 281, "{}", text.chars().count());
}

#[test]
fn the_answer_follows_the_order_asked_and_leaves_out_what_has_no_excerpt() {
    let (_fx, mut core, space) = world();
    let one = create_page(&mut core, space, None, "One").id;
    let two = create_page(&mut core, space, None, "Two").id;
    let empty = create_page(&mut core, space, None, "Empty").id;
    write_text(&mut core, one, "one text", 1);
    write_text(&mut core, two, "two text", 2);

    let template = create_page(&mut core, space, None, "Template").id;
    write_text(&mut core, template, "template text", 3);
    let inside = create_page(&mut core, space, Some(template), "Inside").id;
    write_text(&mut core, inside, "inside text", 4);
    core.dispatch(Request::PageSetTemplate {
        page_id: template,
        template: true,
    })
    .unwrap();

    let gone = create_page(&mut core, space, None, "Gone").id;
    write_text(&mut core, gone, "gone text", 5);
    trash(&mut core, gone);

    let other = create_space(&mut core, "Other").id;
    let foreign = create_page(&mut core, other, None, "Foreign").id;
    write_text(&mut core, foreign, "foreign text", 6);

    let asked = [
        two,
        empty,
        one,
        template,
        inside,
        gone,
        foreign,
        stranger(3),
        two,
    ];
    let answer = excerpts(&mut core, space, &asked).unwrap();
    let read: Vec<(PageId, &str)> = answer
        .iter()
        .map(|excerpt| (excerpt.page_id, excerpt.text.as_str()))
        .collect();
    assert_eq!(
        read,
        [(two, "two text"), (one, "one text")],
        "the order asked, each page once; no empty page, template, page of a template, trashed or foreign page"
    );
    assert!(excerpts(&mut core, space, &[]).unwrap().is_empty());
    // Another space answers its own pages.
    assert_eq!(
        excerpt_of(&mut core, other, foreign).as_deref(),
        Some("foreign text")
    );
    assert_eq!(excerpt_of(&mut core, space, foreign), None);
}

#[test]
fn excerpts_are_limited_to_500_pages_and_need_a_known_space() {
    let (_fx, mut core, space) = world();
    let ids: Vec<PageId> = (0..501).map(stranger).collect();
    let err = excerpts(&mut core, space, &ids).unwrap_err();
    assert_eq!(field(err), "pageIds");
    assert!(excerpts(&mut core, space, &ids[..500]).unwrap().is_empty());
    let err = excerpts(&mut core, SpaceId::from_bytes([7; 16]), &ids[..1]).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn the_text_rule_in_isolation() {
    // One row.
    assert_eq!(excerpt_row("  a \t b\n\nc  ").as_deref(), Some("a b c"));
    assert_eq!(excerpt_row("a\u{FFFD}b").as_deref(), Some("ab"));
    assert_eq!(excerpt_row("a \u{FFFD} b").as_deref(), Some("a b"));
    assert_eq!(excerpt_row(" \u{FFFD}\t\u{3000}\u{a0}\u{FFFD} "), None);
    assert_eq!(excerpt_row(""), None);
    assert_eq!(excerpt_row("é 😀").as_deref(), Some("é 😀"));
    // The cut.
    assert_eq!(cut_excerpt("short".into()), "short");
    assert_eq!(cut_excerpt("a".repeat(280)), "a".repeat(280));
    assert_eq!(
        cut_excerpt("a".repeat(281)),
        format!("{}…", "a".repeat(280))
    );
    let spaced = format!("{} {}", "a".repeat(100), "b".repeat(300));
    assert_eq!(cut_excerpt(spaced), format!("{}…", "a".repeat(100)));
    // The line break is whitespace.
    let rows = format!("{}\n{}", "a".repeat(100), "b".repeat(300));
    assert_eq!(cut_excerpt(rows), format!("{}…", "a".repeat(100)));
}
