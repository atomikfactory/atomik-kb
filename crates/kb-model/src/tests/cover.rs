//! Page covers (ADR-0040): the node meta `cover` and `coverY`, what a page reports, the undo step,
//! the portable form and the readers the derived attachment references use.

use serde_json::json;

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{ModelError, StructureDoc};

const IMAGE: &str = "attachment:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn cover_of(doc: &StructureDoc, n: u8) -> (Option<String>, Option<u8>) {
    let page = doc.page(pid(n)).unwrap();
    (page.cover, page.cover_y)
}

#[test]
fn a_cover_and_its_position_are_set_changed_and_cleared() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    assert_eq!(cover_of(&doc, 1), (None, None));

    assert_eq!(
        doc.set_page_cover(a, Some("gradient:sunset"), None, 100)
            .unwrap(),
        vec![a]
    );
    assert_eq!(
        cover_of(&doc, 1),
        (Some("gradient:sunset".to_owned()), None)
    );
    assert_eq!(
        doc.page(a).unwrap().updated_at,
        100,
        "a cover change stamps the page"
    );

    // A position other than the default is stored; the same value again writes nothing.
    assert_eq!(
        doc.set_page_cover(a, Some("gradient:sunset"), Some(30), 110)
            .unwrap(),
        vec![a]
    );
    assert_eq!(
        cover_of(&doc, 1),
        (Some("gradient:sunset".to_owned()), Some(30))
    );
    let version = doc.doc().version();
    assert!(
        doc.set_page_cover(a, Some("gradient:sunset"), Some(30), 120)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        doc.doc().version(),
        version,
        "the same cover writes nothing"
    );
    assert_eq!(doc.page(a).unwrap().updated_at, 110);

    // 0 and 100 are the ends; 50 is the default and is stored as no value.
    for y in [0u8, 100] {
        doc.set_page_cover(a, Some(IMAGE), Some(y), 130).unwrap();
        assert_eq!(cover_of(&doc, 1), (Some(IMAGE.to_owned()), Some(y)));
    }
    doc.set_page_cover(a, Some(IMAGE), Some(50), 140).unwrap();
    assert_eq!(cover_of(&doc, 1), (Some(IMAGE.to_owned()), None));
    assert!(
        doc.set_page_cover(a, Some(IMAGE), None, 150)
            .unwrap()
            .is_empty(),
        "50 and no position are the same state"
    );
    // A position above 100 is clamped to the bottom, never stored as it is.
    doc.set_page_cover(a, Some(IMAGE), Some(200), 160).unwrap();
    assert_eq!(cover_of(&doc, 1), (Some(IMAGE.to_owned()), Some(100)));

    // Changing the cover without a position resets the position.
    doc.set_page_cover(a, Some("color:blue"), None, 170)
        .unwrap();
    assert_eq!(cover_of(&doc, 1), (Some("color:blue".to_owned()), None));

    // No cover clears the position too, whatever position is given.
    doc.set_page_cover(a, Some("color:blue"), Some(20), 180)
        .unwrap();
    assert_eq!(doc.set_page_cover(a, None, Some(20), 190).unwrap(), vec![a]);
    assert_eq!(cover_of(&doc, 1), (None, None));
    assert!(doc.set_page_cover(a, None, None, 200).unwrap().is_empty());
    assert_eq!(doc.page(a).unwrap().updated_at, 190);

    // A trashed page keeps editing its cover, like its icon and title.
    doc.set_page_cover(a, Some("color:red"), None, 210).unwrap();
    doc.trash_page(a, 220).unwrap();
    assert_eq!(
        doc.set_page_cover(a, Some("color:green"), Some(10), 230)
            .unwrap(),
        vec![a]
    );
    let trashed = &doc.trashed_pages()[0];
    assert_eq!(trashed.page.cover.as_deref(), Some("color:green"));
    assert_eq!(trashed.page.cover_y, Some(10));

    assert!(matches!(
        doc.set_page_cover(pid(9), None, None, 1),
        Err(ModelError::PageNotFound)
    ));
    assert_healthy(&doc);
}

#[test]
fn a_position_without_a_cover_or_out_of_range_in_the_document_reads_as_the_default() {
    // Another writer (an older or a faulty client) can leave anything under the keys.
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    let node = doc.index.node(a).unwrap();
    let meta = doc.tree.get_meta(node).unwrap();
    crate::meta::set_int(&meta, "coverY", 30).unwrap();
    assert_eq!(
        cover_of(&doc, 1),
        (None, None),
        "a position without a cover is not reported"
    );

    crate::meta::set_string(&meta, "cover", "color:blue").unwrap();
    assert_eq!(cover_of(&doc, 1), (Some("color:blue".to_owned()), Some(30)));
    for stored in [-1, 101, 5_000, 50] {
        crate::meta::set_int(&meta, "coverY", stored).unwrap();
        assert_eq!(
            cover_of(&doc, 1),
            (Some("color:blue".to_owned()), None),
            "{stored} is not a position of its own"
        );
    }
    meta.insert("coverY", "high").unwrap();
    assert_eq!(cover_of(&doc, 1), (Some("color:blue".to_owned()), None));
}

#[test]
fn page_covers_lists_every_page_with_a_cover_live_or_trashed_and_none_after_a_purge() {
    let mut doc = structure(1);
    let (a, b, c, d) = (
        add(&mut doc, 4, None, "A"),
        add(&mut doc, 2, None, "B"),
        add(&mut doc, 3, Some(2), "C"),
        add(&mut doc, 1, None, "D"),
    );
    assert!(doc.page_covers().is_empty());
    assert_eq!(doc.page_cover(a), None);
    assert_eq!(doc.page_cover(pid(9)), None, "an unknown page has none");

    doc.set_page_cover(a, Some(IMAGE), Some(10), 10).unwrap();
    doc.set_page_cover(c, Some("color:blue"), None, 11).unwrap();
    doc.set_page_cover(d, Some("gradient:dawn"), None, 12)
        .unwrap();
    // The icon is another key: it does not show here.
    doc.set_page_icon(b, Some(IMAGE), 13).unwrap();
    assert_eq!(doc.page_cover(c).as_deref(), Some("color:blue"));
    // In page id order, whatever the order of the pages in the tree.
    assert_eq!(
        doc.page_covers(),
        vec![
            (d, "gradient:dawn".to_owned()),
            (c, "color:blue".to_owned()),
            (a, IMAGE.to_owned()),
        ]
    );

    // A trashed page (and the pages of a trashed subtree) keep their covers.
    doc.trash_page(b, 20).unwrap();
    doc.trash_page(a, 21).unwrap();
    assert_eq!(doc.page_covers().len(), 3);

    // Clearing removes the entry; a purge removes the page's.
    doc.set_page_cover(d, None, None, 30).unwrap();
    assert_eq!(doc.page_cover(d), None);
    doc.purge_page(a, 31).unwrap();
    assert_eq!(doc.page_covers(), vec![(c, "color:blue".to_owned())]);
    assert_healthy(&doc);
}

#[test]
fn the_portable_form_lists_the_cover_and_a_position_that_is_not_the_default() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Plain");
    add(&mut doc, 2, None, "Covered");
    add(&mut doc, 3, None, "Moved");
    let before = doc.to_portable();
    assert!(!before.to_string().contains("cover"), "additive: no key");

    doc.set_page_cover(pid(2), Some("color:blue"), None, 10)
        .unwrap();
    doc.set_page_cover(pid(3), Some(IMAGE), Some(20), 11)
        .unwrap();
    let portable = doc.to_portable();
    let pages = portable["pages"].as_array().unwrap();
    assert!(pages[0].get("cover").is_none() && pages[0].get("coverY").is_none());
    assert_eq!(pages[1]["cover"], "color:blue");
    assert!(pages[1].get("coverY").is_none());
    assert_eq!(pages[2]["cover"], IMAGE);
    assert_eq!(pages[2]["coverY"], 20);

    // Keys are in sorted order, so the text is the same whichever way serde_json keeps them.
    let keys: Vec<&str> = pages[2]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "children",
            "cover",
            "coverY",
            "createdAt",
            "id",
            "title",
            "updatedAt"
        ]
    );

    // A trashed page lists it too.
    doc.trash_page(pid(2), 20).unwrap();
    assert_eq!(doc.to_portable()["trash"][0]["cover"], json!("color:blue"));
}

#[test]
fn undo_and_redo_restore_the_cover_in_one_step_and_a_burst_of_edits_merges() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    let plain = doc.to_portable();

    doc.set_page_cover(a, Some("color:blue"), Some(40), 5_000)
        .unwrap();
    let covered = doc.to_portable();
    assert_eq!(doc.undo().unwrap(), Some(vec![a]));
    assert_eq!(
        doc.to_portable(),
        plain,
        "one undo takes cover and position"
    );
    assert_eq!(cover_of(&doc, 1), (None, None));
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(doc.to_portable(), covered);
    assert_healthy(&doc);

    // Dragging the position in quick succession is one step; a later change is another.
    doc.set_page_cover(a, Some("color:blue"), Some(41), 10_000)
        .unwrap();
    doc.set_page_cover(a, Some("color:blue"), Some(42), 10_100)
        .unwrap();
    doc.set_page_cover(a, Some("color:blue"), Some(60), 20_000)
        .unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(cover_of(&doc, 1).1, Some(42));
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(cover_of(&doc, 1).1, Some(40));
    assert_healthy(&doc);
}

#[test]
fn two_devices_converge_on_the_cover() {
    let mut a = structure(1);
    add(&mut a, 1, None, "A");
    let mut b = replica(&a, 2);

    a.set_page_cover(pid(1), Some("color:blue"), Some(10), 100)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(cover_of(&b, 1), (Some("color:blue".to_owned()), Some(10)));

    // The two devices change it at the same time: both end with one of the two, the same one.
    a.set_page_cover(pid(1), Some("color:red"), None, 200)
        .unwrap();
    b.set_page_cover(pid(1), Some("gradient:dawn"), Some(90), 200)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(cover_of(&a, 1).0, cover_of(&b, 1).0);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
}
