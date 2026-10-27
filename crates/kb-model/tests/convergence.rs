//! Property tests: replicas of one structure document converge.
//!
//! 2 to 4 replicas start from one snapshot, apply random scripts of operations
//! (create, rename, move, trash, restore, purge, and the schema and values of typed
//! properties, ADR-0026) with arbitrary interleavings, and
//! exchange update blobs in random order and chunking (single-operation deltas,
//! catch-up deltas, whole histories, duplicates, deltas whose dependencies have not
//! arrived yet). After a final full exchange every replica must have the same
//! portable JSON, the same version and no invariant violation
//! (`docs/data-model/domain-model.md`: acyclic, one parent, unique ids, live pages
//! under live parents, trashed subtrees outside the live tree, tombstoned ids
//! absent). Invariants are also checked after every step; the two findings that a
//! concurrent delete makes legitimate (a value of a property that is gone, a value naming
//! an option that is gone: kept on purpose, ADR-0026) are not counted.
//!
//! Deterministic by default (fixed seed, 64 cases). Explore further with
//! `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=<n> cargo test -p kb-model --test convergence`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_doc::{Doc, DocKind, Version};
use kb_model::{ModelError, SpaceMeta, StructureDoc};
use kb_types::{
    AccountId, AttachmentId, ChecklistItem, ChecklistItemId, OptionColor, OptionId, PageId,
    PropertyId, PropertyKind, PropertyValue, SpaceId,
};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, RngSeed, TestRunner};

#[derive(Debug, Clone)]
enum Op {
    Create {
        parent: Option<u8>,
        index: Option<u8>,
    },
    Rename {
        page: u8,
        title: u8,
    },
    Move {
        page: u8,
        parent: Option<u8>,
        index: u8,
    },
    Trash {
        page: u8,
    },
    Restore {
        page: u8,
    },
    Purge {
        page: u8,
    },
    /// Creates a property of the `kind`th kind.
    CreateProperty {
        kind: u8,
    },
    RenameProperty {
        property: u8,
        name: u8,
    },
    DeleteProperty {
        property: u8,
    },
    MoveProperty {
        property: u8,
        before: Option<u8>,
    },
    AddOption {
        property: u8,
        color: u8,
    },
    UpdateOption {
        property: u8,
        option: u8,
        name: u8,
    },
    DeleteOption {
        property: u8,
        option: u8,
    },
    /// Sets (or, for `value == 0`, clears) a page's value of a property; the value is built
    /// from the property's kind and options.
    SetValue {
        page: u8,
        property: u8,
        value: u8,
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
        3 => (proptest::option::of(any::<u8>()), proptest::option::weighted(0.5, any::<u8>()))
            .prop_map(|(parent, index)| Op::Create { parent, index }),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(page, title)| Op::Rename { page, title }),
        3 => (any::<u8>(), proptest::option::of(any::<u8>()), any::<u8>())
            .prop_map(|(page, parent, index)| Op::Move { page, parent, index }),
        2 => any::<u8>().prop_map(|page| Op::Trash { page }),
        2 => any::<u8>().prop_map(|page| Op::Restore { page }),
        1 => any::<u8>().prop_map(|page| Op::Purge { page }),
        2 => any::<u8>().prop_map(|kind| Op::CreateProperty { kind }),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(property, name)| Op::RenameProperty { property, name }),
        1 => any::<u8>().prop_map(|property| Op::DeleteProperty { property }),
        1 => (any::<u8>(), proptest::option::of(any::<u8>()))
            .prop_map(|(property, before)| Op::MoveProperty { property, before }),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(property, color)| Op::AddOption { property, color }),
        1 => (any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(property, option, name)| Op::UpdateOption { property, option, name }),
        1 => (any::<u8>(), any::<u8>()).prop_map(|(property, option)| Op::DeleteOption { property, option }),
        5 => (any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(page, property, value)| Op::SetValue { page, property, value }),
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
        rng_seed: RngSeed::Fixed(env("PROPTEST_RNG_SEED").unwrap_or(0x5eed_2026)),
        failure_persistence: None,
        ..Config::default()
    }
}

/// What a replica made so far: ids stay unique across replicas.
struct Replica {
    doc: StructureDoc,
    /// Blobs exported after each local operation (they may also carry changes this
    /// replica had imported since its previous export).
    outbox: Vec<Vec<u8>>,
    exported: Version,
    created: u16,
    /// Properties and options this replica created.
    made: u16,
}

fn property_id(replica: u8, n: u16) -> PropertyId {
    let [hi, lo] = n.to_be_bytes();
    PropertyId::from_random_bytes([
        0xd0, replica, hi, lo, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, replica,
    ])
}

fn option_id(replica: u8, n: u16) -> OptionId {
    let [hi, lo] = n.to_be_bytes();
    OptionId::from_random_bytes([
        0xe0, replica, hi, lo, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, replica,
    ])
}

fn page_id(replica: u8, n: u16) -> PageId {
    let [hi, lo] = n.to_be_bytes();
    PageId::from_random_bytes([
        0xc0, replica, hi, lo, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, replica,
    ])
}

fn pick(ids: &[PageId], n: u8) -> Option<PageId> {
    (!ids.is_empty()).then(|| ids[usize::from(n) % ids.len()])
}

/// Operations attempted and actually applied (`Ok` with affected pages).
#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    attempted: usize,
    applied: usize,
}

/// Whether the operation changed something; refusals must be the domain errors.
fn applied(result: Result<Vec<PageId>, ModelError>, step: &Op) -> bool {
    match result {
        Ok(affected) => !affected.is_empty(),
        Err(
            ModelError::PageNotFound
            | ModelError::PageTrashed
            | ModelError::PageNotTrashed
            | ModelError::ParentUnavailable
            | ModelError::CyclicMove
            | ModelError::DuplicatePage,
        ) => false,
        Err(other) => panic!("{step:?} failed unexpectedly: {other}"),
    }
}

/// Whether a property operation changed something; refusals must be the domain errors.
fn applied_property<T>(result: Result<T, ModelError>, step: &Op) -> bool {
    match result {
        Ok(_) => true,
        Err(
            ModelError::PropertyNotFound
            | ModelError::OptionNotFound
            | ModelError::PageNotFound
            | ModelError::InvalidProperty { .. },
        ) => false,
        Err(other) => panic!("{step:?} failed unexpectedly: {other}"),
    }
}

/// A property operation on the replica's current schema.
fn apply_property(replica: &mut Replica, index: u8, op: &Op, now: i64) -> bool {
    let doc = &mut replica.doc;
    let schema = doc.property_schema();
    let pick_property = |n: u8| schema[usize::from(n) % schema.len()].clone();
    match *op {
        Op::CreateProperty { kind } => {
            replica.made += 1;
            let kind = PropertyKind::ALL[usize::from(kind) % PropertyKind::ALL.len()];
            let mut made = replica.made;
            let mut new_option = || {
                made += 1;
                option_id(index, made)
            };
            // Every other relation is a two-way pair (its back property takes the next id).
            let result = if kind == PropertyKind::Relation && replica.made.is_multiple_of(2) {
                let back = property_id(index, replica.made.wrapping_add(500));
                doc.create_relation_pair(
                    property_id(index, replica.made),
                    back,
                    &format!("pair {index}-{}", replica.made),
                    &format!("pair {index}-{} (back)", replica.made),
                    now,
                )
            } else {
                doc.create_property(
                    property_id(index, replica.made),
                    &format!("prop {index}-{}", replica.made),
                    kind,
                    &mut new_option,
                    now,
                )
            };
            replica.made = made;
            applied_property(result, op)
        }
        Op::RenameProperty { property, name } => {
            let target = pick_property(property);
            applied_property(
                doc.rename_property(target.id, &format!("name {name}"), now),
                op,
            )
        }
        Op::DeleteProperty { property } => {
            let target = pick_property(property);
            applied_property(doc.delete_property(target.id), op)
        }
        Op::MoveProperty { property, before } => {
            let target = pick_property(property);
            let before = before.map(|n| pick_property(n).id);
            applied_property(doc.move_property(target.id, before), op)
        }
        Op::AddOption { property, color } => {
            replica.made += 1;
            let target = pick_property(property);
            let color = OptionColor::ALL[usize::from(color) % OptionColor::ALL.len()];
            applied_property(
                doc.add_option(
                    target.id,
                    option_id(index, replica.made),
                    &format!("option {index}-{}", replica.made),
                    color,
                    None,
                    now,
                ),
                op,
            )
        }
        Op::UpdateOption {
            property,
            option,
            name,
        } => {
            let target = pick_property(property);
            match target.options.is_empty() {
                true => false,
                false => {
                    let chosen = &target.options[usize::from(option) % target.options.len()];
                    applied_property(
                        doc.update_option(
                            target.id,
                            chosen.id,
                            Some(&format!("renamed {name}")),
                            None,
                            None,
                        ),
                        op,
                    )
                }
            }
        }
        Op::DeleteOption { property, option } => {
            let target = pick_property(property);
            match target.options.is_empty() {
                true => false,
                false => {
                    let chosen = &target.options[usize::from(option) % target.options.len()];
                    applied_property(doc.delete_option(target.id, chosen.id, now), op)
                }
            }
        }
        Op::SetValue {
            page,
            property,
            value,
        } => {
            let pages = doc.all_page_ids();
            let Some(page) = pick(&pages, page) else {
                return false;
            };
            let target = pick_property(property);
            let value = (value != 0).then(|| match target.kind {
                // Some pages of the replica (and, now and then, one it does not know).
                PropertyKind::Relation => PropertyValue::Relation(
                    (0..=(value % 3))
                        .filter_map(|step| pick(&pages, value.wrapping_add(step.wrapping_mul(7))))
                        .fold(Vec::new(), |mut listed, page| {
                            if !listed.contains(&page) {
                                listed.push(page);
                            }
                            listed
                        }),
                ),
                _ => value_for(&target, value),
            });
            applied_property(
                doc.set_page_property(page, target.id, value, now)
                    .map(|changed| changed.len()),
                op,
            )
        }
        _ => false,
    }
}

/// A valid value for a property, chosen by `n`.
fn value_for(property: &kb_types::PropertyDef, n: u8) -> PropertyValue {
    let option = |n: u8| {
        property
            .options
            .get(usize::from(n) % property.options.len().max(1))
    };
    match property.kind {
        PropertyKind::Text => PropertyValue::Text(format!("text {n}")),
        PropertyKind::Number => PropertyValue::Number(f64::from(n)),
        PropertyKind::Checkbox => PropertyValue::Checkbox(n.is_multiple_of(2)),
        PropertyKind::Url => PropertyValue::Url(format!("https://example.com/{n}")),
        PropertyKind::Email => PropertyValue::Email(format!("u{n}@example.com")),
        PropertyKind::Phone => PropertyValue::Phone(format!("+1 555 {n:04}")),
        PropertyKind::Date => PropertyValue::Date(kb_types::DateValue {
            date: format!("2026-10-{:02}", 1 + n % 28),
            time: n.is_multiple_of(3).then(|| "09:30".to_owned()),
        }),
        // An option the property lacks (none yet) makes the write refused: a refusal, not a bug.
        PropertyKind::Select => {
            PropertyValue::Select(option(n).map_or(option_id(0xff, 0), |o| o.id))
        }
        PropertyKind::Status => {
            PropertyValue::Status(option(n).map_or(option_id(0xff, 0), |o| o.id))
        }
        PropertyKind::MultiSelect => {
            let mut ids: Vec<OptionId> = Vec::new();
            for step in 0..=(n % 3) {
                if let Some(o) = option(n.wrapping_add(step))
                    && !ids.contains(&o.id)
                {
                    ids.push(o.id);
                }
            }
            PropertyValue::MultiSelect(ids)
        }
        // Attachments are not checked when a value is written: any ids do.
        PropertyKind::Files => PropertyValue::Files(
            (0..=(n % 3))
                .map(|step| {
                    AttachmentId::from_random_bytes([
                        0xa5,
                        n.wrapping_add(step),
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                        13,
                        step,
                    ])
                })
                .collect(),
        ),
        PropertyKind::Place => PropertyValue::Place(kb_types::PlaceValue {
            name: format!("place {n}"),
            lat: n.is_multiple_of(2).then(|| f64::from(n) / 4.0),
            lon: n.is_multiple_of(2).then(|| f64::from(n) / 2.0),
        }),
        // Relations are built from the replica's pages by the caller; a rollup and a formula are computed.
        PropertyKind::Relation | PropertyKind::Rollup | PropertyKind::Formula => {
            PropertyValue::Relation(Vec::new())
        }
        // Membership of the space is the core's rule, not the model's: any distinct accounts do.
        PropertyKind::Person => PropertyValue::Person(
            (0..=(n % 3))
                .map(|step| {
                    AccountId::from_bytes([
                        0xac,
                        n.wrapping_add(step),
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                        13,
                        step,
                    ])
                })
                .collect(),
        ),
        // Computed kinds take no value: the write is refused, like an unknown option.
        PropertyKind::CreatedTime | PropertyKind::LastEditedTime => {
            PropertyValue::CreatedTime(i64::from(n))
        }
        PropertyKind::CreatedBy | PropertyKind::LastEditedBy => {
            PropertyValue::CreatedBy(kb_types::DeviceId::from_random_bytes([n; 16]))
        }
        PropertyKind::Checklist => PropertyValue::Checklist(
            (0..=(n % 4))
                .map(|step| ChecklistItem {
                    id: ChecklistItemId::from_random_bytes([
                        0xc5, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, step,
                    ]),
                    text: format!("item {n}.{step}"),
                    done: (n.wrapping_add(step)).is_multiple_of(2),
                })
                .collect(),
        ),
    }
}

fn apply(replica: &mut Replica, index: u8, op: &Op, now: i64) -> bool {
    if matches!(
        op,
        Op::CreateProperty { .. }
            | Op::RenameProperty { .. }
            | Op::DeleteProperty { .. }
            | Op::MoveProperty { .. }
            | Op::AddOption { .. }
            | Op::UpdateOption { .. }
            | Op::DeleteOption { .. }
            | Op::SetValue { .. }
    ) {
        let changed = apply_property(replica, index, op, now);
        export(replica);
        return changed;
    }
    let doc = &mut replica.doc;
    let pages = doc.all_page_ids();
    let result = match *op {
        Op::Create { parent, index: at } => {
            replica.created += 1;
            let id = page_id(index, replica.created);
            let parent = parent.and_then(|p| pick(&pages, p));
            let at = at.map(|i| usize::from(i % 5));
            doc.create_page(
                id,
                parent,
                at,
                &format!("p{index}-{}", replica.created),
                now,
            )
        }
        Op::Rename { page, title } => match pick(&pages, page) {
            Some(id) => doc.rename_page(id, &format!("t{title}"), now),
            None => Ok(Vec::new()),
        },
        Op::Move {
            page,
            parent,
            index: at,
        } => match pick(&pages, page) {
            Some(id) => {
                let parent = parent.and_then(|p| pick(&pages, p));
                doc.move_page(id, parent, usize::from(at % 6), now)
            }
            None => Ok(Vec::new()),
        },
        Op::Trash { page } => match pick(&pages, page) {
            Some(id) => doc.trash_page(id, now),
            None => Ok(Vec::new()),
        },
        Op::Restore { page } => {
            let trashed: Vec<PageId> = doc.trashed_pages().iter().map(|t| t.page.id).collect();
            // Mostly restorable pages, sometimes any page (expected refusals).
            let candidates = if page % 4 == 0 || trashed.is_empty() {
                &pages
            } else {
                &trashed
            };
            match pick(candidates, page) {
                Some(id) => doc.restore_page(id, now),
                None => Ok(Vec::new()),
            }
        }
        Op::Purge { page } => match pick(&pages, page) {
            Some(id) => doc.purge_page(id, now),
            None => Ok(Vec::new()),
        },
        _ => Ok(Vec::new()),
    };
    let changed = applied(result, op);
    export(replica);
    changed
}

/// Puts what the replica did since its last export into its outbox.
fn export(replica: &mut Replica) {
    if let Some(blob) = replica
        .doc
        .doc()
        .export_updates_since(Some(&replica.exported))
        .unwrap()
    {
        replica.outbox.push(blob);
        replica.exported = replica.doc.doc().version();
    }
}

fn receive(to: &mut Replica, blob: &[u8]) {
    to.doc.doc().import(blob).unwrap();
    to.doc.refresh().unwrap();
}

fn check(replica: &Replica, context: &str) {
    let mut problems = replica.doc.check_invariants();
    // Kept on purpose after a concurrent delete (ADR-0026).
    problems.retain(|p| !p.contains("unknown property") && !p.contains("unknown option"));
    assert!(problems.is_empty(), "{context}: {problems:#?}");
}

fn origin() -> StructureDoc {
    let mut doc = StructureDoc::new(
        Doc::new(DocKind::Structure, 0x0e1),
        SpaceMeta {
            id: SpaceId::from_random_bytes([0x5a; 16]),
            name: "Shared".into(),
            icon: None,
            created_at: 1,
        },
    );
    let a = page_id(0xff, 1);
    let b = page_id(0xff, 2);
    doc.create_page(a, None, None, "A", 1).unwrap();
    doc.create_page(b, Some(a), None, "B", 2).unwrap();
    doc.create_page(page_id(0xff, 3), None, None, "C", 3)
        .unwrap();
    doc.create_page(page_id(0xff, 4), Some(b), None, "D", 4)
        .unwrap();
    doc
}

fn run(replicas: u8, steps: &[Step], redeliver: &[u8]) -> Stats {
    let mut stats = Stats::default();
    let snapshot = origin().doc().export_snapshot().unwrap();
    let mut all: Vec<Replica> = (0..replicas)
        .map(|i| {
            let doc = StructureDoc::from_doc(
                Doc::load(
                    DocKind::Structure,
                    0x100 + u64::from(i),
                    Some(&snapshot),
                    &[],
                )
                .unwrap(),
            )
            .unwrap();
            let exported = doc.doc().version();
            Replica {
                doc,
                outbox: Vec::new(),
                exported,
                created: 0,
                made: 0,
            }
        })
        .collect();

    for (n, step) in steps.iter().enumerate() {
        let now = 1_000 + 10 * i64::try_from(n).unwrap();
        match step {
            Step::Local { replica, op } => {
                stats.attempted += 1;
                if apply(&mut all[usize::from(*replica)], *replica, op, now) {
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
                    receive(&mut all[to], &blob);
                    check(&all[to], &format!("after delivering to {to}"));
                }
            }
            Step::CatchUp { from, to } | Step::Full { from, to } => {
                let (from, to) = (usize::from(*from), usize::from(*to));
                if from == to {
                    continue;
                }
                let since = match step {
                    Step::CatchUp { .. } => Some(all[to].doc.doc().version()),
                    _ => None,
                };
                if let Some(blob) = all[from]
                    .doc
                    .doc()
                    .export_updates_since(since.as_ref())
                    .unwrap()
                {
                    receive(&mut all[to], &blob);
                    check(&all[to], &format!("after {step:?}"));
                }
            }
        }
    }

    // Final exchange: every replica's whole history to every other replica, then
    // stale per-operation blobs once more (already known: must be no-ops).
    let histories: Vec<Vec<u8>> = all
        .iter()
        .map(|r| r.doc.doc().export_updates_since(None).unwrap().unwrap())
        .collect();
    for (to, replica) in all.iter_mut().enumerate() {
        for (from, history) in histories.iter().enumerate() {
            if from != to {
                receive(replica, history);
            }
        }
    }
    let blobs: Vec<Vec<u8>> = all.iter().flat_map(|r| r.outbox.iter().cloned()).collect();
    if !blobs.is_empty() {
        for (i, pick) in redeliver.iter().enumerate() {
            let to = i % all.len();
            receive(&mut all[to], &blobs[usize::from(*pick) % blobs.len()]);
        }
    }

    let expected = all[0].doc.to_portable();
    let version = all[0].doc.doc().version();
    for (i, replica) in all.iter().enumerate() {
        check(replica, &format!("replica {i} at the end"));
        assert_eq!(replica.doc.to_portable(), expected, "replica {i} diverged");
        assert_eq!(
            replica.doc.doc().version(),
            version,
            "replica {i} misses changes"
        );
        assert_eq!(
            usize::try_from(replica.doc.live_page_count()).unwrap() + trashed_total(&replica.doc),
            replica.doc.all_page_ids().len()
        );
    }
    stats
}

fn trashed_total(doc: &StructureDoc) -> usize {
    doc.trashed_pages()
        .iter()
        .map(|t| 1 + usize::try_from(t.descendant_count).unwrap())
        .sum()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn replicas_converge_whatever_the_delivery((replicas, steps, redeliver) in script()) {
        run(replicas, &steps, &redeliver);
    }
}

/// Guards the strength of the property test: most generated operations must
/// really change the document (not be refused or be no-ops).
#[test]
fn generated_scripts_mostly_apply_their_operations() {
    let mut runner = TestRunner::new(Config {
        rng_seed: RngSeed::Fixed(7),
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
    // About three quarters apply with the default strategy weights.
    assert!(total.applied * 10 >= total.attempted * 6, "{total:?}");
}

/// A fixed, adversarial interleaving that exercises every operation against
/// concurrent moves, trash and purge (also documents the expected outcome shape).
#[test]
fn a_hand_written_concurrent_script_converges() {
    use Op::*;
    use Step::*;
    let steps = vec![
        Local {
            replica: 0,
            op: Move {
                page: 0,
                parent: Some(2),
                index: 0,
            },
        },
        Local {
            replica: 1,
            op: Move {
                page: 2,
                parent: Some(0),
                index: 0,
            },
        },
        Local {
            replica: 2,
            op: Purge { page: 1 },
        },
        Local {
            replica: 1,
            op: Create {
                parent: Some(1),
                index: Some(0),
            },
        },
        Local {
            replica: 0,
            op: Trash { page: 3 },
        },
        Deliver {
            from: 0,
            to: 1,
            picks: vec![1, 0, 1],
        },
        Local {
            replica: 1,
            op: Restore { page: 1 },
        },
        Local {
            replica: 2,
            op: Rename { page: 0, title: 7 },
        },
        CatchUp { from: 2, to: 0 },
        Local {
            replica: 0,
            op: Create {
                parent: None,
                index: Some(0),
            },
        },
        Full { from: 1, to: 2 },
        Local {
            replica: 2,
            op: Move {
                page: 3,
                parent: None,
                index: 1,
            },
        },
    ];
    let stats = run(3, &steps, &[0, 3, 5, 1]);
    assert_eq!(stats.attempted, 9);
}
