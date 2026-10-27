// The in-page half of the spaces pass (drive-spaces.ps1, ADR-0038 moving pages between spaces and ADR-0039 importing an AppFlowy
// workspace): trusted mouse and key input, the DOM, the core (through the page's own IPC), the native dialogs (Win32 helpers of the
// .ps1) and the files, over the DevTools port of the app's WebView2 (the calls of cdp.mjs). Phases:
//
//   node drive-spaces.mjs mark           <outfile>  the marker of the document is (re)set (after a restart of the binary)
//   node drive-spaces.mjs launch         <outfile>  the document's marker; the first space is empty
//   node drive-spaces.mjs seed           <outfile>  Garden (child Tomatoes, a tag, an image), Plans (a chip to Garden), a second space Work, a favourite
//   node drive-spaces.mjs move           <outfile>  Garden's row menu -> Move to... -> Work -> Top level -> Next -> Move; trees, image, link, search, tabs, favourite
//   node drive-spaces.mjs move-back      <outfile>  Garden's page menu -> Move to... -> the first space -> under Plans -> Move; the same checks
//   node drive-spaces.mjs appflowy-no    <outfile>  a SYNTHETIC backup, "Download images?" -> No: one new space, the image stays a link
//   node drive-spaces.mjs appflowy-real  <outfile>  a real backup (-RealZip, read-only), "Download images?" -> Yes: counts only
//   node drive-spaces.mjs appflowy-big   <outfile>  a second backup (-BigZip, read-only), "Download images?" -> No: counts only
//   node drive-spaces.mjs move-imported  <outfile>  the first root page of the first new space of appflowy-real moved to the first space
//   node drive-spaces.mjs move-board     <outfile>  the board page of the real import (38 rows, a select property) moved to the first space
//   node drive-spaces.mjs move-files     <outfile>  the first page of the real import with file blocks moved to the first space
//   node drive-spaces.mjs restart        <outfile>  after a restart of the binary on the same data every space is as it was
//   node drive-spaces.mjs logs           <outfile>  the log of the shell and the core: no panic, no ERROR, no page text, no archive file name
//   node drive-spaces.mjs verify         <outfile>  the summary of every phase
//
// Every page phase writes what the page asked the network for, what failed, the console and the content-security-policy violations
// it saw, as JSON, and exits 1 when its own check fails, when the page asked another host for something (the page never downloads:
// the shell does), when a violation, a failed request or a console error was seen, or when the document of the webview was replaced
// (the marker `window.__spacesMarker` is lost by a reload). The two real archives are only read (by the app, through the native
// Open dialog): nothing of them is copied here, and the reports hold counts and codes, never titles, names or text.

import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync } from "node:zlib";

const CDP_PORT = Number(process.env.CDP_PORT ?? 9333);
const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.SPACES_OUT_DIR ?? join(HERE, "out");
const STATE_FILE = join(OUT, "spaces-state.json");
const SRC = join(OUT, "spaces-src");
const REAL_ZIP = process.env.SPACES_REAL_ZIP ?? "";
const BIG_ZIP = process.env.SPACES_BIG_ZIP ?? "";
const SYNTH_ZIP = join(SRC, "spaces-synthetic-backup.zip");
const PHASES = ["launch", "seed", "move", "move-back", "appflowy-no", "appflowy-real", "appflowy-big", "move-imported", "move-board", "move-files", "restart", "logs"];
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(OUT, { recursive: true });

function readState() {
  return existsSync(STATE_FILE) ? JSON.parse(readFileSync(STATE_FILE, "utf8")) : {};
}
function writeState(patch) {
  writeFileSync(STATE_FILE, JSON.stringify({ ...readState(), ...patch }, null, 2));
}
const round = (n) => Math.round(n * 10) / 10;

// The texts of the pages this pass makes: they must never be in the log.
const GARDEN = "Garden";
const TOMATOES = "Tomatoes";
const PLANS = "Plans";
const WORK = "Work";
const GARDEN_TEXT = "Compost and heirloom seeds go in the north bed.";
const TOMATOES_TEXT = "Tomatoes need full sun and a long stake.";
const PLANS_TEXT = "Plans for the spring season, see ";
const TAG = "vegetables";
const SYNTH_SPACE = "Synthetic Space";
const SYNTH_ONE = "Synthetic one";
const SYNTH_TWO = "Synthetic two";
const SYNTH_IMAGE = "https://example.invalid/x.png";

// ---------------------------------------------------------------------------------------------------------------
// helper steps of the .ps1 (native dialogs, zip text): needed by the log scan too
// ---------------------------------------------------------------------------------------------------------------

let helperCount = 0;
/** Runs one of drive-spaces.ps1's helper steps; its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}) {
  helperCount += 1;
  const argFile = join(OUT, `spaces-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `spaces-helper-${process.pid}-${helperCount}.result.json`);
  writeFileSync(argFile, JSON.stringify(params));
  rmSync(resultFile, { force: true });
  return new Promise((resolvePromise, reject) => {
    execFile(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", process.env.SPACES_PS1, "-Step", step, "-Arg", argFile, "-Result", resultFile],
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

/** The log files of the pass, oldest first (`spaces.err.log`, `spaces-r1.err.log`, ...). */
const logFiles = () => readdirSync(OUT).filter((n) => /^spaces(-r\d+)?\.err\.log$/.test(n)).sort((a, b) => (a === "spaces.err.log" ? -1 : b === "spaces.err.log" ? 1 : a.localeCompare(b, undefined, { numeric: true })));
const stripAnsi = (text) => text.replace(/\u001b\[[0-9;]*m/g, "");
/** The size of the newest log, to read only what a step wrote. */
function logMark() {
  const files = logFiles();
  const file = files[files.length - 1];
  return file === undefined ? { file: null, size: 0 } : { file, size: statSync(join(OUT, file)).size };
}
function logSince(mark) {
  if (mark.file === null) return "";
  const text = readFileSync(join(OUT, mark.file));
  return stripAnsi(text.subarray(mark.size).toString("utf8"));
}

/**
 * Words that must never be in the log: what this pass types, the names of the synthetic archive, the file names of the real ones,
 * and (read from the archives into memory only, never written) their page names and workspace ids. The report shows an index for
 * those, never the word.
 */
async function archiveWords(zip) {
  const words = [];
  if (!zip || !existsSync(zip)) return words;
  try {
    const answer = await helper("zip-text", { zip, entry: "relation_map.json" });
    if (answer.ok !== true) return words;
    const map = JSON.parse(answer.text);
    if (typeof map.workspace_id === "string") words.push(map.workspace_id);
    for (const view of Object.values(map.views ?? {})) {
      const name = typeof view?.name === "string" ? view.name.trim() : "";
      if (name.length >= 6 && name !== "Untitled") words.push(name);
    }
  } catch {
    // unreadable: no needles from this archive
  }
  return [...new Set(words)].slice(0, 800);
}

async function scanLogs() {
  const files = logFiles();
  const log = { files: [], lines: 0, panics: [], errors: [], warnings: [], leaks: [], needles: 0, hostLines: 0 };
  const named = [GARDEN, TOMATOES, PLANS, "heirloom", TAG, "Spaces workspace", SYNTH_SPACE, SYNTH_ONE, SYNTH_TWO, "garden.png", "spaces-synthetic-backup", ...[REAL_ZIP, BIG_ZIP].filter(Boolean).map((zip) => basename(zip).replace(/\.zip$/i, "")), "example.invalid", "x.png", "Compost", "north bed"];
  const realWords = [...(await archiveWords(REAL_ZIP)), ...(await archiveWords(BIG_ZIP))];
  log.needles = named.length + realWords.length;
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
      if (/beta\.appflowy\.cloud/.test(line)) log.hostLines += 1;
    }
    for (const needle of named) {
      if (!text.includes(needle)) continue;
      const pattern = new RegExp(`(?<![A-Za-z0-9_])${escapeRe(needle)}(?![A-Za-z0-9_])`);
      const hits = lines.filter((l) => pattern.test(l));
      if (hits.length > 0) log.leaks.push({ needle, kind: "named", file, count: hits.length, first: hits[0].slice(0, 300) });
    }
    realWords.forEach((word, index) => {
      if (!text.includes(word)) return;
      const pattern = new RegExp(`(?<![A-Za-z0-9_])${escapeRe(word)}(?![A-Za-z0-9_])`);
      const hits = lines.filter((l) => pattern.test(l)).length;
      if (hits > 0) log.leaks.push({ needle: `(word #${index} of a real archive)`, kind: "archive", file, count: hits });
    });
  }
  return log;
}

if (phase === "logs") {
  const log = await scanLogs();
  const ok = log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0;
  const result = { phase: "logs", ok, notes: [], log };
  result.notes.push(`${log.files.length > 0 ? "ok" : "FAIL"}: the log is there (${log.files.map((f) => `${f.file} ${f.lines} lines`).join(", ")})`);
  result.notes.push(`${log.panics.length === 0 ? "ok" : "FAIL"}: no panic (${log.panics.length})`);
  result.notes.push(`${log.errors.length === 0 ? "ok" : "FAIL"}: no ERROR line (${log.errors.length}${log.errors.length ? `: ${log.errors.slice(0, 3).join(" | ")}` : ""})`);
  result.notes.push(`${log.leaks.length === 0 ? "ok" : "FAIL"}: none of the page text, titles, archive names or archive words is in the log (${log.leaks.length} leaks${log.leaks.length ? `: ${log.leaks.map((l) => `${l.needle} x${l.count}`).join(", ")}` : ""}; ${log.needles} needles)`);
  const control = /(?<![A-Za-z0-9_])Garden(?![A-Za-z0-9_])/.test("INFO a made-up line that says Garden in it") && !/(?<![A-Za-z0-9_])Garden(?![A-Za-z0-9_])/.test("INFO a line about Gardener");
  result.notes.push(`${control ? "ok" : "FAIL"}: control: the whole-word matcher finds a needle in a made-up line and not inside another word`);
  result.ok = result.ok && control;
  result.notes.push(`info: ${log.warnings.length} WARN lines; ${log.hostLines} log lines name the AppFlowy server host`);
  if (outFile) writeFileSync(outFile, JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ phase, ok: result.ok, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, warnings: log.warnings.slice(0, 40), hostLines: log.hostLines, files: log.files }, null, 2));
  process.exit(result.ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// verify: the summary of the phase reports (no app needed)
// ---------------------------------------------------------------------------------------------------------------
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0, details: {}, bugs: [], timings: {}, log: null };
  for (const name of PHASES) {
    const file = join(OUT, `spaces-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = { ok: report.ok, checks: report.notes.filter((n) => !n.startsWith("info:")).length, failed: report.notes.filter((n) => n.startsWith("FAIL")).length, seconds: report.seconds };
    summary.captures.push(...(report.captures ?? []));
    if (report.details !== undefined) summary.details[name] = report.details;
    if (report.timings !== undefined) summary.timings[name] = report.timings;
    // The one finding of this machine: it refuses outbound connections to Rust-built executables (os error 10013), so the 28 image
    // downloads of appflowy-real fail. With SPACES_DOWNLOADS_BLOCKED=1 that single failed check is listed apart, not as a failure.
    const failedHere = report.notes.filter((n) => n.startsWith("FAIL"));
    const blockedNote = (n) => process.env.SPACES_DOWNLOADS_BLOCKED === "1" && name === "appflowy-real" && n.startsWith("FAIL: appflowy-real: 28 images downloaded (0 downloaded, 28 failed");
    summary.acknowledged = [...(summary.acknowledged ?? []), ...failedHere.filter(blockedNote).map((n) => `${name}: ${n}`)];
    if (failedHere.length > 0 && failedHere.every(blockedNote)) {
      summary.phases[name].ok = true;
      summary.phases[name].acknowledged = failedHere.length;
    }
    summary.failedNotes.push(...failedHere.filter((n) => !blockedNote(n)).map((n) => `${name}: ${n}`));
    summary.bugs.push(...(report.bugs ?? []).map((b) => `${name}: ${b}`));
    summary.foreignRequests += report.foreignRequests?.length ?? 0;
    summary.cspViolations += report.cspViolations?.length ?? 0;
    summary.failedRequests += (report.failures ?? []).filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED").length;
    summary.consoleErrors += (report.console ?? []).filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).length;
  }
  summary.log = await scanLogs();
  const log = summary.log;
  summary.ok = Object.values(summary.phases).every((p) => p.ok) && log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0;
  writeFileSync(outFile, JSON.stringify(summary, null, 2));
  console.log(
    JSON.stringify(
      {
        phase: "verify",
        ok: summary.ok,
        phases: summary.phases,
        failedNotes: summary.failedNotes,
        acknowledged: summary.acknowledged ?? [],
        bugs: summary.bugs,
        foreignRequests: summary.foreignRequests,
        cspViolations: summary.cspViolations,
        failedRequests: summary.failedRequests,
        consoleErrors: summary.consoleErrors,
        timings: summary.timings,
        log: { files: log.files, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, warnings: log.warnings.slice(0, 40) },
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
      await new Promise((r) => setTimeout(r, 1000));
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
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const n = ++id;
      waiting.set(n, { resolve, reject });
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  await send("Network.enable");
  await send("Log.enable");
  await send("Runtime.enable");
  await send("Page.enable");
  // The window of a freshly started binary is often behind the terminal that started it: without this the page sees a blur and
  // fields that commit on blur (the sidebar's rename field) close at once. The page behaves as if its window had the focus.
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
    async key(key, code = key, windowsVirtualKeyCode = 0, modifiers = 0) {
      await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode, modifiers });
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
async function ctrl(letter) {
  const upper = letter.toUpperCase();
  await page.key(letter, `Key${upper}`, upper.charCodeAt(0), MOD.ctrl);
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

/** Captures the window with PrintWindow (capture.ps1's Capture-Window, run in PowerShell). */
function capture(name) {
  const path = join(OUT, `spaces-${name}.png`);
  return new Promise((resolvePromise) => {
    execFile(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        '. $env:SPACES_CAPTURE_PS1; $p = Get-Process -Name atomik-kb-desktop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1; Capture-Window $p.MainWindowHandle $env:SPACES_CAPTURE_PATH',
      ],
      { env: { ...process.env, SPACES_CAPTURE_PATH: path }, timeout: 40000 },
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

// ----- native dialogs ---------------------------------------------------------------------------------------------

/** Drives a native file dialog that the page has just asked for: types `text` and closes it. */
async function fileDialog(title, text, timeoutMs = 30000) {
  const answer = await helper("dialog", { title, steps: [text], timeoutMs });
  const shown = text.replace(/^.*[\\/]/, "");
  info(`dialog "${title}": waited ${answer.waitedMs} ms, edit control ${answer.edit}, ${answer.closed ? "closed" : "STILL OPEN"}`);
  check(answer.ok === true, `the "${title}" dialog took the file and closed${answer.error ? ` (${answer.error}; seen ${JSON.stringify(answer.seen)}; controls ${JSON.stringify(answer.controls)})` : ""}`);
  void shown;
  return answer;
}

/**
 * Answers a native confirmation of the shell (a message box with custom buttons): finds it by its title, reads what it says,
 * captures it (`captureAs` is a name for out/spaces-<name>.png) and presses the button named `press`.
 */
async function confirmation(title, press, { captureAs = null, timeoutMs = 30000 } = {}) {
  const capturePath = captureAs === null ? "" : join(OUT, `spaces-${captureAs}.png`);
  const answer = await helper("message-dialog", { title, press, capture: capturePath, timeoutMs });
  const said = [...(answer.texts ?? [])].join(" | ");
  const buttons = [...new Set([...(answer.buttons ?? []), ...(answer.children ?? []).filter((c) => c.startsWith("Button ")).map((c) => c.replace(/^Button '(.*)'$/, "$1").replace(/&/g, ""))])];
  if (answer.captured) result.captures.push(answer.captured);
  return { ...answer, said, buttons };
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
const attachmentsOf = (spaceId) => ask({ type: "AttachmentList", data: { spaceId } });
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
const sameSet = (a, b) => a.length === b.length && [...a].sort().join("|") === [...b].sort().join("|");

/** The ProseMirror JSON of a page, walked: what the page holds, as counts (never text). */
function walkNodes(node, fn) {
  fn(node);
  for (const child of node.content ?? []) walkNodes(child, fn);
}
function contentFacts(content) {
  const facts = { images: 0, imagesWithAttachment: 0, files: 0, boards: 0, tables: 0, hrefs: [], attachmentIds: [], pageRefs: 0, pageLinks: 0, blocks: 0 };
  walkNodes(content, (node) => {
    if (node.type === "image") {
      facts.images += 1;
      if (typeof node.attrs?.attachmentId === "string") {
        facts.imagesWithAttachment += 1;
        facts.attachmentIds.push(node.attrs.attachmentId);
      }
    } else if (node.type === "file") {
      facts.files += 1;
      if (typeof node.attrs?.attachmentId === "string") facts.attachmentIds.push(node.attrs.attachmentId);
    } else if (node.type === "pageList") {
      if (node.attrs?.layout === "board") facts.boards += 1;
      if (node.attrs?.layout === "table") facts.tables += 1;
    } else if (node.type === "pageRef") {
      facts.pageRefs += 1;
    } else if (node.type === "pageLink") {
      facts.pageLinks += 1;
    }
    for (const mark of node.marks ?? []) {
      if (mark.type === "link" && typeof mark.attrs?.href === "string") facts.hrefs.push(mark.attrs.href);
    }
    if (node.type !== "doc") facts.blocks += 1;
  });
  return facts;
}
const pageContent = async (pageId) => (await ask({ type: "PageContentGet", data: { pageId } })).content;

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

/** Opens the page menu ("Page options") of the open page and chooses `label`; returns the labels the menu showed. */
async function pageMenu(label) {
  await normalize();
  await clickOn('button[aria-label="Page options"]', "the page options button");
  await waitFor('[role="menu"]', "the page options menu");
  const labels = await menuLabels();
  await chooseMenuItem(label);
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

/** Opens the list of spaces of the switcher. */
async function openSpaceList() {
  await normalize();
  await clickOn("button.space-button", "the space switcher button");
  await waitFor(".space-popover", "the list of spaces");
}

/** Makes `spaceId` the current space through the switcher (the nth entry of the list, by position: names may be private). */
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

/** A page opened in a background tab with a Ctrl+click on its row (the ancestors expanded first). */
async function openInBackgroundTab(spaceId, id) {
  await expandAncestors(spaceId, id);
  const row = await box(`[role="treeitem"][data-page-id="${id}"] .tree-title`);
  await mouse(row.cx, row.cy, { modifiers: MOD.ctrl });
  await wait(500);
}

/** The tabs of the strip: title, active, position. */
const tabsInfo = () =>
  page.evaluate(`[...document.querySelectorAll('.tab-list [role="tab"]')].map((t) => {
    const r = t.getBoundingClientRect();
    return { title: t.querySelector(".tab-title")?.textContent.trim() ?? t.getAttribute("aria-label") ?? "", active: t.getAttribute("aria-selected") === "true", cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })`);

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
  const made = livePages(await treeOf(spaceId)).find((p) => p.title === title);
  must(made !== undefined, `the page "${title}" exists`);
  return made.id;
}

/** Types `text` into the editor at a fresh line and leaves the editor, so the text is saved. */
async function writeLine(text) {
  await newLine();
  await send("Input.insertText", { text });
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
}

// ----- files ------------------------------------------------------------------------------------------------------------

/** A PNG of `size` x `size` pixels in two colours (a green square with a gold disc), written with node's zlib. */
function pngBytes(size = 40, base = [40, 150, 80], gold = [255, 200, 40]) {
  const raw = Buffer.alloc((size * 3 + 1) * size);
  const c = size / 2;
  for (let y = 0; y < size; y++) {
    raw[y * (size * 3 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      const inside = (x - c) ** 2 + (y - c) ** 2 < (size / 3) ** 2;
      raw.set(inside ? gold : base, y * (size * 3 + 1) + 1 + x * 3);
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

/** A .zip with every entry stored (no compression), names as given, UTF-8 names. */
function makeZip(path, entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const { name, data } of entries) {
    const nameBytes = Buffer.from(name, "utf8");
    const crc = crc32(data) >>> 0;
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6);
    local.writeUInt16LE(0, 8);
    local.writeUInt16LE(0x6000, 10);
    local.writeUInt16LE(0x5d4d, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    local.writeUInt16LE(0, 28);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(0x6000, 12);
    central.writeUInt16LE(0x5d4d, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, data);
    centrals.push(central, nameBytes);
    offset += local.length + nameBytes.length + data.length;
  }
  const centralSize = centrals.reduce((n, b) => n + b.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centralSize, 12);
  end.writeUInt32LE(offset, 16);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, Buffer.concat([...locals, ...centrals, end]));
}

/** A SYNTHETIC AppFlowy backup (the format of appflowy-import.md section 2): one space, two documents, one remote image. */
function makeSyntheticBackup(path) {
  const WS = "ws-spaces-0001";
  const doc = () => {
    const blocks = { root: { id: "root", ty: "page", parent: "", children: "ch-root", external_id: null, external_type: null, data: {} } };
    const children = { "ch-root": [] };
    const text = {};
    let n = 0;
    const add = (parent, ty, data, delta) => {
      n += 1;
      const id = `b${n}`;
      let external = [null, null];
      if (delta !== null) {
        text[`tx-${id}`] = JSON.stringify(delta);
        external = [`tx-${id}`, "text"];
      }
      blocks[id] = { id, ty, parent, children: `ch-${id}`, external_id: external[0], external_type: external[1], data };
      children[`ch-${id}`] = [];
      children[`ch-${parent}`].push(id);
    };
    return {
      add,
      json: () => JSON.stringify({ page_id: "root", blocks, meta: { children_map: children, text_map: text } }),
    };
  };
  const one = doc();
  one.add("root", "heading", { level: 1 }, [{ insert: "A synthetic heading" }]);
  one.add("root", "paragraph", {}, [{ insert: "The first paragraph of the first synthetic page." }]);
  one.add("root", "paragraph", {}, [{ insert: "The second paragraph, with " }, { insert: "bold words", attributes: { bold: true } }, { insert: " in it." }]);
  one.add("root", "image", { url: SYNTH_IMAGE, align: "center" }, null);
  one.add("root", "paragraph", {}, [{ insert: "A closing paragraph after the picture." }]);
  const two = doc();
  two.add("root", "paragraph", {}, [{ insert: "The only paragraph of the second synthetic page." }]);
  two.add("root", "bulleted_list", {}, [{ insert: "A bullet" }]);
  const view = (id, name, parent, children, extra) => ({
    view_id: id, name, layout: 0, parent_id: parent, children, collab_object_id: id, created_at: 1700000010, updated_at: 1700000010, extra: extra === null ? null : JSON.stringify(extra), icon: null,
  });
  const relation = {
    workspace_id: WS,
    export_timestamp: 1700000100,
    archive_capabilities: ["content_addressed_attachments_v1"],
    views: {
      "sp-1": view("sp-1", SYNTH_SPACE, WS, ["doc-1", "doc-2"], { is_space: true, space_icon: "", space_icon_color: "", space_permission: 0, space_created_at: 1700000000000 }),
      "doc-1": view("doc-1", SYNTH_ONE, "sp-1", [], null),
      "doc-2": view("doc-2", SYNTH_TWO, "sp-1", [], null),
    },
    collab_objects: {},
    dependencies: [],
    workspace_database_meta: [],
  };
  const metadata = { export_info: { workspace_id: WS, export_timestamp: 1700000100, total_views: 3, version: "1.0", integrity: "none", omitted_item_count: 0 } };
  const utf8 = (s) => Buffer.from(s, "utf8");
  makeZip(path, [
    { name: "metadata.json", data: utf8(JSON.stringify(metadata)) },
    { name: "relation_map.json", data: utf8(JSON.stringify(relation)) },
    { name: "collab_jsons/documents/doc-1.json", data: utf8(one.json()) },
    { name: "collab_jsons/documents/doc-2.json", data: utf8(two.json()) },
  ]);
}

// ---------------------------------------------------------------------------------------------------------------
// the moves (phases move, move-back, move-imported)
// ---------------------------------------------------------------------------------------------------------------

/** What the dialog's confirmation says, read from the page. */
const readConfirmation = () =>
  page.evaluate(`(() => {
    const dialog = document.querySelector('[role="dialog"]');
    if (!dialog) return null;
    return {
      summary: dialog.querySelector(".move-summary")?.textContent.trim() ?? null,
      where: [...dialog.querySelectorAll("p.muted")].map((p) => p.textContent.trim()),
      notes: [...dialog.querySelectorAll(".move-notes li")].map((li) => li.textContent.trim()),
      error: dialog.querySelector(".error-text")?.textContent.trim() ?? null,
    };
  })()`);

const dialogButtons = () => page.evaluate(`[...document.querySelectorAll('[role="dialog"] button')].map((b) => ({ text: b.textContent.trim(), disabled: b.disabled }))`);

/**
 * Drives the Move dialog that is open: step 1 picks the space named `targetName`, step 2 the place (`parentLabel` null = Top
 * level), step 3 reads the confirmation and presses Move. Returns what the dialog said.
 */
async function driveMoveDialog({ title, targetName, parentLabel, currentSpaceName, capturePrefix }) {
  await waitFor('[role="dialog"] .subtitle', "the Move dialog");
  const dialogTitle = await page.evaluate(`document.querySelector('[role="dialog"] .subtitle')?.textContent.trim()`);
  check(dialogTitle === `Move “${title}”`, `the dialog is titled Move “<the page>” (${title === GARDEN ? dialogTitle : "title not shown"})`);
  // Step 1: the other spaces.
  const others = await page.evaluate(`[...document.querySelectorAll('[role="dialog"] .move-spaces .space-item')].map((b) => ({ name: b.querySelector(".space-item-name")?.textContent.trim(), count: b.querySelector(".move-space-count")?.textContent.trim() ?? "" }))`);
  info(`step 1 lists ${others.length} other spaces`);
  if (currentSpaceName !== null) {
    check(!others.some((o) => o.name === currentSpaceName), "step 1 does not list the page's own space");
  }
  const total = (await spaceList()).length;
  check(others.length === total - 1, `step 1 lists every other space (${others.length} of ${total - 1})`);
  const targetBox = await page.evaluate(`(() => {
    const el = [...document.querySelectorAll('[role="dialog"] .move-spaces .space-item')].find((b) => b.querySelector(".space-item-name")?.textContent.trim() === ${JSON.stringify(targetName)});
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })()`);
  must(targetBox !== null, "step 1 offers the target space");
  if (capturePrefix) await capture(`${capturePrefix}-step1`);
  await mouse(targetBox.cx, targetBox.cy);
  // Step 2: the place.
  await waitFor('[role="dialog"] fieldset.move-picker', "the place picker (step 2)");
  const labels = await page.evaluate(`[...document.querySelectorAll('[role="dialog"] .move-option label')].map((l) => l.textContent.trim())`);
  check(labels[0] === "Top level", `the picker lists "Top level" first (${labels.length} places)`);
  const checked = await page.evaluate(`[...document.querySelectorAll('[role="dialog"] .move-option input')].map((i) => i.checked)`);
  check(checked[0] === true && checked.filter(Boolean).length === 1, "Top level is chosen at first");
  if (parentLabel !== null) {
    const labelBox = await page.evaluate(`(() => {
      const el = [...document.querySelectorAll('[role="dialog"] .move-option label')].find((l) => l.textContent.trim() === ${JSON.stringify(parentLabel)});
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
    })()`);
    must(labelBox !== null, `the picker lists the page "${parentLabel}"`);
    await mouse(labelBox.cx, labelBox.cy);
    const nowChecked = await page.evaluate(`[...document.querySelectorAll('[role="dialog"] .move-option')].filter((o) => o.querySelector("input").checked).map((o) => o.querySelector("label").textContent.trim())`);
    check(nowChecked.length === 1 && nowChecked[0] === parentLabel, `"${parentLabel}" is the chosen place (${nowChecked.join(", ")})`);
  }
  if (capturePrefix) await capture(`${capturePrefix}-step2`);
  await clickOn('[role="dialog"] button', "Next", { text: "Next" });
  // Step 3: the confirmation.
  await waitUntil(`document.querySelector('[role="dialog"] .move-summary')?.textContent.trim() ?? null`, "the confirmation (step 3)", 60000);
  const confirmed = await readConfirmation();
  if (capturePrefix) await capture(`${capturePrefix}-confirm`);
  const buttons = await dialogButtons();
  check(buttons.some((b) => b.text === "Move" && !b.disabled) && buttons.some((b) => b.text === "Back") && buttons.some((b) => b.text === "Cancel"), `the confirmation has Back, Cancel and Move (${buttons.map((b) => b.text).join(", ")})`);
  const pressedAt = Date.now();
  await clickOn('[role="dialog"] button', "Move", { text: "Move" });
  await waitUntil(`!document.querySelector('[role="dialog"]')`, "the Move dialog closed after Move", 120000);
  confirmed.moveMs = Date.now() - pressedAt;
  await wait(1200);
  return confirmed;
}

/** A snapshot of what the move changes: both trees, favourites, attachments, counts. */
async function snapshot(fromId, toId) {
  const [from, to] = [await treeOf(fromId), await treeOf(toId)];
  const spaces = await spaceList();
  return {
    from: { pages: livePages(from).map((p) => p.id), favorites: from.favorites ?? [], attachments: (await attachmentsOf(fromId)).map((a) => a.attachmentId), count: spaces.find((s) => s.id === fromId)?.pageCount },
    to: { pages: livePages(to).map((p) => p.id), favorites: to.favorites ?? [], attachments: (await attachmentsOf(toId)).map((a) => a.attachmentId), count: spaces.find((s) => s.id === toId)?.pageCount },
  };
}

/** The image of the open page loads (decoded, non-zero width). */
const imageLoaded = (ms = 30000) => waitUntil(`(() => { const i = document.querySelector(".ProseMirror img"); return i && i.complete && i.naturalWidth > 0 ? i.naturalWidth : 0; })()`, "the image of the open page loads", ms);

/**
 * The checks after a move of Garden (and Tomatoes) from `fromId` to `toId` (phases move and move-back): the core's trees, counts,
 * favourites, attachments, tags and backlinks; the sidebar; the image; the chip of Plans; search; tabs.
 */
async function verifyGardenMove({ label, fromId, toId, toName, parentId, before, plan }) {
  const s = readState();
  const after = await snapshot(fromId, toId);
  const treeTo = await treeOf(toId);
  const inTo = livePages(treeTo);
  const gardenTo = inTo.find((p) => p.id === s.gardenId);
  const tomatoesTo = inTo.find((p) => p.id === s.tomatoesId);
  check(gardenTo !== undefined && tomatoesTo !== undefined, `${label}: the target's tree has Garden and Tomatoes (same ids)`);
  check((gardenTo?.parentId ?? null) === parentId, `${label}: Garden is ${parentId === null ? "at the top level" : "under Plans"} in the target (parent ${gardenTo?.parentId ?? "none"})`);
  check(tomatoesTo?.parentId === s.gardenId, `${label}: Tomatoes is still Garden's child`);
  check(!after.from.pages.includes(s.gardenId) && !after.from.pages.includes(s.tomatoesId), `${label}: the source's tree no longer has them`);
  check(after.to.count === before.to.count + 2 && after.from.count === before.from.count - 2, `${label}: page counts moved by 2 (${before.from.count}->${after.from.count}, ${before.to.count}->${after.to.count})`);
  check(sameSet(after.from.pages, before.from.pages.filter((id) => id !== s.gardenId && id !== s.tomatoesId)), `${label}: nothing else left the source`);
  check(plan.pageIds.length === 2 && plan.pageIds[0] === s.gardenId && plan.pageIds[1] === s.tomatoesId, `${label}: the move's answer lists Garden then Tomatoes in pre-order`);
  check(plan.attachmentsMoved === 1 && plan.attachmentsLeft === 0 && plan.propertiesCreated === 0 && plan.valuesDropped === 0 && plan.relationLinksRemoved === 0, `${label}: the answer counts 1 attachment moved and nothing else (${JSON.stringify({ ...plan, pageIds: plan.pageIds.length })})`);
  // Favourite, attachments, tag, backlinks.
  check(after.to.favorites.includes(s.gardenId) && !after.from.favorites.includes(s.gardenId), `${label}: the favourite followed (target ${after.to.favorites.length}, source ${after.from.favorites.length})`);
  check(after.to.attachments.includes(s.attachmentId) && !after.from.attachments.includes(s.attachmentId), `${label}: the attachment is listed by the target space only`);
  const info_ = await ask({ type: "AttachmentGet", data: { attachmentId: s.attachmentId } });
  check(info_.spaceId === toId && info_.availability === "Complete" && info_.name === "garden.png" && info_.references >= 1, `${label}: AttachmentGet says it belongs to the target, complete, named garden.png (${JSON.stringify({ space: info_.spaceId === toId, availability: info_.availability, name: info_.name, refs: info_.references })})`);
  const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId: toId } })).properties;
  const tags = schema.find((d) => d.name === "Tags");
  const values = (await ask({ type: "PagePropertiesGet", data: { pageId: s.gardenId } })).values;
  const tagValue = values.find(([id]) => id === tags?.id)?.[1];
  const tagNames = (tagValue?.value ?? []).map((o) => tags?.options?.find((x) => x.id === o)?.name);
  check(tagValue?.kind === "multi_select" && tagNames.includes(TAG), `${label}: Garden's tag "${TAG}" is still there, named in the target's schema (${JSON.stringify(tagNames)})`);
  const backlinks = await ask({ type: "BacklinksGet", data: { pageId: s.gardenId } });
  check(backlinks.some((b) => b.pageId === s.plansId), `${label}: Plans still shows as a backlink of Garden (${backlinks.length} backlinks)`);
  // The sidebar and the page.
  check((await shownSpace()) === toName, `${label}: the sidebar switched to "${toName}" (${await shownSpace()})`);
  await waitFor(`[role="treeitem"][data-page-id="${s.gardenId}"]`, "Garden's row in the sidebar of the target");
  check((await heading()) === GARDEN, `${label}: Garden is the open page (${await heading()})`);
  const rowsInSidebar = await page.evaluate(`[...document.querySelectorAll('[role="treeitem"]')].map((e) => e.getAttribute("data-page-id"))`);
  check(!rowsInSidebar.includes(null) && new Set(rowsInSidebar).size === rowsInSidebar.length, `${label}: the sidebar lists no page twice`);
  const favoriteRows = await page.evaluate(`[...document.querySelectorAll(".favorites-list .favorite-open .tree-title")].map((e) => e.textContent.trim())`);
  check(favoriteRows.length === 1 && favoriteRows[0] === GARDEN, `${label}: the sidebar's Favourites list shows Garden (${favoriteRows.join(", ")})`);
  await capture(`${label}-after`);
  // Tabs: the active tab is Garden, Tomatoes' tab survived and opens in the target space.
  const tabs = await tabsInfo();
  check(before.tabCount === tabs.length, `${label}: the tab count is unchanged (${before.tabCount} -> ${tabs.length})`);
  check(tabs.filter((t) => t.active).length === 1 && tabs.find((t) => t.active)?.title === GARDEN, `${label}: the active tab is Garden (${tabs.map((t) => `${t.title}${t.active ? "*" : ""}`).join(", ")})`);
  check(tabs.filter((t) => t.title === GARDEN).length === 1 && tabs.filter((t) => t.title === TOMATOES).length === 1, `${label}: one tab each for Garden and Tomatoes`);
  const tomatoTab = tabs.find((t) => t.title === TOMATOES);
  if (tomatoTab !== undefined) {
    await mouse(tomatoTab.cx, tomatoTab.cy);
    await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(TOMATOES)}`, "the Tomatoes tab opens its page", 20000);
    await waitUntil(`document.querySelector(".ProseMirror")?.textContent.includes("long stake")`, "Tomatoes' text shows", 20000);
    check((await shownSpace()) === toName, `${label}: the Tomatoes tab opens the page in "${toName}" (${await shownSpace()})`);
    const again = await tabsInfo();
    const garden = again.find((t) => t.title === GARDEN);
    if (garden !== undefined) await mouse(garden.cx, garden.cy);
    await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN)}`, "the Garden tab opens its page", 20000);
  }
  // The link in Plans still opens Garden, wherever it is now.
  const spaces = await spaceList();
  const plansSpace = spaces.find((sp) => sp.id === s.spaceA.id);
  await switchSpaceUi(plansSpace.id);
  await openPageId(plansSpace.id, s.plansId);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(PLANS)}`, "Plans is open");
  const chip = await waitFor(".ProseMirror [data-page-ref]", "the chip to Garden in Plans");
  const chipFacts = await page.evaluate(`(() => { const c = document.querySelector(".ProseMirror [data-page-ref]"); return { id: c.getAttribute("data-page-id"), text: c.textContent.trim() }; })()`);
  check(chipFacts.id === s.gardenId && chipFacts.text === GARDEN, `${label}: the chip in Plans still reads "Garden" and points at Garden's id (${JSON.stringify(chipFacts)})`);
  await mouse(chip.cx, chip.cy);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN)}`, "the chip opens Garden", 20000);
  await wait(500);
  check((await shownSpace()) === toName, `${label}: following the chip leads to Garden in "${toName}" (${await shownSpace()})`);
  // A fresh mount of the page: the image is read again from the target space.
  const width = await imageLoaded();
  info(`${label}: the image shows after the chip opened Garden, ${width} px wide`);
  check(width === 40, `${label}: Garden's image still loads (readable in the target space, ${width} px)`);
  const text = await page.evaluate(`document.querySelector(".ProseMirror")?.textContent ?? ""`);
  check(text.includes("heirloom"), `${label}: Garden's text is intact`);
  // Search: the core and the dialog.
  const byTitle = await ask({ type: "Search", data: { query: GARDEN } });
  const byBody = await ask({ type: "Search", data: { query: "heirloom" } });
  check(byTitle.some((h) => h.pageId === s.gardenId && h.spaceId === toId) && !byTitle.some((h) => h.pageId === s.gardenId && h.spaceId !== toId), `${label}: search finds Garden by title in the target space only (${byTitle.filter((h) => h.pageId === s.gardenId).length} hits)`);
  check(byBody.some((h) => h.pageId === s.gardenId && h.spaceId === toId && h.blockId !== undefined) && !byBody.some((h) => h.spaceId !== toId && h.pageId === s.gardenId), `${label}: search finds Garden's text in the target space only`);
  const byTomato = await ask({ type: "Search", data: { query: "stake" } });
  check(byTomato.some((h) => h.pageId === s.tomatoesId && h.spaceId === toId), `${label}: search finds Tomatoes' text in the target space`);
  await page.key("k", "KeyK", 75, MOD.ctrl);
  await waitFor('input[role="combobox"].search-input', "the search dialog (Ctrl+K)");
  await clickOn('input[role="combobox"].search-input', "the search field");
  await send("Input.insertText", { text: "heirloom" });
  await waitUntil(`document.querySelectorAll(".search-results [role=option]").length > 0`, "search results show", 15000);
  const groups = await page.evaluate(`[...document.querySelectorAll(".search-results .search-group")].map((g) => g.getAttribute("aria-label"))`);
  check(groups.includes(GARDEN), `${label}: Ctrl+K lists Garden for a word of its text (${groups.join(", ")})`);
  await capture(`${label}-search`);
  // The hit opens the page (in the space it is in now).
  const hit = await box(`.search-results .search-group[aria-label=${JSON.stringify(GARDEN)}] [role=option]`);
  must(hit !== null, `${label}: the Garden hit is there to click`);
  await mouse(hit.cx, hit.cy);
  await waitUntil(`!document.querySelector(".search-dialog")`, "the search dialog closed after the click", 10000).catch(() => {});
  if (await box(".search-dialog")) {
    await page.key("Escape", "Escape", 27);
  }
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN)}`, "the search hit opens Garden", 15000);
  check((await shownSpace()) === toName, `${label}: the hit opened Garden in "${toName}" (${await shownSpace()})`);
  const idsNow = await ask({ type: "PageGet", data: { pageId: s.gardenId } });
  check(idsNow.spaceId === toId, `${label}: PageGet says Garden is in the target space`);
  return after;
}

async function moveGardenPhase({ label, via, fromKey, toKey, parentLabel, expectUnder, firstTime }) {
  const s = readState();
  const from = s[fromKey];
  const to = s[toKey];
  // Garden is open in the sidebar of its space, with a background tab for Tomatoes.
  const spaces = await spaceList();
  check((await shownSpace()) !== null, `${label}: the sidebar shows a space`);
  await switchSpaceUi(from.id);
  await openPageId(from.id, s.gardenId);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN)}`, "Garden is open");
  let tabs = await tabsInfo();
  if (!tabs.some((t) => t.title === TOMATOES)) {
    await openInBackgroundTab(from.id, s.tomatoesId);
    tabs = await tabsInfo();
    await openPageId(from.id, s.gardenId);
    await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN)}`, "Garden is open again");
    tabs = await tabsInfo();
  }
  check(tabs.some((t) => t.title === TOMATOES) && tabs.find((t) => t.active)?.title === GARDEN, `${label}: tabs before the move: ${tabs.map((t) => `${t.title}${t.active ? "*" : ""}`).join(", ")}`);
  const before = { ...(await snapshot(from.id, to.id)), tabCount: tabs.length };
  // The plan the core gives (the dialog must say the same).
  const planAnswer = await core({ type: "PageMoveToSpacePlan", data: { pageId: s.gardenId, spaceId: to.id, ...(parentLabel === null ? {} : { parentId: s.plansId }) } });
  check(planAnswer.ok === true && planAnswer.value.data.pageIds.length === 2, `${label}: PageMoveToSpacePlan says 2 pages (${planAnswer.ok ? planAnswer.value.data.pageIds.length : JSON.stringify(planAnswer.error)})`);
  const afterPlan = await snapshot(from.id, to.id);
  check(sameSet(afterPlan.from.pages, before.from.pages) && sameSet(afterPlan.to.pages, before.to.pages), `${label}: the plan wrote nothing`);
  // The menu.
  let labels;
  if (via === "row") {
    labels = await rowMenu(s.gardenId, "Move to…");
    const at = labels.indexOf("Move to…");
    check(at > 0 && labels[at - 1] === "Outdent" && labels[at + 1] === "Rename", `${label}: the row menu has "Move to…" between Outdent and Rename (${labels.join(" | ")})`);
  } else {
    labels = await pageMenu("Move to…");
    check(labels[labels.length - 1] === "Move to…", `${label}: the page menu ends with "Move to…" (${labels.join(" | ")})`);
  }
  const t0 = Date.now();
  const dialog = await driveMoveDialog({ title: GARDEN, targetName: to.name, parentLabel, currentSpaceName: from.name, capturePrefix: label });
  check(dialog.summary === `Move 2 pages to “${to.name}”`, `${label}: the confirmation says ${dialog.summary}`);
  check(dialog.where.some((w) => w === expectUnder), `${label}: it says where the page goes: ${dialog.where.join(" / ")}`);
  check(dialog.notes.length === 0, `${label}: no note line (all counts are zero): ${dialog.notes.length}`);
  check(dialog.error === null, `${label}: no error in the dialog (${dialog.error})`);
  info(`${label}: Move pressed to dialog closed in ${dialog.moveMs} ms (the whole flow ${Date.now() - t0} ms)`);
  const moved = (await ask({ type: "PageGet", data: { pageId: s.gardenId } })).spaceId === to.id;
  check(moved, `${label}: Garden belongs to the target space now`);
  const summary = planAnswer.value.data;
  const after = await verifyGardenMove({ label, fromId: from.id, toId: to.id, toName: to.name, parentId: parentLabel === null ? null : s.plansId, before, plan: summary });
  result.details = { label, via, menuLabels: labels, dialog: { summary: dialog.summary, notes: dialog.notes.length, moveMs: dialog.moveMs }, counts: { before: { from: before.from.count, to: before.to.count }, after: { from: after.from.count, to: after.to.count } }, spaces: spaces.length };
  void firstTime;
}

async function phaseMove() {
  const s = readState();
  await moveGardenPhase({ label: "move", via: "row", fromKey: "spaceA", toKey: "spaceB", parentLabel: null, expectUnder: "It goes at the top level.", firstTime: true });
  writeState({ gardenIn: s.spaceB.id });
}

async function phaseMoveBack() {
  const s = readState();
  // Garden is in Work: the sidebar switches there and opens it first (moveGardenPhase does).
  await moveGardenPhase({ label: "move-back", via: "header", fromKey: "spaceB", toKey: "spaceA", parentLabel: PLANS, expectUnder: `It goes under “${PLANS}”.`, firstTime: false });
  writeState({ gardenIn: s.spaceA.id });
}

// ---------------------------------------------------------------------------------------------------------------
// phase: mark, launch, seed
// ---------------------------------------------------------------------------------------------------------------

async function phaseMark() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `spaces-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__spacesMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href });
  info(`marker set on ${href}`);
}

async function phaseLaunch() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  rmSync(STATE_FILE, { force: true }); // a fresh pass starts with no memory of an earlier one
  await phaseMark();
  const size = await page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio, theme: document.documentElement.dataset.theme ?? "(none)" })`);
  info(`window ${size.w}x${size.h} at ${size.dpr}x, data-theme ${size.theme}`);
  check((await heading()) === "Spaces workspace", `the main area shows the workspace's Home (${await heading()})`);
  const spaces = await spaceList();
  must(spaces.length === 1, `the workspace has one space (${spaces.length})`);
  writeState({ spaceA: { id: spaces[0].id, name: spaces[0].name } });
  info(`the first space is "${spaces[0].name}"`);
  const tree = await treeOf(spaces[0].id);
  check(tree.roots.length === 0, `the space starts empty (${tree.roots.length} pages)`);
  check((await shownSpace()) === spaces[0].name, `the sidebar shows the space (${await shownSpace()})`);
  check(existsSync(join(OUT, "spaces-data")), "the data directory is the pass's own (out/spaces-data)");
}

/** Adds the tag `name` to the open page through the properties panel (the built-in Tags property). */
async function addTag(name) {
  const chip = await waitFor('.property-row[aria-label="Tags"] button.property-chip', "the Tags row's Add tags button", 20000, null, true);
  await mouse(chip.cx, chip.cy);
  await waitFor('input[aria-label="Filter or create an option"]', "the option filter field");
  await clickOn('input[aria-label="Filter or create an option"]', "the option filter field");
  await send("Input.insertText", { text: name });
  await wait(300);
  const option = await page.evaluate(`(() => {
    const el = [...document.querySelectorAll('[role="option"]')].find((e) => e.textContent.includes("Create"));
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })()`);
  must(option !== null, `the picker offers to create "${name}"`);
  await mouse(option.cx, option.cy);
  await page.key("Escape", "Escape", 27);
  await waitUntil(`[...document.querySelectorAll('.property-row[aria-label="Tags"] .option-chip')].some((c) => c.textContent.trim() === ${JSON.stringify(name)})`, `the Tags row shows "${name}"`);
}

async function phaseSeed() {
  const s = readState();
  const A = s.spaceA;
  const png = join(SRC, "garden.png");
  mkdirSync(SRC, { recursive: true });
  writeFileSync(png, pngBytes(40));

  // Garden: a line of text, an image (the native Open dialog), a tag.
  await newPage(GARDEN);
  await writeLine(GARDEN_TEXT);
  await newLine();
  await slash("image", "Image");
  await fileDialog("Open", png);
  const width = await imageLoaded();
  info(`the image block shows the picture, ${width} px wide`);
  await page.evaluate(`document.activeElement?.blur?.()`);
  await addTag(TAG);
  await wait(1500);
  const garden = livePages(await treeOf(A.id)).find((p) => p.title === GARDEN);
  must(garden !== undefined, "the core has Garden");

  // Tomatoes: a child of Garden, with a line of text.
  const tomatoesId = await newChild(A.id, garden.id, TOMATOES);
  await openPageId(A.id, tomatoesId);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(TOMATOES)}`, "Tomatoes is open");
  await writeLine(TOMATOES_TEXT);

  // Plans: a line of text and a chip to Garden (the [[ picker).
  await newPage(PLANS);
  await newLine();
  await send("Input.insertText", { text: PLANS_TEXT });
  await page.type("[[Garden");
  await wait(600);
  await waitFor("[role=listbox]", "the page picker for [[");
  const choices = await page.evaluate(`[...document.querySelectorAll("[role=option]")].map((o) => (o.querySelector(".suggestion-title") ?? o).textContent.trim())`);
  check(choices.some((c) => c.includes(GARDEN)), `the [[ picker offers Garden (${choices.join(", ")})`);
  await page.key("Enter", "Enter", 13);
  await wait(500);
  await page.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  const plans = livePages(await treeOf(A.id)).find((p) => p.title === PLANS);
  must(plans !== undefined, "the core has Plans");
  const chipId = await page.evaluate(`document.querySelector(".ProseMirror [data-page-ref]")?.getAttribute("data-page-id") ?? null`);
  check(chipId === garden.id, "Plans holds a chip that points at Garden");

  // A second space, then back to the first one.
  await createSpaceUi(WORK);
  const spaces = await spaceList();
  const B = spaces.find((sp) => sp.name === WORK);
  must(B !== undefined && spaces.length === 2, `the second space "${WORK}" exists (${spaces.length} spaces)`);
  check((await treeOf(B.id)).roots.length === 0, "Work starts empty");
  await switchSpaceUi(A.id);

  // Garden is a favourite (the star in the page header).
  await openPageId(A.id, garden.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN)}`, "Garden is open");
  await clickOn('button[aria-label="Add to favourites"]', "the star of the page header");
  await waitUntil(`document.querySelectorAll(".favorites-list .favorite-open").length === 1`, "Favourites lists Garden", 15000);

  // The state of the core.
  const tree = await treeOf(A.id);
  const pages = livePages(tree);
  check(pages.length === 3 && pages.some((p) => p.id === tomatoesId && p.parentId === garden.id), `the first space holds Garden, Tomatoes (a child) and Plans (${pages.length} pages)`);
  check((tree.favorites ?? []).join() === garden.id, "Garden is the only favourite of the first space");
  const attachments = await attachmentsOf(A.id);
  check(attachments.length === 1 && attachments[0].name === "garden.png" && attachments[0].availability === "Complete", `one attachment, garden.png, complete (${attachments.length})`);
  const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId: A.id } })).properties;
  const tags = schema.find((d) => d.name === "Tags");
  const values = (await ask({ type: "PagePropertiesGet", data: { pageId: garden.id } })).values;
  const tagValue = values.find(([id]) => id === tags?.id)?.[1];
  check(tagValue?.kind === "multi_select" && tagValue.value.length === 1, "Garden has one Tags value");
  const backlinks = await ask({ type: "BacklinksGet", data: { pageId: garden.id } });
  check(backlinks.some((b) => b.pageId === plans.id), "Plans is a backlink of Garden");
  const spaceCounts = await spaceList();
  check(spaceCounts.find((sp) => sp.id === A.id)?.pageCount === 3 && spaceCounts.find((sp) => sp.id === B.id)?.pageCount === 0, `space counts: 3 and 0 (${spaceCounts.map((sp) => sp.pageCount).join(", ")})`);
  writeState({ spaceB: { id: B.id, name: B.name }, gardenId: garden.id, tomatoesId, plansId: plans.id, attachmentId: attachments[0].attachmentId, gardenIn: A.id });
  await capture("seeded");
}

// ---------------------------------------------------------------------------------------------------------------
// the AppFlowy import (phases appflowy-no, appflowy-real, appflowy-big)
// ---------------------------------------------------------------------------------------------------------------

const WARNING_CODES = {
  "Files that were skipped": "unsupportedFile",
  "Files that were not valid text (some characters replaced)": "notUtf8",
  "Files that were too large": "tooLarge",
  "Links to something that is not in the import": "unresolvedLink",
  "Files that were not found": "missingFile",
  "Images on the web (kept as links, not downloaded)": "remoteImage",
  "Embedded images (not imported)": "embeddedImage",
  "Properties that were not created (the space has too many)": "propertyLimit",
  "Property values that did not fit": "invalidValue",
  "Pages that were left out (too many)": "truncated",
  "Content that could not be imported": "unsupportedContent",
  "Files that could not be added as attachments": "attachmentFailed",
};
/** Details that are words of the app (a kind of content, never a name of the person's). */
const GENERIC_DETAIL = /^(icon|cover|calendar|linked view|database|sort|filter|sorts and filters|sort or filter|[0-9]+)$/;

/** The report of the import dialog as counts: the facts, the warning groups (code and count, hosts of failed downloads), the new spaces. */
const readImportReport = () =>
  page.evaluate(`(() => {
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
      details: [...g.querySelectorAll(".import-warning-detail")].map((d) => d.textContent.trim()),
      items: g.querySelectorAll("li").length,
    }));
    const spaces = [...document.querySelectorAll(".import-spaces .import-space")].map((li) => ({ pagesText: li.querySelector(".muted")?.textContent.trim() ?? "", hasOpen: !!li.querySelector("button") }));
    const text = document.querySelector('[role="dialog"]')?.innerText ?? "";
    return { facts, groups, spaces, noWarnings: text.includes("Nothing was left out or changed."), more: (text.match(/(\\d+) more warnings are not listed/) ?? [])[1] ?? null };
  })()`);

/** The counts of a warning group: the code, how many, the hosts named (for a failed download), generic details only. */
function summariseGroups(groups) {
  return groups.map((g) => {
    const m = /^(.*) \((\d+)\)$/.exec(g.label);
    const label = m === null ? g.label : m[1];
    const code = WARNING_CODES[label] ?? `(unknown label)`;
    const entry = { code, count: m === null ? g.items : Number(m[2]) };
    if (code === "attachmentFailed") entry.hosts = [...new Set(g.details)];
    if (code === "unsupportedContent") {
      entry.generic = {};
      let other = 0;
      for (const d of g.details) {
        if (GENERIC_DETAIL.test(d)) entry.generic[d] = (entry.generic[d] ?? 0) + 1;
        else other += 1;
      }
      entry.otherDetails = other;
    }
    return entry;
  });
}

/** What the shell logged since `mark` about the import, as counts. */
function importLogFacts(text) {
  const lines = text.split(/\r?\n/);
  const find = (re) => lines.find((l) => re.test(l));
  const num = (line, key) => {
    const m = line === undefined ? null : new RegExp(`${key}=(\\d+)`).exec(line);
    return m === null ? null : Number(m[1]);
  };
  const needed = find(/needed files uploaded/);
  const fetched = find(/remote files fetched/);
  const finished = find(/import finished/);
  return {
    leftAsLinks: lines.filter((l) => /remote files left as links: not agreed to/.test(l)).length,
    couldNotFetch: lines.filter((l) => /a remote file of the import could not be fetched/.test(l)).length,
    fetched: fetched === undefined ? null : { remote: num(fetched, "remote"), uploaded: num(fetched, "uploaded"), failed: num(fetched, "failed") },
    needed: needed === undefined ? null : { needed: num(needed, "needed"), uploaded: num(needed, "uploaded"), failed: num(needed, "failed") },
    finished: finished === undefined ? null : { pages: num(finished, "pages"), attachments: num(finished, "attachments"), properties: num(finished, "properties"), spaces: num(finished, "spaces"), warnings: num(finished, "warnings") },
    errorLines: lines.filter((l) => /(^|\s)ERROR(\s|$)/.test(l)).length,
    warnLines: lines.filter((l) => /(^|\s)WARN(\s|$)/.test(l)).length,
  };
}

/**
 * Space menu -> "Import AppFlowy workspace…" -> "Choose backup .zip…" -> the Open dialog with `zipPath` -> the shell's "Download
 * images?" (answered `answer`, when `consent` is expected) -> the report. Returns timings, the report as counts, the new spaces.
 */
async function importAppflowy({ zipPath, answer, label, consent, capturePrefix }) {
  const mark = logMark();
  const spacesBefore = await spaceList();
  const idsBefore = spacesBefore.map((sp) => sp.id);
  await spaceMenu("Import AppFlowy workspace…");
  await waitFor('[role="dialog"] .subtitle', "the AppFlowy import dialog");
  const intro = await page.evaluate(`({ title: document.querySelector('[role="dialog"] .subtitle')?.textContent.trim(), buttons: [...document.querySelectorAll('[role="dialog"] button')].map((b) => b.textContent.trim()) })`);
  check(intro.title === "Import an AppFlowy workspace", `${label}: the dialog is titled "Import an AppFlowy workspace" (${intro.title})`);
  check(intro.buttons.includes("Choose backup .zip…") && intro.buttons.includes("Cancel"), `${label}: it offers "Choose backup .zip…" and Cancel (${intro.buttons.join(", ")})`);
  const introText = await page.evaluate(`document.querySelector('[role="dialog"]').innerText`);
  check(/Settings > Manage data > Back up your workspace/.test(introText), `${label}: it says where AppFlowy makes the backup`);
  if (capturePrefix) await capture(`${capturePrefix}-dialog`);
  const t0 = Date.now();
  await clickOn('[role="dialog"] button', "Choose backup .zip…", { text: "Choose backup .zip…" });
  const open = await fileDialog("Open", zipPath);
  const tOpened = Date.now();
  let consentFacts = null;
  let tConsent = null;
  let tAnswered = null;
  if (consent) {
    const c = await confirmation("Download images?", answer, { captureAs: capturePrefix ? `${capturePrefix}-consent` : null, timeoutMs: 600000 });
    tConsent = Date.now() - c.waitedMs; // the moment the dialog was found
    tAnswered = Date.now();
    consentFacts = { ok: c.ok, said: c.said, buttons: c.buttons, class: c.class, waitedMs: c.waitedMs, method: c.method, error: c.error ?? null };
    check(c.ok === true, `${label}: the shell's "Download images?" dialog appeared and "${answer}" closed it${c.error ? ` (${c.error}; seen ${JSON.stringify(c.seen)})` : ""}`);
    check(c.buttons.includes("Yes") && c.buttons.includes("No"), `${label}: the dialog has Yes and No (${JSON.stringify(c.buttons)})`);
  } else {
    const none = await helper("message-dialog", { title: "Download images?", press: "No", timeoutMs: 1500, noPress: true });
    check(none.ok === false, `${label}: no "Download images?" dialog appeared (the archive has no remote images)`);
  }
  // The report (or an error back at the first step).
  const end = Date.now() + 1200000;
  let outcome = null;
  while (Date.now() < end && outcome === null) {
    outcome = await page.evaluate(`(() => {
      const heading = document.querySelector('[role="dialog"] .subtitle')?.textContent.trim() ?? "";
      if (heading === "Import finished") return "report";
      const error = document.querySelector('[role="dialog"] .error-text')?.textContent.trim();
      if (heading !== "Importing" && heading !== "" && error) return "error:" + error;
      return null;
    })()`);
    if (outcome === null) await wait(400);
  }
  const tReport = Date.now();
  const timings = { openDialogMs: open.waitedMs ?? null, chooseToDialogClosedMs: tOpened - t0, readAndPlanMs: tConsent === null ? null : tConsent - tOpened, consentMs: consentFacts === null ? null : consentFacts.waitedMs, afterConsentMs: tAnswered === null ? null : tReport - tAnswered, totalMs: tReport - t0, totalSeconds: round((tReport - t0) / 1000) };
  must(outcome === "report", `${label}: the import ended with its report (${outcome === null ? "timed out" : outcome})`);
  const report = await readImportReport();
  const groups = summariseGroups(report.groups);
  if (capturePrefix) await capture(`${capturePrefix}-report`);
  const spacesAfter = await spaceList();
  const newSpaces = spacesAfter.filter((sp) => !idsBefore.includes(sp.id));
  const logFacts = importLogFacts(logSince(mark));
  return { report, groups, timings, newSpaces, spacesAfter, logFacts, consentFacts, mark };
}

const pagesOfReport = (report) => Number(report.facts["Pages"]);

/** Closes the report with the first "Open <space>" button when there is one (the page opens in that space); else with Close. */
async function closeReport({ open }) {
  if (open) {
    const button = await box('[role="dialog"] .import-space button');
    must(button !== null, "the report offers Open for a new space");
    await mouse(button.cx, button.cy);
  } else {
    await clickOn('[role="dialog"] button', "Close", { text: "Close" });
  }
  await waitUntil(`!document.querySelector('[role="dialog"]')`, "the report closed");
  await wait(800);
}

async function phaseAppflowyNo() {
  makeSyntheticBackup(SYNTH_ZIP);
  info(`a synthetic backup of ${statSync(SYNTH_ZIP).size} bytes was written to out/spaces-src (4 files)`);
  const run = await importAppflowy({ zipPath: SYNTH_ZIP, answer: "No", label: "appflowy-no", consent: true, capturePrefix: "appflowy-no" });
  const { report, groups, newSpaces, logFacts, consentFacts, timings } = run;
  info(`import took ${timings.totalSeconds} s: ${JSON.stringify(report.facts)}; groups ${JSON.stringify(groups)}`);
  check(consentFacts.said.includes("This import has 1 image stored on example.invalid.") && /Download it now\?/.test(consentFacts.said) && /nothing else is sent/.test(consentFacts.said), `appflowy-no: the dialog names the image count and the host (${consentFacts.said})`);
  check(pagesOfReport(report) === 2, `appflowy-no: the report says 2 pages (${report.facts["Pages"]})`);
  check(Number(report.facts["Attachments"]) === 0, `appflowy-no: 0 attachments (${report.facts["Attachments"]})`);
  check(report.spaces.length === 1 && newSpaces.length === 1, `appflowy-no: the report lists 1 new space (${report.spaces.length}; the core has ${newSpaces.length} more than before)`);
  check(newSpaces[0]?.name === SYNTH_SPACE && newSpaces[0]?.pageCount === 2, `appflowy-no: the new space is "${SYNTH_SPACE}" with 2 pages (${newSpaces[0]?.name}, ${newSpaces[0]?.pageCount})`);
  const remote = groups.find((g) => g.code === "remoteImage");
  check(remote?.count === 1, `appflowy-no: one "Images on the web (kept as links)" warning (${JSON.stringify(groups)})`);
  check(!groups.some((g) => g.code === "attachmentFailed"), "appflowy-no: no failed download is reported");
  check(logFacts.leftAsLinks === 1 && logFacts.fetched === null && logFacts.couldNotFetch === 0, `appflowy-no: the shell log says the images were left as links (not agreed to) and nothing was fetched (${JSON.stringify({ leftAsLinks: logFacts.leftAsLinks, fetched: logFacts.fetched, couldNotFetch: logFacts.couldNotFetch })})`);
  check(logFacts.finished?.pages === 2 && logFacts.finished?.spaces === 1, `appflowy-no: "import finished" logged with counts only (${JSON.stringify(logFacts.finished)})`);
  // Open the new space from the report.
  await closeReport({ open: true });
  await waitUntil(`document.querySelector(".space-switcher .space-name")?.textContent.trim() === ${JSON.stringify(SYNTH_SPACE)}`, "the sidebar switched to the new space", 20000);
  const tree = await treeOf(newSpaces[0].id);
  const pages = livePages(tree);
  check(pages.length === 2 && pages.map((p) => p.title).sort().join("|") === [SYNTH_ONE, SYNTH_TWO].sort().join("|") && pages.every((p) => p.depth === 0), `appflowy-no: the new space's pages are the two documents at the top level (${pages.map((p) => p.title).join(", ")})`);
  const one = pages.find((p) => p.title === SYNTH_ONE);
  const content = await pageContent(one.id);
  const facts = contentFacts(content);
  check(facts.images === 0, `appflowy-no: the page has no image block (${facts.images})`);
  check(facts.hrefs.includes(SYNTH_IMAGE), `appflowy-no: the picture's address is a link in the text (${facts.hrefs.length} links)`);
  await openPageId(newSpaces[0].id, one.id);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(SYNTH_ONE)}`, "the imported page is open");
  const view = await page.evaluate(`(() => { const r = document.querySelector(".ProseMirror"); return { text: r.innerText, imgs: r.querySelectorAll("img").length, h1: [...r.querySelectorAll("h1, h2")].map((h) => h.textContent.trim()), bold: [...r.querySelectorAll("strong")].map((s) => s.textContent.trim()), links: [...r.querySelectorAll("a")].map((a) => a.getAttribute("href")) }; })()`);
  check(view.imgs === 0 && view.text.includes("first paragraph") && view.bold.includes("bold words") && view.h1.includes("A synthetic heading"), `appflowy-no: the page shows the heading, the paragraphs and bold text, and no <img> (${view.imgs} images)`);
  check(view.links.includes(SYNTH_IMAGE), "appflowy-no: the link to the picture shows in the page");
  const atts = await attachmentsOf(newSpaces[0].id);
  check(atts.length === 0, `appflowy-no: the new space holds no attachment (${atts.length})`);
  await capture("appflowy-no-page");
  writeState({ synthSpaceId: newSpaces[0].id });
  result.details = { facts: report.facts, groups, timings, log: logFacts, spaces: newSpaces.length, pages: pages.length };
  result.timings = timings;
}

/** Files that look like a hash (base64url of 32 bytes) rather than a name. */
const HASH_NAME = /^[A-Za-z0-9_-]{43}(\.[A-Za-z0-9]{1,8})?$/;

/** The counts of everything an import made in `spaceIds`, read from the core. Never a title, a name or a text. */
async function importedCounts(spaceIds) {
  const out = { spaces: [], pages: 0, roots: 0, maxDepth: 0, attachments: { total: 0, hashNamed: 0, withExtension: 0, complete: 0, referenced: 0, unreferenced: 0, bySpace: [] }, content: { pagesWithContent: 0, images: 0, imagesWithAttachment: 0, imageLinks: 0, files: 0, boards: 0, tables: 0, pageRefs: 0, pageLinks: 0 }, boards: [], properties: [], pageIds: [] };
  const attachmentIds = new Set();
  const usedAttachmentIds = new Set();
  for (const spaceId of spaceIds) {
    const tree = await treeOf(spaceId);
    const pages = livePages(tree);
    out.spaces.push({ pages: pages.length, roots: tree.roots.length, maxDepth: pages.reduce((m, p) => Math.max(m, p.depth), 0), favorites: (tree.favorites ?? []).length });
    out.pages += pages.length;
    out.roots += tree.roots.length;
    out.maxDepth = Math.max(out.maxDepth, ...pages.map((p) => p.depth));
    const attachments = await attachmentsOf(spaceId);
    out.attachments.bySpace.push(attachments.length);
    for (const a of attachments) {
      attachmentIds.add(a.attachmentId);
      out.attachments.total += 1;
      if (HASH_NAME.test(a.name)) out.attachments.hashNamed += 1;
      if (/\.[A-Za-z0-9]{1,8}$/.test(a.name)) out.attachments.withExtension += 1;
      if (a.availability === "Complete") out.attachments.complete += 1;
      if (a.references > 0) out.attachments.referenced += 1;
      else out.attachments.unreferenced += 1;
    }
    const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId } })).properties;
    out.properties.push(schema.filter((d) => d.name !== "Tags" && !["created_time", "last_edited_time", "created_by", "last_edited_by"].includes(d.kind)).length);
    for (const p of pages) {
      out.pageIds.push({ id: p.id, spaceId, children: p.children, depth: p.depth, parentId: p.parentId });
      const facts = contentFacts(await pageContent(p.id));
      if (facts.blocks > 0) out.content.pagesWithContent += 1;
      out.content.images += facts.images;
      out.content.imagesWithAttachment += facts.imagesWithAttachment;
      out.content.imageLinks += facts.hrefs.filter((h) => /^https?:\/\//.test(h)).length;
      out.content.files += facts.files;
      out.content.boards += facts.boards;
      out.content.tables += facts.tables;
      out.content.pageRefs += facts.pageRefs;
      out.content.pageLinks += facts.pageLinks;
      for (const id of facts.attachmentIds) usedAttachmentIds.add(id);
      if (facts.boards > 0) out.boards.push({ id: p.id, spaceId, children: p.children });
    }
  }
  out.attachments.usedByBlocks = usedAttachmentIds.size;
  out.attachments.distinct = attachmentIds.size;
  return out;
}

async function phaseAppflowyReal() {
  must(REAL_ZIP !== "" && existsSync(REAL_ZIP), "a real backup was given with -RealZip and exists");
  const run = await importAppflowy({ zipPath: REAL_ZIP, answer: "Yes", label: "appflowy-real", consent: true, capturePrefix: "appflowy-real" });
  const { report, groups, newSpaces, logFacts, consentFacts, timings } = run;
  info(`import took ${timings.totalSeconds} s (dialog and download included): ${JSON.stringify(report.facts)}; groups ${JSON.stringify(groups)}; timings ${JSON.stringify(timings)}`);
  info(`the "Download images?" dialog said: ${consentFacts.said.replace(/\s+/g, " ")}`);
  check(/^This import has 28 images stored on [\w.-]+\./.test(consentFacts.said.replace(/^.*?This import/, "This import")) , "appflowy-real: the dialog says 28 images stored on one host");
  const failed = groups.find((g) => g.code === "attachmentFailed");
  const pages = pagesOfReport(report);
  check(newSpaces.length === 2 && report.spaces.length === 2, `appflowy-real: 2 new spaces (${newSpaces.length}; the report lists ${report.spaces.length})`);
  check(pages >= 110 && pages <= 125, `appflowy-real: about 118 pages created (${pages})`);
  check(pages === newSpaces.reduce((n, sp) => n + sp.pageCount, 0), `appflowy-real: the report's page count equals the new spaces' page counts (${pages} vs ${newSpaces.map((sp) => sp.pageCount).join("+")})`);
  info(`the report: ${JSON.stringify(report.facts)}; more warnings not listed: ${report.more}`);
  // Downloads: the shell log is the exact record (uploaded / failed).
  info(`shell log: ${JSON.stringify(logFacts)}`);
  check(logFacts.needed?.needed === 18 && logFacts.needed?.uploaded === 18 && logFacts.needed?.failed === 0, `appflowy-real: the 18 files of the archive were uploaded (${JSON.stringify(logFacts.needed)})`);
  check(logFacts.fetched?.remote === 28, `appflowy-real: the shell tried 28 remote files (${JSON.stringify(logFacts.fetched)})`);
  const downloaded = logFacts.fetched?.uploaded ?? null;
  const failedCount = failed?.count ?? 0;
  result.downloadFinding = { downloaded, failed: logFacts.fetched?.failed ?? null, reportFailedWarnings: failedCount, hosts: failed?.hosts ?? [] };
  if (process.env.SPACES_DOWNLOADS_BLOCKED === "1") {
    // This machine refuses outbound connections to Rust-built executables (os error 10013): the finding is the product's
    // behaviour when every download fails, not the download.
    check(downloaded === 0 && failedCount === 28 && logFacts.couldNotFetch === 28 && (failed?.hosts ?? []).length === 1, `appflowy-real (downloads blocked by the machine): 28 failures, each reported as AttachmentFailed naming the host (${downloaded} downloaded, ${failedCount} warnings, hosts ${JSON.stringify(failed?.hosts ?? [])})`);
    check(groups.find((g) => g.code === "remoteImage")?.count === 28, "appflowy-real (downloads blocked): the 28 images stay links (RemoteImage x28)");
  } else {
    check(downloaded === 28 && failedCount === 0, `appflowy-real: 28 images downloaded (${downloaded} downloaded, ${logFacts.fetched?.failed ?? "?"} failed, ${failedCount} AttachmentFailed warnings naming ${JSON.stringify(failed?.hosts ?? [])})`);
  }
  if (failedCount > 0 && process.env.SPACES_DOWNLOADS_BLOCKED !== "1") bug(`appflowy-real: ${failedCount} of 28 image downloads failed (AttachmentFailed, host ${JSON.stringify(failed?.hosts ?? [])}); the shell log carries the code only`);
  const expectAttachments = 18 + (downloaded ?? 0);
  check(Number(report.facts["Attachments"]) === expectAttachments, `appflowy-real: the report counts ${expectAttachments} attachments (${report.facts["Attachments"]})`);
  check(Number(report.facts["Properties created"]) >= 1, `appflowy-real: properties were created (${report.facts["Properties created"]})`);
  check(logFacts.leftAsLinks === 0, "appflowy-real: nothing was left as a link by a refusal");
  await closeReport({ open: true });

  // What the core holds, as counts.
  const counts = await importedCounts(newSpaces.map((sp) => sp.id));
  info(`counts from the core: ${JSON.stringify({ ...counts, pageIds: counts.pageIds.length })}`);
  check(counts.pages === pages, `appflowy-real: the trees hold ${counts.pages} pages, as reported`);
  check(counts.attachments.total === expectAttachments, `appflowy-real: the new spaces hold ${counts.attachments.total} attachments (18 files + ${downloaded ?? 0} images; by space ${JSON.stringify(counts.attachments.bySpace)})`);
  check(counts.attachments.hashNamed === 0, `appflowy-real: no attachment carries a hash as its name (${counts.attachments.hashNamed} do; ${counts.attachments.withExtension} have an extension)`);
  check(counts.attachments.complete === counts.attachments.total, `appflowy-real: every attachment is complete (${counts.attachments.complete} of ${counts.attachments.total})`);
  check(counts.attachments.unreferenced === 0, `appflowy-real: every attachment is referenced by a page (${counts.attachments.unreferenced} are not)`);
  check(counts.content.images === counts.content.imagesWithAttachment + 0 || counts.content.images >= counts.content.imagesWithAttachment, `appflowy-real: image blocks ${counts.content.images}, with an attachment ${counts.content.imagesWithAttachment}, file blocks ${counts.content.files}`);
  check(counts.boards.length >= 1, `appflowy-real: a board view exists (${counts.boards.length}: children ${JSON.stringify(counts.boards.map((b) => b.children))})`);
  const board = counts.boards.find((b) => b.children === 38);
  check(board !== undefined, "appflowy-real: a board page has 38 rows as children");
  if (board !== undefined) {
    await switchSpaceUi(board.spaceId);
    await openPageId(board.spaceId, board.id);
    await waitFor('.ProseMirror [data-type="pageList"][data-layout="board"]', "the board view of the page", 60000);
    await wait(1500);
    const cards = await page.evaluate(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="board"] .page-board-card').length`);
    const columns = await page.evaluate(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="board"] .page-board-column').length`);
    check(cards === 38, `appflowy-real: the board view shows 38 rows (${cards} cards in ${columns} columns)`);
    await capture("appflowy-real-board");
    result.board = { cards, columns };
  }
  // An image that came from the server shows in the page.
  const withImage = [];
  for (const entry of counts.pageIds) {
    if (withImage.length > 0) break;
    const facts = contentFacts(await pageContent(entry.id));
    if (facts.imagesWithAttachment > 0) withImage.push(entry);
  }
  if (withImage.length > 0 && downloaded) {
    await switchSpaceUi(withImage[0].spaceId);
    await openPageId(withImage[0].spaceId, withImage[0].id);
    const width = await imageLoaded(60000).catch(() => 0);
    check(width > 0, `appflowy-real: an imported image loads in its page (${width} px wide)`);
    await capture("appflowy-real-image");
  }
  writeState({ realSpaceIds: newSpaces.map((sp) => sp.id), realCounts: { pages: counts.pages, attachments: counts.attachments.total } });
  result.details = { facts: report.facts, groups, timings, log: logFacts, spaces: newSpaces.length, counts: { ...counts, pageIds: undefined, boards: counts.boards.map((b) => b.children) } };
  result.timings = timings;
}

async function phaseAppflowyBig() {
  must(BIG_ZIP !== "" && existsSync(BIG_ZIP), "a second backup was given with -BigZip and exists");
  const run = await importAppflowy({ zipPath: BIG_ZIP, answer: "No", label: "appflowy-big", consent: true, capturePrefix: "appflowy-big" }).catch((error) => {
    if (error instanceof PhaseStop) throw error;
    throw error;
  });
  const { report, groups, newSpaces, logFacts, consentFacts, timings } = run;
  info(`import took ${timings.totalSeconds} s: ${JSON.stringify(report.facts)}; groups ${JSON.stringify(groups)}; timings ${JSON.stringify(timings)}`);
  info(`the "Download images?" dialog said: ${consentFacts.said.replace(/\s+/g, " ")}`);
  info(`shell log: ${JSON.stringify(logFacts)}`);
  check(logFacts.fetched === null && logFacts.couldNotFetch === 0, "appflowy-big: nothing was fetched (No)");
  check(logFacts.leftAsLinks === 1, `appflowy-big: the shell log says the images were left as links (${logFacts.leftAsLinks})`);
  await closeReport({ open: true });
  const counts = await importedCounts(newSpaces.map((sp) => sp.id));
  info(`counts from the core: ${JSON.stringify({ ...counts, pageIds: counts.pageIds.length })}`);
  check(newSpaces.length >= 1, `appflowy-big: new spaces were made (${newSpaces.length}; the report lists ${report.spaces.length})`);
  check(counts.pages === pagesOfReport(report), `appflowy-big: the trees hold ${counts.pages} pages, as reported (${pagesOfReport(report)})`);
  check(counts.attachments.hashNamed === 0, `appflowy-big: no attachment carries a hash as its name (${counts.attachments.hashNamed})`);
  check(counts.attachments.unreferenced === 0, `appflowy-big: every attachment is referenced (${counts.attachments.unreferenced} are not)`);
  check(Number(report.facts["Attachments"]) === counts.attachments.total, `appflowy-big: the report's attachments equal the spaces' (${report.facts["Attachments"]} vs ${counts.attachments.total})`);
  const failed = groups.filter((g) => g.code === "attachmentFailed" || g.code === "missingFile" || g.code === "truncated" || g.code === "propertyLimit" || g.code === "invalidValue");
  info(`warning groups of the kinds that mean something was lost: ${JSON.stringify(failed)}`);
  writeState({ bigSpaceIds: newSpaces.map((sp) => sp.id) });
  result.details = { facts: report.facts, groups, timings, log: logFacts, spaces: newSpaces.length, counts: { ...counts, pageIds: undefined, boards: counts.boards.map((b) => b.children) } };
  result.timings = timings;
}

// ---------------------------------------------------------------------------------------------------------------
// phases: move-imported, move-board, move-files (pages of the real import moved to the first space)
// ---------------------------------------------------------------------------------------------------------------

/** What a board view of the open page shows, in memory: the columns' header texts and the number of cards in each. */
const readBoard = () =>
  page.evaluate(`(() => {
    const board = document.querySelector('.ProseMirror [data-type="pageList"][data-layout="board"]');
    if (!board) return null;
    const columns = [...board.querySelectorAll(".page-board-column")].map((c) => ({ header: c.querySelector(".page-board-header")?.textContent.trim() ?? "", cards: c.querySelectorAll(".page-board-card").length }));
    return { columns, cards: board.querySelectorAll(".page-board-card").length };
  })()`);

/**
 * Moves `root` (a page of `fromSpaceId`, found by the caller) to the top level of the first space through the sidebar row's
 * "Move to…" and checks the plan's lines, the result and what moved with it. `extra` may add checks before (`before`) and after.
 */
async function moveSubtree({ label, fromSpaceId, root, extra = null }) {
  const s = readState();
  const A = s.spaceA;
  const rootNode = livePages(await treeOf(fromSpaceId)).find((p) => p.id === root.id).node;
  const subtree = flatten([rootNode]);
  const subtreeIds = subtree.map((p) => p.id);
  info(`${label}: the page has a subtree of ${subtreeIds.length} pages`);
  const planAnswer = await ask({ type: "PageMoveToSpacePlan", data: { pageId: root.id, spaceId: A.id } });
  info(`${label}: plan ${JSON.stringify({ ...planAnswer, pageIds: planAnswer.pageIds.length })}`);
  check(planAnswer.pageIds.length === subtreeIds.length && planAnswer.pageIds[0] === root.id, `${label}: the plan lists the subtree in pre-order (${planAnswer.pageIds.length} pages)`);
  check(sameSet(planAnswer.pageIds, subtreeIds), `${label}: the plan's pages are exactly the page's subtree`);

  const attFromBefore = await attachmentsOf(fromSpaceId);
  const attToBefore = await attachmentsOf(A.id);
  const schemaBefore = (await ask({ type: "PropertySchemaGet", data: { spaceId: A.id } })).properties;
  const countsBefore = (await spaceList()).map((sp) => ({ id: sp.id, count: sp.pageCount }));
  const favBefore = (await treeOf(A.id)).favorites ?? [];
  let imagePage = null;
  const expectedAttachmentIds = new Set();
  for (const p of subtree) {
    const facts = contentFacts(await pageContent(p.id));
    for (const id of facts.attachmentIds) expectedAttachmentIds.add(id);
    if (imagePage === null && facts.imagesWithAttachment > 0) imagePage = p.id;
  }
  info(`${label}: the subtree uses ${expectedAttachmentIds.size} attachments; ${imagePage === null ? "no page of it holds an image block" : "a page of it holds an image block"}`);
  const beforeExtra = extra === null ? null : await extra.before({ subtree, planAnswer });

  await switchSpaceUi(fromSpaceId);
  await openPageId(fromSpaceId, root.id);
  await waitFor(".content-body h1", "the page is open");
  const labels = await rowMenu(root.id, "Move to…");
  const at = labels.indexOf("Move to…");
  check(at > 0 && labels[at - 1] === "Outdent" && labels[at + 1] === "Rename", `${label}: the row menu has Move to… between Outdent and Rename`);
  const sourceName = (await spaceList()).find((sp) => sp.id === fromSpaceId)?.name ?? "";
  const t0 = Date.now();
  const dialog = await driveMoveDialog({ title: root.title, targetName: A.name, parentLabel: null, currentSpaceName: null, capturePrefix: label });
  const redact = (text) => (sourceName === "" ? text : text.split(sourceName).join("<source space>")).split(root.title).join("<page>");
  const expectedSummary = planAnswer.pageIds.length === 1 ? `Move 1 page to “${A.name}”` : `Move ${planAnswer.pageIds.length} pages to “${A.name}”`;
  check(dialog.summary === expectedSummary, `${label}: the confirmation says ${expectedSummary}`);
  check(dialog.where.some((w) => w === "It goes at the top level."), `${label}: it says the page goes at the top level`);
  const expectedNotes = [planAnswer.propertiesCreated, planAnswer.valuesDropped, planAnswer.relationLinksRemoved, planAnswer.attachmentsLeft].filter((n) => n > 0).length;
  check(dialog.notes.length === expectedNotes, `${label}: ${expectedNotes} note line(s) for the non-zero counts (the dialog shows ${dialog.notes.length}): ${dialog.notes.map(redact).map((n) => n.replace(/\d+/g, "N")).join(" || ")}`);
  const noteKinds = {
    properties: dialog.notes.some((n) => /propert(y is|ies are) added/.test(n)),
    values: dialog.notes.some((n) => /cannot follow the pages/.test(n)),
    relations: dialog.notes.some((n) => /relation links? (is|are) removed/.test(n)),
    attachmentsStay: dialog.notes.some((n) => /stays? in/.test(n)),
  };
  check(noteKinds.properties === planAnswer.propertiesCreated > 0 && noteKinds.values === planAnswer.valuesDropped > 0 && noteKinds.relations === planAnswer.relationLinksRemoved > 0 && noteKinds.attachmentsStay === planAnswer.attachmentsLeft > 0, `${label}: each line matches a non-zero count (${JSON.stringify(noteKinds)})`);
  if (planAnswer.propertiesCreated > 0) {
    const line = dialog.notes.find((n) => /propert(y is|ies are) added/.test(n)) ?? "";
    check(line.includes(`“${A.name}”`) && new RegExp(planAnswer.propertiesCreated === 1 ? "^1 property is added" : `^${planAnswer.propertiesCreated} properties are added`).test(line), `${label}: the properties line names the target space and the count (${line.replace(/\d+/, "N")})`);
  }
  check(dialog.error === null, `${label}: no error in the dialog (${dialog.error})`);
  info(`${label}: Move pressed to dialog closed in ${dialog.moveMs} ms (the whole flow ${Date.now() - t0} ms)`);

  // The result.
  const afterFrom = await treeOf(fromSpaceId);
  const afterTo = await treeOf(A.id);
  const toPages = livePages(afterTo);
  const stillThere = livePages(afterFrom).filter((p) => subtreeIds.includes(p.id)).length;
  check(stillThere === 0, `${label}: none of the ${subtreeIds.length} pages is left in the source (${stillThere})`);
  const arrived = toPages.filter((p) => subtreeIds.includes(p.id));
  check(arrived.length === subtreeIds.length, `${label}: all ${subtreeIds.length} pages are in the first space (${arrived.length})`);
  const top = toPages.find((p) => p.id === root.id);
  check(top !== undefined && top.parentId === null, `${label}: the page is at the top level of the first space`);
  const parentsOk = subtree.every((p) => p.id === root.id || arrived.find((q) => q.id === p.id)?.parentId === p.parentId);
  check(parentsOk, `${label}: every moved page kept its parent`);
  const targetOrder = flatten(afterTo.roots.filter((n) => n.page.id === root.id)).map((p) => p.id);
  check(subtreeIds.join() === targetOrder.join(), `${label}: the subtree kept its order (pre-order equal)`);
  const countsAfter = (await spaceList()).map((sp) => ({ id: sp.id, count: sp.pageCount }));
  const deltaOf = (id) => (countsAfter.find((c) => c.id === id)?.count ?? 0) - (countsBefore.find((c) => c.id === id)?.count ?? 0);
  check(deltaOf(A.id) === subtreeIds.length && deltaOf(fromSpaceId) === -subtreeIds.length, `${label}: page counts moved by ${subtreeIds.length} (${deltaOf(fromSpaceId)}, +${deltaOf(A.id)})`);
  const attFromAfter = await attachmentsOf(fromSpaceId);
  const attToAfter = await attachmentsOf(A.id);
  check(attToAfter.length - attToBefore.length === planAnswer.attachmentsMoved && attFromBefore.length - attFromAfter.length === planAnswer.attachmentsMoved, `${label}: ${planAnswer.attachmentsMoved} attachment(s) moved with the pages (${attFromBefore.length}->${attFromAfter.length}, ${attToBefore.length}->${attToAfter.length}); ${planAnswer.attachmentsLeft} stay in the source`);
  const stuck = [...expectedAttachmentIds].filter((id) => !attToAfter.some((a) => a.attachmentId === id) && !attFromAfter.some((a) => a.attachmentId === id));
  check(stuck.length === 0, `${label}: every attachment the moved pages use is still listed by a space (${stuck.length} are not)`);
  let incomplete = 0;
  let wrongSpace = 0;
  for (const id of expectedAttachmentIds) {
    const got = await ask({ type: "AttachmentGet", data: { attachmentId: id } });
    if (got.availability !== "Complete") incomplete += 1;
    const moved = attToAfter.some((a) => a.attachmentId === id);
    if (moved && got.spaceId !== A.id) wrongSpace += 1;
  }
  check(incomplete === 0 && wrongSpace === 0, `${label}: every attachment of the moved pages is complete (${incomplete} are not) and the moved ones belong to the first space (${wrongSpace} do not)`);
  const schemaAfter = (await ask({ type: "PropertySchemaGet", data: { spaceId: A.id } })).properties;
  check(schemaAfter.length - schemaBefore.length === planAnswer.propertiesCreated, `${label}: ${planAnswer.propertiesCreated} propert(y|ies) created in the first space (${schemaBefore.length} -> ${schemaAfter.length})`);
  check(JSON.stringify((await treeOf(A.id)).favorites ?? []) === JSON.stringify(favBefore), `${label}: the first space's favourites are unchanged`);
  check((await shownSpace()) === A.name, `${label}: the sidebar switched to the first space (${await shownSpace()})`);
  await waitFor(`[role="treeitem"][data-page-id="${root.id}"]`, "the moved page's row in the sidebar");
  check((await heading()) === root.title, `${label}: the moved page is the open page`);
  const rowsInSidebar = await page.evaluate(`[...document.querySelectorAll('[role="treeitem"]')].map((e) => e.getAttribute("data-page-id"))`);
  check(new Set(rowsInSidebar).size === rowsInSidebar.length, `${label}: the sidebar lists no page twice`);
  const tabs = await tabsInfo();
  check(tabs.filter((t) => t.active).length === 1 && tabs.find((t) => t.active)?.title === root.title, `${label}: the active tab is the moved page (${tabs.length} tabs)`);
  const hits = await ask({ type: "Search", data: { query: root.title, scope: "Titles" } });
  const rootHits = hits.filter((h) => h.pageId === root.id);
  check(rootHits.length > 0 && rootHits.every((h) => h.spaceId === A.id), `${label}: search finds the moved page in the first space only (${rootHits.length} hits)`);
  const afterExtra = extra === null ? null : await extra.after({ subtree, planAnswer, beforeExtra, toSpace: A, attToAfter });
  if (imagePage !== null) {
    await openPageId(A.id, imagePage);
    const width = await imageLoaded(60000).catch(() => 0);
    check(width > 0, `${label}: an image of the moved pages loads from the new space (${width} px)`);
  } else {
    info(`${label}: no page of the moved subtree holds an image block: the image check was skipped`);
  }
  await capture(`${label}-after`);
  result.details = {
    pages: subtreeIds.length,
    plan: { ...planAnswer, pageIds: planAnswer.pageIds.length },
    noteLines: dialog.notes.length,
    summaryOk: dialog.summary === expectedSummary,
    moveMs: dialog.moveMs,
    attachments: { moved: planAnswer.attachmentsMoved, left: planAnswer.attachmentsLeft, usedByPages: expectedAttachmentIds.size },
    imageChecked: imagePage !== null,
    ...(afterExtra === null ? {} : { extra: afterExtra }),
  };
}

async function phaseMoveImported() {
  const s = readState();
  must(Array.isArray(s.realSpaceIds) && s.realSpaceIds.length > 0, "the real import left its spaces in the state");
  const firstNew = s.realSpaceIds[0];
  const treeBefore = await treeOf(firstNew);
  must(treeBefore.roots.length > 0, "the first new space has a root page");
  // The first root page by position, whatever its title.
  await moveSubtree({ label: "move-imported", fromSpaceId: firstNew, root: treeBefore.roots[0].page });
}

/** The board page of the real import (the page with 38 rows under it and a board view), searched in its spaces. */
async function findBoard() {
  const s = readState();
  for (const spaceId of s.realSpaceIds ?? []) {
    for (const p of livePages(await treeOf(spaceId))) {
      if (p.children === 38 && contentFacts(await pageContent(p.id)).boards > 0) return { spaceId, page: p.node.page };
    }
  }
  return null;
}

async function phaseMoveBoard() {
  const board = await findBoard();
  must(board !== null, "the board page (38 rows) is still in the real import's spaces");
  const source = board.spaceId;
  await moveSubtree({
    label: "move-board",
    fromSpaceId: source,
    root: board.page,
    extra: {
      // Before: the board as the source shows it, the select property of the rows and their values (in memory only).
      async before({ subtree, planAnswer }) {
        check(planAnswer.propertiesCreated >= 1, `move-board: the plan says the select property of the rows has to be created in the first space (${planAnswer.propertiesCreated})`);
        const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId: source } })).properties;
        const def = schema.find((d) => d.name !== "Tags" && (d.kind === "select" || d.kind === "multi_select" || d.kind === "status") && (d.options?.length ?? 0) > 0);
        must(def !== undefined, "the source space has the select property of the rows");
        const rows = subtree.filter((p) => p.id !== board.page.id);
        const values = {};
        for (const row of rows) {
          const v = (await ask({ type: "PagePropertiesGet", data: { pageId: row.id } })).values.find(([id]) => id === def.id)?.[1];
          if (v !== undefined) values[row.id] = (Array.isArray(v.value) ? v.value : [v.value]).map((o) => def.options.find((x) => x.id === o)?.name ?? "?").join("|");
        }
        await switchSpaceUi(source);
        await openPageId(source, board.page.id);
        await waitFor('.ProseMirror [data-type="pageList"][data-layout="board"] .page-board-card', "the board's cards in the source");
        await wait(1000);
        const shown = await readBoard();
        return { def: { kind: def.kind, options: def.options.map((o) => ({ name: o.name, color: o.color })) }, values, shown, rows: rows.length };
      },
      async after({ planAnswer, beforeExtra, toSpace }) {
        const schema = (await ask({ type: "PropertySchemaGet", data: { spaceId: toSpace.id } })).properties;
        const made = schema.find((d) => d.kind === beforeExtra.def.kind && d.options?.length === beforeExtra.def.options.length && beforeExtra.def.options.every((o) => d.options.some((x) => x.name === o.name && x.color === o.color)));
        check(made !== undefined, `move-board: the first space got the select property with the same ${beforeExtra.def.options.length} options and colours`);
        let same = 0;
        let differ = 0;
        for (const [rowId, text] of Object.entries(beforeExtra.values)) {
          const v = (await ask({ type: "PagePropertiesGet", data: { pageId: rowId } })).values.find(([id]) => id === made?.id)?.[1];
          const now = v === undefined ? "" : (Array.isArray(v.value) ? v.value : [v.value]).map((o) => made.options.find((x) => x.id === o)?.name ?? "?").join("|");
          if (now === text) same += 1;
          else differ += 1;
        }
        check(differ === 0, `move-board: the rows kept their values in the new property (${same} of ${Object.keys(beforeExtra.values).length} equal, ${differ} differ)`);
        check(planAnswer.valuesDropped === 0, `move-board: no value is dropped (${planAnswer.valuesDropped})`);
        await openPageId(toSpace.id, board.page.id);
        await waitFor('.ProseMirror [data-type="pageList"][data-layout="board"] .page-board-card', "the board's cards in the first space");
        await wait(1000);
        const shown = await readBoard();
        check(shown !== null && shown.cards === 38 && shown.cards === beforeExtra.shown.cards, `move-board: the board still shows its ${beforeExtra.shown.cards} rows (${shown?.cards})`);
        check(JSON.stringify(shown?.columns) === JSON.stringify(beforeExtra.shown.columns), `move-board: the board's columns and their cards are the same as before the move (${shown?.columns.length} columns)`);
        await capture("move-board-board");
        return { propertiesCreated: planAnswer.propertiesCreated, rowsWithValue: Object.keys(beforeExtra.values).length, columns: shown?.columns.length, cards: shown?.cards };
      },
    },
  });
}

/** The first page (pre-order, by position) of the real import's spaces that holds a file block. */
async function findFilePage() {
  const s = readState();
  for (const spaceId of s.realSpaceIds ?? []) {
    for (const p of livePages(await treeOf(spaceId))) {
      const facts = contentFacts(await pageContent(p.id));
      if (facts.files > 0) return { spaceId, page: p.node.page, files: facts.files };
    }
  }
  return null;
}

async function phaseMoveFiles() {
  const found = await findFilePage();
  must(found !== null, "a page with a file block is still in the real import's spaces");
  await moveSubtree({
    label: "move-files",
    fromSpaceId: found.spaceId,
    root: found.page,
    extra: {
      async before({ planAnswer }) {
        info(`move-files: the page holds ${found.files} file block(s); the plan moves ${planAnswer.attachmentsMoved} attachment(s) and leaves ${planAnswer.attachmentsLeft}`);
        return {};
      },
      async after({ toSpace }) {
        // The page shows its file cards with the original names of the attachments.
        const facts = contentFacts(await pageContent(found.page.id));
        const infos = [];
        for (const id of facts.attachmentIds) infos.push(await ask({ type: "AttachmentGet", data: { attachmentId: id } }));
        await openPageId(toSpace.id, found.page.id);
        await wait(1500);
        const shown = await page.evaluate(`document.querySelector(".ProseMirror")?.innerText ?? ""`);
        const named = infos.filter((i) => shown.includes(i.name)).length;
        check(infos.length > 0 && infos.every((i) => i.availability === "Complete"), `move-files: the page's ${infos.length} file(s) are complete`);
        check(named === infos.length, `move-files: the page shows every file under its name (${named} of ${infos.length})`);
        return { files: infos.length, named, inFirstSpace: infos.filter((i) => i.spaceId === toSpace.id).length };
      },
    },
  });
}

// ---------------------------------------------------------------------------------------------------------------
// phase: restart
// ---------------------------------------------------------------------------------------------------------------

/** For every space: its pages (pre-order, with parents), favourites and attachment count, as one hash and counts. */
async function fingerprint() {
  const spaces = await spaceList();
  const out = [];
  const seen = new Map();
  for (const sp of spaces) {
    const tree = await treeOf(sp.id);
    const pages = livePages(tree);
    for (const p of pages) seen.set(p.id, (seen.get(p.id) ?? 0) + 1);
    const hash = createHash("sha1").update(pages.map((p) => `${p.id}:${p.parentId ?? ""}`).join("|")).digest("hex").slice(0, 16);
    out.push({ id: sp.id, pageCount: sp.pageCount, pages: pages.length, hash, favorites: (tree.favorites ?? []).join(","), attachments: (await attachmentsOf(sp.id)).length, trashed: tree.trashed.length });
  }
  return { spaces: out, duplicatePages: [...seen.values()].filter((n) => n > 1).length, total: seen.size };
}

async function phaseRestart() {
  const s = readState();
  must(s.fingerprint !== undefined, "an earlier phase saved a fingerprint of every space");
  await waitFor(".tab-strip", "the tab strip", 60000);
  await wait(4000); // the reconciliation at open settles
  const now = await fingerprint();
  check(now.spaces.length === s.fingerprint.spaces.length, `restart: ${now.spaces.length} spaces after the restart (${s.fingerprint.spaces.length} before)`);
  for (const [index, before] of s.fingerprint.spaces.entries()) {
    const after = now.spaces[index];
    check(after !== undefined && after.id === before.id && after.pages === before.pages && after.hash === before.hash && after.favorites === before.favorites && after.attachments === before.attachments && after.trashed === before.trashed && after.pageCount === before.pageCount, `restart: space ${index + 1} is as it was (${before.pages} pages, ${before.attachments} attachments, ${before.favorites === "" ? 0 : before.favorites.split(",").length} favourites)`);
  }
  check(now.duplicatePages === 0, `restart: no page is live in two spaces (${now.duplicatePages})`);
  // Garden is where the last move left it, and opens with its image.
  const garden = await ask({ type: "PageGet", data: { pageId: s.gardenId } });
  check(garden.spaceId === s.gardenIn, "restart: Garden is in the space the last move left it in");
  const space = s.gardenIn;
  await switchSpaceUi(space);
  await openPageId(space, s.gardenId);
  const width = await imageLoaded();
  check(width === 40, `restart: Garden's image loads after the restart (${width} px)`);
  const hits = await ask({ type: "Search", data: { query: "heirloom" } });
  check(hits.some((h) => h.pageId === s.gardenId && h.spaceId === space), "restart: search still finds Garden in its space");
  const tabs = await tabsInfo();
  info(`tabs after the restart: ${tabs.length}`);
  result.details = { spaces: now.spaces.length, pages: now.total, duplicatePages: now.duplicatePages };
}

// ---------------------------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------------------------

const PHASE_FUNCS = {
  mark: phaseMark,
  launch: phaseLaunch,
  seed: phaseSeed,
  move: phaseMove,
  "move-back": phaseMoveBack,
  "appflowy-no": phaseAppflowyNo,
  "appflowy-real": phaseAppflowyReal,
  "appflowy-big": phaseAppflowyBig,
  "move-imported": phaseMoveImported,
  "move-board": phaseMoveBoard,
  "move-files": phaseMoveFiles,
  restart: phaseRestart,
};
/** The phases that change what the spaces hold: a fingerprint is saved after each, for the restart. */
const CHANGING = new Set(["seed", "move", "move-back", "appflowy-no", "appflowy-real", "appflowy-big", "move-imported", "move-board", "move-files"]);

/** The webview's own document: nothing may have replaced it. */
async function intact(when) {
  const state = readState();
  const now = await page.evaluate(`(() => ({ marker: window.__spacesMarker ?? null, href: location.href }))()`).catch(() => null);
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

if (CHANGING.has(phase)) {
  try {
    await normalize();
    writeState({ fingerprint: await fingerprint() });
  } catch (error) {
    info(`the fingerprint of the spaces could not be saved: ${error instanceof Error ? error.message : String(error)}`);
  }
}

if (phase !== "mark" && phase !== "launch") {
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
// The origins the app itself talks to: its own pages and assets, the IPC bridge, the media scheme. Nothing else: the page never
// downloads (the shell does).
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
// The report file has every note; the console has what failed (or, with SPACES_VERBOSE, everything).
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
      ...(result.downloadFinding ? { downloadFinding: result.downloadFinding } : {}),
      ...(process.env.SPACES_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
page.close();
process.exit(result.ok ? 0 : 1);
