// The in-page half of the small-gaps pass (drive-s-gaps.ps1, ADR-0040): trusted mouse and key input, the DOM, the core (through the
// page's own IPC), the native side (Win32 helpers of the .ps1: the Open dialog, the menu bar, the tray, the hotkey, the window state)
// and the files, over the DevTools port of the app's WebView2 (the calls of cdp.mjs). Phases:
//
//   node drive-s-gaps.mjs mark            <outfile>  the marker of the document is (re)set (after every start of the binary)
//   node drive-s-gaps.mjs launch          <outfile>  the first page of a fresh workspace
//   node drive-s-gaps.mjs seed            <outfile>  pages (one three deep, one long), a table view with Cost, a toggle, a second space
//   node drive-s-gaps.mjs ui              <outfile>  path, info, duplicate, trash, toggle headings, footer, covers, look, accent, search, languages
//   node drive-s-gaps.mjs menu            <outfile>  the native menu bar, two items by WM_COMMAND, undo / copy / paste once
//   node drive-s-gaps.mjs deeplink-cold   <outfile>  the app was started with a page link: the page opens at the block
//   node drive-s-gaps.mjs deeplink-invalid <outfile> the app was started with an invalid link: ignored, no error
//   node drive-s-gaps.mjs deeplink-before <outfile>  the window is minimised before a second launch
//   node drive-s-gaps.mjs deeplink-forward <outfile> a second launch with a page link was forwarded to the running app
//   node drive-s-gaps.mjs deeplink-plain  <outfile>  a second launch with no link only brought the window to the front
//   node drive-s-gaps.mjs tray            <outfile>  tray icon, close to the tray, the quick capture shortcut, locked
//   node drive-s-gaps.mjs tray-quit       <outfile>  File > Quit by WM_COMMAND exits with code 0
//   node drive-s-gaps.mjs tray-closeoff   <outfile>  close to the tray off: the close button exits
//   node drive-s-gaps.mjs prerestart      <outfile>  facts saved, the language set to French (it must survive the restart)
//   node drive-s-gaps.mjs restart         <outfile>  after a restart on the same data everything is as it was
//   node drive-s-gaps.mjs logs            <outfile>  the log of the shell and the core: no panic, no ERROR, no text, no link, no id
//   node drive-s-gaps.mjs verify          <outfile>  the summary of every phase
//
// Every page phase writes what the page asked the network for, what failed, the console and the content-security-policy violations
// it saw, as JSON, and exits 1 when its own check fails, when the page asked another host for something, when a violation, a failed
// request or a console error was seen, or when the document of the webview was replaced (the marker `window.__sgMarker` is lost by a
// reload).

import { execFile } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync } from "node:zlib";

const CDP_PORT = Number(process.env.CDP_PORT ?? 9333);
const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.SG_OUT_DIR ?? join(HERE, "out");
const STATE_FILE = join(OUT, "s-gaps-state.json");
const PID_FILE = join(OUT, "s-gaps-pid.txt");
const SRC = join(OUT, "s-gaps-src");
const PHASES = [
  "launch",
  "seed",
  "ui",
  "menu",
  "deeplink-cold",
  "deeplink-invalid",
  "deeplink-before",
  "deeplink-forward",
  "deeplink-before2",
  "deeplink-plain",
  "deeplink-lockprep",
  "deeplink-locked",
  "tray",
  "tray-quit",
  "tray-closeoff",
  "prerestart",
  "restart",
];
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(OUT, { recursive: true });

function readState() {
  return existsSync(STATE_FILE) ? JSON.parse(readFileSync(STATE_FILE, "utf8")) : {};
}
function writeState(patch) {
  writeFileSync(STATE_FILE, JSON.stringify({ ...readState(), ...patch }, null, 2));
}
const round = (n) => Math.round(n * 10) / 10;
const appPid = () => (existsSync(PID_FILE) ? Number(readFileSync(PID_FILE, "utf8").trim()) : 0);

// The texts and values of this pass: they must never be in the log. (Ids and links are added from the state.)
const WORKSPACE = "Small gaps workspace";
const GARDEN = "Garden";
const TOMATOES = "Tomatoes";
const CHERRY = "Cherry";
const BUDGET = "Budget";
const ORCHARD = "Orchard plan";
const NOTES = "Notes";
const LONG = "Long page";
const WORK = "Work";
const WORK_PAGE = "Orchard budget";
const GARDEN_TEXT = "Compost and heirloom seeds go in the north bed.";
const TOMATOES_TEXT = "Tomatoes need full sun and a long stake.";
const CHERRY_TEXT = "Cherry tomatoes ripen first.";
const ORCHARD_A = "Plant apples along the south fence.";
const ORCHARD_B = "Pears prefer the shady corner.";
const WORK_TEXT = "Quarterly figures for the work orchard.";
const TOGGLE_SUMMARY = "Folded notes";
const ROWS = [
  { title: "Rent", cost: 1200 },
  { title: "Food", cost: 300 },
  { title: "Fuel", cost: 80 },
];
const COST_SUM = ROWS.reduce((n, r) => n + r.cost, 0);

// ---------------------------------------------------------------------------------------------------------------
// helper steps of the .ps1 (native dialogs, menus, tray, hotkey, window state): needed by the log scan too
// ---------------------------------------------------------------------------------------------------------------

let helperCount = 0;
/** Runs one of drive-s-gaps.ps1's helper steps; its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}) {
  helperCount += 1;
  const argFile = join(OUT, `s-gaps-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `s-gaps-helper-${process.pid}-${helperCount}.result.json`);
  writeFileSync(argFile, JSON.stringify({ pid: appPid(), ...params }));
  rmSync(resultFile, { force: true });
  return new Promise((resolvePromise, reject) => {
    execFile(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", process.env.SG_PS1, "-Step", step, "-Arg", argFile, "-Result", resultFile],
      { timeout: 600000, maxBuffer: 64 * 1024 * 1024 },
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

// ---------------------------------------------------------------------------------------------------------------
// logs: the scan of the log of the shell and the core
// ---------------------------------------------------------------------------------------------------------------

/** The log files of the pass, oldest first (`s-gaps.err.log`, `s-gaps-r1.err.log`, ..., `s-gaps-second.err.log`). */
const logFiles = () =>
  readdirSync(OUT)
    .filter((n) => /^s-gaps(-r\d+|-second\d*)?\.err\.log$/.test(n))
    .sort((a, b) => (a === "s-gaps.err.log" ? -1 : b === "s-gaps.err.log" ? 1 : a.localeCompare(b, undefined, { numeric: true })));
const stripAnsi = (text) => text.replace(/\u001b\[[0-9;]*m/g, "");

/** The core's debug line that names a new page's id. */
const CORE_CREATED_PAGE = /kb_core::pages: created page document page=/;

async function scanLogs() {
  const state = readState();
  const files = logFiles();
  const log = { files: [], lines: 0, panics: [], errors: [], warnings: [], leaks: [], needles: 0, infoLines: {}, createdPageLines: 0, outOfSync: [] };
  // Words and values: the texts typed here, the names of the pages, the links, the ids of the pages and blocks the links name.
  const named = [
    WORKSPACE, GARDEN, TOMATOES, CHERRY, BUDGET, ORCHARD, NOTES, LONG, WORK_PAGE, "Orchard budget", "heirloom", "Compost", "north bed", "Quarterly figures", "apples", "south fence", "shady corner",
    TOGGLE_SUMMARY, "Cherry tomatoes", "Section A", "Rent", "Fuel", "(copy)", "atomik-kb://", "not-a-uuid", "secret=1", "garden.png", "s-gaps-cover", "Scratch pad", "alpha beta", "Captured one", "Captured two", "Captured in Work", "Captured locked", "orchard pears", "apples pears", "Orchord", "zzzqqq", "Orchrad",
  ];
  const ids = [...(state.logNeedles ?? [])];
  if (typeof state.passphrase === "string") ids.push(state.passphrase);
  for (const link of Object.values(state.deepLinks ?? {})) ids.push(String(link));
  const needles = [...new Set([...named, ...ids])].filter((n) => typeof n === "string" && n.length >= 4);
  log.needles = needles.length;
  const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  for (const file of files) {
    const text = stripAnsi(readFileSync(join(OUT, file), "utf8"));
    const lines = text.split(/\r?\n/);
    log.files.push({ file, lines: lines.length });
    log.lines += lines.length;
    for (const line of lines) {
      if (/panicked|panic:|thread '.*' panicked/i.test(line)) log.panics.push(`${file}: ${line.slice(0, 300)}`);
      if (/(^|\s)ERROR(\s|$)/.test(line)) log.errors.push(`${file}: ${line.slice(0, 300)}`);
      if (/(^|\s)WARN(\s|$)/.test(line)) log.warnings.push(`${file}: ${line.slice(0, 300)}`);
      if (/text index out of sync/.test(line)) log.outOfSync.push(`${file}: ${line.slice(0, 300)}`);
      for (const key of ["deep link opened", "deep link ignored", "the window was hidden to the tray", "a second launch was forwarded", "the tray icon is ready", "quitting from the tray", "the desktop settings were applied"]) {
        if (line.includes(key)) log.infoLines[key] = (log.infoLines[key] ?? 0) + 1;
      }
    }
    for (const needle of needles) {
      if (!text.includes(needle)) continue;
      const alnum = /^[A-Za-z0-9 ]+$/.test(needle);
      const pattern = alnum ? new RegExp(`(?<![A-Za-z0-9_])${escapeRe(needle)}(?![A-Za-z0-9_])`) : new RegExp(escapeRe(needle));
      // The core's debug event for a page that was just made ("created page document page=<id>", kb_core::pages, existing behaviour at
      // debug level) names the page's id: it is counted, not a leak of the deep-link handling; any other line with an id is.
      const matching = lines.filter((l) => pattern.test(l));
      const hits = matching.filter((l) => !CORE_CREATED_PAGE.test(l));
      if (/^[0-9a-f-]{36}$/.test(needle)) log.createdPageLines += matching.length - hits.length;
      if (hits.length > 0) log.leaks.push({ needle: needle.length > 12 && /^[0-9a-f-]{36}/.test(needle) ? `(id ${needle.slice(0, 8)}...)` : needle.startsWith("atomik-kb://") ? "(a link)" : needle, file, count: hits.length, first: hits[0].slice(0, 200).replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, "<id>") });
    }
  }
  return log;
}

if (phase === "logs") {
  const log = await scanLogs();
  const ok = log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0 && log.outOfSync.length === 0;
  const result = { phase: "logs", ok, notes: [], log };
  result.notes.push(`${log.files.length > 0 ? "ok" : "FAIL"}: the log is there (${log.files.map((f) => `${f.file} ${f.lines} lines`).join(", ")})`);
  result.notes.push(`${log.panics.length === 0 ? "ok" : "FAIL"}: no panic (${log.panics.length})`);
  result.notes.push(`${log.errors.length === 0 ? "ok" : "FAIL"}: no ERROR line (${log.errors.length}${log.errors.length ? `: ${log.errors.slice(0, 3).join(" | ")}` : ""})`);
  result.notes.push(`${log.leaks.length === 0 ? "ok" : "FAIL"}: none of the page text, titles, deep-link URLs or page and block ids is in the log (${log.leaks.length} leaks${log.leaks.length ? `: ${log.leaks.map((l) => `${l.needle} x${l.count} in ${l.file}: ${l.first}`).join(" || ")}` : ""}; ${log.needles} needles)`);
  const control = /(?<![A-Za-z0-9_])Garden(?![A-Za-z0-9_])/.test("INFO a made-up line that says Garden in it") && !/(?<![A-Za-z0-9_])Garden(?![A-Za-z0-9_])/.test("INFO a line about Gardener");
  result.notes.push(`${control ? "ok" : "FAIL"}: control: the whole-word matcher finds a needle in a made-up line and not inside another word`);
  result.ok = result.ok && control;
  result.notes.push(`${log.outOfSync.length === 0 ? "ok" : "FAIL"}: no WARN says the text index is out of sync (${log.outOfSync.length}${log.outOfSync.length ? ": " + log.outOfSync.slice(0, 2).join(" | ") : ""})`);
  result.notes.push(`info: ${log.warnings.length} WARN lines; ${log.createdPageLines} lines are the core's debug event for a new page (it names the page id); the shell's own lines: ${JSON.stringify(log.infoLines)}`);
  if (outFile) writeFileSync(outFile, JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ phase, ok: result.ok, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, warnings: log.warnings.slice(0, 40), infoLines: log.infoLines, files: log.files }, null, 2));
  process.exit(result.ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// verify: the summary of the phase reports (no app needed)
// ---------------------------------------------------------------------------------------------------------------
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0, details: {}, bugs: [], notVerifiable: [], log: null };
  for (const name of PHASES) {
    const file = join(OUT, `s-gaps-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = { ok: report.ok, checks: report.notes.filter((n) => !n.startsWith("info:")).length, failed: report.notes.filter((n) => n.startsWith("FAIL")).length, seconds: report.seconds };
    summary.captures.push(...(report.captures ?? []));
    if (report.details !== undefined) summary.details[name] = report.details;
    summary.failedNotes.push(...report.notes.filter((n) => n.startsWith("FAIL")).map((n) => `${name}: ${n}`));
    summary.bugs.push(...(report.bugs ?? []).map((b) => `${name}: ${b}`));
    summary.notVerifiable.push(...report.notes.filter((n) => n.startsWith("info: NOT VERIFIABLE")).map((n) => `${name}: ${n}`));
    summary.foreignRequests += report.foreignRequests?.length ?? 0;
    summary.cspViolations += report.cspViolations?.length ?? 0;
    summary.failedRequests += (report.failures ?? []).filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED").length;
    summary.consoleErrors += (report.console ?? []).filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).length;
  }
  summary.log = await scanLogs();
  const log = summary.log;
  const logsReportOk = existsSync(join(OUT, "s-gaps-logs.json")) ? JSON.parse(readFileSync(join(OUT, "s-gaps-logs.json"), "utf8")).ok === true : true;
  summary.ok = Object.values(summary.phases).every((p) => p.ok) && log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0 && log.outOfSync.length === 0 && logsReportOk;
  writeFileSync(outFile, JSON.stringify(summary, null, 2));
  console.log(
    JSON.stringify(
      {
        phase: "verify",
        ok: summary.ok,
        phases: summary.phases,
        failedNotes: summary.failedNotes,
        bugs: summary.bugs,
        notVerifiable: summary.notVerifiable,
        foreignRequests: summary.foreignRequests,
        cspViolations: summary.cspViolations,
        failedRequests: summary.failedRequests,
        consoleErrors: summary.consoleErrors,
        log: { files: log.files, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, warnings: log.warnings.slice(0, 40), infoLines: log.infoLines },
      },
      null,
      2,
    ),
  );
  process.exit(summary.ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// the page
// ---------------------------------------------------------------------------------------------------------------

/**
 * A DevTools client for the app's page target (the same calls as cdp.mjs: trusted input, evaluation, a record of the network, the
 * console and the violations).
 */
async function connect() {
  const accept = (url) => url.startsWith("http://tauri.localhost") || url.startsWith("https://tauri.localhost") || url.startsWith("tauri://localhost");
  let target;
  for (let i = 0; i < 60 && target === undefined; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json`)).json();
      target = list.find((t) => t.type === "page" && accept(t.url));
    } catch {
      // not up yet
    }
    if (target === undefined) {
      await wait(1000);
    }
  }
  if (target === undefined) {
    throw new Error("no page target of the app on the DevTools port");
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
    } else {
      events.push(data);
    }
  });
  // A page that goes away (the app quit, as the tray steps make it) fails what is waiting for it, so that no await stays unsettled.
  let gone = false;
  ws.addEventListener("close", () => {
    gone = true;
    for (const entry of waiting.values()) entry.reject(new Error("the page went away"));
    waiting.clear();
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      if (gone) {
        reject(new Error("the page went away"));
        return;
      }
      const n = ++id;
      waiting.set(n, { resolve, reject });
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  await send("Network.enable");
  await send("Log.enable");
  await send("Runtime.enable");
  await send("Page.enable");
  // The window of a freshly started binary is often behind the terminal that started it, and a hidden window is not focused at all:
  // without this the page sees a blur and fields that commit on blur (the sidebar's rename field) close at once. The page behaves as if
  // its window had the focus.
  await send("Emulation.setFocusEmulationEnabled", { enabled: true }).catch(() => {});
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
    async key(key, code = key, windowsVirtualKeyCode = 0, modifiers = 0, extra = {}) {
      await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode, modifiers, ...extra });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode, modifiers });
    },
    /** Types text as key events (what a keyboard produces). */
    async type(text) {
      for (const ch of text) {
        await send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
      }
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
          failures.push({ requestId: e.params.requestId, error: e.params.errorText, blocked: e.params.blockedReason ?? null, canceled: e.params.canceled ?? false });
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
/** Something that could not be verified here, and why (listed in the summary). */
const notVerifiable = (note) => result.notes.push(`info: NOT VERIFIABLE: ${note}`);
/** A thing the app does that it should not (kept for the final report, besides the failed check that found it). */
const bug = (note) => result.bugs.push(note);

// ----- input ----------------------------------------------------------------------------------------------------

const BUTTON_BIT = { left: 1, right: 2, middle: 4 };
const send = (method, params) => page.send(method, params);
const MOD = { alt: 1, ctrl: 2, meta: 4, shift: 8 };

/** A mouse button pressed and released at a point (`modifiers` held: `MOD.ctrl` for a Ctrl+click). */
async function mouse(x, y, { button = "left", clickCount = 1, modifiers = 0 } = {}) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, modifiers });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, buttons: BUTTON_BIT[button], clickCount, modifiers });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, buttons: 0, clickCount, modifiers });
  await wait(250);
}

/** The pointer moved to a point, without a button (a hover). */
async function hover(x, y) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await wait(300);
}

/** A key with Ctrl held, as a trusted event (`k` -> Ctrl+K). */
async function ctrl(letter, extraModifiers = 0, extra = {}) {
  const upper = letter.toUpperCase();
  await page.key(letter, `Key${upper}`, upper.charCodeAt(0), MOD.ctrl | extraModifiers, extra);
  await wait(300);
}

/** One key, with the modifier keys pressed before it and released after it, as a keyboard does (each modifier is an event of its own). */
async function chord(modifierNames, key, code, vk) {
  const keys = { alt: ["Alt", "AltLeft", 18, MOD.alt], ctrl: ["Control", "ControlLeft", 17, MOD.ctrl], shift: ["Shift", "ShiftLeft", 16, MOD.shift] };
  let bits = 0;
  for (const name of modifierNames) {
    const [k, c, v, bit] = keys[name];
    bits |= bit;
    await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code: c, windowsVirtualKeyCode: v, modifiers: bits });
  }
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode: vk, modifiers: bits });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: vk, modifiers: bits });
  for (const name of [...modifierNames].reverse()) {
    const [k, c, v, bit] = keys[name];
    bits &= ~bit;
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: c, windowsVirtualKeyCode: v, modifiers: bits });
  }
  await wait(250);
}

/** A lone Alt press: Alt down and up, nothing between. */
async function loneAlt() {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: "Alt", code: "AltLeft", windowsVirtualKeyCode: 18, modifiers: MOD.alt });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Alt", code: "AltLeft", windowsVirtualKeyCode: 18, modifiers: 0 });
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
  must(false, `${what} (timed out after ${round(ms / 1000)} s)`);
  return last;
}

/** Waits until `expression` is truthy and says whether it was (no stop): for a check that may legitimately fail. */
async function becomes(expression, ms = 8000) {
  const end = Date.now() + ms;
  let last = null;
  while (Date.now() < end) {
    last = await page.evaluate(expression).catch(() => null);
    if (last) return last;
    await wait(200);
  }
  return last;
}

async function clickOn(selector, what, options = {}) {
  // Scrolled into view first (a button at the bottom of a tall panel).
  const found = await waitFor(selector, what, 20000, options.text ?? null, true);
  await wait(100);
  const settled = (await box(selector, options.text ?? null, true)) ?? found;
  await mouse(settled.cx, settled.cy, options);
  return settled;
}
/** The title shown in the main area. */
const heading = () => page.evaluate(`document.querySelector(".content-body h1")?.textContent.trim() ?? null`);
const norm = (s) => String(s ?? "").replace(/\s+/g, " ").trim();

/** Captures the window with PrintWindow (the helper `capture-main`: the app's window, found by its process). */
async function capture(name) {
  const path = join(OUT, `s-gaps-${name}.png`);
  try {
    const answer = await helper("capture-main", { path });
    if (answer.ok) {
      result.captures.push(path);
    } else {
      check(false, `capture ${name} failed (${answer.error ?? "no file"})`);
    }
  } catch (error) {
    check(false, `capture ${name} failed: ${error.message}`);
  }
}

/** Leaves nothing open from an earlier phase that failed half-way. */
async function normalize() {
  for (let i = 0; i < 4; i++) {
    const open = await page.evaluate(`!!document.querySelector('[role="menu"], [role="dialog"], [role="listbox"], .page-info, .cover-picker')`);
    if (!open) {
      return;
    }
    await page.key("Escape", "Escape", 27);
    await wait(250);
  }
}

// ----- native dialogs ---------------------------------------------------------------------------------------------

/** Drives a native file dialog that the page has just asked for: types `text` and closes it. */
async function fileDialog(title, text, timeoutMs = 30000) {
  const answer = await helper("dialog", { title, steps: [text], timeoutMs });
  info(`dialog "${title}": waited ${answer.waitedMs} ms, edit control ${answer.edit}, ${answer.closed ? "closed" : "STILL OPEN"}`);
  check(answer.ok === true, `the "${title}" dialog took the file and closed${answer.error ? ` (${answer.error}; seen ${JSON.stringify(answer.seen)}; controls ${JSON.stringify(answer.controls)})` : ""}`);
  return answer;
}

// ----- the core, through the page's own IPC -------------------------------------------------------------------------

/** One request to the core as the UI sends it (`invoke("dispatch", { request })`): `{ ok, value }` or `{ ok: false, error }`. */
async function core(request) {
  // A request without fields (`SpaceList`) is a unit variant: it has no `data`.
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
const spaceList = () => ask({ type: "SpaceList", data: {} });
const treeOf = (spaceId) => ask({ type: "TreeGet", data: { spaceId } });
const pageGet = (pageId) => ask({ type: "PageGet", data: { pageId } });
const SETTING = async (key) => (await ask({ type: "SettingsGet", data: { key } })) ?? null;
/** Every live page of a tree, depth first, with its path of titles. */
function flatten(nodes, trail = []) {
  const out = [];
  for (const node of nodes) {
    const path = [...trail, node.page.title];
    out.push({ id: node.page.id, title: node.page.title, path, depth: trail.length, parentId: node.page.parentId ?? null, children: node.children.length, node });
    out.push(...flatten(node.children, path));
  }
  return out;
}
const livePages = (tree) => flatten(tree.roots);
const pageContent = async (pageId) => (await ask({ type: "PageContentGet", data: { pageId } })).content;
function walkNodes(node, fn) {
  fn(node);
  for (const child of node.content ?? []) walkNodes(child, fn);
}
const searchHits = (query, extra = {}) => ask({ type: "Search", data: { query, ...extra } });

// ----- DOM helpers -----------------------------------------------------------------------------------------------

/** The menu item whose text starts with `label`. */
async function menuItem(label) {
  return page.evaluate(`(() => {
    const el = [...document.querySelectorAll('[role="menuitem"]')].find((e) => e.textContent.trim().startsWith(${JSON.stringify(label)}));
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2, disabled: el.disabled === true, text: el.textContent.trim() };
  })()`);
}
const menuLabels = () => page.evaluate(`[...document.querySelectorAll('[role="menuitem"]')].map((e) => e.textContent.trim())`);

async function chooseMenuItem(label) {
  let found = null;
  const end = Date.now() + 10000;
  while (Date.now() < end && found === null) {
    found = await menuItem(label);
    if (found === null) await wait(200);
  }
  if (found === null) must(false, `the menu has "${label}" (${(await menuLabels()).join(", ")})`);
  if (found.disabled) must(false, `"${label}" is enabled`);
  await mouse(found.cx, found.cy);
}

/** Opens the page menu ("Page options") of the open page and chooses `label` (null: leaves it open); returns the labels the menu showed. */
async function pageMenu(label) {
  await normalize();
  await clickOn('button[aria-label="Page options"]', "the page options button");
  await waitFor('[role="menu"]', "the page options menu");
  const labels = await menuLabels();
  if (label !== null) await chooseMenuItem(label);
  return labels;
}

/** Opens the actions menu of a sidebar row ("Page actions", shown on hover) and chooses `label`; returns the labels the menu showed. */
async function rowMenu(pageId, label) {
  await normalize();
  const row = await waitFor(`[role="treeitem"][data-page-id="${pageId}"] .tree-title`, "the sidebar row of the page", 20000, null, true);
  await hover(row.cx, row.cy);
  const button = await waitFor(`[role="treeitem"][data-page-id="${pageId}"] button[aria-label="Page actions"]`, "the row's Page actions button");
  await mouse(button.cx, button.cy);
  await waitFor('[role="menu"]', "the row's menu");
  const labels = await menuLabels();
  await chooseMenuItem(label);
  return labels;
}

/** The name of the current space as the sidebar shows it. */
const shownSpace = () => page.evaluate(`document.querySelector(".space-switcher .space-name")?.textContent.trim() ?? null`);

async function openSpaceList() {
  await normalize();
  await clickOn("button.space-button", "the space switcher button");
  await waitFor(".space-popover", "the list of spaces");
}

/** Makes `spaceId` the current space through the switcher (the nth entry of the list, by position). */
async function switchSpaceUi(spaceId) {
  const spaces = await spaceList();
  const index = spaces.findIndex((s) => s.id === spaceId);
  must(index >= 0, "the space to switch to exists");
  await openSpaceList();
  const items = await page.evaluate(`[...document.querySelectorAll(".space-popover .space-item")].map((b) => { const r = b.getBoundingClientRect(); return { cx: r.x + r.width / 2, cy: r.y + r.height / 2, current: b.getAttribute("aria-current") === "true" }; })`);
  must(items.length === spaces.length, `the switcher lists every space (${items.length} of ${spaces.length})`);
  await mouse(items[index].cx, items[index].cy);
  await waitUntil(`!document.querySelector(".space-popover")`, "the space list closed");
  await wait(600);
  await waitUntil(`!!document.querySelector('[role="tree"], .tree-new')`, "the sidebar shows the space's pages");
}

/** Creates a space with the switcher's "New space" form; the new space becomes the current one. */
async function createSpaceUi(name) {
  await openSpaceList();
  await clickOn(".space-popover button.menu-item", "New space", { text: "New space" });
  await waitFor(".space-popover .space-form input", "the space name field");
  await clickOn(".space-popover .space-form input", "the space name field");
  await send("Input.insertText", { text: name });
  await clickOn('.space-popover .space-form button[type="submit"]', "Create space");
  await waitUntil(`!document.querySelector(".space-popover")`, "the new space form closed", 20000);
  await waitUntil(`document.querySelector(".space-switcher .space-name")?.textContent.trim() === ${JSON.stringify(name)}`, "the new space is the current one");
  await wait(500);
}

/** The sidebar rows of the ancestors of a page are expanded (their chevrons clicked) so that the page's own row is shown. */
async function expandAncestors(spaceId, id) {
  await normalize();
  const tree = await treeOf(spaceId);
  const all = livePages(tree);
  const target = all.find((p) => p.id === id);
  if (target === undefined) must(false, `the page is in the tree of its space`);
  const chain = [];
  for (let p = target; p !== undefined; p = all.find((q) => q.id === p.parentId)) {
    chain.unshift(p.id);
  }
  for (const [index, pageId] of chain.slice(0, -1).entries()) {
    await waitFor(`[role="treeitem"][data-page-id="${pageId}"]`, `the sidebar row of an ancestor (${index + 1} of ${chain.length - 1})`, 20000, null, true);
    const row = await page.evaluate(`(() => {
      const el = document.querySelector('[role="treeitem"][data-page-id="${pageId}"]');
      const c = el.querySelector("button.tree-chevron")?.getBoundingClientRect() ?? null;
      el.scrollIntoView({ block: "nearest" });
      return { expanded: el.getAttribute("aria-expanded"), chevron: c ? { cx: c.x + c.width / 2, cy: c.y + c.height / 2 } : null };
    })()`);
    if (row.expanded === "false" && row.chevron !== null) {
      await mouse(row.chevron.cx, row.chevron.cy);
      await wait(300);
    }
  }
  await waitFor(`[role="treeitem"][data-page-id="${id}"] .tree-title`, "the sidebar row of the page", 20000, null, true);
}

/** Opens a page by clicking its row in the sidebar (ancestors expanded first, the page found by its id) in the current space. */
async function openPageId(spaceId, id) {
  await expandAncestors(spaceId, id);
  const again = await box(`[role="treeitem"][data-page-id="${id}"] .tree-title`);
  await mouse(again.cx, again.cy);
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

/** `/query` typed with key events, the entry named `label` chosen (ArrowDown until it is the selected one), Enter. */
async function slash(query, label) {
  await page.type(`/${query}`);
  await wait(500);
  await waitFor("[role=listbox]", `the slash menu for /${query}`);
  for (let i = 0; i < 40; i++) {
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

/** A page made inside another one with the row's "Add a page inside" button, named and left open. */
async function newChild(spaceId, parentId, title) {
  const row = await waitFor(`[role="treeitem"][data-page-id="${parentId}"] .tree-title`, "the parent row", 20000, null, true);
  await hover(row.cx, row.cy);
  const add = await waitFor(`[role="treeitem"][data-page-id="${parentId}"] button[aria-label="Add a page inside"]`, 'the "Add a page inside" button');
  await mouse(add.cx, add.cy);
  await waitFor("input.tree-rename", "the rename field of the new page");
  await send("Input.insertText", { text: title });
  await page.key("Enter", "Enter", 13);
  await wait(900);
  const made = livePages(await treeOf(spaceId)).find((p) => p.title === title && p.parentId === parentId);
  must(made !== undefined, `the page "${title}" exists under its parent`);
  return made.id;
}

/** Types `text` into the editor at a fresh line and leaves the editor, so the text is saved. */
async function writeLine(text) {
  await newLine();
  await send("Input.insertText", { text });
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
}

/** The text of the open page's editor, one string per top-level block. */
const editorBlocks = () => page.evaluate(`(() => { const e = document.querySelector(".ProseMirror")?.editor; if (!e) return null; const out = []; e.state.doc.forEach((n) => out.push(n.textContent)); return out; })()`);

// ----- files ------------------------------------------------------------------------------------------------------------

/** A PNG of `w` x `h` pixels whose colour runs from blue at the top to orange at the bottom (so a position of the picture is visible). */
function pngBytes(w = 600, h = 600) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) {
    const t = y / (h - 1);
    const row = y * (w * 3 + 1);
    raw[row] = 0;
    for (let x = 0; x < w; x++) {
      raw[row + 1 + x * 3] = Math.round(40 + 215 * t);
      raw[row + 2 + x * 3] = Math.round(120 + 40 * (1 - t) + 20 * Math.sin(x / 30));
      raw[row + 3 + x * 3] = Math.round(220 * (1 - t) + 20);
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
  header.writeUInt32BE(w, 0);
  header.writeUInt32BE(h, 4);
  header[8] = 8;
  header[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

// ---------------------------------------------------------------------------------------------------------------
// phase: mark, launch
// ---------------------------------------------------------------------------------------------------------------

async function phaseMark() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `sg-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__sgMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href });
  info(`marker set on ${href}`);
}

async function phaseLaunch() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const keep = readState();
  rmSync(STATE_FILE, { force: true }); // a fresh pass starts with no memory of an earlier one
  writeState({ marker: keep.marker, href: keep.href });
  const size = await page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio, theme: document.documentElement.dataset.theme ?? "(none)", lang: document.documentElement.lang })`);
  info(`window ${size.w}x${size.h} at ${size.dpr}x, data-theme ${size.theme}, html lang "${size.lang}"`);
  check((await heading()) === WORKSPACE, `the main area shows the workspace's Home (${await heading()})`);
  const spaces = await spaceList();
  must(spaces.length === 1, `the workspace has one space (${spaces.length})`);
  writeState({ spaceA: { id: spaces[0].id, name: spaces[0].name } });
  info(`the first space is "${spaces[0].name}"`);
  const tree = await treeOf(spaces[0].id);
  check(tree.roots.length === 0, `the space starts empty (${tree.roots.length} pages)`);
  check(existsSync(join(OUT, "s-gaps-data")), "the data directory is the pass's own (out/s-gaps-data)");
  // The passphrase of the app lock this pass turns on (made up here, for this data directory only; never printed).
  const bytes = new Uint8Array(12);
  crypto.getRandomValues(bytes);
  writeState({ passphrase: `sg ${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}` });
  check(!(await page.evaluate(`!!document.querySelector(".page-path, .page-cover")`)), "no page path and no cover on the Home screen");
  // Whether the atomik-kb scheme is registered for this user before the pass (an installed app does): the pass must leave it as it is.
  const scheme = await helper("reg-scheme", {});
  writeState({ scheme: { exists: scheme.exists, command: scheme.command } });
  info(`the atomik-kb scheme in the registry before the pass: ${scheme.exists ? "registered" : "not registered"}`);
}

// ---------------------------------------------------------------------------------------------------------------
// phase: seed
// ---------------------------------------------------------------------------------------------------------------

async function phaseSeed() {
  const s = readState();
  const A = s.spaceA;
  mkdirSync(SRC, { recursive: true });
  const ids = {};

  // Garden, Tomatoes inside it, Cherry inside Tomatoes: a page three deep (its path has two pages above it).
  await newPage(GARDEN);
  await writeLine(GARDEN_TEXT);
  const garden = livePages(await treeOf(A.id)).find((p) => p.title === GARDEN);
  must(garden !== undefined, "the core has Garden");
  ids.garden = garden.id;
  ids.tomatoes = await newChild(A.id, ids.garden, TOMATOES);
  await openPageId(A.id, ids.tomatoes);
  await writeLine(TOMATOES_TEXT);
  ids.cherry = await newChild(A.id, ids.tomatoes, CHERRY);
  await openPageId(A.id, ids.cherry);
  await writeLine(CHERRY_TEXT);

  // Budget: a table view of its pages (Rent, Food, Fuel) and a number property "Cost" they hold values of.
  await newPage(BUDGET);
  const budget = livePages(await treeOf(A.id)).find((p) => p.title === BUDGET);
  must(budget !== undefined, "the core has Budget");
  ids.budget = budget.id;
  await newLine();
  await slash("table", "Table view");
  must((await box('.ProseMirror [data-type="pageList"][data-layout="table"]')) !== null, "Budget has a table view");
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1500);
  ids.rows = [];
  for (const row of ROWS) {
    ids.rows.push(await newChild(A.id, ids.budget, row.title));
  }
  const cost = await ask({ type: "PropertyCreate", data: { spaceId: A.id, name: "Cost", kind: "number" } });
  ids.costProperty = cost.id;
  for (const [index, row] of ROWS.entries()) {
    await ask({ type: "PagePropertySet", data: { pageId: ids.rows[index], propertyId: cost.id, value: { kind: "number", value: row.cost } } });
  }

  // Orchard plan: two paragraphs; "Orchard" is in the title, "apples" in one block and "pears" in the other.
  await newPage(ORCHARD);
  await writeLine(ORCHARD_A);
  await writeLine(ORCHARD_B);
  ids.orchard = livePages(await treeOf(A.id)).find((p) => p.title === ORCHARD)?.id;
  must(ids.orchard !== undefined, "the core has Orchard plan");

  // Notes: a toggle (summary and a line inside it).
  await newPage(NOTES);
  await newLine();
  await slash("toggle", "Toggle");
  await send("Input.insertText", { text: TOGGLE_SUMMARY });
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  ids.notes = livePages(await treeOf(A.id)).find((p) => p.title === NOTES)?.id;
  must(ids.notes !== undefined, "the core has Notes");
  check((await page.evaluate(`document.querySelectorAll('.ProseMirror [data-type="details"]').length`)) === 1, "Notes has a toggle (a details block)");

  // Long page: 60 paragraphs, so that a link to a block near the end needs a scroll.
  await newPage(LONG);
  await page.evaluate(`(() => {
    const editor = document.querySelector(".ProseMirror").editor;
    editor.commands.insertContent(Array.from({ length: 60 }, (_, i) => ({ type: "paragraph", content: [{ type: "text", text: "Line " + (i + 1) + " of the long page." }] })));
    document.activeElement?.blur?.();
  })()`);
  await wait(2500);
  ids.long = livePages(await treeOf(A.id)).find((p) => p.title === LONG)?.id;
  must(ids.long !== undefined, "the core has the long page");
  const longContent = await pageContent(ids.long);
  const paragraphs = [];
  walkNodes(longContent, (node) => {
    if (node.type === "paragraph" && typeof node.attrs?.id === "string" && /^Line \d+ of/.test(node.content?.[0]?.text ?? "")) paragraphs.push({ id: node.attrs.id, text: node.content[0].text });
  });
  must(paragraphs.length === 60, `the long page holds its 60 paragraphs (${paragraphs.length})`);
  const target = paragraphs.find((p) => p.text.startsWith("Line 55 "));
  ids.longBlock = target.id;

  // A second space with a page whose title has the word "Orchard" too (the Space chip of the search), then back to the first one.
  await createSpaceUi(WORK);
  const spaces = await spaceList();
  const B = spaces.find((sp) => sp.name === WORK);
  must(B !== undefined && spaces.length === 2, `the second space "${WORK}" exists (${spaces.length} spaces)`);
  await newPage(WORK_PAGE);
  await writeLine(WORK_TEXT);
  ids.workPage = livePages(await treeOf(B.id)).find((p) => p.title === WORK_PAGE)?.id;
  must(ids.workPage !== undefined, "the second space has its page");
  await switchSpaceUi(A.id);
  await openPageId(A.id, ids.garden);

  // The state of the core.
  const tree = await treeOf(A.id);
  const pages = livePages(tree);
  check(pages.length === 10, `the first space holds ten pages (${pages.length}: ${pages.map((p) => p.title).join(", ")})`);
  const cherry = pages.find((p) => p.id === ids.cherry);
  check(cherry?.path.join(" > ") === `${GARDEN} > ${TOMATOES} > ${CHERRY}`, "Cherry is three deep (Garden > Tomatoes > Cherry)");
  const values = (await ask({ type: "PagePropertiesGet", data: { pageId: ids.rows[0] } })).values;
  check(values.some(([id, value]) => id === ids.costProperty && value.value === ROWS[0].cost), "Rent has Cost 1200");
  writeState({
    spaceB: { id: B.id, name: B.name },
    ids,
    // The deep links of the deeplink step: a page at a block (a cold start), an invalid one, a page to forward.
    deepLinks: {
      cold: `atomik-kb://page/${ids.long}#${ids.longBlock}`,
      invalid: "atomik-kb://page/not-a-uuid?secret=1",
      forward: `atomik-kb://page/${ids.orchard}`,
    },
    // Ids that must never be in the log (page ids, the block id).
    logNeedles: [ids.garden, ids.tomatoes, ids.cherry, ids.budget, ids.orchard, ids.notes, ids.long, ids.longBlock, ids.workPage, ...ids.rows],
  });
  await capture("seeded");
}

// ---------------------------------------------------------------------------------------------------------------
// phase: ui
// ---------------------------------------------------------------------------------------------------------------

const I18N_DIR = join(HERE, "..", "..", "packages", "ui", "src", "i18n");
const messagesOf = (language) => JSON.parse(readFileSync(join(I18N_DIR, `${language}.json`), "utf8"));
const spaceA = () => readState().spaceA;
const ids = () => readState().ids;
const openPage = (id) => openPageId(spaceA().id, id);

/** One part of a phase: a failed `must` in it ends only this part (what it left open is closed), the others still run. */
async function section(name, fn) {
  info(`--- ${name}`);
  try {
    await normalize();
    await fn();
  } catch (error) {
    if (!(error instanceof PhaseStop)) {
      check(false, `${name}: exception: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
    }
  }
  await normalize().catch(() => {});
}

/** The first button matching `selector` whose text starts with `prefix`, clicked. */
async function clickButtonStarting(selector, prefix, what) {
  const found = await page.evaluate(`(() => {
    const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => e.textContent.trim().startsWith(${JSON.stringify(prefix)}));
    if (!el) return null;
    el.scrollIntoView({ block: "nearest" });
    const r = el.getBoundingClientRect();
    return { cx: r.x + r.width / 2, cy: r.y + r.height / 2, w: r.width, h: r.height };
  })()`);
  must(found !== null, `${what} is there (${selector} starting with "${prefix}")`);
  await wait(100);
  await mouse(found.cx, found.cy);
  return found;
}

/** The text of the title of the open page. */
const titleOf = () => heading();

// ----- 1. the path above the title -------------------------------------------------------------------------------------

async function uiPath() {
  const A = spaceA();
  const I = ids();
  await openPage(I.garden);
  check((await box(".content-body .page-path")) === null, "a top-level page has no path above its title");
  await openPage(I.cherry);
  const trail = await page.evaluate(`(() => {
    const nav = document.querySelector(".content-body nav.page-path");
    const h1 = document.querySelector(".content-body h1");
    if (!nav || !h1) return null;
    const n = nav.getBoundingClientRect();
    const h = h1.getBoundingClientRect();
    return { label: nav.getAttribute("aria-label"), items: [...nav.querySelectorAll(".breadcrumb-item")].map((li) => li.textContent.trim()), links: nav.querySelectorAll("button.breadcrumb-link").length, navBottom: n.bottom, h1Top: h.top, navTop: n.top };
  })()`);
  must(trail !== null, "a page three deep shows the page path");
  check(JSON.stringify(trail.items) === JSON.stringify([A.name, GARDEN, TOMATOES]), `the path is the space, then the pages above (${trail.items.join(" > ")})`);
  check(trail.links === 2, `the two ancestors are buttons (${trail.links})`);
  check(trail.navBottom <= trail.h1Top + 1, `the path sits above the title (path bottom ${round(trail.navBottom)}, title top ${round(trail.h1Top)})`);
  check(trail.label === "Page path", `the path is labelled "Page path" (${trail.label})`);
  check(!trail.items.includes(CHERRY), "the page itself is not in its path: the title stands for it");
  await capture("path");
  await clickOn(".content-body nav.page-path button.breadcrumb-link", "the Tomatoes crumb", { text: TOMATOES });
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(TOMATOES)}`, "the crumb opens Tomatoes");
  check((await box(".content-body nav.page-path")) !== null, "Tomatoes, one level down, has a path (the space and Garden)");
}

// ----- 2. Page info ---------------------------------------------------------------------------------------------------

async function uiInfo() {
  const I = ids();
  await openPage(I.tomatoes);
  await page.evaluate(`document.activeElement?.blur?.()`);
  const labels = await pageMenu("Page info");
  check(labels.includes("Page info") && labels.includes("Duplicate") && labels.includes("Copy link to page"), `the page menu has Page info, Duplicate and Copy link to page (${labels.join(", ")})`);
  await waitFor(".page-info", "the Page info popover");
  const facts = await page.evaluate(`(() => {
    const dl = document.querySelector(".page-info .page-info-list");
    const dts = [...dl.querySelectorAll("dt")];
    const dds = [...dl.querySelectorAll("dd")];
    const out = {};
    dts.forEach((dt, i) => { out[dt.textContent.trim()] = dds[i]?.textContent.trim() ?? ""; });
    return { out, times: [...dl.querySelectorAll("time")].map((t) => t.getAttribute("datetime")), title: document.querySelector(".page-info .page-info-title")?.textContent.trim() };
  })()`);
  info(`Page info says ${JSON.stringify(facts.out)}`);
  const blocks = (await editorBlocks()).length;
  const device = await ask({ type: "DeviceInfo", data: {} });
  const words = TOMATOES_TEXT.split(/\s+/).length;
  check(facts.out.Words === String(words), `Page info counts ${words} words (${facts.out.Words})`);
  check(facts.out.Characters === String(TOMATOES_TEXT.length), `Page info counts ${TOMATOES_TEXT.length} characters (${facts.out.Characters})`);
  check(facts.out.Blocks === String(blocks), `Page info counts the editor's ${blocks} top-level blocks (${facts.out.Blocks})`);
  check(facts.out.Location === `${spaceA().name} › ${GARDEN}`, `Page info's location is the space and the pages above (${facts.out.Location})`);
  check(facts.times.length === 2 && facts.times.every((t) => !Number.isNaN(Date.parse(t))), `Created and Last edited are two times (${facts.times.join(", ")})`);
  const now = Date.now();
  check(facts.times.every((t) => Math.abs(now - Date.parse(t)) < 60 * 60 * 1000), "both times are within the hour (this pass made the page)");
  check((facts.out.Created ?? "").includes(device.name) && (facts.out["Last edited"] ?? "").includes(device.name), `Created and Last edited say this device's name ("${device.name}")`);
  check(facts.title === "Page info", "the popover is titled Page info");
  await capture("info");
  // It follows the text while it is open: a word typed in the editor changes the count.
  await page.key("Escape", "Escape", 27);
  await waitUntil(`!document.querySelector(".page-info")`, "Escape closes the popover");
  // The page menu's button has the focus back.
  const back = await page.evaluate(`document.activeElement?.getAttribute("aria-label") ?? null`);
  check(back === "Page options", `focus returns to the page options button (${back})`);
}

// ----- 3. Duplicate ---------------------------------------------------------------------------------------------------

async function uiDuplicate() {
  const A = spaceA();
  const I = ids();
  const beforePages = livePages(await treeOf(A.id));
  const beforeIds = new Set(beforePages.map((p) => p.id));
  // The sidebar row's menu.
  const labels = await rowMenu(I.garden, "Duplicate");
  check(labels.includes("Duplicate") && labels.indexOf("Duplicate") > labels.indexOf("Outdent") && labels.indexOf("Duplicate") < labels.indexOf("Rename"), `the row menu has Duplicate between Outdent and Rename (${labels.join(", ")})`);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Garden (copy)"`, "the copy opens, titled Garden (copy)");
  let tree = await treeOf(A.id);
  const roots = tree.roots.map((n) => n.page.title);
  const at = roots.indexOf(GARDEN);
  check(roots[at + 1] === "Garden (copy)", `the copy sits right after Garden among its siblings (${roots.join(", ")})`);
  const copyNode = tree.roots[at + 1];
  const sub = livePages({ roots: copyNode.children });
  check(JSON.stringify(sub.map((p) => p.title)) === JSON.stringify([TOMATOES, CHERRY]) && sub[1].depth === 1, `the copy has the sub-pages Tomatoes and Cherry under it, keeping their titles (${sub.map((p) => p.title).join(", ")})`);
  const copyIds = [copyNode.page.id, ...sub.map((p) => p.id)];
  check(copyIds.every((id) => !beforeIds.has(id)), "every page of the copy has a new id");
  const original = await pageContent(I.garden);
  const duplicate = await pageContent(copyNode.page.id);
  const textsOf = (content) => {
    const out = [];
    walkNodes(content, (n) => { if (n.type === "text") out.push(n.text); });
    return out.join("|");
  };
  const blockIdsOf = (content) => {
    const out = [];
    walkNodes(content, (n) => { if (typeof n.attrs?.id === "string") out.push(n.attrs.id); });
    return out;
  };
  check(textsOf(original) === textsOf(duplicate) && textsOf(duplicate).includes(GARDEN_TEXT), "the copy holds the same text");
  check(blockIdsOf(duplicate).length > 0 && blockIdsOf(duplicate).every((id) => !blockIdsOf(original).includes(id)), "the copy's blocks have new ids");
  const originalChildren = livePages({ roots: tree.roots[at].children }).map((p) => p.title);
  check(JSON.stringify(originalChildren) === JSON.stringify([TOMATOES, CHERRY]), "the original keeps its own sub-pages");
  const tomatoesCopyContent = await pageContent(sub[0].id);
  check(textsOf(tomatoesCopyContent).includes(TOMATOES_TEXT), "the copied sub-page holds its text too");
  writeState({ ids: { ...I, gardenCopy: copyNode.page.id, gardenCopySub: sub.map((p) => p.id) } });
  await capture("duplicated-row");

  // The page menu of the open page.
  await openPage(I.tomatoes);
  await pageMenu("Duplicate");
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === "Tomatoes (copy)"`, "the page menu's Duplicate opens Tomatoes (copy)");
  tree = await treeOf(A.id);
  const garden = tree.roots.find((n) => n.page.id === I.garden);
  const kids = garden.children.map((n) => n.page.title);
  check(JSON.stringify(kids) === JSON.stringify([TOMATOES, "Tomatoes (copy)"]), `Tomatoes (copy) is right after Tomatoes under Garden (${kids.join(", ")})`);
  const copy2 = garden.children[1];
  check(copy2.children.length === 1 && copy2.children[0].page.title === CHERRY, "Tomatoes (copy) has its sub-page Cherry");
  check(!beforeIds.has(copy2.page.id) && !beforeIds.has(copy2.children[0].page.id), "both have new ids");
  const all = livePages(tree);
  check(all.length === beforePages.length + 5, `five pages were added by the two duplicates (${all.length - beforePages.length})`);
  writeState({ ids: { ...readState().ids, tomatoesCopy: copy2.page.id, tomatoesCopyChild: copy2.children[0].page.id } });
}

// ----- 4. Trash ---------------------------------------------------------------------------------------------------------

async function uiTrash() {
  const A = spaceA();
  const I = ids();
  await rowMenu(I.tomatoesCopy, "Move to trash");
  await waitUntil(`document.querySelectorAll(".trash-toggle").length === 1`, "the sidebar has the Trash");
  await wait(600);
  const tree = await treeOf(A.id);
  check(tree.trashed.some((e) => e.page.id === I.tomatoesCopy), "the core has Tomatoes (copy) in the trash");
  await clickOn(".trash-toggle", "the Trash header");
  await waitFor(".trash-list .trash-item", "the trashed page");
  const item = await page.evaluate(`(() => { const li = [...document.querySelectorAll(".trash-list .trash-item")].find((e) => e.querySelector(".trash-title")?.textContent.trim() === "Tomatoes (copy)"); return li ? { expiry: li.querySelector(".trash-expiry")?.textContent.trim() ?? null, inside: li.querySelector(".trash-count")?.textContent.trim() ?? null } : null; })()`);
  must(item !== null, "the Trash lists Tomatoes (copy)");
  check(item.expiry === "Deleted in 30 days", `the Trash says when it goes: "${item.expiry}"`);
  check(item.inside === "+1 page inside", `the Trash says what is inside it (${item.inside})`);
  const retention = await SETTING("app.trash.retention_days");
  check(retention === null || retention === 30, `the device setting app.trash.retention_days is the default, 30 (${JSON.stringify(retention)})`);
  await capture("trash");
  await clickOn(".trash-toggle", "the Trash header (closing it)");
}

// ----- 5. Toggle headings ---------------------------------------------------------------------------------------------

/** Hovers a top-level block of the editor (found by `selector` and optionally its text), opens its grip menu and chooses Turn into -> `label`. */
async function blockTurnInto(selector, text, label) {
  const target = await page.evaluate(`(() => {
    const wanted = ${JSON.stringify(text)};
    const el = [...document.querySelectorAll(".ProseMirror > " + ${JSON.stringify(selector)})].find((e) => wanted === null || e.textContent.includes(wanted));
    if (!el) return null;
    el.scrollIntoView({ block: "center" });
    const r = el.getBoundingClientRect();
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  })()`);
  must(target !== null, `the block ${selector}${text === null ? "" : ` with "${text}"`} is on the page`);
  await wait(150);
  const again = await page.evaluate(`(() => { const wanted = ${JSON.stringify(text)}; const el = [...document.querySelectorAll(".ProseMirror > " + ${JSON.stringify(selector)})].find((e) => wanted === null || e.textContent.includes(wanted)); const r = el.getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height }; })()`);
  await hover(again.x + 60, again.y + Math.min(14, again.h / 2));
  await hover(again.x + 70, again.y + Math.min(15, again.h / 2));
  const grip = await waitFor(".block-handle-grip", "the block handle's grip");
  await mouse(grip.cx, grip.cy);
  await waitFor('[role="menu"]', "the block menu");
  await chooseMenuItem("Turn into");
  await waitUntil(`[...document.querySelectorAll('[role="menuitem"]')].some((e) => e.textContent.trim().startsWith(${JSON.stringify(label)}))`, `the Turn into list offers "${label}"`);
  await chooseMenuItem(label);
  await wait(500);
}

async function uiToggles() {
  const I = ids();
  await openPage(I.notes);
  await waitFor('.ProseMirror [data-type="details"]', "the toggle of Notes");
  // A plain toggle becomes a toggle heading 1 ("turn into").
  await blockTurnInto('[data-type="details"]', TOGGLE_SUMMARY, "Toggle heading 1");
  const first = await waitUntil(`(() => { const s = document.querySelector('.ProseMirror summary'); return s ? { level: s.getAttribute("data-level"), cls: s.className, text: s.textContent.trim() } : null; })()`, "the toggle has a summary");
  check(first.level === "1" && first.cls.includes("toggle-heading") && first.text === TOGGLE_SUMMARY, `turn into: the plain toggle became a toggle heading 1 (${JSON.stringify(first)})`);
  const size1 = await page.evaluate(`parseFloat(getComputedStyle(document.querySelector('.ProseMirror summary[data-level="1"]')).fontSize)`);
  const body = await page.evaluate(`parseFloat(getComputedStyle(document.querySelector('.ProseMirror')).fontSize)`);
  check(size1 > body * 1.2, `a toggle heading 1's summary is drawn with a heading's type (${size1}px against ${body}px)`);

  // /toggle heading 2
  await newLine();
  // The slash menu closes at a space: /toggle lists the toggle and its three headings.
  await slash("toggle", "Toggle heading 2");
  await send("Input.insertText", { text: "Section A" });
  await wait(400);
  const second = await page.evaluate(`(() => { const s = document.querySelector('.ProseMirror summary[data-level="2"]'); return s ? { text: s.textContent.trim(), cls: s.className, fontSize: parseFloat(getComputedStyle(s).fontSize) } : null; })()`);
  must(second !== null, "/toggle heading 2 made a toggle heading 2");
  check(second.text === "Section A" && second.fontSize > body && second.fontSize < size1, `/toggle heading 2 has its text in a medium heading (${JSON.stringify(second)})`);

  // Turn into, both ways: toggle heading -> heading -> toggle heading.
  await blockTurnInto('[data-type="details"]', "Section A", "Heading 2");
  const heading2 = await waitUntil(`(() => { const h = [...document.querySelectorAll(".ProseMirror > h2")].find((e) => e.textContent.trim() === "Section A"); return h ? true : null; })()`, "the toggle heading 2 became a heading 2");
  check(heading2 === true && (await page.evaluate(`document.querySelectorAll('.ProseMirror summary[data-level="2"]').length`)) === 0, "turn into: a toggle heading 2 became a plain heading 2 (no toggle heading 2 is left)");
  await blockTurnInto("h2", "Section A", "Toggle heading 3");
  const third = await waitUntil(`(() => { const s = document.querySelector('.ProseMirror summary[data-level="3"]'); return s ? s.textContent.trim() : null; })()`, "the heading became a toggle heading 3");
  check(third === "Section A", `turn into: the heading became a toggle heading 3 with its text (${third})`);
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(2000);
  const content = await pageContent(I.notes);
  const levels = [];
  walkNodes(content, (n) => { if (n.type === "detailsSummary") levels.push(n.attrs?.level ?? null); });
  check(JSON.stringify(levels) === JSON.stringify([1, 3]), `the core stores the levels on the summaries (${JSON.stringify(levels)})`);
  await capture("toggles");
}

// ----- 6. The table footer's Sum ----------------------------------------------------------------------------------------

async function uiFooter() {
  const I = ids();
  await openPage(I.budget);
  const view = await waitFor('.ProseMirror [data-type="pageList"][data-layout="table"]', "the table view");
  await hover(view.cx, view.y + 30);
  // Show the Cost column: the block's Properties popover.
  await clickButtonStarting(".page-list-button", "Properties", "the Properties button of the table view");
  await waitFor(".page-list-props", "the properties popover");
  const cost = await box(".page-list-props .page-list-prop input");
  must(cost !== null, "the popover lists the Cost property");
  const label = await page.evaluate(`[...document.querySelectorAll(".page-list-props .page-list-prop")].map((l) => l.textContent.trim())`);
  check(label.includes("Cost"), `the popover lists Cost (${label.join(", ")})`);
  await page.evaluate(`(() => { const l = [...document.querySelectorAll(".page-list-props .page-list-prop")].find((x) => x.textContent.trim() === "Cost"); l.scrollIntoView({ block: "nearest" }); })()`);
  const costBox = await page.evaluate(`(() => { const l = [...document.querySelectorAll(".page-list-props .page-list-prop")].find((x) => x.textContent.trim() === "Cost"); const r = l.querySelector("input").getBoundingClientRect(); return { cx: r.x + r.width / 2, cy: r.y + r.height / 2 }; })()`);
  const alreadyShown = await page.evaluate(`[...document.querySelectorAll(".page-list-props .page-list-prop")].find((x) => x.textContent.trim() === "Cost")?.querySelector("input").checked === true`);
  if (!alreadyShown) await mouse(costBox.cx, costBox.cy);
  await page.key("Escape", "Escape", 27);
  await waitUntil(`!document.querySelector(".page-list-props")`, "the popover closed");
  await waitFor('.page-table-footer-cell[data-kind="number"]', "the footer cell of the Cost column");
  const cell = await box('.page-table-footer-cell[data-kind="number"] .page-table-calc-button');
  must(cell !== null, "the Cost column's footer has a Calculate button");
  const nothing = await page.evaluate(`document.querySelector('.page-table-footer-cell[data-kind="number"]').textContent.trim()`);
  check(nothing === "Calculate", `before a choice the cell offers "Calculate" (${nothing})`);
  await mouse(cell.cx, cell.cy);
  await waitFor('[role="menu"]', "the calculation menu");
  const offered = await menuLabels();
  check(offered[0] === "None" && offered.includes("Sum") && offered.includes("Average") && offered.includes("Minimum"), `a number column offers None and the arithmetic (${offered.join(", ")})`);
  check(!offered.includes("Earliest") && !offered.includes("Count checked"), "and not the functions of other kinds");
  await chooseMenuItem("Sum");
  const shown = await waitUntil(`(() => { const c = document.querySelector('.page-table-footer-cell[data-kind="number"]'); const l = c?.querySelector(".page-table-calc-label")?.textContent.trim(); const v = c?.querySelector(".page-table-calc-value")?.textContent.trim(); return l ? { label: l, value: v } : null; })()`, "the footer shows the Sum");
  check(shown.label === "Sum" && /^1[,.\s  ]?580$/.test(shown.value), `the footer says the sum of the three costs: ${COST_SUM} (${JSON.stringify(shown)})`);
  const placed = await page.evaluate(`(() => {
    const foot = document.querySelector(".page-table-foot");
    const body = document.querySelector(".page-table-body");
    return { foot: !!foot, outside: foot && body ? !body.contains(foot) && foot.parentElement === body.parentElement : null };
  })()`);
  check(placed.foot === true && placed.outside === true, "the footer is a row group of its own, outside the body's rows");
  await capture("footer");
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(2000);
  const content = await pageContent(I.budget);
  let stored = null;
  walkNodes(content, (n) => { if (n.type === "pageList") stored = n.attrs?.calculations ?? null; });
  check(stored !== null && stored[I.costProperty] === "sum" && Object.keys(stored).length === 1, `the block stores the choice by column (${JSON.stringify(stored)})`);
  // A filter changes the rows the footer is computed over: not tested here (unit tests cover the arithmetic); the footer shows the shown rows only.
  const rowsShown = await page.evaluate(`document.querySelectorAll('.ProseMirror [data-type="pageList"] .page-table-body .page-table-row').length`);
  check(rowsShown === 3, `the table shows its three rows (${rowsShown})`);
}

// ----- 7. Covers --------------------------------------------------------------------------------------------------------

/** A drag with the left button from (x1, y1) to (x2, y2), in small steps (the cover's overlay moves with the pointer). */
async function dragMouse(x1, y1, x2, y2, steps = 14) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x1, y: y1 });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: x1, y: y1, button: "left", buttons: 1, clickCount: 1 });
  for (let i = 1; i <= steps; i++) {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x1 + ((x2 - x1) * i) / steps, y: y1 + ((y2 - y1) * i) / steps, button: "left", buttons: 1 });
    await wait(20);
  }
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x2, y: y2, button: "left", buttons: 0, clickCount: 1 });
  await wait(300);
}

const coverState = () =>
  page.evaluate(`(() => {
    const c = document.querySelector(".page-cover");
    if (!c) return null;
    const img = c.querySelector("img.page-cover-image");
    return {
      kind: c.dataset.coverKind ?? null, gradient: c.dataset.coverGradient ?? null, color: c.dataset.coverColor ?? null,
      background: getComputedStyle(c).backgroundImage, height: c.getBoundingClientRect().height, width: c.getBoundingClientRect().width,
      img: img ? { loaded: img.complete && img.naturalWidth > 0, naturalWidth: img.naturalWidth, naturalHeight: img.naturalHeight, objectPosition: img.style.objectPosition } : null,
      repositioning: c.classList.contains("is-repositioning"),
      actions: [...c.querySelectorAll(".page-cover-actions button")].map((b) => b.textContent.trim()),
    };
  })()`);

/** The cover picker opened from "Add cover" (no banner) or "Change cover" (a banner). */
async function openCoverPicker(hasBanner) {
  if (hasBanner) {
    const banner = await waitFor(".page-cover", "the cover banner");
    await hover(banner.cx, banner.cy);
    await clickButtonStarting(".page-cover-actions button", "Change cover", "the banner's Change cover button");
  } else {
    await clickOn("button.page-cover-button", "the Add cover button");
  }
  await waitFor(".cover-picker", "the cover picker");
}

async function uiCovers() {
  const A = spaceA();
  const I = ids();
  mkdirSync(SRC, { recursive: true });
  const png = join(SRC, "s-gaps-cover.png");
  writeFileSync(png, pngBytes(600, 600));

  // A gradient from the Gallery.
  await openPage(I.garden);
  check((await coverState()) === null, "a page with no cover has no banner");
  await openCoverPicker(false);
  const tabs = await page.evaluate(`[...document.querySelectorAll('.cover-picker [role="tab"]')].map((t) => t.textContent.trim())`);
  check(tabs[0] === "Gallery" && tabs[1] === "Upload", `the picker has the tabs Gallery and Upload first (${tabs.join(", ")})`);
  info(`the cover picker's tabs: ${tabs.join(", ")} (Link needs the shell's download)`);
  const swatches = await page.evaluate(`({ colors: document.querySelectorAll(".cover-picker .cover-swatch[data-cover-color]").length, gradients: [...document.querySelectorAll(".cover-picker .cover-swatch[data-cover-gradient]")].map((s) => s.dataset.coverGradient) })`);
  check(swatches.colors === 9 && swatches.gradients.length === 10, `the Gallery has the nine colours and ten gradients (${swatches.colors}, ${swatches.gradients.join(" ")})`);
  await capture("cover-picker");
  await clickOn('.cover-picker .cover-swatch[data-cover-gradient="sunset"]', "the Sunset gradient");
  await waitUntil(`!document.querySelector(".cover-picker") && !!document.querySelector('.page-cover[data-cover-gradient="sunset"]')`, "the banner shows the Sunset gradient");
  const gradient = await coverState();
  check(gradient.kind === "gradient" && gradient.gradient === "sunset" && gradient.background.includes("gradient"), `the banner is drawn with the gradient (${gradient.background.slice(0, 60)})`);
  check(gradient.actions.join(",") === "Change cover,Remove", `a gradient banner offers Change cover and Remove, not Reposition (${gradient.actions.join(", ")})`);
  check((await box("button.page-cover-button")) === null, "Add cover is gone from the header once there is a cover");
  check((await pageGet(I.garden)).cover === "gradient:sunset", "the core stores gradient:sunset");
  // The recovery-kit banner above the page (shown until a kit is made): the cover starts below it, not over it.
  const stacking = await page.evaluate(`(() => {
    const banner = document.querySelector(".home-banners .banner, .home-banners .kit-banner");
    const cover = document.querySelector(".page-cover");
    if (!banner || !cover) return null;
    return { bannerBottom: banner.getBoundingClientRect().bottom, coverTop: cover.getBoundingClientRect().top };
  })()`);
  if (stacking === null) {
    info("no banner above the page: nothing for the cover to overlap");
  } else {
    check(stacking.coverTop >= stacking.bannerBottom - 0.5, `the cover starts below the banner above the page, not over it (banner bottom ${round(stacking.bannerBottom)}, cover top ${round(stacking.coverTop)})`);
  }
  await capture("cover-gradient");

  // Upload a picture through the native Open dialog.
  await openCoverPicker(true);
  await clickButtonStarting('.cover-picker [role="tab"]', "Upload", "the Upload tab");
  await waitFor(".cover-picker .icon-upload", "the Upload tab's panel");
  await clickButtonStarting(".cover-picker .icon-upload button", "Choose an image", "the Choose an image button");
  await fileDialog("Open", png);
  await waitUntil(`document.querySelector('.page-cover[data-cover-kind="image"] img.page-cover-image')?.complete && document.querySelector('.page-cover img.page-cover-image').naturalWidth > 0`, "the banner shows the uploaded picture", 30000);
  const picture = await coverState();
  check(picture.img.naturalWidth === 600 && picture.img.naturalHeight === 600, `the banner shows the 600x600 picture (${picture.img.naturalWidth}x${picture.img.naturalHeight})`);
  const garden = await pageGet(I.garden);
  check(typeof garden.cover === "string" && garden.cover.startsWith("attachment:"), `the core stores attachment:<id> (${String(garden.cover).slice(0, 16)}...)`);
  const attachments = await ask({ type: "AttachmentList", data: { spaceId: A.id } });
  check(attachments.some((a) => a.attachmentId === String(garden.cover).slice("attachment:".length) && a.name === "s-gaps-cover.png"), "the picture is an attachment of the space, kept by the cover");
  check(picture.actions.join(",") === "Change cover,Reposition,Remove", `a picture banner offers Change cover, Reposition and Remove (${picture.actions.join(", ")})`);
  await capture("cover-picture");

  // Reposition by dragging.
  await hover(picture.width / 2, 200);
  await clickButtonStarting(".page-cover-actions button", "Reposition", "the Reposition button");
  await waitFor(".page-cover.is-repositioning .page-cover-drag", "the reposition overlay");
  const slider = await page.evaluate(`(() => { const s = document.querySelector(".page-cover-drag"); const r = s.getBoundingClientRect(); return { cx: r.x + r.width / 2, cy: r.y + r.height / 2, value: Number(s.getAttribute("aria-valuenow")) }; })()`);
  check(slider.value === 50, `the slider starts at the default position, 50 (${slider.value})`);
  await dragMouse(slider.cx, slider.cy, slider.cx, slider.cy - 120);
  const moved = await page.evaluate(`Number(document.querySelector(".page-cover-drag")?.getAttribute("aria-valuenow"))`);
  check(moved > 50 && moved <= 100, `dragging the picture up shows more of its bottom: the position goes up (${moved})`);
  const style = await page.evaluate(`document.querySelector("img.page-cover-image").style.objectPosition`);
  check(style === `50% ${moved}%`, `the picture follows the drag (object-position ${style})`);
  check((await pageGet(I.garden)).coverY === undefined, "nothing is written while dragging");
  await capture("cover-reposition");
  await clickButtonStarting(".page-cover-actions button", "Save position", "the Save position button");
  await waitUntil(`!document.querySelector(".page-cover.is-repositioning")`, "the banner leaves reposition mode");
  const saved = await pageGet(I.garden);
  check(saved.coverY === moved, `the core stores coverY ${moved} (${saved.coverY}), the cover is unchanged (${saved.cover === garden.cover})`);
  check(saved.cover === garden.cover, "saving the position keeps the picture");
  writeState({ covers: { garden: { cover: saved.cover, coverY: saved.coverY } } });

  // Remove (on another page, so that Garden keeps its picture for the restart).
  await openPage(I.budget);
  await openCoverPicker(false);
  await clickOn('.cover-picker .cover-swatch[data-cover-gradient="ocean"]', "the Ocean gradient");
  await waitUntil(`!!document.querySelector('.page-cover[data-cover-gradient="ocean"]')`, "Budget shows Ocean");
  const banner = await waitFor(".page-cover", "Budget's banner");
  await hover(banner.cx, banner.cy);
  await clickButtonStarting(".page-cover-actions button", "Remove", "the Remove button");
  await waitUntil(`!document.querySelector(".page-cover")`, "the banner is gone");
  check((await pageGet(I.budget)).cover === undefined, "the core has no cover for Budget after Remove");
  check((await box("button.page-cover-button")) !== null, "Add cover is back in the header");

  // A gradient kept on Orchard plan for the restart.
  await openPage(I.orchard);
  await openCoverPicker(false);
  await clickOn('.cover-picker .cover-swatch[data-cover-gradient="aurora"]', "the Aurora gradient");
  await waitUntil(`!!document.querySelector('.page-cover[data-cover-gradient="aurora"]')`, "Orchard plan shows Aurora");
  check((await pageGet(I.orchard)).cover === "gradient:aurora", "Orchard plan keeps gradient:aurora");
  writeState({ covers: { ...readState().covers, orchard: "gradient:aurora" } });
}

// ----- 8. Font, small text, lock ------------------------------------------------------------------------------------------

const lookMeasure = () =>
  page.evaluate(`(() => {
    const section = document.querySelector("section.page");
    const content = document.querySelector(".page-editor-content") ?? document.querySelector(".ProseMirror");
    const title = document.querySelector(".page-title");
    const side = document.querySelector(".sidebar");
    return {
      classes: section ? section.className : null,
      titleFont: getComputedStyle(title).fontFamily, titleSize: parseFloat(getComputedStyle(title).fontSize),
      textFont: getComputedStyle(content).fontFamily, textSize: parseFloat(getComputedStyle(content).fontSize),
      sidebarFont: getComputedStyle(side).fontFamily, sidebarSize: parseFloat(getComputedStyle(side).fontSize),
      editable: document.querySelector(".ProseMirror")?.getAttribute("contenteditable") ?? null,
    };
  })()`);

/** A switch of the page menu ("Small text", "Lock page"), clicked. */
async function clickSwitch(name) {
  await clickButtonStarting('[role="menu"] [role="switch"], .menu [role="switch"], [role="switch"].menu-switch', name, `the "${name}" switch of the page menu`);
  await wait(500);
}

async function uiLook() {
  const I = ids();
  await openPage(I.orchard);
  const before = await lookMeasure();
  check(!before.classes.includes("page-font-") && !before.classes.includes("page-small-text") && !before.classes.includes("page-locked"), `the page starts with the default look (${before.classes})`);
  await pageMenu(null);
  await waitFor(".page-look-fonts", "the font buttons of the page menu");
  const fonts = await page.evaluate(`[...document.querySelectorAll(".page-look-font")].map((b) => ({ font: b.dataset.font, pressed: b.getAttribute("aria-pressed"), label: b.getAttribute("aria-label") }))`);
  check(fonts.map((f) => f.font).join() === "default,serif,mono" && fonts[0].pressed === "true", `three Ag font buttons, the default pressed (${JSON.stringify(fonts)})`);
  await capture("look-menu");
  await clickOn('.page-look-font[data-font="serif"]', "the Serif font button");
  await waitUntil(`document.querySelector("section.page")?.classList.contains("page-font-serif")`, "the page takes the serif class");
  const serif = await lookMeasure();
  check(serif.titleFont !== before.titleFont && /serif|georgia|cambria|times/i.test(serif.titleFont), `the title is set in the serif stack (${serif.titleFont.slice(0, 60)})`);
  check(serif.textFont !== before.textFont && /serif|georgia|cambria|times/i.test(serif.textFont), `the text is set in the serif stack (${serif.textFont.slice(0, 60)})`);
  check(serif.sidebarFont === before.sidebarFont, "the sidebar keeps the app's face (the font reaches this page only)");
  check((await page.evaluate(`document.querySelector('.page-look-font[data-font="serif"]').getAttribute("aria-pressed")`)) === "true", "the Serif button is pressed");
  check((await page.evaluate(`!!document.querySelector('[role="menu"]')`)), "the page menu stays open, so the effect shows behind it");
  check((await pageGet(I.orchard)).font === "serif", "the core stores font serif");
  await clickOn('.page-look-font[data-font="mono"]', "the Mono font button");
  await waitUntil(`document.querySelector("section.page")?.classList.contains("page-font-mono")`, "the page takes the mono class");
  const mono = await lookMeasure();
  check(/mono|consolas|courier|menlo/i.test(mono.titleFont) && !mono.classes.includes("page-font-serif"), `Mono replaces Serif (${mono.titleFont.slice(0, 60)})`);
  await clickOn('.page-look-font[data-font="serif"]', "the Serif font button (again)");
  await waitUntil(`document.querySelector("section.page")?.classList.contains("page-font-serif")`, "back to serif");

  // Small text scales the page's sizes by 0.875 and nothing around it.
  await clickSwitch("Small text");
  await waitUntil(`document.querySelector("section.page")?.classList.contains("page-small-text")`, "the page takes the small-text class");
  const small = await lookMeasure();
  const ratio = small.textSize / serif.textSize;
  check(Math.abs(ratio - 0.875) < 0.01, `the text is 0.875 of its size (${serif.textSize}px to ${small.textSize}px)`);
  check(Math.abs(small.titleSize / serif.titleSize - 0.875) < 0.01, `the title is 0.875 of its size (${serif.titleSize}px to ${small.titleSize}px)`);
  check(small.sidebarSize === before.sidebarSize, "the sidebar keeps its size");
  const stored = await pageGet(I.orchard);
  check(stored.font === "serif" && stored.smallText === true, `the core stores serif and small text (${stored.font}, ${stored.smallText})`);
  await capture("look-serif-small");

  // Lock.
  await clickSwitch("Lock page");
  await waitUntil(`document.querySelector("section.page")?.classList.contains("page-locked")`, "the page takes the locked class");
  await page.key("Escape", "Escape", 27);
  await waitUntil(`!document.querySelector('[role="menu"]')`, "the menu closed");
  const locked = await lookMeasure();
  check(locked.editable === "false", `a locked page's editor is read-only (contenteditable ${locked.editable})`);
  const lockNote = await page.evaluate(`(() => { const l = document.querySelector(".page-lock"); return l ? { text: l.textContent.trim(), button: l.querySelector("button")?.textContent.trim() } : null; })()`);
  check(lockNote !== null && lockNote.text.startsWith("Locked") && lockNote.button === "Unlock", `the header says Locked and offers Unlock (${JSON.stringify(lockNote)})`);
  check((await box(".page-title-button")) === null && (await box(".page-title-text")) !== null, "the title cannot be pressed to rename");
  const textBefore = JSON.stringify(await editorBlocks());
  const para = await box(".ProseMirror p");
  await mouse(para.x + 20, para.cy);
  await send("Input.insertText", { text: "QQQ" });
  await page.type("QQQ");
  await wait(500);
  check(JSON.stringify(await editorBlocks()) === textBefore, "typing in a locked page changes nothing");
  check((await page.evaluate(`!!document.querySelector(".page-cover-actions")`)) === false, "a locked page's banner has nothing to press");
  check((await box("button.page-icon-button:not([disabled])")) === null, "a locked page's icon button is disabled");
  check((await pageGet(I.orchard)).locked === true, "the core stores locked");
  await capture("look-locked");
  await clickButtonStarting(".page-lock button", "Unlock", "the Unlock button");
  await waitUntil(`!document.querySelector("section.page")?.classList.contains("page-locked")`, "Unlock lifts the lock");
  const unlocked = await lookMeasure();
  check(unlocked.editable === "true", `an unlocked page is editable again (${unlocked.editable})`);
  check((await pageGet(I.orchard)).locked === undefined, "the core has no lock after Unlock");
  await page.evaluate(`document.querySelector(".ProseMirror p")?.scrollIntoView?.()`);
  const para2 = await box(".ProseMirror p");
  await mouse(para2.x + para2.w - 10, para2.cy);
  await page.key("End", "End", 35);
  await send("Input.insertText", { text: "!" });
  await wait(300);
  check((await editorBlocks())[0].endsWith("!"), "typing works again after Unlock");
  await page.key("Backspace", "Backspace", 8);
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);

  // A page left locked for the restart: Cherry.
  await openPage(I.cherry);
  await pageMenu(null);
  await clickSwitch("Lock page");
  await page.key("Escape", "Escape", 27);
  await waitUntil(`document.querySelector("section.page")?.classList.contains("page-locked")`, "Cherry is locked");
  writeState({ look: { orchard: { font: "serif", smallText: true }, lockedPage: I.cherry } });
}

// ----- 9. Accent colour ---------------------------------------------------------------------------------------------------

const navItem = (index) => box(`.nav-list li:nth-child(${index + 1}) .nav-item`);
async function goSettings() {
  const settings = await navItem(1);
  await mouse(settings.cx, settings.cy);
  await waitFor("#settings-title", "the Settings screen");
  await wait(400);
}
async function goHome() {
  const home = await navItem(0);
  await mouse(home.cx, home.cy);
  await wait(500);
}

/** `--accent` as the page computes it, and the data attribute, and whether the dark theme is in force. */
const accentNow = () =>
  page.evaluate(`(() => {
    const root = document.documentElement;
    const theme = root.dataset.theme ?? null;
    const dark = theme === "dark" || (theme !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
    const current = document.querySelector('.nav-item[aria-current="page"]');
    return { attribute: root.dataset.accent ?? null, token: getComputedStyle(root).getPropertyValue("--accent").trim().toLowerCase(), dark, navBackground: current ? getComputedStyle(current).backgroundColor : null };
  })()`);

const hexToRgb = (hex) => `rgb(${[1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)).join(", ")})`;

async function uiAccent() {
  await goSettings();
  const swatches = await page.evaluate(`[...document.querySelectorAll(".accent-swatch")].map((b) => ({ name: b.dataset.accentSwatch, checked: b.getAttribute("aria-checked") }))`);
  check(swatches.length === 9 && swatches.find((s) => s.checked === "true")?.name === "blue", `nine swatches, blue chosen by default (${swatches.map((s) => s.name).join(" ")})`);
  const before = await accentNow();
  check(before.attribute === null, "the default accent sets no data-accent");
  // The fieldsets of Appearance (theme, accent, language, recent pages) have room between them.
  const gaps = await page.evaluate(`(() => {
    const panel = document.querySelector('section[aria-labelledby="settings-appearance"]');
    const rects = [...panel.querySelectorAll(":scope > fieldset")].map((f) => f.getBoundingClientRect());
    return rects.slice(1).map((r, i) => r.top - rects[i].bottom);
  })()`);
  check(gaps.length === 3 && gaps.every((g) => g >= 12), `the four fieldsets of Appearance have at least 12 px between them (${gaps.map((g) => round(g)).join(", ")})`);
  await clickOn('.accent-swatch[data-accent-swatch="green"]', "the green swatch");
  await waitUntil(`document.documentElement.dataset.accent === "green"`, "the page takes the green accent");
  const after = await accentNow();
  const expected = after.dark ? "#73de9f" : "#1c7842";
  check(after.token !== before.token && after.token === expected, `--accent changed from ${before.token} to ${after.token} (green, ${after.dark ? "dark" : "light"}: ${expected})`);
  check(after.navBackground === hexToRgb(expected), `the current navigation item is drawn with it (${after.navBackground})`);
  check((await SETTING("ui.accent")) === "green", "the device setting ui.accent holds green");
  await capture("accent-green");
  // Blue again is the absence of the setting? Choosing it writes "blue"; the page drops the attribute.
  await clickOn('.accent-swatch[data-accent-swatch="blue"]', "the blue swatch");
  await waitUntil(`!document.documentElement.dataset.accent`, "blue is the default again");
  check((await accentNow()).token === before.token, "blue brings the default token back");
  await clickOn('.accent-swatch[data-accent-swatch="green"]', "the green swatch (kept for the restart)");
  await waitUntil(`document.documentElement.dataset.accent === "green"`, "green again");
  writeState({ accent: "green" });
}

// ----- 10. Search ------------------------------------------------------------------------------------------------------------

const searchGroups = () => page.evaluate(`[...document.querySelectorAll(".search-results .search-group:not(.search-recent):not(.search-related)")].map((g) => g.getAttribute("aria-label"))`);

async function typeInSearch(text) {
  await page.evaluate(`(() => { const i = document.querySelector(".search-input"); i.focus(); i.select(); })()`);
  await page.key("Backspace", "Backspace", 8);
  await send("Input.insertText", { text });
  await wait(900);
}

async function uiSearch() {
  const A = spaceA();
  const I = ids();
  await goHome();
  await ctrl("k");
  await waitFor(".search-dialog .search-input", "the search dialog");
  // The words are spread over the title and the body: "orchard" is the title's, "pears" the second block's.
  await typeInSearch("orchard pears");
  let groups = await searchGroups();
  check(groups.length === 1 && groups[0] === ORCHARD, `"orchard pears" (title + body) finds the page (${groups.join(", ")})`);
  await capture("search-spread");
  // Two blocks, one hit.
  const coreHits = await searchHits("apples pears");
  check(coreHits.filter((h) => h.pageId === I.orchard).length === 1, `"apples pears" (two blocks) is one hit for the page (${coreHits.filter((h) => h.pageId === I.orchard).length})`);
  await typeInSearch("apples pears");
  groups = await searchGroups();
  check(groups.length === 1 && groups[0] === ORCHARD, `"apples pears" finds it in the dialog (${groups.join(", ")})`);
  // A typo finds a title.
  await typeInSearch("Orchord");
  groups = await searchGroups();
  check(groups.includes(ORCHARD), `a typo ("Orchord") still finds the title (${groups.join(", ")})`);
  const typoCore = await searchHits("Orchord");
  check(typoCore.some((h) => h.pageId === I.orchard && h.blockId === undefined), "the core's fuzzy hit is the page's title");
  await typeInSearch("Orchrad");
  info(`"Orchrad" (a swapped pair) finds: ${(await searchGroups()).join(", ") || "nothing"}`);
  await typeInSearch("zzzqqq");
  check((await searchGroups()).length === 0, "a made-up word finds nothing");

  // The Space chip.
  await typeInSearch("orchard");
  groups = await searchGroups();
  check(groups.includes(ORCHARD) && groups.includes(WORK_PAGE), `by default every space is searched (${groups.join(", ")})`);
  const chips = await page.evaluate(`[...document.querySelectorAll(".search-filters .search-chip")].map((c) => c.textContent.replace(/\\s+/g, " ").trim())`);
  check(chips.some((c) => c.startsWith("Space: All spaces")) && chips.some((c) => c.startsWith("Edited: Any time")) && chips.includes("Titles only"), `the chips are Space, Edited and Titles only (${chips.join(" | ")})`);
  await clickButtonStarting(".search-filters .search-chip", "Space:", "the Space chip");
  await waitFor('.search-tag-list[aria-label="Space"]', "the Space list");
  await clickButtonStarting('.search-tag-list[aria-label="Space"] .search-chip', "This space", "This space");
  await wait(900);
  groups = await searchGroups();
  check(groups.includes(ORCHARD) && !groups.includes(WORK_PAGE), `"This space" leaves out the other space (${groups.join(", ")})`);
  const chipNow = await page.evaluate(`document.querySelector(".search-filters .search-chip.is-active")?.textContent.replace(/\\s+/g, " ").trim() ?? null`);
  check(chipNow === "Space: This space", `the Space chip says so and is active (${chipNow})`);
  await capture("search-space-chip");
  await clickButtonStarting(".search-filters .search-chip", "Space:", "the Space chip");
  await clickButtonStarting('.search-tag-list[aria-label="Space"] .search-chip', "All spaces", "All spaces");
  await wait(700);
  check((await searchGroups()).includes(WORK_PAGE), "All spaces brings the other space back");

  // The Edited chip.
  await clickButtonStarting(".search-filters .search-chip", "Edited:", "the Edited chip");
  await waitFor('.search-tag-list[aria-label="Edited"]', "the Edited list");
  const choices = await page.evaluate(`[...document.querySelectorAll('.search-tag-list[aria-label="Edited"] .search-chip')].map((c) => c.textContent.trim())`);
  check(choices.join("|") === "Any time|Today|Past 7 days|Past 30 days", `the Edited chip offers any time, today, 7 and 30 days (${choices.join("|")})`);
  await clickButtonStarting('.search-tag-list[aria-label="Edited"] .search-chip', "Today", "Today");
  await wait(900);
  groups = await searchGroups();
  check(groups.includes(ORCHARD) && groups.includes(WORK_PAGE), `pages edited today are found with Edited: Today (${groups.join(", ")})`);
  const editedChip = await page.evaluate(`document.querySelector(".search-filters .search-chip.is-active")?.textContent.replace(/\\s+/g, " ").trim() ?? null`);
  check(editedChip === "Edited: Today", `the Edited chip says Today (${editedChip})`);
  // The cut itself, in the core: a time after every edit finds nothing, the start of today finds the pages.
  const startOfToday = new Date();
  startOfToday.setHours(0, 0, 0, 0);
  check((await searchHits("orchard", { editedAfter: Date.now() + 3600 * 1000 })).length === 0, "editedAfter in the future finds nothing");
  check((await searchHits("orchard", { editedAfter: startOfToday.getTime() })).length >= 2, "editedAfter the start of today finds the pages edited today");
  await clickButtonStarting(".search-filters .search-chip", "Edited:", "the Edited chip");
  await clickButtonStarting('.search-tag-list[aria-label="Edited"] .search-chip', "Any time", "Any time");
  await wait(500);

  // Titles only.
  await typeInSearch("pears");
  check((await searchGroups()).includes(ORCHARD), "a body word finds the page");
  await clickButtonStarting(".search-filters .search-chip", "Titles only", "the Titles only chip");
  await wait(900);
  check((await searchGroups()).length === 0, "with Titles only a body word finds nothing");
  check((await page.evaluate(`document.querySelector(".search-filters .search-chip[aria-pressed]")?.getAttribute("aria-pressed")`)) === "true", "the Titles only chip is pressed");
  await typeInSearch("orchard");
  check((await searchGroups()).includes(ORCHARD), "and a title word still does");
  await page.key("Escape", "Escape", 27);
  await waitUntil(`!document.querySelector(".search-dialog")`, "Escape closes the search");
  await ctrl("k");
  await waitFor(".search-dialog .search-input", "the search dialog (again)");
  const reset = await page.evaluate(`[...document.querySelectorAll(".search-filters .search-chip")].map((c) => c.textContent.replace(/\\s+/g, " ").trim()).join(" | ")`);
  check(!reset.includes("This space") && !reset.includes("Today"), `a new search starts with the chips at their defaults (${reset})`);
  await page.key("Escape", "Escape", 27);
}

// ----- 11. Languages ------------------------------------------------------------------------------------------------------

/** The menu keys of each menu of the bar, in the order of the shell's `describe`. */
const MENU_KEYS = [
  ["menu.file", ["menu.newPage", "menu.import", "menu.export", "menu.print", "menu.lock", "menu.quit"]],
  ["menu.edit", ["menu.undo", "menu.redo", "menu.cut", "menu.copy", "menu.paste", "menu.selectAll", "menu.find"]],
  ["menu.view", ["menu.toggleSidebar", "menu.back", "menu.forward"]],
  ["menu.window", ["menu.minimize", "menu.closeWindow"]],
  ["menu.help", ["menu.about"]],
];

/** A lone Alt press, and what the native menu bar then is (GetMenu non-null, its texts), waiting up to 6 s for it. */
async function showNativeMenuBar() {
  await loneAlt();
  return helper("menu-read", { want: "shown", timeoutMs: 6000 });
}
/** Escape in the page, and the bar gone (GetMenu null) within 6 s. */
async function hideNativeMenuBar() {
  await page.key("Escape", "Escape", 27);
  return helper("menu-read", { want: "hidden", timeoutMs: 6000 });
}

/** The native menu bar and the tray menu say `language`'s words (GetMenuString). */
async function checkNativeLabels(language) {
  const words = messagesOf(language);
  const shown = await showNativeMenuBar();
  if (!check(shown.hasMenu === true, `${language}: a lone Alt shows the menu bar (GetMenu is not null)`)) {
    await hideNativeMenuBar();
    return;
  }
  const top = shown.menu.map((m) => m.text);
  check(JSON.stringify(top) === JSON.stringify(MENU_KEYS.map(([key]) => words[key])), `${language}: the menu bar's menus are ${MENU_KEYS.map(([key]) => words[key]).join(" | ")} (${top.join(" | ")})`);
  for (const [index, [key, items]] of MENU_KEYS.entries()) {
    const got = (shown.menu[index]?.items ?? []).filter((i) => !i.separator).map((i) => i.text.split("\t")[0]);
    const want = items.map((k) => words[k]);
    check(JSON.stringify(got) === JSON.stringify(want), `${language}: ${words[key]} lists ${want.join(" | ")} (${got.join(" | ")})`);
  }
  const hint = (shown.menu[0]?.items ?? []).find((i) => i.text.includes("\t") && i.text.endsWith("Ctrl+P"));
  check(hint !== undefined, `${language}: Print keeps its shortcut hint after a tab (${hint?.text ?? "none"})`);
  const hidden = await hideNativeMenuBar();
  check(hidden.hasMenu === false, `${language}: Escape hides the bar again`);
  // The tray's menu.
  const tray = await helper("tray-menu", {});
  if (!tray.ok) {
    notVerifiable(`${language}: the tray's menu could not be opened (${tray.error ?? "no items"})`);
    return;
  }
  const texts = tray.items.map((i) => i.text);
  const want = [words["tray.open"], words["tray.newPage"], words["tray.lock"], "", words["tray.quit"]];
  check(JSON.stringify(texts) === JSON.stringify(want), `${language}: the tray menu says ${want.filter(Boolean).join(" | ")} (${texts.join(" | ")})`);
  check(tray.closed === true, `${language}: the tray menu was closed again`);
}

/** Chooses a language in Settings; the page has the language's html lang, nav labels and Settings title. */
async function chooseLanguage(language) {
  const words = messagesOf(language);
  await clickOn(`input[name="language"][value="${language}"]`, `the ${language} radio`);
  await waitUntil(`document.documentElement.lang === ${JSON.stringify(language)}`, `<html lang> follows to ${language}`);
  await waitUntil(`document.querySelector("#settings-title")?.textContent.trim() === ${JSON.stringify(words["settings.title"])}`, `the Settings title is "${words["settings.title"]}"`);
  const seen = await page.evaluate(`({ lang: document.documentElement.lang, nav: [...document.querySelectorAll(".nav-list .nav-item")].map((b) => b.textContent.trim()), legend: document.querySelector(".radio-group")?.closest("fieldset")?.querySelector("legend")?.textContent.trim() })`);
  check(seen.lang === language, `${language}: <html lang> is ${seen.lang}`);
  check(JSON.stringify(seen.nav) === JSON.stringify([words["nav.home"], words["nav.settings"], words["nav.diagnostics"]]), `${language}: the navigation says ${words["nav.home"]} | ${words["nav.settings"]} | ${words["nav.diagnostics"]} (${seen.nav.join(" | ")})`);
  check((await SETTING("ui.language")) === language, `${language}: the device setting ui.language holds ${language}`);
  // Words the page builds from pieces and the locale's own formats.
  const search = await page.evaluate(`document.querySelector(".sidebar-search-label")?.textContent.trim()`);
  check(search === words["search.open"], `${language}: the sidebar's search button says "${words["search.open"]}" (${search})`);
  await wait(700); // the shell gets the new words with the change
}

async function uiLanguages() {
  await goSettings();
  const first = await page.evaluate(`({ lang: document.documentElement.lang, choice: document.querySelector('input[name="language"]:checked')?.value ?? null, options: [...document.querySelectorAll('input[name="language"]')].map((i) => i.value) })`);
  check(first.options.join() === "system,en,fr,es", `the picker offers system, en, fr and es (${first.options.join()})`);
  check(first.choice === "system" && first.lang === "en", `the default is "System language" and the page is English (${first.choice}, ${first.lang})`);
  await checkNativeLabels("en");
  await chooseLanguage("fr");
  await capture("language-fr");
  await checkNativeLabels("fr");
  await chooseLanguage("es");
  await capture("language-es");
  await checkNativeLabels("es");
  await chooseLanguage("en");
  await checkNativeLabels("en");
  // The page menu of a page: words and a date in the chosen language (a smoke look at a screen that is not Settings).
  await goHome();
}

// <<ui sections go above this line>>

async function phaseUi() {
  const only = process.env.SG_ONLY ? process.env.SG_ONLY.split(",") : null;
  const sections = [
    ["path", "the path above the title", uiPath],
    ["info", "Page info", uiInfo],
    ["duplicate", "Duplicate", uiDuplicate],
    ["trash", "the trash says when a page is deleted", uiTrash],
    ["toggles", "toggle headings", uiToggles],
    ["footer", "the table footer's Sum", uiFooter],
    ["covers", "covers", uiCovers],
    ["look", "font, small text and lock", uiLook],
    ["accent", "the accent colour", uiAccent],
    ["search", "search", uiSearch],
    ["languages", "French, Spanish and back, with the native menus", uiLanguages],
    // <<ui section entries go above this line>>
  ];
  for (const [key, name, fn] of sections) {
    if (only === null || only.includes(key)) await section(name, fn);
  }
}

// ---------------------------------------------------------------------------------------------------------------
// phase: menu
// ---------------------------------------------------------------------------------------------------------------

const SCRATCH = "Scratch pad";
const winState = (extra = {}) => helper("win-state", extra);
/** The item of a native menu read by the helper, found by its text before the tab (an id of -1 is a submenu). */
function findMenuItem(menu, text) {
  for (const section of menu) {
    const item = (section.items ?? []).find((i) => i.text.split("\t")[0] === text);
    if (item !== undefined) return item;
  }
  return undefined;
}
const sidebarHidden = () => page.evaluate(`!!document.querySelector(".layout.is-sidebar-hidden")`);

async function phaseMenu() {
  await goHome();
  const base = await winState();
  check(base.visible === true && base.hasMenu === false, "the window starts with no menu bar (GetMenu is null)");
  check(base.client.height > 300, `the client area is ${base.client.width}x${base.client.height}`);

  // A lone Alt shows the bar; Escape hides it.
  const shown = await showNativeMenuBar();
  must(shown.hasMenu === true, "a lone Alt (down and up, as trusted key events) shows the native menu bar: GetMenu is not null");
  const top = shown.menu.map((m) => m.text);
  check(top.join("|") === "&File|&Edit|&View|&Window|&Help", `the bar has File, Edit, View, Window and Help (${top.join(" | ")})`);
  const delta = base.client.height - shown.client.height;
  check(delta >= 10 && delta <= 60, `the client area is ${delta} px shorter while the bar is shown (${base.client.height} to ${shown.client.height})`);
  check(shown.window.height === base.window.height, `the window itself keeps its size (${base.window.height})`);
  await capture("menu-bar");
  const hidden = await hideNativeMenuBar();
  check(hidden.hasMenu === false && hidden.client.height === base.client.height, `Escape hides the bar and the client area is back (${hidden.client.height})`);

  // Chords that are not a lone Alt must not show it.
  const chords = [
    ["Alt+Left", ["alt"], "ArrowLeft", "ArrowLeft", 37],
    ["Ctrl+Alt+N", ["ctrl", "alt"], "n", "KeyN", 78],
    ["Alt+Down", ["alt"], "ArrowDown", "ArrowDown", 40],
    ["Alt+Up", ["alt"], "ArrowUp", "ArrowUp", 38],
    ["Alt+Right", ["alt"], "ArrowRight", "ArrowRight", 39],
  ];
  for (const [name, mods, key, code, vk] of chords) {
    await chord(mods, key, code, vk);
    await wait(500);
    const state = await winState();
    check(state.hasMenu === false, `${name} does not show the menu bar (GetMenu is ${state.hasMenu ? "NOT null" : "null"})`);
    if (state.hasMenu) await hideNativeMenuBar();
  }
  await goHome();
  const control = await showNativeMenuBar();
  check(control.hasMenu === true, "control: a lone Alt still shows it after those chords");
  await hideNativeMenuBar();

  // A window that loses the focus hides the bar. In an unlocked session the window is minimised (the system moves the focus); in a
  // locked one the window never had the focus, so the two window messages that tao turns into `Focused(true)` and `Focused(false)`
  // are posted instead (the shell's reaction to the event is what is under test).
  const again = await showNativeMenuBar();
  check(again.hasMenu === true, "the bar is shown again");
  const session = await winState();
  if (session.sessionLocked !== true) {
    await helper("win-minimize", {});
    const lost = await helper("menu-read", { want: "hidden", timeoutMs: 6000 });
    check(lost.hasMenu === false, "the bar hides when the window loses the focus (minimised)");
    await helper("tray-click", { button: "left" });
    const back = await winState({ want: "restored", timeoutMs: 8000 });
    check(back.visible === true && back.iconic === false && back.hasMenu === false, "the window comes back with no bar");
  } else {
    await helper("win-post", { msgs: [[0x0007, 0, 0], [0x0008, 0, 0]] });
    const lost = await helper("menu-read", { want: "hidden", timeoutMs: 6000 });
    check(lost.hasMenu === false, "the bar hides when the window gets WM_KILLFOCUS (posted after WM_SETFOCUS: tao raises Focused(false))");
    notVerifiable("a real loss of the keyboard focus: the Windows session is locked, so the window never has it; the window messages were posted instead");
  }

  // Two items by WM_COMMAND, ids read from the menu.
  const wide = await winState();
  check((await sidebarHidden()) === false, "the sidebar is shown");
  let bar = await showNativeMenuBar();
  const toggle = findMenuItem(bar.menu, "Toggle &sidebar");
  must(toggle !== undefined && toggle.id > 0, `View has "Toggle &sidebar" with a command id (${toggle?.id})`);
  await helper("menu-command", { id: toggle.id });
  await waitUntil(`!!document.querySelector(".layout.is-sidebar-hidden")`, "View > Toggle sidebar hides the sidebar");
  const afterChoice = await helper("menu-read", { want: "hidden", timeoutMs: 6000 });
  check(afterChoice.hasMenu === false, "choosing an item hides the bar again");
  await capture("menu-sidebar-hidden");
  bar = await showNativeMenuBar();
  await helper("menu-command", { id: toggle.id });
  await waitUntil(`!document.querySelector(".layout.is-sidebar-hidden")`, "View > Toggle sidebar shows it again");
  await helper("menu-read", { want: "hidden", timeoutMs: 6000 });

  bar = await showNativeMenuBar();
  const about = findMenuItem(bar.menu, "&About atomik-kb");
  must(about !== undefined && about.id > 0, `Help has "&About atomik-kb" (${about?.id})`);
  await helper("menu-command", { id: about.id });
  const dialog = await waitUntil(`(() => { const d = document.querySelector('[role="dialog"]'); return d ? d.innerText : null; })()`, "Help > About opens the About dialog");
  check(dialog.includes("About atomik-kb") && /Version \d+\.\d+/.test(dialog), `the About dialog names the app and its version (${dialog.replace(/\s+/g, " ").slice(0, 120)})`);
  await capture("menu-about");
  const afterAbout = await helper("menu-read", { want: "hidden", timeoutMs: 6000 });
  check(afterAbout.hasMenu === false, "the bar is hidden after Help > About too");
  await clickButtonStarting('[role="dialog"] button', "Close", "the About dialog's Close button");
  await waitUntil(`!document.querySelector('[role="dialog"]')`, "the About dialog closed");
  info(`the window is ${wide.window.width}x${wide.window.height}`);

  // The editor: one Ctrl+Z is one undo step, Ctrl+C and Ctrl+V act once.
  await newPage(SCRATCH);
  await newLine();
  await send("Input.insertText", { text: "alpha" });
  await wait(2200);
  await send("Input.insertText", { text: " beta" });
  await wait(2200);
  check((await editorBlocks()).join("|") === "alpha beta", `the editor holds "alpha beta" (${(await editorBlocks()).join("|")})`);
  await ctrl("z");
  await wait(900);
  const undone = (await editorBlocks()).join("|");
  check(undone === "alpha", `one Ctrl+Z (a trusted key event) undid exactly one step: " beta" (the page says "${undone}")`);
  await ctrl("y");
  await wait(900);
  check((await editorBlocks()).join("|") === "alpha beta", `one Ctrl+Y redid exactly one step (${(await editorBlocks()).join("|")})`);

  // Copy and paste. With a clipboard the keys do the work (Ctrl+C and Ctrl+V as trusted events that carry the editing commands); where
  // the session has no clipboard for this program (a locked workstation) the same editor handlers are exercised with clipboard events.
  const saved = await helper("clip-get", {});
  const probeSet = await helper("clip-set", { text: "sg-clipboard-probe" });
  const clipboardWorks = probeSet.ok === true;
  const selectAlpha = `(() => {
    const e = document.querySelector(".ProseMirror").editor;
    let from = null;
    e.state.doc.descendants((n, pos) => { if (from === null && n.isTextblock && n.textContent.startsWith("alpha")) { from = pos + 1; return false; } return true; });
    e.chain().focus().setTextSelection({ from, to: from + 5 }).run();
  })()`;
  if (clipboardWorks) {
    await page.evaluate(selectAlpha);
    await ctrl("c", 0, { commands: ["copy"] });
    await wait(400);
    await page.evaluate(`document.querySelector(".ProseMirror").editor.chain().focus("end").run()`);
    await newLine();
    await ctrl("v", 0, { commands: ["paste"] });
    await wait(900);
    const pasted = await editorBlocks();
    check(pasted.join("|") === "alpha beta|alpha", `Ctrl+C then Ctrl+V once each (trusted key events with the copy and paste commands): "alpha" arrives once (${pasted.join("|")})`);
    if (typeof saved.text === "string" && saved.text !== "") await helper("clip-set", { text: saved.text.replace(/\r?\n$/, "") });
  } else {
    notVerifiable(`the system clipboard is not available to this session (${probeSet.error ?? "the workstation is locked"}): Ctrl+C and Ctrl+V as key events could not run; the editor's copy and paste handlers ran on clipboard events instead`);
    const outcome = await page.evaluate(`(() => {
      const el = document.querySelector(".ProseMirror");
      const e = el.editor;
      let from = null;
      e.state.doc.descendants((n, pos) => { if (from === null && n.isTextblock && n.textContent.startsWith("alpha")) { from = pos + 1; return false; } return true; });
      e.chain().focus().setTextSelection({ from, to: from + 5 }).run();
      const data = new DataTransfer();
      el.dispatchEvent(new ClipboardEvent("copy", { clipboardData: data, bubbles: true, cancelable: true }));
      const copied = data.getData("text/plain");
      e.chain().insertContentAt(e.state.doc.content.size, { type: "paragraph" }).focus("end").run();
      el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
      return { copied, types: [...data.types] };
    })()`);
    await wait(700);
    const pasted = await editorBlocks();
    check(outcome.copied === "alpha" && pasted.join("|") === "alpha beta|alpha", `the copy handler wrote "alpha" (${outcome.copied}) and one paste event inserted it once (${pasted.join("|")})`);
  }
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1500);

  // The real keyboard: Ctrl+Z sent with keybd_event while the window is the foreground window (the native menu's accelerators and the
  // page's own binding would both answer it): still one step.
  await page.evaluate(`document.querySelector(".ProseMirror").editor.chain().focus("end").run()`);
  await send("Input.insertText", { text: " gamma" });
  await wait(2400);
  const real = await helper("keys-real", { letter: "z", count: 1 });
  if (real.sent === true && real.interrupted === undefined) {
    await wait(1000);
    const afterReal = (await editorBlocks()).join("|");
    check(afterReal === "alpha beta|alpha", `one real Ctrl+Z (the keyboard, not a DevTools event) undid exactly one step (${afterReal})`);
  } else {
    notVerifiable(`the real Ctrl+Z check was not sent (${real.reason ?? real.error ?? "interrupted"}); the foreground window was "${real.foregroundTitle ?? ""}"`);
  }
}

// ---------------------------------------------------------------------------------------------------------------
// phases: deeplink-*
// ---------------------------------------------------------------------------------------------------------------

/** The log of the running binary (the newest of the pass's own logs, not the second launches'). */
function runningLog() {
  const files = logFiles().filter((f) => !/second/.test(f));
  return stripAnsi(readFileSync(join(OUT, files[files.length - 1]), "utf8"));
}
const countLines = (text, key) => text.split(/\r?\n/).filter((l) => l.includes(key)).length;
const secondLaunch = () => JSON.parse(readFileSync(join(OUT, "s-gaps-second.json"), "utf8").replace(/^﻿/, ""));

/** The page text and ids of the pass are not in a log, and no link is. */
function noLinksIn(log, what) {
  const S = readState();
  const bad = [S.deepLinks.cold, S.deepLinks.forward, S.ids.long, S.ids.longBlock, S.ids.orchard, "atomik-kb://", "not-a-uuid", "secret"].filter((needle) => log.includes(needle));
  check(bad.length === 0, `${what}: the log has no link and no page or block id (${bad.length === 0 ? "none" : bad.map((b) => (/^[0-9a-f-]{20,}$/.test(b) ? "<id>" : b)).join(", ")})`);
}

async function phaseDeeplinkCold() {
  const I = ids();
  const S = readState();
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(LONG)}`, "the app started by a link opens the page the link names", 40000);
  await waitUntil(`!!document.querySelector('[data-id="${I.longBlock}"]')`, "the linked block is in the page");
  await wait(1800);
  const where = await page.evaluate(`(() => {
    const el = document.querySelector('[data-id="${I.longBlock}"]');
    const r = el.getBoundingClientRect();
    const first = document.querySelector(".ProseMirror p").getBoundingClientRect();
    return { top: r.top, bottom: r.bottom, vh: innerHeight, firstBottom: first.bottom, text: el.textContent };
  })()`);
  check(where.text.startsWith("Line 55 "), `the block is line 55 (${where.text})`);
  check(where.top >= 0 && where.bottom <= where.vh, `the page is scrolled to the block: line 55 is in view (${round(where.top)}..${round(where.bottom)} of ${where.vh})`);
  check(where.firstBottom < 0, `line 1 is above the view (bottom ${round(where.firstBottom)}): the page did scroll`);
  const tabs = await page.evaluate(`[...document.querySelectorAll('.tab-list [role="tab"]')].map((t) => t.querySelector(".tab-title")?.textContent.trim() ?? t.getAttribute("aria-label"))`);
  check(tabs.length === 1 && tabs[0] === LONG, `it opened in the active tab, no extra tab (${tabs.join(", ")})`);
  const log = runningLog();
  check(countLines(log, "deep link opened") === 1 && countLines(log, "deep link ignored") === 0, `the shell logged "deep link opened" once and ignored none (${countLines(log, "deep link opened")}, ${countLines(log, "deep link ignored")})`);
  noLinksIn(log, "cold start");
  // The pass never registers the scheme: the registry is as it was before the pass.
  const scheme = await helper("reg-scheme", {});
  check(scheme.exists === S.scheme?.exists && scheme.command === S.scheme?.command, `the atomik-kb scheme in the registry is as it was before the pass (registered: ${scheme.exists})`);
  await capture("deeplink-cold");
}

async function phaseDeeplinkInvalid() {
  const S = readState();
  await waitFor(".tab-strip", "the tab strip", 40000);
  await wait(3500);
  // The app reopens the tab the last run ended with (the long page the cold start opened): the invalid link opens nothing of its own.
  check((await heading()) === LONG, `a link that is not valid opens nothing: the tab of the last run is as it was (${await heading()})`);
  check((await page.evaluate(`document.querySelectorAll(".toast").length`)) === 0, "no toast, no message");
  check((await page.evaluate(`document.querySelectorAll('[role="alert"]').length`)) === 0, "no alert");
  const tabs = await page.evaluate(`document.querySelectorAll('.tab-list [role="tab"]').length`);
  check(tabs === 1, `one tab (${tabs})`);
  const log = runningLog();
  check(countLines(log, "deep link ignored") === 1 && countLines(log, "deep link opened") === 0, `the shell logged "deep link ignored" once and opened none (${countLines(log, "deep link ignored")}, ${countLines(log, "deep link opened")})`);
  noLinksIn(log, "invalid link");
  void S;
}

async function phaseDeeplinkBefore() {
  const title = await heading();
  const tabs = await page.evaluate(`document.querySelectorAll('.tab-list [role="tab"]').length`);
  const minimised = await helper("win-minimize", {});
  check(minimised.iconic === true, "the running app's window is minimised before the second launch");
  writeState({ forwardBefore: { title, tabs } });
  info(`before the second launch: the page shows "${title}" in ${tabs} tab(s)`);
}

async function phaseDeeplinkForward() {
  const I = ids();
  const second = secondLaunch();
  check(second.exited === true && second.exitCode === 0, `the second launch exited by itself with code 0 after ${second.ms} ms (it forwarded its arguments and left)`);
  const restored = await winState({ want: "restored", timeoutMs: 10000 });
  check(restored.visible === true && restored.iconic === false, "the running app's window was restored from the minimised state");
  const focused = await winState({ want: "foreground", timeoutMs: 4000 });
  if (focused.foreground === true) {
    check(true, "and it is the foreground window (focused)");
  } else {
    notVerifiable(`the window is not the foreground window after the forwarded launch (the foreground window is "${focused.foregroundTitle}"): Windows' foreground lock may keep it behind`);
  }
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(ORCHARD)}`, "the running app opened the page of the forwarded link", 15000);
  const tabs = await page.evaluate(`document.querySelectorAll('.tab-list [role="tab"]').length`);
  check(tabs === 1, `in the active tab, no new one (${tabs})`);
  const count = await helper("proc-count", {});
  check(count.count === 1, `exactly one app process is running (${count.count})`);
  const log = runningLog();
  check(countLines(log, "a second launch was forwarded") === 1 && countLines(log, "deep link opened") === 1, `the running app logged the forwarded launch and the link once (${countLines(log, "a second launch was forwarded")}, ${countLines(log, "deep link opened")})`);
  noLinksIn(log, "forwarded link");
  const secondLog = stripAnsi(readFileSync(join(OUT, second.log), "utf8"));
  check(!secondLog.includes("core ready"), "the second process never opened the core");
  noLinksIn(secondLog, "the second process");
  void I;
  await capture("deeplink-forward");
}

async function phaseDeeplinkPlain() {
  const S = readState();
  const second = secondLaunch();
  check(second.exited === true && second.exitCode === 0, `a plain second launch (no link) exited with code 0 after ${second.ms} ms`);
  const restored = await winState({ want: "restored", timeoutMs: 10000 });
  check(restored.visible === true && restored.iconic === false, "it brought the running app's window back from the minimised state");
  check((await heading()) === (S.forwardBefore?.title ?? ORCHARD), `the page did not change (${await heading()})`);
  const log = runningLog();
  check(countLines(log, "a second launch was forwarded") === 2 && countLines(log, "deep link opened") === 1 && countLines(log, "deep link ignored") === 1, `the log counts: 2 forwarded launches, 1 link opened, 1 ignored (${countLines(log, "a second launch was forwarded")}, ${countLines(log, "deep link opened")}, ${countLines(log, "deep link ignored")})`);
  const scheme = await helper("reg-scheme", {});
  check(scheme.exists === S.scheme?.exists && scheme.command === S.scheme?.command, "the registry still has the atomik-kb scheme as it was before the pass");
}

/** The app lock is turned on and the app locked, its window minimised: a link then arrives from a second launch. */
async function phaseDeeplinkLockPrep() {
  await enableAppLockUi();
  await clickButtonStarting(".sidebar-lock, button", "Lock now", "the Lock now button");
  await waitFor(".lock-card", "the app is locked", 20000);
  const minimised = await helper("win-minimize", {});
  check(minimised.iconic === true, "the locked app's window is minimised before the second launch");
}

async function phaseDeeplinkLocked() {
  const I = ids();
  const second = secondLaunch();
  check(second.exited === true && second.exitCode === 0, `the second launch with a link exited with code 0 after ${second.ms} ms`);
  const restored = await winState({ want: "restored", timeoutMs: 10000 });
  check(restored.visible === true && restored.iconic === false, "the locked app's window was restored");
  check((await box(".lock-card")) !== null && (await box(".tab-strip")) === null, "the lock screen shows, not the app");
  const text = await page.evaluate(`document.body.innerText`);
  check(!text.includes(LONG) && !text.includes(ORCHARD) && !text.includes("Line 55"), "the lock screen shows nothing of the link or of any page");
  await capture("deeplink-locked");
  await unlockUi();
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(LONG)}`, "after the passphrase the linked page opens", 30000);
  await waitUntil(`!!document.querySelector('[data-id="${I.longBlock}"]')`, "the linked block is in the page");
  await wait(1800);
  const where = await page.evaluate(`(() => { const r = document.querySelector('[data-id="${I.longBlock}"]').getBoundingClientRect(); return { top: r.top, bottom: r.bottom, vh: innerHeight }; })()`);
  check(where.top >= 0 && where.bottom <= where.vh, `opened at the block (line 55 in view: ${round(where.top)}..${round(where.bottom)} of ${where.vh})`);
  const log = runningLog();
  check(countLines(log, "deep link opened") === 2 && countLines(log, "a second launch was forwarded") === 3, `the log: two links opened, three launches forwarded (${countLines(log, "deep link opened")}, ${countLines(log, "a second launch was forwarded")})`);
  noLinksIn(log, "the link while locked");
  await disableAppLockUi();
  await goHome();
}

// ---------------------------------------------------------------------------------------------------------------
// phases: tray, tray-quit, tray-closeoff
// ---------------------------------------------------------------------------------------------------------------

const rootIds = async (spaceId) => (await treeOf(spaceId)).roots.map((n) => n.page.id);
/** The title field of the open page has the focus (the page was made by a capture, which asks for its title to be typed). */
const titleFieldFocused = () => page.evaluate(`(() => { const a = document.activeElement; return !!(a && a.tagName === "INPUT" && a.closest(".page-title")); })()`);

/**
 * After a quick capture: the window is back, exactly one new page was made at the top level of `spaceId`, it is open with its title
 * field focused; `title` is typed into it. Returns the new page's id.
 */
async function expectCapture({ spaceId, before, what, title }) {
  const w = await winState({ want: "restored", timeoutMs: 10000 });
  check(w.visible === true && w.iconic === false, `${what}: the window is back on screen`);
  let made = [];
  const end = Date.now() + 15000;
  while (Date.now() < end) {
    made = (await rootIds(spaceId)).filter((id) => !before.includes(id));
    if (made.length > 0) break;
    await wait(300);
  }
  must(made.length === 1, `${what}: exactly one new page was made at the top level of the space (${made.length})`);
  const created = await pageGet(made[0]);
  check(created.parentId === undefined && created.spaceId === spaceId && created.title === "", `${what}: it is untitled, top level, in the space in use`);
  const focused = await becomes(`(() => { const a = document.activeElement; return !!(a && a.tagName === "INPUT" && a.closest(".page-title")); })()`, 8000);
  check(focused === true, `${what}: its title field has the focus`);
  await send("Input.insertText", { text: title });
  await page.key("Enter", "Enter", 13);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(title)}`, `${what}: the title typed into the focused field is the page's title`);
  return made[0];
}

async function hideWindowToTray(how = "close") {
  await helper("win-close", { how });
  const w = await winState({ want: "hidden", timeoutMs: 8000 });
  return w;
}

const passphrase = () => readState().passphrase;
/** Fills the password field labelled `label` of the open form (focus, then typed text: no field is read back). */
async function typeIntoLabelled(label, text) {
  const ok = await page.evaluate(`(() => {
    const l = [...document.querySelectorAll("label")].find((x) => x.textContent.trim() === ${JSON.stringify(label)});
    const i = l ? document.getElementById(l.htmlFor) : null;
    if (!i) return false;
    i.focus();
    return true;
  })()`);
  must(ok, `the field "${label}" is there`);
  await send("Input.insertText", { text });
}

/** Settings -> Security: the app lock is on (turned on with this pass's passphrase when it is off). */
async function enableAppLockUi() {
  await goSettings();
  if ((await page.evaluate(`document.querySelector(".lock-state")?.textContent.trim()`)) !== "App lock is on.") {
    await clickButtonStarting("button", "Turn on app lock...", "the Turn on app lock button");
    await waitFor(".lock-form", "the app lock form");
    await typeIntoLabelled("Passphrase", passphrase());
    await typeIntoLabelled("Repeat the passphrase", passphrase());
    await wait(400);
    await clickButtonStarting(".lock-form button[type=submit]", "Turn on app lock", "the submit button");
    await waitUntil(`document.querySelector(".lock-state")?.textContent.trim() === "App lock is on."`, "the app lock is on", 30000);
  }
}

/** Settings -> Security: the app lock is turned off again (the later steps and the restart start without a lock screen). */
async function disableAppLockUi() {
  await goSettings();
  await clickButtonStarting("button", "Turn off...", "the Turn off button");
  await waitFor(".lock-form", "the form to turn the lock off");
  await typeIntoLabelled("Current passphrase", passphrase());
  await wait(400);
  await clickButtonStarting(".lock-form button[type=submit]", "Turn off app lock", "the submit button");
  await waitUntil(`document.querySelector(".lock-state")?.textContent.trim() === "App lock is off."`, "the app lock is off again", 30000);
}

/** The lock screen's passphrase and Unlock button, then the app opens. */
async function unlockUi() {
  await waitFor(".lock-card", "the lock screen");
  await page.evaluate(`document.querySelector('.lock-card input[type="password"]').focus()`);
  await send("Input.insertText", { text: passphrase() });
  await wait(500);
  await clickOn('.lock-card button[type="submit"]', "the Unlock button");
  await waitFor(".tab-strip", "the app opens after the passphrase", 40000);
}

async function trayLock() {
  const A = spaceA();
  await enableAppLockUi();
  await goHome();
  const rootsBefore = await rootIds(A.id);

  // The tray's Lock: the lock screen shows.
  const menu = await helper("tray-menu", {});
  const lockItem = (menu.items ?? []).find((i) => i.text === "Lock");
  must(lockItem !== undefined && lockItem.id > 0, "the tray menu has Lock");
  await helper("tray-menu", { press: lockItem.id });
  await waitFor(".lock-card", "the tray's Lock shows the lock screen", 20000);
  check((await winState()).visible === true, "the locked app's window is visible, showing the lock screen");

  // The shortcut while locked: the window comes forward on the lock screen, nothing is made yet.
  await hideWindowToTray("close");
  await helper("hotkey-post", {});
  const w = await winState({ want: "restored", timeoutMs: 10000 });
  check(w.visible === true && w.iconic === false, "locked: the shortcut brings the window back");
  check((await box(".lock-card")) !== null && (await box(".tab-strip")) === null, "locked: it shows the lock screen, not the app");
  await capture("tray-locked");
  // Unlock: the page appears.
  await unlockUi();
  await expectCapture({ spaceId: A.id, before: rootsBefore, what: "locked", title: "Captured locked" });
  await capture("tray-locked-captured");

  // Turn the lock off again (the later steps and the restart start without a lock screen).
  await disableAppLockUi();
  await goHome();
}

async function phaseTray() {
  const A = spaceA();
  const B = readState().spaceB;
  await goHome();
  await winState(); // the window is up
  // Close to the tray is on unless an earlier, interrupted run left it off (this pass turns it off in its last tray step).
  if ((await SETTING("app.tray.close_to_tray")) === false) {
    await core({ type: "SettingsSet", data: { key: "app.tray.close_to_tray", value: true } });
    info("close to the tray was left off by an earlier run: set back to on");
    await wait(2000);
  }

  const only = process.env.SG_ONLY ? process.env.SG_ONLY.split(",") : null;
  const wanted = (key) => only === null || only.includes(key);
  if (wanted("icon")) await section("the tray icon", async () => {
    const found = await helper("tray-find", {});
    check(found.trayWindow !== 0, `the tray library's window exists in the app (hwnd ${found.trayWindow})`);
    info(`tray-find: roots ${JSON.stringify(found.roots)}, UI Automation ${JSON.stringify(found.uia)}, registry ${JSON.stringify(found.registry)} ${found.uiaError ?? ""}`);
    if (found.uia.length > 0) {
      check(true, `UI Automation finds the tray icon in the notification area ("${found.uia[0].name}" under ${found.uia[0].root})`);
    } else {
      notVerifiable("UI Automation does not list the tray icon (Windows 11 builds the notification area and its overflow flyout on demand); the shell's own answer to a click is the proof used instead (the next check)");
    }
    if (found.registry.length > 0) info(`the shell's record of the icon (HKCU Control Panel\\NotifyIconSettings): ${found.registry.length} entry for atomik-kb-desktop`);
    // The close button hides the window; the process and the page keep running.
    const hiddenState = await hideWindowToTray("syscommand");
    check(hiddenState.alive === true && hiddenState.visible === false, "the close button (SC_CLOSE) hides the window; the process is alive");
    check((await page.evaluate(`1 + 1`)) === 2, "the page keeps running in the hidden window");
    check(countLines(runningLog(), "the window was hidden to the tray") === 1, "the shell logged that the window was hidden to the tray");
    // A click on the icon, as the shell sends it, shows the window: the library only raises its event after asking the shell where the
    // icon is (Shell_NotifyIconGetRect), so this also proves the icon is registered in the notification area.
    await helper("tray-click", { button: "left" });
    let w = await winState({ want: "restored", timeoutMs: 8000 });
    check(w.visible === true && w.iconic === false, "a left click on the tray icon shows the window again (the icon is registered with the shell)");
    // The menu's Open, from a hidden window.
    await hideWindowToTray("close");
    const menu = await helper("tray-menu", {});
    const open = (menu.items ?? []).find((i) => i.text === "Open atomik-kb");
    must(open !== undefined && open.id > 0, `the tray menu has "Open atomik-kb" (${(menu.items ?? []).map((i) => i.text).join(" | ")})`);
    await helper("tray-menu", { press: open.id });
    w = await winState({ want: "restored", timeoutMs: 8000 });
    check(w.visible === true, "the tray menu's Open shows the window");
    check(countLines(runningLog(), "the window was hidden to the tray") === 2, "the log has two hide events");
  });

  if (wanted("shortcut")) await section("the quick capture shortcut", async () => {
    const probe = await helper("hotkey-probe", {});
    check(probe.registeredByUs === false && probe.error === 1409, `Ctrl+Alt+N cannot be registered by another program: ERROR_HOTKEY_ALREADY_REGISTERED (${probe.error}); the app owns it`);
    await goSettings();
    await page.evaluate(`document.querySelector("#settings-desktop")?.scrollIntoView({ block: "start" })`);
    await wait(500);
    const settings = await page.evaluate(`({ field: document.querySelector(".desktop-shortcut input.shortcut-field")?.value ?? null, status: document.querySelector(".desktop-shortcut p[role=status]")?.textContent.trim() ?? null, tray: document.querySelector('.field input[role="switch"]')?.checked ?? null })`);
    check(settings.field === "Ctrl+Alt+N" && settings.status === "The shortcut is on.", `Settings -> Desktop shows Ctrl+Alt+N and says "${settings.status}"`);
    check(settings.tray === true, "close to the tray is on by default");
    await capture("tray-settings");
    await goHome();
    if (probe.registeredByUs !== false) throw new PhaseStop("the shortcut is not owned by the app: no key is sent");

    // WM_HOTKEY to the hotkey window, the window hidden.
    let before = await rootIds(A.id);
    await hideWindowToTray("close");
    const posted = await helper("hotkey-post", {});
    check(posted.ok === true, `WM_HOTKEY was posted to the global-hotkey window (hwnd ${posted.window})`);
    await expectCapture({ spaceId: A.id, before, what: "WM_HOTKEY, window hidden", title: "Captured one" });

    // The real key press, after the registration was proved above.
    before = await rootIds(A.id);
    await hideWindowToTray("close");
    const sent = await helper("hotkey-send", {});
    if (sent.sent === true) {
      await expectCapture({ spaceId: A.id, before, what: "the real Ctrl+Alt+N", title: "Captured two" });
    } else {
      notVerifiable(`the real Ctrl+Alt+N key press was not sent: ${sent.reason ?? sent.error} (the registration is proved by RegisterHotKey and the handler by WM_HOTKEY above)`);
      await helper("tray-click", { button: "left" });
      await winState({ want: "restored", timeoutMs: 8000 });
    }

    // A page goes to the last used space: the second space.
    await switchSpaceUi(B.id);
    before = await rootIds(B.id);
    const beforeA = await rootIds(A.id);
    await hideWindowToTray("close");
    await helper("hotkey-post", {});
    await expectCapture({ spaceId: B.id, before, what: "in the second space", title: "Captured in Work" });
    check((await rootIds(A.id)).length === beforeA.length, "the first space got no page");
    await switchSpaceUi(A.id);
    await capture("tray-captured");
  });

  if (wanted("lock")) await section("the locked app", trayLock);
}

async function phaseTrayQuit() {
  await goHome();
  const bar = await showNativeMenuBar();
  const quit = findMenuItem(bar.menu, "&Quit");
  must(quit !== undefined && quit.id > 0, `File has "&Quit" (${quit?.id})`);
  const before = await winState();
  check(before.alive === true, "the app is running, close to the tray on");
  const ended = await helper("menu-command", { id: quit.id, waitExitMs: 25000 });
  check(ended.exited === true, "File > Quit (WM_COMMAND) ends the process");
  check(ended.exitCode === 0, `with exit code 0 (${ended.exitCode})`);
  const left = await helper("proc-count", {});
  check(left.count === 0, `no app process is left (${left.count})`);
}

async function phaseTrayCloseoff() {
  await goSettings();
  await page.evaluate(`document.querySelector("#settings-desktop")?.scrollIntoView({ block: "start" })`);
  const on = await page.evaluate(`document.querySelector('.field input[role="switch"]')?.checked`);
  check(on === true, "close to the tray is on after the restart (the default)");
  await clickOn('.field input[role="switch"]', "the close-to-tray switch");
  await waitUntil(`document.querySelector('.field input[role="switch"]')?.checked === false`, "the switch is off");
  check((await SETTING("app.tray.close_to_tray")) === false, "the device setting app.tray.close_to_tray holds false");
  // The shell reads the setting through the core's event: wait for its log line before closing.
  const end = Date.now() + 10000;
  while (Date.now() < end && !/close_to_tray=false/.test(runningLog())) await wait(300);
  check(/close_to_tray=false/.test(runningLog()), "the shell applied close_to_tray=false (its log)");
  await capture("tray-closeoff");
  const ended = await helper("win-close", { how: "syscommand", waitExitMs: 25000 });
  check(ended.exited === true, "with close to the tray off, the close button ends the process");
  check(ended.exitCode === 0, `with exit code 0 (${ended.exitCode})`);
  check(countLines(runningLog(), "the window was hidden to the tray") === 0, "the window was not hidden");
}

// ---------------------------------------------------------------------------------------------------------------
// phases: prerestart, restart
// ---------------------------------------------------------------------------------------------------------------

async function phasePrerestart() {
  await goSettings();
  await page.evaluate(`document.querySelector("#settings-desktop")?.scrollIntoView({ block: "start" })`);
  // Close to the tray goes back on (it was turned off by the last step).
  const on = await page.evaluate(`document.querySelector('.field input[role="switch"]')?.checked`);
  if (on !== true) {
    await clickOn('.field input[role="switch"]', "the close-to-tray switch");
    await waitUntil(`document.querySelector('.field input[role="switch"]')?.checked === true`, "the switch is on again");
  }
  check((await SETTING("app.tray.close_to_tray")) !== false, "close to the tray is on again");
  await page.evaluate(`document.querySelector("#settings-appearance")?.scrollIntoView({ block: "start" })`);
  await chooseLanguage("fr");
  await capture("prerestart");
  const I = ids();
  const S = readState();
  const pages = {};
  for (const [name, id] of Object.entries({ garden: I.garden, orchard: I.orchard, cherry: I.cherry, budget: I.budget })) {
    const p = await pageGet(id);
    pages[name] = { cover: p.cover ?? null, coverY: p.coverY ?? null, font: p.font ?? null, smallText: p.smallText ?? null, locked: p.locked ?? null };
  }
  writeState({ before: { pages, accent: await SETTING("ui.accent"), language: await SETTING("ui.language"), titles: livePages(await treeOf(S.spaceA.id)).map((p) => p.title) } });
  info(`facts before the restart: ${JSON.stringify(readState().before.pages)}`);
}

async function phaseRestart() {
  const S = readState();
  const I = ids();
  const before = S.before;
  must(before !== undefined, "the facts of the pre-restart phase were saved");
  await waitFor(".tab-strip", "the tab strip", 60000);
  await wait(2500);
  // Language.
  const words = messagesOf("fr");
  const lang = await page.evaluate(`({ lang: document.documentElement.lang, nav: [...document.querySelectorAll(".nav-list .nav-item")].map((b) => b.textContent.trim()), accent: document.documentElement.dataset.accent ?? null })`);
  check(lang.lang === "fr" && lang.nav[0] === words["nav.home"], `the language survived: <html lang> ${lang.lang}, "${lang.nav.join(" | ")}"`);
  check((await SETTING("ui.language")) === "fr", "ui.language is fr");
  // Accent.
  const accent = await accentNow();
  const expected = accent.dark ? "#73de9f" : "#1c7842";
  check(lang.accent === "green" && accent.token === expected, `the accent survived: green (${accent.token})`);
  // Pages: covers, appearance, lock.
  const tree = await treeOf(S.spaceA.id);
  check(livePages(tree).length === before.titles.length, `the tree has its ${before.titles.length} pages (${livePages(tree).length})`);
  for (const [name, was] of Object.entries(before.pages)) {
    const p = await pageGet(I[name]);
    const now = { cover: p.cover ?? null, coverY: p.coverY ?? null, font: p.font ?? null, smallText: p.smallText ?? null, locked: p.locked ?? null };
    check(JSON.stringify(now) === JSON.stringify(was), `${name}: cover, position, font, small text and lock are as before (${JSON.stringify(now).slice(0, 110)})`);
  }
  await openPage(I.garden);
  await waitUntil(`(() => { const i = document.querySelector(".page-cover img.page-cover-image"); return !!(i && i.complete && i.naturalWidth === 600); })()`, "Garden's uploaded cover loads after the restart (600 px)", 30000);
  const garden = await coverState();
  check(garden.img.objectPosition === `50% ${before.pages.garden.coverY}%`, `its reposition survived (${garden.img.objectPosition})`);
  await openPage(I.orchard);
  const orchard = await waitUntil(`(() => { const c = document.querySelector('.page-cover[data-cover-gradient="aurora"]'); const s = document.querySelector("section.page"); return c ? { cls: s.className } : null; })()`, "Orchard plan's gradient cover is back");
  check(orchard.cls.includes("page-font-serif") && orchard.cls.includes("page-small-text") && !orchard.cls.includes("page-locked"), `Orchard plan is serif and small (${orchard.cls})`);
  await openPage(I.cherry);
  const cherry = await waitUntil(`(() => { const s = document.querySelector("section.page"); return s ? { cls: s.className, editable: document.querySelector(".ProseMirror")?.getAttribute("contenteditable") } : null; })()`, "Cherry is open");
  check(cherry.cls.includes("page-locked") && cherry.editable === "false", `Cherry is still locked and read-only (${cherry.editable})`);
  // Calculations.
  await openPage(I.budget);
  const footer = await waitUntil(`(() => { const c = document.querySelector('.page-table-footer-cell[data-kind="number"]'); const l = c?.querySelector(".page-table-calc-label")?.textContent.trim(); const v = c?.querySelector(".page-table-calc-value")?.textContent.trim(); return l ? { label: l, value: v } : null; })()`, "Budget's footer shows its calculation", 30000);
  check(footer.label === words["properties.rollup.fn.sum"] && /^1[,.\s  ]?580$/.test(footer.value), `the table footer's calculation survived: ${footer.label} ${footer.value}`);
  await capture("restart-budget");
  // The shell: the shortcut is registered again, the menu and the tray speak French.
  await checkNativeLabels("fr");
  const probe = await helper("hotkey-probe", {});
  check(probe.registeredByUs === false && probe.error === 1409, "after the restart the app owns Ctrl+Alt+N again");
  const trash = await treeOf(S.spaceA.id);
  check(trash.trashed.length === 1, `the trash still holds its page (${trash.trashed.length})`);
}

// <<phases go above this line>>

// ---------------------------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------------------------

/** Phases after which the document may legitimately be gone (the process exits). */
const NO_INTACT = new Set(["tray-quit", "tray-closeoff"]);

const PHASE_FUNCS = {
  mark: phaseMark,
  launch: phaseLaunch,
  seed: phaseSeed,
  ui: phaseUi,
  menu: phaseMenu,
  "deeplink-cold": phaseDeeplinkCold,
  "deeplink-invalid": phaseDeeplinkInvalid,
  "deeplink-before": phaseDeeplinkBefore,
  "deeplink-forward": phaseDeeplinkForward,
  "deeplink-plain": phaseDeeplinkPlain,
  "deeplink-lockprep": phaseDeeplinkLockPrep,
  "deeplink-locked": phaseDeeplinkLocked,
  tray: phaseTray,
  "tray-quit": phaseTrayQuit,
  "tray-closeoff": phaseTrayCloseoff,
  prerestart: phasePrerestart,
  restart: phaseRestart,
  // <<phase table entries go above this line>>
};

/** The webview's own document: nothing may have replaced it. */
async function intact(when) {
  const state = readState();
  const now = await page.evaluate(`(() => ({ marker: window.__sgMarker ?? null, href: location.href }))()`).catch(() => null);
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
    check(false, `exception: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
  }
}

if (phase !== "mark" && phase !== "launch" && !NO_INTACT.has(phase)) {
  try {
    await intact(`end of ${phase}`);
  } catch (error) {
    if (!(error instanceof PhaseStop)) {
      check(false, `exception at the end: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}

await wait(500);
const report = page.report();
const violations = await page.evaluate("window.__cspViolations ?? []").catch(() => []);
// The origins the app itself talks to: its own pages and assets, the IPC bridge, the media scheme. Nothing else.
const ALLOWED = /^(?:https?:\/\/(?:tauri\.localhost|ipc\.localhost|kbmedia\.localhost)|tauri:\/\/localhost|ipc:\/\/localhost|kbmedia:\/\/localhost|data:|blob:|about:)/u;
const foreign = report.requests.filter((r) => !ALLOWED.test(r.url));
result.requests = report.requests.length;
result.foreignRequests = foreign;
result.failures = report.failures;
result.cspViolations = violations;
result.console = report.console.filter((c) => c.type !== "log" && c.type !== "info" && c.type !== "debug");
result.seconds = round((Date.now() - started) / 1000);
check(foreign.length === 0, `no request to another host (${foreign.length}${foreign.length > 0 ? ": " + foreign.map((f) => f.url.slice(0, 80)).slice(0, 3).join(" | ") : ""})`);
check(violations.length === 0, `no content-security-policy violation (${violations.length}${violations.length > 0 ? ": " + violations.map((v) => `${v.directive} ${String(v.blocked).slice(0, 60)}`).slice(0, 3).join(" | ") : ""})`);
// A request the page itself cancelled (a media element that was stopped, an aborted fetch) is not a failure.
const hardFailures = report.failures.filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED");
check(hardFailures.length === 0, `no failed request (${hardFailures.length}${hardFailures.length > 0 ? ": " + hardFailures.map((f) => f.error).slice(0, 3).join(" | ") : ""})`);
const errors = report.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
check(errors.length === 0, `no console error (${errors.length}${errors.length > 0 ? ": " + errors.map((e) => e.text.replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, "<id>")).slice(0, 3).join(" | ") : ""})`);
if (outFile && phase !== "mark") {
  writeFileSync(outFile, JSON.stringify(result, null, 2));
}
// The report file has every note; the console has what failed (or, with SG_VERBOSE, everything).
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
      ...(process.env.SG_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
page.close();
process.exit(result.ok ? 0 : 1);

