// The plan of an import (ADR-0036, 3.3 to 3.8): names, the page tree, titles and icons, front
// matter, Markdown, text, HTML and CSV content, links, files, warnings and limits.

import { describe, expect, it } from "vitest";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { planImport } from "./memory-import-plan";
import type { ImportPlan, PlannedPage } from "./memory-import-plan";

const ID1 = "0123456789abcdef0123456789abcdef";
const ID2 = "fedcba9876543210fedcba9876543210";
const ID3 = "11111111111111111111111111111111";
const ID4 = "22222222222222222222222222222222";
const ID5 = "33333333333333333333333333333333";

function plan(files: Record<string, string>, binary: string[] = [], sourceName?: string, maxPages = 10_000): ImportPlan {
  return planImport({ files: new Map(Object.entries(files)), binaryPaths: binary, sourceName, maxPages });
}

/** The pages as an indented outline of their titles. */
function outline(result: ImportPlan): string[] {
  const depth = (page: PlannedPage): number => (page.parent === null ? 0 : 1 + depth(result.pages[page.parent] as PlannedPage));
  return result.pages.map((page) => `${"  ".repeat(depth(page))}${page.title}`);
}

function page(result: ImportPlan, title: string): PlannedPage {
  const found = result.pages.find((candidate) => candidate.title === title);
  if (found === undefined) {
    throw new Error(`no page titled ${title}`);
  }
  return found;
}

const text = (value: string): JsonValue => ({ type: "text", text: value });
const paragraph = (...content: JsonValue[]): JsonValue => ({ type: "paragraph", content });

describe("names and the tree", () => {
  it("strips Notion ids from names, and a folder named like a page holds its children", () => {
    const result = plan({
      [`Getting Started ${ID1}.md`]: "intro",
      [`Getting Started ${ID1}/Sub ${ID3}.md`]: "sub",
      [`Getting Started ${ID1}/Sub ${ID3}/Deep ${ID4}.md`]: "deep",
    });
    expect(outline(result)).toEqual(["Getting Started", "  Sub", "    Deep"]);
    expect(result.pages.map((entry) => entry.path)).toEqual([
      `Getting Started ${ID1}.md`,
      `Getting Started ${ID1}/Sub ${ID3}.md`,
      `Getting Started ${ID1}/Sub ${ID3}/Deep ${ID4}.md`,
    ]);
  });

  it("makes a page of a folder that has none of its name, at any depth, titled with its cleaned name", () => {
    const result = plan({ [`Archive ${ID1}/2025/note.md`]: "x", "Archive 2/other.md": "y", "readme.txt": "z" });
    expect(outline(result)).toEqual(["Archive", "  2025", "    note", "Archive 2", "  other", "readme"]);
    expect(page(result, "Archive").blocks).toEqual([]);
  });

  it("sorts siblings by natural order of their names, case-insensitively", () => {
    const result = plan({ "Page 10.md": "", "Page 2.md": "", "page 1.md": "", "Page 1a.md": "" });
    expect(outline(result)).toEqual(["page 1", "Page 1a", "Page 2", "Page 10"]);
  });

  it("puts the pages a page links to first, in the order of their first link, then the others", () => {
    const result = plan({
      "Index.md": "Read [C](Index/C.md) then [B](Index/B.md) and [[C]] again.",
      "Index/A.md": "",
      "Index/B.md": "",
      "Index/C.md": "",
      "Index/D.md": "",
    });
    expect(outline(result)).toEqual(["Index", "  C", "  B", "  A", "  D"]);
  });

  it("follows the order of the sidecar when the source has one", () => {
    const result = plan({
      "atomik-export.json": JSON.stringify({ format: 1, properties: [], pages: ["Zeta.md", "Alpha.md", "Alpha/Second.md", "Alpha/First.md"] }),
      "Alpha.md": "",
      "Zeta.md": "",
      "Beta.md": "",
      "Alpha/First.md": "",
      "Alpha/Second.md": "",
    });
    expect(outline(result)).toEqual(["Zeta", "Alpha", "  Second", "  First", "Beta"]);
  });

  it("strips the one folder every file is in, and names the container after it when the source has a name", () => {
    const files = { "Export abc/One.md": "1", "Export abc/Two.md": "2" };
    expect(outline(plan(files, ["Export abc/img/pic.png"]))).toEqual(["One", "Two"]);
    expect(outline(plan(files, [], "My zip"))).toEqual(["Export abc", "  One", "  Two"]);
    // A file at the top level keeps every folder.
    expect(outline(plan({ "Top.md": "", "Export abc/One.md": "1" }))).toEqual(["Export abc", "  One", "Top"]);
  });

  it("groups several top-level pages under a container named after the source, cleaned of its Notion id", () => {
    const result = plan({ "a.md": "", "b.md": "" }, [], `Backup ${ID1}`);
    expect(outline(result)).toEqual(["Backup", "  a", "  b"]);
    expect(result.pages[0]?.path).toBe("");
    expect(result.pages.map((entry) => entry.parent)).toEqual([null, 0, 0]);
  });

  it("makes no container for one top-level page, or without a source name", () => {
    expect(outline(plan({ "only.md": "" }, [], "Name"))).toEqual(["only"]);
    expect(outline(plan({ "a.md": "", "b.md": "" }))).toEqual(["a", "b"]);
  });

  it("plans nothing for an empty source", () => {
    const result = plan({});
    expect(result.pages).toEqual([]);
    expect(result.neededFiles).toEqual([]);
    expect(result.warnings).toEqual([]);
  });

  it("warns about files of a type it does not read, and leaves the sidecar and whiteboards alone", () => {
    const result = plan({ "a.md": "", "b.json": "{}", "c.docx": "x", "board.excalidraw": "{}", "atomik-export.json": "{}" });
    expect(outline(result)).toEqual(["a"]);
    expect(result.warnings).toEqual([
      { path: "b.json", code: "unsupportedFile" },
      { path: "c.docx", code: "unsupportedFile" },
    ]);
  });

  it("stops at the page limit with one warning for the rest", () => {
    const result = plan({ "a.md": "", "b.md": "", "c.md": "", "d.md": "" }, [], undefined, 2);
    expect(outline(result)).toEqual(["a", "b"]);
    expect(result.warnings).toEqual([{ path: "c.md", code: "truncated" }]);
  });
});

describe("titles and icons", () => {
  it("takes the front matter title, then a first heading that says the file's name or the file has a Notion id, then the name", () => {
    const result = plan({
      "A.md": '---\ntitle: "From front matter"\n---\n# A\n\nbody',
      "B.md": "# b\n\nbody",
      [`C ${ID1}.md`]: "# A different heading\n\nbody",
      "D.md": "# Something else\n\nbody",
      "E.md": "body only",
    });
    expect(outline(result)).toEqual(["From front matter", "b", "A different heading", "D", "E"]);
    // A heading that gave the title is not part of the page; the front matter's title leaves it alone.
    expect(page(result, "b").blocks).toEqual([paragraph(text("body"))]);
    expect(page(result, "A different heading").blocks).toEqual([paragraph(text("body"))]);
    expect(page(result, "From front matter").blocks[0]).toMatchObject({ type: "heading" });
    expect(page(result, "D").blocks[0]).toMatchObject({ type: "heading" });
  });

  it("reads an icon from the front matter when it is an emoji or one of the set, and warns about a file path", () => {
    const result = plan({
      "A.md": '---\nicon: "📝"\n---\nx',
      "B.md": "---\nicon: icon:book:blue\n---\nx",
      "C.md": "---\nicon: images/icon.png\n---\nx",
      "D.md": "---\nicon: not an emoji\n---\nx",
    });
    expect(page(result, "A").icon).toBe("📝");
    expect(page(result, "B").icon).toBe("icon:book:blue");
    expect(page(result, "C").icon).toBeUndefined();
    expect(result.warnings).toEqual([
      { path: "C.md", code: "unsupportedContent", detail: "icon" },
      { path: "D.md", code: "unsupportedContent", detail: "icon" },
    ]);
  });

  it("gives a title that has control characters spaces, and an empty one the name Untitled", () => {
    const result = plan({ "A.md": '---\ntitle: "line\\nbreak"\n---\nx', "B.md": '---\ntitle: ""\n---\nx' });
    expect(outline(result)).toEqual(["line break", "B"]);
  });
});

describe("front matter", () => {
  it("turns keys into properties of inferred kinds, matched by name over the whole source", () => {
    const result = plan({
      "A.md": `---
title: "A"
Status: Done
Estimate: 3.5
Done: true
Due: 2026-10-07 14:30
Related: "[[B]]"
Labels:
  - x
  - y
Checklist:
  - "[x] Buy milk"
  - "[ ] Call Ann"
Place:
  name: Paris
  lat: 48.8566
  lon: 2.3522
Ignored: ~
aliases: [other]
---
body`,
      "B.md": "---\nStatus: 5\nEstimate: soon\n---\nbody",
    });
    const kinds = Object.fromEntries(result.properties.map((property) => [property.name, property.kind]));
    expect(kinds).toEqual({
      Status: "text",
      Estimate: "text",
      Done: "checkbox",
      Due: "date",
      Related: "relation",
      Labels: "multi_select",
      Checklist: "checklist",
      Place: "place",
    });
    const values = page(result, "A").values.map((value) => ({ name: result.properties[value.property]?.name, cell: value.cell }));
    expect(values).toEqual([
      { name: "Status", cell: { text: "Done" } },
      { name: "Estimate", cell: { text: "3.5", number: 3.5 } },
      { name: "Done", cell: { text: "Yes", checked: true } },
      { name: "Due", cell: { text: "2026-10-07 14:30" } },
      { name: "Related", cell: { text: "[[B]]", pages: [{ key: 1 }] } },
      { name: "Labels", cell: { text: "x, y", items: ["x", "y"] } },
      {
        name: "Checklist",
        cell: {
          text: "[x] Buy milk, [ ] Call Ann",
          items: ["[x] Buy milk", "[ ] Call Ann"],
          checklist: [
            { done: true, text: "Buy milk" },
            { done: false, text: "Call Ann" },
          ],
        },
      },
      { name: "Place", cell: { text: "Paris", place: { name: "Paris", lat: 48.8566, lon: 2.3522 } } },
    ]);
    expect(result.properties.find((property) => property.name === "Labels")?.options.map((option) => option.name)).toEqual(["x", "y"]);
  });

  it("sends tags to the built-in Tags, from a list or a string, without the hash", () => {
    const result = plan({ "A.md": "---\ntags: [work, '#review']\n---\nx", "B.md": "---\ntags: '#a, b c'\n---\nx" });
    const tags = result.properties.find((property) => property.name === "Tags");
    expect(tags).toMatchObject({ kind: "multi_select", builtin: true });
    expect(tags?.options.map((option) => option.name)).toEqual(["work", "review", "a", "b", "c"]);
    expect(page(result, "B").values[0]?.cell.items).toEqual(["a", "b", "c"]);
  });

  it("drops a block it cannot read, with a warning, and keeps the page", () => {
    const result = plan({ "A.md": '---\nkey: "unterminated\n---\nbody' });
    expect(result.warnings).toEqual([{ path: "A.md", code: "unsupportedContent", detail: "front matter" }]);
    expect(page(result, "A").blocks).toEqual([paragraph(text("body"))]);
    // Without a closing line the whole file is body.
    expect(page(plan({ "B.md": "---\nnot closed" }), "B").blocks).toHaveLength(2);
  });

  it("reads lists of files of the source as files, and needs them", () => {
    const result = plan({ "Doc.md": "---\nAttachments:\n  - files/report.pdf\n---\nx" }, ["files/report.pdf", "files/other.pdf"]);
    expect(result.properties).toMatchObject([{ name: "Attachments", kind: "files" }]);
    expect(page(result, "Doc").values[0]?.cell.files).toEqual(["files/report.pdf"]);
    // The shell uploads it for the property: only the files that are listed are needed.
    expect(result.neededFiles).toEqual(["files/report.pdf"]);
  });
});

describe("Markdown content", () => {
  it("turns relative links to pages of the source into page references, a card when alone in a paragraph", () => {
    const result = plan({
      "Notes/Plan.md": "See [Other](Other.md) now.\n\n[Only](Other.md)\n\n[Spaced](My%20Page.md)\n\n[Up](../Notes/Plan.md)\n",
      "Notes/Other.md": "x",
      "Notes/My Page.md": "y",
    });
    const other = `plan:${String(page(result, "Other").key)}`;
    const spaced = `plan:${String(page(result, "My Page").key)}`;
    const blocks = page(result, "Plan").blocks;
    expect(blocks).toEqual([
      paragraph(text("See "), { type: "pageRef", attrs: { pageId: other, fallback: "Other" } }, text(" now.")),
      { type: "pageLink", attrs: { pageId: other, fallback: "Other" } },
      { type: "pageLink", attrs: { pageId: spaced, fallback: "My Page" } },
      // A link that leaves the source is a link to nothing.
      paragraph(text("Up")),
    ]);
    expect(result.warnings).toEqual([{ path: "Notes/Plan.md", code: "unresolvedLink", detail: "../Notes/Plan.md" }]);
  });

  it("reads wiki links: a page of the source by title (case-insensitive, an alias and a heading ignored), else a title to look up", () => {
    const result = plan({
      "A.md": "[[b]] and [[B|alias]] and [[folder/B#Heading]] and [[Elsewhere]] and `[[code]]`",
      "B.md": "x",
    });
    expect(page(result, "A").blocks).toEqual([
      paragraph(
        { type: "pageRef", attrs: { pageId: "plan:1", fallback: "b" } },
        text(" and "),
        { type: "pageRef", attrs: { pageId: "plan:1", fallback: "B" } },
        text(" and "),
        { type: "pageRef", attrs: { pageId: "plan:1", fallback: "B" } },
        text(" and "),
        { type: "pageRef", attrs: { pageId: "title:Elsewhere", fallback: "[[Elsewhere]]" } },
        text(" and "),
        { type: "text", text: "[[code]]", marks: [{ type: "code" }] },
      ),
    ]);
  });

  it("keeps links to the web and by atomik-kb or attachment, and warns about relative links to nothing", () => {
    const id = "aaaaaaaa-0000-4000-8000-000000000001";
    const result = plan({
      "A.md": `[web](https://example.com/x), [mail](mailto:a@b.c), [app](atomik-kb://page/${id}#blk), [file](attachment://${id}), [gone](nothing.md), [anchor](#top), [js](javascript:alert(1))`,
    });
    const blocks = page(result, "A").blocks;
    const content = (blocks[0] as { content: JsonValue[] }).content;
    expect(content[0]).toEqual({ type: "text", text: "web", marks: [{ type: "link", attrs: { href: "https://example.com/x" } }] });
    expect(content[2]).toEqual({ type: "text", text: "mail", marks: [{ type: "link", attrs: { href: "mailto:a@b.c" } }] });
    expect(content[4]).toEqual({ type: "pageRef", attrs: { pageId: `existing:${id}`, fallback: "app" } });
    expect(content[6]).toMatchObject({ type: "text", text: "file", marks: [{ type: "link" }] });
    // Unresolved, an anchor and a script: the text stays, the link goes.
    expect(content.slice(8)).toEqual([text("gone"), text(", "), text("anchor"), text(", "), text("js")]);
    expect(result.warnings).toEqual([
      { path: "A.md", code: "unresolvedLink", detail: "nothing.md" },
      { path: "A.md", code: "unresolvedLink", detail: "javascript:alert(1)" },
    ]);
  });

  it("resolves a notion.so link that ends in the id of a page of the source", () => {
    const result = plan({
      [`A ${ID1}.md`]: `[Go](https://www.notion.so/workspace/B-${ID2}) and [Elsewhere](https://www.notion.so/B-${ID3})`,
      [`B ${ID2}.md`]: "x",
    });
    const content = (page(result, "A").blocks[0] as { content: JsonValue[] }).content;
    expect(content[0]).toEqual({ type: "pageRef", attrs: { pageId: "plan:1", fallback: "B" } });
    expect(content[2]).toMatchObject({ marks: [{ type: "link" }] });
  });

  it("makes image blocks of images that are files of the source, once their file is needed", () => {
    const result = plan(
      { "Notes/A.md": '![A picture](img/a%20b.png "The caption")\n\ntext ![inline](img/c.png) more\n' },
      ["Notes/img/a b.png", "Notes/img/c.png", "Notes/img/unused.png"],
    );
    const [first, second, third, fourth] = page(result, "A").blocks as { type: string; attrs?: Record<string, unknown>; content?: JsonValue[] }[];
    expect(first).toMatchObject({ type: "image", attrs: { alt: "A picture", caption: "The caption" } });
    expect(second).toMatchObject({ type: "paragraph" });
    expect(third).toMatchObject({ type: "image", attrs: { alt: "inline" } });
    expect(fourth).toMatchObject({ type: "paragraph" });
    expect(result.neededFiles).toEqual(["Notes/img/a b.png", "Notes/img/c.png"]);
    expect(result.placeholders.get(first?.attrs?.attachmentId as string)).toBe("Notes/img/a b.png");
    expect(result.placeholders.size).toBe(2);
  });

  it("makes a file block of a plain link to a file alone in its paragraph, an image block for a picture's extension", () => {
    const result = plan({ "A.md": "[Report](docs/report.pdf)\n\n[Photo](docs/p.JPG)\n\nSee [the report](docs/report.pdf) here.\n" }, [
      "docs/report.pdf",
      "docs/p.JPG",
    ]);
    const blocks = page(result, "A").blocks as { type: string }[];
    expect(blocks.map((block) => block.type)).toEqual(["file", "image", "paragraph", "file", "paragraph"]);
    expect(result.neededFiles).toEqual(["docs/report.pdf", "docs/p.JPG"]);
  });

  it("leaves remote and embedded images as links or alt text, warns about missing ones, and fetches nothing", () => {
    const result = plan({
      "A.md": "![Remote](https://example.com/x.png)\n\n![](https://example.com/y.png)\n\n![Data](data:image/png;base64,AAAA)\n\n![Lost](img/lost.png)\n",
    });
    const blocks = page(result, "A").blocks;
    expect(blocks[0]).toEqual(paragraph({ type: "text", text: "Remote", marks: [{ type: "link", attrs: { href: "https://example.com/x.png" } }] }));
    expect(blocks[1]).toEqual(paragraph({ type: "text", text: "https://example.com/y.png", marks: [{ type: "link", attrs: { href: "https://example.com/y.png" } }] }));
    expect(blocks[2]).toEqual(paragraph(text("Data")));
    expect(blocks[3]).toEqual(paragraph(text("Lost")));
    expect(result.warnings).toEqual([
      { path: "A.md", code: "remoteImage", detail: "https://example.com/x.png" },
      { path: "A.md", code: "remoteImage", detail: "https://example.com/y.png" },
      { path: "A.md", code: "embeddedImage" },
      { path: "A.md", code: "missingFile", detail: "img/lost.png" },
    ]);
    expect(result.neededFiles).toEqual([]);
  });

  it("leaves links inside code spans and code blocks alone", () => {
    const result = plan({ "A.md": "`![x](img/a.png)` and\n\n```\n![y](img/a.png)\n[[Wiki]]\n```\n" }, ["img/a.png"]);
    expect(result.neededFiles).toEqual([]);
    expect(page(result, "A").blocks).toHaveLength(2);
    expect(result.warnings).toEqual([]);
  });

  it("turns GitHub alerts and Notion asides into callouts", () => {
    const result = plan({
      "A.md": "> [!WARNING]\n> Careful\n\n> [!tip]\n\n<aside>\n💡 A tip\n\nsecond\n</aside>\n\n<aside>\nplain\n</aside>\n",
    });
    const blocks = page(result, "A").blocks;
    expect(blocks).toEqual([
      { type: "callout", attrs: { icon: "⚠️" }, content: [paragraph(text("Careful"))] },
      { type: "callout", attrs: { icon: "💡" }, content: [{ type: "paragraph" }] },
      { type: "callout", attrs: { icon: "💡" }, content: [paragraph(text("A tip")), paragraph(text("second"))] },
      { type: "blockquote", content: [paragraph(text("plain"))] },
    ]);
  });

  it("drops annotation comments", () => {
    const result = plan({ "A.md": '<!-- atomik:{"node":"callout"} -->\ntext\n<!-- another -->\n' });
    expect(page(result, "A").blocks).toEqual([paragraph(text("text"))]);
  });

  it("is deterministic", () => {
    const files = { "A.md": "[[B]] ![x](i.png)", "B.md": "y", "Dir/C.csv": "Name,Tag\nx,1\n" };
    expect(JSON.stringify(plan(files, ["i.png"], "S"))).toBe(JSON.stringify(plan(files, ["i.png"], "S")));
  });
});

describe("text and HTML content", () => {
  it("reads plain text as paragraphs, single line breaks as hard breaks, the file name as title", () => {
    const result = plan({ "Notes.txt": "\u{FEFF}line one\r\nline two\r\n\r\n\r\npara two\r\n" });
    expect(outline(result)).toEqual(["Notes"]);
    expect(page(result, "Notes").blocks).toEqual([
      paragraph(text("line one"), { type: "hardBreak" }, text("line two")),
      paragraph(text("para two")),
    ]);
  });

  it("reads HTML as its text in paragraphs, with its title, its icon, and a warning for what it drops", () => {
    const result = plan({
      "Page.html": `<!doctype html><html><head><title>Head title</title><style>p{}</style></head><body>
<div class="page-header-icon"><span class="icon">📝</span></div><h1 class="page-title">The title</h1>
<p>one &amp; two &lt;b&gt;</p><ul><li>item</li></ul><script>bad()</script><p>last<br>line</p></body></html>`,
    });
    const found = page(result, "The title");
    expect(found.icon).toBe("📝");
    expect(found.blocks).toEqual([paragraph(text("one & two <b>")), paragraph(text("item")), paragraph(text("last")), paragraph(text("line"))]);
    expect(result.warnings).toEqual([{ path: "Page.html", code: "unsupportedContent", detail: "html" }]);
  });

  it("takes the title tag, then the first h1 (which is then not part of the text), then the name", () => {
    expect(outline(plan({ "A.html": "<title>T</title><p>x</p>" }))).toEqual(["T"]);
    const fromH1 = plan({ "B.html": "<h1>Heading</h1><p>x</p>" });
    expect(outline(fromH1)).toEqual(["Heading"]);
    expect(page(fromH1, "Heading").blocks).toEqual([paragraph(text("x"))]);
    expect(outline(plan({ "C.html": "<p>x</p>" }))).toEqual(["C"]);
  });
});

describe("CSV databases", () => {
  it("makes a page with a table view of its children and a child page per row, with columns as properties of inferred kinds", () => {
    const result = plan({
      "Tasks.csv": [
        "Name,Done,Count,Due,Link,Mail,Tags,Status,Notes",
        'Write,Yes,3,2026-10-07,https://example.com,a@b.co,"x, y",Todo,free text one',
        'Test,No,4.5,"October 12, 2026",https://example.org,c@d.co,y,Done,another note',
        'Ship,Yes,-1,2026-10-09 09:30,mailto:z@y.xx,e@f.gh,"y, z",Todo,',
        "Plan,No,0,2026-10-10,https://example.net,g@h.ij,x,Done,yet another",
      ].join("\r\n"),
    });
    expect(outline(result)).toEqual(["Tasks", "  Write", "  Test", "  Ship", "  Plan"]);
    expect(page(result, "Tasks").blocks).toEqual([
      {
        type: "pageList",
        attrs: {
          source: { kind: "children" },
          layout: "table",
          properties: ["prop:Done", "prop:Count", "prop:Due", "prop:Link", "prop:Mail", "prop:Tags", "prop:Status", "prop:Notes"],
        },
      },
    ]);
    const kinds = Object.fromEntries(result.properties.map((property) => [property.name, property.kind]));
    expect(kinds).toEqual({
      Done: "checkbox",
      Count: "number",
      Due: "date",
      Link: "url",
      Mail: "email",
      Tags: "multi_select",
      Status: "select",
      Notes: "text",
    });
    expect(result.properties.find((property) => property.name === "Status")?.options.map((option) => option.name)).toEqual(["Todo", "Done"]);
    expect(result.properties.find((property) => property.name === "Tags")?.options.map((option) => option.name)).toEqual(["x", "y", "z"]);
    const write = page(result, "Write");
    expect(write.values).toHaveLength(8);
    expect(page(result, "Ship").values.map((value) => result.properties[value.property]?.name)).toEqual(["Done", "Count", "Due", "Link", "Mail", "Tags", "Status"]);
  });

  it("takes the first column named Name or Title as the page title, else the first, and names duplicate and empty headers", () => {
    const named = plan({ "A.csv": "Id,title,Id\n1,One,2\n" });
    expect(outline(named)).toEqual(["A", "  One"]);
    expect(named.properties.map((property) => property.name)).toEqual(["Id", "Id (2)"]);
    const first = plan({ "B.csv": "Thing,,Extra\nOne,x,y\n" });
    expect(outline(first)).toEqual(["B", "  One"]);
    expect(first.properties.map((property) => property.name)).toEqual(["Column 2", "Extra"]);
    // An empty line is skipped; a record of empty fields is a row without a title.
    expect(outline(plan({ "C.csv": "Name\n\n,\nx\n" }))).toEqual(["C", "  Untitled", "  x"]);
  });

  it("reads a semicolon delimiter, removes the formula guard, pads short rows and cuts long ones", () => {
    const result = plan({ "D.csv": "Name;Note;Extra\n'=cmd;ok\nshort\nlong;a;b;c;d\n" });
    expect(outline(result)).toEqual(["D", "  =cmd", "  short", "  long"]);
    expect(page(result, "long").values.map((value) => value.cell.text)).toEqual(["a", "b"]);
    expect(page(result, "short").values).toEqual([]);
  });

  it("keeps the order of the rows, and an empty row title is Untitled", () => {
    expect(outline(plan({ "E.csv": "Name\nZed\n\"\"\nAnn\n" }))).toEqual(["E", "  Zed", "  Untitled", "  Ann"]);
  });

  it("reads an empty file as an empty database", () => {
    const result = plan({ "Empty.csv": "" });
    expect(outline(result)).toEqual(["Empty"]);
    expect(page(result, "Empty").blocks).toEqual([{ type: "pageList", attrs: { source: { kind: "children" }, layout: "table", properties: [] } }]);
  });

  it("makes a select of a column of repeated values and text of a column of distinct long ones", () => {
    const result = plan({
      "F.csv": ["Name,Kind,Free", "a,x,one", "b,x,two", "c,y,three", "d,y,four"].join("\n"),
    });
    expect(Object.fromEntries(result.properties.map((property) => [property.name, property.kind]))).toEqual({ Kind: "select", Free: "text" });
    // At most three rows: a column of distinct values is a select too.
    expect(plan({ "G.csv": "Name,Kind\na,x\nb,y\n" }).properties[0]?.kind).toBe("select");
  });

  it("reads a Notion export: the _all file, the folder of row pages matched by title, property lines removed, relations", () => {
    const result = plan({
      [`Hub ${ID1}.md`]: `# Hub\n\nGo to [Tasks](Tasks%20${ID2}.csv).`,
      [`Tasks ${ID2}.csv`]: "Name,Status\nignored,1\n",
      [`Tasks ${ID2}_all.csv`]: [
        "Name,Status,Done,Related",
        `Task A,Todo,Yes,"Hub (Hub%20${ID1}.md)"`,
        "Task B,Done,No,",
        "Task C,Todo,Yes,",
      ].join("\r\n"),
      [`Tasks ${ID2}/Task A ${ID4}.md`]: "# Task A\n\nStatus: Todo\nDone: Yes\n\nReal content\n",
      [`Tasks ${ID2}/Task C ${ID3}.md`]: "# Task C\n\nNot a property line\n\nmore\n",
      [`Tasks ${ID2}/Extra ${ID5}.md`]: "# Extra\n\nno row\n",
    });
    expect(outline(result)).toEqual(["Hub", "Tasks", "  Task A", "  Task B", "  Task C", "  Extra"]);
    // The link to the plain file reaches the database.
    expect((page(result, "Hub").blocks[0] as { content: JsonValue[] }).content[1]).toEqual({
      type: "pageRef",
      attrs: { pageId: `plan:${String(page(result, "Tasks").key)}`, fallback: "Tasks" },
    });
    const names = (row: PlannedPage): (string | undefined)[] => row.values.map((value) => result.properties[value.property]?.name);
    expect(names(page(result, "Task A"))).toEqual(["Status", "Done", "Related"]);
    expect(page(result, "Task A").blocks).toEqual([paragraph(text("Real content"))]);
    expect(page(result, "Task A").path).toBe(`Tasks ${ID2}/Task A ${ID4}.md`);
    expect(page(result, "Task B").blocks).toEqual([]);
    expect(page(result, "Task B").path).toBe("");
    // A page whose first paragraph is no property line keeps it.
    expect((page(result, "Task C").blocks[0] as { content: JsonValue[] }).content).toEqual([text("Not a property line")]);
    expect(page(result, "Extra").values).toEqual([]);
    expect(Object.fromEntries(result.properties.map((property) => [property.name, property.kind]))).toEqual({
      Status: "select",
      Done: "checkbox",
      Related: "relation",
    });
    const related = page(result, "Task A").values.find((value) => result.properties[value.property]?.name === "Related");
    expect(related?.cell.pages).toEqual([{ key: page(result, "Hub").key }]);
  });
});

describe("the sidecar of an atomik-kb archive", () => {
  it("brings the schema: kinds kept whatever the values look like, options with colours, relations and rollups", () => {
    const result = plan({
      "atomik-export.json": JSON.stringify({
        format: 1,
        properties: [
          { name: "Tags", kind: "multi_select", options: [{ name: "work", color: "blue" }] },
          { name: "Status", kind: "status", options: [{ name: "Done", color: "green", group: "done" }] },
          { name: "Estimate", kind: "number" },
          { name: "Related", kind: "relation", config: { twoWay: true, pairName: "Back" } },
          { name: "Total", kind: "rollup", config: { relation: "Related", target: { property: "Estimate" }, function: "sum" } },
          { name: "Nope", kind: "bogus" },
        ],
        pages: [],
      }),
      "A.md": "---\ntags: [work, play]\nStatus: Done\nEstimate: soon\n---\nx",
    });
    expect(result.properties.map((property) => `${property.name}:${property.kind}`)).toEqual([
      "Tags:multi_select",
      "Status:status",
      "Estimate:number",
      "Related:relation",
      "Total:rollup",
    ]);
    expect(result.properties[0]).toMatchObject({ builtin: true, fixed: true, options: [{ name: "work", color: "blue" }, { name: "play" }] });
    expect(result.properties[1]).toMatchObject({ options: [{ name: "Done", color: "green", group: "done" }] });
    expect(result.properties[3]).toMatchObject({ relation: { twoWay: true, pairName: "Back" } });
    expect(result.properties[4]).toMatchObject({ rollup: { relation: "Related", target: "Estimate", function: "sum" } });
  });

  it("ignores a sidecar that is not one", () => {
    expect(plan({ "atomik-export.json": "not json", "A.md": "x" }).properties).toEqual([]);
    expect(plan({ "atomik-export.json": "[1,2]", "A.md": "x" }).properties).toEqual([]);
  });
});
