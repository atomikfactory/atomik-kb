//! Unit tests of the typed documents. Shared helpers live here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod account;
mod appearance;
mod append;
mod attachments;
mod authorship;
mod bulk;
mod comments;
mod concurrency;
mod cover;
mod duplicate;
mod favorites;
mod formulas;
mod links;
mod markdown_file;
mod moves;
mod page;
mod page_bulk;
mod person;
mod properties;
mod relations;
mod remap;
mod reminder;
mod reorder;
mod sharing;
mod structure;
mod template_button;
mod template_create;
mod templates;
mod undo;

use kb_doc::{Doc, DocKind};
use kb_types::{PageId, PageNode, SpaceId};

use crate::{SpaceMeta, StructureDoc};

/// A deterministic page id (`n` distinct -> ids distinct).
pub(crate) fn pid(n: u8) -> PageId {
    PageId::from_random_bytes([n; 16])
}

/// A deterministic space id.
pub(crate) fn sid(n: u8) -> SpaceId {
    SpaceId::from_random_bytes([n; 16])
}

pub(crate) fn space_meta() -> SpaceMeta {
    SpaceMeta {
        id: sid(1),
        name: "Personal".into(),
        icon: None,
        created_at: 5,
    }
}

/// A new structure document with peer `peer`.
pub(crate) fn structure(peer: u64) -> StructureDoc {
    StructureDoc::new(Doc::new(DocKind::Structure, peer), space_meta())
}

/// A replica of `doc` (loaded from its snapshot) with peer `peer`.
pub(crate) fn replica(doc: &StructureDoc, peer: u64) -> StructureDoc {
    let snapshot = doc.doc().export_snapshot().unwrap();
    StructureDoc::from_doc(Doc::load(DocKind::Structure, peer, Some(&snapshot), &[]).unwrap())
        .unwrap()
}

/// Sends what `to` lacks from `from`, then refreshes `to`.
pub(crate) fn send(from: &StructureDoc, to: &mut StructureDoc) {
    if let Some(blob) = from
        .doc()
        .export_updates_since(Some(&to.doc().version()))
        .unwrap()
    {
        to.doc().import(&blob).unwrap();
    }
    to.refresh().unwrap();
}

/// Exchanges updates both ways.
pub(crate) fn sync(a: &mut StructureDoc, b: &mut StructureDoc) {
    send(a, b);
    send(b, a);
}

/// Creates a page titled `title` with id `pid(n)` under `pid(parent)` (or the root),
/// appended, at time `n`.
pub(crate) fn add(doc: &mut StructureDoc, n: u8, parent: Option<u8>, title: &str) -> PageId {
    let id = pid(n);
    doc.create_page(id, parent.map(pid), None, title, i64::from(n))
        .unwrap();
    id
}

/// Titles of the live tree, depth-first, indented by two spaces per level.
pub(crate) fn outline(doc: &StructureDoc) -> String {
    fn walk(nodes: &[PageNode], depth: usize, out: &mut String) {
        for node in nodes {
            out.push_str(&"  ".repeat(depth));
            out.push_str(&node.page.title);
            out.push('\n');
            walk(&node.children, depth + 1, out);
        }
    }
    let mut out = String::new();
    walk(&doc.live_tree(), 0, &mut out);
    out
}

/// Titles of the top-level trashed pages, in listing order.
pub(crate) fn trash_titles(doc: &StructureDoc) -> Vec<String> {
    doc.trashed_pages()
        .into_iter()
        .map(|t| t.page.title)
        .collect()
}

pub(crate) fn title(doc: &StructureDoc, id: PageId) -> String {
    doc.page(id).expect("page exists").title
}

pub(crate) fn assert_healthy(doc: &StructureDoc) {
    let problems = doc.check_invariants();
    assert!(problems.is_empty(), "invariant violations: {problems:#?}");
}
