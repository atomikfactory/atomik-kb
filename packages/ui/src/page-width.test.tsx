// Page width (ADR-0035): every page fills the main area, text and blocks alike. jsdom has no layout, so the rules
// of app.css are read from the stylesheet: the page's section, and, for the elements the editor really draws,
// that no rule narrows or centres them (a block nested in another one is simply as wide as its parent). The page
// menu has no width setting, and the Home screen and its banners start where the page's text does.

import type { JsonValue, MemoryTransportOptions } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { screen, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { MODIFIER } from "./shortcuts";
import messages from "./i18n/en.json";
import type * as Engine from "./editor/pdf-engine";
import { openPdf } from "./editor/pdf-engine";
import type { PdfDocument } from "./editor/pdf-engine";
import { paragraphs, pid } from "./test/editor";
import { openPage, renderPages } from "./test/page";
import { testPdf } from "./test/pdf";

vi.mock("./editor/pdf-engine", async (importOriginal) => ({
  ...(await importOriginal<typeof Engine>()),
  openPdf: vi.fn(),
}));

afterEach(() => {
  vi.restoreAllMocks();
});

// ----- the stylesheet -----------------------------------------------------------------------

const CSS = readFileSync("src/styles/app.css", "utf8");

/** The rule whose selector starts a line with `start`: its whole selector and its declarations. */
function rule(start: string): { selector: string; body: string } {
  const at = CSS.indexOf(`\n${start}`) + 1;
  if (at === 0) {
    throw new Error(`no rule starts with ${start}`);
  }
  const open = CSS.indexOf("{", at);
  const close = CSS.indexOf("}", open);
  return { selector: CSS.slice(at, open).trim(), body: CSS.slice(open + 1, close) };
}

interface Rule {
  selector: string;
  body: string;
}

/** Every style rule of the sheet, those inside `@media`, `@supports` and `@container` too (other at-rules are skipped). */
function rulesOf(text: string): Rule[] {
  const rules: Rule[] = [];
  let from = 0;
  while (from < text.length) {
    const open = text.indexOf("{", from);
    if (open < 0) {
      break;
    }
    let depth = 1;
    let close = open + 1;
    while (depth > 0 && close < text.length) {
      depth += text[close] === "{" ? 1 : text[close] === "}" ? -1 : 0;
      close += 1;
    }
    const prelude = text.slice(from, open).trim();
    const inner = text.slice(open + 1, close - 1);
    if (/^@(media|supports|container)\b/u.test(prelude)) {
      rules.push(...rulesOf(inner));
    } else if (!prelude.startsWith("@")) {
      rules.push({ selector: prelude, body: inner });
    }
    from = close;
  }
  return rules;
}

const RULES = rulesOf(CSS.replace(/\/\*[\s\S]*?\*\//gu, ""));

/** The alternatives of a selector list (commas inside brackets and parentheses stay). */
function alternatives(list: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  for (const char of list) {
    if (depth === 0 && char === ",") {
      parts.push(current.trim());
      current = "";
      continue;
    }
    depth += char === "(" || char === "[" ? 1 : char === ")" || char === "]" ? -1 : 0;
    current += char;
  }
  parts.push(current.trim());
  return parts.filter((part) => part !== "");
}

/** Whether `element` is one the selector list names (a selector jsdom cannot read names nothing). */
function matches(element: Element, list: string): boolean {
  return alternatives(list).some((selector) => {
    try {
      return element.matches(selector);
    } catch {
      return false;
    }
  });
}

/** Whether a margin declaration leaves the room on the left and the right to the browser (it centres a block). */
function centres(property: string, value: string): boolean {
  const parts = value.trim().split(/\s+/u);
  switch (property) {
    case "margin-inline":
    case "margin-left":
    case "margin-right":
      return parts.includes("auto");
    case "margin":
      return parts.length === 1
        ? parts[0] === "auto"
        : parts.length === 4
          ? parts[1] === "auto" || parts[3] === "auto"
          : parts[1] === "auto";
    default:
      return false;
  }
}

/**
 * What the sheet does to the width of `element` beyond filling its parent: a `max-width` other than `none` or `100%`, a
 * `width` that is a length, or auto margins (the centring of a column). The cascade is not applied: a rule that names
 * the element counts, so the page's section, which a general rule names and a more specific one overrides, is left to
 * its own test.
 */
function narrowings(element: Element): string[] {
  const found: string[] = [];
  for (const { selector, body } of RULES) {
    if (!matches(element, selector)) {
      continue;
    }
    for (const [, property = "", value = ""] of body.matchAll(/(?:^|;)\s*([a-z-]+)\s*:\s*([^;]+)/gu)) {
      const text = value.trim();
      if (property === "max-width" && text !== "none" && text !== "100%") {
        found.push(`${selector} { ${property}: ${text} }`);
      } else if (property === "width" && /^[\d.]+(?:px|rem|em|ch|vw)\b|^(?:calc|min|max|clamp|var)\(/u.test(text)) {
        found.push(`${selector} { ${property}: ${text} }`);
      } else if (centres(property, text)) {
        found.push(`${selector} { ${property}: ${text} }`);
      }
    }
  }
  return found;
}

describe("the stylesheet", () => {
  it("fills the main area: the page's section has no maximum width, and keeps the 52 px gutter of the block handle", () => {
    const section = rule(".content-body > section.page {");
    expect(section.body).toContain("max-width: none");
    expect(section.body).toContain("padding-left: 52px");
    expect(CSS).not.toContain("812px"); // the old column of 720 + 52
    // The narrow window drops the gutter, as it did.
    const narrow = CSS.slice(CSS.indexOf("@media (max-width: 640px) {\n  .content-body > section.page,"));
    expect(narrow.slice(0, 160)).toContain("padding-left: 0");
  });

  it("has nothing left of a text column or of a per-page width", () => {
    for (const gone of ["--page-text-width", "is-full-width", "--block-left", "menuitemcheckbox"]) {
      expect(CSS, gone).not.toContain(gone);
    }
    expect(Object.keys(messages).filter((key) => key.startsWith("page.fullWidth"))).toEqual([]);
  });

  it("lets the grid of a table view fill its block, and scroll when the block is narrower", () => {
    expect(rule(".page-editor-content .page-table-grid {").body).toContain("width: 100%");
    expect(rule(".page-editor-content .page-table-grid {").body).not.toContain("max-content");
  });

  it("starts the Home screen and the banners where the page's text starts, with the width they had", () => {
    expect(rule(".content-body > :is(.home-banners, section.home) {").body).toContain("padding-left: 52px");
    // 720 px of text and the gutter. The banners keep the 720 px of their own rules.
    expect(rule(".content-body > section.home {").body).toContain("max-width: 772px");
    expect(rule(".kit-banner {").body).toContain("max-width: 720px");
    const narrow = CSS.slice(CSS.indexOf("@media (max-width: 640px) {\n  .content-body > section.page,"));
    expect(narrow.slice(0, 160)).toContain(".content-body > :is(.home-banners, section.home)");
  });
});

// ----- the blocks the editor draws -----------------------------------------------------------

const PDF = "a0000000-0000-4000-8000-0000000000f1";
const IMAGE = "a0000000-0000-4000-8000-0000000000f2";

let nextId = 1000;

type Json = { [key: string]: JsonValue };

/** A node with an id of its own, and its content. */
function node(type: string, attrs: Json = {}, ...content: JsonValue[]): Json {
  nextId += 1;
  return { type, attrs: { id: pid(nextId), ...attrs }, ...(content.length === 0 ? {} : { content }) };
}

function text(value: string): JsonValue {
  return { type: "text", text: value };
}

function para(value: string): Json {
  return node("paragraph", {}, text(value));
}

function table(): Json {
  const cell = (value: string): Json => node("tableCell", { colspan: 1, rowspan: 1 }, para(value));
  return node("table", {}, node("tableRow", {}, cell("a"), cell("b")), node("tableRow", {}, cell("c"), cell("d")));
}

const gallery = (): Json => node("gallery", { columns: 3, items: [] });
const whiteboard = (): Json => node("whiteboard", { height: 300 });
const list = (layout: string): Json =>
  node("pageList", { source: { kind: "children" }, layout, properties: [], sort: { by: "title", dir: "asc" } });

/** Every kind of top-level block, in the order of the page, and blocks nested in a toggle and in columns. */
function blocks(): Json[] {
  return [
    para("Intro"),
    node("heading", { level: 2 }, text("Title")),
    node("bulletList", {}, node("listItem", {}, para("one"))),
    node("taskList", {}, node("taskItem", { checked: false }, para("do"))),
    node("blockquote", {}, para("said")),
    node("codeBlock", { language: "rust" }, text("fn main() {}")),
    node("mathBlock", {}, text("x^2")),
    node("horizontalRule"),
    node("callout", { icon: "💡", color: "" }, para("note")),
    node(
      "details",
      { open: true },
      node("detailsSummary", {}, text("More")),
      node("detailsContent", {}, para("inside"), table(), gallery(), whiteboard(), list("table"), list("board")),
    ),
    table(),
    node("image", { attachmentId: IMAGE }),
    node("file", { attachmentId: PDF }),
    node("file", { attachmentId: PDF, view: "preview" }),
    gallery(),
    whiteboard(),
    node("columnList", {}, node("column", {}, para("left"), table()), node("column", {}, para("right"))),
    node("tableOfContents"),
    node("breadcrumb"),
    node("pageLink", { pageId: pid(999) }),
    list("list"),
    list("table"),
    list("board"),
  ];
}

function fakePdf(): PdfDocument {
  return {
    pages: 1,
    page: () =>
      Promise.resolve({
        width: 200,
        height: 300,
        render: () => ({ done: Promise.resolve(), cancel: vi.fn() }),
      }),
    destroy: () => Promise.resolve(),
  };
}

const attachments: MemoryTransportOptions["attachments"] = [
  { attachmentId: PDF, name: "report.pdf", mime: "application/pdf", bytes: testPdf({ pages: 1 }) },
];

/** The DOM element of every top-level block of the open page. */
function topLevel(editor: Editor): HTMLElement[] {
  const elements: HTMLElement[] = [];
  editor.state.doc.forEach((_child, offset) => {
    const dom = editor.view.nodeDOM(offset);
    if (!(dom instanceof HTMLElement)) {
      throw new Error(`no element at ${String(offset)}`);
    }
    elements.push(dom);
  });
  return elements;
}

function pageSection(): HTMLElement {
  const section = document.querySelector("section.page");
  if (!(section instanceof HTMLElement)) {
    throw new Error("no page is open");
  }
  return section;
}

async function openBlocks() {
  vi.mocked(openPdf).mockImplementation(() => Promise.resolve(fakePdf()));
  const content: JsonValue = { type: "doc", content: blocks() };
  const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }], { attachments });
  const editor = await openPage(app.user, "Notes");
  await vi.waitFor(() => {
    expect(editor.view.dom.querySelector(".is-pdf-preview")).not.toBeNull();
  });
  return { ...app, editor };
}

describe("the page's own parts and blocks", () => {
  it("are not narrowed or centred: the header, the properties, the editor, the backlinks and every block", async () => {
    const { editor } = await openBlocks();
    const section = pageSection();
    const backlinks = document.createElement("section"); // what BacklinksPanel draws under a page that is linked to
    backlinks.className = "backlinks";
    section.append(backlinks);

    const parts = [
      section.querySelector(":scope > .page-header"),
      section.querySelector(":scope > .properties"),
      section.querySelector(":scope > .page-editor"),
      section.querySelector(":scope > .page-editor > .page-editor-body"),
      editor.view.dom,
      backlinks,
      ...topLevel(editor),
    ];
    expect(parts.filter((part) => part === null)).toEqual([]);
    expect(topLevel(editor)).toHaveLength(blocks().length);
    for (const part of parts) {
      expect(part === null ? [] : narrowings(part), part?.outerHTML.slice(0, 80)).toEqual([]);
    }
    // The editor's root is the child of the body that is the child of `.page-editor`: nothing sits between them.
    expect(editor.view.dom.matches(".page-editor > .page-editor-body > .page-editor-content")).toBe(true);
  });

  it("follow their parent when nested: a table, a gallery, a whiteboard and the views in a toggle, a table in a column", async () => {
    const { editor } = await openBlocks();
    const nested = Array.from(
      editor.view.dom.querySelectorAll<HTMLElement>(
        ".tableWrapper, [data-type='gallery'], [data-type='whiteboard'], [data-type='pageList'], [data-type='columnList']",
      ),
    ).filter((element) => element.parentElement !== editor.view.dom);
    // Five in the toggle (the table, the gallery, the whiteboard, two views) and the table in the first column.
    expect(nested).toHaveLength(6);
    for (const element of nested) {
      expect(narrowings(element), element.outerHTML.slice(0, 80)).toEqual([]);
    }
  });

  it("detect a rule that does narrow or centre (the checker finds what it should)", () => {
    const probe = document.createElement("div");
    probe.className = "page-editor-content";
    const inner = document.createElement("div");
    inner.className = "callout";
    probe.append(inner);
    document.body.append(probe);
    try {
      expect(narrowings(probe)).toEqual([]);
      // The columns hang over the edge with a negative margin and the page list has `margin: 6px 0`: neither centres.
      const columns = document.createElement("div");
      columns.setAttribute("data-type", "columnList");
      probe.append(columns);
      expect(narrowings(columns)).toEqual([]);
      // Whereas a column of a width of its own is what the checker reports.
      const column = document.createElement("div");
      column.className = "kit-banner";
      document.body.append(column);
      expect(narrowings(column)).toEqual([".kit-banner { max-width: 720px }"]);
      column.remove();
    } finally {
      probe.remove();
    }
  });
});

// ----- the page menu ---------------------------------------------------------------------------

describe("the page menu", () => {
  it("has no width setting: Find, Copy as Markdown, Copy link to page (ADR-0040), the import and export entries (ADR-0036), Duplicate and Page info only", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("x") }] }]);
    await openPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    const menu = await screen.findByRole("menu", { name: t("page.actions") });
    expect(within(menu).getAllByRole("menuitem").map((entry) => entry.textContent)).toEqual([
      t("page.find"),
      t("comments.menu.page"),
      t("page.copyMarkdown"),
      t("page.copyLink"),
      t("viewLink.menu"), // a personal space's page can be given a view link (ADR-0051)
      t("interop.menu.export"),
      `${t("interop.menu.print")}${MODIFIER}+P`, // the label and the keys that do the same
      t("interop.menu.importPage"),
      t("page.duplicate"),
      t("template.turnInto"),
      t("history.dialog.menu"), // the page's history (ADR-0057)
      t("page.info"),
    ]);
    expect(within(menu).queryByRole("menuitemcheckbox")).toBeNull();
  });
});

// ----- Home and its banners ----------------------------------------------------------------------

describe("the Home screen and the banners above a page", () => {
  const NOTES: MemoryTransportOptions["spaces"] = [
    { name: "Personal", pages: [{ title: "Notes", content: paragraphs("x") }] },
  ];
  const HOME = rule(".content-body > :is(.home-banners, section.home) {");

  it("start where the page's text does, with no page open", async () => {
    await renderPages(NOTES);
    const banner = await screen.findByLabelText(t("kitBanner.label"));
    const home = document.querySelector(".content-body > section.home");
    expect(home).not.toBeNull();
    expect(matches(home as Element, HOME.selector)).toBe(true);
    const wrapper = banner.parentElement;
    expect(wrapper).toHaveClass("home-banners");
    expect(wrapper?.parentElement).toHaveClass("content-body");
    expect(matches(wrapper as Element, HOME.selector)).toBe(true);
    // Left aligned: nothing centres them.
    for (const element of [home, wrapper, banner]) {
      expect(narrowings(element as Element).filter((entry) => entry.includes("auto"))).toEqual([]);
    }
  });

  it("stand above the page, on the same left edge", async () => {
    const { user } = await renderPages(NOTES);
    await screen.findByLabelText(t("kitBanner.label"));
    await openPage(user, "Notes");
    const wrapper = (await screen.findByLabelText(t("kitBanner.label"))).parentElement;
    expect(wrapper).toHaveClass("home-banners");
    expect(wrapper?.nextElementSibling).toBe(pageSection());
    expect(document.querySelector(".content-body > section.home")).toBeNull();
  });

  it("leave the Settings screen alone", async () => {
    const { user } = await renderPages(NOTES);
    await screen.findByLabelText(t("kitBanner.label"));
    await user.click(within(screen.getByRole("navigation", { name: t("nav.label") })).getByRole("button", { name: t("nav.settings") }));
    await screen.findByRole("heading", { level: 1, name: t("nav.settings") });
    expect(document.querySelector(".home-banners")).toBeNull();
    expect(document.querySelector("section.home")).toBeNull();
    for (const section of Array.from(document.querySelectorAll(".content-body > section"))) {
      expect(matches(section, HOME.selector)).toBe(false);
    }
  });
});
