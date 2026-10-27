// The in-page half of the page width pass (drive-width.ps1, ADR-0035: every page fills the main area, text and blocks
// alike): trusted mouse and key input, the boxes of the page and the window's size, over the DevTools port of the
// app's WebView2 (see cdp.mjs). Phases:
//
//   node drive-width.mjs launch   <outfile>  the marker of the document is set
//   node drive-width.mjs page     <outfile>  the page "Wide": paragraph, heading, table, columns (a table in the left one), gallery,
//                                            whiteboard, table view (three pages in it), board view, closing paragraph
//   node drive-width.mjs sizes    <outfile>  1100x760, 1500x900, maximised, restored, 800x600: boxes of the section, header, blocks,
//                                            the nested table and the table view's grid; F11 probed for a real full screen
//   node drive-width.mjs handle   <outfile>  the block handle sits left of a paragraph, the table, the columns and the table view
//   node drive-width.mjs floating <outfile>  the format menu, the table and columns toolbars and the slash menu beside their blocks
//   node drive-width.mjs home     <outfile>  the recovery-kit banner and the Home screen (a new tab) start at the page's left edge
//   node drive-width.mjs dark     <outfile>  the maximised page in the dark theme
//   node drive-width.mjs menu     <outfile>  the page options menu has no "Full width" item; the maximised layout is recorded
//   node drive-width.mjs restart  <outfile>  after the app was started again on the same data: "Wide" has the same layout
//   node drive-width.mjs verify   <outfile>  the summary of every phase
//
// Every phase writes what the page asked the network for, what failed, the console and the content-security-policy
// violations it saw, as JSON, and exits 1 when its own check fails, when the page asked another host for
// something, when a violation, a failed request or a console error was seen, or when the document of the
// webview was replaced (the marker `window.__widthMarker` set by `launch` is lost by a reload, and `location.href`
// must not change either).
//
// The layout is read from the real boxes (getBoundingClientRect) at real window sizes: the window is resized with
// Win32 by drive-width.ps1's `-Step window` (MoveWindow for a size, SW_MAXIMIZE, SW_RESTORE), and the page's own
// innerWidth is read back, so a size is the size of the WEBVIEW, not of the frame.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { connect } from "./cdp.mjs";

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.WIDTH_OUT_DIR ?? join(HERE, "out");
const STATE_FILE = join(OUT, "width-state.json");
const PHASES = ["launch", "page", "sizes", "handle", "floating", "home", "dark", "menu", "restart"];
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
mkdirSync(OUT, { recursive: true });

const OLD_COLUMN = 720; // the width of the text column before ADR-0035 (a page used to stop there)
const PAGE_TITLE = "Wide";

function readState() {
  return existsSync(STATE_FILE) ? JSON.parse(readFileSync(STATE_FILE, "utf8")) : {};
}
function writeState(patch) {
  writeFileSync(STATE_FILE, JSON.stringify({ ...readState(), ...patch }, null, 2));
}

// ---------------------------------------------------------------------------------------------------------------
// verify: the summary of the phase reports
// ---------------------------------------------------------------------------------------------------------------
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0 };
  for (const name of PHASES) {
    const file = join(OUT, `width-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = { ok: report.ok, checks: report.notes.filter((n) => !n.startsWith("info:")).length, failed: report.notes.filter((n) => n.startsWith("FAIL")).length };
    summary.ok = summary.ok && report.ok;
    summary.captures.push(...(report.captures ?? []));
    summary.failedNotes.push(...report.notes.filter((n) => n.startsWith("FAIL")).map((n) => `${name}: ${n}`));
    summary.foreignRequests += report.foreignRequests?.length ?? 0;
    summary.cspViolations += report.cspViolations?.length ?? 0;
    summary.failedRequests += report.failures?.length ?? 0;
    summary.consoleErrors += (report.console ?? []).filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).length;
  }
  writeFileSync(outFile, JSON.stringify(summary, null, 2));
  console.log(JSON.stringify({ phase, ok: summary.ok, phases: summary.phases, failedNotes: summary.failedNotes }, null, 2));
  process.exit(summary.ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// the page
// ---------------------------------------------------------------------------------------------------------------
const page = await connect();
// CSP violations are reported to the page as events; collect them from here on.
await page.evaluate(`(() => {
  window.__cspViolations = [];
  document.addEventListener("securitypolicyviolation", (e) => {
    window.__cspViolations.push({ directive: e.violatedDirective, blocked: e.blockedURI, source: e.sourceFile });
  });
})()`);

const result = { phase, ok: true, notes: [], captures: [] };
class PhaseStop extends Error {}

function check(condition, note) {
  result.notes.push(`${condition ? "ok" : "FAIL"}: ${note}`);
  if (!condition) {
    result.ok = false;
  }
  return condition;
}
/** A check the rest of the phase cannot go on without: the phase stops here when it fails. */
function must(condition, note) {
  if (!check(condition, note)) {
    throw new PhaseStop(note);
  }
}
const info = (note) => result.notes.push(`info: ${note}`);
const near = (a, b, tolerance) => Math.abs(a - b) <= tolerance;
const round = (n) => Math.round(n * 10) / 10;

// ----- input ----------------------------------------------------------------------------------------------------

const BUTTON_BIT = { left: 1, right: 2, middle: 4 };
const send = (method, params) => page.send(method, params);

/** A mouse button pressed and released at a point. */
async function mouse(x, y, { button = "left", clickCount = 1 } = {}) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, buttons: BUTTON_BIT[button], clickCount });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, buttons: 0, clickCount });
  await wait(250);
}

/** The pointer moved to a point, without a button (a hover). */
async function hover(x, y) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await wait(300);
}

// ----- reading the page -----------------------------------------------------------------------------------------

/** The box of the first element matching `selector` (or, with `text`, whose text is exactly it), in page coordinates, or null. */
async function box(selector, text = null) {
  return page.evaluate(`(() => {
    const wanted = ${JSON.stringify(text)};
    const candidates = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const el = wanted === null ? candidates[0] : candidates.find((e) => e.textContent.trim() === wanted);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })()`);
}

async function waitFor(selector, what, ms = 20000, text = null) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const found = await box(selector, text);
    if (found !== null && found.w > 0 && found.h > 0) {
      return found;
    }
    await wait(200);
  }
  must(false, `${what} did not appear (${selector})`);
  return null;
}

async function clickOn(selector, what, options = {}) {
  const found = await waitFor(selector, what, 20000, options.text ?? null);
  await mouse(found.cx, found.cy, options);
  return found;
}

/** The title shown in the main area. */
const heading = () => page.evaluate(`document.querySelector(".content-body h1")?.textContent.trim() ?? null`);

/** Captures the window with PrintWindow (capture.ps1's Capture-Window, run in PowerShell). */
function capture(name) {
  const path = join(OUT, `width-${name}.png`);
  try {
    execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        '. $env:WIDTH_CAPTURE_PS1; $p = Get-Process -Name atomik-kb-desktop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1; Capture-Window $p.MainWindowHandle $env:WIDTH_CAPTURE_PATH',
      ],
      { env: { ...process.env, WIDTH_CAPTURE_PATH: path }, timeout: 40000, stdio: "pipe" },
    );
    result.captures.push(path);
  } catch (error) {
    check(false, `capture ${name} failed: ${error instanceof Error ? error.message : String(error)}`);
  }
}

/** Leaves nothing open from an earlier phase that failed half-way. */
async function normalize() {
  for (let i = 0; i < 3; i++) {
    const open = await page.evaluate(`!!document.querySelector('[role="menu"], [role="dialog"], [role="listbox"]')`);
    if (!open) {
      return;
    }
    await page.key("Escape", "Escape", 27);
    await wait(200);
  }
}

// ----- the window -----------------------------------------------------------------------------------------------

/** drive-width.ps1 -Step window: the window's rectangle after the change, as the script prints it. */
function win(mode, width = 0, height = 0) {
  const out = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", process.env.WIDTH_PS1, "-Step", "window", "-Mode", mode, "-Width", String(width), "-Height", String(height)],
    { timeout: 40000, stdio: "pipe" },
  ).toString();
  const line = out.trim().split(/\r?\n/).filter((l) => l.startsWith("{")).pop();
  if (line === undefined) {
    throw new Error(`window helper printed no JSON: ${out}`);
  }
  return JSON.parse(line);
}

const inner = () => page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio })`);

/** Waits until the webview's size has not changed for a moment, and the layout after it. */
async function settle() {
  let last = "";
  let steady = 0;
  for (let i = 0; i < 60 && steady < 3; i++) {
    const now = await inner();
    const key = `${now.w}x${now.h}`;
    steady = key === last ? steady + 1 : 0;
    last = key;
    await wait(150);
  }
  await wait(400);
  return inner();
}

/** Sets the size of the WEBVIEW (the frame is added to the window's size, measured, not guessed). */
async function setInner(width, height) {
  let rect = win("info");
  let now = await inner();
  for (let i = 0; i < 4; i++) {
    if (near(now.w, width, 1) && near(now.h, height, 1) && !rect.zoomed) {
      break;
    }
    const dpr = now.dpr;
    rect = win("size", Math.round(rect.width + (width - now.w) * dpr), Math.round(rect.height + (height - now.h) * dpr));
    now = await settle();
    rect = win("info");
  }
  return { rect, now };
}

// ----- the page "Wide" --------------------------------------------------------------------------------------------

/** Shows the page "Wide" from its top (the sidebar's row when another page, or the Home, is shown). */
async function openWide() {
  await normalize();
  if ((await heading()) !== PAGE_TITLE) {
    const row = await box(`[role="treeitem"][aria-label=${JSON.stringify(PAGE_TITLE)}] .tree-title`);
    must(row !== null, `the sidebar has a row "${PAGE_TITLE}"`);
    await mouse(row.cx, row.cy);
  }
  await waitFor(".content-body > section.page .ProseMirror", `the page "${PAGE_TITLE}" with its editor`);
  must((await heading()) === PAGE_TITLE, `the page "${PAGE_TITLE}" is open (${await heading()})`);
  await wait(500);
  await scrollTo(0);
}

async function scrollTo(top) {
  await page.evaluate(`(() => { document.querySelector(".content-body").scrollTop = ${top}; })()`);
  await wait(200);
}

/** The boxes of the section, the header and every top-level block of the page, and of the nested table. */
async function measure() {
  return page.evaluate(`(() => {
    const px = (v) => Number.parseFloat(v) || 0;
    const rect = (el) => { const b = el.getBoundingClientRect(); return { left: b.left, right: b.right, top: b.top, bottom: b.bottom, width: b.width, height: b.height }; };
    const body = document.querySelector(".content-body");
    const section = document.querySelector(".content-body > section.page");
    const root = document.querySelector(".content-body > section.page .ProseMirror");
    if (!body || !section || !root) return null;
    const bodyCs = getComputedStyle(body);
    const cs = getComputedStyle(section);
    const sr = rect(section);
    const content = {
      left: sr.left + px(cs.borderLeftWidth) + px(cs.paddingLeft),
      right: sr.right - px(cs.borderRightWidth) - px(cs.paddingRight),
    };
    content.width = content.right - content.left;
    const kindOf = (el) => {
      if (el.classList.contains("tableWrapper")) return "table";
      const inside = el.dataset.type ? el : el.querySelector(":scope > [data-type]");
      const type = inside?.dataset.type ?? el.tagName.toLowerCase();
      if (type === "pageList") return "pageList:" + (inside.dataset.layout ?? el.dataset.layout);
      return type;
    };
    const skip = ".ProseMirror-gapcursor, .ProseMirror-widget, .ProseMirror-separator, .ProseMirror-trailingBreak";
    const blocks = [...root.children]
      .filter((el) => !el.matches(skip) && el.getBoundingClientRect().height > 0)
      .map((el, index) => ({ index, type: kindOf(el), tag: el.tagName.toLowerCase(), text: (el.textContent || "").trim().slice(0, 24), ...rect(el) }));
    const tv = root.querySelector('[data-type="pageList"][data-layout="table"]');
    const gridEl = tv?.querySelector(".page-table-grid");
    const scrollEl = tv?.querySelector(".page-table-scroll");
    const headRow = tv?.querySelector(".page-table-head .page-table-row");
    const firstRow = tv?.querySelector(".page-table-body .page-table-row");
    const widths = (row) => (row ? [...row.children].map((c) => c.getBoundingClientRect().width) : []);
    const tableView = gridEl && scrollEl
      ? { grid: rect(gridEl), scroll: rect(scrollEl), head: headRow ? rect(headRow) : null, row: firstRow ? rect(firstRow) : null, headCells: widths(headRow), cells: widths(firstRow), clientWidth: scrollEl.clientWidth, scrollWidth: scrollEl.scrollWidth, rows: tv.querySelectorAll(".page-table-body .page-table-row").length }
      : null;
    const header = document.querySelector(".content-body > section.page > .page-header");
    const properties = document.querySelector(".content-body > section.page > .properties");
    const banner = document.querySelector(".content-body > .home-banners .banner");
    const nested = [...root.querySelectorAll('[data-type="column"] .tableWrapper')].map((t) => {
      const col = t.closest('[data-type="column"]');
      const c = rect(col);
      const ccs = getComputedStyle(col);
      return { table: rect(t), column: c, columnInner: { left: c.left + px(ccs.paddingLeft), right: c.right - px(ccs.paddingRight) } };
    });
    return {
      inner: { w: innerWidth, h: innerHeight, dpr: devicePixelRatio },
      fullWidthClass: section.classList.contains("is-full-width"),
      body: { ...rect(body), clientWidth: body.clientWidth, scrollWidth: body.scrollWidth, innerWidth: body.clientWidth - px(bodyCs.paddingLeft) - px(bodyCs.paddingRight) },
      docScrollOver: document.documentElement.scrollWidth - innerWidth,
      section: sr,
      content,
      header: header ? rect(header) : null,
      properties: properties ? rect(properties) : null,
      banner: banner ? rect(banner) : null,
      tableView,
      blocks,
      nested,
      columns: [...root.querySelectorAll('[data-type="column"]')].map(rect),
    };
  })()`);
}

/**
 * The layout against ADR-0035 (every page fills the main area): the section fills the main area, and the header,
 * the properties panel, the paragraphs, the heading and every top-level block span the section's content width
 * (the columns may keep small side margins: reported), the nested table is as wide as its column, the table
 * view's grid, header and rows span its block with the title column taking the extra width, the banner above
 * the page starts where the page's text does, and nothing scrolls sideways.
 */
function assertLayout(name, m, { tableNoScroll = false } = {}) {
  const tag = name;
  check(!m.fullWidthClass, `${tag}: section.page has no is-full-width class (there is no such switch any more)`);
  check(m.body.scrollWidth <= m.body.clientWidth + 1 && m.docScrollOver <= 1, `${tag}: no horizontal scroll (content-body ${m.body.scrollWidth} <= ${m.body.clientWidth}, document ${m.docScrollOver})`);
  check(near(m.section.width, m.body.innerWidth, 2), `${tag}: the page section fills the main area (${round(m.section.width)} wide, main area ${round(m.body.innerWidth)})`);
  const spans = (label, b, slack) => {
    check(
      near(b.left, m.content.left, slack) && near(b.right, m.content.right, slack),
      `${tag}: ${label} spans the section's content width (${round(b.left)} to ${round(b.right)}, ${round(b.width)} wide; section ${round(m.content.left)} to ${round(m.content.right)}, ${round(m.content.width)} wide)`,
    );
  };
  if (m.header !== null) {
    spans("the header", m.header, 2);
  } else {
    check(false, `${tag}: the page has a header`);
  }
  if (m.properties !== null) {
    spans("the properties panel", m.properties, 2);
  }
  if (m.banner !== null && m.header !== null) {
    check(near(m.banner.left, m.header.left, 4), `${tag}: the banner above the page starts where the page's text does (banner ${round(m.banner.left)}, page ${round(m.header.left)})`);
  }
  for (const b of m.blocks) {
    const label = `block ${b.index} ${b.type}${b.text === "" ? "" : ` "${b.text}"`}`;
    spans(label, b, b.tag === "p" || b.tag === "h1" || b.tag === "h2" || b.tag === "h3" ? 2 : 10);
    if (b.type === "columnList") {
      info(`${tag}: the column list's margins: ${round(b.left - m.content.left)} px on the left, ${round(b.right - m.content.right)} px on the right of the content box`);
    }
  }
  const textBlocks = m.blocks.filter((b) => b.tag === "p" || b.tag.startsWith("h"));
  info(`${tag}: window ${m.inner.w}x${m.inner.h}, content ${round(m.content.left)}..${round(m.content.right)} (${round(m.content.width)}), ${textBlocks.length} text blocks of ${m.blocks.length} blocks`);
  for (const n of m.nested) {
    check(
      near(n.table.width, n.columnInner.right - n.columnInner.left, 2) && n.table.left >= n.columnInner.left - 1 && n.table.right <= n.columnInner.right + 1,
      `${tag}: the table nested in a column is as wide as the column (table ${round(n.table.left)}..${round(n.table.right)} = ${round(n.table.width)}, column ${round(n.columnInner.left)}..${round(n.columnInner.right)})`,
    );
  }
  check(m.nested.length >= 1, `${tag}: a table is nested in a column (${m.nested.length})`);
  if (m.tableView === null) {
    check(false, `${tag}: the page has a table view`);
  } else {
    const { grid, scroll, head, row, headCells, cells, rows } = m.tableView;
    const rest = cells.slice(1).reduce((sum, w) => sum + w, 0);
    info(`${tag}: the table view's grid is ${round(grid.width)} wide in a block area of ${round(scroll.width)} (${rows} rows; title cell ${round(cells[0] ?? 0)}, other columns ${round(rest)})`);
    check(grid.width >= scroll.width - 2, `${tag}: the table view's grid spans its block (${round(grid.width)} wide in ${round(scroll.width)})`);
    check(head !== null && near(head.width, grid.width, 2), `${tag}: the table view's header row spans the grid (${round(head?.width ?? 0)} of ${round(grid.width)})`);
    check(row !== null && near(row.width, grid.width, 2), `${tag}: a table view row (its border and hover) spans the grid (${round(row?.width ?? 0)} of ${round(grid.width)})`);
    check(cells.length >= 1 && near(headCells[0] ?? 0, cells[0] ?? 0, 2) && cells[0] >= scroll.width - rest - 2, `${tag}: the title column takes the extra width (title ${round(cells[0] ?? 0)}, block ${round(scroll.width)} minus the other columns ${round(rest)})`);
    check(near(cells.reduce((sum, w) => sum + w, 0), grid.width, 2), `${tag}: the cells of a row add up to the grid's width (${round(cells.reduce((sum, w) => sum + w, 0))} of ${round(grid.width)})`);
    if (tableNoScroll) {
      // The title column spans the block exactly: nothing of the table may overflow it (a 2 px overflow makes a scrollbar of 15 px).
      check(m.tableView.scrollWidth <= m.tableView.clientWidth + 1, `${tag}: the table view does not scroll sideways when its columns fit (scrollWidth ${m.tableView.scrollWidth}, clientWidth ${m.tableView.clientWidth})`);
    }
  }
}

/** What a later measurement of the same window size must find again (the restart pass compares to it). */
function layoutSummary(m) {
  return {
    inner: m.inner,
    content: { left: m.content.left, right: m.content.right, width: m.content.width },
    header: m.header === null ? null : { left: m.header.left, width: m.header.width },
    blocks: m.blocks.map((b) => ({ type: b.type, left: b.left, width: b.width })),
    grid: m.tableView === null ? null : { width: m.tableView.grid.width, cells: m.tableView.cells },
    nested: m.nested.map((n) => ({ left: n.table.left, width: n.table.width })),
  };
}

// ---------------------------------------------------------------------------------------------------------------
// phases
// ---------------------------------------------------------------------------------------------------------------

async function phaseLaunch() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `width-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__widthMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href });
  const size = await page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio, theme: document.documentElement.dataset.theme ?? "(none)", dark: matchMedia("(prefers-color-scheme: dark)").matches })`);
  result.window = size;
  info(`window ${size.w}x${size.h} at ${size.dpr}x, data-theme ${size.theme}, system prefers dark: ${size.dark}`);
  const rect = win("info");
  result.frame = rect;
  info(`frame ${rect.width}x${rect.height} at ${rect.left},${rect.top}, maximised ${rect.zoomed}, monitor ${rect.monitor.width}x${rect.monitor.height} (work area ${rect.monitor.workWidth}x${rect.monitor.workHeight})`);
  check((await heading()) === "Smoke workspace", `the main area shows the workspace's Home (${await heading()})`);
}

/** A new empty line at the end of the page, the caret in it (what a click below the last block does). */
async function newLine() {
  await page.evaluate(`(() => {
    const editor = document.querySelector(".ProseMirror").editor;
    const { doc } = editor.state;
    const last = doc.lastChild;
    if (last && last.type.name === "paragraph" && last.content.size === 0) { editor.commands.focus("end"); return; }
    editor.chain().insertContentAt(doc.content.size, { type: "paragraph" }).focus("end").run();
  })()`);
  await wait(200);
}

/**
 * `/query` typed with key events, the entry named `label` chosen (ArrowDown until it is the selected one), Enter.
 * With `optional`, an entry the menu does not offer leaves the line as it was and returns false.
 */
async function slash(query, label, { optional = false } = {}) {
  await page.type(`/${query}`);
  await wait(500);
  await waitFor("[role=listbox]", `the slash menu for /${query}`);
  for (let i = 0; i < 20; i++) {
    const seen = await page.evaluate(`(() => {
      const options = [...document.querySelectorAll("[role=option]")];
      const title = (o) => o.querySelector(".suggestion-title")?.textContent.trim() ?? "";
      return { selected: title(options.find((o) => o.getAttribute("aria-selected") === "true") ?? options[0] ?? document.body), all: options.map(title) };
    })()`);
    if (!seen.all.includes(label)) {
      if (optional) {
        info(`the slash menu for /${query} does not offer "${label}" (${seen.all.join(", ")})`);
        await page.key("Escape", "Escape", 27);
        for (let n = 0; n <= query.length; n++) {
          await page.key("Backspace", "Backspace", 8);
        }
        return false;
      }
      must(false, `the slash menu for /${query} offers "${label}" (${seen.all.join(", ")})`);
    }
    if (seen.selected === label) {
      break;
    }
    await page.key("ArrowDown", "ArrowDown", 40);
    await wait(80);
  }
  await page.key("Enter", "Enter", 13);
  await wait(700);
  return true;
}

async function typeCells(cells) {
  for (let i = 0; i < cells.length; i++) {
    await page.type(cells[i]);
    if (i < cells.length - 1) {
      await page.key("Tab", "Tab", 9);
      await wait(60);
    }
  }
}

async function phasePage() {
  await clickOn(".tree-new", `the sidebar's "New page" button`);
  await waitFor("input.tree-rename", "the rename field of the new page");
  await send("Input.insertText", { text: PAGE_TITLE });
  await page.key("Enter", "Enter", 13);
  await waitFor(`[role="treeitem"][aria-label=${JSON.stringify(PAGE_TITLE)}]`, `the row "${PAGE_TITLE}"`);
  await wait(500);
  await waitFor(".ProseMirror", "the page editor");
  must((await heading()) === PAGE_TITLE, `the new page is open (${await heading()})`);

  // A paragraph, then a heading.
  await newLine();
  await send("Input.insertText", { text: "A text paragraph at the top of the page. It keeps a readable line length of about seventy-two characters however wide the window is, so a long sentence wraps here and the box of the paragraph shows the column." });
  await newLine();
  await slash("heading", "Heading 1");
  await send("Input.insertText", { text: "A heading in the text column" });

  // A simple table, three rows of three cells (the cell cursor is in the first one).
  await newLine();
  await slash("table", "Table");
  await typeCells(["Item", "Quantity", "Price", "Apples", "3", "1.20", "Pears", "5", "2.40"]);
  await wait(300);
  must((await box(".page-editor-content > .tableWrapper")) !== null, "a table is a top-level block of the page");

  // Two columns with a line of text each; a table inside the left one (the nested case).
  await newLine();
  await slash("columns", "2 columns");
  await page.type("Left column text");
  await page.key("Enter", "Enter", 13);
  await slash("table", "Table");
  await typeCells(["A", "B", "C", "1", "2", "3", "4", "5", "6"]);
  await wait(300);
  // The second column: scrolled into view, clicked in its empty line; the caret is checked to be there before typing.
  await waitFor('[data-type="column"]:nth-of-type(2)', "the second column");
  await page.evaluate(`document.querySelector('[data-type="column"]:nth-of-type(2)').scrollIntoView({ block: "center" })`);
  await wait(300);
  const inSecondColumn = () =>
    page.evaluate(`(() => {
      const { $from } = document.querySelector(".ProseMirror").editor.state.selection;
      for (let d = $from.depth; d > 0; d--) {
        if ($from.node(d).type.name === "column") return $from.index(d - 1) === 1;
      }
      return false;
    })()`);
  // (The table's toolbar floats over the top of the columns while the caret is in the nested table: the click goes low in the column.)
  for (const from of ["bottom", "middle", "top"]) {
    const second = await box('[data-type="column"]:nth-of-type(2)');
    await mouse(second.x + 24, from === "bottom" ? second.y + second.h - 12 : from === "middle" ? second.y + second.h / 2 : second.y + 16);
    if (await inSecondColumn()) {
      break;
    }
  }
  must(await inSecondColumn(), "the caret is in the second column");
  await page.type("Right column text");
  await wait(300);
  must((await box('[data-type="column"] .tableWrapper')) !== null, "a table is nested in the left column");

  // The other wide blocks.
  await newLine();
  const gallery = await slash("gallery", "Photo gallery", { optional: true });
  await newLine();
  await slash("whiteboard", "Whiteboard");
  await newLine();
  const tableView = await slash("table", "Table view", { optional: true });
  await newLine();
  const boardView = await slash("board", "Board view", { optional: true });

  // The closing paragraph.
  await newLine();
  await send("Input.insertText", { text: "A closing paragraph after the last wide block." });
  await wait(300);
  await page.evaluate(`(() => { document.activeElement?.blur?.(); })()`);
  await wait(2000); // the document is written a moment after the last key, and on blur

  // Pages inside "Wide" (the sidebar row's "Add a page inside"), so that the table view and the board show real rows.
  const children = ["Child one", "Child two", "Child three"];
  let made = 0;
  for (const title of children) {
    const row = await box(`[role="treeitem"][aria-label=${JSON.stringify(PAGE_TITLE)}] .tree-title`);
    if (row === null) break;
    await hover(row.cx, row.cy);
    const add = await box(`[role="treeitem"][aria-label=${JSON.stringify(PAGE_TITLE)}] button[aria-label="Add a page inside"]`);
    if (add === null || add.w === 0) break;
    await mouse(add.cx, add.cy);
    const rename = await box("input.tree-rename");
    if (rename === null) break;
    await send("Input.insertText", { text: title });
    await page.key("Enter", "Enter", 13);
    await wait(700);
    made += 1;
  }
  info(`${made} of ${children.length} pages made inside "${PAGE_TITLE}"`);
  check(made === children.length, `the table view has ${children.length} pages to list (${made} made)`);
  await openWide();
  await wait(800);

  const m = await measure();
  must(m !== null, "the page can be measured");
  result.blocks = m.blocks.map((b) => `${b.type}${b.text === "" ? "" : `:${b.text}`}`);
  info(`top-level blocks: ${result.blocks.join(" | ")}`);
  const types = m.blocks.map((b) => b.type);
  check(types.filter((t) => t === "p").length >= 2, `the page has its paragraphs (${types.filter((t) => t === "p").length})`);
  check(types.some((t) => t.startsWith("h")), "the page has a heading");
  check(types.includes("table"), "the page has a top-level table");
  check(types.includes("columnList"), "the page has a column list");
  check(types.includes("whiteboard"), "the page has a whiteboard");
  if (gallery) check(types.includes("gallery"), "the page has a gallery");
  if (tableView) check(types.includes("pageList:table"), "the page has a table view");
  if (boardView) check(types.includes("pageList:board"), "the page has a board view");
  result.offered = { gallery, tableView, boardView };
  check(m.nested.length === 1, `one table is nested in a column (${m.nested.length})`);
  const columnTexts = await page.evaluate(`[...document.querySelectorAll('.ProseMirror [data-type="column"]')].map((c) => c.firstElementChild?.textContent.trim() ?? "")`);
  check(columnTexts[0] === "Left column text" && columnTexts[1] === "Right column text", `each column holds its line of text (${columnTexts.join(" | ")})`);
  const cells = await page.evaluate(`[...document.querySelectorAll(".ProseMirror td, .ProseMirror th")].map((c) => c.textContent.trim()).join(",")`);
  check(cells === "Item,Quantity,Price,Apples,3,1.20,Pears,5,2.40,A,B,C,1,2,3,4,5,6", `the cells hold what was typed (${cells})`);
}

/** The sizes of the pass: the webview's size, or the window's state. */
const SIZES = [
  { name: "1100", label: "1100x760", w: 1100, h: 760, capture: "1100" },
  { name: "1500", label: "1500x900", w: 1500, h: 900 },
  { name: "maximised", label: "maximised", maximise: true, capture: "maximised", lower: "maximised-lower" },
  { name: "restored", label: "restored", restore: true },
  { name: "800", label: "800x600", w: 800, h: 600, capture: "800" },
];

async function phaseSizes() {
  await openWide();
  const measured = {};
  for (const size of SIZES) {
    let rect;
    if (size.maximise) {
      rect = win("maximise");
      await settle();
    } else if (size.restore) {
      rect = win("restore");
      await settle();
    } else {
      ({ rect } = await setInner(size.w, size.h));
    }
    rect = win("info");
    await scrollTo(0);
    const m = await measure();
    must(m !== null, `${size.label}: the page can be measured`);
    measured[size.name] = m;
    info(`${size.label}: frame ${rect.width}x${rect.height}, maximised ${rect.zoomed}, webview ${m.inner.w}x${m.inner.h}`);
    if (size.w !== undefined) {
      check(near(m.inner.w, size.w, 1) && near(m.inner.h, size.h, 1), `${size.label}: the webview is ${size.w}x${size.h} (${m.inner.w}x${m.inner.h})`);
    }
    if (size.maximise) {
      check(rect.zoomed === true, `maximised: the window is maximised (IsZoomed ${rect.zoomed})`);
      check(m.inner.w >= rect.monitor.width - 40, `maximised: the webview fills the screen's width (${m.inner.w} of ${rect.monitor.width})`);
      check(m.content.width > OLD_COLUMN + 300, `maximised: the section is much wider than the old 720 px column (${round(m.content.width)} > ${OLD_COLUMN})`);
    }
    if (size.restore) {
      check(rect.zoomed === false, `restored: the window is not maximised (IsZoomed ${rect.zoomed})`);
      // A restore goes back to the last normal size: the 1500x900 one, set just before the window was maximised.
      const first = measured["1500"];
      check(near(m.inner.w, first.inner.w, 2) && near(m.content.width, first.content.width, 2), `restored: the layout is back to the 1500x900 one (content ${round(m.content.width)} against ${round(first.content.width)}, webview ${m.inner.w} against ${first.inner.w})`);
      const stale = m.blocks.filter((b, i) => first.blocks[i] === undefined || !near(b.width, first.blocks[i].width, 2));
      check(stale.length === 0, `restored: every block has the width it had before the window was maximised (${stale.map((b) => `${b.type} ${round(b.width)}`).join(", ") || "all equal"})`);
    }
    assertLayout(size.label, m, { tableNoScroll: size.name === "1100" || size.name === "maximised" });
    const firstPara = m.blocks.find((b) => b.tag === "p");
    if (size.name === "800") {
      check(m.content.width < OLD_COLUMN, `800x600: the section is narrower than the old column, so everything fits the window (${round(m.content.width)} < ${OLD_COLUMN})`);
    }
    if (size.name === "1500" || size.name === "maximised" || size.name === "restored") {
      check(firstPara !== undefined && firstPara.width > OLD_COLUMN + 100, `${size.label}: a paragraph is wider than the old 720 px column (${round(firstPara?.width ?? 0)} > ${OLD_COLUMN})`);
    }
    if (size.capture) {
      capture(size.capture);
    }
    if (size.lower) {
      const body = await box(".content-body");
      const target = await page.evaluate(`(() => {
        const el = document.querySelector('.content-body > section.page [data-type="pageList"][data-layout="table"]');
        const b = document.querySelector(".content-body");
        if (!el) return 0;
        // The table view, the board view and the closing paragraph under it, and the whiteboard's lower part above it.
        return Math.max(0, el.getBoundingClientRect().top - b.getBoundingClientRect().top + b.scrollTop - 300);
      })()`);
      info(`lower capture scrolls to ${Math.round(target)} (main area ${Math.round(body.h)} tall)`);
      await scrollTo(target);
      capture(size.lower);
      await scrollTo(0);
    }
  }
  result.measured = measured;

  // A real full screen? The shell is a Tauri window: F11 is probed with a posted key (the DevTools port's keys go to the page).
  win("maximise");
  await settle();
  const before = win("info");
  const probe = win("f11");
  await wait(800);
  const after = win("info");
  const fullscreenElement = await page.evaluate(`document.fullscreenElement?.tagName ?? null`);
  const changed = after.caption !== before.caption || after.width !== before.width || after.height !== before.height || after.left !== before.left || after.top !== before.top;
  info(`F11 probe: ${changed || fullscreenElement !== null ? "THE WINDOW CHANGED" : "nothing happened"} (frame ${before.width}x${before.height} caption ${before.caption} -> ${probe.width}x${probe.height} caption ${probe.caption} -> ${after.width}x${after.height} caption ${after.caption}; document.fullscreenElement ${fullscreenElement})`);
  result.f11 = { before, after, fullscreenElement, changed };
  if (changed) {
    win("f11"); // leave it as it was
    await wait(800);
    await settle();
  }
  // Left as the next phases expect it: the webview at 1100x760.
  await setInner(1100, 760);
}

/** The handle's box, or null while none is shown. */
async function handleState() {
  return page.evaluate(`(() => {
    const h = document.querySelector(".block-handle");
    if (!h) return null;
    const r = h.getBoundingClientRect();
    const cs = getComputedStyle(h);
    const visible = cs.display !== "none" && cs.visibility !== "hidden" && Number(cs.opacity) > 0 && r.width > 0 && r.height > 0 && r.right > 0 && r.left < innerWidth;
    return { left: r.left, right: r.right, top: r.top, width: r.width, height: r.height, visible };
  })()`);
}

async function phaseHandle() {
  await openWide();
  win("maximise");
  await settle();
  await scrollTo(0);
  const side = (await box(".sidebar .sidebar-header")) ?? (await box(".sidebar"));
  must(side !== null, "the sidebar is on screen");
  const m = await measure();
  must(m !== null, "the page can be measured");
  const para = m.blocks.find((b) => b.tag === "p" && b.text.startsWith("A text paragraph"));
  const head = m.blocks.find((b) => b.tag === "h1");
  const table = m.blocks.find((b) => b.type === "table");
  const columns = m.blocks.find((b) => b.type === "columnList");
  const tableView = m.blocks.find((b) => b.type === "pageList:table");
  must(para !== undefined && head !== undefined && table !== undefined && columns !== undefined, "the paragraph, the heading, the table and the columns are there");

  /** The handle with the pointer at (x, y): it is on screen, left of `block`'s left edge (within 60 px), at its height. */
  const handleAt = async (what, x, y, block) => {
    await hover(side.cx, side.cy); // away first: the handle is for the block under the pointer
    await hover(x, y);
    const under = await page.evaluate(`(() => { const el = document.elementFromPoint(${x}, ${y}); return el === null ? "(nothing)" : el.tagName.toLowerCase() + (el.className && typeof el.className === "string" ? "." + el.className.split(" ")[0] : ""); })()`);
    const h = await handleState();
    if (!check(h !== null && h.visible, `${what}: the block handle is visible (pointer over ${under})`)) {
      return null;
    }
    // A block may start left of the content box (the columns' outlines hang over it): the handle then overlaps that margin.
    const overhang = Math.max(0, m.content.left - block.left);
    const gap = block.left - h.right;
    check(gap >= -2 - overhang && block.left - h.left <= 60 + overhang, `${what}: the handle sits left of the block (handle ${round(h.left)}..${round(h.right)}, block starts at ${round(block.left)}, gap ${round(gap)}${overhang > 0 ? `, the block hangs ${round(overhang)} px over the content edge` : ""})`);
    check(Math.abs(h.top - block.top) <= 16, `${what}: the handle is at the block's height (handle top ${round(h.top)}, block top ${round(block.top)})`);
    return h;
  };

  /** Scrolls the page so that `block` (measured at the top of the page) is in view, and returns its box there. */
  const reveal = async (block) => {
    if (block.bottom < m.body.bottom - 40) {
      return block;
    }
    await scrollTo(Math.max(0, block.top - m.body.top - 250));
    const again = await measure();
    return again.blocks[block.index];
  };

  const forParagraph = await handleAt("over a paragraph", para.left + 60, para.top + para.height / 2, para);
  await handleAt("over the heading", head.left + 40, head.top + head.height / 2, head);
  const forTable = await handleAt("over the table", table.left + 200, table.top + table.height / 2, table);
  if (forParagraph && forTable) {
    check(near(forParagraph.left, forTable.left, 8), `the handle is at the same x for a paragraph and the table, they start together (${round(forParagraph.left)} and ${round(forTable.left)})`);
  }
  await handleAt("over the columns", columns.left + 40, columns.top + 20, columns);
  if (tableView) {
    const shown = await reveal(tableView);
    await handleAt("over the table view", shown.left + 60, shown.top + 20, shown);
    await scrollTo(0);
  }
  // The capture: the pointer over the paragraph, the handle beside the text.
  await hover(side.cx, side.cy);
  await hover(para.left + 60, para.top + para.height / 2);
  capture("handle");
}

/** Three clicks in a row at a point (selects the paragraph). */
async function tripleClick(x, y) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  for (const n of [1, 2, 3]) {
    await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: n });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: n });
  }
  await wait(400);
}

const within = (b, outer, slack) => b.x >= outer.left - slack && b.x + b.w <= outer.right + slack;

/** The floating menus follow the blocks: the format menu, the table and columns toolbars and the slash menu, on the maximised page. */
async function phaseFloating() {
  await openWide();
  win("maximise");
  await settle();
  await scrollTo(0);
  let m = await measure();
  must(m !== null, "the page can be measured");
  const para = m.blocks.find((b) => b.tag === "p" && b.text.startsWith("A text paragraph"));
  const table = m.blocks.find((b) => b.type === "table");
  const columns = m.blocks.find((b) => b.type === "columnList");
  must(para !== undefined && table !== undefined && columns !== undefined, "the paragraph, the table and the columns are there");

  // The format menu over a selected paragraph in the centred column.
  await tripleClick(para.left + 80, para.top + 12);
  const format = await waitFor(".format-menu", "the format menu over the selected paragraph");
  info(`format menu ${round(format.x)}..${round(format.x + format.w)} x ${round(format.y)}..${round(format.y + format.h)} over the paragraph ${round(para.left)}..${round(para.right)} x ${round(para.top)}..${round(para.bottom)}`);
  check(format.x + format.w > para.left && format.x < para.right, "the format menu is over the selected paragraph, not over empty space beside it");
  check(format.y + format.h >= para.top - 150 && format.y <= para.bottom + 150, "the format menu is next to the selected paragraph");

  // The table toolbar over the wide table (a click in a cell).
  const cell = await waitFor(".ProseMirror > .tableWrapper td", "a cell of the wide table");
  await mouse(cell.cx, cell.cy);
  const tableMenu = await waitFor(".table-menu", "the table toolbar");
  m = await measure();
  const tableBlock = m.blocks.find((b) => b.type === "table");
  info(`table toolbar ${round(tableMenu.x)}..${round(tableMenu.x + tableMenu.w)} x ${round(tableMenu.y)}..${round(tableMenu.y + tableMenu.h)}, table ${round(tableBlock.left)}..${round(tableBlock.right)} x ${round(tableBlock.top)}..${round(tableBlock.bottom)}`);
  check(within(tableMenu, tableBlock, 4), "the table toolbar is within the wide table's width");
  check(tableMenu.y + tableMenu.h >= tableBlock.top - 120 && tableMenu.y <= tableBlock.bottom + 120, "the table toolbar is next to the table");

  // The columns toolbar over the column list (a click in the right column).
  const right = await waitFor('.ProseMirror [data-type="column"]:nth-of-type(2) p', "the right column's line");
  await mouse(right.x + 20, right.cy);
  const columnsMenu = await waitFor(".columns-menu", "the columns toolbar");
  m = await measure();
  const columnList = m.blocks.find((b) => b.type === "columnList");
  info(`columns toolbar ${round(columnsMenu.x)}..${round(columnsMenu.x + columnsMenu.w)} x ${round(columnsMenu.y)}..${round(columnsMenu.y + columnsMenu.h)}, columns ${round(columnList.left)}..${round(columnList.right)}`);
  check(within(columnsMenu, columnList, 4), "the columns toolbar is within the column list's width");

  // The slash menu at the end of the closing paragraph: a new line, "/", the menu at the caret.
  await page.evaluate(`(() => { const els = [...document.querySelectorAll(".ProseMirror > p")]; els[els.length - 1].scrollIntoView({ block: "center" }); })()`);
  await wait(300);
  m = await measure();
  const closing = m.blocks.filter((b) => b.tag === "p").at(-1);
  await mouse(closing.left + 10, closing.top + closing.height / 2);
  await page.key("End", "End", 35);
  await page.key("Enter", "Enter", 13);
  await page.type("/");
  const slashMenu = await waitFor(".slash-menu", "the slash menu");
  const caret = await page.evaluate(`(() => { const e = document.querySelector(".ProseMirror").editor; const c = e.view.coordsAtPos(e.state.selection.from); return { left: c.left, top: c.top, bottom: c.bottom }; })()`);
  info(`slash menu ${round(slashMenu.x)}..${round(slashMenu.x + slashMenu.w)} x ${round(slashMenu.y)}..${round(slashMenu.y + slashMenu.h)}, caret at ${round(caret.left)},${round(caret.top)}..${round(caret.bottom)}, centred text starts at ${round(closing.left)}`);
  check(slashMenu.x >= caret.left - 80 && slashMenu.x <= caret.left + 20, `the slash menu starts at the caret (menu left ${round(slashMenu.x)}, caret ${round(caret.left)})`);
  check(
    (slashMenu.y >= caret.bottom - 6 && slashMenu.y <= caret.bottom + 60) || (slashMenu.y + slashMenu.h <= caret.top + 6 && slashMenu.y + slashMenu.h >= caret.top - 60),
    `the slash menu is just below (or above) the caret's line (menu ${round(slashMenu.y)}..${round(slashMenu.y + slashMenu.h)}, caret ${round(caret.top)}..${round(caret.bottom)})`,
  );
  capture("floating");
  // The page as it was: the menu closed, the "/" and the new line removed.
  await page.key("Escape", "Escape", 27);
  await page.key("Backspace", "Backspace", 8);
  await page.key("Backspace", "Backspace", 8);
  await wait(400);
  const tail = await page.evaluate(`(() => { const els = [...document.querySelectorAll(".ProseMirror > p")]; return { count: els.length, last: els[els.length - 1].textContent }; })()`);
  check(tail.last === "A closing paragraph after the last wide block.", `the closing paragraph is as it was (${tail.last})`);
  await wait(1500);
}

async function chooseTheme(label) {
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor("input[name=theme]", "the theme choices");
  await clickOn(".radio", `the "${label}" theme`, { text: label });
  await wait(500);
}

async function phaseDark() {
  await openWide();
  win("maximise");
  await settle();
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor("input[name=theme]", "the theme choices");
  const original = await page.evaluate(`document.querySelector("input[name=theme]:checked")?.value ?? null`);
  info(`theme choice at the start: ${original}`);
  const labelOf = { light: "Light", dark: "Dark", system: "Match system" };
  try {
    await chooseTheme("Dark");
    const applied = await page.evaluate(`document.documentElement.dataset.theme ?? "(none)"`);
    check(applied === "dark", `data-theme is dark (${applied})`);
    await openWide();
    const m = await measure();
    must(m !== null, "the page can be measured in the dark theme");
    assertLayout("maximised, dark theme", m);
    const bg = await page.evaluate(`getComputedStyle(document.body).backgroundColor`);
    info(`dark theme body background ${bg}`);
    capture("dark");
  } finally {
    await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
    await waitFor("input[name=theme]", "the theme choices");
    await clickOn(".radio", `the "${labelOf[original] ?? "Match system"}" theme`, { text: labelOf[original] ?? "Match system" });
    await wait(400);
    await openWide();
    const restored = await page.evaluate(`document.documentElement.dataset.theme ?? "(none)"`);
    check(original === "dark" || restored !== "dark", `the theme is back to the choice the pass started with (${original}; data-theme ${restored})`);
  }
}

/** The Home screen and the banner above a page start where the page's text does (the same x, +-4 px), not centred. */
async function phaseHome() {
  const sizes = [
    { label: "maximised", maximise: true },
    { label: "1100x760", w: 1100, h: 760 },
  ];
  await openWide();
  for (const size of sizes) {
    if (size.maximise) {
      win("maximise");
      await settle();
    } else {
      await setInner(size.w, size.h);
    }
    await openWide();
    await scrollTo(0);
    const m = await measure();
    must(m !== null && m.header !== null, `${size.label}: the page can be measured`);
    // The banner above the page (there is no recovery kit yet).
    must(m.banner !== null, `${size.label}: the recovery-kit banner is above the page`);
    check(near(m.banner.left, m.header.left, 4), `${size.label}: the recovery-kit banner starts where the page's text does (banner ${round(m.banner.left)}, page header ${round(m.header.left)})`);
    const firstText = m.blocks.find((b) => b.tag === "p");
    if (firstText) {
      check(near(m.banner.left, firstText.left, 4), `${size.label}: the banner and the first paragraph start at the same x (${round(m.banner.left)}, ${round(firstText.left)})`);
    }
    info(`${size.label}: banner ${round(m.banner.left)}..${round(m.banner.right)} (${round(m.banner.width)} wide); the page's content box ${round(m.content.left)}..${round(m.content.right)} (${round(m.content.width)} wide)`);
    // The Home screen: a new tab (the strip's "New tab" button) opens one.
    await clickOn('.tab-strip button[aria-label="New tab"]', "the New tab button");
    await waitFor("section.home", "the Home screen of the new tab");
    await wait(400);
    const home = await page.evaluate(`(() => {
      const rect = (el) => { if (!el) return null; const b = el.getBoundingClientRect(); return { left: b.left, right: b.right, top: b.top, width: b.width }; };
      const body = document.querySelector(".content-body");
      return {
        body: rect(body),
        banner: rect(document.querySelector(".content-body > .home-banners .banner")),
        section: rect(document.querySelector(".content-body > section.home")),
        title: rect(document.querySelector(".content-body > section.home h1")),
        lead: rect(document.querySelector(".content-body > section.home .lead")),
        scrollOver: body.scrollWidth - body.clientWidth,
      };
    })()`);
    must(home.title !== null, `${size.label}: the Home screen shows its title`);
    check(near(home.title.left, m.content.left, 4), `${size.label}: the Home screen starts where the page's text does (title ${round(home.title.left)}, page ${round(m.content.left)})`);
    if (home.lead !== null) {
      check(near(home.lead.left, m.content.left, 4), `${size.label}: the Home screen's text starts there too (${round(home.lead.left)})`);
    }
    if (home.banner !== null) {
      check(near(home.banner.left, m.content.left, 4), `${size.label}: the banner on the Home screen starts there too (${round(home.banner.left)})`);
    }
    check(home.title.left - home.body.left < 140, `${size.label}: the Home screen is at the left edge of the main area, not centred (${round(home.title.left - home.body.left)} px from it)`);
    check(home.scrollOver <= 1, `${size.label}: the Home screen does not scroll sideways (${home.scrollOver})`);
    info(`${size.label}: Home section ${round(home.section.left)}..${round(home.section.right)} (${round(home.section.width)} wide), banner ${home.banner === null ? "none" : `${round(home.banner.left)}..${round(home.banner.right)} (${round(home.banner.width)} wide)`}`);
    if (size.maximise) {
      capture("home");
    }
    await clickOn(".tab-strip .tab.is-active .tab-close", "the close button of the Home tab");
    await wait(400);
  }
  await openWide();
}

/** The page's ⋯ menu has no "Full width" item (there is no per-page width any more), and the layout it leaves is recorded for the restart. */
async function phaseMenu() {
  await openWide();
  win("maximise");
  await settle();
  await scrollTo(0);
  await clickOn('button[aria-label="Page options"]', "the page options button");
  await waitFor('[role="menu"]', "the page options menu");
  const items = await page.evaluate(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map((i) => ({ role: i.getAttribute("role"), text: i.textContent.trim(), checked: i.getAttribute("aria-checked") }))`);
  result.menu = items;
  info(`page options: ${items.map((i) => `${i.text} (${i.role})`).join("; ")}`);
  check(items.length >= 2, `the page options menu has its items (${items.map((i) => i.text).join(", ")})`);
  check(!items.some((i) => /width/i.test(i.text)), `the page options menu has no "Full width" item (${items.map((i) => i.text).join(", ")})`);
  check(items.every((i) => i.role === "menuitem" && i.checked === null), "every item of the page options menu is a plain command (no switch)");
  capture("menu");
  await page.key("Escape", "Escape", 27);
  await wait(300);
  check((await box('[role="menu"]')) === null, "Escape closed the menu");
  await scrollTo(0);
  const m = await measure();
  must(m !== null, "the page can be measured");
  assertLayout("maximised", m);
  writeState({ layout: layoutSummary(m) });
}

async function phaseRestart() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `width-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__widthMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href });
  const before = readState().layout;
  must(before !== undefined, "the layout before the restart was recorded (the menu phase)");
  await wait(1000);
  await openWide();
  win("maximise");
  await settle();
  await scrollTo(0);
  const m = await measure();
  must(m !== null, "the page can be measured after the restart");
  assertLayout("maximised after the restart", m);
  const now = layoutSummary(m);
  check(now.inner.w === before.inner.w, `the window is the same size as before the restart (${now.inner.w} against ${before.inner.w})`);
  check(near(now.content.width, before.content.width, 2), `the content box is as wide as before the restart (${round(now.content.width)} against ${round(before.content.width)})`);
  const differing = now.blocks
    .map((b, i) => ({ b, was: before.blocks[i] }))
    .filter(({ b, was }) => was === undefined || b.type !== was.type || !near(b.left, was.left, 2) || !near(b.width, was.width, 2));
  check(now.blocks.length === before.blocks.length && differing.length === 0, `every block has the box it had before the restart (${now.blocks.length} blocks${differing.length > 0 ? `; differing: ${differing.map(({ b }) => b.type).join(", ")}` : ""})`);
  check(now.header !== null && before.header !== null && near(now.header.width, before.header.width, 2), `the header is as wide as before the restart (${round(now.header?.width ?? 0)} against ${round(before.header?.width ?? 0)})`);
  check(now.grid !== null && before.grid !== null && near(now.grid.width, before.grid.width, 2), `the table view's grid is as wide as before the restart (${round(now.grid?.width ?? 0)} against ${round(before.grid?.width ?? 0)})`);
  await clickOn('button[aria-label="Page options"]', "the page options button");
  await waitFor('[role="menu"]', "the page options menu");
  const items = await page.evaluate(`[...document.querySelectorAll('[role="menu"] [role^="menuitem"]')].map((i) => i.textContent.trim())`);
  check(!items.some((t) => /width/i.test(t)), `the page options menu has no "Full width" item after the restart (${items.join(", ")})`);
  await page.key("Escape", "Escape", 27);
  await wait(300);
  capture("restarted");
}

/** The webview's own document: nothing may have replaced it. */
async function intact(when) {
  const state = readState();
  const now = await page.evaluate(`(() => ({ marker: window.__widthMarker ?? null, href: location.href }))()`).catch(() => null);
  must(now !== null, `${when}: the window answers`);
  check(now.marker === state.marker && state.marker !== undefined, `${when}: the document was not replaced (marker ${now.marker === state.marker ? "kept" : "LOST: " + now.marker})`);
  check(now.href === state.href, `${when}: the address is unchanged (${now.href})`);
}

const PHASE_FUNCS = {
  launch: phaseLaunch,
  page: phasePage,
  sizes: phaseSizes,
  handle: phaseHandle,
  floating: phaseFloating,
  home: phaseHome,
  dark: phaseDark,
  menu: phaseMenu,
  restart: phaseRestart,
};

try {
  const run = PHASE_FUNCS[phase];
  if (run === undefined) {
    throw new Error(`unknown phase ${phase}`);
  }
  await run();
} catch (error) {
  if (!(error instanceof PhaseStop)) {
    check(false, `exception: ${error instanceof Error ? error.message : String(error)}`);
  }
}

try {
  await intact(`end of ${phase}`);
} catch (error) {
  if (!(error instanceof PhaseStop)) {
    check(false, `exception at the end: ${error instanceof Error ? error.message : String(error)}`);
  }
}

await wait(500);
const report = page.report();
const violations = await page.evaluate("window.__cspViolations ?? []").catch(() => []);
// The origins the app itself talks to: its own pages and assets, the IPC bridge, the media scheme.
const ALLOWED = /^(?:https?:\/\/(?:tauri\.localhost|ipc\.localhost|kbmedia\.localhost)|tauri:\/\/localhost|ipc:\/\/localhost|kbmedia:\/\/localhost|data:|blob:|about:)/u;
const foreign = report.requests.filter((r) => !ALLOWED.test(r.url));
result.requests = report.requests.length;
result.foreignRequests = foreign;
result.failures = report.failures;
result.cspViolations = violations;
result.console = report.console.filter((c) => c.type !== "log" && c.type !== "info" && c.type !== "debug");
check(foreign.length === 0, `no request to another host (${foreign.length})`);
check(violations.length === 0, `no content-security-policy violation (${violations.length})`);
check(report.failures.length === 0, `no failed request (${report.failures.length})`);
const errors = report.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
check(errors.length === 0, `no console error (${errors.length}${errors.length > 0 ? ": " + errors.map((e) => e.text).slice(0, 3).join(" | ") : ""})`);
if (outFile) {
  writeFileSync(outFile, JSON.stringify(result, null, 2));
}
// The report file has every note; the console has what failed (or, with WIDTH_VERBOSE, everything).
const checks = result.notes.filter((n) => !n.startsWith("info:"));
console.log(
  JSON.stringify(
    {
      phase,
      ok: result.ok,
      checks: checks.length,
      failed: checks.filter((n) => n.startsWith("FAIL")),
      captures: result.captures,
      ...(process.env.WIDTH_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
page.close();
process.exit(result.ok ? 0 : 1);
