//! Typed page properties (ADR-0026) through `Request`: the schema, the values, the derived
//! `page_properties` table and the property rows of the text index, the queries, events, restarts,
//! rebuilds and backups. The two-device scenarios are in `tests_sync.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::{PagePropertyQueries, PropertyMatch};
use kb_types::properties::{MAX_NAME_CHARS, TAGS_PROPERTY_ID};
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, ChecklistItem, ChecklistItemId, DateValue, ErrorCode,
    Event, OptionColor, OptionId, PageId, PageProperties, PropertyDef, PropertyFilter, PropertyId,
    PropertyKind, PropertyOption, PropertyValue, Request, Response, SearchHit, SearchScope,
    SpaceId, StatusGroup,
};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, export_backup, import_backup, kit_create,
    page, tree,
};

/// An open core with a workspace and its first space.
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

fn prop_create(core: &mut Core, space_id: SpaceId, name: &str, kind: PropertyKind) -> PropertyDef {
    match call(
        core,
        Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config: None,
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
            color: OptionColor::Blue,
            group: None,
        },
    ) {
        Response::PropertyOption(option) => option,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_value(
    core: &mut Core,
    page_id: PageId,
    property_id: PropertyId,
    value: Option<PropertyValue>,
) -> PageProperties {
    match call(
        core,
        Request::PagePropertySet {
            page_id,
            property_id,
            value,
        },
    ) {
        Response::PageProperties(values) => values,
        other => panic!("unexpected {other:?}"),
    }
}

fn values(core: &mut Core, page_id: PageId) -> Vec<(PropertyId, PropertyValue)> {
    match call(core, Request::PagePropertiesGet { page_id }) {
        Response::PageProperties(values) => values.values,
        other => panic!("unexpected {other:?}"),
    }
}

fn by_property(
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

fn search(core: &mut Core, query: &str, scope: Option<SearchScope>) -> Vec<SearchHit> {
    match call(
        core,
        Request::Search {
            query: query.into(),
            space_id: None,
            scope,
            limit: Some(100),
            edited_after: None,
        },
    ) {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

fn text(value: &str) -> PropertyValue {
    PropertyValue::Text(value.into())
}

fn multi(options: &[OptionId]) -> PropertyValue {
    PropertyValue::MultiSelect(options.to_vec())
}

fn field_of(err: &ApiError) -> Option<&str> {
    err.details.as_ref()?.get("field")?.as_str()
}

fn events_of(core: &mut Core) -> Vec<Event> {
    core.drain_events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                Event::PropertySchemaChanged { .. }
                    | Event::StructureChanged { .. }
                    | Event::SpacesChanged
            )
        })
        .collect()
}

// ----- schema ---------------------------------------------------------------------------------

#[test]
fn a_new_space_lists_tags_first_with_no_options() {
    let (_fx, mut core, space) = setup();
    let listed = schema(&mut core, space);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, TAGS_PROPERTY_ID);
    assert_eq!(listed[0].name, "Tags");
    assert_eq!(listed[0].kind, PropertyKind::MultiSelect);
    assert!(listed[0].options.is_empty());
    let err = fails(
        &mut core,
        Request::PropertySchemaGet {
            space_id: SpaceId::from_bytes([9; 16]),
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn properties_are_created_renamed_moved_and_deleted_with_their_events() {
    let (_fx, mut core, space) = setup();
    let notes = prop_create(&mut core, space, "  Notes  ", PropertyKind::Text);
    assert_eq!(notes.name, "Notes", "names are trimmed");
    assert_eq!(notes.kind, PropertyKind::Text);
    assert_eq!(
        events_of(&mut core),
        [
            Event::PropertySchemaChanged { space_id: space },
            Event::StructureChanged { space_id: space }
        ],
        "a schema change says so first"
    );
    let stage = prop_create(&mut core, space, "Stage", PropertyKind::Status);
    assert_eq!(
        stage
            .options
            .iter()
            .map(|o| (o.name.as_str(), o.group, o.color))
            .collect::<Vec<_>>(),
        [
            ("Not started", Some(StatusGroup::Todo), OptionColor::Gray),
            (
                "In progress",
                Some(StatusGroup::InProgress),
                OptionColor::Blue
            ),
            ("Done", Some(StatusGroup::Done), OptionColor::Green),
        ]
    );
    let ids: Vec<OptionId> = stage.options.iter().map(|o| o.id).collect();
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number);
    core.drain_events();

    let names = |core: &mut Core| -> Vec<String> {
        schema(core, space).into_iter().map(|p| p.name).collect()
    };
    assert_eq!(names(&mut core), ["Tags", "Notes", "Stage", "Cost"]);

    match call(
        &mut core,
        Request::PropertyRename {
            space_id: space,
            property_id: notes.id,
            name: "Remarks".into(),
        },
    ) {
        Response::Property(def) => assert_eq!(def.name, "Remarks"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        call(
            &mut core,
            Request::PropertyMove {
                space_id: space,
                property_id: cost.id,
                before: Some(stage.id),
            }
        ),
        Response::Ok
    );
    assert_eq!(names(&mut core), ["Tags", "Remarks", "Cost", "Stage"]);
    assert_eq!(
        call(
            &mut core,
            Request::PropertyMove {
                space_id: space,
                property_id: cost.id,
                before: None,
            }
        ),
        Response::Ok
    );
    assert_eq!(names(&mut core), ["Tags", "Remarks", "Stage", "Cost"]);
    assert_eq!(
        events_of(&mut core).len(),
        6,
        "three changes, two events each"
    );

    assert_eq!(
        call(
            &mut core,
            Request::PropertyDelete {
                space_id: space,
                property_id: stage.id,
            }
        ),
        Response::Ok
    );
    assert_eq!(names(&mut core), ["Tags", "Remarks", "Cost"]);
}

#[test]
fn property_requests_are_validated_with_the_usual_error_shapes() {
    let (_fx, mut core, space) = setup();
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let select = prop_create(&mut core, space, "Kind", PropertyKind::Select);
    core.drain_events();
    let too_long = "x".repeat(MAX_NAME_CHARS + 1);
    for bad in ["", "   ", "tab\there", too_long.as_str()] {
        let err = fails(
            &mut core,
            Request::PropertyCreate {
                space_id: space,
                name: bad.into(),
                kind: PropertyKind::Text,
                config: None,
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
        assert_eq!(field_of(&err), Some("name"));
    }
    let err = fails(
        &mut core,
        Request::PropertyRename {
            space_id: space,
            property_id: notes.id,
            name: " ".into(),
        },
    );
    assert_eq!(field_of(&err), Some("name"));
    let unknown_space = SpaceId::from_bytes([9; 16]);
    let unknown_property = PropertyId::from_bytes([9; 16]);
    for request in [
        Request::PropertyCreate {
            space_id: unknown_space,
            name: "x".into(),
            kind: PropertyKind::Text,
            config: None,
        },
        Request::PropertyRename {
            space_id: space,
            property_id: unknown_property,
            name: "x".into(),
        },
        Request::PropertyDelete {
            space_id: space,
            property_id: unknown_property,
        },
        Request::PropertyMove {
            space_id: space,
            property_id: unknown_property,
            before: None,
        },
        Request::PropertyMove {
            space_id: space,
            property_id: notes.id,
            before: Some(unknown_property),
        },
        Request::PropertyOptionAdd {
            space_id: space,
            property_id: unknown_property,
            name: "x".into(),
            color: OptionColor::Gray,
            group: None,
        },
        Request::PropertyOptionUpdate {
            space_id: space,
            property_id: select.id,
            option_id: OptionId::from_bytes([9; 16]),
            name: Some("x".into()),
            color: None,
            group: None,
        },
        Request::PropertyOptionDelete {
            space_id: space,
            property_id: select.id,
            option_id: OptionId::from_bytes([9; 16]),
        },
    ] {
        let err = fails(&mut core, request.clone());
        assert_eq!(err.code, ErrorCode::NotFound, "{request:?}");
    }
    // Tags cannot be deleted or moved; options only go to the kinds that have them.
    for request in [
        Request::PropertyDelete {
            space_id: space,
            property_id: TAGS_PROPERTY_ID,
        },
        Request::PropertyMove {
            space_id: space,
            property_id: TAGS_PROPERTY_ID,
            before: None,
        },
        Request::PropertyOptionAdd {
            space_id: space,
            property_id: notes.id,
            name: "x".into(),
            color: OptionColor::Gray,
            group: None,
        },
        Request::PropertyOptionAdd {
            space_id: space,
            property_id: select.id,
            name: "x".into(),
            color: OptionColor::Gray,
            group: Some(StatusGroup::Done),
        },
        Request::PropertyOptionAdd {
            space_id: space,
            property_id: select.id,
            name: "  ".into(),
            color: OptionColor::Gray,
            group: None,
        },
    ] {
        let err = fails(&mut core, request.clone());
        assert_eq!(err.code, ErrorCode::InvalidInput, "{request:?}");
    }
    assert!(
        core.drain_events().is_empty(),
        "refused requests emit nothing"
    );
    assert_eq!(schema(&mut core, space).len(), 3);
}

#[test]
fn a_property_is_per_space() {
    let (_fx, mut core, space) = setup();
    let other = create_space(&mut core, "Work").id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    assert_eq!(schema(&mut core, space).len(), 2);
    assert_eq!(
        schema(&mut core, other).len(),
        1,
        "the other space has Tags only"
    );
    let page = create_page(&mut core, other, None, "Elsewhere").id;
    let err = fails(
        &mut core,
        Request::PagePropertySet {
            page_id: page,
            property_id: notes.id,
            value: Some(text("x")),
        },
    );
    assert_eq!(
        err.code,
        ErrorCode::NotFound,
        "the property belongs to another space"
    );
}

#[test]
fn options_are_added_updated_and_deleted_with_their_values() {
    let (_fx, mut core, space) = setup();
    let a = create_page(&mut core, space, None, "A").id;
    let b = create_page(&mut core, space, None, "B").id;
    let kind = prop_create(&mut core, space, "Kind", PropertyKind::Select);
    let red = option_add(&mut core, space, kind.id, "red");
    let green = option_add(&mut core, space, kind.id, "green");
    let urgent = option_add(&mut core, space, TAGS_PROPERTY_ID, "urgent");
    let later = option_add(&mut core, space, TAGS_PROPERTY_ID, "later");
    set_value(&mut core, a, kind.id, Some(PropertyValue::Select(red.id)));
    set_value(&mut core, b, kind.id, Some(PropertyValue::Select(green.id)));
    set_value(
        &mut core,
        a,
        TAGS_PROPERTY_ID,
        Some(multi(&[urgent.id, later.id])),
    );
    set_value(&mut core, b, TAGS_PROPERTY_ID, Some(multi(&[later.id])));
    core.drain_events();

    match call(
        &mut core,
        Request::PropertyOptionUpdate {
            space_id: space,
            property_id: kind.id,
            option_id: green.id,
            name: Some("  lime ".into()),
            color: Some(OptionColor::Yellow),
            group: None,
        },
    ) {
        Response::PropertyOption(option) => {
            assert_eq!(option.name, "lime");
            assert_eq!(option.color, OptionColor::Yellow);
            assert_eq!(option.id, green.id, "the id is stable");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        events_of(&mut core),
        [
            Event::PropertySchemaChanged { space_id: space },
            Event::StructureChanged { space_id: space }
        ]
    );

    // Deleting an option cascades through the values, in one change.
    assert_eq!(
        call(
            &mut core,
            Request::PropertyOptionDelete {
                space_id: space,
                property_id: TAGS_PROPERTY_ID,
                option_id: later.id,
            }
        ),
        Response::Ok
    );
    assert_eq!(
        events_of(&mut core),
        [
            Event::PropertySchemaChanged { space_id: space },
            Event::StructureChanged { space_id: space }
        ]
    );
    assert_eq!(
        values(&mut core, a),
        [
            (TAGS_PROPERTY_ID, multi(&[urgent.id])),
            (kind.id, PropertyValue::Select(red.id))
        ]
    );
    assert_eq!(
        values(&mut core, b),
        [(kind.id, PropertyValue::Select(green.id))],
        "the last option cleared the value"
    );
    assert_eq!(page(&mut core, b).properties, values(&mut core, b));
    assert_eq!(
        by_property(&mut core, space, TAGS_PROPERTY_ID, PropertyFilter::HasValue),
        [a]
    );
    assert!(
        by_property(
            &mut core,
            space,
            TAGS_PROPERTY_ID,
            PropertyFilter::ContainsOption(later.id)
        )
        .is_empty()
    );
    let tags = schema(&mut core, space)[0].clone();
    assert_eq!(tags.options.len(), 1);
}

// ----- values ---------------------------------------------------------------------------------

#[test]
fn values_are_set_read_through_page_info_and_cleared() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number);
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let done = prop_create(&mut core, space, "Done", PropertyKind::Checkbox);
    let link = prop_create(&mut core, space, "Link", PropertyKind::Url);
    let stage = prop_create(&mut core, space, "Stage", PropertyKind::Status);
    let email = prop_create(&mut core, space, "Contact", PropertyKind::Email);
    let phone = prop_create(&mut core, space, "Phone", PropertyKind::Phone);
    let tag = option_add(&mut core, space, TAGS_PROPERTY_ID, "urgent");
    core.drain_events();

    set_value(&mut core, page_id, notes.id, Some(text("Quarterly plan")));
    set_value(
        &mut core,
        page_id,
        cost.id,
        Some(PropertyValue::Number(12.5)),
    );
    set_value(
        &mut core,
        page_id,
        due.id,
        Some(PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
        })),
    );
    set_value(
        &mut core,
        page_id,
        done.id,
        Some(PropertyValue::Checkbox(true)),
    );
    set_value(
        &mut core,
        page_id,
        link.id,
        Some(PropertyValue::Url("https://example.com".into())),
    );
    set_value(
        &mut core,
        page_id,
        stage.id,
        Some(PropertyValue::Status(stage.options[1].id)),
    );
    set_value(
        &mut core,
        page_id,
        email.id,
        Some(PropertyValue::Email("ada@example.com".into())),
    );
    let all = set_value(
        &mut core,
        page_id,
        phone.id,
        Some(PropertyValue::Phone("+1 555 0100".into())),
    );
    set_value(&mut core, page_id, TAGS_PROPERTY_ID, Some(multi(&[tag.id])));
    // The answer lists the page's values in schema order (Tags first).
    let listed: Vec<PropertyId> = values(&mut core, page_id)
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        listed,
        [
            TAGS_PROPERTY_ID,
            notes.id,
            cost.id,
            due.id,
            done.id,
            link.id,
            stage.id,
            email.id,
            phone.id
        ]
    );
    assert_eq!(all.values.len(), 8, "Tags was not set yet");
    // PageInfo carries them: PageGet and TreeGet.
    let info = page(&mut core, page_id);
    assert_eq!(info.properties, values(&mut core, page_id));
    assert_eq!(
        tree(&mut core, space).roots[0].page.properties,
        info.properties
    );
    // The events of a value change: the structure only.
    assert!(!events_of(&mut core).contains(&Event::PropertySchemaChanged { space_id: space }));

    // Clearing and the empty values.
    set_value(&mut core, page_id, cost.id, None);
    set_value(&mut core, page_id, notes.id, Some(text("")));
    set_value(
        &mut core,
        page_id,
        link.id,
        Some(PropertyValue::Url(String::new())),
    );
    set_value(&mut core, page_id, TAGS_PROPERTY_ID, Some(multi(&[])));
    let left: Vec<PropertyId> = values(&mut core, page_id)
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(left, [due.id, done.id, stage.id, email.id, phone.id]);
    assert_eq!(page(&mut core, page_id).properties.len(), 5);
}

#[test]
fn a_value_that_does_not_fit_is_refused_with_its_field() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let kind = prop_create(&mut core, space, "Kind", PropertyKind::Select);
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let email = prop_create(&mut core, space, "Contact", PropertyKind::Email);
    let red = option_add(&mut core, space, kind.id, "red");
    core.drain_events();
    for (property, value) in [
        (notes.id, PropertyValue::Number(1.0)),
        (notes.id, text(&"x".repeat(10_001))),
        (
            kind.id,
            PropertyValue::Select(OptionId::from_bytes([9; 16])),
        ),
        (kind.id, PropertyValue::Status(red.id)),
        (kind.id, PropertyValue::MultiSelect(vec![red.id])),
        (TAGS_PROPERTY_ID, multi(&[red.id])),
        (
            due.id,
            PropertyValue::Date(DateValue {
                date: "2026-02-30".into(),
                time: None,
            }),
        ),
        (
            due.id,
            PropertyValue::Date(DateValue {
                date: "2026-10-07".into(),
                time: Some("25:00".into()),
            }),
        ),
        (email.id, PropertyValue::Email(String::new())),
        (notes.id, PropertyValue::Number(f64::NAN)),
    ] {
        let err = fails(
            &mut core,
            Request::PagePropertySet {
                page_id,
                property_id: property,
                value: Some(value.clone()),
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{value:?}");
        assert_eq!(field_of(&err), Some("value"), "{value:?}");
    }
    let err = fails(
        &mut core,
        Request::PagePropertySet {
            page_id,
            property_id: PropertyId::from_bytes([9; 16]),
            value: Some(text("x")),
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = fails(
        &mut core,
        Request::PagePropertySet {
            page_id: PageId::from_bytes([9; 16]),
            property_id: notes.id,
            value: None,
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = fails(
        &mut core,
        Request::PagePropertiesGet {
            page_id: PageId::from_bytes([9; 16]),
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(core.drain_events().is_empty());
    assert!(values(&mut core, page_id).is_empty());
}

#[test]
fn setting_a_value_sets_the_page_time_and_the_same_value_changes_nothing() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    set_value(&mut core, page_id, notes.id, Some(text("x")));
    let after = page(&mut core, page_id);
    assert_eq!(after.updated_at, 1_000, "the fixed clock of the test");
    let updates = |core: &mut Core| -> i64 {
        core.db
            .connection()
            .query_row("SELECT count(*) FROM doc_updates", [], |row| row.get(0))
            .unwrap()
    };
    let before = updates(&mut core);
    set_value(&mut core, page_id, notes.id, Some(text("x")));
    assert_eq!(
        updates(&mut core),
        before,
        "the same value writes no update"
    );
    set_value(&mut core, page_id, notes.id, Some(text("y")));
    assert_eq!(updates(&mut core), before + 1);
}

#[test]
fn values_survive_a_restart_and_structural_undo() {
    let fx = Fixture::new();
    let (space, page_id, notes, tag) = {
        let mut core = fx.open(1, 1_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = match core.dispatch(Request::SpaceList).unwrap() {
            Response::Spaces(spaces) => spaces[0].id,
            other => panic!("unexpected {other:?}"),
        };
        let page_id = create_page(&mut core, space, None, "Plan").id;
        let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
        let tag = option_add(&mut core, space, TAGS_PROPERTY_ID, "urgent");
        set_value(&mut core, page_id, notes.id, Some(text("kept")));
        set_value(&mut core, page_id, TAGS_PROPERTY_ID, Some(multi(&[tag.id])));
        // Undo takes back the page's creation... after a rename it only takes back the rename.
        call(
            &mut core,
            Request::PageRename {
                page_id,
                title: "Renamed".into(),
            },
        );
        call(&mut core, Request::StructureUndo { space_id: space });
        assert_eq!(page(&mut core, page_id).title, "Plan");
        assert_eq!(
            values(&mut core, page_id).len(),
            2,
            "undo leaves values alone"
        );
        assert_eq!(schema(&mut core, space).len(), 2, "and the schema");
        (space, page_id, notes, tag)
    };
    let mut core = fx.open(2, 2_000);
    assert_eq!(schema(&mut core, space).len(), 2);
    assert_eq!(
        values(&mut core, page_id),
        [
            (TAGS_PROPERTY_ID, multi(&[tag.id])),
            (notes.id, text("kept"))
        ]
    );
    assert_eq!(
        by_property(&mut core, space, notes.id, PropertyFilter::HasValue),
        [page_id],
        "the derived rows are there after the restart"
    );
}

// ----- queries --------------------------------------------------------------------------------

/// Pages "Banana", "apple", "Cherry" with values of every filterable kind.
struct World {
    core: Core,
    space: SpaceId,
    banana: PageId,
    apple: PageId,
    cherry: PageId,
    trashed: PageId,
    notes: PropertyId,
    cost: PropertyId,
    stage: PropertyDef,
    kind: PropertyDef,
    red: OptionId,
    due: PropertyId,
    done: PropertyId,
    urgent: OptionId,
    later: OptionId,
    _fx: Fixture,
}

fn world() -> World {
    let (fx, mut core, space) = setup();
    let banana = create_page(&mut core, space, None, "Banana").id;
    let apple = create_page(&mut core, space, None, "apple").id;
    let cherry = create_page(&mut core, space, None, "Cherry").id;
    let trashed = create_page(&mut core, space, None, "Aardvark").id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text).id;
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number).id;
    let stage = prop_create(&mut core, space, "Stage", PropertyKind::Status);
    let kind = prop_create(&mut core, space, "Kind", PropertyKind::Select);
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date).id;
    let done = prop_create(&mut core, space, "Done", PropertyKind::Checkbox).id;
    let red = option_add(&mut core, space, kind.id, "red");
    let urgent = option_add(&mut core, space, TAGS_PROPERTY_ID, "urgent").id;
    let later = option_add(&mut core, space, TAGS_PROPERTY_ID, "later").id;
    let day = |date: &str, time: Option<&str>| {
        PropertyValue::Date(DateValue {
            date: date.into(),
            time: time.map(Into::into),
        })
    };
    for (page_id, tags, note, number, flag, when) in [
        (
            banana,
            vec![urgent, later],
            "Hello World",
            12.0,
            true,
            day("2026-10-07", Some("14:30")),
        ),
        (
            apple,
            vec![urgent],
            "hello",
            12.0,
            false,
            day("2026-10-07", None),
        ),
        (
            cherry,
            vec![later],
            "ÉCOLE",
            3.5,
            true,
            day("2026-11-01", None),
        ),
        (
            trashed,
            vec![urgent],
            "hello trashed",
            12.0,
            true,
            day("2026-10-07", None),
        ),
    ] {
        set_value(&mut core, page_id, TAGS_PROPERTY_ID, Some(multi(&tags)));
        set_value(&mut core, page_id, notes, Some(text(note)));
        set_value(
            &mut core,
            page_id,
            cost,
            Some(PropertyValue::Number(number)),
        );
        set_value(
            &mut core,
            page_id,
            done,
            Some(PropertyValue::Checkbox(flag)),
        );
        set_value(&mut core, page_id, due, Some(when));
    }
    set_value(
        &mut core,
        banana,
        stage.id,
        Some(PropertyValue::Status(stage.options[1].id)),
    );
    set_value(
        &mut core,
        apple,
        kind.id,
        Some(PropertyValue::Select(red.id)),
    );
    call(&mut core, Request::PageTrash { page_id: trashed });
    core.drain_events();
    World {
        core,
        space,
        banana,
        apple,
        cherry,
        trashed,
        notes,
        cost,
        stage,
        kind,
        red: red.id,
        due,
        done,
        urgent,
        later,
        _fx: fx,
    }
}

#[test]
fn pages_by_property_has_value_lists_live_pages_by_title_ignoring_case() {
    let mut w = world();
    let (space, apple, banana, cherry) = (w.space, w.apple, w.banana, w.cherry);
    assert_eq!(
        by_property(&mut w.core, space, w.notes, PropertyFilter::HasValue),
        [apple, banana, cherry],
        "apple, Banana, Cherry: by title, case-insensitively; the trashed page is left out"
    );
    assert_eq!(
        by_property(&mut w.core, space, w.stage.id, PropertyFilter::HasValue),
        [banana]
    );
    assert!(by_property(&mut w.core, space, w.kind.id, PropertyFilter::HasValue) == [apple]);
    let _ = (w.trashed, w.cost);
}

#[test]
fn pages_by_property_answers_every_filter() {
    let mut w = world();
    let (space, apple, banana, cherry) = (w.space, w.apple, w.banana, w.cherry);
    let mut q = |property, filter| by_property(&mut w.core, space, property, filter);

    // ContainsOption and Equals on options.
    assert_eq!(
        q(TAGS_PROPERTY_ID, PropertyFilter::ContainsOption(w.urgent)),
        [apple, banana]
    );
    assert_eq!(
        q(TAGS_PROPERTY_ID, PropertyFilter::ContainsOption(w.later)),
        [banana, cherry]
    );
    assert_eq!(
        q(
            TAGS_PROPERTY_ID,
            PropertyFilter::Equals(multi(&[w.later, w.urgent]))
        ),
        [banana],
        "a set, in any order"
    );
    assert_eq!(
        q(TAGS_PROPERTY_ID, PropertyFilter::Equals(multi(&[w.urgent]))),
        [apple]
    );
    assert_eq!(
        q(
            w.stage.id,
            PropertyFilter::Equals(PropertyValue::Status(w.stage.options[1].id))
        ),
        [banana]
    );
    assert_eq!(
        q(
            w.stage.id,
            PropertyFilter::ContainsOption(w.stage.options[1].id)
        ),
        [banana]
    );
    assert!(
        q(
            w.stage.id,
            PropertyFilter::ContainsOption(w.stage.options[0].id)
        )
        .is_empty()
    );
    assert_eq!(
        q(
            w.kind.id,
            PropertyFilter::Equals(PropertyValue::Select(w.red))
        ),
        [apple]
    );
    assert_eq!(q(w.kind.id, PropertyFilter::ContainsOption(w.red)), [apple]);

    // Text.
    assert_eq!(
        q(w.notes, PropertyFilter::Equals(text("hello"))),
        [apple],
        "equality is exact"
    );
    assert!(q(w.notes, PropertyFilter::Equals(text("Hello"))).is_empty());
    assert!(q(w.notes, PropertyFilter::Equals(text(""))).is_empty());
    assert_eq!(
        q(w.notes, PropertyFilter::TextContains("HELLO".into())),
        [apple, banana]
    );
    assert_eq!(
        q(w.notes, PropertyFilter::TextContains("école".into())),
        [cherry],
        "Unicode case folding"
    );
    // Numbers, checkboxes, dates.
    assert_eq!(
        q(w.cost, PropertyFilter::Equals(PropertyValue::Number(12.0))),
        [apple, banana]
    );
    assert_eq!(q(w.done, PropertyFilter::Checked(true)), [banana, cherry]);
    assert_eq!(
        q(w.done, PropertyFilter::Checked(false)),
        [apple],
        "off or unset, among the live pages"
    );
    assert_eq!(
        q(
            w.done,
            PropertyFilter::Equals(PropertyValue::Checkbox(false))
        ),
        [apple]
    );
    assert_eq!(
        q(
            w.due,
            PropertyFilter::Equals(PropertyValue::Date(DateValue {
                date: "2026-10-07".into(),
                time: None
            }))
        ),
        [apple],
        "no time matches only a date without one"
    );
    assert_eq!(
        q(
            w.due,
            PropertyFilter::DateBetween {
                from: Some("2026-10-01".into()),
                to: Some("2026-10-31".into())
            }
        ),
        [apple, banana]
    );
    assert_eq!(
        q(
            w.due,
            PropertyFilter::DateBetween {
                from: Some("2026-10-08".into()),
                to: None
            }
        ),
        [cherry]
    );
    assert_eq!(
        q(
            w.due,
            PropertyFilter::DateBetween {
                from: None,
                to: None
            }
        ),
        [apple, banana, cherry]
    );
}

#[test]
fn a_filter_that_does_not_fit_the_property_is_invalid_input() {
    let mut w = world();
    let space = w.space;
    let mut refused = |property: PropertyId, filter: PropertyFilter| {
        let err = fails(
            &mut w.core,
            Request::PagesByProperty {
                space_id: space,
                property_id: property,
                filter: filter.clone(),
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{filter:?}");
        assert_eq!(field_of(&err), Some("filter"), "{filter:?}");
    };
    refused(w.notes, PropertyFilter::Checked(true));
    refused(w.notes, PropertyFilter::ContainsOption(w.urgent));
    refused(w.notes, PropertyFilter::Equals(PropertyValue::Number(1.0)));
    refused(w.notes, PropertyFilter::TextContains(String::new()));
    refused(w.notes, PropertyFilter::TextContains("x".repeat(201)));
    refused(w.notes, PropertyFilter::TextContains("a\nb".into()));
    refused(w.cost, PropertyFilter::TextContains("1".into()));
    refused(
        w.done,
        PropertyFilter::DateBetween {
            from: None,
            to: None,
        },
    );
    refused(
        w.due,
        PropertyFilter::DateBetween {
            from: Some("2026-02-30".into()),
            to: None,
        },
    );
    refused(
        w.due,
        PropertyFilter::DateBetween {
            from: Some("2026-10-31".into()),
            to: Some("2026-10-01".into()),
        },
    );
    refused(w.kind.id, PropertyFilter::TextContains("red".into()));
    let err = fails(
        &mut w.core,
        Request::PagesByProperty {
            space_id: space,
            property_id: PropertyId::from_bytes([9; 16]),
            filter: PropertyFilter::HasValue,
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = fails(
        &mut w.core,
        Request::PagesByProperty {
            space_id: SpaceId::from_bytes([9; 16]),
            property_id: w.notes,
            filter: PropertyFilter::HasValue,
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn trashed_pages_are_left_out_and_come_back_with_a_restore() {
    let mut w = world();
    let (space, trashed) = (w.space, w.trashed);
    let urgent = w.urgent;
    let listed = |core: &mut Core| {
        by_property(
            core,
            space,
            TAGS_PROPERTY_ID,
            PropertyFilter::ContainsOption(urgent),
        )
    };
    assert!(!listed(&mut w.core).contains(&trashed));
    // The trashed page still reads and takes values.
    assert_eq!(values(&mut w.core, trashed).len(), 5);
    set_value(
        &mut w.core,
        trashed,
        w.notes,
        Some(text("edited in the trash")),
    );
    call(&mut w.core, Request::PageRestore { page_id: trashed });
    // "Aardvark" sorts first.
    assert_eq!(listed(&mut w.core), vec![trashed, w.apple, w.banana]);
    assert_eq!(
        by_property(
            &mut w.core,
            space,
            w.notes,
            PropertyFilter::Equals(text("edited in the trash"))
        ),
        [trashed]
    );
}

#[test]
fn a_purged_page_loses_its_rows() {
    let mut w = world();
    let count = |core: &mut Core, space: SpaceId| core.db.page_properties_count(space).unwrap();
    let before = count(&mut w.core, w.space);
    assert!(before > 0);
    call(&mut w.core, Request::PagePurge { page_id: w.trashed });
    let after = count(&mut w.core, w.space);
    assert_eq!(before - after, 5, "the purged page held five rows");
    assert_eq!(
        w.core
            .db
            .page_properties_pages(w.space, w.notes, &PropertyMatch::Text("hello trashed"))
            .unwrap(),
        []
    );
}

// ----- search ---------------------------------------------------------------------------------

#[test]
fn a_search_for_a_tag_name_finds_the_page_without_changing_the_title_and_body_rule() {
    let (_fx, mut core, space) = setup();
    let plan = create_page(&mut core, space, None, "Plan").id;
    let other = create_page(&mut core, space, None, "Other").id;
    crate::test_support::write_text(&mut core, plan, "the body says urgent matters", 1);
    let urgent = option_add(&mut core, space, TAGS_PROPERTY_ID, "urgent");
    let work = option_add(&mut core, space, TAGS_PROPERTY_ID, "work");
    set_value(
        &mut core,
        plan,
        TAGS_PROPERTY_ID,
        Some(multi(&[urgent.id, work.id])),
    );
    set_value(&mut core, other, TAGS_PROPERTY_ID, Some(multi(&[work.id])));
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    set_value(
        &mut core,
        other,
        notes.id,
        Some(text("Remember the Quokka")),
    );

    // The title and the body keep matching as before: the body block is a block hit.
    let hits = search(&mut core, "urgent", None);
    let body: Vec<&SearchHit> = hits.iter().filter(|h| h.block_id.is_some()).collect();
    assert_eq!(body.len(), 1, "{hits:?}");
    assert_eq!(body[0].page_id, plan);
    // The tag is a page-level hit (no block), with the tag text as the snippet.
    let property_hits: Vec<&SearchHit> = hits.iter().filter(|h| h.block_id.is_none()).collect();
    assert_eq!(property_hits.len(), 1, "{hits:?}");
    assert_eq!(property_hits[0].page_id, plan);
    assert_eq!(property_hits[0].snippet, "urgent, work");
    assert_eq!(property_hits[0].highlights.len(), 1);
    assert_eq!(
        search(&mut core, "work", None).len(),
        2,
        "both pages are tagged work"
    );
    assert_eq!(search(&mut core, "quokka", None)[0].page_id, other);
    assert_eq!(
        search(&mut core, "plan", None).len(),
        1,
        "titles still match"
    );

    // A titles-only search (page pickers) never matches a property.
    assert!(search(&mut core, "urgent", Some(SearchScope::Titles)).is_empty());
    assert_eq!(
        search(&mut core, "plan", Some(SearchScope::Titles)).len(),
        1
    );

    // Renaming the option changes what is found, deleting it removes the text.
    call(
        &mut core,
        Request::PropertyOptionUpdate {
            space_id: space,
            property_id: TAGS_PROPERTY_ID,
            option_id: work.id,
            name: Some("office".into()),
            color: None,
            group: None,
        },
    );
    assert!(search(&mut core, "work", None).is_empty());
    assert_eq!(search(&mut core, "office", None).len(), 2);
    let office = office_id(&mut core, space);
    call(
        &mut core,
        Request::PropertyOptionDelete {
            space_id: space,
            property_id: TAGS_PROPERTY_ID,
            option_id: office,
        },
    );
    assert!(search(&mut core, "office", None).is_empty());
    assert_eq!(
        search(&mut core, "urgent", None).len(),
        2,
        "plan's other tag and body remain"
    );

    // Clearing a value and deleting a property both take their text out.
    set_value(&mut core, other, notes.id, None);
    assert!(search(&mut core, "quokka", None).is_empty());
    set_value(&mut core, other, notes.id, Some(text("Quokka again")));
    assert_eq!(search(&mut core, "quokka", None).len(), 1);
    call(
        &mut core,
        Request::PropertyDelete {
            space_id: space,
            property_id: notes.id,
        },
    );
    assert!(search(&mut core, "quokka", None).is_empty());

    // A trashed page is not found, a restored one is.
    call(&mut core, Request::PageTrash { page_id: plan });
    assert!(search(&mut core, "urgent", None).is_empty());
    call(&mut core, Request::PageRestore { page_id: plan });
    assert_eq!(search(&mut core, "urgent", None).len(), 2);
}

/// The id of the option of Tags that was renamed to "office".
fn office_id(core: &mut Core, space_id: SpaceId) -> OptionId {
    schema(core, space_id)[0]
        .options
        .iter()
        .find(|o| o.name == "office")
        .unwrap()
        .id
}

#[test]
fn a_page_text_rewrite_keeps_its_property_rows() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let tag = option_add(&mut core, space, TAGS_PROPERTY_ID, "keepme");
    set_value(&mut core, page_id, TAGS_PROPERTY_ID, Some(multi(&[tag.id])));
    // Editing the body rewrites the page's text rows; the tag stays searchable.
    crate::test_support::write_text(&mut core, page_id, "some body text", 1);
    crate::test_support::write_text(&mut core, page_id, "more body text", 2);
    assert_eq!(search(&mut core, "keepme", None).len(), 1);
    // A rename of the page rewrites its title row only.
    call(
        &mut core,
        Request::PageRename {
            page_id,
            title: "Renamed".into(),
        },
    );
    assert_eq!(search(&mut core, "keepme", None).len(), 1);
}

// ----- the derived rows -----------------------------------------------------------------------

/// Every derived property row of the space, as text, for comparisons: the `page_properties`
/// rows (without the rowid order) and the property rows of the text index.
fn derived(core: &Core) -> (Vec<String>, Vec<String>) {
    let conn = core.db.connection();
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let mut out: Vec<String> = stmt
            .query_map([], |row| {
                let n = row.as_ref().column_count();
                let mut parts = Vec::new();
                for i in 0..n {
                    let value: kb_storage::rusqlite::types::Value = row.get(i)?;
                    parts.push(format!("{value:?}"));
                }
                Ok(parts.join("|"))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        out.sort();
        out
    };
    (
        rows("SELECT * FROM page_properties"),
        rows("SELECT page_id, block_id, title, body, kind FROM fts_blocks WHERE kind = 1"),
    )
}

#[test]
fn index_rebuild_restores_the_property_rows_from_the_documents() {
    let mut w = world();
    let before = derived(&w.core);
    assert!(!before.0.is_empty() && !before.1.is_empty());

    // Wipe both derived tables, as a damaged or older database would have them.
    w.core.db.page_properties_clear().unwrap();
    w.core
        .db
        .connection()
        .execute("DELETE FROM fts_blocks WHERE kind = 1", [])
        .unwrap();
    assert!(
        by_property(&mut w.core, w.space, w.notes, PropertyFilter::HasValue).is_empty(),
        "the rows are gone"
    );
    call(&mut w.core, Request::IndexRebuild { space_id: None });
    assert_eq!(derived(&w.core), before);
    assert_eq!(
        by_property(&mut w.core, w.space, w.notes, PropertyFilter::HasValue),
        [w.apple, w.banana, w.cherry]
    );
    assert_eq!(search(&mut w.core, "urgent", None).len(), 2);

    // One space at a time works too.
    w.core.db.page_properties_clear_space(w.space).unwrap();
    call(
        &mut w.core,
        Request::IndexRebuild {
            space_id: Some(w.space),
        },
    );
    assert_eq!(derived(&w.core), before);
    // And a rebuild of a healthy index changes nothing.
    call(&mut w.core, Request::IndexRebuild { space_id: None });
    assert_eq!(derived(&w.core), before);
}

#[test]
fn a_database_from_before_schema_v10_gets_its_rows_at_open() {
    let fx = Fixture::new();
    let (space, notes) = {
        let mut core = fx.open(1, 1_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = match core.dispatch(Request::SpaceList).unwrap() {
            Response::Spaces(spaces) => spaces[0].id,
            other => panic!("unexpected {other:?}"),
        };
        let page_id = create_page(&mut core, space, None, "Plan").id;
        let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text).id;
        set_value(&mut core, page_id, notes, Some(text("indexed at open")));
        // Pretend the rows were never written: no table content, no marker.
        core.db.page_properties_clear().unwrap();
        core.db
            .connection()
            .execute("DELETE FROM fts_blocks WHERE kind = 1", [])
            .unwrap();
        core.db
            .connection()
            .execute("DELETE FROM meta WHERE key = 'page_properties_indexed'", [])
            .unwrap();
        (space, notes)
    };
    let mut core = fx.open(2, 2_000);
    assert_eq!(
        by_property(&mut core, space, notes, PropertyFilter::HasValue).len(),
        1
    );
    assert_eq!(search(&mut core, "indexed at open", None).len(), 1);
}

#[test]
fn undo_that_brings_a_purged_page_back_restores_its_rows() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    set_value(
        &mut core,
        page_id,
        notes.id,
        Some(text("survives the undo")),
    );
    call(&mut core, Request::PageTrash { page_id });
    call(&mut core, Request::PagePurge { page_id });
    assert!(by_property(&mut core, space, notes.id, PropertyFilter::HasValue).is_empty());
    // Undoing the purge brings the page back into the trash: its rows are back too, and the
    // query still leaves it out.
    call(&mut core, Request::StructureUndo { space_id: space });
    assert_eq!(core.db.page_properties_count(space).unwrap(), 1);
    assert!(by_property(&mut core, space, notes.id, PropertyFilter::HasValue).is_empty());
    call(&mut core, Request::PageRestore { page_id });
    assert_eq!(
        by_property(&mut core, space, notes.id, PropertyFilter::HasValue),
        [page_id]
    );
}

#[test]
fn a_deleted_propertys_leftovers_are_cleaned_by_the_next_write_of_that_page() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let cost = prop_create(&mut core, space, "Cost", PropertyKind::Number);
    set_value(&mut core, page_id, notes.id, Some(text("left behind")));
    call(
        &mut core,
        Request::PropertyDelete {
            space_id: space,
            property_id: notes.id,
        },
    );
    assert!(values(&mut core, page_id).is_empty(), "unread at once");
    assert_eq!(
        core.db.page_properties_count(space).unwrap(),
        0,
        "unindexed at once"
    );
    let portable = |core: &Core| core.docs.structures[&space].model.to_portable().to_string();
    assert!(
        portable(&core).contains("left behind"),
        "kept in the document"
    );
    assert!(
        !core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
    set_value(
        &mut core,
        page_id,
        cost.id,
        Some(PropertyValue::Number(1.0)),
    );
    assert!(
        !portable(&core).contains("left behind"),
        "gone with the next write"
    );
    assert!(
        core.docs.structures[&space]
            .model
            .check_invariants()
            .is_empty()
    );
}

// ----- backups --------------------------------------------------------------------------------

#[test]
fn a_backup_restores_the_schema_the_values_and_their_rows() {
    let fx = Fixture::new();
    let (bytes, kit, space, page_id, notes, tag) = {
        let mut core = fx.open(1, 1_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let kit = kit_create(&mut core).words.join(" ");
        let space = match core.dispatch(Request::SpaceList).unwrap() {
            Response::Spaces(spaces) => spaces[0].id,
            other => panic!("unexpected {other:?}"),
        };
        let page_id = create_page(&mut core, space, None, "Plan").id;
        let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text).id;
        let tag = option_add(&mut core, space, TAGS_PROPERTY_ID, "restored tag");
        set_value(&mut core, page_id, notes, Some(text("in the backup")));
        set_value(&mut core, page_id, TAGS_PROPERTY_ID, Some(multi(&[tag.id])));
        (export_backup(&mut core), kit, space, page_id, notes, tag)
    };
    let fresh = Fixture::new();
    let mut core = fresh.open(7, 5_000);
    import_backup(&mut core, &bytes, Some(&kit));
    assert_eq!(schema(&mut core, space).len(), 2);
    assert_eq!(
        values(&mut core, page_id),
        [
            (TAGS_PROPERTY_ID, multi(&[tag.id])),
            (notes, text("in the backup"))
        ]
    );
    assert_eq!(
        by_property(&mut core, space, notes, PropertyFilter::HasValue),
        [page_id],
        "the restore rebuilds the derived rows"
    );
    assert_eq!(search(&mut core, "restored tag", None).len(), 1);
}

#[test]
fn page_info_is_the_same_everywhere_it_appears() {
    let (_fx, mut core, space) = setup();
    let parent = create_page(&mut core, space, None, "Parent").id;
    let child = create_page(&mut core, space, Some(parent), "Child").id;
    let tag = option_add(&mut core, space, TAGS_PROPERTY_ID, "t");
    set_value(&mut core, child, TAGS_PROPERTY_ID, Some(multi(&[tag.id])));
    let expected = vec![(TAGS_PROPERTY_ID, multi(&[tag.id]))];
    assert_eq!(page(&mut core, child).properties, expected);
    let view = tree(&mut core, space);
    assert!(view.roots[0].page.properties.is_empty());
    assert_eq!(view.roots[0].children[0].page.properties, expected);
    call(&mut core, Request::PageTrash { page_id: parent });
    let view = tree(&mut core, space);
    assert_eq!(view.trashed.len(), 1);
    assert!(view.trashed[0].page.properties.is_empty());
    assert_eq!(
        page(&mut core, child).properties,
        expected,
        "inside a trashed subtree"
    );
    // The JSON a shell sees leaves the key out when there are no values.
    let json = serde_json::to_value(page(&mut core, parent)).unwrap();
    assert!(json.get("properties").is_none());
    let json = serde_json::to_value(page(&mut core, child)).unwrap();
    assert_eq!(json["properties"][0][0], TAGS_PROPERTY_ID.to_string());
}

// ----- files and checklists (ADR-0026, amendment of 2026-10-07) -------------------------------

const DAY: i64 = 86_400_000;

/// Uploads a small file into the space and returns its id.
fn upload(core: &mut Core, space_id: SpaceId, name: &str) -> AttachmentId {
    let id = match call(
        core,
        Request::AttachmentBegin {
            space_id,
            name: name.into(),
            mime: "application/octet-stream".into(),
        },
    ) {
        Response::AttachmentStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    };
    core.attachment_write(id, name.as_bytes()).unwrap();
    match call(core, Request::AttachmentFinish { attachment_id: id }) {
        Response::Attachment(info) => info.attachment_id,
        other => panic!("unexpected {other:?}"),
    }
}

fn attachment(core: &mut Core, attachment_id: AttachmentId) -> Result<AttachmentInfo, ApiError> {
    match core.dispatch(Request::AttachmentGet { attachment_id })? {
        Response::Attachment(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn collect(core: &mut Core) -> u32 {
    match call(core, Request::AttachmentGc) {
        Response::AttachmentGc(summary) => summary.purged,
        other => panic!("unexpected {other:?}"),
    }
}

fn files(ids: &[AttachmentId]) -> PropertyValue {
    PropertyValue::Files(ids.to_vec())
}

fn item(n: u8, text: &str, done: bool) -> ChecklistItem {
    ChecklistItem {
        id: ChecklistItemId::from_random_bytes([
            0xe0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n,
        ]),
        text: text.to_owned(),
        done,
    }
}

fn checklist(items: Vec<ChecklistItem>) -> PropertyValue {
    PropertyValue::Checklist(items)
}

#[test]
fn files_and_checklists_are_set_read_through_page_info_and_cleared() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files);
    let todo = prop_create(&mut core, space, "To do", PropertyKind::Checklist);
    assert!(attached.options.is_empty() && todo.options.is_empty());
    let a = upload(&mut core, space, "plan.pdf");
    let b = upload(&mut core, space, "map.png");
    let list = checklist(vec![item(1, "Pack", true), item(2, "Call Ada", false)]);

    set_value(&mut core, page_id, attached.id, Some(files(&[b, a])));
    let answered = set_value(&mut core, page_id, todo.id, Some(list.clone()));
    assert_eq!(
        answered.values,
        [(attached.id, files(&[b, a])), (todo.id, list.clone())],
        "in schema order, the lists as written"
    );
    assert_eq!(page(&mut core, page_id).properties, answered.values);

    // The JSON a shell sees.
    let json = serde_json::to_value(&answered.values[1].1).unwrap();
    assert_eq!(json["kind"], "checklist");
    assert_eq!(json["value"][0]["text"], "Pack");
    assert_eq!(json["value"][0]["done"], true);
    assert!(json["value"][0]["id"].is_string());
    assert_eq!(
        serde_json::to_value(&answered.values[0].1).unwrap()["value"][0],
        b.to_string()
    );

    // An empty list is no value; the other value stays.
    set_value(&mut core, page_id, attached.id, Some(files(&[])));
    assert_eq!(values(&mut core, page_id), [(todo.id, list)]);
    set_value(&mut core, page_id, todo.id, None);
    assert!(values(&mut core, page_id).is_empty());
    set_value(&mut core, page_id, todo.id, Some(checklist(Vec::new())));
    assert!(values(&mut core, page_id).is_empty());
}

#[test]
fn files_and_checklists_follow_their_rules_and_a_missing_attachment_is_fine() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let todo = prop_create(&mut core, space, "To do", PropertyKind::Checklist).id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text).id;
    let refuse = |core: &mut Core, property_id: PropertyId, value: PropertyValue| {
        let err = fails(
            core,
            Request::PagePropertySet {
                page_id,
                property_id,
                value: Some(value),
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        assert_eq!(field_of(&err), Some("value"), "{err}");
        err
    };
    let id = |n: u8| AttachmentId::from_bytes([n; 16]);

    // The attachment does not have to exist (a value can arrive before its manifest).
    set_value(&mut core, page_id, attached, Some(files(&[id(1), id(2)])));
    assert_eq!(
        values(&mut core, page_id),
        [(attached, files(&[id(1), id(2)]))]
    );

    refuse(&mut core, attached, files(&[id(1), id(2), id(1)]));
    let many: Vec<AttachmentId> = (1..=51).map(id).collect();
    let err = refuse(&mut core, attached, files(&many));
    assert!(err.message.contains("at most 50 files"), "{err}");
    set_value(&mut core, page_id, attached, Some(files(&many[..50])));
    refuse(&mut core, attached, checklist(vec![item(1, "x", false)]));
    refuse(&mut core, todo, files(&[id(1)]));
    refuse(&mut core, notes, files(&[id(1)]));
    refuse(
        &mut core,
        todo,
        checklist(vec![item(1, "a", false), item(1, "b", true)]),
    );
    let err = refuse(
        &mut core,
        todo,
        checklist(vec![item(1, &"x".repeat(501), false)]),
    );
    assert!(err.message.contains("at most 500 characters"), "{err}");
    refuse(
        &mut core,
        todo,
        checklist(vec![item(1, "two\nlines", false)]),
    );
    let items: Vec<ChecklistItem> = (0..201_u16)
        .map(|n| {
            let [low, high] = n.to_le_bytes();
            ChecklistItem {
                id: ChecklistItemId::from_bytes([
                    1, low, high, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ]),
                text: "x".into(),
                done: false,
            }
        })
        .collect();
    refuse(&mut core, todo, checklist(items.clone()));
    set_value(
        &mut core,
        page_id,
        todo,
        Some(checklist(items[..200].to_vec())),
    );
    // A blank text is a valid item.
    set_value(
        &mut core,
        page_id,
        todo,
        Some(checklist(vec![item(1, "", true)])),
    );
    assert_eq!(
        values(&mut core, page_id)[1],
        (todo, checklist(vec![item(1, "", true)]))
    );
}

#[test]
fn a_file_that_only_a_property_holds_is_kept_and_removing_it_makes_it_collectable() {
    let (fx, mut core, space) = setup();
    let t0 = 1_000;
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let a = upload(&mut core, space, "kept.bin");
    let b = upload(&mut core, space, "dropped.bin");
    assert_eq!(attachment(&mut core, a).unwrap().references, 0);
    assert_eq!(
        attachment(&mut core, a).unwrap().unreferenced_since,
        Some(t0)
    );

    core.drain_events();
    set_value(&mut core, page_id, attached, Some(files(&[a, b])));
    for id in [a, b] {
        let info = attachment(&mut core, id).unwrap();
        assert_eq!(info.references, 1, "one value references it");
        assert_eq!(info.unreferenced_since, None);
    }
    assert!(
        core.drain_events()
            .contains(&Event::AttachmentsChanged { space_id: space }),
        "the bookkeeping announces the attachments that changed"
    );
    drop(core);

    // Well past the retention: referenced files stay.
    let t1 = t0 + 40 * DAY;
    let mut core = fx.open(2, t1);
    assert_eq!(collect(&mut core), 0);
    assert!(attachment(&mut core, a).is_ok() && attachment(&mut core, b).is_ok());

    // Taking one out of the value starts its clock; the other keeps the value.
    set_value(&mut core, page_id, attached, Some(files(&[a])));
    let dropped = attachment(&mut core, b).unwrap();
    assert_eq!(
        (dropped.references, dropped.unreferenced_since),
        (0, Some(t1))
    );
    assert_eq!(attachment(&mut core, a).unwrap().references, 1);
    drop(core);
    let mut core = fx.open(3, t1 + 29 * DAY);
    assert!(attachment(&mut core, b).is_ok(), "within the retention");
    drop(core);
    let mut core = fx.open(4, t1 + 31 * DAY);
    assert_eq!(
        attachment(&mut core, b).unwrap_err().code,
        ErrorCode::NotFound,
        "collected at open"
    );
    assert!(attachment(&mut core, a).is_ok());
    assert_eq!(collect(&mut core), 0);

    // Putting it back in time keeps it; clearing the value releases the last file.
    let t2 = t1 + 31 * DAY;
    set_value(&mut core, page_id, attached, None);
    let released = attachment(&mut core, a).unwrap();
    assert_eq!(
        (released.references, released.unreferenced_since),
        (0, Some(t2))
    );
    set_value(&mut core, page_id, attached, Some(files(&[a])));
    assert_eq!(attachment(&mut core, a).unwrap().unreferenced_since, None);
    set_value(&mut core, page_id, attached, None);
    drop(core);
    let mut core = fx.open(5, t2 + 31 * DAY);
    assert_eq!(
        attachment(&mut core, a).unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[test]
fn deleting_the_property_or_purging_the_page_makes_its_files_collectable() {
    let (fx, mut core, space) = setup();
    let t0 = 1_000;
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let other = create_page(&mut core, space, None, "Other").id;
    let first = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let second = prop_create(&mut core, space, "More", PropertyKind::Files).id;
    let a = upload(&mut core, space, "a.bin");
    let b = upload(&mut core, space, "b.bin");
    let shared = upload(&mut core, space, "shared.bin");
    set_value(&mut core, page_id, first, Some(files(&[a, shared])));
    set_value(&mut core, other, second, Some(files(&[b, shared])));
    assert_eq!(attachment(&mut core, shared).unwrap().references, 2);

    // A trashed page still references its files (it can be restored).
    call(&mut core, Request::PageTrash { page_id: other });
    assert_eq!(attachment(&mut core, b).unwrap().references, 1);
    assert_eq!(attachment(&mut core, b).unwrap().unreferenced_since, None);

    // Purging it releases what only it held; the shared file is still held by the first page.
    call(&mut core, Request::PagePurge { page_id: other });
    let b_info = attachment(&mut core, b).unwrap();
    assert_eq!(
        (b_info.references, b_info.unreferenced_since),
        (0, Some(t0))
    );
    let shared_info = attachment(&mut core, shared).unwrap();
    assert_eq!(
        (shared_info.references, shared_info.unreferenced_since),
        (1, None)
    );

    // Deleting the property releases its values at once, whatever is left in the documents.
    call(
        &mut core,
        Request::PropertyDelete {
            space_id: space,
            property_id: first,
        },
    );
    for id in [a, shared] {
        let info = attachment(&mut core, id).unwrap();
        assert_eq!(
            (info.references, info.unreferenced_since),
            (0, Some(t0)),
            "{id}"
        );
    }
    drop(core);
    let mut core = fx.open(2, t0 + 31 * DAY);
    for id in [a, b, shared] {
        assert_eq!(
            attachment(&mut core, id).unwrap_err().code,
            ErrorCode::NotFound
        );
    }
}

#[test]
fn index_rebuild_restores_the_references_of_property_files() {
    use kb_storage::AttachmentQueries;
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let a = upload(&mut core, space, "a.bin");
    let stale = upload(&mut core, space, "stale.bin");
    set_value(&mut core, page_id, attached, Some(files(&[a])));
    assert_eq!(attachment(&mut core, a).unwrap().references, 1);

    // The derived rows are lost: the references go with them.
    core.db.page_properties_clear().unwrap();
    assert_eq!(attachment(&mut core, a).unwrap().references, 0);
    call(&mut core, Request::IndexRebuild { space_id: None });
    let restored = attachment(&mut core, a).unwrap();
    assert_eq!(
        (restored.references, restored.unreferenced_since),
        (1, None)
    );

    // A row for a file the documents do not attach (a stale one) is gone after a rebuild, and
    // that file starts its clock.
    let page_rows = core.db.page_properties_of(page_id).unwrap();
    let mut with_stale = page_rows.clone();
    with_stale.push(kb_storage::PagePropertyRow {
        property_id: attached,
        kind: PropertyKind::Files,
        value_text: None,
        value_number: None,
        value_bool: None,
        value_date: None,
        value_time: None,
        option_id: None,
        attachment_id: Some(stale),
        value_total: None,
        related_page_id: None,
        account_id: None,
    });
    core.db
        .page_properties_replace(space, page_id, &with_stale)
        .unwrap();
    core.db
        .attachment_set_unreferenced_since(stale, None)
        .unwrap();
    assert_eq!(attachment(&mut core, stale).unwrap().references, 1);
    call(
        &mut core,
        Request::IndexRebuild {
            space_id: Some(space),
        },
    );
    let released = attachment(&mut core, stale).unwrap();
    assert_eq!(
        (released.references, released.unreferenced_since),
        (0, Some(1_000))
    );
    assert_eq!(core.db.page_properties_of(page_id).unwrap(), page_rows);
    assert_eq!(attachment(&mut core, a).unwrap().references, 1);
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(attachment(&mut core, a).unwrap().references, 1);
}

#[test]
fn a_file_name_finds_the_page_and_a_purged_file_is_forgotten() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let a = upload(&mut core, space, "Quarterly report.pdf");
    let b = upload(&mut core, space, "Route map.png");
    set_value(&mut core, page_id, attached, Some(files(&[b, a])));

    let hits = search(&mut core, "quarterly", None);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].page_id, page_id);
    assert!(hits[0].block_id.is_none(), "a property hit is page-level");
    assert_eq!(hits[0].snippet, "Route map.png, Quarterly report.pdf");
    assert_eq!(search(&mut core, "route", None).len(), 1);
    assert!(
        search(&mut core, "quarterly", Some(SearchScope::Titles)).is_empty(),
        "never a title hit"
    );

    // Purging a file takes its name out of the index (the value keeps naming it).
    call(&mut core, Request::AttachmentDelete { attachment_id: a });
    assert!(search(&mut core, "quarterly", None).is_empty());
    assert_eq!(search(&mut core, "route", None).len(), 1);
    assert_eq!(values(&mut core, page_id), [(attached, files(&[b, a]))]);

    // Removing the files leaves nothing to find.
    set_value(&mut core, page_id, attached, None);
    assert!(search(&mut core, "route", None).is_empty());
}

#[test]
fn checklist_items_are_searchable_and_follow_the_list() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let todo = prop_create(&mut core, space, "To do", PropertyKind::Checklist).id;
    set_value(
        &mut core,
        page_id,
        todo,
        Some(checklist(vec![
            item(1, "Pack the charger", true),
            item(2, "", false),
            item(3, "Call Ada", false),
        ])),
    );
    let hits = search(&mut core, "charger", None);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].block_id.is_none());
    assert_eq!(hits[0].snippet, "Pack the charger, Call Ada");
    assert_eq!(search(&mut core, "ada", None).len(), 1);
    set_value(
        &mut core,
        page_id,
        todo,
        Some(checklist(vec![item(3, "Call Ada", true)])),
    );
    assert!(search(&mut core, "charger", None).is_empty());
    assert_eq!(search(&mut core, "ada", None).len(), 1);
    set_value(&mut core, page_id, todo, None);
    assert!(search(&mut core, "ada", None).is_empty());
}

#[test]
fn files_and_checklists_answer_has_value_and_completion_and_refuse_the_other_filters() {
    let (_fx, mut core, space) = setup();
    let done = create_page(&mut core, space, None, "Done").id;
    let open = create_page(&mut core, space, None, "Open").id;
    let none = create_page(&mut core, space, None, "None").id;
    let trashed = create_page(&mut core, space, None, "Trashed").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let todo = prop_create(&mut core, space, "To do", PropertyKind::Checklist).id;
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text).id;
    let a = upload(&mut core, space, "a.bin");
    let all_done = checklist(vec![item(1, "a", true), item(2, "b", true)]);
    let half = checklist(vec![item(1, "a", true), item(2, "b", false)]);
    set_value(&mut core, done, todo, Some(all_done));
    set_value(&mut core, open, todo, Some(half));
    set_value(
        &mut core,
        trashed,
        todo,
        Some(checklist(vec![item(1, "a", true)])),
    );
    set_value(&mut core, done, attached, Some(files(&[a])));
    set_value(&mut core, open, attached, Some(files(&[a])));
    set_value(&mut core, none, notes, Some(text("x")));
    call(&mut core, Request::PageTrash { page_id: trashed });

    let has = PropertyFilter::HasValue;
    assert_eq!(
        by_property(&mut core, space, todo, has.clone()),
        [done, open],
        "by title; the trashed page is left out"
    );
    let complete = |on| PropertyFilter::ChecklistComplete(on);
    assert_eq!(by_property(&mut core, space, todo, complete(true)), [done]);
    assert_eq!(
        by_property(&mut core, space, todo, complete(false)),
        [open],
        "a page without a checklist is in neither answer"
    );
    assert_eq!(by_property(&mut core, space, attached, has), [done, open]);
    // After a change the filter follows.
    set_value(
        &mut core,
        open,
        todo,
        Some(checklist(vec![item(2, "b", true)])),
    );
    assert_eq!(
        by_property(&mut core, space, todo, complete(true)),
        [done, open]
    );

    let refused = |core: &mut Core, property_id: PropertyId, filter: PropertyFilter| {
        let err = fails(
            core,
            Request::PagesByProperty {
                space_id: space,
                property_id,
                filter,
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        assert_eq!(field_of(&err), Some("filter"), "{err}");
    };
    refused(&mut core, notes, complete(true));
    refused(&mut core, attached, complete(false));
    refused(&mut core, todo, PropertyFilter::TextContains("a".into()));
    refused(
        &mut core,
        attached,
        PropertyFilter::TextContains("a".into()),
    );
    refused(&mut core, todo, PropertyFilter::Checked(true));
    refused(
        &mut core,
        todo,
        PropertyFilter::Equals(checklist(vec![item(1, "a", true)])),
    );
    refused(&mut core, attached, PropertyFilter::Equals(files(&[a])));
    let option = OptionId::from_bytes([1; 16]);
    refused(&mut core, attached, PropertyFilter::ContainsOption(option));
}

#[test]
fn files_and_checklists_survive_a_restart_and_a_backup() {
    let (fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let attached = prop_create(&mut core, space, "Attached", PropertyKind::Files).id;
    let todo = prop_create(&mut core, space, "To do", PropertyKind::Checklist).id;
    let a = upload(&mut core, space, "plan.pdf");
    let list = checklist(vec![item(1, "Pack", true), item(2, "Call Ada", false)]);
    set_value(&mut core, page_id, attached, Some(files(&[a])));
    set_value(&mut core, page_id, todo, Some(list));
    let expected = values(&mut core, page_id);
    let derived_before = derived(&core);
    drop(core);

    let mut core = fx.open(2, 2_000);
    assert_eq!(values(&mut core, page_id), expected);
    assert_eq!(
        derived(&core),
        derived_before,
        "the rows come back from the documents"
    );
    assert_eq!(attachment(&mut core, a).unwrap().references, 1);
    assert_eq!(search(&mut core, "plan", None).len(), 1);

    let kit = kit_create(&mut core).words.join(" ");
    let backup = export_backup(&mut core);
    let other = Fixture::new();
    let mut restored = other.open(7, 3_000);
    import_backup(&mut restored, &backup, Some(&kit));
    assert_eq!(values(&mut restored, page_id), expected);
    assert_eq!(derived(&restored).0.len(), derived_before.0.len());
    assert_eq!(attachment(&mut restored, a).unwrap().references, 1);
}
