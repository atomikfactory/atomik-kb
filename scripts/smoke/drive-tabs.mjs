// The in-page half of the tabs pass (drive-tabs.ps1, ADR-0034): trusted mouse and key input and the page's own
// state, over the DevTools port of the app's WebView2 (see cdp.mjs). Phases:
//
//   node drive-tabs.mjs launch  <outfile>   the marker of the document is set; one Home tab, nothing else
//   node drive-tabs.mjs pages   <outfile>   Alpha, Beta, Gamma and Long (80 paragraphs) through the sidebar
//   node drive-tabs.mjs tabs    <outfile>   click, Ctrl+click, Ctrl+Shift+click, a click on an open page, Pin tab, middle click
//   node drive-tabs.mjs keys    <outfile>   the tab keys, in the page, in the editor and in the title field
//   node drive-tabs.mjs mouse   <outfile>   the mouse back and forward buttons
//   node drive-tabs.mjs scroll  <outfile>   scroll positions of a long page across tab switches, back and forward
//   node drive-tabs.mjs drag    <outfile>   a tab dragged to a new place (HTML5 drag and drop, through the DevTools port's drag interception)
//   node drive-tabs.mjs views   <outfile>   an overflowing strip, the sidebar hidden, both themes
//   node drive-tabs.mjs restart <outfile>   after the app was started again on the same data: the tabs came back
//   node drive-tabs.mjs verify  <outfile>   the summary of every phase
//
// Every phase writes what the page asked the network for, what failed, the console and the content-security-policy
// violations it saw, as JSON, and exits 1 when its own check fails, when the page asked another host for
// something, when a violation, a failed request or a console error was seen, or when the document of the
// webview was replaced (the marker `window.__tabsMarker` set by `launch` is lost by a reload and by a
// navigation of the webview's own history, and `location.href` must not change either).
//
// The strip is written like this in the notes: the titles of the tabs in order, `^` after a pinned tab, `*` after
// the active one: "Gamma^ | Alpha | Long*".

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { connect } from "./cdp.mjs";

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.TABS_OUT_DIR ?? join(HERE, "out");
const STATE_FILE = join(OUT, "tabs-state.json");
const PHASES = ["launch", "pages", "tabs", "keys", "mouse", "scroll", "drag", "views", "restart"];
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
mkdirSync(OUT, { recursive: true });

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
    const file = join(OUT, `tabs-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = { ok: report.ok, checks: report.notes.length, failed: report.notes.filter((n) => n.startsWith("FAIL")).length };
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

// ----- input ----------------------------------------------------------------------------------------------------

const MOD = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
const BUTTON_BIT = { left: 1, right: 2, middle: 4, back: 8, forward: 16 };
const send = (method, params) => page.send(method, params);

/**
 * A key with modifiers as a keyboard sends it: the modifier keys go down, the key goes down and up, the
 * modifiers go up. `modifiers` is a list of "ctrl" | "alt" | "shift".
 */
async function chord(modifiers, key, code, vk) {
  const keys = [];
  if (modifiers.includes("ctrl")) keys.push({ key: "Control", code: "ControlLeft", vk: 17, bit: MOD.ctrl });
  if (modifiers.includes("alt")) keys.push({ key: "Alt", code: "AltLeft", vk: 18, bit: MOD.alt });
  if (modifiers.includes("shift")) keys.push({ key: "Shift", code: "ShiftLeft", vk: 16, bit: MOD.shift });
  const system = modifiers.includes("alt");
  let mask = 0;
  for (const m of keys) {
    mask |= m.bit;
    await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: m.key, code: m.code, windowsVirtualKeyCode: m.vk, modifiers: mask, isSystemKey: system });
  }
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode: vk, modifiers: mask, isSystemKey: system });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: vk, modifiers: mask, isSystemKey: system });
  for (const m of keys.reverse()) {
    mask &= ~m.bit;
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: m.key, code: m.code, windowsVirtualKeyCode: m.vk, modifiers: mask, isSystemKey: system });
  }
  await wait(250);
}

const KEYS = {
  newTab: (c) => c(["ctrl"], "t", "KeyT", 84),
  closeTab: (c) => c(["ctrl"], "w", "KeyW", 87),
  reopen: (c) => c(["ctrl", "shift"], "T", "KeyT", 84),
  next: (c) => c(["ctrl"], "Tab", "Tab", 9),
  previous: (c) => c(["ctrl", "shift"], "Tab", "Tab", 9),
  pageDown: (c) => c(["ctrl"], "PageDown", "PageDown", 34),
  pageUp: (c) => c(["ctrl"], "PageUp", "PageUp", 33),
  moveLeft: (c) => c(["ctrl", "shift"], "PageUp", "PageUp", 33),
  moveRight: (c) => c(["ctrl", "shift"], "PageDown", "PageDown", 34),
  back: (c) => c(["alt"], "ArrowLeft", "ArrowLeft", 37),
  forward: (c) => c(["alt"], "ArrowRight", "ArrowRight", 39),
  sidebar: (c) => c(["ctrl"], "\\", "Backslash", 220),
};
const digit = (n) => (c) => c(["ctrl"], String(n), `Digit${n}`, 48 + n);
const press = (name) => (typeof name === "string" ? KEYS[name] : name)(chord);

/** A mouse button pressed and released at a point, with modifiers ("ctrl" | "shift" | "alt"). */
async function mouse(x, y, { button = "left", modifiers = [], clickCount = 1 } = {}) {
  const mask = modifiers.reduce((sum, m) => sum | MOD[m], 0);
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, modifiers: mask });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, buttons: BUTTON_BIT[button], clickCount, modifiers: mask });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, buttons: 0, clickCount, modifiers: mask });
  await wait(250);
}

async function wheel(x, y, deltaY, deltaX = 0) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await send("Input.dispatchMouseEvent", { type: "mouseWheel", x, y, deltaX, deltaY });
  await wait(60);
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

/** What the strip and the main area show. */
async function look() {
  return page.evaluate(`(() => {
    const buttons = [...document.querySelectorAll('.tab-strip [role="tab"]')];
    const tabs = buttons.map((b) => {
      const label = b.getAttribute("aria-label") ?? "";
      const pinned = b.closest(".tab")?.classList.contains("is-pinned") === true;
      return {
        name: pinned ? label.replace(/ \\(pinned\\)$/, "") : label,
        pinned,
        active: b.getAttribute("aria-selected") === "true",
        pinnedLabel: !pinned || / \\(pinned\\)$/.test(label),
        iconOnly: pinned ? b.querySelector(".tab-title") === null : b.querySelector(".tab-title") !== null,
      };
    });
    const nav = (name) => {
      const b = document.querySelector('.tab-strip button[aria-label="' + name + '"]');
      return b === null ? null : !b.disabled;
    };
    const h1 = document.querySelector(".content-body h1");
    const body = document.querySelector(".content-body");
    // While the title is being renamed the heading holds a field, not text.
    const heading = h1 === null ? null : h1.textContent.trim() || (h1.querySelector("input")?.value ?? "");
    return {
      tabs,
      back: nav("Back"),
      forward: nav("Forward"),
      newTab: nav("New tab"),
      h1: heading,
      scrollTop: body === null ? null : Math.round(body.scrollTop),
      listRole: document.querySelector(".tab-strip .tab-list")?.getAttribute("role") ?? null,
    };
  })()`);
}

const text = (s) => s.tabs.map((t) => t.name + (t.pinned ? "^" : "") + (t.active ? "*" : "")).join(" | ");

/** The strip is as expected (polled for a moment: the store and the labels settle after an action). The phase stops when it is not. */
async function expectStrip(expected, note, { title = null, back = null, forward = null, ms = 4000, ignoreActive = false } = {}) {
  const shown = (s) => (ignoreActive ? text(s).replaceAll("*", "") : text(s));
  let seen;
  const end = Date.now() + ms;
  while (Date.now() < end) {
    seen = await look();
    if (shown(seen) === expected && (title === null || seen.h1 === title)) {
      break;
    }
    await wait(150);
  }
  const wanted = `strip "${expected}"${title === null ? "" : `, showing "${title}"`}`;
  if (shown(seen) === expected && (title === null || seen.h1 === title)) {
    check(true, `${note}: ${wanted}`);
  } else {
    must(false, `${note}: expected ${wanted}, got strip "${shown(seen)}", showing "${seen.h1}"`);
  }
  if (back !== null) {
    check(seen.back === back, `${note}: Back is ${back ? "enabled" : "disabled"} (${seen.back})`);
  }
  if (forward !== null) {
    check(seen.forward === forward, `${note}: Forward is ${forward ? "enabled" : "disabled"} (${seen.forward})`);
  }
  return seen;
}

/** The centre of a tab in the strip, scrolled into view, by its title (as the strip names it: pinned ones too). */
async function tabBox(title) {
  return page.evaluate(`(() => {
    const b = [...document.querySelectorAll('.tab-strip [role="tab"]')].find((x) => {
      const label = x.getAttribute("aria-label") ?? "";
      return label === ${JSON.stringify(title)} || label === ${JSON.stringify(title + " (pinned)")};
    });
    if (!b) return null;
    b.scrollIntoView({ inline: "nearest", block: "nearest" });
    const r = b.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })()`);
}

async function clickTab(title, options = {}) {
  const found = await tabBox(title);
  must(found !== null, `the tab "${title}" is in the strip`);
  await mouse(found.cx, found.cy, options);
}

/** A row of the sidebar's tree, by page title. */
async function rowBox(title) {
  return box(`[role="treeitem"][aria-label=${JSON.stringify(title)}] .tree-title`);
}

async function clickRow(title, options = {}) {
  const found = await rowBox(title);
  must(found !== null, `the sidebar has a row "${title}"`);
  await mouse(found.cx, found.cy, options);
}

async function focusInfo() {
  return page.evaluate(`(() => {
    const el = document.activeElement;
    return el === null ? "" : el.tagName + (el.className && typeof el.className === "string" ? "." + el.className.split(" ")[0] : "") + (el.isContentEditable ? "[editable]" : "");
  })()`);
}

/** The scroll position and extent of the main area. */
async function geometry() {
  return page.evaluate(`(() => {
    const b = document.querySelector(".content-body");
    return { top: Math.round(b.scrollTop), max: b.scrollHeight - b.clientHeight, height: b.clientHeight };
  })()`);
}

/** The webview's own document and history: nothing may have replaced the first or moved the second. */
async function intact(when) {
  const state = readState();
  const now = await page.evaluate(`(() => ({ marker: window.__tabsMarker ?? null, href: location.href, alive: document.readyState }))()`).catch(() => null);
  must(now !== null, `${when}: the window answers`);
  check(now.marker === state.marker && state.marker !== undefined, `${when}: the document was not replaced (marker ${now.marker === state.marker ? "kept" : "LOST: " + now.marker})`);
  check(now.href === state.href, `${when}: the address is unchanged (${now.href})`);
}

/** Captures the window with PrintWindow (capture.ps1's Capture-Window, run in PowerShell). */
function capture(name) {
  const path = join(OUT, `tabs-${name}.png`);
  try {
    execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        '. $env:TABS_CAPTURE_PS1; $p = Get-Process -Name atomik-kb-desktop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1; Capture-Window $p.MainWindowHandle $env:TABS_CAPTURE_PATH',
      ],
      { env: { ...process.env, TABS_CAPTURE_PATH: path }, timeout: 40000, stdio: "pipe" },
    );
    result.captures.push(path);
  } catch (error) {
    check(false, `capture ${name} failed: ${error instanceof Error ? error.message : String(error)}`);
  }
}

/** Leaves nothing open from an earlier phase that failed half-way. */
async function normalize() {
  for (let i = 0; i < 3; i++) {
    const open = await page.evaluate(`!!document.querySelector('[role="menu"], [role="dialog"]')`);
    if (!open) {
      return;
    }
    await page.key("Escape", "Escape", 27);
    await wait(200);
  }
}

// ---------------------------------------------------------------------------------------------------------------
// phases
// ---------------------------------------------------------------------------------------------------------------
const PARAGRAPH = (n) => `Paragraph ${n}. The quick brown fox jumps over the lazy dog again and again until this page is taller than the window.`;

async function phaseLaunch() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `tabs-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__tabsMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href, snapshot: null });
  const size = await page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio, theme: document.documentElement.dataset.theme ?? "(none)", dark: matchMedia("(prefers-color-scheme: dark)").matches })`);
  result.window = size;
  info(`window ${size.w}x${size.h} at ${size.dpr}x, data-theme ${size.theme}, system prefers dark: ${size.dark}`);
  const seen = await look();
  must(seen.tabs.length === 1, `one tab at the start (${text(seen)})`);
  check(text(seen) === "Home*", `the first tab is a Home tab, active (${text(seen)})`);
  check(seen.h1 === "Smoke workspace", `the main area shows the workspace's Home (${seen.h1})`);
  check(seen.listRole === "tablist", "the strip is a tablist");
  check(seen.back === false && seen.forward === false, "Back and Forward are disabled with one entry");
  check(seen.newTab === true, "New tab is enabled");
}

async function phasePages() {
  for (const title of ["Alpha", "Beta", "Gamma", "Long"]) {
    await clickOn(".tree-new", `the sidebar's "New page" button (${title})`);
    await waitFor("input.tree-rename", `the rename field of the new page (${title})`);
    await send("Input.insertText", { text: title });
    await page.key("Enter", "Enter", 13);
    await waitFor(`[role="treeitem"][aria-label=${JSON.stringify(title)}]`, `the row "${title}"`);
    await wait(500);
    await expectStrip(`${title}*`, `after creating ${title}`, { title });
  }
  // Long: taller than the window, typed into the editor with real key events.
  await waitFor(".ProseMirror", "the page editor");
  await page.evaluate(`document.querySelector(".ProseMirror").editor.commands.focus("end")`);
  await wait(300);
  for (let n = 1; n <= 80; n++) {
    await send("Input.insertText", { text: PARAGRAPH(n) });
    await page.key("Enter", "Enter", 13);
  }
  await wait(2000); // the document is written a moment after the last key
  const size = await page.evaluate(`(() => {
    const body = document.querySelector(".content-body");
    return { paragraphs: document.querySelectorAll(".ProseMirror p").length, scrollHeight: body.scrollHeight, clientHeight: body.clientHeight };
  })()`);
  result.long = size;
  check(size.paragraphs >= 80, `Long holds the 80 paragraphs (${size.paragraphs})`);
  must(size.scrollHeight > size.clientHeight * 2, `Long is taller than the window (${size.scrollHeight} > 2 x ${size.clientHeight})`);
  await expectStrip("Long*", "after typing into Long", { title: "Long" });
}

async function phaseTabs() {
  await normalize();
  await expectStrip("Long*", "start: one tab");

  // A plain click navigates the active tab (the one tab keeps being the only one).
  await clickRow("Alpha");
  await expectStrip("Alpha*", "plain click on Alpha replaces the page of the tab", { title: "Alpha", back: true, forward: false });

  // Ctrl+click: a background tab right after the active one; the next one lines up after it.
  await clickRow("Beta", { modifiers: ["ctrl"] });
  await expectStrip("Alpha* | Beta", "Ctrl+click Beta opens it in the background", { title: "Alpha" });
  await clickRow("Gamma", { modifiers: ["ctrl"] });
  await expectStrip("Alpha* | Beta | Gamma", "Ctrl+click Gamma lines up after Beta", { title: "Alpha" });

  // Ctrl+Shift+click: a tab in front, behind the background tabs opened from the active one.
  await clickRow("Long", { modifiers: ["ctrl", "shift"] });
  await expectStrip("Alpha | Beta | Gamma | Long*", "Ctrl+Shift+click Long opens it in front", { title: "Long" });

  // A plain click on a page that is open in a tab brings that tab to the front: no new tab.
  await clickRow("Beta");
  let seen = await expectStrip("Alpha | Beta* | Gamma | Long", "plain click on Beta in the sidebar activates its tab (no new tab)", { title: "Beta" });
  check(seen.tabs.length === 4, `still four tabs (${seen.tabs.length})`);

  // The context menu of a tab: its items, then "Pin tab".
  const gamma = await tabBox("Gamma");
  must(gamma !== null, "the Gamma tab is there");
  await mouse(gamma.cx, gamma.cy, { button: "right" });
  await waitFor('[role="menu"]', "the context menu of the tab");
  const items = await page.evaluate(`[...document.querySelectorAll('[role="menu"] [role="menuitem"]')].map((i) => i.textContent.trim() + (i.disabled ? " (disabled)" : ""))`);
  result.menu = items;
  check(
    JSON.stringify(items.map((i) => i.replace(" (disabled)", ""))) === JSON.stringify(["Pin tab", "Close tab", "Close other tabs", "Close tabs to the right", "Reopen closed tab"]),
    `the tab menu lists Pin tab, Close tab, Close other tabs, Close tabs to the right, Reopen closed tab (${items.join("; ")})`,
  );
  check(items[4]?.includes("disabled") === true, "Reopen closed tab is disabled while nothing was closed");
  capture("menu");
  await clickOn('[role="menu"] [role="menuitem"]', 'the menu item "Pin tab"', { text: "Pin tab" });
  seen = await expectStrip("Gamma^ | Alpha | Beta* | Long", "Pin tab moves Gamma first, pinned", { title: "Beta" });
  const pinned = seen.tabs[0];
  check(pinned.pinnedLabel && pinned.iconOnly, `the pinned tab is an icon named "Gamma (pinned)", without a title (${JSON.stringify(pinned)})`);
  const closeButtons = await page.evaluate(`document.querySelectorAll(".tab-strip .tab.is-pinned .tab-close").length`);
  check(closeButtons === 0, "a pinned tab has no close button");
  await wait(300);
  capture("several");

  // A middle click on a tab closes it (the active one here: the tab to its right takes the front).
  const beta = await tabBox("Beta");
  must(beta !== null, "the Beta tab is there");
  await mouse(beta.cx, beta.cy, { button: "middle" });
  await expectStrip("Gamma^ | Alpha | Long*", "middle click on the Beta tab closes it", { title: "Long" });
}

async function phaseKeys() {
  await normalize();
  await expectStrip("Gamma^ | Alpha | Long*", "start");

  await press("newTab");
  const fresh = await expectStrip("Gamma^ | Alpha | Long | Home*", "Ctrl+T opens a Home tab, last", { title: "Smoke workspace" });
  check(fresh.back === false, "a new tab has no history");
  await press("closeTab");
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+W closes it", { title: "Long" });
  await press("reopen");
  await expectStrip("Gamma^ | Alpha | Long | Home*", "Ctrl+Shift+T brings it back, in place", { title: "Smoke workspace" });
  await press("closeTab");
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+W again", { title: "Long" });

  await press("next");
  await expectStrip("Gamma^* | Alpha | Long", "Ctrl+Tab goes to the next tab, wrapping", { title: "Gamma" });
  await press("next");
  await expectStrip("Gamma^ | Alpha* | Long", "Ctrl+Tab again", { title: "Alpha" });
  await press("previous");
  await expectStrip("Gamma^* | Alpha | Long", "Ctrl+Shift+Tab goes back", { title: "Gamma" });
  await press("previous");
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+Shift+Tab wraps to the last", { title: "Long" });
  await press("pageDown");
  await expectStrip("Gamma^* | Alpha | Long", "Ctrl+PageDown is next, wrapping", { title: "Gamma" });
  await press("pageUp");
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+PageUp is previous, wrapping", { title: "Long" });

  await press(digit(1));
  await expectStrip("Gamma^* | Alpha | Long", "Ctrl+1 is the first tab", { title: "Gamma" });
  await press(digit(2));
  await expectStrip("Gamma^ | Alpha* | Long", "Ctrl+2 is the second tab", { title: "Alpha" });
  await press(digit(9));
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+9 is the last tab", { title: "Long" });
  await press(digit(8)); // there is no eighth tab: nothing changes
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+8 with three tabs changes nothing", { title: "Long" });

  await press("moveLeft");
  await expectStrip("Gamma^ | Long* | Alpha", "Ctrl+Shift+PageUp moves the active tab left", { title: "Long" });
  await press("moveLeft");
  await expectStrip("Gamma^ | Long* | Alpha", "Ctrl+Shift+PageUp does not move it in front of the pinned tab", { title: "Long" });
  await press("moveRight");
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+Shift+PageDown moves it right again", { title: "Long" });
  await press("moveRight");
  await expectStrip("Gamma^ | Alpha | Long*", "Ctrl+Shift+PageDown at the end changes nothing", { title: "Long" });

  // Back and forward inside a tab: Alpha's tab opens Beta (it is open nowhere else), Alt+Left returns.
  await press(digit(2));
  await expectStrip("Gamma^ | Alpha* | Long", "Ctrl+2: the Alpha tab", { title: "Alpha" });
  await clickRow("Beta");
  await expectStrip("Gamma^ | Beta* | Long", "a click on Beta navigates the Alpha tab", { title: "Beta", back: true, forward: false });
  await press("back");
  await expectStrip("Gamma^ | Alpha* | Long", "Alt+Left goes back inside the tab", { title: "Alpha", back: true, forward: true });
  await press("forward");
  await expectStrip("Gamma^ | Beta* | Long", "Alt+Right goes forward inside the tab", { title: "Beta", forward: false });
  await intact("after Alt+Left and Alt+Right (the webview must not have gone back in its own history)");

  // Ctrl+W while the focus is inside the page editor.
  await waitFor(".ProseMirror", "the editor of Beta");
  await page.evaluate(`document.querySelector(".ProseMirror").editor.commands.focus("end")`);
  await wait(300);
  const inEditor = await focusInfo();
  must(inEditor.includes("[editable]"), `the focus is in the page editor (${inEditor})`);
  await press("closeTab");
  await expectStrip("Gamma^ | Long*", "Ctrl+W inside the editor closes the tab", { title: "Long" });
  await press("reopen");
  await expectStrip("Gamma^ | Beta* | Long", "Ctrl+Shift+T reopens it in its place", { title: "Beta", back: true });

  // Ctrl+Tab while the focus is in the page title field; Alt+Left is not for a form field.
  await clickOn(".content-body .page-title-button", "the title of the page (a button that becomes a field)");
  await waitFor("input.page-title-edit", "the title field");
  const inTitle = await focusInfo();
  must(inTitle.startsWith("INPUT"), `the focus is in the title field (${inTitle})`);
  await press("back");
  await expectStrip("Gamma^ | Beta* | Long", "Alt+Left in the title field does not go back", { title: "Beta", back: true });
  check((await focusInfo()).startsWith("INPUT"), "the title field kept the focus");
  await press("next");
  await expectStrip("Gamma^ | Beta | Long*", "Ctrl+Tab in the title field goes to the next tab", { title: "Long" });

  // What the checks above cannot show. A key sent over the DevTools port goes to the page; it does not
  // pass the browser process's own key accelerators (the ones a real Alt+Left would meet, and WebView2's
  // switch for them is a host setting). Control: with the app's key handler kept from running, a CDP Alt+Left
  // does not take the webview back in its own history, so "the webview did not move" is not evidence for keys.
  const control = await page.evaluate(`(() => {
    history.pushState({ keys: 1 }, "", location.href);
    history.pushState({ keys: 2 }, "", location.href);
    window.__keyControl = new AbortController();
    window.__keyPops = 0;
    addEventListener("popstate", () => { window.__keyPops += 1; }, { signal: window.__keyControl.signal });
    addEventListener("keydown", (e) => { if (e.altKey) e.stopImmediatePropagation(); }, { capture: true, signal: window.__keyControl.signal });
    return navigation.currentEntry.index;
  })()`);
  await press("back");
  await wait(600);
  const moved = await page.evaluate(`({ index: navigation.currentEntry.index, popstates: window.__keyPops })`);
  await page.evaluate(`window.__keyControl.abort()`);
  info(
    `limit of this phase: with the app's handler bypassed, a DevTools-port Alt+Left ${moved.index === control && moved.popstates === 0 ? "does NOT" : "DOES"} move the webview in its own history (index ${control} -> ${moved.index}); real key presses were not exercised, check Alt+Left, Ctrl+W, Ctrl+Tab by hand on a real keyboard`,
  );
  await expectStrip("Gamma^ | Beta | Long*", "the key control did not change the tabs", { title: "Long" });
}

/** What the webview's own history looks like: a back on it would change every one of these. */
async function webviewHistory() {
  return page.evaluate(`(() => ({
    popstates: window.__popstates ?? null,
    navigations: window.__navigations ?? null,
    index: window.navigation?.currentEntry?.index ?? null,
    entries: window.navigation?.entries().length ?? null,
    state: JSON.stringify(history.state),
  }))()`);
}

async function phaseMouse() {
  await normalize();
  await expectStrip("Gamma^ | Beta | Long*", "start");
  await clickTab("Beta");
  await expectStrip("Gamma^ | Beta* | Long", "the Beta tab, which has a history", { title: "Beta", back: true, forward: false });

  // Entries in the webview's own history, so that a back on it would do something that shows.
  const before = await page.evaluate(`(() => {
    history.pushState({ smoke: 1 }, "", location.href);
    history.pushState({ smoke: 2 }, "", location.href);
    window.__popstates = 0;
    window.__navigations = [];
    addEventListener("popstate", () => { window.__popstates += 1; });
    if (window.navigation) navigation.addEventListener("navigate", (e) => window.__navigations.push(e.navigationType + " " + e.destination.url));
    return true;
  })()`);
  must(before === true, "entries were added to the webview's history");

  // Control: the harness can see the webview going back. With the app's mouse handlers kept from running
  // (capture listeners that stop the events, removed again), the back button of the mouse takes the webview
  // back in its own history, and forward returns: this is what the app has to prevent.
  const main = await box(".content-body");
  await page.evaluate(`(() => {
    window.__control = new AbortController();
    for (const type of ["mousedown", "mouseup", "auxclick", "click"]) {
      window.addEventListener(type, (e) => e.stopImmediatePropagation(), { capture: true, signal: window.__control.signal });
    }
  })()`);
  await mouse(main.cx, main.cy, { button: "back" });
  await wait(600);
  const controlBack = await webviewHistory();
  await mouse(main.cx, main.cy, { button: "forward" });
  await wait(600);
  await page.evaluate(`window.__control.abort()`);
  const controlForward = await webviewHistory();
  check(
    controlBack.popstates >= 1 && controlBack.index === controlForward.index - 1 && controlForward.popstates >= 2,
    `control: with the app's handlers bypassed the mouse buttons do move the webview in its own history, so a check that it did not move can fail (back: index ${controlBack.index}, popstates ${controlBack.popstates}; forward: index ${controlForward.index}, popstates ${controlForward.popstates})`,
  );
  await expectStrip("Gamma^ | Beta* | Long", "the control did not change the tabs", { title: "Beta" });
  await page.evaluate(`(() => { window.__popstates = 0; window.__navigations.length = 0; })()`);
  const base = await webviewHistory();
  info(`webview history before: ${JSON.stringify(base)}`);
  const unmoved = async (when) => {
    const now = await webviewHistory();
    check(
      now.popstates === 0 && now.index === base.index && now.state === base.state && now.navigations.length === 0,
      `${when}: the webview did not go back or forward in its own history (${JSON.stringify(now)})`,
    );
  };

  const side = await box(".sidebar .tree-title");
  const editor = (await box(".ProseMirror")) ?? main;
  const stripSpace = await box(".tab-strip-space");

  await mouse(main.cx, main.cy, { button: "back" });
  await expectStrip("Gamma^ | Alpha* | Long", "mouse back (over the page) goes back in the tab", { title: "Alpha", back: true, forward: true });
  await unmoved("after the mouse back button");
  await mouse(side.cx, side.cy, { button: "forward" });
  await expectStrip("Gamma^ | Beta* | Long", "mouse forward (over the sidebar) goes forward in the tab", { title: "Beta", forward: false });
  await unmoved("after the mouse forward button");
  await mouse(editor.cx, editor.y + 20, { button: "back" });
  await expectStrip("Gamma^ | Alpha* | Long", "mouse back (over the editor) goes back in the tab", { title: "Alpha" });
  await mouse(stripSpace.cx, stripSpace.cy, { button: "forward" });
  await expectStrip("Gamma^ | Beta* | Long", "mouse forward (over the strip) goes forward in the tab", { title: "Beta" });
  await unmoved("after four presses");

  // A tab without history: the buttons change nothing in the app, and the webview still must not move.
  await clickTab("Long");
  await expectStrip("Gamma^ | Beta | Long*", "the Long tab, one entry", { title: "Long", back: false, forward: false });
  const spot = await box(".content-body");
  await mouse(spot.cx, spot.cy, { button: "back" });
  await mouse(spot.cx, spot.cy, { button: "forward" });
  await expectStrip("Gamma^ | Beta | Long*", "mouse back and forward in a tab with one entry change nothing", { title: "Long" });
  await unmoved("after the buttons in a tab with one entry");
}

async function phaseScroll() {
  await normalize();
  await expectStrip("Gamma^ | Beta | Long*", "start", { title: "Long" });
  // The page opens asynchronously: wait for it to be tall.
  let geo = await geometry();
  for (let i = 0; i < 50 && geo.max < 1000; i++) {
    await wait(200);
    geo = await geometry();
  }
  must(geo.max >= 1000, `the Long page is taller than the window (${JSON.stringify(geo)})`);

  // Real wheel events, to about 60 %.
  const body = await box(".content-body");
  const target = Math.round(geo.max * 0.6);
  for (let i = 0; i < 400 && geo.top < target; i++) {
    await wheel(body.cx, body.cy, 100);
    geo = await geometry();
  }
  await wait(600);
  geo = await geometry();
  const scrolled = geo.top;
  must(scrolled >= geo.max * 0.5 && scrolled <= geo.max * 0.75, `Long is scrolled to about 60 % with the wheel (${scrolled} of ${geo.max})`);

  /** The position comes back (the page is laid out some frames after the tab shows). */
  const restoredTo = async (wanted, note) => {
    let top = -1;
    const end = Date.now() + 6000;
    while (Date.now() < end) {
      top = (await geometry()).top;
      if (Math.abs(top - wanted) <= 40) {
        break;
      }
      await wait(150);
    }
    check(Math.abs(top - wanted) <= 40, `${note}: scrollTop ${top}, wanted ${wanted} within 40`);
  };

  // Another tab and back.
  await clickTab("Beta");
  await expectStrip("Gamma^ | Beta* | Long", "the Beta tab", { title: "Beta" });
  check((await geometry()).top <= 5, `Beta shows from the top (${(await geometry()).top})`);
  await clickTab("Long");
  await expectStrip("Gamma^ | Beta | Long*", "back on the Long tab", { title: "Long" });
  await restoredTo(scrolled, "after a switch to Beta and back");
  // The pinned tab and back.
  await clickTab("Gamma");
  await expectStrip("Gamma^* | Beta | Long", "the pinned Gamma tab", { title: "Gamma" });
  await clickTab("Long");
  await expectStrip("Gamma^ | Beta | Long*", "back on the Long tab", { title: "Long" });
  await restoredTo(scrolled, "after a switch to the pinned tab and back");

  // Back and forward between Long and another page, inside the tab.
  await clickRow("Alpha");
  await expectStrip("Gamma^ | Beta | Alpha*", "the Long tab opens Alpha", { title: "Alpha", back: true });
  check((await geometry()).top <= 5, `Alpha shows from the top (${(await geometry()).top})`);
  await clickOn('.tab-strip button[aria-label="Back"]', "the Back button");
  await expectStrip("Gamma^ | Beta | Long*", "Back returns to Long", { title: "Long", forward: true });
  await restoredTo(scrolled, "after Back");
  await clickOn('.tab-strip button[aria-label="Forward"]', "the Forward button");
  await expectStrip("Gamma^ | Beta | Alpha*", "Forward returns to Alpha", { title: "Alpha" });
  check((await geometry()).top <= 5, `Alpha shows from the top again (${(await geometry()).top})`);
  await press("back");
  await expectStrip("Gamma^ | Beta | Long*", "Alt+Left returns to Long", { title: "Long" });
  await restoredTo(scrolled, "after Alt+Left");
  await press("forward");
  await expectStrip("Gamma^ | Beta | Alpha*", "Alt+Right returns to Alpha", { title: "Alpha" });

  // A new entry for Long, in another tab, starts at the top although another entry of Long was scrolled.
  await clickTab("Beta");
  await expectStrip("Gamma^ | Beta* | Alpha", "the Beta tab", { title: "Beta" });
  await clickRow("Long");
  await expectStrip("Gamma^ | Long* | Alpha", "the Beta tab opens Long as a new entry", { title: "Long", back: true });
  await wait(1000);
  geo = await geometry();
  check(geo.max >= 1000 && geo.top <= 5, `a page opened in a tab starts at the top (${geo.top} of ${geo.max})`);
}

/**
 * Drags a tab onto the left or right half of another (HTML5 drag and drop). A mouse drag over the DevTools port would
 * start the operating system's drag loop, so the drag is intercepted (Input.setInterceptDrags) and its events are sent
 * to the page with Input.dispatchDragEvent. Returns the class names of the tabs while the pointer is over the target.
 */
async function dragTab(from, to, zone) {
  const source = await tabBox(from);
  const target = await tabBox(to);
  must(source !== null && target !== null, `the tabs "${from}" and "${to}" are in the strip`);
  await send("Input.setInterceptDrags", { enabled: true });
  const mark = page.events.length;
  const sx = source.cx;
  const sy = source.cy;
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: sx, y: sy });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: sx, y: sy, button: "left", buttons: 1, clickCount: 1 });
  for (let i = 1; i <= 6; i++) {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: sx + i * 8, y: sy, button: "left", buttons: 1 });
  }
  let drag;
  for (let i = 0; i < 20 && drag === undefined; i++) {
    await wait(100);
    drag = page.events.slice(mark).find((e) => e.method === "Input.dragIntercepted");
  }
  let classes = "";
  try {
    must(drag !== undefined, `dragging the "${from}" tab starts a drag`);
    const data = drag.params.data;
    const tx = target.x + target.w * (zone === "before" ? 0.25 : 0.75);
    const ty = target.cy;
    await send("Input.dispatchDragEvent", { type: "dragEnter", x: tx, y: ty, data });
    await send("Input.dispatchDragEvent", { type: "dragOver", x: tx, y: ty, data });
    await wait(200);
    classes = await page.evaluate(`[...document.querySelectorAll(".tab-strip .tab")].map((t) => t.className).join(" / ")`);
    await send("Input.dispatchDragEvent", { type: "drop", x: tx, y: ty, data });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: tx, y: ty, button: "left", buttons: 0, clickCount: 1 });
  } finally {
    await send("Input.setInterceptDrags", { enabled: false });
  }
  await wait(400);
  return classes;
}

async function phaseDrag() {
  await normalize();
  await expectStrip("Gamma^ | Long* | Alpha", "start", { title: "Long" });

  let classes = await dragTab("Alpha", "Long", "before");
  check(classes.includes("is-dragging"), "the dragged tab is marked while it is dragged");
  check(classes.includes("drop-before"), `the tab under the pointer shows where the drop lands (drop-before: ${classes})`);
  await expectStrip("Gamma^ | Alpha | Long*", "dropping Alpha on the left half of Long puts it before Long (the front tab stays)", { title: "Long" });

  // A tab stays in its group: onto the pinned tab nothing moves.
  classes = await dragTab("Alpha", "Gamma", "after");
  check(!classes.includes("drop-after") && !classes.includes("drop-before"), `a drag over a pinned tab shows no drop mark (${classes})`);
  await expectStrip("Gamma^ | Alpha | Long*", "dropping Alpha on the pinned tab changes nothing", { title: "Long" });

  await dragTab("Alpha", "Long", "after");
  await expectStrip("Gamma^ | Long* | Alpha", "dropping Alpha on the right half of Long puts it after Long", { title: "Long" });
  const after = await page.evaluate(`document.querySelectorAll(".tab-strip .tab.is-dragging, .tab-strip .tab.drop-before, .tab-strip .tab.drop-after").length`);
  check(after === 0, "no drag or drop mark is left on a tab");
}

/** The relative luminance and contrast of what the strip shows, read from the computed styles. */
async function stripColours() {
  return page.evaluate(`(() => {
    const parse = (c) => (c.match(/[\\d.]+/g) ?? []).slice(0, 4).map(Number);
    const lum = ([r, g, b]) => {
      const f = (v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4); };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const backdrop = (el) => {
      for (let e = el; e !== null; e = e.parentElement) {
        const c = parse(getComputedStyle(e).backgroundColor);
        if (c.length >= 3 && (c.length === 3 || c[3] > 0)) return c;
      }
      return [255, 255, 255];
    };
    const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
    const report = (el) => {
      if (!el) return null;
      const fg = parse(getComputedStyle(el).color), bg = backdrop(el);
      return { fg, bg, ratio: Math.round(ratio(fg, bg) * 100) / 100 };
    };
    const strip = document.querySelector(".tab-strip");
    return {
      theme: document.documentElement.dataset.theme ?? "(none)",
      stripLuminance: Math.round(lum(backdrop(strip)) * 1000) / 1000,
      active: report(document.querySelector(".tab.is-active .tab-title, .tab.is-active .tab-button")),
      inactive: report(document.querySelector(".tab:not(.is-active):not(.is-pinned) .tab-title")),
      pinned: report(document.querySelector(".tab.is-pinned .tab-button")),
    };
  })()`);
}

async function chooseTheme(label) {
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor("input[name=theme]", "the theme choices");
  await clickOn(".radio", `the "${label}" theme`, { text: label });
  await wait(500);
}

async function phaseViews() {
  await normalize();
  const base = text(await look());
  const baseSeen = await look();
  const baseCount = baseSeen.tabs.length;
  const baseActive = baseSeen.tabs.findIndex((t) => t.active);
  info(`baseline strip: ${base}`);

  // ----- the strip overflows ------------------------------------------------------------------------------
  for (let i = 0; i < 12; i++) {
    await press("newTab");
  }
  let seen = await look();
  must(seen.tabs.length === baseCount + 12, `12 Home tabs were opened (${seen.tabs.length} tabs)`);
  const strip = async () =>
    page.evaluate(`(() => {
      const list = document.querySelector(".tab-strip .tab-list");
      const active = list.querySelector(".tab.is-active");
      const l = list.getBoundingClientRect(), a = active.getBoundingClientRect();
      return { overflow: list.scrollWidth > list.clientWidth, scrollLeft: Math.round(list.scrollLeft), scrollWidth: list.scrollWidth, clientWidth: list.clientWidth, activeInView: a.left >= l.left - 1 && a.right <= l.right + 1, tabWidth: Math.round(a.width) };
    })()`);
  let geo = await strip();
  result.overflow = geo;
  check(geo.overflow, `the strip overflows with ${seen.tabs.length} tabs and scrolls (${JSON.stringify(geo)})`);
  check(geo.activeInView, "the active (last) tab is in view");
  const list = await box(".tab-strip .tab-list");
  const leftBefore = geo.scrollLeft;
  await wheel(list.cx, list.cy, -300);
  await wait(300);
  geo = await strip();
  check(geo.scrollLeft < leftBefore, `the wheel over the strip scrolls it sideways (${leftBefore} -> ${geo.scrollLeft})`);
  await press(digit(1));
  geo = await strip();
  // The list has 2px of padding for the active tab's outline, and the scroll puts the tab's edge (offsetLeft 2) at the left edge.
  check(geo.activeInView && geo.scrollLeft <= 2, `Ctrl+1 shows the first tab at the left end (${JSON.stringify(geo)})`);
  if (geo.scrollLeft > 0) {
    info(`nit: with the first tab active the strip rests ${geo.scrollLeft}px from its left end (the list's own padding, which holds the outline, is scrolled out of sight)`);
  }
  await press(digit(9));
  geo = await strip();
  check(geo.activeInView && geo.scrollLeft > 0, `Ctrl+9 scrolls the strip to the last tab (${JSON.stringify(geo)})`);
  const newTab = await look();
  check(newTab.newTab === true, "New tab is still enabled with 15 tabs");
  capture("overflow");
  // Back to the tabs there were.
  for (let i = 0; i < 12; i++) {
    await press("closeTab");
  }
  seen = await look();
  must(seen.tabs.length === baseCount, `the 12 extra tabs were closed (${seen.tabs.length} tabs left)`);
  await press(digit(baseActive + 1));
  await expectStrip(base, "the strip is as it was before the 12 tabs");
  geo = await strip();
  check(!geo.overflow, `the strip does not overflow any more (${JSON.stringify(geo)})`);

  // ----- the sidebar hidden ---------------------------------------------------------------------------------
  const focused = await focusInfo();
  info(`focus before Ctrl+\\: ${focused}`);
  await press("sidebar");
  await wait(500);
  const hidden = await page.evaluate(`(() => ({
    layout: document.querySelector(".layout").classList.contains("is-sidebar-hidden"),
    sidebarHidden: document.querySelector("nav.sidebar")?.hidden === true,
  }))()`);
  check(hidden.layout && hidden.sidebarHidden, `Ctrl+\\ hides the sidebar (${JSON.stringify(hidden)})`);
  const show = await box(".sidebar-show");
  const nav = await box(".tab-strip .tab-nav");
  must(show !== null && nav !== null, "the button that shows the sidebar and the strip's navigation are on screen");
  check(nav.x >= show.x + show.w - 1, `the strip leaves room for the button that shows the sidebar (button right edge ${Math.round(show.x + show.w)}, Back starts at ${Math.round(nav.x)})`);
  const stripBox = await box(".tab-strip");
  info(`strip with the sidebar hidden: x ${Math.round(stripBox.x)}, width ${Math.round(stripBox.w)}`);
  await expectStrip(base, "the tabs are unchanged with the sidebar hidden");
  capture("sidebar-hidden");
  await press("sidebar");
  await wait(500);
  const shown = await page.evaluate(`document.querySelector("nav.sidebar") !== null && document.querySelector("nav.sidebar").hidden === false && !document.querySelector(".layout").classList.contains("is-sidebar-hidden")`);
  check(shown, "Ctrl+\\ shows the sidebar again");

  // ----- the themes ---------------------------------------------------------------------------------------------
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor("input[name=theme]", "the theme choices");
  const original = await page.evaluate(`document.querySelector("input[name=theme]:checked")?.value ?? null`);
  info(`theme choice at the start: ${original}`);
  await expectStrip(base, "the strip stays in Settings");
  const settingsTitle = (await look()).h1;
  check(settingsTitle === "Settings", `Settings shows under the strip (${settingsTitle})`);
  const initialColours = await stripColours();
  result.colours = { initial: initialColours };

  const lookTheme = async (value, label, captureName) => {
    await chooseTheme(label);
    const applied = await page.evaluate(`document.documentElement.dataset.theme ?? "(none)"`);
    check(applied === value, `data-theme is ${value} (${applied})`);
    // A press on a tab shows the main screen again (Settings is not a tab).
    const wanted = baseSeen.tabs[baseActive].name;
    await clickTab(wanted);
    await expectStrip(base, `a press on the active tab leaves Settings (${value} theme)`, { title: wanted });
    await wait(400);
    const colours = await stripColours();
    result.colours[value] = colours;
    info(`${value}: ${JSON.stringify(colours)}`);
    check(colours.active !== null && colours.active.ratio >= 4.5, `${value}: the active tab's text has a contrast of at least 4.5 (${colours.active?.ratio})`);
    check(colours.inactive === null || colours.inactive.ratio >= 4.5, `${value}: an inactive tab's text has a contrast of at least 4.5 (${colours.inactive?.ratio})`);
    capture(captureName);
    return colours;
  };
  const light = await lookTheme("light", "Light", "theme-light");
  check(light.stripLuminance > 0.6, `the light theme's strip is light (luminance ${light.stripLuminance})`);
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  const dark = await lookTheme("dark", "Dark", "theme-dark");
  check(dark.stripLuminance < 0.25, `the dark theme's strip is dark (luminance ${dark.stripLuminance})`);
  // Back to the choice the pass started with.
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor("input[name=theme]", "the theme choices");
  const labelOf = { light: "Light", dark: "Dark", system: "Match system" };
  await clickOn(".radio", `the "${labelOf[original] ?? "Match system"}" theme`, { text: labelOf[original] ?? "Match system" });
  await wait(400);
  await clickTab(baseSeen.tabs[baseActive].name);
  await expectStrip(base, "the strip after restoring the theme choice");
}

async function phaseRestart() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `tabs-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__tabsMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  const expected = readState().snapshot;
  writeState({ marker, href });
  must(typeof expected === "string" && expected.length > 0, `the strip before the restart was recorded (${expected})`);
  const names = expected.split(" | ").map((s) => s.replace(/[\^*]+$/, ""));
  const expectedActive = names[expected.split(" | ").findIndex((s) => s.endsWith("*"))];
  // The order and the pinned tabs first (the phase cannot go on without them), then which tab is in front.
  const seen = await expectStrip(expected.replaceAll("*", ""), "the tabs came back in order, the pinned one pinned", { ms: 20000, ignoreActive: true });
  check(seen.tabs.filter((t) => t.pinned).length === expected.split("^").length - 1, "the pinned tabs are still pinned");
  const active = seen.tabs.find((t) => t.active);
  must(active !== undefined, "one tab is active");
  check(active.name === expectedActive, `the same tab is active as before the restart ("${expectedActive}" before, "${active.name}" after; strip before "${expected}")`);
  check(seen.h1 === active.name, `the active tab shows its page (${seen.h1})`);
  check(seen.back === false && seen.forward === false, `the active tab has a one-entry history: Back and Forward are disabled (${seen.back}, ${seen.forward})`);
  capture("restarted");
  // Every tab has a one-entry history too.
  for (const name of names) {
    await clickTab(name);
    const here = await look();
    check(here.back === false && here.forward === false, `the "${name}" tab has a one-entry history (Back ${here.back}, Forward ${here.forward})`);
    check(here.h1 === name, `the "${name}" tab shows "${name}" (${here.h1})`);
  }
  await clickTab(expectedActive);
  await expectStrip(expected, "back to the tab that was in front before the restart");
}

const PHASE_FUNCS = {
  launch: phaseLaunch,
  pages: phasePages,
  tabs: phaseTabs,
  keys: phaseKeys,
  mouse: phaseMouse,
  scroll: phaseScroll,
  drag: phaseDrag,
  views: phaseViews,
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

// What the strip showed when the phase ended: the next phase starts from it, and the restart pass compares to it.
try {
  const end = await look();
  result.strip = text(end);
  if (phase !== "restart") {
    writeState({ snapshot: result.strip });
  }
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
// The report file has every note; the console has what failed (or, with TABS_VERBOSE, everything).
const checks = result.notes.filter((n) => !n.startsWith("info:"));
console.log(
  JSON.stringify(
    {
      phase,
      ok: result.ok,
      strip: result.strip,
      checks: checks.length,
      failed: checks.filter((n) => n.startsWith("FAIL")),
      captures: result.captures,
      ...(process.env.TABS_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
page.close();
process.exit(result.ok ? 0 : 1);
