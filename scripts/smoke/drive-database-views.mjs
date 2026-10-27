// The in-page half of drive-database-views.ps1: database views (ADR-0054) in the dev app, over the WebView2 DevTools port
// (cdp.mjs: trusted mouse and key events, DOM reads). The native "Open" dialog of the image import is driven by the .ps1's
// "file-dialog" helper (Win32 messages, drive-m4.ps1's pattern). Never SendKeys. A `<select>` and a date `<input>` have no
// popup CDP can drive, so their value is set with the native setter plus the input and change events (what React listens to).
//
//   node drive-database-views.mjs <build|verify> <report.json> <png path>
//
// build:  schema (Due, Until, Status) through the core, the Projects page with a table view, five children created with the
//         view's "New page", dates and statuses through the core (a reload picks them up), an image block (native dialog) and a
//         colour cover, the Calendar view (Due / Until, a three-day span, "Change date...", the "No date" tray), the Gallery
//         view (first-image preview, cover, icons, size L), the peek (side, rename, Esc, center, Open as page) and the filter
//         group "Status has Done OR (Due is before D AND Status has In progress)".
// verify: after a restart on the same data directory: the tabs, the active tab and the filter group survive.
//
// Writes the report (every check: id, ok, detail) and out\dbv-*.png; exits 1 when a check failed.

import { execFile } from "node:child_process";
import { deflateSync } from "node:zlib";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { connect } from "./cdp.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "out");
mkdirSync(out, { recursive: true });
const phase = process.argv[2] ?? "build";
const reportPath = process.argv[3] ?? join(out, `dbv-report-${phase}.json`);
const pngPath = process.argv[4] ?? join(out, "dbv-image.png");
const ps1 = join(here, "drive-database-views.ps1");

const report = { startedAt: new Date().toISOString(), phase, checks: [], notes: {} };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const t0 = Date.now();
const stamp = () => `${((Date.now() - t0) / 1000).toFixed(1)}s`;
function check(id, ok, detail) {
  report.checks.push({ id, ok: Boolean(ok), detail, at: stamp() });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} [${stamp()}] ${typeof detail === "string" ? detail : JSON.stringify(detail)}`);
}
function note(key, value) {
  report.notes[key] = value;
  console.log(`note ${key}: ${typeof value === "string" ? value : JSON.stringify(value)}`);
}
function flush() {
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
}

/** A helper step of the .ps1 (reads a JSON argument file, writes a JSON result file). */
function helper(step, args) {
  const argFile = join(out, `dbv-${step}-arg.json`);
  const resFile = join(out, `dbv-${step}-res.json`);
  writeFileSync(argFile, JSON.stringify(args));
  rmSync(resFile, { force: true });
  return new Promise((resolve) => {
    execFile("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps1, "-Step", step, "-Arg", argFile, "-Result", resFile], { windowsHide: true }, () => {
      try {
        resolve(JSON.parse(readFileSync(resFile, "utf8")));
      } catch {
        resolve({ ok: false, error: "no result file" });
      }
    });
  });
}
const shot = (name) => helper("capture-main", { path: join(out, `${name}.png`) });

const cdp = await connect();
const ev = (expression) => cdp.evaluate(expression);

// ---- DOM and input helpers --------------------------------------------------------------------------------------------
/** The centre of the first element (by selector, optionally filtered by text or aria-label) that is on screen. */
async function where(selector, text = null) {
  return ev(`(() => {
    const want = ${JSON.stringify(text)};
    const list = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const el = list.find((e) => {
      const r = e.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return false;
      if (want === null) return true;
      const s = (e.getAttribute("aria-label") ?? "") + "|" + (e.textContent ?? "");
      return s.includes(want);
    });
    if (!el) return null;
    el.scrollIntoView({ block: "nearest" });
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
}
async function waitWhere(selector, text = null, ms = 15000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const p = await where(selector, text);
    if (p) return p;
    await sleep(200);
  }
  throw new Error(`not found: ${selector} ${text ?? ""}`);
}
async function clickOn(selector, text = null, button = "left") {
  const p = await waitWhere(selector, text);
  await cdp.click(p.x, p.y, button);
  await sleep(300);
}
/** Waits until a JS expression is truthy. */
async function until(expression, label, ms = 15000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      last = await ev(expression);
      if (last) return last;
    } catch (e) {
      last = String(e);
    }
    await sleep(200);
  }
  throw new Error(`timed out waiting for ${label} (${String(last).slice(0, 200)})`);
}
async function press(key, code, vk, modifiers = 0, text = undefined, commands = undefined) {
  await cdp.send("Input.dispatchKeyEvent", { type: text === undefined ? "rawKeyDown" : "keyDown", key, code, windowsVirtualKeyCode: vk, modifiers, ...(text === undefined ? {} : { text }), ...(commands === undefined ? {} : { commands }) });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(150);
}
const enter = () => press("Enter", "Enter", 13, 0, "\r");
const escape = () => press("Escape", "Escape", 27);
async function typeText(text) {
  await cdp.send("Input.insertText", { text });
  await sleep(150);
}
/** Sets a `<select>` (by aria-label, the n-th match inside a scope) to the option with this text or value; fires change. */
async function setSelect(scope, label, optionMatch, n = 0) {
  const r = await ev(`(() => {
    const root = ${scope === null ? "document" : `document.querySelector(${JSON.stringify(scope)})`};
    if (!root) return "no scope";
    const sels = [...root.querySelectorAll("select")].filter((s) => (s.getAttribute("aria-label") === ${JSON.stringify(label)} || s.closest("label")?.querySelector(".page-list-field-label")?.textContent === ${JSON.stringify(label)}) && s.getBoundingClientRect().width > 0);
    const sel = sels[${n}];
    if (!sel) return "no select (" + sels.length + ")";
    const want = ${JSON.stringify(optionMatch)};
    const opt = [...sel.options].find((o) => o.value === want || o.textContent.trim() === want);
    if (!opt) return "no option; have " + [...sel.options].map((o) => o.textContent.trim() + "=" + o.value).join(", ");
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(sel, opt.value);
    sel.dispatchEvent(new Event("input", { bubbles: true }));
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    return "ok";
  })()`);
  if (r !== "ok") throw new Error(`setSelect ${label}[${n}] -> ${optionMatch}: ${r}`);
  await sleep(350);
}
/**
 * Sets a select of the toolbar's view settings menu (the layout, the source, the layout's own settings, "Open pages in"),
 * opening the menu first when it is not open (the sliders button at the end of the bar).
 */
async function setViewSetting(label, optionMatch) {
  await openSettings();
  await setSelect(".page-list-settings", label, optionMatch);
  // The menu is a fixed-position popover: left open it stays where it was while the page scrolls, and the press that closes it
  // (outside) hands focus back to the sliders button, which scrolls and loses the click. So every setting closes it again.
  await closeSettings();
}
async function openSettings() {
  if (!(await ev(`!!document.querySelector(".page-list-settings")`))) {
    await clickOn(".page-list-toolbar button.page-list-settings-button");
    await until(`!!document.querySelector(".page-list-settings")`, "the view settings menu");
  }
}
async function closeSettings() {
  if (await ev(`!!document.querySelector(".page-list-settings")`)) {
    await escape();
    await until(`!document.querySelector(".page-list-settings")`, "the view settings menu closed", 5000).catch(() => {});
  }
}
/** Scrolls the view's block to the top of the window, so a shot shows its tabs and its toolbar (focus is on the sliders button, which shows the bar). */
async function showBlockTop() {
  await ev(`document.querySelector(".page-list")?.scrollIntoView({ block: "start" })`);
  await sleep(300);
}
/** Sets an `<input>` (by aria-label) with the native setter and the input event. */
async function setInput(scope, label, value, n = 0) {
  const r = await ev(`(() => {
    const root = ${scope === null ? "document" : `document.querySelector(${JSON.stringify(scope)})`};
    if (!root) return "no scope";
    const els = [...root.querySelectorAll('input[aria-label=${JSON.stringify(label)}]')].filter((s) => s.getBoundingClientRect().width > 0);
    const el = els[${n}];
    if (!el) return "no input (" + els.length + ")";
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return "ok";
  })()`);
  if (r !== "ok") throw new Error(`setInput ${label}[${n}]: ${r}`);
  await sleep(350);
}

/** A core request over the Tauri bridge (the same `dispatch` the UI uses). */
async function core(request) {
  if (request.data !== undefined && Object.keys(request.data).length === 0) request = { type: request.type };
  const answer = await ev(`(async () => {
    try { return { ok: true, value: await window.__TAURI_INTERNALS__.invoke("dispatch", { request: ${JSON.stringify(request)} }) }; }
    catch (e) { return { ok: false, error: e instanceof Error ? { message: e.message } : e }; }
  })()`);
  if (!answer.ok) throw new Error(`${request.type}: ${JSON.stringify(answer.error ?? "").slice(0, 300)}`);
  return answer.value.data;
}

const editorSelector = '.ProseMirror[contenteditable="true"], [contenteditable="true"].ProseMirror, main [contenteditable="true"]';
async function focusEditorEnd(scope = "") {
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(`${scope} ${editorSelector}`.trim())}); if (!e) return false; e.focus(); const s = getSelection(); s.selectAllChildren(e); s.collapseToEnd(); return true; })()`);
  await sleep(200);
}
async function reload() {
  await cdp.send("Page.reload", {});
  await sleep(1500);
  await until(`!!document.querySelector(".tab-strip")`, "the tab strip after a reload", 60000);
  await sleep(800);
}
async function onboard() {
  const has = await where('input[aria-label="Workspace name"], input[placeholder="Personal"]');
  if (has) {
    await cdp.click(has.x, has.y);
    await typeText("Views workspace");
    await clickOn("button", "Create workspace");
    await sleep(2500);
  }
  // The recovery-kit screen follows (a few seconds after the workspace is made): "Do this later".
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    if (await where("button.tree-new")) break;
    const later = await where("button, a", "Do this later");
    if (later) {
      await cdp.click(later.x, later.y);
      await sleep(2000);
    } else {
      await sleep(400);
    }
  }
}
const treeItems = () => ev(`[...document.querySelectorAll('[role="treeitem"]')].map((e) => ({ t: e.getAttribute("aria-label"), l: Number(e.getAttribute("aria-level")) }))`);
async function openFromTree(title) {
  await clickOn(`[role="treeitem"][aria-label=${JSON.stringify(title)}]`);
  await sleep(700);
  await until(`document.querySelector(".content-body .page-title-text, .content-body .page-title-button")?.textContent.trim() === ${JSON.stringify(title)}`, `the page "${title}" open`);
}
async function openChild(parent, title) {
  await ev(`(() => { const row = document.querySelector('[role="treeitem"][aria-label=${JSON.stringify(parent)}]'); if (row?.getAttribute("aria-expanded") === "false") row.querySelector("button.tree-chevron")?.click(); })()`);
  await sleep(400);
  await openFromTree(title);
}
const mainTitle = () => ev(`(document.querySelector("section.page .page-title, .content-body .page-title")?.textContent ?? "").trim()`);
async function renameOpenPage(scope, title) {
  await clickOn(`${scope} button.page-title-button`);
  await sleep(250);
  await cdp.send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: "a", code: "KeyA", windowsVirtualKeyCode: 65, modifiers: 2, commands: ["selectAll"] });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "a", code: "KeyA", windowsVirtualKeyCode: 65, modifiers: 2 });
  await typeText(title);
  await enter();
  await sleep(500);
}
async function pickSlash(query, expectNth, nth = 0) {
  const expectFirst = expectNth;
  await typeText(query);
  await sleep(500);
  const menu = await ev(`(() => { const m = document.querySelector('[role="listbox"]'); return m ? [...m.querySelectorAll('[role="option"]')].map((o) => o.textContent.trim()) : null; })()`);
  check(`slash ${query} offers ${expectFirst} at position ${nth + 1}`, Array.isArray(menu) && menu[nth]?.startsWith(expectFirst), menu);
  for (let i = 0; i < nth; i++) await press("ArrowDown", "ArrowDown", 40);
  await enter();
  await sleep(900);
}

// ---- the views' DOM ---------------------------------------------------------------------------------------------------
const tabsInfo = () => ev(`[...document.querySelectorAll(".page-list-tab")].map((b) => ({ id: b.dataset.viewId, name: b.querySelector(".page-list-tab-name")?.textContent.trim(), active: b.getAttribute("aria-selected") === "true" }))`);
const activeLayout = () => ev(`document.querySelector(".page-list[data-layout]")?.dataset.layout ?? null`);
const tableTitles = () => ev(`[...document.querySelectorAll(".page-table-row[data-page] .page-table-title")].map((e) => e.textContent.trim())`);
const toolbarSelects = () => ev(`[...document.querySelectorAll(".page-list-toolbar select")].map((s) => ({ label: s.getAttribute("aria-label"), value: s.value }))`);
async function clickTab(name) {
  await closeSettings();
  const p = await waitWhere(".page-list-tab", name);
  await cdp.click(p.x, p.y);
  await sleep(500);
}
async function addView(layoutName) {
  await clickOn("button.page-list-tab-add");
  await until(`!!document.querySelector('[role="menu"]')`, "the add-view menu");
  await clickOn('[role="menuitem"]', layoutName);
  await sleep(600);
}
/** Dates in the current month, so the calendar's first screen shows them. */
const pad = (n) => String(n).padStart(2, "0");
const NOW = new Date();
const day = (d) => `${NOW.getFullYear()}-${pad(NOW.getMonth() + 1)}-${pad(d)}`;

function pngBytes(size = 96, blue = [60, 110, 220], gold = [255, 200, 40]) {
  const crcTable = new Int32Array(256).map((_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c;
  });
  const crc32 = (buf) => {
    let c = -1;
    for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    return ~c;
  };
  const raw = Buffer.alloc((size * 3 + 1) * size);
  const c = size / 2;
  for (let y = 0; y < size; y++) {
    raw[y * (size * 3 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      const px = (x - c) ** 2 + (y - c) ** 2 < (size / 3) ** 2 ? gold : blue;
      raw.set(px, y * (size * 3 + 1) + 1 + x * 3);
    }
  }
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body) >>> 0);
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8;
  header[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

const TITLES = ["Alpha launch", "Beta review", "Gamma sprint", "Delta handoff", "Epsilon idea"];
const FILTER_DAY = day(10);

// ---- section runner: a failure of one section is recorded and the next one still runs -----------------------------------
let aborted = false;
async function section(name, fn, fatal = false) {
  if (aborted) return;
  try {
    await fn();
  } catch (e) {
    if (fatal) aborted = true;
    check(`${name} (aborted)`, false, String(e?.message ?? e));
    await shot(`dbv-fail-${name.replace(/\W+/g, "-")}`).catch(() => {});
  }
  flush();
}

// ==== build ================================================================================================================
async function build() {
  let spaceId = null;
  let props = {};
  const ids = {}; // title -> page id

  await section("1a onboarding and schema", async () => {
    await until(`!!document.querySelector(".tab-strip, input[placeholder='Personal'], button.link-button, button.tree-new")`, "the tab strip or the onboarding", 90000);
    await onboard();
    await until(`!!document.querySelector("button.tree-new")`, "the sidebar", 30000);
    spaceId = (await core({ type: "SpaceList", data: {} }))[0].id;
    const have = (await core({ type: "PropertySchemaGet", data: { spaceId } })).properties.map((d) => d.name);
    for (const [name, kind] of [["Due", "date"], ["Until", "date"], ["Status", "status"]]) {
      if (!have.includes(name)) await core({ type: "PropertyCreate", data: { spaceId, name, kind } });
    }
    const schema = (await core({ type: "PropertySchemaGet", data: { spaceId } })).properties;
    props = Object.fromEntries(schema.map((d) => [d.name, d]));
    check("1a schema has Due (date), Until (date) and Status (status)", props.Due?.kind === "date" && props.Until?.kind === "date" && props.Status?.kind === "status", schema.map((d) => `${d.name}:${d.kind}`));
    await reload();
  }, true);

  await section("1b Projects with a table view", async () => {
    if (await where('[role="treeitem"][aria-label="Projects"]')) {
      await openFromTree("Projects");
    } else {
      await clickOn("button.tree-new");
      await sleep(900);
      await renameOpenPage(".content-body", "Projects");
    }
    await focusEditorEnd();
    await press("a", "KeyA", 65, 2, undefined, ["selectAll"]);
    await press("Backspace", "Backspace", 8);
    await pickSlash("/table", "Table view", 1);
    await until(`!!document.querySelector('.page-list[data-layout="table"]')`, "the table view");
    check("1b the table view of Projects is drawn", true, await tabsInfo());
  }, true);

  await section("1c five children via New page", async () => {
    for (let i = 0; i < TITLES.length; i++) {
      const before = (await tableTitles()).length;
      await until(`!!document.querySelector("button.page-table-new") && !document.querySelector("button.page-table-new").disabled`, "New page enabled");
      await sleep(800);
      await clickOn("button.page-table-new");
      await until(`document.querySelectorAll(".page-table-row[data-page]").length === ${before + 1}`, "the new row");
      await sleep(600);
      const where_ = await ev(`({ peek: !!document.querySelector(".peek"), main: (document.querySelector("section.page .page-title, .content-body .page-title")?.textContent ?? "").trim() })`);
      note(`newPage${i}`, where_);
      if (where_.peek) {
        await renameOpenPage(".peek", TITLES[i]);
      } else {
        await renameOpenPage(".content-body", TITLES[i]);
        await openFromTree("Projects");
      }
      await escape();
      await sleep(300);
    }
    await until(`${JSON.stringify(TITLES)}.every((t) => [...document.querySelectorAll(".page-table-row[data-page] .page-table-title")].some((e) => e.textContent.trim() === t))`, "the five titles in the table", 8000).catch(() => {});
    const titles = await tableTitles();
    check("1c the table lists the five children", TITLES.every((t) => titles.includes(t)), titles);
  }, true);

  await section("1d values through the core, image block, colour cover", async () => {
    const rows = await ev(`[...document.querySelectorAll(".page-table-row[data-page]")].map((r) => ({ id: r.dataset.page, title: r.querySelector(".page-table-title")?.textContent.trim() }))`);
    for (const r of rows) ids[r.title] = r.id;
    const statusOf = (name) => props.Status.options.find((o) => o.name === name)?.id;
    const setDate = (title, prop, date) => core({ type: "PagePropertySet", data: { pageId: ids[title], propertyId: props[prop].id, value: { kind: "date", value: { date } } } });
    const setStatus = (title, name) => core({ type: "PagePropertySet", data: { pageId: ids[title], propertyId: props.Status.id, value: { kind: "status", value: statusOf(name) } } });
    await setDate(TITLES[0], "Due", day(5));
    await setDate(TITLES[1], "Due", day(8));
    await setDate(TITLES[2], "Due", day(12));
    await setDate(TITLES[2], "Until", day(14));
    await setDate(TITLES[3], "Due", day(20));
    await setStatus(TITLES[0], "Done");
    await setStatus(TITLES[1], "In progress");
    await setStatus(TITLES[2], "Not started");
    await setStatus(TITLES[3], "Done");
    await setStatus(TITLES[4], "In progress");
    note("ids", ids);
    check("1d values set through the core (PagePropertySet)", Object.keys(ids).length === 5, ids);

    // An image block on Beta: /image, the native Open dialog (Win32), the block appears.
    writeFileSync(pngPath, pngBytes(96));
    await openFromTree("Projects");
    await openChild("Projects", TITLES[1]);
    await focusEditorEnd();
    await ev(`(() => { const e = document.querySelector(${JSON.stringify(editorSelector)}); e.focus(); })()`);
    await typeText("/image");
    await sleep(600);
    const menu = await ev(`[...document.querySelectorAll('[role="listbox"] [role="option"]')].map((o) => o.textContent.trim())`);
    note("slashImage", menu);
    const dialogDone = helper("file-dialog", { path: pngPath, timeoutMs: 25000 });
    await enter();
    const dlg = await dialogDone;
    check("1d native Open dialog driven (Win32)", dlg.ok, dlg);
    const img = await until(`!!document.querySelector(".ProseMirror [data-type='image'], .ProseMirror img")`, "the image block", 20000).catch(() => false);
    check("1d Beta review has an image block", img, "image in the editor");

    // A colour cover on Delta.
    await openChild("Projects", TITLES[3]);
    await clickOn("button.page-cover-button");
    await until(`!!document.querySelector(".cover-picker")`, "the cover picker");
    await clickOn(".cover-picker .cover-swatch[data-cover-color]");
    await until(`!!document.querySelector(".page-cover[data-cover-color]")`, "the banner with the colour");
    check("1d Delta handoff has a colour cover", true, await ev(`document.querySelector(".page-cover")?.dataset.coverColor`));
    await openFromTree("Projects");
    await reload();
    await openFromTree("Projects");
    const titles = await tableTitles();
    check("1d after a reload the table still lists the five pages", titles.length === 5, titles);
    const cells = await ev(`[...document.querySelectorAll(".page-table-row[data-page]")].map((r) => r.innerText.replace(/\\s+/g, " ").trim())`);
    note("tableRows", cells);
  });

  await section("2 calendar", async () => {
    await openFromTree("Projects");
    await addView("Calendar");
    const tabs = await tabsInfo();
    check("2a the Calendar tab is added and active", tabs.length === 2 && tabs[1].active && (await activeLayout()) === "calendar", tabs);
    check("2a nothing is preselected: the layout asks to choose a date property", /Choose a date property/.test(await ev(`document.querySelector(".page-calendar")?.innerText ?? ""`)), "pageCalendar.choose");
    await setViewSetting("Date", props.Due.id);
    await until(`!!document.querySelector('.page-calendar[data-state="ok"]')`, "the calendar with a date");
    await setViewSetting("End date", props.Until.id);
    await sleep(500);
    // A page of one day is a chip inside its day's cell; a page of several days is a bar (`.page-calendar-bar`) in the overlay
    // of its week row, one segment per row it touches, placed by `data-column` and `data-span` over the row's seven cells.
    const chipsOn = (d) => ev(`[...document.querySelectorAll('.page-calendar-day[data-day="${d}"] .page-calendar-chip')].map((c) => ({ t: c.querySelector(".page-list-name")?.textContent.trim(), pos: c.dataset.position }))`);
    const barSegments = () => ev(`[...document.querySelectorAll(".page-calendar-weekrow")].flatMap((row) => {
      const days = [...row.querySelectorAll(".page-calendar-week .page-calendar-day")].map((d) => d.dataset.day);
      return [...row.querySelectorAll(".page-calendar-bars .page-calendar-bar")].map((b) => {
        const column = Number(b.dataset.column);
        const span = Number(b.dataset.span);
        return { t: b.querySelector(".page-list-name")?.textContent.trim(), days: days.slice(column - 1, column - 1 + span), lane: Number(b.dataset.lane), before: b.hasAttribute("data-continues-before"), after: b.hasAttribute("data-continues-after") };
      });
    })`);
    const d5 = await chipsOn(day(5));
    const d8 = await chipsOn(day(8));
    check("2b Alpha sits on day 5 and Beta on day 8", d5.some((c) => c.t === TITLES[0]) && d8.some((c) => c.t === TITLES[1]), { d5, d8 });
    // Gamma (Due 12, Until 14): bar segments that together cover 12, 13 and 14 (more than one only when a week row begins
    // inside the span; then the segments continue into each other) and not the 15th, and no chip on those days.
    const gamma = (await barSegments()).filter((b) => b.t === TITLES[2]).sort((a, b) => (a.days[0] < b.days[0] ? -1 : 1));
    const gammaDays = gamma.flatMap((b) => b.days);
    const joined = gamma.every((b, i) => b.before === (i > 0) && b.after === (i < gamma.length - 1));
    const gammaChips = [await chipsOn(day(12)), await chipsOn(day(13)), await chipsOn(day(14)), await chipsOn(day(15))].flat().filter((c) => c.t === TITLES[2]);
    check("2b Gamma is a bar over three days (12, 13, 14), not the 15th, and has no chip per day", gamma.length > 0 && JSON.stringify(gammaDays) === JSON.stringify([day(12), day(13), day(14)]) && joined && gammaChips.length === 0, { gamma, gammaChips });
    const tray = await ev(`[...document.querySelectorAll(".page-calendar-tray .page-calendar-chip .page-list-name")].map((e) => e.textContent.trim())`);
    check('2c the "No date" tray lists Epsilon idea', tray.includes(TITLES[4]) && tray.length === 1, tray);
    // The shot: the block from its tabs and toolbar down, scrolled further only when the bars would be below the window.
    await showBlockTop();
    await ev(`(() => { const bar = document.querySelector(".page-calendar-bar"); if (bar && bar.getBoundingClientRect().bottom > innerHeight - 20) bar.scrollIntoView({ block: "center" }); })()`);
    await sleep(200);
    await shot("dbv-1-calendar");

    // Reschedule Beta from day 8 to day 9 with the card menu "Change date...".
    const menuBtn = await where(`.page-calendar-day[data-day="${day(8)}"] .page-calendar-chip .page-calendar-more-button`);
    if (!menuBtn) throw new Error("no card menu button on day 8");
    await cdp.click(menuBtn.x, menuBtn.y);
    await until(`!!document.querySelector('[role="menu"]')`, "the card menu");
    await clickOn('[role="menuitem"]', "Change date");
    await until(`!!document.querySelector('.date-picker-grid [data-date="${day(9)}"]')`, "the date picker");
    await clickOn(`.date-picker-grid [data-date="${day(9)}"]`);
    await sleep(900);
    const due = (await core({ type: "PagePropertiesGet", data: { pageId: ids[TITLES[1]] } }));
    note("betaProps", due);
    const text = JSON.stringify(due);
    check("2d Change date... moved Beta's Due to day 9 in the core", text.includes(day(9)) && !text.includes(day(8)), text.slice(0, 300));
    const onNine = await chipsOn(day(9));
    check("2d the chip now sits on day 9", onNine.some((c) => c.t === TITLES[1]), onNine);

    // Tabs: Table <-> Calendar; the active tab survives a reload (device-local setting).
    await clickTab("Table");
    check("2e clicking the Table tab shows the table", (await activeLayout()) === "table", await tabsInfo());
    await clickTab("Calendar");
    check("2e the Calendar tab shows the calendar again", (await activeLayout()) === "calendar", "");
    await reload();
    await openFromTree("Projects");
    await sleep(500);
    check("2e the active tab (Calendar) persists after a reload", (await activeLayout()) === "calendar", await tabsInfo());
  });

  await section("3 gallery", async () => {
    await addView("Gallery");
    check("3a the Gallery tab is added and active", (await activeLayout()) === "gallery", await tabsInfo());
    await sleep(2500);
    const cards = () => ev(`[...document.querySelectorAll(".page-gallery-card[data-page]")].map((c) => ({ t: c.querySelector(".page-gallery-name")?.textContent.trim(), preview: c.querySelector(".page-gallery-preview")?.dataset.preview ?? null, state: c.querySelector(".page-gallery-preview")?.dataset.state ?? null, img: !!c.querySelector("img.page-gallery-image"), color: c.querySelector(".page-gallery-preview")?.dataset.coverColor ?? null, icon: !!c.querySelector(".page-gallery-body .page-list-icon"), bigIcon: !!c.querySelector(".page-gallery-preview[data-preview=icon] .page-gallery-preview-icon") }))`);
    let list = await cards();
    for (let i = 0; i < 20 && list.find((c) => c.t === TITLES[1])?.state !== "ready"; i++) {
      await sleep(500);
      list = await cards();
    }
    note("galleryCards", list);
    const by = Object.fromEntries(list.map((c) => [c.t, c]));
    check("3b the card with the image shows it as the preview (first-image index)", by[TITLES[1]]?.preview === "image" && by[TITLES[1]]?.img, by[TITLES[1]]);
    check("3b the colour-cover card shows its cover", by[TITLES[3]]?.preview === "cover" && by[TITLES[3]]?.color !== null, by[TITLES[3]]);
    check("3b the other cards show their icon, large in the preview area, and no picture", [0, 2, 4].every((i) => by[TITLES[i]]?.preview === "icon" && by[TITLES[i]]?.bigIcon && !by[TITLES[i]]?.img && by[TITLES[i]]?.icon), [0, 2, 4].map((i) => by[TITLES[i]]));
    const sizes = {};
    for (const size of ["s", "m", "l"]) {
      await setViewSetting("Card size", size);
      await sleep(500);
      sizes[size] = await ev(`(() => { const g = document.querySelector(".page-gallery"); const grid = document.querySelector(".page-gallery-grid"); return { size: g?.dataset.size, min: getComputedStyle(grid).getPropertyValue("--gallery-card-width").trim(), cols: getComputedStyle(grid).gridTemplateColumns.split(" ").length, w: Math.round(document.querySelector(".page-gallery-card")?.getBoundingClientRect().width) }; })()`);
    }
    check("3c card size L: 320 px minimum, wider cards and fewer columns than S", sizes.l.size === "l" && sizes.l.min === "320px" && sizes.l.w >= 320 && sizes.l.w > sizes.s.w && sizes.l.cols < sizes.s.cols, sizes);
    // The shot: the block's tabs and the compact toolbar (shown by the focus on the sliders button), then the labelled settings menu.
    await showBlockTop();
    check("3d the compact toolbar shows Filter, Sort, Properties and the view settings button", await ev(`(() => { const bar = document.querySelector(".page-list-toolbar"); const names = bar ? [...bar.querySelectorAll("button")].map((b) => b.getAttribute("aria-label") ?? b.textContent.trim().replace(/ +[0-9]+$/, "")) : []; return names.length === 4 && names.slice(0, 3).join() === "Filter,Sort,Properties"; })()`), await ev(`[...(document.querySelector(".page-list-toolbar")?.querySelectorAll("button") ?? [])].map((b) => b.getAttribute("aria-label") ?? b.textContent.trim())`));
    await shot("dbv-2-gallery");
    await openSettings();
    await shot("dbv-2b-gallery-settings");
    await closeSettings();
  });

  await section("4 peek", async () => {
    await clickTab("Table");
    await sleep(400);
    const hostBefore = await mainTitle();
    await clickOn("button.page-table-open", TITLES[0]);
    await until(`!!document.querySelector(".peek")`, "the side peek");
    const state = await ev(`({ peek: !!document.querySelector(".peek"), dialog: !!document.querySelector(".peek-dialog"), peekTitle: document.querySelector(".peek #peek-title, .peek .page-title")?.textContent.trim(), main: (document.querySelector("section.page .page-title")?.textContent ?? "").trim() })`);
    check("4a a click on a row opens the side peek and the main page stays", state.peek && !state.dialog && state.peekTitle === TITLES[0] && state.main === hostBefore, state);
    await shot("dbv-3-peek");
    await renameOpenPage(".peek", "Alpha launch v2");
    const renamed = (await tableTitles()).includes("Alpha launch v2");
    check("4b renaming the page in the peek updates the table row", renamed, await tableTitles());
    const focusAfterRename = await ev(`document.activeElement?.tagName + "." + (document.activeElement?.className ?? "") + " inPeek=" + !!document.activeElement?.closest(".peek")`);
    note("focusAfterRename", focusAfterRename);
    await escape();
    await sleep(500);
    let closed = !(await ev(`!!document.querySelector(".peek")`));
    note("escWithFocusOnBodyAfterRename", { closed, focus: focusAfterRename });
    if (!closed) {
      await ev(`document.querySelector(".peek-scroll")?.focus?.(); document.querySelector(".peek .ProseMirror")?.focus()`);
      await escape();
      await sleep(500);
      closed = !(await ev(`!!document.querySelector(".peek")`));
      check("4c Esc closes the peek (focus in the peek)", closed, await ev(`document.activeElement?.className`));
      if (!closed) { await clickOn(".peek-close"); }
    }
    // Center peek: the toolbar's "Open pages in".
    await setViewSetting("Open pages in", "center");
    await clickOn("button.page-table-open", TITLES[1]);
    await until(`!!document.querySelector(".peek-dialog, [role='dialog'] .peek")`, "the center peek", 8000).catch(() => {});
    const center = await ev(`({ dialog: !!document.querySelector(".peek-dialog"), role: document.querySelector(".peek-dialog")?.closest('[role="dialog"]') !== null || document.querySelector('[role="dialog"] .peek') !== null })`);
    check("4d with 'Center peek' a click opens a dialog", center.dialog, center);
    await shot("dbv-4-center-peek");
    await clickOn(".peek button", "Open as page");
    await until(`!document.querySelector(".peek") && (document.querySelector("section.page .page-title")?.textContent ?? "").trim() === ${JSON.stringify(TITLES[1])}`, "Open as page navigated");
    check("4e Open as page navigates to the page and closes the peek", true, await mainTitle());
    await openFromTree("Projects");
    await setViewSetting("Open pages in", "side");
  });

  await section("5 filter groups", async () => {
    await openFromTree("Projects");
    await clickTab("Table");
    // Rename the page back if the renaming above failed.
    await clickOn(".page-list-toolbar button.page-list-button", "Filter");
    await until(`!!document.querySelector(".page-filters")`, "the filter editor");
    await clickOn(".page-filters button.page-filter-add", "Add a filter");
    await until(`!!document.querySelector('.page-filters .page-filter-row')`, "the first filter row");
    await setSelect(".page-filters", "Property", props.Status.id, 0);
    await setSelect(".page-filters", "Condition", "hasOption", 0);
    await setSelect(".page-filters", "Option", props.Status.options.find((o) => o.name === "Done").id, 0);
    await clickOn(".page-filters button.page-filter-add", "Add filter group");
    await until(`!!document.querySelector('.page-filters .page-filter-group')`, "the filter group");
    await clickOn(".page-filters .page-filter-group button.page-filter-add", "Add a filter to group 1");
    await setSelect(".page-filters .page-filter-group", "Property", props.Due.id, 0);
    await setSelect(".page-filters .page-filter-group", "Condition", "before", 0);
    await setInput(".page-filters .page-filter-group", "Day", FILTER_DAY, 0);
    await clickOn(".page-filters .page-filter-group button.page-filter-add", "Add a filter to group 1");
    await setSelect(".page-filters .page-filter-group", "Property", props.Status.id, 1);
    await setSelect(".page-filters .page-filter-group", "Condition", "hasOption", 1);
    await setSelect(".page-filters .page-filter-group", "Option", props.Status.options.find((o) => o.name === "In progress").id, 0);
    // The top-level match: any filter (OR). The nested group starts with the opposite (AND).
    const matches = await ev(`[...document.querySelectorAll(".page-filters select")].filter((s) => /^Match/.test(s.getAttribute("aria-label") ?? "")).map((s) => s.getAttribute("aria-label") + "=" + s.value)`);
    note("matchSelects", matches);
    await setSelect(".page-filters", "Match", "or", 0);
    await setSelect(".page-filters", "Match in group 1", "and", 0);
    await sleep(600);
    await shot("dbv-5-filter-editor");
    const shown = await tableTitles();
    note("filteredRows", shown);
    // Expected: Done (Alpha, Delta) OR (Due before 10 AND In progress: Beta, Due 9 after the move; Epsilon has no Due).
    const expected = ["Alpha launch v2", TITLES[1], TITLES[3]].sort();
    check(`5 'Status has Done OR (Due is before ${FILTER_DAY} AND Status has In progress)' leaves the right rows`, JSON.stringify([...shown].sort()) === JSON.stringify(expected), { shown, expected });
    await escape();
    await sleep(300);
    // Leave the Calendar as the active tab for the restart check.
    await clickTab("Calendar");
    check("5 the filter group did not change the calendar's own rows (it has its own filter)", (await activeLayout()) === "calendar", await tabsInfo());
  });
  flush();
}

// ==== verify (after a restart) =============================================================================================
async function verify() {
  await section("6 restart persistence", async () => {
    await until(`!!document.querySelector(".tab-strip")`, "the tab strip", 90000);
    await sleep(1500);
    await until(`!!document.querySelector('[role="treeitem"][aria-label="Projects"]')`, "the Projects page in the tree", 30000);
    await openFromTree("Projects");
    await sleep(800);
    const tabs = await tabsInfo();
    check("6a the three views survive the restart (Table, Calendar, Gallery)", tabs.length === 3 && ["Table", "Calendar", "Gallery"].every((n, i) => tabs[i].name === n), tabs);
    check("6b the active tab (Calendar) survives the restart", tabs[1]?.active === true && (await activeLayout()) === "calendar", tabs);
    await shot("dbv-6-restart-calendar");
    await clickTab("Table");
    await clickOn(".page-list-toolbar button.page-list-button", "Filter");
    await until(`!!document.querySelector(".page-filters")`, "the filter editor");
    const f = await ev(`({ rows: [...document.querySelectorAll(".page-filters .page-filter-row")].length, groups: document.querySelectorAll(".page-filters .page-filter-group").length, match: [...document.querySelectorAll(".page-filters select")].filter((s) => /^Match/.test(s.getAttribute("aria-label") ?? "")).map((s) => s.value) })`);
    check("6c the filter group survives (3 filters, 1 group, OR at the top)", f.rows === 3 && f.groups === 1 && f.match[0] === "or", f);
    const shown = await tableTitles();
    const expected = ["Alpha launch v2", TITLES[1], TITLES[3]].sort();
    check("6c the same rows are shown after the restart", JSON.stringify([...shown].sort()) === JSON.stringify(expected), shown);
    await escape();
  });
  flush();
}

try {
  if (phase === "verify") await verify();
  else await build();
} catch (e) {
  check("driver", false, String(e?.stack ?? e));
}
const failed = report.checks.filter((c) => !c.ok);
const consoleReport = cdp.report();
report.notes.consoleErrors = consoleReport.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).slice(0, 15);
console.log(`\n${report.checks.length - failed.length}/${report.checks.length} checks passed in ${stamp()}`);
flush();
cdp.close();
process.exit(failed.length > 0 ? 1 : 0);
