//! Import and export through the real converters (ADR-0036, `docs/architecture/import-export.md`,
//! section 8): Markdown files imported end to end, and the fidelity round trip: a space exported as
//! a Markdown archive and imported into another workspace gives back the same pages, properties and
//! portable JSON up to ids and the documented losses (colours of text, block ids, `[[Title]]`
//! links outside the export).
//!
//! These tests need `kb-export` and `kb-import`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{
    AttachmentId, BlockId, ChangeSource, ChecklistItem, ChecklistItemId, DateValue, Event,
    ExportFormat, OptionColor, OptionId, PageId, PlaceValue, PropertyConfig, PropertyDef,
    PropertyId, PropertyKind, PropertyValue, Request, Response, RollupFunction, RollupTarget,
    SpaceId, StatusGroup,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{create_page, outline, page, tree};
use crate::test_support_interop::{
    another_core, as_source, content_of, doc_of, export_all, info_of, json_differences, normalised,
    option_add, prop_create, prop_create_with, property_named, run_import, schema, set_value,
    setup, upload, value_named, values_of,
};

fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
        .collect()
}

fn page_ids_of(core: &mut Core, space: SpaceId) -> BTreeMap<String, PageId> {
    fn walk(nodes: &[kb_types::PageNode], out: &mut BTreeMap<String, PageId>) {
        for node in nodes {
            out.insert(node.page.title.clone(), node.page.id);
            walk(&node.children, out);
        }
    }
    let mut out = BTreeMap::new();
    walk(&tree(core, space).roots, &mut out);
    out
}

// ----- end to end ------------------------------------------------------------------------------

#[test]
fn markdown_files_become_pages_with_properties_links_and_images() {
    let (_fx, mut core, space) = setup();
    let plan_md = "---\n\
title: Plan\n\
icon: \"📝\"\n\
tags: [work, alpha]\n\
Status: Done\n\
Estimate: 3\n\
Due: 2026-10-07\n\
---\n\
# Plan\n\
\n\
See [[Ideas]] and the picture:\n\
\n\
![diagram](img/d.png \"A caption\")\n";
    core.drain_events();
    let imported = run_import(
        &mut core,
        space,
        None,
        None,
        &files(&[
            ("notes/Plan.md", plan_md),
            ("notes/Ideas.md", "# Ideas\n\nSome text\n"),
        ]),
        &[("notes/img/d.png".into(), b"PNGDATA".to_vec())],
        50,
    );
    let events = core.drain_events();
    assert_eq!(imported.plan.pages, 2);
    assert_eq!(imported.plan.needed_files, ["notes/img/d.png"]);
    assert_eq!(imported.report.pages, 2);
    assert_eq!(imported.report.attachments, 1);
    assert_eq!(
        imported.report.properties_created, 3,
        "Status, Estimate and Due; Tags exists"
    );
    let view = tree(&mut core, space);
    assert_eq!(outline(&view), "Ideas\nPlan\n");
    let ids = page_ids_of(&mut core, space);
    assert_eq!(page(&mut core, ids["Plan"]).icon.as_deref(), Some("📝"));
    assert_eq!(
        value_named(&mut core, space, ids["Plan"], "Estimate"),
        Some(PropertyValue::Number(3.0))
    );
    let tags = property_named(&mut core, space, "Tags");
    assert_eq!(
        tags.options
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["work", "alpha"]
    );
    let content = content_of(&mut core, ids["Plan"]).to_string();
    assert!(
        content.contains(&ids["Ideas"].to_string()),
        "the wiki link became a reference to the new page: {content}"
    );
    assert!(content.contains("\"image\""), "{content}");
    assert!(content.contains("A caption"), "{content}");
    // The usual events: the tree, the schema, each page's content, the link's target, the file.
    assert!(events.contains(&Event::StructureChanged { space_id: space }));
    assert!(events.contains(&Event::PropertySchemaChanged { space_id: space }));
    assert!(events.contains(&Event::AttachmentsChanged { space_id: space }));
    assert!(events.contains(&Event::BacklinksChanged {
        page_id: ids["Ideas"]
    }));
    for title in ["Plan", "Ideas"] {
        assert!(
            events.contains(&Event::PageContentChanged {
                page_id: ids[title],
                source: ChangeSource::Core
            }),
            "{title}"
        );
    }
    assert!(events.contains(&Event::ImportProgress {
        import_id: imported.import_id,
        done: 2,
        total: 2
    }));
    // The front matter gave the title, so the heading stays in the page.
    assert!(content.contains("\"heading\""), "{content}");
}

// ----- the fidelity round trip -----------------------------------------------------------------

/// The values of a page by property name, with option ids, pages and files written as names.
fn described_values(core: &mut Core, space: SpaceId, page: PageId) -> BTreeMap<String, Value> {
    let defs = schema(core, space);
    let titles: BTreeMap<PageId, String> = page_ids_of(core, space)
        .into_iter()
        .map(|(title, id)| (id, title))
        .collect();
    let mut out = BTreeMap::new();
    for (property, value) in values_of(core, page) {
        let def = defs.iter().find(|d| d.id == property).unwrap();
        let option = |id: OptionId| {
            def.options
                .iter()
                .find(|o| o.id == id)
                .map(|o| o.name.clone())
                .unwrap()
        };
        let described = match value {
            PropertyValue::Select(o) | PropertyValue::Status(o) => json!(option(o)),
            PropertyValue::MultiSelect(ids) => {
                json!(ids.into_iter().map(option).collect::<Vec<_>>())
            }
            PropertyValue::Relation(pages) => json!(
                pages
                    .into_iter()
                    .map(|p| titles.get(&p).cloned().unwrap_or_default())
                    .collect::<Vec<_>>()
            ),
            PropertyValue::Files(ids) => json!(
                ids.into_iter()
                    .map(|a| info_of(core, a).name)
                    .collect::<Vec<_>>()
            ),
            PropertyValue::Checklist(items) => json!(
                items
                    .into_iter()
                    .map(|i| (i.done, i.text))
                    .collect::<Vec<_>>()
            ),
            // Computed values come from the pages and the schema, not from the files.
            PropertyValue::CreatedTime(_)
            | PropertyValue::LastEditedTime(_)
            | PropertyValue::CreatedBy(_)
            | PropertyValue::LastEditedBy(_) => continue,
            other => serde_json::to_value(other).unwrap(),
        };
        out.insert(def.name.clone(), described);
    }
    out
}

/// The schema by names: kinds, options, relations and rollups, with ids left out.
fn described_schema(core: &mut Core, space: SpaceId) -> Vec<Value> {
    let defs = schema(core, space);
    let name_of = |id: PropertyId| defs.iter().find(|d| d.id == id).unwrap().name.clone();
    defs.iter()
        .map(|def| {
            let config = match &def.config {
                Some(PropertyConfig::Relation {
                    two_way,
                    paired_property_id,
                }) => json!({ "twoWay": two_way, "pair": paired_property_id.map(name_of) }),
                Some(PropertyConfig::Rollup {
                    relation_property_id,
                    target,
                    function,
                }) => json!({
                    "relation": name_of(*relation_property_id),
                    "target": match target {
                        RollupTarget::Title => "title".to_owned(),
                        RollupTarget::Property(id) => name_of(*id),
                    },
                    "function": function.as_str()
                }),
                Some(PropertyConfig::Formula { expression, .. }) => {
                    json!({ "expression": expression })
                }
                None => Value::Null,
            };
            let options: Vec<Value> = def
                .options
                .iter()
                .map(|o| json!([o.name, o.color.as_str(), o.group.map(StatusGroup::as_str)]))
                .collect();
            json!({
                "name": def.name,
                "kind": def.kind.as_str(),
                "options": options,
                "config": config
            })
        })
        .collect()
}

/// The golden sample with the ids it cannot keep in another space replaced by real ones.
fn golden_for(
    attachments: &[AttachmentId; 4],
    page: PageId,
    tag_option: OptionId,
    property: PropertyId,
) -> Value {
    let mut text = include_str!("../../../fixtures/doc/page-v1.portable.json").to_owned();
    for (from, to) in [
        (
            "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5",
            attachments[0].to_string(),
        ),
        (
            "6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6",
            attachments[1].to_string(),
        ),
        (
            "7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7",
            attachments[2].to_string(),
        ),
        (
            "8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708",
            attachments[3].to_string(),
        ),
        ("7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6", page.to_string()),
        (
            "3c9d1e52-6a47-4b8e-9f20-5d1c7a8b4e61",
            tag_option.to_string(),
        ),
        ("5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74", property.to_string()),
    ] {
        assert!(
            text.contains(from),
            "the golden sample no longer holds {from}"
        );
        text = text.replace(from, &to);
    }
    serde_json::from_str(&text).unwrap()
}

fn upload_four(core: &mut Core, space: SpaceId) -> [AttachmentId; 4] {
    let list: Vec<AttachmentId> = [
        ("diagram.png", "image/png", &b"PNG one"[..]),
        ("notes.pdf", "application/pdf", &b"%PDF two"[..]),
        ("photo.png", "image/png", &b"PNG three"[..]),
        ("stage.png", "image/png", &b"PNG four"[..]),
    ]
    .iter()
    .map(|(name, mime, bytes)| upload(core, space, name, mime, bytes).attachment_id)
    .collect();
    list.try_into().unwrap()
}

fn prop_two_way(core: &mut Core, space: SpaceId, name: &str) -> PropertyDef {
    prop_create_with(
        core,
        space,
        name,
        PropertyKind::Relation,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: None,
        }),
    )
}

#[test]
fn a_space_survives_a_round_trip_through_a_markdown_archive() {
    let (_fx, mut source, space) = setup();

    // The schema: every kind that crosses, a two-way relation, a rollup, a view's properties.
    let status = prop_create(&mut source, space, "Status", PropertyKind::Status);
    let done = status.options.iter().find(|o| o.name == "Done").unwrap().id;
    let due = prop_create(&mut source, space, "Due", PropertyKind::Date);
    // Only a view names it (the calendar's end date): the import plans it from the view.
    let end = prop_create(&mut source, space, "End", PropertyKind::Date);
    let shipped = prop_create(&mut source, space, "Shipped", PropertyKind::Checkbox);
    let estimate = prop_create(&mut source, space, "Estimate", PropertyKind::Number);
    let link = prop_create(&mut source, space, "Link", PropertyKind::Url);
    let todo = prop_create(&mut source, space, "Todo", PropertyKind::Checklist);
    let place = prop_create(&mut source, space, "Place", PropertyKind::Place);
    let docs = prop_create(&mut source, space, "Docs", PropertyKind::Files);
    let related = prop_two_way(&mut source, space, "Related");
    prop_create_with(
        &mut source,
        space,
        "Total",
        PropertyKind::Rollup,
        Some(PropertyConfig::Rollup {
            relation_property_id: related.id,
            target: RollupTarget::Property(estimate.id),
            function: RollupFunction::Sum,
        }),
    );
    let work = option_add(
        &mut source,
        space,
        TAGS_PROPERTY_ID,
        "work",
        OptionColor::Blue,
    );
    option_add(
        &mut source,
        space,
        TAGS_PROPERTY_ID,
        "review",
        OptionColor::Pink,
    );

    // The tree: Projects with Alpha, Beta, the golden Sample and a database; a root Notes.
    let projects = create_page(&mut source, space, None, "Projects");
    let alpha = create_page(&mut source, space, Some(projects.id), "Alpha");
    let beta = create_page(&mut source, space, Some(projects.id), "Beta");
    let sample = create_page(&mut source, space, Some(projects.id), "Sample");
    let database = create_page(&mut source, space, Some(projects.id), "Tasks");
    let notes = create_page(&mut source, space, None, "Notes");
    source
        .dispatch(Request::PageSetIcon {
            page_id: alpha.id,
            icon: Some("📝".into()),
        })
        .unwrap();

    let attachments = upload_four(&mut source, space);
    source
        .replace_page_document_for_test(
            sample.id,
            &golden_for(&attachments, alpha.id, work.id, estimate.id),
        )
        .unwrap();
    source
        .replace_page_document_for_test(
            alpha.id,
            &doc_of(
                10,
                &["Alpha has special characters: [x] a_b <tag> 1. not a list | pipe"],
            ),
        )
        .unwrap();
    source
        .replace_page_document_for_test(notes.id, &doc_of(20, &["Notes body", "Second paragraph"]))
        .unwrap();
    let view_block = BlockId::from_random_bytes([77; 16]).to_string();
    source
        .replace_page_document_for_test(
            database.id,
            &json!({
                "type": "doc",
                "content": [{
                    "type": "pageList",
                    "attrs": {
                        "id": view_block,
                        "source": { "kind": "children" },
                        "layout": "table",
                        "properties": [status.id.to_string(), due.id.to_string()],
                        "sort": { "by": due.id.to_string(), "dir": "asc" },
                        "filters": [{
                            "propertyId": status.id.to_string(),
                            "op": "hasOption",
                            "value": done.to_string()
                        }],
                        "columnWidths": { "title": 300 },
                        "groupBy": status.id.to_string(),
                        "hiddenGroups": ["none"],
                        // ADR-0054: four views; the attributes above are the mirror of the first one.
                        "views": [
                            { "id": "k3f9a2bc", "name": "", "layout": "table",
                              "properties": [status.id.to_string(), due.id.to_string()],
                              "sort": { "by": due.id.to_string(), "dir": "asc" },
                              "filter": { "op": "and", "items": [
                                  { "propertyId": status.id.to_string(), "op": "hasOption", "value": done.to_string() },
                                  { "op": "or", "items": [
                                      { "propertyId": due.id.to_string(), "op": "isSet" },
                                      { "propertyId": end.id.to_string(), "op": "isSet" } ] } ] },
                              "columnWidths": { "title": 300 },
                              "groupBy": status.id.to_string(), "hiddenGroups": ["none"] },
                            { "id": "b81d00e4", "name": "By status", "layout": "board",
                              "properties": [due.id.to_string()],
                              "sort": { "by": "title", "dir": "asc" },
                              "groupBy": status.id.to_string(),
                              "hiddenGroups": [done.to_string(), "none"], "openIn": "center" },
                            { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar",
                              "properties": [status.id.to_string()],
                              "sort": { "by": "updated", "dir": "asc" },
                              "dateBy": due.id.to_string(), "endBy": end.id.to_string() },
                            { "id": "e2a9b6f0", "name": "", "layout": "gallery", "properties": [],
                              "sort": { "by": "created", "dir": "desc" },
                              "dateBy": "created", "cardSize": "l" }
                        ]
                    }
                }]
            }),
        )
        .unwrap();

    // The values.
    set_value(
        &mut source,
        alpha.id,
        status.id,
        PropertyValue::Status(done),
    );
    set_value(
        &mut source,
        alpha.id,
        due.id,
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
        }),
    );
    set_value(
        &mut source,
        alpha.id,
        shipped.id,
        PropertyValue::Checkbox(true),
    );
    set_value(
        &mut source,
        alpha.id,
        estimate.id,
        PropertyValue::Number(3.5),
    );
    set_value(
        &mut source,
        beta.id,
        estimate.id,
        PropertyValue::Number(2.0),
    );
    set_value(
        &mut source,
        alpha.id,
        link.id,
        PropertyValue::Url("https://example.com/a?b=1".into()),
    );
    set_value(
        &mut source,
        alpha.id,
        todo.id,
        PropertyValue::Checklist(vec![
            ChecklistItem {
                id: ChecklistItemId::from_random_bytes([1; 16]),
                text: "buy milk".into(),
                done: true,
            },
            ChecklistItem {
                id: ChecklistItemId::from_random_bytes([2; 16]),
                text: "call Ann".into(),
                done: false,
            },
        ]),
    );
    set_value(
        &mut source,
        alpha.id,
        place.id,
        PropertyValue::Place(PlaceValue {
            name: "Paris".into(),
            lat: Some(48.8566),
            lon: Some(2.3522),
        }),
    );
    set_value(
        &mut source,
        alpha.id,
        docs.id,
        PropertyValue::Files(vec![attachments[1]]),
    );
    set_value(
        &mut source,
        alpha.id,
        related.id,
        PropertyValue::Relation(vec![beta.id]),
    );
    set_value(
        &mut source,
        alpha.id,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![work.id]),
    );

    // Export the whole space as Markdown and import the archive into another workspace.
    let exported = export_all(&mut source, space, None, false, ExportFormat::Markdown, 50);
    assert!(!exported.plan.single_file);
    assert!(exported.warnings.is_empty(), "{:?}", exported.warnings);
    let (texts, binaries) = as_source(&mut source, &exported);
    let (_fx2, mut target, target_space) = another_core(5);
    let imported = run_import(&mut target, target_space, None, None, &texts, &binaries, 50);
    assert_eq!(imported.report.pages, 6);
    assert_eq!(imported.report.attachments, 4);

    // The tree, the icons and the schema.
    assert_eq!(
        outline(&tree(&mut target, target_space)),
        outline(&tree(&mut source, space)),
        "the same tree in the same order"
    );
    let source_ids = page_ids_of(&mut source, space);
    let target_ids = page_ids_of(&mut target, target_space);
    assert_eq!(
        page(&mut target, target_ids["Alpha"]).icon.as_deref(),
        Some("📝")
    );
    assert_eq!(
        described_schema(&mut target, target_space),
        described_schema(&mut source, space),
        "kinds, options, colours, relations and rollups come back by name"
    );

    // The contents: the same portable JSON up to ids and the documented losses.
    for title in ["Projects", "Alpha", "Beta", "Sample", "Tasks", "Notes"] {
        let before = normalised(&content_of(&mut source, source_ids[title]));
        let after = normalised(&content_of(&mut target, target_ids[title]));
        let differences = json_differences(&before, &after, 12);
        assert!(
            differences.is_empty(),
            "the content of {title} differs:
{}",
            differences.join(
                "
"
            )
        );
    }
    // The values.
    for title in ["Alpha", "Beta", "Sample"] {
        let before = described_values(&mut source, space, source_ids[title]);
        let after = described_values(&mut target, target_space, target_ids[title]);
        assert_eq!(after, before, "the values of {title}");
    }
    // The files.
    let mut source_names: Vec<String> = attachments
        .iter()
        .map(|id| info_of(&mut source, *id).name)
        .collect();
    source_names.sort();
    let mut target_names: Vec<String> = match target
        .dispatch(Request::AttachmentList {
            space_id: target_space,
        })
        .unwrap()
    {
        Response::Attachments(list) => list.into_iter().map(|a| a.name).collect(),
        other => panic!("unexpected {other:?}"),
    };
    target_names.sort();
    assert_eq!(target_names, source_names);
}

#[test]
fn the_golden_sample_alone_comes_back_through_an_archive() {
    // The page-v1 sample: every block and mark of the schema, round-tripped on its own.
    let (_fx, mut source, space) = setup();
    let estimate = prop_create(&mut source, space, "Estimate", PropertyKind::Number);
    let work = option_add(
        &mut source,
        space,
        TAGS_PROPERTY_ID,
        "work",
        OptionColor::Blue,
    );
    let target_page = create_page(&mut source, space, None, "Target");
    let sample = create_page(&mut source, space, None, "Sample");
    let attachments = upload_four(&mut source, space);
    source
        .replace_page_document_for_test(
            sample.id,
            &golden_for(&attachments, target_page.id, work.id, estimate.id),
        )
        .unwrap();

    let exported = export_all(&mut source, space, None, false, ExportFormat::Markdown, 500);
    let (texts, binaries) = as_source(&mut source, &exported);
    let (_fx2, mut target, target_space) = another_core(6);
    run_import(&mut target, target_space, None, None, &texts, &binaries, 50);
    let source_ids = page_ids_of(&mut source, space);
    let target_ids = page_ids_of(&mut target, target_space);
    let before = normalised(&content_of(&mut source, source_ids["Sample"]));
    let after = normalised(&content_of(&mut target, target_ids["Sample"]));
    let differences = json_differences(&before, &after, 20);
    assert!(
        differences.is_empty(),
        "the sample differs:
{}",
        differences.join(
            "
"
        )
    );
}

// ----- covers (ADR-0040) ------------------------------------------------------------------------

#[test]
fn front_matter_covers_are_read_named_or_as_pictures_of_the_source() {
    let (_fx, mut core, space) = setup();
    let source_page = |title: &str, front: &str| {
        (
            format!("notes/{title}.md"),
            format!("---\ntitle: {title}\n{front}---\n{title} text\n"),
        )
    };
    let imported = run_import(
        &mut core,
        space,
        None,
        None,
        &[
            source_page("Named", "cover: \"color:green\"\ncover_y: 10\n"),
            source_page("Gradient", "cover: gradient:sunset\n"),
            source_page("Picture", "cover: img/banner.png\ncover_y: 75\n"),
            source_page("Missing", "cover: img/gone.png\n"),
            source_page("Link", "cover: https://example.com/a.png\n"),
            source_page("Plain", ""),
        ],
        &[("notes/img/banner.png".into(), b"BANNER".to_vec())],
        50,
    );
    assert_eq!(imported.plan.needed_files, ["notes/img/banner.png"]);
    assert_eq!(imported.report.attachments, 1);
    let ids = page_ids_of(&mut core, space);
    let cover = |core: &mut Core, title: &str| {
        let info = page(core, ids[title]);
        (info.cover, info.cover_y)
    };
    assert_eq!(
        cover(&mut core, "Named"),
        (Some("color:green".into()), Some(10))
    );
    assert_eq!(
        cover(&mut core, "Gradient"),
        (Some("gradient:sunset".into()), None)
    );
    let (picture, y) = cover(&mut core, "Picture");
    assert_eq!(y, Some(75));
    let attachment: AttachmentId = picture
        .as_deref()
        .and_then(|cover| cover.strip_prefix("attachment:"))
        .expect("an uploaded cover")
        .parse()
        .unwrap();
    assert_eq!(info_of(&mut core, attachment).name, "banner.png");
    assert_eq!(
        crate::test_support_interop::read_attachment(&mut core, attachment),
        b"BANNER"
    );
    assert_eq!(info_of(&mut core, attachment).references, 1);
    // What is neither stays a property, as before; no cover is made from it.
    for title in ["Missing", "Link", "Plain"] {
        assert_eq!(cover(&mut core, title), (None, None), "{title}");
    }
    assert_eq!(
        value_named(&mut core, space, ids["Link"], "cover"),
        Some(PropertyValue::Text("https://example.com/a.png".into()))
    );
}

#[test]
fn covers_make_a_markdown_round_trip_and_the_html_export_shows_them() {
    let (_fx, mut source, space) = setup();
    let named = create_page(&mut source, space, None, "Named");
    let gradient = create_page(&mut source, space, None, "Gradient");
    let picture = create_page(&mut source, space, None, "Picture");
    create_page(&mut source, space, None, "Plain");
    let banner = upload(
        &mut source,
        space,
        "banner.png",
        "image/png",
        b"BANNER-BYTES",
    );
    let set = |core: &mut Core, page_id: PageId, cover: &str, cover_y: Option<u8>| {
        core.dispatch(Request::PageSetCover {
            page_id,
            cover: Some(cover.to_owned()),
            cover_y,
        })
        .unwrap();
    };
    set(&mut source, named.id, "color:purple", Some(20));
    set(&mut source, gradient.id, "gradient:ocean", None);
    set(
        &mut source,
        picture.id,
        &format!("attachment:{}", banner.attachment_id),
        Some(90),
    );

    // Markdown: the front matter says it, the picture is a file of the archive.
    let exported = export_all(&mut source, space, None, false, ExportFormat::Markdown, 50);
    assert!(exported.warnings.is_empty(), "{:?}", exported.warnings);
    let text_of = |path_end: &str| {
        exported
            .entries
            .iter()
            .find_map(|entry| match &entry.content {
                kb_types::ExportContent::Text { text } if entry.path.ends_with(path_end) => {
                    Some(text.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {path_end} in {:?}", exported.paths()))
    };
    assert!(text_of("Named.md").contains("cover: \"color:purple\"\ncover_y: 20\n"));
    assert!(text_of("Gradient.md").contains("cover: \"gradient:ocean\"\n"));
    assert!(!text_of("Gradient.md").contains("cover_y"));
    let picture_md = text_of("Picture.md");
    assert!(
        picture_md.contains("cover: \"") && picture_md.contains("banner.png\"\ncover_y: 90\n"),
        "{picture_md}"
    );
    assert!(!text_of("Plain.md").contains("cover"));
    assert!(
        exported
            .paths()
            .iter()
            .any(|path| path.ends_with("banner.png"))
    );

    let (texts, binaries) = as_source(&mut source, &exported);
    let (_fx2, mut target, target_space) = another_core(7);
    let imported = run_import(&mut target, target_space, None, None, &texts, &binaries, 50);
    assert_eq!(imported.report.pages, 4);
    let source_ids = page_ids_of(&mut source, space);
    let target_ids = page_ids_of(&mut target, target_space);
    let cover = |core: &mut Core, id: PageId| {
        let info = page(core, id);
        (info.cover, info.cover_y)
    };
    for title in ["Named", "Gradient", "Plain"] {
        assert_eq!(
            cover(&mut target, target_ids[title]),
            cover(&mut source, source_ids[title]),
            "{title}"
        );
    }
    let (arrived, y) = cover(&mut target, target_ids["Picture"]);
    assert_eq!(y, Some(90));
    let attachment: AttachmentId = arrived
        .as_deref()
        .and_then(|cover| cover.strip_prefix("attachment:"))
        .expect("the picture came back as a cover")
        .parse()
        .unwrap();
    assert_eq!(
        crate::test_support_interop::read_attachment(&mut target, attachment),
        b"BANNER-BYTES"
    );
    assert_eq!(info_of(&mut target, attachment).references, 1);

    // HTML: the banner, as an image of the archive for the picture, as a block for the others.
    let html = export_all(&mut source, space, None, false, ExportFormat::Html, 50);
    let html_of = |path_end: &str| {
        html.entries
            .iter()
            .find_map(|entry| match &entry.content {
                kb_types::ExportContent::Text { text } if entry.path.ends_with(path_end) => {
                    Some(text.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {path_end} in {:?}", html.paths()))
    };
    assert!(
        html_of("Named.html").contains("<div class=\"page-cover\" data-color=\"purple\"></div>")
    );
    assert!(
        html_of("Gradient.html")
            .contains("<div class=\"page-cover\" data-gradient=\"ocean\"></div>")
    );
    let picture_html = html_of("Picture.html");
    assert!(
        picture_html.contains("<img class=\"page-cover\" src=\"")
            && picture_html.contains("banner.png\" alt=\"\" style=\"object-position:50% 90%\">"),
        "{picture_html}"
    );
    assert!(
        !html_of("Plain.html").contains("<div class=\"page-cover\"")
            && !html_of("Plain.html").contains("<img class=\"page-cover\"")
    );
}

#[test]
fn a_cover_whose_file_is_not_in_the_export_is_left_out_with_its_position() {
    let (_fx, mut source, space) = setup();
    let lost = create_page(&mut source, space, None, "Lost");
    let banner = upload(&mut source, space, "banner.png", "image/png", b"BANNER");
    source
        .dispatch(Request::PageSetCover {
            page_id: lost.id,
            cover: Some(format!("attachment:{}", banner.attachment_id)),
            cover_y: Some(30),
        })
        .unwrap();
    // The file is not on this device (as after a sync that did not fetch it yet).
    crate::test_support_interop::make_not_downloaded(&mut source, banner.attachment_id);
    let exported = export_all(&mut source, space, None, false, ExportFormat::Markdown, 50);
    let text = exported
        .entries
        .iter()
        .find_map(|entry| match &entry.content {
            kb_types::ExportContent::Text { text } if entry.path.ends_with("Lost.md") => {
                Some(text.clone())
            }
            _ => None,
        })
        .expect("the page is exported");
    assert!(!text.contains("cover"), "{text}");
    assert_eq!(
        exported
            .warnings
            .iter()
            .map(|warning| (warning.code, warning.attachment_id))
            .collect::<Vec<_>>(),
        [(
            kb_types::ExportWarningCode::AttachmentNotDownloaded,
            Some(banner.attachment_id)
        )],
        "the missing file is warned about, once"
    );
}

#[test]
fn several_top_level_pages_of_a_named_source_go_into_one_container() {
    let (_fx, mut core, space) = setup();
    let home = create_page(&mut core, space, None, "Home");
    let imported = run_import(
        &mut core,
        space,
        Some(home.id),
        Some("Notion export"),
        &files(&[("Alpha.md", "Alpha text\n"), ("Beta.md", "Beta text\n")]),
        &[],
        50,
    );
    assert_eq!(imported.report.pages, 3);
    assert_eq!(
        outline(&tree(&mut core, space)),
        "Home\n  Notion export\n    Alpha\n    Beta\n"
    );
    let ids = page_ids_of(&mut core, space);
    assert_eq!(imported.report.root_page_ids, [ids["Notion export"]]);

    // Without a name the pages go straight under the destination.
    let imported = run_import(
        &mut core,
        space,
        Some(home.id),
        None,
        &files(&[("Gamma.md", "Gamma text\n"), ("Delta.md", "Delta text\n")]),
        &[],
        50,
    );
    assert_eq!(imported.report.root_page_ids.len(), 2);
    let ids = page_ids_of(&mut core, space);
    assert_eq!(imported.report.root_page_ids, [ids["Delta"], ids["Gamma"]]);
}
