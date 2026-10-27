//! Attachment manifests in the structure document (Milestone 5, ADR-0020).

use kb_doc::{Doc, DocKind};
use kb_types::{AttachmentId, ChunkId, DeviceId};
use loro::{LoroValue, ToJson};
use serde_json::json;

use super::{add, assert_healthy, replica, send, structure, sync, title};
use crate::{AttachmentManifest, MANIFEST_VERSION, ModelError, StructureDoc};

const MIB: u64 = 1024 * 1024;
const CHUNK: u32 = 4 * 1024 * 1024;

fn aid(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([0xa7, n, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, n])
}

fn device() -> DeviceId {
    DeviceId::from_random_bytes([0xde; 16])
}

fn chunk(n: u8, index: usize) -> ChunkId {
    let index = u8::try_from(index).unwrap();
    ChunkId::from_bytes(std::array::from_fn(|i| {
        n ^ index.wrapping_mul(31) ^ u8::try_from(i).unwrap()
    }))
}

/// A consistent manifest of `size` bytes in 4 MiB chunks.
fn manifest(n: u8, size: u64, created_at: i64) -> AttachmentManifest {
    let count = AttachmentManifest::expected_chunk_count(size, CHUNK).unwrap();
    AttachmentManifest {
        v: MANIFEST_VERSION,
        attachment_id: aid(n),
        name: format!("file {n}.bin"),
        mime: "application/octet-stream".into(),
        size,
        hash: [n; 32],
        chunk_size: CHUNK,
        chunks: (0..usize::try_from(count).unwrap())
            .map(|index| chunk(n, index))
            .collect(),
        created_at,
        created_by: device(),
    }
}

fn is_shape(err: &ModelError) -> bool {
    matches!(err, ModelError::Shape(_))
}

/// The raw `attachments` root map.
fn raw(doc: &StructureDoc) -> loro::LoroMap {
    doc.doc().inner().get_map("attachments")
}

// ----- the manifest value ---------------------------------------------------------------------

#[test]
fn a_manifest_round_trips_through_its_loro_value() {
    let mut original = manifest(1, 9 * MIB, 1_700_000_000_123);
    original.name = "Rapport — ünïcödé.pdf".into();
    original.mime = "application/pdf".into();
    original.hash = std::array::from_fn(|i| u8::try_from(i * 7).unwrap());
    let value = original.to_loro().unwrap();
    assert_eq!(
        value.to_json_value(),
        json!({
            "v": 1,
            "name": "Rapport — ünïcödé.pdf",
            "mime": "application/pdf",
            "size": 9 * MIB,
            "hash": "00070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9",
            "chunk_size": CHUNK,
            "chunks": [chunk(1, 0).to_hex(), chunk(1, 1).to_hex(), chunk(1, 2).to_hex()],
            "created_at": 1_700_000_000_123_i64,
            "created_by": device().to_string(),
        })
    );
    // Integers are I64, never floats; hex is lowercase.
    assert_eq!(
        value.get_by_key("size"),
        Some(&LoroValue::I64(9 * 1024 * 1024))
    );
    assert!(matches!(
        value.get_by_key("chunks"),
        Some(LoroValue::List(_))
    ));
    assert_eq!(
        AttachmentManifest::from_loro(aid(1), &value).unwrap(),
        original
    );

    // The id comes from the key; unknown fields (a later additive field) are ignored.
    let LoroValue::Map(map) = value else {
        panic!("a manifest is a map")
    };
    let mut fields: std::collections::HashMap<String, LoroValue> =
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    fields.insert("thumbnail".into(), LoroValue::from("later"));
    let extended = LoroValue::from(fields);
    let read = AttachmentManifest::from_loro(aid(9), &extended).unwrap();
    assert_eq!(read.attachment_id, aid(9));
    assert_eq!(read.chunks, original.chunks);
}

#[test]
fn reading_refuses_other_versions_and_malformed_fields() {
    let good = manifest(1, 10, 5).to_loro().unwrap();
    let with = |key: &str, value: Option<LoroValue>| {
        let LoroValue::Map(map) = &good else {
            panic!("a manifest is a map")
        };
        let mut fields: std::collections::HashMap<String, LoroValue> =
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        match value {
            Some(value) => fields.insert(key.to_owned(), value),
            None => fields.remove(key),
        };
        LoroValue::from(fields)
    };
    let upper = chunk(1, 0).to_hex().to_uppercase();
    let cases = [
        ("v = 2", with("v", Some(LoroValue::I64(2)))),
        ("v = 0", with("v", Some(LoroValue::I64(0)))),
        ("v missing", with("v", None)),
        ("v as text", with("v", Some(LoroValue::from("1")))),
        ("v as float", with("v", Some(LoroValue::Double(1.0)))),
        ("name missing", with("name", None)),
        ("mime as number", with("mime", Some(LoroValue::I64(3)))),
        ("negative size", with("size", Some(LoroValue::I64(-1)))),
        (
            "chunk size too big",
            with("chunk_size", Some(LoroValue::I64(1 << 33))),
        ),
        (
            "negative chunk size",
            with("chunk_size", Some(LoroValue::I64(-4))),
        ),
        (
            "chunks not a list",
            with("chunks", Some(LoroValue::from("abc"))),
        ),
        (
            "uppercase chunk id",
            with(
                "chunks",
                Some(LoroValue::from(vec![LoroValue::from(upper.clone())])),
            ),
        ),
        (
            "short chunk id",
            with(
                "chunks",
                Some(LoroValue::from(vec![LoroValue::from("abcd")])),
            ),
        ),
        (
            "chunk id not a string",
            with("chunks", Some(LoroValue::from(vec![LoroValue::I64(1)]))),
        ),
        ("uppercase hash", with("hash", Some(LoroValue::from(upper)))),
        (
            "hash as bytes",
            with("hash", Some(LoroValue::from(vec![0u8; 32]))),
        ),
        ("created_at missing", with("created_at", None)),
        (
            "bad device",
            with("created_by", Some(LoroValue::from("phone"))),
        ),
        ("not a map", LoroValue::from("manifest")),
        ("null", LoroValue::Null),
    ];
    for (case, value) in cases {
        let err = AttachmentManifest::from_loro(aid(1), &value).unwrap_err();
        assert!(is_shape(&err), "{case}: {err:?}");
        assert!(!err.to_string().contains("file 1"), "{case}: no content");
    }
}

#[test]
fn writing_refuses_other_versions_and_sizes_beyond_i64() {
    let mut wrong_version = manifest(1, 10, 5);
    wrong_version.v = 2;
    assert!(is_shape(&wrong_version.to_loro().unwrap_err()));
    let mut huge = manifest(1, 10, 5);
    huge.size = u64::MAX;
    assert!(is_shape(&huge.to_loro().unwrap_err()));

    let doc = structure(1);
    let version = doc.doc().version();
    assert!(is_shape(&doc.attachment_put(&wrong_version).unwrap_err()));
    assert!(is_shape(&doc.attachment_put(&huge).unwrap_err()));
    assert_eq!(doc.doc().version(), version, "nothing written");
}

#[test]
fn the_expected_chunk_count_always_includes_the_final_padded_chunk() {
    // floor(size / chunk_size) + 1: an exact multiple still ends with a padded chunk.
    let chunk = u64::from(CHUNK);
    for (size, expected) in [
        (0, 1),
        (1, 1),
        (chunk - 1, 1),
        (chunk, 2),
        (chunk + 1, 2),
        (2 * chunk - 1, 2),
        (2 * chunk, 3),
        (9 * MIB, 3),
        (2 * 1024 * MIB, 513),
    ] {
        assert_eq!(
            AttachmentManifest::expected_chunk_count(size, CHUNK),
            Some(expected),
            "size {size}"
        );
    }
    assert_eq!(AttachmentManifest::expected_chunk_count(10, 0), None);
    assert_eq!(
        AttachmentManifest::expected_chunk_count(u64::MAX, 1),
        Some(u64::MAX)
    );
}

// ----- the attachments map --------------------------------------------------------------------

#[test]
fn manifests_are_put_read_listed_replaced_and_removed() {
    let doc = structure(1);
    assert!(doc.attachments().is_empty());
    assert!(doc.attachment(aid(1)).is_none());

    let (a, b, c, d) = (
        manifest(1, 0, 300),
        manifest(2, 5 * MIB, 100),
        manifest(3, 1, 200),
        manifest(4, 1, 200),
    );
    for m in [&a, &b, &d, &c] {
        doc.attachment_put(m).unwrap();
    }
    assert_eq!(doc.attachment(aid(2)), Some(b.clone()));
    assert_eq!(
        doc.attachments(),
        vec![b.clone(), c.clone(), d.clone(), a.clone()],
        "by created_at, then id"
    );
    // Stored under the id's string form, as a plain value.
    assert!(raw(&doc).get(&aid(1).to_string()).is_some());

    // Putting again replaces the whole manifest.
    let mut renamed = a.clone();
    renamed.name = "renamed.bin".into();
    doc.attachment_put(&renamed).unwrap();
    assert_eq!(doc.attachment(aid(1)), Some(renamed));

    doc.attachment_remove(aid(2)).unwrap();
    assert!(doc.attachment(aid(2)).is_none());
    assert_eq!(doc.attachments().len(), 3);
    // Removing an absent manifest writes nothing.
    let version = doc.doc().version();
    doc.attachment_remove(aid(2)).unwrap();
    doc.attachment_remove(aid(7)).unwrap();
    assert_eq!(doc.doc().version(), version);
    assert_healthy(&doc);

    // Persisted: a reload reads the same manifests.
    let reloaded = replica(&doc, 2);
    assert_eq!(reloaded.attachments(), doc.attachments());
}

#[test]
fn undo_never_touches_manifests() {
    let mut doc = structure(1);
    // Manifest writes alone leave nothing to undo.
    doc.attachment_put(&manifest(9, 1, 1)).unwrap();
    assert!(!doc.can_undo());
    assert!(doc.undo().unwrap().is_none());

    // Create a page, put a manifest, undo: the page is gone, the manifest stays.
    let page = add(&mut doc, 1, None, "Page");
    doc.attachment_put(&manifest(1, 10, 2)).unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert!(doc.page(page).is_none(), "the page creation is undone");
    assert!(doc.attachment(aid(1)).is_some(), "the manifest stays");
    assert!(!doc.can_undo());
    // Redo brings the page back and leaves the manifest alone too.
    assert!(doc.redo().unwrap().is_some());
    assert!(doc.page(page).is_some());
    assert_eq!(doc.attachments().len(), 2);

    // A removal is not undone either: undo reverts the rename, the manifest stays gone.
    doc.rename_page(page, "Renamed", 1_000).unwrap();
    doc.attachment_remove(aid(9)).unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(title(&doc, page), "Page");
    assert!(doc.attachment(aid(9)).is_none());
    assert!(doc.attachment(aid(1)).is_some());
    assert_healthy(&doc);
}

#[test]
fn a_manifest_written_inside_an_open_undo_group_stays_out_of_it() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    doc.rename_page(page, "AB", 1_000).unwrap();
    doc.attachment_put(&manifest(1, 10, 2)).unwrap();
    // Within the merge window, but the manifest's operations sit between the two
    // renames in this device's history, so Loro keeps them as two undo steps (the same
    // happens with the untracked space edits). Neither step touches the manifest.
    doc.rename_page(page, "ABC", 1_100).unwrap();
    doc.rename_page(page, "ABCD", 1_200).unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(title(&doc, page), "AB");
    assert!(doc.attachment(aid(1)).is_some());
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(title(&doc, page), "A");
    assert!(doc.attachment(aid(1)).is_some());
    assert!(doc.undo().unwrap().is_some());
    assert!(doc.page(page).is_none());
    assert!(doc.attachment(aid(1)).is_some());
    assert!(!doc.can_undo());
    assert_healthy(&doc);
}

#[test]
fn undo_after_a_remote_manifest_leaves_it_alone() {
    let mut a = structure(1);
    let mut b = replica(&a, 2);
    let page = add(&mut a, 1, None, "Local");
    b.attachment_put(&manifest(1, 10, 2)).unwrap();
    sync(&mut a, &mut b);
    assert!(a.attachment(aid(1)).is_some());
    assert!(a.undo().unwrap().is_some());
    assert!(a.page(page).is_none());
    assert!(a.attachment(aid(1)).is_some());
    assert_healthy(&a);
}

#[test]
fn concurrent_manifests_merge_and_the_same_id_converges() {
    let mut a = structure(1);
    let mut b = replica(&a, 2);
    a.attachment_put(&manifest(1, 10, 1)).unwrap();
    b.attachment_put(&manifest(2, 20, 2)).unwrap();
    // The same attachment written on both sides (never happens with random ids, but
    // must still converge): one whole manifest wins everywhere, never a mix.
    let mut left = manifest(3, 30, 3);
    left.name = "left".into();
    let mut right = manifest(3, 5 * MIB, 4);
    right.name = "right".into();
    a.attachment_put(&left).unwrap();
    b.attachment_put(&right).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.attachments(), b.attachments());
    assert_eq!(a.attachments().len(), 3);
    let winner = a.attachment(aid(3)).unwrap();
    assert!(winner == left || winner == right);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);

    // A removal on one side and a put of another manifest on the other both apply.
    a.attachment_remove(aid(1)).unwrap();
    b.attachment_put(&manifest(4, 1, 5)).unwrap();
    send(&a, &mut b);
    send(&b, &mut a);
    assert_eq!(a.attachments(), b.attachments());
    assert!(a.attachment(aid(1)).is_none());
    assert!(a.attachment(aid(4)).is_some());
}

#[test]
fn portable_json_lists_manifests_by_id_only_when_there_are_some() {
    let doc = structure(1);
    assert!(doc.to_portable().get("attachments").is_none());
    let mut m = manifest(2, 9 * MIB, 77);
    m.hash = [0xab; 32];
    doc.attachment_put(&m).unwrap();
    doc.attachment_put(&manifest(1, 0, 88)).unwrap();
    let portable = doc.to_portable();
    let attachments = portable["attachments"].as_object().unwrap();
    let keys: Vec<&String> = attachments.keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys, sorted, "keyed by attachment id");
    assert_eq!(
        portable["attachments"][aid(2).to_string()],
        json!({
            "chunkSize": CHUNK,
            "chunks": [chunk(2, 0).to_hex(), chunk(2, 1).to_hex(), chunk(2, 2).to_hex()],
            "createdAt": 77,
            "createdBy": device().to_string(),
            "hash": "ab".repeat(32),
            "mime": "application/octet-stream",
            "name": "file 2.bin",
            "size": 9 * MIB,
            "v": 1,
        })
    );
    // The rest of the document is unchanged.
    assert_eq!(portable["format"], 1);
    assert!(portable["pages"].as_array().unwrap().is_empty());
}

#[test]
fn check_invariants_reports_inconsistent_and_unreadable_manifests() {
    let doc = structure(1);
    doc.attachment_put(&manifest(1, 9 * MIB, 1)).unwrap();
    // Consistency is not checked on write: these land and are reported.
    let mut short = manifest(2, 9 * MIB, 2);
    short.chunks.truncate(2);
    short.name = "secret name".into();
    doc.attachment_put(&short).unwrap();
    let mut empty = manifest(3, 0, 3);
    empty.chunks.clear();
    doc.attachment_put(&empty).unwrap();
    let mut zero = manifest(4, 10, 4);
    zero.chunk_size = 0;
    doc.attachment_put(&zero).unwrap();
    // An exact multiple of the chunk size still ends with a padded chunk: one chunk
    // of data plus the final one.
    let exact = manifest(8, u64::from(CHUNK), 8);
    assert_eq!(exact.chunks.len(), 2);
    doc.attachment_put(&exact).unwrap();
    let mut without_final = manifest(7, u64::from(CHUNK), 7);
    without_final.chunks.truncate(1);
    doc.attachment_put(&without_final).unwrap();
    // Written by someone else: another version, a non-map value, a foreign key.
    let attachments = raw(&doc);
    let mut newer = manifest(5, 10, 5).to_loro().unwrap();
    if let LoroValue::Map(map) = &newer {
        let mut fields: std::collections::HashMap<String, LoroValue> =
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        fields.insert("v".into(), LoroValue::I64(2));
        newer = LoroValue::from(fields);
    }
    attachments.insert(&aid(5).to_string(), newer).unwrap();
    attachments.insert(&aid(6).to_string(), "secret").unwrap();
    attachments.insert("my secret key", 1).unwrap();
    doc.doc().inner().commit();

    let problems = doc.check_invariants();
    let about = |n: u8| -> Vec<&String> {
        problems
            .iter()
            .filter(|p| p.contains(&aid(n).to_string()))
            .collect()
    };
    assert!(about(1).is_empty(), "{problems:#?}");
    assert_eq!(about(2).len(), 1, "{problems:#?}");
    assert!(about(2)[0].contains("lists 2 chunks but its size needs 3"));
    assert!(about(3)[0].contains("lists 0 chunks but its size needs 1"));
    assert!(about(4)[0].contains("chunk size of 0"));
    assert!(about(7)[0].contains("lists 1 chunks but its size needs 2"));
    assert!(about(8).is_empty(), "{problems:#?}");
    assert!(about(5)[0].contains("version"));
    assert!(about(6)[0].contains("not a map"));
    assert!(
        problems
            .iter()
            .any(|p| p == "an attachments entry has a malformed key")
    );
    assert_eq!(problems.len(), 7, "{problems:#?}");
    assert!(problems.iter().all(|p| !p.contains("secret")));

    // Readers skip what they cannot read; the rest still lists.
    assert!(doc.attachment(aid(5)).is_none());
    assert!(doc.attachment(aid(6)).is_none());
    let listed: Vec<AttachmentId> = doc
        .attachments()
        .into_iter()
        .map(|m| m.attachment_id)
        .collect();
    assert_eq!(listed, vec![aid(1), aid(2), aid(3), aid(4), aid(7), aid(8)]);
    let portable = doc.to_portable();
    assert_eq!(portable["attachments"].as_object().unwrap().len(), 6);
}

#[test]
fn a_loaded_document_without_the_map_reads_as_having_no_attachments() {
    let doc = StructureDoc::from_doc(
        Doc::load(
            DocKind::Structure,
            3,
            Some(&structure(1).doc().export_snapshot().unwrap()),
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(doc.attachments().is_empty());
    assert!(doc.check_invariants().is_empty());
    // The first manifest creates nothing but the entry (the root map exists by name).
    doc.attachment_put(&manifest(1, 1, 1)).unwrap();
    assert_eq!(doc.attachments().len(), 1);
}
