// The in-page half of the import / export pass (drive-interop.ps1, ADR-0036): trusted mouse and key input, the DOM, the
// core (through the page's own IPC) and the files written, over the DevTools port of the app's WebView2 (see cdp.mjs).
// Phases:
//
//   node drive-interop.mjs launch        <outfile>  the document's marker is set; the space is empty and has a recovery kit
//   node drive-interop.mjs seed          <outfile>  "Source": paragraph, heading, to-do list, an image, a table view with two pages inside
//                                                  (a select property each)
//   node drive-interop.mjs fixtures      <outfile>  out/interop-src: Export-abc.zip (Notion style), notes/ (Markdown), loose/list.csv
//   node drive-interop.mjs import-zip    <outfile>  space menu -> Import... -> .zip -> report -> Open -> the tree, the pages
//   node drive-interop.mjs import-folder <outfile>  page menu -> Import into this page... -> folder -> report, properties, wiki links
//   node drive-interop.mjs import-files  <outfile>  space menu -> Import... -> files (quoted names) -> a database page
//   node drive-interop.mjs export-zip    <outfile>  "Source": Export... -> Markdown + sub-pages -> a .zip, read back with System.IO.Compression
//   node drive-interop.mjs reimport      <outfile>  that .zip imported again and compared with the original
//   node drive-interop.mjs export-html   <outfile>  a page without attachments as one standalone .html file
//   node drive-interop.mjs export-space  <outfile>  Export space... -> Markdown: every page in one .zip
//   node drive-interop.mjs csv           <outfile>  the imported Tasks database: view settings -> Export CSV
//   node drive-interop.mjs print         <outfile>  Ctrl+P: print mode and the webview's print preview
//   node drive-interop.mjs backup        <outfile>  Settings -> Scheduled backups: three runs, keep 2
//   node drive-interop.mjs guard         <outfile>  SettingsSet of app.backup.folder through the page's IPC is refused
//   node drive-interop.mjs cancel        <outfile>  every native dialog cancelled (Import's three sources, both Exports, the CSV, the backup folder)
//   node drive-interop.mjs import-hostile <outfile> a zip with zip-slip and absolute names, a nested zip, scripts and remote pictures
//   node drive-interop.mjs verify        <outfile>  the summary of every phase and the scan of the log
//
// Every phase writes what the page asked the network for, what failed, the console and the content-security-policy
// violations it saw, as JSON, and exits 1 when its own check fails, when the page asked another host for something,
// when a violation, a failed request or a console error was seen, or when the document of the webview was replaced
// (the marker `window.__interopMarker` set by `launch` is lost by a reload, and `location.href` must not change either).
//
// The native file dialogs are driven by drive-interop.ps1's helper steps (Win32: WM_SETTEXT on the file name edit,
// BM_CLICK on IDOK; never SendKeys). The page is asked to open one by a trusted click over the DevTools port; the
// helper then waits for the dialog, types the path and closes it.

import { execFile } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { deflateSync, crc32 } from "node:zlib";

const CDP_PORT = Number(process.env.CDP_PORT ?? 9333);

/**
 * A DevTools client for one page target of the WebView2 (the same calls as cdp.mjs: trusted input, evaluation, a record of
 * the network, the console and the violations), but the target is chosen by the address, not "the first page": the print
 * preview (edge://print/) and the downloads flyout that "Save as PDF" opens are page targets of their own, and
 * cdp.mjs's connect() would attach to whichever comes first in the list.
 */
async function connect(accept = (url) => url.startsWith("http://tauri.localhost") || url.startsWith("https://tauri.localhost") || url.startsWith("tauri://localhost"), { record = true } = {}) {
  let target;
  for (let i = 0; i < 60 && target === undefined; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json`)).json();
      target = list.find((t) => t.type === "page" && accept(t.url));
    } catch {
      // not up yet
    }
    if (target === undefined) {
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
  if (target === undefined) {
    throw new Error("no matching page target on the DevTools port");
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  let id = 0;
  const waiting = new Map();
  const events = [];
  ws.addEventListener("message", (message) => {
    const data = JSON.parse(String(message.data));
    if (data.id !== undefined) {
      const entry = waiting.get(data.id);
      waiting.delete(data.id);
      if (data.error) {
        entry?.reject(new Error(`${data.error.message}`));
      } else {
        entry?.resolve(data.result);
      }
    } else if (record) {
      events.push(data);
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const n = ++id;
      waiting.set(n, { resolve, reject });
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  if (record) {
    await send("Network.enable");
    await send("Log.enable");
  }
  await send("Runtime.enable");
  await send("Page.enable");
  return {
    target,
    send,
    events,
    close: () => ws.close(),
    async evaluate(expression) {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails) {
        throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
      }
      return result.result.value;
    },
    async click(x, y, button = "left", clickCount = 1) {
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
      await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, buttons: 1, clickCount });
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, buttons: 0, clickCount });
    },
    /** Types text as key events (what a keyboard produces). */
    async type(text) {
      for (const ch of text) {
        await send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
      }
    },
    async key(key, code = key, windowsVirtualKeyCode = 0, modifiers = 0) {
      await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode, modifiers });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode, modifiers });
    },
    /** What the page asked the network for, failed, printed and reported so far. */
    report() {
      const requests = [];
      const failures = [];
      const console_ = [];
      for (const e of events) {
        if (e.method === "Network.requestWillBeSent") {
          requests.push({ url: e.params.request.url, type: e.params.type });
        } else if (e.method === "Network.loadingFailed") {
          failures.push({ requestId: e.params.requestId, error: e.params.errorText, blocked: e.params.blockedReason ?? null });
        } else if (e.method === "Runtime.consoleAPICalled") {
          console_.push({ type: e.params.type, text: e.params.args.map((a) => a.value ?? a.description ?? "").join(" ").slice(0, 300) });
        } else if (e.method === "Log.entryAdded") {
          console_.push({ type: `log:${e.params.entry.level}:${e.params.entry.source}`, text: String(e.params.entry.text).slice(0, 300), url: e.params.entry.url });
        } else if (e.method === "Runtime.exceptionThrown") {
          console_.push({ type: "exception", text: String(e.params.exceptionDetails.exception?.description ?? e.params.exceptionDetails.text).slice(0, 300) });
        }
      }
      return { requests, failures, console: console_ };
    },
  };
}

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.INTEROP_OUT_DIR ?? join(HERE, "out");
const STATE_FILE = join(OUT, "interop-state.json");
const SRC = join(OUT, "interop-src");
const EXPORT_DIR = join(OUT, "interop-export");
const BACKUP_DIR = join(OUT, "interop-backups");
const DATA_DIR = join(OUT, "interop-data");
const ERR_LOG = join(OUT, "interop.err.log");
const PHASES = [
  "launch",
  "seed",
  "fixtures",
  "import-zip",
  "import-folder",
  "import-files",
  "export-zip",
  "reimport",
  "export-html",
  "export-space",
  "csv",
  "print",
  "backup",
  "guard",
  "cancel",
  "import-hostile",
];
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(OUT, { recursive: true });

/**
 * What must never appear in the log of the shell and the core (ADR-0036: paths and names are never logged): the titles of the
 * pages this pass makes, the names of the files it reads and writes, the folders it uses, and some of the text it types or
 * imports. A whole-word match, so "Tasks" is not a hit in "SubTasks" (it is in "the Tasks page").
 */
const PRIVACY_NEEDLES = [
  "Source", "Alpha task", "Beta task", "Priority", "Inbox", "Getting Started", "Sub page", "Tasks", "Write docs", "Ship it",
  "Project Alpha", "Meeting Notes", "Loose note", "Export-abc", "list.csv", "loose.md", "pic.png", "diagram.png", "sample.png",
  "source.zip", "space.zip", "alpha.html", "tasks.csv", "interop-src", "interop-export", "interop-backups", "interop-print", "atomik-kb-auto",
  "Interop workspace", "A paragraph in the source page", "Buy milk", "Call Ann", "Draft the docs", "Welcome to the import test",
  "Kickoff notes", "Agenda for the weekly meeting", "Just a loose file", "Release the build", "notes", "owner",
  "Fine page", "Scripted", "Inner page", "hostile", "hostile.zip", "Part-1.zip", "escape-one", "pixel.png", "track.png", "example.com",
];

function readState() {
  return existsSync(STATE_FILE) ? JSON.parse(readFileSync(STATE_FILE, "utf8")) : {};
}
function writeState(patch) {
  writeFileSync(STATE_FILE, JSON.stringify({ ...readState(), ...patch }, null, 2));
}
const round = (n) => Math.round(n * 10) / 10;

// ---------------------------------------------------------------------------------------------------------------
// verify: the summary of the phase reports and the scan of the log (no app needed)
// ---------------------------------------------------------------------------------------------------------------
if (phase === "verify") {
  verify();
  process.exit(0);
}

function verify() {
  const summary = {
    ok: true,
    phases: {},
    captures: [],
    failedNotes: [],
    foreignRequests: 0,
    cspViolations: 0,
    failedRequests: 0,
    consoleErrors: 0,
    details: {},
    bugs: [],
    log: null,
  };
  for (const name of PHASES) {
    const file = join(OUT, `interop-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = {
      ok: report.ok,
      checks: report.notes.filter((n) => !n.startsWith("info:")).length,
      failed: report.notes.filter((n) => n.startsWith("FAIL")).length,
      seconds: report.seconds,
    };
    summary.ok = summary.ok && report.ok;
    summary.captures.push(...(report.captures ?? []));
    const detail = {};
    for (const key of ["importZip", "importFolder", "importFiles", "exportZip", "exportSpace", "reimport", "print", "backup", "importHostile"]) {
      if (report[key] !== undefined) detail[key] = report[key];
    }
    if (Object.keys(detail).length > 0) summary.details[name] = detail;
    summary.failedNotes.push(...report.notes.filter((n) => n.startsWith("FAIL")).map((n) => `${name}: ${n}`));
    summary.bugs.push(...(report.bugs ?? []).map((b) => `${name}: ${b}`));
    summary.foreignRequests += report.foreignRequests?.length ?? 0;
    summary.cspViolations += report.cspViolations?.length ?? 0;
    summary.failedRequests += report.failures?.length ?? 0;
    summary.consoleErrors += (report.console ?? []).filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).length;
  }
  // The log of the shell and the core: no panic, no ERROR, and nothing from the person's files (names, paths).
  const log = { present: existsSync(ERR_LOG), lines: 0, panics: [], errors: [], warnings: [], leaks: [] };
  if (log.present) {
    const text = readFileSync(ERR_LOG, "utf8");
    // The ANSI colours of tracing's formatter are not part of what the log says.
    const lines = text.split(/\r?\n/).map((l) => l.replace(/\u001b\[[0-9;]*m/g, ""));
    log.lines = lines.length;
    for (const line of lines) {
      if (/panicked|panic:|thread '.*' panicked/i.test(line)) log.panics.push(line.slice(0, 300));
      if (/(^|\s)ERROR(\s|$)/.test(line)) log.errors.push(line.slice(0, 300));
      if (/(^|\s)WARN(\s|$)/.test(line)) log.warnings.push(line.slice(0, 300));
    }
    const state = readState();
    const needles = [...new Set([...PRIVACY_NEEDLES, ...(state.privacyNeedles ?? [])])];
    for (const needle of needles) {
      // A whole word or name: "Source" is not a hit in "ImportSource".
      const escaped = needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const pattern = new RegExp(`(?<![A-Za-z0-9_])${escaped}(?![A-Za-z0-9_])`);
      const hits = lines.filter((l) => pattern.test(l));
      if (hits.length > 0) log.leaks.push({ needle, count: hits.length, first: hits[0].slice(0, 300) });
    }
    log.needles = needles.length;
  }
  summary.log = log;
  summary.ok = summary.ok && log.present && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0;
  writeFileSync(outFile, JSON.stringify(summary, null, 2));
  console.log(
    JSON.stringify(
      {
        phase: "verify",
        ok: summary.ok,
        phases: summary.phases,
        failedNotes: summary.failedNotes,
        bugs: summary.bugs,
        log: { present: log.present, lines: log.lines, panics: log.panics, errors: log.errors, warnings: log.warnings.slice(0, 40), leaks: log.leaks, needles: log.needles },
      },
      null,
      2,
    ),
  );
  process.exitCode = summary.ok ? 0 : 1;
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

const started = Date.now();
const result = { phase, ok: true, notes: [], captures: [], bugs: [] };
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
/** A thing the app does that it should not (kept for the final report, besides the failed check that found it). */
const bug = (note) => result.bugs.push(note);

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
async function box(selector, text = null, scroll = false) {
  return page.evaluate(`(() => {
    const wanted = ${JSON.stringify(text)};
    const candidates = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const el = wanted === null ? candidates[0] : candidates.find((e) => e.textContent.trim() === wanted);
    if (!el) return null;
    if (${scroll}) el.scrollIntoView({ block: "nearest", inline: "nearest" });
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })()`);
}

async function waitFor(selector, what, ms = 20000, text = null, scroll = false) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const found = await box(selector, text, scroll);
    if (found !== null && found.w > 0 && found.h > 0) {
      return found;
    }
    await wait(200);
  }
  must(false, `${what} did not appear (${selector}${text === null ? "" : ` with the text "${text}"`})`);
  return null;
}

/** Waits until `expression` (evaluated in the page) is truthy; returns its last value. */
async function waitUntil(expression, what, ms = 20000) {
  const end = Date.now() + ms;
  let last = null;
  while (Date.now() < end) {
    last = await page.evaluate(expression).catch(() => null);
    if (last) {
      return last;
    }
    await wait(250);
  }
  must(false, `${what} (timed out after ${ms / 1000} s)`);
  return last;
}

async function clickOn(selector, what, options = {}) {
  // Scrolled into view first (a button at the bottom of a tall dialog).
  const found = await waitFor(selector, what, 20000, options.text ?? null, true);
  await wait(100);
  const settled = (await box(selector, options.text ?? null, true)) ?? found;
  await mouse(settled.cx, settled.cy, options);
  return settled;
}
/** The title shown in the main area. */
const heading = () => page.evaluate(`document.querySelector(".content-body h1")?.textContent.trim() ?? null`);

/** Captures the window with PrintWindow (capture.ps1's Capture-Window, run in PowerShell). */
function capture(name) {
  const path = join(OUT, `interop-${name}.png`);
  return new Promise((resolvePromise) => {
    execFile(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        '. $env:INTEROP_CAPTURE_PS1; $p = Get-Process -Name atomik-kb-desktop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1; Capture-Window $p.MainWindowHandle $env:INTEROP_CAPTURE_PATH',
      ],
      { env: { ...process.env, INTEROP_CAPTURE_PATH: path }, timeout: 40000 },
      (error) => {
        if (error) {
          check(false, `capture ${name} failed: ${error.message}`);
        } else {
          result.captures.push(path);
        }
        resolvePromise();
      },
    );
  });
}

/** Leaves nothing open from an earlier phase that failed half-way. */
async function normalize() {
  for (let i = 0; i < 4; i++) {
    const open = await page.evaluate(`!!document.querySelector('[role="menu"], [role="dialog"], [role="listbox"]')`);
    if (!open) {
      return;
    }
    await page.key("Escape", "Escape", 27);
    await wait(250);
  }
}

// ----- the helpers of the .ps1 (native dialogs, zip files, windows) ------------------------------------------------

let helperCount = 0;
/** Runs one of drive-interop.ps1's helper steps; its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}) {
  helperCount += 1;
  const argFile = join(OUT, `interop-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `interop-helper-${process.pid}-${helperCount}.result.json`);
  writeFileSync(argFile, JSON.stringify(params));
  rmSync(resultFile, { force: true });
  return new Promise((resolvePromise, reject) => {
    execFile(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", process.env.INTEROP_PS1, "-Step", step, "-Arg", argFile, "-Result", resultFile],
      { timeout: 180000, maxBuffer: 16 * 1024 * 1024 },
      (error, stdout, stderr) => {
        rmSync(argFile, { force: true });
        if (!existsSync(resultFile)) {
          reject(new Error(`helper ${step} wrote no result (${error?.message ?? "exit 0"}): ${String(stdout).slice(0, 400)} ${String(stderr).slice(0, 400)}`));
          return;
        }
        const answer = JSON.parse(readFileSync(resultFile, "utf8").replace(/^﻿/, ""));
        rmSync(resultFile, { force: true });
        resolvePromise(answer);
      },
    );
  });
}

/** Drives a native file dialog that the page has just asked for: types `steps` (one text, or several for a multi-select) and closes it. */
async function fileDialog(title, steps) {
  const answer = await helper("dialog", { title, steps: Array.isArray(steps) ? steps : [steps] });
  info(`dialog "${title}": waited ${answer.waitedMs} ms, edit control ${answer.edit}${answer.initial ? `, proposed "${answer.initial}"` : ""}, ${answer.confirmed ? "overwrite confirmed, " : ""}${answer.closed ? "closed" : "STILL OPEN"}`);
  check(answer.ok === true, `the "${title}" dialog took "${(Array.isArray(steps) ? steps : [steps]).join(" | ")}" and closed${answer.error ? ` (${answer.error}; seen ${JSON.stringify(answer.seen)}; controls ${JSON.stringify(answer.controls)})` : ""}`);
  return answer;
}

/** The entries and the texts of a .zip, read with System.IO.Compression. */
async function readZip(path) {
  const answer = await helper("zip-read", { zip: path });
  must(answer.ok === true, `System.IO.Compression opens ${path.split(/[\\/]/).pop()} (${answer.error ?? "ok"})`);
  const entries = Array.isArray(answer.entries) ? answer.entries : answer.entries ? [answer.entries] : [];
  return { entries, names: entries.map((e) => e.name), texts: answer.texts ?? {}, heads: answer.heads ?? {} };
}

// ----- the core, through the page's own IPC -------------------------------------------------------------------------

/** One request to the core as the UI sends it (`invoke("dispatch", { request })`): `{ ok, value }` or `{ ok: false, error }`. */
async function core(request) {
  // A request without fields (`SpaceList`, `SecurityStatus`) is a unit variant: it has no `data`.
  if (request.data !== undefined && Object.keys(request.data).length === 0) {
    request = { type: request.type };
  }
  return page.evaluate(`(async () => {
    try {
      const value = await window.__TAURI_INTERNALS__.invoke("dispatch", { request: ${JSON.stringify(request)} });
      return { ok: true, value };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? { message: e.message } : e };
    }
  })()`);
}
/** The data of a successful answer (the `Response` is `{ type, data }`). */
async function ask(request) {
  const answer = await core(request);
  if (!answer.ok) {
    must(false, `${request.type} answered (${JSON.stringify(answer.error ?? "").slice(0, 300)})`);
  }
  return answer.value.data;
}

async function spaceInfo() {
  const spaces = await ask({ type: "SpaceList", data: {} });
  return spaces[0];
}
async function treeOf(spaceId) {
  return ask({ type: "TreeGet", data: { spaceId } });
}
/** Every live page of a tree, depth first, with its path of titles. */
function flatten(nodes, trail = []) {
  const out = [];
  for (const node of nodes) {
    const path = [...trail, node.page.title];
    out.push({ id: node.page.id, title: node.page.title, path, depth: trail.length, parentId: node.page.parentId ?? null, node });
    out.push(...flatten(node.children, path));
  }
  return out;
}
const livePages = (tree) => flatten(tree.roots);

// ----- DOM helpers -----------------------------------------------------------------------------------------------

/** The menu item whose text starts with `label` (a hint such as "Ctrl+P" follows the label in its text). */
async function menuItem(label) {
  return page.evaluate(`(() => {
    const el = [...document.querySelectorAll('[role="menuitem"]')].find((e) => e.textContent.trim().startsWith(${JSON.stringify(label)}));
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2, disabled: el.disabled === true, text: el.textContent.trim() };
  })()`);
}

async function chooseMenuItem(label) {
  let found = null;
  const end = Date.now() + 10000;
  while (Date.now() < end && found === null) {
    found = await menuItem(label);
    if (found === null) await wait(200);
  }
  if (found === null) must(false, `the menu has "${label}"`);
  if (found.disabled) must(false, `"${label}" is enabled`);
  await mouse(found.cx, found.cy);
}

/** Opens the space menu ("Space options") and chooses `label`. */
async function spaceMenu(label) {
  await normalize();
  await clickOn('button[aria-label="Space options"]', "the space options button");
  await waitFor('[role="menu"]', "the space options menu");
  await chooseMenuItem(label);
}

/** Opens the page menu ("Page options") of the open page and chooses `label`. */
async function pageMenu(label) {
  await normalize();
  await clickOn('button[aria-label="Page options"]', "the page options button");
  await waitFor('[role="menu"]', "the page options menu");
  await chooseMenuItem(label);
}

const dialogTitle = () => page.evaluate(`document.querySelector('[role="dialog"] .subtitle')?.textContent.trim() ?? null`);

/** A button of the open dialog, by its text. */
async function dialogButton(text) {
  return box('[role="dialog"] button', text);
}

/**
 * Opens a page by clicking its row in the sidebar. The rows are a flat list (`aria-level`); the ancestors of the page
 * are expanded first (their chevrons clicked), the page is found by its id (`data-page-id`).
 */
async function openPageId(spaceId, id) {
  await normalize();
  const tree = await treeOf(spaceId);
  const target = livePages(tree).find((p) => p.id === id);
  if (target === undefined) must(false, `the page ${id} is in the tree`);
  const chain = [];
  for (let p = target; p !== undefined; p = livePages(tree).find((q) => q.id === p.parentId)) {
    chain.unshift(p.id);
  }
  for (const [index, pageId] of chain.entries()) {
    const row = await page.evaluate(`(() => {
      const el = document.querySelector('[role="treeitem"][data-page-id="${pageId}"]');
      if (!el) return null;
      const t = (el.querySelector(".tree-title") ?? el).getBoundingClientRect();
      const c = el.querySelector("button.tree-chevron")?.getBoundingClientRect() ?? null;
      el.scrollIntoView({ block: "nearest" });
      return { cx: t.x + t.width / 2, cy: t.y + t.height / 2, expanded: el.getAttribute("aria-expanded"), chevron: c ? { cx: c.x + c.width / 2, cy: c.y + c.height / 2 } : null };
    })()`);
    if (row === null) must(false, `the sidebar has the row of "${livePages(tree).find((p) => p.id === pageId)?.title}" (${index + 1} of ${chain.length})`);
    if (index < chain.length - 1) {
      if (row.expanded === "false" && row.chevron !== null) {
        await mouse(row.chevron.cx, row.chevron.cy);
        await wait(300);
      }
    } else {
      // The row may have moved when the ancestors expanded: measured again.
      const again = await box(`[role="treeitem"][data-page-id="${pageId}"] .tree-title`);
      await mouse(again.cx, again.cy);
    }
  }
  await wait(700);
  await waitFor(".content-body h1", "the title of the opened page");
}

// ----- typing in the editor ----------------------------------------------------------------------------------------

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
 */
async function slash(query, label) {
  await page.type(`/${query}`);
  await wait(500);
  await waitFor("[role=listbox]", `the slash menu for /${query}`);
  for (let i = 0; i < 25; i++) {
    const seen = await page.evaluate(`(() => {
      const options = [...document.querySelectorAll("[role=option]")];
      const title = (o) => o.querySelector(".suggestion-title")?.textContent.trim() ?? "";
      return { selected: title(options.find((o) => o.getAttribute("aria-selected") === "true") ?? options[0] ?? document.body), all: options.map(title) };
    })()`);
    if (!seen.all.includes(label)) {
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
}

/** Creates a page at the top of the sidebar with the "New page" button and names it. */
async function newPage(title) {
  await clickOn(".tree-new", `the sidebar's "New page" button`);
  await waitFor("input.tree-rename", "the rename field of the new page");
  await send("Input.insertText", { text: title });
  await page.key("Enter", "Enter", 13);
  await waitFor(`[role="treeitem"][aria-label=${JSON.stringify(title)}]`, `the row "${title}"`);
  await wait(500);
  await waitFor(".ProseMirror", "the page editor");
  must((await heading()) === title, `the new page "${title}" is open (${await heading()})`);
}

// ----- files -----------------------------------------------------------------------------------------------------------

/** A PNG of `size` x `size` pixels in two colours (a blue square with a gold disc), written with node's zlib. */
function pngBytes(size = 48, blue = [60, 110, 220], gold = [255, 200, 40]) {
  const raw = Buffer.alloc((size * 3 + 1) * size);
  const c = size / 2;
  for (let y = 0; y < size; y++) {
    raw[y * (size * 3 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      const inside = (x - c) ** 2 + (y - c) ** 2 < (size / 3) ** 2;
      const px = inside ? gold : blue;
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
  header[8] = 8; // bit depth
  header[9] = 2; // RGB
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

function write(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

/** Every file under a folder, relative paths with "/". */
function walk(root, base = root) {
  const out = [];
  for (const name of readdirSync(root)) {
    const full = join(root, name);
    if (statSync(full).isDirectory()) out.push(...walk(full, base));
    else out.push(full.slice(base.length + 1).replaceAll("\\", "/"));
  }
  return out;
}

// ---------------------------------------------------------------------------------------------------------------
// phases
// ---------------------------------------------------------------------------------------------------------------


async function phaseLaunch() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  rmSync(STATE_FILE, { force: true }); // a fresh pass starts with no memory of an earlier one
  const marker = `interop-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__interopMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href });
  const size = await page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio, theme: document.documentElement.dataset.theme ?? "(none)" })`);
  info(`window ${size.w}x${size.h} at ${size.dpr}x, data-theme ${size.theme}`);
  check((await heading()) === "Interop workspace", `the main area shows the workspace's Home (${await heading()})`);
  const space = await spaceInfo();
  must(space !== undefined, "the workspace has a space");
  writeState({ spaceId: space.id, spaceName: space.name });
  info(`the space is "${space.name}"`);
  const tree = await treeOf(space.id);
  check(tree.roots.length === 0, `the space starts empty (${tree.roots.length} pages)`);
  // A recovery kit exists: no banner offers one, and the core says the kit is verified.
  const banner = await box(".home-banners .banner");
  check(banner === null, "no recovery-kit banner on Home (the kit was made and checked)");
  const security = await ask({ type: "SecurityStatus", data: {} });
  check(typeof security.recoveryKit?.verifiedAt === "number" && security.recoveryKit.stale === false, `the recovery kit exists, was checked and is not stale (${JSON.stringify(security.recoveryKit)})`);
}

// ----- phase: seed -------------------------------------------------------------------------------------------------

const SOURCE_TITLE = "Source";
const CHILDREN = [
  { title: "Alpha task", priority: "High" },
  { title: "Beta task", priority: "Low" },
];
const SAMPLE_PNG = join(SRC, "sample.png");

/** Adds a select property value to the open page (creating the property the first time); the value's option is created when new. */
async function giveSelectValue(name, value, { create }) {
  await clickOn("button.properties-add", "the Add a property button");
  await waitFor('[role="menu"]', "the add-property menu");
  if (create) {
    await chooseMenuItem("New property");
    await waitFor('[role="dialog"] input[type="text"]', "the new property dialog");
    await clickOn('[role="dialog"] input[type="text"]', "the property name field");
    await send("Input.insertText", { text: name });
    const kind = await page.evaluate(`(() => {
      const l = [...document.querySelectorAll('[role="dialog"] label.new-property-kind')].find((e) => e.querySelector(".new-property-kind-name")?.textContent.trim() === "Select");
      if (!l) return null;
      const r = l.getBoundingClientRect();
      return { cx: r.x + 20, cy: r.y + r.height / 2 };
    })()`);
    must(kind !== null, "the new property dialog offers the Select kind");
    await mouse(kind.cx, kind.cy);
    const checked = await page.evaluate(`document.querySelector('[role="dialog"] input[aria-label="Select"]')?.checked ?? null`);
    must(checked === true, "the Select kind is chosen");
    await clickOn('[role="dialog"] button', "Create property", { text: "Create property" });
    await waitUntil(`!document.querySelector('[role="dialog"]')`, "the new property dialog closed");
  } else {
    await chooseMenuItem(name);
  }
  // The value: the chip of the new row (its picker may already be open).
  const label = `${name}: value`;
  if ((await box('input[aria-label="Filter or create an option"]')) === null) {
    await clickOn(`button[aria-label=${JSON.stringify(label)}]`, `the value chip of "${name}"`);
  }
  await clickOn('input[aria-label="Filter or create an option"]', "the option filter field");
  await send("Input.insertText", { text: value });
  await wait(300);
  const option = await page.evaluate(`(() => {
    const el = [...document.querySelectorAll('[role="option"]')].find((e) => e.textContent.trim() === ${JSON.stringify(value)} || e.textContent.includes("Create"));
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { cx: r.x + r.width / 2, cy: r.y + r.height / 2, text: el.textContent.trim() };
  })()`);
  must(option !== null, `the picker offers "${value}" or creating it`);
  await mouse(option.cx, option.cy);
  await waitUntil(`(() => { const c = document.querySelector('button[aria-label=${JSON.stringify(label)}]'); return !!c && c.textContent.trim() === ${JSON.stringify(value)}; })()`, `"${name}" shows "${value}"`);
}

async function phaseSeed() {
  const state = readState();
  write(SAMPLE_PNG, pngBytes(48));
  await newPage(SOURCE_TITLE);

  // A paragraph, a heading, a to-do list.
  await newLine();
  await send("Input.insertText", { text: "A paragraph in the source page. It carries no markup of its own, only text." });
  await newLine();
  await slash("heading", "Heading 1");
  await send("Input.insertText", { text: "A heading in the source page" });
  await newLine();
  await slash("to-do", "To-do list");
  await send("Input.insertText", { text: "Buy milk" });
  await page.key("Enter", "Enter", 13);
  await send("Input.insertText", { text: "Call Ann" });
  await wait(300);
  const boxes = await page.evaluate(`document.querySelectorAll('.ProseMirror input[type="checkbox"]').length`);
  must(boxes >= 2, `the to-do list has its two items (${boxes} checkboxes)`);
  const first = await box('.ProseMirror input[type="checkbox"]');
  await mouse(first.cx, first.cy);

  // An image through the "Image" slash command and the native Open dialog.
  await newLine();
  await slash("image", "Image");
  await fileDialog("Open", SAMPLE_PNG);
  const loaded = await waitUntil(`(() => { const i = document.querySelector(".ProseMirror img"); return i && i.complete && i.naturalWidth > 0 ? i.naturalWidth : 0; })()`, "the image block shows the picture", 30000);
  info(`the image block is ${loaded} px wide`);

  // A table view of the pages inside this one.
  await newLine();
  await slash("table", "Table view");
  must((await box('.ProseMirror [data-type="pageList"][data-layout="table"]')) !== null, "the page has a table view");
  await newLine();
  await send("Input.insertText", { text: "A closing paragraph after the table view." });
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(2000);

  const space = state.spaceId;
  const tree = await treeOf(space);
  const source = livePages(tree).find((p) => p.title === SOURCE_TITLE);
  must(source !== undefined, "the core has the page Source");

  // Two pages inside "Source" (the sidebar's "Add a page inside"), each with a Priority select value.
  const ids = [];
  for (const [index, child] of CHILDREN.entries()) {
    const row = await waitFor(`[role="treeitem"][data-page-id="${source.id}"] .tree-title`, 'the row "Source"');
    await hover(row.cx, row.cy);
    const add = await waitFor(`[role="treeitem"][data-page-id="${source.id}"] button[aria-label="Add a page inside"]`, 'the "Add a page inside" button');
    await mouse(add.cx, add.cy);
    await waitFor("input.tree-rename", "the rename field of the new page");
    await send("Input.insertText", { text: child.title });
    await page.key("Enter", "Enter", 13);
    await wait(900);
    const now = livePages(await treeOf(space)).find((p) => p.title === child.title);
    must(now !== undefined, `the page "${child.title}" exists`);
    ids.push(now.id);
    await openPageId(space, now.id);
    must((await heading()) === child.title, `"${child.title}" is open (${await heading()})`);
    await giveSelectValue("Priority", child.priority, { create: index === 0 });
  }
  writeState({
    sourceId: source.id,
    childIds: ids,
    privacyNeedles: ["Source", "Alpha task", "Beta task", "Priority", "A paragraph in the source page", "Buy milk", "Call Ann", "sample.png", "interop-src", "interop-export", "interop-backups"],
  });

  // The page as it is: open Source and look at it.
  await openPageId(space, source.id);
  await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row', "the table view's rows");
  const rows = await page.evaluate(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row').length`);
  check(rows === 2, `the table view lists its two pages (${rows})`);
  const props = await ask({ type: "PagePropertiesGet", data: { pageId: ids[0] } });
  check(Array.isArray(props.values) && props.values.length === 1, `the first page has one property value (${JSON.stringify(props.values)})`);
  await capture("seed-source");
}

// ----- phase: fixtures ---------------------------------------------------------------------------------------------

/** 32 lowercase hex digits from a seed (a Notion id). */
const id32 = (seed) => createHash("md5").update(seed).digest("hex");
const HOME_ID = "0123456789abcdef0123456789abcdef";
const SUB_ID = id32("sub");
const DB_ID = id32("tasks");
const ROW1_ID = id32("row1");
const ROW2_ID = id32("row2");
const NOTION_ZIP = join(SRC, "Export-abc.zip");
const NOTES_DIR = join(SRC, "notes");
const LOOSE_DIR = join(SRC, "loose");
const enc = (name) => encodeURIComponent(name).replaceAll("'", "%27");

async function phaseFixtures() {
  for (const target of [join(SRC, "notion-tree"), NOTES_DIR, LOOSE_DIR, NOTION_ZIP]) {
    rmSync(target, { recursive: true, force: true });
  }
  mkdirSync(SRC, { recursive: true });

  // (a) A Notion "Markdown & CSV" export: no top-level folder, ids in every name, the pages' folder next to its file.
  const tree = join(SRC, "notion-tree");
  const home = `Getting Started ${HOME_ID}`;
  write(
    join(tree, `${home}.md`),
    [
      "# Getting Started",
      "",
      `Welcome to the import test. Read the [Sub page](${enc(home)}/${enc(`Sub page ${SUB_ID}.md`)}) first, then open the [Tasks](${enc(`Tasks ${DB_ID}.csv`)}) database.`,
      "",
      `![pic](${enc(home)}/pic.png)`,
      "",
      "A closing line with **bold** text.",
      "",
    ].join("\n"),
  );
  write(
    join(tree, home, `Sub page ${SUB_ID}.md`),
    ["# Sub page", "", `This page lives inside Getting Started. It has a [link back](../${enc(`${home}.md`)}) and some **bold** text.`, ""].join("\n"),
  );
  write(join(tree, home, "pic.png"), pngBytes(40, [200, 60, 90], [250, 250, 250]));
  const csv = ["Name,Status,Due,Done", 'Write docs,In progress,"October 7, 2026",No', 'Ship it,Done,"October 8, 2026",Yes', ""].join("\r\n");
  write(join(tree, `Tasks ${DB_ID}.csv`), csv);
  write(join(tree, `Tasks ${DB_ID}_all.csv`), csv);
  write(
    join(tree, `Tasks ${DB_ID}`, `Write docs ${ROW1_ID}.md`),
    ["# Write docs", "", "Status: In progress", "Due: October 7, 2026", "Done: No", "", "Draft the docs for the import feature.", ""].join("\n"),
  );
  write(
    join(tree, `Tasks ${DB_ID}`, `Ship it ${ROW2_ID}.md`),
    ["# Ship it", "", "Status: Done", "Due: October 8, 2026", "Done: Yes", "", "Release the build.", ""].join("\n"),
  );
  const built = await helper("zip-build", { dir: tree, zip: NOTION_ZIP });
  must(built.ok === true, "System.IO.Compression wrote Export-abc.zip");
  const listed = await readZip(NOTION_ZIP);
  check(listed.names.length === 7, `Export-abc.zip holds the 7 files of the Notion layout (${listed.names.length}: ${listed.names.join(" | ")})`);
  check(listed.names.every((n) => !n.includes("\\") && !n.startsWith("/")), "its entries use forward slashes and no top-level folder");
  check(listed.names.some((n) => n === `${home}.md`) && listed.names.some((n) => n.endsWith(`${enc(home)}`) === false && n.includes(`Tasks ${DB_ID}_all.csv`)), "the page and the _all.csv database are in it");

  // (b) A Markdown folder (Obsidian style): front matter, a wiki link between two files, an image in img/.
  write(
    join(NOTES_DIR, "Project Alpha.md"),
    ["---", "title: Project Alpha", "tags: [work, alpha]", "owner: Ben", "rating: 4", "---", "# Project Alpha", "", "Kickoff notes for the project. See [[Meeting Notes]] for the agenda.", "", "![diagram](img/diagram.png)", ""].join("\n"),
  );
  write(join(NOTES_DIR, "Meeting Notes.md"), ["---", "title: Meeting Notes", "tags: meetings, weekly", "---", "Agenda for the weekly meeting. Back to [[Project Alpha]].", ""].join("\n"));
  write(join(NOTES_DIR, "img", "diagram.png"), pngBytes(36, [30, 150, 90], [255, 255, 255]));

  // (c) Loose files: a CSV (its Priority column has the name of a property that already exists) and a note.
  write(join(LOOSE_DIR, "list.csv"), ["Name,Priority,Done", "Alpha,High,Yes", "Beta,Low,No", "Gamma,High,No", ""].join("\r\n"));
  write(join(LOOSE_DIR, "loose.md"), ["# Loose note", "", "Just a loose file picked together with the CSV.", ""].join("\n"));

  const files = walk(SRC);
  info(`out/interop-src holds ${files.length} files: ${files.join(", ")}`);
  check(files.includes("notes/img/diagram.png") && files.includes("loose/list.csv") && files.includes("Export-abc.zip"), "the three fixtures are written");
  writeState({
    fixtures: { home: HOME_ID, sub: SUB_ID, db: DB_ID },
    privacyNeedles: [
      ...new Set([
        ...(readState().privacyNeedles ?? []),
        "Getting Started",
        "Sub page",
        "Tasks",
        "Write docs",
        "Ship it",
        "Project Alpha",
        "Meeting Notes",
        "Loose note",
        "Export-abc",
        "list.csv",
        "loose.md",
        "pic.png",
        "diagram.png",
        "Inbox",
        "owner",
        HOME_ID,
      ]),
    ],
  });
}

// ----- importing through the dialogs ---------------------------------------------------------------------------------

/** The import dialog's report as the page shows it: the counts, the warning groups, the "nothing left out" line. */
async function readReport() {
  return page.evaluate(`(() => {
    const facts = {};
    const dl = document.querySelector("dl.import-facts");
    if (dl) {
      const dts = [...dl.querySelectorAll("dt")];
      const dds = [...dl.querySelectorAll("dd")];
      dts.forEach((dt, i) => { facts[dt.textContent.trim()] = dds[i]?.textContent.trim() ?? ""; });
    }
    const groups = [...document.querySelectorAll(".import-warning-group")].map((g) => ({
      label: g.querySelector("summary")?.textContent.trim() ?? "",
      open: g.open,
      items: [...g.querySelectorAll("li")].map((li) => li.textContent.trim()),
    }));
    const text = document.querySelector('[role="dialog"]')?.innerText ?? "";
    return { facts, groups, noWarnings: text.includes("Nothing was left out or changed."), none: text.includes("No pages were imported.") };
  })()`);
}

/**
 * Imports through the "Import" dialog that is open: the source button, the native dialog (`steps` typed), then the report.
 * Returns the report as read from the page, the seconds the import took, and the dialog's own failure text, if it had one.
 */
async function importThroughDialog(button, dialogTitleText, steps) {
  const title = await dialogTitle();
  must(title === "Import", `the Import dialog is open (${title})`);
  const t0 = Date.now();
  await clickOn('[role="dialog"] button', `the "${button}" button`, { text: button });
  await fileDialog(dialogTitleText, steps);
  const end = Date.now() + 240000;
  let outcome = null;
  while (Date.now() < end && outcome === null) {
    outcome = await page.evaluate(`(() => {
      const heading = document.querySelector('[role="dialog"] .subtitle')?.textContent.trim() ?? "";
      if (heading === "Import finished") return "report";
      const error = document.querySelector('[role="dialog"] .error-text')?.textContent.trim();
      if (heading === "Import" && error) return "error:" + error;
      return null;
    })()`);
    if (outcome === null) await wait(300);
  }
  must(outcome === "report", `the import ended with its report (${outcome ?? "timed out"})`);
  const seconds = round((Date.now() - t0) / 1000);
  const report = await readReport();
  info(`import took ${seconds} s (dialog included): ${JSON.stringify(report.facts)}; ${report.groups.length} warning groups${report.noWarnings ? " (nothing left out)" : ""}`);
  for (const g of report.groups) {
    info(`warnings "${g.label}": ${g.items.slice(0, 6).join(" | ")}${g.items.length > 6 ? ` | ... ${g.items.length - 6} more` : ""}`);
  }
  return { report, seconds };
}

/** "Open" in the report: the dialog closes and the first page created opens. */
async function openFromReport() {
  await clickOn('[role="dialog"] button', "the report's Open button", { text: "Open" });
  await waitUntil(`!document.querySelector('[role="dialog"]')`, "the import dialog closed");
  await wait(800);
}

const BAD_WARNINGS = new Set(["attachmentFailed", "tooLarge", "notUtf8", "propertyLimit", "truncated", "missingFile", "invalidValue"]);
function warningLabelsOf(report) {
  return report.groups.map((g) => g.label);
}

/** The pages under the roots the import reported (its container, or the pages it made at the top), as paths of titles. */
function importedPaths(tree, rootIds) {
  const all = livePages(tree);
  const out = [];
  for (const rootId of rootIds) {
    const root = all.find((p) => p.id === rootId);
    if (root === undefined) continue;
    const walkNode = (node, trail) => {
      const path = [...trail, node.page.title];
      out.push({ id: node.page.id, path: path.join(" > "), title: node.page.title });
      for (const child of node.children) walkNode(child, path);
    };
    walkNode(root.node, []);
  }
  return out;
}

/** The values of a page's properties by property name (options and dates as the user reads them). */
async function readProps(spaceId, pageId) {
  const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId } })).properties;
  const values = (await ask({ type: "PagePropertiesGet", data: { pageId } })).values;
  const out = {};
  for (const [propertyId, value] of values) {
    const def = schema.find((d) => d.id === propertyId);
    if (def === undefined) continue;
    const option = (id) => def.options?.find((o) => o.id === id)?.name ?? id;
    if (value.kind === "select" || value.kind === "status") out[def.name] = option(value.value);
    else if (value.kind === "multi_select") out[def.name] = value.value.map(option);
    else if (value.kind === "date") out[def.name] = value.value.time ? `${value.value.date} ${value.value.time}` : value.value.date;
    else out[def.name] = value.value;
  }
  return out;
}

/** What the open page shows: the text of its editor, the heading blocks, the images and the page chips. */
async function readOpenPage() {
  return page.evaluate(`(() => {
    const root = document.querySelector(".content-body > section.page .ProseMirror");
    if (!root) return null;
    const imgs = [...root.querySelectorAll("img")].map((i) => ({ w: i.naturalWidth, complete: i.complete, src: (i.getAttribute("src") ?? "").slice(0, 40) }));
    return {
      title: document.querySelector(".content-body h1")?.textContent.trim() ?? null,
      text: root.innerText,
      headings: [...root.querySelectorAll("h1, h2, h3")].map((h) => h.textContent.trim()),
      images: imgs,
      chips: [...root.querySelectorAll("[data-page-ref]")].map((c) => ({ id: c.getAttribute("data-page-id"), text: c.textContent.trim() })),
      cards: [...root.querySelectorAll('[data-type="pageLink"]')].map((c) => ({ id: c.getAttribute("data-page-id"), text: c.textContent.trim() })),
      table: (() => {
        const t = root.querySelector('[data-type="pageList"][data-layout="table"]');
        if (!t) return null;
        const head = [...t.querySelectorAll(".page-table-head .page-table-row > *")].map((c) => c.textContent.trim());
        const rows = [...t.querySelectorAll(".page-table-body .page-table-row")].map((r) => [...r.children].map((c) => c.textContent.trim()));
        return { head, rows };
      })(),
      tasks: [...root.querySelectorAll('input[type="checkbox"]')].map((c) => c.checked),
    };
  })()`);
}

const PROBLEM_GUIDS = /[0-9a-f]{32}/;

// ----- phase: import-zip -------------------------------------------------------------------------------------------

async function phaseImportZip() {
  const state = readState();
  const space = state.spaceId;
  const before = livePages(await treeOf(space)).length;
  await openPageId(space, state.sourceId);
  await spaceMenu("Import…");
  await waitFor('[role="dialog"]', "the Import dialog");
  const { report, seconds } = await importThroughDialog("Choose a .zip file…", "Open", NOTION_ZIP);
  await capture("import-zip-report");
  const pagesFact = Number(report.facts["Pages"]);
  check(pagesFact >= 5 && pagesFact <= 6, `the report says ${pagesFact} pages (Getting Started, Sub page, Tasks, two rows, and the container when there is one)`);
  check(Number(report.facts["Attachments"]) === 1, `the report says 1 attachment (pic.png): ${report.facts["Attachments"]}`);
  check(Number(report.facts["Properties created"]) === 3, `the report says 3 properties created (Status, Due, Done): ${report.facts["Properties created"]}`);
  const bad = warningLabelsOf(report).filter((label) => /skipped|not valid text|too large|were not found|too many|did not fit|not created|attachments/i.test(label));
  check(bad.length === 0, `no warning of a kind that means data was lost (${warningLabelsOf(report).join("; ") || "none"})`);
  result.importZip = { seconds, facts: report.facts, warnings: report.groups };
  await openFromReport();

  const tree = await treeOf(space);
  const all = livePages(tree);
  check(all.length - before === pagesFact, `the tree grew by the ${pagesFact} pages the report counts (${before} -> ${all.length})`);
  const roots = tree.roots.map((n) => n.page.title);
  const container = tree.roots.find((n) => n.page.title === "Export-abc");
  info(`root pages: ${roots.join(", ")}`);
  const imported = container ? importedPaths(tree, [container.page.id]) : importedPaths(tree, tree.roots.filter((n) => ["Getting Started", "Tasks"].includes(n.page.title)).map((n) => n.page.id));
  const paths = imported.map((p) => p.path);
  info(`imported tree: ${paths.join(" | ")}`);
  const expected = ["Getting Started", "Sub page", "Tasks", "Write docs", "Ship it"];
  for (const title of expected) {
    check(imported.some((p) => p.title === title), `the page "${title}" was imported`);
  }
  check(container !== undefined, 'the zip\'s pages sit under a container page named after the file ("Export-abc")');
  check(imported.every((p) => !PROBLEM_GUIDS.test(p.title) && p.title === p.title.trim()), `no title carries a Notion id (${imported.map((p) => p.title).join(", ")})`);
  const byTitle = (t) => imported.find((p) => p.title === t);
  check(byTitle("Sub page")?.path.endsWith("Getting Started > Sub page"), "Sub page is a child of Getting Started");
  check(byTitle("Write docs")?.path.endsWith("Tasks > Write docs") && byTitle("Ship it")?.path.endsWith("Tasks > Ship it"), "the two rows are children of Tasks");
  const rowOrder = (container ? container.children : []).find((n) => n.page.title === "Tasks")?.children.map((n) => n.page.title) ?? [];
  check(rowOrder.join(",") === "Write docs,Ship it", `the rows keep the CSV's order (${rowOrder.join(", ")})`);
  writeState({ importedTitles: expected, importContainerId: container?.page.id ?? null, gettingStartedId: byTitle("Getting Started")?.id, tasksId: byTitle("Tasks")?.id, subPageId: byTitle("Sub page")?.id, rowIds: { "Write docs": byTitle("Write docs")?.id, "Ship it": byTitle("Ship it")?.id } });

  // The page "Open" went to.
  const shown = await readOpenPage();
  check(shown?.title === "Export-abc", `"Open" shows the first page the import made (${shown?.title})`);

  // Getting Started: no duplicated title heading, text, the image, the chips.
  const gs = byTitle("Getting Started");
  await openPageId(space, gs.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Getting Started"`, "Getting Started is open");
  await waitUntil(`(() => { const i = document.querySelector(".ProseMirror img"); return !!i && i.complete && i.naturalWidth > 0; })()`, "the image of Getting Started loads", 30000);
  const gsPage = await readOpenPage();
  check(gsPage.images.length === 1 && gsPage.images[0].w > 0, `the image block shows pic.png (${JSON.stringify(gsPage.images)})`);
  check(!gsPage.headings.includes("Getting Started"), `the "# Getting Started" heading became the title, not a heading of the content (${gsPage.headings.join(", ") || "no headings"})`);
  check(gsPage.text.includes("Welcome to the import test."), "the text is there");
  const sub = byTitle("Sub page");
  const tasks = byTitle("Tasks");
  check(gsPage.chips.some((c) => c.id === sub.id && c.text === "Sub page"), `the link to the sub-page is a page chip of that page (${JSON.stringify(gsPage.chips)})`);
  check(gsPage.chips.some((c) => c.id === tasks.id), `the link to the database is a page chip too`);
  check(!/\.md\)|%20/.test(gsPage.text), "no raw file link is left in the text");
  await capture("import-zip-page");

  // Tasks: a table view with the two rows and the CSV's columns.
  await openPageId(space, tasks.id);
  await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row', "the table view of Tasks");
  const tasksPage = await readOpenPage();
  check(tasksPage.table !== null && tasksPage.table.rows.length === 2, `the database page shows a table view with 2 rows (${tasksPage.table?.rows.length})`);
  check(["Status", "Due", "Done"].every((h) => tasksPage.table.head.some((c) => c.includes(h))), `its columns are the CSV's (${tasksPage.table.head.join(" | ")})`);
  const rowText = tasksPage.table.rows.map((r) => r.join(" | "));
  check(rowText.some((r) => r.includes("Write docs") && r.includes("In progress")) && rowText.some((r) => r.includes("Ship it") && r.includes("Done")), `the rows carry their values (${rowText.join(" ; ")})`);
  await capture("import-zip-tasks");

  // A row page: its properties, and the Notion property lines are not left in the text.
  const props = await readProps(space, byTitle("Write docs").id);
  info(`Write docs properties: ${JSON.stringify(props)}`);
  check(props["Status"] === "In progress" && props["Due"] === "2026-10-07" && props["Done"] === false, `Write docs has Status, Due and Done (${JSON.stringify(props)})`);
  const props2 = await readProps(space, byTitle("Ship it").id);
  check(props2["Status"] === "Done" && props2["Due"] === "2026-10-08" && props2["Done"] === true, `Ship it has Status, Due and Done (${JSON.stringify(props2)})`);
  await openPageId(space, byTitle("Write docs").id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Write docs"`, "Write docs is open");
  const row = await readOpenPage();
  check(row.text.includes("Draft the docs for the import feature.") && !/Status: In progress/.test(row.text), `the row page keeps its body and drops the property lines (${JSON.stringify(row.text.slice(0, 80))})`);
  const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties;
  const kinds = Object.fromEntries(schema.map((d) => [d.name, d.kind]));
  info(`schema: ${JSON.stringify(kinds)}`);
  check(kinds["Status"] === "select" && kinds["Due"] === "date" && kinds["Done"] === "checkbox", `the CSV columns became a select, a date and a checkbox property (${JSON.stringify(kinds)})`);
}

// ----- phase: import-folder ------------------------------------------------------------------------------------------

async function phaseImportFolder() {
  const state = readState();
  const space = state.spaceId;
  // A page to import into.
  await newPage("Inbox");
  const inbox = livePages(await treeOf(space)).find((p) => p.title === "Inbox");
  must(inbox !== undefined, "the page Inbox exists");
  writeState({ inboxId: inbox.id });
  const schemaBefore = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties.map((d) => d.name);
  await pageMenu("Import into this page…");
  await waitFor('[role="dialog"]', "the Import dialog");
  const intro = await page.evaluate(`document.querySelector('[role="dialog"] p')?.textContent.trim() ?? ""`);
  check(intro.includes("Inbox"), `the dialog names its destination (${intro})`);
  const { report, seconds } = await importThroughDialog("Choose a folder…", "Select Folder", NOTES_DIR);
  await capture("import-folder-report");
  check(Number(report.facts["Pages"]) === 3, `the report says 3 pages (the folder's container and its two notes): ${report.facts["Pages"]}`);
  check(Number(report.facts["Attachments"]) === 1, `the report says 1 attachment (the image in img/): ${report.facts["Attachments"]}`);
  check(Number(report.facts["Properties created"]) === 2, `the report says 2 properties created (owner, rating; Tags is built in): ${report.facts["Properties created"]}`);
  result.importFolder = { seconds, facts: report.facts, warnings: report.groups };
  await openFromReport();

  const tree = await treeOf(space);
  const inboxNode = livePages(tree).find((p) => p.id === inbox.id);
  const under = importedPaths(tree, inboxNode.node.children.map((n) => n.page.id));
  info(`under Inbox: ${under.map((p) => p.path).join(" | ")}`);
  check(under.some((p) => p.path === "notes") && under.some((p) => p.path === "notes > Project Alpha") && under.some((p) => p.path === "notes > Meeting Notes"), "Inbox holds the page notes with Project Alpha and Meeting Notes inside it");
  const alpha = under.find((p) => p.title === "Project Alpha");
  const meeting = under.find((p) => p.title === "Meeting Notes");
  must(alpha !== undefined && meeting !== undefined, "both notes exist");
  writeState({ notesIds: { alpha: alpha.id, meeting: meeting.id } });

  // Front matter became properties.
  const alphaProps = await readProps(space, alpha.id);
  info(`Project Alpha properties: ${JSON.stringify(alphaProps)}`);
  check(JSON.stringify(alphaProps["Tags"]?.slice().sort()) === JSON.stringify(["alpha", "work"]), `front matter tags became the built-in Tags (${JSON.stringify(alphaProps["Tags"])})`);
  check(alphaProps["owner"] === "Ben", `a custom text property "owner" has its value (${JSON.stringify(alphaProps["owner"])})`);
  check(alphaProps["rating"] === 4, `a custom number property "rating" has its value (${JSON.stringify(alphaProps["rating"])})`);
  const meetingProps = await readProps(space, meeting.id);
  check(JSON.stringify(meetingProps["Tags"]?.slice().sort()) === JSON.stringify(["meetings", "weekly"]), `a comma-separated tags string became Tags too (${JSON.stringify(meetingProps["Tags"])})`);
  const schemaAfter = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties.map((d) => d.name);
  check(schemaAfter.filter((n) => !schemaBefore.includes(n)).sort().join(",") === "owner,rating", `only owner and rating were added to the schema (${schemaAfter.filter((n) => !schemaBefore.includes(n)).join(", ")})`);
  check(schemaAfter.filter((n) => n === "Tags").length === 1, "Tags exists once");

  // The wiki link resolved to a page chip, and the other note lists it as a backlink; the image loads.
  await openPageId(space, alpha.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Project Alpha"`, "Project Alpha is open");
  await waitUntil(`(() => { const i = document.querySelector(".ProseMirror img"); return !!i && i.complete && i.naturalWidth > 0; })()`, "the image of Project Alpha loads", 30000);
  const alphaPage = await readOpenPage();
  check(alphaPage.chips.some((c) => c.id === meeting.id && c.text === "Meeting Notes"), `the wiki link [[Meeting Notes]] is a page chip of that page (${JSON.stringify(alphaPage.chips)})`);
  check(!alphaPage.text.includes("[[") && !alphaPage.text.includes("---"), "no wiki brackets or front matter are left in the text");
  check(alphaPage.images.length === 1 && alphaPage.images[0].w > 0, `the image of img/ shows (${JSON.stringify(alphaPage.images)})`);
  const chipsShown = await page.evaluate(`[...document.querySelectorAll(".properties .property-row")].map((r) => r.getAttribute("aria-label") + "=" + r.querySelector(".property-value")?.textContent.trim())`);
  info(`properties panel: ${chipsShown.join("; ")}`);
  check(chipsShown.some((r) => r.startsWith("owner=") && r.includes("Ben")), "the properties panel shows owner");
  await capture("import-folder-alpha");
  const back = await ask({ type: "BacklinksGet", data: { pageId: meeting.id } });
  const backTitles = (Array.isArray(back) ? back : (back.backlinks ?? [])).map((b) => b.title ?? b.pageTitle ?? JSON.stringify(b).slice(0, 60));
  info(`backlinks of Meeting Notes: ${backTitles.join(", ")}`);
  check(backTitles.some((t) => t === "Project Alpha"), `Meeting Notes has Project Alpha among its backlinks (${backTitles.join(", ")})`);
  await openPageId(space, meeting.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Meeting Notes"`, "Meeting Notes is open");
  const meetingPage = await readOpenPage();
  check(meetingPage.chips.some((c) => c.id === alpha.id), `the link back, [[Project Alpha]], is a page chip (${JSON.stringify(meetingPage.chips)})`);
}

// ----- phase: import-files ------------------------------------------------------------------------------------------

async function phaseImportFiles() {
  const state = readState();
  const space = state.spaceId;
  const before = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties;
  await openPageId(space, state.sourceId);
  await spaceMenu("Import…");
  await waitFor('[role="dialog"]', "the Import dialog");
  // A multi-select Open dialog: the folder first (it navigates), then the quoted names relative to it.
  const { report, seconds } = await importThroughDialog("Choose files…", "Open", [LOOSE_DIR, '"list.csv" "loose.md"']);
  await capture("import-files-report");
  check(Number(report.facts["Pages"]) === 5, `the report says 5 pages (list, its three rows, loose): ${report.facts["Pages"]}`);
  check(Number(report.facts["Properties created"]) === 0, `no property was created: Priority and Done exist already and were reused (${report.facts["Properties created"]})`);
  result.importFiles = { seconds, facts: report.facts, warnings: report.groups };
  await openFromReport();
  const tree = await treeOf(space);
  const list = tree.roots.find((n) => n.page.title === "list");
  const loose = tree.roots.find((n) => n.page.title === "loose");
  check(list !== undefined && loose !== undefined, `"list" and "loose" are pages at the top of the space (${tree.roots.map((n) => n.page.title).join(", ")})`);
  must(list !== undefined, "the database page list exists");
  check(list.children.map((n) => n.page.title).join(",") === "Alpha,Beta,Gamma", `list has its rows in order (${list.children.map((n) => n.page.title).join(", ")})`);
  writeState({ listId: list.page.id, looseId: loose?.page.id });
  const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties;
  check(schema.length === before.length, `the schema did not grow (${before.length} -> ${schema.length})`);
  const priority = schema.filter((d) => d.name === "Priority");
  check(priority.length === 1 && priority[0].kind === "select", "Priority is still the one select property");
  const first = await readProps(space, list.children[0].page.id);
  check(first["Priority"] === "High" && first["Done"] === true, `the first row has its values (${JSON.stringify(first)})`);
  await openPageId(space, list.page.id);
  await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row', "the table view of list");
  const shown = await readOpenPage();
  check(shown.table !== null && shown.table.rows.length === 3, `"list" is a database page with a table view of 3 rows (${shown.table?.rows.length})`);
  check(["Priority", "Done"].every((h) => shown.table.head.some((c) => c.includes(h))), `its columns are the CSV's (${shown.table.head.join(" | ")})`);
  await capture("import-files-list");
}

// ----- exporting through the dialogs ---------------------------------------------------------------------------------

/** The toasts on screen, their texts. */
const toastTexts = () => page.evaluate(`[...document.querySelectorAll(".toasts .toast-text")].map((t) => t.textContent.trim())`);

/**
 * Exports through the "Export" dialog that is open: the format (the radio's label), "Include sub-pages" when given, the
 * Export button, the native Save As dialog (`savePath` typed), then waits for the toast. Returns the toast and the name the
 * Save As dialog proposed.
 */
async function exportThroughDialog({ format = "Markdown", subpages, savePath, titlePrefix }) {
  const title = await waitUntil(`document.querySelector('[role="dialog"] .subtitle')?.textContent.trim() || null`, "the Export dialog");
  check(title.startsWith(titlePrefix), `the Export dialog is titled "${title}"`);
  const formats = await page.evaluate(`[...document.querySelectorAll('[role="dialog"] .export-format label.radio')].map((l) => ({ text: l.textContent.trim(), checked: l.querySelector("input").checked }))`);
  info(`export formats offered: ${formats.map((f) => f.text + (f.checked ? " (chosen)" : "")).join(", ")}`);
  check(formats.find((f) => f.checked)?.text === "Markdown", "Markdown is the format chosen at first");
  if (format !== "Markdown") {
    const label = await page.evaluate(`(() => {
      const l = [...document.querySelectorAll('[role="dialog"] .export-format label.radio')].find((e) => e.textContent.trim() === ${JSON.stringify(format)});
      if (!l) return null;
      const r = l.getBoundingClientRect();
      return { cx: r.x + 12, cy: r.y + r.height / 2 };
    })()`);
    must(label !== null, `the dialog offers ${format}`);
    await mouse(label.cx, label.cy);
  }
  const sub = await page.evaluate(`(() => { const i = document.querySelector('[role="dialog"] .export-subpages input'); return i ? i.checked : null; })()`);
  if (subpages !== undefined) {
    check(sub === false, `"Include sub-pages" is offered and starts unchecked (${sub})`);
    const b = await box('[role="dialog"] .export-subpages input');
    await mouse(b.cx, b.cy);
    check(await page.evaluate(`document.querySelector('[role="dialog"] .export-subpages input').checked`), '"Include sub-pages" is checked');
  } else {
    info(`"Include sub-pages" is ${sub === null ? "not offered" : "offered"}`);
  }
  rmSync(savePath, { force: true });
  const t0 = Date.now();
  await clickOn('[role="dialog"] button[type="submit"]', "the Export button");
  const dlg = await fileDialog("Save As", savePath);
  const base = basename(savePath);
  // The toast of THIS export: an earlier one (they stay 15 s) may still be on screen.
  const toastNow = () => page.evaluate(`[...document.querySelectorAll(".toasts .toast-text")].map((t) => t.textContent.trim()).find((t) => t.startsWith("Exported") && t.includes(${JSON.stringify(base)})) ?? null`);
  let toast = null;
  for (let i = 0; i < 480 && toast === null; i++) {
    toast = await toastNow();
    if (toast === null) await wait(250);
  }
  if (toast === null) {
    const seen = await page.evaluate(`({ dialog: document.querySelector('[role="dialog"]')?.innerText ?? null, toasts: [...document.querySelectorAll(".toasts .toast-text")].map((t) => t.textContent.trim()) })`);
    info(`no toast for ${base}: dialog ${JSON.stringify(seen.dialog)}, toasts ${JSON.stringify(seen.toasts)}`);
    await capture("export-no-toast");
    must(false, `the export's toast for ${base} appeared (not within 120 s)`);
  }
  const seconds = round((Date.now() - t0) / 1000);
  info(`export took ${seconds} s (dialog included): "${toast}"`);
  await waitUntil(`!document.querySelector('[role="dialog"]')`, "the Export dialog closed");
  check(existsSync(savePath), `the file ${savePath.split(/[\\/]/).pop()} exists (${existsSync(savePath) ? statSync(savePath).size : 0} bytes)`);
  return { toast, proposed: dlg.initial ?? "", seconds };
}

/** The destinations of the links and images of a Markdown text that point at files (no scheme, no #fragment), decoded. */
function fileLinks(markdown) {
  const out = [];
  const re = /!?\[(?:\\.|[^\]\\])*\]\(\s*(<[^>]*>|[^\s)]*)(?:\s+"(?:\\.|[^"\\])*")?\s*\)/g;
  for (const m of markdown.matchAll(re)) {
    let dest = m[1];
    if (dest.startsWith("<") && dest.endsWith(">")) dest = dest.slice(1, -1);
    if (dest === "" || dest.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(dest)) continue;
    dest = dest.split("#")[0];
    try {
      dest = decodeURIComponent(dest);
    } catch {
      // left as written: the check then says it does not resolve
    }
    out.push(dest);
  }
  return out;
}
function resolveRelative(fromFile, dest) {
  const parts = fromFile.split("/").slice(0, -1);
  for (const seg of dest.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== "." && seg !== "") parts.push(seg);
  }
  return parts.join("/");
}
/** Every relative link of every .md of an archive resolves to an entry. */
function checkLinks(archive) {
  const names = new Set(archive.names);
  let total = 0;
  const broken = [];
  for (const [name, text] of Object.entries(archive.texts)) {
    if (!name.endsWith(".md")) continue;
    for (const dest of fileLinks(text)) {
      total += 1;
      if (!names.has(resolveRelative(name, dest))) broken.push(`${name} -> ${dest}`);
    }
  }
  return { total, broken };
}
/** The `key: value` lines of a file's front matter (values as written), or null without one. */
function frontMatter(text) {
  const m = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(text);
  if (m === null) return null;
  const out = {};
  for (const line of m[1].split(/\r?\n/)) {
    const kv = /^("(?:[^"\\]|\\.)*"|[^\s:][^:]*):\s*(.*)$/.exec(line);
    if (kv) out[kv[1].replace(/^"|"$/g, "")] = kv[2];
  }
  return out;
}
const tmpLeft = (dir) => readdirSync(dir).filter((n) => n.endsWith(".tmp"));

// ----- phase: export-zip ---------------------------------------------------------------------------------------------

async function phaseExportZip() {
  const state = readState();
  const space = state.spaceId;
  mkdirSync(EXPORT_DIR, { recursive: true });
  const zipPath = join(EXPORT_DIR, "source.zip");
  await openPageId(space, state.sourceId);
  await pageMenu("Export…");
  const { toast, proposed, seconds } = await exportThroughDialog({ format: "Markdown", subpages: true, savePath: zipPath, titlePrefix: "Export “Source”" });
  await capture("export-zip-done");
  check(/^Exported 3 pages to source\.zip\./.test(toast), `the toast says 3 pages went to source.zip ("${toast}")`);
  check(!/left out/.test(toast), "no item was left out");
  check(proposed === "Source.zip", `the Save As dialog proposed "Source.zip" (${proposed})`);
  check(tmpLeft(EXPORT_DIR).length === 0, `no .tmp file is left next to the export (${tmpLeft(EXPORT_DIR).join(", ") || "none"})`);
  result.exportZip = { seconds, bytes: statSync(zipPath).size };

  const zip = await readZip(zipPath);
  const files = zip.names.filter((n) => !n.endsWith("/"));
  info(`entries: ${zip.entries.map((e) => `${e.name} (${e.length} bytes, ${e.compressed} stored)`).join(" | ")}`);
  const want = ["Source.md", "Source/Alpha task.md", "Source/Beta task.md", "Source/sample.png", "atomik-export.json"];
  check(want.every((n) => files.includes(n)), `the archive has ${want.join(", ")}`);
  check(files.length === want.length, `and nothing else (${files.length} files: ${files.filter((n) => !want.includes(n)).join(", ") || "none"})`);
  check(zip.names.every((n) => !n.includes("\\") && !n.startsWith("/") && !n.includes("..")), "every entry name is a safe relative path with forward slashes");

  const md = zip.texts["Source.md"] ?? "";
  const fm = frontMatter(md);
  check(fm !== null && fm["title"] === '"Source"', `Source.md starts with front matter: title "Source" (${JSON.stringify(fm)})`);
  check(fm !== null && "created" in fm && "updated" in fm, "the front matter has created and updated");
  check(md.includes("# A heading in the source page") && md.includes("- [x] Buy milk") && md.includes("- [ ] Call Ann"), "the heading and the to-do list (one checked) are Markdown");
  check(md.includes("A paragraph in the source page. It carries no markup of its own, only text.") && md.includes("A closing paragraph after the table view."), "the paragraphs are there");
  check(/!\[[^\]]*\]\(Source\/sample\.png/.test(md), "the image is a Markdown image of Source/sample.png");
  const annotations = [...md.matchAll(/<!-- atomik:(\{.*?\}) -->/g)].map((m) => m[1]);
  info(`annotations: ${annotations.join(" ")}`);
  check(annotations.some((a) => a.includes('"node":"pageList"') && a.includes('"layout":"table"')), "an <!-- atomik: annotation holds the table view's pageList");
  const alpha = zip.texts["Source/Alpha task.md"] ?? "";
  const beta = zip.texts["Source/Beta task.md"] ?? "";
  check(frontMatter(alpha)?.["title"] === '"Alpha task"' && frontMatter(alpha)?.["Priority"] === '"High"', `Alpha task's front matter has title and Priority "High" (${JSON.stringify(frontMatter(alpha))})`);
  check(frontMatter(beta)?.["title"] === '"Beta task"' && frontMatter(beta)?.["Priority"] === '"Low"', `Beta task's front matter has title and Priority "Low" (${JSON.stringify(frontMatter(beta))})`);
  const links = checkLinks(zip);
  check(links.total >= 1 && links.broken.length === 0, `${links.total} relative link(s) decode to existing entries (broken: ${links.broken.join("; ") || "none"})`);
  const png = zip.entries.find((e) => e.name === "Source/sample.png");
  check(zip.heads["Source/sample.png"]?.startsWith("89504e47"), `the image entry is a PNG (${zip.heads["Source/sample.png"]})`);
  check(png !== undefined && png.compressed >= png.length, `attachments are stored without compression (${png?.length} bytes, ${png?.compressed} stored)`);
  const mdEntry = zip.entries.find((e) => e.name === "Source.md");
  check(mdEntry !== undefined && mdEntry.compressed < mdEntry.length, `text entries are deflated (${mdEntry?.length} -> ${mdEntry?.compressed})`);
  let sidecar = null;
  try {
    sidecar = JSON.parse(zip.texts["atomik-export.json"]);
  } catch (error) {
    check(false, `atomik-export.json is valid JSON (${error.message})`);
  }
  if (sidecar !== null) {
    info(`sidecar: ${JSON.stringify(sidecar).slice(0, 400)}`);
    check(sidecar.format === 1 && sidecar.generator === "atomik-kb", `the sidecar says format 1 by atomik-kb (${sidecar.format}, ${sidecar.generator})`);
    const priority = (sidecar.properties ?? []).find((p) => p.name === "Priority");
    check(priority?.kind === "select" && JSON.stringify(priority.options.map((o) => o.name).sort()) === '["High","Low"]', `the sidecar describes Priority as a select with High and Low (${JSON.stringify(priority)})`);
    check(JSON.stringify(sidecar.pages) === JSON.stringify(["Source.md", "Source/Alpha task.md", "Source/Beta task.md"]), `the sidecar lists the pages in tree order (${JSON.stringify(sidecar.pages)})`);
  }
  writeState({ exportZip: zipPath });
}

// ----- phase: reimport -----------------------------------------------------------------------------------------------

/** The portable JSON of a page without what differs between two copies of it (ids). */
function normalizeDoc(node) {
  if (Array.isArray(node)) return node.map(normalizeDoc);
  if (node !== null && typeof node === "object") {
    const out = {};
    for (const [key, value] of Object.entries(node).sort(([a], [b]) => a.localeCompare(b))) {
      if (key === "id" || key === "blockId") continue;
      out[key] = key === "attachmentId" ? "<attachment>" : normalizeDoc(value);
    }
    return out;
  }
  return node;
}
function firstDifference(a, b, path = "$") {
  if (JSON.stringify(a) === JSON.stringify(b)) return null;
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return `${path}: ${JSON.stringify(a)} against ${JSON.stringify(b)}`;
  const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])];
  for (const key of keys) {
    const d = firstDifference(a[key], b[key], `${path}.${key}`);
    if (d !== null) return d;
  }
  return `${path}: differs`;
}

async function phaseReimport() {
  const state = readState();
  const space = state.spaceId;
  must(existsSync(state.exportZip), "the export of the earlier phase is there");
  const schemaBefore = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties.map((d) => d.name);
  const treeBefore = await treeOf(space);
  const original = livePages(treeBefore).find((p) => p.id === state.sourceId);
  const originalChildren = original.node.children.map((n) => ({ id: n.page.id, title: n.page.title }));
  await openPageId(space, state.sourceId);
  await spaceMenu("Import…");
  await waitFor('[role="dialog"]', "the Import dialog");
  const { report, seconds } = await importThroughDialog("Choose a .zip file…", "Open", state.exportZip);
  await capture("reimport-report");
  result.reimport = { seconds, facts: report.facts, warnings: report.groups };
  check(Number(report.facts["Pages"]) === 3, `the report says 3 pages: ${report.facts["Pages"]}`);
  check(Number(report.facts["Attachments"]) === 1, `the report says 1 attachment: ${report.facts["Attachments"]}`);
  check(Number(report.facts["Properties created"]) === 0, `no property was created, the sidecar's Priority matched the existing one (${report.facts["Properties created"]})`);
  check(report.groups.length === 0, `nothing was left out or changed (${warningLabelsOf(report).join("; ") || "none"})`);
  await openFromReport();

  const tree = await treeOf(space);
  const imported = importedPaths(tree, [tree.roots.find((n) => n.page.id !== state.sourceId && n.page.title === "Source")?.page.id].filter(Boolean));
  info(`re-imported tree: ${imported.map((p) => p.path).join(" | ") || "(no root titled Source)"}`);
  must(imported.length === 3, `the zip came back as a page "Source" with its two pages (${imported.length} pages)`);
  const copy = imported[0];
  const copyChildren = imported.slice(1);
  check(copyChildren.map((c) => c.title).sort().join(",") === originalChildren.map((c) => c.title).sort().join(","), `same page titles (${copyChildren.map((c) => c.title).join(", ")})`);
  check(imported.every((p) => p.path.startsWith("Source")), "the copy sits at the top of the space, under no container");
  writeState({ copyId: copy.id, copyChildIds: copyChildren.map((c) => c.id) });

  // The same property values on the child pages.
  for (const child of originalChildren) {
    const twin = copyChildren.find((c) => c.title === child.title);
    const was = await readProps(space, child.id);
    const now = await readProps(space, twin.id);
    check(JSON.stringify(was) === JSON.stringify(now) && was["Priority"] !== undefined, `"${child.title}" has the same property values (${JSON.stringify(was)} / ${JSON.stringify(now)})`);
  }
  const schemaAfter = (await ask({ type: "PropertySchemaGet", data: { spaceId: space } })).properties.map((d) => d.name);
  check(JSON.stringify(schemaAfter) === JSON.stringify(schemaBefore), `the schema is unchanged (${schemaAfter.join(", ")})`);

  // The same page: the portable JSON, ids aside.
  const docOf = async (pageId) => normalizeDoc((await ask({ type: "PageContentGet", data: { pageId } })).content);
  const docWas = await docOf(state.sourceId);
  const docNow = await docOf(copy.id);
  const diff = firstDifference(docWas, docNow);
  info(`document of the original: ${JSON.stringify(docWas).slice(0, 300)}`);
  check(diff === null, `the page's content is the same up to ids (${diff ?? "identical"})`);
  if (diff !== null) {
    info(`document of the copy: ${JSON.stringify(docNow).slice(0, 500)}`);
  }
  const blocks = (docNow.content ?? []).map((b) => b.type);
  check(blocks.includes("pageList") && blocks.includes("image") && blocks.includes("taskList") && blocks.includes("heading"), `the copy has a table view, an image, a to-do list and a heading (${blocks.join(", ")})`);
  const pageList = (docNow.content ?? []).find((b) => b.type === "pageList");
  check(pageList?.attrs?.layout === "table", `the view is still a table (${pageList?.attrs?.layout})`);

  // On screen: the copy's table view lists its own two pages, the image loads, the first to-do is checked.
  await openPageId(space, copy.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Source"`, "the copy is open");
  await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row', "the copy's table view");
  await waitUntil(`(() => { const i = document.querySelector(".ProseMirror img"); return !!i && i.complete && i.naturalWidth > 0; })()`, "the copy's image loads", 30000);
  const shown = await readOpenPage();
  check(shown.table.rows.length === 2 && shown.table.rows.some((r) => r[0].startsWith("Alpha task")) && shown.table.rows.some((r) => r[0].startsWith("Beta task")), `the copy's table view lists its two pages (${JSON.stringify(shown.table.rows)})`);
  check(shown.images.length === 1 && shown.images[0].w === 48, `the image shows, 48 px wide as it was (${JSON.stringify(shown.images)})`);
  check(shown.tasks.join(",") === "true,false", `the to-do items keep their state (${shown.tasks.join(",")})`);
  await capture("reimport-copy");
}

// ----- phase: export-html --------------------------------------------------------------------------------------------

async function phaseExportHtml() {
  const state = readState();
  const space = state.spaceId;
  mkdirSync(EXPORT_DIR, { recursive: true });
  const htmlPath = join(EXPORT_DIR, "alpha.html");
  await openPageId(space, state.childIds[0]);
  await pageMenu("Export…");
  const { toast, proposed } = await exportThroughDialog({ format: "HTML", savePath: htmlPath, titlePrefix: "Export “Alpha task”" });
  check(/^Exported 1 page to alpha\.html\./.test(toast), `the toast says 1 page went to alpha.html ("${toast}")`);
  check(proposed === "Alpha task.html", `the Save As dialog proposed "Alpha task.html" (${proposed})`);
  check(tmpLeft(EXPORT_DIR).length === 0, "no .tmp file is left next to the export");
  const html = readFileSync(htmlPath, "utf8");
  info(`html: ${html.length} characters`);
  check(/^<!doctype html>/i.test(html.trimStart()), "it is a standalone document: <!doctype html>");
  check(!/<script/i.test(html) && !/javascript:/i.test(html), "it has no <script> and no javascript: link");
  check(/<title>Alpha task<\/title>/.test(html), "it has the page's <title>");
  check(/<html[^>]*lang="en"/.test(html) && /<meta charset="utf-8">/i.test(html), "it declares lang and charset");
  check(/<h1[^>]*class="page-title"[^>]*>[^<]*Alpha task/.test(html), "it shows the title as the page's <h1>");
  check(/<table class="properties">[\s\S]*Priority[\s\S]*High/.test(html), "its properties table has Priority: High");
  check(!/(?:src|href)="https?:/i.test(html), "it loads nothing from the web (no http(s) src or href)");
  check(/<style>/.test(html) && !/<link[^>]*stylesheet/i.test(html), "its style is embedded, not linked");
  check(html.startsWith("\uFEFF") === false, "it has no byte order mark");
  // Opened in the webview-less file system: a file of its own, no sibling files written.
  check(readdirSync(EXPORT_DIR).filter((n) => n.startsWith("alpha")).join(",") === "alpha.html", "only alpha.html was written for it");
}

// ----- phase: export-space -------------------------------------------------------------------------------------------

async function phaseExportSpace() {
  const state = readState();
  const space = state.spaceId;
  mkdirSync(EXPORT_DIR, { recursive: true });
  const zipPath = join(EXPORT_DIR, "space.zip");
  await openPageId(space, state.sourceId);
  const tree = await treeOf(space);
  const live = livePages(tree);
  info(`the space has ${live.length} live pages`);
  await spaceMenu("Export space…");
  const { toast, proposed, seconds } = await exportThroughDialog({ format: "Markdown", savePath: zipPath, titlePrefix: "Export the space" });
  check(new RegExp(`^Exported ${live.length} pages to space\\.zip\\.`).test(toast), `the toast says ${live.length} pages went to space.zip ("${toast}")`);
  check(proposed === `${state.spaceName}.zip`, `the Save As dialog proposed "${state.spaceName}.zip" (${proposed})`);
  result.exportSpace = { seconds, pages: live.length, bytes: statSync(zipPath).size };
  const zip = await readZip(zipPath);
  const files = zip.names.filter((n) => !n.endsWith("/"));
  const mdFiles = files.filter((n) => n.endsWith(".md"));
  check(mdFiles.length === live.length, `one .md per page (${mdFiles.length} of ${live.length})`);
  const titlesInArchive = mdFiles.map((n) => JSON.parse(frontMatter(zip.texts[n] ?? "")?.["title"] ?? '"?"')).sort();
  const titlesInTree = live.map((p) => p.title).sort();
  check(JSON.stringify(titlesInArchive) === JSON.stringify(titlesInTree), `the titles in the front matters are the pages' titles (${titlesInArchive.length})`);
  const pngs = files.filter((n) => n.endsWith(".png"));
  check(pngs.length === 4 && pngs.every((n) => zip.heads[n]?.startsWith("89504e47")), `the four pictures are in it as PNG files (${pngs.join(", ")})`);
  check(files.includes("Source.md") && files.includes("Source (2).md") && files.some((n) => n.startsWith("Source (2)/")), `the two pages titled Source are Source.md and Source (2).md, each with its folder`);
  let sidecar = null;
  try {
    sidecar = JSON.parse(zip.texts["atomik-export.json"]);
  } catch (error) {
    check(false, `atomik-export.json is valid JSON (${error.message})`);
  }
  if (sidecar !== null) {
    check(sidecar.pages.length === live.length && sidecar.pages.every((p) => files.includes(p)), `the sidecar lists the ${sidecar.pages.length} page files, all in the archive`);
    const names = sidecar.properties.map((p) => p.name);
    check(["Tags", "Priority", "Status", "Due", "Done", "owner", "rating"].every((n) => names.includes(n)), `the sidecar carries the whole schema (${names.join(", ")})`);
  }
  const links = checkLinks(zip);
  check(links.broken.length === 0, `${links.total} relative links, none broken (${links.broken.slice(0, 5).join("; ") || "none"})`);
  const withoutFm = mdFiles.filter((n) => frontMatter(zip.texts[n] ?? "") === null);
  check(withoutFm.length === 0, `every page file has front matter (${withoutFm.join(", ") || "all do"})`);
  // The imported database keeps its columns: Tasks' front matter and the row pages' values.
  const row = Object.entries(zip.texts).find(([n]) => n.endsWith("Write docs.md"));
  check(row !== undefined && frontMatter(row[1])?.["Status"] === '"In progress"' && frontMatter(row[1])?.["Done"] === "false", `a row page of the Notion import has its values (${JSON.stringify(row ? frontMatter(row[1]) : null)})`);
  const gs = Object.entries(zip.texts).find(([n]) => n.endsWith("Getting Started.md"));
  check(gs !== undefined && /\]\([^)]*Sub%20page\.md\)/.test(gs[1]), "Getting Started links to Sub page by a percent-encoded relative path");
  check(tmpLeft(EXPORT_DIR).length === 0, "no .tmp file is left next to the export");
  writeState({ exportSpaceZip: zipPath });
}

// ----- phase: csv ----------------------------------------------------------------------------------------------------

async function phaseCsv() {
  const state = readState();
  const space = state.spaceId;
  mkdirSync(EXPORT_DIR, { recursive: true });
  const csvPath = join(EXPORT_DIR, "tasks.csv");
  rmSync(csvPath, { force: true });
  await openPageId(space, state.tasksId);
  await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row', "the table view of Tasks");
  const shown = await readOpenPage();
  const viewRows = shown.table.rows.map((r) => r[0].replace(/↗$/, ""));
  info(`the view shows: ${shown.table.head.join(" | ")} ; ${shown.table.rows.map((r) => r.join(" | ")).join(" ; ")}`);
  await clickOn('.ProseMirror button[aria-label="View settings"]', "the view's settings button");
  await waitFor(".page-list-settings", "the view settings menu");
  const items = await page.evaluate(`[...document.querySelectorAll(".page-list-settings label, .page-list-settings button")].map((i) => i.textContent.trim())`);
  info(`view settings: ${items.join(", ")}`);
  await clickOn(".page-list-settings button", "Export CSV of the view settings menu", { text: "Export CSV" });
  const dlg = await fileDialog("Save As", csvPath);
  const toast = await waitUntil(`[...document.querySelectorAll(".toasts .toast-text")].map((t) => t.textContent.trim()).find((t) => t.startsWith("Saved") && t.includes("tasks.csv")) ?? null`, "the CSV toast", 60000);
  check(toast === "Saved tasks.csv.", `the toast says the file was saved ("${toast}")`);
  check((dlg.initial ?? "") === "Tasks.csv", `the Save As dialog proposed "Tasks.csv" (${dlg.initial})`);
  await capture("csv-done");
  const bytes = readFileSync(csvPath);
  check(bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf, `the file starts with a UTF-8 byte order mark (${bytes.subarray(0, 3).toString("hex")})`);
  const text = bytes.subarray(3).toString("utf8");
  const lines = text.split("\r\n");
  check(!/[^\r]\n/.test(text) && text.endsWith("\r\n"), "every line ends with CRLF");
  check(lines[0] === "Name,Status,Due,Done", `the header is Name,Status,Due,Done (${lines[0]})`);
  const rows = lines.slice(1).filter((l) => l !== "");
  check(rows.length === 2, `two data rows (${rows.length}): ${rows.join(" ; ")}`);
  check(JSON.stringify(rows.map((r) => r.split(",")[0])) === JSON.stringify(viewRows), `the rows come in the order the view shows them (${rows.map((r) => r.split(",")[0]).join(", ")} against ${viewRows.join(", ")})`);
  check(rows.includes("Write docs,In progress,2026-10-07,No") && rows.includes("Ship it,Done,2026-10-08,Yes"), `the cells are the values as text: dates as YYYY-MM-DD, checkboxes Yes/No (${rows.join(" ; ")})`);
  check(tmpLeft(EXPORT_DIR).length === 0, "no .tmp file is left next to the CSV");
}

// ----- phase: print --------------------------------------------------------------------------------------------------

const PDF_PATH = join(OUT, "interop-print.pdf");
const targetList = async () => (await (await fetch(`http://127.0.0.1:${CDP_PORT}/json`)).json()).map((t) => ({ id: t.id, type: t.type, url: t.url }));
/** A promise that gives up after `ms` (the page's thread is blocked in window.print() while the preview is open). */
const within = (promise, ms) => Promise.race([promise, new Promise((resolve) => setTimeout(() => resolve({ timedOut: true }), ms))]);

async function waitTarget(prefix, present, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const found = (await targetList()).some((t) => t.url.startsWith(prefix));
    if (found === present) return true;
    await wait(150);
  }
  return false;
}
/** Closes what the webview opened besides the app's page: the print preview and the downloads flyout. */
async function closeStrayTargets() {
  const closed = [];
  for (const t of await targetList()) {
    if (t.type === "page" && !t.url.startsWith("http://tauri.localhost") && !t.url.startsWith("tauri://localhost")) {
      await fetch(`http://127.0.0.1:${CDP_PORT}/json/close/${t.id}`).catch(() => undefined);
      closed.push(t.url);
    }
  }
  return closed;
}
const DEEP = `const deepAll = (root, sel, out = []) => { for (const el of root.querySelectorAll("*")) { if (el.shadowRoot) deepAll(el.shadowRoot, sel, out); } root.querySelectorAll(sel).forEach((e) => out.push(e)); return out; };`;
/** The centre of an element of the print preview (its own page, shadow roots included), or null. */
async function previewRect(preview, selector, text) {
  const json = await preview.evaluate(`(() => { ${DEEP} const el = deepAll(document, ${JSON.stringify(selector)}).find((e) => ${text === undefined ? "true" : `e.textContent.trim().startsWith(${JSON.stringify(text)})`}); if (!el) return null; const r = el.getBoundingClientRect(); return JSON.stringify({ cx: r.x + r.width / 2, cy: r.y + r.height / 2, w: r.width, h: r.height, text: el.textContent.trim().slice(0, 40) }); })()`);
  return json === null ? null : JSON.parse(json);
}
async function previewClick(preview, rect) {
  await preview.click(rect.cx, rect.cy);
  await wait(700);
}

/** Records what the page does around window.print(): the events, the theme and the notice, in order. */
async function installPrintProbes() {
  await page.evaluate(`(() => {
    const events = [];
    window.__print = events;
    const t = () => Math.round(performance.now());
    const snap = () => ({ theme: document.documentElement.dataset.theme ?? null, notice: document.querySelector(".page-notice")?.textContent.trim() ?? null });
    for (const name of ["beforeprint", "afterprint"]) window.addEventListener(name, () => events.push({ kind: name, t: t(), ...snap() }));
    const original = window.print;
    window.print = function () {
      events.push({ kind: "print() called", t: t(), ...snap(), images: [...document.querySelectorAll(".ProseMirror img")].map((i) => i.complete && i.naturalWidth > 0), pending: document.querySelectorAll("[data-busy], [data-state=loading]").length, tableRows: document.querySelectorAll('.ProseMirror [data-type="pageList"] .page-table-body .page-table-row').length });
      try { return original.apply(this, arguments); } finally { events.push({ kind: "print() returned", t: t(), ...snap() }); }
    };
    let last = JSON.stringify(snap());
    new MutationObserver(() => { const now = JSON.stringify(snap()); if (now !== last) { last = now; events.push({ kind: "changed", t: t(), ...snap() }); } }).observe(document.documentElement, { attributes: true, childList: true, subtree: true, characterData: true });
  })()`);
}
const printEvents = () => page.evaluate(`window.__print ?? []`);

/** Presses Ctrl+P (trusted: key events over the DevTools port) and waits for the webview's print preview. */
async function openPrintPreview() {
  const baseline = (await helper("windows", {})).children.filter((w) => w.class === "Chrome_RenderWidgetHostHWND").map((w) => w.hwnd);
  await page.key("p", "KeyP", 80, 2);
  const shown = await waitTarget("edge://print", true, 20000);
  must(shown, "Ctrl+P brought up the webview's print preview (a page target edge://print/)");
  const preview = await connect((url) => url.startsWith("edge://print"), { record: false });
  await waitUntilPreview(preview, `(() => { ${DEEP} return deepAll(document, "button[role=combobox]").length > 0; })()`, "the print preview has its printer list");
  const windows = await helper("windows", {});
  const dialogWidget = windows.children.find((w) => w.class === "Chrome_RenderWidgetHostHWND" && !baseline.includes(w.hwnd));
  return { preview, windows, dialogWidget };
}
async function waitUntilPreview(preview, expression, what, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await preview.evaluate(expression).catch(() => false)) return;
    await wait(200);
  }
  must(false, `${what} (timed out)`);
}
/** After the preview closed the page's thread runs again: waits until it answers. */
async function waitPageResponsive(ms = 30000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const r = await within(page.evaluate("1"), 1500);
    if (r === 1) return true;
  }
  return false;
}

async function phasePrint() {
  const state = readState();
  const space = state.spaceId;
  info(`closed before the phase: ${(await closeStrayTargets()).join(", ") || "nothing"}`);
  rmSync(PDF_PATH, { force: true });
  await openPageId(space, state.sourceId);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Source"`, "Source is open");
  await waitUntil(`(() => { const i = document.querySelector(".ProseMirror img"); return !!i && i.complete && i.naturalWidth > 0; })()`, "the image of Source loads", 30000);
  const themeBefore = await page.evaluate(`document.documentElement.dataset.theme ?? null`);
  info(`data-theme before: ${themeBefore}`);

  // The print stylesheet, in the real webview: print media emulated, what shows and what does not.
  await page.send("Emulation.setEmulatedMedia", { media: "print" });
  const shows = await page.evaluate(`(() => {
    const state = (sel) => { const el = document.querySelector(sel); if (!el) return "missing"; const cs = getComputedStyle(el); const r = el.getBoundingClientRect(); return cs.display === "none" || cs.visibility === "hidden" || (r.width === 0 && r.height === 0) ? "hidden" : "shown"; };
    return { sidebar: state(".sidebar"), tabs: state(".tab-strip"), page: state(".content-body > section.page"), title: state(".content-body > section.page .page-title"), editor: state(".content-body > section.page .ProseMirror"), addProperty: state(".properties-add"), viewToolbar: state(".page-list-toolbar") };
  })()`);
  await page.send("Emulation.setEmulatedMedia", { media: "" });
  info(`@media print: ${JSON.stringify(shows)}`);
  check(shows.sidebar === "hidden" && shows.tabs === "hidden", `@media print hides the sidebar and the tab strip (${shows.sidebar}, ${shows.tabs})`);
  check(shows.page === "shown" && shows.title === "shown" && shows.editor === "shown", `@media print shows the page, its title and its content (${shows.page}, ${shows.title}, ${shows.editor})`);
  check(shows.addProperty !== "shown" && shows.viewToolbar !== "shown", `@media print hides the "Add a property" button and the view toolbar (${shows.addProperty}, ${shows.viewToolbar})`);

  // --- A: Ctrl+P, look at the preview, close it with Escape ---
  await installPrintProbes();
  const t0 = Date.now();
  const { preview, windows, dialogWidget } = await openPrintPreview();
  info(`print preview up ${Date.now() - t0} ms after Ctrl+P; targets: ${(await targetList()).map((t) => `${t.type} ${t.url.slice(0, 50)}`).join(" | ")}`);
  info(`the preview's own window: ${dialogWidget ? `Chrome_RenderWidgetHostHWND ${dialogWidget.width}x${dialogWidget.height} at ${dialogWidget.left},${dialogWidget.top}` : "not found"}; top-level windows of the app's processes: ${windows.top.filter((w) => w.visible).map((w) => `${w.class} '${w.title}'`).join(", ")}`);
  await capture("print-preview");
  const dest = await preview.evaluate(`(() => { ${DEEP} return { dest: deepAll(document, "button[role=combobox]")[0]?.textContent.trim() ?? null, total: deepAll(document, "*").find((e) => /^Total:/.test(e.textContent?.trim() ?? "") && e.children.length === 0)?.textContent.trim() ?? null, save: deepAll(document, "fluent-button.print")[0]?.textContent.trim() ?? null }; })()`);
  info(`preview: printer "${dest.dest}", ${dest.total}, button "${dest.save}"`);
  result.print = { printer: dest.dest, total: dest.total, button: dest.save, shows };

  // What UI Automation sees.
  const uia = await helper("uia", { hwnd: windows.main, limit: 400 });
  const elements = Array.isArray(uia.elements) ? uia.elements : uia.elements ? [uia.elements] : [];
  result.print.uia = elements.map((e) => `${e.type}:${e.name}`).filter((s) => !s.endsWith(":"));
  info(`UI Automation sees ${elements.length} elements under the main window: ${result.print.uia.slice(0, 40).join(" | ")}`);
  check(elements.some((e) => e.type === "Window" && e.name === "Print"), `UI Automation lists the print preview as a Window named "Print"`);
  check(elements.some((e) => e.type === "Button" && e.name === "Cancel") && elements.some((e) => e.type === "Button" && (e.name === "Save" || e.name === "Print")), "UI Automation lists its Save (or Print) and Cancel buttons");
  check(elements.some((e) => /PDF document containing 1 page/.test(e.name)), `UI Automation lists the preview as "PDF document containing 1 page" (${elements.filter((e) => /PDF/.test(e.name)).map((e) => e.name).join("; ")})`);
  const sidebarWords = elements.filter((e) => /^(Settings|Diagnostics|Home|Trash|New page|Search)\b/.test(e.name) && e.type !== "Document");
  check(sidebarWords.length === 0, `the preview shows none of the sidebar's items (${sidebarWords.map((e) => e.name).join(", ") || "none seen"})`);
  check(elements.some((e) => /closing paragraph after the table view/.test(e.name)), "the page's own text is in the preview");

  // Escape: first the trusted key over the DevTools port to the preview's page, then the posted key to its window.
  await preview.key("Escape", "Escape", 27);
  let gone = await waitTarget("edge://print", false, 2500);
  info(`Escape sent over the DevTools port to the preview's page: ${gone ? "the preview closed" : "the preview stayed"}`);
  let how = "a DevTools key event";
  if (!gone) {
    must(dialogWidget !== undefined, "the preview's window was found to post Escape to");
    await helper("postkey", { hwnd: dialogWidget.hwnd, key: "escape" });
    gone = await waitTarget("edge://print", false, 10000);
    how = "Escape posted to the preview's window (WM_KEYDOWN, WM_KEYUP)";
    info(`Escape posted to the preview's window: ${gone ? "the preview closed" : "the preview stayed"}`);
  }
  check(gone, `Escape closed the preview (${how})`);
  preview.close();
  result.print.escape = gone ? how : "did not close";
  const responsive = await waitPageResponsive();
  check(responsive, "the page's thread runs again after the preview closed (window.print() returned)");
  const events = await printEvents();
  result.print.events = events;
  info(`window.print() events: ${events.map((e) => `${e.kind}${e.theme !== undefined ? ` [theme ${e.theme}, notice ${JSON.stringify(e.notice)}]` : ""}`).join(" -> ")}`);
  const called = events.find((e) => e.kind === "print() called");
  check(called !== undefined && called.notice === "Preparing to print…" && called.theme === "light", `window.print() was called in print mode: the "Preparing to print…" line shown and data-theme light (${JSON.stringify(called)})`);
  check(called !== undefined && called.images?.length >= 1 && called.images.every(Boolean) && called.pending === 0 && called.tableRows === 2, `the page had waited for its image and drawn both table rows before printing (${JSON.stringify(called)})`);
  check(events.filter((e) => e.kind === "print() called").length === 1, "window.print() was called once");
  check(events.some((e) => e.kind === "beforeprint") && events.some((e) => e.kind === "afterprint"), "beforeprint and afterprint fired");
  const after = await page.evaluate(`({ theme: document.documentElement.dataset.theme ?? null, notice: document.querySelector(".page-notice")?.textContent.trim() ?? null })`);
  check(after.notice === null, `print mode ended: the "Preparing to print…" line is gone (${JSON.stringify(after.notice)})`);
  check(after.theme === themeBefore, `print mode ended: data-theme is back to ${JSON.stringify(themeBefore)} (${JSON.stringify(after.theme)})`);
  await capture("print-after-escape");

  // --- B: Ctrl+P again and "Save as PDF" to a file ---
  const second = await openPrintPreview();
  const p2 = second.preview;
  let printer = await p2.evaluate(`(() => { ${DEEP} return deepAll(document, "button[role=combobox]")[0]?.textContent.trim() ?? null; })()`);
  info(`the printer the preview starts with: ${printer}`);
  if (printer !== "Save as PDF") {
    const combo = await previewRect(p2, "button[role=combobox]");
    await previewClick(p2, combo);
    const option = await previewRect(p2, "fluent-option", "Save as PDF");
    must(option !== null, 'the printer list offers "Save as PDF"');
    await previewClick(p2, option);
    printer = await p2.evaluate(`(() => { ${DEEP} return deepAll(document, "button[role=combobox]")[0]?.textContent.trim() ?? null; })()`);
  }
  check(printer === "Save as PDF", `the printer is "Save as PDF" (${printer})`);
  const save = await previewRect(p2, "fluent-button.print");
  check(save !== null && save.text === "Save", `the main button reads "Save" (${save?.text})`);
  await capture("print-save-as-pdf");
  await previewClick(p2, save);
  const dlg = await fileDialog("Save As", PDF_PATH);
  info(`the Save As dialog of the preview proposed "${dlg.initial}"`);
  result.print.proposedName = dlg.initial;
  const closed = await waitTarget("edge://print", false, 30000);
  check(closed, "the preview closed after Save");
  p2.close();
  check(await waitPageResponsive(), "the page runs again");
  const stray = await closeStrayTargets();
  info(`after saving the webview opened and this pass closed: ${stray.join(", ") || "nothing"}`);
  const events2 = await printEvents();
  check(events2.filter((e) => e.kind === "afterprint").length === 2 && events2.filter((e) => e.kind === "print() called").length === 2, `afterprint fired for the second print as well (${events2.map((e) => e.kind).join(", ")})`);
  const after2 = await page.evaluate(`({ theme: document.documentElement.dataset.theme ?? null, notice: document.querySelector(".page-notice")?.textContent.trim() ?? null })`);
  check(after2.notice === null && after2.theme === themeBefore, `print mode ended again: data-theme ${JSON.stringify(after2.theme)}, notice ${JSON.stringify(after2.notice)}`);
  for (let i = 0; i < 20 && !existsSync(PDF_PATH); i++) await wait(250);
  check(existsSync(PDF_PATH), "interop-print.pdf was written");
  if (existsSync(PDF_PATH)) {
    const bytes = readFileSync(PDF_PATH);
    const head = bytes.subarray(0, 8).toString("latin1");
    const pages = (bytes.toString("latin1").match(/\/Type\s*\/Page(?![s\w])/g) ?? []).length;
    info(`the PDF is ${bytes.length} bytes, starts with "${head}", has ${pages} page object(s)`);
    check(head.startsWith("%PDF-"), `the file starts with %PDF (${head})`);
    check(bytes.length > 5000, `it is not empty (${bytes.length} bytes)`);
    check(pages === 1, `it has 1 page (${pages})`);
    result.print.pdf = { bytes: bytes.length, head, pages };
  }
  await capture("print-done");
}

// ----- phase: backup -------------------------------------------------------------------------------------------------

const BACKUP_NAME = /^atomik-kb-auto-(\d{4})-(\d{2})-(\d{2})-(\d{2})(\d{2})\.atomikkb$/;
const SETTING = async (key) => (await ask({ type: "SettingsGet", data: { key } })) ?? null;

async function phaseBackup() {
  const state = readState();
  rmSync(BACKUP_DIR, { recursive: true, force: true });
  mkdirSync(BACKUP_DIR, { recursive: true });
  // Files that are not scheduled backups: none of them may be touched.
  const decoys = { "readme.txt": "not a backup", "atomik-kb-2020-01-01.atomikkb": "a manual backup", "atomik-kb-auto-notes.atomikkb": "almost, but not the name of a scheduled one" };
  for (const [name, text] of Object.entries(decoys)) writeFileSync(join(BACKUP_DIR, name), text);
  const scheduled = () => readdirSync(BACKUP_DIR).filter((n) => BACKUP_NAME.test(n)).sort();

  await normalize();
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor(".scheduled-backups", "the Scheduled backups section", 20000, null, true);
  const section = (selector) => `.scheduled-backups ${selector}`;

  // Before a folder is chosen the controls are off.
  const before = await page.evaluate(`(() => { const s = document.querySelector(".scheduled-backups"); return { folder: s.querySelector(".scheduled-backups-folder-row span")?.textContent.trim(), enabled: s.querySelector(".scheduled-backups-enabled input").disabled, now: [...s.querySelectorAll(".row-actions button")][0].disabled, note: s.querySelector(".note")?.textContent.trim() ?? null, last: s.querySelector(".scheduled-backups-last")?.innerText.replace(/\\s+/g, " ").trim() }; })()`);
  info(`before: ${JSON.stringify(before)}`);
  check(before.folder === "No folder chosen" && before.enabled === true && before.now === true, "without a folder, the switch and Back up now are off");
  check(before.last.includes("No scheduled backup has run yet."), `the last result says nothing has run (${before.last})`);
  check((await SETTING("app.backup.folder")) === null, "the setting app.backup.folder is empty");

  // The folder: the shell's own Select Folder dialog.
  await clickOn(".scheduled-backups button", "Choose folder…", { text: "Choose folder…" });
  await fileDialog("Select Folder", BACKUP_DIR);
  const shown = await waitUntil(`document.querySelector(".scheduled-backups-folder-row .mono")?.textContent.trim() ?? null`, "the chosen folder is shown");
  const norm = (p) => p.replaceAll("/", "\\").replace(/\\+$/, "").toLowerCase();
  check(norm(shown) === norm(BACKUP_DIR), `the folder shown is the folder chosen (${shown})`);
  const stored = await SETTING("app.backup.folder");
  check(typeof stored === "string" && norm(stored) === norm(BACKUP_DIR), `the shell stored it in app.backup.folder (${JSON.stringify(stored)})`);
  await capture("backup-folder");

  // On: the switch, then keep 2.
  const enabledBox = await waitFor(section(".scheduled-backups-enabled input"), "the Back up automatically switch");
  check(await page.evaluate(`!document.querySelector(".scheduled-backups-enabled input").disabled`), "with a folder, the switch is on offer");
  const wasOn = (await SETTING("app.backup.schedule"))?.enabled === true;
  info(`the schedule is ${wasOn ? "already on (left from an earlier run)" : "off"} before the switch is used`);
  if (!wasOn) await mouse(enabledBox.cx, enabledBox.cy);
  let schedule = null;
  for (let i = 0; i < 20; i++) {
    schedule = await SETTING("app.backup.schedule");
    if (schedule?.enabled === true) break;
    await wait(250);
  }
  check(schedule?.enabled === true && schedule?.interval === "daily", `the schedule is on, every day (${JSON.stringify(schedule)})`);
  await clickOn(section(".scheduled-backups-keep input"), "the backups-to-keep field");
  await page.evaluate(`document.querySelector(".scheduled-backups-keep input").select()`);
  await send("Input.insertText", { text: "2" });
  await page.key("Enter", "Enter", 13);
  for (let i = 0; i < 20; i++) {
    schedule = await SETTING("app.backup.schedule");
    if (schedule?.keep === 2) break;
    await wait(250);
  }
  check(schedule?.keep === 2 && schedule?.enabled === true, `keep is 2 and the schedule is still on (${JSON.stringify(schedule)})`);

  const sizes = [];
  /** "Back up now": the button, the notice, the new file, the stored result. */
  async function backUpNow(label) {
    const lastBefore = (await SETTING("app.backup.last"))?.at ?? 0;
    const filesBefore = scheduled();
    const t0 = Date.now();
    const clock = new Date();
    await clickOn(".scheduled-backups .row-actions button", "Back up now", { text: "Back up now" });
    const notice = await waitUntil(`(() => { const p = document.querySelector('.scheduled-backups p[role="status"]'); const t = p?.textContent.trim() ?? ""; const b = [...document.querySelectorAll(".scheduled-backups .row-actions button")][0]; return t.startsWith("Backup written:") && b && !b.disabled ? t : null; })()`, `${label}: the Back up now notice`, 120000);
    const seconds = round((Date.now() - t0) / 1000);
    let last = null;
    for (let i = 0; i < 20; i++) {
      last = await SETTING("app.backup.last");
      if (last !== null && last.at > lastBefore) break;
      await wait(250);
    }
    const file = notice.replace("Backup written: ", "");
    const m = BACKUP_NAME.exec(file);
    const pad = (n) => String(n).padStart(2, "0");
    const expectedStamp = `${clock.getFullYear()}-${pad(clock.getMonth() + 1)}-${pad(clock.getDate())}-${pad(clock.getHours())}${pad(clock.getMinutes())}`;
    info(`${label}: "${notice}" after ${seconds} s; stored result ${JSON.stringify(last)}`);
    check(m !== null, `${label}: the file is named atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb (${file})`);
    const minuteDiff = m === null ? 99 : Math.abs(new Date(`${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:00`).getTime() - new Date(`${expectedStamp.slice(0, 10)}T${expectedStamp.slice(11, 13)}:${expectedStamp.slice(13, 15)}:00`).getTime()) / 60000;
    check(minuteDiff <= 1, `${label}: its name carries the local time (${file}, the clock said ${expectedStamp})`);
    check(existsSync(join(BACKUP_DIR, file)), `${label}: the file exists in the folder (${existsSync(join(BACKUP_DIR, file)) ? statSync(join(BACKUP_DIR, file)).size : 0} bytes)`);
    check(last !== null && last.ok === true && last.file === file, `${label}: app.backup.last says it worked (${JSON.stringify(last)})`);
    const lastLine = await page.evaluate(`document.querySelector(".scheduled-backups-last")?.innerText.replace(/\\s+/g, " ").trim() ?? ""`);
    check(lastLine.includes("Backed up") && lastLine.includes(file) && !/Failed/.test(lastLine), `${label}: the last-result line shows success (${lastLine})`);
    check(await page.evaluate(`!document.querySelector('.scheduled-backups [role="alert"]')`), `${label}: no error is shown`);
    check(readdirSync(BACKUP_DIR).every((n) => !n.endsWith(".tmp")), `${label}: no .tmp file is left in the folder`);
    return { file, seconds, filesBefore, last };
  }

  const first = await backUpNow("run 1");
  result.backup = { runs: [{ file: first.file, seconds: first.seconds }] };
  sizes.push(statSync(join(BACKUP_DIR, first.file)).size);
  await capture("backup-run1");
  // The file is encrypted: nothing of the workspace is readable in it.
  const bytes = readFileSync(join(BACKUP_DIR, first.file));
  const plain = ["Source", "Alpha task", "Interop workspace", "A paragraph in the source page", "SQLite format"].filter((w) => bytes.includes(Buffer.from(w)));
  check(plain.length === 0, `the backup file holds no readable page title or text (${plain.join(", ") || "none found"}); it starts with ${bytes.subarray(0, 8).toString("hex")}`);
  check(bytes.length > 2000, `it is a real file, not a stub (${bytes.length} bytes)`);
  check(JSON.stringify(scheduled()) === JSON.stringify([first.file]), `one scheduled backup in the folder (${scheduled().join(", ")})`);

  info("waiting 61 s so that the second backup has another minute in its name");
  await wait(61000);
  const second = await backUpNow("run 2");
  result.backup.runs.push({ file: second.file, seconds: second.seconds });
  check(second.file !== first.file, `the second run wrote another file (${second.file})`);
  check(scheduled().length === 2, `two scheduled backups are kept (${scheduled().join(", ")})`);

  info("waiting 61 s again for the third");
  await wait(61000);
  const third = await backUpNow("run 3");
  result.backup.runs.push({ file: third.file, seconds: third.seconds });
  const kept = scheduled();
  check(kept.length === 2 && kept.includes(second.file) && kept.includes(third.file), `keep is 2: the two newest remain (${kept.join(", ")})`);
  check(!existsSync(join(BACKUP_DIR, first.file)), `the oldest, ${first.file}, was deleted`);
  for (const [name, text] of Object.entries(decoys)) {
    check(existsSync(join(BACKUP_DIR, name)) && readFileSync(join(BACKUP_DIR, name), "utf8") === text, `${name} was not touched`);
  }
  await capture("backup-run3");

  // Two backups in the same minute share a name: the second one replaces the first.
  const stamp = statSync(join(BACKUP_DIR, third.file));
  const fourth = await backUpNow("run 4, in the same minute as run 3");
  const replaced = statSync(join(BACKUP_DIR, fourth.file));
  info(`run 4: ${fourth.file === third.file ? "the same name as run 3: the file was replaced" : "another name"}; run 3's file was ${stamp.size} bytes at ${stamp.mtime.toISOString()}, now ${replaced.size} bytes at ${replaced.mtime.toISOString()}`);
  result.backup.sameMinute = { sameName: fourth.file === third.file, replaced: replaced.mtimeMs > stamp.mtimeMs };
  check(scheduled().length === 2, `still two scheduled backups after a run in the same minute (${scheduled().join(", ")})`);
  const finalSchedule = await SETTING("app.backup.schedule");
  check(finalSchedule?.enabled === true && finalSchedule.keep === 2, `the schedule is still on, keep 2 (${JSON.stringify(finalSchedule)})`);
}

// ----- phase: guard --------------------------------------------------------------------------------------------------

async function phaseGuard() {
  const folderBefore = await SETTING("app.backup.folder");
  const lastBefore = await SETTING("app.backup.last");
  const decoyFolder = join(OUT, "interop-guard-folder");
  // The page's own IPC, as the app's code calls it: invoke("dispatch", { request }).
  const refused = await core({ type: "SettingsSet", data: { key: "app.backup.folder", value: decoyFolder } });
  info(`SettingsSet app.backup.folder: ${JSON.stringify(refused)}`);
  check(refused.ok === false && refused.error?.code === "InvalidInput", `SettingsSet of app.backup.folder is refused with InvalidInput (${JSON.stringify(refused.error)})`);
  check(refused.error?.details?.field === "key", `... on the field "key" (${JSON.stringify(refused.error?.details)})`);
  check(!String(refused.error?.message ?? "").includes(decoyFolder), "the message does not repeat the value");
  const lastRefused = await core({ type: "SettingsSet", data: { key: "app.backup.last", value: { at: 1, ok: true } } });
  check(lastRefused.ok === false && lastRefused.error?.code === "InvalidInput" && lastRefused.error?.details?.field === "key", `app.backup.last is refused too (${JSON.stringify(lastRefused.error)})`);
  check(JSON.stringify(await SETTING("app.backup.folder")) === JSON.stringify(folderBefore), "app.backup.folder still holds what the shell stored");
  check(JSON.stringify(await SETTING("app.backup.last")) === JSON.stringify(lastBefore), "app.backup.last is unchanged");
  check(!existsSync(decoyFolder), "no folder was made for the refused value");
  // A control: a setting the webview may write is accepted (the check can fail).
  const scheduleBefore = await SETTING("app.backup.schedule");
  const allowed = await core({ type: "SettingsSet", data: { key: "app.backup.schedule", value: scheduleBefore ?? { enabled: false, interval: "daily", keep: 7 } } });
  check(allowed.ok === true, `a control: SettingsSet of app.backup.schedule (the value it has) is accepted (${JSON.stringify(allowed.error ?? "ok")})`);
  const other = await core({ type: "SettingsSet", data: { key: "interop.control", value: "x" } });
  info(`a control: SettingsSet of another key: ${other.ok ? "accepted" : JSON.stringify(other.error)}`);
  // Through the transport the UI itself uses, too: the Tauri invoke with the same request.
  const viaInvoke = await page.evaluate(`(async () => { try { await window.__TAURI_INTERNALS__.invoke("dispatch", { request: { type: "SettingsSet", data: { key: "app.backup.folder", value: "C:\\\\Windows" } } }); return "accepted"; } catch (e) { return JSON.stringify(e); } })()`);
  check(viaInvoke !== "accepted" && viaInvoke.includes("InvalidInput"), `a second refusal, another value (${viaInvoke})`);
}

// ----- phase: cancel -------------------------------------------------------------------------------------------------

/** Cancels the native dialog the page has just asked for. */
async function cancelDialog(title) {
  const answer = await helper("dialog", { title, cancel: true });
  info(`dialog "${title}" cancelled: ${answer.closed ? "closed" : "STILL OPEN"}${answer.error ? ` (${answer.error})` : ""}`);
  check(answer.ok === true, `the "${title}" dialog closed when cancelled`);
}

/** The text of the dialog's error line, its title and its buttons. */
const dialogState = () =>
  page.evaluate(`(() => {
    const d = document.querySelector('[role="dialog"]');
    if (!d) return null;
    return { title: d.querySelector(".subtitle")?.textContent.trim() ?? null, error: d.querySelector(".error-text")?.textContent.trim() ?? null, buttons: [...d.querySelectorAll("button")].map((b) => ({ text: b.textContent.trim(), disabled: b.disabled })) };
  })()`);

async function phaseCancel() {
  const state = readState();
  const space = state.spaceId;
  const toastsBefore = await toastTexts();

  // Import: a closed source dialog says nothing and leaves the dialog at its first step, for each of the three sources.
  await openPageId(space, state.sourceId);
  await spaceMenu("Import…");
  await waitFor('[role="dialog"]', "the Import dialog");
  for (const [button, title] of [
    ["Choose a .zip file…", "Open"],
    ["Choose files…", "Open"],
    ["Choose a folder…", "Select Folder"],
  ]) {
    await clickOn('[role="dialog"] button', `the "${button}" button`, { text: button });
    await cancelDialog(title);
    await wait(700);
    const st = await dialogState();
    check(st !== null && st.title === "Import" && st.error === null && st.buttons.some((b) => b.text === button && !b.disabled), `after cancelling "${button}" the Import dialog is back at its first step, with no message (${JSON.stringify(st)})`);
  }
  await clickOn('[role="dialog"] button', "Cancel of the Import dialog", { text: "Cancel" });
  await waitUntil(`!document.querySelector('[role="dialog"]')`, "the Import dialog closed");

  // Export of a page and of the space: the dialog stays, ready for another choice, and no toast says anything.
  for (const [what, open] of [
    ["page", () => pageMenu("Export…")],
    ["space", () => spaceMenu("Export space…")],
  ]) {
    await open();
    await waitFor('[role="dialog"] button[type="submit"]', `the Export dialog of the ${what}`);
    await clickOn('[role="dialog"] button[type="submit"]', "the Export button");
    await cancelDialog("Save As");
    await wait(800);
    const st = await dialogState();
    check(st !== null && st.title.startsWith("Export") && st.error === null && st.buttons.some((b) => b.text === "Export" && !b.disabled), `after cancelling Save As the ${what}'s Export dialog is ready again, with no message (${JSON.stringify(st)})`);
    await clickOn('[role="dialog"] button', "Cancel of the Export dialog", { text: "Cancel" });
    await waitUntil(`!document.querySelector('[role="dialog"]')`, "the Export dialog closed");
  }

  // The CSV of a view.
  await openPageId(space, state.tasksId);
  await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row', "the table view of Tasks");
  await clickOn('.ProseMirror button[aria-label="View settings"]', "the view's settings button");
  await waitFor(".page-list-settings", "the view settings menu");
  await clickOn(".page-list-settings button", "Export CSV of the view settings menu", { text: "Export CSV" });
  await cancelDialog("Save As");
  await wait(800);
  check((await page.evaluate(`!document.querySelector('.page-notice[role="alert"], [role="alert"]')`)) === true, "after cancelling the CSV's Save As no error is shown");

  // The folder of scheduled backups.
  const folderBefore = await ask({ type: "SettingsGet", data: { key: "app.backup.folder" } });
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor(".scheduled-backups", "the Scheduled backups section", 20000, null, true);
  await clickOn(".scheduled-backups button", "Choose folder…", { text: "Choose folder…" });
  await cancelDialog("Select Folder");
  const note = await waitUntil(`document.querySelector('.scheduled-backups p[role="status"]')?.textContent.trim() ?? null`, "the folder note");
  check(note === "No folder was chosen.", `cancelling the folder dialog says "No folder was chosen." (${note})`);
  const folderAfter = await ask({ type: "SettingsGet", data: { key: "app.backup.folder" } });
  check(JSON.stringify(folderBefore) === JSON.stringify(folderAfter), `app.backup.folder is as it was (${JSON.stringify(folderAfter)})`);

  const toastsAfter = await toastTexts();
  check(toastsAfter.filter((t) => !toastsBefore.includes(t)).length === 0, `no toast appeared for any cancelled dialog (${toastsAfter.filter((t) => !toastsBefore.includes(t)).join(" | ") || "none"})`);
}

// ----- phase: import-hostile -----------------------------------------------------------------------------------------

const HOSTILE_ZIP = join(SRC, "hostile.zip");

async function phaseImportHostile() {
  const state = readState();
  const space = state.spaceId;
  mkdirSync(SRC, { recursive: true });
  // A nested zip (Notion's Part-N.zip) and a file that is not a zip.
  const innerPath = join(SRC, "inner-part.zip");
  const inner = await helper("zip-build", { zip: innerPath, entries: [{ name: "Inner page.md", text: "# Inner page\n\nFrom the nested archive.\n" }] });
  must(inner.ok === true, "the nested archive was written");
  const entries = [
    { name: "../escape-one.md", text: "# Escaped one\n\nA zip-slip entry." },
    { name: "..\\escape-two.md", text: "# Escaped two" },
    { name: "/absolute-three.md", text: "# Absolute three" },
    { name: "C:/drive-four.md", text: "# Drive four" },
    { name: ".hidden/Secret.md", text: "# Secret" },
    { name: "__MACOSX/._Junk.md", text: "junk" },
    {
      name: "ok/Fine page.md",
      text: [
        "# Fine page",
        "",
        "A fine page with a [bad link](javascript:alert(1)) and a remote picture ![remote](https://example.com/pixel.png) and some <script>window.__pwned = 1</script> inline script.",
        "",
        "A [mail link](mailto:someone@example.com) and a [web link](https://example.com/page).",
        "",
      ].join("\n"),
    },
    {
      name: "Scripted.html",
      text: '<!doctype html><html><head><title>Scripted</title><script>window.__pwned = 2</script></head><body><h1>Scripted</h1><p onclick="window.__pwned = 3">Click me</p><iframe src="https://example.com/frame"></iframe><img src="https://example.com/track.png" alt="tracker"><script>window.__pwned = 4</script><p>After the scripts.</p></body></html>',
    },
    { name: "Part-1.zip", base64: readFileSync(innerPath).toString("base64") },
    { name: "bad.zip", text: "this is not a zip file" },
  ];
  const built = await helper("zip-build", { zip: HOSTILE_ZIP, entries });
  must(built.ok === true, "hostile.zip was written");
  const listed = await readZip(HOSTILE_ZIP);
  info(`hostile.zip entries: ${listed.names.join(" | ")}`);

  const before = livePages(await treeOf(space)).length;
  await openPageId(space, state.sourceId);
  await spaceMenu("Import…");
  await waitFor('[role="dialog"]', "the Import dialog");
  const { report, seconds } = await importThroughDialog("Choose a .zip file…", "Open", HOSTILE_ZIP);
  await capture("import-hostile-report");
  result.importHostile = { seconds, facts: report.facts, warnings: report.groups };
  const skipped = report.groups.find((g) => /skipped/i.test(g.label));
  info(`skipped: ${skipped ? skipped.items.join(" | ") : "(no group)"}`);
  check(skipped !== undefined && skipped.items.length >= 4, `the four unsafe entries are listed as skipped (${skipped?.items.length ?? 0})`);
  const flagged = (needle) => skipped?.items.some((i) => i.includes(needle)) === true;
  check(flagged("escape-one") && flagged("escape-two") && flagged("absolute-three") && flagged("drive-four"), "each of the four is named in the report");
  check(report.groups.some((g) => /could not be imported/i.test(g.label)), `the script, iframe and event handler of the HTML are reported as content that could not be imported (${warningLabelsOf(report).join("; ")})`);
  check(report.groups.some((g) => /web \(kept as links/i.test(g.label)), "remote images are reported as kept as links, not downloaded");
  await openFromReport();

  const tree = await treeOf(space);
  const all = livePages(tree);
  const titles = all.map((p) => p.title);
  info(`pages now: ${titles.join(", ")}`);
  for (const bad of ["Escaped one", "Escaped two", "Absolute three", "Drive four", "Secret", "Junk", "._Junk"]) {
    check(!titles.includes(bad), `no page "${bad}" was made from an unsafe or hidden entry`);
  }
  const container = tree.roots.find((n) => n.page.title === "hostile");
  const under = container ? importedPaths(tree, [container.page.id]).map((p) => p.path) : [];
  info(`imported: ${under.join(" | ")}`);
  check(container !== undefined && under.some((p) => p.endsWith("Fine page")) && under.some((p) => p.endsWith("Scripted")) && under.some((p) => p.endsWith("Inner page")), `the safe pages came in, the nested archive's page too (${under.join(", ")})`);
  check(all.length - before === Number(report.facts["Pages"]), `the tree grew by the number of pages the report counts (${before} -> ${all.length}, report ${report.facts["Pages"]})`);

  // Nothing was written outside: no escaped file next to the data, in the pass folder, at the root of C:, or in the temp folder.
  const found = [];
  const scan = (dir, depth) => {
    if (depth > 6) return;
    for (const name of readdirSync(dir)) {
      const full = join(dir, name);
      if (/^(escape-one|escape-two|absolute-three|drive-four)\.md$/.test(name) || name === "._Junk.md") found.push(full);
      try {
        if (statSync(full).isDirectory()) scan(full, depth + 1);
      } catch {
        // unreadable: skipped
      }
    }
  };
  scan(OUT, 0);
  for (const name of ["escape-one.md", "escape-two.md", "absolute-three.md", "drive-four.md"]) {
    for (const dir of ["C:\\", join(OUT, ".."), join(OUT, "..", ".."), process.env.TEMP ?? "C:\\Windows\\Temp"]) {
      if (existsSync(join(dir, name))) found.push(join(dir, name));
    }
  }
  check(found.length === 0, `no file of a skipped entry was written anywhere (${found.join(", ") || "none found"})`);

  // The pages: what they hold is text, not active content.
  const fine = all.find((p) => p.title === "Fine page");
  await openPageId(space, fine.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Fine page"`, "Fine page is open");
  const dom = await page.evaluate(`(() => {
    const root = document.querySelector(".content-body > section.page .ProseMirror");
    return {
      text: root.innerText,
      scripts: root.querySelectorAll("script, iframe, object, embed").length,
      jsLinks: [...root.querySelectorAll("a")].filter((a) => /^\\s*javascript:/i.test(a.getAttribute("href") ?? "")).length,
      links: [...root.querySelectorAll("a")].map((a) => a.getAttribute("href")),
      images: root.querySelectorAll("img").length,
      pwned: window.__pwned ?? null,
    };
  })()`);
  info(`Fine page: ${JSON.stringify(dom).slice(0, 500)}`);
  check(dom.scripts === 0 && dom.jsLinks === 0 && dom.pwned === null, `Fine page has no script, iframe or javascript: link, and nothing ran (${JSON.stringify({ scripts: dom.scripts, jsLinks: dom.jsLinks, pwned: dom.pwned })})`);
  check(dom.images === 0 && dom.links.includes("https://example.com/pixel.png"), `the remote picture is a plain link, not an <img> (${dom.images} images; links ${JSON.stringify(dom.links)})`);
  check(dom.links.includes("mailto:someone@example.com") && dom.links.includes("https://example.com/page"), "the mail and web links are links");
  check(dom.text.includes("bad link"), "the text of the javascript: link is kept as text");
  const scripted = all.find((p) => p.title === "Scripted");
  await openPageId(space, scripted.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Scripted"`, "Scripted is open");
  const dom2 = await page.evaluate(`(() => {
    const root = document.querySelector(".content-body > section.page .ProseMirror");
    return { text: root.innerText, scripts: root.querySelectorAll("script, iframe, object, embed").length, handlers: [...root.querySelectorAll("*")].filter((e) => e.getAttributeNames().some((n) => n.startsWith("on"))).length, imgs: [...root.querySelectorAll("img")].map((i) => i.getAttribute("src")), pwned: window.__pwned ?? null };
  })()`);
  info(`Scripted: ${JSON.stringify(dom2).slice(0, 400)}`);
  check(dom2.scripts === 0 && dom2.handlers === 0 && dom2.pwned === null, `Scripted has no script, iframe or event handler, and nothing ran (${JSON.stringify({ scripts: dom2.scripts, handlers: dom2.handlers, pwned: dom2.pwned })})`);
  check(dom2.text.includes("After the scripts.") && dom2.text.includes("Click me"), "its text is kept");
  check(!dom2.text.includes("__pwned"), "the script's source is not shown as text");
  check(dom2.imgs.every((s) => !(s ?? "").includes("example.com")), `the tracker image is not an <img> of the page (${JSON.stringify(dom2.imgs)})`);
  await capture("import-hostile-scripted");
  // The page asked nobody else for anything: the network record of this phase has no foreign request (checked at its end).
}

const PHASE_FUNCS = {
  launch: phaseLaunch,
  seed: phaseSeed,
  fixtures: phaseFixtures,
  "import-zip": phaseImportZip,
  "import-folder": phaseImportFolder,
  "import-files": phaseImportFiles,
  "export-zip": phaseExportZip,
  reimport: phaseReimport,
  "export-html": phaseExportHtml,
  "export-space": phaseExportSpace,
  csv: phaseCsv,
  print: phasePrint,
  backup: phaseBackup,
  guard: phaseGuard,
  cancel: phaseCancel,
  "import-hostile": phaseImportHostile,
};

/** The webview's own document: nothing may have replaced it. */
async function intact(when) {
  const state = readState();
  const now = await page.evaluate(`(() => ({ marker: window.__interopMarker ?? null, href: location.href }))()`).catch(() => null);
  must(now !== null, `${when}: the window answers`);
  check(now.marker === state.marker && state.marker !== undefined, `${when}: the document was not replaced (marker ${now.marker === state.marker ? "kept" : "LOST: " + now.marker})`);
  check(now.href === state.href, `${when}: the address is unchanged (${now.href})`);
}

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
result.seconds = round((Date.now() - started) / 1000);
check(foreign.length === 0, `no request to another host (${foreign.length}${foreign.length > 0 ? ": " + foreign.map((f) => f.url.slice(0, 80)).slice(0, 3).join(" | ") : ""})`);
check(violations.length === 0, `no content-security-policy violation (${violations.length})`);
check(report.failures.length === 0, `no failed request (${report.failures.length})`);
const errors = report.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
check(errors.length === 0, `no console error (${errors.length}${errors.length > 0 ? ": " + errors.map((e) => e.text).slice(0, 3).join(" | ") : ""})`);
if (outFile) {
  writeFileSync(outFile, JSON.stringify(result, null, 2));
}
// The report file has every note; the console has what failed (or, with INTEROP_VERBOSE, everything).
const checks = result.notes.filter((n) => !n.startsWith("info:"));
console.log(
  JSON.stringify(
    {
      phase,
      ok: result.ok,
      seconds: result.seconds,
      checks: checks.length,
      failed: checks.filter((n) => n.startsWith("FAIL")),
      bugs: result.bugs,
      captures: result.captures,
      ...(process.env.INTEROP_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
page.close();
process.exit(result.ok ? 0 : 1);
