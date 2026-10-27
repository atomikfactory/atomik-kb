// The texts of an export (ADR-0036, 2.4 to 2.8): property values and their text forms, front
// matter, the HTML document (escaping, links), `.excalidraw` files and the sidecar.

import { describe, expect, it } from "vitest";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyValue } from "./generated/PropertyValue";
import {
  escapeHtml,
  excalidrawFile,
  exportValue,
  frontMatter,
  htmlPage,
  rustNumber,
  sidecarFile,
  valueText,
} from "./memory-export-text";
import type { ExportValue, HtmlLinks, ValueLookups } from "./memory-export-text";
import { TAGS_PROPERTY_ID } from "./properties";

const look: ValueLookups = {
  optionName: (id) => ({ o1: "Done", o2: "work", o3: "review" })[id],
  pageTitle: (id) => ({ p1: "Other page", p2: "Third, page" })[id],
  fileName: (id) => ({ a1: "report.pdf", a2: "photo.png" })[id],
  filePath: (id) => ({ a1: "Weekly review/report.pdf" })[id],
  deviceName: (id) => (id === "d1" ? "Laptop" : undefined),
};

const def = (id: string, name: string, kind: PropertyDef["kind"]): PropertyDef => ({ id, name, kind, options: [], createdAt: 0 });

describe("rustNumber", () => {
  it("writes numbers as Rust's shortest Display does: no exponent, no trailing .0", () => {
    expect(rustNumber(3)).toBe("3");
    expect(rustNumber(3.5)).toBe("3.5");
    expect(rustNumber(-0.25)).toBe("-0.25");
    expect(rustNumber(1e21)).toBe("1000000000000000000000");
    expect(rustNumber(1.2345e25)).toBe("12345000000000000000000000");
    expect(rustNumber(1e-7)).toBe("0.0000001");
    expect(rustNumber(-1.5e-9)).toBe("-0.0000000015");
    expect(rustNumber(2.5e21)).toBe("2500000000000000000000");
    expect(rustNumber(48.8566)).toBe("48.8566");
  });
});

describe("exportValue and valueText", () => {
  const text = (value: PropertyValue): string | undefined => {
    const resolved = exportValue(value, look);
    return resolved === undefined ? undefined : valueText(resolved, (ms) => `@${String(ms)}`);
  };

  it("writes each kind the way the CSV shows it", () => {
    expect(text({ kind: "text", value: "hello" })).toBe("hello");
    expect(text({ kind: "url", value: "https://example.com" })).toBe("https://example.com");
    expect(text({ kind: "number", value: 3.5 })).toBe("3.5");
    expect(text({ kind: "checkbox", value: true })).toBe("Yes");
    expect(text({ kind: "checkbox", value: false })).toBe("No");
    expect(text({ kind: "date", value: { date: "2026-10-07" } })).toBe("2026-10-07");
    expect(text({ kind: "date", value: { date: "2026-10-07", time: "14:30" } })).toBe("2026-10-07 14:30");
    expect(text({ kind: "select", value: "o1" })).toBe("Done");
    expect(text({ kind: "status", value: "o1" })).toBe("Done");
    expect(text({ kind: "multi_select", value: ["o2", "o3"] })).toBe("work, review");
    expect(text({ kind: "relation", value: ["p1", "p2"] })).toBe("Other page, Third, page");
    expect(text({ kind: "files", value: ["a1", "a2"] })).toBe("report.pdf, photo.png");
    expect(
      text({
        kind: "checklist",
        value: [
          { id: "i1", text: "Buy milk", done: true },
          { id: "i2", text: "Call Ann", done: false },
        ],
      }),
    ).toBe("[x] Buy milk; [ ] Call Ann");
    expect(text({ kind: "place", value: { name: "Paris", lat: 48.8566, lon: 2.3522 } })).toBe("Paris (48.8566, 2.3522)");
    expect(text({ kind: "place", value: { name: "Home" } })).toBe("Home");
    expect(text({ kind: "created_time", value: 5 })).toBe("@5");
    expect(text({ kind: "created_by", value: "d1" })).toBe("Laptop");
  });

  it("writes rollup results as text, progress as done/total", () => {
    expect(text({ kind: "rollup", value: { function: "sum", result: { type: "number", data: 7.5 } } })).toBe("7.5");
    expect(text({ kind: "rollup", value: { function: "earliest", result: { type: "date", data: { date: "2026-01-02" } } } })).toBe(
      "2026-01-02",
    );
    expect(text({ kind: "rollup", value: { function: "checklist_progress", result: { type: "progress", data: { done: 3, total: 5 } } } })).toBe(
      "3/5",
    );
    expect(
      text({
        kind: "rollup",
        value: { function: "show_original", result: { type: "values", data: [{ kind: "select", value: "o1" }, { kind: "text", value: "x" }] } },
      }),
    ).toBe("Done, x");
  });

  it("reads empty and unresolvable values as nothing", () => {
    expect(text({ kind: "text", value: "" })).toBeUndefined();
    expect(text({ kind: "select", value: "gone" })).toBeUndefined();
    expect(text({ kind: "multi_select", value: ["gone"] })).toBeUndefined();
    expect(text({ kind: "relation", value: ["nobody"] })).toBeUndefined();
    expect(text({ kind: "files", value: ["nothing"] })).toBeUndefined();
    expect(text({ kind: "created_by", value: "unknown" })).toBeUndefined();
    expect(exportValue({ kind: "files", value: ["a1", "a2"] }, look)).toEqual({
      kind: "files",
      names: ["report.pdf", "photo.png"],
      paths: ["Weekly review/report.pdf", "photo.png"],
    });
  });
});

describe("frontMatter", () => {
  const tags = def(TAGS_PROPERTY_ID, "Tags", "multi_select");

  it("writes title, icon, times, tags and the other properties in schema order under their names", () => {
    const value = (kind: PropertyValue): ExportValue | undefined => exportValue(kind, look);
    const text = frontMatter({
      title: 'Weekly "review"',
      icon: "📝",
      createdAt: Date.UTC(2026, 9, 1, 9, 30, 0),
      updatedAt: Date.UTC(2026, 9, 5, 18, 2, 11, 456),
      values: [
        { property: tags, value: value({ kind: "multi_select", value: ["o2", "o3"] }) },
        { property: def("s", "Status", "status"), value: value({ kind: "status", value: "o1" }) },
        { property: def("d", "Due", "date"), value: value({ kind: "date", value: { date: "2026-10-07", time: "14:30" } }) },
        { property: def("c", "Done", "checkbox"), value: value({ kind: "checkbox", value: true }) },
        { property: def("e", "Estimate", "number"), value: value({ kind: "number", value: 3.5 }) },
        { property: def("r", "Related", "relation"), value: value({ kind: "relation", value: ["p1"] }) },
        { property: def("f", "Attachments", "files"), value: value({ kind: "files", value: ["a1"] }) },
        {
          property: def("k", "Checklist", "checklist"),
          value: value({
            kind: "checklist",
            value: [
              { id: "i1", text: "Buy milk", done: true },
              { id: "i2", text: "Call Ann", done: false },
            ],
          }),
        },
        { property: def("l", "Place", "place"), value: value({ kind: "place", value: { name: "Paris", lat: 48.8566, lon: 2.3522 } }) },
        { property: def("m", "Choices", "multi_select"), value: value({ kind: "multi_select", value: ["o2"] }) },
      ],
    });
    expect(text).toBe(`---
title: "Weekly \\"review\\""
icon: "📝"
created: "2026-10-01T09:30:00Z"
updated: "2026-10-05T18:02:11Z"
tags:
  - "work"
  - "review"
"Status": "Done"
"Due": "2026-10-07 14:30"
"Done": true
"Estimate": 3.5
"Related":
  - "[[Other page]]"
"Attachments":
  - "Weekly review/report.pdf"
"Checklist":
  - "[x] Buy milk"
  - "[ ] Call Ann"
"Place":
  name: "Paris"
  lat: 48.8566
  lon: 2.3522
"Choices":
  - "work"
---
`);
  });

  it("leaves out an icon that is not set, empty values and computed kinds, and always writes the title", () => {
    const text = frontMatter({
      title: "",
      icon: undefined,
      createdAt: 0,
      updatedAt: 0,
      values: [
        { property: def("a", "Empty", "text"), value: undefined },
        { property: def("b", "Created", "created_time"), value: { kind: "instant", ms: 5 } },
        { property: def("c", "Total", "rollup"), value: { kind: "computed", text: "3" } },
        { property: def("d", "Place", "place"), value: { kind: "place", name: "Home" } },
      ],
    });
    expect(text).toBe(`---
title: ""
created: "1970-01-01T00:00:00Z"
updated: "1970-01-01T00:00:00Z"
"Place":
  name: "Home"
---
`);
  });

  it("writes tags only when the page has some", () => {
    const none = frontMatter({ title: "T", icon: undefined, createdAt: 0, updatedAt: 0, values: [{ property: tags, value: undefined }] });
    expect(none).not.toContain("tags");
  });
});

describe("htmlPage", () => {
  const links: HtmlLinks = {
    pageHref: (id) => (id === "p1" ? "Other%20page.html" : undefined),
    attachmentHref: (id) => ({ a1: "img/photo.png", a2: "clip.mp4", a3: "report.pdf" })[id],
    attachmentName: (id) => ({ a2: "clip.mp4", a3: "report.pdf" })[id],
    pageTitle: (id) => ({ p1: "Other page", p9: "Outside" })[id],
    whiteboardHref: (index) => (index === 0 ? "Whiteboard.excalidraw" : undefined),
  };
  const page = (content: JsonValue, title = "Title"): string =>
    htmlPage({ title, icon: undefined, properties: [], content }, links);
  const doc = (...content: JsonValue[]): JsonValue => ({ type: "doc", content });

  it("is a standalone document with an embedded style, no script and no external resource", () => {
    const html = page(doc({ type: "paragraph", content: [{ type: "text", text: "Hi" }] }), "A & B");
    expect(html.startsWith("<!doctype html>\n<html lang=\"en\">")).toBe(true);
    expect(html).toContain('<meta charset="utf-8">');
    expect(html).toContain('<meta name="generator" content="atomik-kb">');
    expect(html).toContain("<title>A &amp; B</title>");
    expect(html).toContain('<h1 class="page-title">A &amp; B</h1>');
    expect(html).toContain("<p>Hi</p>");
    expect(html).toContain("prefers-color-scheme:dark");
    expect(html).not.toContain("<script");
    expect(html).not.toMatch(/https?:\/\//u);
  });

  it("escapes the title, the text and the attribute values", () => {
    const html = page(
      doc({
        type: "paragraph",
        content: [{ type: "text", text: `<script>alert("x")</script> & 'q'`, marks: [{ type: "link", attrs: { href: 'https://e.com/?a="b"&c=<d>' } }] }],
      }),
      "<img src=x onerror=alert(1)>",
    );
    expect(html).toContain("<title>&lt;img src=x onerror=alert(1)&gt;</title>");
    expect(html).not.toContain("<img src=x");
    expect(html).toContain("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;q&#39;");
    expect(html).toContain('href="https://e.com/?a=&quot;b&quot;&amp;c=&lt;d&gt;"');
    expect(escapeHtml(`&<>"'`)).toBe("&amp;&lt;&gt;&quot;&#39;");
  });

  it("keeps link hrefs only for http, https, mailto and tel", () => {
    const link = (href: string): string =>
      page(doc({ type: "paragraph", content: [{ type: "text", text: "go", marks: [{ type: "link", attrs: { href } }] }] }));
    expect(link("https://example.com")).toContain('<a href="https://example.com">go</a>');
    expect(link("http://example.com")).toContain('<a href="http://example.com">go</a>');
    expect(link("mailto:a@b.c")).toContain('<a href="mailto:a@b.c">go</a>');
    expect(link("tel:+123")).toContain('<a href="tel:+123">go</a>');
    for (const bad of ["javascript:alert(1)", "JavaScript:alert(1)", "data:text/html,x", "file:///etc/passwd", "attachment://x", "../x"]) {
      const html = link(bad);
      expect(html).not.toContain("<a ");
      expect(html).toContain("<p>go</p>");
    }
  });

  it("renders marks, headings, lists, quotes, code, rules and tables", () => {
    const html = page(
      doc(
        {
          type: "heading",
          attrs: { id: "h1", level: 2 },
          content: [{ type: "text", text: "Head" }],
        },
        {
          type: "paragraph",
          content: [
            { type: "text", text: "b", marks: [{ type: "bold" }] },
            { type: "text", text: "i", marks: [{ type: "italic" }] },
            { type: "text", text: "c", marks: [{ type: "code" }] },
            { type: "hardBreak" },
            { type: "text", text: "s", marks: [{ type: "strike" }, { type: "underline" }] },
          ],
        },
        { type: "bulletList", content: [{ type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "one" }] }] }] },
        {
          type: "orderedList",
          attrs: { start: 3 },
          content: [{ type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "three" }] }] }],
        },
        {
          type: "taskList",
          content: [
            { type: "taskItem", attrs: { checked: true }, content: [{ type: "paragraph", content: [{ type: "text", text: "done" }] }] },
          ],
        },
        { type: "blockquote", content: [{ type: "paragraph", content: [{ type: "text", text: "q" }] }] },
        { type: "codeBlock", attrs: { language: "rust" }, content: [{ type: "text", text: "a < b" }] },
        { type: "horizontalRule" },
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [
                { type: "tableHeader", content: [{ type: "paragraph", content: [{ type: "text", text: "H" }] }] },
                { type: "tableCell", content: [{ type: "paragraph", content: [{ type: "text", text: "C" }] }] },
              ],
            },
          ],
        },
      ),
    );
    expect(html).toContain('<h2 id="h-h1">Head</h2>');
    expect(html).toContain("<strong>b</strong><em>i</em><code>c</code><br><u><s>s</s></u>");
    expect(html).toContain("<ul>\n<li><p>one</p></li>\n</ul>");
    expect(html).toContain('<ol start="3">');
    expect(html).toContain('<li class="task" data-checked="true"><input type="checkbox" disabled checked>');
    expect(html).toContain("<blockquote>\n<p>q</p>\n</blockquote>");
    expect(html).toContain('<pre><code class="language-rust">a &lt; b</code></pre>');
    expect(html).toContain("<hr>");
    expect(html).toContain("<tr><th><p>H</p></th><td><p>C</p></td></tr>");
  });

  it("renders callouts, toggles, images, files, page references, dates and whiteboards with the links of the export", () => {
    const html = page(
      doc(
        { type: "callout", attrs: { icon: "💡", color: "blue" }, content: [{ type: "paragraph", content: [{ type: "text", text: "note" }] }] },
        {
          type: "details",
          attrs: { open: true },
          content: [
            { type: "detailsSummary", content: [{ type: "text", text: "More" }] },
            { type: "detailsContent", content: [{ type: "paragraph", content: [{ type: "text", text: "hidden" }] }] },
          ],
        },
        { type: "image", attrs: { attachmentId: "a1", alt: 'a "pic"', caption: "Cap", widthPercent: 50, align: "left" } },
        { type: "image", attrs: { attachmentId: "unknown", alt: "x" } },
        { type: "file", attrs: { attachmentId: "a3" } },
        { type: "file", attrs: { attachmentId: "a2", view: "preview" } },
        {
          type: "paragraph",
          content: [
            { type: "pageRef", attrs: { pageId: "p1" } },
            { type: "text", text: " and " },
            { type: "pageRef", attrs: { pageId: "p9" } },
            { type: "dateMention", attrs: { date: "2026-10-07", time: "14:30" } },
            { type: "mathInline", attrs: { latex: "a<b" } },
          ],
        },
        { type: "pageLink", attrs: { pageId: "p1" } },
        { type: "whiteboard", attrs: { id: "w1" } },
        { type: "mathBlock", content: [{ type: "text", text: "x^2" }] },
      ),
    );
    expect(html).toContain('<aside class="callout" data-color="blue"><span class="callout-icon">💡</span>');
    expect(html).toContain("<details open><summary>More</summary>");
    expect(html).toContain('<figure class="image align-left" style="width:50%"><img src="img/photo.png" alt="a &quot;pic&quot;"><figcaption>Cap</figcaption></figure>');
    expect(html).toContain('<figure class="image"></figure>');
    expect(html).toContain('<a class="file" href="report.pdf">report.pdf</a>');
    expect(html).toContain('<video controls src="clip.mp4"></video>');
    expect(html).toContain('<a class="page-ref" href="Other%20page.html">Other page</a>');
    expect(html).toContain('<span class="page-ref">Outside</span>');
    expect(html).toContain('<time datetime="2026-10-07T14:30">2026-10-07 14:30</time>');
    expect(html).toContain('<span class="math math-inline">\\(a&lt;b\\)</span>');
    expect(html).toContain('<p class="page-link"><a class="page-ref" href="Other%20page.html">Other page</a></p>');
    expect(html).toContain('<figure class="whiteboard"><a href="Whiteboard.excalidraw">Whiteboard</a></figure>');
    expect(html).toContain('<div class="math math-display">\\[x^2\\]</div>');
  });

  it("writes a toggle heading as a heading inside the summary, and lists it in the table of contents (ADR-0040)", () => {
    const toggle = (id: string, level: JsonValue | undefined, title: string): JsonValue => ({
      type: "details",
      attrs: { open: false },
      content: [
        { type: "detailsSummary", attrs: level === undefined ? { id } : { id, level }, content: [{ type: "text", text: title }] },
        { type: "detailsContent", content: [{ type: "paragraph", content: [{ type: "text", text: "body" }] }] },
      ],
    });
    const html = page(
      doc(
        { type: "tableOfContents", attrs: { id: "t" } },
        toggle("s1", 2, "Folded & more"),
        toggle("s2", undefined, "Plain"),
        toggle("s3", 4, "Damaged"),
      ),
    );
    expect(html).toContain('<details><summary><h2 id="h-s1">Folded &amp; more</h2></summary>');
    expect(html).toContain("<details><summary>Plain</summary>");
    expect(html).toContain("<details><summary>Damaged</summary>");
    expect(html).toContain('<nav class="toc"><ul>\n<li><a href="#h-s1">Folded &amp; more</a></li>\n</ul></nav>');
  });

  it("writes the icon, the property table and a table of contents of the headings", () => {
    const html = htmlPage(
      {
        title: "T",
        icon: "📝",
        properties: [{ name: "Status", text: "Done <b>" }],
        content: doc(
          { type: "tableOfContents", attrs: { id: "t" } },
          { type: "heading", attrs: { id: "h1", level: 1 }, content: [{ type: "text", text: "First" }] },
        ),
      },
      links,
    );
    expect(html).toContain('<div class="page-header-icon">📝</div>');
    expect(html).toContain('<table class="properties">\n<tr><th>Status</th><td>Done &lt;b&gt;</td></tr>\n</table>');
    expect(html).toContain('<nav class="toc"><ul>\n<li><a href="#h-h1">First</a></li>\n</ul></nav>');
  });
});

describe("excalidrawFile and sidecarFile", () => {
  it("writes the live elements in z-order, pretty-printed", () => {
    const file = excalidrawFile({
      b: { id: "b", type: "text", index: "a1", text: "second" },
      a: { id: "a", type: "rectangle", index: "a0" },
      gone: { id: "gone", type: "text", index: "a2", isDeleted: true },
    });
    const parsed = JSON.parse(file) as { type: string; version: number; source: string; elements: { id: string }[]; appState: unknown; files: unknown };
    expect(parsed.type).toBe("excalidraw");
    expect(parsed.version).toBe(2);
    expect(parsed.source).toBe("atomik-kb");
    expect(parsed.elements.map((element) => element.id)).toEqual(["a", "b"]);
    expect(parsed.appState).toEqual({ viewBackgroundColor: "#ffffff" });
    expect(parsed.files).toEqual({});
    expect(file).toContain('\n  "elements": [');
    expect(file.endsWith("}\n")).toBe(true);
  });

  it("lists the whole schema by names and the page files in tree order", () => {
    const schema: PropertyDef[] = [
      { ...def(TAGS_PROPERTY_ID, "Tags", "multi_select"), options: [{ id: "o2", name: "work", color: "blue" }] },
      {
        ...def("s", "Status", "status"),
        options: [{ id: "o1", name: "Done", color: "green", group: "done" }],
      },
      { ...def("r", "Related", "relation"), config: { kind: "relation", twoWay: true, pairedPropertyId: "rb" } },
      def("rb", "Related (back)", "relation"),
      { ...def("t", "Total", "rollup"), config: { kind: "rollup", relationPropertyId: "r", target: "e", function: "sum" } },
      def("e", "Estimate", "number"),
    ];
    const text = sidecarFile({ name: "Projects", exportedAt: Date.UTC(2026, 9, 12, 9, 30, 0), schema, pages: ["Projects.md", "Projects/Alpha.md"] });
    expect(JSON.parse(text)).toEqual({
      format: 1,
      generator: "atomik-kb",
      name: "Projects",
      exportedAt: "2026-10-12T09:30:00Z",
      properties: [
        { name: "Tags", kind: "multi_select", options: [{ name: "work", color: "blue" }] },
        { name: "Status", kind: "status", options: [{ name: "Done", color: "green", group: "done" }] },
        { name: "Related", kind: "relation", config: { twoWay: true, pairName: "Related (back)" } },
        { name: "Related (back)", kind: "relation" },
        { name: "Total", kind: "rollup", config: { relation: "Related", target: { property: "Estimate" }, function: "sum" } },
        { name: "Estimate", kind: "number" },
      ],
      pages: ["Projects.md", "Projects/Alpha.md"],
    });
  });
});
