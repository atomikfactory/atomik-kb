//! Property tests: replicas of one page document converge.
//!
//! 2 to 4 replicas start from one snapshot (the canonical sample page) and apply
//! random scripts that mix the semantic operations of `PageDoc` (insert, set text,
//! delete, insert a page reference) with raw edits shaped like the editor binding's
//! (typing and deleting characters in any text run, marking and unmarking ranges with
//! the v1 marks, inserting a `pageRef` node between inline children, removing an
//! inline atom). They exchange update blobs in random order and chunking
//! (single-operation deltas, catch-up deltas, whole histories, duplicates, deltas
//! whose dependencies have not arrived yet). Invariants hold after every step; after
//! a final full exchange every replica has the same portable JSON, Markdown, block
//! ids and version, and the portable JSON rebuilds into itself
//! (`from_portable(to_portable(x))` is the identity).
//!
//! Deterministic by default (fixed seed, 64 cases). Explore further with
//! `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=<n> cargo test -p kb-model --test page_convergence`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_doc::{Doc, DocKind, Version};
use kb_model::{BlockKind, ModelError, PageDoc, schema};
use kb_types::{BlockId, PageId};
use loro::{Container, LoroList, LoroMap, LoroText, LoroValue, ValueOrContainer};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, RngSeed, TestRunner};

const SAMPLE: &str = include_str!("../../../fixtures/doc/page-v1.portable.json");

/// Texts the operations write (empty, ASCII, accents, CJK, astral, ZWJ sequences).
const TEXTS: &[&str] = &["", "a", "Hello", "é日𝔘", "two words", "👩‍💻 dev", " "];

#[derive(Debug, Clone)]
enum Op {
    Insert {
        after: Option<u8>,
        kind: u8,
        text: u8,
    },
    SetText {
        block: u8,
        text: u8,
    },
    Delete {
        block: u8,
    },
    PageRef {
        block: u8,
        target: u8,
        with_block: bool,
    },
    Type {
        run: u8,
        pos: u16,
        text: u8,
    },
    Erase {
        run: u8,
        pos: u16,
        len: u8,
    },
    Mark {
        run: u8,
        start: u16,
        len: u8,
        mark: u8,
    },
    Unmark {
        run: u8,
        start: u16,
        len: u8,
        mark: u8,
    },
    RawRef {
        block: u8,
        pos: u8,
        target: u8,
    },
    RemoveAtom {
        block: u8,
        pick: u8,
    },
}

#[derive(Debug, Clone)]
enum Step {
    /// A local operation on one replica.
    Local { replica: u8, op: Op },
    /// Delivers some of `from`'s per-operation blobs (any order, repeats allowed,
    /// dependencies possibly missing).
    Deliver { from: u8, to: u8, picks: Vec<u8> },
    /// Sends exactly what `to` lacks, as one blob.
    CatchUp { from: u8, to: u8 },
    /// Sends `from`'s whole history.
    Full { from: u8, to: u8 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (proptest::option::weighted(0.8, any::<u8>()), any::<u8>(), any::<u8>())
            .prop_map(|(after, kind, text)| Op::Insert { after, kind, text }),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(block, text)| Op::SetText { block, text }),
        2 => any::<u8>().prop_map(|block| Op::Delete { block }),
        1 => (any::<u8>(), any::<u8>(), any::<bool>())
            .prop_map(|(block, target, with_block)| Op::PageRef { block, target, with_block }),
        3 => (any::<u8>(), any::<u16>(), any::<u8>())
            .prop_map(|(run, pos, text)| Op::Type { run, pos, text }),
        2 => (any::<u8>(), any::<u16>(), any::<u8>())
            .prop_map(|(run, pos, len)| Op::Erase { run, pos, len }),
        2 => (any::<u8>(), any::<u16>(), any::<u8>(), any::<u8>())
            .prop_map(|(run, start, len, mark)| Op::Mark { run, start, len, mark }),
        1 => (any::<u8>(), any::<u16>(), any::<u8>(), any::<u8>())
            .prop_map(|(run, start, len, mark)| Op::Unmark { run, start, len, mark }),
        1 => (any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(block, pos, target)| Op::RawRef { block, pos, target }),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(block, pick)| Op::RemoveAtom { block, pick }),
    ]
}

fn step(replicas: u8) -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (0..replicas, op()).prop_map(|(replica, op)| Step::Local { replica, op }),
        3 => (0..replicas, 0..replicas, proptest::collection::vec(any::<u8>(), 1..6))
            .prop_map(|(from, to, picks)| Step::Deliver { from, to, picks }),
        1 => (0..replicas, 0..replicas).prop_map(|(from, to)| Step::CatchUp { from, to }),
        1 => (0..replicas, 0..replicas).prop_map(|(from, to)| Step::Full { from, to }),
    ]
}

fn script() -> impl Strategy<Value = (u8, Vec<Step>, Vec<u8>)> {
    (2u8..=4).prop_flat_map(|n| {
        (
            Just(n),
            proptest::collection::vec(step(n), 1..60),
            proptest::collection::vec(any::<u8>(), 0..12),
        )
    })
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn config() -> Config {
    Config {
        cases: env("PROPTEST_CASES").unwrap_or(64),
        rng_seed: RngSeed::Fixed(env("PROPTEST_RNG_SEED").unwrap_or(0x9a6e_2026)),
        failure_persistence: None,
        ..Config::default()
    }
}

struct Replica {
    page: PageDoc,
    /// Blobs exported after each local operation.
    outbox: Vec<Vec<u8>>,
    exported: Version,
    created: u16,
}

/// Unique per replica and counter. Byte 15 equals byte 1, so the id a quote derives
/// for its paragraph (byte 15 inverted) never collides with a generated one.
fn new_id(replica: &mut Replica, index: u8) -> BlockId {
    replica.created += 1;
    let [hi, lo] = replica.created.to_be_bytes();
    BlockId::from_random_bytes([
        0xb1, index, hi, lo, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, index,
    ])
}

fn target_page(n: u8) -> PageId {
    PageId::from_random_bytes([0x7a, n % 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14])
}

fn text(n: u8) -> &'static str {
    TEXTS[usize::from(n) % TEXTS.len()]
}

fn pick<T: Clone>(items: &[T], n: u16) -> Option<T> {
    (!items.is_empty()).then(|| items[usize::from(n) % items.len()].clone())
}

fn mark(n: u8) -> (&'static str, LoroValue) {
    let names = schema::MARKS;
    let (name, _) = names[usize::from(n) % names.len()];
    let value = if name == "link" {
        LoroValue::Map(
            vec![(
                "href".to_owned(),
                LoroValue::from(format!("https://example.com/{}", n % 3)),
            )]
            .into(),
        )
    } else if name == "textColor" || name == "highlight" {
        let colors = ["gray", "brown", "orange", "yellow", "green", "blue", "red"];
        LoroValue::Map(
            vec![(
                "color".to_owned(),
                LoroValue::from(colors[usize::from(n / 8) % colors.len()]),
            )]
            .into(),
        )
    } else {
        LoroValue::Map(Vec::<(String, LoroValue)>::new().into())
    };
    (name, value)
}

// Raw access to the engine shape, as the editor binding has it.

fn children(map: &LoroMap) -> Option<LoroList> {
    match map.get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => Some(list),
        _ => None,
    }
}

fn name(map: &LoroMap) -> Option<String> {
    match map.get("nodeName") {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

/// Every text run and every element map, pre-order.
fn walk(map: &LoroMap, texts: &mut Vec<LoroText>, maps: &mut Vec<LoroMap>) {
    maps.push(map.clone());
    let Some(list) = children(map) else {
        return;
    };
    for index in 0..list.len() {
        match list.get(index) {
            Some(ValueOrContainer::Container(Container::Text(text))) => texts.push(text),
            Some(ValueOrContainer::Container(Container::Map(child))) => walk(&child, texts, maps),
            _ => {}
        }
    }
}

fn shape(page: &PageDoc) -> (Vec<LoroText>, Vec<LoroMap>) {
    let (mut texts, mut maps) = (Vec::new(), Vec::new());
    walk(&page.doc().inner().get_map("doc"), &mut texts, &mut maps);
    (texts, maps)
}

fn inline_blocks(page: &PageDoc) -> Vec<LoroMap> {
    shape(page)
        .1
        .into_iter()
        .filter(|map| matches!(name(map).as_deref(), Some("paragraph" | "heading")))
        .collect()
}

fn ids_of_kind(page: &PageDoc, kinds: &[&str]) -> Vec<BlockId> {
    page.blocks()
        .into_iter()
        .filter(|block| kinds.contains(&block.kind.as_str()))
        .map(|block| block.id)
        .collect()
}

fn top_level(page: &PageDoc) -> Vec<BlockId> {
    page.to_portable()["content"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["attrs"]["id"].as_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Operations attempted and actually applied.
#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    attempted: usize,
    applied: usize,
}

/// Whether a semantic operation applied; refusals must be domain errors.
fn applied(result: Result<(), ModelError>, op: &Op) -> bool {
    match result {
        Ok(()) => true,
        Err(ModelError::Doc(err)) => panic!("{op:?} failed in the engine: {err}"),
        Err(_) => false,
    }
}

fn apply(replica: &mut Replica, index: u8, op: &Op) -> bool {
    let changed = match *op {
        Op::Insert {
            after,
            kind,
            text: t,
        } => {
            let tops = top_level(&replica.page);
            let after = after.and_then(|a| pick(&tops, u16::from(a)));
            let kind = match kind % 6 {
                0 => BlockKind::Paragraph,
                1 => BlockKind::Heading(1 + kind % 3),
                2 => BlockKind::CodeBlock {
                    language: Some("rust".into()),
                },
                3 => BlockKind::CodeBlock { language: None },
                4 => BlockKind::Quote,
                _ => BlockKind::Divider,
            };
            let content = if kind == BlockKind::Divider {
                ""
            } else {
                text(t)
            };
            let id = new_id(replica, index);
            applied(replica.page.insert_block(after, kind, content, id), op)
        }
        Op::SetText { block, text: t } => {
            match pick(
                &ids_of_kind(&replica.page, schema::TEXT_NODES),
                u16::from(block),
            ) {
                Some(id) => applied(replica.page.set_block_text(id, text(t)), op),
                None => false,
            }
        }
        Op::Delete { block } => match pick(&top_level(&replica.page), u16::from(block)) {
            Some(id) => applied(replica.page.delete_block(id), op),
            None => false,
        },
        Op::PageRef {
            block,
            target,
            with_block,
        } => match pick(
            &ids_of_kind(&replica.page, schema::INLINE_CONTENT_NODES),
            u16::from(block),
        ) {
            Some(id) => {
                let ref_id = new_id(replica, index);
                let to_block = with_block.then_some(id);
                applied(
                    replica
                        .page
                        .insert_page_ref(id, ref_id, target_page(target), to_block),
                    op,
                )
            }
            None => false,
        },
        Op::Type { run, pos, text: t } => match pick(&shape(&replica.page).0, u16::from(run)) {
            Some(run) => {
                let len = run.len_unicode();
                let insert = if text(t).is_empty() { "x" } else { text(t) };
                run.insert(usize::from(pos) % (len + 1), insert).unwrap();
                true
            }
            None => false,
        },
        Op::Erase { run, pos, len } => match pick(&shape(&replica.page).0, u16::from(run)) {
            Some(run) if run.len_unicode() > 0 => {
                let total = run.len_unicode();
                let start = usize::from(pos) % total;
                let count = 1 + usize::from(len) % (total - start);
                run.delete(start, count).unwrap();
                true
            }
            _ => false,
        },
        Op::Mark {
            run,
            start,
            len,
            mark: m,
        }
        | Op::Unmark {
            run,
            start,
            len,
            mark: m,
        } => match pick(&shape(&replica.page).0, u16::from(run)) {
            Some(run) if run.len_unicode() > 0 => {
                let total = run.len_unicode();
                let from = usize::from(start) % total;
                let to = from + 1 + usize::from(len) % (total - from);
                let (name, value) = mark(m);
                if matches!(op, Op::Mark { .. }) {
                    run.mark(from..to, name, value).unwrap();
                } else {
                    run.unmark(from..to, name).unwrap();
                }
                true
            }
            _ => false,
        },
        Op::RawRef { block, pos, target } => {
            match pick(&inline_blocks(&replica.page), u16::from(block)) {
                Some(map) => {
                    let list = children(&map).unwrap();
                    let at = usize::from(pos) % (list.len() + 1);
                    let id = new_id(replica, index);
                    // Exactly what the binding's `createLoroMap` writes.
                    let node = list.insert_container(at, LoroMap::new()).unwrap();
                    node.insert("nodeName", "pageRef").unwrap();
                    let attributes = node.insert_container("attributes", LoroMap::new()).unwrap();
                    attributes.insert("id", id.to_string()).unwrap();
                    attributes
                        .insert("pageId", target_page(target).to_string())
                        .unwrap();
                    node.insert_container("children", LoroList::new()).unwrap();
                    true
                }
                None => false,
            }
        }
        Op::RemoveAtom { block, pick: p } => {
            let lists: Vec<(LoroList, Vec<usize>)> = inline_blocks(&replica.page)
                .iter()
                .filter_map(|map| {
                    let list = children(map)?;
                    let atoms: Vec<usize> = (0..list.len())
                        .filter(|i| {
                            matches!(
                                list.get(*i),
                                Some(ValueOrContainer::Container(Container::Map(_)))
                            )
                        })
                        .collect();
                    (!atoms.is_empty()).then_some((list, atoms))
                })
                .collect();
            match pick(&lists, u16::from(block)) {
                Some((list, atoms)) => {
                    list.delete(atoms[usize::from(p) % atoms.len()], 1).unwrap();
                    true
                }
                None => false,
            }
        }
    };
    replica.page.doc().inner().commit();
    if let Some(blob) = replica
        .page
        .doc()
        .export_updates_since(Some(&replica.exported))
        .unwrap()
    {
        replica.outbox.push(blob);
        replica.exported = replica.page.doc().version();
    }
    changed
}

fn receive(to: &Replica, blob: &[u8]) {
    to.page.doc().import(blob).unwrap();
}

fn check(replica: &Replica, context: &str) {
    let problems = replica.page.check_invariants();
    assert!(problems.is_empty(), "{context}: {problems:#?}");
}

fn origin_snapshot() -> Vec<u8> {
    let json: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
    PageDoc::from_portable(Doc::new(DocKind::Page, 0x0e1), &json)
        .unwrap()
        .doc()
        .export_snapshot()
        .unwrap()
}

fn run(replicas: u8, steps: &[Step], redeliver: &[u8]) -> Stats {
    let mut stats = Stats::default();
    let snapshot = origin_snapshot();
    let mut all: Vec<Replica> = (0..replicas)
        .map(|i| {
            let page = PageDoc::from_doc(
                Doc::load(DocKind::Page, 0x100 + u64::from(i), Some(&snapshot), &[]).unwrap(),
            )
            .unwrap();
            let exported = page.doc().version();
            Replica {
                page,
                outbox: Vec::new(),
                exported,
                created: 0,
            }
        })
        .collect();

    for step in steps {
        match step {
            Step::Local { replica, op } => {
                stats.attempted += 1;
                if apply(&mut all[usize::from(*replica)], *replica, op) {
                    stats.applied += 1;
                }
                check(&all[usize::from(*replica)], &format!("after {step:?}"));
            }
            Step::Deliver { from, to, picks } => {
                let (from, to) = (usize::from(*from), usize::from(*to));
                if from == to || all[from].outbox.is_empty() {
                    continue;
                }
                for pick in picks {
                    let blob =
                        all[from].outbox[usize::from(*pick) % all[from].outbox.len()].clone();
                    receive(&all[to], &blob);
                    check(&all[to], &format!("after delivering to {to}"));
                }
            }
            Step::CatchUp { from, to } | Step::Full { from, to } => {
                let (from, to) = (usize::from(*from), usize::from(*to));
                if from == to {
                    continue;
                }
                let since = match step {
                    Step::CatchUp { .. } => Some(all[to].page.doc().version()),
                    _ => None,
                };
                if let Some(blob) = all[from]
                    .page
                    .doc()
                    .export_updates_since(since.as_ref())
                    .unwrap()
                {
                    receive(&all[to], &blob);
                    check(&all[to], &format!("after {step:?}"));
                }
            }
        }
    }

    // Final exchange: every replica's whole history to every other replica, then
    // stale per-operation blobs once more (already known: must be no-ops).
    let histories: Vec<Vec<u8>> = all
        .iter()
        .map(|r| r.page.doc().export_updates_since(None).unwrap().unwrap())
        .collect();
    for (to, replica) in all.iter().enumerate() {
        for (from, history) in histories.iter().enumerate() {
            if from != to {
                receive(replica, history);
            }
        }
    }
    let blobs: Vec<Vec<u8>> = all.iter().flat_map(|r| r.outbox.iter().cloned()).collect();
    if !blobs.is_empty() {
        for (i, pick) in redeliver.iter().enumerate() {
            receive(
                &all[i % all.len()],
                &blobs[usize::from(*pick) % blobs.len()],
            );
        }
    }

    let expected = all[0].page.to_portable();
    let markdown = all[0].page.to_markdown(&|_| None);
    let ids = all[0].page.block_ids();
    let version = all[0].page.doc().version();
    for (i, replica) in all.iter().enumerate() {
        check(replica, &format!("replica {i} at the end"));
        assert_eq!(replica.page.to_portable(), expected, "replica {i} diverged");
        assert_eq!(replica.page.to_markdown(&|_| None), markdown, "replica {i}");
        assert_eq!(replica.page.block_ids(), ids, "replica {i}");
        assert_eq!(
            replica.page.doc().version(),
            version,
            "replica {i} misses changes"
        );
    }
    let rebuilt = PageDoc::from_portable(Doc::new(DocKind::Page, 0x999), &expected).unwrap();
    assert_eq!(
        rebuilt.to_portable(),
        expected,
        "portable JSON is not an identity"
    );
    assert!(rebuilt.check_invariants().is_empty());
    stats
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn replicas_converge_whatever_the_delivery((replicas, steps, redeliver) in script()) {
        run(replicas, &steps, &redeliver);
    }
}

/// Guards the strength of the property test: most generated operations must really
/// change the document (not be refused or find nothing to act on).
#[test]
fn generated_scripts_mostly_apply_their_operations() {
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(11),
        failure_persistence: None,
        ..Config::default()
    });
    let mut total = Stats::default();
    for _ in 0..32 {
        let (replicas, steps, redeliver) = script().new_tree(&mut runner).unwrap().current();
        let stats = run(replicas, &steps, &redeliver);
        total.attempted += stats.attempted;
        total.applied += stats.applied;
    }
    assert!(total.attempted > 200, "{total:?}");
    assert!(total.applied * 10 >= total.attempted * 8, "{total:?}");
}

/// A fixed interleaving where every replica edits the same paragraph and the same
/// text run concurrently (set text against typing, marks and new references).
#[test]
fn a_hand_written_concurrent_script_converges() {
    use Op::*;
    use Step::*;
    let steps = vec![
        Local {
            replica: 0,
            op: SetText { block: 1, text: 2 },
        },
        Local {
            replica: 1,
            op: Type {
                run: 1,
                pos: 3,
                text: 3,
            },
        },
        Local {
            replica: 2,
            op: Mark {
                run: 1,
                start: 0,
                len: 40,
                mark: 0,
            },
        },
        Local {
            replica: 1,
            op: RawRef {
                block: 1,
                pos: 1,
                target: 1,
            },
        },
        Deliver {
            from: 0,
            to: 1,
            picks: vec![0, 0],
        },
        Local {
            replica: 2,
            op: Delete { block: 1 },
        },
        Local {
            replica: 0,
            op: PageRef {
                block: 1,
                target: 2,
                with_block: true,
            },
        },
        CatchUp { from: 2, to: 0 },
        Local {
            replica: 1,
            op: Insert {
                after: Some(0),
                kind: 4,
                text: 5,
            },
        },
        Full { from: 1, to: 2 },
        Local {
            replica: 2,
            op: RemoveAtom { block: 0, pick: 0 },
        },
    ];
    let stats = run(3, &steps, &[0, 3, 5, 1]);
    assert_eq!(stats.attempted, 8);
}

/// Exchanges everything both ways.
fn sync_pair(a: &PageDoc, b: &PageDoc) {
    let to_b = a
        .doc()
        .export_updates_since(Some(&b.doc().version()))
        .unwrap();
    let to_a = b
        .doc()
        .export_updates_since(Some(&a.doc().version()))
        .unwrap();
    if let Some(blob) = to_a {
        a.doc().import(&blob).unwrap();
    }
    if let Some(blob) = to_b {
        b.doc().import(&blob).unwrap();
    }
    assert_eq!(a.doc().version(), b.doc().version());
}

fn paragraph(id: BlockId, text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "paragraph",
        "attrs": { "id": id.to_string() },
        "content": [{ "type": "text", "text": text }]
    })
}

/// Two devices create the document of the same page offline (`PageDoc::new`, each
/// with its own first paragraph, typed into), then sync.
///
/// Resulting shape: TWO paragraphs, both texts visible. The root's `children` is a
/// mergeable list whose id derives from the root and the key, so both replicas
/// inserted into the same list; the two concurrent inserts at position 0 are ordered
/// by the list CRDT, the same way on every replica.
#[test]
fn concurrent_creations_merge_both_first_paragraphs() {
    let first_a = BlockId::from_random_bytes([0xa1; 16]);
    let first_b = BlockId::from_random_bytes([0xb2; 16]);
    let a = PageDoc::new(Doc::new(DocKind::Page, 1), first_a).unwrap();
    let b = PageDoc::new(Doc::new(DocKind::Page, 2), first_b).unwrap();
    a.set_block_text(first_a, "typed on a").unwrap();
    b.set_block_text(first_b, "typed on b").unwrap();
    sync_pair(&a, &b);

    let expected = serde_json::json!({
        "type": "doc",
        "content": [paragraph(first_a, "typed on a"), paragraph(first_b, "typed on b")]
    });
    assert_eq!(a.to_portable(), expected);
    assert_eq!(b.to_portable(), expected);
    assert_eq!(a.block_ids(), vec![first_a, first_b]);
    assert!(a.check_invariants().is_empty());
    assert!(b.check_invariants().is_empty());

    // Editing after the merge works on both sides and converges.
    let third = BlockId::from_random_bytes([0xc3; 16]);
    a.insert_block(
        Some(first_b),
        BlockKind::Paragraph,
        "after the merge",
        third,
    )
    .unwrap();
    b.set_block_text(first_b, "typed on b, edited").unwrap();
    b.delete_block(first_a).unwrap();
    sync_pair(&a, &b);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(
        a.to_portable()["content"],
        serde_json::json!([
            paragraph(first_b, "typed on b, edited"),
            paragraph(third, "after the merge")
        ])
    );
}

/// The other ways of creating the root merge the same way: semantic inserts into
/// documents that have no root yet, and imports of portable JSON.
#[test]
fn every_way_of_creating_the_root_merges() {
    let x = BlockId::from_random_bytes([0x11; 16]);
    let y = BlockId::from_random_bytes([0x22; 16]);
    let z = BlockId::from_random_bytes([0x33; 16]);
    let a = PageDoc::from_doc(Doc::new(DocKind::Page, 1)).unwrap();
    let b = PageDoc::from_doc(Doc::new(DocKind::Page, 2)).unwrap();
    let json = serde_json::json!({ "type": "doc", "content": [paragraph(z, "imported")] });
    let c = PageDoc::from_portable(Doc::new(DocKind::Page, 3), &json).unwrap();
    a.insert_block(None, BlockKind::Paragraph, "from a", x)
        .unwrap();
    b.insert_block(None, BlockKind::Heading(1), "from b", y)
        .unwrap();
    sync_pair(&a, &b);
    sync_pair(&a, &c);
    sync_pair(&b, &c);
    let mut ids = a.block_ids();
    ids.sort_unstable();
    let mut expected = vec![x, y, z];
    expected.sort_unstable();
    assert_eq!(ids, expected, "all three first blocks survive");
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(a.to_portable(), c.to_portable());
    for page in [&a, &b, &c] {
        assert!(page.check_invariants().is_empty());
    }
}

/// A root written the old way (regular `attributes`/`children`, as the binding's
/// `getOrCreateContainer` or an older TypeScript mirror does) and a mergeable root
/// created concurrently cannot merge: the root fields resolve last-writer-wins (equal
/// lamports, the higher peer wins), as every root did before the change. Both
/// replicas still converge.
#[test]
fn a_regular_root_and_a_mergeable_root_created_concurrently_converge() {
    let old = Doc::new(DocKind::Page, 1);
    {
        let root = old.inner().get_map("doc");
        root.insert("nodeName", "doc").unwrap();
        root.insert_container("attributes", LoroMap::new()).unwrap();
        let children = root.insert_container("children", LoroList::new()).unwrap();
        let node = children.insert_container(0, LoroMap::new()).unwrap();
        node.insert("nodeName", "paragraph").unwrap();
        let attributes = node.insert_container("attributes", LoroMap::new()).unwrap();
        attributes
            .insert("id", BlockId::from_random_bytes([0x44; 16]).to_string())
            .unwrap();
        node.insert_container("children", LoroList::new()).unwrap();
        old.inner().commit();
    }
    let old = PageDoc::from_doc(old).unwrap();
    let new = PageDoc::new(
        Doc::new(DocKind::Page, 2),
        BlockId::from_random_bytes([0x55; 16]),
    )
    .unwrap();
    sync_pair(&old, &new);
    assert_eq!(old.to_portable(), new.to_portable());
    assert_eq!(
        old.block_ids(),
        vec![BlockId::from_random_bytes([0x55; 16])],
        "peer 2 (mergeable) wrote the surviving fields"
    );
    assert!(old.check_invariants().is_empty());
}
