//! The `place` kind and the computed kinds (created and last edited time and device) through
//! `Request` (ADR-0026, amendment of 2026-10-08): values, the device that makes an edit, the
//! throttled stamp of a content edit, the queries, the text index, restarts and rebuilds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_model::EDIT_STAMP_INTERVAL_MS;
use kb_types::{
    ApiError, ErrorCode, Event, PageId, PlaceValue, PropertyFilter, PropertyId, PropertyKind,
    PropertyValue, Request, Response, SearchHit, SpaceId,
};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, page, status, tree, write_text,
};

fn call(core: &mut Core, request: Request) -> Response {
    core.dispatch(request).unwrap()
}

fn fails(core: &mut Core, request: Request) -> ApiError {
    core.dispatch(request).unwrap_err()
}

fn field_of(err: &ApiError) -> Option<&str> {
    err.details.as_ref()?.get("field")?.as_str()
}

fn first_space(core: &mut Core) -> SpaceId {
    match call(core, Request::SpaceList) {
        Response::Spaces(spaces) => spaces[0].id,
        other => panic!("unexpected {other:?}"),
    }
}

fn prop_create(core: &mut Core, space_id: SpaceId, name: &str, kind: PropertyKind) -> PropertyId {
    match call(
        core,
        Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config: None,
        },
    ) {
        Response::Property(def) => def.id,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_value(
    core: &mut Core,
    page_id: PageId,
    property_id: PropertyId,
    value: Option<PropertyValue>,
) -> Result<Response, ApiError> {
    core.dispatch(Request::PagePropertySet {
        page_id,
        property_id,
        value,
    })
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

fn search(core: &mut Core, query: &str) -> Vec<SearchHit> {
    match call(
        core,
        Request::Search {
            query: query.into(),
            space_id: None,
            scope: None,
            limit: Some(100),
            edited_after: None,
        },
    ) {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

fn place(name: &str, at: Option<(f64, f64)>) -> PropertyValue {
    PropertyValue::Place(PlaceValue {
        name: name.into(),
        lat: at.map(|c| c.0),
        lon: at.map(|c| c.1),
    })
}

fn has_structure_changed(core: &mut Core) -> bool {
    core.drain_events()
        .iter()
        .any(|event| matches!(event, Event::StructureChanged { .. }))
}

struct World {
    fx: Fixture,
    core: Core,
    space: SpaceId,
    created: PropertyId,
    edited: PropertyId,
    author: PropertyId,
    editor: PropertyId,
    where_: PropertyId,
}

/// A space with one property of each new kind, the clock at 1,000.
fn world() -> World {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = first_space(&mut core);
    let created = prop_create(&mut core, space, "Created", PropertyKind::CreatedTime);
    let edited = prop_create(&mut core, space, "Edited", PropertyKind::LastEditedTime);
    let author = prop_create(&mut core, space, "Author", PropertyKind::CreatedBy);
    let editor = prop_create(&mut core, space, "Editor", PropertyKind::LastEditedBy);
    let where_ = prop_create(&mut core, space, "Where", PropertyKind::Place);
    core.drain_events();
    World {
        fx,
        core,
        space,
        created,
        edited,
        author,
        editor,
        where_,
    }
}

#[test]
fn a_new_page_records_its_creation_time_and_device() {
    let mut w = world();
    let device = status(&mut w.core).device_id;
    let trip = create_page(&mut w.core, w.space, None, "Trip").id;
    assert_eq!(
        values(&mut w.core, trip),
        [
            (w.created, PropertyValue::CreatedTime(1_000)),
            (w.edited, PropertyValue::LastEditedTime(1_000)),
            (w.author, PropertyValue::CreatedBy(device)),
            (w.editor, PropertyValue::LastEditedBy(device)),
        ]
    );
    assert_eq!(
        page(&mut w.core, trip).properties,
        values(&mut w.core, trip)
    );
    // Computed values are not stored, so a reopened core reads the same.
    drop(w.core);
    let mut core = w.fx.open(2, 9_000);
    assert_eq!(values(&mut core, trip).len(), 4);
    assert_eq!(
        status(&mut core).device_id,
        device,
        "the device identity persists"
    );
}

#[test]
fn page_info_names_the_device_that_created_and_the_one_that_last_edited_the_page() {
    let mut w = world();
    let device = status(&mut w.core).device_id;
    let created = create_page(&mut w.core, w.space, None, "Trip");
    assert_eq!(created.created_by, Some(device));
    assert_eq!(created.edited_by, Some(device));
    assert_eq!(page(&mut w.core, created.id), created);

    // The tree carries the same, and a reopened core reads it back from the node meta.
    assert_eq!(tree(&mut w.core, w.space).roots[0].page, created);
    drop(w.core);
    let mut core = w.fx.open(2, 9_000);
    let info = page(&mut core, created.id);
    assert_eq!(
        (info.created_by, info.edited_by),
        (Some(device), Some(device))
    );
}

#[test]
fn a_computed_property_cannot_be_set_and_adds_no_rows() {
    let mut w = world();
    let trip = create_page(&mut w.core, w.space, None, "Trip").id;
    for (property, value) in [
        (w.created, Some(PropertyValue::CreatedTime(5))),
        (w.author, None),
    ] {
        let err = set_value(&mut w.core, trip, property, value).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(field_of(&err), Some("propertyId"));
        assert_eq!(
            err.message,
            "this property is computed from the page and cannot be set"
        );
    }
    let rows: i64 = w
        .core
        .db
        .connection()
        .query_row(
            "SELECT count(*) FROM page_properties WHERE page_id = ?1",
            [trip.to_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
    // Nor do they feed the text index.
    assert!(search(&mut w.core, "1000").is_empty());
}

#[test]
fn a_content_edit_moves_the_last_edit_once_per_interval() {
    let mut w = world();
    let trip = create_page(&mut w.core, w.space, None, "Trip").id;
    let device = status(&mut w.core).device_id;
    w.core.drain_events();

    // Within the interval of the creation (the same device): nothing is stamped.
    write_text(&mut w.core, trip, "first", 1);
    assert!(!has_structure_changed(&mut w.core));
    assert_eq!(page(&mut w.core, trip).updated_at, 1_000);

    // After the interval the next edit is stamped (a fresh core stands for later).
    drop(w.core);
    let later = 1_000 + EDIT_STAMP_INTERVAL_MS + 5;
    let mut core = w.fx.open(2, later);
    core.drain_events();
    write_text(&mut core, trip, "second", 2);
    assert!(has_structure_changed(&mut core));
    assert_eq!(page(&mut core, trip).updated_at, later);
    assert_eq!(
        values(&mut core, trip)[1],
        (w.edited, PropertyValue::LastEditedTime(later))
    );
    assert_eq!(
        values(&mut core, trip)[3],
        (w.editor, PropertyValue::LastEditedBy(device))
    );
    // The created time stays.
    assert_eq!(
        values(&mut core, trip)[0],
        (w.created, PropertyValue::CreatedTime(1_000))
    );
    // And the stamp is persisted with the page index.
    drop(core);
    let mut core = w.fx.open(3, later + 10);
    assert_eq!(page(&mut core, trip).updated_at, later);
    let indexed: i64 = core
        .db
        .connection()
        .query_row(
            "SELECT updated_at FROM pages_index WHERE page_id = ?1",
            [trip.to_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, later);
}

#[test]
fn computed_values_are_queried_by_time_and_device_in_title_order() {
    let mut w = world();
    let device = status(&mut w.core).device_id;
    let beta = create_page(&mut w.core, w.space, None, "beta").id;
    let alpha = create_page(&mut w.core, w.space, None, "Alpha").id;
    let trashed = create_page(&mut w.core, w.space, None, "Trashed").id;
    call(&mut w.core, Request::PageTrash { page_id: trashed });
    // A later edit of "beta" by a rename (this device, a later clock).
    drop(w.core);
    let mut core = w.fx.open(2, 5_000);
    call(
        &mut core,
        Request::PageRename {
            page_id: beta,
            title: "beta 2".into(),
        },
    );
    let space = w.space;

    assert_eq!(
        by_property(&mut core, space, w.created, PropertyFilter::HasValue),
        [alpha, beta]
    );
    assert_eq!(
        by_property(
            &mut core,
            space,
            w.edited,
            PropertyFilter::InstantBetween {
                from: Some(2_000),
                to: None
            }
        ),
        [beta]
    );
    assert_eq!(
        by_property(
            &mut core,
            space,
            w.edited,
            PropertyFilter::InstantBetween {
                from: None,
                to: Some(1_000)
            }
        ),
        [alpha],
        "inclusive, live pages only"
    );
    assert_eq!(
        by_property(
            &mut core,
            space,
            w.author,
            PropertyFilter::Equals(PropertyValue::CreatedBy(device))
        ),
        [alpha, beta]
    );
    assert!(
        by_property(
            &mut core,
            space,
            w.editor,
            PropertyFilter::Equals(PropertyValue::LastEditedBy(kb_types::DeviceId::from_bytes(
                [9; 16]
            )))
        )
        .is_empty()
    );

    // Filters that do not fit are refused with the core's messages.
    let refuse = |core: &mut Core, property, filter, message: &str| {
        let err = fails(
            core,
            Request::PagesByProperty {
                space_id: space,
                property_id: property,
                filter,
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(field_of(&err), Some("filter"));
        assert_eq!(err.message, message);
    };
    refuse(
        &mut core,
        w.edited,
        PropertyFilter::InstantBetween {
            from: Some(5),
            to: Some(4),
        },
        "from must not be after to",
    );
    refuse(
        &mut core,
        w.author,
        PropertyFilter::InstantBetween {
            from: None,
            to: None,
        },
        "instants can only be searched in a created or last edited time property",
    );
    refuse(
        &mut core,
        w.edited,
        PropertyFilter::DateBetween {
            from: Some("2026-10-01".into()),
            to: None,
        },
        "dates can only be searched in a date property",
    );
    refuse(
        &mut core,
        w.edited,
        PropertyFilter::Equals(PropertyValue::CreatedBy(device)),
        "the value does not match the kind of the property",
    );
    refuse(
        &mut core,
        w.where_,
        PropertyFilter::InstantBetween {
            from: None,
            to: None,
        },
        "instants can only be searched in a created or last edited time property",
    );
}

#[test]
fn a_place_is_set_found_by_name_filtered_and_survives_a_rebuild() {
    let mut w = world();
    let cafe = create_page(&mut w.core, w.space, None, "Cafe").id;
    let home = create_page(&mut w.core, w.space, None, "Home").id;
    let _plain = create_page(&mut w.core, w.space, None, "Plain").id;
    let flore = place("Café de Flore, Paris", Some((48.854, 2.333)));
    set_value(&mut w.core, cafe, w.where_, Some(flore.clone())).unwrap();
    set_value(
        &mut w.core,
        home,
        w.where_,
        Some(place("Home office", None)),
    )
    .unwrap();
    assert!(values(&mut w.core, cafe).contains(&(w.where_, flore)));

    let space = w.space;
    let check = |core: &mut Core| {
        assert_eq!(
            by_property(
                core,
                space,
                w.where_,
                PropertyFilter::TextContains("PARIS".into())
            ),
            [cafe]
        );
        assert_eq!(
            by_property(core, space, w.where_, PropertyFilter::HasValue),
            [cafe, home]
        );
        let hits = search(core, "Flore");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page_id, cafe);
        assert!(hits[0].block_id.is_none(), "a page-level hit");
    };
    check(&mut w.core);

    let err = fails(
        &mut w.core,
        Request::PagesByProperty {
            space_id: space,
            property_id: w.where_,
            filter: PropertyFilter::Equals(place("Home office", None)),
        },
    );
    assert_eq!(field_of(&err), Some("filter"));
    assert_eq!(
        err.message,
        "a place cannot be compared; use TextContains or HasValue"
    );

    // Rules of the value.
    for (value, message) in [
        (place("", None), "must not be empty"),
        (place("a\nb", None), "must not contain control characters"),
        (
            place("x", Some((91.0, 0.0))),
            "the latitude must be between -90 and 90",
        ),
        (
            place("x", Some((0.0, -181.0))),
            "the longitude must be between -180 and 180",
        ),
        (
            PropertyValue::Place(PlaceValue {
                name: "x".into(),
                lat: Some(1.0),
                lon: None,
            }),
            "the latitude and the longitude go together",
        ),
    ] {
        let err = set_value(&mut w.core, home, w.where_, Some(value)).unwrap_err();
        assert_eq!(field_of(&err), Some("value"));
        assert_eq!(err.message, message);
    }

    // Derived rows are rebuilt from the document, and a restart reads the same.
    call(&mut w.core, Request::IndexRebuild { space_id: None });
    check(&mut w.core);
    drop(w.core);
    let mut core = w.fx.open(2, 2_000);
    check(&mut core);

    // Clearing removes the row and the text.
    set_value(&mut core, cafe, w.where_, None).unwrap();
    assert!(search(&mut core, "Flore").is_empty());
    assert_eq!(
        by_property(&mut core, space, w.where_, PropertyFilter::HasValue),
        [home]
    );
    let _ = create_space;
}
