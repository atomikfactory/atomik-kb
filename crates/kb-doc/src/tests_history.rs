//! Page history on the engine boundary (ADR-0057): the version id, forking a document at a version,
//! reverting to one as a new commit, and the runs of history no journal row accounts for.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cmp::Ordering;

use loro::{LoroValue, ValueOrContainer};

use super::*;

/// One local commit that sets `key` on the test map `m`.
fn set(doc: &Doc, key: &str, value: i64) {
    doc.inner().get_map("m").insert(key, value).unwrap();
    doc.inner().commit();
}

/// `a` takes everything `b` has.
fn pull(a: &Doc, b: &Doc) {
    if let Some(blob) = b.export_updates_since(Some(&a.version())).unwrap() {
        a.import(&blob).unwrap();
    }
}

fn get(doc: &Doc, key: &str) -> Option<i64> {
    match doc.inner().get_map("m").get(key)? {
        ValueOrContainer::Value(LoroValue::I64(v)) => Some(v),
        _ => None,
    }
}

fn keys(doc: &Doc) -> Vec<String> {
    let mut keys: Vec<String> = doc
        .inner()
        .get_map("m")
        .keys()
        .map(|key| key.to_string())
        .collect();
    keys.sort();
    keys
}

fn changes_of(doc: &Doc) -> usize {
    doc.inner().len_changes()
}

// ----- the id -------------------------------------------------------------------------------

#[test]
fn a_version_id_is_sorted_peer_at_counter_pairs_and_round_trips() {
    // Two replicas write concurrently; a third takes both: two heads.
    let a = Doc::new(DocKind::Page, 991_827_364);
    let b = Doc::new(DocKind::Page, 8_291_736_450_123);
    set(&a, "x", 1);
    set(&b, "y", 1);
    let merged = Doc::new(DocKind::Page, 6);
    pull(&merged, &b);
    pull(&merged, &a);
    let frontiers = merged.frontiers();
    let text = frontiers.encode();
    // Sorted by peer, "peer@counter", comma separated.
    let parts: Vec<&str> = text.split(',').collect();
    assert!(parts.len() >= 2, "{text}");
    let mut sorted = parts.clone();
    sorted.sort_by_key(|part| part.split('@').next().unwrap().parse::<u64>().unwrap());
    assert_eq!(parts, sorted);
    assert!(parts.iter().all(|part| {
        part.split_once('@')
            .is_some_and(|(p, c)| p.parse::<u64>().is_ok() && c.parse::<i32>().is_ok())
    }));
    assert_eq!(Frontiers::parse(&text).unwrap(), frontiers);
    assert_eq!(frontiers.to_string(), text);
    // The same version on every replica that has the changes.
    let other = Doc::new(DocKind::Page, 7);
    pull(&other, &merged);
    assert_eq!(other.frontiers().encode(), text);
}

#[test]
fn a_known_text_parses_and_encodes_in_the_documented_shape() {
    let parsed = Frontiers::parse("991827364@1203,8291736450123@57").unwrap();
    assert_eq!(parsed.encode(), "991827364@1203,8291736450123@57");
    assert!(!parsed.is_empty());
    // The order given does not matter: the encoding sorts.
    assert_eq!(
        Frontiers::parse("8291736450123@57,991827364@1203")
            .unwrap()
            .encode(),
        "991827364@1203,8291736450123@57"
    );
    assert!(Frontiers::parse("").unwrap().is_empty());
}

#[test]
fn texts_that_are_not_version_ids_are_refused() {
    for bad in [
        "a@1",
        "1@",
        "@1",
        "1",
        "1@x",
        "1@-2",
        "+1@2",
        "1@+2",
        "1@2,",
        ",1@2",
        "1@2,,3@4",
        "1@2,1@3",
        " 1@2",
        "1@2 ",
        "1 @2",
        "1@2;3@4",
        "18446744073709551615@1",
        "18446744073709551616@1",
        "1@2147483648",
        "0x10@1",
    ] {
        assert!(
            matches!(Frontiers::parse(bad), Err(DocError::InvalidVersion(_))),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn resolving_checks_the_history_and_answers_the_minimal_frontiers() {
    let a = Doc::new(DocKind::Page, 1);
    set(&a, "k0", 0);
    set(&a, "k1", 1);
    let b = Doc::new(DocKind::Page, 2);
    pull(&b, &a);
    set(&b, "k2", 2);

    // An operation the document has.
    assert_eq!(a.resolve_frontiers("1@0").unwrap().encode(), "1@0");
    // Peer 2's first operation follows peer 1's second: naming both is naming the later.
    assert_eq!(b.resolve_frontiers("1@1,2@0").unwrap().encode(), "2@0");
    // Unknown operations, a replica that is not there, the empty id, and bad syntax.
    for bad in ["1@2", "3@0", "2@0", "", "nonsense"] {
        assert!(
            matches!(a.resolve_frontiers(bad), Err(DocError::InvalidVersion(_))),
            "{bad:?}"
        );
    }
    // Comparing versions.
    let early = b.resolve_frontiers("1@0").unwrap();
    let late = b.resolve_frontiers("2@0").unwrap();
    assert_eq!(
        b.compare_frontiers(&early, &late).unwrap(),
        Some(Ordering::Less)
    );
    assert_eq!(
        b.compare_frontiers(&late, &early).unwrap(),
        Some(Ordering::Greater)
    );
    assert_eq!(
        b.compare_frontiers(&late, &late).unwrap(),
        Some(Ordering::Equal)
    );
    let unknown = Frontiers::parse("9@9").unwrap();
    assert!(b.compare_frontiers(&early, &unknown).is_err());
}

#[test]
fn the_frontiers_of_a_version_vector_are_the_version_the_document_had() {
    let doc = Doc::new(DocKind::Page, 4);
    set(&doc, "a", 1);
    let before = doc.version();
    let at_before = doc.frontiers();
    set(&doc, "b", 2);
    assert_ne!(doc.frontiers(), at_before);
    assert_eq!(doc.frontiers_at(&before).unwrap(), at_before);
    assert_eq!(doc.frontiers_at(&doc.version()).unwrap(), doc.frontiers());
    // The empty document has the empty version.
    let empty = Doc::new(DocKind::Page, 5);
    assert!(empty.frontiers().is_empty());
    assert!(empty.frontiers_at(&empty.version()).unwrap().is_empty());
    assert!(
        doc.frontiers_at(&Version(vec![0xff, 0x01]))
            .is_err_and(|e| matches!(e, DocError::Corrupt(_)))
    );
}

// ----- fork and revert ------------------------------------------------------------------------

#[test]
fn a_fork_at_a_version_reads_the_old_state_and_leaves_the_document_alone() {
    let doc = Doc::new(DocKind::Page, 10);
    set(&doc, "a", 1);
    set(&doc, "b", 2);
    let at_two = doc.frontiers();
    set(&doc, "c", 3);
    set(&doc, "a", 99);

    let fork = doc.fork_at(&at_two, 77).unwrap();
    assert_eq!(fork.kind(), DocKind::Page);
    assert_eq!(keys(&fork), ["a", "b"]);
    assert_eq!(get(&fork, "a"), Some(1));
    assert_eq!(fork.frontiers(), at_two);
    // The document itself is untouched.
    assert_eq!(keys(&doc), ["a", "b", "c"]);
    assert_eq!(get(&doc, "a"), Some(99));
    // The copy is a document of its own: it takes the given peer and records no timestamps.
    set(&fork, "z", 5);
    let change = fork.inner().get_change(loro::ID::new(77, 0)).unwrap();
    assert_eq!(change.timestamp, 0);
    assert_eq!(
        keys(&doc),
        ["a", "b", "c"],
        "the copy's edit stayed in the copy"
    );
    // A fork at the current version is the document.
    assert_eq!(
        keys(&doc.fork_at(&doc.frontiers(), 78).unwrap()),
        ["a", "b", "c"]
    );
    // A version the history does not have.
    let unknown = Frontiers::parse("1@500").unwrap();
    assert!(matches!(
        doc.fork_at(&unknown, 79),
        Err(DocError::InvalidVersion(_))
    ));
}

#[test]
fn a_page_fork_keeps_the_page_configuration() {
    let doc = Doc::new(DocKind::Page, 11);
    set(&doc, "a", 1);
    let fork = doc.fork_at(&doc.frontiers(), 12).unwrap();
    assert_eq!(fork.kind(), DocKind::Page);
    // The copy exports and imports like any document of its kind.
    let snapshot = fork.export_snapshot().unwrap();
    assert!(Doc::load(DocKind::Page, 13, Some(&snapshot), &[]).is_ok());
}

#[test]
fn reverting_writes_one_new_commit_and_keeps_every_version() {
    let doc = Doc::new(DocKind::Page, 20);
    set(&doc, "a", 1);
    set(&doc, "b", 2);
    let v2 = doc.frontiers();
    set(&doc, "c", 3);
    set(&doc, "a", 99);
    let v4 = doc.frontiers();
    let changes_before = changes_of(&doc);

    doc.revert_to(&v2).unwrap();
    assert_eq!(keys(&doc), ["a", "b"]);
    assert_eq!(get(&doc, "a"), Some(1));
    // A forward change: new operations after the old ones, in one commit (one new change, or
    // the commit merged into the replica's last).
    let after = doc.frontiers();
    assert_ne!(after, v2);
    assert_ne!(after, v4);
    assert_eq!(
        doc.compare_frontiers(&v4, &after).unwrap(),
        Some(Ordering::Less)
    );
    assert!(changes_of(&doc) <= changes_before + 1);
    // The versions before are all still there.
    assert_eq!(keys(&doc.fork_at(&v4, 21).unwrap()), ["a", "b", "c"]);
    assert_eq!(get(&doc.fork_at(&v4, 22).unwrap(), "a"), Some(99));
    // And the revert is undone by reverting to the version before it.
    doc.revert_to(&v4).unwrap();
    assert_eq!(keys(&doc), ["a", "b", "c"]);
    assert_eq!(get(&doc, "a"), Some(99));
}

#[test]
fn reverting_to_the_state_the_document_has_writes_nothing() {
    let doc = Doc::new(DocKind::Page, 30);
    set(&doc, "a", 1);
    let now = doc.frontiers();
    doc.revert_to(&now).unwrap();
    assert_eq!(doc.frontiers(), now);
    // An unknown version is an error and changes nothing.
    let unknown = Frontiers::parse("30@500").unwrap();
    assert!(matches!(
        doc.revert_to(&unknown),
        Err(DocError::InvalidVersion(_))
    ));
    assert_eq!(doc.frontiers(), now);
    // The empty version would blank the document: refused.
    assert!(matches!(
        doc.revert_to(&Frontiers::parse("").unwrap()),
        Err(DocError::InvalidVersion(_))
    ));
    assert_eq!(get(&doc, "a"), Some(1));
}

#[test]
fn a_revert_merges_and_syncs_like_an_edit() {
    let a = Doc::new(DocKind::Page, 40);
    set(&a, "a", 1);
    let v1 = a.frontiers();
    set(&a, "b", 2);
    let b = Doc::new(DocKind::Page, 41);
    pull(&b, &a);
    a.revert_to(&v1).unwrap();
    // The other replica takes the revert as an update.
    pull(&b, &a);
    assert_eq!(keys(&b), ["a"]);
    assert_eq!(b.frontiers(), a.frontiers());
}

// ----- runs ------------------------------------------------------------------------------------

#[test]
fn a_history_nobody_accounts_for_is_one_run_per_replica_newest_first() {
    let a = Doc::new(DocKind::Page, 1);
    set(&a, "k0", 0);
    set(&a, "k1", 1);
    set(&a, "k2", 2);
    let b = Doc::new(DocKind::Page, 2);
    pull(&b, &a);
    set(&b, "k3", 3);
    set(&b, "k4", 4);
    let c = Doc::new(DocKind::Page, 3);
    pull(&c, &b);
    set(&c, "k5", 5);

    let runs = c.runs_outside(&[]).unwrap();
    let described: Vec<(String, usize)> = runs
        .iter()
        .map(|run| (run.version.encode(), run.operations))
        .collect();
    assert_eq!(
        described,
        [
            ("3@0".to_owned(), 1),
            ("2@1".to_owned(), 2),
            ("1@2".to_owned(), 3)
        ]
    );
    assert!(runs[0].lamport > runs[1].lamport && runs[1].lamport > runs[2].lamport);
    assert_eq!(
        runs.iter().map(|run| run.peer).collect::<Vec<_>>(),
        [3, 2, 1]
    );
    // Each run's version is a version of the document that forks cleanly.
    for run in &runs {
        assert!(c.fork_at(&run.version, 50).is_ok());
    }
    // The newest run is the current version here (a single head).
    assert_eq!(runs[0].version, c.frontiers());
}

#[test]
fn an_empty_document_has_no_runs() {
    let doc = Doc::new(DocKind::Page, 1);
    assert!(doc.runs_outside(&[]).unwrap().is_empty());
}

#[test]
fn a_covered_session_leaves_the_runs_before_it() {
    let a = Doc::new(DocKind::Page, 1);
    set(&a, "k0", 0);
    set(&a, "k1", 1);
    let b = Doc::new(DocKind::Page, 2);
    pull(&b, &a);
    let base = b.frontiers();
    set(&b, "k2", 2);
    set(&b, "k3", 3);
    let end = b.frontiers();

    // The journal saw peer 2's session only: peer 1's history stays undated.
    let runs = b
        .runs_outside(&[Span {
            base: base.clone(),
            end: end.clone(),
        }])
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].peer, 1);
    assert_eq!(runs[0].version.encode(), "1@1");
    assert_eq!(runs[0].operations, 2);

    // A session that began on the empty document covers it all.
    let all = b
        .runs_outside(&[Span {
            base: Frontiers::parse("").unwrap(),
            end,
        }])
        .unwrap();
    assert!(all.is_empty());
}

#[test]
fn a_session_covers_part_of_a_change_and_the_rest_stays_a_run() {
    // One replica, five operations in one commit.
    let doc = Doc::new(DocKind::Page, 9);
    for i in 0..5 {
        doc.inner()
            .get_map("m")
            .insert(&format!("k{i}"), i)
            .unwrap();
    }
    doc.inner().commit();
    assert_eq!(changes_of(&doc), 1);
    let base = doc.resolve_frontiers("9@2").unwrap();
    let end = doc.resolve_frontiers("9@4").unwrap();

    let runs = doc.runs_outside(&[Span { base, end }]).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].version.encode(), "9@2");
    assert_eq!(runs[0].operations, 3);
    // The partial version is a version of the document like any other.
    assert_eq!(keys(&doc.fork_at(&runs[0].version, 51).unwrap()).len(), 3);
}

#[test]
fn a_covered_operation_between_two_stretches_of_one_peer_ends_the_run() {
    let a = Doc::new(DocKind::Page, 1);
    set(&a, "k0", 0);
    set(&a, "k1", 1);
    let b = Doc::new(DocKind::Page, 2);
    pull(&b, &a);
    let base = b.frontiers();
    set(&b, "k2", 2);
    let end = b.frontiers();
    pull(&a, &b);
    set(&a, "k3", 3);
    set(&a, "k4", 4);

    let runs = a.runs_outside(&[Span { base, end }]).unwrap();
    let described: Vec<(String, usize)> = runs
        .iter()
        .map(|run| (run.version.encode(), run.operations))
        .collect();
    assert_eq!(
        described,
        [("1@3".to_owned(), 2), ("1@1".to_owned(), 2)],
        "peer 1's two stretches are two runs: peer 2's covered edit lies between them"
    );
}

#[test]
fn a_span_the_history_does_not_have_is_ignored() {
    let doc = Doc::new(DocKind::Page, 1);
    set(&doc, "k0", 0);
    let runs = doc
        .runs_outside(&[Span {
            base: Frontiers::parse("").unwrap(),
            end: Frontiers::parse("77@5").unwrap(),
        }])
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].operations, 1);
}

#[test]
fn concurrent_replicas_make_their_own_runs() {
    let a = Doc::new(DocKind::Page, 1);
    set(&a, "k0", 0);
    let b = Doc::new(DocKind::Page, 2);
    set(&b, "k1", 1);
    let merged = Doc::new(DocKind::Page, 3);
    pull(&merged, &a);
    pull(&merged, &b);
    set(&merged, "k2", 2);
    let runs = merged.runs_outside(&[]).unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].peer, 3);
    let mut others: Vec<u64> = runs[1..].iter().map(|run| run.peer).collect();
    others.sort_unstable();
    assert_eq!(others, [1, 2]);
}
