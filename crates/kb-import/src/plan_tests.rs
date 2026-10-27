//! Tests of `plan` through the public API: trees, titles, ordering, Notion and Obsidian sources,
//! databases, properties, limits and hostile paths.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::{ImportWarningCode, PropertyKind};
use serde_json::{Value, json};

use crate::{
    ImportOptions, ImportValue, PageTarget, Plan, PlannedConfig, PlannedProperty, SourceFile, plan,
};

pub(crate) const HOME: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(crate) const SUB: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub(crate) const DB: &str = "cccccccccccccccccccccccccccccccc";
pub(crate) const ROW1: &str = "dddddddddddddddddddddddddddddddd";
pub(crate) const ROW2: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

pub(crate) fn file(path: &str, text: &str) -> SourceFile {
    SourceFile {
        path: path.to_owned(),
        text: text.to_owned(),
    }
}

pub(crate) fn options(source_name: Option<&str>) -> ImportOptions {
    ImportOptions {
        source_name: source_name.map(str::to_owned),
        max_pages: 10_000,
    }
}

pub(crate) fn run(files: Vec<SourceFile>, binaries: &[&str], source_name: Option<&str>) -> Plan {
    let binaries: Vec<String> = binaries.iter().map(|b| (*b).to_owned()).collect();
    plan(files, &binaries, &options(source_name))
}

fn tree(plan: &Plan) -> Vec<(Option<usize>, &str)> {
    plan.pages
        .iter()
        .map(|p| (p.parent, p.title.as_str()))
        .collect()
}

fn titles(plan: &Plan) -> Vec<&str> {
    plan.pages.iter().map(|p| p.title.as_str()).collect()
}

fn property<'a>(plan: &'a Plan, name: &str) -> &'a PlannedProperty {
    plan.properties
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no property {name}: {:?}", plan.properties))
}

fn value<'a>(plan: &'a Plan, page: usize, name: &str) -> Option<&'a ImportValue> {
    plan.pages[page]
        .properties
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
}

fn blocks(plan: &Plan, page: usize) -> &Vec<Value> {
    plan.pages[page].content["content"].as_array().unwrap()
}

fn json_of(plan: &Plan, page: usize) -> String {
    serde_json::to_string(&plan.pages[page].content).unwrap()
}

fn codes(plan: &Plan) -> Vec<ImportWarningCode> {
    plan.warnings.iter().map(|w| w.code).collect()
}

#[test]
fn one_markdown_file() {
    let plan = run(vec![file("My Notes.md", "Hello **world**\n")], &[], None);
    assert_eq!(tree(&plan), [(None, "My Notes")]);
    assert_eq!(plan.pages[0].source_path, "My Notes.md");
    assert_eq!(plan.pages[0].content["type"], "doc");
    assert_eq!(blocks(&plan, 0)[0]["content"][0]["text"], "Hello ");
    assert!(plan.properties.is_empty() && plan.needed_files.is_empty() && plan.warnings.is_empty());
}

#[test]
fn an_empty_source_is_an_empty_plan() {
    let plan = run(Vec::new(), &[], Some("Nothing"));
    assert!(plan.pages.is_empty() && plan.warnings.is_empty());
    let plan = run(vec![file("empty.md", "")], &[], None);
    assert_eq!(plan.pages.len(), 1);
    // An empty page still has a block, as the schema needs one.
    assert_eq!(blocks(&plan, 0), &vec![json!({"type": "paragraph"})]);
}

#[test]
fn titles_come_from_front_matter_a_heading_or_the_name() {
    let plan = run(
        vec![
            file(
                "a.md",
                "---\ntitle: From front matter\nicon: \"📝\"\n---\n# A heading\n\nbody\n",
            ),
            file("b.md", "# B\n\nbody\n"),
            file("c.md", "# Different heading\n\nbody\n"),
            file(&format!("d {HOME}.md"), "# The real title\n\nbody\n"),
            file("e.md", "No heading\n"),
            file("f.md", "---\ntitle: \"\"\n---\nx\n"),
        ],
        &[],
        None,
    );
    assert_eq!(
        titles(&plan),
        ["From front matter", "B", "c", "The real title", "e", "f"]
    );
    assert_eq!(plan.pages[0].icon.as_deref(), Some("📝"));
    // The heading that gave the title is removed; others stay.
    assert!(json_of(&plan, 0).contains("A heading"));
    assert!(!json_of(&plan, 1).contains("\"B\""));
    assert!(json_of(&plan, 2).contains("Different heading"));
    assert!(!json_of(&plan, 3).contains("The real title"));
}

#[test]
fn text_and_html_files() {
    let plan = run(
        vec![
            file("plain.txt", "one\ntwo\n\nthree"),
            file(
                "page.html",
                "<html><head><title>Head</title></head><body><article><div class=\"page-header-icon\"><span class=\"icon\">🔥</span></div><h1 class=\"page-title\">Page title</h1><p>Body</p></article></body></html>",
            ),
        ],
        &[],
        None,
    );
    assert_eq!(titles(&plan), ["Page title", "plain"]);
    assert_eq!(plan.pages[0].icon.as_deref(), Some("🔥"));
    assert_eq!(blocks(&plan, 0).len(), 1);
    assert_eq!(blocks(&plan, 1).len(), 2);
    assert_eq!(blocks(&plan, 1)[0]["content"][1]["type"], "hardBreak");
}

#[test]
fn children_live_in_the_folder_with_the_page_stem() {
    let plan = run(
        vec![
            file("Projects.md", "# Projects\n"),
            file("Projects/Alpha.md", "alpha"),
            file("Projects/Beta.md", "beta"),
            file("Projects/Alpha/Detail.md", "detail"),
            file("Loose/One.md", "one"),
            file("Loose/Deeper/Two.md", "two"),
            file("Top.md", "top"),
        ],
        &[],
        None,
    );
    // A folder without a page of its own is a page; siblings are in natural order.
    assert_eq!(
        tree(&plan),
        [
            (None, "Loose"),
            (Some(0), "Deeper"),
            (Some(1), "Two"),
            (Some(0), "One"),
            (None, "Projects"),
            (Some(4), "Alpha"),
            (Some(5), "Detail"),
            (Some(4), "Beta"),
            (None, "Top"),
        ]
    );
    // Folder pages have no file and no content.
    assert_eq!(plan.pages[0].source_path, "");
    assert_eq!(blocks(&plan, 0), &vec![json!({"type": "paragraph"})]);
}

#[test]
fn siblings_follow_links_then_natural_order() {
    let plan = run(
        vec![
            file(
                "Index.md",
                "[Chapter 10](Index/Chapter%2010.md)\n\n[Chapter 2](Index/Chapter%202.md)\n",
            ),
            file("Index/Chapter 1.md", "x"),
            file("Index/Chapter 2.md", "x"),
            file("Index/Chapter 10.md", "x"),
            file("Index/Appendix.md", "x"),
        ],
        &[],
        None,
    );
    // Linked children first, in the order of their first link; then the others, naturally.
    assert_eq!(
        titles(&plan),
        ["Index", "Chapter 10", "Chapter 2", "Appendix", "Chapter 1"]
    );
}

#[test]
fn a_single_top_level_folder_is_stripped_and_names_the_container() {
    let files = vec![
        file("Export-1/One.md", "one"),
        file("Export-1/Two.md", "two"),
        file("Export-1/Two/Sub.md", "sub"),
    ];
    let plan = run(files.clone(), &[], Some("Export-1.zip"));
    assert_eq!(
        tree(&plan),
        [
            (None, "Export-1"),
            (Some(0), "One"),
            (Some(0), "Two"),
            (Some(2), "Sub")
        ]
    );
    // The paths in the plan are the ones the shell gave.
    assert_eq!(plan.pages[1].source_path, "Export-1/One.md");
    // Without a source name there is no container.
    let plan = run(files, &[], None);
    assert_eq!(
        tree(&plan),
        [(None, "One"), (None, "Two"), (Some(1), "Sub")]
    );
    // One page alone is not wrapped.
    let plan = run(vec![file("Only/Page.md", "x")], &[], Some("Only"));
    assert_eq!(tree(&plan), [(None, "Page")]);
    // Files at the root and in folders: nothing to strip.
    let plan = run(
        vec![file("a.md", "x"), file("b/c.md", "y")],
        &["b/d.png"],
        Some("Source"),
    );
    assert_eq!(titles(&plan), ["Source", "a", "b", "c"]);
}

#[test]
fn the_container_takes_the_source_name_when_nothing_was_stripped() {
    let plan = run(
        vec![file("a.md", "x"), file("b.md", "y")],
        &[],
        Some("My export 0123456789abcdef0123456789abcdef"),
    );
    assert_eq!(titles(&plan), ["My export", "a", "b"]);
}

#[test]
fn the_page_limit_truncates_with_one_warning() {
    let files: Vec<SourceFile> = (0..10).map(|n| file(&format!("p{n:02}.md"), "x")).collect();
    let binaries: Vec<String> = Vec::new();
    let plan = plan(
        files,
        &binaries,
        &ImportOptions {
            source_name: None,
            max_pages: 4,
        },
    );
    assert_eq!(titles(&plan), ["p00", "p01", "p02", "p03"]);
    assert_eq!(codes(&plan), [ImportWarningCode::Truncated]);
    assert_eq!(plan.warnings[0].path, "p04.md");
    assert_eq!(plan.warnings[0].detail.as_deref(), Some("6 pages left out"));
    // Zero pages allowed.
    let plan = crate::plan(
        vec![file("a.md", "x")],
        &binaries,
        &ImportOptions {
            source_name: None,
            max_pages: 0,
        },
    );
    assert!(plan.pages.is_empty());
}

#[test]
fn unsafe_paths_and_unknown_files_are_refused() {
    let plan = run(
        vec![
            file("../escape.md", "x"),
            file("/absolute.md", "x"),
            file("C:/drive.md", "x"),
            file("back\\slash.md", "x"),
            file("a/../b.md", "x"),
            file("dup.md", "first"),
            file("dup.md", "second"),
            file("data.json", "{}"),
            file(".trash/old.md", "x"),
            file("__MACOSX/._fine.md", "x"),
            file("fine.md", "ok"),
        ],
        &["../bin.png", "ok.png", ".hidden.png"],
        None,
    );
    assert_eq!(titles(&plan), ["dup", "fine"]);
    assert!(json_of(&plan, 0).contains("first"));
    let refused: Vec<_> = plan
        .warnings
        .iter()
        .filter(|w| w.code == ImportWarningCode::UnsupportedFile)
        .map(|w| w.path.as_str())
        .collect();
    assert_eq!(
        refused,
        [
            "../escape.md",
            "/absolute.md",
            "C:/drive.md",
            "back\\slash.md",
            "a/../b.md",
            "dup.md",
            ".trash/old.md",
            "__MACOSX/._fine.md",
            "data.json"
        ]
    );
}

#[test]
fn obsidian_style_folder() {
    let plan = run(
        vec![
            file(
                "Vault/Daily.md",
                "---\ntags: [journal, \"#daily\"]\nmood: 7\nDone: true\nDue: 2026-10-07\nlinks: \"[[Ideas]]\"\n---\nSee [[Ideas]] and [[Projects/Alpha|the alpha]] and [[Missing note]].\n",
            ),
            file(
                "Vault/Ideas.md",
                "---\ntags: ideas\nmood: great\n---\nIdeas\n",
            ),
            file("Vault/Projects/Alpha.md", "alpha\n"),
        ],
        &[],
        Some("Vault"),
    );
    assert_eq!(
        tree(&plan),
        [
            (None, "Vault"),
            (Some(0), "Daily"),
            (Some(0), "Ideas"),
            (Some(0), "Projects"),
            (Some(3), "Alpha")
        ]
    );
    let daily = 1;
    assert_eq!(
        value(&plan, daily, "Tags"),
        Some(&ImportValue::List(vec!["journal".into(), "daily".into()]))
    );
    assert_eq!(value(&plan, daily, "Done"), Some(&ImportValue::Bool(true)));
    assert_eq!(
        value(&plan, daily, "Due"),
        Some(&ImportValue::Date {
            date: "2026-10-07".into(),
            time: None
        })
    );
    assert_eq!(
        value(&plan, daily, "links"),
        Some(&ImportValue::Pages(vec![PageTarget::Planned(2)]))
    );
    // `mood` is a number in one page and a word in another: text.
    assert_eq!(property(&plan, "mood").kind, PropertyKind::Text);
    assert_eq!(
        value(&plan, daily, "mood"),
        Some(&ImportValue::Text("7".into()))
    );
    assert_eq!(
        value(&plan, 2, "mood"),
        Some(&ImportValue::Text("great".into()))
    );
    assert_eq!(property(&plan, "Tags").kind, PropertyKind::MultiSelect);
    let tag_options: Vec<_> = property(&plan, "Tags")
        .options
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(tag_options, ["journal", "daily", "ideas"]);
    assert_eq!(property(&plan, "Done").kind, PropertyKind::Checkbox);
    assert_eq!(property(&plan, "Due").kind, PropertyKind::Date);
    assert_eq!(property(&plan, "links").kind, PropertyKind::Relation);
    assert_eq!(
        property(&plan, "links").config,
        Some(PlannedConfig::Relation {
            two_way: false,
            pair_name: None
        })
    );
    // Wiki links: a planned page by name, by path with an alias, and an unknown one by title.
    let content = &blocks(&plan, daily)[0]["content"];
    assert_eq!(content[1]["attrs"]["$page"], json!({"planned": 2}));
    assert_eq!(content[3]["attrs"]["$page"], json!({"planned": 4}));
    assert_eq!(
        content[5]["attrs"]["$page"],
        json!({"title": "Missing note"})
    );
}

#[test]
fn front_matter_problems_are_reported_not_fatal() {
    let plan = run(
        vec![
            file("a.md", "---\nkey: [unclosed\n---\nbody\n"),
            file("b.md", "---\nicon: pictures/me.png\n---\nbody\n"),
        ],
        &[],
        None,
    );
    assert_eq!(titles(&plan), ["a", "b"]);
    assert_eq!(
        codes(&plan),
        [
            ImportWarningCode::UnsupportedContent,
            ImportWarningCode::UnsupportedContent
        ]
    );
    assert_eq!(plan.warnings[0].detail.as_deref(), Some("front matter"));
    assert_eq!(plan.warnings[1].detail.as_deref(), Some("icon"));
    assert!(plan.properties.is_empty());
    assert!(json_of(&plan, 0).contains("body"));
}

#[test]
fn front_matter_files_are_needed_files() {
    let plan = run(
        vec![file(
            "Notes/Weekly.md",
            "---\nAttachments:\n  - \"Weekly/report.pdf\"\n---\nx",
        )],
        &["Notes/Weekly/report.pdf", "Notes/unused.png"],
        None,
    );
    assert_eq!(plan.needed_files, ["Notes/Weekly/report.pdf"]);
    assert_eq!(
        value(&plan, 0, "Attachments"),
        Some(&ImportValue::Files(vec!["Notes/Weekly/report.pdf".into()]))
    );
    assert_eq!(property(&plan, "Attachments").kind, PropertyKind::Files);
}

/// A small Notion "Markdown & CSV" export, as a zip would hold it.
pub(crate) fn notion_files() -> Vec<SourceFile> {
    let csv = "Name,Status,Due,Estimate,Done,Tags,Related\n\
               Write docs,In progress,\"October 7, 2026\",3.5,No,\"a, b\",Sub page (Home%20aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/Sub%20page%20bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.md)\n\
               Fix bug,Done,\"October 8, 2026 2:30 PM\",1,Yes,\"b, c\",\n\
               Third row,Not started,,2,No,a,\n\
               Fourth row,Done,,5,Yes,c,\n";
    let old_csv = "Name,Status\nWrite docs,Old\n";
    vec![
        file(
            &format!("Export-1234/Home {HOME}.md"),
            &format!(
                "# Home\n\n<aside>\n💡 Remember this\n</aside>\n\n[Sub page](Home%20{HOME}/Sub%20page%20{SUB}.md)\n\n[Tasks](Tasks%20{DB}.csv)\n\n![Diagram](Home%20{HOME}/diagram.png)\n\nBy URL: [Sub](https://www.notion.so/Sub-page-{SUB}).\n"
            ),
        ),
        file(
            &format!("Export-1234/Home {HOME}/Sub page {SUB}.md"),
            "# Sub page\n\nSome $E=mc^2$ and a missing [link](Gone%20ffffffffffffffffffffffffffffffff.md).\n",
        ),
        file(&format!("Export-1234/Tasks {DB}.csv"), old_csv),
        file(&format!("Export-1234/Tasks {DB}_all.csv"), csv),
        file(
            &format!("Export-1234/Tasks {DB}/Write docs {ROW1}.md"),
            &format!(
                "# Write docs\n\nStatus: In progress\nDue: October 7, 2026\nEstimate: 3.5\nRelated: [Sub page](../Home%20{HOME}/Sub%20page%20{SUB}.md)\n\nBody of the task.\n"
            ),
        ),
        file(
            &format!("Export-1234/Tasks {DB}/Fix bug {ROW2}.md"),
            "# Fix bug\n\nStatus: Done\n\nFixed it.\n",
        ),
    ]
}

#[test]
fn a_notion_export() {
    let plan = run(
        notion_files(),
        &[
            &format!("Export-1234/Home {HOME}/diagram.png"),
            "Export-1234/Unused.png",
        ],
        Some("Export-1234"),
    );
    // The container, the pages, and the database with its rows in the CSV's order.
    assert_eq!(
        tree(&plan),
        [
            (None, "Export-1234"),
            (Some(0), "Home"),
            (Some(1), "Sub page"),
            (Some(0), "Tasks"),
            (Some(3), "Write docs"),
            (Some(3), "Fix bug"),
            (Some(3), "Third row"),
            (Some(3), "Fourth row"),
        ]
    );
    assert_eq!(
        plan.pages[3].source_path,
        format!("Export-1234/Tasks {DB}_all.csv")
    );
    assert_eq!(plan.pages[6].source_path, "");
    assert_eq!(
        plan.needed_files,
        [format!("Export-1234/Home {HOME}/diagram.png")]
    );

    // Home: a callout, sub-page cards, a database card, an image, a link by URL.
    let home = blocks(&plan, 1);
    assert_eq!(home[0]["type"], "callout");
    assert_eq!(home[0]["attrs"]["icon"], "💡");
    assert_eq!(home[0]["content"][0]["content"][0]["text"], "Remember this");
    assert_eq!(home[1]["type"], "pageLink");
    assert_eq!(home[1]["attrs"]["$page"], json!({"planned": 2}));
    assert_eq!(home[2]["type"], "pageLink");
    assert_eq!(home[2]["attrs"]["$page"], json!({"planned": 3}));
    assert_eq!(home[3]["type"], "image");
    assert_eq!(
        home[3]["attrs"]["$file"],
        format!("Export-1234/Home {HOME}/diagram.png")
    );
    assert_eq!(
        home[4]["content"][1]["attrs"]["$page"],
        json!({"planned": 2})
    );

    // The sub page: an equation, and a link to a page that is not in the export.
    let sub = blocks(&plan, 2);
    assert_eq!(sub[0]["content"][1]["type"], "mathInline");
    assert_eq!(sub[0]["content"][2]["text"], " and a missing link.");
    assert!(plan.warnings.iter().any(|w| {
        w.code == ImportWarningCode::UnresolvedLink
            && w.path
                .ends_with("Sub page bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.md")
    }));

    // The database page shows its rows as a table; its columns are its properties.
    let tasks = blocks(&plan, 3);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["type"], "pageList");
    assert_eq!(tasks[0]["attrs"]["layout"], "table");
    assert_eq!(tasks[0]["attrs"]["source"], json!({"kind": "children"}));
    assert_eq!(
        tasks[0]["attrs"]["properties"],
        json!(["Status", "Due", "Estimate", "Done", "Tags", "Related"])
    );
    assert_eq!(tasks[0]["attrs"]["$names"], true);

    // Kinds are inferred from the `_all` CSV (the other one is ignored).
    assert_eq!(property(&plan, "Status").kind, PropertyKind::Select);
    let status: Vec<_> = property(&plan, "Status")
        .options
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(status, ["In progress", "Done", "Not started"]);
    assert_eq!(property(&plan, "Due").kind, PropertyKind::Date);
    assert_eq!(property(&plan, "Estimate").kind, PropertyKind::Number);
    assert_eq!(property(&plan, "Done").kind, PropertyKind::Checkbox);
    assert_eq!(property(&plan, "Tags").kind, PropertyKind::MultiSelect);
    assert_eq!(property(&plan, "Related").kind, PropertyKind::Relation);
    assert_eq!(plan.properties.len(), 6);

    // The row page is matched by title and brings its content, without the property lines.
    assert_eq!(
        plan.pages[4].source_path,
        format!("Export-1234/Tasks {DB}/Write docs {ROW1}.md")
    );
    let write_docs = blocks(&plan, 4);
    assert_eq!(write_docs.len(), 1);
    assert_eq!(write_docs[0]["content"][0]["text"], "Body of the task.");
    assert_eq!(
        value(&plan, 4, "Status"),
        Some(&ImportValue::Text("In progress".into()))
    );
    assert_eq!(
        value(&plan, 4, "Due"),
        Some(&ImportValue::Date {
            date: "2026-10-07".into(),
            time: None
        })
    );
    assert_eq!(value(&plan, 4, "Estimate"), Some(&ImportValue::Number(3.5)));
    assert_eq!(value(&plan, 4, "Done"), Some(&ImportValue::Bool(false)));
    assert_eq!(
        value(&plan, 4, "Tags"),
        Some(&ImportValue::List(vec!["a".into(), "b".into()]))
    );
    assert_eq!(
        value(&plan, 4, "Related"),
        Some(&ImportValue::Pages(vec![PageTarget::Planned(2)]))
    );
    assert_eq!(
        value(&plan, 5, "Due"),
        Some(&ImportValue::Date {
            date: "2026-10-08".into(),
            time: Some("14:30".into())
        })
    );
    assert_eq!(value(&plan, 5, "Done"), Some(&ImportValue::Bool(true)));
    assert_eq!(value(&plan, 5, "Related"), None);
    // A row with no page of its own gets an empty page with its values.
    assert_eq!(blocks(&plan, 6), &vec![json!({"type": "paragraph"})]);
    assert_eq!(value(&plan, 6, "Estimate"), Some(&ImportValue::Number(2.0)));
    assert_eq!(
        value(&plan, 7, "Status"),
        Some(&ImportValue::Text("Done".into()))
    );
}

#[test]
fn a_database_without_row_pages_and_unmatched_row_pages() {
    let plan = run(
        vec![
            file(
                "People.csv",
                "Name;City;Age\nAnn;Paris;31\nBob;Rome;42\nCy;Paris;\n",
            ),
            file("People/Dana.md", "# Dana\n\nStatus: x\n"),
        ],
        &[],
        None,
    );
    assert_eq!(
        tree(&plan),
        [
            (None, "People"),
            (Some(0), "Ann"),
            (Some(0), "Bob"),
            (Some(0), "Cy"),
            (Some(0), "Dana")
        ]
    );
    assert_eq!(property(&plan, "City").kind, PropertyKind::Select);
    assert_eq!(property(&plan, "Age").kind, PropertyKind::Number);
    assert_eq!(
        value(&plan, 1, "City"),
        Some(&ImportValue::Text("Paris".into()))
    );
    assert_eq!(value(&plan, 3, "Age"), None);
    // The page that matches no row is a child without values, and keeps its text.
    assert!(plan.pages[4].properties.is_empty());
    assert!(json_of(&plan, 4).contains("Status: x"));
}

#[test]
fn csv_edge_cases() {
    let plan = run(
        vec![file("Empty.csv", ""), file("Header.csv", "Name,A\n")],
        &[],
        None,
    );
    assert_eq!(titles(&plan), ["Empty", "Header"]);
    assert_eq!(blocks(&plan, 0)[0]["attrs"]["properties"], json!([]));
    assert_eq!(blocks(&plan, 1)[0]["attrs"]["properties"], json!(["A"]));
    // The formula guard is removed and the title column found.
    let plan = run(vec![file("t.csv", "Other,Title\nx,'=1+1\ny,\n")], &[], None);
    assert_eq!(titles(&plan), ["t", "=1+1", "Untitled"]);
}

#[test]
fn sidecar_properties_and_order() {
    let sidecar = json!({
        "format": 1,
        "properties": [
            {"name": "Status", "kind": "status", "options": [
                {"name": "Todo", "color": "gray", "group": "todo"},
                {"name": "Done", "color": "green", "group": "done"}]},
            {"name": "Related", "kind": "relation", "config": {"twoWay": true, "pairName": "Related (back)"}},
            {"name": "Estimate", "kind": "number"},
            {"name": "Total", "kind": "rollup", "config": {"relation": "Related", "target": {"property": "Estimate"}, "function": "sum"}},
            {"name": "Note", "kind": "text"}
        ],
        "pages": ["Zed.md", "Alpha.md", "Zed/Child B.md", "Zed/Child A.md"]
    })
    .to_string();
    let plan = run(
        vec![
            file("atomik-export.json", &sidecar),
            file(
                "Alpha.md",
                "---\ntitle: Alpha\nStatus: Done\nEstimate: 2\nNote: 2026-10-07\nRelated:\n  - \"[[Zed]]\"\n---\n",
            ),
            file(
                "Zed.md",
                "---\ntitle: Zed\nStatus: Todo\nEstimate: 3\nlinks: [a]\n---\n[Child A](Zed/Child%20A.md)\n",
            ),
            file("Zed/Child A.md", "a"),
            file("Zed/Child B.md", "b"),
        ],
        &[],
        None,
    );
    // The sidecar's order, not the alphabet and not the links.
    assert_eq!(titles(&plan), ["Zed", "Child B", "Child A", "Alpha"]);
    let names: Vec<_> = plan
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("Status", PropertyKind::Status),
            ("Related", PropertyKind::Relation),
            ("Estimate", PropertyKind::Number),
            ("Total", PropertyKind::Rollup),
            ("Note", PropertyKind::Text),
            ("links", PropertyKind::MultiSelect),
        ]
    );
    assert_eq!(property(&plan, "Status").options[1].name, "Done");
    assert_eq!(
        property(&plan, "Related").config,
        Some(PlannedConfig::Relation {
            two_way: true,
            pair_name: Some("Related (back)".into())
        })
    );
    // A date-looking text stays text where the schema says text.
    assert_eq!(
        value(&plan, 3, "Note"),
        Some(&ImportValue::Text("2026-10-07".into()))
    );
    assert_eq!(
        value(&plan, 3, "Status"),
        Some(&ImportValue::Text("Done".into()))
    );
    // A page named in a relation is a planned page.
    assert_eq!(
        value(&plan, 3, "Related"),
        Some(&ImportValue::Pages(vec![PageTarget::Planned(0)]))
    );
}

#[test]
fn values_of_one_name_with_several_kinds_become_text() {
    let plan = run(
        vec![
            file("a.md", "---\nWhen: 2026-10-07\nCount: 3\nFlag: true\n---\n"),
            file("b.md", "---\nWhen: soon\nCount: 4\nFlag: true\n---\n"),
        ],
        &[],
        None,
    );
    assert_eq!(property(&plan, "When").kind, PropertyKind::Text);
    assert_eq!(
        value(&plan, 0, "When"),
        Some(&ImportValue::Text("2026-10-07".into()))
    );
    assert_eq!(property(&plan, "Count").kind, PropertyKind::Number);
    assert_eq!(property(&plan, "Flag").kind, PropertyKind::Checkbox);
}

#[test]
fn many_files_plan_quickly() {
    let mut files = Vec::new();
    for folder in 0..40 {
        for page in 0..50 {
            files.push(file(
                &format!("F{folder}/Page {page}.md"),
                &format!(
                    "# Page {page}\n\nSee [[Page {}]] and [x](../F{}/Page {}.md).\n",
                    (page + 1) % 50,
                    (folder + 1) % 40,
                    page
                ),
            ));
        }
    }
    let started = std::time::Instant::now();
    let plan = run(files, &[], Some("Many"));
    assert_eq!(plan.pages.len(), 2_000 + 40 + 1);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn row_pages_match_by_title_then_by_name_and_keep_their_lines_when_unmatched() {
    let plan = run(
        vec![
            file(
                "Tasks.csv",
                "Name,Status\nOne,Open\nTwo,Closed\nLong title that Notion cut,Open\n",
            ),
            // Matches the row `One`; its property lines are in the CSV.
            file("Tasks/One.md", "# One\n\nStatus: Open\n\nBody one\n"),
            // Matches by the name of the file (the heading differs).
            file(
                "Tasks/Long title that Notion cut.md",
                "# The long title that Notion cut in its file name\n\nStatus: Open\n\nBody long\n",
            ),
            // Matches no row: nothing carries its values, so its lines stay.
            file(
                "Tasks/Stray.md",
                "# Stray\n\nStatus: Closed\n\nBody stray\n",
            ),
        ],
        &[],
        None,
    );
    assert_eq!(
        tree(&plan),
        [
            (None, "Tasks"),
            (Some(0), "One"),
            (Some(0), "Two"),
            (Some(0), "Long title that Notion cut"),
            (Some(0), "Stray"),
        ]
    );
    assert!(plan.pages[1].source_path.ends_with("One.md"));
    assert!(!json_of(&plan, 1).contains("Status"));
    assert!(json_of(&plan, 1).contains("Body one"));
    assert_eq!(
        value(&plan, 1, "Status"),
        Some(&ImportValue::Text("Open".into()))
    );
    // `Two` has no page.
    assert_eq!(plan.pages[2].source_path, "");
    assert!(
        plan.pages[3]
            .source_path
            .ends_with("Long title that Notion cut.md")
    );
    assert!(json_of(&plan, 3).contains("Body long"));
    assert_eq!(
        value(&plan, 3, "Status"),
        Some(&ImportValue::Text("Open".into()))
    );
    assert!(json_of(&plan, 4).contains("Status: Closed"));
    assert!(plan.pages[4].properties.is_empty());
}

#[test]
fn a_notion_html_export() {
    let html = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Weekly plan</title><style>.x{}</style></head><body>\
        <article id=\"abc\" class=\"page sans\"><header><div class=\"page-header-icon undefined\"><span class=\"icon\">📅</span></div>\
        <h1 class=\"page-title\">Weekly plan</h1><p class=\"page-description\"></p></header>\
        <div class=\"page-body\"><p id=\"p1\">Intro with <strong>bold</strong> and <a href=\"Other.md\">a link</a>.</p>\
        <ul class=\"to-do-list\"><li><div class=\"checkbox checkbox-on\"></div> <span class=\"to-do-children-checked\">Done</span></li>\
        <li><div class=\"checkbox checkbox-off\"></div> <span class=\"to-do-children-unchecked\">Todo</span></li></ul>\
        <figure class=\"callout\" style=\"white-space:pre-wrap\"><div style=\"font-size:1.5em\"><span class=\"icon\">💡</span></div><div style=\"width:100%\"><p>Tip text</p></div></figure>\
        <div class=\"column-list\"><div class=\"column\" style=\"width:50%\"><p>Left</p></div><div class=\"column\" style=\"width:50%\"><p>Right</p></div></div>\
        <table class=\"simple-table\"><tbody><tr><td>a</td><td>b</td></tr></tbody></table>\
        <figure id=\"f\" class=\"image\"><a href=\"pic.png\"><img style=\"width:288px\" src=\"pic.png\"/></a></figure>\
        <script>alert(1)</script></div></article></body></html>";
    let plan = run(
        vec![
            file("Weekly plan abcdefabcdefabcdefabcdefabcdefab.html", html),
            file("Other.md", "x"),
        ],
        &["pic.png"],
        None,
    );
    assert_eq!(titles(&plan), ["Other", "Weekly plan"]);
    assert_eq!(plan.pages[1].icon.as_deref(), Some("📅"));
    let blocks = blocks(&plan, 1);
    let types: Vec<_> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        [
            "paragraph",
            "taskList",
            "callout",
            "columnList",
            "table",
            "image"
        ]
    );
    assert_eq!(
        blocks[0]["content"][3]["attrs"]["$page"],
        json!({"planned": 0})
    );
    assert_eq!(plan.needed_files, ["pic.png"]);
    assert_eq!(codes(&plan), [ImportWarningCode::UnsupportedContent]);
    assert_eq!(plan.warnings[0].detail.as_deref(), Some("script"));
}

#[test]
fn deeply_nested_html_keeps_its_text() {
    let html = format!(
        "<html><body><p>before</p>{}deep text{}<p>after</p></body></html>",
        "<div>".repeat(5_000),
        "</div>".repeat(5_000)
    );
    let plan = run(vec![file("deep.html", &html)], &[], None);
    let all = json_of(&plan, 0);
    assert!(all.contains("before") && all.contains("deep text") && all.contains("after"));
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.detail.as_deref() == Some("html nesting"))
    );
}

#[test]
fn a_path_that_is_too_long_is_refused() {
    let long = format!("{}.md", "a".repeat(1_100));
    let plan = run(vec![file(&long, "x"), file("ok.md", "y")], &[], None);
    assert_eq!(titles(&plan), ["ok"]);
    assert_eq!(codes(&plan), [ImportWarningCode::UnsupportedFile]);
}
