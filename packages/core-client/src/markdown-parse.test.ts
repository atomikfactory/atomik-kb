import { describe, expect, it } from "vitest";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import { parseMarkdown, portableNodesToJson } from "./markdown-parse";
import { dateMentionText, normalizePortable } from "./page-doc";
import { whiteboardTextsOfPortable } from "./whiteboard";
import { MARKDOWN_GOLDEN, fixtureStatus, readFixtureText, samplePage } from "./testing/fixtures";

const TARGET_PAGE = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
const IMAGE_ATTACHMENT = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
const FILE_ATTACHMENT = "6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6";
/** A caption with a quote, a backslash and a parenthesis: everything the link title must survive. */
const CAPTION = 'A "quoted" caption, C:\\temp\\ and (a bracket)';
const FENCE = "```";

type Json = { [key: string]: JsonValue };

/** The blocks `parseMarkdown` finds in `md`, as plain JSON. */
function parse(md: string): JsonValue[] {
  return portableNodesToJson(parseMarkdown(md));
}

/** Blocks written by hand, normalised the way the parser's output is (sorted attributes and marks). */
function blocks(...nodes: JsonValue[]): JsonValue[] {
  const doc = normalizePortable({ type: "doc", content: nodes }) as Json;
  return (doc.content as JsonValue[] | undefined) ?? [];
}

function node(type: string, attrs?: Json, ...content: JsonValue[]): Json {
  const result: Json = { type };
  if (attrs !== undefined) {
    result.attrs = attrs;
  }
  if (content.length > 0) {
    result.content = content;
  }
  return result;
}

function p(...content: JsonValue[]): Json {
  return node("paragraph", undefined, ...content);
}

/** A text node; a mark is its name, or `link:<href>`. */
function text(value: string, ...marks: string[]): Json {
  const result: Json = { type: "text", text: value };
  if (marks.length > 0) {
    result.marks = marks.map((mark): Json =>
      mark.startsWith("link:") ? { type: "link", attrs: { href: mark.slice(5) } } : { type: mark },
    );
  }
  return result;
}

const hardBreak: Json = { type: "hardBreak" };

function item(...content: JsonValue[]): Json {
  return node("listItem", undefined, ...content);
}

function task(checked: boolean, ...content: JsonValue[]): Json {
  return node("taskItem", { checked }, ...content);
}

function ul(...items: JsonValue[]): Json {
  return node("bulletList", undefined, ...items);
}

function lines(...rows: string[]): string {
  return `${rows.join("\n")}\n`;
}

const unresolved = (): undefined => undefined;

function render(nodes: JsonValue[]): string {
  return renderMarkdown({ type: "doc", content: nodes }, unresolved);
}

/** Markdown of `md` parsed and rendered again. */
function back(md: string): string {
  return render(parse(md));
}

describe("parseMarkdown basics", () => {
  it("gives no nodes for empty or blank text", () => {
    expect(parseMarkdown("")).toEqual([]);
    expect(parseMarkdown("\n")).toEqual([]);
    expect(parseMarkdown("  \n\t\n \n")).toEqual([]);
  });

  it("separates blocks by blank lines, however many, and ignores trailing newlines", () => {
    expect(parse("one\n\n\n\ntwo\n\n\n")).toEqual(blocks(p(text("one")), p(text("two"))));
    expect(parse("one")).toEqual(blocks(p(text("one"))));
  });

  it("accepts CRLF, CR and a BOM", () => {
    expect(parse("\uFEFFa\r\n\r\nb\rc")).toEqual(blocks(p(text("a")), p(text("b c"))));
  });

  it("joins the lines of a paragraph with a space, trimming the indentation", () => {
    expect(parse("one\n   two\nthree  ")).toEqual(blocks(p(text("one two three"))));
  });

  it("makes a hard break of two spaces (or more) at the end of a line", () => {
    expect(parse("a  \nb")).toEqual(blocks(p(text("a"), hardBreak, text("b"))));
    expect(parse("a     \nb")).toEqual(blocks(p(text("a"), hardBreak, text("b"))));
    expect(parse("a \nb")).toEqual(blocks(p(text("a b"))));
    // At the end of the paragraph there is nothing to break.
    expect(parse("a  ")).toEqual(blocks(p(text("a"))));
  });

  it("returns nodes without ids, in the normalised portable shape", () => {
    const nodes = parseMarkdown("# T\n\n- a");
    expect(nodes.map((n) => n.type)).toEqual(["heading", "bulletList"]);
    const json = JSON.stringify(portableNodesToJson(nodes));
    expect(json).not.toContain('"id"');
    expect(nodes[0]).toEqual({
      type: "heading",
      attrs: { level: 1 },
      content: [{ type: "text", attrs: undefined, content: [], marks: [], text: "T" }],
      marks: [],
      text: undefined,
    });
  });

  it("is deterministic", () => {
    const source = readFixtureText(MARKDOWN_GOLDEN);
    expect(parse(source)).toEqual(parse(source));
  });
});

describe("headings", () => {
  it("parses levels 1 to 3 and caps the deeper ones at 3", () => {
    expect(parse(lines("# One", "## Two", "### Three", "#### Four", "###### Six"))).toEqual(
      blocks(
        node("heading", { level: 1 }, text("One")),
        node("heading", { level: 2 }, text("Two")),
        node("heading", { level: 3 }, text("Three")),
        node("heading", { level: 3 }, text("Four")),
        node("heading", { level: 3 }, text("Six")),
      ),
    );
  });

  it("needs a space after the hashes, and at most six of them", () => {
    expect(parse("#hashtag")).toEqual(blocks(p(text("#hashtag"))));
    expect(parse("####### seven")).toEqual(blocks(p(text("####### seven"))));
  });

  it("parses a lone # as an empty heading", () => {
    expect(parse("#")).toEqual(blocks(node("heading", { level: 1 })));
    expect(parse("## ")).toEqual(blocks(node("heading", { level: 2 })));
  });

  it("drops a closing run of hashes and keeps inline marks", () => {
    expect(parse("## Title ##")).toEqual(blocks(node("heading", { level: 2 }, text("Title"))));
    expect(parse("# C# and #1")).toEqual(blocks(node("heading", { level: 1 }, text("C# and #1"))));
    expect(parse("# Welcome to **atomik-kb**")).toEqual(
      blocks(node("heading", { level: 1 }, text("Welcome to "), text("atomik-kb", "bold"))),
    );
  });

  it("interrupts a paragraph", () => {
    expect(parse("text\n# Title\nmore")).toEqual(
      blocks(p(text("text")), node("heading", { level: 1 }, text("Title")), p(text("more"))),
    );
  });
});

describe("inline marks", () => {
  const one = (md: string): JsonValue[] => parse(md);

  it("parses bold, italic, strike, underline and code", () => {
    expect(one("**b**")).toEqual(blocks(p(text("b", "bold"))));
    expect(one("*i*")).toEqual(blocks(p(text("i", "italic"))));
    expect(one("_i_")).toEqual(blocks(p(text("i", "italic"))));
    expect(one("__b__")).toEqual(blocks(p(text("b", "bold"))));
    expect(one("~~s~~")).toEqual(blocks(p(text("s", "strike"))));
    expect(one("<u>u</u>")).toEqual(blocks(p(text("u", "underline"))));
    expect(one("`c`")).toEqual(blocks(p(text("c", "code"))));
  });

  it("parses a run of text between plain words", () => {
    expect(one("a **b** c *d* e")).toEqual(
      blocks(p(text("a "), text("b", "bold"), text(" c "), text("d", "italic"), text(" e"))),
    );
  });

  it("parses links, a title after the href is dropped", () => {
    expect(one("see [the docs](https://example.com/a_(b)) now")).toEqual(
      blocks(p(text("see "), text("the docs", "link:https://example.com/a_(b)"), text(" now"))),
    );
    expect(one('[t](https://x.org "A title")')).toEqual(blocks(p(text("t", "link:https://x.org"))));
    expect(one("[t](<https://x.org/a b>)")).toEqual(blocks(p(text("t", "link:https://x.org/a b"))));
  });

  it("keeps marks inside a link and links inside marks", () => {
    expect(one("[**bold** and *it*](u)")).toEqual(
      blocks(p(text("bold", "bold", "link:u"), text(" and ", "link:u"), text("it", "italic", "link:u"))),
    );
    expect(one("**[a](u)**")).toEqual(blocks(p(text("a", "bold", "link:u"))));
  });

  it("combines marks on one run in any nesting", () => {
    expect(one("***both***")).toEqual(blocks(p(text("both", "bold", "italic"))));
    expect(one("**bold *and italic* bold**")).toEqual(
      blocks(p(text("bold ", "bold"), text("and italic", "bold", "italic"), text(" bold", "bold"))),
    );
    expect(one("~~**x**~~")).toEqual(blocks(p(text("x", "bold", "strike"))));
    expect(one("*<u>x</u>*")).toEqual(blocks(p(text("x", "italic", "underline"))));
    expect(one("**`c`**")).toEqual(blocks(p(text("c", "bold", "code"))));
  });

  it("reads adjacent runs with different marks", () => {
    expect(one("**a***b*")).toEqual(blocks(p(text("a", "bold"), text("b", "italic"))));
    expect(one("**a**<u>b</u>")).toEqual(blocks(p(text("a", "bold"), text("b", "underline"))));
  });

  it("takes code literally, with backtick runs of any length", () => {
    expect(one("`**not bold**`")).toEqual(blocks(p(text("**not bold**", "code"))));
    expect(one("`` a`b ``")).toEqual(blocks(p(text(" a`b ", "code"))));
    expect(one("`[x](y)` and `a")).toEqual(blocks(p(text("[x](y)", "code"), text(" and `a"))));
    // A bracket inside code does not close the link text before it.
    expect(one("[a `]` b](u)")).toEqual(blocks(p(text("a ", "link:u"), text("]", "code", "link:u"), text(" b", "link:u"))));
  });

  it("leaves text that only looks like marks", () => {
    expect(one("snake_case_name and 2 * 3 * 4")).toEqual(blocks(p(text("snake_case_name and 2 * 3 * 4"))));
    expect(one("a * b and *c")).toEqual(blocks(p(text("a * b and *c"))));
    expect(one("**unclosed and ~~also")).toEqual(blocks(p(text("**unclosed and ~~also"))));
    expect(one("~one~ and ~~~three~~~")).toEqual(blocks(p(text("~one~ and ~~~three~~~"))));
    expect(one("x <u>never closed")).toEqual(blocks(p(text("x <u>never closed"))));
    expect(one("[not a link] and [also](")).toEqual(blocks(p(text("[not a link] and [also]("))));
    expect(one("[empty]()")).toEqual(blocks(p(text("[empty]()"))));
    expect(one("[a b](two words)")).toEqual(blocks(p(text("[a b](two words)"))));
  });

  it("keeps [[Title]] page references as text", () => {
    expect(one("See [[Linked page]] and [[Other]].")).toEqual(blocks(p(text("See [[Linked page]] and [[Other]]."))));
  });

  it("keeps images that are not attachments, and unknown HTML, as text", () => {
    expect(one("![alt](https://x.org/a.png)")).toEqual(blocks(p(text("![alt](https://x.org/a.png)"))));
    expect(one("a <span>b</span> <br> c")).toEqual(blocks(p(text("a <span>b</span> <br> c"))));
    expect(one("<div>x</div>")).toEqual(blocks(p(text("<div>x</div>"))));
  });

  it("does not parse colours: the export writes none, so spans and marks stay text", () => {
    // `textColor` and `highlight` (2026-10-07) render as plain text; HTML that looks like them is text too.
    const html = '<span data-text-color="red">x</span> and <mark data-highlight="yellow">y</mark> and <mark>z</mark>';
    expect(one(html)).toEqual(blocks(p(text(html))));
    const styled = 'a <span style="color: red">b</span>';
    expect(one(styled)).toEqual(blocks(p(text(styled))));
    expect(one("**bold** and plain")).toEqual(blocks(p(text("bold", "bold"), text(" and plain"))));
  });

  it("reads backslashes literally (the export writes no escapes)", () => {
    expect(one("a \\* b \\_ c")).toEqual(blocks(p(text("a \\* b \\_ c"))));
    expect(one("\\*x\\*")).toEqual(blocks(p(text("\\*x\\*"))));
  });

  it("has no trouble with Unicode next to marks", () => {
    expect(one("**日本語**と*café*、👩‍💻**x**")).toEqual(
      blocks(p(text("日本語", "bold"), text("と"), text("café", "italic"), text("、👩‍💻"), text("x", "bold"))),
    );
  });
});

describe("lists", () => {
  it("parses bullet lists with any of the three markers", () => {
    for (const marker of ["-", "*", "+"]) {
      expect(parse(lines(`${marker} a`, `${marker} b`))).toEqual(blocks(ul(item(p(text("a"))), item(p(text("b"))))));
    }
  });

  it("parses ordered lists with their start, `.` or `)`", () => {
    const list = (start: number | undefined, ...items: string[]): Json =>
      node("orderedList", start === undefined ? undefined : { start }, ...items.map((s) => item(p(text(s)))));
    expect(parse(lines("1. a", "2. b"))).toEqual(blocks(list(undefined, "a", "b")));
    expect(parse(lines("3. a", "4. b"))).toEqual(blocks(list(3, "a", "b")));
    expect(parse(lines("0. a"))).toEqual(blocks(list(0, "a")));
    expect(parse(lines("7) a", "8) b"))).toEqual(blocks(list(7, "a", "b")));
    // Numbers after the first are ignored.
    expect(parse(lines("1. a", "1. b", "9. c"))).toEqual(blocks(list(undefined, "a", "b", "c")));
  });

  it("parses task lists", () => {
    expect(parse(lines("- [x] Done", "- [ ] To do", "- [X] Also done"))).toEqual(
      blocks(node("taskList", undefined, task(true, p(text("Done"))), task(false, p(text("To do"))), task(true, p(text("Also done"))))),
    );
    // An empty task, and a checkbox with no space after it is text.
    expect(parse("- [ ]")).toEqual(blocks(node("taskList", undefined, task(false, p()))));
    expect(parse("- [x]no")).toEqual(blocks(ul(item(p(text("[x]no"))))));
    expect(parse("1. [ ] x")).toEqual(blocks(node("orderedList", undefined, item(p(text("[ ] x"))))));
  });

  it("makes lists of their own of the runs of tasks and of other items", () => {
    expect(parse(lines("- a", "- [ ] b", "- [x] c", "- d"))).toEqual(
      blocks(
        ul(item(p(text("a")))),
        node("taskList", undefined, task(false, p(text("b"))), task(true, p(text("c")))),
        ul(item(p(text("d")))),
      ),
    );
  });

  it("starts a new list when the marker changes", () => {
    expect(parse(lines("- a", "* b", "1. c", "2) d"))).toEqual(
      blocks(
        ul(item(p(text("a")))),
        ul(item(p(text("b")))),
        node("orderedList", undefined, item(p(text("c")))),
        node("orderedList", { start: 2 }, item(p(text("d")))),
      ),
    );
  });

  it("nests by indentation of 2 spaces, 4 spaces or a tab", () => {
    const nested = blocks(ul(item(p(text("a")), ul(item(p(text("b")), ul(item(p(text("c"))))))), item(p(text("d")))));
    expect(parse(lines("- a", "  - b", "    - c", "- d"))).toEqual(nested);
    expect(parse(lines("- a", "    - b", "        - c", "- d"))).toEqual(nested);
    expect(parse(lines("- a", "\t- b", "\t\t- c", "- d"))).toEqual(nested);
  });

  it("nests under ordered items, however far the child is indented", () => {
    const expected = blocks(node("orderedList", undefined, item(p(text("a")), ul(item(p(text("b")))))));
    expect(parse(lines("1. a", "   - b"))).toEqual(expected);
    expect(parse(lines("1. a", "  - b"))).toEqual(expected);
    expect(parse(lines("1. a", "    - b"))).toEqual(expected);
  });

  it("puts any block in an item: paragraphs, quotes, code", () => {
    expect(parse(lines("- a", "", "  second paragraph", "", "  > quoted", "- b"))).toEqual(
      blocks(
        ul(
          item(p(text("a")), p(text("second paragraph")), node("blockquote", undefined, p(text("quoted")))),
          item(p(text("b"))),
        ),
      ),
    );
    expect(parse(lines("- code:", "  ```js", "  let x;", "  ```"))).toEqual(
      blocks(ul(item(p(text("code:")), node("codeBlock", { language: "js" }, text("let x;"))))),
    );
  });

  it("starts an item with a paragraph, an empty one when the first block is something else", () => {
    expect(parse("- ## Title")).toEqual(blocks(ul(item(p(), node("heading", { level: 2 }, text("Title"))))));
    expect(parse("- > q")).toEqual(blocks(ul(item(p(), node("blockquote", undefined, p(text("q")))))));
    expect(parse("-")).toEqual(blocks(ul(item(p()))));
  });

  it("keeps a list going over blank lines, one list for loose items", () => {
    expect(parse(lines("1. a", "", "2. b", "", "", "3. c"))).toEqual(
      blocks(node("orderedList", undefined, item(p(text("a"))), item(p(text("b"))), item(p(text("c"))))),
    );
    expect(parse(lines("- a", "", "- b", "", "paragraph"))).toEqual(
      blocks(ul(item(p(text("a"))), item(p(text("b")))), p(text("paragraph"))),
    );
  });

  it("continues an item's paragraph on an indented line, hard breaks included", () => {
    expect(parse(lines("- a long item", "  continued here", "- b  ", "  with a break"))).toEqual(
      blocks(ul(item(p(text("a long item continued here"))), item(p(text("b"), hardBreak, text("with a break"))))),
    );
  });

  it("ends the list at a line that is not indented, instead of folding it into the item", () => {
    expect(parse(lines("Intro:", "- a", "- b", "Thanks!"))).toEqual(
      blocks(p(text("Intro:")), ul(item(p(text("a"))), item(p(text("b")))), p(text("Thanks!"))),
    );
  });

  it("is interrupted by blocks that are not part of the item", () => {
    expect(parse(lines("- a", "# H", "- b"))).toEqual(
      blocks(ul(item(p(text("a")))), node("heading", { level: 1 }, text("H")), ul(item(p(text("b"))))),
    );
    expect(parse(lines("- a", "---", "text"))).toEqual(blocks(ul(item(p(text("a")))), node("horizontalRule"), p(text("text"))));
  });

  it("reads markers only with a space after them", () => {
    expect(parse("-text")).toEqual(blocks(p(text("-text"))));
    expect(parse("1.5 is a number")).toEqual(blocks(p(text("1.5 is a number"))));
    expect(parse("+1 for this")).toEqual(blocks(p(text("+1 for this"))));
    expect(parse("**bold** start")).toEqual(blocks(p(text("bold", "bold"), text(" start"))));
  });

  it("lets a bullet interrupt a paragraph, an ordered list only from 1 (or inside an item)", () => {
    expect(parse(lines("text", "- a"))).toEqual(blocks(p(text("text")), ul(item(p(text("a"))))));
    expect(parse(lines("text", "1. a"))).toEqual(blocks(p(text("text")), node("orderedList", undefined, item(p(text("a"))))));
    expect(parse(lines("in 1999.", "2000. next"))).toEqual(blocks(p(text("in 1999. 2000. next"))));
    expect(parse(lines("- a", "  2. b"))).toEqual(
      blocks(ul(item(p(text("a")), node("orderedList", { start: 2 }, item(p(text("b"))))))),
    );
  });
});

describe("quotes and callouts", () => {
  it("parses a blockquote, with blank quote lines between its blocks", () => {
    expect(parse(lines("> a", ">", "> - b", "> c"))).toEqual(
      blocks(node("blockquote", undefined, p(text("a")), ul(item(p(text("b")))), p(text("c")))),
    );
    expect(parse("> one\n> two")).toEqual(blocks(node("blockquote", undefined, p(text("one two")))));
    expect(parse(">no space")).toEqual(blocks(node("blockquote", undefined, p(text("no space")))));
  });

  it("nests quotes and ends them at the first line without a marker", () => {
    expect(parse(lines("> a", "> > b", "after"))).toEqual(
      blocks(node("blockquote", undefined, p(text("a")), node("blockquote", undefined, p(text("b")))), p(text("after"))),
    );
  });

  it("makes a callout of a quote that starts with an icon and a space", () => {
    expect(parse("> 💡 Callouts carry an icon.")).toEqual(
      blocks(node("callout", { icon: "💡" }, p(text("Callouts carry an icon.")))),
    );
    for (const icon of ["⚠️", "👩‍💻", "👍🏽", "🇫🇷", "1️⃣", "★", "ℹ️"]) {
      expect(parse(`> ${icon} text`), icon).toEqual(blocks(node("callout", { icon }, p(text("text")))));
    }
  });

  it("keeps the other blocks of a callout, and an icon alone is an empty callout", () => {
    expect(parse(lines("> ⚠️ a", ">", "> b", "> - c"))).toEqual(
      blocks(node("callout", { icon: "⚠️" }, p(text("a")), p(text("b")), ul(item(p(text("c")))))),
    );
    expect(parse("> 💡")).toEqual(blocks(node("callout", { icon: "💡" }, p())));
    expect(parse("> 💡 ")).toEqual(blocks(node("callout", { icon: "💡" }, p())));
  });

  it("leaves quotes whose first word is not an icon", () => {
    for (const first of ["Note: x", "- item", "1. item", "**bold** text", "> nested", "# Title", "日本語 text", "é text", '"quoted"', "• dot", "[link](x)"]) {
      expect(parse(`> ${first}`)[0], first).toMatchObject({ type: "blockquote" });
    }
    // A long run of symbols is not an icon either.
    expect(parse(`> ${"😀".repeat(30)} x`)[0]).toMatchObject({ type: "blockquote" });
  });
});

describe("code and math", () => {
  it("parses a fenced code block with its language", () => {
    expect(parse(lines(`${FENCE}rust`, "fn main() {", '    println!("hi");', "}", FENCE))).toEqual(
      blocks(node("codeBlock", { language: "rust" }, text('fn main() {\n    println!("hi");\n}'))),
    );
    expect(parse(lines(FENCE, "plain", FENCE))).toEqual(blocks(node("codeBlock", undefined, text("plain"))));
    expect(parse(lines(`${FENCE} ts title="a.ts"`, "x", FENCE))).toEqual(blocks(node("codeBlock", { language: "ts" }, text("x"))));
  });

  it("parses tilde fences and fences with a longer closing line", () => {
    expect(parse(lines("~~~py", "x = 1", "~~~"))).toEqual(blocks(node("codeBlock", { language: "py" }, text("x = 1"))));
    expect(parse(lines("````md", FENCE, "inner", FENCE, "````"))).toEqual(
      blocks(node("codeBlock", { language: "md" }, text(`${FENCE}\ninner\n${FENCE}`))),
    );
    // A different fence character does not close it.
    expect(parse(lines(FENCE, "a", "~~~", FENCE))).toEqual(blocks(node("codeBlock", undefined, text("a\n~~~"))));
  });

  it("keeps the content as it is: markers, blank lines, indentation", () => {
    expect(parse(lines(FENCE, "# not a heading", "", "  - not a list", "**", FENCE))).toEqual(
      blocks(node("codeBlock", undefined, text("# not a heading\n\n  - not a list\n**"))),
    );
  });

  it("parses empty and unterminated fences", () => {
    expect(parse(lines(FENCE, FENCE))).toEqual(blocks(node("codeBlock")));
    expect(parse(lines(FENCE, "", FENCE))).toEqual(blocks(node("codeBlock")));
    expect(parse(lines(`${FENCE}js`, "let x;", "", "# still code"))).toEqual(
      blocks(node("codeBlock", { language: "js" }, text("let x;\n\n# still code"))),
    );
    expect(parse(`${FENCE}js\n\n\n`)).toEqual(blocks(node("codeBlock", { language: "js" })));
    expect(parse(FENCE)).toEqual(blocks(node("codeBlock")));
  });

  it("does not take inline triple backticks for a fence", () => {
    expect(parse(`${FENCE}code${FENCE} and more`)).toEqual(blocks(p(text("code", "code"), text(" and more"))));
  });

  it("interrupts a paragraph", () => {
    expect(parse(lines("text", FENCE, "x", FENCE, "after"))).toEqual(
      blocks(p(text("text")), node("codeBlock", undefined, text("x")), p(text("after"))),
    );
  });

  it("parses math blocks", () => {
    expect(parse(lines("$$", "E = mc^2", "$$"))).toEqual(blocks(node("mathBlock", undefined, text("E = mc^2"))));
    expect(parse(lines("$$", "\\begin{aligned}", "a &= $b$ \\\\", "\\end{aligned}", "$$"))).toEqual(
      blocks(node("mathBlock", undefined, text("\\begin{aligned}\na &= $b$ \\\\\n\\end{aligned}"))),
    );
    expect(parse(lines("$$", "$$"))).toEqual(blocks(node("mathBlock")));
    expect(parse(lines("$$", "", "$$"))).toEqual(blocks(node("mathBlock")));
    expect(parse("$$ x^2 $$")).toEqual(blocks(node("mathBlock", undefined, text("x^2"))));
  });

  it("leaves an unterminated $$ and prices as text", () => {
    expect(parse(lines("$$", "a", "b"))).toEqual(blocks(p(text("$$ a b"))));
    expect(parse("$$a$$ and $$b$$")).toEqual(blocks(p(text("$$a$$ and $$b$$"))));
    expect(parse("costs $5 or $10")).toEqual(blocks(p(text("costs $5 or $10"))));
  });
});

describe("dividers", () => {
  it("parses ---, ***, ___ and their spaced forms", () => {
    for (const rule of ["---", "***", "___", "- - -", "* * *", "_ _ _", "-----", "  ---  "]) {
      expect(parse(`a\n\n${rule}\n\nb`), rule).toEqual(blocks(p(text("a")), node("horizontalRule"), p(text("b"))));
    }
  });

  it("does not take two characters, or bold text, for a divider", () => {
    expect(parse("--")).toEqual(blocks(p(text("--"))));
    expect(parse("**bold**")).toEqual(blocks(p(text("bold", "bold"))));
  });
});

describe("toggles", () => {
  it("parses details the way the export writes them", () => {
    expect(parse(lines("<details open>", "<summary>Toggle summary</summary>", "", "Hidden until opened.", "", "</details>"))).toEqual(
      blocks(
        node(
          "details",
          { open: true },
          node("detailsSummary", undefined, text("Toggle summary")),
          node("detailsContent", undefined, p(text("Hidden until opened."))),
        ),
      ),
    );
    expect(parse(lines("<details>", "<summary>S</summary>", "", "a", "", "- b", "", "</details>"))).toEqual(
      blocks(
        node(
          "details",
          undefined,
          node("detailsSummary", undefined, text("S")),
          node("detailsContent", undefined, p(text("a")), ul(item(p(text("b"))))),
        ),
      ),
    );
  });

  it("reads a toggle heading back from the heading inside the summary (ADR-0040)", () => {
    for (const level of [1, 2, 3]) {
      const tag = `h${String(level)}`;
      const source = lines("<details open>", `<summary><${tag}>Title</${tag}></summary>`, "", "body", "", "</details>");
      expect(parse(source), tag).toEqual(
        blocks(
          node(
            "details",
            { open: true },
            node("detailsSummary", { level }, text("Title")),
            node("detailsContent", undefined, p(text("body"))),
          ),
        ),
      );
    }
    // An empty one keeps its level; h4 to h6 clamp to 3 as headings do; the tag is read in any case
    // and may carry attributes (an HTML export from another tool).
    expect(parse(lines("<details>", "<summary><h1></h1></summary>", "", "", "", "</details>"))).toEqual(
      blocks(node("details", undefined, node("detailsSummary", { level: 1 }), node("detailsContent", undefined, p()))),
    );
    expect(parse(lines("<details>", '<summary><H5 id="x">Deep</H5></summary>', "", "a", "", "</details>"))).toEqual(
      blocks(
        node(
          "details",
          undefined,
          node("detailsSummary", { level: 3 }, text("Deep")),
          node("detailsContent", undefined, p(text("a"))),
        ),
      ),
    );
    // A heading that does not wrap the whole summary is text, like any other markup in a summary.
    expect(parse(lines("<details>", "<summary>a <h2>b</h2></summary>", "", "x", "", "</details>"))).toEqual(
      blocks(
        node(
          "details",
          undefined,
          node("detailsSummary", undefined, text("a <h2>b</h2>")),
          node("detailsContent", undefined, p(text("x"))),
        ),
      ),
    );
  });

  it("parses empty and summary-less toggles, nested toggles, and leaves unclosed ones as text", () => {
    expect(parse(lines("<details>", "<summary></summary>", "", "", "", "</details>"))).toEqual(
      blocks(node("details", undefined, node("detailsSummary"), node("detailsContent", undefined, p()))),
    );
    expect(parse(lines("<details>", "just text", "</details>"))).toEqual(
      blocks(node("details", undefined, node("detailsSummary"), node("detailsContent", undefined, p(text("just text"))))),
    );
    expect(parse(lines("<details>", "<summary>Out</summary>", "<details>", "<summary>In</summary>", "x", "</details>", "</details>"))).toEqual(
      blocks(
        node(
          "details",
          undefined,
          node("detailsSummary", undefined, text("Out")),
          node(
            "detailsContent",
            undefined,
            node("details", undefined, node("detailsSummary", undefined, text("In")), node("detailsContent", undefined, p(text("x")))),
          ),
        ),
      ),
    );
    expect(parse("<details>\n<summary>S</summary>\ntext")).toEqual(blocks(p(text("<details> <summary>S</summary> text"))));
  });
});

describe("tables", () => {
  const cell = (type: string, ...content: JsonValue[]): Json => node(type, undefined, ...content);
  const row = (...cells: JsonValue[]): Json => node("tableRow", undefined, ...cells);

  it("parses a GFM table: header cells in the first row, cells after", () => {
    expect(parse(lines("| a | b |", "| --- | --- |", "| 1 | **2** |"))).toEqual(
      blocks(
        node(
          "table",
          undefined,
          row(cell("tableHeader", p(text("a"))), cell("tableHeader", p(text("b")))),
          row(cell("tableCell", p(text("1"))), cell("tableCell", p(text("2", "bold")))),
        ),
      ),
    );
  });

  it("accepts tables without outer pipes, alignment colons and short delimiters", () => {
    const expected = blocks(
      node(
        "table",
        undefined,
        row(cell("tableHeader", p(text("a"))), cell("tableHeader", p(text("b")))),
        row(cell("tableCell", p(text("1"))), cell("tableCell", p(text("2")))),
      ),
    );
    expect(parse(lines("a | b", ":--- | ---:", "1 | 2"))).toEqual(expected);
    expect(parse(lines("|a|b|", "|-|:-:|", "|1|2|"))).toEqual(expected);
  });

  it("unescapes \\| and turns <br> into block and line breaks inside a cell", () => {
    expect(parse(lines("| h |", "| --- |", "| a \\| b |", "| x<br><br>y |", "| p<br>q |", "| u  <br>v |"))).toEqual(
      blocks(
        node(
          "table",
          undefined,
          row(cell("tableHeader", p(text("h")))),
          row(cell("tableCell", p(text("a | b")))),
          row(cell("tableCell", p(text("x")), p(text("y")))),
          row(cell("tableCell", p(text("p"), hardBreak, text("q")))),
          row(cell("tableCell", p(text("u"), hardBreak, text("v")))),
        ),
      ),
    );
  });

  it("puts other blocks in cells: lists, code, images", () => {
    const md = lines("| h |", "| --- |", `| - a<br>- b |`, `| ${FENCE}js<br>x<br>${FENCE} |`, `| ![alt](attachment://${IMAGE_ATTACHMENT}) |`);
    expect(parse(md)).toEqual(
      blocks(
        node(
          "table",
          undefined,
          row(cell("tableHeader", p(text("h")))),
          row(cell("tableCell", ul(item(p(text("a"))), item(p(text("b")))))),
          row(cell("tableCell", node("codeBlock", { language: "js" }, text("x")))),
          row(cell("tableCell", node("image", { alt: "alt", attachmentId: IMAGE_ATTACHMENT }))),
        ),
      ),
    );
  });

  it("pads short rows, drops cells beyond the header and gives empty cells an empty paragraph", () => {
    expect(parse(lines("| a | b |", "| - | - |", "| 1 |", "| 1 | 2 | 3 |", "| | |"))).toEqual(
      blocks(
        node(
          "table",
          undefined,
          row(cell("tableHeader", p(text("a"))), cell("tableHeader", p(text("b")))),
          row(cell("tableCell", p(text("1"))), cell("tableCell", p())),
          row(cell("tableCell", p(text("1"))), cell("tableCell", p(text("2")))),
          row(cell("tableCell", p()), cell("tableCell", p())),
        ),
      ),
    );
  });

  it("ends a table at a blank line, a line without a pipe or another block, and may interrupt a paragraph", () => {
    const table = node(
      "table",
      undefined,
      node("tableRow", undefined, cell("tableHeader", p(text("a")))),
      node("tableRow", undefined, cell("tableCell", p(text("1")))),
    );
    expect(parse(lines("intro", "| a |", "| - |", "| 1 |", "", "after"))).toEqual(blocks(p(text("intro")), table, p(text("after"))));
    expect(parse(lines("| a |", "| - |", "| 1 |", "no pipe"))).toEqual(blocks(table, p(text("no pipe"))));
    expect(parse(lines("| a |", "| - |", "| 1 |", "# Heading"))).toEqual(blocks(table, node("heading", { level: 1 }, text("Heading"))));
  });

  it("leaves malformed tables as text", () => {
    // No delimiter row, a delimiter row of another width, a delimiter row that is not one.
    expect(parse(lines("| a | b |", "| 1 | 2 |"))).toEqual(blocks(p(text("| a | b | | 1 | 2 |"))));
    expect(parse(lines("| a | b |", "| --- |"))).toEqual(blocks(p(text("| a | b | | --- |"))));
    expect(parse(lines("| a | b |", "| x | y |", "| 1 | 2 |"))).toEqual(blocks(p(text("| a | b | | x | y | | 1 | 2 |"))));
    expect(parse(lines("a | b", "---"))[0]).toMatchObject({ type: "paragraph" });
    expect(parse("| only one line |")).toEqual(blocks(p(text("| only one line |"))));
  });
});

describe("attachments", () => {
  it("parses a line that is only an image or a file link with a valid id", () => {
    expect(parse(`![Diagram of the sync flow](attachment://${IMAGE_ATTACHMENT})`)).toEqual(
      blocks(node("image", { alt: "Diagram of the sync flow", attachmentId: IMAGE_ATTACHMENT })),
    );
    expect(parse(`![](attachment://${IMAGE_ATTACHMENT})`)).toEqual(blocks(node("image", { attachmentId: IMAGE_ATTACHMENT })));
    expect(parse(`[Quarterly report.pdf](attachment://${FILE_ATTACHMENT})`)).toEqual(
      blocks(node("file", { attachmentId: FILE_ATTACHMENT })),
    );
  });

  it("reads the link title of an image line as its caption, unescaping \\\\ and \\\" (2026-10-09)", () => {
    const line = (title: string): string => `![A cat](attachment://${IMAGE_ATTACHMENT} "${title}")`;
    const captioned = (caption: string): JsonValue[] =>
      blocks(node("image", { alt: "A cat", attachmentId: IMAGE_ATTACHMENT, caption }));
    expect(parse(line("Our cat, asleep"))).toEqual(captioned("Our cat, asleep"));
    expect(parse(line('She said \\"hi\\" at C:\\\\temp\\\\'))).toEqual(captioned('She said "hi" at C:\\temp\\'));
    // Another backslash stays one, and a parenthesis or a space inside the title is part of it.
    expect(parse(line("a \\n b (c)  d"))).toEqual(captioned("a \\n b (c)  d"));
    // Without an alt text, and with the id in another form.
    expect(parse(`![](attachment://${IMAGE_ATTACHMENT.toUpperCase().replaceAll("-", "")} "No alt")`)).toEqual(
      blocks(node("image", { attachmentId: IMAGE_ATTACHMENT, caption: "No alt" })),
    );
    // An empty title is no caption.
    expect(parse(line(""))).toEqual(blocks(node("image", { alt: "A cat", attachmentId: IMAGE_ATTACHMENT })));
    // A title that is not a well-formed one keeps the line as text, as before.
    for (const source of [
      `![A cat](attachment://${IMAGE_ATTACHMENT} "unterminated)`,
      `![A cat](attachment://${IMAGE_ATTACHMENT} 'single quotes')`,
      `![A cat](attachment://${IMAGE_ATTACHMENT} "a" "b")`,
      `![A cat](attachment://${IMAGE_ATTACHMENT}"glued")`,
      `![A cat](attachment://${IMAGE_ATTACHMENT} "ends with a backslash\\")`,
    ]) {
      expect(parse(source)[0], source).toMatchObject({ type: "paragraph" });
    }
    // A title on a file link is not read (a file has no caption); the line stays a link or text.
    expect(parse(`[f](attachment://${FILE_ATTACHMENT} "t")`)[0]).toMatchObject({ type: "paragraph" });
  });

  it("round-trips an image's caption through renderMarkdown", () => {
    const image = node("image", { id: "x", attachmentId: IMAGE_ATTACHMENT, alt: "A cat", caption: CAPTION });
    const markdown = render([image]);
    expect(markdown).toBe(
      `![A cat](attachment://${IMAGE_ATTACHMENT} "A \\"quoted\\" caption, C:\\\\temp\\\\ and (a bracket)")\n`,
    );
    expect(parse(markdown)).toEqual(blocks(node("image", { attachmentId: IMAGE_ATTACHMENT, alt: "A cat", caption: CAPTION })));
    expect(back(markdown)).toBe(markdown);
  });

  it("gives the id in its canonical form, whatever its case or hyphens", () => {
    expect(parse(`[f](attachment://${FILE_ATTACHMENT.toUpperCase().replaceAll("-", "")})`)).toEqual(
      blocks(node("file", { attachmentId: FILE_ATTACHMENT })),
    );
  });

  it("keeps links and images with an invalid id as text", () => {
    expect(parse("![alt](attachment://nope)")).toEqual(blocks(p(text("![alt](attachment://nope)"))));
    expect(parse("![alt](attachment://)")).toEqual(blocks(p(text("![alt](attachment://)"))));
    expect(parse("[name](attachment://nope)")).toEqual(blocks(p(text("name", "link:attachment://nope"))));
  });

  it("makes a link of a file link in the middle of text, and keeps an image there as text", () => {
    expect(parse(`see [f](attachment://${FILE_ATTACHMENT}) here`)).toEqual(
      blocks(p(text("see "), text("f", `link:attachment://${FILE_ATTACHMENT}`), text(" here"))),
    );
    expect(parse(`see ![a](attachment://${IMAGE_ATTACHMENT}) here`)).toEqual(
      blocks(p(text(`see ![a](attachment://${IMAGE_ATTACHMENT}) here`))),
    );
  });

  it("takes a line of its own out of a paragraph", () => {
    expect(parse(lines("text", `![a](attachment://${IMAGE_ATTACHMENT})`, "more"))).toEqual(
      blocks(p(text("text")), node("image", { alt: "a", attachmentId: IMAGE_ATTACHMENT }), p(text("more"))),
    );
  });
});

describe("robustness", () => {
  it("does not overflow on deeply nested containers", () => {
    const quotes = `${">".repeat(5000)} deep`;
    const text5 = parseMarkdown(quotes);
    expect(text5).toHaveLength(1);
    const lists = Array.from({ length: 3000 }, (_, depth) => `${"  ".repeat(depth)}- x`).join("\n");
    expect(parseMarkdown(lists)).toHaveLength(1);
  });

  it("stays fast on text made of delimiters", () => {
    const started = Date.now();
    for (const unit of ["`a ", "[", "*a ", "a* ", "_a ", "](", "<u>", "~~a ", "| a ", "- ", "> ", "a\n", "- a\n", "[a](b) ", "[a](<", "[a](", '[a](x "']) {
      parseMarkdown(unit.repeat(20_000));
    }
    expect(Date.now() - started).toBeLessThan(5000);
  });

  /** Random sources made of `pieces`, from a fixed seed. */
  function sources(pieces: readonly string[], rounds: number, seed: number): string[] {
    let state = seed;
    const random = (limit: number): number => {
      state = (Math.imul(state, 1103515245) + 12345) & 0x7fffffff;
      return state % limit;
    };
    return Array.from({ length: rounds }, () => {
      let source = "";
      for (let n = random(40); n > 0; n -= 1) {
        source += pieces[random(pieces.length)] ?? "";
      }
      return source;
    });
  }

  it("never throws, whatever it is given", () => {
    // Every character that means something in Markdown, in random order.
    const pieces = ["*", "**", "_", "~~", "`", "[", "]", "(", ")", "<u>", "</u>", "!", "|", " ", "  \n", "\n", "\n\n", "# ", "- ", "1. ", "> ", "---", "$$", FENCE, "a", "b c", "💡 ", "[ ] ", "<br>", "\\", "\t", "<details>", "</details>", "<summary>x</summary>", ":-:", "\uD83D"];
    for (const source of sources(pieces, 1000, 12345)) {
      expect(() => render(parse(source)), JSON.stringify(source)).not.toThrow();
    }
  });

  it("renders its output to a fixed point when the text has no marks to be ambiguous", () => {
    // Block markers and words: what the parser finds in them renders to text it finds again. (Lists are
    // left out: an item of several paragraphs is written one paragraph under the other, which reads back
    // as one paragraph.)
    const pieces = ["\n", "\n\n", "# ", "## ", "> ", "> 💡 ", "---", "$$", FENCE, "| a | b |", "| --- | --- |", "a", "b c", "<details>", "</details>", "<summary>x</summary>", "$x$", "$", "$5", "a $b c$"];
    for (const source of sources(pieces, 600, 777)) {
      const once = back(source);
      expect(back(once), JSON.stringify(source)).toBe(once);
    }
  });
});

// ----- the round trip -------------------------------------------------------------------

/** A page with a node of every kind the export writes, built as the editor builds it. */
function sampleBlocks(): JsonValue[] {
  const id = { id: "x" };
  return [
    node("heading", { ...id, level: 2 }, text("Title", "bold"), text(" here")),
    p(text("one "), text("two", "italic", "bold"), text(" "), text("three", "strike"), text(" "), text("under", "underline"), text(" "), text("code", "code"), hardBreak, text("next "), text("link", "link:https://example.com/a?b=1")),
    node("bulletList", id, node("listItem", id, p(text("a")), node("orderedList", { ...id, start: 4 }, node("listItem", id, p(text("x"))))), node("listItem", id, p(text("b")))),
    node("taskList", id, node("taskItem", { ...id, checked: true }, p(text("done"))), node("taskItem", { ...id, checked: false }, p(text("todo")), node("bulletList", id, node("listItem", id, p(text("sub")))))),
    node("blockquote", id, p(text("quote")), node("bulletList", id, node("listItem", id, p(text("in quote"))))),
    node("callout", { ...id, icon: "⚠️", color: "red" }, p(text("careful")), p(text("second"))),
    node("callout", id, p(text("default icon"))),
    node("codeBlock", { ...id, language: "ts" }, text("let a = 1;\n\nlet b = 2;")),
    node("codeBlock", id, text("no language")),
    node("mathBlock", id, text("a^2 + b^2 = c^2")),
    p(text("energy "), { type: "mathInline", attrs: { ...id, latex: "E=mc^2" } }, text(" and "), { type: "mathInline", attrs: { ...id, latex: "a_i^2" } }, text(".")),
    node("horizontalRule", id),
    node("details", { ...id, open: false }, node("detailsSummary", id, text("More")), node("detailsContent", id, p(text("inside")), node("bulletList", id, node("listItem", id, p(text("x")))))),
    node("table", id, node("tableRow", id, node("tableHeader", id, p(text("H1"))), node("tableHeader", id, p(text("H2")))), node("tableRow", id, node("tableCell", id, p(text("a | b"))), node("tableCell", id, p(text("x")), p(text("y", "bold"))))),
    node("image", { ...id, attachmentId: IMAGE_ATTACHMENT, alt: "An image", width: 320 }),
    node("image", { ...id, attachmentId: IMAGE_ATTACHMENT, alt: "Captioned", widthPercent: 40, align: "right", caption: CAPTION }),
    node("file", { ...id, attachmentId: FILE_ATTACHMENT }),
    p(text("end")),
  ];
}

describe("the round trip with renderMarkdown", () => {
  it("renders the same Markdown after parsing it back, for every kind of block the export writes", () => {
    const names: Record<string, string> = { [FILE_ATTACHMENT]: "Report.pdf" };
    const rendered = renderMarkdown({ type: "doc", content: sampleBlocks() }, unresolved, { resolveAttachmentName: (id) => names[id] });
    const again = renderMarkdown({ type: "doc", content: parse(rendered) }, unresolved, { resolveAttachmentName: (id) => names[id] });
    expect(again).toBe(rendered);
  });

  it("gives back the page's structure, once the parts Markdown does not hold are normalised away", () => {
    const rendered = renderMarkdown({ type: "doc", content: sampleBlocks() }, unresolved);
    expect(parse(rendered)).toEqual(blocks(...lossy(sampleBlocks())));
  });

  it("round-trips Markdown written by hand in the export's shapes", () => {
    const samples = [
      lines("# Heading", "", "Paragraph with **bold**, *italic*, ~~strike~~, <u>underline</u>, `code` and [a link](https://x.org)."),
      lines("- a", "  - b", "    - c", "- d"),
      lines("1. one", "2. two", "   - nested", "   - more"),
      lines("5. five", "6. six"),
      lines("- [x] done", "- [ ] todo", "  - [ ] child"),
      lines("> quote", ">", "> - item", ">", "> second"),
      lines("> 🚀 Launch", ">", "> ```js", "> run();", "> ```"),
      lines(FENCE, "no language", FENCE),
      lines("$$", "x", "$$", "", "---", "", "end"),
      lines("<details>", "<summary>S</summary>", "", "body", "", "</details>"),
      lines("| a | b |", "| --- | --- |", "| `x` | [l](u) |", "| 1<br><br>2 | a \\| b |"),
      lines("one  ", "two", "", "three"),
      lines("- first  ", "  second", "- third"),
      lines("- item", "  ```", "  code", "  ```", "- next"),
    ];
    for (const sample of samples) {
      expect(back(sample), sample).toBe(sample);
    }
  });

  it("round-trips runs of marks drawn at random", () => {
    let seed = 987654;
    const random = (limit: number): number => {
      seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
      return seed % limit;
    };
    const words = ["alpha", "beta gamma", "x", "d e f", "naïve", "日本語"];
    const markSets: string[][] = [[], [], ["bold"], ["italic"], ["strike"], ["underline"], ["code"], ["bold", "italic"], ["link:https://x.org/p"], ["bold", "underline"], ["italic", "strike"]];
    for (let round = 0; round < 300; round += 1) {
      const runs: JsonValue[] = [];
      let afterBreak = true;
      let afterMarked = false;
      for (let n = 1 + random(6); n > 0; n -= 1) {
        const marks = markSets[random(markSets.length)] ?? [];
        const word = words[random(words.length)] ?? "";
        const breakAfter = n > 1 && random(5) === 0;
        if (marks.length > 0 && afterMarked) {
          // Two marked runs side by side are ambiguous without a character between them.
          runs.push(text(" "));
        }
        // Plain runs are padded with spaces, except against a line break (which drops them).
        const padded = `${afterBreak ? "" : " "}${word}${breakAfter || n === 1 ? "" : " "}`;
        runs.push(text(marks.length === 0 ? padded : word, ...marks));
        if (breakAfter) {
          runs.push(hardBreak);
        }
        afterBreak = breakAfter;
        afterMarked = marks.length > 0 && !breakAfter;
      }
      const source = render([p(...runs)]);
      expect(back(source), JSON.stringify(runs)).toBe(source);
    }
  });
});

describe("the round trip on the page-v1 fixture", () => {
  const names: Record<string, string> = {};

  /** The name the golden gives the sample's file, which the parser cannot know. */
  function resolveName(golden: string): (id: string) => string | undefined {
    const named = new RegExp(`\\[([^\\]]*)\\]\\(attachment://${FILE_ATTACHMENT}\\)`, "u").exec(golden)?.[1];
    return (id) => (id === FILE_ATTACHMENT && named !== undefined && named !== FILE_ATTACHMENT ? named : names[id]);
  }

  it("renders the sample, parses it and renders the same Markdown", () => {
    const shown = "Roadmap";
    const titleOf = (id: string): string | undefined => (id === TARGET_PAGE ? shown : undefined);
    const options = { resolveAttachmentName: (id: string): string | undefined => (id === FILE_ATTACHMENT ? "Q3.pdf" : undefined) };
    const rendered = renderMarkdown(samplePage(), titleOf, options);
    expect(renderMarkdown({ type: "doc", content: parse(rendered) }, titleOf, options)).toBe(rendered);
  });

  const golden = fixtureStatus(MARKDOWN_GOLDEN, "kb-model (cargo run -p kb-model --example make_golden)");
  it.skipIf(golden.missing)(`parses ${MARKDOWN_GOLDEN} and renders it unchanged${golden.note}`, () => {
    const expected = readFixtureText(MARKDOWN_GOLDEN);
    const resolveAttachmentName = resolveName(expected);
    // `[[Title]]` comes back as the text it is, so the titles need no resolving. Blocks the export writes
    // as nothing (a table of contents, a breadcrumb) leave blank lines that parsing collapses.
    const squeeze = (md: string): string => md.replace(/\n{3,}/gu, "\n\n");
    const again = renderMarkdown({ type: "doc", content: parse(expected) }, unresolved, { resolveAttachmentName });
    expect(squeeze(again)).toBe(squeeze(expected));
  });

  it("finds the structure of the sample in the golden, minus what Markdown cannot hold", () => {
    const titleOf = (id: string): string | undefined => (id === TARGET_PAGE ? "Linked page" : undefined);
    const page = samplePage() as Json;
    expect(parse(renderMarkdown(samplePage(), titleOf))).toEqual(
      blocks(...lossy((page.content as JsonValue[] | undefined) ?? [], titleOf, (page.whiteboards as Json | undefined) ?? {})),
    );
  });
});

// ----- what the Markdown of a page cannot hold ------------------------------------------

/** The marks the export writes as plain text (Markdown has no colour). */
const COLOUR_MARKS: ReadonlySet<string> = new Set(["textColor", "highlight"]);

/** The `pageId` attribute of a node. */
function idOf(source: Json): string {
  const id = (source.attrs as Json | undefined)?.pageId;
  return typeof id === "string" ? id : "";
}

/**
 * The nodes as `parseMarkdown` finds them in their own Markdown: ids gone, a `pageRef` the text
 * `[[Title]]`, a `dateMention` its ISO text (`2026-10-07`, `2026-10-07 14:30`: dates are not parsed back),
 * a `mathInline` itself (its source is one the rules read: no `$` inside, no space at its ends),
 * a `columnList` its blocks one after another, a `pageLink` the paragraph `[[Title]]`, a
 * table of contents, a breadcrumb, a list view and a template button nothing (the export writes them as that: the button's
 * content is template material, ADR-0056), a callout's `color`,
 * the `textColor` and `highlight` marks (the runs they leave join the ones next to them), an image's
 * `width`, `widthPercent` and `align` and a table cell's `colspan`, `rowspan` and `colwidth` gone (the export writes none of them),
 * empty paragraphs gone, and the defaults put in or left out (a callout's icon, `start` 1, `open` false).
 */
function lossy(
  nodes: readonly JsonValue[],
  titleOf: (id: string) => string | undefined = unresolved,
  boards: Json = {},
): JsonValue[] {
  const out: JsonValue[] = [];
  for (const raw of nodes) {
    const source = raw as Json;
    const content = lossy((source.content as JsonValue[] | undefined) ?? [], titleOf, boards);
    if (source.type === "columnList") {
      // The columns come back as their blocks one after another.
      out.push(...content.flatMap((column) => ((column as Json).content as JsonValue[] | undefined) ?? []));
      continue;
    }
    if (source.type === "pageRef") {
      const pageId = idOf(source);
      out.push(text(`[[${titleOf(pageId) ?? pageId}]]`));
      continue;
    }
    if (source.type === "dateMention") {
      const attrs = source.attrs as Json | undefined;
      const iso = dateMentionText(attrs?.date, attrs?.time);
      if (iso !== undefined) {
        out.push(text(iso));
      }
      continue;
    }
    if (source.type === "pageLink") {
      const pageId = idOf(source);
      out.push(node("paragraph", undefined, text(`[[${titleOf(pageId) ?? pageId}]]`)));
      continue;
    }
    if (source.type === "whiteboard") {
      // The export writes a block quote: the marker line in bold and one list item per text of the drawing.
      const id = (source.attrs as Json | undefined)?.id;
      const texts = whiteboardTextsOfPortable(typeof id === "string" ? boards[id] : undefined);
      out.push(
        node(
          "blockquote",
          undefined,
          p(text("Whiteboard", "bold")),
          ...(texts.length === 0 ? [] : [ul(...texts.map((entry) => item(p(text(entry)))))]),
        ),
      );
      continue;
    }
    if (source.type === "gallery") {
      // The export writes one image line per item; each comes back as an image block of its own.
      const items = ((source.attrs as Json | undefined)?.items as Json[] | undefined) ?? [];
      for (const item of items) {
        out.push(
          node("image", {
            attachmentId: item.attachmentId as JsonValue,
            ...(typeof item.caption === "string" && item.caption !== "" ? { caption: item.caption } : {}),
          }),
        );
      }
      continue;
    }
    if (
      source.type === "tableOfContents" ||
      source.type === "breadcrumb" ||
      source.type === "pageList" ||
      source.type === "templateButton"
    ) {
      continue;
    }
    const attrs: Json = { ...(source.attrs as Json | undefined) };
    if (source.type === "callout") {
      attrs.icon ??= "💡";
    }
    for (const key of ["id", "color", "width", "colspan", "rowspan", "colwidth"]) {
      delete attrs[key];
    }
    if (source.type === "image") {
      // The caption is the link title; the width in percent and the alignment are not written.
      delete attrs.widthPercent;
      delete attrs.align;
    }
    if (source.type === "file") {
      delete attrs.view; // the preview is layout: the Markdown is the file link (ADR-0031)
    }
    if (attrs.start === 1) {
      delete attrs.start;
    }
    if (attrs.open === false) {
      delete attrs.open;
    }
    if (source.type === "paragraph" && content.length === 0 && (source.content as JsonValue[] | undefined) === undefined) {
      continue; // an empty paragraph between blocks does not survive (inside cells it is the cell's own)
    }
    const result: Json = { type: typeof source.type === "string" ? source.type : "" };
    if (Object.keys(attrs).length > 0) {
      result.attrs = attrs;
    }
    if (content.length > 0) {
      result.content = content;
    }
    if (Array.isArray(source.marks)) {
      const kept = source.marks.filter((mark) => {
        const type = (mark as Json).type;
        return !(typeof type === "string" && COLOUR_MARKS.has(type));
      });
      if (kept.length > 0) {
        result.marks = kept;
      }
    }
    if (source.text !== undefined) {
      result.text = source.text;
    }
    out.push(result);
  }
  return out;
}

describe("date mentions (additive inline node, 2026-10-07)", () => {
  const mention = (date: string, time?: string): Json => ({
    type: "dateMention",
    attrs: time === undefined ? { id: "d", date } : { id: "d", date, time },
  });

  it("written as text by the clipboard flavour are not parsed back: the ISO text is plain text, one run with the text around it", () => {
    const source = render([p(text("Due "), mention("2026-10-07"), text(", review "), mention("2026-10-08", "14:30"), text("."))]);
    expect(source).toBe("Due 2026-10-07, review 2026-10-08 14:30.\n");
    expect(parse(source)).toEqual([p(text("Due 2026-10-07, review 2026-10-08 14:30."))]);
    expect(JSON.stringify(parse(source))).not.toContain("dateMention");
    // The text is the same after the round trip.
    expect(back(source)).toBe(source);
  });

  /** A mention as the parser makes it (no id: whoever inserts it assigns one). */
  const read = (attrs: Json): Json => ({ type: "dateMention", attrs });

  it("reads the <time> element of the file flavour as a mention, as the importer of the core does", () => {
    expect(parse('Today is <time datetime="2026-10-18">2026-10-18</time>.')).toEqual([
      p(text("Today is "), read({ date: "2026-10-18" }), text(".")),
    ]);
    expect(parse('<time datetime="2026-10-18T14:30">2026-10-18 14:30</time>')).toEqual([p(read({ date: "2026-10-18", time: "14:30" }))]);
    // The attributes the export writes: the reminder in minutes and, from a template, what it is filled in with (ADR-0056).
    expect(
      parse(
        'a <time datetime="2026-10-18" data-reminder="30" data-relative="today">x</time> b <time datetime="2026-10-18T09:05" data-relative="now">y</time>',
      ),
    ).toEqual([
      p(
        text("a "),
        read({ date: "2026-10-18", reminder: 30, relative: "today" }),
        text(" b "),
        read({ date: "2026-10-18", time: "09:05", relative: "now" }),
      ),
    ]);
    // What the importer reads as a date: a time after a space, seconds and a zone ignored, single or no quotes, any case.
    expect(parse("<TIME datetime='2026-10-18 8:05:59Z'>t</TIME> <time datetime=2026-10-19>t</time>")).toEqual([
      p(read({ date: "2026-10-18", time: "08:05" }), text(" "), read({ date: "2026-10-19" })),
    ]);
    // The text of the element is ignored, and the file flavour's own output reads back as the mention it came from.
    expect(parse('<time datetime="2026-10-18">whatever words</time>')).toEqual([p(read({ date: "2026-10-18" }))]);
    expect(parse('**bold <time datetime="2026-10-18">x</time>** end')).toEqual([
      p(text("bold ", "bold"), read({ date: "2026-10-18" }), text(" end")),
    ]);
  });

  it("drops an attribute that is not valid, and leaves text what is no date: tags and all", () => {
    // A bad reminder or a bad `data-relative` is left out; the mention stays.
    expect(parse('<time datetime="2026-10-18" data-reminder="-5" data-relative="yesterday">x</time>')).toEqual([
      p(read({ date: "2026-10-18" })),
    ]);
    for (const reminder of ["soon", "1.5", "", "99999999999"]) {
      expect(parse(`<time datetime="2026-10-18" data-reminder="${reminder}">x</time>`), reminder).toEqual([p(read({ date: "2026-10-18" }))]);
    }
    // No date, no mention.
    for (const source of [
      '<time datetime="2026-02-30">x</time>',
      '<time datetime="2026-10-18T25:00">x</time>',
      '<time datetime="soon">x</time>',
      '<time datetime="">x</time>',
      "<time>2026-10-18</time>",
      '<time datetime="2026-10-18">x <b>y</b></time>',
      '<time datetime="2026-10-18">never closed',
    ]) {
      expect(JSON.stringify(parse(source)), source).not.toContain("dateMention");
      expect(parse(source), source).toEqual([p(text(source))]);
    }
  });

  it("is read in a table cell and a list item, and not inside a code span", () => {
    const element = '<time datetime="2026-10-18">2026-10-18</time>';
    expect(JSON.stringify(parse(lines("| a | b |", "| --- | --- |", `| ${element} | x |`)))).toContain('"dateMention"');
    expect(JSON.stringify(parse(`- due ${element}`))).toContain('"dateMention"');
    expect(JSON.stringify(parse(`\`${element}\``))).not.toContain("dateMention");
    // A date written as text stays text (the clipboard flavour writes nothing else).
    expect(JSON.stringify(parse("due 2026-10-18"))).not.toContain("dateMention");
  });

  it("stay at the start of a line or a list item without being taken for a list marker or a heading", () => {
    for (const sample of [
      lines("2026-10-07 is the day"),
      lines("- 2026-10-07 14:30 call"),
      lines("1. 2026-10-07", "2. 2026-10-08 09:05"),
      lines("> 2026-10-07"),
      lines("## 2026-10-07"),
    ]) {
      expect(back(sample), sample).toBe(sample);
    }
    expect(parse("2026-10-07 is the day\n")).toEqual([p(text("2026-10-07 is the day"))]);
  });
});


describe("inline equations (additive inline node, 2026-10-07)", () => {
  const equation = (latex: string): Json => ({ type: "mathInline", attrs: { latex } });
  /** An equation as the export writes it, with an id. */
  const stored = (latex: string): Json => ({ type: "mathInline", attrs: { id: "m", latex } });

  it("reads `$latex$` in a line of text as a mathInline with that latex", () => {
    expect(parse("Energy $E=mc^2$ is $x$.")).toEqual(
      blocks(p(text("Energy "), equation("E=mc^2"), text(" is "), equation("x"), text("."))),
    );
    expect(parse("$x$")).toEqual(blocks(p(equation("x"))));
    expect(parse("$x$ and more")).toEqual(blocks(p(equation("x"), text(" and more"))));
    expect(parse("$x$$y$")).toEqual(blocks(p(equation("x"), equation("y"))));
    expect(parse("$\\frac{a}{b}$ and $a_i^2$")).toEqual(
      blocks(p(equation("\\frac{a}{b}"), text(" and "), equation("a_i^2"))),
    );
  });

  it("takes one character as an equation, and any characters but `$` and a line break between the dollars", () => {
    expect(parse("$x$")).toEqual(blocks(p(equation("x"))));
    expect(parse("$日本語 a b$")).toEqual(blocks(p(equation("日本語 a b"))));
    expect(parse("$[0,1)$ and $a|b$")).toEqual(blocks(p(equation("[0,1)"), text(" and "), equation("a|b"))));
  });

  it("does not read the inside as Markdown, so its asterisks, underscores and brackets stay LaTeX", () => {
    expect(parse("$a*b*c$")).toEqual(blocks(p(equation("a*b*c"))));
    expect(parse("$a_b_c$ and *it*")).toEqual(blocks(p(equation("a_b_c"), text(" and "), text("it", "italic"))));
    expect(parse("$[a](b)$")).toEqual(blocks(p(equation("[a](b)"))));
    expect(parse("$~~x~~$")).toEqual(blocks(p(equation("~~x~~"))));
  });

  it("needs no whitespace right after the opening `$` or before the closing one, so prices stay text", () => {
    for (const source of [
      "costs $5 and $10",
      "$ x$",
      "$x $",
      "a $ b $ c",
      "$5-$10",
      "from $5 to $10",
      "$x$1",
      "only $x",
      "x$ alone",
      "$",
      "$ $",
    ]) {
      expect(JSON.stringify(parse(source)), source).not.toContain("mathInline");
      expect(back(source), source).toBe(lines(source));
    }
    // A later pair that does qualify is still read.
    expect(parse("costs $5 and $x$")).toEqual(blocks(p(text("costs $5 and "), equation("x"))));
  });

  it("leaves `$$` to the math block, and reads a `$$` run in text as text", () => {
    expect(parse(lines("$$", "x", "$$"))).toEqual(blocks(node("mathBlock", undefined, text("x"))));
    expect(parse("$$x$$")).toEqual(blocks(node("mathBlock", undefined, text("x"))));
    expect(parse("a $$x$$ b")).toEqual(blocks(p(text("a $$x$$ b"))));
    expect(parse("$$$x$")).toEqual(blocks(p(text("$$$x$"))));
    expect(parse(lines("$$", "$x$", "$$"))).toEqual(blocks(node("mathBlock", undefined, text("$x$"))));
  });

  it("does not span lines, and a dollar after a backslash is text", () => {
    expect(parse(lines("$a", "b$"))).toEqual(blocks(p(text("$a b$"))));
    expect(parse("\\$x$")).toEqual(blocks(p(text("\\$x$"))));
    expect(parse("$x\\$")).toEqual(blocks(p(text("$x\\$"))));
    expect(parse("\\$5 and \\$10")).toEqual(blocks(p(text("\\$5 and \\$10"))));
  });

  it("stays inside code, and next to marks, links, headings, lists, quotes and table cells", () => {
    expect(parse("`$x$` and $y$")).toEqual(blocks(p(text("$x$", "code"), text(" and "), equation("y"))));
    expect(parse("**bold $x$ text**")).toEqual(blocks(p(text("bold ", "bold"), equation("x"), text(" text", "bold"))));
    expect(parse("[the $x$ page](https://example.com/p)")).toEqual(
      blocks(p(text("the ", "link:https://example.com/p"), equation("x"), text(" page", "link:https://example.com/p"))),
    );
    expect(parse("## Proof of $a^2$")).toEqual(blocks(node("heading", { level: 2 }, text("Proof of "), equation("a^2"))));
    expect(parse(lines("- Let $n>0$", "- $x$"))).toEqual(
      blocks(ul(item(p(text("Let "), equation("n>0"))), item(p(equation("x"))))),
    );
    expect(parse("> $x$ holds")).toEqual(blocks(node("blockquote", undefined, p(equation("x"), text(" holds")))));
    const cell = (value: JsonValue): Json => node("tableCell", undefined, p(value));
    expect(parse(lines("| a | b |", "| --- | --- |", "| $x$ | $\\|y\\|$ |"))).toEqual(
      blocks(
        node(
          "table",
          undefined,
          node("tableRow", undefined, node("tableHeader", undefined, p(text("a"))), node("tableHeader", undefined, p(text("b")))),
          node("tableRow", undefined, cell(equation("x")), cell(equation("|y|"))),
        ),
      ),
    );
  });

  it("keeps a hard break between equations on two lines", () => {
    expect(parse(lines("$x$  ", "$y$"))).toEqual(blocks(p(equation("x"), hardBreak, equation("y"))));
  });

  it("is written as `$latex$` and read back as the same equation, for any source the rules read", () => {
    for (const latex of ["E=mc^2", "x", "\\frac{a}{b}", "a_i^2 + b_i^2", "[0,1)", "日本語", "a|b", "a*b*c", "~~x~~", "a b  c"]) {
      const rendered = render([p(text("a "), stored(latex), text(" b"))]);
      expect(rendered).toBe(`a $${latex}$ b\n`);
      expect(parse(rendered), latex).toEqual(blocks(p(text("a "), equation(latex), text(" b"))));
      expect(back(rendered), latex).toBe(rendered);
    }
    // At the start and end of a line, next to punctuation and in a list.
    for (const sample of [lines("$x$"), lines("($x$), $y$; $z$."), lines("- $x$", "- b $y$"), lines("> $x$ and **bold**")]) {
      expect(back(sample), sample).toBe(sample);
    }
  });

  it("does not come back from a source the rules cannot read: it is read as the text and equations they find", () => {
    // `$` inside, whitespace at either end, a backslash last, a digit right after, a line break inside.
    for (const latex of ["a$b", " x", "x ", "x\\", "a\nb"]) {
      const rendered = render([p(text("a "), stored(latex), text(" b"))]);
      expect(JSON.stringify(parse(rendered)).includes(`"latex":${JSON.stringify(latex)}`), latex).toBe(false);
    }
    expect(JSON.stringify(parse(render([p(stored("x"), text("1"))])))).not.toContain("mathInline");
  });

  it("writes an empty equation as nothing, as the export does", () => {
    expect(render([p(text("a "), stored(""), text(" b"))])).toBe("a  b\n");
    expect(JSON.stringify(parse("a  b"))).not.toContain("mathInline");
  });
});
