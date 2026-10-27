import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import {
  MARKDOWN_GOLDEN,
  UNKNOWN_JSON,
  fixtureStatus,
  readFixtureJson,
  readFixtureText,
  samplePage,
} from "./testing/fixtures";

const TARGET_PAGE = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
const IMAGE_ATTACHMENT = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
const FILE_ATTACHMENT = "6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6";
const FENCE = "```";

const unresolved = (): undefined => undefined;

function doc(...content: JsonValue[]): JsonValue {
  return { type: "doc", content };
}

/** Markdown of a page made of `blocks`, with no page reference resolvable. */
function md(...blocks: JsonValue[]): string {
  return renderMarkdown(doc(...blocks), unresolved);
}

function p(...content: JsonValue[]): JsonValue {
  const node: { [key: string]: JsonValue } = { type: "paragraph", attrs: { id: "p" } };
  if (content.length > 0) {
    node.content = content;
  }
  return node;
}

function text(value: string, ...marks: string[]): JsonValue {
  const node: { [key: string]: JsonValue } = { type: "text", text: value };
  if (marks.length > 0) {
    node.marks = marks.map((type) => ({ type }));
  }
  return node;
}

function node(type: string, attrs: { [key: string]: JsonValue }, ...content: JsonValue[]): JsonValue {
  return { type, attrs, content };
}

function item(...content: JsonValue[]): JsonValue {
  return node("listItem", { id: "i" }, ...content);
}

function task(checked: boolean, ...content: JsonValue[]): JsonValue {
  return node("taskItem", { id: "t", checked }, ...content);
}

function lines(...rows: string[]): string {
  return `${rows.join("\n")}\n`;
}

/** The sample rendered with its reference resolved to `title` (or to its id) and its file named `fileName`. */
function sampleMarkdown(title: string, fileName: string = FILE_ATTACHMENT): string {
  return lines(
    "# Welcome to **atomik-kb**",
    "",
    "Rich text with *italic*, <u>underline</u>, ~~strike~~, `code` and a [link](https://example.com/docs).  ",
    `See also [[${title}]] and [[${title}]].`,
    "",
    "- First item",
    "  - Nested item",
    "- Second item due 2026-10-07, review 2026-10-08 14:30",
    "",
    "3. Third step",
    "",
    "- [x] Done",
    "- [ ] To do",
    "",
    "> A quoted line with $E=mc^2$ inline.",
    "",
    `${FENCE}rust`,
    "fn main() {",
    '    println!("hi");',
    "}",
    FENCE,
    "",
    "$$",
    "E = mc^2",
    "$$",
    "",
    "---",
    "",
    "<details open>",
    "<summary><h2>Toggle summary</h2></summary>",
    "",
    "Hidden until opened.",
    "",
    "</details>",
    "",
    "> 💡 Callouts carry an icon.",
    "",
    "| Feature | Status | Notes |",
    "| --- | --- | --- |",
    "| Tables | **Done** | Header row<br><br>Plain cells |",
    "| Pipes | a \\| b | Escaped in Markdown |",
    "",
    // The sample image's caption is the link title, with its quote and backslash escaped.
    `![Diagram of the sync flow](attachment://${IMAGE_ATTACHMENT} "Figure 1: the \\"sync\\" flow, C:\\\\sync")`,
    "",
    `[${fileName}](attachment://${FILE_ATTACHMENT})`,
    "",
    // The gallery (2026-10-09, ADR-0030) is one image line per item, the caption its link title.
    `![](attachment://${IMAGE_ATTACHMENT} "Opening night, \\"north\\" wing")`,
    "",
    "![](attachment://7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7)",
    "",
    '![](attachment://8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708 "Backstage")',
    "",
    // The whiteboard (2026-10-10, ADR-0033) is a quote of its live text elements in z-order.
    "> **Whiteboard**",
    ">",
    "> - Plan A ship it",
    "",
    "Left column",
    "",
    "Right column",
    "",
    "Two blocks in this one",
    "",
    `[[${title}]]`,
    "",
    "Unicode: café, 日本語, 👩‍💻 and 𝔘𝔫𝔦𝔠𝔬𝔡𝔢.",
  );
}

describe("renderMarkdown on the page-v1 fixture", () => {
  it("renders every node and mark of the sample", () => {
    const titles: Record<string, string> = { [TARGET_PAGE]: "Roadmap" };
    const names: Record<string, string> = { [FILE_ATTACHMENT]: "Q3.pdf" };
    expect(
      renderMarkdown(samplePage(), (id) => titles[id], { resolveAttachmentName: (id) => names[id] }),
    ).toBe(sampleMarkdown("Roadmap", "Q3.pdf"));
  });

  it("renders unresolved page references as their id", () => {
    expect(renderMarkdown(samplePage(), unresolved)).toBe(sampleMarkdown(TARGET_PAGE));
  });

  const golden = fixtureStatus(MARKDOWN_GOLDEN, "kb-model (cargo run -p kb-model --example make_golden)");
  it.skipIf(golden.missing)(`equals the shared golden ${MARKDOWN_GOLDEN} of the Rust side${golden.note}`, () => {
    const expected = readFixtureText(MARKDOWN_GOLDEN);
    // The sample references one page; the golden shows how kb-model's script resolved it
    // (a title, or the id when it left it unresolved), which is an input of the golden,
    // not part of the mapping.
    const shown = /\[\[([^\]]*)\]\]/u.exec(expected)?.[1];
    const titleOf = (id: string): string | undefined =>
      id === TARGET_PAGE && shown !== undefined && shown !== TARGET_PAGE ? shown : undefined;
    // Likewise the name its script gave the sample's file attachment (or none: the id).
    const named = new RegExp(`\\[([^\\]]*)\\]\\(attachment://${FILE_ATTACHMENT}\\)`, "u").exec(expected)?.[1];
    const resolveAttachmentName = (id: string): string | undefined =>
      id === FILE_ATTACHMENT && named !== undefined && named !== FILE_ATTACHMENT ? named : undefined;
    expect(renderMarkdown(samplePage(), titleOf, { resolveAttachmentName })).toBe(expected);
  });
});

describe("renderMarkdown on the page-v1-unknown fixture", () => {
  const json = fixtureStatus(UNKNOWN_JSON, "kb-model (cargo run -p kb-model --example make_golden)");
  it.skipIf(json.missing)(`renders the children of unknown nodes and the text of marks it cannot show${json.note}`, () => {
    expect(renderMarkdown(readFixtureJson(UNKNOWN_JSON), unresolved)).toBe(
      lines(
        "## Written by a newer client",
        "",
        "Plain, highlighted and **both**.",
        "",
        `Inside the widget, see [[${TARGET_PAGE}]]`,
        "",
        "Loose widget text",
        "",
        "After the widget.",
      ),
    );
  });
});

describe("renderMarkdown blocks", () => {
  it("ends with exactly one newline, also for empty pages", () => {
    expect(renderMarkdown({ type: "doc" }, unresolved)).toBe("\n");
    expect(md(p())).toBe("\n");
    expect(md(p(text("a"), { type: "hardBreak" }))).toBe("a  \n");
  });

  it("joins top-level blocks with one blank line, empty paragraphs included", () => {
    expect(md(p(text("a")), p(), p(text("b")))).toBe("a\n\n\n\nb\n");
  });

  it("renders headings by level, clamped to 1..6", () => {
    const heading = (level: JsonValue): JsonValue => node("heading", { id: "h", level }, text("T"));
    expect(md(heading(1), heading(2), heading(3))).toBe(lines("# T", "", "## T", "", "### T"));
    expect(md(node("heading", { id: "h" }, text("T")))).toBe("# T\n");
    expect(md(heading(9), heading(0), heading(2.5))).toBe(lines("###### T", "", "# T", "", "# T"));
  });

  it("indents nested blocks by the marker width, without blank lines inside lists", () => {
    const nested = node("bulletList", { id: "n" }, item(p(text("x"))));
    const ordered = node(
      "orderedList",
      { id: "o", start: 9 },
      item(p(text("nine")), nested),
      item(p(text("ten")), nested, p(text("more"))),
    );
    expect(md(ordered)).toBe(lines("9. nine", "   - x", "10. ten", "    - x", "    more"));
    expect(md(node("bulletList", { id: "b" }, item(p(text("a")), p(text("b")))))).toBe(lines("- a", "  b"));
  });

  it("renders all blocks of an item, its continuation lines indented, whatever comes first", () => {
    expect(md(node("bulletList", { id: "b" }, item(p(text("a"), { type: "hardBreak" }, text("b")))))).toBe(
      lines("- a  ", "  b"),
    );
    const headed = item(node("heading", { id: "h", level: 2 }, text("H")), p(text("x")));
    expect(md(node("bulletList", { id: "b" }, headed))).toBe(lines("- ## H", "  x"));
    expect(md(node("bulletList", { id: "b" }, item()))).toBe("- \n");
    // Something else than an item stands for itself.
    expect(md(node("bulletList", { id: "b" }, p(text("stray"))))).toBe("- stray\n");
  });

  it("counts ordered lists from 1 by default", () => {
    expect(md(node("orderedList", { id: "o" }, item(p(text("a"))), item(p(text("b")))))).toBe(lines("1. a", "2. b"));
  });

  it("renders task items with their state and indents nested content by two", () => {
    const nested = node("taskList", { id: "n" }, task(true, p(text("child"))));
    expect(md(node("taskList", { id: "l" }, task(false, p(text("parent")), nested)))).toBe(
      lines("- [ ] parent", "  - [x] child"),
    );
  });

  it("prefixes every line of a quote, including empty ones", () => {
    const quote = node("blockquote", { id: "q" }, p(text("a")), node("bulletList", { id: "b" }, item(p(text("b")))));
    expect(md(quote)).toBe(lines("> a", ">", "> - b"));
  });

  it("starts a callout with its icon (default 💡)", () => {
    expect(md(node("callout", { id: "c", icon: "⚠️" }, p(text("a")), p(text("b"))))).toBe(lines("> ⚠️ a", ">", "> b"));
    expect(md(node("callout", { id: "c" }, p(text("a"))))).toBe("> 💡 a\n");
  });

  it("ignores the colour of a callout (a palette name, an unknown name or a non-string)", () => {
    const plain = md(node("callout", { id: "c", icon: "⚠️" }, p(text("a"))));
    expect(plain).toBe("> ⚠️ a\n");
    for (const color of ["blue", "red", "chartreuse", 7]) {
      expect(md(node("callout", { id: "c", icon: "⚠️", color }, p(text("a")))), String(color)).toBe(plain);
    }
    expect(md(node("callout", { id: "c", color: "green" }, p(text("a"))))).toBe("> 💡 a\n");
  });

  it("fences code with the language, or none, and ignores marks inside", () => {
    expect(md(node("codeBlock", { id: "c" }, text("let x = 1;", "bold")))).toBe(lines(FENCE, "let x = 1;", FENCE));
    expect(md(node("codeBlock", { id: "c", language: "ts" }, text("a\nb")))).toBe(lines(`${FENCE}ts`, "a", "b", FENCE));
  });

  it("renders a math block as $$, its LaTeX as stored (marks ignored) and $$", () => {
    // The same inputs and outputs as the tests of kb-model (`src/tests/page.rs`).
    expect(md(node("mathBlock", { id: "m" }, text("E = mc^2")))).toBe(lines("$$", "E = mc^2", "$$"));
    expect(md(node("mathBlock", { id: "m" }, text("x", "bold")))).toBe(lines("$$", "x", "$$"));
    // Empty: an empty line between the fences, like an empty code block.
    expect(md(node("mathBlock", { id: "m" }))).toBe(lines("$$", "", "$$"));
    // Several lines keep their newlines; nothing is escaped, `$` and backslashes included.
    expect(md(node("mathBlock", { id: "m" }, text("\\begin{aligned}\na &= $b$ \\\\\nc &= d\n\\end{aligned}")))).toBe(
      lines("$$", "\\begin{aligned}", "a &= $b$ \\\\", "c &= d", "\\end{aligned}", "$$"),
    );
    // Between other blocks, and inside a container, like any block.
    expect(md(p(text("before")), node("mathBlock", { id: "m" }, text("a^2 + b^2 = c^2")), p(text("after")))).toBe(
      lines("before", "", "$$", "a^2 + b^2 = c^2", "$$", "", "after"),
    );
    expect(md(node("blockquote", { id: "q" }, node("mathBlock", { id: "m" }, text("x = 1"))))).toBe(
      lines("> $$", "> x = 1", "> $$"),
    );
  });

  it("renders closed toggles and their blocks", () => {
    const toggle = node(
      "details",
      { id: "d" },
      node("detailsSummary", { id: "s" }, text("More", "bold")),
      node("detailsContent", { id: "c" }, p(text("one")), p(text("two"))),
    );
    // The summary is plain text: marks are dropped.
    expect(md(toggle)).toBe(lines("<details>", "<summary>More</summary>", "", "one", "", "two", "", "</details>"));
  });

  it("renders a toggle heading: the summary's text inside <h1> to <h3> (ADR-0040)", () => {
    const toggle = (level: JsonValue | undefined, ...summary: JsonValue[]): JsonValue =>
      node(
        "details",
        { id: "d" },
        node("detailsSummary", level === undefined ? { id: "s" } : { id: "s", level }, ...summary),
        node("detailsContent", { id: "c" }, p(text("body"))),
      );
    for (const level of [1, 2, 3]) {
      expect(md(toggle(level, text("Title", "bold")))).toBe(
        lines("<details>", `<summary><h${String(level)}>Title</h${String(level)}></summary>`, "", "body", "", "</details>"),
      );
    }
    // An empty toggle heading keeps its level; an integral float is the integer.
    expect(md(toggle(1))).toBe(lines("<details>", "<summary><h1></h1></summary>", "", "body", "", "</details>"));
    expect(md(toggle(2.0, text("T")))).toBe(lines("<details>", "<summary><h2>T</h2></summary>", "", "body", "", "</details>"));
    // Without a level, or with one that is not 1 to 3, the toggle is a plain one.
    const plain = lines("<details>", "<summary>T</summary>", "", "body", "", "</details>");
    for (const damaged of [undefined, 0, 4, 1.5, "2", null, true]) {
      expect(md(toggle(damaged, text("T"))), String(damaged)).toBe(plain);
    }
  });

  it("renders a toggle without content, and stray content in it", () => {
    const toggle = node("details", { id: "d", open: true }, node("detailsSummary", { id: "s" }, text("S")));
    expect(md(toggle)).toBe(lines("<details open>", "<summary>S</summary>", "", "", "", "</details>"));
    const stray = node("details", { id: "d" }, text("loose"), node("detailsSummary", { id: "s" }, text("S")));
    expect(md(stray)).toBe(lines("<details>", "<summary>S</summary>", "", "loose", "", "</details>"));
  });

  it("renders a divider", () => {
    expect(md(p(text("a")), { type: "horizontalRule", attrs: { id: "r" } })).toBe(lines("a", "", "---"));
  });

  it("renders the children of unknown nodes, joined by blank lines, and keeps inline content together", () => {
    const columns = {
      type: "columns",
      attrs: { id: "c" },
      content: [
        { type: "column", content: [p(text("left"))] },
        { type: "column", content: [p(text("right"))] },
      ],
    };
    expect(md(columns)).toBe(lines("left", "", "right"));
    expect(md({ type: "aside", content: [text("x "), text("y", "bold"), { type: "hardBreak" }, text("z")] })).toBe(
      lines("x **y**  ", "z"),
    );
    expect(md(p(text("a "), { type: "mention", content: [text("@ben")] }, text(" b")))).toBe("a @ben b\n");
  });
});

describe("renderMarkdown inline content", () => {
  it("nests marks from link (outermost) to code (innermost) and ignores unknown marks", () => {
    const marks: JsonValue[] = [
      { type: "code" },
      { type: "spoiler", attrs: { color: "red" } },
      { type: "underline" },
      { type: "link", attrs: { href: "https://a.example" } },
      { type: "italic" },
      { type: "strike" },
      { type: "bold" },
    ];
    expect(md(p({ type: "text", text: "x", marks }))).toBe("[***~~<u>`x`</u>~~***](https://a.example)\n");
  });

  it("joins runs with the same Markdown marks before wrapping them", () => {
    expect(md(p(text("a", "bold"), text("b", "bold"), text("c")))).toBe("**ab**c\n");
    expect(md(p(text("a", "bold", "italic"), text("b", "italic", "bold")))).toBe("***ab***\n");
    // A mark Markdown cannot show does not split the run.
    const unknown: JsonValue = { type: "text", text: "b", marks: [{ type: "bold" }, { type: "spoiler" }] };
    expect(md(p(text("a", "bold"), unknown))).toBe("**ab**\n");
    // Links with different targets stay apart; a link without href renders as plain text.
    const link = (value: string, href: JsonValue): JsonValue => ({
      type: "text",
      text: value,
      marks: [{ type: "link", attrs: { href } }],
    });
    expect(md(p(link("a", "https://a"), link("b", "https://b")))).toBe("[a](https://a)[b](https://b)\n");
    expect(md(p({ type: "text", text: "x", marks: [{ type: "link" }] }))).toBe("x\n");
  });

  it("renders the colour marks as plain text and does not let them split a run (textColor, highlight)", () => {
    // Added 2026-10-07. The same page and the same Markdown as in kb-model's
    // `colour_marks_render_as_plain_text_and_do_not_split_a_run`.
    const colour = (type: string, color: string): JsonValue => ({ type, attrs: { color } });
    const run = (value: string, ...marks: JsonValue[]): JsonValue => ({ type: "text", text: value, marks });
    const link = (href: string): JsonValue => ({ type: "link", attrs: { href } });
    const heading: JsonValue = node(
      "heading",
      { id: "h", level: 2 },
      run("linked ", colour("textColor", "green"), link("https://l.example")),
      run("twice", colour("textColor", "red"), link("https://l.example")),
      run("other", link("https://m.example")),
    );
    const code = node("codeBlock", { id: "c" }, run("plain code", colour("textColor", "red")));
    expect(
      md(
        p(
          text("plain "),
          run("red", colour("textColor", "red")),
          text(" "),
          run("marked", colour("highlight", "yellow")),
          text(" "),
          run("both", colour("textColor", "blue"), colour("highlight", "pink")),
          text(" "),
          run("unknown", colour("textColor", "chartreuse")),
          text(" "),
          run("none", { type: "textColor" }),
        ),
        p(
          run("one ", { type: "bold" }),
          run("red", { type: "bold" }, colour("textColor", "red")),
          run(" two", { type: "bold" }, colour("highlight", "red")),
          text(" "),
          run("thr", { type: "italic" }),
          run("ee", { type: "italic" }, colour("highlight", "green")),
        ),
        heading,
        code,
      ),
    ).toBe(
      "plain red marked both unknown none\n\n**one red two** *three*\n\n" +
        "## [linked twice](https://l.example)[other](https://m.example)\n\n```\nplain code\n```\n",
    );
  });

  it("does not escape anything in v1", () => {
    expect(md(p(text("*not emphasis* # [x](y) <b>")))).toBe("*not emphasis* # [x](y) <b>\n");
  });

  it("renders page references through titleOf, ignoring their blockId, and the stored id otherwise", () => {
    const alpha = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const empty = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
    const unknown = "99999999-9999-4999-8999-999999999999";
    const titles: Record<string, string> = { [alpha]: "Alpha", [empty]: "" };
    const asked: string[] = [];
    const titleOf = (id: string): string | undefined => {
      asked.push(id);
      return titles[id];
    };
    const ref = (pageId: string, blockId?: string): JsonValue => ({
      type: "pageRef",
      attrs: blockId === undefined ? { id: "r", pageId } : { id: "r", pageId, blockId },
    });
    const page = doc(
      p(ref(alpha), text(" "), ref(alpha, unknown), text(" "), ref(unknown), text(" "), ref(empty), text(" ")),
      // Ids are read like kb-types reads them: hyphens ignored, case folded.
      p(ref("AAAAAAAAAAAA4AAA8AAAAAAAAAAAAAAA"), text(" "), ref("not-a-page")),
    );
    expect(renderMarkdown(page, titleOf)).toBe(
      lines(`[[Alpha]] [[Alpha]] [[${unknown}]] [[]] `, "", "[[Alpha]] [[not-a-page]]"),
    );
    expect(asked).not.toContain("not-a-page");
  });
});

describe("renderMarkdown input", () => {
  it("accepts raw ProseMirror JSON with null attributes", () => {
    expect(md({ type: "codeBlock", attrs: { id: "c", language: null }, content: [{ type: "text", text: "x" }] })).toBe(
      lines(FENCE, "x", FENCE),
    );
  });

  it("refuses what is not a node tree", () => {
    expect(() => renderMarkdown({ type: "doc", content: "nope" }, unresolved)).toThrow(CoreError);
  });
});

describe("renderMarkdown attachments (Milestone 5)", () => {
  const image = (attrs: { [key: string]: JsonValue }): JsonValue => ({ type: "image", attrs: { id: "i", ...attrs } });
  const file = (attrs: { [key: string]: JsonValue }): JsonValue => ({ type: "file", attrs: { id: "f", ...attrs } });

  it("renders an image as its alt text and an attachment link; width is not rendered", () => {
    expect(md(image({ attachmentId: FILE_ATTACHMENT, alt: "A chart", width: 300 }))).toBe(
      `![A chart](attachment://${FILE_ATTACHMENT})\n`,
    );
    expect(md(image({ attachmentId: FILE_ATTACHMENT }))).toBe(`![](attachment://${FILE_ATTACHMENT})\n`);
    expect(md(image({}))).toBe("![](attachment://)\n");
  });

  it("renders a gallery as one image line per item, the caption its link title (2026-10-09, ADR-0030)", () => {
    const gallery = (attrs: { [key: string]: JsonValue }): JsonValue => ({ type: "gallery", attrs: { id: "g", ...attrs } });
    const other = "a2020908-0706-4504-8302-010001020302";
    expect(
      md(
        gallery({
          columns: 2,
          items: [
            { attachmentId: FILE_ATTACHMENT, caption: 'She said "hi" at C:\\temp\\' },
            { attachmentId: other },
            { attachmentId: FILE_ATTACHMENT, caption: "one\ntwo" },
            { attachmentId: other, caption: "" },
            { caption: "no picture" },
            7,
          ],
        }),
      ),
    ).toBe(
      `![](attachment://${FILE_ATTACHMENT} "She said \\"hi\\" at C:\\\\temp\\\\")\n\n` +
        `![](attachment://${other})\n\n` +
        `![](attachment://${FILE_ATTACHMENT} "one two")\n\n` +
        `![](attachment://${other})\n`,
    );
    // No list, or nothing usable in it: the empty string, like the derived blocks.
    const unusable: { [key: string]: JsonValue }[] = [{}, { items: "x" }, { items: [] }, { items: [{ caption: "x" }] }];
    for (const attrs of unusable) {
      expect(md(gallery(attrs)), JSON.stringify(attrs)).toBe("\n");
    }
  });

  it("renders a non-empty caption as the link title (2026-10-09); width in percent and alignment are not rendered", () => {
    expect(md(image({ attachmentId: FILE_ATTACHMENT, alt: "A cat", caption: "Our cat, asleep" }))).toBe(
      `![A cat](attachment://${FILE_ATTACHMENT} "Our cat, asleep")\n`,
    );
    expect(md(image({ attachmentId: FILE_ATTACHMENT, caption: "No alt" }))).toBe(
      `![](attachment://${FILE_ATTACHMENT} "No alt")\n`,
    );
    // Quotes and backslashes are escaped; every kind of line break is one space.
    expect(md(image({ attachmentId: FILE_ATTACHMENT, alt: "x", caption: 'She said "hi" at C:\\temp\\' }))).toBe(
      `![x](attachment://${FILE_ATTACHMENT} "She said \\"hi\\" at C:\\\\temp\\\\")\n`,
    );
    expect(md(image({ attachmentId: FILE_ATTACHMENT, alt: "x", caption: "one\ntwo\r\nthree\rfour\n\nfive" }))).toBe(
      `![x](attachment://${FILE_ATTACHMENT} "one two three four  five")\n`,
    );
    // No caption (absent, empty, damaged): the line is as before.
    const plain = `![plain](attachment://${FILE_ATTACHMENT})\n`;
    const withoutCaption: { [key: string]: JsonValue }[] = [
      { alt: "plain" },
      { alt: "plain", caption: "" },
      { alt: "plain", caption: 3 },
      { alt: "plain", widthPercent: 40, align: "left", width: 200 },
    ];
    for (const attrs of withoutCaption) {
      expect(md(image({ attachmentId: FILE_ATTACHMENT, ...attrs })), JSON.stringify(attrs)).toBe(plain);
    }
  });

  it("renders a file as the resolved name, else the attachment id as stored", () => {
    const names: Record<string, string> = { [FILE_ATTACHMENT]: "Q3 report.pdf" };
    const resolveAttachmentName = (id: string): string | undefined => names[id];
    const page = doc(p(text("Before")), file({ attachmentId: FILE_ATTACHMENT.toUpperCase() }), p(text("After")));
    expect(renderMarkdown(page, unresolved, { resolveAttachmentName })).toBe(
      lines("Before", "", `[Q3 report.pdf](attachment://${FILE_ATTACHMENT.toUpperCase()})`, "", "After"),
    );
    expect(renderMarkdown(page, unresolved)).toBe(
      lines(
        "Before",
        "",
        `[${FILE_ATTACHMENT.toUpperCase()}](attachment://${FILE_ATTACHMENT.toUpperCase()})`,
        "",
        "After",
      ),
    );
    // A malformed id is never resolved.
    const asked: string[] = [];
    expect(
      renderMarkdown(doc(file({ attachmentId: "report" })), unresolved, {
        resolveAttachmentName: (id) => {
          asked.push(id);
          return "never";
        },
      }),
    ).toBe("[report](attachment://report)\n");
    expect(asked).toEqual([]);
  });

  it("asks for each distinct file attachment once, in document order, before rendering", () => {
    const second = "77777777-7777-4777-8777-777777777777";
    const asked: string[] = [];
    const page = doc(
      file({ attachmentId: second }),
      node("callout", { id: "c" }, file({ attachmentId: FILE_ATTACHMENT })),
      file({ attachmentId: second.toUpperCase() }),
      image({ attachmentId: IMAGE_ATTACHMENT }),
    );
    const out = renderMarkdown(page, unresolved, {
      resolveAttachmentName: (id) => {
        asked.push(id);
        return id === second ? "two.txt" : undefined;
      },
    });
    expect(asked).toEqual([second, FILE_ATTACHMENT]);
    expect(out).toBe(
      lines(
        `[two.txt](attachment://${second})`,
        "",
        `> 💡 [${FILE_ATTACHMENT}](attachment://${FILE_ATTACHMENT})`,
        "",
        `[two.txt](attachment://${second.toUpperCase()})`,
        "",
        `![](attachment://${IMAGE_ATTACHMENT})`,
      ),
    );
  });

  it("renders attachments inside lists and quotes like other blocks", () => {
    expect(md(node("bulletList", { id: "l" }, item(p(text("Photo")), image({ attachmentId: IMAGE_ATTACHMENT, alt: "x" }))))).toBe(
      lines("- Photo", `  ![x](attachment://${IMAGE_ATTACHMENT})`),
    );
    expect(md(node("blockquote", { id: "q" }, file({ attachmentId: "a" })))).toBe("> [a](attachment://a)\n");
  });
});

describe("renderMarkdown tables (additive nodes, 2026-10-05)", () => {
  // The same inputs and outputs as the tests of kb-model (`src/tests/page.rs`): both runtimes
  // must write the same bytes.
  const table = (...rows: JsonValue[]): JsonValue => node("table", { id: "t" }, ...rows);
  const row = (...cells: JsonValue[]): JsonValue => node("tableRow", { id: "r" }, ...cells);
  const cell = (type: string, ...blocks: JsonValue[]): JsonValue =>
    node(type, { id: "c", colspan: 1, rowspan: 1 }, ...blocks);
  const one = (value: string): JsonValue => cell("tableCell", p(text(value)));

  it("renders a table as GFM with one line per row", () => {
    const boldBreak = p(text("bold", "bold"), { type: "hardBreak" }, text("after"));
    const list = node("bulletList", { id: "l" }, item(p(text("x"))), item(p(text("y"))));
    const page = doc(
      p(text("before")),
      table(
        row(cell("tableHeader", p(text("Name"))), cell("tableHeader", p(text("Note")))),
        row(one("a | b"), cell("tableCell", p(text("one")), p(text("two")))),
        row(cell("tableCell", boldBreak), cell("tableCell", list)),
      ),
      p(text("after")),
    );
    expect(renderMarkdown(page, unresolved)).toBe(
      lines(
        "before",
        "",
        "| Name | Note |",
        "| --- | --- |",
        "| a \\| b | one<br><br>two |",
        "| **bold**  <br>after | - x<br>- y |",
        "",
        "after",
      ),
    );
  });

  it("is literal in the odd cases", () => {
    // No rows: nothing, like an empty paragraph.
    const empty = node("table", { id: "t" });
    expect(md(empty)).toBe("\n");
    expect(md(p(text("a")), empty, p(text("b")))).toBe("a\n\n\n\nb\n");

    // The separator has as many columns as the first row; other rows keep their own cells.
    expect(md(table(row(one("a"), one("b")), row(one("c"), one("d"), one("e")), row(one("f"))))).toBe(
      "| a | b |\n| --- | --- |\n| c | d | e |\n| f |\n",
    );

    // Empty cells and rows without cells.
    expect(md(table(row(one(""), one("")), row(), row(cell("tableCell"))))).toBe(
      "|  |  |\n| --- | --- |\n|  |\n|  |\n",
    );
    expect(md(table(row(), row(one("x"))))).toBe("|  |\n|  |\n| x |\n");

    // Spans and widths are ignored; cells and rows are whatever the children are called.
    const spanned = node("tableCell", { id: "s", colspan: 2, rowspan: 3, colwidth: [120, 80] }, p(text("wide")));
    const odd = table(
      row(spanned, { type: "gridCell", content: [p(text("odd"))] }),
      { type: "line", content: [{ type: "tableCell", content: [p(text("other"))] }] },
    );
    expect(md(odd)).toBe("| wide | odd |\n| --- | --- |\n| other |\n");

    // Nothing but `|` and newlines is touched: marks, links, backslashes and code stay.
    const link: JsonValue = {
      type: "text",
      text: "x|y",
      marks: [{ type: "link", attrs: { href: "https://a.example/?q=a|b" } }],
    };
    const code = node("codeBlock", { id: "k", language: "rs" }, text("a || b\nc"));
    expect(md(table(row(cell("tableCell", p(link, text(" back\\slash")), code))))).toBe(
      "| [x\\|y](https://a.example/?q=a\\|b) back\\slash<br><br>```rs<br>a \\|\\| b<br>c<br>``` |\n| --- |\n",
    );

    // A table in a cell is one more block of that cell; a table in a quote or an item takes
    // the container's prefix on each line.
    const inner = table(row(one("i")));
    expect(md(table(row(cell("tableCell", p(text("o")), inner))))).toBe(
      "| o<br><br>\\| i \\|<br>\\| --- \\| |\n| --- |\n",
    );
    expect(md(node("blockquote", { id: "q" }, inner))).toBe("> | i |\n> | --- |\n");
    expect(md(node("bulletList", { id: "l" }, item(p(text("item")), inner)))).toBe("- item\n  | i |\n  | --- |\n");
  });

  it("names files inside table cells, asking once per attachment", () => {
    const asked: string[] = [];
    const page = doc(
      table(row(cell("tableCell", { type: "file", attrs: { id: "f", attachmentId: FILE_ATTACHMENT } }))),
    );
    expect(
      renderMarkdown(page, unresolved, {
        resolveAttachmentName: (id) => {
          asked.push(id);
          return "report.pdf";
        },
      }),
    ).toBe(`| [report.pdf](attachment://${FILE_ATTACHMENT}) |\n| --- |\n`);
    expect(asked).toEqual([FILE_ATTACHMENT]);
  });
});

describe("renderMarkdown columns (additive nodes, 2026-10-06)", () => {
  // The same inputs and outputs as the tests of kb-model (`src/tests/page.rs`): both runtimes
  // must write the same bytes.
  const columnList = (...columns: JsonValue[]): JsonValue => node("columnList", { id: "cl" }, ...columns);
  const column = (...blocks: JsonValue[]): JsonValue => node("column", { id: "c" }, ...blocks);

  it("renders the columns one after another, each column's blocks joined by blank lines", () => {
    expect(
      md(p(text("before")), columnList(column(p(text("one")), p(text("two"))), column(p(text("three")))), p(text("after"))),
    ).toBe("before\n\none\n\ntwo\n\nthree\n\nafter\n");
  });

  it("renders a list and a quote in a column as it does anywhere else", () => {
    const list = node("bulletList", { id: "l" }, item(p(text("x"))), item(p(text("y"))));
    const quote = node("blockquote", { id: "q" }, p(text("q")));
    expect(md(columnList(column(list), column(quote, p(text("tail")))))).toBe("- x\n- y\n\n> q\n\ntail\n");
    // The layout takes the prefix of the container it is in, as any block does.
    expect(md(node("blockquote", { id: "bq" }, columnList(column(p(text("a"))), column(p(text("b"))))))).toBe(
      "> a\n>\n> b\n",
    );
  });

  it("does not look at the number of columns or at the nesting", () => {
    const inner = columnList(column(p(text("i1"))), column(p(text("i2"))));
    expect(md(columnList(column(inner), column(p(text("o")))))).toBe("i1\n\ni2\n\no\n");
  });

  it("gives an empty column its place in the join, as an empty top-level paragraph has", () => {
    const layout = (middle: JsonValue): JsonValue[] => [
      p(text("x")),
      columnList(column(p(text("a"))), middle, column(p(text("b")))),
      p(text("y")),
    ];
    const expected = "x\n\na\n\n\n\nb\n\ny\n";
    expect(md(...layout(column(p())))).toBe(expected); // one empty paragraph
    expect(md(...layout(column()))).toBe(expected); // no blocks at all
    // At the very end of the page the blank lines are trimmed.
    expect(md(columnList(column(p(text("a"))), column(p())))).toBe("a\n");
    // A column list without columns is nothing, like an empty paragraph.
    expect(md(p(text("a")), node("columnList", { id: "cl" }), p(text("b")))).toBe("a\n\n\n\nb\n");
  });

  it("names files inside columns, asking once per attachment", () => {
    const asked: string[] = [];
    const page = doc(
      columnList(column(p(text("see"))), column({ type: "file", attrs: { id: "f", attachmentId: FILE_ATTACHMENT } })),
    );
    expect(
      renderMarkdown(page, unresolved, {
        resolveAttachmentName: (id) => {
          asked.push(id);
          return "report.pdf";
        },
      }),
    ).toBe(`see\n\n[report.pdf](attachment://${FILE_ATTACHMENT})\n`);
    expect(asked).toEqual([FILE_ATTACHMENT]);
  });
});

describe("renderMarkdown page links (additive node, 2026-10-06)", () => {
  const known = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const unknown = "99999999-9999-4999-8999-999999999999";
  const block = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
  const link = (pageId: string, blockId?: string): JsonValue => ({
    type: "pageLink",
    attrs: blockId === undefined ? { id: "l", pageId } : { id: "l", pageId, blockId },
  });
  const titles = (id: string): string | undefined => (id === known ? "Known" : undefined);
  const render = (...blocks: JsonValue[]): string => renderMarkdown(doc(...blocks), titles);

  it("renders the card like a reference on a line of its own, its blockId ignored", () => {
    expect(render(p(text("before")), link(known), link(known, block), p(text("after")))).toBe(
      lines("before", "", "[[Known]]", "", "[[Known]]", "", "after"),
    );
  });

  it("renders the stored page id when the title is unknown", () => {
    expect(render(link(unknown))).toBe(`[[${unknown}]]\n`);
    expect(render(link("not-a-page"))).toBe("[[not-a-page]]\n");
    expect(render(node("pageLink", { id: "l" }))).toBe("[[]]\n");
  });

  it("is a block: a chip in text stays in its paragraph, and a card inside a callout or a column renders as one", () => {
    const chip: JsonValue = { type: "pageRef", attrs: { id: "r", pageId: known } };
    expect(
      render(
        p(text("see "), chip),
        node("callout", { id: "c" }, link(known)),
        node("columnList", { id: "cl" }, node("column", { id: "c1" }, link(unknown)), node("column", { id: "c2" }, p(text("x")))),
      ),
    ).toBe(lines("see [[Known]]", "", "> 💡 [[Known]]", "", `[[${unknown}]]`, "", "x"));
  });
});

describe("renderMarkdown derived blocks (additive nodes, 2026-10-06)", () => {
  // The same inputs and outputs as the tests of kb-model (`src/tests/page.rs`): both runtimes
  // must write the same bytes. A table of contents and a breadcrumb are leaves with an `id`
  // (nothing is stored of what they show), so they render as the empty string and keep their
  // place in the join, as an empty top-level paragraph does.
  const toc = (id = "toc"): JsonValue => ({ type: "tableOfContents", attrs: { id } });
  const crumbs = (id = "crumbs"): JsonValue => ({ type: "breadcrumb", attrs: { id } });

  it("leaves three blank lines between two blocks, as the empty chunk and its two joins", () => {
    expect(md(p(text("a")), toc(), p(text("b")))).toBe("a\n\n\n\nb\n");
    expect(md(p(text("a")), crumbs(), p(text("b")))).toBe("a\n\n\n\nb\n");
  });

  it("adds two newlines for each one: two in a row leave five blank lines", () => {
    expect(md(p(text("a")), toc(), crumbs(), p(text("b")))).toBe("a\n\n\n\n\n\nb\n");
  });

  it("leaves two empty lines before the first line when it starts the page", () => {
    expect(md(toc(), p(text("a")))).toBe("\n\na\n");
  });

  it("vanishes with the trailing newlines when it ends the page, and an empty page is one newline", () => {
    expect(md(p(text("a")), toc(), crumbs())).toBe("a\n");
    expect(md(toc())).toBe("\n");
    expect(md(toc(), crumbs())).toBe("\n");
  });

  it("is the same with an empty children list as without a children key", () => {
    expect(md(p(text("a")), node("tableOfContents", { id: "toc" }), p(text("b")))).toBe(md(p(text("a")), toc(), p(text("b"))));
  });

  it("is the empty string among the blocks of a container, as anywhere", () => {
    expect(md(node("blockquote", { id: "q" }, p(text("a")), toc("t2"), p(text("b"))))).toBe("> a\n>\n>\n>\n> b\n");
  });
});

describe("renderMarkdown list views (additive node, 2026-10-07)", () => {
  // The same inputs and outputs as the tests of kb-model (`src/tests/page.rs`). The pages a list view
  // shows are derived at run time and the document stores only the configuration of the view, so the
  // export writes nothing for it, by the generic rule of the other derived blocks.
  const list = (id = "list"): JsonValue => ({
    type: "pageList",
    attrs: { id, source: { kind: "children" }, layout: "list", properties: [], sort: { by: "title", dir: "asc" } },
  });
  const tagged = (id = "tagged"): JsonValue => ({
    type: "pageList",
    attrs: {
      id,
      source: { kind: "tag", optionId: "opt" },
      layout: "list",
      properties: ["prop-1", "prop-2"],
      sort: { by: "prop-2", dir: "desc" },
    },
  });

  it("is the empty string, leaving three blank lines between two blocks", () => {
    expect(md(p(text("a")), list(), p(text("b")))).toBe("a\n\n\n\nb\n");
    expect(md(p(text("a")), tagged(), p(text("b")))).toBe("a\n\n\n\nb\n");
  });

  it("adds two newlines like any derived block: next to another one it leaves five blank lines", () => {
    expect(md(p(text("a")), list(), node("breadcrumb", { id: "c" }), p(text("b")))).toBe("a\n\n\n\n\n\nb\n");
  });

  it("leaves two empty lines when it starts the page, vanishes when it ends it, and alone is an empty page", () => {
    expect(md(list(), p(text("a")))).toBe("\n\na\n");
    expect(md(p(text("a")), tagged())).toBe("a\n");
    expect(md(list())).toBe("\n");
  });

  it("is the empty string as a table with filters, widths and calculations too (2026-10-08, 2026-10-14), whatever those hold", () => {
    const table: JsonValue = {
      type: "pageList",
      attrs: {
        id: "table",
        source: { kind: "children" },
        layout: "table",
        properties: ["prop-1"],
        sort: { by: "prop-1", dir: "asc" },
        filters: [{ propertyId: "prop-1", op: "an-operator-of-a-newer-version", value: [1, "two"] }],
        columnWidths: { title: 320, "prop-1": 120.5 },
        calculations: { title: "count_all", "prop-1": "a-function-of-a-newer-version" },
      },
    };
    expect(md(p(text("a")), table, p(text("b")))).toBe("a\n\n\n\nb\n");
    expect(md(p(text("a")), table)).toBe("a\n");
  });

  it("writes nothing of its configuration, and is the empty string among the blocks of a container", () => {
    expect(md(p(text("a")), tagged(), p(text("b")))).not.toMatch(/tag|title|prop|opt/u);
    expect(md(node("blockquote", { id: "q" }, p(text("a")), list("l2"), p(text("b"))))).toBe("> a\n>\n>\n>\n> b\n");
  });
});

describe("renderMarkdown date mentions (additive inline node, 2026-10-07)", () => {
  const mention = (date: JsonValue, time?: JsonValue, id = "d"): JsonValue => ({
    type: "dateMention",
    attrs: time === undefined ? { id, date } : { id, date, time },
  });

  it("renders the ISO text, with the time after a space, among the text around it", () => {
    expect(
      md(p(text("Due "), mention("2026-10-07"), text(", review "), mention("2026-10-08", "14:30"), text("."))),
    ).toBe("Due 2026-10-07, review 2026-10-08 14:30.\n");
  });

  it("does not render the reminder: it is not text (ADR-0024)", () => {
    const reminded = (date: string, time: string | undefined, reminder: JsonValue): JsonValue => ({
      type: "dateMention",
      attrs: time === undefined ? { id: "d", date, reminder } : { id: "d", date, time, reminder },
    });
    expect(
      md(p(text("Due "), reminded("2026-10-07", undefined, 1440), text(", review "), reminded("2026-10-08", "14:30", 15))),
    ).toBe("Due 2026-10-07, review 2026-10-08 14:30\n");
    // A damaged reminder changes nothing either.
    expect(md(p(text("On "), reminded("2026-10-07", "14:30", -5), text(" or "), reminded("2026-10-08", undefined, "soon")))).toBe(
      "On 2026-10-07 14:30 or 2026-10-08\n",
    );
  });

  it("does not carry the marks of the text around it, which keep their runs", () => {
    expect(md(p(text("bold ", "bold"), mention("2026-10-07"), text(" bold", "bold")))).toBe(
      "**bold **2026-10-07** bold**\n",
    );
  });

  it("renders in a heading, a list item, a quote and a toggle's summary", () => {
    const summary = node("detailsSummary", { id: "s" }, text("Until "), mention("2026-10-11"));
    const toggle = node("details", { id: "t", open: false }, summary, node("detailsContent", { id: "c" }, p(text("body"))));
    expect(
      md(
        node("heading", { id: "h", level: 2 }, text("Plan for "), mention("2026-10-07")),
        node("bulletList", { id: "b" }, item(p(text("Call "), mention("2026-10-09", "08:00")))),
        node("blockquote", { id: "q" }, p(mention("2026-10-10"))),
        toggle,
      ),
    ).toBe(
      lines(
        "## Plan for 2026-10-07",
        "",
        "- Call 2026-10-09 08:00",
        "",
        "> 2026-10-10",
        "",
        "<details>",
        "<summary>Until 2026-10-11</summary>",
        "",
        "body",
        "",
        "</details>",
      ),
    );
  });

  it("joins the inline nodes around a mention that stands among blocks into one paragraph", () => {
    expect(md(p(text("x")), mention("2026-10-07"), p(text("y")))).toBe(lines("x", "", "2026-10-07", "", "y"));
    expect(md(mention("2026-10-07"), text("tail"))).toBe("2026-10-07tail\n");
  });

  it("renders nothing for a date that is not a real day, and leaves a malformed time out", () => {
    for (const date of ["", "2026-13-01", "2026-02-30", "2025-02-29", "1900-02-29", "0000-01-01", "2026-1-1", "tomorrow", 20261007, null]) {
      expect(md(p(text("a"), mention(date), text("b"))), String(date)).toBe("ab\n");
    }
    expect(md(p(mention(null)))).toBe("\n");
    for (const time of ["", "9:05", "24:00", "12:60", "12:05:00", "noon", 930, true]) {
      expect(md(p(mention("2026-10-07", time))), String(time)).toBe("2026-10-07\n");
    }
    expect(md(node("dateMention", { id: "d" }))).toBe("\n");
  });

  it("accepts leap days only in leap years", () => {
    expect(md(p(mention("2024-02-29"), text(" "), mention("2000-02-29")))).toBe("2024-02-29 2000-02-29\n");
    expect(md(p(mention("2023-02-29"), mention("1900-02-29")))).toBe("\n");
  });
});

describe("renderMarkdown inline equations (additive inline node, 2026-10-07)", () => {
  const equation = (latex: JsonValue, id = "m"): JsonValue => ({ type: "mathInline", attrs: { id, latex } });

  it("renders `$latex$` among the text around it, as stored", () => {
    expect(md(p(text("Energy "), equation("E=mc^2"), text(" is "), equation("\\frac{a}{b}"), text(".")))).toBe(
      "Energy $E=mc^2$ is $\\frac{a}{b}$.\n",
    );
  });

  it("writes the LaTeX as stored: nothing is escaped or trimmed, `$` and spaces included", () => {
    expect(md(p(equation("a$b"), text(" "), equation(" x "), text(" "), equation("a\\b")))).toBe(
      "$a$b$ $ x $ $a\\b$\n",
    );
  });

  it("renders nothing for an equation with no source (`$$` is the block's fence) or whose latex is no string", () => {
    expect(md(p(text("a "), equation(""), text(" b")))).toBe("a  b\n");
    for (const latex of [null, 5, true, ["x"]]) {
      expect(md(p(text("a"), equation(latex), text("b"))), JSON.stringify(latex)).toBe("ab\n");
    }
    expect(md(p(equation("")))).toBe("\n");
    expect(md(node("mathInline", { id: "m" }))).toBe("\n");
  });

  it("does not carry the marks of the text around it, which keep their runs", () => {
    expect(md(p(text("bold ", "bold"), equation("x"), text(" bold", "bold")))).toBe("**bold **$x$** bold**\n");
  });

  it("renders in a heading, a list item, a quote, a table cell (with the cell's own pipe escape) and a toggle's summary", () => {
    const summary = node("detailsSummary", { id: "s" }, text("About "), equation("e"));
    const toggle = node("details", { id: "t", open: false }, summary, node("detailsContent", { id: "c" }, p(text("body"))));
    const table = node("table", { id: "tb" }, node("tableRow", { id: "r" }, node("tableCell", { id: "tc" }, p(equation("|x|")))));
    expect(
      md(
        node("heading", { id: "h", level: 2 }, text("Proof of "), equation("a^2+b^2=c^2")),
        node("bulletList", { id: "b" }, item(p(text("Let "), equation("n>0")))),
        node("blockquote", { id: "q" }, p(equation("x"))),
        table,
        toggle,
      ),
    ).toBe(
      lines(
        "## Proof of $a^2+b^2=c^2$",
        "",
        "- Let $n>0$",
        "",
        "> $x$",
        "",
        "| $\\|x\\|$ |",
        "| --- |",
        "",
        "<details>",
        "<summary>About $e$</summary>",
        "",
        "body",
        "",
        "</details>",
      ),
    );
  });

  it("joins the inline nodes around an equation that stands among blocks into one paragraph", () => {
    expect(md(p(text("x")), equation("y"), p(text("z")))).toBe(lines("x", "", "$y$", "", "z"));
    expect(md(equation("y"), text("tail"))).toBe("$y$tail\n");
  });
});
