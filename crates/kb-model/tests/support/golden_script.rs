//! The fixed script behind the golden fixtures in `fixtures/doc/`.
//!
//! Shared by `examples/make_golden.rs` (writes the fixtures, run once per format
//! version) and `tests/golden.rs` (checks them on every test run). Peer ids and
//! timestamps are fixed, so the documents are reproducible; the script touches every
//! field of the v1 shapes (icons, nesting, ordering by index, moves, trash with and
//! without `trashed_from`, restore, purge and tombstones, undo, unicode, a second
//! peer merged in). [`structure_with_attachments`] continues it with the attachment
//! manifests of Milestone 5 (`structure-v1-attachments.*`), and
//! [`structure_with_properties`] with the property schema and values of ADR-0026
//! (`structure-v1-properties.*`), so the first structure fixture keeps the bytes written
//! before either existed. [`account_with_sharing`] continues the account document with the maps of
//! shared spaces of Milestone 13 (`account-v1-sharing.*`), so `account-v1.*` keeps its bytes too, and
//! [`structure_with_links`] continues the structure document with the view links of Milestone 13
//! slice 4 (`structure-v1-links.*`), so `structure-v1.*` keeps its bytes as well.

#![allow(dead_code)]

use kb_doc::{Doc, DocKind};
use kb_model::{
    AccountDoc, AttachmentManifest, MANIFEST_VERSION, SpaceMeta, StructureDoc, ViewLinkRecord,
};
use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{
    AccountId, AttachmentId, ChecklistItem, ChecklistItemId, ChunkId, DateValue, DeviceId, FeedId,
    LinkId, OptionColor, OptionId, PageId, PropertyId, PropertyKind, PropertyValue, SpaceId,
    StatusGroup,
};

/// Peer of the device that writes the structure document.
pub const STRUCTURE_PEER: u64 = 0x0000_51de_0000_0001;
/// Peer of a second device whose edits are merged into the structure document.
pub const REMOTE_PEER: u64 = 0x0000_51de_0000_0002;
/// Peer of the account document.
pub const ACCOUNT_PEER: u64 = 0x0000_acc0_0000_0001;

/// Base time of the script (Unix ms, 2023-11-14).
pub const T0: i64 = 1_700_000_000_000;

pub fn space_id(n: u8) -> SpaceId {
    SpaceId::from_random_bytes([
        0x5a, n, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, n,
    ])
}

pub fn page_id(n: u8) -> PageId {
    PageId::from_random_bytes([
        0x70, n, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, n,
    ])
}

fn t(offset: i64) -> i64 {
    T0 + offset
}

/// Builds the golden structure document.
pub fn structure() -> StructureDoc {
    let mut doc = StructureDoc::new(
        Doc::new(DocKind::Structure, STRUCTURE_PEER),
        SpaceMeta {
            id: space_id(1),
            name: "Personal".into(),
            icon: Some("🏠".into()),
            created_at: T0,
        },
    );
    let p = page_id;
    let ok = |result: Result<Vec<PageId>, kb_model::ModelError>| {
        result.expect("golden script step");
    };

    ok(doc.create_page(p(1), None, None, "Inbox", t(1)));
    ok(doc.create_page(p(2), None, None, "Projects", t(2)));
    ok(doc.set_page_icon(p(2), Some("📁"), t(3)));
    ok(doc.create_page(p(3), Some(p(2)), None, "Atomik", t(4)));
    ok(doc.create_page(p(4), Some(p(3)), None, "Specs", t(5)));
    ok(doc.create_page(p(5), Some(p(2)), Some(0), "Notes — ünïcödé ✓", t(6)));
    ok(doc.create_page(p(6), None, Some(0), "Archive", t(7)));
    ok(doc.create_page(p(7), Some(p(6)), None, "Old idea", t(8)));
    ok(doc.create_page(p(8), None, None, "Scratch", t(9)));
    ok(doc.create_page(p(9), Some(p(8)), None, "Temp", t(10)));

    // Reorder and reparent.
    ok(doc.move_page(p(4), Some(p(2)), 1, t(11)));
    ok(doc.move_page(p(1), None, 2, t(12)));

    // Trash a root (no `trashed_from`) and a nested page (with it).
    ok(doc.trash_page(p(6), t(13)));
    ok(doc.trash_page(p(3), t(14)));

    // Trash and restore (fields cleared again), purge (tombstone).
    ok(doc.trash_page(p(8), t(15)));
    ok(doc.restore_page(p(8), t(16)));
    ok(doc.purge_page(p(9), t(17)));

    // A purge that is undone leaves no tombstone; a rename that is undone leaves the
    // old title. Both put engine-generated inverse operations into the history.
    ok(doc.create_page(p(10), None, None, "Mistake", t(18)));
    ok(doc.purge_page(p(10), t(19)));
    doc.undo().expect("undo purge").expect("something undone");
    ok(doc.rename_page(p(10), "Recovered", t(20)));
    ok(doc.rename_page(p(5), "Typo", t(21)));
    doc.undo().expect("undo rename").expect("something undone");
    ok(doc.rename_page(p(1), "Inbox ✉", t(22)));

    // A second device edits concurrently and both sides merge.
    let snapshot = doc.doc().export_snapshot().expect("snapshot");
    let mut phone = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, REMOTE_PEER, Some(&snapshot), &[]).expect("load"),
    )
    .expect("structure");
    ok(phone.create_page(p(11), Some(p(1)), None, "From the phone", t(23)));
    ok(phone.rename_page(p(2), "Projects 2026", t(24)));
    ok(doc.set_page_icon(p(8), Some("🧪"), t(25)));
    let to_doc = phone
        .doc()
        .export_updates_since(Some(&doc.doc().version()))
        .expect("export")
        .expect("phone changes");
    doc.doc().import(&to_doc).expect("import");
    doc.refresh().expect("refresh");
    doc
}

/// Builds the golden account document.
pub fn account() -> AccountDoc {
    let doc = AccountDoc::new(Doc::new(DocKind::Account, ACCOUNT_PEER));
    for n in 1..=3 {
        doc.add_space(space_id(n)).expect("add");
    }
    doc.move_space(space_id(3), 0).expect("move");
    doc.remove_space(space_id(2)).expect("remove");
    doc.add_space(space_id(4)).expect("add");
    doc
}

// ----- shared spaces (Milestone 13) -------------------------------------------------------

/// Peer of a second device of the account whose edits are merged into the account document.
pub const ACCOUNT_REMOTE_PEER: u64 = 0x0000_acc0_0000_0002;

pub fn account_id(n: u8) -> AccountId {
    AccountId::from_random_bytes([
        0xac, n, 0xc0, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, n,
    ])
}

pub fn feed_id(n: u8) -> FeedId {
    FeedId::from_random_bytes([
        0xfe, n, 0xed, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, n,
    ])
}

/// Builds the golden account document continued with the maps of shared spaces: space 1 is shared
/// through feed 1, the profile name, the AI switch of two spaces, three favourites of space 1 (one
/// moved to the front), two verified people (one marked twice, one unmarked again). A second device
/// then writes concurrently, and both sides merge: it starts the favourites of space 4 (a map no
/// one had) and a favourite of space 1, and records a second shared space.
pub fn account_with_sharing() -> AccountDoc {
    let doc = account();
    let ok = |result: Result<(), kb_model::ModelError>| result.expect("golden script step");
    let p = page_id;

    ok(doc.set_shared_space(space_id(1), feed_id(1)));
    ok(doc.set_profile_name("Alice ✓"));
    ok(doc.set_space_ai(space_id(1), false));
    ok(doc.set_space_ai(space_id(3), true));
    for n in 1..=3 {
        doc.set_favorite(space_id(1), p(n), true)
            .expect("favourite");
    }
    doc.move_favorite(space_id(1), p(3), Some(p(1)))
        .expect("move favourite");
    ok(doc.set_verified(account_id(1), Some(T0 + 1)));
    ok(doc.set_verified(account_id(1), Some(T0 + 99)));
    ok(doc.set_verified(account_id(2), Some(T0 + 2)));
    ok(doc.set_verified(account_id(3), Some(T0 + 3)));
    ok(doc.set_verified(account_id(3), None));

    // A second device edits concurrently and both sides merge.
    let snapshot = doc.doc().export_snapshot().expect("snapshot");
    let phone = AccountDoc::from_doc(
        Doc::load(DocKind::Account, ACCOUNT_REMOTE_PEER, Some(&snapshot), &[]).expect("load"),
    )
    .expect("account");
    phone
        .set_favorite(space_id(4), p(7), true)
        .expect("favourite");
    phone
        .set_favorite(space_id(1), p(4), true)
        .expect("favourite");
    ok(phone.set_shared_space(space_id(4), feed_id(4)));
    ok(doc.set_space_ai(space_id(4), true));
    doc.set_favorite(space_id(4), p(8), true)
        .expect("favourite");
    for (from, to) in [(&phone, &doc), (&doc, &phone)] {
        if let Some(blob) = from
            .doc()
            .export_updates_since(Some(&to.doc().version()))
            .expect("export")
        {
            to.doc().import(&blob).expect("import");
        }
    }
    doc
}

// ----- attachments (Milestone 5) ----------------------------------------------------------

/// Plaintext bytes per chunk of the golden manifests (`kb_types::attachments::CHUNK_SIZE`).
pub const GOLDEN_CHUNK_SIZE: u32 = 4 * 1024 * 1024;

pub fn attachment_id(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([
        0xa7, n, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xa9, 0xba, 0xcb, 0xdc, 0xed, n,
    ])
}

pub fn device_id(n: u8) -> DeviceId {
    DeviceId::from_random_bytes([
        0xde, n, 0x0e, 0x1e, 0x2e, 0x3e, 0x4e, 0x5e, 0x6e, 0x7e, 0x8e, 0x9e, 0xae, 0xbe, 0xce, n,
    ])
}

/// 32 bytes derived from `seed` and `index` (stands for a BLAKE3 digest).
pub fn digest(seed: u8, index: u8) -> [u8; 32] {
    core::array::from_fn(|i| {
        let i = u8::try_from(i).unwrap_or(u8::MAX);
        seed.wrapping_mul(37) ^ index.wrapping_mul(101) ^ i.wrapping_mul(13)
    })
}

/// A consistent manifest: `floor(size / GOLDEN_CHUNK_SIZE) + 1` chunks (the final chunk
/// is always a padded one).
pub fn manifest(
    n: u8,
    name: &str,
    mime: &str,
    size: u64,
    created_at: i64,
    device: u8,
) -> AttachmentManifest {
    let count = AttachmentManifest::expected_chunk_count(size, GOLDEN_CHUNK_SIZE)
        .and_then(|count| u8::try_from(count).ok())
        .expect("a small golden manifest");
    AttachmentManifest {
        v: MANIFEST_VERSION,
        attachment_id: attachment_id(n),
        name: name.into(),
        mime: mime.into(),
        size,
        hash: digest(n, 0xff),
        chunk_size: GOLDEN_CHUNK_SIZE,
        chunks: (0..count)
            .map(|index| ChunkId::from_bytes(digest(n, index)))
            .collect(),
        created_at,
        created_by: device_id(device),
    }
}

/// Builds the golden structure document with attachments: the golden structure, then
/// manifests written around a structural edit and its undo (which leaves them alone),
/// a removal, and a manifest from a second device merged in.
pub fn structure_with_attachments() -> StructureDoc {
    let mut doc = structure();
    let put = |doc: &StructureDoc, manifest: AttachmentManifest| {
        doc.attachment_put(&manifest).expect("golden manifest");
    };
    put(
        &doc,
        manifest(
            1,
            "Diagram of the sync flow.png",
            "image/png",
            48_213,
            t(30),
            1,
        ),
    );
    doc.create_page(page_id(12), Some(page_id(1)), None, "Undone", t(31))
        .expect("golden script step");
    put(
        &doc,
        manifest(
            2,
            "Quarterly report — ünïcödé.pdf",
            "application/pdf",
            9 * 1024 * 1024,
            t(32),
            1,
        ),
    );
    put(&doc, manifest(3, "empty.txt", "text/plain", 0, t(33), 1));
    put(
        &doc,
        manifest(5, "removed.bin", "application/octet-stream", 1, t(34), 1),
    );
    doc.attachment_remove(attachment_id(5)).expect("remove");
    // Undoes the page creation only.
    doc.undo().expect("undo").expect("something undone");

    let snapshot = doc.doc().export_snapshot().expect("snapshot");
    let phone = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, REMOTE_PEER, Some(&snapshot), &[]).expect("load"),
    )
    .expect("structure");
    put(
        &phone,
        manifest(
            4,
            "Photo.jpg",
            "image/jpeg",
            u64::from(GOLDEN_CHUNK_SIZE),
            t(35),
            2,
        ),
    );
    let to_doc = phone
        .doc()
        .export_updates_since(Some(&doc.doc().version()))
        .expect("export")
        .expect("phone changes");
    doc.doc().import(&to_doc).expect("import");
    doc.refresh().expect("refresh");
    doc
}

// ----- properties (ADR-0026) ----------------------------------------------------------------

pub fn property_id(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([
        0xb7, n, 0x31, 0x42, 0x53, 0x64, 0x75, 0x86, 0x97, 0xa8, 0xb9, 0xca, 0xdb, 0xec, 0xfd, n,
    ])
}

pub fn option_id(n: u8) -> OptionId {
    OptionId::from_random_bytes([
        0x0c, n, 0x41, 0x52, 0x63, 0x74, 0x85, 0x96, 0xa7, 0xb8, 0xc9, 0xda, 0xeb, 0xfc, 0x0d, n,
    ])
}

/// An item of the golden checklist (amendment of 2026-10-07).
pub fn checklist_item(n: u8, text: &str, done: bool) -> ChecklistItem {
    ChecklistItem {
        id: ChecklistItemId::from_random_bytes([
            0x1c, n, 0x51, 0x62, 0x73, 0x84, 0x95, 0xa6, 0xb7, 0xc8, 0xd9, 0xea, 0xfb, 0x0c, 0x1d,
            n,
        ]),
        text: text.to_owned(),
        done,
    }
}

/// Builds the golden structure document with properties: the golden structure, then a schema
/// of every kind (Tags written by its first option), values on live and trashed pages, a
/// rename, a move, option edits, an option deleted with its values, a property deleted and
/// its leftovers pruned by the next write, a structural edit and its undo (which leaves the
/// properties alone), and a second device's schema and value edits merged in.
pub fn structure_with_properties() -> StructureDoc {
    let mut doc = structure();
    let p = page_id;
    let mut new_option = {
        let mut next = 40;
        move || {
            next += 1;
            option_id(next)
        }
    };
    let kinds = [
        (1, "Summary", PropertyKind::Text),
        (2, "Cost", PropertyKind::Number),
        (3, "Area", PropertyKind::Select),
        (4, "Stage", PropertyKind::Status),
        (5, "Due", PropertyKind::Date),
        (6, "Done", PropertyKind::Checkbox),
        (7, "Link", PropertyKind::Url),
        (8, "Contact", PropertyKind::Email),
        (9, "Phone", PropertyKind::Phone),
        (10, "Skills", PropertyKind::MultiSelect),
        (12, "Attachments", PropertyKind::Files),
        (13, "To do", PropertyKind::Checklist),
    ];
    for (n, name, kind) in kinds {
        doc.create_property(
            property_id(n),
            name,
            kind,
            &mut new_option,
            t(40 + i64::from(n)),
        )
        .expect("golden property");
    }
    let add =
        |doc: &mut StructureDoc, property: PropertyId, n: u8, name: &str, color: OptionColor| {
            doc.add_option(property, option_id(n), name, color, None, t(60))
                .expect("golden option");
        };
    add(&mut doc, property_id(3), 11, "Home", OptionColor::Blue);
    add(&mut doc, property_id(3), 12, "Work", OptionColor::Green);
    add(
        &mut doc,
        property_id(3),
        13,
        "Ünïcödé ✓",
        OptionColor::Purple,
    );
    add(&mut doc, TAGS_PROPERTY_ID, 21, "urgent", OptionColor::Red);
    add(&mut doc, TAGS_PROPERTY_ID, 22, "later", OptionColor::Gray);
    add(&mut doc, TAGS_PROPERTY_ID, 23, "ideas", OptionColor::Yellow);
    add(&mut doc, property_id(10), 31, "rust", OptionColor::Orange);
    add(&mut doc, property_id(10), 32, "design", OptionColor::Pink);

    let set = |doc: &mut StructureDoc, page: PageId, property: PropertyId, value: PropertyValue| {
        doc.set_page_property(page, property, Some(value), t(70))
            .expect("golden value");
    };
    // Live pages: the first one has a value of every kind.
    set(
        &mut doc,
        p(1),
        property_id(1),
        PropertyValue::Text("Quarterly plan\nsecond line".into()),
    );
    set(&mut doc, p(1), property_id(2), PropertyValue::Number(12.5));
    set(
        &mut doc,
        p(1),
        property_id(3),
        PropertyValue::Select(option_id(12)),
    );
    set(
        &mut doc,
        p(1),
        property_id(4),
        PropertyValue::Status(option_id(42)),
    );
    set(
        &mut doc,
        p(1),
        property_id(5),
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
        }),
    );
    set(
        &mut doc,
        p(1),
        property_id(6),
        PropertyValue::Checkbox(true),
    );
    set(
        &mut doc,
        p(1),
        property_id(7),
        PropertyValue::Url("https://example.com/plan".into()),
    );
    set(
        &mut doc,
        p(1),
        property_id(8),
        PropertyValue::Email("ada@example.com".into()),
    );
    set(
        &mut doc,
        p(1),
        property_id(9),
        PropertyValue::Phone("+1 555 0100".into()),
    );
    set(
        &mut doc,
        p(1),
        property_id(10),
        PropertyValue::MultiSelect(vec![option_id(32), option_id(31)]),
    );
    set(
        &mut doc,
        p(1),
        property_id(12),
        PropertyValue::Files(vec![attachment_id(2), attachment_id(1)]),
    );
    set(
        &mut doc,
        p(1),
        property_id(13),
        PropertyValue::Checklist(vec![
            checklist_item(1, "Draft the plan", true),
            checklist_item(2, "Review with the team \u{2713}", false),
            checklist_item(3, "", false),
        ]),
    );
    set(
        &mut doc,
        p(1),
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![option_id(21), option_id(23)]),
    );
    set(
        &mut doc,
        p(2),
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![option_id(22)]),
    );
    set(
        &mut doc,
        p(2),
        property_id(5),
        PropertyValue::Date(DateValue {
            date: "2026-12-31".into(),
            time: None,
        }),
    );
    set(
        &mut doc,
        p(2),
        property_id(3),
        PropertyValue::Select(option_id(11)),
    );
    set(
        &mut doc,
        p(5),
        property_id(3),
        PropertyValue::Select(option_id(12)),
    );
    set(
        &mut doc,
        p(5),
        property_id(6),
        PropertyValue::Checkbox(false),
    );
    // A trashed page keeps its values.
    set(
        &mut doc,
        p(6),
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![option_id(22)]),
    );

    // Schema edits: a rename, a move, a recolour and rename of an option, a status option
    // with a group.
    doc.rename_property(property_id(1), "Summary line", t(71))
        .expect("rename");
    doc.move_property(property_id(9), Some(property_id(2)))
        .expect("move");
    doc.update_option(
        property_id(3),
        option_id(13),
        Some("Elsewhere"),
        Some(OptionColor::Brown),
        None,
    )
    .expect("update option");
    doc.add_option(
        property_id(4),
        option_id(50),
        "Blocked",
        OptionColor::Red,
        Some(StatusGroup::InProgress),
        t(72),
    )
    .expect("status option");
    // An option deleted with its values: "Work" leaves pages 1 and 5.
    doc.delete_option(property_id(3), option_id(12), t(73))
        .expect("delete option");
    // A property deleted; the next write of a page that held a value of it removes it.
    doc.delete_property(property_id(9))
        .expect("delete property");
    set(&mut doc, p(1), property_id(2), PropertyValue::Number(13.0));

    // A structural edit and its undo leave the properties alone.
    doc.create_page(p(13), Some(p(1)), None, "Undone", t(74))
        .expect("create");
    doc.undo().expect("undo").expect("something undone");

    // A second device edits concurrently and both sides merge: a property of its own, a tag
    // (Tags exists on both), a value of another property of a page the first device also
    // edits, and the first device's own edits meanwhile.
    let snapshot = doc.doc().export_snapshot().expect("snapshot");
    let mut phone = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, REMOTE_PEER, Some(&snapshot), &[]).expect("load"),
    )
    .expect("structure");
    phone
        .create_property(
            property_id(11),
            "From the phone",
            PropertyKind::Text,
            &mut new_option,
            t(80),
        )
        .expect("phone property");
    phone
        .add_option(
            TAGS_PROPERTY_ID,
            option_id(24),
            "from phone",
            OptionColor::Blue,
            None,
            t(81),
        )
        .expect("phone tag");
    phone
        .set_page_property(
            p(2),
            property_id(6),
            Some(PropertyValue::Checkbox(true)),
            t(82),
        )
        .expect("phone value");
    doc.add_option(
        TAGS_PROPERTY_ID,
        option_id(25),
        "from the laptop",
        OptionColor::Green,
        None,
        t(83),
    )
    .expect("laptop tag");
    doc.set_page_property(
        p(2),
        property_id(2),
        Some(PropertyValue::Number(1.5)),
        t(84),
    )
    .expect("laptop value");
    let to_doc = phone
        .doc()
        .export_updates_since(Some(&doc.doc().version()))
        .expect("export")
        .expect("phone changes");
    doc.doc().import(&to_doc).expect("import");
    doc.refresh().expect("refresh");
    doc
}

// ----- view links (Milestone 13, slice 4) -------------------------------------------------

/// A derived-looking link id (the bytes of `HKDF(K_link, "link-id")[..16]` are not stamped as a UUID).
pub fn link_id(n: u8) -> LinkId {
    LinkId::from_bytes([
        0x1c, n, 0x7a, 0x31, 0x42, 0x53, 0x64, 0x75, 0x86, 0x97, 0xa8, 0xb9, 0xca, 0xdb, 0xec, n,
    ])
}

fn view_link(n: u8, page: u8, account: u8, at: i64, expires_at: Option<i64>) -> ViewLinkRecord {
    ViewLinkRecord {
        link_id: link_id(n),
        page_id: page_id(page),
        created_by: account_id(account),
        created_at: at,
        expires_at,
    }
}

/// The golden structure document continued with the `links` map: two links of one page (one that
/// never expires, one that does), a link replaced by a later write, a link that was revoked, and one
/// from a second device merged in. The portable form is the one of `structure-v1` (links are not
/// exported), so the fixture is the engine's deep value next to the snapshot.
pub fn structure_with_links() -> StructureDoc {
    let mut doc = structure();
    let put = |doc: &StructureDoc, record: ViewLinkRecord| {
        doc.link_put(&record).expect("golden link");
    };
    put(&doc, view_link(1, 3, 1, t(80), None));
    put(
        &doc,
        view_link(2, 3, 2, t(81), Some(t(81) + 7 * 86_400_000)),
    );
    put(&doc, view_link(3, 5, 1, t(82), None));
    put(&doc, view_link(4, 4, 1, t(83), Some(t(83) + 1)));
    // A later write replaces the entry as a whole; a revoked link leaves no entry.
    put(
        &doc,
        view_link(3, 5, 1, t(82), Some(t(82) + 30 * 86_400_000)),
    );
    doc.link_remove(link_id(4)).expect("golden revoke");

    let snapshot = doc.doc().export_snapshot().expect("snapshot");
    let phone = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, REMOTE_PEER, Some(&snapshot), &[]).expect("load"),
    )
    .expect("structure");
    put(&phone, view_link(5, 2, 2, t(90), None));
    let to_doc = phone
        .doc()
        .export_updates_since(Some(&doc.doc().version()))
        .expect("export")
        .expect("phone changes");
    doc.doc().import(&to_doc).expect("import");
    doc.refresh().expect("refresh");
    doc
}
