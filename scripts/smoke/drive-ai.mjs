// The in-page half of the local AI and MCP pass (drive-ai.ps1, ADR-0037): trusted mouse and key input, the DOM, the core
// (through the page's own IPC), the native dialogs (Win32 helpers of the .ps1), the real Ollama and the real kb-mcp-bridge,
// over the DevTools port of the app's WebView2 (the calls of cdp.mjs). Phases:
//
//   node drive-ai.mjs mark           <outfile>  the marker of the document is (re)set (after a restart of the binary)
//   node drive-ai.mjs launch         <outfile>  the document's marker; the space is empty
//   node drive-ai.mjs seed           <outfile>  "Budget review" and "Garden", typed through the UI
//   node drive-ai.mjs settings       <outfile>  Settings -> AI: on, Connected, default models, transcription r2t2
//   node drive-ai.mjs consent        <outfile>  a remote address -> the native "Send text to another computer?" -> Cancel
//   node drive-ai.mjs guard          <outfile>  the shell-only requests and settings are refused from the page
//   node drive-ai.mjs index          <outfile>  every page indexed (nomic-embed-text)
//   node drive-ai.mjs writer         <outfile>  selection -> AI -> Improve -> Replace; Ctrl+Z; /ai
//   node drive-ai.mjs chat           <outfile>  Ctrl+J, a question, an answer with a citation chip that opens the page
//   node drive-ai.mjs related        <outfile>  Ctrl+K "watering vegetables": a Related group with "Garden"
//   node drive-ai.mjs transcribe     <outfile>  /audio test.wav -> Transcribe -> a Transcript toggle and a summary
//   node drive-ai.mjs record         <outfile>  (fake microphone, permission accepted) /record, Stop: an audio block
//   node drive-ai.mjs record-prompt  <outfile>  (fake microphone only) does WebView2 prompt, allow or deny the microphone?
//   node drive-ai.mjs mcp            <outfile>  Settings -> MCP: a client, grants (native dialog), the bridge, raw HTTP checks
//   node drive-ai.mjs logs           <outfile>  the log of the shell and the core: no panic, no ERROR, no token, text or host
//   node drive-ai.mjs verify         <outfile>  the summary of every phase
//
// Every page phase writes what the page asked the network for, what failed, the console and the content-security-policy
// violations it saw, as JSON, and exits 1 when its own check fails, when the page asked another host for something (the page
// talks to the shell only: a request to 127.0.0.1:11434 is a failure), when a violation, a failed request or a console error was
// seen, or when the document of the webview was replaced (the marker `window.__aiMarker` is lost by a reload).

import { execFile, spawn } from "node:child_process";
import http from "node:http";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const CDP_PORT = Number(process.env.CDP_PORT ?? 9333);
const OLLAMA = "http://127.0.0.1:11434";
const MCP_PORT = 47811;

/**
 * A DevTools client for the app's page target (the same calls as cdp.mjs: trusted input, evaluation, a record of the
 * network, the console and the violations).
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

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.AI_OUT_DIR ?? join(HERE, "out");
const STATE_FILE = join(OUT, "ai-state.json");
const WAV = join(OUT, "ai-test.wav");
const PHASES = ["launch", "seed", "settings", "consent", "guard", "index", "writer", "chat", "related", "transcribe", "record-prompt-none", "record-prompt-block", "record-prompt-allow", "record", "mcp", "logs"];
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
const BUDGET_TITLE = "Budget review";
const BUDGET_TEXT = "The budget review moved to Thursday at 3 pm. Alice owns the spreadsheet draft and Bob checks the totals.";
const GARDEN_TITLE = "Garden";
const GARDEN_TEXT = "Tomatoes need water every morning; basil grows next to them.";
const QUESTION = "When is the budget review and who owns the draft?";
const REMOTE_URL = "http://10.255.255.1:11434";
const LOCAL_URL = "http://127.0.0.1:11434";

// ---------------------------------------------------------------------------------------------------------------
// logs: the scan of the log of the shell and the core (no app needed)
// ---------------------------------------------------------------------------------------------------------------

/** What must never be in the log (ai.md section 6.1: outcome, model, sizes and durations only). */
function privacyNeedles() {
  const state = readState();
  const hard = ["Thursday", "Tomatoes", "quarterly", "10.255.255.1"];
  const names = [BUDGET_TITLE, GARDEN_TITLE, "Alice", "From MCP", "Smoke", "watering vegetables", "When is the budget review", "ai-test.wav", "AI workspace", "spreadsheet draft", "Meeting audio", "Transcript"];
  return { hard, names, token: state.mcpToken ?? null };
}

function scanLogs() {
  const files = readdirSync(OUT).filter((n) => /^ai(-r\d+)?\.err\.log$/.test(n)).sort();
  const log = { files: [], lines: 0, panics: [], errors: [], warnings: [], leaks: [], tokenFound: false, needles: 0 };
  const { hard, names, token } = privacyNeedles();
  const needles = [...hard.map((n) => ({ n, kind: "hard" })), ...names.map((n) => ({ n, kind: "name" }))];
  log.needles = needles.length + (token ? 1 : 0);
  for (const file of files) {
    const text = readFileSync(join(OUT, file), "utf8");
    // The ANSI colours of tracing's formatter are not part of what the log says.
    const lines = text.split(/\r?\n/).map((l) => l.replace(/\u001b\[[0-9;]*m/g, ""));
    log.files.push({ file, lines: lines.length });
    log.lines += lines.length;
    for (const line of lines) {
      if (/panicked|panic:|thread '.*' panicked/i.test(line)) log.panics.push(`${file}: ${line.slice(0, 300)}`);
      if (/(^|\s)ERROR(\s|$)/.test(line)) log.errors.push(`${file}: ${line.slice(0, 300)}`);
      if (/(^|\s)WARN(\s|$)/.test(line)) log.warnings.push(`${file}: ${line.slice(0, 300)}`);
      if (token && line.includes(token)) log.tokenFound = true;
    }
    for (const { n, kind } of needles) {
      const escaped = n.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const pattern = new RegExp(`(?<![A-Za-z0-9_])${escaped}(?![A-Za-z0-9_])`);
      const hits = lines.filter((l) => pattern.test(l));
      if (hits.length > 0) log.leaks.push({ needle: n, kind, file, count: hits.length, first: hits[0].slice(0, 300) });
    }
  }
  if (token) log.leaks.push(...(log.tokenFound ? [{ needle: "(the MCP token)", kind: "hard", count: 1, first: "(not shown)" }] : []));
  return log;
}

if (phase === "logs") {
  const log = scanLogs();
  const ok = log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0;
  const result = { phase: "logs", ok, notes: [], log };
  result.notes.push(`${log.files.length > 0 ? "ok" : "FAIL"}: the log is there (${log.files.map((f) => `${f.file} ${f.lines} lines`).join(", ")})`);
  result.notes.push(`${log.panics.length === 0 ? "ok" : "FAIL"}: no panic (${log.panics.length})`);
  result.notes.push(`${log.errors.length === 0 ? "ok" : "FAIL"}: no ERROR line (${log.errors.length}${log.errors.length ? `: ${log.errors.slice(0, 3).join(" | ")}` : ""})`);
  result.notes.push(`${log.leaks.length === 0 ? "ok" : "FAIL"}: none of the token, page text, titles, question or host is in the log (${log.leaks.length} leaks${log.leaks.length ? `: ${log.leaks.map((l) => `${l.needle} x${l.count}`).join(", ")}` : ""}; ${log.needles} needles)`);
  result.notes.push(`info: ${log.warnings.length} WARN lines`);
  if (outFile) writeFileSync(outFile, JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ phase, ok, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, warnings: log.warnings.slice(0, 40), files: log.files }, null, 2));
  process.exit(ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// verify: the summary of the phase reports (no app needed)
// ---------------------------------------------------------------------------------------------------------------
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0, details: {}, bugs: [], log: null };
  for (const name of PHASES) {
    const file = join(OUT, `ai-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = { ok: report.ok, checks: report.notes.filter((n) => !n.startsWith("info:")).length, failed: report.notes.filter((n) => n.startsWith("FAIL")).length, seconds: report.seconds };
    summary.ok = summary.ok && report.ok;
    summary.captures.push(...(report.captures ?? []));
    const detail = {};
    for (const key of ["settings", "consent", "index", "writer", "chat", "related", "transcribe", "record", "micPrompt", "mcp"]) {
      if (report[key] !== undefined) detail[key] = report[key];
    }
    if (Object.keys(detail).length > 0) summary.details[name] = detail;
    summary.failedNotes.push(...report.notes.filter((n) => n.startsWith("FAIL")).map((n) => `${name}: ${n}`));
    summary.bugs.push(...(report.bugs ?? []).map((b) => `${name}: ${b}`));
    summary.foreignRequests += report.foreignRequests?.length ?? 0;
    summary.cspViolations += report.cspViolations?.length ?? 0;
    summary.failedRequests += (report.failures ?? []).filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED").length;
    summary.consoleErrors += (report.console ?? []).filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).length;
  }
  summary.log = scanLogs();
  const log = summary.log;
  summary.ok = summary.ok && log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0;
  writeFileSync(outFile, JSON.stringify(summary, null, 2));
  console.log(
    JSON.stringify(
      {
        phase: "verify",
        ok: summary.ok,
        phases: summary.phases,
        failedNotes: summary.failedNotes,
        bugs: summary.bugs,
        foreignRequests: summary.foreignRequests,
        cspViolations: summary.cspViolations,
        failedRequests: summary.failedRequests,
        consoleErrors: summary.consoleErrors,
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

/** A mouse button pressed and released at a point (`clickCount` 3 sends the clicks 1, 2 and 3: a triple click). */
async function mouse(x, y, { button = "left", clickCount = 1 } = {}) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  for (let n = 1; n <= clickCount; n++) {
    await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, buttons: BUTTON_BIT[button], clickCount: n });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, buttons: 0, clickCount: n });
  }
  await wait(250);
}

/** The pointer moved to a point, without a button (a hover). */
async function hover(x, y) {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await wait(300);
}

/** A key with Ctrl held, as a trusted event (`z` -> Ctrl+Z). */
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
  const path = join(OUT, `ai-${name}.png`);
  return new Promise((resolvePromise) => {
    execFile(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        '. $env:AI_CAPTURE_PS1; $p = Get-Process -Name atomik-kb-desktop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1; Capture-Window $p.MainWindowHandle $env:AI_CAPTURE_PATH',
      ],
      { env: { ...process.env, AI_CAPTURE_PATH: path }, timeout: 40000 },
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

// ----- the helpers of the .ps1 (native dialogs) --------------------------------------------------------------------

let helperCount = 0;
/** Runs one of drive-ai.ps1's helper steps; its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}) {
  helperCount += 1;
  const argFile = join(OUT, `ai-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `ai-helper-${process.pid}-${helperCount}.result.json`);
  writeFileSync(argFile, JSON.stringify(params));
  rmSync(resultFile, { force: true });
  return new Promise((resolvePromise, reject) => {
    execFile(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", process.env.AI_PS1, "-Step", step, "-Arg", argFile, "-Result", resultFile],
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

/** Drives a native file dialog that the page has just asked for: types `steps` (one text) and closes it. */
async function fileDialog(title, text) {
  const answer = await helper("dialog", { title, steps: [text] });
  info(`dialog "${title}": waited ${answer.waitedMs} ms, edit control ${answer.edit}${answer.initial ? `, proposed "${answer.initial}"` : ""}, ${answer.closed ? "closed" : "STILL OPEN"}`);
  check(answer.ok === true, `the "${title}" dialog took "${text}" and closed${answer.error ? ` (${answer.error}; seen ${JSON.stringify(answer.seen)}; controls ${JSON.stringify(answer.controls)})` : ""}`);
  return answer;
}

/**
 * Answers a native confirmation of the shell (a message box with custom buttons): finds it by its title, reads what it says,
 * captures it (`capturePath` is a name for out/ai-<name>.png) and presses the button named `press`.
 */
async function confirmation(title, press, { captureAs = null, timeoutMs = 30000 } = {}) {
  const capturePath = captureAs === null ? "" : join(OUT, `ai-${captureAs}.png`);
  const answer = await helper("message-dialog", { title, press, capture: capturePath, timeoutMs });
  const said = [...(answer.texts ?? [])].join(" | ");
  // The buttons UI Automation names, and the Button windows Win32 finds (a task dialog's buttons are in both, or one of them).
  const buttons = [...new Set([...(answer.buttons ?? []), ...(answer.children ?? []).filter((c) => c.startsWith("Button ")).map((c) => c.replace(/^Button '(.*)'$/, "$1").replace(/&/g, ""))])];
  info(`dialog "${title}": waited ${answer.waitedMs} ms, class ${answer.class}, text "${said}", buttons ${JSON.stringify(buttons)}, children ${JSON.stringify(answer.children)}, pressed ${press} by ${answer.method}, ${answer.closed ? "closed" : "STILL OPEN"}`);
  if (answer.captured) result.captures.push(answer.captured);
  return { ...answer, said, buttons };
}

// ----- the core, through the page's own IPC -------------------------------------------------------------------------

/** One request to the core as the UI sends it (`invoke("dispatch", { request })`): `{ ok, value }` or `{ ok: false, error }`. */
async function core(request) {
  // A request without fields (`SpaceList`, `AiIndexStatus`) is a unit variant: it has no `data`.
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
/** A shell command of the AI side (`ai_status`, ...), through the page's own IPC. */
async function command(name, args = {}) {
  return page.evaluate(`(async () => {
    try {
      const value = await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(name)}, ${JSON.stringify(args)});
      return { ok: true, value };
    } catch (e) {
      return { ok: false, error: e instanceof Error ? { message: e.message } : e };
    }
  })()`);
}
const SETTING = async (key) => (await ask({ type: "SettingsGet", data: { key } })) ?? null;

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

/** The menu item whose text starts with `label`. */
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

/** Opens a page by clicking its row in the sidebar (ancestors expanded first, the page found by its id). */
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
      const again = await box(`[role="treeitem"][data-page-id="${pageId}"] .tree-title`);
      await mouse(again.cx, again.cy);
    }
  }
  await wait(700);
  await waitFor(".content-body h1", "the title of the opened page");
}

/** Opens Settings from the sidebar (nothing to do when it is open). */
async function openSettings() {
  await normalize();
  if (await page.evaluate(`!!document.querySelector(".ai-settings, .mcp-settings")`)) return;
  await clickOn(".nav-item", "Settings in the sidebar", { text: "Settings" });
  await waitFor("#settings-ai", "the AI section of Settings", 20000, null, true);
  await wait(500);
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
  for (const ch of `/${query}`) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
  }
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

/** The top-level blocks of the open page, as text. */
const blocksText = () =>
  page.evaluate(`(() => {
    const editor = document.querySelector(".ProseMirror")?.editor;
    if (!editor) return null;
    const out = [];
    editor.state.doc.forEach((node) => out.push(node.textContent));
    return out;
  })()`);
const pageMarkdown = async (pageId) => ask({ type: "PageExportMarkdown", data: { pageId } });

async function pageIds() {
  const state = readState();
  const tree = await treeOf(state.spaceId);
  const pages = livePages(tree);
  return { state, pages, budget: pages.find((p) => p.title === BUDGET_TITLE), garden: pages.find((p) => p.title === GARDEN_TITLE) };
}

/** Writes a note and waits until the answer of `fn` (a poll) is truthy. */
async function ollamaJson(path, body = null) {
  const res = await fetch(`${OLLAMA}${path}`, body === null ? {} : { method: "POST", body: JSON.stringify(body) });
  return res.json();
}

// ---------------------------------------------------------------------------------------------------------------
// phases
// ---------------------------------------------------------------------------------------------------------------

async function phaseMark() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  const marker = `ai-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await page.evaluate(`(() => { window.__aiMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  writeState({ marker, href });
  info(`marker set on ${href}`);
}

async function phaseLaunch() {
  await waitFor(".tab-strip", "the tab strip", 60000);
  rmSync(STATE_FILE, { force: true }); // a fresh pass starts with no memory of an earlier one
  await phaseMark();
  const size = await page.evaluate(`({ w: innerWidth, h: innerHeight, dpr: devicePixelRatio, theme: document.documentElement.dataset.theme ?? "(none)" })`);
  info(`window ${size.w}x${size.h} at ${size.dpr}x, data-theme ${size.theme}`);
  check((await heading()) === "AI workspace", `the main area shows the workspace's Home (${await heading()})`);
  const space = await spaceInfo();
  must(space !== undefined, "the workspace has a space");
  writeState({ spaceId: space.id, spaceName: space.name });
  info(`the space is "${space.name}"`);
  const tree = await treeOf(space.id);
  check(tree.roots.length === 0, `the space starts empty (${tree.roots.length} pages)`);
  check(existsSync(WAV), `the speech WAV is in out/ (${existsSync(WAV) ? statSync(WAV).size : 0} bytes)`);
  // Before anything is turned on: no AI setting, no endpoint, and Ollama itself is up on this machine.
  check((await SETTING("app.ai.settings")) === null, "AI is off before Settings is used (no app.ai.settings)");
  let version = null;
  try {
    version = (await ollamaJson("/api/version")).version;
  } catch (error) {
    version = `unreachable: ${error instanceof Error ? error.message : String(error)}`;
  }
  info(`Ollama says ${version}`);
  check(/^\d+\.\d+/.test(String(version)), `Ollama answers on ${OLLAMA} (${version})`);
  writeState({ ollamaVersion: version });
}

// ----- phase: seed -------------------------------------------------------------------------------------------------

async function phaseSeed() {
  const state = readState();
  for (const [title, text] of [
    [BUDGET_TITLE, BUDGET_TEXT],
    [GARDEN_TITLE, GARDEN_TEXT],
  ]) {
    await newPage(title);
    await newLine();
    await send("Input.insertText", { text });
    await page.evaluate(`document.activeElement?.blur?.()`);
    await wait(2500);
  }
  const { pages, budget, garden } = await pageIds();
  must(budget !== undefined && garden !== undefined, `the core has both pages (${pages.map((p) => p.title).join(", ")})`);
  writeState({ budgetId: budget.id, gardenId: garden.id });
  const md = await pageMarkdown(budget.id);
  check(norm(md).includes(norm(BUDGET_TEXT)), `"${BUDGET_TITLE}" holds its text in the core (${norm(md).slice(0, 120)})`);
  const md2 = await pageMarkdown(garden.id);
  check(norm(md2).includes(norm(GARDEN_TEXT)), `"${GARDEN_TITLE}" holds its text in the core (${norm(md2).slice(0, 120)})`);
  void state;
  await capture("seeded");
}

// ----- phase: settings ---------------------------------------------------------------------------------------------

/** The value of the model picker whose label is `label`, or null. */
const modelValue = (label) =>
  page.evaluate(`(() => {
    const field = [...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === ${JSON.stringify(label)});
    return field ? field.querySelector("select").value : null;
  })()`);

/** Chooses `value` in the picker whose label is `label`: type-ahead keys on the focused select, else the native setter. */
async function chooseModel(label, typeAhead, value) {
  await page.evaluate(`(() => {
    const field = [...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === ${JSON.stringify(label)});
    const select = field.querySelector("select");
    select.scrollIntoView({ block: "center" });
    select.focus();
  })()`);
  await wait(200);
  for (const ch of typeAhead) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
  }
  await wait(600);
  if ((await modelValue(label)) === value) return "keys";
  await page.evaluate(`(() => {
    const field = [...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === ${JSON.stringify(label)});
    const select = field.querySelector("select");
    const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set;
    set.call(select, ${JSON.stringify(value)});
    select.dispatchEvent(new Event("change", { bubbles: true }));
  })()`);
  await wait(600);
  return "setter";
}

async function phaseSettings() {
  await openSettings();
  await waitFor(".ai-settings", "Settings -> AI");
  // Off at first: the pickers are disabled.
  const off = await page.evaluate(`({ enabled: document.querySelector(".ai-enabled input").checked, chatDisabled: document.querySelector(".ai-model select").disabled })`);
  check(off.enabled === false && off.chatDisabled === true, `AI starts off, the model pickers are disabled (${JSON.stringify(off)})`);
  const lineOf = () => page.evaluate(`document.querySelector(".ai-status-line p[role=status]")?.textContent.trim() ?? null`);
  const line = await waitUntil(`(() => { const t = document.querySelector(".ai-status-line p[role=status]")?.textContent.trim() ?? ""; return t.startsWith("Connected") || t.startsWith("Not connected") ? t : null; })()`, "the connection status", 30000);
  const version = readState().ollamaVersion;
  check(/^Connected, Ollama \d+\.\d+\.\d+, \d+ models$/.test(line), `the status says Connected with the Ollama version and the model count (${line})`);
  check(line.includes(`Ollama ${version}`), `the version is the one Ollama reports (${version})`);
  const badge = await page.evaluate(`document.querySelector(".ai-endpoint-badge .ai-badge")?.textContent.trim() ?? null`);
  check(badge === "Local", `the endpoint is badged Local (${badge})`);
  const endpoint = await page.evaluate(`document.querySelector(".ai-endpoint input").value`);
  check(endpoint === LOCAL_URL, `the endpoint field shows the default address (${endpoint})`);

  // Turn AI on: the empty models are filled from what Ollama lists.
  await clickOn(".ai-enabled input", "the Use AI features switch");
  await waitUntil(`document.querySelector(".ai-enabled input").checked === true`, "the AI switch is on");
  const chat = await waitUntil(`(() => { const f = [...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === "Chat and writing model"); const v = f?.querySelector("select").value; return v ? v : null; })()`, "a chat model is chosen by default", 20000);
  check(chat === "qwen3:8b", `the chat model defaults to qwen3:8b (${chat})`);
  const embedding = await waitUntil(`(() => { const f = [...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === "Embedding model"); const v = f?.querySelector("select").value; return v ? v : null; })()`, "an embedding model is chosen by default", 20000);
  check(embedding === "nomic-embed-text:latest", `the embedding model defaults to nomic-embed-text:latest (${embedding})`);
  check((await modelValue("Transcription model")) === "", `the transcription model is left empty ("None")`);
  const audioOptions = await page.evaluate(`[...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === "Transcription model").querySelectorAll("option").length`);
  info(`the transcription picker offers ${audioOptions} options (None included)`);
  const chatOptions = await page.evaluate(`[...[...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === "Chat and writing model").querySelectorAll("option")].map((o) => o.value)`);
  check(!chatOptions.some((v) => v.includes("embed")), `the chat picker lists no embedding model (${chatOptions.join(", ")})`);

  // Transcription: r2t2.
  const how = await chooseModel("Transcription model", "r2", "r2t2:latest");
  const transcription = await waitUntil(`(() => { const f = [...document.querySelectorAll(".ai-model")].find((f) => f.querySelector("label")?.textContent.trim() === "Transcription model"); const v = f?.querySelector("select").value; return v ? v : null; })()`, "a transcription model is chosen", 10000);
  check(transcription === "r2t2:latest", `the transcription model is r2t2:latest (chosen with ${how}; ${transcription})`);
  await wait(800);
  const stored = await SETTING("app.ai.settings");
  check(stored?.enabled === true && stored.chatModel === "qwen3:8b" && stored.embeddingModel === "nomic-embed-text:latest" && stored.transcriptionModel === "r2t2:latest", `app.ai.settings holds the choice (${JSON.stringify(stored)})`);
  check(stored?.semanticIndex === true && stored?.think === false, `the semantic index is on and think is off by default (${JSON.stringify({ semanticIndex: stored?.semanticIndex, think: stored?.think })})`);
  const status = await command("ai_status");
  check(status.ok && status.value.reachable === true && status.value.local === true && status.value.endpoint === LOCAL_URL, `ai_status through the shell: reachable, local, ${LOCAL_URL} (${JSON.stringify(status.ok ? { reachable: status.value.reachable, local: status.value.local, endpoint: status.value.endpoint, version: status.value.version, models: status.value.models?.length, indexer: status.value.indexer } : status.error)})`);
  result.settings = { line, chat, embedding, transcription, stored, indexer: status.ok ? status.value.indexer : null, models: status.ok ? status.value.models.map((m) => `${m.name} [${(m.capabilities ?? []).join(",")}]`) : null };
  await page.evaluate(`document.querySelector("#settings-ai").scrollIntoView({ block: "start" })`);
  await wait(300);
  await capture("settings");
  void lineOf;
}

// ----- phase: consent ----------------------------------------------------------------------------------------------

/** Types `text` in the Ollama address field (replacing what it holds). */
async function typeEndpoint(text) {
  await clickOn(".ai-endpoint input", "the Ollama address field");
  await page.evaluate(`document.querySelector(".ai-endpoint input").select()`);
  await send("Input.insertText", { text });
  await wait(200);
}

async function phaseConsent() {
  await openSettings();
  await waitFor(".ai-endpoint input", "the Ollama address field");
  check((await SETTING("app.ai.endpoint")) === null || (await SETTING("app.ai.endpoint")).remoteConsent === false, "before: no consent to a remote endpoint is stored");
  // A remote address: Save asks in a native dialog (only the shell can show it).
  await typeEndpoint(REMOTE_URL);
  await clickOn('.ai-endpoint button[type="submit"]', "Save", { text: "Save" });
  const dialog = await confirmation("Send text to another computer?", "Cancel", { captureAs: "consent-dialog" });
  check(dialog.ok === true, `the native "Send text to another computer?" dialog appeared and Cancel closed it${dialog.error ? ` (${dialog.error}; seen ${JSON.stringify(dialog.seen)})` : ""}`);
  check(dialog.said.includes("10.255.255.1"), `the dialog names the host (${dialog.said})`);
  check(/text of your pages/i.test(dialog.said) && /Anyone who controls that computer/i.test(dialog.said), "the dialog says that pages, questions and recordings are sent and who can read them");
  check((dialog.buttons ?? []).includes("Allow") && (dialog.buttons ?? []).includes("Cancel"), `the dialog has Allow and Cancel (${JSON.stringify(dialog.buttons)})`);
  const err = await waitUntil(`document.querySelector(".ai-endpoint .error-text")?.textContent.trim() ?? null`, "a declined note shows", 10000);
  check(err === "The address was not allowed, so nothing changed.", `the page says the address was not allowed (${err})`);
  const status = await command("ai_status");
  check(status.ok && status.value.endpoint === LOCAL_URL && status.value.local === true && status.value.remoteConsent === false, `the endpoint stays the local one, with no consent (${JSON.stringify(status.ok ? { endpoint: status.value.endpoint, local: status.value.local, consent: status.value.remoteConsent } : status.error)})`);
  const stored = await SETTING("app.ai.endpoint");
  check(stored === null || (stored.url === LOCAL_URL && stored.remoteConsent === false), `app.ai.endpoint holds nothing remote (${JSON.stringify(stored)})`);
  const badge = await page.evaluate(`document.querySelector(".ai-endpoint-badge .ai-badge")?.textContent.trim() ?? null`);
  check(badge === "Local", `the badge still says Local (${badge})`);
  await capture("consent-declined");

  // An address that is not an endpoint is refused in the page without any dialog.
  await typeEndpoint("ftp://example.com");
  await clickOn('.ai-endpoint button[type="submit"]', "Save", { text: "Save" });
  const bad = await waitUntil(`document.querySelector(".ai-endpoint .error-text")?.textContent.trim() ?? null`, "an invalid address is refused", 10000);
  info(`the invalid address note: ${bad}`);
  check(bad !== "The address was not allowed, so nothing changed." && bad.length > 0, `an ftp address is refused as invalid, not as declined (${bad})`);
  const none = await helper("message-dialog", { title: "Send text to another computer?", press: "Cancel", timeoutMs: 1500 });
  check(none.ok === false && /no 'Send text/.test(none.error ?? ""), "no confirmation dialog was shown for the invalid address");
  // Put the local address back.
  await typeEndpoint(LOCAL_URL);
  const saveDisabled = await page.evaluate(`document.querySelector('.ai-endpoint button[type="submit"]').disabled`);
  const errNow = await page.evaluate(`document.querySelector(".ai-endpoint .error-text")?.textContent.trim() ?? null`);
  check(saveDisabled === true && errNow === null, `with the local address back, Save is off and no note shows (${saveDisabled}, ${errNow})`);
  const requests = page.report().requests.filter((r) => r.url.includes("10.255.255.1"));
  check(requests.length === 0, "the page asked nothing of the remote host");
  result.consent = { dialogText: dialog.said, buttons: dialog.buttons, class: dialog.class, method: dialog.method, note: err, invalidNote: bad };
}

// ----- phase: guard ------------------------------------------------------------------------------------------------

async function phaseGuard() {
  const { budget } = await pageIds();
  const endpointBefore = await SETTING("app.ai.endpoint");
  const settingsBefore = await SETTING("app.ai.settings");
  const refused = await core({ type: "SettingsSet", data: { key: "app.ai.endpoint", value: { url: REMOTE_URL, remoteConsent: true } } });
  info(`SettingsSet app.ai.endpoint: ${JSON.stringify(refused)}`);
  check(refused.ok === false && refused.error?.code === "InvalidInput", `SettingsSet of app.ai.endpoint is refused with InvalidInput (${JSON.stringify(refused.error)})`);
  check(refused.error?.details?.field === "key", `... on the field "key" (${JSON.stringify(refused.error?.details)})`);
  check(!String(refused.error?.message ?? "").includes("10.255.255.1"), "the message does not repeat the value");
  check(JSON.stringify(await SETTING("app.ai.endpoint")) === JSON.stringify(endpointBefore), "app.ai.endpoint is unchanged");
  const second = await page.evaluate(`(async () => { try { await window.__TAURI_INTERNALS__.invoke("dispatch", { request: { type: "SettingsSet", data: { key: "app.ai.endpoint", value: "http://evil.example" } } }); return "accepted"; } catch (e) { return JSON.stringify(e); } })()`);
  check(second !== "accepted" && second.includes("InvalidInput"), `a second refusal, another value (${second})`);

  const shellOnly = [
    ["AiIndexNext", { type: "AiIndexNext", data: { maxChunks: 4 } }],
    ["AiIndexStore", { type: "AiIndexStore", data: { pageId: budget.id, sourceHash: "00", model: "nomic-embed-text:latest", vectors: [] } }],
    ["McpCall", { type: "McpCall", data: { token: "kbm_x", message: '{"jsonrpc":"2.0","id":1,"method":"ping"}' } }],
    ["McpClientGrantsSet", { type: "McpClientGrantsSet", data: { clientId: "x", grants: [] } }],
  ];
  for (const [name, request] of shellOnly) {
    const answer = await core(request);
    check(answer.ok === false && answer.error?.code === "InvalidInput" && answer.error?.details?.field === "type", `${name} from the page is refused (InvalidInput, field "type"): ${JSON.stringify(answer.ok ? "ACCEPTED" : answer.error)}`);
    if (answer.ok) bug(`${name} is accepted from the webview (guard.rs should refuse it)`);
  }
  // Controls: what the webview may do is accepted (the check can fail).
  const same = await core({ type: "SettingsSet", data: { key: "app.ai.settings", value: settingsBefore } });
  check(same.ok === true, `a control: SettingsSet of app.ai.settings (the value it has) is accepted (${JSON.stringify(same.error ?? "ok")})`);
  const statusAnswer = await core({ type: "AiIndexStatus", data: {} });
  check(statusAnswer.ok === true, `a control: AiIndexStatus is accepted (${JSON.stringify(statusAnswer.error ?? "ok")})`);
  const clients = await core({ type: "McpClientList", data: {} });
  check(clients.ok === true, `a control: McpClientList is accepted (${JSON.stringify(clients.error ?? "ok")})`);
  const query = await core({ type: "AiRetrieve", data: { query: "budget", scope: { type: "all" } } });
  info(`a control: AiRetrieve from the page: ${query.ok ? `${query.value.data?.length ?? "?"} passages` : JSON.stringify(query.error)}`);
}

// ----- phase: index ------------------------------------------------------------------------------------------------

async function phaseIndex() {
  const t0 = Date.now();
  const finalInfo = await waitUntil(
    `(async () => {
      const value = await window.__TAURI_INTERNALS__.invoke("dispatch", { request: { type: "AiIndexStatus" } });
      const d = value.data;
      return d.pagesTotal > 0 && d.pagesIndexed === d.pagesTotal && d.chunks > 0 ? d : null;
    })()`,
    "every page is indexed",
    180000,
  );
  const seconds = round((Date.now() - t0) / 1000);
  info(`indexed after ${seconds} s: ${JSON.stringify(finalInfo)}`);
  check(finalInfo.pagesIndexed === 2 && finalInfo.pagesTotal === 2, `both pages are indexed (${finalInfo.pagesIndexed} of ${finalInfo.pagesTotal})`);
  check(finalInfo.chunks >= 2, `the index holds chunks (${finalInfo.chunks})`);
  check(finalInfo.model === "nomic-embed-text:latest", `the index model is nomic-embed-text:latest (${finalInfo.model})`);
  check(typeof finalInfo.dimensions === "number" && finalInfo.dimensions >= 16, `the vectors have a dimension (${finalInfo.dimensions})`);
  check(finalInfo.dirty === 0, `no page waits (${finalInfo.dirty})`);
  await openSettings();
  const progress = await waitUntil(`(() => { const t = document.querySelector(".ai-index-progress")?.textContent.trim() ?? ""; return t.includes("2 of 2 pages indexed") ? t : null; })()`, "Settings shows every page indexed", 30000);
  check(progress.includes("Up to date"), `Settings says the index is up to date (${progress})`);
  const status = await command("ai_status");
  info(`the indexer: ${JSON.stringify(status.ok ? status.value.indexer : status.error)}`);
  check(status.ok && status.value.indexer?.phase !== "error", `the indexer is not in error (${JSON.stringify(status.ok ? status.value.indexer : status.error)})`);
  await page.evaluate(`document.querySelector(".ai-index")?.scrollIntoView({ block: "center" })`);
  await wait(300);
  await capture("index");

  // "Rebuild": the vectors are deleted, every page is marked, and the indexer embeds them again.
  const t1 = Date.now();
  await clickOn(".ai-index .row-actions button", "Rebuild", { text: "Rebuild" });
  let sawEmpty = false;
  for (let i = 0; i < 60 && !sawEmpty; i++) {
    const now = await ask({ type: "AiIndexStatus", data: {} });
    sawEmpty = now.pagesIndexed < 2 || now.dirty > 0 || now.chunks === 0;
    if (!sawEmpty) await wait(150);
  }
  check(sawEmpty, "right after Rebuild the index is empty or its pages wait (the vectors were deleted)");
  const again = await waitUntil(
    `(async () => {
      const value = await window.__TAURI_INTERNALS__.invoke("dispatch", { request: { type: "AiIndexStatus" } });
      const d = value.data;
      return d.pagesTotal > 0 && d.pagesIndexed === d.pagesTotal && d.chunks > 0 ? d : null;
    })()`,
    "every page is indexed again after Rebuild",
    180000,
  );
  const reindexSeconds = round((Date.now() - t1) / 1000);
  info(`indexed again ${reindexSeconds} s after Rebuild: ${JSON.stringify(again)}`);
  check(again.model === "nomic-embed-text:latest" && again.pagesIndexed === 2 && again.dimensions === finalInfo.dimensions, `the pages are indexed again with the same model and dimension (${again.pagesIndexed} of ${again.pagesTotal}, ${again.dimensions})`);
  result.index = { seconds, ...finalInfo, progress, indexer: status.ok ? status.value.indexer : null, reindexSeconds };
}

// ----- phase: writer -----------------------------------------------------------------------------------------------

/** Selects the first paragraph of the open page with a trusted triple click and returns its text. */
async function selectFirstParagraph() {
  const target = await waitFor(".ProseMirror p", "the first paragraph");
  const text = await page.evaluate(`document.querySelector(".ProseMirror p").textContent`);
  await mouse(target.x + 40, target.cy, { clickCount: 3 });
  await wait(400);
  const selected = await page.evaluate(`window.getSelection().toString()`);
  return { text, selected };
}

/** Runs one action of the writing panel on the selected paragraph; returns the answer (the panel stays open on it). */
async function runWriter(label) {
  await waitFor("button.format-ai", "the AI button of the selection toolbar");
  await clickOn("button.format-ai", "the AI button of the selection toolbar");
  await waitFor(".ai-writer", "the AI writing panel");
  const items = await page.evaluate(`[...document.querySelectorAll(".ai-writer-menu [role=menuitem]")].map((e) => e.textContent.trim().replace(/[▸▾]$/, ""))`);
  info(`the panel's actions: ${items.join(" | ")}`);
  check(items.includes(label), `the panel offers "${label}"`);
  await capture("writer-menu");
  const t0 = Date.now();
  await clickOn(".ai-writer-menu [role=menuitem]", `"${label}"`, { text: label });
  const done = await waitUntil(
    `(() => {
      const panel = document.querySelector(".ai-writer"); if (!panel) return null;
      const err = panel.querySelector(".ai-writer-error")?.textContent.trim();
      if (err) return { error: err };
      const buttons = [...panel.querySelectorAll(".ai-writer-buttons button")].map((b) => b.textContent.trim());
      if (buttons.includes("Stop") || !buttons.includes("Replace")) return null;
      return { text: panel.querySelector(".ai-writer-preview")?.innerText.trim() ?? "", buttons };
    })()`,
    `the answer to "${label}" is complete`,
    240000,
  );
  must(done.error === undefined, `"${label}" answered (${done.error})`);
  const seconds = round((Date.now() - t0) / 1000);
  info(`"${label}" answered in ${seconds} s: ${done.text.slice(0, 160)}`);
  return { ...done, seconds };
}

async function phaseWriter() {
  const { budget } = await pageIds();
  await openPageId(readState().spaceId, budget.id);
  await waitFor(".ProseMirror p", "the paragraph of the page");
  const before = await blocksText();
  check(norm(before[0]) === norm(BUDGET_TEXT), `the page starts with its text (${norm(before[0]).slice(0, 80)})`);
  check(before.length === 1, `the page has one block (${before.length})`);

  let answer = null;
  let used = null;
  for (const label of ["Improve writing", "Make shorter"]) {
    const { selected } = await selectFirstParagraph();
    must(norm(selected) === norm(BUDGET_TEXT), `the paragraph is selected (${norm(selected).slice(0, 60)})`);
    answer = await runWriter(label);
    used = label;
    if (norm(answer.text) !== norm(BUDGET_TEXT) && answer.text.trim() !== "") break;
    info(`"${label}" gave the same text back: trying another action`);
    await clickOn(".ai-writer .ai-writer-discard", "Discard", { text: "Discard" });
    await waitUntil(`!document.querySelector(".ai-writer")`, "the panel closed");
  }
  must(answer !== null && answer.text.trim() !== "", "the writing panel produced a text");
  check(answer.buttons.includes("Replace") && answer.buttons.includes("Insert below") && answer.buttons.includes("Retry") && answer.buttons.includes("Discard"), `the panel offers Replace, Insert below, Retry, Discard (${answer.buttons.join(", ")})`);
  check(norm(answer.text) !== norm(BUDGET_TEXT), `the result differs from the original (${used})`);
  // Nothing was written while it streamed.
  const during = await blocksText();
  check(norm(during[0]) === norm(BUDGET_TEXT) && during.length === 1, "the page still holds the original until Replace");
  await capture("writer-result");

  await clickOn(".ai-writer .ai-writer-buttons button", "Replace", { text: "Replace" });
  await waitUntil(`!document.querySelector(".ai-writer")`, "the panel closes when the answer is applied");
  await wait(600);
  const after = await blocksText();
  info(`after Replace: ${JSON.stringify(after).slice(0, 300)}`);
  check(norm(after.join(" ")) !== norm(BUDGET_TEXT), "the page text changed");
  const firstWords = norm(answer.text).slice(0, 25);
  check(norm(after.join(" ")).includes(firstWords), `the page now holds the answer (${firstWords}...)`);

  // One undo step brings the original back.
  const focused = await page.evaluate(`!!document.activeElement?.closest?.(".ProseMirror")`);
  if (!focused) await page.evaluate(`document.querySelector(".ProseMirror").editor.commands.focus()`);
  await ctrl("z");
  await wait(400);
  const undone = await blocksText();
  check(undone.length === 1 && norm(undone[0]) === norm(BUDGET_TEXT), `one Ctrl+Z brings the original text back (${JSON.stringify(undone).slice(0, 160)})`);
  await wait(2500);
  const md = await pageMarkdown(budget.id);
  check(norm(md).includes(norm(BUDGET_TEXT)), "the core holds the original text again");

  // /ai in an empty paragraph opens the panel at the cursor.
  await newLine();
  await slash("ai", "Ask AI");
  await waitFor(".ai-writer", "the panel opened by /ai");
  const prompt = await page.evaluate(`document.querySelector(".ai-writer input")?.placeholder ?? null`);
  check(prompt === "Ask AI to write…", `/ai opens the panel with the prompt field (${prompt})`);
  const cursorItems = await page.evaluate(`[...document.querySelectorAll(".ai-writer-menu [role=menuitem]")].map((e) => e.textContent.trim().replace(/[▸▾]$/, ""))`);
  check(!cursorItems.includes("Fix spelling and grammar") && cursorItems.includes("Continue writing"), `without a selection, the actions that need text are left out (${cursorItems.join(" | ")})`);
  await page.key("Escape", "Escape", 27);
  await wait(400);
  check(await page.evaluate(`!document.querySelector(".ai-writer")`), "Escape closes the panel");
  const left = await blocksText();
  check(norm(left[0]) === norm(BUDGET_TEXT), `the first paragraph is untouched after Escape (${left.length} blocks)`);
  result.writer = { action: used, seconds: answer.seconds, original: norm(BUDGET_TEXT), answer: answer.text.slice(0, 400), after: norm(after.join(" ")).slice(0, 400) };
}

// ----- phase: chat -------------------------------------------------------------------------------------------------

async function phaseChat() {
  const { garden, state } = await pageIds();
  await openPageId(state.spaceId, garden.id);
  await page.key("j", "KeyJ", 74, MOD.ctrl);
  await waitFor(".ai-chat", "the chat panel (Ctrl+J)");
  const scope = await page.evaluate(`document.querySelector(".ai-chat-scope select").value`);
  check(scope === "space", `the chat looks in this space by default (${scope})`);
  const empty = await page.evaluate(`document.querySelector(".ai-chat-empty")?.textContent.trim() ?? null`);
  check(empty !== null, `the empty chat explains itself (${empty})`);
  const t0 = Date.now();
  await clickOn("textarea.ai-chat-box", "the question box");
  await send("Input.insertText", { text: QUESTION });
  await wait(200);
  await page.key("Enter", "Enter", 13);
  await waitFor(".ai-msg-user", "the question in the conversation");
  // The sources come first, then the tokens.
  const outcome = await waitUntil(
    `(() => {
      const m = [...document.querySelectorAll(".ai-msg-assistant")].pop();
      if (!m) return null;
      const status = m.getAttribute("data-status");
      if (status === "done" || status === "error" || status === "stopped") return { status, text: m.innerText.trim(), error: m.querySelector("[role=alert]")?.textContent.trim() ?? null };
      return null;
    })()`,
    "the answer is complete",
    240000,
  );
  const seconds = round((Date.now() - t0) / 1000);
  info(`the answer came after ${seconds} s (${outcome.status}): ${outcome.text.slice(0, 600)}`);
  must(outcome.status === "done", `the answer finished (${outcome.status}${outcome.error ? `: ${outcome.error}` : ""})`);
  const answer = await page.evaluate(`(() => {
    const m = [...document.querySelectorAll(".ai-msg-assistant")].pop();
    const chips = [...m.querySelectorAll(".ai-cite")].map((c) => ({ n: c.querySelector(".ai-cite-n")?.textContent.trim(), title: c.getAttribute("title"), label: c.getAttribute("aria-label") }));
    const sources = [...m.querySelectorAll(".ai-source-list li")].map((li) => ({ title: li.querySelector(".ai-source-title")?.textContent.trim(), snippet: li.querySelector(".ai-source-snippet")?.textContent.trim().slice(0, 160) }));
    const prose = [...m.querySelectorAll("p, li")].map((e) => e.textContent).join(" ");
    return { chips, sources, prose, sourcesSummary: m.querySelector(".ai-sources summary")?.textContent.trim() ?? null };
  })()`);
  info(`chips ${JSON.stringify(answer.chips)}, sources ${JSON.stringify(answer.sources)}`);
  check(answer.chips.length >= 1, `the answer shows at least one citation chip (${answer.chips.length})`);
  check(answer.sources.length >= 1 && answer.sources.some((s) => s.title === BUDGET_TITLE), `the sources list "${BUDGET_TITLE}" (${answer.sources.map((s) => s.title).join(", ")}; ${answer.sourcesSummary})`);
  if (!/thursday/i.test(outcome.text)) info("the answer does not mention Thursday (model quality, not a failure)");
  if (!/alice/i.test(outcome.text)) info("the answer does not mention Alice (model quality, not a failure)");
  check(/thursday/i.test(outcome.text) && /alice/i.test(outcome.text), `the answer mentions Thursday and Alice (a quality check on qwen3:8b)`);
  // Actions of the answer.
  const actions = await page.evaluate(`[...document.querySelectorAll(".ai-msg-actions button")].map((b) => b.textContent.trim())`);
  check(["Copy", "Save as page", "Insert into page"].every((a) => actions.includes(a)), `the answer offers Copy, Save as page, Insert into page (${actions.join(", ")})`);
  await capture("chat");
  // Clicking the chip opens the cited page (the Garden page was open).
  check((await heading()) === GARDEN_TITLE, `before the click, "${GARDEN_TITLE}" is open (${await heading()})`);
  const chip = await box(`.ai-msg-assistant .ai-cite[aria-label*="${BUDGET_TITLE}"]`);
  must(chip !== null, `a chip names "${BUDGET_TITLE}"`);
  await mouse(chip.cx, chip.cy);
  await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(BUDGET_TITLE)}`, `clicking the chip opens "${BUDGET_TITLE}"`, 10000);
  check(true, `the chip opened "${BUDGET_TITLE}"`);
  await capture("chat-opened");
  // Ctrl+J closes the panel, and the conversation is kept in memory for the next opening.
  await page.key("j", "KeyJ", 74, MOD.ctrl);
  await wait(500);
  const closed = await page.evaluate(`!document.querySelector(".ai-chat")`);
  check(closed, "Ctrl+J closes the chat panel");
  result.chat = { seconds, question: QUESTION, answer: outcome.text, chips: answer.chips, sources: answer.sources };
}

// ----- phase: related ----------------------------------------------------------------------------------------------

async function phaseRelated() {
  await normalize();
  await page.key("k", "KeyK", 75, MOD.ctrl);
  await waitFor('input[role="combobox"].search-input', "the search dialog (Ctrl+K)");
  await clickOn('input[role="combobox"].search-input', "the search field");
  await send("Input.insertText", { text: "watering vegetables" });
  const related = await waitUntil(
    `(() => {
      const group = document.querySelector(".search-related");
      if (!group) return null;
      return [...group.querySelectorAll("[role=option], .search-hit")].map((e) => ({ label: e.getAttribute("aria-label"), text: e.textContent.trim().slice(0, 200) }));
    })()`,
    'a "Related" group shows',
    60000,
  );
  info(`related rows: ${JSON.stringify(related)}`);
  check(related.some((r) => (r.label ?? "").includes(`Related: ${GARDEN_TITLE}`) || r.text.includes(GARDEN_TITLE)), `the Related group lists "${GARDEN_TITLE}" (${related.map((r) => r.label).join(", ")})`);
  const full = await page.evaluate(`[...document.querySelectorAll(".search-group:not(.search-related) .search-hit")].map((e) => e.textContent.trim().slice(0, 80))`);
  check(full.length === 0, `no full-text hit: the words are not in the page (${full.length})`);
  const title = await page.evaluate(`document.querySelector(".search-related .search-group-title")?.textContent.trim() ?? null`);
  check(title !== null && title.includes("Related"), `the group is titled Related (${title})`);
  await capture("related");
  // Enter opens it.
  const row = await box(`.search-related [aria-label="Related: ${GARDEN_TITLE}"]`);
  if (row !== null) {
    await mouse(row.cx, row.cy);
    await waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(GARDEN_TITLE)}`, `the related row opens "${GARDEN_TITLE}"`, 10000);
    check(true, `the related row opened "${GARDEN_TITLE}"`);
  } else {
    await page.key("Escape", "Escape", 27);
  }
  result.related = { rows: related };
}

// ----- phase: transcribe -------------------------------------------------------------------------------------------

/** The editor's document as JSON. */
const docJson = () => page.evaluate(`document.querySelector(".ProseMirror").editor.getJSON()`);
const nodeText = (node) => (node.text ?? "") + (node.content ?? []).map(nodeText).join(node.type === "paragraph" || node.type === "heading" ? "" : " ");

async function phaseTranscribe() {
  must(existsSync(WAV), `the speech WAV is in out/ (${WAV}); drive-ai.ps1 -Step launch copies it`);
  const state = readState();
  await normalize();
  await newPage("Meeting audio");
  await newLine();
  await slash("audio", "Audio");
  await fileDialog("Open", WAV);
  await waitFor(".ProseMirror .attachment-block", "the audio block", 30000);
  await waitFor(".ProseMirror .media-preview audio", "the player of the audio block", 30000);
  const name = await page.evaluate(`document.querySelector(".ProseMirror .media-title")?.textContent.trim() ?? null`);
  check(name === "ai-test.wav", `the player shows the file name (${name})`);
  await capture("audio-block");
  // The block's "..." menu: Transcribe.
  await clickOn(".ProseMirror button.attachment-menu-button", "the File actions button");
  await waitFor('[role="menu"]', "the block menu");
  const items = await page.evaluate(`[...document.querySelectorAll('[role="menuitem"]')].map((e) => e.textContent.trim())`);
  info(`the block menu: ${items.join(" | ")}`);
  check(items.includes("Transcribe"), `the block menu offers Transcribe (${items.join(", ")})`);
  const t0 = Date.now();
  await chooseMenuItem("Transcribe");
  const seen = await waitUntil(`document.querySelector(".transcribe-status")?.textContent.trim() ?? null`, "the progress shows under the block", 30000);
  info(`progress: ${seen}`);
  check(true, `progress shows under the block (${seen})`);
  await capture("transcribe-progress");
  // Wait for the end: no progress left, or an error.
  const end = await waitUntil(
    `(() => {
      const s = document.querySelector(".transcribe-status");
      if (!s) return { done: true };
      if (s.classList.contains("is-error")) return { error: s.textContent.trim() };
      return null;
    })()`,
    "the transcription and the summary are finished",
    240000,
  );
  const seconds = round((Date.now() - t0) / 1000);
  must(end.error === undefined, `the transcription ended without an error (${end.error})`);
  info(`transcribed and summarised in ${seconds} s`);
  await wait(1500);
  const doc = await docJson();
  const top = doc.content ?? [];
  const at = top.findIndex((n) => n.type === "details");
  must(at >= 0, `a toggle block was inserted (${top.map((n) => n.type).join(", ")})`);
  const summaryNode = top[at].content?.[0];
  check(nodeText(summaryNode ?? {}).trim() === "Transcript", `the toggle is titled "Transcript" (${nodeText(summaryNode ?? {})})`);
  const transcript = nodeText({ type: "x", content: top[at].content?.slice(1) ?? [] });
  info(`transcript: ${transcript}`);
  check(/quarterly meeting/i.test(transcript), `the transcript has "quarterly meeting" (${transcript.slice(0, 200)})`);
  check(/thursday/i.test(transcript), `... and Thursday`);
  const stamps = (transcript.match(/\[\d+:\d{2}\]/g) ?? []).length;
  check(stamps >= 1, `each paragraph starts with a timestamp (${stamps})`);
  // The summary under the toggle: headings, key points and the action items.
  const after = top.slice(at + 1);
  const summaryText = after.map((n) => `${n.type}: ${nodeText(n)}`).join("\n");
  info(`after the toggle: ${summaryText.slice(0, 900)}`);
  check(after.length > 0, `a summary was inserted under the toggle (${after.length} blocks)`);
  const alice = after.filter((n) => /alice/i.test(nodeText(n)));
  check(alice.length > 0, `the summary mentions Alice (${alice.map((n) => n.type).join(", ") || "no"})`);
  const asTask = after.some((n) => n.type === "taskList" && /alice/i.test(nodeText(n)));
  check(asTask, `the action item with Alice is a to-do (a task list) (${after.map((n) => n.type).join(", ")})`);
  if (!asTask) {
    bug("meetingSummary's action items come out as a plain bullet list, not to-dos (ADR-0037 decision 5): crates/kb-ai/src/prompts.rs says only \"task list\" for the Action items section and never shows the `- [ ]` syntax (the actionItems action does), so qwen3:8b answers \"- Send budget draft (Alice, None)\"");
  }
  const hasHeadings = after.filter((n) => n.type === "heading").map((n) => nodeText(n));
  info(`summary headings: ${hasHeadings.join(" | ")}`);
  check(hasHeadings.some((h) => /summary/i.test(h)) && hasHeadings.some((h) => /action/i.test(h)), `the summary has its Summary and Action items sections (${hasHeadings.join(", ")})`);
  const { pages } = await pageIds();
  const meeting = pages.find((p) => p.title === "Meeting audio");
  writeState({ meetingId: meeting?.id });
  await capture("transcribe");
  // The transcript toggle is collapsed or open? Open it for the picture.
  const toggle = await box('.ProseMirror [data-type="details"] button');
  if (toggle !== null) {
    await mouse(toggle.cx, toggle.cy);
    await wait(400);
    await page.evaluate(`document.querySelector('.ProseMirror [data-type="details"]')?.scrollIntoView({ block: "center" })`);
    await capture("transcribe-open");
  }
  result.transcribe = { seconds, transcript, summary: summaryText.slice(0, 1500), blocks: top.map((n) => n.type) };
  void state;
}

// ----- phase: record -----------------------------------------------------------------------------------------------

async function phaseRecord() {
  const state = readState();
  await normalize();
  // A fake microphone (a beep every second) and its permission accepted: --use-fake-device-for-media-stream and
  // --use-fake-ui-for-media-stream were given to the WebView2 by drive-ai.ps1.
  await newPage("Recorder");
  await newLine();
  await send("Input.insertText", { text: "Before the recording." });
  await newLine();
  await slash("record", "Record audio");
  await waitFor(".recorder-bar", "the recorder bar");
  const started = await waitUntil(`document.querySelector(".recorder-bar .recorder-time")?.textContent.trim() ?? null`, "the recorder is recording (a timer shows)", 20000);
  info(`recording: timer ${started}`);
  // The fake microphone plays the 8 s speech clip once (drive-ai.ps1): 10 s are recorded before Pause, so no word is cut.
  let maxLevel = 0;
  for (let i = 0; i < 10; i++) {
    await wait(1000);
    const level = await page.evaluate(`Number(document.querySelector(".recorder-bar .recorder-meter")?.getAttribute("aria-valuenow") ?? 0)`);
    maxLevel = Math.max(maxLevel, level);
  }
  const timer = await page.evaluate(`document.querySelector(".recorder-bar .recorder-time")?.textContent.trim() ?? null`);
  check(/^00:(09|1\d)/.test(timer ?? ""), `the timer ran for about 10 s (${timer})`);
  check(maxLevel > 0, `the level meter moved with the fake microphone (max ${maxLevel})`);
  const lines = await page.evaluate(`[...document.querySelectorAll(".recorder-bar .recorder-line")].map((p) => p.textContent.trim())`);
  info(`the live transcript lines in the bar: ${JSON.stringify(lines)}`);
  const dot = await page.evaluate(`!!document.querySelector(".recorder-bar .recorder-dot")`);
  check(dot, "the recording dot shows");
  await capture("record-bar");
  // Pause and resume.
  await clickOn(".recorder-bar button", "Pause", { text: "Pause" });
  await waitUntil(`document.querySelector(".recorder-bar .recorder-state")?.textContent.trim() === "Paused"`, "the recorder is paused", 5000);
  await clickOn(".recorder-bar button", "Resume", { text: "Resume" });
  await waitUntil(`document.querySelector(".recorder-bar .recorder-state")?.textContent.trim() === "Recording"`, "the recorder is recording again", 5000);
  await wait(2000);
  const t0 = Date.now();
  await clickOn(".recorder-bar button", "Stop", { text: "Stop" });
  // Saving, transcribing, summarising: the bar goes when everything is done (or shows a notice).
  const end = await waitUntil(
    `(() => {
      const bar = document.querySelector(".recorder-bar");
      if (!bar) return { done: true };
      const notice = bar.querySelector(".recorder-notice-text")?.textContent.trim();
      if (notice) return { notice };
      return null;
    })()`,
    "the recorder has finished saving, transcribing and summarising",
    240000,
  );
  info(`finished after ${round((Date.now() - t0) / 1000)} s${end.notice ? `, notice: ${end.notice}` : ""}`);
  if (end.notice) {
    check(false, `the recorder ended with a notice (${end.notice})`);
  }
  await wait(1500);
  // The audio block, and the attachment behind it.
  const blockName = await waitUntil(`document.querySelector(".ProseMirror .media-title")?.textContent.trim() ?? null`, "an audio block shows its file name", 20000);
  check(/^Recording \d{4}-\d{2}-\d{2} \d{2}\.\d{2}\.webm$/.test(blockName), `the audio block is "Recording YYYY-MM-DD HH.mm.webm" (${blockName})`);
  const space = state.spaceId;
  const list = await ask({ type: "AttachmentList", data: { spaceId: space } });
  const recording = list.find((a) => a.name === blockName);
  check(recording !== undefined && recording.mime.startsWith("audio/webm") && Number(recording.size) > 1000, `the attachment exists (${recording ? `${recording.mime}, ${recording.size} bytes, ${recording.availability}` : "not found"})`);
  const kinds = await blocksText();
  info(`the page: ${JSON.stringify(kinds)}`);
  check(kinds[0] === "Before the recording.", "the page keeps its first line");
  const doc = await docJson();
  const types = (doc.content ?? []).map((n) => n.type);
  info(`blocks: ${types.join(", ")}`);
  const toggled = types.includes("details");
  info(`a transcript toggle ${toggled ? "was" : "was not"} inserted (without words in the recording it may be empty: not a failure)`);
  let transcript = null;
  let summary = null;
  if (toggled) {
    const at = types.indexOf("details");
    transcript = nodeText({ type: "x", content: doc.content[at].content?.slice(1) ?? [] });
    summary = doc.content.slice(at + 1).map((n) => `${n.type}: ${nodeText(n)}`).join("\n");
    info(`the recording's transcript: ${transcript}`);
    info(`the recording's summary: ${summary.slice(0, 600)}`);
    check(/quarterly meeting|thursday|alice/i.test(transcript), `the recording's transcript has the words of the clip (${transcript.slice(0, 160)})`);
  } else {
    check(false, "the recording of the speech clip was transcribed (no Transcript toggle was inserted: the fake microphone may not have played the file, or the transcript was empty)");
  }
  const audioState = await page.evaluate(`(() => { const a = document.querySelector(".ProseMirror audio"); return a ? { readyState: a.readyState, error: a.error ? a.error.code : null, src: a.currentSrc.slice(0, 40), duration: a.duration } : null; })()`);
  info(`the audio element: ${JSON.stringify(audioState)}`);
  await capture("record-done");
  result.record = { blockName, attachment: recording ? { mime: recording.mime, size: Number(recording.size) } : null, toggled, types, maxLevel, lines, transcript, summary: summary?.slice(0, 1000) ?? null };
}

// ----- phase: record-prompt ----------------------------------------------------------------------------------------

/** The permission bubble of the webview (a window of its own, a Chromium popup), or null. */
async function findBubble() {
  const windows = await helper("windows");
  const bubble = (windows.top ?? []).find((w) => w.visible && w.class === "Chrome_WidgetWin_1" && w.hwnd !== windows.main) ?? null;
  return { bubble, windows };
}

/** What a recorder bar shows: the timer, the status line, the notice (and the label of its buttons). */
const recorderState = () =>
  page.evaluate(`(() => {
    const bar = document.querySelector(".recorder-bar");
    return bar === null ? null : {
      timer: bar.querySelector(".recorder-time")?.textContent.trim() ?? null,
      status: bar.querySelector(".recorder-status")?.textContent.trim() ?? null,
      notice: bar.querySelector(".recorder-notice-text")?.textContent.trim() ?? null,
      buttons: [...bar.querySelectorAll("button")].map((b) => b.textContent.trim()),
    };
  })()`);

/** Polls the recorder until `done(state)` holds or `ms` pass; returns the last state and when it held. */
async function recorderUntil(done, ms) {
  const t0 = Date.now();
  let state = await recorderState();
  while (Date.now() - t0 < ms) {
    if (done(state)) return { state, ms: Date.now() - t0, held: true };
    await wait(250);
    state = await recorderState();
  }
  return { state, ms: Date.now() - t0, held: done(state) };
}

/** Presses the recorder's Cancel and Dismiss buttons, whichever are there. */
async function closeRecorder() {
  for (const label of ["Cancel", "Dismiss"]) {
    const button = await box(".recorder-bar button", label);
    if (button !== null) {
      await mouse(button.cx, button.cy);
      await wait(600);
    }
  }
}

/**
 * The microphone under the webview's own handling of the permission (a fake microphone, no fake permission UI): what does the person
 * see, and what does the recorder say? `AI_PROMPT_ANSWER` is none (only look, 15 s), block (press Block on the bubble) or allow
 * (press Allow). drive-ai.ps1 starts each from a profile with no stored microphone choice.
 */
async function phaseRecordPrompt() {
  const answer = process.env.AI_PROMPT_ANSWER ?? "none";
  const tag = `record-prompt-${answer}`;
  await normalize();
  await newPage(`Recorder ${answer}`);
  await newLine();
  // A log of what the page asks the browser for (diagnostics only: the calls are passed on unchanged).
  await page.evaluate(`(() => {
    window.__micLog = [];
    const t0 = performance.now();
    const at = () => Math.round(performance.now() - t0);
    const media = navigator.mediaDevices;
    const original = media.getUserMedia.bind(media);
    media.getUserMedia = (constraints) => {
      window.__micLog.push({ at: at(), event: "getUserMedia called" });
      return original(constraints).then(
        (stream) => { window.__micLog.push({ at: at(), event: "getUserMedia granted" }); return stream; },
        (error) => { window.__micLog.push({ at: at(), event: "getUserMedia rejected: " + error.name }); throw error; },
      );
    };
  })()`);
  const permissionOf = () => page.evaluate(`navigator.permissions.query({ name: "microphone" }).then((s) => s.state).catch((e) => "error: " + e.message)`);
  const permission0 = await permissionOf();
  info(`before /record: navigator.permissions says ${permission0}`);
  await slash("record", "Record audio");
  await waitFor(".recorder-bar", "the recorder bar");
  // At once: is there a window of its own besides the app's (a permission bubble of the webview would be one)?
  const early = await findBubble();
  const earlyTop = (early.windows.top ?? []).filter((w) => w.visible).map((w) => `${w.class} '${w.title}' ${w.width}x${w.height}`);
  let bubbleTexts = null;
  if (early.bubble !== null) {
    const path = join(OUT, `ai-${tag}-bubble.png`);
    await helper("capture-hwnd", { hwnd: early.bubble.hwnd, path });
    if (existsSync(path)) result.captures.push(path);
    const inside = await helper("uia", { hwnd: early.bubble.hwnd, limit: 100 });
    bubbleTexts = (inside.elements ?? []).map((e) => `${e.type}:${e.name}`).filter((n) => !/:$/.test(n));
  }
  info(`right after /record: top-level windows ${JSON.stringify(earlyTop)}; the bubble reads ${JSON.stringify(bubbleTexts)}; the last keyboard or mouse input of the real user was ${early.windows.idleMs} ms ago`);
  await capture(`${tag}-early`);
  await wait(3000);
  await capture(`${tag}-3s`);
  const after3s = await recorderState();
  const permission3 = await permissionOf();
  info(`recorder after 3 s: ${JSON.stringify(after3s)}; navigator.permissions says ${permission3}`);
  const outcome = { answer, permissionBefore: permission0, bubble: bubbleTexts, after3s, permissionAfter3s: permission3 };
  const bubbleShown = early.bubble !== null && (bubbleTexts ?? []).some((n) => /wants to/i.test(n));
  const pending = after3s !== null && after3s.timer === null && after3s.notice === null;
  const bubbleLine = (bubbleTexts ?? []).find((n) => /wants to/i.test(n)) ?? "no bubble";
  if (answer === "none") {
    // Nobody is meant to answer here, but a person at the machine may press Allow while the pass looks: that is reported, not failed.
    info(`unanswered: the bubble was ${bubbleShown ? `shown ("${bubbleLine}")` : "not seen"}; the recorder after 3 s ${pending ? "was still starting" : "was no longer waiting (somebody answered?)"}`);
  } else {
    check(bubbleShown, `the webview asks the person in a bubble of its own ("${bubbleLine}")`);
    check(pending, `until the person answers, the recorder is still starting (${JSON.stringify(after3s)})`);
  }

  if (answer === "none") {
    // Nobody answers: for how long does the recorder wait, and does it say anything?
    const waited = await recorderUntil((s) => s === null || s.timer !== null || s.notice !== null, 15000);
    info(`15 s without an answer: ${JSON.stringify(waited)}`);
    outcome.unanswered = waited;
    if (!waited.held && pending) {
      bug("an unanswered microphone prompt leaves the recorder bar at \"Starting the microphone…\" with only Cancel: there is no timeout and no hint that WebView2's own bubble (a window of its own, \"http://tauri.localhost wants to: Use your microphones\", Block / Allow) is waiting for an answer; Cancel does not close the bubble and a new request stays pending too");
    }
    await capture(`${tag}-waiting`);
  } else {
    const name = answer === "block" ? "Block" : "Allow";
    must(early.bubble !== null, `a bubble to press ${name} on`);
    const { bubble } = await findBubble();
    const pressed = await helper("invoke", { hwnd: (bubble ?? early.bubble).hwnd, name });
    check(pressed.ok === true, `${name} pressed on the bubble${pressed.error ? ` (${pressed.error})` : ""}`);
    const settled = await recorderUntil((s) => s === null || s.timer !== null || s.notice !== null, 10000);
    info(`after ${name}: ${JSON.stringify(settled)}`);
    outcome.afterAnswer = settled;
    await capture(`${tag}-after`);
    if (answer === "allow") {
      check(settled.state !== null && settled.state.timer !== null, `after Allow the recorder records (${JSON.stringify(settled.state)})`);
      await wait(2000);
      const timer = (await recorderState())?.timer ?? null;
      check(timer !== null && timer !== "00:00", `the timer runs (${timer})`);
    } else {
      check(settled.state !== null && settled.state.notice !== null, `after Block the recorder says so (${JSON.stringify(settled.state)})`);
      const notice = settled.state?.notice ?? "";
      outcome.blockedNotice = notice;
      if (/privacy settings|windows/i.test(notice)) {
        bug(`after the person presses Block on the WebView2 bubble, the recorder says "${notice}": the choice is kept by the webview in its own profile (media_stream_mic for http://tauri.localhost), not in the system's privacy settings, so the advice does not undo it, and nothing in the app can reset it`);
      }
    }
    // Close the recorder and try again: what happens now that a choice is stored?
    await closeRecorder();
    await newLine();
    await slash("record", "Record audio");
    await waitFor(".recorder-bar", "the recorder bar again");
    const second = await recorderUntil((s) => s === null || s.timer !== null || s.notice !== null, 8000);
    const again = await findBubble();
    info(`second /record: ${JSON.stringify(second)}; a bubble: ${again.bubble !== null}`);
    outcome.second = { ...second, bubble: again.bubble !== null };
    check(again.bubble === null, `the second /record asks nobody (the choice is kept) (bubble: ${again.bubble !== null})`);
    if (answer === "allow") check(second.state !== null && second.state.timer !== null, `the second /record records at once (${JSON.stringify(second.state)}, ${second.ms} ms)`);
    else check(second.state !== null && second.state.notice !== null, `the second /record is refused at once with the notice (${JSON.stringify(second.state)}, ${second.ms} ms)`);
    await capture(`${tag}-second`);
  }
  const log = await page.evaluate(`window.__micLog`);
  info(`what the page asked the browser for (ms since the start): ${JSON.stringify(log)}`);
  outcome.log = log;
  await closeRecorder();
  check(!(await page.evaluate(`!!document.querySelector(".recorder-bar .recorder-time")`)), "Cancel ends the recorder (no timer left)");
  outcome.permissionEnd = await permissionOf();
  info(`navigator.permissions at the end: ${outcome.permissionEnd}`);
  result.micPrompt = outcome;
  // The recorder's own errors are console errors here: they are the finding, not a failure of this phase.
  result.allowConsoleErrors = true;
}

// ----- phase: mcp --------------------------------------------------------------------------------------------------

/** One raw HTTP request to the MCP server (node:http, so that Host and Origin can be set freely). */
function rawHttp({ method = "POST", path = "/mcp", headers = {}, body = null }) {
  return new Promise((resolvePromise) => {
    const req = http.request({ host: "127.0.0.1", port: MCP_PORT, method, path, headers: { connection: "close", ...headers }, agent: false }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => resolvePromise({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks).toString("utf8") }));
    });
    req.on("error", (e) => resolvePromise({ error: e.message }));
    req.setTimeout(15000, () => req.destroy(new Error("timeout")));
    req.end(body);
  });
}

/** The stdio bridge as a child process: lines in, lines out. */
function startBridge(command, token) {
  const child = spawn(command, ["--port", String(MCP_PORT)], { env: { ...process.env, ATOMIK_KB_MCP_TOKEN: token }, stdio: ["pipe", "pipe", "pipe"] });
  let buffer = "";
  const lines = [];
  const waiters = [];
  let stderr = "";
  child.stdout.on("data", (data) => {
    buffer += data.toString("utf8");
    let at;
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at).replace(/\r$/, "");
      buffer = buffer.slice(at + 1);
      const waiter = waiters.shift();
      if (waiter) waiter(line);
      else lines.push(line);
    }
  });
  child.stderr.on("data", (data) => {
    stderr += data.toString("utf8");
  });
  let ids = 0;
  const next = (ms = 20000) =>
    new Promise((resolvePromise) => {
      if (lines.length > 0) {
        resolvePromise(lines.shift());
        return;
      }
      const timer = setTimeout(() => {
        const at = waiters.indexOf(waiter);
        if (at >= 0) waiters.splice(at, 1);
        resolvePromise(null);
      }, ms);
      const waiter = (line) => {
        clearTimeout(timer);
        resolvePromise(line);
      };
      waiters.push(waiter);
    });
  return {
    child,
    stderr: () => stderr,
    next,
    notify(method, params) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, ...(params ? { params } : {}) })}\n`);
    },
    async rpc(method, params) {
      ids += 1;
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: ids, method, ...(params ? { params } : {}) })}\n`);
      const line = await next();
      if (line === null) return { timeout: true };
      try {
        return { ...JSON.parse(line), _line: line };
      } catch {
        return { parseError: true, _line: line };
      }
    },
    stop() {
      child.stdin.end();
      child.kill();
    },
  };
}

const toolText = (reply) => (reply?.result?.content ?? []).map((c) => c.text ?? "").join("\n");

/** After Save permissions: does "Permissions saved." show (the grants editor is keyed by the stored grants, so it may start again)? */
async function savedNote(what) {
  let note = null;
  for (let i = 0; i < 30 && note === null; i++) {
    note = await page.evaluate(`document.querySelector(".mcp-grants p[role=status]")?.textContent.trim() ?? null`);
    if (note === null) await wait(100);
  }
  info(`after saving the ${what} permission: the editor says ${JSON.stringify(note)}`);
  if (note !== "Permissions saved.") {
    bug(`Settings -> MCP -> Permissions: after Save permissions (${what}) the saved grants show in the client's summary but "Permissions saved." never appears: McpSettings.tsx keys GrantsEditor by JSON.stringify(client.grants), so the new grants remount the editor and its message state is lost`);
  }
  check(note === "Permissions saved.", `the editor confirms "Permissions saved." after the ${what} permission (${note})`);
}

async function phaseMcp() {
  const state = readState();
  const space = await spaceInfo();
  const { budget } = await pageIds();
  await openSettings();
  await waitFor(".mcp-settings", "Settings -> MCP", 20000, null, true);
  await page.evaluate(`document.querySelector(".mcp-settings").scrollIntoView({ block: "start" })`);
  await wait(300);
  const off = await page.evaluate(`({ on: document.querySelector(".mcp-enabled input").checked, status: document.querySelector(".mcp-status")?.textContent.trim(), port: document.querySelector(".mcp-port input").value })`);
  check(off.on === false && off.status === "The MCP server is off.", `MCP starts off (${JSON.stringify(off)})`);
  check(off.port === String(MCP_PORT), `the default port is ${MCP_PORT} (${off.port})`);
  // Nothing listens before it is turned on.
  const before = await rawHttp({ body: '{"jsonrpc":"2.0","id":1,"method":"ping"}', headers: { "content-type": "application/json" } });
  check(before.error !== undefined, `nothing listens on ${MCP_PORT} while MCP is off (${before.error ?? before.status})`);

  await clickOn(".mcp-enabled input", "the Run the MCP server switch");
  const line = await waitUntil(`(() => { const t = document.querySelector(".mcp-status")?.textContent.trim() ?? ""; return t.startsWith("Listening") || t.includes("not running") ? t : null; })()`, "the MCP status", 20000);
  check(line === `Listening on 127.0.0.1:${MCP_PORT}`, `the status says it is listening (${line})`);
  const status = await command("mcp_status");
  check(status.ok && status.value.running === true && status.value.port === MCP_PORT && status.value.url === `http://127.0.0.1:${MCP_PORT}/mcp`, `mcp_status: running on the loopback (${JSON.stringify(status.ok ? status.value : status.error)})`);
  const listening = await new Promise((resolvePromise) => {
    const netStat = spawn("powershell.exe", ["-NoProfile", "-Command", `Get-NetTCPConnection -LocalPort ${MCP_PORT} -State Listen | ForEach-Object { $_.LocalAddress }`]);
    let text = "";
    netStat.stdout.on("data", (d) => (text += d.toString()));
    netStat.on("close", () => resolvePromise(text.trim().split(/\s+/).filter(Boolean)));
  });
  check(listening.length > 0 && listening.every((a) => a === "127.0.0.1"), `the port is bound to 127.0.0.1 only (${listening.join(", ") || "none"})`);

  // A client.
  await clickOn(".mcp-add input", "the client name field");
  await send("Input.insertText", { text: "Smoke" });
  await clickOn('.mcp-add button[type="submit"]', "Add client", { text: "Add client" });
  await waitFor(".mcp-new", "the new client's key and configuration");
  const token = await page.evaluate(`document.querySelector(".mcp-new .mcp-token")?.textContent.trim() ?? null`);
  check(/^kbm_[A-Za-z0-9_-]{43}$/.test(token ?? ""), "the key is kbm_ and 43 characters of base64url (not shown here)");
  writeState({ mcpToken: token });
  const config = await page.evaluate(`[...document.querySelectorAll(".mcp-new pre.mcp-config")].map((p) => p.textContent)`);
  const noBridgeNote = await page.evaluate(`!![...document.querySelectorAll(".mcp-new .muted")].find((e) => e.textContent.includes("bridge program was not found"))`);
  let desktop = null;
  try {
    desktop = JSON.parse(config[0]);
  } catch {
    // reported below
  }
  const entry = desktop?.mcpServers?.["atomik-kb"];
  check(entry !== undefined && entry.args?.[0] === "--port" && entry.args?.[1] === String(MCP_PORT) && entry.env?.ATOMIK_KB_MCP_TOKEN === token, "the Claude Desktop JSON has the bridge, --port 47811 and the key in its environment");
  const bridgePath = entry?.command ?? null;
  info(`the Claude Desktop JSON's command: ${bridgePath}`);
  const bridgeFound = bridgePath !== null && !bridgePath.startsWith("<");
  check(bridgeFound && /target[\\/]debug[\\/]kb-mcp-bridge\.exe$/i.test(bridgePath), `the bridge path is target\\debug\\kb-mcp-bridge.exe (${bridgePath})`);
  check(!noBridgeNote === bridgeFound, `the "bridge not found" note matches (${noBridgeNote})`);
  check(bridgeFound ? existsSync(bridgePath) : true, "the bridge path exists on disk");
  check(/^claude mcp add --transport http atomik-kb http:\/\/127\.0\.0\.1:47811\/mcp --header "Authorization: Bearer kbm_/.test(config[1] ?? ""), `the Claude Code command is shown (${(config[1] ?? "").slice(0, 80)}...)`);
  await capture("mcp-client");

  // No grants yet: the new client sees nothing.
  const access0 = await page.evaluate(`document.querySelector(".mcp-client-access")?.textContent.trim() ?? null`);
  check(access0 === "No access yet.", `the new client has no access (${access0})`);
  await clickOn(".mcp-new button", "Done", { text: "Done" });
  await waitUntil(`!document.querySelector(".mcp-new")`, "the key is gone from the screen once Done is pressed");

  // Read access on the space: the native confirmation, then Allow.
  await clickOn('button[aria-label="Permissions of Smoke"]', "Permissions");
  await waitFor(".mcp-grants", "the permissions grid");
  await clickOn(`.mcp-grants input[aria-label=${JSON.stringify(`Read in ${space.name}`)}]`, `the Read box of ${space.name}`);
  await clickOn('.mcp-grants button[type="submit"]', "Save permissions", { text: "Save permissions" });
  const readDialog = await confirmation("Allow access to your pages?", "Allow", { captureAs: "mcp-grants-dialog" });
  check(readDialog.ok === true, `the native "Allow access to your pages?" dialog appeared and Allow closed it${readDialog.error ? ` (${readDialog.error}; ${JSON.stringify(readDialog.seen)})` : ""}`);
  check(readDialog.said.includes("Smoke") && readDialog.said.includes(space.name) && /read/i.test(readDialog.said) && !/change/i.test(readDialog.said), `the dialog asks to read ${space.name} for Smoke (${readDialog.said})`);
  await savedNote("read");
  const access1 = await waitUntil(`(() => { const t = document.querySelector(".mcp-client-access")?.textContent.trim(); return t && t !== "No access yet." ? t : null; })()`, "the client shows its access", 10000);
  check(access1 === `Can read: ${space.name}.`, `the client can read ${space.name} (${access1})`);
  const clientsNow = await ask({ type: "McpClientList", data: {} });
  check(clientsNow.length === 1 && clientsNow[0].name === "Smoke" && clientsNow[0].grants.length === 1 && clientsNow[0].grants[0].read === true && clientsNow[0].grants[0].write === false, `the core holds the read grant (${JSON.stringify(clientsNow.map((c) => ({ name: c.name, grants: c.grants })))})`);
  check(JSON.stringify(clientsNow).includes(token) === false, "the core's client list does not hold the key (only its hash)");

  // The bridge over stdio: what a Claude Desktop does.
  const bridge = startBridge(bridgeFound ? bridgePath : join(HERE, "..", "..", "target", "debug", "kb-mcp-bridge.exe"), token);
  const bridgeSteps = {};
  try {
    const init = await bridge.rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "smoke", version: "1" } });
    bridgeSteps.initialize = init.result?.serverInfo;
    check(init.result?.serverInfo?.name === "atomik-kb" && init.result?.protocolVersion === "2025-06-18" && init.result?.capabilities?.tools !== undefined, `initialize: atomik-kb, protocol 2025-06-18, tools (${JSON.stringify(init.result?.serverInfo)}, ${init.result?.protocolVersion}${init.timeout ? ", TIMEOUT: " + bridge.stderr().slice(0, 200) : ""})`);
    bridge.notify("notifications/initialized");
    const silent = await bridge.next(1500);
    check(silent === null, `notifications/initialized writes no line on stdout (${silent === null ? "silent" : silent})`);
    const tools = await bridge.rpc("tools/list");
    const names = (tools.result?.tools ?? []).map((t) => t.name).sort();
    bridgeSteps.tools = names;
    check(names.length === 6 && ["append_to_page", "create_page", "list_pages", "list_spaces", "read_page", "search"].every((n) => names.includes(n)), `tools/list: the six tools (${names.join(", ")})`);
    const spaces = await bridge.rpc("tools/call", { name: "list_spaces", arguments: {} });
    check(spaces.result?.isError !== true && toolText(spaces).includes(space.name), `list_spaces shows ${space.name} (${toolText(spaces).slice(0, 160)})`);
    const search = await bridge.rpc("tools/call", { name: "search", arguments: { query: "budget" } });
    bridgeSteps.search = toolText(search).slice(0, 300);
    check(search.result?.isError !== true && toolText(search).includes(BUDGET_TITLE), `search "budget" finds ${BUDGET_TITLE} (${toolText(search).slice(0, 200)})`);
    const read = await bridge.rpc("tools/call", { name: "read_page", arguments: { pageId: budget.id } });
    bridgeSteps.read = toolText(read).slice(0, 300);
    check(read.result?.isError !== true && toolText(read).includes("Thursday"), `read_page of ${BUDGET_TITLE} contains Thursday (${toolText(read).slice(0, 200).replace(/\n/g, " / ")})`);
    const denied = await bridge.rpc("tools/call", { name: "create_page", arguments: { spaceId: space.id, title: "Denied page", markdown: "- nope" } });
    bridgeSteps.denied = toolText(denied);
    check(denied.result?.isError === true, `create_page without the write grant is isError (${toolText(denied).slice(0, 160)})`);
    const missing = await bridge.rpc("tools/call", { name: "no_such_tool", arguments: {} });
    info(`an unknown tool: ${JSON.stringify(missing.error ?? missing.result)?.slice(0, 200)}`);
    const noMethod = await bridge.rpc("no/such/method");
    check(noMethod.error?.code === -32601, `an unknown method is -32601 (${JSON.stringify(noMethod.error)})`);
    check((await ask({ type: "TreeGet", data: { spaceId: space.id } })).roots.every((r) => r.page.title !== "Denied page"), "the denied call created no page");

    // Write access: another confirmation.
    await clickOn(`.mcp-grants input[aria-label=${JSON.stringify(`Write in ${space.name}`)}]`, `the Write box of ${space.name}`);
    await clickOn('.mcp-grants button[type="submit"]', "Save permissions", { text: "Save permissions" });
    const writeDialog = await confirmation("Allow access to your pages?", "Allow", { captureAs: "mcp-grants-write-dialog" });
    check(writeDialog.ok === true, `the confirmation for write access appeared and Allow closed it${writeDialog.error ? ` (${writeDialog.error})` : ""}`);
    check(/change/i.test(writeDialog.said) && writeDialog.said.includes(space.name), `the dialog asks to change ${space.name} (${writeDialog.said})`);
    await savedNote("write");
    const access2 = await waitUntil(`(() => { const t = document.querySelector(".mcp-client-access")?.textContent.trim(); return t && t.startsWith("Can read and change") ? t : null; })()`, "the client shows write access", 10000);
    check(access2 === `Can read and change: ${space.name}.`, `the client can read and change ${space.name} (${access2})`);

    const created = await bridge.rpc("tools/call", { name: "create_page", arguments: { spaceId: space.id, title: "From MCP", markdown: "- one\n- two" } });
    bridgeSteps.created = toolText(created);
    check(created.result?.isError !== true, `create_page "From MCP" is ok (${toolText(created).slice(0, 200)})`);
    await waitFor(`[role="treeitem"][aria-label="From MCP"]`, 'the page "From MCP" appears in the sidebar tree', 15000);
    const tree = await treeOf(space.id);
    const made = livePages(tree).find((p) => p.title === "From MCP");
    must(made !== undefined, "the core has the page From MCP");
    const md = await pageMarkdown(made.id);
    check(/-\s+one/.test(md) && /-\s+two/.test(md), `the page holds the two bullets (${md.replace(/\n/g, " / ").slice(0, 120)})`);
    writeState({ mcpPageId: made.id });
    const appended = await bridge.rpc("tools/call", { name: "append_to_page", arguments: { pageId: made.id, markdown: "three" } });
    check(appended.result?.isError !== true && /Appended/.test(toolText(appended)), `append_to_page is ok (${toolText(appended)})`);
    const listPages = await bridge.rpc("tools/call", { name: "list_pages", arguments: { spaceId: space.id } });
    check(listPages.result?.isError !== true && toolText(listPages).includes("From MCP"), `list_pages shows From MCP (${toolText(listPages).slice(0, 160).replace(/\n/g, " / ")})`);
    // The bridge answers a wrong key with -32001 (a second bridge with a bad token).
    const badBridge = startBridge(bridgeFound ? bridgePath : join(HERE, "..", "..", "target", "debug", "kb-mcp-bridge.exe"), "kbm_" + "A".repeat(43));
    const badReply = await badBridge.rpc("ping");
    badBridge.stop();
    check(badReply.error?.code === -32001, `a bridge with a wrong key answers -32001 (${JSON.stringify(badReply.error)})`);
  } finally {
    bridge.stop();
  }
  await capture("mcp-bridge");

  // Raw HTTP: what is refused before the core is asked.
  const ping = '{"jsonrpc":"2.0","id":7,"method":"ping"}';
  const json = { "content-type": "application/json" };
  const good = { ...json, authorization: `Bearer ${token}` };
  const noToken = await rawHttp({ body: ping, headers: json });
  check(noToken.status === 401 && /Bearer/i.test(String(noToken.headers?.["www-authenticate"] ?? "")), `no token: 401 with WWW-Authenticate: Bearer (${noToken.status}, ${noToken.headers?.["www-authenticate"]})`);
  const wrong = await rawHttp({ body: ping, headers: { ...json, authorization: `Bearer kbm_${"B".repeat(43)}` } });
  check(wrong.status === 401, `a wrong token: 401 (${wrong.status})`);
  const evil = await rawHttp({ body: ping, headers: { ...good, origin: "http://evil.example" } });
  check(evil.status === 403, `Origin http://evil.example: 403 (${evil.status})`);
  const evilHost = await rawHttp({ body: ping, headers: { ...good, host: `evil.example:${MCP_PORT}` } });
  check(evilHost.status === 403, `Host evil.example: 403 (${evilHost.status})`);
  const localOrigin = await rawHttp({ body: ping, headers: { ...good, origin: "http://localhost:5173" } });
  check(localOrigin.status === 200, `Origin http://localhost:5173 is allowed (${localOrigin.status})`);
  const okPing = await rawHttp({ body: ping, headers: good });
  check(okPing.status === 200 && /json/.test(String(okPing.headers?.["content-type"])) && (okPing.body ?? "").includes('"result"'), `a ping with the token: 200 application/json (${okPing.status ?? okPing.error}, ${(okPing.body ?? "").slice(0, 80)})`);
  const get = await rawHttp({ method: "GET", headers: { authorization: `Bearer ${token}` } });
  check(get.status === 405, `GET /mcp: 405 (${get.status})`);
  const del = await rawHttp({ method: "DELETE", headers: { authorization: `Bearer ${token}` } });
  check(del.status === 405, `DELETE /mcp: 405 (${del.status})`);
  const other = await rawHttp({ path: "/other", body: ping, headers: good });
  check(other.status === 404, `POST /other: 404 (${other.status})`);
  const note = await rawHttp({ body: '{"jsonrpc":"2.0","method":"notifications/initialized"}', headers: good });
  check(note.status === 202 && note.body === "", `a notification: 202 with no body (${note.status}, "${note.body}")`);
  const big = await rawHttp({ body: `{"jsonrpc":"2.0","id":1,"method":"ping","params":{"pad":"${"x".repeat(1024 * 1024 + 10)}"}}`, headers: good });
  check(big.status === 413 || big.error !== undefined, `a body over 1 MiB: 413 (${big.status ?? big.error})`);
  const text = await rawHttp({ body: ping, headers: { "content-type": "text/plain", authorization: `Bearer ${token}` } });
  info(`a text/plain body: ${text.status ?? text.error} ${(text.body ?? "").slice(0, 100)}`);
  const batch = await rawHttp({ body: `[${ping}]`, headers: good });
  info(`a JSON-RPC batch: ${batch.status ?? batch.error} ${(batch.body ?? "").slice(0, 120)}`);
  check(batch.status === 200 && /-32600/.test(batch.body ?? ""), `a batch is refused with -32600 (${batch.status ?? batch.error}, ${(batch.body ?? "").slice(0, 100)})`);

  // The Activity table in Settings: every call the bridge made is there, ok or not.
  await openSettings();
  await page.evaluate(`document.querySelector(".mcp-audit")?.scrollIntoView({ block: "start" })`);
  const readRows = () =>
    page.evaluate(`[...document.querySelectorAll(".mcp-audit-table tbody tr")].map((tr) => ({ outcome: tr.dataset.outcome, tool: tr.querySelector("td.mono")?.textContent.trim(), client: tr.children[1]?.textContent.trim(), target: tr.children[3]?.textContent.trim() }))`);
  const expectedTools = ["list_spaces", "search", "read_page", "create_page", "append_to_page", "list_pages"];
  let rows = await readRows();
  for (let i = 0; i < 60 && !expectedTools.every((tool) => rows.some((r) => r.tool === tool)); i++) {
    await wait(500);
    rows = await readRows();
  }
  info(`activity: ${JSON.stringify(rows)}`);
  const audit = await ask({ type: "McpAuditList", data: { limit: 100 } });
  info(`the core's audit: ${JSON.stringify(audit.map((a) => ({ tool: a.tool, outcome: a.outcome, summary: a.summary })))}`);
  const uiTools = rows.map((r) => r.tool).sort().join(",");
  const coreTools = audit.map((a) => a.tool).sort().join(",");
  check(expectedTools.every((tool) => audit.some((a) => a.tool === tool)), `the core audited every tool the bridge called (${[...new Set(audit.map((a) => a.tool))].join(", ")})`);
  check(uiTools === coreTools, `the Activity table shows every call of the audit log (table: ${rows.length} rows, audit: ${audit.length} rows)`);
  if (uiTools !== coreTools) {
    bug(`Settings -> MCP -> Activity is stale: after the calls the table has ${rows.length} rows (${uiTools}) while the core's audit log has ${audit.length} (${coreTools}); the trailing McpChanged re-fetch is lost`);
  }
  check(rows.some((r) => r.outcome === "ok" && r.tool === "search") && rows.some((r) => r.outcome === "denied" && r.tool === "create_page") && rows.some((r) => r.outcome === "ok" && r.tool === "create_page"), "the Activity table shows search (ok), create_page (denied) and create_page (ok)");
  check(rows.every((r) => r.client === "Smoke"), "every row names the client");
  check(!rows.some((r) => /Thursday|Alice|budget review moved/i.test(r.target ?? "")), "no target shows page text (a query or a title only)");
  check(!JSON.stringify(audit).includes(token), "the audit log holds no key");
  check(!JSON.stringify(audit).match(/Thursday|spreadsheet draft/), "the audit log holds no page text");
  await capture("mcp");
  result.mcp = { bridgePath, bridgeFound, bridgeSteps: { ...bridgeSteps, tools: bridgeSteps.tools }, http: { noToken: noToken.status, wrong: wrong.status, evilOrigin: evil.status, evilHost: evilHost.status, get: get.status, del: del.status, other: other.status, notification: note.status, big: big.status ?? big.error, textPlain: text.status, batch: batch.status }, activity: rows };
  void state;
}

// ---------------------------------------------------------------------------------------------------------------

const PHASE_FUNCS = {
  mark: phaseMark,
  launch: phaseLaunch,
  seed: phaseSeed,
  settings: phaseSettings,
  consent: phaseConsent,
  guard: phaseGuard,
  index: phaseIndex,
  writer: phaseWriter,
  chat: phaseChat,
  related: phaseRelated,
  transcribe: phaseTranscribe,
  record: phaseRecord,
  "record-prompt": phaseRecordPrompt,
  mcp: phaseMcp,
};

/** The webview's own document: nothing may have replaced it. */
async function intact(when) {
  const state = readState();
  const now = await page.evaluate(`(() => ({ marker: window.__aiMarker ?? null, href: location.href }))()`).catch(() => null);
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
// The origins the app itself talks to: its own pages and assets, the IPC bridge, the media scheme. Ollama (127.0.0.1:11434)
// and the MCP port are NOT among them: the page never talks to either (the shell does the HTTP).
const ALLOWED = /^(?:https?:\/\/(?:tauri\.localhost|ipc\.localhost|kbmedia\.localhost)|tauri:\/\/localhost|ipc:\/\/localhost|kbmedia:\/\/localhost|data:|blob:|about:)/u;
const foreign = report.requests.filter((r) => !ALLOWED.test(r.url));
result.requests = report.requests.length;
result.foreignRequests = foreign;
result.failures = report.failures;
result.cspViolations = violations;
result.console = report.console.filter((c) => c.type !== "log" && c.type !== "info" && c.type !== "debug");
result.seconds = round((Date.now() - started) / 1000);
check(foreign.length === 0, `no request to another host (${foreign.length}${foreign.length > 0 ? ": " + foreign.map((f) => f.url.slice(0, 80)).slice(0, 3).join(" | ") : ""})`);
const ollamaFromPage = report.requests.filter((r) => /:11434/.test(r.url));
check(ollamaFromPage.length === 0, `the page never asked Ollama (127.0.0.1:11434) for anything (${ollamaFromPage.length})`);
check(violations.length === 0, `no content-security-policy violation (${violations.length}${violations.length > 0 ? ": " + violations.map((v) => `${v.directive} ${String(v.blocked).slice(0, 60)}`).slice(0, 3).join(" | ") : ""})`);
// A request the page itself cancelled (a media element that was stopped, an aborted fetch) is not a failure.
const hardFailures = report.failures.filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED");
check(hardFailures.length === 0, `no failed request (${hardFailures.length}${hardFailures.length > 0 ? ": " + hardFailures.map((f) => f.error).slice(0, 3).join(" | ") : ""})`);
const errors = report.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
if (result.allowConsoleErrors) {
  info(`console errors (the finding of this phase, not a failure): ${errors.map((e) => e.text).slice(0, 5).join(" | ")}`);
} else {
  check(errors.length === 0, `no console error (${errors.length}${errors.length > 0 ? ": " + errors.map((e) => e.text).slice(0, 3).join(" | ") : ""})`);
}
if (outFile && phase !== "mark") {
  writeFileSync(outFile, JSON.stringify(result, null, 2));
}
// The report file has every note; the console has what failed (or, with AI_VERBOSE, everything).
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
      ...(process.env.AI_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
page.close();
void copyFileSync;
process.exit(result.ok ? 0 : 1);
