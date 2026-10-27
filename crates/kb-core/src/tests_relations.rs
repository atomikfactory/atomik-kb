//! Relations and rollups (ADR-0027) through `Request`: creating a two-way relation (two properties
//! in one commit), the symmetric reads, the filters, the search text and its refresh, purging,
//! deleting, rollups and their configuration. The two-device scenario is in `tests_sync.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::PagePropertyQueries;
use kb_types::{
    ApiError, DateValue, ErrorCode, Event, PageId, PropertyConfig, PropertyDef, PropertyFilter,
    PropertyId, PropertyKind, PropertyValue, Request, Response, RollupFunction, RollupResult,
    RollupTarget, RollupValue, SearchHit, SpaceId,
};

use crate::Core;
use crate::test_support::{Fixture, create_page, create_workspace};

fn new_page(core: &mut Core, space: SpaceId, title: &str) -> PageId {
    create_page(core, space, None, title).id
}

fn setup() -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = match core.dispatch(Request::SpaceList).unwrap() {
        Response::Spaces(spaces) => spaces[0].id,
        other => panic!("unexpected {other:?}"),
    };
    core.drain_events();
    (fx, core, space)
}

fn call(core: &mut Core, request: Request) -> Response {
    core.dispatch(request).unwrap()
}

fn fails(core: &mut Core, request: Request) -> ApiError {
    core.dispatch(request).unwrap_err()
}

fn schema(core: &mut Core, space_id: SpaceId) -> Vec<PropertyDef> {
    match call(core, Request::PropertySchemaGet { space_id }) {
        Response::PropertySchema(schema) => schema.properties,
        other => panic!("unexpected {other:?}"),
    }
}

fn create(
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

fn two_way(core: &mut Core, space_id: SpaceId, name: &str) -> (PropertyDef, PropertyDef) {
    let forward = create(
        core,
        space_id,
        name,
        PropertyKind::Relation,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: None,
        }),
    );
    let Some(PropertyConfig::Relation {
        paired_property_id: Some(back_id),
        ..
    }) = forward.config
    else {
        panic!("not paired: {forward:?}");
    };
    let back = schema(core, space_id)
        .into_iter()
        .find(|def| def.id == back_id)
        .unwrap();
    (forward, back)
}

fn set(core: &mut Core, page_id: PageId, property_id: PropertyId, value: Option<PropertyValue>) {
    call(
        core,
        Request::PagePropertySet {
            page_id,
            property_id,
            value,
        },
    );
}

fn relation(pages: &[PageId]) -> PropertyValue {
    PropertyValue::Relation(pages.to_vec())
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

/// The value as the page info of the tree shows it.
fn tree_value(core: &mut Core, page_id: PageId, property_id: PropertyId) -> Option<PropertyValue> {
    match call(core, Request::PageGet { page_id }) {
        Response::Page(info) => info
            .properties
            .into_iter()
            .find(|(id, _)| *id == property_id)
            .map(|(_, value)| value),
        other => panic!("unexpected {other:?}"),
    }
}

fn by(
    core: &mut Core,
    space_id: SpaceId,
    property_id: PropertyId,
    filter: PropertyFilter,
) -> Vec<PageId> {
    match call(
        core,
        Request::PagesByProperty {
            space_id,
            property_id,
            filter,
        },
    ) {
        Response::PageIds(ids) => ids,
        other => panic!("unexpected {other:?}"),
    }
}

fn search(core: &mut Core, query: &str) -> Vec<SearchHit> {
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
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

fn invalid_field(err: &ApiError) -> Option<String> {
    (err.code == ErrorCode::InvalidInput)
        .then(|| {
            err.details
                .as_ref()?
                .get("field")?
                .as_str()
                .map(str::to_owned)
        })
        .flatten()
}

// ----- two-way relations -----------------------------------------------------------------------------

#[test]
fn a_two_way_relation_creates_two_properties_that_name_each_other_in_one_change() {
    let (_fx, mut core, space) = setup();
    let (forward, back) = two_way(&mut core, space, "Project");
    assert_eq!(forward.name, "Project");
    assert_eq!(back.name, "Project (back)");
    assert_eq!(
        forward.config,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(back.id)
        })
    );
    assert_eq!(
        back.config,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(forward.id)
        })
    );
    let events = core.drain_events();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::PropertySchemaChanged { .. }))
            .count(),
        1,
        "one change, one event: {events:?}"
    );
    // A very long name still leaves room for the suffix of the back property.
    let long = "x".repeat(100);
    let (_, back) = two_way(&mut core, space, &long);
    assert_eq!(back.name.chars().count(), 100);
    assert!(back.name.ends_with(" (back)"));
    // The creation request refuses a configuration for another kind, and a rollup without one.
    for (kind, config) in [
        (
            PropertyKind::Text,
            Some(PropertyConfig::Relation {
                two_way: false,
                paired_property_id: None,
            }),
        ),
        (PropertyKind::Rollup, None),
    ] {
        let err = fails(
            &mut core,
            Request::PropertyCreate {
                space_id: space,
                name: "x".into(),
                kind,
                config,
            },
        );
        assert_eq!(invalid_field(&err).as_deref(), Some("config"), "{err:?}");
    }
}

#[test]
fn the_values_of_a_two_way_relation_are_symmetric_in_every_read() {
    let (_fx, mut core, space) = setup();
    let a = new_page(&mut core, space, "Alpha");
    let b = new_page(&mut core, space, "Beta");
    let c = new_page(&mut core, space, "Gamma");
    let (forward, back) = two_way(&mut core, space, "Related");
    set(&mut core, a, forward.id, Some(relation(&[b, c])));
    for page in [b, c] {
        assert_eq!(value_of(&mut core, page, back.id), Some(relation(&[a])));
        assert_eq!(tree_value(&mut core, page, back.id), Some(relation(&[a])));
    }
    assert_eq!(
        tree_value(&mut core, a, forward.id),
        Some(relation(&[b, c]))
    );
    // The filters see both directions.
    assert_eq!(
        by(&mut core, space, forward.id, PropertyFilter::RelatesTo(b)),
        [a]
    );
    assert_eq!(
        by(&mut core, space, back.id, PropertyFilter::RelatesTo(a)),
        [b, c]
    );
    assert_eq!(
        by(&mut core, space, forward.id, PropertyFilter::HasValue),
        [a]
    );
    assert_eq!(
        by(&mut core, space, back.id, PropertyFilter::HasValue),
        [b, c]
    );
    // Writing the back side shows on the forward side.
    set(&mut core, c, back.id, Some(relation(&[a, b])));
    assert_eq!(
        value_of(&mut core, b, forward.id),
        Some(relation(&[c])),
        "b is now listed by c"
    );
    assert_eq!(
        by(&mut core, space, forward.id, PropertyFilter::RelatesTo(c)),
        [a, b],
        "c lists a and b on its back side, so both list c"
    );
    // Removing: both sides go.
    set(&mut core, a, forward.id, None);
    assert_eq!(value_of(&mut core, b, back.id), None);
    assert_eq!(value_of(&mut core, c, back.id), Some(relation(&[b])));
    assert_eq!(
        by(&mut core, space, back.id, PropertyFilter::RelatesTo(a)),
        Vec::<PageId>::new()
    );
}

#[test]
fn a_one_way_relation_and_the_refused_filters() {
    let (_fx, mut core, space) = setup();
    let a = new_page(&mut core, space, "Alpha");
    let b = new_page(&mut core, space, "Beta");
    let links = create(&mut core, space, "Links", PropertyKind::Relation, None);
    set(&mut core, a, links.id, Some(relation(&[b])));
    assert_eq!(value_of(&mut core, b, links.id), None);
    assert_eq!(
        by(&mut core, space, links.id, PropertyFilter::RelatesTo(b)),
        [a]
    );
    assert_eq!(
        by(&mut core, space, links.id, PropertyFilter::HasValue),
        [a]
    );
    assert_eq!(
        by(&mut core, space, links.id, PropertyFilter::RelatesTo(a)),
        Vec::<PageId>::new()
    );
    for filter in [
        PropertyFilter::Equals(relation(&[b])),
        PropertyFilter::TextContains("x".into()),
        PropertyFilter::Checked(true),
    ] {
        let err = fails(
            &mut core,
            Request::PagesByProperty {
                space_id: space,
                property_id: links.id,
                filter,
            },
        );
        assert_eq!(invalid_field(&err).as_deref(), Some("filter"), "{err:?}");
    }
    // RelatesTo on another kind.
    let notes = create(&mut core, space, "Notes", PropertyKind::Text, None);
    let err = fails(
        &mut core,
        Request::PagesByProperty {
            space_id: space,
            property_id: notes.id,
            filter: PropertyFilter::RelatesTo(a),
        },
    );
    assert_eq!(invalid_field(&err).as_deref(), Some("filter"));
    // A value of the wrong shape.
    let err = fails(
        &mut core,
        Request::PagePropertySet {
            page_id: a,
            property_id: links.id,
            value: Some(PropertyValue::Text("x".into())),
        },
    );
    assert_eq!(invalid_field(&err).as_deref(), Some("value"));
}

#[test]
fn trashed_related_pages_stay_listed_and_are_not_in_the_filters_answer_but_purged_ones_leave() {
    let (_fx, mut core, space) = setup();
    let a = new_page(&mut core, space, "Alpha");
    let b = new_page(&mut core, space, "Beta");
    let (forward, back) = two_way(&mut core, space, "Related");
    set(&mut core, a, forward.id, Some(relation(&[b])));
    call(&mut core, Request::PageTrash { page_id: b });
    assert_eq!(value_of(&mut core, a, forward.id), Some(relation(&[b])));
    assert_eq!(
        by(&mut core, space, back.id, PropertyFilter::RelatesTo(a)),
        Vec::<PageId>::new(),
        "only live pages are answers"
    );
    assert_eq!(
        by(&mut core, space, forward.id, PropertyFilter::RelatesTo(b)),
        [a],
        "the trashed page may be the one asked about"
    );
    call(&mut core, Request::PagePurge { page_id: b });
    assert_eq!(value_of(&mut core, a, forward.id), None);
    assert_eq!(
        by(&mut core, space, forward.id, PropertyFilter::HasValue),
        Vec::<PageId>::new()
    );
    // The derived rows and the search text of a no longer list the purged page.
    assert!(
        core.db
            .page_properties_pages_relating_to(b)
            .unwrap()
            .is_empty()
    );
    assert!(search(&mut core, "Beta").is_empty());
}

#[test]
fn the_search_text_of_a_relation_holds_the_titles_and_follows_a_rename() {
    let (_fx, mut core, space) = setup();
    let a = new_page(&mut core, space, "Alpha");
    let b = new_page(&mut core, space, "Quarterly roadmap");
    let links = create(&mut core, space, "Links", PropertyKind::Relation, None);
    set(&mut core, a, links.id, Some(relation(&[b])));
    let hits = search(&mut core, "roadmap");
    assert!(
        hits.iter()
            .any(|hit| hit.page_id == a && hit.block_id.is_none()),
        "a finds b's title through its relation: {hits:?}"
    );
    call(
        &mut core,
        Request::PageRename {
            page_id: b,
            title: "Yearly horizon".into(),
        },
    );
    assert!(
        !search(&mut core, "roadmap")
            .iter()
            .any(|hit| hit.page_id == a),
        "the old title leaves the text of a"
    );
    assert!(
        search(&mut core, "horizon")
            .iter()
            .any(|hit| hit.page_id == a),
        "the new title is in the text of a"
    );
}

#[test]
fn deleting_one_property_of_a_pair_keeps_the_other_as_a_one_way_relation() {
    let (_fx, mut core, space) = setup();
    let a = new_page(&mut core, space, "Alpha");
    let b = new_page(&mut core, space, "Beta");
    let (forward, back) = two_way(&mut core, space, "Related");
    set(&mut core, a, forward.id, Some(relation(&[b])));
    call(
        &mut core,
        Request::PropertyDelete {
            space_id: space,
            property_id: back.id,
        },
    );
    let remaining = schema(&mut core, space)
        .into_iter()
        .find(|def| def.id == forward.id)
        .unwrap();
    assert_eq!(
        remaining.config,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None
        })
    );
    assert_eq!(value_of(&mut core, a, forward.id), Some(relation(&[b])));
    assert_eq!(
        by(&mut core, space, forward.id, PropertyFilter::RelatesTo(b)),
        [a]
    );
}

// ----- rollups ---------------------------------------------------------------------------------------------

fn rollup(
    core: &mut Core,
    space: SpaceId,
    relation: PropertyId,
    target: RollupTarget,
    function: RollupFunction,
) -> PropertyDef {
    create(
        core,
        space,
        "Rollup",
        PropertyKind::Rollup,
        Some(PropertyConfig::Rollup {
            relation_property_id: relation,
            target,
            function,
        }),
    )
}

#[test]
fn a_rollup_is_computed_into_the_page_info_follows_the_related_pages_and_is_read_only() {
    let (_fx, mut core, space) = setup();
    let project = new_page(&mut core, space, "Project");
    let task1 = new_page(&mut core, space, "Task 1");
    let task2 = new_page(&mut core, space, "Task 2");
    let (tasks, project_of) = two_way(&mut core, space, "Tasks");
    let cost = create(&mut core, space, "Cost", PropertyKind::Number, None);
    let sum = rollup(
        &mut core,
        space,
        tasks.id,
        RollupTarget::Property(cost.id),
        RollupFunction::Sum,
    );
    set(
        &mut core,
        project,
        tasks.id,
        Some(relation(&[task1, task2])),
    );
    set(&mut core, task1, cost.id, Some(PropertyValue::Number(10.0)));
    set(&mut core, task2, cost.id, Some(PropertyValue::Number(2.5)));
    let expected = PropertyValue::Rollup(RollupValue {
        function: RollupFunction::Sum,
        result: RollupResult::Number(12.5),
    });
    assert_eq!(
        tree_value(&mut core, project, sum.id),
        Some(expected.clone())
    );
    assert_eq!(value_of(&mut core, project, sum.id), Some(expected));

    // A change of a related page's value is announced (StructureChanged) and read again.
    core.drain_events();
    set(&mut core, task2, cost.id, Some(PropertyValue::Number(7.5)));
    assert!(
        core.drain_events()
            .iter()
            .any(|e| matches!(e, Event::StructureChanged { .. }))
    );
    assert_eq!(
        tree_value(&mut core, project, sum.id),
        Some(PropertyValue::Rollup(RollupValue {
            function: RollupFunction::Sum,
            result: RollupResult::Number(17.5)
        }))
    );
    // The back side of the pair rolls up too: a task's own rollup sees its project.
    let titles = rollup(
        &mut core,
        space,
        project_of.id,
        RollupTarget::Title,
        RollupFunction::ShowOriginal,
    );
    assert_eq!(
        tree_value(&mut core, task1, titles.id),
        Some(PropertyValue::Rollup(RollupValue {
            function: RollupFunction::ShowOriginal,
            result: RollupResult::Values(vec![PropertyValue::Text("Project".into())])
        }))
    );
    // Read-only, and not searched through the derived rows.
    let err = fails(
        &mut core,
        Request::PagePropertySet {
            page_id: project,
            property_id: sum.id,
            value: Some(PropertyValue::Number(1.0)),
        },
    );
    assert_eq!(invalid_field(&err).as_deref(), Some("propertyId"));
    let err = fails(
        &mut core,
        Request::PagePropertySet {
            page_id: project,
            property_id: sum.id,
            value: None,
        },
    );
    assert_eq!(invalid_field(&err).as_deref(), Some("propertyId"));
    // HasValue and Equals work on the computed value (a sum of nothing is 0, so every page has
    // one); the other filters are refused.
    assert_eq!(
        by(&mut core, space, sum.id, PropertyFilter::HasValue),
        [project, task1, task2]
    );
    assert_eq!(
        by(
            &mut core,
            space,
            sum.id,
            PropertyFilter::Equals(PropertyValue::Rollup(RollupValue {
                function: RollupFunction::Sum,
                result: RollupResult::Number(17.5)
            }))
        ),
        [project]
    );
    let err = fails(
        &mut core,
        Request::PagesByProperty {
            space_id: space,
            property_id: sum.id,
            filter: PropertyFilter::RelatesTo(project),
        },
    );
    assert_eq!(invalid_field(&err).as_deref(), Some("filter"));
}

#[test]
fn a_rollup_configuration_is_checked_and_can_be_changed() {
    let (_fx, mut core, space) = setup();
    let links = create(&mut core, space, "Links", PropertyKind::Relation, None);
    let cost = create(&mut core, space, "Cost", PropertyKind::Number, None);
    let sum = rollup(
        &mut core,
        space,
        links.id,
        RollupTarget::Property(cost.id),
        RollupFunction::Sum,
    );
    let bad = Request::PropertyCreate {
        space_id: space,
        name: "Bad".into(),
        kind: PropertyKind::Rollup,
        config: Some(PropertyConfig::Rollup {
            relation_property_id: links.id,
            target: RollupTarget::Title,
            function: RollupFunction::Sum,
        }),
    };
    assert_eq!(
        invalid_field(&fails(&mut core, bad)).as_deref(),
        Some("config")
    );
    let updated = match call(
        &mut core,
        Request::PropertyConfigSet {
            space_id: space,
            property_id: sum.id,
            config: PropertyConfig::Rollup {
                relation_property_id: links.id,
                target: RollupTarget::Property(cost.id),
                function: RollupFunction::Max,
            },
        },
    ) {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        updated.config,
        Some(PropertyConfig::Rollup {
            relation_property_id: links.id,
            target: RollupTarget::Property(cost.id),
            function: RollupFunction::Max,
        })
    );
    let err = fails(
        &mut core,
        Request::PropertyConfigSet {
            space_id: space,
            property_id: links.id,
            config: PropertyConfig::Relation {
                two_way: true,
                paired_property_id: None,
            },
        },
    );
    assert_eq!(invalid_field(&err).as_deref(), Some("config"));
}

#[test]
fn a_rollup_of_dates_counts_and_checklists_over_the_real_pages() {
    let (_fx, mut core, space) = setup();
    let hub = new_page(&mut core, space, "Hub");
    let one = new_page(&mut core, space, "One");
    let two = new_page(&mut core, space, "Two");
    let links = create(&mut core, space, "Links", PropertyKind::Relation, None);
    let due = create(&mut core, space, "Due", PropertyKind::Date, None);
    let done = create(&mut core, space, "Done", PropertyKind::Checkbox, None);
    set(&mut core, hub, links.id, Some(relation(&[one, two])));
    let day = |date: &str| {
        Some(PropertyValue::Date(DateValue {
            date: date.into(),
            time: None,
        }))
    };
    set(&mut core, one, due.id, day("2026-10-09"));
    set(&mut core, two, due.id, day("2026-10-07"));
    set(&mut core, one, done.id, Some(PropertyValue::Checkbox(true)));
    let earliest = rollup(
        &mut core,
        space,
        links.id,
        RollupTarget::Property(due.id),
        RollupFunction::Earliest,
    );
    let percent = rollup(
        &mut core,
        space,
        links.id,
        RollupTarget::Property(done.id),
        RollupFunction::PercentChecked,
    );
    assert_eq!(
        tree_value(&mut core, hub, earliest.id),
        Some(PropertyValue::Rollup(RollupValue {
            function: RollupFunction::Earliest,
            result: RollupResult::Date(DateValue {
                date: "2026-10-07".into(),
                time: None
            })
        }))
    );
    assert_eq!(
        tree_value(&mut core, hub, percent.id),
        Some(PropertyValue::Rollup(RollupValue {
            function: RollupFunction::PercentChecked,
            result: RollupResult::Number(50.0)
        }))
    );
    // A rollup property of the page without relations has no value for an average, 0 for a count.
    assert_eq!(tree_value(&mut core, one, earliest.id), None);
}

#[test]
fn the_derived_rows_of_relations_are_rebuilt_by_an_index_rebuild() {
    let (_fx, mut core, space) = setup();
    let a = new_page(&mut core, space, "Alpha");
    let b = new_page(&mut core, space, "Beta");
    let (forward, back) = two_way(&mut core, space, "Related");
    set(&mut core, a, forward.id, Some(relation(&[b])));
    let before_a = core.db.page_properties_of(a).unwrap();
    let before_b = core.db.page_properties_of(b).unwrap();
    assert_eq!(before_a.len(), 1);
    assert_eq!(before_a[0].related_page_id, Some(b));
    assert_eq!(before_b[0].related_page_id, Some(a));
    assert_eq!(before_b[0].property_id, back.id);
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(core.db.page_properties_of(a).unwrap(), before_a);
    assert_eq!(core.db.page_properties_of(b).unwrap(), before_b);
    assert_eq!(
        by(&mut core, space, back.id, PropertyFilter::RelatesTo(a)),
        [b]
    );
}
