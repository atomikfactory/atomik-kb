//! Import sessions (ADR-0036, `docs/architecture/import-export.md`, section 3): the protocol and its
//! limits, how properties are matched, created and filled, the structure-first commits, events,
//! warnings, the speed of a large import and the fidelity round trip through a Markdown archive.
//!
//! The protocol, limits and commit tests build their plans by hand (`Plan`), so they run whatever
//! the state of the converters; the end-to-end and round-trip tests at the end need `kb-import` and
//! `kb-export`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use kb_import::{
    ImportValue, PageTarget, Plan, PlannedConfig, PlannedCover, PlannedOption, PlannedPage,
    PlannedProperty, PlannedRollupTarget,
};
use kb_storage::DocQueries;
use kb_types::properties::{MAX_PROPERTIES, TAGS_PROPERTY_ID};
use kb_types::{
    AttachmentId, ChangeSource, DateValue, DocId, ErrorCode, Event, ExportFormat, ImportWarning,
    ImportWarningCode, OptionColor, PageId, PlaceValue, PropertyConfig, PropertyDef,
    PropertyFilter, PropertyKind, PropertyValue, Request, Response, RollupFunction, RollupResult,
    RollupTarget, SpaceId, StatusGroup,
};
use serde_json::json;

use crate::Core;
use crate::test_support::{create_page, create_space, outline, tree};
use crate::test_support_interop::{
    content_of, doc_of, export_all, import_begin, import_commit, import_commit_all, info_of,
    option_add, prop_create, schema, setup, texts_of, upload, value_named, values_of,
};

fn planned_page(parent: Option<usize>, title: &str, seed: u8) -> PlannedPage {
    PlannedPage {
        parent,
        title: title.into(),
        icon: None,
        cover: None,
        cover_y: None,
        template: false,
        template_placeholders: Vec::new(),
        source_path: format!("{title}.md"),
        content: doc_of(seed, &[&format!("{title} text")]),
        properties: Vec::new(),
    }
}

fn plan_of(pages: Vec<PlannedPage>) -> Plan {
    Plan {
        pages,
        ..Plan::default()
    }
}

fn planned_property(name: &str, kind: PropertyKind) -> PlannedProperty {
    PlannedProperty {
        name: name.into(),
        kind,
        options: Vec::new(),
        config: None,
    }
}

fn option(name: &str) -> PlannedOption {
    PlannedOption {
        name: name.into(),
        color: None,
        group: None,
    }
}

fn text(value: &str) -> ImportValue {
    ImportValue::Text(value.into())
}

/// A session with a hand-built plan installed.
fn session_with(core: &mut Core, space: SpaceId, parent: Option<PageId>, plan: Plan) -> u32 {
    let id = import_begin(core, space, parent, None);
    core.import_install_plan_for_test(id, plan).unwrap();
    id
}

fn fails(core: &mut Core, request: Request) -> kb_types::ApiError {
    core.dispatch(request).unwrap_err()
}

fn page_ids_of(view: &kb_types::TreeView) -> BTreeMap<String, PageId> {
    fn walk(nodes: &[kb_types::PageNode], out: &mut BTreeMap<String, PageId>) {
        for node in nodes {
            out.insert(node.page.title.clone(), node.page.id);
            walk(&node.children, out);
        }
    }
    let mut out = BTreeMap::new();
    walk(&view.roots, &mut out);
    out
}

fn has_document(core: &Core, page_id: PageId) -> bool {
    core.db.doc_get(DocId::for_page(page_id)).unwrap().is_some()
}

// ----- the protocol -----------------------------------------------------------------------------

#[test]
fn import_sessions_are_numbered_replaced_and_aborted() {
    let (_fx, mut core, space) = setup();
    let first = import_begin(&mut core, space, None, None);
    let second = import_begin(&mut core, space, None, Some("Notion export"));
    assert_eq!((first, second), (1, 2), "ids count up from 1");

    // A new session replaced the old one.
    let err = fails(
        &mut core,
        Request::ImportAddFile {
            import_id: first,
            path: "a.md".into(),
            text: "x".into(),
        },
    );
    assert_eq!(err.code, ErrorCode::NotFound);

    // Aborting an unknown session is fine; aborting the live one drops it.
    assert_eq!(
        core.dispatch(Request::ImportAbort { import_id: 99 })
            .unwrap(),
        Response::Ok
    );
    assert_eq!(
        core.dispatch(Request::ImportAbort { import_id: second })
            .unwrap(),
        Response::Ok
    );
    for request in [
        Request::ImportAddFile {
            import_id: second,
            path: "a.md".into(),
            text: "x".into(),
        },
        Request::ImportPlan {
            import_id: second,
            binary_paths: Vec::new(),
        },
        Request::ImportCommit {
            import_id: second,
            max_pages: 10,
        },
        Request::ImportAddAttachment {
            import_id: second,
            path: "a.png".into(),
            attachment_id: AttachmentId::from_random_bytes([1; 16]),
        },
    ] {
        assert_eq!(fails(&mut core, request).code, ErrorCode::NotFound);
    }
    assert_eq!(
        import_begin(&mut core, space, None, None),
        3,
        "ids are not reused"
    );
}

#[test]
fn a_session_runs_in_order() {
    let (_fx, mut core, space) = setup();
    let id = import_begin(&mut core, space, None, None);

    // Nothing to commit or attach before the plan.
    let err = fails(
        &mut core,
        Request::ImportCommit {
            import_id: id,
            max_pages: 10,
        },
    );
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = fails(
        &mut core,
        Request::ImportAddAttachment {
            import_id: id,
            path: "a.png".into(),
            attachment_id: AttachmentId::from_random_bytes([1; 16]),
        },
    );
    assert_ne!(err.code, ErrorCode::Internal, "{err:?}");

    core.import_install_plan_for_test(id, Plan::default())
        .unwrap();
    // Planned: no more files, no second plan.
    let err = fails(
        &mut core,
        Request::ImportAddFile {
            import_id: id,
            path: "a.md".into(),
            text: "x".into(),
        },
    );
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = fails(
        &mut core,
        Request::ImportPlan {
            import_id: id,
            binary_paths: Vec::new(),
        },
    );
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // The limits of one commit.
    for bad in [0, 501] {
        let err = fails(
            &mut core,
            Request::ImportCommit {
                import_id: id,
                max_pages: bad,
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}");
        assert_eq!(err.details, Some(json!({ "field": "maxPages" })));
    }
    // An empty plan is done at once, with an empty report; the session is gone after.
    let progress = import_commit(&mut core, id, 500);
    assert!(progress.done);
    assert_eq!((progress.created, progress.total), (0, 0));
    let report = progress.report.unwrap();
    assert_eq!(
        (report.pages, report.attachments, report.properties_created),
        (0, 0, 0)
    );
    assert!(report.root_page_ids.is_empty() && report.warnings.is_empty());
    assert_eq!(
        fails(
            &mut core,
            Request::ImportCommit {
                import_id: id,
                max_pages: 10
            }
        )
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn the_destination_must_be_a_live_page_of_the_space() {
    let (_fx, mut core, space) = setup();
    let other = create_space(&mut core, "Other");
    let foreign = create_page(&mut core, other.id, None, "Foreign");
    let trashed = create_page(&mut core, space, None, "Trashed");
    core.dispatch(Request::PageTrash {
        page_id: trashed.id,
    })
    .unwrap();
    let begin = |space_id, parent_id| Request::ImportBegin {
        space_id,
        parent_id,
        source_name: None,
    };

    let err = fails(&mut core, begin(space, Some(trashed.id)));
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details, Some(json!({ "field": "parentId" })));
    let err = fails(&mut core, begin(space, Some(foreign.id)));
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = fails(
        &mut core,
        begin(space, Some(PageId::from_random_bytes([2; 16]))),
    );
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = fails(&mut core, begin(SpaceId::from_random_bytes([3; 16]), None));
    assert_eq!(err.code, ErrorCode::NotFound);
    // A failed begin does not replace the running session.
    let id = import_begin(&mut core, space, None, None);
    let _ = fails(&mut core, begin(space, Some(trashed.id)));
    core.dispatch(Request::ImportAddFile {
        import_id: id,
        path: "still.md".into(),
        text: "here".into(),
    })
    .unwrap();
}

#[test]
fn files_are_checked_when_they_are_added() {
    let (_fx, mut core, space) = setup();
    let id = import_begin(&mut core, space, None, None);
    let add = |core: &mut Core, path: &str, text: &str| {
        core.dispatch(Request::ImportAddFile {
            import_id: id,
            path: path.into(),
            text: text.into(),
        })
    };

    let long_segment = "a".repeat(1_025);
    for bad in [
        "",
        "/absolute.md",
        "../up.md",
        "a/../b.md",
        "a/./b.md",
        "a//b.md",
        "trailing/",
        "back\\slash.md",
        "C:\\windows.md",
        "C:file.md",
        "nul\0byte.md",
        "tab\tinside.md",
        long_segment.as_str(),
    ] {
        let err = add(&mut core, bad, "x").unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
        assert_eq!(err.details, Some(json!({ "field": "path" })), "{bad:?}");
        assert!(!err.message.contains("windows"), "messages carry no path");
    }
    // A path of exactly 1,024 bytes, nested names and non-ASCII names are fine.
    add(&mut core, &"a".repeat(1_024), "x").unwrap();
    add(&mut core, "notes/Été/日本語.md", "x").unwrap();
    add(&mut core, "notes/a:b.md", "x").unwrap();

    // A file added twice.
    let err = add(&mut core, "notes/Été/日本語.md", "again").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // 16 MiB is the most a file may hold.
    add(&mut core, "big.md", &"x".repeat(16 * 1024 * 1024)).unwrap();
    let err = add(&mut core, "bigger.md", &"x".repeat(16 * 1024 * 1024 + 1)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details, Some(json!({ "field": "text" })));
}

#[test]
fn an_import_takes_at_most_ten_thousand_files() {
    let (_fx, mut core, space) = setup();
    let id = import_begin(&mut core, space, None, None);
    let have = 3;
    for n in 0..(10_000 - have) {
        core.dispatch(Request::ImportAddFile {
            import_id: id,
            path: format!("f{n}.md"),
            text: String::new(),
        })
        .unwrap();
    }
    for n in 0..have {
        core.dispatch(Request::ImportAddFile {
            import_id: id,
            path: format!("g{n}.md"),
            text: String::new(),
        })
        .unwrap();
    }
    let err = core
        .dispatch(Request::ImportAddFile {
            import_id: id,
            path: "one-too-many.md".into(),
            text: String::new(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn the_binary_paths_of_a_plan_are_checked() {
    let (_fx, mut core, space) = setup();
    let id = import_begin(&mut core, space, None, None);
    for bad in ["../x.png", "/x.png", "a//b.png", "a\\b.png", ""] {
        let err = fails(
            &mut core,
            Request::ImportPlan {
                import_id: id,
                binary_paths: vec!["ok.png".into(), bad.into()],
            },
        );
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
        assert_eq!(err.details, Some(json!({ "field": "binaryPaths" })));
    }
    let err = fails(
        &mut core,
        Request::ImportPlan {
            import_id: id,
            binary_paths: vec!["a.png".into(); 100_001],
        },
    );
    assert_eq!(err.code, ErrorCode::InvalidInput);
    // A refused plan leaves the session open for files.
    core.dispatch(Request::ImportAddFile {
        import_id: id,
        path: "a.md".into(),
        text: "x".into(),
    })
    .unwrap();
}

#[test]
fn only_needed_complete_attachments_of_the_space_are_accepted() {
    let (_fx, mut core, space) = setup();
    let other = create_space(&mut core, "Other");
    let ours = upload(&mut core, space, "ours.png", "image/png", b"ours");
    let theirs = upload(&mut core, other.id, "theirs.png", "image/png", b"theirs");
    let partial = upload(&mut core, space, "partial.png", "image/png", b"partial");
    crate::test_support_interop::make_not_downloaded(&mut core, partial.attachment_id);

    let id = import_begin(&mut core, space, None, None);
    let mut plan = plan_of(vec![planned_page(None, "Page", 1)]);
    plan.needed_files = vec!["img/ours.png".into(), "img/partial.png".into()];
    core.import_install_plan_for_test(id, plan).unwrap();
    let attach = |core: &mut Core, path: &str, attachment: AttachmentId| {
        core.dispatch(Request::ImportAddAttachment {
            import_id: id,
            path: path.into(),
            attachment_id: attachment,
        })
    };

    let err = attach(&mut core, "img/unlisted.png", ours.attachment_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details, Some(json!({ "field": "path" })));
    let err = attach(
        &mut core,
        "img/ours.png",
        AttachmentId::from_random_bytes([4; 16]),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = attach(&mut core, "img/ours.png", theirs.attachment_id).unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidInput,
        "another space's attachment"
    );
    let err = attach(&mut core, "img/partial.png", partial.attachment_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "not all of it is here");
    attach(&mut core, "img/ours.png", ours.attachment_id).unwrap();
    assert_eq!(
        info_of(&mut core, ours.attachment_id).attachment_id,
        ours.attachment_id
    );
}

// ----- the commits ------------------------------------------------------------------------------

#[test]
fn the_first_commit_creates_every_page_and_later_ones_write_the_documents() {
    let (_fx, mut core, space) = setup();
    let inbox = create_page(&mut core, space, None, "Inbox");
    create_page(&mut core, space, Some(inbox.id), "Old");
    let mut root_a = planned_page(None, "Root A", 1);
    root_a.icon = Some("📝".into());
    let plan = plan_of(vec![
        root_a,
        planned_page(Some(0), "Child", 2),
        planned_page(Some(1), "Grandchild", 3),
        planned_page(None, "Root B", 4),
        planned_page(Some(0), "Sibling", 5),
    ]);
    let id = session_with(&mut core, space, Some(inbox.id), plan);
    core.drain_events();

    let first = import_commit(&mut core, id, 2);
    assert!(!first.done);
    assert_eq!((first.created, first.total), (2, 5));
    assert!(first.report.is_none());
    let events = core.drain_events();
    assert!(events.contains(&Event::StructureChanged { space_id: space }));
    assert!(events.contains(&Event::ImportProgress {
        import_id: id,
        done: 2,
        total: 5
    }));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                Event::PageContentChanged {
                    source: ChangeSource::Core,
                    ..
                }
            ))
            .count(),
        2,
        "two documents were written"
    );

    // Every page is in the tree after the first call, appended after the destination's children.
    let view = tree(&mut core, space);
    assert_eq!(
        outline(&view),
        "Inbox\n  Old\n  Root A\n    Child\n      Grandchild\n    Sibling\n  Root B\n"
    );
    let ids = page_ids_of(&view);
    let written: Vec<bool> = ["Root A", "Child", "Grandchild", "Root B", "Sibling"]
        .iter()
        .map(|title| has_document(&core, ids[*title]))
        .collect();
    assert_eq!(
        written,
        [true, true, false, false, false],
        "two documents so far"
    );
    assert_eq!(
        crate::test_support::page(&mut core, ids["Root A"])
            .icon
            .as_deref(),
        Some("📝")
    );

    let second = import_commit(&mut core, id, 2);
    assert_eq!((second.created, second.total, second.done), (4, 5, false));
    let last = import_commit(&mut core, id, 2);
    assert_eq!((last.created, last.total, last.done), (5, 5, true));
    let report = last.report.unwrap();
    assert_eq!(report.pages, 5);
    assert_eq!(report.root_page_ids, [ids["Root A"], ids["Root B"]]);
    assert_eq!(
        (
            report.attachments,
            report.properties_created,
            report.more_warnings
        ),
        (0, 0, 0)
    );
    assert!(report.warnings.is_empty());

    for title in ["Root A", "Child", "Grandchild", "Root B", "Sibling"] {
        let content = content_of(&mut core, ids[title]);
        assert_eq!(texts_of(&content), [format!("{title} text")], "{title}");
    }
    // The titles are searchable and the session is over.
    let hits = match core
        .dispatch(Request::Search {
            query: "Grandchild".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    };
    assert!(hits.iter().any(|hit| hit.page_id == ids["Grandchild"]));
    assert_eq!(
        fails(
            &mut core,
            Request::ImportCommit {
                import_id: id,
                max_pages: 1
            }
        )
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn titles_are_made_valid_and_the_root_of_the_space_is_a_destination() {
    let (_fx, mut core, space) = setup();
    create_page(&mut core, space, None, "Existing");
    let long = "x".repeat(600);
    let plan = plan_of(vec![
        planned_page(None, "  Tabs\tand\nlines  ", 1),
        planned_page(None, &long, 2),
        planned_page(None, "", 3),
    ]);
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 50);
    assert_eq!(report.pages, 3);
    let view = tree(&mut core, space);
    let titles: Vec<String> = view.roots.iter().map(|n| n.page.title.clone()).collect();
    assert_eq!(titles[0], "Existing", "appended after the existing pages");
    assert_eq!(titles[1], "Tabs and lines");
    assert_eq!(titles[2].chars().count(), 500);
    assert_eq!(titles[3], "");
}

#[test]
fn a_page_opened_during_the_import_keeps_both_its_texts() {
    let (_fx, mut core, space) = setup();
    let plan = plan_of(vec![
        planned_page(None, "One", 1),
        planned_page(None, "Two", 2),
        planned_page(None, "Three", 3),
    ]);
    let id = session_with(&mut core, space, None, plan);
    import_commit(&mut core, id, 1);
    let ids = page_ids_of(&tree(&mut core, space));
    // The sidebar shows "Three" at once and the person opens it: the core creates its document.
    assert!(!has_document(&core, ids["Three"]));
    core.page_doc_open(ids["Three"]).unwrap();
    assert!(has_document(&core, ids["Three"]));
    let (report, _) = import_commit_all(&mut core, id, 10);
    assert_eq!(report.pages, 3);
    let content = content_of(&mut core, ids["Three"]);
    assert!(
        texts_of(&content).contains(&"Three text".to_owned()),
        "the imported text is in the page: {content}"
    );
}

#[test]
fn warnings_are_listed_in_order_up_to_five_hundred() {
    let (_fx, mut core, space) = setup();
    let warning = |n: usize| ImportWarning {
        path: format!("f{n}.md"),
        code: ImportWarningCode::UnresolvedLink,
        detail: Some(format!("target {n}")),
    };
    let mut plan = plan_of(vec![planned_page(None, "Only", 1)]);
    plan.warnings = (0..600).map(warning).collect();
    plan.properties = vec![planned_property("Estimate", PropertyKind::Number)];
    plan.pages[0].properties = vec![("Estimate".into(), text("not a number"))];
    let id = import_begin(&mut core, space, None, None);
    let info = core.import_install_plan_for_test(id, plan).unwrap();
    assert_eq!(
        info.warnings.len(),
        600,
        "the plan's own answer lists them all"
    );
    let (report, _) = import_commit_all(&mut core, id, 10);
    assert_eq!(report.warnings.len(), 500);
    assert_eq!(
        report.more_warnings, 101,
        "600 + the coercion warning - 500"
    );
    assert_eq!(
        report.warnings[0].path, "f0.md",
        "planning warnings come first"
    );
    assert_eq!(report.warnings[499].path, "f499.md");

    // With few of them, the order is planning, then coercion.
    let mut plan = plan_of(vec![planned_page(None, "Second", 2)]);
    plan.warnings = (0..2).map(warning).collect();
    plan.properties = vec![planned_property("Count", PropertyKind::Number)];
    plan.pages[0].properties = vec![("Count".into(), text("x"))];
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);
    let codes: Vec<ImportWarningCode> = report.warnings.iter().map(|w| w.code).collect();
    assert_eq!(
        codes,
        [
            ImportWarningCode::UnresolvedLink,
            ImportWarningCode::UnresolvedLink,
            ImportWarningCode::InvalidValue
        ]
    );
    assert_eq!(report.warnings[2].detail.as_deref(), Some("Count"));
    assert_eq!(report.warnings[2].path, "Second.md");
}

#[test]
fn people_sent_as_text_never_become_a_person_value_and_a_planned_person_column_is_left_out() {
    let (_fx, mut core, space) = setup();
    // The space has a Person property already; an exported file brings its people as text (the
    // importer cannot name members of a space), and one column that still says Person.
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person);
    let mut plan = plan_of(vec![
        planned_page(None, "Alpha", 1),
        planned_page(None, "Beta", 2),
    ]);
    plan.properties = vec![
        planned_property("Owner", PropertyKind::Text),
        planned_property("Reviewers", PropertyKind::Person),
        planned_property("Notes", PropertyKind::Text),
    ];
    plan.pages[0].properties = vec![
        ("Owner".into(), text("Alice, Bob")),
        ("Reviewers".into(), text("Carol")),
        ("Notes".into(), text("kept")),
    ];
    plan.pages[1].properties = vec![("Owner".into(), text("Dave"))];
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);

    // Nothing failed, the Person property is as it was and holds no value, no Person property was
    // created, and the text column that does not collide came through.
    let defs = schema(&mut core, space);
    assert_eq!(
        defs.iter()
            .filter(|def| def.kind == PropertyKind::Person)
            .map(|def| def.id)
            .collect::<Vec<_>>(),
        [owner.id]
    );
    assert!(defs.iter().all(|def| def.name != "Reviewers"));
    let pages = page_ids_of(&tree(&mut core, space));
    for title in ["Alpha", "Beta"] {
        assert_eq!(
            value_named(&mut core, space, pages[title], "Owner"),
            None,
            "{title}"
        );
    }
    assert_eq!(
        value_named(&mut core, space, pages["Alpha"], "Notes"),
        Some(PropertyValue::Text("kept".into()))
    );
    let seen: Vec<(String, ImportWarningCode, Option<String>)> = report
        .warnings
        .iter()
        .map(|w| (w.path.clone(), w.code, w.detail.clone()))
        .collect();
    assert_eq!(
        seen,
        [
            (
                String::new(),
                ImportWarningCode::UnsupportedContent,
                Some("Reviewers".to_owned())
            ),
            (
                "Alpha.md".to_owned(),
                ImportWarningCode::InvalidValue,
                Some("Owner".to_owned())
            ),
            (
                "Beta.md".to_owned(),
                ImportWarningCode::InvalidValue,
                Some("Owner".to_owned())
            ),
        ],
        "one warning for the column that cannot be created, one per value the Person property refuses"
    );
}

#[test]
fn covers_of_planned_pages_are_named_or_uploaded_images_and_the_rest_is_reported() {
    let (_fx, mut core, space) = setup();
    let banner = upload(&mut core, space, "banner.png", "image/png", b"banner");
    let pdf = upload(&mut core, space, "notes.pdf", "application/pdf", b"pdf");
    let covered = |title: &str, seed: u8, cover: Option<PlannedCover>, y: Option<u8>| {
        let mut page = planned_page(None, title, seed);
        page.cover = cover;
        page.cover_y = y;
        page
    };
    let mut plan = plan_of(vec![
        covered(
            "Named",
            1,
            Some(PlannedCover::Named("color:blue".into())),
            Some(30),
        ),
        covered(
            "Gradient",
            2,
            Some(PlannedCover::Named("gradient:dawn".into())),
            None,
        ),
        covered(
            "Picture",
            3,
            Some(PlannedCover::File("img/banner.png".into())),
            Some(80),
        ),
        covered(
            "Same picture",
            4,
            Some(PlannedCover::File("img/banner.png".into())),
            Some(50),
        ),
        covered(
            "Not uploaded",
            5,
            Some(PlannedCover::File("img/gone.png".into())),
            None,
        ),
        covered(
            "Not an image",
            6,
            Some(PlannedCover::File("files/notes.pdf".into())),
            None,
        ),
        covered(
            "Bad name",
            7,
            Some(PlannedCover::Named("color:teal".into())),
            None,
        ),
        covered(
            "Bad position",
            8,
            Some(PlannedCover::Named("color:red".into())),
            Some(200),
        ),
        covered("Plain", 9, None, Some(40)),
    ]);
    plan.needed_files = vec![
        "img/banner.png".into(),
        "img/gone.png".into(),
        "files/notes.pdf".into(),
    ];
    let id = import_begin(&mut core, space, None, None);
    core.import_install_plan_for_test(id, plan).unwrap();
    for (path, attachment) in [
        ("img/banner.png", banner.attachment_id),
        ("files/notes.pdf", pdf.attachment_id),
    ] {
        core.dispatch(Request::ImportAddAttachment {
            import_id: id,
            path: path.into(),
            attachment_id: attachment,
        })
        .unwrap();
    }
    let (report, _) = import_commit_all(&mut core, id, 3);

    let ids = page_ids_of(&tree(&mut core, space));
    let cover = |core: &mut Core, title: &str| {
        let info = crate::test_support::page(core, ids[title]);
        (info.cover, info.cover_y)
    };
    assert_eq!(
        cover(&mut core, "Named"),
        (Some("color:blue".into()), Some(30))
    );
    assert_eq!(
        cover(&mut core, "Gradient"),
        (Some("gradient:dawn".into()), None)
    );
    let image = format!("attachment:{}", banner.attachment_id);
    assert_eq!(cover(&mut core, "Picture"), (Some(image.clone()), Some(80)));
    assert_eq!(
        cover(&mut core, "Same picture"),
        (Some(image), None),
        "50 is the default and is stored as none"
    );
    for title in ["Not uploaded", "Not an image", "Bad name", "Plain"] {
        assert_eq!(cover(&mut core, title), (None, None), "{title}");
    }
    assert_eq!(
        cover(&mut core, "Bad position"),
        (Some("color:red".into()), None),
        "a position out of range is the default"
    );

    // The image of the covers is referenced by the two pages that use it (and counted once).
    assert_eq!(info_of(&mut core, banner.attachment_id).references, 2);
    assert_eq!(info_of(&mut core, pdf.attachment_id).references, 0);
    assert_eq!(report.attachments, 1);
    let warnings: Vec<(String, ImportWarningCode, Option<String>)> = report
        .warnings
        .iter()
        .map(|w| (w.path.clone(), w.code, w.detail.clone()))
        .collect();
    assert_eq!(
        warnings,
        [
            (
                "Not uploaded.md".to_owned(),
                ImportWarningCode::MissingFile,
                Some("img/gone.png".to_owned())
            ),
            (
                "Not an image.md".to_owned(),
                ImportWarningCode::UnsupportedContent,
                Some("cover".to_owned())
            ),
            (
                "Bad name.md".to_owned(),
                ImportWarningCode::UnsupportedContent,
                Some("cover".to_owned())
            ),
        ]
    );
    // An import is not an undo step: undoing in the space does not take the covers away.
    assert!(!crate::test_support::tree(&mut core, space).undo.can_undo);
}

#[test]
fn aborting_keeps_the_pages_already_written() {
    let (_fx, mut core, space) = setup();
    let plan = plan_of(vec![
        planned_page(None, "One", 1),
        planned_page(None, "Two", 2),
        planned_page(None, "Three", 3),
    ]);
    let id = session_with(&mut core, space, None, plan);
    let progress = import_commit(&mut core, id, 1);
    assert_eq!(
        (progress.created, progress.total, progress.done),
        (1, 3, false)
    );
    assert_eq!(
        core.dispatch(Request::ImportAbort { import_id: id })
            .unwrap(),
        Response::Ok
    );
    // The pages are in the tree (the first one with its content); the session is gone.
    let view = tree(&mut core, space);
    assert_eq!(outline(&view), "One\nTwo\nThree\n");
    let ids = page_ids_of(&view);
    assert!(has_document(&core, ids["One"]));
    assert!(!has_document(&core, ids["Two"]));
    assert_eq!(
        fails(
            &mut core,
            Request::ImportCommit {
                import_id: id,
                max_pages: 10
            }
        )
        .code,
        ErrorCode::NotFound
    );
    // Opening one of the pages the import did not reach gives an ordinary empty page.
    core.page_doc_open(ids["Two"]).unwrap();
    assert!(texts_of(&content_of(&mut core, ids["Two"])).is_empty());
}

fn undo(core: &mut Core, space_id: SpaceId, redo: bool) -> kb_types::TreeView {
    let request = if redo {
        Request::StructureRedo { space_id }
    } else {
        Request::StructureUndo { space_id }
    };
    match core.dispatch(request).unwrap() {
        Response::Tree(view) => view,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn an_import_is_not_an_undo_step_and_the_persons_earlier_steps_survive_it() {
    let (_fx, mut core, space) = setup();
    // Three structural edits of the person's own...
    let before = create_page(&mut core, space, None, "Before");
    core.dispatch(Request::PageRename {
        page_id: before.id,
        title: "Renamed".into(),
    })
    .unwrap();
    create_page(&mut core, space, None, "Second");
    // ...then an import of 150 pages (more than the 100 steps the undo stack holds), some nested.
    let mut pages: Vec<PlannedPage> = Vec::new();
    for n in 0..150usize {
        let parent = if n % 3 == 2 { Some(n - 1) } else { None };
        pages.push(planned_page(parent, &format!("Imported {n}"), 1));
    }
    let id = session_with(&mut core, space, None, plan_of(pages));
    let (report, _) = import_commit_all(&mut core, id, 50);
    assert_eq!(report.pages, 150);
    let imported = |core: &mut Core| {
        page_ids_of(&tree(core, space))
            .keys()
            .filter(|title| title.starts_with("Imported"))
            .count()
    };
    assert_eq!(imported(&mut core), 150);
    assert!(tree(&mut core, space).undo.can_undo);

    // Undo goes back through the person's three steps and leaves the imported pages alone.
    let view = undo(&mut core, space, false);
    assert!(!outline(&view).contains("Second"), "{}", outline(&view));
    assert!(outline(&view).contains("Renamed"));
    assert_eq!(imported(&mut core), 150);
    let view = undo(&mut core, space, false);
    assert!(outline(&view).contains("Before") && !outline(&view).contains("Renamed"));
    let view = undo(&mut core, space, false);
    assert!(!outline(&view).contains("Before"));
    assert!(!view.undo.can_undo, "the import left nothing to undo");
    assert_eq!(imported(&mut core), 150);
    // Redo brings the person's steps back, and only those.
    undo(&mut core, space, true);
    undo(&mut core, space, true);
    let view = undo(&mut core, space, true);
    assert!(outline(&view).contains("Renamed") && outline(&view).contains("Second"));
    assert!(!view.undo.can_redo);
    assert_eq!(imported(&mut core), 150);
}

#[test]
#[allow(clippy::print_stderr)]
fn a_flat_import_of_thousands_of_pages_creates_its_tree_quickly() {
    let (_fx, mut core, space) = setup();
    let pages: Vec<PlannedPage> = (0..5_000)
        .map(|n| planned_page(None, &format!("Flat {n}"), 1))
        .collect();
    let id = session_with(&mut core, space, None, plan_of(pages));
    let started = std::time::Instant::now();
    let first = import_commit(&mut core, id, 1);
    let elapsed = started.elapsed();
    eprintln!("5,000 flat pages: the first commit took {elapsed:?} (debug build)");
    assert_eq!((first.created, first.total), (1, 5_000));
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "creating the tree took {elapsed:?}"
    );
    let view = tree(&mut core, space);
    assert_eq!(view.roots.len(), 5_000);
    assert_eq!(view.roots[0].page.title, "Flat 0");
    assert_eq!(view.roots[4_999].page.title, "Flat 4999");
    core.dispatch(Request::ImportAbort { import_id: id })
        .unwrap();
}

// ----- properties -------------------------------------------------------------------------------

#[test]
fn properties_are_matched_by_name_or_created_and_values_fit_their_kind() {
    let (_fx, mut core, space) = setup();
    // The space already has these.
    let priority = prop_create(&mut core, space, "Priority", PropertyKind::Select);
    let high = option_add(&mut core, space, priority.id, "High", OptionColor::Blue);
    let estimate = prop_create(&mut core, space, "estimate", PropertyKind::Number);
    option_add(&mut core, space, TAGS_PROPERTY_ID, "work", OptionColor::Red);

    let report_file = upload(&mut core, space, "report.pdf", "application/pdf", b"%PDF");
    let stage = PlannedProperty {
        options: vec![
            PlannedOption {
                name: "Draft".into(),
                color: Some(OptionColor::Gray),
                group: Some(StatusGroup::Todo),
            },
            PlannedOption {
                name: "Review".into(),
                color: Some(OptionColor::Orange),
                group: Some(StatusGroup::InProgress),
            },
            PlannedOption {
                name: "Shipped".into(),
                color: Some(OptionColor::Green),
                group: Some(StatusGroup::Done),
            },
        ],
        ..planned_property("Stage", PropertyKind::Status)
    };
    let labels = PlannedProperty {
        options: vec![option("a"), option("b")],
        ..planned_property("Labels", PropertyKind::MultiSelect)
    };
    let mut plan = plan_of(vec![
        planned_page(None, "Alpha", 1),
        planned_page(None, "Beta", 2),
    ]);
    plan.properties = vec![
        // The same names as existing properties, in another case and with another kind.
        PlannedProperty {
            options: vec![option("High"), option("Low")],
            ..planned_property("priority", PropertyKind::MultiSelect)
        },
        planned_property("ESTIMATE", PropertyKind::Text),
        PlannedProperty {
            options: vec![option("work")],
            ..planned_property("tags", PropertyKind::MultiSelect)
        },
        stage,
        labels,
        planned_property("Shipped", PropertyKind::Checkbox),
        planned_property("Due", PropertyKind::Date),
        planned_property("Link", PropertyKind::Url),
        planned_property("Where", PropertyKind::Place),
        planned_property("Todo", PropertyKind::Checklist),
        planned_property("Docs", PropertyKind::Files),
        PlannedProperty {
            config: Some(PlannedConfig::Relation {
                two_way: true,
                pair_name: Some("Back links".into()),
            }),
            ..planned_property("Related", PropertyKind::Relation)
        },
        PlannedProperty {
            config: Some(PlannedConfig::Rollup {
                relation: "Related".into(),
                target: PlannedRollupTarget::Property("Estimate".into()),
                function: RollupFunction::Sum,
            }),
            ..planned_property("Total", PropertyKind::Rollup)
        },
    ];
    plan.needed_files = vec!["files/report.pdf".into()];
    plan.pages[0].properties = vec![
        ("priority".into(), ImportValue::List(vec!["High".into()])),
        ("ESTIMATE".into(), text("3.5")),
        (
            "tags".into(),
            ImportValue::List(vec!["work".into(), "brand new".into()]),
        ),
        ("Stage".into(), text("Review")),
        (
            "Labels".into(),
            ImportValue::List(vec!["a".into(), "c".into()]),
        ),
        ("Shipped".into(), ImportValue::Bool(true)),
        (
            "Due".into(),
            ImportValue::Date {
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
            },
        ),
        ("Link".into(), text("https://example.com/a")),
        (
            "Where".into(),
            ImportValue::Place {
                name: "Paris".into(),
                lat: Some(48.8566),
                lon: Some(2.3522),
            },
        ),
        (
            "Todo".into(),
            ImportValue::Checklist(vec![(true, "buy milk".into()), (false, "call Ann".into())]),
        ),
        (
            "Docs".into(),
            ImportValue::Files(vec!["files/report.pdf".into()]),
        ),
        (
            "Related".into(),
            ImportValue::Pages(vec![
                PageTarget::Planned(1),
                PageTarget::Title("Nowhere".into()),
            ]),
        ),
    ];
    plan.pages[1].properties = vec![
        ("ESTIMATE".into(), ImportValue::Number(2.0)),
        ("Link".into(), text("not a url")),
        ("Shipped".into(), text("maybe")),
    ];
    let id = session_with(&mut core, space, None, plan);
    core.dispatch(Request::ImportAddAttachment {
        import_id: id,
        path: "files/report.pdf".into(),
        attachment_id: report_file.attachment_id,
    })
    .unwrap();
    core.drain_events();
    let (report, _) = import_commit_all(&mut core, id, 50);
    let events = core.drain_events();
    assert!(events.contains(&Event::PropertySchemaChanged { space_id: space }));

    // The schema: reused by name whatever the planned kind, the rest created as planned.
    let defs = schema(&mut core, space);
    let named = |name: &str| -> &PropertyDef {
        defs.iter()
            .find(|def| def.name == name)
            .unwrap_or_else(|| panic!("no property {name}"))
    };
    assert_eq!(
        defs.iter()
            .filter(|def| def.name.eq_ignore_ascii_case("priority"))
            .count(),
        1,
        "matched without regard to case"
    );
    assert_eq!(named("Priority").id, priority.id);
    assert_eq!(
        named("Priority").kind,
        PropertyKind::Select,
        "the kind of the existing one"
    );
    let option_names = |def: &PropertyDef| -> Vec<(String, OptionColor)> {
        def.options
            .iter()
            .map(|o| (o.name.clone(), o.color))
            .collect()
    };
    assert_eq!(
        option_names(named("Priority")),
        [
            ("High".to_owned(), OptionColor::Blue),
            ("Low".to_owned(), OptionColor::Purple)
        ],
        "a missing option is added with the next colour of the palette"
    );
    assert_eq!(named("estimate").id, estimate.id);
    assert!(defs.iter().all(|def| def.name != "ESTIMATE"));
    assert_eq!(named("Tags").id, TAGS_PROPERTY_ID);
    assert_eq!(
        named("Tags")
            .options
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["work", "brand new"]
    );

    let stage_def = named("Stage");
    assert_eq!(stage_def.kind, PropertyKind::Status);
    assert_eq!(
        stage_def
            .options
            .iter()
            .map(|o| (o.name.as_str(), o.color, o.group))
            .collect::<Vec<_>>(),
        [
            ("Draft", OptionColor::Gray, Some(StatusGroup::Todo)),
            ("Review", OptionColor::Orange, Some(StatusGroup::InProgress)),
            ("Shipped", OptionColor::Green, Some(StatusGroup::Done)),
        ],
        "the defaults give way to the planned options"
    );
    assert_eq!(
        option_names(named("Labels")),
        [
            ("a".to_owned(), OptionColor::Gray),
            ("b".to_owned(), OptionColor::Brown),
            ("c".to_owned(), OptionColor::Orange)
        ],
        "colours follow the palette, a value may add an option"
    );
    for (name, kind) in [
        ("Shipped", PropertyKind::Checkbox),
        ("Due", PropertyKind::Date),
        ("Link", PropertyKind::Url),
        ("Where", PropertyKind::Place),
        ("Todo", PropertyKind::Checklist),
        ("Docs", PropertyKind::Files),
    ] {
        assert_eq!(named(name).kind, kind, "{name}");
    }
    let related = named("Related");
    let back = named("Back links");
    assert_eq!(
        related.config,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(back.id)
        })
    );
    assert_eq!(
        back.config,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(related.id)
        })
    );
    assert_eq!(
        named("Total").config,
        Some(PropertyConfig::Rollup {
            relation_property_id: related.id,
            target: RollupTarget::Property(estimate.id),
            function: RollupFunction::Sum
        })
    );
    // Stage, Labels, Shipped, Due, Link, Where, Todo, Docs, Related and its pair, Total.
    assert_eq!(report.properties_created, 11);
    assert_eq!(report.attachments, 1);

    // The values of the first page.
    let ids = page_ids_of(&tree(&mut core, space));
    let (alpha, beta) = (ids["Alpha"], ids["Beta"]);
    let value = |core: &mut Core, page: PageId, name: &str| value_named(core, space, page, name);
    assert_eq!(
        value(&mut core, alpha, "Priority"),
        Some(PropertyValue::Select(high.id))
    );
    assert_eq!(
        value(&mut core, alpha, "estimate"),
        Some(PropertyValue::Number(3.5))
    );
    let tag_options: Vec<_> = named("Tags").options.iter().map(|o| o.id).collect();
    assert_eq!(
        value(&mut core, alpha, "Tags"),
        Some(PropertyValue::MultiSelect(tag_options))
    );
    assert_eq!(
        value(&mut core, alpha, "Stage"),
        Some(PropertyValue::Status(stage_def.options[1].id))
    );
    let label_ids: Vec<_> = named("Labels").options.iter().map(|o| o.id).collect();
    assert_eq!(
        value(&mut core, alpha, "Labels"),
        Some(PropertyValue::MultiSelect(vec![label_ids[0], label_ids[2]]))
    );
    assert_eq!(
        value(&mut core, alpha, "Shipped"),
        Some(PropertyValue::Checkbox(true))
    );
    assert_eq!(
        value(&mut core, alpha, "Due"),
        Some(PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into())
        }))
    );
    assert_eq!(
        value(&mut core, alpha, "Link"),
        Some(PropertyValue::Url("https://example.com/a".into()))
    );
    assert_eq!(
        value(&mut core, alpha, "Where"),
        Some(PropertyValue::Place(PlaceValue {
            name: "Paris".into(),
            lat: Some(48.8566),
            lon: Some(2.3522)
        }))
    );
    match value(&mut core, alpha, "Todo") {
        Some(PropertyValue::Checklist(items)) => {
            let seen: Vec<(bool, &str)> = items.iter().map(|i| (i.done, i.text.as_str())).collect();
            assert_eq!(seen, [(true, "buy milk"), (false, "call Ann")]);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        value(&mut core, alpha, "Docs"),
        Some(PropertyValue::Files(vec![report_file.attachment_id]))
    );
    assert_eq!(
        value(&mut core, alpha, "Related"),
        Some(PropertyValue::Relation(vec![beta])),
        "the planned page; the title nobody has is dropped"
    );
    assert_eq!(
        value(&mut core, beta, "Back links"),
        Some(PropertyValue::Relation(vec![alpha])),
        "the paired side shows it too"
    );
    match value(&mut core, alpha, "Total") {
        Some(PropertyValue::Rollup(rollup)) => {
            assert_eq!(rollup.result, RollupResult::Number(2.0));
        }
        other => panic!("unexpected {other:?}"),
    }

    // The second page: a number from a number, a URL and a checkbox that do not fit.
    assert_eq!(
        value(&mut core, beta, "estimate"),
        Some(PropertyValue::Number(2.0))
    );
    assert_eq!(value(&mut core, beta, "Link"), None);
    assert_eq!(value(&mut core, beta, "Shipped"), None);
    let seen: Vec<(String, ImportWarningCode, Option<String>)> = report
        .warnings
        .iter()
        .map(|w| (w.path.clone(), w.code, w.detail.clone()))
        .collect();
    assert_eq!(
        seen,
        [
            (
                "Alpha.md".to_owned(),
                ImportWarningCode::UnresolvedLink,
                Some("Nowhere".to_owned())
            ),
            (
                "Beta.md".to_owned(),
                ImportWarningCode::InvalidValue,
                Some("Link".to_owned())
            ),
            (
                "Beta.md".to_owned(),
                ImportWarningCode::InvalidValue,
                Some("Shipped".to_owned())
            ),
        ]
    );
    // Values are searchable like any other (the derived rows were written with them).
    let found = match core
        .dispatch(Request::PagesByProperty {
            space_id: space,
            property_id: estimate.id,
            filter: PropertyFilter::HasValue,
        })
        .unwrap()
    {
        Response::PageIds(ids) => ids,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(found.len(), 2);
}

#[test]
fn a_property_named_like_an_existing_one_is_reused_by_every_folding() {
    let (_fx, mut core, space) = setup();
    let status = prop_create(&mut core, space, "Statut Évalué", PropertyKind::Text);
    let mut plan = plan_of(vec![planned_page(None, "P", 1)]);
    plan.properties = vec![planned_property("STATUT ÉVALUÉ", PropertyKind::Number)];
    plan.pages[0].properties = vec![("STATUT ÉVALUÉ".into(), text("fine"))];
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);
    assert_eq!(report.properties_created, 0);
    let page = page_ids_of(&tree(&mut core, space))["P"];
    assert_eq!(
        values_of(&mut core, page),
        [(status.id, PropertyValue::Text("fine".into()))],
        "Unicode lower-casing, and the text property takes the text"
    );
}

#[test]
fn a_planned_status_replaces_the_default_options_in_the_plan_order() {
    let (_fx, mut core, space) = setup();
    let mut plan = plan_of(vec![planned_page(None, "P", 1)]);
    plan.properties = vec![
        PlannedProperty {
            options: vec![
                PlannedOption {
                    name: "Done".into(),
                    color: Some(OptionColor::Pink),
                    group: Some(StatusGroup::Done),
                },
                option("Blocked"),
                PlannedOption {
                    name: "In progress".into(),
                    color: None,
                    group: Some(StatusGroup::InProgress),
                },
            ],
            ..planned_property("Stage", PropertyKind::Status)
        },
        // Without planned options a new status property keeps its three defaults.
        planned_property("Plain", PropertyKind::Status),
    ];
    let id = session_with(&mut core, space, None, plan);
    import_commit_all(&mut core, id, 10);
    let defs = schema(&mut core, space);
    let stage = defs.iter().find(|d| d.name == "Stage").unwrap();
    assert_eq!(
        stage
            .options
            .iter()
            .map(|o| (o.name.as_str(), o.color, o.group))
            .collect::<Vec<_>>(),
        [
            ("Done", OptionColor::Pink, Some(StatusGroup::Done)),
            ("Blocked", OptionColor::Red, Some(StatusGroup::Todo)),
            (
                "In progress",
                OptionColor::Gray,
                Some(StatusGroup::InProgress)
            ),
        ],
        "the plan's order; a colour left out is the one after the previous option's"
    );
    let plain = defs.iter().find(|d| d.name == "Plain").unwrap();
    assert_eq!(
        plain
            .options
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["Not started", "In progress", "Done"]
    );
}

#[test]
fn the_space_limit_turns_the_rest_into_warnings_and_drops_their_values() {
    let (_fx, mut core, space) = setup();
    // Tags and 97 more make 98 of the 100 a space may hold.
    for n in 0..97 {
        prop_create(&mut core, space, &format!("P{n}"), PropertyKind::Text);
    }
    assert_eq!(schema(&mut core, space).len(), MAX_PROPERTIES - 2);
    let mut plan = plan_of(vec![planned_page(None, "Page", 1)]);
    plan.properties = (0..4)
        .map(|n| planned_property(&format!("N{n}"), PropertyKind::Text))
        .collect();
    plan.pages[0].properties = (0..4)
        .map(|n| (format!("N{n}"), text(&format!("value {n}"))))
        .collect();
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);
    assert_eq!(report.properties_created, 2);
    let limits: Vec<Option<&str>> = report
        .warnings
        .iter()
        .filter(|w| w.code == ImportWarningCode::PropertyLimit)
        .map(|w| w.detail.as_deref())
        .collect();
    assert_eq!(limits, [Some("N2"), Some("N3")]);
    assert_eq!(schema(&mut core, space).len(), MAX_PROPERTIES);
    let page = page_ids_of(&tree(&mut core, space))["Page"];
    assert_eq!(values_of(&mut core, page).len(), 2, "N0 and N1 only");
    assert_eq!(
        value_named(&mut core, space, page, "N1"),
        Some(PropertyValue::Text("value 1".into()))
    );
}

#[test]
fn a_two_way_relation_needs_room_for_both_sides() {
    let (_fx, mut core, space) = setup();
    for n in 0..98 {
        prop_create(&mut core, space, &format!("P{n}"), PropertyKind::Text);
    }
    assert_eq!(schema(&mut core, space).len(), MAX_PROPERTIES - 1);
    let mut plan = plan_of(vec![planned_page(None, "A", 1), planned_page(None, "B", 2)]);
    plan.properties = vec![
        PlannedProperty {
            config: Some(PlannedConfig::Relation {
                two_way: true,
                pair_name: None,
            }),
            ..planned_property("Links", PropertyKind::Relation)
        },
        planned_property("Fits", PropertyKind::Text),
    ];
    plan.pages[0].properties = vec![
        (
            "Links".into(),
            ImportValue::Pages(vec![PageTarget::Planned(1)]),
        ),
        ("Fits".into(), text("yes")),
    ];
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);
    // The pair did not fit, the text property after it did.
    assert_eq!(report.properties_created, 1);
    assert_eq!(
        report
            .warnings
            .iter()
            .map(|w| (w.code, w.detail.as_deref()))
            .collect::<Vec<_>>(),
        [(ImportWarningCode::PropertyLimit, Some("Links"))]
    );
    assert!(schema(&mut core, space).iter().all(|d| d.name != "Links"));
}

#[test]
fn a_rollup_whose_relation_is_unknown_is_left_out_with_a_warning() {
    let (_fx, mut core, space) = setup();
    let mut plan = plan_of(vec![planned_page(None, "A", 1)]);
    plan.properties = vec![PlannedProperty {
        config: Some(PlannedConfig::Rollup {
            relation: "No such relation".into(),
            target: PlannedRollupTarget::Title,
            function: RollupFunction::CountAll,
        }),
        ..planned_property("Orphan", PropertyKind::Rollup)
    }];
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);
    assert_eq!(report.properties_created, 0);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].detail.as_deref(), Some("Orphan"));
    assert!(schema(&mut core, space).iter().all(|d| d.name != "Orphan"));
}

#[test]
fn relations_by_title_resolve_only_unique_pages_of_the_space() {
    let (_fx, mut core, space) = setup();
    let unique = create_page(&mut core, space, None, "Unique");
    create_page(&mut core, space, None, "Twin");
    create_page(&mut core, space, None, "Twin");
    let trashed = create_page(&mut core, space, None, "Trashed");
    core.dispatch(Request::PageTrash {
        page_id: trashed.id,
    })
    .unwrap();
    let mut plan = plan_of(vec![planned_page(None, "Source", 1)]);
    plan.properties = vec![planned_property("See", PropertyKind::Relation)];
    plan.pages[0].properties = vec![(
        "See".into(),
        ImportValue::Pages(vec![
            PageTarget::Title("Unique".into()),
            PageTarget::Title("Twin".into()),
            PageTarget::Title("Trashed".into()),
        ]),
    )];
    let id = session_with(&mut core, space, None, plan);
    let (report, _) = import_commit_all(&mut core, id, 10);
    let source = page_ids_of(&tree(&mut core, space))["Source"];
    assert_eq!(
        value_named(&mut core, space, source, "See"),
        Some(PropertyValue::Relation(vec![unique.id]))
    );
    let unresolved: Vec<Option<&str>> = report
        .warnings
        .iter()
        .filter(|w| w.code == ImportWarningCode::UnresolvedLink)
        .map(|w| w.detail.as_deref())
        .collect();
    assert_eq!(unresolved, [Some("Twin"), Some("Trashed")]);
}

// ----- speed ------------------------------------------------------------------------------------

#[test]
#[allow(clippy::print_stderr)]
fn a_thousand_page_import_takes_seconds() {
    let (_fx, mut core, space) = setup();
    // One container, nine sections of 110 pages: 1,000 pages with three paragraphs and three
    // properties each.
    let mut pages = vec![planned_page(None, "Imported", 0)];
    for section in 0..9 {
        pages.push(planned_page(Some(0), &format!("Section {section}"), 1));
    }
    for section in 0..9 {
        for n in 0..110 {
            let at = pages.len();
            let mut page = planned_page(Some(1 + section), &format!("Page {section}-{n}"), 2);
            page.content = doc_of(
                (at % 200) as u8,
                &[
                    &format!("First paragraph of page {at}"),
                    "A second paragraph with a little more text in it, for the index.",
                    "And a third.",
                ],
            );
            page.properties = vec![
                (
                    "Stage".into(),
                    text(if n % 2 == 0 { "Open" } else { "Closed" }),
                ),
                ("Estimate".into(), ImportValue::Number(f64::from(n))),
                ("Done".into(), ImportValue::Bool(n % 3 == 0)),
            ];
            pages.push(page);
        }
    }
    assert_eq!(pages.len(), 1_000);
    let mut plan = plan_of(pages);
    plan.properties = vec![
        PlannedProperty {
            options: vec![option("Open"), option("Closed")],
            ..planned_property("Stage", PropertyKind::Select)
        },
        planned_property("Estimate", PropertyKind::Number),
        planned_property("Done", PropertyKind::Checkbox),
    ];

    let started = std::time::Instant::now();
    let id = session_with(&mut core, space, None, plan);
    let (report, calls) = import_commit_all(&mut core, id, 50);
    let elapsed = started.elapsed();
    eprintln!("imported 1,000 pages in {elapsed:?} ({calls} commit calls, debug build)");

    assert_eq!(report.pages, 1_000);
    assert_eq!(report.properties_created, 3);
    assert_eq!(calls, 20);
    assert!(
        elapsed < std::time::Duration::from_secs(180),
        "a thousand pages took {elapsed:?}"
    );
    let ids = page_ids_of(&tree(&mut core, space));
    assert_eq!(ids.len(), 1_000);
    assert_eq!(
        texts_of(&content_of(&mut core, ids["Page 8-109"]))[0],
        "First paragraph of page 999"
    );

    // And the same thousand pages out again.
    let started = std::time::Instant::now();
    let exported = export_all(&mut core, space, None, false, ExportFormat::Markdown, 500);
    let elapsed = started.elapsed();
    eprintln!(
        "exported 1,000 pages in {elapsed:?} ({} calls)",
        exported.calls
    );
    assert_eq!(
        exported.entries.len(),
        1_001,
        "a file per page and the sidecar"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(180),
        "a thousand pages took {elapsed:?} to export"
    );
}
