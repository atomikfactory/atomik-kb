#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::Rng;
use kb_types::testing::SeededRng;
use loro::{ID, LoroDoc, LoroValue, TreeParentId, ValueOrContainer, VersionVector};

use super::*;

fn blob(engine: u8, format: u16, min_reader: u16, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = BLOB_MAGIC.to_vec();
    out.push(engine);
    out.extend_from_slice(&format.to_le_bytes());
    out.extend_from_slice(&min_reader.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// A document with `n` local edits on the map `m`.
fn edited(kind: DocKind, peer: u64, n: i64) -> Doc {
    let doc = Doc::new(kind, peer);
    for i in 0..n {
        doc.inner()
            .get_map("m")
            .insert(&format!("k{i}"), i)
            .unwrap();
        doc.inner().commit();
    }
    doc
}

fn state(doc: &Doc) -> LoroValue {
    doc.inner().get_deep_value()
}

/// Entries of the test map `m`.
fn map_len(doc: &Doc) -> usize {
    doc.inner().get_map("m").len()
}

fn map_i64(doc: &Doc, key: &str) -> Option<i64> {
    match doc.inner().get_map("m").get(key)? {
        ValueOrContainer::Value(LoroValue::I64(v)) => Some(v),
        _ => None,
    }
}

fn decoded(version: &Version) -> VersionVector {
    decode_version(version).unwrap()
}

/// Every change of every peer in the document's history.
fn changes(doc: &LoroDoc) -> Vec<loro::ChangeMeta> {
    let mut out = Vec::new();
    for (peer, end) in doc.oplog_vv().iter() {
        let mut counter = 0;
        while counter < *end {
            let change = doc.get_change(ID::new(*peer, counter)).unwrap();
            counter = change.id.counter + i32::try_from(change.len).unwrap();
            out.push(change);
        }
    }
    out
}

// ----- envelope ---------------------------------------------------------------------

#[test]
fn envelope_round_trips_both_kinds_with_the_documented_layout() {
    for (kind, byte) in [(BlobKind::Snapshot, 0u8), (BlobKind::Update, 1u8)] {
        let encoded = encode_blob(kind, b"payload");
        assert_eq!(&encoded[..4], b"KBD1");
        assert_eq!(encoded[4], ENGINE_LORO);
        assert_eq!(&encoded[5..7], &FORMAT_VERSION.to_le_bytes());
        assert_eq!(&encoded[7..9], &MIN_READER_VERSION.to_le_bytes());
        assert_eq!(encoded[9], byte);
        assert_eq!(encoded.len(), HEADER_LEN + 7);
        let (header, payload) = decode_blob(&encoded).unwrap();
        assert_eq!(
            header,
            BlobHeader {
                engine: ENGINE_LORO,
                format_version: FORMAT_VERSION,
                min_reader_version: MIN_READER_VERSION,
                kind,
            }
        );
        assert_eq!(payload, b"payload");
    }
    let empty = encode_blob(BlobKind::Update, &[]);
    let (header, payload) = decode_blob(&empty).unwrap();
    assert_eq!(header.kind, BlobKind::Update);
    assert!(payload.is_empty(), "an empty payload is a valid envelope");
}

#[test]
fn decode_rejects_truncated_headers() {
    let full = encode_blob(BlobKind::Snapshot, b"x");
    for len in 0..HEADER_LEN {
        match decode_blob(&full[..len]) {
            Err(DocError::Corrupt(msg)) => assert!(msg.contains("shorter"), "{msg}"),
            other => panic!("length {len}: unexpected {other:?}"),
        }
    }
}

#[test]
fn decode_rejects_bad_magic() {
    let mut bad = encode_blob(BlobKind::Update, b"x");
    bad[0] = b'X';
    assert!(matches!(decode_blob(&bad), Err(DocError::Corrupt(msg)) if msg == "bad magic"));
    // A raw Loro payload (no envelope) is refused, not passed to the engine.
    let raw = edited(DocKind::Account, 1, 1)
        .inner()
        .export(loro::ExportMode::all_updates())
        .unwrap();
    assert!(matches!(decode_blob(&raw), Err(DocError::Corrupt(_))));
}

#[test]
fn decode_rejects_unknown_engines() {
    for engine in [0u8, 2, 255] {
        assert!(matches!(
            decode_blob(&blob(engine, 1, 1, 1, b"x")),
            Err(DocError::UnsupportedEngine(e)) if e == engine
        ));
    }
}

#[test]
fn decode_requires_a_new_enough_reader_before_looking_at_the_kind() {
    match decode_blob(&blob(ENGINE_LORO, 2, 2, 1, b"x")) {
        Err(DocError::NeedsNewerReader { needed, supported }) => {
            assert_eq!(needed, 2);
            assert_eq!(supported, FORMAT_VERSION);
        }
        other => panic!("unexpected {other:?}"),
    }
    // A future kind byte from a newer writer still reports the reader version.
    assert!(matches!(
        decode_blob(&blob(ENGINE_LORO, 3, 3, 7, b"x")),
        Err(DocError::NeedsNewerReader { needed: 3, .. })
    ));
    // A newer format that old readers may still read is accepted.
    let (header, _) = decode_blob(&blob(ENGINE_LORO, 5, 1, 0, b"x")).unwrap();
    assert_eq!(header.format_version, 5);
    assert_eq!(header.kind, BlobKind::Snapshot);
}

#[test]
fn decode_rejects_unknown_kinds_and_inconsistent_versions() {
    for kind in [2u8, 9, 255] {
        assert!(matches!(
            decode_blob(&blob(ENGINE_LORO, 1, 1, kind, b"x")),
            Err(DocError::Corrupt(msg)) if msg.contains("kind")
        ));
    }
    for (format, min_reader) in [(0u16, 0u16), (0, 1), (1, 0)] {
        assert!(
            matches!(
                decode_blob(&blob(ENGINE_LORO, format, min_reader, 1, b"x")),
                Err(DocError::Corrupt(_))
            ),
            "format {format}, min reader {min_reader}"
        );
    }
}

#[test]
fn decode_and_import_never_panic_on_arbitrary_bytes() {
    let rng = SeededRng::new(0x5eed);
    let valid = edited(DocKind::Structure, 3, 3).export_snapshot().unwrap();
    let target = Doc::new(DocKind::Structure, 4);

    // Every truncation of a valid blob.
    for len in 0..valid.len() {
        let _ = decode_blob(&valid[..len]);
        assert!(
            target.import(&valid[..len]).is_err(),
            "truncated to {len} bytes"
        );
    }
    // Random garbage, with and without a valid header.
    for round in 0..300usize {
        let mut len_bytes = [0u8; 2];
        rng.fill_bytes(&mut len_bytes);
        let len = usize::from(u16::from_le_bytes(len_bytes)) % 512;
        let mut bytes = vec![0u8; len];
        rng.fill_bytes(&mut bytes);
        if round % 2 == 0 {
            let _ = decode_blob(&bytes);
            let _ = target.import(&bytes);
        } else {
            let wrapped = encode_blob(BlobKind::Update, &bytes);
            assert!(decode_blob(&wrapped).is_ok());
            assert!(
                matches!(target.import(&wrapped), Err(DocError::Engine(_))),
                "random payload of {len} bytes must be rejected by the engine"
            );
        }
    }
    // Single-byte corruption of a valid blob anywhere.
    for i in 0..valid.len() {
        let mut bad = valid.clone();
        bad[i] ^= 0x5a;
        let _ = decode_blob(&bad);
        let fresh = Doc::new(DocKind::Structure, 5);
        let _ = fresh.import(&bad);
    }
    assert!(
        decoded(&target.version()).is_empty(),
        "rejected blobs leave no trace"
    );
}

// ----- import / export --------------------------------------------------------------

#[test]
fn import_refuses_foreign_engines_and_newer_readers() {
    let doc = Doc::new(DocKind::Account, 1);
    let payload = edited(DocKind::Account, 2, 1)
        .inner()
        .export(loro::ExportMode::all_updates())
        .unwrap();
    assert!(matches!(
        doc.import(&blob(2, 1, 1, 1, &payload)),
        Err(DocError::UnsupportedEngine(2))
    ));
    assert!(matches!(
        doc.import(&blob(ENGINE_LORO, 2, 2, 1, &payload)),
        Err(DocError::NeedsNewerReader {
            needed: 2,
            supported: 1
        })
    ));
    assert!(matches!(
        Doc::load(
            DocKind::Account,
            1,
            Some(&blob(ENGINE_LORO, 2, 2, 0, &payload)),
            &[]
        ),
        Err(DocError::NeedsNewerReader { .. })
    ));
    // The same payload in a correct envelope is fine.
    doc.import(&blob(ENGINE_LORO, 1, 1, 1, &payload)).unwrap();
    assert_eq!(map_i64(&doc, "k0"), Some(0));
}

#[test]
fn export_updates_since_reports_nothing_new_as_none() {
    let doc = Doc::new(DocKind::Structure, 9);
    assert_eq!(
        doc.export_updates_since(None).unwrap(),
        None,
        "empty document"
    );
    let v0 = doc.version();
    assert_eq!(doc.export_updates_since(Some(&v0)).unwrap(), None);

    doc.inner().get_map("m").insert("a", 1).unwrap();
    // No explicit commit: export commits pending operations itself.
    let delta = doc
        .export_updates_since(Some(&v0))
        .unwrap()
        .expect("a change");
    assert_eq!(decode_blob(&delta).unwrap().0.kind, BlobKind::Update);
    let v1 = doc.version();
    assert_ne!(v0, v1);
    assert_eq!(doc.export_updates_since(Some(&v1)).unwrap(), None);

    // A version from a replica that is ahead (knows more) also means nothing new.
    let ahead = Doc::new(DocKind::Structure, 10);
    ahead.import(&doc.export_snapshot().unwrap()).unwrap();
    ahead.inner().get_map("m").insert("b", 2).unwrap();
    assert_eq!(
        doc.export_updates_since(Some(&ahead.version())).unwrap(),
        None
    );

    // A malformed version is reported, not trusted.
    assert!(matches!(
        doc.export_updates_since(Some(&Version(vec![0xff, 0xff, 0xff]))),
        Err(DocError::Corrupt(_))
    ));
}

#[test]
fn updates_and_snapshots_round_trip_and_imports_are_idempotent() {
    let a = edited(DocKind::Account, 1, 3);
    let all = a.export_updates_since(None).unwrap().unwrap();
    let snapshot = a.export_snapshot().unwrap();
    assert_eq!(decode_blob(&snapshot).unwrap().0.kind, BlobKind::Snapshot);

    let from_updates = Doc::load(DocKind::Account, 2, None, std::slice::from_ref(&all)).unwrap();
    let from_snapshot = Doc::load(DocKind::Account, 3, Some(&snapshot), &[]).unwrap();
    assert_eq!(state(&from_updates), state(&a));
    assert_eq!(state(&from_snapshot), state(&a));

    // Incremental: snapshot + later deltas, in persisted order.
    let v = a.version();
    a.inner().get_map("m").insert("late", "yes").unwrap();
    let d1 = a.export_updates_since(Some(&v)).unwrap().unwrap();
    let v2 = a.version();
    a.inner().get_map("m").insert("later", "too").unwrap();
    let d2 = a.export_updates_since(Some(&v2)).unwrap().unwrap();
    let reloaded = Doc::load(
        DocKind::Account,
        4,
        Some(&snapshot),
        &[d1.clone(), d2.clone()],
    )
    .unwrap();
    assert_eq!(state(&reloaded), state(&a));
    assert_eq!(reloaded.version(), a.version(), "canonical version bytes");

    // Re-importing known changes (and a whole snapshot) changes nothing.
    let before = state(&reloaded);
    let version_before = reloaded.version();
    for blob in [&all, &snapshot, &d1, &d2, &d1] {
        reloaded.import(blob).unwrap();
    }
    assert_eq!(state(&reloaded), before);
    assert_eq!(reloaded.version(), version_before);
}

#[test]
fn out_of_order_updates_wait_for_their_dependencies() {
    let a = edited(DocKind::Account, 1, 1);
    let v1 = a.version();
    a.inner().get_map("m").insert("second", 2).unwrap();
    let second = a.export_updates_since(Some(&v1)).unwrap().unwrap();
    let all_first = a.export_updates_since(None).unwrap().unwrap();

    let b = Doc::new(DocKind::Account, 2);
    // Depends on the first change: kept pending, and the import says so.
    assert!(!b.import_checked(&second).unwrap());
    assert_eq!(
        map_len(&b),
        0,
        "nothing applied before the dependency arrives"
    );
    assert!(b.import_checked(&all_first).unwrap());
    assert_eq!(state(&b), state(&a));
    assert!(
        b.import_checked(&second).unwrap(),
        "known changes import in full"
    );
}

#[test]
fn version_is_monotonic_and_canonical() {
    let doc = Doc::new(DocKind::Structure, 11);
    let mut previous = doc.version();
    for i in 0..5 {
        doc.inner().get_map("m").insert("k", i).unwrap();
        let next = doc.version();
        let (p, n) = (decoded(&previous), decoded(&next));
        assert!(n.includes_vv(&p) && !p.includes_vv(&n), "step {i}");
        previous = next;
    }
    // Two replicas with the same history report byte-identical versions, whatever
    // the order in which they learnt about the peers.
    let a = Doc::new(DocKind::Account, 100);
    let b = Doc::new(DocKind::Account, 200);
    let c = Doc::new(DocKind::Account, 300);
    for (doc, key) in [(&a, "a"), (&b, "b"), (&c, "c")] {
        doc.inner().get_map("m").insert(key, 1).unwrap();
    }
    let blobs: Vec<Vec<u8>> = [&a, &b, &c]
        .iter()
        .map(|d| d.export_updates_since(None).unwrap().unwrap())
        .collect();
    let x = Doc::new(DocKind::Account, 400);
    let y = Doc::new(DocKind::Account, 500);
    for blob in &blobs {
        x.import(blob).unwrap();
    }
    for blob in blobs.iter().rev() {
        y.import(blob).unwrap();
    }
    assert_eq!(x.version(), y.version());
}

#[test]
fn kind_is_kept_and_load_of_garbage_fails_cleanly() {
    for kind in [DocKind::Account, DocKind::Structure, DocKind::Page] {
        assert_eq!(Doc::new(kind, 1).kind(), kind);
        assert_eq!(Doc::load(kind, 1, None, &[]).unwrap().kind(), kind);
    }
    assert!(matches!(
        Doc::load(DocKind::Structure, 1, Some(b"not a blob"), &[]),
        Err(DocError::Corrupt(_))
    ));
    let garbage = encode_blob(BlobKind::Snapshot, b"definitely not loro");
    assert!(matches!(
        Doc::load(DocKind::Structure, 1, Some(&garbage), &[]),
        Err(DocError::Engine(_))
    ));
}

// ----- privacy (ADR-0011) -----------------------------------------------------------

#[test]
fn documents_use_the_given_peer_and_record_no_timestamps() {
    let doc = edited(DocKind::Structure, 0x1234_5678_9abc_def0, 3);
    assert_eq!(doc.inner().peer_id(), 0x1234_5678_9abc_def0);
    let tree = doc.inner().get_tree(STRUCTURE_PAGES_TREE);
    let node = tree.create(TreeParentId::Root).unwrap();
    tree.get_meta(node).unwrap().insert("title", "t").unwrap();
    doc.inner().commit();

    let all = changes(doc.inner());
    assert!(!all.is_empty());
    for change in &all {
        assert_eq!(change.id.peer, 0x1234_5678_9abc_def0);
        assert_eq!(change.timestamp, 0, "no wall-clock time inside changes");
        assert!(change.message.is_none());
    }

    // A loaded copy edits under its own new peer id, also without timestamps.
    let snapshot = doc.export_snapshot().unwrap();
    let copy = Doc::load(DocKind::Structure, 42, Some(&snapshot), &[]).unwrap();
    assert_eq!(copy.inner().peer_id(), 42);
    copy.inner().get_map("m").insert("x", 1).unwrap();
    copy.inner().commit();
    let peers: Vec<u64> = changes(copy.inner()).iter().map(|c| c.id.peer).collect();
    assert!(peers.contains(&42) && peers.contains(&0x1234_5678_9abc_def0));
    assert!(changes(copy.inner()).iter().all(|c| c.timestamp == 0));
}

#[test]
fn documents_never_share_a_peer_unless_told_to() {
    let rng = SeededRng::new(77);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..50 {
        let peer = u64::from_le_bytes(kb_types::RngExt::random_array(&rng));
        let doc = Doc::new(DocKind::Account, peer);
        assert_eq!(doc.inner().peer_id(), peer);
        assert!(seen.insert(doc.inner().peer_id()));
    }
    // The same peer only when the caller passes the same value.
    assert_eq!(
        Doc::new(DocKind::Page, 5).inner().peer_id(),
        Doc::new(DocKind::Account, 5).inner().peer_id()
    );
    // Loro reserves u64::MAX; the reserved value is remapped, never a panic.
    assert_eq!(
        Doc::new(DocKind::Account, u64::MAX).inner().peer_id(),
        u64::MAX - 1
    );
}

// ----- engine configuration -----------------------------------------------------------

/// Hex length of a fresh root's fractional index: one terminator byte + jitter bytes.
const JITTERED_ROOT_FI_HEX: usize = 2 * (1 + TREE_JITTER as usize);

fn first_root_fi(doc: &Doc) -> String {
    let tree = doc.inner().get_tree(STRUCTURE_PAGES_TREE);
    let node = tree.create(TreeParentId::Root).unwrap();
    tree.fractional_index(node).unwrap()
}

#[test]
fn structure_trees_use_jittered_fractional_indexes_seeded_per_peer() {
    let a = Doc::new(DocKind::Structure, 1);
    let b = Doc::new(DocKind::Structure, 2);
    let (fa, fb) = (first_root_fi(&a), first_root_fi(&b));
    assert_eq!(fa.len(), JITTERED_ROOT_FI_HEX, "{fa}");
    assert_ne!(fa, fb, "different peers draw different jitter");
    assert_eq!(
        fa,
        first_root_fi(&Doc::new(DocKind::Structure, 1)),
        "deterministic per peer"
    );

    // Same configuration after a load (configuration is not part of the data).
    let base = Doc::new(DocKind::Structure, 3);
    base.inner().get_map("space").insert("id", "s").unwrap();
    let snapshot = base.export_snapshot().unwrap();
    let la = Doc::load(DocKind::Structure, 10, Some(&snapshot), &[]).unwrap();
    let lb = Doc::load(DocKind::Structure, 11, Some(&snapshot), &[]).unwrap();
    assert!(
        la.inner()
            .get_tree(STRUCTURE_PAGES_TREE)
            .is_fractional_index_enabled()
    );
    let (fla, flb) = (first_root_fi(&la), first_root_fi(&lb));
    assert_eq!(fla.len(), JITTERED_ROOT_FI_HEX);
    assert_ne!(fla, flb);

    // And after importing a snapshot into a fresh, empty document.
    let fresh = Doc::new(DocKind::Structure, 12);
    fresh.import(&snapshot).unwrap();
    assert_eq!(first_root_fi(&fresh).len(), JITTERED_ROOT_FI_HEX);

    // Ordered inserts work (create_at needs the fractional index).
    let tree = la.inner().get_tree(STRUCTURE_PAGES_TREE);
    let n = tree.create_at(TreeParentId::Root, 0).unwrap();
    assert_eq!(tree.roots()[0], n);
}

#[test]
fn snapshot_values_survive_with_loro_value_types() {
    let doc = Doc::new(DocKind::Account, 1);
    let list = doc.inner().get_movable_list("spaces");
    list.push("a").unwrap();
    list.push("b").unwrap();
    let loaded = Doc::load(
        DocKind::Account,
        2,
        Some(&doc.export_snapshot().unwrap()),
        &[],
    )
    .unwrap();
    let value = loaded.inner().get_movable_list("spaces").get_value();
    match value {
        LoroValue::List(items) => assert_eq!(items.len(), 2),
        other => panic!("unexpected {other:?}"),
    }
}

/// Marks of the character typed at `pos` (null values are what an unmark leaves).
fn marks_at(text: &loro::LoroText, pos: usize) -> Vec<String> {
    let mut start = 0;
    for delta in text.to_delta() {
        if let loro::TextDelta::Insert { insert, attributes } = delta {
            let end = start + insert.chars().count();
            if (start..end).contains(&pos) {
                let mut names: Vec<String> = attributes
                    .into_iter()
                    .flatten()
                    .filter(|(_, value)| !matches!(value, LoroValue::Null))
                    .map(|(name, _)| name)
                    .collect();
                names.sort();
                return names;
            }
            start = end;
        }
    }
    panic!("position {pos} is outside the text");
}

#[test]
fn page_documents_expand_marks_per_the_shape_table() {
    let empty = || LoroValue::Map(Vec::<(String, LoroValue)>::new().into());
    let link =
        LoroValue::Map(vec![("href".to_owned(), LoroValue::from("https://a.example"))].into());
    let color =
        |name: &str| LoroValue::Map(vec![("color".to_owned(), LoroValue::from(name))].into());
    let marks: Vec<(&str, LoroValue, bool)> = vec![
        ("bold", empty(), true),
        ("italic", empty(), true),
        ("underline", empty(), true),
        ("strike", empty(), true),
        ("code", empty(), false),
        ("link", link, false),
        // Added 2026-10-07: one attribute each, expanding like bold.
        ("textColor", color("red"), true),
        ("highlight", color("yellow"), true),
        // Not in the table (a newer client's mark): applied with the default style.
        ("spoiler", empty(), false),
    ];
    // The table is the one documented for shape v1.
    let expanding: Vec<&str> = marks.iter().filter(|m| m.2).map(|m| m.0).collect();
    let table_expanding: Vec<&str> = PAGE_MARKS
        .iter()
        .filter(|(_, expand)| *expand == MarkExpand::After)
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(expanding, table_expanding);
    assert_eq!(PAGE_MARKS.len(), 8);

    // A new document, one loaded from a snapshot and one that imported a snapshot.
    let snapshot = Doc::new(DocKind::Page, 1).export_snapshot().unwrap();
    let fresh = Doc::new(DocKind::Page, 3);
    fresh.import(&snapshot).unwrap();
    let docs = [
        Doc::new(DocKind::Page, 1),
        Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap(),
        fresh,
    ];
    for (n, doc) in docs.iter().enumerate() {
        for (mark, value, expands) in &marks {
            let text = doc.inner().get_text(format!("t-{mark}").as_str());
            text.insert(0, "abc").unwrap();
            text.mark(0..3, mark, value.clone()).unwrap();
            // Typing right after the marked range, and right before it.
            text.insert(3, "X").unwrap();
            text.insert(0, "Y").unwrap();
            doc.inner().commit();
            let after = marks_at(&text, 4);
            let before = marks_at(&text, 0);
            assert_eq!(
                after.contains(&(*mark).to_owned()),
                *expands,
                "doc {n}: {mark} after"
            );
            assert!(before.is_empty(), "doc {n}: {mark} never expands before");
            assert_eq!(marks_at(&text, 2), [*mark], "doc {n}: {mark} inside");
        }
    }

    // The expansion is recorded in each mark operation: a replica without the table
    // (Loro's defaults know nothing about `strike` or `textColor` and say `none` for
    // `spoiler`) types next to imported marks exactly as a configured one does.
    let writer = Doc::new(DocKind::Page, 10);
    for (name, mark) in [("s", "strike"), ("c", "code"), ("t", "textColor")] {
        let text = writer.inner().get_text(name);
        text.insert(0, "abc").unwrap();
        text.mark(0..3, mark, color("red")).unwrap();
    }
    let blob = writer.export_updates_since(None).unwrap().unwrap();
    let (_, payload) = decode_blob(&blob).unwrap();
    let bare = LoroDoc::new();
    bare.import(payload).unwrap();
    for (name, mark, expands) in [
        ("s", "strike", true),
        ("c", "code", false),
        ("t", "textColor", true),
    ] {
        let text = bare.get_text(name);
        text.insert(3, "X").unwrap();
        assert_eq!(
            marks_at(&text, 3).contains(&mark.to_owned()),
            expands,
            "{mark} typed on a replica without the table"
        );
    }
}

// ----- snapshot dominance (ADR-0045): update_end_vv and Version::includes ---------------

/// One more local edit on the map `m` of `doc`, committed.
fn edit(doc: &Doc, key: &str) {
    doc.inner().get_map("m").insert(key, key).unwrap();
    doc.inner().commit();
}

/// `to` imports everything `from` has that `to` lacks (a sync in one direction).
fn pull(to: &Doc, from: &Doc) {
    if let Some(blob) = from.export_updates_since(Some(&to.version())).unwrap() {
        to.import(&blob).unwrap();
    }
}

/// What a device writes as one entry: the update since `since`, and its end version vector.
fn entry_since(doc: &Doc, since: &Version) -> (Vec<u8>, Version) {
    let blob = doc.export_updates_since(Some(since)).unwrap().unwrap();
    let end = update_end_vv(&blob).unwrap();
    (blob, end)
}

/// The version of the document a snapshot blob describes, as a device that applied it would see.
fn snapshot_version(kind: DocKind, snapshot: &[u8]) -> Version {
    Doc::load(kind, 4_000, Some(snapshot), &[])
        .unwrap()
        .version()
}

#[test]
fn the_end_version_vector_of_the_whole_history_is_the_documents_version() {
    // One peer, many commits: the update that holds all of them ends where the document does.
    let doc = edited(DocKind::Account, 7, 5);
    let blob = doc.export_updates_since(None).unwrap().unwrap();
    assert_eq!(update_end_vv(&blob).unwrap(), doc.version());

    // Several peers in one blob: the vector names each of them.
    let (a, b, c) = (
        Doc::new(DocKind::Account, 1),
        Doc::new(DocKind::Account, 2),
        Doc::new(DocKind::Account, 3),
    );
    edit(&a, "a1");
    edit(&a, "a2");
    pull(&b, &a);
    edit(&b, "b1");
    pull(&c, &b);
    edit(&c, "c1");
    edit(&c, "c2");
    edit(&c, "c3");
    let all = c.export_updates_since(None).unwrap().unwrap();
    let end = update_end_vv(&all).unwrap();
    assert_eq!(end, c.version(), "canonical bytes, equal to the document's");
    let vv = decoded(&end);
    assert_eq!(vv.len(), 3, "three writers: {vv:?}");
    assert_eq!(vv.get(&3), Some(&3), "peer 3 wrote three changes");
}

#[test]
fn the_end_version_vector_of_a_delta_covers_only_the_peers_it_carries() {
    let (a, b) = (Doc::new(DocKind::Account, 1), Doc::new(DocKind::Account, 2));
    edit(&a, "a1");
    edit(&a, "a2");
    pull(&b, &a);
    let known = b.version();
    edit(&b, "b1");
    edit(&b, "b2");

    let (delta, end) = entry_since(&b, &known);
    assert_eq!(decode_blob(&delta).unwrap().0.kind, BlobKind::Update);
    let vv = decoded(&end);
    assert_eq!(vv.len(), 1, "peer 1's operations are not in this delta");
    assert_eq!(vv.get(&2), Some(&2));
    // It is a function of the blob alone: parsing does not import anything.
    assert_eq!(update_end_vv(&delta).unwrap(), end);
    assert!(
        b.version().includes(&end).unwrap(),
        "the writer is at or past it"
    );
    assert!(
        !a.version().includes(&end).unwrap(),
        "a replica that lacks the delta is not"
    );
}

#[test]
fn the_end_version_vectors_of_one_writers_entries_grow() {
    // The marks of ADR-0045 are per (document, writer device): consecutive entries of one
    // writer have increasing end vectors.
    let doc = Doc::new(DocKind::Account, 9);
    let mut last = doc.version();
    let mut previous: Option<Version> = None;
    for n in 0..4 {
        edit(&doc, &format!("k{n}"));
        edit(&doc, &format!("l{n}"));
        let (_, end) = entry_since(&doc, &last);
        if let Some(previous) = &previous {
            assert!(end.includes(previous).unwrap());
            assert!(!previous.includes(&end).unwrap());
        }
        assert_eq!(end, doc.version());
        previous = Some(end);
        last = doc.version();
    }
}

#[test]
fn a_snapshot_blob_a_foreign_blob_and_a_damaged_one_have_no_end_version_vector() {
    let doc = edited(DocKind::Account, 1, 2);
    let snapshot = doc.export_snapshot().unwrap();
    assert!(matches!(
        update_end_vv(&snapshot),
        Err(DocError::Corrupt(msg)) if msg.contains("snapshot")
    ));
    // The envelope says update but the engine payload is a snapshot.
    let (_, payload) = decode_blob(&snapshot).unwrap();
    assert!(update_end_vv(&encode_blob(BlobKind::Update, payload)).is_err());

    let update = doc.export_updates_since(None).unwrap().unwrap();
    let mut bad_magic = update.clone();
    bad_magic[0] = b'X';
    assert!(matches!(
        update_end_vv(&bad_magic),
        Err(DocError::Corrupt(_))
    ));
    assert!(matches!(
        update_end_vv(&blob(2, 1, 1, 1, b"x")),
        Err(DocError::UnsupportedEngine(2))
    ));
    assert!(matches!(
        update_end_vv(&blob(ENGINE_LORO, 2, 2, 1, b"x")),
        Err(DocError::NeedsNewerReader { .. })
    ));
    assert!(matches!(
        update_end_vv(&encode_blob(BlobKind::Update, b"not an engine payload")),
        Err(DocError::Engine(_))
    ));
    assert!(update_end_vv(&encode_blob(BlobKind::Update, &[])).is_err());
    assert!(update_end_vv(&[]).is_err());
}

#[test]
fn an_update_without_operations_has_no_end_version_vector() {
    // A valid engine update that carries no change at all.
    let empty = LoroDoc::new()
        .export(loro::ExportMode::all_updates())
        .unwrap();
    assert!(matches!(
        update_end_vv(&encode_blob(BlobKind::Update, &empty)),
        Err(DocError::Corrupt(_) | DocError::Engine(_))
    ));
}

#[test]
fn update_end_vv_never_panics_on_damaged_updates() {
    let (a, b) = (Doc::new(DocKind::Account, 1), Doc::new(DocKind::Account, 2));
    edit(&a, "a1");
    pull(&b, &a);
    edit(&b, "b1");
    let valid = b.export_updates_since(None).unwrap().unwrap();
    for len in 0..valid.len() {
        assert!(update_end_vv(&valid[..len]).is_err(), "truncated to {len}");
    }
    // A flipped byte is an error or, if the engine cannot tell, still no panic.
    for i in 0..valid.len() {
        let mut bad = valid.clone();
        bad[i] ^= 0x5a;
        let _ = update_end_vv(&bad);
    }
}

#[test]
fn includes_is_the_partial_order_of_version_vectors() {
    let (a, b) = (Doc::new(DocKind::Account, 1), Doc::new(DocKind::Account, 2));
    let empty = a.version();
    edit(&a, "a1");
    let v1 = a.version();
    edit(&a, "a2");
    let v2 = a.version();
    edit(&b, "b1");
    let vb = b.version();

    assert!(v2.includes(&v1).unwrap());
    assert!(!v1.includes(&v2).unwrap());
    assert!(
        v1.includes(&v1).unwrap(),
        "equal vectors include each other"
    );
    assert!(v1.includes(&empty).unwrap());
    assert!(empty.includes(&empty).unwrap());
    assert!(!empty.includes(&v1).unwrap());
    // Concurrent: neither includes the other.
    assert!(!v2.includes(&vb).unwrap());
    assert!(!vb.includes(&v2).unwrap());
    // A replica that merged both includes both, and more than either.
    pull(&a, &b);
    assert!(a.version().includes(&v2).unwrap());
    assert!(a.version().includes(&vb).unwrap());
    assert!(!v2.includes(&a.version()).unwrap());

    // Malformed vectors are reported on either side, never guessed at.
    let junk = Version(vec![0xff, 0xff, 0xff]);
    assert!(matches!(v1.includes(&junk), Err(DocError::Corrupt(_))));
    assert!(matches!(junk.includes(&v1), Err(DocError::Corrupt(_))));
}

#[test]
fn a_snapshot_that_holds_every_entry_dominates_and_a_truncating_one_does_not() {
    // Three devices of one shared space edit one document independently; each publishes an entry.
    let (alice, bob, carol) = (
        Doc::new(DocKind::Page, 11),
        Doc::new(DocKind::Page, 22),
        Doc::new(DocKind::Page, 33),
    );
    let (alice_start, bob_start, carol_start) = (alice.version(), bob.version(), carol.version());
    edit(&alice, "a1");
    edit(&bob, "b1");
    edit(&bob, "b2");
    edit(&carol, "c1");
    let (a_entry, a_end) = entry_since(&alice, &alice_start);
    let (b_entry, b_end) = entry_since(&bob, &bob_start);
    let (c_entry, c_end) = entry_since(&carol, &carol_start);
    for end in [&a_end, &b_end, &c_end] {
        assert_eq!(decoded(end).len(), 1, "one writer per entry");
    }

    // An honest snapshot: a device that applied every entry (and wrote more of its own).
    let honest = Doc::new(DocKind::Page, 44);
    for entry in [&a_entry, &b_entry, &c_entry] {
        honest.import(entry).unwrap();
    }
    edit(&honest, "own");
    let honest_vv = snapshot_version(DocKind::Page, &honest.export_snapshot().unwrap());
    for end in [&a_end, &b_end, &c_end] {
        assert!(honest_vv.includes(end).unwrap());
    }

    // A truncating snapshot: its author applied Alice's and Carol's entries and left Bob's out.
    let truncating = Doc::new(DocKind::Page, 55);
    truncating.import(&a_entry).unwrap();
    truncating.import(&c_entry).unwrap();
    edit(&truncating, "t1");
    let truncating_vv = snapshot_version(DocKind::Page, &truncating.export_snapshot().unwrap());
    assert!(truncating_vv.includes(&a_end).unwrap());
    assert!(truncating_vv.includes(&c_end).unwrap());
    assert!(
        !truncating_vv.includes(&b_end).unwrap(),
        "it omits Bob's entry: not dominant"
    );
    // Taking the entry makes it dominant.
    truncating.import(&b_entry).unwrap();
    assert!(truncating.version().includes(&b_end).unwrap());
}

#[test]
fn a_snapshot_that_stops_short_of_a_writers_last_change_is_not_dominant() {
    // The sharper truncation: the snapshot knows the writer but not its last changes.
    let writer = Doc::new(DocKind::Page, 5);
    let start = writer.version();
    edit(&writer, "w1");
    let first = writer.version();
    let first_blob = writer.export_updates_since(Some(&start)).unwrap().unwrap();
    edit(&writer, "w2");
    edit(&writer, "w3");
    let (last_blob, last_end) = entry_since(&writer, &first);
    assert_eq!(decoded(&last_end).get(&5), Some(&3));

    let reader = Doc::new(DocKind::Page, 6);
    reader.import(&first_blob).unwrap();
    let short = snapshot_version(DocKind::Page, &reader.export_snapshot().unwrap());
    assert!(
        !short.includes(&last_end).unwrap(),
        "peer 5 at 1 is below the entry's end at 3"
    );
    reader.import(&last_blob).unwrap();
    let complete = snapshot_version(DocKind::Page, &reader.export_snapshot().unwrap());
    assert!(complete.includes(&last_end).unwrap());
}

// ----- the records engine (M13 slice 2, ADR-0048) -------------------------------------------

#[test]
fn a_records_update_holds_one_record_and_a_snapshot_the_set() {
    let update = encode_records_update(b"record one");
    assert_eq!(&update[..4], BLOB_MAGIC);
    assert_eq!(update[4], ENGINE_RECORDS);
    let blob = decode_records(&update).unwrap();
    assert_eq!(blob.kind, BlobKind::Update);
    assert_eq!(blob.records, vec![b"record one".as_slice()]);

    let records: [&[u8]; 3] = [b"a", b"second", b"third record"];
    let snapshot = encode_records_snapshot(&records);
    assert_eq!(snapshot[4], ENGINE_RECORDS);
    let blob = decode_records(&snapshot).unwrap();
    assert_eq!(blob.kind, BlobKind::Snapshot);
    assert_eq!(blob.records, records.to_vec());
    // The same set in the same order is the same bytes.
    assert_eq!(snapshot, encode_records_snapshot(&records));
    // An empty set is a snapshot too.
    let empty_blob = encode_records_snapshot(&[]);
    let empty = decode_records(&empty_blob).unwrap();
    assert_eq!(empty.kind, BlobKind::Snapshot);
    assert!(empty.records.is_empty());
}

#[test]
fn records_blobs_and_loro_blobs_are_told_apart() {
    // A Loro blob is not a records blob, and the other way round: each reader names the engine.
    let loro = Doc::new(DocKind::Account, 1).export_snapshot().unwrap();
    assert!(matches!(
        decode_records(&loro),
        Err(DocError::UnsupportedEngine(ENGINE_LORO))
    ));
    let records = encode_records_update(b"x");
    assert!(matches!(
        decode_blob(&records),
        Err(DocError::UnsupportedEngine(ENGINE_RECORDS))
    ));
    // The comments kind has no engine document to configure: nothing here makes a `Doc` of it.
    assert_eq!(DocKind::Comments, DocKind::Comments);
}

#[test]
fn a_damaged_records_blob_is_refused_never_a_panic() {
    let snapshot = encode_records_snapshot(&[b"one", b"two"]);
    // Every truncation is refused.
    for cut in 0..snapshot.len() {
        assert!(decode_records(&snapshot[..cut]).is_err(), "cut at {cut}");
    }
    // Trailing bytes, an empty update, an unknown kind, bad magic and a newer reader are refused.
    let mut longer = snapshot.clone();
    longer.push(0);
    assert!(matches!(decode_records(&longer), Err(DocError::Corrupt(_))));
    assert!(matches!(
        decode_records(&blob(ENGINE_RECORDS, 1, 1, KIND_UPDATE, b"")),
        Err(DocError::Corrupt(_))
    ));
    assert!(matches!(
        decode_records(&blob(ENGINE_RECORDS, 1, 1, 9, b"x")),
        Err(DocError::Corrupt(_))
    ));
    let mut bad_magic = encode_records_update(b"x");
    bad_magic[0] ^= 0xff;
    assert!(matches!(
        decode_records(&bad_magic),
        Err(DocError::Corrupt(_))
    ));
    assert!(matches!(
        decode_records(&blob(ENGINE_RECORDS, 2, 2, KIND_UPDATE, b"x")),
        Err(DocError::NeedsNewerReader { .. })
    ));
    // A count that promises more than the bytes hold, a zero-length record, a huge count.
    let mut lying = blob(ENGINE_RECORDS, 1, 1, KIND_SNAPSHOT, &2u32.to_le_bytes());
    lying.extend_from_slice(&1u32.to_le_bytes());
    lying.push(7);
    assert!(decode_records(&lying).is_err());
    let mut zero = blob(ENGINE_RECORDS, 1, 1, KIND_SNAPSHOT, &1u32.to_le_bytes());
    zero.extend_from_slice(&0u32.to_le_bytes());
    assert!(decode_records(&zero).is_err());
    let huge = blob(ENGINE_RECORDS, 1, 1, KIND_SNAPSHOT, &u32::MAX.to_le_bytes());
    assert!(decode_records(&huge).is_err());
}

// ----- shallow snapshot (M13 slice 4, ADR-0051) -------------------------------------------------

const SECRET_TEXT: &str = "the-launch-code-is-4711";
const SECRET_URL: &str = "https://secret.example/private-path";
const SECRET_VALUE: &str = "the-old-secret-value-of-a-key";
const SECRET_BLOCK: &str = "a-block-that-was-deleted-from-the-page";
const SECRET_NODE: &str = "a-tree-node-that-was-deleted-today";
const SECRET_CONCURRENT: &str = "typed-and-deleted-on-another-device";
const KEPT_TEXT: &str = "the text the page shows now";

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn leaks(bytes: &[u8]) -> Vec<&'static str> {
    [
        SECRET_TEXT,
        SECRET_URL,
        SECRET_VALUE,
        SECRET_BLOCK,
        SECRET_NODE,
        SECRET_CONCURRENT,
    ]
    .into_iter()
    .filter(|secret| contains(bytes, secret))
    .collect()
}

/// A page-like document on one device: text with a deleted passage and a deleted link mark, a map
/// key overwritten, a list item (a map holding a text container) deleted and a tree node deleted.
fn page_with_deletions(peer: u64) -> Doc {
    let doc = Doc::new(DocKind::Page, peer);
    let inner = doc.inner();

    let text = inner.get_text("body");
    text.insert(0, &format!("{KEPT_TEXT} {SECRET_TEXT} and the end"))
        .unwrap();
    inner.commit();
    let start = KEPT_TEXT.chars().count() + 1;
    text.mark(
        start..start + SECRET_TEXT.chars().count(),
        "link",
        SECRET_URL,
    )
    .unwrap();
    inner.commit();
    text.delete(start, SECRET_TEXT.chars().count() + 1).unwrap();
    inner.commit();

    let map = inner.get_map("m");
    map.insert("k", SECRET_VALUE).unwrap();
    inner.commit();
    map.insert("k", "current").unwrap();
    inner.commit();

    let blocks = inner.get_list("blocks");
    let keep = blocks.insert_container(0, loro::LoroMap::new()).unwrap();
    let keep_text = keep
        .insert_container("text", loro::LoroText::new())
        .unwrap();
    keep_text.insert(0, "a block that stays").unwrap();
    let gone = blocks.insert_container(1, loro::LoroMap::new()).unwrap();
    let gone_text = gone
        .insert_container("text", loro::LoroText::new())
        .unwrap();
    gone_text.insert(0, SECRET_BLOCK).unwrap();
    inner.commit();
    blocks.delete(1, 1).unwrap();
    inner.commit();

    let tree = inner.get_tree("tree");
    let kept = tree.create(TreeParentId::Root).unwrap();
    tree.get_meta(kept)
        .unwrap()
        .insert("title", "kept node")
        .unwrap();
    let node = tree.create(TreeParentId::Root).unwrap();
    tree.get_meta(node)
        .unwrap()
        .insert("title", SECRET_NODE)
        .unwrap();
    inner.commit();
    tree.delete(node).unwrap();
    inner.commit();

    text.insert(0, "> ").unwrap();
    inner.commit();
    doc
}

#[test]
fn a_shallow_snapshot_holds_the_current_state_and_no_history() {
    let doc = page_with_deletions(11);
    // The control: the whole history names every secret (as readable JSON), and the full snapshot
    // carries at least the plain text, so the searches below can see what they look for.
    let full = doc.export_snapshot().unwrap();
    assert!(contains(&full, SECRET_TEXT), "control: the snapshot bytes");
    let history = doc
        .inner()
        .export_json_updates(&VersionVector::default(), &doc.inner().oplog_vv());
    let history = format!("{history:?}");
    for secret in [
        SECRET_TEXT,
        SECRET_BLOCK,
        SECRET_NODE,
        SECRET_VALUE,
        SECRET_URL,
    ] {
        assert!(contains(history.as_bytes(), secret), "control: {secret}");
    }

    let before = state(&doc);
    let blob = doc.export_shallow_snapshot().unwrap();
    assert_eq!(leaks(&blob), Vec::<&str>::new(), "the blob leaks");
    assert!(
        blob.len() < full.len(),
        "shallow is smaller than the full snapshot"
    );
    // An ordinary snapshot blob: the header, the kind, the engine's payload.
    let (header, _) = decode_blob(&blob).unwrap();
    assert_eq!(header.kind, BlobKind::Snapshot);
    assert_eq!(&blob[..4], BLOB_MAGIC);
    // The source document is untouched by the export and still takes edits.
    assert_eq!(state(&doc), before);
    doc.inner().get_text("body").insert(0, "more").unwrap();
    doc.inner().commit();

    // A document imported from it shows the same state, and has no history to recover from.
    let viewer = Doc::load(DocKind::Page, 12, Some(&blob), &[]).unwrap();
    assert_eq!(state(&viewer), before);
    assert!(viewer.inner().is_shallow());
    let shown = viewer.inner().get_text("body").to_string();
    assert!(
        shown.contains(KEPT_TEXT) && !shown.contains(SECRET_TEXT),
        "{shown}"
    );
    for export in [
        viewer.export_snapshot().unwrap(),
        viewer.inner().export(ExportMode::all_updates()).unwrap(),
        viewer.export_shallow_snapshot().unwrap(),
    ] {
        assert_eq!(
            leaks(&export),
            Vec::<&str>::new(),
            "the imported document leaks"
        );
    }
    // The history before the export is gone: an earlier version cannot be checked out.
    let earlier = loro::Frontiers::from(ID::new(11, 0));
    assert!(viewer.inner().checkout(&earlier).is_err());
    assert!(viewer.inner().fork_at(&earlier).is_err());
    // Nor does the JSON history of the imported document name the deleted text.
    let json = viewer.inner().export_json_updates(
        &viewer.inner().shallow_since_vv().to_vv(),
        &viewer.inner().oplog_vv(),
    );
    assert_eq!(leaks(format!("{json:?}").as_bytes()), Vec::<&str>::new());
}

#[test]
fn a_shallow_snapshot_of_a_new_document_loads() {
    let doc = Doc::new(DocKind::Page, 1);
    let blob = doc.export_shallow_snapshot().unwrap();
    let loaded = Doc::load(DocKind::Page, 2, Some(&blob), &[]).unwrap();
    assert_eq!(state(&loaded), state(&doc));
}

#[test]
fn a_shallow_snapshot_after_a_merge_of_concurrent_edits_holds_no_history() {
    // Two devices edit the same page at once; the second types a passage and deletes it again
    // before the first merges. The merged document has two heads: the point where the history is
    // trimmed must not leave the second device's typing behind.
    let base = Doc::new(DocKind::Page, 1);
    base.inner().get_text("body").insert(0, "base ").unwrap();
    base.inner().commit();
    let snapshot = base.export_snapshot().unwrap();
    let first = Doc::load(DocKind::Page, 2, Some(&snapshot), &[]).unwrap();
    let second = Doc::load(DocKind::Page, 3, Some(&snapshot), &[]).unwrap();

    first.inner().get_text("body").insert(5, "kept").unwrap();
    first.inner().commit();
    let text = second.inner().get_text("body");
    text.insert(5, SECRET_CONCURRENT).unwrap();
    second.inner().commit();
    text.delete(5, SECRET_CONCURRENT.len()).unwrap();
    second.inner().commit();
    text.insert(5, "other").unwrap();
    second.inner().commit();

    pull(&first, &second);
    assert!(first.inner().oplog_frontiers().len() > 1, "two heads");
    let before = state(&first);
    let blob = first.export_shallow_snapshot().unwrap();
    assert_eq!(leaks(&blob), Vec::<&str>::new(), "the blob leaks");
    // The export left the document alone: same state, same heads, no scratch container.
    assert_eq!(state(&first), before);
    assert!(first.inner().oplog_frontiers().len() > 1);

    let viewer = Doc::load(DocKind::Page, 4, Some(&blob), &[]).unwrap();
    assert_eq!(state(&viewer), before);
    for export in [
        viewer.export_snapshot().unwrap(),
        viewer.inner().export(ExportMode::all_updates()).unwrap(),
    ] {
        assert_eq!(
            leaks(&export),
            Vec::<&str>::new(),
            "the imported document leaks"
        );
    }

    // Once a local edit follows the merge there is one head again and the plain export is enough.
    first.inner().get_text("body").insert(0, "! ").unwrap();
    first.inner().commit();
    assert_eq!(first.inner().oplog_frontiers().len(), 1);
    let blob = first.export_shallow_snapshot().unwrap();
    assert_eq!(
        leaks(&blob),
        Vec::<&str>::new(),
        "the blob after the local edit leaks"
    );
    let viewer = Doc::load(DocKind::Page, 5, Some(&blob), &[]).unwrap();
    assert_eq!(state(&viewer), state(&first));
}
