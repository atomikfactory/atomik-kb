//! Random sources through `plan` and `resolve`: whatever the files hold, the plan is consistent
//! and every resolved page loads into a page document that passes its invariants.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

use std::collections::{HashMap, HashSet};

use kb_types::{AttachmentId, OptionId, PageId, PropertyId};
use serde_json::Value;

use crate::resolve_tests::{Fake, resolve_and_load};
use crate::{ImportOptions, ImportValue, PageTarget, Plan, SourceFile, plan};

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n as u64).max(1)).unwrap()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NAMES: &[&str] = &[
    "Home",
    "home",
    "Notes",
    "Tasks",
    "Sub page",
    "Chapter 1",
    "Chapter 10",
    "Chapter 2",
    "a b",
    "日本",
    "Weird (name)",
    "x.y",
    "Untitled",
];
const IDS: &[&str] = &[
    "",
    " 0123456789abcdef0123456789abcdef",
    " fedcba9876543210fedcba9876543210",
];

fn random_path(rng: &mut Lcg, ext: &str) -> String {
    let depth = rng.below(4);
    let mut parts: Vec<String> = Vec::new();
    for _ in 0..depth {
        parts.push(format!("{}{}", rng.pick(NAMES), rng.pick(IDS)));
    }
    parts.push(format!("{}{}.{ext}", rng.pick(NAMES), rng.pick(IDS)));
    parts.join("/")
}

fn link_to(rng: &mut Lcg) -> String {
    let name = rng.pick(NAMES);
    let id = rng.pick(IDS);
    let dots = "../".repeat(rng.below(3));
    format!("{dots}{name}{id}.md").replace(' ', "%20")
}

fn random_content(rng: &mut Lcg, ext: &str) -> String {
    let mut out = String::new();
    match ext {
        "csv" => {
            out.push_str("Name,Status,N,When,Related\n");
            for _ in 0..rng.below(6) {
                out.push_str(&format!(
                    "{},{},{},{},{}\n",
                    rng.pick(NAMES),
                    ["Open", "Done", ""][rng.below(3)],
                    ["1", "2.5", "x", ""][rng.below(4)],
                    ["2026-10-07", "October 7, 2026", ""][rng.below(3)],
                    if rng.below(3) == 0 {
                        format!("{} ({})", rng.pick(NAMES), link_to(rng))
                    } else {
                        String::new()
                    }
                ));
            }
        }
        "html" => {
            out.push_str("<html><head><title>T</title></head><body>");
            for _ in 0..rng.below(6) {
                let piece = match rng.below(8) {
                    0 => format!("<p>text <a href=\"{}\">link</a></p>", link_to(rng)),
                    1 => "<ul><li>a</li><li>b<ul><li>c</li></ul></li></ul>".to_owned(),
                    2 => {
                        "<table><tr><th>a</th></tr><tr><td>b</td><td>c</td></tr></table>".to_owned()
                    }
                    3 => "<aside>💡 tip</aside>".to_owned(),
                    4 => "<details open><summary>s</summary><p>x</p></details>".to_owned(),
                    5 => "<img src=\"pic.png\" alt=\"pic\">".to_owned(),
                    6 => "<script>1</script>".to_owned(),
                    _ => "<h1>Heading</h1>".to_owned(),
                };
                out.push_str(&piece);
            }
            out.push_str("</body></html>");
        }
        "txt" => {
            for _ in 0..rng.below(5) {
                out.push_str("some text\nmore text\n\n");
            }
        }
        _ => {
            if rng.below(2) == 0 {
                out.push_str("---\n");
                out.push_str(&format!(
                    "tags: [{}, {}]\n",
                    rng.pick(NAMES).replace(' ', "-"),
                    "b"
                ));
                out.push_str(&format!("Status: {}\n", ["Open", "Done"][rng.below(2)]));
                out.push_str(&format!("N: {}\n", ["1", "two", "3.5"][rng.below(3)]));
                out.push_str(&format!("Related: [\"[[{}]]\"]\n", rng.pick(NAMES)));
                if rng.below(4) == 0 {
                    out.push_str("bad: [unclosed\n");
                }
                out.push_str("---\n");
            }
            for _ in 0..rng.below(8) {
                let piece = match rng.below(14) {
                    0 => format!("# {}\n\n", rng.pick(NAMES)),
                    1 => format!("[{}]({})\n\n", rng.pick(NAMES), link_to(rng)),
                    2 => format!("See [[{}]] and [[{}|alias]].\n\n", rng.pick(NAMES), rng.pick(NAMES)),
                    3 => "- a\n  - b\n- [x] c\n\n".to_owned(),
                    4 => "> 💡 callout\n\n".to_owned(),
                    5 => "| a | b |\n|---|---|\n| 1 | 2 |\n\n".to_owned(),
                    6 => "![pic](pic.png \"cap\")\n\n".to_owned(),
                    7 => "[doc](doc.pdf) and ![missing](nothing.png)\n\n".to_owned(),
                    8 => "<details>\n<summary>s</summary>\n\nbody\n\n</details>\n\n".to_owned(),
                    9 => "<!-- atomik:{\"node\":\"columnList\"} -->\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nl\n\n<!-- atomik:{\"node\":\"column\"} -->\n\nr\n\n<!-- atomik:{\"end\":\"columnList\"} -->\n\n".to_owned(),
                    10 => "```rust\nfn x() {}\n```\n\n$$\nx\n$$\n\nand $y$\n\n".to_owned(),
                    11 => "<aside>\n🔥 hot\n</aside>\n\n".to_owned(),
                    12 => "<!-- atomik:{\"node\":\"whiteboard\",\"file\":\"board.excalidraw\"} -->\n> **Whiteboard**\n\n".to_owned(),
                    _ => format!("[atomik](atomik-kb://page/{}) <time datetime=\"2026-10-07\">x</time>\n\n", PageId::from_random_bytes([5; 16])),
                };
                out.push_str(&piece);
            }
        }
    }
    out
}

fn random_source(seed: u64) -> (Vec<SourceFile>, Vec<String>, Option<String>) {
    let mut rng = Lcg(seed);
    let mut files = Vec::new();
    for _ in 0..rng.below(14) {
        let ext = *rng.pick(&["md", "md", "md", "html", "csv", "txt", "markdown"]);
        let path = random_path(&mut rng, ext);
        let text = random_content(&mut rng, ext);
        files.push(SourceFile { path, text });
    }
    if rng.below(3) == 0 {
        files.push(SourceFile {
            path: "board.excalidraw".into(),
            text: r#"{"elements":[{"id":"a","type":"rectangle"}]}"#.into(),
        });
    }
    if rng.below(4) == 0 {
        files.push(SourceFile {
            path: "atomik-export.json".into(),
            text: r#"{"properties":[{"name":"Status","kind":"select","options":[{"name":"Open"}]},{"name":"N","kind":"number"},{"name":"Related","kind":"relation","config":{"twoWay":false}}],"pages":["Home.md"]}"#.into(),
        });
    }
    let binaries = ["pic.png", "doc.pdf", "Home/pic.png"]
        .iter()
        .map(|b| (*b).to_owned())
        .collect();
    let name = (rng.below(2) == 0).then(|| "Source".to_owned());
    (files, binaries, name)
}

/// The plan is consistent with itself.
fn check_plan(plan: &Plan, binaries: &[String]) {
    let names: HashSet<&str> = plan.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), plan.properties.len(), "property names repeat");
    let lowercase: HashSet<String> = plan
        .properties
        .iter()
        .map(|p| p.name.to_lowercase())
        .collect();
    assert_eq!(
        lowercase.len(),
        plan.properties.len(),
        "property names repeat in another case"
    );
    for (index, page) in plan.pages.iter().enumerate() {
        if let Some(parent) = page.parent {
            assert!(parent < index, "a parent comes before its children");
        }
        assert!(!page.title.is_empty() && page.title.chars().count() <= 500);
        assert!(!page.title.contains('\n'));
        assert_eq!(page.content["type"], "doc");
        assert!(
            page.content["content"]
                .as_array()
                .is_some_and(|c| !c.is_empty())
        );
        let mut seen = HashSet::new();
        for (name, value) in &page.properties {
            assert!(names.contains(name.as_str()), "{name} is not planned");
            assert!(seen.insert(name.to_lowercase()), "a value repeats");
            match value {
                ImportValue::Pages(targets) => {
                    for target in targets {
                        if let PageTarget::Planned(i) = target {
                            assert!(*i < plan.pages.len());
                        }
                    }
                }
                ImportValue::Files(paths) => {
                    for path in paths {
                        assert!(plan.needed_files.contains(path));
                    }
                }
                _ => {}
            }
        }
        check_refs(&page.content, plan.pages.len(), &plan.needed_files);
    }
    let unique: HashSet<&String> = plan.needed_files.iter().collect();
    assert_eq!(unique.len(), plan.needed_files.len(), "needed files repeat");
    for path in &plan.needed_files {
        assert!(binaries.contains(path), "{path} is not a source file");
    }
}

fn check_refs(value: &Value, pages: usize, needed: &[String]) {
    match value {
        Value::Object(map) => {
            if let Some(target) = map
                .get("attrs")
                .and_then(|a| a.get("$page"))
                .and_then(|p| p.get("planned"))
            {
                assert!(
                    target
                        .as_u64()
                        .is_some_and(|i| usize::try_from(i).unwrap() < pages)
                );
            }
            if let Some(path) = map
                .get("attrs")
                .and_then(|a| a.get("$file"))
                .and_then(Value::as_str)
            {
                assert!(
                    needed.iter().any(|n| n == path),
                    "{path} is used but not needed"
                );
            }
            map.values().for_each(|v| check_refs(v, pages, needed));
        }
        Value::Array(items) => items.iter().for_each(|v| check_refs(v, pages, needed)),
        _ => {}
    }
}

fn resolver_for(plan: &Plan) -> Fake {
    let mut resolver = Fake::default();
    resolver.pages = (0..plan.pages.len())
        .map(|n| PageId::from_random_bytes([u8::try_from(n % 250).unwrap() + 1; 16]))
        .collect();
    for (n, path) in plan.needed_files.iter().enumerate() {
        resolver.attachments.insert(
            path.clone(),
            AttachmentId::from_random_bytes([u8::try_from(n % 250).unwrap() + 1; 16]),
        );
    }
    resolver
        .existing_pages
        .insert(PageId::from_random_bytes([5; 16]));
    let mut properties: HashMap<String, PropertyId> = HashMap::new();
    for (n, property) in plan.properties.iter().enumerate() {
        properties.insert(
            property.name.to_lowercase(),
            PropertyId::from_random_bytes([u8::try_from(n % 250).unwrap() + 1; 16]),
        );
        for (m, option) in property.options.iter().enumerate() {
            resolver.options.insert(
                (property.name.to_lowercase(), option.name.clone()),
                OptionId::from_random_bytes([u8::try_from(m % 250).unwrap() + 1; 16]),
            );
        }
    }
    resolver.properties = properties;
    resolver
}

#[test]
fn random_sources_plan_and_resolve_consistently() {
    let mut pages_seen = 0;
    for seed in 0..400 {
        let (files, binaries, name) = random_source(seed);
        let plan = plan(
            files,
            &binaries,
            &ImportOptions {
                source_name: name,
                max_pages: if seed % 7 == 0 { 5 } else { 10_000 },
            },
        );
        check_plan(&plan, &binaries);
        let resolver = resolver_for(&plan);
        for page in &plan.pages {
            let (json, _warnings, _doc) = resolve_and_load(page, &resolver);
            assert_eq!(json["type"], "doc");
            pages_seen += 1;
        }
    }
    assert!(pages_seen > 500, "{pages_seen}");
}

/// Documents of every format, mutated at random: nothing may panic, and what comes out is a sound
/// page.
#[test]
fn mutated_documents_never_panic() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/doc/");
    let seeds: Vec<(&str, String)> = vec![
        ("a.md", std::fs::read_to_string(format!("{root}page-v1.file.md")).unwrap_or_default()),
        ("b.md", std::fs::read_to_string(format!("{root}page-v1.md")).unwrap_or_default()),
        (
            "c.md",
            "---\ntitle: T\ntags: [a, b]\nX: 1\nY:\n  - \"[[Z]]\"\nP:\n  name: Paris\n  lat: 1\n  lon: 2\n---\n# T\n\n<aside>\n💡 note\n</aside>\n\n<details open>\n<summary>s</summary>\n\nbody\n\n</details>\n\n[[Wiki|alias]] ![[pic.png]] [x](doc.pdf)\n".to_owned(),
        ),
        (
            "d.html",
            "<html><head><title>T</title></head><body><article><h1 class=\"page-title\">T</h1><ul class=\"to-do-list\"><li><div class=\"checkbox checkbox-on\"></div>x</li></ul><table><tr><th>a</th></tr><tr><td>b</td></tr></table><figure class=\"callout\"><span class=\"icon\">🔥</span><p>t</p></figure><div class=\"column-list\"><div class=\"column\">a</div><div class=\"column\">b</div></div><math><annotation encoding=\"application/x-tex\">x</annotation></math><time datetime=\"2026-10-07\">d</time></article></body></html>".to_owned(),
        ),
        (
            "e.csv",
            "Name,Status,N,When,Related\nA,Open,1,2026-10-07,\"B (B.md), C (C.md)\"\nB,Done,2,\"October 7, 2026\",\nC,Open,x,,\n".to_owned(),
        ),
        (
            "atomik-export.json",
            r#"{"format":1,"properties":[{"name":"Status","kind":"status","options":[{"name":"Done","color":"green","group":"done"}]},{"name":"R","kind":"relation","config":{"twoWay":true,"pairName":"R2"}},{"name":"T","kind":"rollup","config":{"relation":"R","target":{"title":true},"function":"count_all"}}],"pages":["a.md","b.md"]}"#.to_owned(),
        ),
        (
            "board.excalidraw",
            r#"{"type":"excalidraw","elements":[{"id":"a","type":"rectangle","index":"a0"},{"id":"b","type":"text","text":"hi"}]}"#.to_owned(),
        ),
    ];
    // `KB_IMPORT_FUZZ_ROUNDS=20000 cargo test --release -p kb-import mutated` digs deeper.
    let rounds: u64 = std::env::var("KB_IMPORT_FUZZ_ROUNDS")
        .ok()
        .and_then(|rounds| rounds.parse().ok())
        .unwrap_or(150);
    let mut rng = Lcg(2026);
    for round in 0..rounds {
        let mut files: Vec<SourceFile> = Vec::new();
        for (path, text) in &seeds {
            let mut bytes: Vec<u8> = text.as_bytes().to_vec();
            for _ in 0..rng.below(8) {
                if bytes.is_empty() {
                    break;
                }
                let at = rng.below(bytes.len());
                match rng.below(6) {
                    0 => {
                        bytes.remove(at);
                    }
                    1 => bytes.insert(at, u8::try_from(rng.below(256)).unwrap()),
                    2 => {
                        let end = (at + rng.below(40)).min(bytes.len());
                        let chunk: Vec<u8> = bytes[at..end].to_vec();
                        bytes.splice(at..at, chunk);
                    }
                    3 => {
                        let end = (at + rng.below(40)).min(bytes.len());
                        bytes.drain(at..end);
                    }
                    4 => bytes.insert(at, *rng.pick(b"<>[]()*_`$|#\\&!~-:\"'{}\n ")),
                    _ => bytes.truncate(at),
                }
            }
            files.push(SourceFile {
                path: (*path).to_owned(),
                text: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        let binaries = vec!["pic.png".to_owned(), "doc.pdf".to_owned()];
        let plan = plan(
            files,
            &binaries,
            &ImportOptions {
                source_name: (round % 2 == 0).then(|| "S".to_owned()),
                max_pages: 10_000,
            },
        );
        check_plan(&plan, &binaries);
        let resolver = resolver_for(&plan);
        for page in &plan.pages {
            let _ = resolve_and_load(page, &resolver);
        }
    }
}
