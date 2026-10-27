//! Page appearance (ADR-0040): the node meta `font`, `smallText` and `locked`, what a page reports,
//! the undo step, the portable form and the merge of two devices. Moves and duplicates have their
//! appearance cases in `moves.rs` and `duplicate.rs`.

use kb_types::PageFont;
use serde_json::json;

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{ModelError, StructureDoc};

/// `(font, small text, locked)` of page `n`.
fn look_of(doc: &StructureDoc, n: u8) -> (Option<PageFont>, bool, bool) {
    let page = doc.page(pid(n)).unwrap();
    (page.font, page.small_text, page.locked)
}

/// The keys the node meta of page `n` holds, sorted.
fn meta_keys(doc: &StructureDoc, n: u8) -> Vec<String> {
    let node = doc.index.node(pid(n)).unwrap();
    let mut keys: Vec<String> = doc
        .meta_of(node)
        .unwrap()
        .keys()
        .map(|key| key.to_string())
        .collect();
    keys.sort();
    keys
}

#[test]
fn font_small_text_and_lock_are_set_changed_and_cleared_independently() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    assert_eq!(look_of(&doc, 1), (None, false, false));

    assert_eq!(
        doc.set_page_appearance(a, Some(PageFont::Serif), None, None, 100)
            .unwrap(),
        vec![a]
    );
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), false, false));
    assert_eq!(
        doc.page(a).unwrap().updated_at,
        100,
        "an appearance change stamps the page"
    );

    // Each part changes on its own and leaves the others as they are.
    doc.set_page_appearance(a, None, Some(true), None, 110)
        .unwrap();
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), true, false));
    doc.set_page_appearance(a, None, None, Some(true), 120)
        .unwrap();
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), true, true));
    doc.set_page_appearance(a, Some(PageFont::Mono), None, None, 130)
        .unwrap();
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Mono), true, true));
    // The values are the strings and booleans the contract names.
    assert_eq!(
        meta_keys(&doc, 1),
        [
            "created_at",
            "font",
            "id",
            "locked",
            "smallText",
            "title",
            "updated_at"
        ]
    );

    // All three at once are one write.
    doc.set_page_appearance(a, Some(PageFont::Serif), Some(false), Some(false), 140)
        .unwrap();
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), false, false));

    // The neutral values clear: the keys go, they are not stored as `false` or "default".
    doc.set_page_appearance(a, Some(PageFont::Default), None, None, 150)
        .unwrap();
    assert_eq!(look_of(&doc, 1), (None, false, false));
    assert_eq!(
        meta_keys(&doc, 1),
        ["created_at", "id", "title", "updated_at"]
    );
    assert_healthy(&doc);
}

#[test]
fn setting_what_the_page_already_has_writes_nothing() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    // Nothing asked, or the defaults on a page that has them.
    for (font, small, locked) in [
        (None, None, None),
        (Some(PageFont::Default), None, None),
        (None, Some(false), None),
        (None, None, Some(false)),
        (Some(PageFont::Default), Some(false), Some(false)),
    ] {
        let version = doc.doc().version();
        assert!(
            doc.set_page_appearance(a, font, small, locked, 50)
                .unwrap()
                .is_empty()
        );
        assert_eq!(doc.doc().version(), version);
    }
    assert_eq!(doc.page(a).unwrap().updated_at, 1);

    doc.set_page_appearance(a, Some(PageFont::Mono), Some(true), Some(true), 60)
        .unwrap();
    assert_eq!(doc.page(a).unwrap().updated_at, 60);
    let version = doc.doc().version();
    assert!(
        doc.set_page_appearance(a, Some(PageFont::Mono), Some(true), Some(true), 70)
            .unwrap()
            .is_empty()
    );
    // A part that is already there does not hide one that changes.
    assert_eq!(
        doc.set_page_appearance(a, Some(PageFont::Mono), Some(true), Some(false), 80)
            .unwrap(),
        vec![a]
    );
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Mono), true, false));
    assert_ne!(doc.doc().version(), version);
    assert_eq!(doc.page(a).unwrap().updated_at, 80);
}

#[test]
fn a_trashed_page_keeps_editing_its_appearance_and_an_unknown_page_is_refused() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    doc.trash_page(a, 20).unwrap();
    assert_eq!(
        doc.set_page_appearance(a, Some(PageFont::Serif), Some(true), Some(true), 30)
            .unwrap(),
        vec![a]
    );
    let trashed = &doc.trashed_pages()[0].page;
    assert_eq!(
        (trashed.font, trashed.small_text, trashed.locked),
        (Some(PageFont::Serif), true, true)
    );
    doc.restore_page(a, 40).unwrap();
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), true, true));

    assert!(matches!(
        doc.set_page_appearance(pid(9), Some(PageFont::Mono), None, None, 1),
        Err(ModelError::PageNotFound)
    ));
    assert_healthy(&doc);
}

#[test]
fn a_lock_is_a_ui_guard_the_model_does_not_enforce() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    let b = add(&mut doc, 2, None, "B");
    doc.set_page_appearance(a, None, None, Some(true), 10)
        .unwrap();
    // Renaming, the icon, the cover, moving and trashing all go through (ADR-0040).
    assert_eq!(doc.rename_page(a, "Renamed", 20).unwrap(), vec![a]);
    assert_eq!(doc.set_page_icon(a, Some("🌱"), 21).unwrap(), vec![a]);
    assert_eq!(
        doc.set_page_cover(a, Some("color:blue"), None, 22).unwrap(),
        vec![a]
    );
    assert!(!doc.move_page(a, Some(b), 0, 23).unwrap().is_empty());
    assert!(!doc.trash_page(a, 24).unwrap().is_empty());
    assert!(
        look_of_trashed(&doc).2,
        "the lock stays on the trashed page"
    );
    assert_healthy(&doc);
}

fn look_of_trashed(doc: &StructureDoc) -> (Option<PageFont>, bool, bool) {
    let page = &doc.trashed_pages()[0].page;
    (page.font, page.small_text, page.locked)
}

#[test]
fn values_a_foreign_writer_left_read_as_the_defaults_and_are_cleaned_up_by_a_change() {
    // Another writer (a newer or a faulty client) can leave anything under the keys.
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    let node = doc.index.node(a).unwrap();
    let meta = doc.tree.get_meta(node).unwrap();

    crate::meta::set_string(&meta, "font", "comic").unwrap();
    crate::meta::set_string(&meta, "smallText", "yes").unwrap();
    meta.insert("locked", 1).unwrap();
    assert_eq!(
        look_of(&doc, 1),
        (None, false, false),
        "a font that is not one and a flag that is not `true` read as off"
    );
    crate::meta::set_string(&meta, "font", "default").unwrap();
    assert_eq!(look_of(&doc, 1).0, None, "`default` is not a stored value");

    // The default is no value, so asking for it clears what a foreign writer left.
    crate::meta::set_string(&meta, "font", "comic").unwrap();
    assert_eq!(
        doc.set_page_appearance(a, Some(PageFont::Default), None, None, 10)
            .unwrap(),
        vec![a]
    );
    assert!(!meta_keys(&doc, 1).contains(&"font".to_owned()));
    // `false` over a flag that reads as off (a stored `false` here) is not a change.
    meta.insert("locked", false).unwrap();
    assert!(
        doc.set_page_appearance(a, None, None, Some(false), 11)
            .unwrap()
            .is_empty()
    );
    // A font replaces the unknown text.
    crate::meta::set_string(&meta, "font", "comic").unwrap();
    doc.set_page_appearance(a, Some(PageFont::Serif), None, None, 12)
        .unwrap();
    assert_eq!(look_of(&doc, 1).0, Some(PageFont::Serif));
    // The flags are real booleans once set.
    doc.set_page_appearance(a, None, Some(true), Some(true), 13)
        .unwrap();
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), true, true));
    assert_healthy(&doc);
}

#[test]
fn the_portable_form_lists_font_small_text_and_lock_only_on_pages_that_have_them() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Plain");
    add(&mut doc, 2, None, "Styled");
    add(&mut doc, 3, None, "Trashed");
    let before = doc.to_portable();
    let text = before.to_string();
    assert!(
        !text.contains("font") && !text.contains("smallText") && !text.contains("locked"),
        "additive: no key without a value"
    );

    doc.set_page_appearance(pid(2), Some(PageFont::Mono), Some(true), Some(true), 10)
        .unwrap();
    doc.set_page_appearance(pid(3), Some(PageFont::Serif), None, None, 11)
        .unwrap();
    let portable = doc.to_portable();
    let pages = portable["pages"].as_array().unwrap();
    for key in ["font", "smallText", "locked"] {
        assert!(pages[0].get(key).is_none(), "{key}");
    }
    assert_eq!(pages[1]["font"], "mono");
    assert_eq!(pages[1]["smallText"], true);
    assert_eq!(pages[1]["locked"], true);
    assert_eq!(pages[2]["font"], "serif");
    assert!(pages[2].get("smallText").is_none() && pages[2].get("locked").is_none());

    // Keys are in sorted order, so the text is the same whichever way serde_json keeps them.
    let keys: Vec<&str> = pages[1]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "children",
            "createdAt",
            "font",
            "id",
            "locked",
            "smallText",
            "title",
            "updatedAt"
        ]
    );

    // A trashed page lists them too.
    doc.trash_page(pid(2), 20).unwrap();
    let trash = &doc.to_portable()["trash"][0];
    assert_eq!(trash["font"], json!("mono"));
    assert_eq!(trash["locked"], json!(true));
    assert_eq!(trash["smallText"], json!(true));
}

#[test]
fn undo_and_redo_restore_the_appearance_in_one_step_and_a_burst_of_edits_merges() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    let plain = doc.to_portable();

    doc.set_page_appearance(a, Some(PageFont::Serif), Some(true), Some(true), 5_000)
        .unwrap();
    let styled = doc.to_portable();
    assert_eq!(doc.undo().unwrap(), Some(vec![a]));
    assert_eq!(
        doc.to_portable(),
        plain,
        "one undo takes font, small text and lock"
    );
    assert_eq!(look_of(&doc, 1), (None, false, false));
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(doc.to_portable(), styled);
    assert_healthy(&doc);

    // Switches pressed in quick succession are one step; a later change is another.
    doc.set_page_appearance(a, Some(PageFont::Mono), None, None, 10_000)
        .unwrap();
    doc.set_page_appearance(a, None, Some(false), None, 10_100)
        .unwrap();
    doc.set_page_appearance(a, None, None, Some(false), 20_000)
        .unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Mono), false, true));
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(look_of(&doc, 1), (Some(PageFont::Serif), true, true));
    assert_healthy(&doc);

    // An icon change right after is its own step, not merged into the appearance.
    doc.set_page_appearance(a, None, Some(false), None, 30_000)
        .unwrap();
    doc.set_page_icon(a, Some("🌱"), 30_050).unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(doc.page(a).unwrap().icon, None);
    assert!(!look_of(&doc, 1).1);
    assert_healthy(&doc);
}

#[test]
fn two_devices_converge_on_the_appearance_and_keep_each_others_independent_parts() {
    let mut a = structure(1);
    add(&mut a, 1, None, "A");
    let mut b = replica(&a, 2);

    a.set_page_appearance(pid(1), Some(PageFont::Serif), None, Some(true), 100)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(look_of(&b, 1), (Some(PageFont::Serif), false, true));

    // One device unlocks while the other turns on small text: both changes survive.
    b.set_page_appearance(pid(1), None, None, Some(false), 200)
        .unwrap();
    a.set_page_appearance(pid(1), None, Some(true), None, 200)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(look_of(&a, 1), (Some(PageFont::Serif), true, false));
    assert_eq!(look_of(&b, 1), look_of(&a, 1));

    // The two change the font at the same time: both end with one of the two, the same one.
    a.set_page_appearance(pid(1), Some(PageFont::Mono), None, None, 300)
        .unwrap();
    b.set_page_appearance(pid(1), Some(PageFont::Default), None, None, 300)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(look_of(&a, 1).0, look_of(&b, 1).0);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
    assert_healthy(&b);
}
