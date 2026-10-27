// The in-page half of the shared-spaces pass (drive-m13-share.ps1, Milestone 13 slice 1): trusted mouse and key input, the DOM, the
// core (through the pages' own IPC), the native dialogs (Win32 helpers of the .ps1), the processes and the logs, over the DevTools
// ports of several app instances at once (the calls of cdp.mjs, one connection per instance). Phases:
//
//   node drive-m13-share.mjs launch   <outfile>  A (9333), B (9334) and C (9335) started on fresh data, each onboarded (workspace + kit)
//   node drive-m13-share.mjs share    <outfile>  A connects (token dev), seeds a space, "Share this space...", the history warning, Share
//   node drive-m13-share.mjs code     <outfile>  "Invite with a code": B joins with no account on the server; six digits; both edit
//   node drive-m13-share.mjs roles    <outfile>  A sets B to Viewer (read-only, a forced write refused) and back to Editor
//   node drive-m13-share.mjs link     <outfile>  "Invite with a link": C started with the link as its argument; preview; accept
//   node drive-m13-share.mjs remove   <outfile>  A removes B: the archive banner; a later page never reaches B; C still syncs
//   node drive-m13-share.mjs leave    <outfile>  the AI switch is off by default; C leaves and A's member list updates
//   node drive-m13-share.mjs reinvite <outfile>  A invites the removed B again by code: B reads what was written while it was out
//   node drive-m13-share.mjs devices  <outfile>  A's second device D, a kit recovery on a fresh install E, attachment sync with deletion
//   node drive-m13-share.mjs restart  <outfile>  every instance started again on its data: the state survives
//   node drive-m13-share.mjs close    <outfile>  A closes the space: a read-only copy for every member, the server refuses a late write, B removes its copy
//   node drive-m13-share.mjs restart-closed <outfile>  the same restart, after the close
//   node drive-m13-share.mjs logs     <outfile>  the logs of the server and of every instance
//   node drive-m13-share.mjs verify   <outfile>  the summary of every phase
//   node drive-m13-share.mjs stop     <outfile>  stops the instances this pass started (by process id)
//
// Every page phase writes what each page asked the network for, what failed, the console and the content-security-policy violations
// it saw, as JSON, and exits 1 when a check fails, when a page asked another host for something (the pages never talk to the server:
// the shell does), when a violation, a failed request or a console error was seen, or when the document of a webview was replaced
// (the marker `window.__m13Marker` is lost by a reload). The invite link of the link invite is held in the memory of its phase; the
// code, the link's secret and the kit words are kept in the state file for the log scan, and never written to a report.

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync } from "node:zlib";

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.M13_OUT_DIR ?? join(HERE, "out");
const PS1 = process.env.M13_PS1 ?? join(HERE, "drive-m13-share.ps1");
const BASE_PORT = Number(process.env.M13_BASE_PORT ?? 9333);
const STATE_FILE = join(OUT, "m13-share-state.json");
const SERVER_URL = "http://127.0.0.1:8787";
const SERVER_TOKEN = "dev";
const NAMES = ["a", "b", "c", "d", "e"];
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const round = (n) => Math.round(n * 10) / 10;
mkdirSync(OUT, { recursive: true });

function readState() {
  return existsSync(STATE_FILE) ? JSON.parse(readFileSync(STATE_FILE, "utf8")) : {};
}
function writeState(patch) {
  writeFileSync(STATE_FILE, JSON.stringify({ ...readState(), ...patch }, null, 2));
}
function patchInstance(name, patch) {
  const state = readState();
  writeState({ instances: { ...(state.instances ?? {}), [name]: { ...(state.instances?.[name] ?? {}), ...patch } } });
}

// What this pass types and shows: none of it may be in a log (names, texts, titles).
const SPACE = "Orchard Collective";
const PAGES = { plan: "Orchard plan", pears: "Pear notes", gallery: "Gallery", old: "Old orchard", harvest: "Harvest log", files: "Cider files" };
const TEXT = {
  plan: "Prune the pear trees before the first frost.",
  pears: "Conference pears keep best in a cool cellar.",
  gallery: "The south wall at noon.",
  old: "Remove the fallen quince tree.",
  fromB: "Boris adds a note about the cider press.",
  fromA: "Alma replies with the bottling dates.",
  fromB2: "Boris is an editor again and keeps writing.",
  harvest: "Apples picked after the removal.",
  afterRemoval: "Alma writes a line Boris must never see.",
  files: "The press manual is attached below.",
  afterRestart: "Alma writes after every window was restarted.",
  fromBAgain: "Boris is back and writes again.",
  lateWrite: "A late line from a device that never heard of the close.",
};
const PEOPLE = { a: "Alma Quillfeather", b: "Boris Thistledown" };
const WORKSPACES = { a: "Quill Alpha", b: "Quill Beta", c: "Quill Gamma", d: "Quill Delta", e: "Quill Epsilon" };
const FILE_NAME = "cider-press-manual.bin";

// ---------------------------------------------------------------------------------------------------------------
// helper steps of the .ps1 (native dialogs, captures, onboarding, launching, killing)
// ---------------------------------------------------------------------------------------------------------------

let helperCount = 0;
/** Runs one of drive-m13-share.ps1's helper steps; its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}, extraEnv = {}) {
  helperCount += 1;
  const argFile = join(OUT, `m13-share-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `m13-share-helper-${process.pid}-${helperCount}.result.json`);
  writeFileSync(argFile, JSON.stringify(params));
  rmSync(resultFile, { force: true });
  // No pipes: the app a helper starts would inherit them, and the helper would not be finished until the app ends. The helper's answer is
  // its result file; a helper that fails before writing one is told by its exit code.
  return new Promise((resolvePromise, reject) => {
    const child = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", PS1, "-Step", step, "-Arg", argFile, "-Result", resultFile], {
      stdio: "ignore",
      env: { ...process.env, ...extraEnv },
      windowsHide: true,
    });
    const timer = setTimeout(() => child.kill(), 900000);
    child.on("error", (error) => {
      clearTimeout(timer);
      rmSync(argFile, { force: true });
      reject(new Error(`helper ${step} did not start: ${error.message}`));
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      rmSync(argFile, { force: true });
      if (!existsSync(resultFile)) {
        reject(new Error(`helper ${step} wrote no result (exit code ${code})`));
        return;
      }
      const answer = JSON.parse(readFileSync(resultFile, "utf8").replace(/^﻿/, ""));
      rmSync(resultFile, { force: true });
      resolvePromise(answer);
    });
  });
}

// ---------------------------------------------------------------------------------------------------------------
// stop: the instances this pass started
// ---------------------------------------------------------------------------------------------------------------

async function stopAll() {
  const state = readState();
  const pids = Object.values(state.instances ?? {})
    .map((i) => i.pid)
    .filter((pid) => typeof pid === "number");
  if (pids.length === 0) return [];
  const answer = await helper("kill", { pids }).catch(() => ({ stopped: [] }));
  for (const name of Object.keys(state.instances ?? {})) patchInstance(name, { pid: null });
  return answer.stopped ?? [];
}

if (phase === "stop") {
  const stopped = await stopAll();
  console.log(JSON.stringify({ phase, stopped }));
  process.exit(0);
}

// ---------------------------------------------------------------------------------------------------------------
// logs: the scan of the logs of the server and of every instance
// ---------------------------------------------------------------------------------------------------------------

const stripAnsi = (text) => text.replace(/\u001b\[[0-9;]*m/g, "");
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
/** The log files of the pass: every instance's (`m13-share-a.err.log`, `-r1`, and the stdout files) and the server's. */
const logFiles = () => [
  ...readdirSync(OUT).filter((n) => /^m13-share-[a-e](-r\d+)?\.(err|out)\.log$/.test(n)).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })),
  ...readdirSync(OUT).filter((n) => /^m13-share-server-.*\.log$/.test(n)),
  ...["server.log", "server.err.log"].filter((n) => existsSync(join(OUT, n))),
];

function needleList() {
  const state = readState();
  const named = [
    SPACE,
    ...Object.values(PAGES),
    ...Object.values(PEOPLE),
    ...Object.values(WORKSPACES),
    "Quillfeather",
    "Thistledown",
    "Alma",
    "Boris",
    FILE_NAME,
    ...Object.values(TEXT).flatMap((text) => [text, text.split(" ").slice(0, 4).join(" ")]),
  ];
  const secret = [];
  if (state.needles?.code) secret.push({ label: "the invite code", value: state.needles.code }, { label: "the invite code (grouped)", value: state.needles.code.replace(/(.{4})(?=.)/g, "$1 ") });
  if (state.needles?.code2) secret.push({ label: "the second invite code", value: state.needles.code2 });
  if (state.needles?.linkSecret) secret.push({ label: "the link's invite secret", value: state.needles.linkSecret });
  if (state.needles?.pairingCode) secret.push({ label: "the device pairing code", value: state.needles.pairingCode });
  secret.push({ label: "the invite link scheme", value: "atomik-kb://invite" });
  const kitRuns = [];
  for (const [owner, words] of Object.entries(state.needles?.kit ?? {})) {
    for (let i = 0; i + 3 <= words.length; i += 3) kitRuns.push({ label: `kit words of ${owner.toUpperCase()} (${i}..${i + 2})`, value: words.slice(i, i + 3).join(" ") });
  }
  return { named: [...new Set(named)], secret, kitRuns };
}

const ID_PATTERNS = [
  { label: "a hex string of 12 or more digits (a full id, hash or key)", re: /(?<![0-9A-Za-z])(?=[0-9a-f]*[a-f])(?=[0-9a-f]*[0-9])[0-9a-f]{12,}(?![0-9A-Za-z])/i },
  { label: "a UUID (a full page, space or device id)", re: /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i },
  { label: "base64 text of 43 or more characters (key bytes)", re: /(?<![A-Za-z0-9+/_=-])[A-Za-z0-9+/_-]{43,}={0,2}(?![A-Za-z0-9+/_=-])/ },
];

async function scanLogs() {
  const files = logFiles();
  const { named, secret, kitRuns } = needleList();
  const log = { files: [], lines: 0, panics: [], errors: [], warnings: [], leaks: [], needles: named.length + secret.length + kitRuns.length, idHits: [], shortIds: 0 };
  for (const file of files) {
    const text = stripAnsi(readFileSync(join(OUT, file), "utf8"));
    const lines = text.split(/\r?\n/);
    log.files.push({ file, lines: lines.length, bytes: Buffer.byteLength(text) });
    log.lines += lines.length;
    for (const line of lines) {
      if (/panicked|panic:|thread '.*' panicked/i.test(line)) log.panics.push(`${file}: ${line.slice(0, 300)}`);
      if (/(^|\s)ERROR(\s|$)/.test(line)) log.errors.push(`${file}: ${line.slice(0, 300)}`);
      if (/(^|\s)WARN(\s|$)/.test(line)) log.warnings.push(`${file}: ${line.slice(0, 300)}`);
    }
    for (const needle of named) {
      if (!text.includes(needle)) continue;
      const pattern = new RegExp(`(?<![A-Za-z0-9_])${escapeRe(needle)}(?![A-Za-z0-9_])`);
      const hits = lines.filter((l) => pattern.test(l));
      if (hits.length > 0) log.leaks.push({ needle, kind: "named", file, count: hits.length, first: hits[0].slice(0, 200) });
    }
    for (const { label, value } of [...secret, ...kitRuns]) {
      if (!text.includes(value)) continue;
      const count = lines.filter((l) => l.includes(value)).length;
      // The report names what leaked, never the value.
      log.leaks.push({ needle: `(${label})`, kind: "secret", file, count });
    }
    // The name of a pairing mailbox (6 bytes, 12 hex digits) is a rendezvous name, not an id: kb-server's own test pins that its
    // path is logged whole (`/v2/pair/<mailbox>`); every other path segment of 32 or more hex digits is cut by the server.
    const forIds = lines.map((l) => l.replace(/\/v2\/pair\/[0-9a-f]{12}(?![0-9a-f])/g, "/v2/pair/<mailbox>"));
    for (const { label, re } of ID_PATTERNS) {
      const hits = forIds.filter((l) => re.test(l));
      if (hits.length > 0) log.idHits.push({ kind: label, file, count: hits.length, first: hits[0].slice(0, 220) });
    }
    log.shortIds += lines.filter((l) => /(?<![0-9A-Za-z])[0-9a-f]{8}(?![0-9A-Za-z])/i.test(l)).length;
  }
  return log;
}

if (phase === "logs") {
  const log = await scanLogs();
  const ok = log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0 && log.idHits.length === 0;
  const result = { phase: "logs", ok, notes: [], log };
  result.notes.push(`${log.files.length > 0 ? "ok" : "FAIL"}: the logs are there (${log.files.map((f) => `${f.file} ${f.lines} lines`).join(", ")})`);
  result.notes.push(`${log.panics.length === 0 ? "ok" : "FAIL"}: no panic (${log.panics.length})`);
  result.notes.push(`${log.errors.length === 0 ? "ok" : "FAIL"}: no ERROR line (${log.errors.length}${log.errors.length ? `: ${log.errors.slice(0, 3).join(" | ")}` : ""})`);
  result.notes.push(`${log.leaks.length === 0 ? "ok" : "FAIL"}: none of the page texts, titles, space names, display names, workspace names, invite code, link, invite secret or kit words is in a log (${log.leaks.length} leaks${log.leaks.length ? `: ${log.leaks.map((l) => `${l.needle} x${l.count} in ${l.file}`).join(", ")}` : ""}; ${log.needles} needles)`);
  result.notes.push(`${log.idHits.length === 0 ? "ok" : "FAIL"}: no full id, hash or key bytes in a log, only 8-hex ids (${log.idHits.length} kinds of hit${log.idHits.length ? `: ${log.idHits.map((h) => `${h.kind} x${h.count} in ${h.file}`).join(", ")}` : ""})`);
  const control =
    /(?<![A-Za-z0-9_])Orchard plan(?![A-Za-z0-9_])/.test("INFO a made-up line that says Orchard plan in it") &&
    !/(?<![A-Za-z0-9_])Alma(?![A-Za-z0-9_])/.test("INFO a line about Almanac") &&
    ID_PATTERNS[0].re.test("INFO feed=0123456789abcdef0123") &&
    !ID_PATTERNS[0].re.test("INFO feed=0123abcd and 1700000000000");
  result.notes.push(`${control ? "ok" : "FAIL"}: control: the matchers find a needle and a long id in made-up lines and not a whole-word miss, an 8-hex id or a timestamp`);
  result.ok = result.ok && control;
  result.notes.push(`info: ${log.warnings.length} WARN lines; ${log.shortIds} lines hold an 8-hex id`);
  for (const w of log.warnings.slice(0, 30)) result.notes.push(`info: WARN ${w.slice(0, 240)}`);
  if (outFile) writeFileSync(outFile, JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ phase, ok: result.ok, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, idHits: log.idHits, warnings: log.warnings.slice(0, 40), files: log.files }, null, 2));
  process.exit(result.ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// verify: the summary of the phase reports (no app needed)
// ---------------------------------------------------------------------------------------------------------------

const PHASES = ["launch", "share", "code", "roles", "link", "remove", "leave", "reinvite", "devices", "restart", "close", "restart-closed", "logs"];
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], notVerifiable: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0, bugs: [], details: {}, log: null };
  for (const name of PHASES) {
    const file = join(OUT, `m13-share-${name}.json`);
    if (!existsSync(file)) {
      summary.phases[name] = { ok: false, missing: true };
      summary.ok = false;
      continue;
    }
    const report = JSON.parse(readFileSync(file, "utf8"));
    summary.phases[name] = { ok: report.ok, checks: report.notes.filter((n) => !n.startsWith("info:")).length, failed: report.notes.filter((n) => n.startsWith("FAIL")).length, notVerifiable: report.notes.filter((n) => n.startsWith("NOT VERIFIABLE")).length, seconds: report.seconds };
    summary.captures.push(...(report.captures ?? []));
    if (report.details !== undefined) summary.details[name] = report.details;
    summary.failedNotes.push(...report.notes.filter((n) => n.startsWith("FAIL")).map((n) => `${name}: ${n}`));
    summary.notVerifiable.push(...report.notes.filter((n) => n.startsWith("NOT VERIFIABLE")).map((n) => `${name}: ${n}`));
    summary.bugs.push(...(report.bugs ?? []).map((b) => `${name}: ${b}`));
    for (const per of Object.values(report.instances ?? {})) {
      summary.foreignRequests += per.foreignRequests?.length ?? 0;
      summary.cspViolations += per.cspViolations?.length ?? 0;
      summary.failedRequests += (per.failures ?? []).filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED").length;
      summary.consoleErrors += (per.console ?? []).filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error")).length;
    }
  }
  summary.log = await scanLogs();
  const log = summary.log;
  summary.ok = Object.values(summary.phases).every((p) => p.ok) && log.files.length > 0 && log.panics.length === 0 && log.errors.length === 0 && log.leaks.length === 0 && log.idHits.length === 0;
  writeFileSync(outFile, JSON.stringify(summary, null, 2));
  console.log(
    JSON.stringify(
      {
        phase: "verify",
        ok: summary.ok,
        phases: summary.phases,
        failedNotes: summary.failedNotes,
        notVerifiable: summary.notVerifiable,
        bugs: summary.bugs,
        foreignRequests: summary.foreignRequests,
        cspViolations: summary.cspViolations,
        failedRequests: summary.failedRequests,
        consoleErrors: summary.consoleErrors,
        log: { files: log.files, panics: log.panics, errors: log.errors.slice(0, 20), leaks: log.leaks, idHits: log.idHits, warnings: log.warnings.slice(0, 40) },
      },
      null,
      2,
    ),
  );
  process.exit(summary.ok ? 0 : 1);
}

// ---------------------------------------------------------------------------------------------------------------
// the pages: one DevTools connection per instance
// ---------------------------------------------------------------------------------------------------------------

const started = Date.now();
const result = { phase, ok: true, notes: [], captures: [], bugs: [], instances: {} };
class PhaseStop extends Error {}

function check(condition, note) {
  result.notes.push(`${condition ? "ok" : "FAIL"}: ${note}`);
  if (!condition) result.ok = false;
  return condition;
}
/** A check the rest of the phase cannot go on without: the phase stops here when it fails. */
function must(condition, note) {
  if (!check(condition, note)) throw new PhaseStop(note);
}
const info = (note) => result.notes.push(`info: ${note}`);
/** Something that cannot be verified here (a locked Windows session, say): listed, never counted as passed or failed. */
const notVerifiable = (note) => result.notes.push(`NOT VERIFIABLE: ${note}`);
/** A thing the app does that it should not (kept for the final report, besides the failed check that found it). */
const bug = (note) => result.bugs.push(note);

const BUTTON_BIT = { left: 1, right: 2, middle: 4 };
const MOD = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
const norm = (s) => String(s ?? "").replace(/\s+/g, " ").trim();

/** The functions the page-side snippets use, defined once and prepended (they are real functions here, so a typo shows up in node). */
const prelude = {
  norm: (s) => String(s ?? "").replace(/\s+/g, " ").trim(),
  vis: (e) => {
    if (!e || !e.getBoundingClientRect) return false;
    const r = e.getBoundingClientRect();
    const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== "hidden" && s.display !== "none";
  },
  dialog: () => [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')].filter((e) => prelude_vis(e)).pop() ?? null,
  byText: (sel, text, exact = true, root = document) =>
    [...(root ?? document).querySelectorAll(sel)].find((e) => prelude_vis(e) && (exact ? prelude_norm(e.textContent) === text : prelude_norm(e.textContent).includes(text))) ?? null,
  byLabel: (text, root = document) => {
    const label = [...(root ?? document).querySelectorAll("label")].find((e) => prelude_vis(e) && prelude_norm(e.textContent) === text);
    return label ? document.getElementById(label.htmlFor) : null;
  },
};
// The snippets name them without a prefix: this builds `const norm = ..., vis = ..., ...` with the references rewritten.
const PRE = Object.entries(prelude)
  .map(([name, fn]) => `const ${name} = ${fn.toString().replace(/prelude_vis/g, "vis").replace(/prelude_norm/g, "norm")};`)
  .join("\n");

/** One app instance: a DevTools connection (trusted input, evaluation, a record of the network, the console, the violations). */
class Inst {
  constructor(name) {
    this.name = name;
    this.tag = name.toUpperCase();
    this.port = BASE_PORT + NAMES.indexOf(name);
    this.events = [];
    this.id = 0;
    this.waiting = new Map();
  }

  get rec() {
    return readState().instances?.[this.name] ?? {};
  }

  async connect(timeoutMs = 120000) {
    // Enabling the console replays what the page printed before: only what comes after this moment is this phase's.
    this.connectedAt = Date.now();
    const accept = (url) => url.startsWith("http://tauri.localhost") || url.startsWith("https://tauri.localhost") || url.startsWith("tauri://localhost");
    let target;
    const end = Date.now() + timeoutMs;
    while (Date.now() < end && target === undefined) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${this.port}/json`)).json();
        target = list.find((t) => t.type === "page" && accept(t.url));
      } catch {
        // not up yet
      }
      if (target === undefined) await wait(1000);
    }
    if (target === undefined) throw new PhaseStop(`no page target of ${this.tag} on the DevTools port ${this.port}`);
    this.target = target;
    this.ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      this.ws.addEventListener("open", resolve, { once: true });
      this.ws.addEventListener("error", reject, { once: true });
    });
    this.ws.addEventListener("message", (message) => {
      const data = JSON.parse(String(message.data));
      if (data.id !== undefined) {
        const entry = this.waiting.get(data.id);
        this.waiting.delete(data.id);
        if (data.error) entry?.reject(new Error(`${data.error.message}`));
        else entry?.resolve(data.result);
      } else {
        this.events.push(data);
      }
    });
    await this.send("Network.enable");
    await this.send("Log.enable");
    await this.send("Runtime.enable");
    await this.send("Page.enable");
    // The window of a freshly started binary is often behind the terminal that started it: without this the page sees a blur and
    // fields that commit on blur close at once. The page behaves as if its window had the focus.
    await this.send("Emulation.setFocusEmulationEnabled", { enabled: true }).catch(() => {});
    await this.evaluate(`(() => {
      window.__cspViolations = window.__cspViolations ?? [];
      if (!window.__cspHooked) { window.__cspHooked = true; document.addEventListener("securitypolicyviolation", (e) => { window.__cspViolations.push({ directive: e.violatedDirective, blocked: e.blockedURI, source: e.sourceFile }); }); }
    })()`);
    return this;
  }

  send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const n = ++this.id;
      this.waiting.set(n, { resolve, reject });
      this.ws.send(JSON.stringify({ id: n, method, params }));
    });
  }

  close() {
    try {
      this.ws?.close();
    } catch {
      // already closed
    }
  }

  /** Evaluates an expression in the page with the prelude (`norm`, `vis`, `dialog`, `byText`, `byLabel`) in scope; a promise is awaited. */
  async evaluate(source) {
    const expression = `(() => { ${PRE}
return (${source}
); })()`;
    const answer = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (answer.exceptionDetails) throw new Error(answer.exceptionDetails.exception?.description ?? answer.exceptionDetails.text);
    return answer.result.value;
  }

  /** Evaluates a snippet with the prelude (`norm`, `vis`, `dialog`, `byText`, `byLabel`) in scope; `body` returns a value. */
  run(body) {
    return this.evaluate(`(() => { ${body}\n})()`);
  }

  async key(key, code = key, windowsVirtualKeyCode = 0, modifiers = 0) {
    await this.send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode, modifiers });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode, modifiers });
  }

  /** Types text as key events (what a keyboard produces). */
  async type(text) {
    for (const ch of text) {
      await this.send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
      await this.send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
    }
  }

  async insertText(text) {
    await this.send("Input.insertText", { text });
  }

  async mouse(x, y, { button = "left", clickCount = 1, modifiers = 0 } = {}) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, modifiers });
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, buttons: BUTTON_BIT[button], clickCount, modifiers });
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, buttons: 0, clickCount, modifiers });
    await wait(250);
  }

  async hover(x, y) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await wait(300);
  }

  /** The box of the element a JS expression (with the prelude in scope) gives, in page coordinates, or null. */
  box(expr, scroll = true) {
    return this.run(`const el = (${expr});
      if (!el) return null;
      if (${scroll}) el.scrollIntoView({ block: "nearest", inline: "nearest" });
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2, disabled: el.disabled === true };`);
  }

  async waitFor(expr, what, ms = 20000) {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      const found = await this.box(expr).catch(() => null);
      if (found !== null && found.w > 0 && found.h > 0) return found;
      await wait(200);
    }
    must(false, `[${this.tag}] ${what} did not appear (${ms / 1000} s)`);
    return null;
  }

  /** Waits until a JS expression (prelude in scope) is truthy; returns its last value. */
  async waitUntil(expr, what, ms = 20000) {
    const end = Date.now() + ms;
    let last = null;
    while (Date.now() < end) {
      last = await this.run(`return (${expr});`).catch(() => null);
      if (last) return last;
      await wait(250);
    }
    must(false, `[${this.tag}] ${what} (timed out after ${round(ms / 1000)} s)`);
    return last;
  }

  /** A trusted click on the centre of an element. */
  async clickEl(expr, what, options = {}) {
    await this.waitFor(expr, what, options.ms ?? 20000);
    await wait(100);
    const settled = await this.box(expr);
    must(settled !== null, `[${this.tag}] ${what} is still there`);
    if (settled.disabled && options.allowDisabled !== true) must(false, `[${this.tag}] ${what} is enabled`);
    await this.mouse(settled.cx, settled.cy, options);
    return settled;
  }

  /** Focuses a field with a click and puts `text` in it (replacing what it held). */
  async fill(expr, what, text) {
    await this.clickEl(expr, what);
    await this.run(`const el = (${expr}); el.select?.();`);
    await this.insertText(text);
    await wait(150);
    const value = await this.run(`return (${expr})?.value ?? null;`);
    must(value === text, `[${this.tag}] ${what} holds what was typed`);
  }

  /** One request to the core as the UI sends it (`invoke("dispatch", { request })`): `{ ok, value }` or `{ ok: false, error }`. */
  async core(request) {
    if (request.data !== undefined && Object.keys(request.data).length === 0) request = { type: request.type };
    return this.evaluate(`(async () => {
      try {
        const value = await window.__TAURI_INTERNALS__.invoke("dispatch", { request: ${JSON.stringify(request)} });
        return { ok: true, value };
      } catch (e) {
        return { ok: false, error: e instanceof Error ? { message: e.message } : e };
      }
    })()`);
  }

  /** The data of a successful answer; the phase stops when the core refused. */
  async ask(request) {
    const answer = await this.core(request);
    if (!answer.ok) must(false, `[${this.tag}] ${request.type} answered (${JSON.stringify(answer.error ?? "").slice(0, 300)})`);
    return answer.value.data;
  }

  /** The record of the page so far: requests, failures, console, violations. */
  async report() {
    const requests = [];
    const failures = [];
    const console_ = [];
    for (const e of this.events) {
      const stamp = e.params?.timestamp ?? e.params?.entry?.timestamp;
      if (typeof stamp === "number" && stamp > 1e12 && stamp < this.connectedAt - 200) continue; // replayed by enabling the console
      if (e.method === "Network.requestWillBeSent") requests.push({ url: e.params.request.url, type: e.params.type });
      else if (e.method === "Network.loadingFailed") failures.push({ requestId: e.params.requestId, error: e.params.errorText, blocked: e.params.blockedReason ?? null, canceled: e.params.canceled ?? false });
      else if (e.method === "Runtime.consoleAPICalled") console_.push({ type: e.params.type, text: e.params.args.map((a) => a.value ?? a.description ?? "").join(" ").slice(0, 300) });
      else if (e.method === "Log.entryAdded") console_.push({ type: `log:${e.params.entry.level}:${e.params.entry.source}`, text: String(e.params.entry.text).slice(0, 300), url: e.params.entry.url });
      else if (e.method === "Runtime.exceptionThrown") console_.push({ type: "exception", text: String(e.params.exceptionDetails.exception?.description ?? e.params.exceptionDetails.text).slice(0, 300) });
    }
    const violations = await this.evaluate("window.__cspViolations ?? []").catch(() => []);
    return { requests, failures, console: console_, violations };
  }

  /** Captures the window with PrintWindow (capture helper of the .ps1). */
  async capture(label) {
    const path = join(OUT, `m13-share-${label}.png`);
    const pid = this.rec.pid;
    if (typeof pid !== "number") return;
    const answer = await helper("capture", { pid, path }).catch((e) => ({ ok: false, error: e.message }));
    if (answer.ok) result.captures.push(path);
    else info(`capture ${label} of ${this.tag} failed: ${answer.error ?? "no file"}`);
  }
}

const insts = {};
/** The instance of a name, connected (once per phase). */
async function inst(name) {
  if (insts[name] === undefined) {
    const i = new Inst(name);
    await i.connect();
    insts[name] = i;
  }
  return insts[name];
}

// ---------------------------------------------------------------------------------------------------------------
// expression builders for the page-side snippets
// ---------------------------------------------------------------------------------------------------------------

/** `document.querySelector(css)`, the first match. */
const Q = (css) => `document.querySelector(${JSON.stringify(css)})`;
/** The visible element matching `css` whose text is exactly `text` (or contains it with `exact` false), anywhere. */
const T = (css, text, exact = true) => `byText(${JSON.stringify(css)}, ${JSON.stringify(text)}, ${exact})`;
/** The same inside the topmost dialog. */
const TD = (css, text, exact = true) => `byText(${JSON.stringify(css)}, ${JSON.stringify(text)}, ${exact}, dialog())`;
/** The input a visible label names (in the topmost dialog when `inDialog`). */
const L = (text, inDialog = false) => `byLabel(${JSON.stringify(text)}, ${inDialog ? "dialog()" : "document"})`;
/** The text of the topmost dialog (or of the page when none is open). */
const DIALOG_TEXT = `norm((dialog() ?? document.body).innerText)`;

// ---------------------------------------------------------------------------------------------------------------
// processes
// ---------------------------------------------------------------------------------------------------------------

const RUST_LOG = "info,loro_internal=warn,atomik_kb_desktop_lib=debug,kb_core=debug,kb_net=debug,kb_sync=debug,kb_keys=debug,kb_storage=info";
const dirsOf = (name) => ({ data: join(OUT, `m13-share-${name}-data`), webview: join(OUT, `m13-share-${name}-webview`) });

/** Starts an instance on its own data directory and WebView2 profile. `fresh` wipes both first; `link` is the invite link argument. */
async function launchInstance(name, { fresh = false, link = null, singleInstance = false } = {}) {
  const rec = readState().instances?.[name] ?? {};
  if (typeof rec.pid === "number") {
    await helper("kill", { pids: [rec.pid] }).catch(() => {});
    patchInstance(name, { pid: null });
    await wait(3000);
  }
  const { data, webview } = dirsOf(name);
  if (fresh) {
    rmSync(data, { recursive: true, force: true });
    rmSync(webview, { recursive: true, force: true });
  }
  const launches = fresh ? 0 : (rec.launches ?? 0);
  const stem = launches === 0 ? `m13-share-${name}` : `m13-share-${name}-r${launches}`;
  const port = BASE_PORT + NAMES.indexOf(name);
  const answer = await helper("spawn", { name, data, webview, port, log: stem, rustLog: RUST_LOG, singleInstance, useLink: link !== null }, link === null ? {} : { M13_LINK: link });
  must(answer.ok === true, `${name.toUpperCase()} started (${answer.error ?? "pid " + answer.pid})`);
  patchInstance(name, { name, pid: answer.pid, port, data, webview, launches: launches + 1, log: stem, marker: null, href: null });
  if (insts[name] !== undefined) {
    insts[name].close();
    delete insts[name];
  }
  info(`${name.toUpperCase()} started: pid ${answer.pid}, DevTools port ${port}, log ${stem}.err.log`);
  return answer;
}

async function stopInstance(name) {
  const rec = readState().instances?.[name] ?? {};
  if (typeof rec.pid === "number") await helper("kill", { pids: [rec.pid] }).catch(() => {});
  patchInstance(name, { pid: null });
  if (insts[name] !== undefined) {
    insts[name].close();
    delete insts[name];
  }
  await wait(3000);
}

/** Sets the marker of an instance's document (after its workspace is up, or after a restart). */
async function markDocument(i) {
  const marker = `m13-${i.name}-${Math.random().toString(36).slice(2)}-${Date.now()}`;
  const href = await i.evaluate(`(() => { window.__m13Marker = ${JSON.stringify(marker)}; return location.href; })()`);
  patchInstance(i.name, { marker, href });
}

/** The webview's own document: nothing may have replaced it. */
async function intact(i, when) {
  const rec = i.rec;
  const now = await i.evaluate(`(() => ({ marker: window.__m13Marker ?? null, href: location.href }))()`).catch(() => null);
  check(now !== null, `[${i.tag}] ${when}: the window answers`);
  if (now === null) return;
  check(rec.marker !== null && rec.marker !== undefined && now.marker === rec.marker, `[${i.tag}] ${when}: the document was not replaced (marker ${now.marker === rec.marker ? "kept" : "LOST"})`);
  check(now.href === rec.href, `[${i.tag}] ${when}: the address is unchanged`);
}

// ---------------------------------------------------------------------------------------------------------------
// reading the core
// ---------------------------------------------------------------------------------------------------------------

const spaceList = (i) => i.ask({ type: "SpaceList", data: {} });
const treeOf = (i, spaceId) => i.ask({ type: "TreeGet", data: { spaceId } });
const syncStatus = (i) => i.ask({ type: "SyncStatus" });
const membersOf = (i, spaceId) => i.ask({ type: "SpaceMembers", data: { spaceId } });
const syncNow = (i) => i.core({ type: "SyncNow" }).catch(() => null);
/** Every live page of a tree, depth first, with its path of titles. */
function flatten(nodes, trail = []) {
  const out = [];
  for (const node of nodes) {
    const path = [...trail, node.page.title];
    out.push({ id: node.page.id, title: node.page.title, path, depth: trail.length, parentId: node.page.parentId ?? null, children: node.children.length });
    out.push(...flatten(node.children, path));
  }
  return out;
}
const livePages = (tree) => flatten(tree.roots);
/** The shape of a tree, comparable across devices (titles with their paths, sorted, and the trashed titles). */
const shapeOf = (tree) => ({ live: livePages(tree).map((p) => p.path.join(" > ")).sort(), trashed: tree.trashed.map((t) => (t.page ?? t).title).sort() });
const sameShape = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const sharedSpaceOf = async (i) => (await spaceList(i)).find((s) => s.name === SPACE && (s.shareState === "Shared" || s.shareState === "Archived"));

/** The text of a ProseMirror document, walked: what the page says. */
function textOf(node) {
  let text = "";
  const walk = (n) => {
    if (typeof n.text === "string") text += `${n.text} `;
    for (const child of n.content ?? []) walk(child);
  };
  walk(node);
  return text.replace(/\s+/g, " ").trim();
}
const pageText = async (i, pageId) => textOf((await i.ask({ type: "PageContentGet", data: { pageId } })).content);
const pageIdByTitle = async (i, spaceId, title) => livePages(await treeOf(i, spaceId)).find((p) => p.title === title)?.id ?? null;

/** Polls `predicate` (async, true when satisfied), asking the instances in `kick` to sync now every few seconds. */
async function eventually(predicate, what, { ms = 120000, kick = [] } = {}) {
  const end = Date.now() + ms;
  let lastKick = 0;
  while (Date.now() < end) {
    try {
      if (await predicate()) return true;
    } catch {
      // not yet
    }
    if (Date.now() - lastKick > 4000) {
      lastKick = Date.now();
      for (const k of kick) await syncNow(k);
    }
    await wait(1000);
  }
  must(false, `${what} (timed out after ${round(ms / 1000)} s)`);
  return false;
}

// ---------------------------------------------------------------------------------------------------------------
// the UI flows both sides use
// ---------------------------------------------------------------------------------------------------------------

/** Leaves nothing open from an earlier step that failed half-way. */
async function normalize(i) {
  for (let k = 0; k < 5; k++) {
    const open = await i.run(`return !!document.querySelector('[role="menu"], [role="dialog"], [role="alertdialog"], [role="listbox"], .space-popover');`);
    if (!open) return;
    await i.key("Escape", "Escape", 27);
    await wait(250);
  }
}

const heading = (i) => i.evaluate(`document.querySelector(".content-body h1")?.textContent.trim() ?? null`);

/** The menu item whose text starts with `label`, chosen with a click. */
async function chooseMenuItem(i, label) {
  const expr = `[...document.querySelectorAll('[role="menuitem"]')].find((e) => norm(e.textContent).startsWith(${JSON.stringify(label)}) && vis(e))`;
  const found = await i.waitFor(expr, `the menu item "${label}"`, 10000);
  if (found.disabled) must(false, `[${i.tag}] the menu item "${label}" is enabled`);
  await i.mouse(found.cx, found.cy);
}

/** Opens the space menu ("Space options") and chooses `label`. */
async function spaceMenu(i, label) {
  for (let attempt = 1; ; attempt++) {
    await normalize(i);
    await i.clickEl(Q('button[aria-label="Space options"]'), "the space options button");
    const end = Date.now() + (attempt < 3 ? 4000 : 20000);
    let opened = false;
    while (Date.now() < end && !opened) {
      opened = await i.run(`return !!document.querySelector('[role="menu"]');`);
      if (!opened) await wait(200);
    }
    if (!opened && attempt >= 3) must(false, `[${i.tag}] the space options menu did not appear`);
    if (opened) break;
    info(`[${i.tag}] the space options menu did not open at the click (attempt ${attempt}): clicking again`);
  }
  await chooseMenuItem(i, label);
}

const shownSpace = (i) => i.evaluate(`document.querySelector(".space-switcher .space-name")?.textContent.trim() ?? null`);

async function openSpaceList(i) {
  await normalize(i);
  await i.clickEl(Q("button.space-button"), "the space switcher button");
  await i.waitFor(Q(".space-popover"), "the list of spaces");
}

/** Makes `spaceId` the current space through the switcher (the nth entry of the list, by position: names may be private). */
async function switchSpace(i, spaceId) {
  const spaces = await spaceList(i);
  const index = spaces.findIndex((s) => s.id === spaceId);
  must(index >= 0, `[${i.tag}] the space to switch to exists`);
  await openSpaceList(i);
  const items = await i.evaluate(`[...document.querySelectorAll(".space-popover .space-item")].map((b) => { const r = b.getBoundingClientRect(); return { cx: r.x + r.width / 2, cy: r.y + r.height / 2 }; })`);
  must(items.length === spaces.length, `[${i.tag}] the switcher lists every space (${items.length} of ${spaces.length})`);
  await i.mouse(items[index].cx, items[index].cy);
  await i.waitUntil(`!document.querySelector(".space-popover")`, "the space list closed");
  await wait(600);
  await i.waitUntil(`!!document.querySelector('[role="tree"], .tree-new, .tree-empty, .home-empty')`, "the sidebar shows the space's pages", 30000).catch(() => null);
}

/** Creates a space with the switcher's "New space" form; the new space becomes the current one. */
async function createSpaceUi(i, name) {
  await openSpaceList(i);
  await i.clickEl(T(".space-popover button.menu-item", "New space"), `"New space" in the list`);
  await i.waitFor(Q(".space-popover .space-form input"), "the space name field");
  await i.clickEl(Q(".space-popover .space-form input"), "the space name field");
  await i.insertText(name);
  await i.clickEl(Q('.space-popover .space-form button[type="submit"]'), "Create space");
  await i.waitUntil(`!document.querySelector(".space-popover")`, "the new space form closed", 20000);
  await i.waitUntil(`document.querySelector(".space-switcher .space-name")?.textContent.trim() === ${JSON.stringify(name)}`, "the new space is the current one");
  await wait(500);
}

/** The sidebar rows of the ancestors of a page are expanded so that the page's own row is shown. */
async function expandAncestors(i, spaceId, id) {
  await normalize(i);
  const all = livePages(await treeOf(i, spaceId));
  const target = all.find((p) => p.id === id);
  must(target !== undefined, `[${i.tag}] the page is in the tree of its space`);
  const chain = [];
  for (let p = target; p !== undefined; p = all.find((q) => q.id === p.parentId)) chain.unshift(p.id);
  for (const pageId of chain.slice(0, -1)) {
    await i.waitFor(Q(`[role="treeitem"][data-page-id="${pageId}"]`), "the sidebar row of an ancestor", 20000);
    const row = await i.evaluate(`(() => {
      const el = document.querySelector('[role="treeitem"][data-page-id="${pageId}"]');
      const c = el.querySelector("button.tree-chevron")?.getBoundingClientRect() ?? null;
      el.scrollIntoView({ block: "nearest" });
      return { expanded: el.getAttribute("aria-expanded"), chevron: c ? { cx: c.x + c.width / 2, cy: c.y + c.height / 2 } : null };
    })()`);
    if (row.expanded === "false" && row.chevron !== null) {
      await i.mouse(row.chevron.cx, row.chevron.cy);
      await wait(300);
    }
  }
  await i.waitFor(Q(`[role="treeitem"][data-page-id="${id}"] .tree-title`), "the sidebar row of the page", 20000);
}

/** Opens a page by clicking its row in the sidebar (the page's space must be the current one). */
async function openPageId(i, spaceId, id) {
  await expandAncestors(i, spaceId, id);
  const again = await i.box(Q(`[role="treeitem"][data-page-id="${id}"] .tree-title`));
  await i.mouse(again.cx, again.cy);
  await wait(700);
  await i.waitFor(Q(".content-body h1"), "the title of the opened page");
}

/** A new empty line at the end of the page, the caret in it (what a click below the last block does). */
async function newLine(i) {
  await i.evaluate(`(() => {
    const editor = document.querySelector(".ProseMirror").editor;
    const { doc } = editor.state;
    const last = doc.lastChild;
    if (last && last.type.name === "paragraph" && last.content.size === 0) { editor.commands.focus("end"); return; }
    editor.chain().insertContentAt(doc.content.size, { type: "paragraph" }).focus("end").run();
  })()`);
  await wait(200);
}

/** `/query` typed with key events, the entry named `label` chosen (ArrowDown until it is the selected one), Enter. */
async function slash(i, query, label) {
  await i.type(`/${query}`);
  await wait(500);
  await i.waitFor(Q("[role=listbox]"), `the slash menu for /${query}`);
  for (let k = 0; k < 25; k++) {
    const seen = await i.evaluate(`(() => {
      const options = [...document.querySelectorAll("[role=option]")];
      const title = (o) => o.querySelector(".suggestion-title")?.textContent.trim() ?? "";
      return { selected: title(options.find((o) => o.getAttribute("aria-selected") === "true") ?? options[0] ?? document.body), all: options.map(title) };
    })()`);
    if (!seen.all.includes(label)) must(false, `[${i.tag}] the slash menu for /${query} offers "${label}" (${seen.all.join(", ")})`);
    if (seen.selected === label) break;
    await i.key("ArrowDown", "ArrowDown", 40);
    await wait(80);
  }
  await i.key("Enter", "Enter", 13);
  await wait(700);
}

/** Creates a page at the top of the sidebar with the "New page" button and names it. */
async function newPage(i, title) {
  await i.clickEl(Q(".tree-new"), `the sidebar's "New page" button`);
  await i.waitFor(Q("input.tree-rename"), "the rename field of the new page");
  await i.insertText(title);
  await i.key("Enter", "Enter", 13);
  await i.waitFor(Q(`[role="treeitem"][aria-label=${JSON.stringify(title)}]`), `the row "${title}"`);
  await wait(500);
  await i.waitFor(Q(".ProseMirror"), "the page editor");
  await i.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(title)}`, `the new page "${title}" is open`, 20000);
}

/** A page made inside another one with the row's "Add a page inside" button, named and left open. */
async function newChild(i, spaceId, parentId, title) {
  const row = await i.waitFor(Q(`[role="treeitem"][data-page-id="${parentId}"] .tree-title`), "the parent row", 20000);
  await i.hover(row.cx, row.cy);
  const add = await i.waitFor(Q(`[role="treeitem"][data-page-id="${parentId}"] button[aria-label="Add a page inside"]`), 'the "Add a page inside" button');
  await i.mouse(add.cx, add.cy);
  await i.waitFor(Q("input.tree-rename"), "the rename field of the new page");
  await i.insertText(title);
  await i.key("Enter", "Enter", 13);
  await wait(900);
  const made = livePages(await treeOf(i, spaceId)).find((p) => p.title === title);
  must(made !== undefined, `[${i.tag}] the page "${title}" exists`);
  return made.id;
}

/** Types `text` into the editor at a fresh line and leaves the editor, so the text is saved. */
async function writeLine(i, text) {
  await newLine(i);
  await i.insertText(text);
  await i.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
}

const imageLoaded = (i, ms = 30000) => i.waitUntil(`(() => { const e = document.querySelector(".ProseMirror img"); return e && e.complete && e.naturalWidth > 0 ? e.naturalWidth : 0; })()`, `[${i.tag}] the image of the open page loads`, ms);

/** Drives a native file dialog that the page has just asked for: types `text` and closes it. */
async function fileDialog(i, title, text, timeoutMs = 30000) {
  const answer = await helper("dialog", { pid: i.rec.pid, title, steps: [text], timeoutMs });
  info(`dialog "${title}" of ${i.tag}: waited ${answer.waitedMs} ms, edit control ${answer.edit}, ${answer.closed ? "closed" : "STILL OPEN"}`);
  check(answer.ok === true, `[${i.tag}] the "${title}" dialog took the file and closed${answer.error ? ` (${answer.error}; seen ${JSON.stringify(answer.seen)})` : ""}`);
  return answer;
}

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

/** Settings is the route of the sidebar's "Settings" button. */
async function goSettings(i) {
  await normalize(i);
  await i.clickEl(T("nav.sidebar button", "Settings"), "the Settings button");
  await i.waitFor(Q("h1, h2"), "the Settings screen");
  await wait(600);
}

/** The server address (and, when asked, the invite token) of Settings -> Sync, then Connect; waits until the sync status says connected. */
async function connectSync(i, url, token) {
  await goSettings(i);
  // A form left at the token step by an earlier attempt of this phase is picked up where it stands.
  if (!(await i.run(`return !!${L("Invite token")};`))) {
    await i.fill(L("Server address"), "the server address field", url);
    await i.clickEl(T("button", "Connect"), "the Connect button");
  }
  let tokenGiven = false;
  await eventually(
    async () => {
      // The server does not know the account: the form asks for the invite token (once).
      if (!tokenGiven && (await i.run(`return !!${L("Invite token")};`))) {
        tokenGiven = true;
        await i.fill(L("Invite token"), "the invite token field", token);
        await i.clickEl(T("button", "Connect"), "the Connect button of the token step");
      }
      const status = await syncStatus(i);
      return status.configured === true && status.needsInviteToken === false && status.lastPullAt !== undefined && status.state !== "Error";
    },
    `[${i.tag}] sync is connected`,
    { ms: 150000, kick: [i] },
  );
  info(`[${i.tag}] the server asked for the invite token: ${tokenGiven}`);
}

/** Sets a `<select>` by focusing it and typing the start of the option (a trusted key event: the browser's type-ahead), with a fallback. */
async function chooseOption(i, selectExpr, optionText, what) {
  await i.run(`const s = (${selectExpr}); s.scrollIntoView({ block: "nearest" }); s.focus();`);
  await wait(200);
  await i.type(optionText.slice(0, 2));
  await wait(400);
  let chosen = await i.run(`const s = (${selectExpr}); return s ? norm(s.options[s.selectedIndex]?.textContent) : null;`);
  if (chosen !== optionText) {
    info(`${what}: the keyboard type-ahead gave "${chosen}", so the value was set with the browser's own setter and a change event`);
    await i.run(`const s = (${selectExpr});
      const option = [...s.options].find((o) => norm(o.textContent) === ${JSON.stringify(optionText)});
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(s, option.value);
      s.dispatchEvent(new Event("change", { bubbles: true }));`);
    await wait(300);
    chosen = await i.run(`const s = (${selectExpr}); return s ? norm(s.options[s.selectedIndex]?.textContent) : null;`);
  }
  must(chosen === optionText, `[${i.tag}] ${what}: "${optionText}" is chosen`);
}

// ----- the member panel --------------------------------------------------------------------------------------------

/** Opens "Members and sharing..." of the current (shared) space and waits for the list. */
async function openMembers(i) {
  await spaceMenu(i, "Members and sharing");
  await i.waitFor(`byText("h2", "Members and sharing", true)`, "the Members and sharing dialog");
  await i.waitFor(Q(".members-dialog .member-item"), "the member list");
}

const closeDialog = async (i, label = "Close") => {
  await i.clickEl(TD("button", label), `the "${label}" button of the dialog`);
  await wait(400);
};

/** The member rows of the open panel. */
const memberRows = (i) =>
  i.evaluate(`[...document.querySelectorAll(".members-dialog .member-item")].map((li) => ({
    name: li.querySelector(".member-name")?.textContent.trim() ?? "",
    you: (li.textContent ?? "").includes("(you)"),
    verified: [...li.querySelectorAll(".member-head .tag")].some((t) => t.textContent.trim() === "Verified"),
    unverified: [...li.querySelectorAll(".member-head .tag")].some((t) => t.textContent.trim() === "Not verified"),
    role: li.querySelector("select.member-role") ? li.querySelector("select.member-role").options[li.querySelector("select.member-role").selectedIndex]?.textContent.trim() : (li.querySelector(".member-role-tag")?.textContent.trim() ?? ""),
    canManage: !!li.querySelector("select.member-role"),
    meta: li.querySelector(".member-meta")?.textContent.trim() ?? "",
  }))`);

/** Waits until the panel's rows satisfy `predicate`, re-reading as the core's events arrive. */
async function waitMembers(i, what, predicate, ms = 90000, kick = []) {
  let last = [];
  await eventually(
    async () => {
      last = await memberRows(i);
      return predicate(last);
    },
    `[${i.tag}] ${what}`,
    { ms, kick },
  );
  return last;
}

/** The AI switch of the open member panel: `true` when on. */
const aiSwitchOn = (i) => i.evaluate(`document.querySelector('.members-dialog input[role="switch"]')?.checked ?? null`);

/** Reads the six digits, shown in two groups of three, of the topmost dialog or of the screen. */
const digitsOf = (i) => i.evaluate(`(document.querySelector(".pair-digits")?.textContent ?? "").replace(/\\D/g, "") || null`);

// ---------------------------------------------------------------------------------------------------------------
// phase: launch
// ---------------------------------------------------------------------------------------------------------------

const kitFileOf = (name) => join(OUT, `m13-share-kit-${name}.txt`);

/** The workspace and its kit, by UI Automation (the helper of the .ps1); the kit words are kept for the recovery and the log scan. */
async function onboard(name) {
  const rec = readState().instances?.[name] ?? {};
  const answer = await helper("onboard", { pid: rec.pid, workspace: WORKSPACES[name], kitFile: kitFileOf(name) });
  check(answer.ok === true, `${name.toUpperCase()} onboarded by UI Automation (a workspace and a recovery kit${answer.error ? `: ${answer.error}` : ""})`);
  if (answer.ok !== true) throw new PhaseStop(`onboarding of ${name}`);
  const words = readFileSync(kitFileOf(name), "utf8").trim().split(/\s+/);
  check(words.length === 24, `${name.toUpperCase()}'s kit has 24 words`);
  const state = readState();
  writeState({ needles: { ...(state.needles ?? {}), kit: { ...(state.needles?.kit ?? {}), [name]: words } } });
}

async function phaseLaunch() {
  rmSync(STATE_FILE, { force: true }); // a fresh pass starts with no memory of an earlier one
  writeState({ started: new Date().toISOString(), instances: {}, needles: { kit: {} } });
  const session = await helper("session");
  info(`the Windows session is ${session.locked ? "LOCKED" : "unlocked"}`);
  writeState({ sessionLocked: session.locked === true });
  if (session.locked) notVerifiable("the Windows session is locked: UI Automation and the real keyboard cannot be relied on");

  const server = await fetch(`${SERVER_URL}/v2/server`).then((r) => r.json());
  check(server.protocolVersion === 2, `the server answers on protocol v2 (${server.protocolVersion})`);
  check(typeof server.serverEpoch === "string" && server.serverEpoch.length > 0, "the server has an epoch (it started fresh)");

  for (const name of ["a", "b", "c"]) await launchInstance(name, { fresh: true });
  await Promise.all(["a", "b", "c"].map((name) => onboard(name)));
  for (const name of ["a", "b", "c"]) {
    const i = await inst(name);
    await i.waitFor(Q(".tab-strip"), "the tab strip", 60000);
    await markDocument(i);
    check((await heading(i)) === WORKSPACES[name], `[${i.tag}] the main area shows the workspace's Home (${await heading(i)})`);
    const spaces = await spaceList(i);
    check(spaces.length === 1, `[${i.tag}] the workspace has one space (${spaces.length})`);
    const sync = await syncStatus(i);
    check(sync.configured === false, `[${i.tag}] sync is not configured yet`);
    check(existsSync(dirsOf(name).data), `[${i.tag}] the data directory is the pass's own (out/m13-share-${name}-data)`);
    await i.capture(`launched-${name}`);
  }
  const a = await spaceList(await inst("a"));
  writeState({ personal: { a: a[0].id } });
  result.details = { ports: Object.fromEntries(["a", "b", "c"].map((n) => [n, BASE_PORT + NAMES.indexOf(n)])), serverProtocol: server.protocolVersion };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: share
// ---------------------------------------------------------------------------------------------------------------

async function phaseShare() {
  const A = await inst("a");
  const dir = join(OUT, "m13-share-src");
  mkdirSync(dir, { recursive: true });
  const png = join(dir, "orchard.png");
  writeFileSync(png, pngBytes(40));

  // A connects to the server with the token and no profile name yet.
  await connectSync(A, SERVER_URL, SERVER_TOKEN);
  const sync = await syncStatus(A);
  check(sync.configured === true && sync.state !== "Error", `[A] sync is connected to the server (${sync.state})`);
  for (const name of ["b", "c"]) {
    const other = await inst(name);
    check((await syncStatus(other)).configured === false, `[${other.tag}] has no sync configuration: the share signup will make its account`);
  }
  check((await A.ask({ type: "ProfileGet" })).displayName === undefined, "[A] has no profile name yet");

  // A seeds a space: pages, a nested page, an image, a trashed page.
  await createSpaceUi(A, SPACE);
  const space = (await spaceList(A)).find((s) => s.name === SPACE);
  must(space !== undefined, "[A] the space exists");
  writeState({ spaceId: space.id });
  await newPage(A, PAGES.plan);
  await writeLine(A, TEXT.plan);
  const planId = await pageIdByTitle(A, space.id, PAGES.plan);
  must(planId !== null, "[A] the core has the first page");
  const pearsId = await newChild(A, space.id, planId, PAGES.pears);
  await openPageId(A, space.id, pearsId);
  await writeLine(A, TEXT.pears);
  await newPage(A, PAGES.gallery);
  await writeLine(A, TEXT.gallery);
  await newLine(A);
  await slash(A, "image", "Image");
  await fileDialog(A, "Open", png);
  const width = await imageLoaded(A);
  info(`[A] the image block shows the picture, ${width} px wide`);
  await A.evaluate(`document.activeElement?.blur?.()`);
  await wait(1500);
  await newPage(A, PAGES.old);
  await writeLine(A, TEXT.old);
  const oldId = await pageIdByTitle(A, space.id, PAGES.old);
  await A.ask({ type: "PageTrash", data: { pageId: oldId } });
  await wait(800);
  const tree = await treeOf(A, space.id);
  check(sameShape(shapeOf(tree), { live: [`${PAGES.gallery}`, `${PAGES.plan}`, `${PAGES.plan} > ${PAGES.pears}`].sort(), trashed: [PAGES.old] }), "[A] the space holds three live pages (one nested) and a trashed one");
  const attachments = await A.ask({ type: "AttachmentList", data: { spaceId: space.id } });
  check(attachments.length === 1, `[A] the space holds one attachment (${attachments.length})`);

  // "Share this space...": the warning and the counts, read; the profile name; Share.
  const plan = await A.ask({ type: "SpaceSharePlan", data: { spaceId: space.id } });
  check(plan.pageCount === 3 && plan.trashedPageCount === 1 && plan.attachmentCount === 1, `[A] the plan counts 3 pages, 1 trashed page and 1 file (${plan.pageCount}, ${plan.trashedPageCount}, ${plan.attachmentCount})`);
  check(plan.serverConfigured === true && plan.hasAccount === true && plan.hasProfileName === false, "[A] the plan says: sync on, an account, no profile name yet");
  await spaceMenu(A, "Share this space");
  await A.waitFor(T("h2", `Share “${SPACE}”`), "the Share dialog");
  await A.waitFor(Q(".share-counts"), "the counts of what sharing sends");
  const dialog = await A.evaluate(`(() => {
    const d = document.querySelector(".share-dialog");
    return {
      warning: d.querySelector(".share-history")?.innerText ?? "",
      roles: [...d.querySelectorAll(".role-item dt")].map((e) => e.textContent.trim()),
      counts: Object.fromEntries([...d.querySelectorAll(".share-counts dt")].map((dt) => [dt.textContent.trim(), dt.nextElementSibling?.textContent.trim() ?? ""])),
      ai: d.querySelector(".share-ai")?.textContent.trim() ?? "",
      nameField: !!d.querySelector("input.input"),
    };
  })()`);
  info(`[A] the history warning reads: ${norm(dialog.warning).slice(0, 400)}`);
  check(/whole history/i.test(dialog.warning) && /trash/i.test(dialog.warning) && /cannot take them back/i.test(dialog.warning), "[A] the warning says: the whole history, the trash, and that removing a member cannot take copies back");
  check(dialog.roles.join(",") === "Owner,Editor,Commenter,Viewer", `[A] the four roles are listed (${dialog.roles.join(", ")})`);
  check(dialog.counts.Pages === "3" && dialog.counts["Pages in the trash"] === "1", `[A] the dialog counts the pages (${JSON.stringify(dialog.counts)})`);
  check(/local AI/i.test(dialog.ai), "[A] the dialog says that members may turn on local AI on their own devices");
  check(dialog.nameField === true, "[A] the dialog asks for the name other members see");
  await A.capture("share-dialog");
  await A.fill(L("Your name for other members", true), "the profile name field", PEOPLE.a);
  await A.clickEl(TD("button", "Share"), "the Share button");
  await A.waitUntil(`${DIALOG_TEXT}.includes("is shared")`, "the dialog says the space is shared", 120000);
  await wait(500);
  await A.capture("shared");
  await closeDialog(A, "Close");
  const shared = (await spaceList(A)).find((s) => s.id === space.id);
  check(shared.shareState === "Shared" && shared.role === "owner" && shared.memberCount === 1, `[A] the space is shared, A is its Owner, one member (${shared.shareState}, ${shared.role}, ${shared.memberCount})`);
  check(shared.aiEnabled === false, "[A] the shared space's AI switch is off by default on the Owner's device too");
  check((await A.ask({ type: "ProfileGet" })).displayName === PEOPLE.a, "[A] the profile name was saved");
  await eventually(async () => {
    const s = await syncStatus(A);
    const feed = (s.feeds ?? []).find((f) => f.spaceId === space.id);
    return feed !== undefined && feed.pendingEntries === 0 && feed.forked === false;
  }, "[A] the shared feed is uploaded (no pending entries)", { ms: 120000, kick: [A] });
  const feeds = (await syncStatus(A)).feeds ?? [];
  check(feeds.length === 2, `[A] sync reports two feeds: the personal one and the shared one (${feeds.length})`);
  result.details = { pages: 3, trashed: 1, attachments: 1, feeds: feeds.length };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: code
// ---------------------------------------------------------------------------------------------------------------

const bodyText = (i) => i.evaluate(`document.body.innerText`);
function addNeedles(patch) {
  const state = readState();
  writeState({ needles: { ...(state.needles ?? {}), ...patch } });
}

/** Makes the shared space the current one of an instance, when it is not. */
async function inSharedSpace(i, spaceId) {
  const spaces = await spaceList(i);
  const space = spaces.find((s) => s.id === spaceId);
  must(space !== undefined, `[${i.tag}] the shared space is in the list`);
  if ((await shownSpace(i)) !== space.name) await switchSpace(i, spaceId);
  await normalize(i);
}

/** A page of the shared space opened in an instance (the tree row clicked). */
async function openShared(i, spaceId, title) {
  await inSharedSpace(i, spaceId);
  const id = await pageIdByTitle(i, spaceId, title);
  must(id !== null, `[${i.tag}] the page "${title}" is in the tree`);
  await openPageId(i, spaceId, id);
  await i.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(title)}`, `"${title}" is open`);
  return id;
}

/** Both dialogs show that B joined (a run that stopped right after the join). */
async function resumedAfterJoin(A, B) {
  const a = await A.evaluate(`norm(document.querySelector(".invite-dialog")?.innerText ?? "")`).catch(() => "");
  const b = await B.evaluate(`document.body.innerText`).catch(() => "");
  return /joined as Editor/.test(a) && /You joined/.test(b);
}

async function phaseCode() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId } = readState();
  must(typeof spaceId === "string", "the share phase made the space");
  await B.ask({ type: "ProfileSet", data: { displayName: PEOPLE.b } });

  // A run that stopped after the join (the dialogs still open) is resumed from the member panels.
  let rows = [];
  if ((await sharedSpaceOf(B)) === undefined && !(await resumedAfterJoin(A, B))) {
    check((await syncStatus(B)).configured === false, "[B] has no account on the server and no sync configuration before it joins");
    // A: Members and sharing -> "Invite with a code..." (Editor): the code and the server address are shown.
    await inSharedSpace(A, spaceId);
    await openMembers(A);
    rows = await memberRows(A);
    check(rows.length === 1 && rows[0].you && rows[0].role === "Owner", `[A] the member list holds A alone, as Owner (${JSON.stringify(rows.map((r) => [r.name, r.role]))})`);
    check(/No invitations yet/.test(await A.evaluate(`document.querySelector(".members-dialog").innerText`)), "[A] no invitations yet");
    await A.clickEl(T("button", "Invite with a code..."), `the "Invite with a code..." button`);
    await A.waitFor(T("h2", "Invite with a code"), "the invitation dialog");
    const roleShown = await A.run(`const s = ${L("Role of the new member", true)}; return s ? norm(s.options[s.selectedIndex].textContent) : null;`);
    check(roleShown === "Editor", `[A] the role of the new member starts on Editor (${roleShown})`);
    await A.clickEl(TD("button", "Show the code"), "the Show the code button");
    await A.waitFor(Q(".pair-code"), "the invitation code", 60000);
    const code = await A.evaluate(`document.querySelector(".pair-code").textContent.replace(/\\s/g, "")`);
    check(/^[0-9A-Z]{12}$/.test(code), `[A] the code has 12 characters (${code.length})`);
    addNeedles({ code });
    const serverShown = await A.evaluate(`document.querySelector(".invite-server-url")?.textContent.trim() ?? null`);
    check(serverShown === SERVER_URL, `[A] the dialog shows the server address (${serverShown})`);
    await A.capture("code-shown");

    // B: "Join a shared space...": the server and the code typed; B has no account on the server.
    await openSpaceList(B);
    await B.clickEl(T(".space-popover button.menu-item", "Join a shared space..."), `"Join a shared space..." in the list`);
    await B.waitFor(T("h2", "Join a shared space"), "the Join a shared space dialog");
    await B.fill(L("Server address", true), "the server address field", SERVER_URL);
    await B.fill(L("Code", true), "the code field", code);
    await B.clickEl(TD("button", "Continue"), "the Continue button");
    let sawTokenPrompt = false;
    let sawFailure = null;
    const watchB = async () => {
      const text = await bodyText(B);
      if (/invite token/i.test(text)) sawTokenPrompt = true;
      const status = await syncStatus(B).catch(() => null);
      if (status !== null && status.needsInviteToken === true) sawTokenPrompt = true;
      const stopped = /Joining stopped: ([^\n]*)/.exec(text);
      if (stopped) sawFailure = stopped[1];
    };
    let digitsA = null;
    let digitsB = null;
    await eventually(async () => {
      await watchB();
      if (sawFailure !== null) must(false, `[B] joining stopped: ${sawFailure}`);
      digitsA = await digitsOf(A);
      digitsB = await digitsOf(B);
      return digitsA !== null && digitsB !== null;
    }, "both windows show the six digits", { ms: 120000 });
    info(`the digits were compared: A and B ${digitsA === digitsB ? "agree" : "DIFFER"}`);
    check(/^\d{6}$/.test(digitsA ?? ""), "[A] shows six digits");
    check(digitsA === digitsB, "the six digits of A and of B are the same");
    await A.capture("code-digits-a");
    await B.capture("code-digits-b");

    // B confirms first, then A (who sees B's name and safety number).
    await B.clickEl(TD("button", "Confirm"), "B's Confirm button");
    await A.waitUntil(`${DIALOG_TEXT}.includes("You are inviting")`, "A's dialog says who is being invited", 90000);
    const inviting = await A.evaluate(`document.querySelector(".invite-dialog .pair-peer")?.textContent.trim() ?? ""`);
    check(inviting.includes(PEOPLE.b), `[A] the dialog names the person being invited (${inviting})`);
    const safetyOfBSeenByA = await A.evaluate(`[...document.querySelectorAll(".invite-dialog .member-safety-number span")].map((s) => s.textContent.trim()).join("")`);
    check(/^\d{30}$/.test(safetyOfBSeenByA), `[A] the dialog shows B's safety number (${safetyOfBSeenByA.length} digits)`);
    check((await digitsOf(A)) === digitsB, "[A] still shows the same six digits when it is asked to confirm");
    await A.clickEl(TD("button", "Confirm"), "A's Confirm button");
    await eventually(async () => {
      await watchB();
      const a = await A.evaluate(`norm(document.querySelector(".invite-dialog")?.innerText ?? "")`);
      const b = await B.evaluate(`document.body.innerText`);
      if (/The invitation stopped/.test(a)) must(false, `[A] the invitation stopped: ${a.slice(0, 200)}`);
      if (/Joining stopped/.test(b)) must(false, `[B] joining stopped: ${b.slice(b.indexOf("Joining stopped"), b.indexOf("Joining stopped") + 160)}`);
      return /joined as Editor/.test(a) && /You joined/.test(b);
    }, "both windows say that B joined as Editor", { ms: 300000 });
    await A.capture("code-done-a");
    await B.capture("code-done-b");
    check(sawTokenPrompt === false, "[B] never showed \"needs invite token\" (the share signup made its account)");
    const aSaid = await A.evaluate(`norm(document.querySelector(".invite-dialog [role=status]")?.textContent ?? "")`);
    check(/verified/.test(aSaid), `[A] the dialog says they now see each other as verified (${aSaid})`);

  }

  // The member panels: both show the other as verified.
  if (await A.run(`return !!document.querySelector(".invite-dialog");`)) await closeDialog(A, "Close"); // the invitation dialog
  if (!(await A.run(`return !!document.querySelector(".members-dialog");`))) await openMembers(A);
  rows = await waitMembers(A, "the member list shows B as Editor and verified", (r) => r.length === 2 && r.some((m) => m.name === PEOPLE.b && m.role === "Editor" && m.verified), 90000, [A]);
  check(rows.find((m) => m.you)?.role === "Owner", "[A] the list shows A as Owner");
  const invites = await A.evaluate(`[...document.querySelectorAll(".members-dialog .invite-item")].map((e) => e.textContent.trim())`);
  check(invites.length === 1 && /Code/.test(invites[0]) && /Used by/.test(invites[0]), `[A] the invitation list shows one used code invitation (${invites.join(" | ")})`);
  await A.capture("code-members-a");
  await closeDialog(A, "Close");

  // B opens the space.
  await B.clickEl(TD("button", "Open the space"), "B's Open the space button");
  await B.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, "B's join dialog closed", 20000);
  const sharedB = await eventually(async () => (await sharedSpaceOf(B)) !== undefined, "[B] the shared space is in B's list", { ms: 60000 });
  void sharedB;
  const spaceB = await sharedSpaceOf(B);
  check(spaceB.id === spaceId, "[B] the shared space has A's space id");
  check(spaceB.shareState === "Shared" && spaceB.role === "editor" && spaceB.memberCount === 2, `[B] B sees the space as a shared one, as Editor, with two members (${spaceB.shareState}, ${spaceB.role}, ${spaceB.memberCount})`);
  check(spaceB.aiEnabled === false, "[B] the shared space's AI switch is off by default");
  const syncB = await syncStatus(B);
  check(syncB.configured === true && syncB.serverUrl === SERVER_URL && syncB.needsInviteToken === false && syncB.state !== "Error", `[B] sync is on and points at the server, with no token asked (${syncB.state})`);
  // "Open the space" selects the space when it is in the list: the row of a joined space can arrive a moment after "You joined".
  let selected = false;
  for (let k = 0; k < 40 && !selected; k++) {
    selected = (await shownSpace(B)) === SPACE;
    if (!selected) await wait(250);
  }
  if (!selected) {
    bug("\"Open the space\" closed the join dialog without opening the space: the row of the space was not in the list yet when it was pressed");
    info(`[B] the sidebar still showed "${await shownSpace(B)}" 10 s after "Open the space"; the space is opened through the switcher`);
    await inSharedSpace(B, spaceId);
  }
  check((await shownSpace(B)) === SPACE, `[B] the sidebar shows the shared space (${await shownSpace(B)})`);

  // B's tree equals A's, trashed page and image included.
  const treeA = await treeOf(A, spaceId);
  await eventually(async () => sameShape(shapeOf(await treeOf(B, spaceId)), shapeOf(treeA)), "[B] B's tree equals A's (pages, nesting and the trash)", { ms: 180000, kick: [A, B] });
  info(`B's tree: ${JSON.stringify(shapeOf(await treeOf(B, spaceId)))}`);
  const galleryB = await pageIdByTitle(B, spaceId, PAGES.gallery);
  await openShared(B, spaceId, PAGES.gallery);
  const imageWidth = await imageLoaded(B, 180000);
  check(imageWidth === 40, `[B] the image of the gallery page is readable on B (${imageWidth} px wide)`);
  check((await pageText(B, galleryB)).includes(TEXT.gallery), "[B] the gallery page's text is there");
  await B.capture("code-gallery-b");
  const textsB = {};
  for (const [key, title] of [["plan", PAGES.plan], ["pears", PAGES.pears]]) textsB[key] = await pageText(B, await pageIdByTitle(B, spaceId, title));
  check(textsB.plan.includes(TEXT.plan) && textsB.pears.includes(TEXT.pears), "[B] the other pages' texts are there, the nested one included");

  // B's panel: A is Owner and verified; the AI switch is off.
  await openMembers(B);
  const rowsB = await memberRows(B);
  check(rowsB.length === 2 && rowsB.some((m) => m.name === PEOPLE.a && m.role === "Owner" && m.verified), `[B] B's list shows A as Owner and verified (${JSON.stringify(rowsB.map((m) => [m.name, m.role, m.verified]))})`);
  check(rowsB.some((m) => m.you && m.role === "Editor"), "[B] B's list shows B as Editor");
  check((await aiSwitchOn(B)) === false, "[B] the \"Use AI in this space\" switch is off");
  await B.capture("code-members-b");
  await closeDialog(B, "Close");

  // Edits both ways.
  const planA = await openShared(A, spaceId, PAGES.plan);
  const planB = await openShared(B, spaceId, PAGES.plan);
  await writeLine(B, TEXT.fromB);
  await eventually(async () => (await pageText(A, planA)).includes(TEXT.fromB), "[A] A sees the line B wrote (core)", { ms: 120000, kick: [A, B] });
  const liveA = await A.waitUntil(`(document.querySelector(".ProseMirror")?.innerText ?? "").includes(${JSON.stringify(TEXT.fromB)})`, "[A] A's open editor shows the line B wrote", 30000).catch(() => false);
  if (!liveA) info("[A] the open editor did not refresh by itself; checking after reopening the page");
  await openShared(A, spaceId, PAGES.pears);
  await openShared(A, spaceId, PAGES.plan);
  check(await A.waitUntil(`(document.querySelector(".ProseMirror")?.innerText ?? "").includes(${JSON.stringify(TEXT.fromB)})`, "[A] A's editor shows B's line", 20000), "[A] B's edit shows in A's editor");
  await writeLine(A, TEXT.fromA);
  await eventually(async () => (await pageText(B, planB)).includes(TEXT.fromA), "[B] B sees the line A wrote (core)", { ms: 120000, kick: [A, B] });
  await openShared(B, spaceId, PAGES.pears);
  await openShared(B, spaceId, PAGES.plan);
  check(await B.waitUntil(`(document.querySelector(".ProseMirror")?.innerText ?? "").includes(${JSON.stringify(TEXT.fromA)})`, "[B] B's editor shows A's line", 20000), "[B] A's edit shows in B's editor");
  check((await pageText(A, planA)).includes(TEXT.plan) && (await pageText(B, planB)).includes(TEXT.fromB), "both pages keep every line (nothing was lost in the merge)");

  // Both lists still show the other as verified after the edits.
  await openMembers(A);
  check((await memberRows(A)).some((m) => m.name === PEOPLE.b && m.verified), "[A] B is still shown as verified");
  await closeDialog(A, "Close");
  result.details = { digits: "compared", imageWidth, members: 2 };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: roles
// ---------------------------------------------------------------------------------------------------------------

/** What a page shows about being read-only: the mark, the editor, the title, the structure controls and the properties. */
const readOnlyFacts = (i) =>
  i.run(`
    const pm = document.querySelector(".ProseMirror");
    const addInside = document.querySelector('[role="treeitem"] button[aria-label="Add a page inside"]');
    return {
      pill: norm(document.querySelector(".role-lock")?.textContent ?? ""),
      editable: pm?.getAttribute("contenteditable") ?? null,
      titleButton: !!document.querySelector(".page-title-button"),
      titleText: !!document.querySelector(".page-title-text"),
      newPage: !!document.querySelector(".tree-new"),
      addInside: !!addInside,
      iconDisabled: document.querySelector(".page-icon-button")?.disabled === true,
      propertiesAdd: !!document.querySelector(".properties-add"),
      propertyControls: document.querySelectorAll(".properties .property-value button, .properties .property-value input, .properties .property-value select, .properties .property-value textarea").length,
      toasts: [...document.querySelectorAll(".toast-text")].map((e) => norm(e.textContent)),
    };`);

async function phaseRoles() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId } = readState();
  const planB = await openShared(B, spaceId, PAGES.plan);
  const facts0 = await readOnlyFacts(B);
  check(facts0.editable === "true" && facts0.titleButton && facts0.newPage && facts0.pill === "", `[B] as Editor: the editor is editable, the title is a button, "New page" is there, no read-only mark (${JSON.stringify({ e: facts0.editable, t: facts0.titleButton, n: facts0.newPage, p: facts0.pill })})`);

  // A: Members and sharing -> the role of B -> Viewer.
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  const select = `document.querySelector('select[aria-label="Role of ${PEOPLE.b}"]')`;
  await chooseOption(A, select, "Viewer", "B's role");
  const toasts = new Set();
  await eventually(
    async () => {
      for (const text of (await readOnlyFacts(B)).toasts) toasts.add(text);
      return (await sharedSpaceOf(B)).role === "viewer";
    },
    "[B] B's role became Viewer",
    { ms: 120000, kick: [A, B] },
  );
  await B.waitUntil(`!!document.querySelector(".role-lock")`, "the page shows the Read only mark", 30000);
  for (let k = 0; k < 6; k++) {
    for (const text of (await readOnlyFacts(B)).toasts) toasts.add(text);
    await wait(250);
  }
  const roleToast = [...toasts].find((text) => /changed from Editor to Viewer/.test(text));
  check(roleToast !== undefined && roleToast.includes(SPACE), `[B] a toast says the role changed from Editor to Viewer (${[...toasts].length} toasts seen)`);
  await B.capture("roles-viewer-b");
  const facts1 = await readOnlyFacts(B);
  check(facts1.pill === "Read only", `[B] the page shows the "Read only" mark (${facts1.pill})`);
  check(facts1.editable === "false", `[B] the editor is read-only (contenteditable ${facts1.editable})`);
  check(facts1.titleText === true && facts1.titleButton === false, "[B] the title is plain text, not a rename button");
  check(facts1.iconDisabled === true, "[B] the page icon button is disabled");
  check(facts1.propertiesAdd === false && facts1.propertyControls === 0, `[B] the properties are read-only (add button ${facts1.propertiesAdd}, ${facts1.propertyControls} controls)`);
  check(facts1.newPage === false, `[B] the structure controls are hidden ("New page" ${facts1.newPage})`);

  // Typing into the read-only editor changes nothing; forced writes through the page's own IPC are refused.
  const before = await pageText(B, planB);
  await B.clickEl(Q(".ProseMirror"), "the read-only editor");
  await B.insertText("A Viewer typed this");
  await B.type("zz");
  await wait(2500);
  check((await pageText(B, planB)) === before, "[B] typing into the read-only editor changes nothing");
  const refusal = async (request, what) => {
    const answer = await B.core(request);
    const error = answer.error ?? {};
    check(answer.ok === false && error.code === "Forbidden" && error.details?.reason === "role", `[B] ${what} is refused with Forbidden for the role (${answer.ok ? "ACCEPTED" : `${error.code} ${JSON.stringify(error.details ?? {})}`})`);
  };
  await refusal({ type: "PageRename", data: { pageId: planB, title: "Forced by a Viewer" } }, "a forced PageRename");
  await refusal({ type: "PageCreate", data: { spaceId, title: "Forced page" } }, "a forced PageCreate");
  await refusal({ type: "PageTrash", data: { pageId: planB } }, "a forced PageTrash");
  await refusal({ type: "SpaceRename", data: { spaceId, name: "Forced name" } }, "a forced SpaceRename");
  // The raw document path: the page's own snapshot imported back. A Viewer's document import must be refused as well.
  const raw = await B.evaluate(`(async () => {
    try {
      const snapshot = await window.__TAURI_INTERNALS__.invoke("doc_export", new Uint8Array(), { headers: { "x-kb-page": ${JSON.stringify(planB)} } });
      try {
        await window.__TAURI_INTERNALS__.invoke("doc_import", new Uint8Array(snapshot), { headers: { "x-kb-page": ${JSON.stringify(planB)} } });
        return { exported: true, imported: true };
      } catch (e) {
        return { exported: true, imported: false, error: e instanceof Error ? e.message : e };
      }
    } catch (e) {
      return { exported: false, error: e instanceof Error ? e.message : e };
    }
  })()`);
  info(`[B] a Viewer's doc_export then doc_import of the page's own snapshot: ${JSON.stringify(raw).slice(0, 240)}`);
  check(raw.exported === true, "[B] a Viewer can still read the page document (export)");
  check(raw.imported === false && raw.error?.code === "Forbidden", `[B] a Viewer's raw document import is refused with Forbidden (${raw.imported ? "ACCEPTED" : JSON.stringify(raw.error).slice(0, 120)})`);
  check((await pageIdByTitle(B, spaceId, PAGES.plan)) === planB && (await pageText(B, planB)) === before, "[B] the page is unchanged after the forced writes");
  // A's side: B is listed as Viewer.
  const rows = await waitMembers(A, "the list shows B as Viewer", (r) => r.some((m) => m.name === PEOPLE.b && m.role === "Viewer"), 60000, [A]);
  check(rows.some((m) => m.name === PEOPLE.b && m.role === "Viewer"), "[A] the member list shows B as Viewer");

  // A sets B back to Editor.
  await chooseOption(A, select, "Editor", "B's role");
  await eventually(async () => (await sharedSpaceOf(B)).role === "editor", "[B] B's role became Editor again", { ms: 120000, kick: [A, B] });
  await B.waitUntil(`!document.querySelector(".role-lock")`, "the Read only mark is gone", 30000);
  const facts2 = await readOnlyFacts(B);
  check(facts2.editable === "true" && facts2.titleButton === true && facts2.newPage === true && facts2.propertiesAdd === true, `[B] as Editor again: the editor, the title, "New page" and the properties are editable (${JSON.stringify({ e: facts2.editable, t: facts2.titleButton, n: facts2.newPage, p: facts2.propertiesAdd })})`);
  await closeDialog(A, "Close");
  await openShared(B, spaceId, PAGES.plan);
  await writeLine(B, TEXT.fromB2);
  await eventually(async () => (await pageText(A, await pageIdByTitle(A, spaceId, PAGES.plan))).includes(TEXT.fromB2), "[A] A sees the line B wrote as Editor again", { ms: 120000, kick: [A, B] });
  await B.capture("roles-editor-b");
  result.details = { toast: roleToast !== undefined };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: link
// ---------------------------------------------------------------------------------------------------------------

/** The member row of a person in the open panel, as a page-side expression. */
const rowOf = (name) => `[...document.querySelectorAll(".members-dialog .member-item")].find((li) => li.querySelector(".member-name")?.textContent.trim() === ${JSON.stringify(name)})`;

/** Opens the safety number of a person in the open panel, compares it, and marks them verified; returns the digits. */
async function markVerifiedUi(i, name) {
  await i.clickEl(`byText("summary", "Safety number", true, ${rowOf(name)})`, `the safety number of ${name}`);
  const digits = await i.evaluate(`[...(${rowOf(name)}).querySelectorAll(".member-safety-number span")].map((s) => s.textContent.trim()).join("")`);
  await i.clickEl(`byText("button", "Mark verified", true, ${rowOf(name)})`, `"Mark verified" for ${name}`);
  return digits;
}

async function phaseLink() {
  const A = await inst("a");
  const { spaceId } = readState();
  let C;
  // A run that stopped after C joined is resumed from the checks of C's space.
  const known = readState().instances?.c;
  const resumed = typeof known?.pid === "number" && (await sharedSpaceOf(await inst("c")).catch(() => undefined)) !== undefined;
  if (resumed) {
    C = await inst("c");
    info("[C] already joined: the run resumes after the link flow");
  } else {
    await inSharedSpace(A, spaceId);
    await openMembers(A);
    await A.clickEl(T("button", "Invite with a link..."), `the "Invite with a link..." button`);
    await A.waitFor(T("h2", "Invite with a link"), "the link dialog");
    const form = await A.run(`const r = ${L("Role of the new member", true)}; const d = ${L("The link works for", true)}; return { role: r && norm(r.options[r.selectedIndex].textContent), days: d && norm(d.options[d.selectedIndex].textContent) };`);
    check(form.role === "Viewer" && form.days === "7 days", `[A] the link form starts on Viewer and 7 days (${JSON.stringify(form)})`);
    await A.capture("link-form");
    await A.clickEl(TD("button", "Create the link"), "the Create the link button");
    await A.waitFor(Q(".invite-link-box"), "the invitation link", 60000);
    // The link is read in memory only: never written to a report, a log or a capture.
    const link = await A.evaluate(`document.querySelectorAll(".invite-link-box")[0].value`);
    const webLink = await A.evaluate(`document.querySelectorAll(".invite-link-box")[1]?.value ?? ""`);
    check(link.startsWith("atomik-kb://invite?server=") && link.includes("#"), "[A] the link has the form atomik-kb://invite?server=...#<secret>");
    const secret = link.slice(link.indexOf("#") + 1);
    check(/^[0-9A-Z]{52}$/i.test(secret), `[A] the link's secret is 52 letters and digits (${secret.length})`);
    check(webLink.startsWith(`${SERVER_URL}/join#`), "[A] the link for a browser points at the server's /join page");
    addNeedles({ linkSecret: secret });
    const once = await A.evaluate(`document.querySelector(".invite-once")?.textContent.trim() ?? ""`);
    check(/only time the link is shown/.test(once), `[A] the dialog says the link is shown once (${once.slice(0, 60)})`);
    const summary = await A.evaluate(`norm(document.querySelector(".invite-result")?.innerText ?? "")`);
    check(/joins as Viewer/.test(summary), "[A] the dialog says the person joins as Viewer");
    await A.clickEl(T(".invite-link-line button", "Copy"), "the Copy button");
    await wait(800);
    const copyState = await A.evaluate(`norm(document.querySelector(".invite-link-line button")?.textContent ?? "") + " | " + (document.querySelector(".invite-link-line .error-text")?.textContent ?? "")`);
    info(`[A] Copy: ${/Copied/.test(copyState) ? "copied to the clipboard" : "the clipboard refused, the dialog offered to select and copy by hand"}`);
    await closeDialog(A, "Close"); // the link dialog; the link is gone from the page
    const invites = await A.evaluate(`[...document.querySelectorAll(".members-dialog .invite-item")].map((e) => norm(e.textContent))`);
    check(invites.some((t) => /Link/.test(t) && /Viewer/.test(t) && /Open/.test(t)), `[A] the invitation list shows an open Viewer link (${invites.join(" | ")})`);
    await closeDialog(A, "Close");

    // C is started with the link as its argument (a cold start of the debug binary, one running app: KB_SINGLE_INSTANCE=1).
    await stopInstance("c");
    await launchInstance("c", { link, singleInstance: true });
    const C = await inst("c");
    await markDocument(C);
    await C.waitFor(T("h2", "Invitation to a shared space"), "the invitation preview", 180000);
    await C.waitUntil(`document.querySelector(".invite-preview .pair-peer") !== null`, "the preview names the inviter, the space and the role", 120000);
    const preview = await C.evaluate(`({ line: norm(document.querySelector(".invite-preview .pair-peer")?.textContent ?? ""), text: norm(document.querySelector(".invite-preview")?.innerText ?? "") })`);
    info(`[C] the preview reads: ${preview.text.slice(0, 300)}`);
    check(preview.line === `${PEOPLE.a} invites you to “${SPACE}” as Viewer.`, `[C] the preview names the inviter, the space and the role (${preview.line})`);
    check(/whole history of the space/.test(preview.text) && /trash/.test(preview.text), "[C] the preview says C will receive the whole history, the trash included");
    check(/Not verified/.test(preview.text), "[C] the preview says the invitation is not verified");
    await C.capture("link-preview-c");
    await C.clickEl(TD("button", "Join the space"), "the Join the space button");
    await eventually(
      async () => {
        const text = await C.evaluate(`document.body.innerText`);
        if (/This invitation cannot be used/.test(text)) must(false, `[C] the invitation was refused: ${text.slice(text.indexOf("This invitation"), text.indexOf("This invitation") + 140)}`);
        return /You joined/.test(text);
      },
      "[C] C joined the space",
      { ms: 300000 },
    );
    await C.capture("link-joined-c");
    await C.clickEl(TD("button", "Open the space"), "C's Open the space button");
    await C.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, "C's dialog closed", 20000);
    await eventually(async () => (await sharedSpaceOf(C)) !== undefined, "[C] the shared space is in C's list", { ms: 120000, kick: [C] });

  }
  C = await inst("c");
  const spaceC = await sharedSpaceOf(C);
  check(spaceC.id === spaceId && spaceC.shareState === "Shared" && spaceC.role === "viewer", `[C] C sees the space as a shared one, as Viewer (${spaceC.shareState}, ${spaceC.role})`);
  check(spaceC.aiEnabled === false, "[C] the shared space's AI switch is off by default");
  const syncC = await syncStatus(C);
  check(syncC.configured === true && syncC.serverUrl === SERVER_URL && syncC.needsInviteToken === false, `[C] sync points at the link's server, no token asked (${syncC.state})`);
  const treeA = await treeOf(A, spaceId);
  await eventually(async () => sameShape(shapeOf(await treeOf(C, spaceId)), shapeOf(treeA)), "[C] C's tree equals A's", { ms: 180000, kick: [A, C] });
  const galleryC = await openShared(C, spaceId, PAGES.gallery);
  check((await imageLoaded(C, 180000)) === 40, "[C] the image is readable on C");
  check((await pageText(C, galleryC)).includes(TEXT.gallery), "[C] C reads the pages' text");
  const readOnlyC = await readOnlyFacts(C);
  check(readOnlyC.pill === "Read only" && readOnlyC.editable === "false", "[C] C, a Viewer, has a read-only editor");

  // The member panels: C is not verified; the safety numbers are compared and C marks A verified, A marks C.
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  const rowsA = await waitMembers(A, "A's list shows three members, C not verified", (r) => r.length === 3 && r.some((m) => m.role === "Viewer" && m.unverified), 120000, [A, C]);
  const nameC = rowsA.find((m) => m.role === "Viewer").name;
  check(nameC !== PEOPLE.a && nameC !== PEOPLE.b && nameC.length > 0, "[A] C is listed by a stand-in name (C set no profile name)");
  writeState({ nameC });
  check(rowsA.find((m) => m.name === nameC).unverified, "[A] A's list shows C as not verified");
  const invitesAfter = await A.evaluate(`[...document.querySelectorAll(".members-dialog .invite-item")].map((e) => norm(e.textContent))`);
  check(invitesAfter.some((t) => /Link/.test(t) && /Used by/.test(t)), `[A] the link invitation is shown as used (${invitesAfter.join(" | ").slice(0, 160)})`);
  await openMembers(C);
  const rowsC = await memberRows(C);
  check(rowsC.length === 3 && rowsC.some((m) => m.name === PEOPLE.a && m.unverified), `[C] C's list shows A as not verified (${JSON.stringify(rowsC.map((m) => [m.name, m.role, m.unverified]))})`);
  check((await aiSwitchOn(C)) === false, "[C] the \"Use AI in this space\" switch is off");
  const numbersA = (await membersOf(A, spaceId)).members.find((m) => m.name === nameC)?.safetyNumber;
  const numbersC = (await membersOf(C, spaceId)).members.find((m) => m.name === PEOPLE.a)?.safetyNumber;
  check(typeof numbersA === "string" && numbersA === numbersC, "A's safety number for C and C's for A are the same 30 digits");
  const shownC = await markVerifiedUi(C, PEOPLE.a);
  const shownA = await markVerifiedUi(A, nameC);
  const digitsOnly = (text) => String(text).replace(/[^0-9]/g, "");
  check(digitsOnly(shownC) === digitsOnly(numbersC) && digitsOnly(shownA) === digitsOnly(numbersA) && digitsOnly(numbersA).length === 30, "the digits the panels show are the core's safety numbers (30 digits)");
  await waitMembers(C, "C's list shows A as verified", (r) => r.some((m) => m.name === PEOPLE.a && m.verified), 30000);
  await waitMembers(A, "A's list shows C as verified", (r) => r.some((m) => m.name === nameC && m.verified), 30000);
  check(true, "both lists show the other as verified after the safety numbers were compared");
  await C.capture("link-members-c");
  await closeDialog(C, "Close");
  await closeDialog(A, "Close");
  result.details = { role: "viewer", members: 3 };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: remove
// ---------------------------------------------------------------------------------------------------------------

/** The Home screen of the space in use (where the archive banner lives). */
async function goHome(i) {
  await normalize(i);
  await i.clickEl(T("nav.sidebar button", "Home"), "the Home button");
  await wait(600);
}

async function phaseRemove() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId, nameC } = readState();
  must(typeof nameC === "string", "the link phase named C's row");
  await inSharedSpace(A, spaceId);
  const epochBefore = (await membersOf(A, spaceId)).epoch;
  const planId = await pageIdByTitle(A, spaceId, PAGES.plan);

  // A removes B: the question says what a removed member keeps.
  await openMembers(A);
  await A.clickEl(`byText("button", "Remove", true, ${rowOf(PEOPLE.b)})`, "B's Remove button");
  await A.waitFor(T("h2", `Remove ${PEOPLE.b}?`), "the removal question");
  const question = await A.evaluate(`norm(document.querySelector('[role="alertdialog"] p')?.textContent ?? "")`);
  check(/cannot read anything written from now on/.test(question) && /keep what they already received/.test(question) && /new key/.test(question), `[A] the question says what a removed member loses and keeps (${question.slice(0, 120)})`);
  await A.capture("remove-question");
  await A.clickEl(TD("button", `Remove ${PEOPLE.b}`), "the Remove button of the question");
  await waitMembers(A, "the member list no longer has B", (r) => r.length === 2 && !r.some((m) => m.name === PEOPLE.b), 90000, [A]);
  check(true, "[A] the member list no longer has B");
  await closeDialog(A, "Close");
  const countStart = Date.now();
  await eventually(async () => (await spaceList(A)).find((s) => s.id === spaceId).memberCount === 2, "[A] the space has two members now (the space list's count)", { ms: 60000, kick: [A] });
  info(`[A] the space list's member count followed the panel after ${round((Date.now() - countStart) / 1000)} s`);

  // B: the archive banner, within a few cycles.
  await eventually(async () => (await sharedSpaceOf(B))?.shareState === "Archived", "[B] B's space became an archive", { ms: 240000, kick: [A, B] });
  const spaceB = await sharedSpaceOf(B);
  check(spaceB.archiveReason === "Removed", `[B] the archive's reason is Removed (${spaceB.archiveReason})`);
  await inSharedSpace(B, spaceId);
  await goHome(B);
  await B.waitFor(Q(".archive-banner"), "the archive banner", 60000);
  const banner = await B.evaluate(`norm(document.querySelector(".archive-banner")?.innerText ?? "")`);
  check(banner.startsWith("You were removed from this space. This is a read-only copy; it will not update.") && /Remove from this device/.test(banner), `[B] the banner says B was removed and the copy is read-only (${banner.slice(0, 140)})`);
  await B.capture("remove-banner-b");
  const planB = await pageIdByTitle(B, spaceId, PAGES.plan);
  must(planB !== null, "[B] B's old pages are still in its tree");
  const oldText = await pageText(B, planB);
  check(oldText.includes(TEXT.plan) && oldText.includes(TEXT.fromB) && oldText.includes(TEXT.fromA), "[B] B's old pages stay readable");
  await openShared(B, spaceId, PAGES.plan);
  const facts = await readOnlyFacts(B);
  check(facts.editable === "false" && /Read-only copy/.test(facts.pill), `[B] the archive's pages are read-only (${facts.pill})`);
  const statusB = await syncStatus(B);
  info(`[B] sync after the removal: state ${statusB.state}${statusB.lastError ? `, last error: ${statusB.lastError}` : ""}, feeds ${JSON.stringify((statusB.feeds ?? []).map((f) => ({ closed: f.closed, forked: f.forked, refused: f.refused })))}`);

  // A writes a new page and a line afterwards: C (still a member) gets them, B never does.
  await inSharedSpace(A, spaceId);
  await newPage(A, PAGES.harvest);
  await writeLine(A, TEXT.harvest);
  await openShared(A, spaceId, PAGES.plan);
  await writeLine(A, TEXT.afterRemoval);
  const epochAfter = (await membersOf(A, spaceId)).epoch;
  check(epochAfter > epochBefore, `[A] the removal renewed the space key (epoch ${epochBefore} -> ${epochAfter})`);
  await eventually(
    async () => {
      const id = await pageIdByTitle(C, spaceId, PAGES.harvest);
      return id !== null && (await pageText(C, id)).includes(TEXT.harvest) && (await pageText(C, await pageIdByTitle(C, spaceId, PAGES.plan))).includes(TEXT.afterRemoval);
    },
    "[C] C still syncs: it reads the new page and the new line",
    { ms: 240000, kick: [A, C] },
  );
  check(true, "[C] C still syncs after the removal (one Rekey): the new page and line arrived");
  for (let k = 0; k < 5; k++) {
    await syncNow(B);
    await wait(6000);
  }
  check((await pageIdByTitle(B, spaceId, PAGES.harvest)) === null, "[B] the page A wrote after the removal never reached B");
  check(!(await pageText(B, planB)).includes(TEXT.afterRemoval), "[B] the line A wrote after the removal never reached B");
  const treeB = shapeOf(await treeOf(B, spaceId));
  check(!treeB.live.some((p) => p.includes(PAGES.harvest)), `[B] B's tree has no new page (${treeB.live.length} live pages)`);
  await B.capture("remove-after-sync-b");
  const rowsC = await memberRows(C).catch(() => []);
  void rowsC;
  result.details = { epochBefore, epochAfter };
  void planId;
  void nameC;
}

// ---------------------------------------------------------------------------------------------------------------
// phase: leave
// ---------------------------------------------------------------------------------------------------------------

async function phaseLeave() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId } = readState();

  // The AI switch of the shared space is off by default on B (the archive) and on C.
  const spaceB = await sharedSpaceOf(B);
  check(spaceB.aiEnabled === false, "[B] the archive's AI flag is off");
  await inSharedSpace(B, spaceId);
  await openMembers(B);
  check((await aiSwitchOn(B)) === false, "[B] the \"Use AI in this space\" switch is off on B");
  const bannerInPanel = await B.evaluate(`norm(document.querySelector(".members-dialog .banner")?.textContent ?? "")`);
  check(/You were removed from this space/.test(bannerInPanel), "[B] the panel of the archive says B was removed");
  await closeDialog(B, "Close");
  await inSharedSpace(C, spaceId);
  await openMembers(C);
  check((await aiSwitchOn(C)) === false, "[C] the \"Use AI in this space\" switch is off on C");
  const leaveLabel = await C.evaluate(`!![...document.querySelectorAll(".members-dialog button")].find((b) => b.textContent.trim() === "Leave this space")`);
  check(leaveLabel === true, "[C] the panel offers \"Leave this space\"");

  // C leaves: the question, then the copy is gone from C and A's list updates.
  await C.clickEl(TD("button", "Leave this space"), "C's Leave this space button");
  await C.waitFor(T("h2", `Leave “${SPACE}”?`), "the leave question");
  const question = await C.evaluate(`norm(document.querySelector('[role="alertdialog"] p')?.textContent ?? "")`);
  check(/copy on this device is deleted/.test(question) && /can be invited again/.test(question), `[C] the question says the local copy goes and C can be invited again (${question.slice(0, 100)})`);
  await C.capture("leave-question-c");
  await C.clickEl(TD("button", "Leave the space"), "the Leave the space button");
  await C.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, "C's dialogs closed", 60000);
  await eventually(async () => (await spaceList(C)).every((s) => s.name !== SPACE), "[C] the shared space is gone from C", { ms: 120000, kick: [C] });
  check(true, "[C] the shared space is gone from C's list");
  check((await spaceList(C)).length === 1, "[C] C keeps its own personal space");
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  const rows = await waitMembers(A, "A's list holds A alone", (r) => r.length === 1, 180000, [A, C]);
  check(rows.length === 1 && rows[0].you, "[A] A's member list updated: only A is left");
  check((await spaceList(A)).find((s) => s.id === spaceId).memberCount === 1, "[A] the space has one member");
  const note = await A.evaluate(`norm(document.querySelector(".members-dialog .note")?.textContent ?? "")`);
  info(`[A] the panel notes after the departure: ${note || "(nothing)"}`);
  await A.capture("leave-members-a");
  await closeDialog(A, "Close");
  // B's archive is untouched by C's departure.
  check((await sharedSpaceOf(B))?.shareState === "Archived", "[B] B's archive is still there");
  result.details = { membersAfter: 1 };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: devices
// ---------------------------------------------------------------------------------------------------------------

/** How many lines of the server's request log match `pattern` (the log holds requests only, never bodies). */
function serverLogCount(pattern) {
  const file = join(OUT, "server.log");
  if (!existsSync(file)) return 0;
  const text = stripAnsi(readFileSync(file, "utf8"));
  return text.split(/\r?\n/).filter((l) => pattern.test(l)).length;
}

/** The `.chunk` files under a data directory's blobs folder. */
function countChunks(dir) {
  const blobs = join(dir, "blobs");
  if (!existsSync(blobs)) return 0;
  let count = 0;
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(join(d, entry.name));
      else if (entry.name.endsWith(".chunk")) count += 1;
    }
  };
  walk(blobs);
  return count;
}

/** The trees of every space of two instances are the same (the shared space's and the personal ones'). */
async function sameSpaces(x, y) {
  const a = await spaceList(x);
  const b = await spaceList(y);
  if (a.length !== b.length || JSON.stringify(a.map((s) => s.id).sort()) !== JSON.stringify(b.map((s) => s.id).sort())) return false;
  for (const space of a) {
    if (!sameShape(shapeOf(await treeOf(x, space.id)), shapeOf(await treeOf(y, space.id)))) return false;
  }
  return true;
}

/** An instance of an earlier, stopped run of this phase is up and has its workspace (the run resumes from there). */
async function alreadyUp(name) {
  if (typeof readState().instances?.[name]?.pid !== "number") return false;
  const i = await inst(name).catch(() => null);
  return i !== null && (await i.run("return !!document.querySelector('.tab-strip');").catch(() => false));
}

async function phaseDevices() {
  const A = await inst("a");
  const { spaceId } = readState();
  const kitA = readState().needles?.kit?.a;
  must(Array.isArray(kitA) && kitA.length === 24, "A's kit words are known");

  // ---- A pairs a second device D on /v2/pair: the code on D, the six digits on both ------------------------------
  let D;
  if (await alreadyUp("d")) {
    D = await inst("d");
    info("[D] is already paired: the run resumes after the pairing");
  } else {
    await launchInstance("d", { fresh: true });
    D = await inst("d");
    await D.clickEl(T("button", "Add this device to my account"), `D's "Add this device to my account" button`, { ms: 90000 });
    await D.fill(L("Sync server address"), "D's server address field", SERVER_URL);
    await D.clickEl(T("button", "Connect"), "D's Connect button");
    await D.waitFor(Q(".pair-code"), "D's pairing code", 90000);
    const pairingCode = await D.evaluate(`document.querySelector(".pair-code").textContent.replace(/\\s/g, "")`);
    check(/^[0-9A-Z]{12}$/.test(pairingCode), "[D] the pairing code has 12 characters");
    addNeedles({ pairingCode });
    await goSettings(A);
    await A.clickEl(T("button", "Add a device..."), `A's "Add a device..." button`);
    await A.fill(L("Code shown on the new device", true), "the code field", pairingCode);
    await A.clickEl(TD("button", "Continue"), "A's Continue button");
    let digitsA = null;
    let digitsD = null;
    await eventually(async () => {
      digitsA = await digitsOf(A);
      digitsD = await digitsOf(D);
      return digitsA !== null && digitsD !== null;
    }, "both windows show the six digits of the pairing", { ms: 120000 });
    check(/^\d{6}$/.test(digitsA ?? "") && digitsA === digitsD, "the six digits of A and of D are the same (device pairing on v2)");
    await A.capture("devices-digits-a");
    await D.capture("devices-digits-d");
    await D.clickEl(TD("button", "Confirm"), "D's Confirm button");
    await A.waitUntil(`${DIALOG_TEXT}.includes("You are adding")`, "A's dialog names the new device", 90000);
    await A.clickEl(TD("button", "Confirm"), "A's Confirm button");
    await A.waitUntil(`${DIALOG_TEXT}.includes("was added to your account")`, "A's dialog says the device was added", 180000);
    await A.capture("devices-added-a");
    await closeDialog(A, "Done");
  }
  await D.waitFor(Q(".tab-strip"), "D's workspace", 240000);
  await markDocument(D);
  check((await heading(D)) === WORKSPACES.a, `[D] D has A's workspace (${await heading(D)})`);
  await eventually(async () => (await sharedSpaceOf(D)) !== undefined && (await sameSpaces(A, D)), "[D] D's trees equal A's, the shared space included", { ms: 300000, kick: [A, D] });
  const sharedD = await sharedSpaceOf(D);
  check(sharedD.shareState === "Shared" && sharedD.role === "owner" && sharedD.id === spaceId, `[D] D has the shared space as its owner account (${sharedD.shareState}, ${sharedD.role})`);
  const planD = await pageIdByTitle(D, spaceId, PAGES.plan);
  const textD = await pageText(D, planD);
  check(textD.includes(TEXT.plan) && textD.includes(TEXT.fromB) && textD.includes(TEXT.afterRemoval), "[D] D reads the shared pages (through the personal keyring)");
  await D.capture("devices-d");
  const trees = {};
  for (const space of await spaceList(A)) trees[space.id.slice(0, 8)] = shapeOf(await treeOf(A, space.id)).live.length;
  info(`[D] the trees equal A's: live pages per space ${JSON.stringify(trees)}`);

  // ---- a kit recovery on a fresh install E against the server -------------------------------------------------------
  let E;
  if (await alreadyUp("e")) {
    E = await inst("e");
    info("[E] is already recovered: the run resumes after the recovery");
  } else {
    await launchInstance("e", { fresh: true });
    E = await inst("e");
    await E.clickEl(T("button", "Recover with my kit"), `E's "Recover with my kit" button`, { ms: 90000 });
    await E.fill(L("Sync server address"), "E's server address field", SERVER_URL);
    await E.clickEl(Q("textarea.kit-input"), "E's kit field");
    await E.insertText(kitA.join(" "));
    await wait(300);
    await E.clickEl(T("button", "Recover"), "E's Recover button");
    await E.waitFor(Q(".tab-strip"), "E's recovered workspace", 300000);
    // A recovered workspace is named "My notes" (its name is not in the account's log, M7).
    check((await heading(E)) === "My notes", `[E] E has a recovered workspace named "My notes" (${await heading(E)})`);
  }
  await E.waitFor(Q(".tab-strip"), "E's recovered workspace", 300000);
  await markDocument(E);
  await eventually(async () => (await sharedSpaceOf(E)) !== undefined && (await sameSpaces(A, E)), "[E] after the recovery E's trees equal A's, the shared space included", { ms: 300000, kick: [A, E] });
  const sharedE = await sharedSpaceOf(E);
  check(sharedE.shareState === "Shared" && sharedE.role === "owner", `[E] the shared space is there after the recovery, as Owner (${sharedE.shareState}, ${sharedE.role})`);
  const planE = await pageIdByTitle(E, spaceId, PAGES.plan);
  check((await pageText(E, planE)).includes(TEXT.fromA), "[E] the shared space is readable after the kit recovery");
  await switchSpace(E, spaceId);
  await openShared(E, spaceId, PAGES.gallery);
  check((await imageLoaded(E, 180000)) === 40, "[E] the image of the shared space is readable after the recovery");
  await E.capture("devices-recovered-e");
  // A's own panel counts the devices of its account and notes the kit recovery.
  await eventually(
    async () => {
      const me = (await membersOf(A, spaceId)).members.find((m) => m.isMe);
      return me?.deviceCount === 3 && me.viaKitAt !== undefined;
    },
    "[A] A's own member row counts three devices and the one added with the kit (A learns of E's log through the server)",
    { ms: 180000, kick: [A, E] },
  );
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  const meta = (await memberRows(A)).find((m) => m.you)?.meta ?? "";
  check(/3 devices/.test(meta) && /recovery kit/.test(meta), `[A] A's panel counts three devices and notes the one added with the recovery kit (${meta})`);
  await closeDialog(A, "Close");

  // ---- attachment sync with deletion, on the shared space (the checks of drive-m8, on v2) -----------------------------
  const dir = join(OUT, "m13-share-src");
  mkdirSync(dir, { recursive: true });
  const big = join(dir, FILE_NAME);
  writeFileSync(big, randomBytes(9 * 1024 * 1024));
  addNeedles({ fileName: FILE_NAME });
  const chunksA0 = countChunks(dirsOf("a").data);
  const putsBefore = serverLogCount(/method=PUT path=\/v2\/feeds\/[0-9a-f.]+\/chunks\//);
  await inSharedSpace(A, spaceId);
  await newPage(A, PAGES.files);
  await writeLine(A, TEXT.files);
  await newLine(A);
  await slash(A, "file", "File");
  await fileDialog(A, "Open", big, 60000);
  const attachmentOf = async (i) => (await i.ask({ type: "AttachmentList", data: { spaceId } })).find((a) => a.name === FILE_NAME);
  await eventually(async () => {
    const a = await attachmentOf(A);
    const s = await syncStatus(A);
    return a !== undefined && a.chunkCount === 3 && a.localChunks === 3 && s.transfers.uploads === 0;
  }, "[A] the file is added and uploaded (3 chunks of 4 MiB, nothing left to upload)", { ms: 300000, kick: [A] });
  const attachment = await attachmentOf(A);
  check(attachment.size === 9 * 1024 * 1024 && attachment.references === 1, `[A] the attachment is 9 MiB and used by one block (${attachment.size} bytes, ${attachment.references} references)`);
  const puts = serverLogCount(/method=PUT path=\/v2\/feeds\/[0-9a-f.]+\/chunks\//) - putsBefore;
  check(puts >= 3, `[A] the server received the chunks of the shared feed (${puts} PUT requests)`);
  await eventually(async () => {
    const a = await attachmentOf(D);
    return a !== undefined && a.localChunks === 3 && a.availability === "Complete";
  }, "[D] A's second device downloaded the whole file (3 chunks)", { ms: 300000, kick: [A, D] });
  check(countChunks(dirsOf("d").data) >= 3, `[D] D's blob store holds the chunks (${countChunks(dirsOf("d").data)})`);
  await eventually(async () => (await attachmentOf(E))?.availability === "Complete", "[E] the recovered device downloaded the file as well", { ms: 300000, kick: [A, E] });
  check(countChunks(dirsOf("a").data) - chunksA0 >= 3, "[A] A's blob store holds the file's chunks");
  await A.capture("devices-file-a");
  // Remove the block, shorten the retention to a few seconds and clean up: the Owner's device deletes the chunks on the server.
  const deletesBefore = serverLogCount(/method=DELETE path=\/v2\/feeds\/[0-9a-f.]+\/chunks\//);
  await A.clickEl(Q('button[aria-label="File actions"]'), "the File actions button");
  await chooseMenuItem(A, "Remove");
  await wait(1500);
  check((await attachmentOf(A))?.references === 0, "[A] the block is removed: nothing references the file any more");
  await A.ask({ type: "SettingsSet", data: { key: "app.attachments.retention_days", value: 0.00002 } });
  await wait(4000);
  await goSettings(A);
  await A.clickEl(T("button", "Clean up now"), "the Clean up now button");
  await A.waitUntil(`${T("p, span, div", "Cleaned up", false)} !== null`, "the clean-up result", 60000);
  const cleaned = await A.evaluate(`norm([...document.querySelectorAll("p, span, div")].map((e) => e.textContent).find((t) => /^Cleaned up/.test(norm(t))) ?? "")`);
  check(/Cleaned up: 1 unused files? deleted/.test(cleaned), `[A] "Clean up now" deleted the unused file (${cleaned.slice(0, 80)})`);
  await eventually(async () => serverLogCount(/method=DELETE path=\/v2\/feeds\/[0-9a-f.]+\/chunks\//) - deletesBefore >= 3, "the server logged the deletion of the chunks of the shared feed", { ms: 180000, kick: [A] });
  const deletions = serverLogCount(/method=DELETE path=\/v2\/feeds\/[0-9a-f.]+\/chunks\//) - deletesBefore;
  check(deletions >= 3, `[A] the Owner's device deleted the file's chunks on the server (${deletions} DELETE requests)`);
  check(countChunks(dirsOf("a").data) <= chunksA0, "[A] A's own blob store is back to what it held before");
  check((await attachmentOf(A)) === undefined, "[A] the attachment is gone from A's list");
  await eventually(async () => (await attachmentOf(D)) === undefined || (await attachmentOf(D)).references === 0, "[D] D no longer references the file", { ms: 180000, kick: [A, D] });
  result.details = { puts, deletions, digits: "compared" };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: reinvite
// ---------------------------------------------------------------------------------------------------------------

async function phaseReinvite() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId } = readState();
  const copy = await sharedSpaceOf(B);
  must(copy?.shareState === "Archived", "[B] B holds the read-only copy of its removal");
  check((await pageIdByTitle(B, spaceId, PAGES.harvest)) === null, "[B] the copy has no page that was written while B was out");

  // A invites the removed B again, by code.
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  await A.clickEl(T("button", "Invite with a code..."), `the "Invite with a code..." button`);
  await A.waitFor(T("h2", "Invite with a code"), "the invitation dialog");
  await A.clickEl(TD("button", "Show the code"), "the Show the code button");
  await A.waitFor(Q(".pair-code"), "the invitation code", 60000);
  const code = await A.evaluate(`document.querySelector(".pair-code").textContent.replace(/\\s/g, "")`);
  check(/^[0-9A-Z]{12}$/.test(code), `[A] the code has 12 characters (${code.length})`);
  addNeedles({ code2: code });
  await openSpaceList(B);
  await B.clickEl(T(".space-popover button.menu-item", "Join a shared space..."), `"Join a shared space..." in the list`);
  await B.waitFor(T("h2", "Join a shared space"), "the Join a shared space dialog");
  await B.fill(L("Server address", true), "the server address field", SERVER_URL);
  await B.fill(L("Code", true), "the code field", code);
  await B.clickEl(TD("button", "Continue"), "the Continue button");
  let digitsA = null;
  let digitsB = null;
  await eventually(
    async () => {
      const text = await bodyText(B);
      const stopped = /Joining stopped: ([^\n]*)/.exec(text);
      if (stopped) must(false, `[B] joining stopped: ${stopped[1]}`);
      digitsA = await digitsOf(A);
      digitsB = await digitsOf(B);
      return digitsA !== null && digitsB !== null;
    },
    "both windows show the six digits",
    { ms: 120000 },
  );
  check(/^\d{6}$/.test(digitsA ?? "") && digitsA === digitsB, "the six digits of A and of B are the same");
  await A.capture("reinvite-digits-a");
  await B.capture("reinvite-digits-b");
  await B.clickEl(TD("button", "Confirm"), "B's Confirm button");
  await A.waitUntil(`${DIALOG_TEXT}.includes("You are inviting")`, "A's dialog says who is being invited", 90000);
  const inviting = await A.evaluate(`document.querySelector(".invite-dialog .pair-peer")?.textContent.trim() ?? ""`);
  check(inviting.includes(PEOPLE.b), `[A] the dialog names the person being invited (${inviting})`);
  await A.clickEl(TD("button", "Confirm"), "A's Confirm button");
  await eventually(
    async () => {
      const a = await A.evaluate(`norm(document.querySelector(".invite-dialog")?.innerText ?? "")`);
      const b = await B.evaluate(`document.body.innerText`);
      if (/The invitation stopped/.test(a)) must(false, `[A] the invitation stopped: ${a.slice(0, 200)}`);
      if (/Joining stopped/.test(b)) must(false, `[B] joining stopped: ${b.slice(b.indexOf("Joining stopped"), b.indexOf("Joining stopped") + 160)}`);
      return /joined as Editor/.test(a) && /You joined/.test(b);
    },
    "both windows say that B joined as Editor again",
    { ms: 300000 },
  );
  await A.capture("reinvite-done-a");
  await B.capture("reinvite-done-b");
  await closeDialog(A, "Close"); // the invitation dialog
  await B.clickEl(TD("button", "Open the space"), "B's Open the space button");
  await B.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, "B's join dialog closed", 20000);

  // The space is B's again: one space of that name, shared, as Editor; the copy of the removal is the live space.
  await eventually(
    async () => {
      const s = await sharedSpaceOf(B);
      return s?.shareState === "Shared" && s.role === "editor";
    },
    "[B] the space is B's again, shared, as Editor",
    { ms: 120000, kick: [A, B] },
  );
  const named = (await spaceList(B)).filter((s) => s.name === SPACE);
  check(named.length === 1 && named[0].id === spaceId, `[B] one space of that name, the same one as before (${named.length})`);
  check(named[0].aiEnabled === false, "[B] the AI switch of the space is off again");

  // B sees what was written while it was out: the page and the line A wrote after the removal.
  await eventually(
    async () => {
      const harvest = await pageIdByTitle(B, spaceId, PAGES.harvest);
      const plan = await pageIdByTitle(B, spaceId, PAGES.plan);
      return harvest !== null && plan !== null && (await pageText(B, harvest)).includes(TEXT.harvest) && (await pageText(B, plan)).includes(TEXT.afterRemoval);
    },
    "[B] B reads the page and the line written while it was out",
    { ms: 240000, kick: [A, B] },
  );
  check(true, "[B] B reads the page and the line A wrote while it was out");
  const treeA = await treeOf(A, spaceId);
  await eventually(async () => sameShape(shapeOf(await treeOf(B, spaceId)), shapeOf(treeA)), "[B] B's tree equals A's again", { ms: 180000, kick: [A, B] });
  await openShared(B, spaceId, PAGES.harvest);
  await B.capture("reinvite-harvest-b");
  const factsB = await readOnlyFacts(B);
  check(factsB.editable === "true" && factsB.pill === "", "[B] B can edit the space again (the editor is editable)");

  // The lists: A's panel has B as Editor and verified; the invitation list holds two used codes.
  if (!(await A.run(`return !!document.querySelector(".members-dialog");`))) await openMembers(A);
  const rows = await waitMembers(A, "A's list holds A and B as Editor", (r) => r.length === 2 && r.some((m) => m.name === PEOPLE.b && m.role === "Editor"), 90000, [A, B]);
  check(rows.find((m) => m.name === PEOPLE.b)?.verified === true, "[A] B is verified again");
  const invites = await A.evaluate(`[...document.querySelectorAll(".members-dialog .invite-item")].map((e) => e.textContent.trim())`);
  check(invites.filter((t) => /Code/.test(t) && /Used by/.test(t)).length === 2, `[A] the invitation list holds two used code invitations (${invites.length} invitations)`);
  await closeDialog(A, "Close");

  // Edits go both ways once more.
  await writeLine(B, TEXT.fromBAgain);
  await eventually(async () => (await pageText(A, await pageIdByTitle(A, spaceId, PAGES.harvest))).includes(TEXT.fromBAgain), "[A] A sees the line B wrote after coming back", { ms: 120000, kick: [A, B] });
  result.details = { members: 2 };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: close
// ---------------------------------------------------------------------------------------------------------------

/** Runs kb-server's script (`server.ps1 -Action stop|start`: start keeps the data). */
function serverCtl(action) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(HERE, "server.ps1"), "-Action", action], { stdio: "ignore", windowsHide: true });
    child.on("error", reject);
    child.on("exit", (code) => resolvePromise(code));
  });
}

/** The lines of the server's log that match `pattern`. */
function serverLogLines(pattern) {
  const file = join(OUT, "server.log");
  if (!existsSync(file)) return [];
  return stripAnsi(readFileSync(file, "utf8")).split(/\r?\n/).filter((l) => pattern.test(l));
}

async function phaseClose() {
  const A = await inst("a");
  const B = await inst("b");
  const D = await inst("d");
  const E = await inst("e");
  const { spaceId } = readState();
  for (const i of [A, B, D, E]) check((await sharedSpaceOf(i))?.shareState === "Shared", `[${i.tag}] the space is shared and open before it is closed`);

  // A device that never hears of the close: the server goes away, D writes a line it cannot send and is frozen, the server comes
  // back, A closes the space, and only then D wakes up: it pushes first, and the server refuses it.
  await serverCtl("stop");
  copyFileSync(join(OUT, "server.log"), join(OUT, "m13-share-server-before-close.log"));
  await openShared(D, spaceId, PAGES.plan);
  await writeLine(D, TEXT.lateWrite);
  const pending = ((await syncStatus(D)).feeds ?? []).find((f) => f.spaceId === spaceId)?.pendingEntries ?? 0;
  check(pending > 0, `[D] D holds a change it could not send (${pending} entries wait)`);
  const dPid = D.rec.pid;
  const frozen = await helper("freeze", { pid: dPid });
  check(frozen.ok === true, "[D] D is frozen (its process is suspended)");
  let dFrozen = frozen.ok === true;
  try {
    await closeAndWatch();
  } finally {
    // A device left frozen by a failed check would hang every phase after this one.
    if (dFrozen) await helper("thaw", { pid: dPid }).catch(() => null);
  }
  result.details = { refused: result.refused, chunksBefore: result.chunksBefore, chunksAfter: result.chunksAfter };
  return;

  async function closeAndWatch() {
  await serverCtl("start");
  check(typeof (await fetch(`${SERVER_URL}/v2/server`).then((r) => r.json()).catch(() => null))?.serverEpoch === "string", "the server is back with its data");

  // A closes the space: the question says what closing does.
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  await A.clickEl(TD("button", "Close this space"), "A's Close this space button");
  await A.waitFor(T("h2", `Close “${SPACE}” for everyone?`), "the close question");
  const question = await A.evaluate(`norm(document.querySelector('[role="alertdialog"] p')?.textContent ?? "")`);
  check(/Nobody can change the space after this/.test(question) && /read-only copy/.test(question) && /cannot be undone/.test(question), `[A] the question says what closing does (${question.slice(0, 110)})`);
  await A.capture("close-question");
  await A.clickEl(TD("button", "Close the space"), "the Close the space button");
  await A.waitUntil(`!document.querySelector('[role="alertdialog"]')`, "the close question went", 60000);
  await closeDialog(A, "Close"); // the member panel, which stays open under the question

  // Every member gets the read-only archive: A, then B and E (online); D is still frozen.
  const archived = async (i) => {
    const s = await sharedSpaceOf(i);
    return s?.shareState === "Archived" && s.archiveReason === "Closed";
  };
  await eventually(async () => (await archived(A)) && (await archived(B)) && (await archived(E)), "[A] [B] [E] the space is a read-only copy (closed by an Owner)", { ms: 300000, kick: [A, B, E] });
  check(true, "[A] [B] [E] the space became a read-only copy: closed by an Owner");
  const logBefore = serverLogLines(/method=POST path=\/v2\/feeds\/[0-9a-f.]+\/entries.*status=409/).length;
  const thawed = await helper("thaw", { pid: dPid });
  dFrozen = false;
  check(thawed.ok === true, "[D] D wakes up");
  await eventually(async () => await archived(D), "[D] D, which never heard of the close, learns it", { ms: 300000, kick: [D] });
  const refused = serverLogLines(/method=POST path=\/v2\/feeds\/[0-9a-f.]+\/entries.*status=409/).length - logBefore;
  check(refused >= 1, `the server refused the write of the device that did not know (${refused} request(s) answered 409 feed_closed)`);
  const lateText = async (i) => {
    const id = await pageIdByTitle(i, spaceId, PAGES.plan);
    return id === null ? "" : await pageText(i, id);
  };
  check(!(await lateText(A)).includes(TEXT.lateWrite), "[A] the refused line never reached the closed space");
  const spacesD = await spaceList(D);
  info(`[D] after the refusal D lists ${spacesD.length} spaces; its unsent line ${(await lateText(D)).includes(TEXT.lateWrite) ? "stays in its read-only copy" : "is not in the copy"}`);

  // The banners and the gate: A and B cannot write anything; the copy says why.
  for (const i of [A, B]) {
    await inSharedSpace(i, spaceId);
    await goHome(i);
    await i.waitFor(Q(".archive-banner"), "the archive banner", 60000);
    const banner = await i.evaluate(`norm(document.querySelector(".archive-banner")?.innerText ?? "")`);
    check(banner.startsWith("This space was closed by an Owner. This is a read-only copy; it will not update.") && /Remove from this device/.test(banner), `[${i.tag}] the banner says an Owner closed the space and the copy is read-only (${banner.slice(0, 120)})`);
  }
  await A.capture("close-banner-a");
  await B.capture("close-banner-b");
  const planA = await pageIdByTitle(A, spaceId, PAGES.plan);
  for (const [i, id] of [[A, planA], [B, await pageIdByTitle(B, spaceId, PAGES.plan)]]) {
    const answer = await i.core({ type: "PageRename", data: { pageId: id, title: "Forced into a closed space" } });
    const error = answer.error ?? {};
    check(answer.ok === false && error.code === "Forbidden" && error.details?.reason === "archived", `[${i.tag}] a write into the closed space is refused: Forbidden, archived (${answer.ok ? "ACCEPTED" : `${error.code} ${JSON.stringify(error.details ?? {})}`})`);
  }
  const members = await membersOf(A, spaceId);
  check(typeof members.closedAt === "number", "[A] the member panel says when the space was closed");
  check((await A.ask({ type: "AttachmentList", data: { spaceId } })).every((a) => a.availability === "Complete"), "[A] the files of the copy stay readable");

  // "Remove from this device": B's copy goes, its other space stays.
  const chunksBefore = countChunks(dirsOf("b").data);
  await B.clickEl(T(".archive-banner button", "Remove from this device"), "B's Remove from this device button");
  await B.waitFor(T("h2", `Remove “${SPACE}” from this device?`), "the removal question of the copy");
  const removal = await B.evaluate(`norm(document.querySelector('[role="alertdialog"] p')?.textContent ?? "")`);
  check(/deleted from this device/.test(removal) && /People who still have access keep the space/.test(removal), `[B] the question says what goes (${removal.slice(0, 100)})`);
  await B.clickEl(TD("button", "Remove from this device"), "the confirming button");
  await B.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, "B's question closed", 60000);
  await eventually(async () => (await spaceList(B)).every((s) => s.name !== SPACE), "[B] the copy is gone from B's list", { ms: 120000, kick: [B] });
  check((await spaceList(B)).length === 1, "[B] B keeps its own personal space");
  // The local data goes with the row (the files and the feed's state follow within a cycle or two).
  await eventually(async () => chunksBefore === 0 || countChunks(dirsOf("b").data) < chunksBefore, "[B] the files of the copy are deleted", { ms: 120000, kick: [B] });
  const chunksAfter = countChunks(dirsOf("b").data);
  check(chunksBefore === 0 || chunksAfter < chunksBefore, `[B] the files of the copy went with it (${chunksBefore} chunks before, ${chunksAfter} after)`);
  await eventually(async () => ((await syncStatus(B)).feeds ?? []).every((f) => f.spaceId !== spaceId), "[B] sync no longer lists the space's feed", { ms: 120000, kick: [B] });
  check(true, "[B] sync no longer lists the space's feed");
  result.refused = refused;
  result.chunksBefore = chunksBefore;
  result.chunksAfter = chunksAfter;
  }
}

// ---------------------------------------------------------------------------------------------------------------
// phase: restart, restart-closed
// ---------------------------------------------------------------------------------------------------------------

/** What an instance holds, comparable before and after a restart: every space with its share state, role, members and tree. */
async function fingerprint(i) {
  const out = [];
  for (const space of await spaceList(i)) {
    const entry = { id: space.id, shareState: space.shareState ?? "Personal", role: space.role ?? null, memberCount: space.memberCount ?? 0, aiEnabled: space.aiEnabled !== false, shape: shapeOf(await treeOf(i, space.id)) };
    if (space.shareState === "Shared" || space.shareState === "Archived") {
      const members = await membersOf(i, space.id).catch(() => null);
      entry.members = members === null ? null : members.members.map((m) => `${m.role}:${m.isMe ? "me" : "other"}:${m.verified}`).sort();
      entry.epoch = members?.epoch ?? null;
    }
    out.push(entry);
  }
  return out.sort((x, y) => x.id.localeCompare(y.id));
}

/**
 * Every instance is started again on its own data: spaces, trees, roles, members and the sync state are as before. `live` is after
 * the re-invitation (A owns the shared space, B is an Editor again, C left; a line A writes reaches D, E and B); `closed` is after
 * the close (A, D and E hold the read-only copy, B removed its own, C has none; nothing can be written).
 */
async function phaseRestart(mode) {
  const { spaceId } = readState();
  const names = Object.entries(readState().instances ?? {}).filter(([, rec]) => typeof rec.pid === "number").map(([name]) => name);
  must(names.length >= 4, `the instances of the pass are running (${names.join(", ")})`);
  const before = {};
  for (const name of names) {
    const i = await inst(name);
    before[name] = await fingerprint(i);
  }
  for (const name of names) {
    await stopInstance(name);
    await launchInstance(name);
    const i = await inst(name);
    await i.waitFor(Q(".tab-strip"), "the tab strip", 120000);
    await markDocument(i);
  }
  for (const name of names) {
    const i = await inst(name);
    let after = null;
    await eventually(async () => {
      after = await fingerprint(i);
      return JSON.stringify(after) === JSON.stringify(before[name]);
    }, `[${i.tag}] the spaces, trees, roles and members are as before the restart`, { ms: 120000, kick: [i] });
    check(JSON.stringify(after) === JSON.stringify(before[name]), `[${i.tag}] the spaces, trees, roles and members are as before the restart (${after.length} spaces)`);
  }
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const D = await inst("d");
  const E = await inst("e");
  check((await sharedSpaceOf(C)) === undefined, "[C] C, which left, still has no copy of the space");
  for (const i of [A, D, E]) {
    const s = await syncStatus(i);
    check(s.configured === true && s.state !== "Error" && s.forkDetected !== true, `[${i.tag}] sync is on after the restart (${s.state})`);
  }
  const planOf = async (i) => pageIdByTitle(i, spaceId, PAGES.plan);
  if (mode === "live") {
    check((await sharedSpaceOf(A)).shareState === "Shared" && (await sharedSpaceOf(A)).role === "owner", "[A] the shared space is still shared, A its Owner");
    check((await sharedSpaceOf(B))?.shareState === "Shared" && (await sharedSpaceOf(B)).role === "editor", "[B] B is still an Editor of the space it came back to");
    // The restarted devices go on syncing: a line A writes reaches D, E and B.
    const line = TEXT.afterRestart;
    await openShared(A, spaceId, PAGES.plan);
    await writeLine(A, line);
    await eventually(
      async () => (await pageText(D, await planOf(D))).includes(line) && (await pageText(E, await planOf(E))).includes(line) && (await pageText(B, await planOf(B))).includes(line),
      "[D] [E] [B] get the line A wrote after the restart",
      { ms: 240000, kick: [A, B, D, E] },
    );
    check(true, "[D] [E] [B] get the line A wrote after the restart");
  } else {
    for (const i of [A, D, E]) {
      const s = await sharedSpaceOf(i);
      check(s?.shareState === "Archived" && s.archiveReason === "Closed", `[${i.tag}] the closed space is still a read-only copy after the restart (${s?.shareState}, ${s?.archiveReason})`);
    }
    check((await sharedSpaceOf(B)) === undefined, "[B] B, which removed its copy, has none after the restart");
    await inSharedSpace(A, spaceId);
    await goHome(A);
    await A.waitFor(Q(".archive-banner"), "the archive banner after the restart", 60000);
    check(/This space was closed by an Owner/.test(await A.evaluate(`norm(document.querySelector(".archive-banner")?.innerText ?? "")`)), "[A] the archive banner of the closed space is there after the restart");
    const answer = await A.core({ type: "PageCreate", data: { spaceId, title: "Forced into a closed space" } });
    check(answer.ok === false && answer.error?.code === "Forbidden" && answer.error?.details?.reason === "archived", "[A] a write into the closed space is still refused after the restart");
    for (const i of [A, D, E]) {
      const text = (await pageText(i, await planOf(i))) ?? "";
      check(text.includes(TEXT.plan), `[${i.tag}] the pages of the copy are readable after the restart`);
    }
  }
  result.details = { mode, restarted: names };
}

// ---------------------------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------------------------

const PHASE_FUNCS = {
  launch: phaseLaunch,
  share: phaseShare,
  code: phaseCode,
  roles: phaseRoles,
  link: phaseLink,
  remove: phaseRemove,
  leave: phaseLeave,
  reinvite: phaseReinvite,
  devices: phaseDevices,
  restart: () => phaseRestart("live"),
  close: phaseClose,
  "restart-closed": () => phaseRestart("closed"),
};

try {
  const run = PHASE_FUNCS[phase];
  if (run === undefined) throw new Error(`unknown phase ${phase}`);
  await run();
} catch (error) {
  if (!(error instanceof PhaseStop)) check(false, `exception: ${error instanceof Error ? (error.stack ?? error.message).split("\n").slice(0, 4).join(" | ") : String(error)}`);
}

// The pages of this phase: the documents were not replaced, nothing asked another host, nothing violated the policy.
// The origins the app itself talks to: its own pages and assets, the IPC bridge, the media scheme. Nothing else: the pages never
// talk to the server (the shell does).
const ALLOWED = /^(?:https?:\/\/(?:tauri\.localhost|ipc\.localhost|kbmedia\.localhost)|tauri:\/\/localhost|ipc:\/\/localhost|kbmedia:\/\/localhost|data:|blob:|about:)/u;
await wait(500);
for (const [name, i] of Object.entries(insts)) {
  if (phase !== "launch") {
    try {
      await intact(i, `end of ${phase}`);
    } catch (error) {
      check(false, `[${i.tag}] exception at the end: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  const report = await i.report();
  const foreign = report.requests.filter((r) => !ALLOWED.test(r.url));
  const hardFailures = report.failures.filter((f) => f.canceled !== true && f.error !== "net::ERR_ABORTED");
  const errors = report.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
  result.instances[name] = {
    requests: report.requests.length,
    foreignRequests: foreign,
    failures: report.failures,
    cspViolations: report.violations,
    console: report.console.filter((c) => c.type !== "log" && c.type !== "info" && c.type !== "debug"),
  };
  check(foreign.length === 0, `[${i.tag}] no request to another host (${foreign.length}${foreign.length > 0 ? ": " + foreign.map((f) => f.url.slice(0, 80)).slice(0, 3).join(" | ") : ""})`);
  check(report.violations.length === 0, `[${i.tag}] no content-security-policy violation (${report.violations.length}${report.violations.length > 0 ? ": " + report.violations.map((v) => `${v.directive} ${String(v.blocked).slice(0, 60)}`).slice(0, 3).join(" | ") : ""})`);
  check(hardFailures.length === 0, `[${i.tag}] no failed request (${hardFailures.length}${hardFailures.length > 0 ? ": " + hardFailures.map((f) => f.error).slice(0, 3).join(" | ") : ""})`);
  check(errors.length === 0, `[${i.tag}] no console error (${errors.length}${errors.length > 0 ? ": " + errors.map((e) => e.text.replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, "<id>")).slice(0, 3).join(" | ") : ""})`);
}
if (!result.ok) {
  for (const i of Object.values(insts)) await i.capture(`${phase}-failed-${i.name}`).catch(() => {});
}
result.seconds = round((Date.now() - started) / 1000);
if (outFile) writeFileSync(outFile, JSON.stringify(result, null, 2));
// The report file has every note; the console has what failed (or, with M13_VERBOSE, everything).
const checks = result.notes.filter((n) => !n.startsWith("info:"));
console.log(
  JSON.stringify(
    {
      phase,
      ok: result.ok,
      seconds: result.seconds,
      checks: checks.length,
      failed: checks.filter((n) => n.startsWith("FAIL")),
      notVerifiable: checks.filter((n) => n.startsWith("NOT VERIFIABLE")),
      bugs: result.bugs,
      captures: result.captures,
      ...(process.env.M13_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
for (const i of Object.values(insts)) i.close();
process.exit(result.ok ? 0 : 1);
