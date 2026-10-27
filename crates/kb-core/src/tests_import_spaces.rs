//! Import sessions into new spaces (ADR-0039, `docs/architecture/appflowy-import.md` sections 5 and 9):
//! `ImportBeginSpaces`, the spaces made at plan time, which space each page and file belongs to,
//! properties per space, the resolver across the session's spaces, remote file keys, the report,
//! and the spaces an aborted session deletes.
//!
//! The plans are built by hand (`Plan`, with the placeholders of `kb-import`'s `placeholder`
//! module), so these tests run whatever the state of the AppFlowy reader.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use kb_import::{ImportValue, PageTarget, Plan, PlannedOption, PlannedPage, PlannedProperty};
use kb_types::{
    AttachmentId, ErrorCode, Event, ImportPlanInfo, ImportReport, ImportWarningCode, PageId,
    PropertyKind, PropertyValue, Request, Response, SpaceId, SpaceInfo,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{outline, spaces, tree};
use crate::test_support_interop::{
    content_of, import_begin, import_commit, import_commit_all, schema, setup, upload, value_named,
};

// ----- plans built by hand --------------------------------------------------------------------

fn paragraph(text: &str) -> Value {
    json!({ "type": "paragraph", "content": [{ "type": "text", "text": text }] })
}

/// A paragraph that links to the planned page `index`.
fn link_to(index: usize) -> Value {
    json!({ "type": "paragraph", "content": [
        { "type": "text", "text": "see " },
        { "type": "pageRef", "attrs": { "$page": { "planned": index }, "$label": "label" } }
    ]})
}

/// An image block on a needed path or a remote key.
fn image(path: &str) -> Value {
    json!({ "type": "image", "attrs": { "$file": path, "alt": "picture" } })
}

fn page(parent: Option<usize>, title: &str, blocks: Vec<Value>) -> PlannedPage {
    PlannedPage {
        parent,
        title: title.into(),
        icon: None,
        cover: None,
        cover_y: None,
        template: false,
        template_placeholders: Vec::new(),
        source_path: format!("{title}.json"),
        content: json!({ "type": "doc", "content": blocks }),
        properties: Vec::new(),
    }
}

fn with_values(mut page: PlannedPage, values: Vec<(&str, ImportValue)>) -> PlannedPage {
    page.properties = values
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    page
}

fn text(value: &str) -> ImportValue {
    ImportValue::Text(value.into())
}

fn property(name: &str, kind: PropertyKind, options: &[&str]) -> PlannedProperty {
    PlannedProperty {
        name: name.into(),
        kind,
        options: options
            .iter()
            .map(|name| PlannedOption {
                name: (*name).into(),
                color: None,
                group: None,
            })
            .collect(),
        config: None,
    }
}

fn remote(n: usize, space_host: &str) -> kb_import::RemoteFile {
    kb_import::RemoteFile {
        key: format!("remote/{n}"),
        url: format!("https://{space_host}/images/{n}.png"),
        host: space_host.into(),
    }
}

/// Two AppFlowy-like spaces:
///
/// ```text
/// 0 Work (root, icon)          4 Home (root)
/// 1   Plan                     5   Recipes
/// 2     Sub                    6     Deep
/// 3   Board
/// ```
///
/// `Plan` links to `Recipes` and has two images; `Recipes` links back and has three; one file is
/// used by both. Values: `Status` only in Work, `Note` only in Home, `Both` in both; a relation
/// from `Plan` to a page of each space.
fn two_spaces() -> Plan {
    let mut work = page(None, "Work", vec![]);
    work.icon = Some("💼".into());
    let pages = vec![
        work,
        with_values(
            page(
                Some(0),
                "Plan",
                vec![
                    paragraph("plan"),
                    link_to(5),
                    image("local_files/a.png"),
                    image("local_files/shared.png"),
                    image("remote/1"),
                ],
            ),
            vec![
                ("Status", text("Todo")),
                ("Both", text("plan")),
                (
                    "Related",
                    ImportValue::Pages(vec![PageTarget::Planned(2), PageTarget::Planned(6)]),
                ),
            ],
        ),
        page(Some(1), "Sub", vec![paragraph("sub")]),
        with_values(
            page(Some(0), "Board", vec![paragraph("board")]),
            vec![("Status", text("Done"))],
        ),
        page(None, "Home", vec![]),
        page(
            Some(4),
            "Recipes",
            vec![
                link_to(1),
                image("local_files/b.png"),
                image("local_files/shared.png"),
                image("remote/0"),
            ],
        ),
        with_values(
            page(Some(5), "Deep", vec![paragraph("deep")]),
            vec![("Note", text("n")), ("Both", text("deep"))],
        ),
    ];
    Plan {
        pages,
        properties: vec![
            property("Status", PropertyKind::Select, &["Todo", "Done"]),
            property("Note", PropertyKind::Text, &[]),
            property("Both", PropertyKind::Text, &[]),
            property("Related", PropertyKind::Relation, &[]),
        ],
        needed_files: vec![
            "local_files/a.png".into(),
            "local_files/b.png".into(),
            "local_files/shared.png".into(),
        ],
        needed_file_names: vec![Some("Alpha.png".into()), None, Some("Shared.png".into())],
        warnings: Vec::new(),
        space_roots: vec![0, 4],
        remote_files: vec![
            remote(0, "cloud.example.com"),
            remote(1, "cloud.example.com"),
        ],
    }
}

// ----- drivers -----------------------------------------------------------------------------------

fn begin_spaces(core: &mut Core, source_name: Option<&str>) -> u32 {
    match core
        .dispatch(Request::ImportBeginSpaces {
            source_name: source_name.map(str::to_owned),
        })
        .unwrap()
    {
        Response::ImportStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    }
}

fn install(core: &mut Core, id: u32, plan: Plan) -> ImportPlanInfo {
    core.import_install_plan_for_test(id, plan).unwrap()
}

fn space_names(core: &mut Core) -> Vec<String> {
    spaces(core).into_iter().map(|space| space.name).collect()
}

fn space_named(core: &mut Core, name: &str) -> SpaceInfo {
    spaces(core)
        .into_iter()
        .find(|space| space.name == name)
        .unwrap_or_else(|| panic!("no space named {name}"))
}

fn titles(core: &mut Core, space_id: SpaceId) -> BTreeMap<String, PageId> {
    fn walk(nodes: &[kb_types::PageNode], out: &mut BTreeMap<String, PageId>) {
        for node in nodes {
            out.insert(node.page.title.clone(), node.page.id);
            walk(&node.children, out);
        }
    }
    let mut out = BTreeMap::new();
    walk(&tree(core, space_id).roots, &mut out);
    out
}

fn nodes_of<'a>(node: &'a Value, kind: &str, out: &mut Vec<&'a Value>) {
    match node {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some(kind) {
                out.push(node);
            }
            for value in map.values() {
                nodes_of(value, kind, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| nodes_of(item, kind, out)),
        _ => {}
    }
}

fn found<'a>(content: &'a Value, kind: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    nodes_of(content, kind, &mut out);
    out
}

/// Uploads every needed file into the space the plan names and reports it; every remote file but
/// `skip` the same. Answers the attachment of each path or key.
fn upload_all(
    core: &mut Core,
    id: u32,
    info: &ImportPlanInfo,
    skip: &[&str],
) -> BTreeMap<String, AttachmentId> {
    let mut made = BTreeMap::new();
    let wanted: Vec<(String, SpaceId)> = info
        .needed_files
        .iter()
        .cloned()
        .zip(info.file_spaces.iter().copied())
        .chain(
            info.remote_files
                .iter()
                .map(|remote| (remote.key.clone(), remote.space_id)),
        )
        .collect();
    for (path, space_id) in wanted {
        if skip.contains(&path.as_str()) {
            continue;
        }
        let name = path.rsplit('/').next().unwrap();
        let attachment = upload(core, space_id, name, "image/png", path.as_bytes());
        core.dispatch(Request::ImportAddAttachment {
            import_id: id,
            path: path.clone(),
            attachment_id: attachment.attachment_id,
        })
        .unwrap();
        made.insert(path, attachment.attachment_id);
    }
    made
}

fn has_warning(report: &ImportReport, code: ImportWarningCode, path: &str, detail: &str) -> bool {
    report.warnings.iter().any(|warning| {
        warning.code == code && warning.path == path && warning.detail.as_deref() == Some(detail)
    })
}

// ----- the plan creates the spaces ---------------------------------------------------------------

#[test]
fn the_plan_creates_one_space_per_root_and_says_where_everything_goes() {
    let (_fx, mut core, home) = setup();
    let id = begin_spaces(&mut core, Some("AppFlowy backup"));
    core.drain_events();

    let info = install(&mut core, id, two_spaces());

    // The spaces are new, in the root order, after the ones the account had; roots are not pages.
    let listed = spaces(&mut core);
    assert_eq!(
        listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Personal", "Work", "Home"]
    );
    assert_eq!(listed[0].id, home);
    assert_eq!(listed[1].icon.as_deref(), Some("💼"));
    assert_eq!(listed[2].icon, None);
    assert!(
        core.drain_events()
            .iter()
            .any(|event| matches!(event, Event::SpacesChanged))
    );
    let (work, recipes_space) = (listed[1].id, listed[2].id);
    assert_eq!(
        info.spaces
            .iter()
            .map(|space| (space.space_id, space.name.as_str(), space.pages))
            .collect::<Vec<_>>(),
        [(work, "Work", 3), (recipes_space, "Home", 2)]
    );
    assert_eq!(info.pages, 5, "the roots are not pages");

    // Files: the space of the first page that uses each; remote files with theirs.
    assert_eq!(
        info.needed_files,
        [
            "local_files/a.png",
            "local_files/b.png",
            "local_files/shared.png"
        ]
    );
    assert_eq!(info.file_spaces, [work, recipes_space, work]);
    // The source's own names, parallel to the files (the path's last segment where it gave none).
    assert_eq!(info.file_names, ["Alpha.png", "b.png", "Shared.png"]);
    let remote_spaces: Vec<(&str, SpaceId)> = info
        .remote_files
        .iter()
        .map(|remote| (remote.key.as_str(), remote.space_id))
        .collect();
    assert_eq!(
        remote_spaces,
        [("remote/0", recipes_space), ("remote/1", work)]
    );
    assert_eq!(info.remote_files[0].host, "cloud.example.com");
    assert_eq!(
        info.remote_files[0].url,
        "https://cloud.example.com/images/0.png"
    );

    // Nothing is a page yet.
    assert!(titles(&mut core, work).is_empty());
    assert!(titles(&mut core, recipes_space).is_empty());
}

#[test]
fn a_plan_without_roots_gets_one_space_named_after_the_source() {
    let (_fx, mut core, _home) = setup();
    let plan = Plan {
        pages: vec![
            page(None, "One", vec![paragraph("1")]),
            page(Some(0), "Child", vec![paragraph("2")]),
            page(None, "Two", vec![paragraph("3")]),
        ],
        ..Plan::default()
    };

    let id = begin_spaces(&mut core, Some("  notes export \n"));
    let info = install(&mut core, id, plan);
    assert_eq!(info.spaces.len(), 1);
    assert_eq!(
        (info.spaces[0].name.as_str(), info.spaces[0].pages),
        ("notes export", 3)
    );
    assert!(info.file_spaces.is_empty() && info.needed_files.is_empty());
    assert!(info.file_names.is_empty());
    let (report, _) = import_commit_all(&mut core, id, 10);
    let imported = info.spaces[0].space_id;
    assert_eq!(report.space_ids, [imported]);
    assert_eq!(outline(&tree(&mut core, imported)), "One\n  Child\nTwo\n");
    assert_eq!(report.root_page_ids.len(), 2);

    // No source name: "Imported".
    let plan = Plan {
        pages: vec![page(None, "Only", vec![])],
        ..Plan::default()
    };
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, plan);
    assert_eq!(info.spaces[0].name, "Imported");
}

#[test]
fn pages_outside_every_root_go_to_one_more_space_after_the_roots() {
    let (_fx, mut core, _home) = setup();
    let plan = Plan {
        pages: vec![
            page(None, "Space A", vec![]),
            page(Some(0), "In A", vec![]),
            page(None, "Loose", vec![]),
            page(Some(2), "Loose child", vec![]),
        ],
        space_roots: vec![0],
        ..Plan::default()
    };
    let id = begin_spaces(&mut core, Some("Source"));
    let info = install(&mut core, id, plan);
    let names: Vec<(&str, u32)> = info
        .spaces
        .iter()
        .map(|space| (space.name.as_str(), space.pages))
        .collect();
    assert_eq!(names, [("Space A", 1), ("Source", 2)]);
    import_commit_all(&mut core, id, 10);
    let loose = info.spaces[1].space_id;
    assert_eq!(outline(&tree(&mut core, loose)), "Loose\n  Loose child\n");
}

#[test]
fn a_plan_without_pages_makes_no_space() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, Some("Empty"));
    let info = install(&mut core, id, Plan::default());
    assert!(info.spaces.is_empty());
    assert_eq!(info.pages, 0);
    let (report, _) = import_commit_all(&mut core, id, 10);
    assert!(report.space_ids.is_empty() && report.root_page_ids.is_empty());
    assert_eq!(space_names(&mut core), ["Personal"]);
}

#[test]
fn space_names_and_icons_are_made_valid() {
    let (_fx, mut core, _home) = setup();
    let long = "x".repeat(150);
    let mut blank = page(None, "   ", vec![]);
    blank.icon = Some("icon:not a valid name!".into());
    let mut named = page(None, &long, vec![]);
    named.icon = Some("icon:rocket:blue".into());
    let plan = Plan {
        pages: vec![blank, named, page(None, "Tab\tbed\nname", vec![])],
        space_roots: vec![0, 1, 2],
        ..Plan::default()
    };
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, plan);
    let listed = spaces(&mut core);
    assert_eq!(listed[1].name, "Untitled");
    assert_eq!(
        listed[1].icon, None,
        "an icon outside the grammar is dropped"
    );
    assert_eq!(listed[2].name, "x".repeat(100));
    assert_eq!(listed[2].icon.as_deref(), Some("icon:rocket:blue"));
    assert_eq!(listed[3].name, "Tab bed name");
    assert_eq!(info.spaces.len(), 3);
    assert_eq!(info.pages, 0, "spaces made of roots alone hold no page");
}

#[test]
fn an_inconsistent_plan_makes_nothing_and_ends_the_session() {
    let (_fx, mut core, _home) = setup();
    for roots in [vec![9], vec![0, 0]] {
        let plan = Plan {
            pages: vec![page(None, "A", vec![]), page(None, "B", vec![])],
            space_roots: roots,
            ..Plan::default()
        };
        let id = begin_spaces(&mut core, None);
        let err = core.import_install_plan_for_test(id, plan).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(space_names(&mut core), ["Personal"]);
        let err = core
            .dispatch(Request::ImportCommit {
                import_id: id,
                max_pages: 10,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}

// ----- the commit ---------------------------------------------------------------------------------

#[test]
fn pages_are_created_in_their_own_space_and_roots_are_not_pages() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, Some("AppFlowy backup"));
    let info = install(&mut core, id, two_spaces());
    let (work, home) = (info.spaces[0].space_id, info.spaces[1].space_id);
    upload_all(&mut core, id, &info, &[]);

    // Two pages at a time: the first call creates every page and the first two documents.
    let progress = import_commit(&mut core, id, 2);
    assert_eq!(
        (progress.created, progress.total, progress.done),
        (2, 5, false)
    );
    assert_eq!(
        outline(&tree(&mut core, work)),
        "Plan\n  Sub\nBoard\n",
        "the children of a root are the root pages of its space"
    );
    assert_eq!(outline(&tree(&mut core, home)), "Recipes\n  Deep\n");
    let (report, _) = import_commit_all(&mut core, id, 2);

    assert_eq!(report.pages, 5);
    assert_eq!(report.space_ids, [work, home]);
    let work_pages = titles(&mut core, work);
    let home_pages = titles(&mut core, home);
    assert_eq!(
        report.root_page_ids,
        [
            work_pages["Plan"],
            work_pages["Board"],
            home_pages["Recipes"]
        ],
        "the root pages of every space, in order"
    );
    // Every page has its document, from the planned content.
    for (title, page_id) in work_pages.iter().chain(&home_pages) {
        let content = content_of(&mut core, *page_id);
        assert!(!content.to_string().is_empty(), "{title}");
    }
    let plan_texts =
        crate::test_support_interop::texts_of(&content_of(&mut core, work_pages["Plan"]));
    assert!(plan_texts.contains(&"plan".to_owned()));
    assert_eq!(space_names(&mut core), ["Personal", "Work", "Home"]);
}

#[test]
fn links_between_spaces_resolve_to_the_pages_of_the_other_space() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let (work, home) = (info.spaces[0].space_id, info.spaces[1].space_id);
    upload_all(&mut core, id, &info, &[]);
    let (report, _) = import_commit_all(&mut core, id, 50);

    let work_pages = titles(&mut core, work);
    let home_pages = titles(&mut core, home);
    let plan_content = content_of(&mut core, work_pages["Plan"]);
    let refs = found(&plan_content, "pageRef");
    assert_eq!(refs.len(), 1);
    assert_eq!(
        refs[0]["attrs"]["pageId"],
        json!(home_pages["Recipes"].to_string())
    );
    let recipes_content = content_of(&mut core, home_pages["Recipes"]);
    let refs = found(&recipes_content, "pageRef");
    assert_eq!(
        refs[0]["attrs"]["pageId"],
        json!(work_pages["Plan"].to_string())
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::UnresolvedLink
                && warning.detail.as_deref() == Some("label")),
        "a link across spaces is not a problem: {:?}",
        report.warnings
    );
    // The backlink index knows the link.
    let backlinks = match core
        .dispatch(Request::BacklinksGet {
            page_id: home_pages["Recipes"],
        })
        .unwrap()
    {
        Response::Backlinks(links) => links,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        backlinks.iter().map(|b| b.page_id).collect::<Vec<_>>(),
        [work_pages["Plan"]]
    );
}

#[test]
fn properties_are_created_per_space_for_the_pages_that_use_them() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let (work, home) = (info.spaces[0].space_id, info.spaces[1].space_id);
    upload_all(&mut core, id, &info, &[]);
    let (report, _) = import_commit_all(&mut core, id, 50);

    let names = |core: &mut Core, space_id: SpaceId| -> Vec<String> {
        let mut names: Vec<String> = schema(core, space_id)
            .into_iter()
            .map(|def| def.name)
            .filter(|name| name != "Tags")
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(&mut core, work), ["Both", "Related", "Status"]);
    assert_eq!(names(&mut core, home), ["Both", "Note"]);
    assert_eq!(report.properties_created, 5);

    // Values and options are the space's own.
    let work_pages = titles(&mut core, work);
    let home_pages = titles(&mut core, home);
    let status = value_named(&mut core, work, work_pages["Plan"], "Status").unwrap();
    let PropertyValue::Select(todo) = status else {
        panic!("a select value, got {status:?}");
    };
    let def = schema(&mut core, work)
        .into_iter()
        .find(|def| def.name == "Status")
        .unwrap();
    assert_eq!(
        def.options.iter().find(|o| o.id == todo).unwrap().name,
        "Todo"
    );
    assert_eq!(
        value_named(&mut core, work, work_pages["Board"], "Status")
            .map(|value| matches!(value, PropertyValue::Select(_))),
        Some(true)
    );
    assert_eq!(
        value_named(&mut core, home, home_pages["Deep"], "Note"),
        Some(PropertyValue::Text("n".into()))
    );
    assert_eq!(
        value_named(&mut core, home, home_pages["Deep"], "Both"),
        Some(PropertyValue::Text("deep".into()))
    );
    assert_eq!(
        value_named(&mut core, work, work_pages["Plan"], "Both"),
        Some(PropertyValue::Text("plan".into()))
    );
}

#[test]
fn a_relation_keeps_the_pages_of_its_own_space_only() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let work = info.spaces[0].space_id;
    upload_all(&mut core, id, &info, &[]);
    let (report, _) = import_commit_all(&mut core, id, 50);

    let work_pages = titles(&mut core, work);
    assert_eq!(
        value_named(&mut core, work, work_pages["Plan"], "Related"),
        Some(PropertyValue::Relation(vec![work_pages["Sub"]]))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::UnresolvedLink
                && warning.path == "Plan.json"),
        "the page of the other space is reported: {:?}",
        report.warnings
    );
}

// ----- files --------------------------------------------------------------------------------------

#[test]
fn files_go_to_the_space_of_their_first_page_and_are_used_from_every_space() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let (work, home) = (info.spaces[0].space_id, info.spaces[1].space_id);
    let made = upload_all(&mut core, id, &info, &[]);
    let (report, _) = import_commit_all(&mut core, id, 50);

    let work_pages = titles(&mut core, work);
    let home_pages = titles(&mut core, home);
    let image_ids = |content: &Value| -> Vec<String> {
        found(content, "image")
            .iter()
            .filter_map(|node| node["attrs"]["attachmentId"].as_str().map(str::to_owned))
            .collect()
    };
    let plan_content = content_of(&mut core, work_pages["Plan"]);
    assert_eq!(
        image_ids(&plan_content),
        [
            made["local_files/a.png"].to_string(),
            made["local_files/shared.png"].to_string(),
            made["remote/1"].to_string()
        ]
    );
    // `Recipes` (space Home) has its own files, the remote image of its space and the file that
    // was uploaded into Work (its first user's space), referenced across spaces.
    let recipes_content = content_of(&mut core, home_pages["Recipes"]);
    assert_eq!(
        image_ids(&recipes_content),
        [
            made["local_files/b.png"].to_string(),
            made["local_files/shared.png"].to_string(),
            made["remote/0"].to_string()
        ]
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::MissingFile),
        "every file resolves: {:?}",
        report.warnings
    );
    // Four attachments are used by pages: a, shared and remote/1 in Work, b and remote/0 in Home.
    assert_eq!(report.attachments, 5);
}

#[test]
fn a_remote_file_that_was_not_downloaded_is_not_an_attachment() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let (work, home) = (info.spaces[0].space_id, info.spaces[1].space_id);
    // The person said no: no remote file is reported.
    let made = upload_all(&mut core, id, &info, &["remote/0", "remote/1"]);
    assert_eq!(made.len(), 3);
    let (report, _) = import_commit_all(&mut core, id, 50);

    let work_pages = titles(&mut core, work);
    let home_pages = titles(&mut core, home);
    let plan_content = content_of(&mut core, work_pages["Plan"]);
    assert_eq!(found(&plan_content, "image").len(), 2, "a and shared");
    let recipes_content = content_of(&mut core, home_pages["Recipes"]);
    assert_eq!(found(&recipes_content, "image").len(), 2, "b and shared");
    assert!(has_warning(
        &report,
        ImportWarningCode::MissingFile,
        "Plan.json",
        "remote/1"
    ));
    assert!(has_warning(
        &report,
        ImportWarningCode::MissingFile,
        "Recipes.json",
        "remote/0"
    ));
}

#[test]
fn an_attachment_must_be_in_the_space_the_plan_gave_the_file() {
    let (_fx, mut core, home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let (work, recipes_space) = (info.spaces[0].space_id, info.spaces[1].space_id);
    let add = |core: &mut Core, path: &str, attachment: AttachmentId| {
        core.dispatch(Request::ImportAddAttachment {
            import_id: id,
            path: path.into(),
            attachment_id: attachment,
        })
    };

    // `a.png` belongs to Work: a copy in Home (the account's first space), or in the other new
    // space, is refused.
    let wrong = upload(&mut core, home, "a.png", "image/png", b"a");
    let err = add(&mut core, "local_files/a.png", wrong.attachment_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(
        err.details,
        Some(json!({ "field": "attachmentId" })),
        "{err:?}"
    );
    let elsewhere = upload(&mut core, recipes_space, "a.png", "image/png", b"a");
    assert_eq!(
        add(&mut core, "local_files/a.png", elsewhere.attachment_id)
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    let right = upload(&mut core, work, "a.png", "image/png", b"a");
    add(&mut core, "local_files/a.png", right.attachment_id).unwrap();

    // A remote key follows the same rule, and a key the plan did not list is not accepted.
    let key_wrong = upload(&mut core, work, "0.png", "image/png", b"0");
    assert_eq!(
        add(&mut core, "remote/0", key_wrong.attachment_id)
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    let unlisted = upload(&mut core, work, "9.png", "image/png", b"9");
    let err = add(&mut core, "remote/9", unlisted.attachment_id).unwrap_err();
    assert_eq!(err.details, Some(json!({ "field": "path" })));
    let key_right = upload(&mut core, recipes_space, "0.png", "image/png", b"0");
    add(&mut core, "remote/0", key_right.attachment_id).unwrap();
}

// ----- an import into a page keeps the roots as pages ---------------------------------------------

#[test]
fn the_same_plan_into_a_page_makes_container_pages_and_lists_remote_files_with_the_space() {
    let (_fx, mut core, space) = setup();
    let id = import_begin(&mut core, space, None, Some("AppFlowy backup"));
    let info = core.import_install_plan_for_test(id, two_spaces()).unwrap();

    assert!(info.spaces.is_empty() && info.file_spaces.is_empty());
    assert_eq!(info.pages, 7, "the roots are pages here");
    assert_eq!(
        info.remote_files
            .iter()
            .map(|remote| (remote.key.as_str(), remote.space_id))
            .collect::<Vec<_>>(),
        [("remote/0", space), ("remote/1", space)]
    );
    let made = {
        let mut made = BTreeMap::new();
        for path in info
            .needed_files
            .iter()
            .cloned()
            .chain(info.remote_files.iter().map(|remote| remote.key.clone()))
        {
            let attachment = upload(&mut core, space, "f.png", "image/png", path.as_bytes());
            core.dispatch(Request::ImportAddAttachment {
                import_id: id,
                path: path.clone(),
                attachment_id: attachment.attachment_id,
            })
            .unwrap();
            made.insert(path, attachment.attachment_id);
        }
        made
    };
    let (report, _) = import_commit_all(&mut core, id, 50);

    assert!(report.space_ids.is_empty());
    assert_eq!(report.pages, 7);
    assert_eq!(
        outline(&tree(&mut core, space)),
        "Work\n  Plan\n    Sub\n  Board\nHome\n  Recipes\n    Deep\n"
    );
    assert_eq!(report.root_page_ids.len(), 2);
    assert_eq!(space_names(&mut core), ["Personal"], "no space was made");
    // The remote image resolves through its key like a needed file.
    let pages = titles(&mut core, space);
    let recipes = content_of(&mut core, pages["Recipes"]);
    let images: Vec<String> = found(&recipes, "image")
        .iter()
        .filter_map(|node| node["attrs"]["attachmentId"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        images,
        [
            made["local_files/b.png"].to_string(),
            made["local_files/shared.png"].to_string(),
            made["remote/0"].to_string()
        ]
    );
}

// ----- abort and failure ----------------------------------------------------------------------------

#[test]
fn aborting_before_the_commit_deletes_the_spaces_and_their_attachments() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    let work = info.spaces[0].space_id;
    assert_eq!(space_names(&mut core).len(), 3);
    let made = upload_all(&mut core, id, &info, &[]);
    let one = *made.values().next().unwrap();
    core.drain_events();

    assert_eq!(
        core.dispatch(Request::ImportAbort { import_id: id })
            .unwrap(),
        Response::Ok
    );

    assert_eq!(space_names(&mut core), ["Personal"]);
    assert!(
        core.drain_events()
            .iter()
            .any(|event| matches!(event, Event::SpacesChanged))
    );
    // The space and what it held are gone.
    assert_eq!(
        core.dispatch(Request::TreeGet { space_id: work })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(
        core.dispatch(Request::AttachmentGet { attachment_id: one })
            .is_err(),
        "the attachment of a deleted space is gone"
    );
    // The session is gone too.
    assert_eq!(
        core.dispatch(Request::ImportCommit {
            import_id: id,
            max_pages: 10
        })
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn aborting_after_pages_were_written_keeps_the_spaces_that_hold_one() {
    let (_fx, mut core, _home) = setup();
    // Three spaces, the second of which stays empty.
    let plan = Plan {
        pages: vec![
            page(None, "Full", vec![]),
            page(Some(0), "Page", vec![paragraph("kept")]),
            page(None, "Empty", vec![]),
            page(None, "Trashed", vec![]),
            page(Some(3), "Gone", vec![]),
        ],
        space_roots: vec![0, 2, 3],
        ..Plan::default()
    };
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, plan);
    let (full, empty, trashed) = (
        info.spaces[0].space_id,
        info.spaces[1].space_id,
        info.spaces[2].space_id,
    );
    let progress = import_commit(&mut core, id, 1);
    assert!(!progress.done);
    // A page of the third space is in the trash: the space still holds a page.
    let gone = titles(&mut core, trashed)["Gone"];
    core.dispatch(Request::PageTrash { page_id: gone }).unwrap();

    core.dispatch(Request::ImportAbort { import_id: id })
        .unwrap();

    assert_eq!(space_names(&mut core), ["Personal", "Full", "Trashed"]);
    assert!(titles(&mut core, full).contains_key("Page"));
    assert_eq!(
        core.dispatch(Request::TreeGet { space_id: empty })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn a_new_session_replaces_the_old_one_and_deletes_its_empty_spaces() {
    let (_fx, mut core, space) = setup();
    let first = begin_spaces(&mut core, None);
    install(&mut core, first, two_spaces());
    assert_eq!(space_names(&mut core).len(), 3);

    // `ImportBegin` and `ImportBeginSpaces` both replace the running session.
    let second = import_begin(&mut core, space, None, None);
    assert_eq!(space_names(&mut core), ["Personal"]);
    assert_eq!(second, first + 1);

    let third = begin_spaces(&mut core, None);
    install(&mut core, third, two_spaces());
    assert_eq!(space_names(&mut core).len(), 3);
    let fourth = begin_spaces(&mut core, None);
    assert_eq!(space_names(&mut core), ["Personal"]);
    assert_eq!(fourth, third + 1);
}

#[test]
fn a_finished_import_leaves_its_spaces_alone() {
    let (_fx, mut core, _home) = setup();
    let id = begin_spaces(&mut core, None);
    let info = install(&mut core, id, two_spaces());
    upload_all(&mut core, id, &info, &[]);
    let (report, _) = import_commit_all(&mut core, id, 50);
    assert_eq!(report.space_ids.len(), 2);
    // The session is over: aborting it now changes nothing.
    core.dispatch(Request::ImportAbort { import_id: id })
        .unwrap();
    assert_eq!(space_names(&mut core), ["Personal", "Work", "Home"]);
    let work = space_named(&mut core, "Work");
    assert_eq!(work.page_count, 3);
}

// ----- end to end with a synthetic AppFlowy archive -------------------------------------------------

/// An AppFlowy document of top-level blocks: `(type, data, delta)`.
fn af_doc(blocks: &[(&str, Value, Option<Value>)]) -> String {
    let mut map = serde_json::Map::new();
    let mut children = serde_json::Map::new();
    let mut text = serde_json::Map::new();
    let mut ids: Vec<Value> = Vec::new();
    map.insert(
        "root".into(),
        json!({"id": "root", "ty": "page", "parent": "", "children": "ch-root",
               "external_id": null, "external_type": null, "data": {}}),
    );
    for (at, (ty, data, delta)) in blocks.iter().enumerate() {
        let id = format!("b{}", at + 1);
        let (external_id, external_type) = match delta {
            Some(delta) => {
                text.insert(format!("tx-{id}"), Value::String(delta.to_string()));
                (json!(format!("tx-{id}")), json!("text"))
            }
            None => (Value::Null, Value::Null),
        };
        map.insert(
            id.clone(),
            json!({"id": id, "ty": ty, "parent": "root", "children": format!("ch-{id}"),
                   "external_id": external_id, "external_type": external_type, "data": data}),
        );
        children.insert(format!("ch-{id}"), json!([]));
        ids.push(json!(id));
    }
    children.insert("ch-root".into(), Value::Array(ids));
    json!({"page_id": "root", "blocks": map,
           "meta": {"children_map": children, "text_map": text}})
    .to_string()
}

fn af_view(id: &str, name: &str, parent: &str, children: &[&str], space: bool, at: i64) -> Value {
    let extra = space.then(|| json!({"is_space": true, "space_created_at": at * 1000}).to_string());
    json!({"view_id": id, "name": name, "layout": 0, "parent_id": parent, "children": children,
           "collab_object_id": id, "created_at": at, "updated_at": at, "extra": extra, "icon": null})
}

/// The text files of a small backup: two spaces, three pages, a local file used in both spaces, a
/// remote image used in both and another used in one.
fn af_backup() -> Vec<(String, String)> {
    let mention = |page: &str| {
        json!([{"insert": "see "},
               {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": page}}}])
    };
    let views = json!({
        "sp-work": af_view("sp-work", "Work", "ws", &["p-plan"], true, 1_700_000_100),
        "p-plan": af_view("p-plan", "Plan", "sp-work", &["p-sub"], false, 1_700_000_110),
        "p-sub": af_view("p-sub", "Sub", "p-plan", &[], false, 1_700_000_120),
        "sp-home": af_view("sp-home", "Home", "ws", &["p-recipes"], true, 1_700_000_200),
        "p-recipes": af_view("p-recipes", "Recipes", "sp-home", &[], false, 1_700_000_210),
    });
    let plan_doc = af_doc(&[
        ("paragraph", json!({}), Some(mention("p-recipes"))),
        (
            "image",
            json!({"url": "https://cloud.example.com/img/one.png", "align": "center"}),
            None,
        ),
        (
            "file",
            json!({"url": "p-plan/hash1.pdf", "url_type": 2, "name": "Report.pdf"}),
            None,
        ),
    ]);
    let sub_doc = af_doc(&[(
        "paragraph",
        json!({}),
        Some(json!([{"insert": "sub text"}])),
    )]);
    let recipes_doc = af_doc(&[
        ("paragraph", json!({}), Some(mention("p-plan"))),
        (
            "image",
            json!({"url": "https://cloud.example.com/img/one.png"}),
            None,
        ),
        (
            "image",
            json!({"url": "https://cloud.example.com/img/two.png"}),
            None,
        ),
        (
            "file",
            json!({"url": "p-recipes/hash1.pdf", "url_type": 2, "name": "Report.pdf"}),
            None,
        ),
    ]);
    let metadata = json!({"export_info": {"workspace_id": "ws", "version": "1.0"}});
    let relation = json!({"workspace_id": "ws", "views": views, "collab_objects": {},
                          "dependencies": [], "workspace_database_meta": []});
    vec![
        ("metadata.json".into(), metadata.to_string()),
        ("relation_map.json".into(), relation.to_string()),
        ("collab_jsons/documents/p-plan.json".into(), plan_doc),
        ("collab_jsons/documents/p-sub.json".into(), sub_doc),
        ("collab_jsons/documents/p-recipes.json".into(), recipes_doc),
    ]
}

#[test]
fn an_appflowy_backup_goes_into_two_new_spaces_end_to_end() {
    let (_fx, mut core, _first) = setup();
    let id = begin_spaces(&mut core, Some("AppFlowy backup"));
    for (path, text) in af_backup() {
        core.dispatch(Request::ImportAddFile {
            import_id: id,
            path,
            text,
        })
        .unwrap();
    }
    let info = match core
        .dispatch(Request::ImportPlan {
            import_id: id,
            binary_paths: vec!["local_files/hash1.pdf".into()],
        })
        .unwrap()
    {
        Response::ImportPlanned(info) => info,
        other => panic!("unexpected {other:?}"),
    };

    // Two spaces named after the AppFlowy ones, with their pages; the roots are not pages.
    assert_eq!(
        info.spaces
            .iter()
            .map(|space| (space.name.as_str(), space.pages))
            .collect::<Vec<_>>(),
        [("Work", 2), ("Home", 1)]
    );
    assert_eq!(info.pages, 3);
    let (work, home) = (info.spaces[0].space_id, info.spaces[1].space_id);
    // The file is uploaded into the space of the first page that uses it, under its own name.
    assert_eq!(info.needed_files, ["local_files/hash1.pdf"]);
    assert_eq!(info.file_spaces, [work]);
    assert_eq!(info.file_names, ["Report.pdf"]);
    // One remote file per distinct URL, in the space of the first page that uses it.
    assert_eq!(
        info.remote_files
            .iter()
            .map(|remote| (remote.key.as_str(), remote.host.as_str(), remote.space_id))
            .collect::<Vec<_>>(),
        [
            ("remote/0", "cloud.example.com", work),
            ("remote/1", "cloud.example.com", home)
        ]
    );

    // The shell: the file as the plan says, and the first remote image only (the second
    // download failed).
    let pdf = upload(&mut core, work, "Report.pdf", "application/pdf", b"%PDF");
    let one = upload(&mut core, work, "one.png", "image/png", b"one");
    for (path, attachment) in [("local_files/hash1.pdf", &pdf), ("remote/0", &one)] {
        core.dispatch(Request::ImportAddAttachment {
            import_id: id,
            path: path.into(),
            attachment_id: attachment.attachment_id,
        })
        .unwrap();
    }
    let (report, _) = import_commit_all(&mut core, id, 2);

    assert_eq!(report.space_ids, [work, home]);
    assert_eq!(report.pages, 3);
    let work_pages = titles(&mut core, work);
    let home_pages = titles(&mut core, home);
    assert_eq!(
        work_pages.keys().cloned().collect::<Vec<_>>(),
        ["Plan", "Sub"]
    );
    assert_eq!(home_pages.keys().cloned().collect::<Vec<_>>(), ["Recipes"]);
    assert_eq!(
        report.root_page_ids,
        [work_pages["Plan"], home_pages["Recipes"]]
    );

    // Mentions across the two spaces became links to the other space's page.
    let plan_content = content_of(&mut core, work_pages["Plan"]);
    let refs = found(&plan_content, "pageRef");
    assert_eq!(
        refs.iter()
            .map(|node| node["attrs"]["pageId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [home_pages["Recipes"].to_string()]
    );
    let recipes_content = content_of(&mut core, home_pages["Recipes"]);
    assert_eq!(
        found(&recipes_content, "pageRef")
            .iter()
            .map(|node| node["attrs"]["pageId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [work_pages["Plan"].to_string()]
    );

    // `Plan` (Work) has the image and the file; `Recipes` (Home) uses both across spaces (they
    // went into Work, the space of their first page), and its own image was not downloaded: a
    // link.
    let image_ids = |content: &Value| -> Vec<String> {
        found(content, "image")
            .iter()
            .filter_map(|node| node["attrs"]["attachmentId"].as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(image_ids(&plan_content), [one.attachment_id.to_string()]);
    assert_eq!(found(&plan_content, "file").len(), 1);
    assert_eq!(image_ids(&recipes_content), [one.attachment_id.to_string()]);
    assert_eq!(found(&recipes_content, "file").len(), 1);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::RemoteImage),
        "an image that was not downloaded stays a link: {:?}",
        report.warnings
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::MissingFile),
        "a file of the other space is used across spaces: {:?}",
        report.warnings
    );
    assert_eq!(report.attachments, 2);
}
