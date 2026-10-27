//! Hostile and huge inputs: the importer must not panic, overflow the stack of a small thread or
//! take unbounded time.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::plan_tests::{file, run};
use crate::resolve_tests::{Fake, resolve_and_load};
use crate::testing::node_depth;
use crate::{Plan, SourceFile};

/// Runs `work` on a thread with the stack of a Windows main thread.
fn on_small_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(work)
        .expect("a thread")
        .join()
        .expect("the work did not panic or overflow")
}

fn plan_of(path: &str, text: String) -> Plan {
    run(vec![file(path, &text)], &[], None)
}

fn count_nodes(value: &Value) -> usize {
    match value {
        Value::Array(items) => items.iter().map(count_nodes).sum(),
        Value::Object(map) => 1 + map.get("content").map_or(0, count_nodes),
        _ => 0,
    }
}

fn deepest(plan: &Plan) -> usize {
    plan.pages
        .iter()
        .map(|p| node_depth(&p.content))
        .max()
        .unwrap_or(0)
}

/// Inputs that nest as deep as they can: `(file name, text, nodes may be nested)`.
fn nesting_cases() -> Vec<(&'static str, String)> {
    let mut lists = String::new();
    for level in 0..2_000 {
        lists.push_str(&" ".repeat(level.min(60)));
        lists.push_str(
            "- item
",
        );
    }
    vec![
        ("quotes.md", format!("{}deep", "> ".repeat(100_000))),
        ("lists.md", lists),
        ("inline-lists.md", format!("{}x", "- ".repeat(50_000))),
        (
            "divs.html",
            format!(
                "<html><body>{}text{}</body></html>",
                "<div>".repeat(100_000),
                "</div>".repeat(100_000)
            ),
        ),
        (
            "bq.html",
            format!(
                "<body>{}x{}</body>",
                "<blockquote>".repeat(50_000),
                "</blockquote>".repeat(50_000)
            ),
        ),
        (
            "ul.html",
            format!(
                "<body>{}x{}</body>",
                "<ul><li>".repeat(50_000),
                "</li></ul>".repeat(50_000)
            ),
        ),
        (
            "spans.html",
            format!(
                "<body><p>{}x{}</p></body>",
                "<span><b>".repeat(50_000),
                "</b></span>".repeat(50_000)
            ),
        ),
        (
            "tables.html",
            format!(
                "<body>{}x{}</body>",
                "<table><tr><td>".repeat(20_000),
                "</td></tr></table>".repeat(20_000)
            ),
        ),
        (
            "details.html",
            format!(
                "<body>{}x{}</body>",
                "<details><summary>s</summary>".repeat(20_000),
                "</details>".repeat(20_000)
            ),
        ),
        (
            "details.md",
            format!(
                "{}x{}",
                "<details>
<summary>s</summary>

"
                .repeat(20_000),
                "

</details>"
                    .repeat(20_000)
            ),
        ),
        (
            "asides.md",
            format!(
                "{}x{}",
                "<aside>
"
                .repeat(20_000),
                "
</aside>"
                    .repeat(20_000)
            ),
        ),
        (
            "yaml.md",
            format!(
                "---
a: {}x{}
b: 1
---
body",
                "[".repeat(50_000),
                "]".repeat(50_000)
            ),
        ),
        (
            "yaml2.md",
            format!(
                "---
{}
---
body",
                "- ".repeat(50_000)
            ),
        ),
        (
            "yaml3.md",
            format!(
                "---
a: {}
---
",
                "&a ".repeat(10_000)
            ),
        ),
        (
            "yaml4.md",
            format!(
                "---
{}x: 1
---
",
                "? ".repeat(20_000)
            ),
        ),
    ]
}

#[test]
fn deep_nesting_in_every_format_on_a_small_stack() {
    for (path, text) in nesting_cases() {
        let started = Instant::now();
        let name = path.to_owned();
        let depth = on_small_stack(move || {
            let plan = plan_of(&name, text);
            // The deepest result still loads into a page document (which stops at 128 levels);
            // loading tens of thousands of blocks is slow in a debug build and proves nothing more.
            for page in plan
                .pages
                .iter()
                .filter(|p| count_nodes(&p.content) < 3_000)
            {
                let _ = resolve_and_load(page, &Fake::default());
            }
            deepest(&plan)
        });
        eprintln!("{path}: {:?}", started.elapsed());
        assert!(depth <= 63, "{path}: {depth}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{path}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn hostile_csv_html_and_annotation_inputs() {
    let cases: Vec<(&str, String)> = vec![
        ("a.csv", "\"".repeat(100_000)),
        (
            "b.csv",
            format!(
                "a,b
\"{}
",
                "x,".repeat(100_000)
            ),
        ),
        (
            "c.csv",
            format!(
                "{}
",
                ",".repeat(100_000)
            ),
        ),
        (
            "d.csv",
            "a
"
            .repeat(200_000),
        ),
        ("e.html", "<".repeat(100_000)),
        ("f.html", "<a ".repeat(100_000)),
        ("g.html", "<table>".repeat(10_000)),
        ("h.md", "<!-- atomik:{".repeat(10_000)),
        (
            "i.md",
            "<!-- atomik:{\"node\":\"columnList\"} -->
"
            .repeat(20_000),
        ),
        (
            "j.md",
            "<!-- atomik:{\"node\":\"column\"} -->
"
            .repeat(20_000),
        ),
        (
            "k.md",
            "<!-- atomik:{\"items\":50,\"node\":\"gallery\"} -->
![](diagram.png)

"
            .repeat(5_000),
        ),
        ("l.md", "[[".repeat(100_000)),
        ("m.md", "![[".repeat(100_000)),
        ("n.md", "[^a]: ".repeat(50_000)),
        (
            "o.md",
            "| a |
|---|
"
            .repeat(20_000),
        ),
        ("p.md", "<time datetime=\"2026-10-07\">".repeat(50_000)),
        ("q.md", "<u>".repeat(50_000)),
        ("r.md", "<br>".repeat(100_000)),
        ("s.md", "$$".repeat(100_000)),
        ("t.md", "`".repeat(100_000)),
    ];
    for (path, text) in cases {
        let started = Instant::now();
        let name = path.to_owned();
        on_small_stack(move || plan_of(&name, text).pages.len());
        eprintln!("{path}: {:?}", started.elapsed());
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{path}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn many_small_inputs_are_fast_enough() {
    let started = Instant::now();
    let mut files: Vec<SourceFile> = Vec::new();
    for n in 0..3_000 {
        files.push(file(
            &format!("dir{}/page {n}.md", n % 30),
            "# Title\n\nSome *text* with a [link](other.md) and a [[Wiki]].\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        ));
    }
    let plan = run(files, &[], Some("bulk"));
    assert_eq!(plan.pages.len(), 3_000 + 30 + 1);
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
}

/// Big pathological texts, run by hand: `cargo test -p kb-import --release -- --ignored`.
#[test]
#[ignore = "slow: a few megabytes of hostile text"]
fn megabytes_of_hostile_text() {
    let size = 4 * 1024 * 1024;
    let cases: Vec<(&str, String)> = vec![
        ("brackets.md", "[".repeat(size)),
        ("stars.md", "*a ".repeat(size / 3)),
        ("underscores.md", "_a".repeat(size / 2)),
        ("emphasis.md", "*a **b ".repeat(size / 7)),
        ("links.md", "[a](b".repeat(size / 5)),
        ("tags.md", "<a ".repeat(size / 3)),
        ("fences.md", "```\n".repeat(size / 4)),
        (
            "table.md",
            format!("|a|\n|-|\n{}", "|x|\n".repeat(size / 4)),
        ),
        ("quotes.md", ">".repeat(size)),
        ("html.html", "<div><p>".repeat(size / 8)),
        ("csv.csv", "a,b\n1,2\n".repeat(size / 8)),
        ("text.txt", "line\n".repeat(size / 5)),
        ("math.md", "$a$ ".repeat(size / 4)),
    ];
    for (path, text) in cases {
        let started = Instant::now();
        let plan = plan_of(path, text);
        let _ = plan.pages.len();
        eprintln!("{path}: {:?}", started.elapsed());
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "{path}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn plan_output_never_holds_ids_or_oversized_trees() {
    // Whatever the input, the plan's content is a doc whose blocks carry no ids.
    let plan = plan_of(
        "x.md",
        format!("{}\n\n```\ncode\n```\n", "# H\n".repeat(1_000)),
    );
    let content = &plan.pages[0].content;
    assert_eq!(content["type"], "doc");
    fn no_ids(value: &Value) {
        match value {
            Value::Object(map) => {
                assert!(map.get("attrs").and_then(|a| a.get("id")).is_none());
                map.values().for_each(no_ids);
            }
            Value::Array(items) => items.iter().for_each(no_ids),
            _ => {}
        }
    }
    no_ids(content);
}

/// A large, ordinary document: a few megabytes of paragraphs, lists, links, tables and code.
#[test]
#[ignore = "slow in a debug build: run with --release"]
fn a_large_ordinary_document() {
    let mut text = String::from("---\ntitle: Big\ntags: [a, b]\n---\n");
    let chunk = "## Section\n\nSome *emphasis*, **strong** text with a [link](other.md) and `code`, \
                 a footnote[^1] and an equation $x^2$.\n\n- item one\n  - nested item\n- [x] done\n\n\
                 | a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n> 💡 callout\n\n[^1]: note\n\n";
    while text.len() < 8 * 1024 * 1024 {
        text.push_str(chunk);
    }
    let started = Instant::now();
    let plan = plan_of("big.md", text);
    eprintln!(
        "big: {:?} ({} blocks)",
        started.elapsed(),
        plan.pages[0].content["content"]
            .as_array()
            .map_or(0, Vec::len)
    );
    assert!(started.elapsed() < Duration::from_secs(60));
}
