// The in-page half of the comments pass (drive-m13-comments.ps1, Milestone 13 slice 2): trusted mouse and key input, the DOM, the core
// (through the pages' own IPC), the Windows notification database, the processes and the logs, over the DevTools ports of several app
// instances at once (the calls of cdp.mjs, one connection per instance; the infrastructure is that of drive-m13-share.mjs). Phases:
//
//   node drive-m13-comments.mjs setup     <outfile>  a fresh server, A, B and C on fresh data, A shares a space, invites B (Editor) and C (Commenter) by code
//   node drive-m13-comments.mjs comments  <outfile>  anchored comments, replies, roles, delete, resolve, show resolved, anchors that survive edits
//   node drive-m13-comments.mjs mentions  <outfile>  @ in a page and in a comment, own changes, E (A's second device) reads the history silently
//   node drive-m13-comments.mjs person    <outfile>  the Person property: assigning B notifies B; the "me" filter of a table view
//   node drive-m13-comments.mjs reminders <outfile>  "Remind: B" fires on B's devices only
//   node drive-m13-comments.mjs search    <outfile>  a word of a comment is found "in comment"; the export has @label and no comment
//   node drive-m13-comments.mjs languages <outfile>  B in French: the notification texts and the inbox
//   node drive-m13-comments.mjs restart   <outfile>  every instance started again: the state survives
//   node drive-m13-comments.mjs former    <outfile>  A removes B: what B wrote reads "Former member (name)" on A and C
//   node drive-m13-comments.mjs logs      <outfile>  the logs of the server and of every instance
//   node drive-m13-comments.mjs verify    <outfile>  the summary of every phase
//   node drive-m13-comments.mjs stop      <outfile>  stops the instances this pass started (by process id)
//   node drive-m13-comments.mjs os-watch            the watcher of the system's notification database (started by setup, ended by stop)
//
// Every page phase writes what each page asked the network for, what failed, the console and the content-security-policy violations
// it saw, as JSON, and exits 1 when a check fails, when a page asked another host for something (the pages never talk to the server:
// the shell does), when a violation, a failed request or a console error was seen, or when the document of a webview was replaced
// (the marker `window.__m13cMarker` is lost by a reload). The invite codes and the kit words are kept in the state file for the log
// scan, and never written to a report.

import { spawn } from "node:child_process";
import { appendFileSync, copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.M13C_OUT_DIR ?? join(HERE, "out");
const SHARE_PS1 = process.env.M13C_SHARE_PS1 ?? join(HERE, "drive-m13-share.ps1");
const BASE_PORT = Number(process.env.M13C_BASE_PORT ?? 9433);
const STATE_FILE = join(OUT, "m13-comments-state.json");
/** What the watcher of the system's notification database writes, and the file that tells it to stop (see `startOsWatcher`). */
const OS_LOG = join(OUT, "m13-comments-os.jsonl");
const OS_STOP = join(OUT, "m13-comments-os.stop");
const SERVER_URL = process.env.M13C_SERVER_URL ?? "http://127.0.0.1:8797";
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
function addNeedles(patch) {
  const state = readState();
  writeState({ needles: { ...(state.needles ?? {}), ...patch } });
}

// What this pass types and shows: none of it may be in a log (names, texts, titles).
const SPACE = "Orchard Commons";
const PAGES = { plan: "Orchard plan", harvest: "Harvest log", tasks: "Orchard tasks", taskOne: "Graft the plum", taskTwo: "Paint the fence" };
const PEOPLE = { a: "Alma Quillfeather", b: "Boris Thistledown", c: "Clara Wintergreen" };
const WORKSPACES = { a: "Quill Alpha", b: "Quill Beta", c: "Quill Gamma", e: "Quill Epsilon" };
const TEXT = {
  p1: "Prune the pear trees before the first frost.",
  p2: "Conference pears keep best in a cool cellar.",
  harvest: "Apples are picked from the north rows first.",
  harvestB: "Boris counts the crates after lunch.",
  tasks: "Jobs for the cold months are listed below.",
};
const ANCHOR1 = "pear trees";
const ANCHOR2 = "cool cellar";
const ANCHOR3 = "keep best";
const COMMENT = {
  a1: "Should the pruning start before the frost date?",
  b1: "Earlier is fine, the buds are already swelling.",
  b1e: "Earlier is fine, the buds are swelling already.",
  c1: "The cellar felt damp this autumn, so watch the humidity.",
  a2: "Thanks for the cellar warning, we will buy a dehumidifier.",
  a3: "Agreed, we start the pruning next Monday.",
  a4: "One more thing about the cellar, the door seal is loose.",
  a5: "Is this the right variety to store through the winter?",
  aToB: "please read the ladder list before Friday",
  aMention: "can you measure the cellar humidity tomorrow?",
  bMention: "remember the ladder when you prune",
  self: "a note I keep for my own list",
  searchWord: "dehumidifier",
};
const SELF_DISPLAY = "Alma";

// ---------------------------------------------------------------------------------------------------------------
// helper steps of the .ps1 files (native dialogs, captures, onboarding, launching, killing, the notification database)
// ---------------------------------------------------------------------------------------------------------------

let helperCount = 0;
/** Runs one of drive-m13-share.ps1's helper steps (dialog, capture, onboard, spawn, kill, ...); its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}, extraEnv = {}) {
  helperCount += 1;
  const argFile = join(OUT, `m13-comments-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `m13-comments-helper-${process.pid}-${helperCount}.result.json`);
  writeFileSync(argFile, JSON.stringify(params));
  rmSync(resultFile, { force: true });
  // No pipes: the app a helper starts would inherit them, and the helper would not be finished until the app ends. The helper's answer is
  // its result file; a helper that fails before writing one is told by its exit code.
  return new Promise((resolvePromise, reject) => {
    const child = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", SHARE_PS1, "-Step", step, "-Arg", argFile, "-Result", resultFile], {
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
// os-watch: the watcher of the system's notification database (started by `setup`, see `startOsWatcher`)
// ---------------------------------------------------------------------------------------------------------------

if (phase === "os-watch") {
  const log = (entry) => appendFileSync(OS_LOG, `${JSON.stringify(entry)}\n`);
  let sqlite = null;
  try {
    sqlite = await import("node:sqlite");
  } catch (error) {
    log({ error: `node:sqlite is not available (${error instanceof Error ? error.message : String(error)})` });
    process.exit(0);
  }
  const dbPath = join(process.env.LOCALAPPDATA ?? "", "Microsoft", "Windows", "Notifications", "wpndatabase.db");
  const seen = new Set();
  const deadline = Date.now() + 3 * 3600 * 1000;
  let first = true;
  let failures = 0;
  const unescape = (s) => s.replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, String.fromCharCode(34)).replace(/&apos;/g, String.fromCharCode(39));
  while (!existsSync(OS_STOP) && Date.now() < deadline) {
    try {
      const db = new sqlite.DatabaseSync(dbPath, { readOnly: true });
      const rows = db.prepare("SELECT Id, Payload, CAST(ArrivalTime AS TEXT) AS Arrival FROM Notification").all();
      db.close();
      failures = 0;
      for (const r of rows) {
        const key = `${r.Id}:${r.Arrival}`;
        if (seen.has(key)) continue;
        seen.add(key);
        if (first || r.Payload === null || r.Payload === undefined) continue; // what was there before the watcher started is not this pass's
        const xml = Buffer.from(r.Payload).toString("utf8");
        const texts = [...xml.matchAll(/<text[^>]*>([^<]*)<\/text>/g)].map((m) => unescape(m[1]));
        log({ id: Number(r.Id), title: texts[0] ?? "", body: texts.slice(1).filter((x) => x !== "").join(" | "), seenAt: Date.now() });
      }
      if (first) {
        first = false;
        log({ started: Date.now(), existing: rows.length });
      }
    } catch (error) {
      // The database is busy for a moment: try again; a file that never opens is reported.
      failures += 1;
      if (first && failures >= 20) {
        log({ error: error instanceof Error ? error.message : String(error) });
        process.exit(0);
      }
    }
    await wait(250);
  }
  process.exit(0);
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
  writeFileSync(OS_STOP, "stop"); // the watcher of the notification database ends
  console.log(JSON.stringify({ phase, stopped }));
  process.exit(0);
}

// ---------------------------------------------------------------------------------------------------------------
// logs: the scan of the logs of the server and of every instance
// ---------------------------------------------------------------------------------------------------------------

const stripAnsi = (text) => text.replace(/\u001b\[[0-9;]*m/g, "");
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
/** The log files of the pass: every instance's (`m13-comments-a.err.log`, `-r1`, and the stdout files) and the server's. */
const logFiles = () => [
  ...readdirSync(OUT).filter((n) => /^m13-comments-[a-e](-r\d+)?\.(err|out)\.log$/.test(n)).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })),
  ...readdirSync(OUT).filter((n) => /^m13-comments-server-.*\.log$/.test(n)),
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
    "Wintergreen",
    "Alma",
    "Boris",
    "Clara",
    ...Object.values(TEXT).flatMap((text) => [text, text.split(" ").slice(0, 4).join(" ")]),
    ...Object.values(COMMENT).flatMap((text) => [text, text.split(" ").slice(0, 4).join(" ")]),
    ANCHOR1,
    ANCHOR2,
    ANCHOR3,
    ...(state.needles?.extra ?? []),
  ];
  const secret = [];
  for (const key of ["codeB", "codeC"]) if (state.needles?.[key]) secret.push({ label: `the invite code (${key})`, value: state.needles[key] }, { label: `the invite code (${key}, grouped)`, value: state.needles[key].replace(/(.{4})(?=.)/g, "$1 ") });
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
    // The name of a pairing mailbox (6 bytes, 12 hex digits) is a rendezvous name, not an id (`/v2/pair/<mailbox>`).
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
  result.notes.push(`${log.leaks.length === 0 ? "ok" : "FAIL"}: none of the comment texts, page texts, titles, space names, display names, workspace names, invite codes or kit words is in a log (${log.leaks.length} leaks${log.leaks.length ? `: ${log.leaks.map((l) => `${l.needle} x${l.count} in ${l.file}`).join(", ")}` : ""}; ${log.needles} needles)`);
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

const PHASES = ["setup", "comments", "mentions", "person", "reminders", "search", "languages", "restart", "former", "logs"];
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], notVerifiable: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0, bugs: [], details: {}, log: null };
  for (const name of PHASES) {
    const file = join(OUT, `m13-comments-${name}.json`);
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
    const path = join(OUT, `m13-comments-${label}.png`);
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
const dirsOf = (name) => ({ data: join(OUT, `m13-comments-${name}-data`), webview: join(OUT, `m13-comments-${name}-webview`) });

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
  const stem = launches === 0 ? `m13-comments-${name}` : `m13-comments-${name}-r${launches}`;
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
  const href = await i.evaluate(`(() => { window.__m13cMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  patchInstance(i.name, { marker, href });
}

/** The webview's own document: nothing may have replaced it. */
async function intact(i, when) {
  const rec = i.rec;
  const now = await i.evaluate(`(() => ({ marker: window.__m13cMarker ?? null, href: location.href }))()`).catch(() => null);
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

const kitFileOf = (name) => join(OUT, `m13-comments-kit-${name}.txt`);

/** The workspace and its kit, by UI Automation (the helper of the .ps1); the kit words are kept for the recovery and the log scan. */
async function onboard(name) {
  const rec = readState().instances?.[name] ?? {};
  let answer = await helper("onboard", { pid: rec.pid, workspace: WORKSPACES[name], kitFile: kitFileOf(name) });
  // Three windows driven at once by UI Automation: a screen that is not in the accessibility tree yet is looked for again, alone.
  for (let attempt = 1; attempt <= 3 && answer.ok !== true && /no .Workspace name. field/.test(answer.error ?? ""); attempt++) {
    info(`${name.toUpperCase()}: the onboarding screen was not found by UI Automation (attempt ${attempt}): trying again`);
    await wait(4000);
    answer = await helper("onboard", { pid: rec.pid, workspace: WORKSPACES[name], kitFile: kitFileOf(name) });
  }
  check(answer.ok === true, `${name.toUpperCase()} onboarded by UI Automation (a workspace and a recovery kit${answer.error ? `: ${answer.error}` : ""})`);
  if (answer.ok !== true) throw new PhaseStop(`onboarding of ${name}`);
  const words = readFileSync(kitFileOf(name), "utf8").trim().split(/\s+/);
  check(words.length === 24, `${name.toUpperCase()}'s kit has 24 words`);
  const state = readState();
  writeState({ needles: { ...(state.needles ?? {}), kit: { ...(state.needles?.kit ?? {}), [name]: words } } });
}

// ---------------------------------------------------------------------------------------------------------------
// what this pass adds to the shared infrastructure: drags and selections, the comments panel, the inbox, the OS notifications
// ---------------------------------------------------------------------------------------------------------------

const bodyText = (i) => i.evaluate(`document.body.innerText`);

/** A trusted drag from one point to another with the left button (a text selection). */
Inst.prototype.drag = async function drag(x0, y0, x1, y1) {
  await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x0, y: y0 });
  await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x: x0, y: y0, button: "left", buttons: 1, clickCount: 1 });
  const steps = 6;
  for (let k = 1; k <= steps; k++) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x0 + ((x1 - x0) * k) / steps, y: y0 + ((y1 - y0) * k) / steps, button: "left", buttons: 1 });
  }
  await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x1, y: y1, button: "left", buttons: 0, clickCount: 1 });
  await wait(300);
};

const KEYS = {
  Enter: ["Enter", "Enter", 13],
  Escape: ["Escape", "Escape", 27],
  Backspace: ["Backspace", "Backspace", 8],
  Home: ["Home", "Home", 36],
  End: ["End", "End", 35],
  ArrowRight: ["ArrowRight", "ArrowRight", 39],
  ArrowLeft: ["ArrowLeft", "ArrowLeft", 37],
  ArrowDown: ["ArrowDown", "ArrowDown", 40],
  Tab: ["Tab", "Tab", 9],
};
Inst.prototype.press = async function press(name, modifiers = 0, times = 1) {
  const [key, code, vk] = KEYS[name];
  for (let k = 0; k < times; k++) await this.key(key, code, vk, modifiers);
};

/** The page's selection as text. */
const selectionText = (i) => i.evaluate(`String(window.getSelection()?.toString() ?? "")`);

/** The point on the first character and the point after the last one of `phrase` in the open page's text (scrolled into view), or null. */
function phraseBox(i, phrase) {
  return i.run(`
    const walker = document.createTreeWalker(document.querySelector(".ProseMirror"), NodeFilter.SHOW_TEXT);
    let n;
    while ((n = walker.nextNode())) {
      const at = n.textContent.indexOf(${JSON.stringify(phrase)});
      if (at < 0) continue;
      n.parentElement.scrollIntoView({ block: "center" });
      const rect = (from, to) => { const r = document.createRange(); r.setStart(n, from); r.setEnd(n, to); return r.getBoundingClientRect(); };
      const first = rect(at, at + 1);
      const last = rect(at + ${phrase.length} - 1, at + ${phrase.length});
      const all = rect(at, at + ${phrase.length});
      return { x0: first.left + 1, y0: first.top + first.height / 2, x1: last.right - 1, y1: last.top + last.height / 2, cx: all.left + all.width / 2, cy: all.top + all.height / 2 };
    }
    return null;`);
}

/** Selects `phrase` in the open page with a trusted drag of the mouse (it works in a read-only editor too). */
async function selectPhrase(i, phrase) {
  let got = "";
  // The end of the drag falls on a character's half: a drag that stops short is repeated a little further on.
  for (const extra of [0, 2, 4, -3]) {
    const box = await phraseBox(i, phrase);
    must(box !== null, `[${i.tag}] the words "${phrase}" are in the open page (in one piece of text)`);
    await i.drag(box.x0, box.y0, box.x1 + extra, box.y1);
    got = await selectionText(i);
    if (got === phrase) break;
  }
  must(got === phrase, `[${i.tag}] the drag selected "${phrase}" (${got})`);
}

/** Selects a word with a trusted double click; returns what was selected (Windows adds the space after a word). */
async function selectWord(i, word) {
  const box = await phraseBox(i, word);
  must(box !== null, `[${i.tag}] the word "${word}" is in the open page`);
  await i.mouse(box.cx, box.cy, { clickCount: 2 });
  const got = await selectionText(i);
  must(got.trim() === word, `[${i.tag}] the double click selected "${word}" (${got})`);
  return got;
}

/** Replaces a word with another one: a double click, then the typed text (and the space the selection took with it). */
async function replaceWord(i, word, replacement) {
  const got = await selectWord(i, word);
  await i.type(replacement + (got.endsWith(" ") ? " " : ""));
}

/** Opens the page of the shared space by its title (the shared space is made the current one first). */
async function inSharedSpace(i, spaceId) {
  const spaces = await spaceList(i);
  const space = spaces.find((s) => s.id === spaceId);
  must(space !== undefined, `[${i.tag}] the shared space is in the list`);
  if ((await shownSpace(i)) !== space.name) await switchSpace(i, spaceId);
  await normalize(i);
}

async function openShared(i, spaceId, title) {
  await inSharedSpace(i, spaceId);
  const id = await pageIdByTitle(i, spaceId, title);
  must(id !== null, `[${i.tag}] the page "${title}" is in the tree`);
  await openPageId(i, spaceId, id);
  await i.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(title)}`, `"${title}" is open`);
  return id;
}

/** What a page shows about being read-only: the mark, the editor, the title, the structure controls and the properties. */
const readOnlyFacts = (i) =>
  i.run(`
    const pm = document.querySelector(".ProseMirror");
    return {
      pill: norm(document.querySelector(".role-lock")?.textContent ?? ""),
      editable: pm?.getAttribute("contenteditable") ?? null,
      titleButton: !!document.querySelector(".page-title-button"),
      newPage: !!document.querySelector(".tree-new"),
      propertiesAdd: !!document.querySelector(".properties-add"),
      formatMenu: !!document.querySelector(".format-menu, .format-toolbar"),
      toasts: [...document.querySelectorAll(".toasts .toast-text")].map((e) => norm(e.textContent)),
    };`);

/** The page's blocks (ProseMirror JSON) with their ids: `{ id, type, text, nodes: [types of the inline nodes] }`. */
async function blocksOf(i, pageId) {
  const content = (await i.ask({ type: "PageContentGet", data: { pageId } })).content;
  const blocks = [];
  const walk = (node) => {
    if (node.attrs?.id !== undefined && node.type !== "doc") {
      const inline = [];
      const collect = (n) => {
        if (n.type !== undefined && !["paragraph", "heading", "text"].includes(n.type)) inline.push({ type: n.type, attrs: n.attrs ?? {} });
        for (const c of n.content ?? []) collect(c);
      };
      for (const c of node.content ?? []) collect(c);
      blocks.push({ id: node.attrs.id, type: node.type, text: textOf(node), inline });
    }
    for (const c of node.content ?? []) walk(c);
  };
  walk(content);
  return blocks;
}

// ----- the comments panel ------------------------------------------------------------------------------------------

const PANEL = `document.querySelector(".comments-panel")`;
/** The card of the thread whose quote is `quote` (the panel lists it while it is open, or resolved with "Show resolved"). */
const CARD = (quote) => `[...document.querySelectorAll(".comments-panel .comment-thread")].find((li) => norm(li.querySelector(".comment-quote")?.textContent ?? "") === ${JSON.stringify(quote)})`;
/** The row of a comment in a card: the one whose author is `author` and whose text includes `text`. */
const ROW = (card, author, text) => `[...(${card})?.querySelectorAll(".comment") ?? []].find((li) => norm(li.querySelector(".comment-author")?.textContent ?? "") === ${JSON.stringify(author)} && norm(li.querySelector(".comment-body")?.textContent ?? "").includes(${JSON.stringify(text)}))`;

async function openCommentsPanel(i) {
  if (await i.run(`return !!${PANEL};`)) return;
  await i.clickEl(Q(".page-comments-button"), "the page header's Comments button");
  await i.waitFor(Q(".comments-panel"), "the comments panel");
}

async function closeCommentsPanel(i) {
  if (!(await i.run(`return !!${PANEL};`))) return;
  await i.clickEl(Q('.comments-panel button[aria-label="Close the comments"]'), "the panel's close button");
  await i.waitUntil(`!${PANEL}`, "the comments panel closed");
}

/** The threads of the page as the core lists them (open and resolved). */
const threadsOf = (i, pageId) => i.ask({ type: "CommentList", data: { pageId } });

/** Types a comment into the composer whose field has `label` (a trusted click, then the text) and sends it with the button or with Enter. */
async function writeComment(i, scope, label, text, how = "button") {
  const field = `${scope} textarea[aria-label=${JSON.stringify(label)}]`;
  await i.clickEl(`document.querySelector(${JSON.stringify(field)})`, `the "${label}" field`);
  await i.insertText(text);
  await wait(200);
  const value = await i.run(`return document.querySelector(${JSON.stringify(field)})?.value ?? null;`);
  must(value === text, `[${i.tag}] the "${label}" field holds what was typed`);
  if (how === "enter") {
    await i.press("Enter");
  } else {
    await i.clickEl(`document.querySelector(${JSON.stringify(field)}).closest("form").querySelector('button[type="submit"]')`, `the send button of "${label}"`);
  }
  await wait(400);
}

// ----- notifications -----------------------------------------------------------------------------------------------

const unreadCount = async (i) => (await i.ask({ type: "NotificationList", data: { unreadOnly: true } })).length;
const inboxRowsCore = (i) => i.ask({ type: "NotificationList", data: { limit: 200 } });
const badgeText = (i) => i.evaluate(`document.querySelector(".notifications-badge")?.textContent.trim() ?? ""`);
const bellLabel = (i) => i.evaluate(`document.querySelector("button.notifications-bell")?.getAttribute("aria-label") ?? ""`);
const toastTexts = (i) => i.evaluate(`[...document.querySelectorAll(".toasts .toast-text")].map((e) => norm(e.textContent))`);

/** The toasts every instance showed so far (they go after 15 s, so a phase keeps what it saw). */
const toastLog = {};
async function noteToasts(i) {
  const seen = (toastLog[i.name] ??= new Set());
  for (const text of await toastTexts(i).catch(() => [])) seen.add(text);
  return seen;
}

/** Waits until a toast whose text matches `pattern` was shown on `i` (asking the instances in `kick` to sync); returns its text. */
async function waitToast(i, pattern, what, { ms = 120000, kick = [] } = {}) {
  let found = null;
  await eventually(
    async () => {
      const seen = await noteToasts(i);
      found = [...seen].find((text) => pattern.test(text)) ?? null;
      return found !== null;
    },
    `[${i.tag}] ${what}`,
    { ms, kick },
  );
  return found;
}

/** Waits until the core's unread count on `i` is `n`. */
const waitUnread = (i, n, what, opts = {}) => eventually(async () => (await unreadCount(i)) === n, `[${i.tag}] ${what}`, { ms: opts.ms ?? 120000, kick: opts.kick ?? [] });

/** Opens the bell's inbox (a trusted click) and returns its rows: `{ text, unread }`. */
async function openInbox(i) {
  if (!(await i.run(`return !!document.querySelector(".inbox");`))) {
    await normalize(i);
    await i.clickEl(Q("button.notifications-bell"), "the notifications bell");
    await i.waitFor(Q(".inbox"), "the inbox");
    await wait(500);
  }
  return inboxRows(i);
}
const inboxRows = (i) => i.evaluate(`[...document.querySelectorAll(".inbox .inbox-row")].map((li) => ({ text: norm(li.querySelector(".inbox-text")?.textContent ?? ""), unread: li.classList.contains("is-unread") }))`);
async function closeInbox(i) {
  if (!(await i.run(`return !!document.querySelector(".inbox");`))) return;
  await i.press("Escape");
  await i.waitUntil(`!document.querySelector(".inbox")`, "the inbox closed", 8000).catch(() => null);
  await normalize(i);
}

// ----- the Windows notification database (the OS notifications) ------------------------------------------------------
//
// The shell shows an OS notification through the system's toast API. The system keeps the toasts of an app in its notification
// database (wpndatabase.db), but removes some of them again within seconds, so a snapshot taken after the action can miss them: a
// watcher process (this script's `os-watch` mode, started by `setup`) polls the database every 250 ms and appends every toast it
// sees, with the time it saw it, to out/m13-comments-os.jsonl; the checks read that record.

/** Starts the watcher (an earlier one is told to stop first); it ends by itself when the stop file appears. */
async function startOsWatcher() {
  writeFileSync(OS_STOP, "stop");
  await wait(1500);
  rmSync(OS_STOP, { force: true });
  rmSync(OS_LOG, { force: true });
  const child = spawn(process.execPath, ["--no-warnings", fileURLToPath(import.meta.url), "os-watch"], {
    detached: true,
    stdio: "ignore",
    windowsHide: true,
    env: { ...process.env, M13C_OUT_DIR: OUT },
  });
  child.unref();
  writeState({ osWatcherStartedAt: Date.now() });
  await wait(1500);
}

function stopOsWatcher() {
  writeFileSync(OS_STOP, "stop");
}

/** The toasts the watcher saw: `{ id, title, body, seenAt }`, or `null` (with why) when there is no usable record. */
function osNotifications() {
  if (!existsSync(OS_LOG)) return { rows: null, why: "the notification watcher left no record (it did not start)" };
  const rows = [];
  let started = false;
  let error = null;
  for (const line of readFileSync(OS_LOG, "utf8").split("\n")) {
    if (line.trim() === "") continue;
    try {
      const entry = JSON.parse(line);
      if (entry.started) started = true;
      else if (entry.error) error = entry.error;
      else rows.push(entry);
    } catch {
      // a line being written
    }
  }
  if (error !== null) return { rows: null, why: `the notification database cannot be read (${error})` };
  if (!started) return { rows: null, why: "the notification watcher did not start" };
  return { rows };
}

/** The time now, as the mark that `waitOsNotification` counts from. */
const osMark = () => Date.now();

/**
 * The OS notifications whose body matches `pattern` that the watcher saw after `since` (a mark of `osMark`), waiting up to `ms` for
 * `count` of them. Returns `{ verifiable, rows }`; `verifiable` is false when there is no record at all.
 */
async function waitOsNotification(pattern, { since = 0, ms = 30000, count = 1 } = {}) {
  const end = Date.now() + ms;
  let last = { rows: null };
  for (;;) {
    last = osNotifications();
    if (last.rows === null) return { verifiable: false, why: last.why, rows: [] };
    const hits = last.rows.filter((r) => r.seenAt >= since && pattern.test(r.body));
    if (hits.length >= count || Date.now() >= end) return { verifiable: true, rows: hits, all: last.rows };
    await wait(500);
  }
}

/**
 * A check on the OS notification of a row: its body names the actor and the page, its title is the page's, and it holds no comment text.
 * Marks it NOT VERIFIABLE when the database is unreadable or the system showed nothing (notifications off, a locked session).
 */
function checkOsNotification(found, { what, title, bodyPattern, forbidden = [] }) {
  if (!found.verifiable) {
    notVerifiable(`${what}: ${found.why}`);
    return;
  }
  if (found.rows.length === 0) {
    notVerifiable(`${what}: the system's notification database shows no such notification (Windows may have notifications off or a focus mode on; ${found.all?.length ?? 0} rows in it)`);
    return;
  }
  const row = found.rows[0];
  check(row.title === title, `${what}: the OS notification's title is the page's (${row.title})`);
  check(bodyPattern.test(row.body), `${what}: the OS notification's body is the phrase with the actor's name (${row.body})`);
  const all = `${row.title} ${row.body}`;
  check(forbidden.every((text) => !all.includes(text)), `${what}: the OS notification holds no comment text`);
}

// ---------------------------------------------------------------------------------------------------------------
// the invitation by code (the share phases of drive-m13-share.mjs, for a chosen role)
// ---------------------------------------------------------------------------------------------------------------

async function inviteByCode(A, J, spaceId, { role, name, codeKey }) {
  const tag = J.tag;
  await J.ask({ type: "ProfileSet", data: { displayName: name } });
  check((await syncStatus(J)).configured === false, `[${tag}] has no account on the server and no sync configuration before it joins`);
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  await A.clickEl(T("button", "Invite with a code..."), `the "Invite with a code..." button`);
  await A.waitFor(T("h2", "Invite with a code"), "the invitation dialog");
  if (role !== "Editor") await chooseOption(A, L("Role of the new member", true), role, `the role of ${tag}`);
  const shown = await A.run(`const s = ${L("Role of the new member", true)}; return s ? norm(s.options[s.selectedIndex].textContent) : null;`);
  check(shown === role, `[A] the invitation is for the role ${role} (${shown})`);
  await A.clickEl(TD("button", "Show the code"), "the Show the code button");
  await A.waitFor(Q(".pair-code"), "the invitation code", 60000);
  const code = await A.evaluate(`document.querySelector(".pair-code").textContent.replace(/\\s/g, "")`);
  check(/^[0-9A-Z]{12}$/.test(code), `[A] the code has 12 characters (${code.length})`);
  addNeedles({ [codeKey]: code });
  await openSpaceList(J);
  await J.clickEl(T(".space-popover button.menu-item", "Join a shared space..."), `"Join a shared space..." in the list`);
  await J.waitFor(T("h2", "Join a shared space"), "the Join a shared space dialog");
  await J.fill(L("Server address", true), "the server address field", SERVER_URL);
  await J.fill(L("Code", true), "the code field", code);
  await J.clickEl(TD("button", "Continue"), "the Continue button");
  let sawTokenPrompt = false;
  let digitsA = null;
  let digitsJ = null;
  await eventually(
    async () => {
      const text = await bodyText(J);
      if (/invite token/i.test(text)) sawTokenPrompt = true;
      const stopped = /Joining stopped: ([^\n]*)/.exec(text);
      if (stopped) must(false, `[${tag}] joining stopped: ${stopped[1]}`);
      digitsA = await digitsOf(A);
      digitsJ = await digitsOf(J);
      return digitsA !== null && digitsJ !== null;
    },
    "both windows show the six digits",
    { ms: 120000 },
  );
  check(/^\d{6}$/.test(digitsA ?? "") && digitsA === digitsJ, `the six digits of A and of ${tag} are the same`);
  await J.clickEl(TD("button", "Confirm"), `${tag}'s Confirm button`);
  await A.waitUntil(`${DIALOG_TEXT}.includes("You are inviting")`, "A's dialog says who is being invited", 90000);
  const inviting = await A.evaluate(`document.querySelector(".invite-dialog .pair-peer")?.textContent.trim() ?? ""`);
  check(inviting.includes(name), `[A] the dialog names the person being invited (${tag})`);
  await A.clickEl(TD("button", "Confirm"), "A's Confirm button");
  const joinedRe = new RegExp(`joined as ${role}`);
  await eventually(
    async () => {
      const a = await A.evaluate(`norm(document.querySelector(".invite-dialog")?.innerText ?? "")`);
      const b = await J.evaluate(`document.body.innerText`);
      if (/The invitation stopped/.test(a)) must(false, `[A] the invitation stopped: ${a.slice(0, 200)}`);
      if (/Joining stopped/.test(b)) must(false, `[${tag}] joining stopped: ${b.slice(b.indexOf("Joining stopped"), b.indexOf("Joining stopped") + 160)}`);
      return joinedRe.test(a) && /You joined/.test(b);
    },
    `both windows say that ${tag} joined as ${role}`,
    { ms: 300000 },
  );
  check(sawTokenPrompt === false, `[${tag}] never showed "needs invite token" (the share signup made its account)`);
  await closeDialog(A, "Close"); // the invitation dialog
  await closeDialog(A, "Close"); // the member panel under it
  await J.clickEl(TD("button", "Open the space"), `${tag}'s Open the space button`);
  await J.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, `${tag}'s join dialog closed`, 20000);
  await eventually(async () => (await sharedSpaceOf(J)) !== undefined, `[${tag}] the shared space is in ${tag}'s list`, { ms: 60000 });
  let selected = false;
  for (let k = 0; k < 40 && !selected; k++) {
    selected = (await shownSpace(J)) === SPACE;
    if (!selected) await wait(250);
  }
  if (!selected) {
    bug(`"Open the space" of ${tag} closed the join dialog without opening the space`);
    await inSharedSpace(J, spaceId);
  }
  const space = await sharedSpaceOf(J);
  check(space.id === spaceId && space.shareState === "Shared" && space.role === role.toLowerCase(), `[${tag}] ${tag} sees the shared space as ${role} (${space.shareState}, ${space.role})`);
}

// ---------------------------------------------------------------------------------------------------------------
// phase: setup
// ---------------------------------------------------------------------------------------------------------------

async function phaseSetup() {
  rmSync(STATE_FILE, { force: true }); // a fresh pass starts with no memory of an earlier one
  writeState({ started: new Date().toISOString(), instances: {}, needles: { kit: {} } });
  const session = await helper("session");
  info(`the Windows session is ${session.locked ? "LOCKED" : "unlocked"}`);
  writeState({ sessionLocked: session.locked === true });
  if (session.locked) notVerifiable("the Windows session is locked: UI Automation, the real keyboard and the OS notifications cannot be relied on");

  await startOsWatcher();
  const server = await fetch(`${SERVER_URL}/v2/server`).then((r) => r.json());
  check(server.protocolVersion === 2, `the server answers on protocol v2 (${server.protocolVersion}) at ${SERVER_URL}`);
  check(typeof server.serverEpoch === "string" && server.serverEpoch.length > 0, "the server has an epoch (it started fresh)");

  for (const name of ["a", "b", "c"]) await launchInstance(name, { fresh: true });
  await Promise.all(["a", "b", "c"].map((name) => onboard(name)));
  for (const name of ["a", "b", "c"]) {
    const i = await inst(name);
    await i.waitFor(Q(".tab-strip"), "the tab strip", 60000);
    await markDocument(i);
    check((await heading(i)) === WORKSPACES[name], `[${i.tag}] the main area shows the workspace's Home (${await heading(i)})`);
    check((await syncStatus(i)).configured === false, `[${i.tag}] sync is not configured yet`);
    await i.capture(`launched-${name}`);
  }
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");

  // A connects, makes the space with two pages and shares it.
  await connectSync(A, SERVER_URL, SERVER_TOKEN);
  check((await syncStatus(A)).configured === true, "[A] sync is connected to the server");
  await createSpaceUi(A, SPACE);
  const space = (await spaceList(A)).find((s) => s.name === SPACE);
  must(space !== undefined, "[A] the space exists");
  const spaceId = space.id;
  writeState({ spaceId });
  await newPage(A, PAGES.plan);
  await writeLine(A, TEXT.p1);
  await writeLine(A, TEXT.p2);
  await newPage(A, PAGES.harvest);
  await writeLine(A, TEXT.harvest);
  const planId = await pageIdByTitle(A, spaceId, PAGES.plan);
  const harvestId = await pageIdByTitle(A, spaceId, PAGES.harvest);
  must(planId !== null && harvestId !== null, "[A] the core has both pages");
  writeState({ pages: { plan: planId, harvest: harvestId } });
  const planText = await pageText(A, planId);
  check(planText.includes(TEXT.p1) && planText.includes(TEXT.p2), "[A] the plan page holds its two paragraphs");

  await spaceMenu(A, "Share this space");
  await A.waitFor(T("h2", `Share “${SPACE}”`), "the Share dialog");
  await A.fill(L("Your name for other members", true), "the profile name field", PEOPLE.a);
  await A.clickEl(TD("button", "Share"), "the Share button");
  await A.waitUntil(`${DIALOG_TEXT}.includes("is shared")`, "the dialog says the space is shared", 120000);
  await closeDialog(A, "Close");
  const shared = (await spaceList(A)).find((s) => s.id === spaceId);
  check(shared.shareState === "Shared" && shared.role === "owner" && shared.memberCount === 1, `[A] the space is shared, A is its Owner (${shared.shareState}, ${shared.role}, ${shared.memberCount})`);
  await eventually(async () => {
    const s = await syncStatus(A);
    const feed = (s.feeds ?? []).find((f) => f.spaceId === spaceId);
    return feed !== undefined && feed.pendingEntries === 0 && feed.forked === false;
  }, "[A] the shared feed is uploaded", { ms: 120000, kick: [A] });
  await A.capture("shared");

  // B joins as an Editor and C as a Commenter, each with a code.
  await inviteByCode(A, B, spaceId, { role: "Editor", name: PEOPLE.b, codeKey: "codeB" });
  await B.capture("joined-b");
  await inviteByCode(A, C, spaceId, { role: "Commenter", name: PEOPLE.c, codeKey: "codeC" });
  await C.capture("joined-c");

  // Both read the pages; the roles are what the invitations said.
  const treeA = await treeOf(A, spaceId);
  for (const J of [B, C]) {
    await eventually(async () => sameShape(shapeOf(await treeOf(J, spaceId)), shapeOf(treeA)), `[${J.tag}] ${J.tag}'s tree equals A's`, { ms: 180000, kick: [A, J] });
    const planJ = await pageIdByTitle(J, spaceId, PAGES.plan);
    const text = await pageText(J, planJ);
    check(text.includes(TEXT.p1) && text.includes(TEXT.p2), `[${J.tag}] reads the plan page's text`);
  }
  await openShared(B, spaceId, PAGES.plan);
  const factsB = await readOnlyFacts(B);
  check(factsB.editable === "true" && factsB.pill === "", `[B] as Editor the page is editable (${factsB.editable}, "${factsB.pill}")`);
  await openShared(C, spaceId, PAGES.plan);
  const factsC = await readOnlyFacts(C);
  check(factsC.editable === "false" && factsC.pill !== "", `[C] as Commenter the page is not editable and says so ("${factsC.pill}", contenteditable ${factsC.editable})`);
  check(factsC.titleButton === false && factsC.newPage === false && factsC.propertiesAdd === false, "[C] as Commenter the title is plain text and the structure controls and properties are read-only");
  await C.capture("commenter-page");
  // Everybody lists the three members, with their roles.
  await openMembers(A);
  const rows = await waitMembers(A, "A's list shows the three people with their roles", (r) => r.length === 3 && r.some((m) => m.name === PEOPLE.b && m.role === "Editor") && r.some((m) => m.name === PEOPLE.c && m.role === "Commenter"), 90000, [A, B, C]);
  check(rows.find((m) => m.you)?.role === "Owner", "[A] the list shows A as Owner");
  await closeDialog(A, "Close");
  // The space switcher's badge counts the three members on A's own screen (it follows the members, not only the space list's next read).
  await A.waitUntil(`(document.querySelector(".space-switcher .space-badge")?.textContent ?? "").includes("Shared space, 3 members")`, "A's space badge counts three members", 60000);
  check(true, "[A] the badge of the space switcher says \"Shared space, 3 members\" on A's screen");
  result.details = { members: 3, roles: { a: "Owner", b: "Editor", c: "Commenter" }, server: SERVER_URL };
}


// ---------------------------------------------------------------------------------------------------------------
// phase: comments
// ---------------------------------------------------------------------------------------------------------------

/** What a thread card of the open panel shows: its comments with what can be done to each, resolve or reopen, reply, and whether it is an orphan. */
const cardFacts = (i, quote) =>
  i.evaluate(`(() => {
    const card = ${CARD(quote)};
    if (!card) return null;
    const has = (scope, label) => !![...scope.querySelectorAll("button")].find((b) => norm(b.textContent) === label);
    const actions = card.querySelector(".comment-thread-actions");
    return {
      threadId: card.getAttribute("data-thread-id"),
      resolved: card.classList.contains("is-resolved"),
      orphan: card.classList.contains("is-orphan"),
      resolvedText: norm(card.querySelector(".comment-resolved")?.textContent ?? ""),
      orphanText: norm(card.querySelector(".comment-orphan")?.textContent ?? ""),
      reply: actions ? has(actions, "Reply") : false,
      resolve: actions ? has(actions, "Resolve") : false,
      reopen: actions ? has(actions, "Reopen") : false,
      rows: [...card.querySelectorAll(".comment")].map((li) => ({
        author: norm(li.querySelector(".comment-author")?.textContent ?? ""),
        body: norm(li.querySelector(".comment-body")?.textContent ?? ""),
        deleted: li.classList.contains("is-deleted"),
        edited: !!li.querySelector(".comment-edited"),
        edit: has(li, "Edit"),
        del: has(li, "Delete"),
      })),
    };
  })()`);

/** The highlights of the open page's editor: the text of each, whether it is resolved. */
const highlightsOf = (i) => i.evaluate(`[...document.querySelectorAll(".ProseMirror .comment-highlight")].map((e) => ({ text: e.textContent, resolved: e.classList.contains("is-resolved"), thread: e.getAttribute("data-comment-thread") }))`);

/** A forced request answers Forbidden: the check, with the reason the core gives. */
async function forbidden(i, request, what, reason, required) {
  const answer = await i.core(request);
  const error = answer.error ?? {};
  const got = `${error.code} ${JSON.stringify(error.details ?? {})}`;
  const okReason = error.details?.reason === reason && (required === undefined || error.details?.required === required);
  check(answer.ok === false && error.code === "Forbidden" && okReason, `[${i.tag}] ${what} is refused with Forbidden (${answer.ok ? "ACCEPTED" : got})`);
}

/** Types text into the open page at a place a trusted click puts the caret (a word of the page), with a key before it. */
async function typeAtWord(i, word, key, text) {
  const box = await phraseBox(i, word);
  must(box !== null, `[${i.tag}] the word "${word}" is in the open page`);
  await i.mouse(box.cx, box.cy);
  if (key !== null) await i.press(key);
  await i.type(text);
}

async function phaseComments() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId, pages } = readState();
  const planId = pages.plan;
  const mark = {};

  // ---- 1. A selects text and comments on it: the highlight shows --------------------------------------------------
  const osBefore = osMark();
  await openShared(A, spaceId, PAGES.plan);
  check((await unreadCount(A)) === 0 && (await unreadCount(B)) === 0 && (await unreadCount(C)) === 0, "nobody has a notification yet");
  await selectPhrase(A, ANCHOR1);
  const toolbar = await A.evaluate(`({ comment: !!document.querySelector(".format-comment"), bold: !!document.querySelector(".format-bold") })`);
  check(toolbar.comment === true && toolbar.bold === true, "[A] the toolbar of the selection offers Comment next to the formatting");
  await A.clickEl(Q(".format-comment"), "the Comment button of the toolbar");
  await A.waitFor(Q(".comments-panel .comment-draft"), "the comment draft");
  const quoteShown = await A.evaluate(`norm(document.querySelector(".comments-panel .comment-draft .comment-quote")?.textContent ?? "")`);
  check(quoteShown === ANCHOR1, `[A] the draft shows the words it is about (${quoteShown})`);
  await writeComment(A, ".comments-panel .comment-draft", "Write a comment", COMMENT.a1, "button");
  await A.waitFor(`(${CARD(ANCHOR1)})`, "the thread card");
  const hl1 = await highlightsOf(A);
  check(hl1.length === 1 && hl1[0].text === ANCHOR1 && hl1[0].resolved === false, `[A] the page highlights the words the thread is about (${JSON.stringify(hl1.map((h) => h.text))})`);
  const countBadge = await A.evaluate(`document.querySelector(".page-comments-count")?.textContent.trim() ?? ""`);
  check(countBadge === "1", `[A] the page header counts one open thread (${countBadge})`);
  let threads = await threadsOf(A, planId);
  check(threads.length === 1 && threads[0].anchor?.quote === ANCHOR1 && threads[0].comments.length === 1 && threads[0].comments[0].isMine === true, "[A] the core holds one anchored thread with A's comment");
  check(typeof threads[0].anchor?.start === "string" && typeof threads[0].anchor?.end === "string" && typeof threads[0].anchor?.blockId === "string", "[A] the anchor holds a block and two cursors");
  const thread1 = threads[0].threadId;
  const rootA = threads[0].comments[0].commentId;
  await A.capture("comment-a");
  check((await unreadCount(A)) === 0, "[A] A's own comment raised no notification on A");

  // ---- 2. B sees the thread and replies; A is told ----------------------------------------------------------------
  await eventually(async () => (await threadsOf(B, planId)).length === 1, "[B] B's core holds the thread", { ms: 120000, kick: [A, B] });
  await openShared(B, spaceId, PAGES.plan);
  const bLabel = await B.evaluate(`document.querySelector(".page-comments-button")?.getAttribute("aria-label") ?? ""`);
  check(bLabel === "Comments (1 open thread)", `[B] the page header says there is one open thread (${bLabel})`);
  await openCommentsPanel(B);
  await B.waitFor(`(${CARD(ANCHOR1)})`, "the thread card on B");
  let facts = await cardFacts(B, ANCHOR1);
  check(facts.rows.length === 1 && facts.rows[0].author === PEOPLE.a && facts.rows[0].body === COMMENT.a1, `[B] B reads A's comment under A's name`);
  const hlB = await highlightsOf(B);
  check(hlB.length === 1 && hlB[0].text === ANCHOR1, `[B] B's page highlights the same words (${JSON.stringify(hlB.map((h) => h.text))})`);
  check(facts.rows[0].edit === false && facts.rows[0].del === false, "[B] B, an Editor, is offered neither Edit nor Delete on A's comment");
  check(facts.reply === true && facts.resolve === true, "[B] B is offered Reply and Resolve on A's thread (an Editor resolves)");
  await B.capture("comment-b");
  await A.capture("comment-a-before-reply");
  await B.clickEl(`byText("button", "Reply", true, ${CARD(ANCHOR1)})`, "the Reply link");
  await writeComment(B, ".comments-panel", "Reply to the thread", COMMENT.b1, "enter");
  await B.waitUntil(`${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1)} !== undefined`, "B's reply is in the thread on B");
  facts = await cardFacts(B, ANCHOR1);
  check(facts.rows.length === 2 && facts.rows[1].author === PEOPLE.b && facts.rows[1].edit === true && facts.rows[1].del === true, "[B] B's own reply has Edit and Delete");
  check((await unreadCount(B)) === 0, "[B] B's own reply raised no notification on B");

  // A: the toast, the count, the inbox, the OS notification.
  const toastReply = await waitToast(A, /Boris Thistledown replied to your thread on Orchard plan/, "a toast says Boris replied to A's thread", { kick: [A, B] });
  check(toastReply === `${PEOPLE.b} replied to your thread on ${PAGES.plan}`, `[A] the toast says who replied and on which page (${toastReply})`);
  await waitUnread(A, 1, "A has one unread notification", { kick: [A, B] });
  const badge = await badgeText(A);
  const bell = await bellLabel(A);
  check(badge === "1" && bell === "Notifications (1 unread)", `[A] the bell's badge counts one unread (${badge}; ${bell})`);
  let rows = await openInbox(A);
  check(rows.length === 1 && rows[0].unread && rows[0].text === `${PEOPLE.b} replied to your thread on ${PAGES.plan}`, `[A] the inbox lists the reply, unread (${JSON.stringify(rows)})`);
  const inbox = (await inboxRowsCore(A))[0];
  check(inbox.kind === "ThreadReply" && inbox.pageId === planId && typeof inbox.commentId === "string" && inbox.read === false, "[A] the row is a ThreadReply of this page");
  check(!JSON.stringify(inbox).includes(COMMENT.b1), "[A] the row holds no comment text");
  await A.capture("inbox-a");
  await closeInbox(A);
  const osReply = await waitOsNotification(/Boris Thistledown replied to your thread/, { since: osBefore, ms: 20000 });
  checkOsNotification(osReply, { what: "[A] the reply", title: PAGES.plan, bodyPattern: /^Boris Thistledown replied to your thread$/, forbidden: [COMMENT.b1, COMMENT.a1] });
  // A's open panel shows B's reply without a reload.
  await openCommentsPanel(A);
  const liveA = await A.waitUntil(`${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1)} !== undefined`, "B's reply shows in A's open panel", 60000).catch(() => false);
  check(liveA !== false, "[A] B's reply shows in A's panel");
  check((await unreadCount(C)) === 0, "[C] C wrote nothing in the thread and has no notification");

  // ---- 2b. B edits his own reply: the field starts with the text, "(edited)" shows on both sides, nobody is told ------------------
  await B.clickEl(`byText("button", "Edit", true, ${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1)})`, "Edit on B's reply");
  await B.waitFor(Q('.comments-panel textarea[aria-label="Edit the comment"]'), "the edit field");
  check((await B.run(`return document.querySelector('.comments-panel textarea[aria-label="Edit the comment"]').value;`)) === COMMENT.b1, "[B] the edit field starts with the comment's text");
  await B.fill(Q('.comments-panel textarea[aria-label="Edit the comment"]'), "the edit field", COMMENT.b1e);
  await B.clickEl(`byText("button", "Save", true, ${CARD(ANCHOR1)})`, "Save");
  await B.waitUntil(`${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)} !== undefined`, "the edited reply shows on B");
  check((await B.evaluate(`!!(${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)})?.querySelector(".comment-edited")`)) === true, "[B] the reply is marked as edited");
  await A.waitUntil(`${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)} !== undefined`, "the edited reply reaches A's open panel", 90000);
  check((await A.evaluate(`!!(${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)})?.querySelector(".comment-edited")`)) === true, "[A] the reply shows as edited on A too");
  threads = await threadsOf(A, planId);
  check(threads[0].comments[1].editedAt !== undefined && JSON.stringify(threads[0].comments[1].body) === JSON.stringify([{ type: "text", text: COMMENT.b1e }]), "[A] the core holds the new text and an edit time");
  check((await unreadCount(A)) === 1 && (await unreadCount(B)) === 0 && (await unreadCount(C)) === 0, "[A] [B] [C] an edit that adds no mention notifies nobody");

  // ---- 3. C, a Commenter, comments but cannot edit the page ---------------------------------------------------------
  await openShared(C, spaceId, PAGES.plan);
  await eventually(async () => (await threadsOf(C, planId))[0]?.comments.length === 2, "[C] C's core holds both comments", { ms: 120000, kick: [A, B, C] });
  await openCommentsPanel(C);
  await C.waitFor(`(${CARD(ANCHOR1)})`, "the thread card on C");
  facts = await cardFacts(C, ANCHOR1);
  check(facts.rows.length === 2 && facts.rows.every((r) => !r.edit && !r.del), "[C] C is offered neither Edit nor Delete on the comments of others");
  check(facts.reply === true && facts.resolve === false, `[C] C may reply to A's thread but is not offered Resolve on it (reply ${facts.reply}, resolve ${facts.resolve})`);
  const roFacts = await readOnlyFacts(C);
  check(roFacts.editable === "false", "[C] the page is not editable for a Commenter");
  const textBefore = await pageText(C, planId);
  await C.clickEl(Q(".ProseMirror"), "the read-only editor");
  await C.insertText("A Commenter typed this");
  await C.type("zz");
  await wait(2000);
  check((await pageText(C, planId)) === textBefore, "[C] typing into the page changes nothing");
  await forbidden(C, { type: "PageRename", data: { pageId: planId, title: "Forced by a Commenter" } }, "a forced PageRename", "role");
  await forbidden(C, { type: "PageCreate", data: { spaceId, title: "Forced page" } }, "a forced PageCreate", "role");
  const raw = await C.evaluate(`(async () => {
    try {
      const snapshot = await window.__TAURI_INTERNALS__.invoke("doc_export", new Uint8Array(), { headers: { "x-kb-page": ${JSON.stringify(planId)} } });
      try {
        await window.__TAURI_INTERNALS__.invoke("doc_import", new Uint8Array(snapshot), { headers: { "x-kb-page": ${JSON.stringify(planId)} } });
        return { imported: true };
      } catch (e) {
        return { imported: false, error: e instanceof Error ? e.message : e };
      }
    } catch (e) {
      return { exported: false, error: e instanceof Error ? e.message : e };
    }
  })()`);
  check(raw.imported === false && raw.error?.code === "Forbidden", `[C] a Commenter's raw document import is refused with Forbidden (${raw.imported ? "ACCEPTED" : JSON.stringify(raw.error).slice(0, 100)})`);
  // C comments on a selection of the read-only page.
  await selectPhrase(C, ANCHOR2);
  const toolbarC = await C.evaluate(`({ comment: !!document.querySelector(".format-comment"), bold: !!document.querySelector(".format-bold"), link: !!document.querySelector(".format-link") })`);
  check(toolbarC.comment === true && toolbarC.bold === false && toolbarC.link === false, `[C] the toolbar of a Commenter's selection holds Comment alone (${JSON.stringify(toolbarC)})`);
  await C.clickEl(Q(".format-comment"), "the Comment button of the toolbar");
  await C.waitFor(Q(".comments-panel .comment-draft"), "the comment draft on C");
  await writeComment(C, ".comments-panel .comment-draft", "Write a comment", COMMENT.c1, "button");
  await C.waitFor(`(${CARD(ANCHOR2)})`, "C's thread card");
  const hlC = await highlightsOf(C);
  check(hlC.some((h) => h.text === ANCHOR2) && hlC.some((h) => h.text === ANCHOR1), `[C] C's page highlights both threads (${JSON.stringify(hlC.map((h) => h.text))})`);
  const factsC = await cardFacts(C, ANCHOR2);
  check(factsC.rows.length === 1 && factsC.rows[0].edit && factsC.rows[0].del && factsC.resolve, "[C] on C's own thread C may edit, delete and resolve");
  await C.capture("comment-c");
  threads = await threadsOf(C, planId);
  const thread2 = threads.find((t) => t.anchor?.quote === ANCHOR2)?.threadId;
  must(typeof thread2 === "string", "[C] the core holds C's thread");
  check(threads.length === 2, "[C] the page has two threads now");
  // A and B read C's thread (and the highlight).
  for (const J of [A, B]) {
    await eventually(async () => (await threadsOf(J, planId)).length === 2, `[${J.tag}] sees C's thread`, { ms: 120000, kick: [A, B, C] });
  }
  await A.waitUntil(`${ROW(CARD(ANCHOR2), PEOPLE.c, COMMENT.c1)} !== undefined`, "C's comment shows in A's panel", 60000);
  const hlA = await highlightsOf(A);
  check(hlA.some((h) => h.text === ANCHOR2), `[A] A's page highlights C's words (${JSON.stringify(hlA.map((h) => h.text))})`);
  check((await unreadCount(A)) === 1 && (await unreadCount(B)) === 0, "[A] [B] a new thread of another person is not a notification (A still has one unread, B none)");
  check((await pageText(A, planId)) === textBefore, "the page text is what it was: the Commenter changed nothing");

  // ---- 4. B cannot edit or delete A's comment ----------------------------------------------------------------------
  await forbidden(B, { type: "CommentEdit", data: { pageId: planId, commentId: rootA, body: [{ type: "text", text: "Edited by someone else" }] } }, "B's forced CommentEdit of A's comment", "author");
  await forbidden(B, { type: "CommentDelete", data: { pageId: planId, commentId: rootA } }, "B's forced CommentDelete of A's comment", "role", "owner");
  await forbidden(C, { type: "CommentResolve", data: { pageId: planId, threadId: thread1, resolved: true } }, "C's forced CommentResolve of A's thread", "role", "editor");
  await forbidden(C, { type: "CommentEdit", data: { pageId: planId, commentId: rootA, body: [{ type: "text", text: "Edited by a Commenter" }] } }, "C's forced CommentEdit of A's comment", "author");
  await forbidden(C, { type: "CommentDelete", data: { pageId: planId, commentId: rootA } }, "C's forced CommentDelete of A's comment", "role", "owner");
  await wait(1500);
  threads = await threadsOf(A, planId);
  const t1 = threads.find((t) => t.threadId === thread1);
  check(t1.comments[0].commentId === rootA && JSON.stringify(t1.comments[0].body) === JSON.stringify([{ type: "text", text: COMMENT.a1 }]) && t1.comments[0].deleted === false && t1.resolved === false, "[A] A's comment is as it was after the forced requests");
  // A forced raw edit that the UI hides is not on B's screen either.
  facts = await cardFacts(B, ANCHOR1);
  check(facts.rows[0].edit === false && facts.rows[0].del === false, "[B] the UI still hides Edit and Delete on A's comment");

  // ---- 5. A replies after B (B is told), then, an Owner, deletes B's reply; B resolves the thread ---------------------
  const os5 = osMark();
  await A.clickEl(`byText("button", "Reply", true, ${CARD(ANCHOR1)})`, "the Reply link on A");
  await writeComment(A, ".comments-panel", "Reply to the thread", COMMENT.a3, "button");
  await A.waitUntil(`${ROW(CARD(ANCHOR1), PEOPLE.a, COMMENT.a3)} !== undefined`, "A's second comment is in the thread on A");
  const toastB = await waitToast(B, /Alma Quillfeather replied to your thread on Orchard plan/, "a toast says Alma replied to B's thread", { kick: [A, B] });
  check(toastB === `${PEOPLE.a} replied to your thread on ${PAGES.plan}`, `[B] B, who wrote in the thread, is told of A's reply (${toastB})`);
  await waitUnread(B, 1, "B has one unread notification", { kick: [A, B] });
  const osB5 = await waitOsNotification(/Alma Quillfeather replied to your thread/, { since: os5, ms: 20000 });
  checkOsNotification(osB5, { what: "[B] the reply", title: PAGES.plan, bodyPattern: /^Alma Quillfeather replied to your thread$/, forbidden: [COMMENT.a3, COMMENT.b1] });
  check((await unreadCount(A)) === 1, "[A] A's own reply raised nothing on A");
  facts = await cardFacts(A, ANCHOR1);
  check(facts.rows.length === 3 && facts.rows[1].author === PEOPLE.b && facts.rows[1].del === true && facts.rows[1].edit === false, "[A] A, an Owner, is offered Delete (not Edit) on B's reply");
  await A.clickEl(`byText("button", "Delete", true, ${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)})`, "Delete on B's reply");
  await A.waitFor(`(${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)})?.querySelector(".comment-confirm")`, "the delete confirmation");
  const confirmText = await A.evaluate(`norm((${ROW(CARD(ANCHOR1), PEOPLE.b, COMMENT.b1e)})?.querySelector(".comment-confirm")?.textContent ?? "")`);
  check(/This cannot be undone/.test(confirmText), `[A] the confirmation says it cannot be undone (${confirmText})`);
  await A.clickEl(`byText("button", "Delete comment", true, ${CARD(ANCHOR1)})`, "the confirming button");
  await A.waitUntil(`(${CARD(ANCHOR1)})?.querySelectorAll(".comment.is-deleted").length === 1`, "B's reply became a placeholder on A");
  facts = await cardFacts(A, ANCHOR1);
  check(facts.rows.length === 3 && facts.rows[1].deleted && /This comment was deleted/.test(facts.rows[1].body) && !facts.rows[1].body.includes(COMMENT.b1e), `[A] B's reply is a placeholder between the two comments that remain (${facts.rows[1].body})`);
  check(facts.rows[0].body === COMMENT.a1 && facts.rows[2].body === COMMENT.a3, "[A] A's two comments are untouched");
  threads = await threadsOf(A, planId);
  const afterDelete = threads.find((t) => t.threadId === thread1);
  check(afterDelete.comments.length === 3 && afterDelete.comments[1].deleted === true && afterDelete.comments[1].body.length === 0, "[A] the core keeps B's reply as a placeholder with no body");
  await A.capture("comment-deleted-a");
  await eventually(async () => (await threadsOf(B, planId)).find((t) => t.threadId === thread1)?.comments[1]?.deleted === true, "[B] B's core holds the deletion", { ms: 120000, kick: [A, B] });
  await B.waitUntil(`(${CARD(ANCHOR1)})?.querySelectorAll(".comment.is-deleted").length === 1`, "B's reply is a placeholder on B", 30000);
  check((await unreadCount(B)) === 1, "[B] the deletion of B's own reply is no new notification");
  // B reads the inbox and marks everything read.
  rows = await openInbox(B);
  check(rows.length === 1 && rows[0].unread, "[B] B's inbox holds the reply, unread");
  await B.clickEl(T(".inbox header button", "Mark all as read"), "Mark all as read");
  await waitUnread(B, 0, "Mark all as read cleared B's count", { kick: [B] });
  rows = await inboxRows(B);
  check(rows.length === 1 && !rows[0].unread && (await badgeText(B)) === "", "[B] the row is still listed, read, and the badge is gone");
  await closeInbox(B);
  // B resolves.
  await B.clickEl(`byText("button", "Resolve", true, ${CARD(ANCHOR1)})`, "Resolve on A's thread");
  await B.waitUntil(`!(${CARD(ANCHOR1)})`, "the resolved thread leaves B's panel");
  const hlAfterResolve = await highlightsOf(B);
  check(!hlAfterResolve.some((h) => h.text === ANCHOR1) && hlAfterResolve.some((h) => h.text === ANCHOR2), `[B] the resolved thread's highlight is gone, the open one stays (${JSON.stringify(hlAfterResolve.map((h) => h.text))})`);
  const bLabel2 = await B.evaluate(`document.querySelector(".page-comments-button")?.getAttribute("aria-label") ?? ""`);
  check(bLabel2 === "Comments (1 open thread)", `[B] the header counts one open thread (${bLabel2})`);
  threads = await threadsOf(B, planId);
  const resolved = threads.find((t) => t.threadId === thread1);
  check(resolved.resolved === true && resolved.resolvedBy !== undefined, "[B] the core marks the thread resolved, by B");
  await eventually(async () => (await threadsOf(A, planId)).find((t) => t.threadId === thread1)?.resolved === true, "[A] A's core holds the resolution", { ms: 120000, kick: [A, B] });
  await A.waitUntil(`!(${CARD(ANCHOR1)})`, "the resolved thread leaves A's panel", 30000);
  check((await unreadCount(A)) === 1, "[A] resolving raised nothing on A (still the one reply)");
  await B.capture("comment-resolved-b");

  // ---- 6. Show resolved ---------------------------------------------------------------------------------------------
  for (const J of [B, A]) {
    await J.clickEl(`document.querySelector('.comments-panel .comments-show-resolved input')`, "the Show resolved checkbox");
    await J.waitFor(`(${CARD(ANCHOR1)})`, "the resolved thread listed");
    facts = await cardFacts(J, ANCHOR1);
    check(facts.resolved && facts.resolvedText === `Resolved by ${PEOPLE.b}` && facts.reopen === true, `[${J.tag}] "Show resolved" lists the thread: "${facts.resolvedText}", with Reopen`);
    check(facts.rows.length === 3 && facts.rows[1].deleted, `[${J.tag}] the deleted reply is still a placeholder in the resolved thread`);
    const hl = await highlightsOf(J);
    check(hl.some((h) => h.text === ANCHOR1 && h.resolved === true), `[${J.tag}] the resolved thread's words are highlighted while resolved threads show (${JSON.stringify(hl.map((h) => [h.text, h.resolved]))})`);
  }
  await B.capture("comment-show-resolved-b");
  // B reopens the thread (the card, the highlight and the header follow, A reads it) and resolves it again.
  await B.clickEl(`byText("button", "Reopen", true, ${CARD(ANCHOR1)})`, "Reopen");
  await B.waitUntil(`(${CARD(ANCHOR1)}) && !(${CARD(ANCHOR1)}).classList.contains("is-resolved")`, "the thread is open again on B");
  check((await highlightsOf(B)).some((h) => h.text === ANCHOR1 && h.resolved === false), "[B] the thread's words are highlighted as open again");
  check((await B.evaluate(`document.querySelector(".page-comments-button")?.getAttribute("aria-label") ?? ""`)) === "Comments (2 open threads)", "[B] the header counts two open threads");
  await eventually(async () => (await threadsOf(A, planId)).find((t) => t.threadId === thread1)?.resolved === false, "[A] A's core holds the reopening", { ms: 120000, kick: [A, B] });
  await B.clickEl(`byText("button", "Resolve", true, ${CARD(ANCHOR1)})`, "Resolve again");
  await B.waitUntil(`(${CARD(ANCHOR1)})?.classList.contains("is-resolved")`, "the thread is resolved again on B");
  await eventually(async () => (await threadsOf(A, planId)).find((t) => t.threadId === thread1)?.resolved === true, "[A] A's core holds the second resolution", { ms: 120000, kick: [A, B] });
  check((await unreadCount(A)) === 1 && (await unreadCount(B)) === 0, "[A] [B] reopening and resolving notified nobody (A still has the one reply, B has read everything)");

  // ---- 7. The anchors survive an edit of the text around them -----------------------------------------------------
  // B (an Editor) edits the words before, inside the paragraph and after the two anchors, with real keys.
  await B.evaluate(`document.activeElement?.blur?.()`);
  await typeAtWord(B, "Prune", "Home", "Early: ");
  await B.press("End");
  await B.type(" Then mulch the beds.");
  await replaceWord(B, "frost", "freeze");
  await replaceWord(B, "Conference", "Williams");
  await B.press("End");
  await B.type(" Check it weekly.");
  await B.evaluate(`document.activeElement?.blur?.()`);
  await wait(2000);
  const editedText = await pageText(B, planId);
  check(editedText.includes("Early: Prune the pear trees before the first freeze. Then mulch the beds.") && editedText.includes("Williams pears keep best in a cool cellar. Check it weekly."), `[B] B's edits are in the page (${editedText})`);
  for (const J of [B]) {
    const hl = await highlightsOf(J);
    check(hl.some((h) => h.text === ANCHOR1) && hl.some((h) => h.text === ANCHOR2), `[${J.tag}] after B's edits the highlights are on the same words (${JSON.stringify(hl.map((h) => h.text))})`);
  }
  await eventually(async () => (await pageText(A, planId)).includes("Early: Prune") && (await pageText(C, planId)).includes("Early: Prune"), "[A] [C] A and C get B's edits", { ms: 120000, kick: [A, B, C] });
  for (const J of [A, C]) {
    await openShared(J, spaceId, PAGES.plan);
    await openCommentsPanel(J);
    if (J === C) await J.clickEl(`document.querySelector('.comments-panel .comments-show-resolved input')`, "the Show resolved checkbox on C");
    await J.waitUntil(`(document.querySelector(".ProseMirror")?.innerText ?? "").includes("Early: Prune")`, `the editor of ${J.tag} shows B's edit`, 30000);
    await wait(800);
    const hl = await highlightsOf(J);
    check(hl.some((h) => h.text === ANCHOR1) && hl.some((h) => h.text === ANCHOR2), `[${J.tag}] after B's edits ${J.tag}'s highlights are on the same words (${JSON.stringify(hl.map((h) => h.text))})`);
    const f1 = await cardFacts(J, ANCHOR1);
    const f2 = await cardFacts(J, ANCHOR2);
    check(f1 !== null && f2 !== null && !f1.orphan && !f2.orphan && f1.orphanText === "" && f2.orphanText === "", `[${J.tag}] neither thread became an orphan`);
  }
  await A.capture("anchors-after-edit-a");
  await C.capture("anchors-after-edit-c");
  // The anchor is stored as cursors: the core's thread still names the same quote.
  threads = await threadsOf(A, planId);
  check(threads.length === 2 && threads.every((t) => t.anchor !== undefined), "[A] the core still holds both anchors");

  // ---- 8. A replies in C's thread: C is told, and marks that row read with its own button -------------------------------
  const os8 = osMark();
  await A.clickEl(`byText("button", "Reply", true, ${CARD(ANCHOR2)})`, "the Reply link on C's thread");
  await writeComment(A, ".comments-panel", "Reply to the thread", COMMENT.a2, "enter");
  await A.waitUntil(`${ROW(CARD(ANCHOR2), PEOPLE.a, COMMENT.a2)} !== undefined`, "A's reply is in C's thread");
  const toastC = await waitToast(C, /Alma Quillfeather replied to your thread on Orchard plan/, "a toast says Alma replied to C's thread", { kick: [A, C] });
  check(toastC === `${PEOPLE.a} replied to your thread on ${PAGES.plan}`, `[C] C is told of the reply to its thread (${toastC})`);
  await waitUnread(C, 1, "C has one unread notification", { kick: [A, C] });
  const osC8 = await waitOsNotification(/Alma Quillfeather replied to your thread/, { since: os8, ms: 20000 });
  checkOsNotification(osC8, { what: "[C] the reply", title: PAGES.plan, bodyPattern: /^Alma Quillfeather replied to your thread$/, forbidden: [COMMENT.a2, "dehumidifier"] });
  check((await unreadCount(B)) === 0, "[B] B never wrote in that thread and is not told");
  rows = await openInbox(C);
  check(rows.length === 1 && rows[0].unread, "[C] C's inbox holds the reply, unread");
  await C.clickEl(T(".inbox .inbox-row button", "Mark as read"), "the row's Mark as read button");
  await waitUnread(C, 0, "the row's own button marked it read", { kick: [C] });
  rows = await inboxRows(C);
  check(rows.length === 1 && !rows[0].unread, "[C] the row is read");
  await closeInbox(C);
  check((await unreadCount(A)) === 1, "[A] A is left with the one reply of B that it did not read");
  // ---- 9. Text that goes: the thread of deleted words reads as a page-level thread with its quote --------------------------
  await openShared(A, spaceId, PAGES.plan);
  await selectPhrase(A, ANCHOR3);
  await A.clickEl(Q(".format-comment"), "the Comment button of the toolbar");
  await A.waitFor(Q(".comments-panel .comment-draft"), "the comment draft");
  await writeComment(A, ".comments-panel .comment-draft", "Write a comment", COMMENT.a5, "button");
  await A.waitFor(`(${CARD(ANCHOR3)})`, "the third thread's card");
  check((await highlightsOf(A)).some((h) => h.text === ANCHOR3), "[A] the third thread's words are highlighted");
  await eventually(async () => (await threadsOf(B, planId)).length === 3, "[B] B's core holds the third thread", { ms: 120000, kick: [A, B] });
  await openShared(B, spaceId, PAGES.plan);
  await selectPhrase(B, ANCHOR3);
  await B.press("Backspace");
  await B.evaluate(`document.activeElement?.blur?.()`);
  await wait(2000);
  const afterDelete3 = await pageText(B, planId);
  check(!afterDelete3.includes(ANCHOR3) && afterDelete3.includes("Williams pears") && afterDelete3.includes("cool cellar"), `[B] B deleted the words the thread was about (${afterDelete3.slice(afterDelete3.indexOf("Williams"), afterDelete3.indexOf("Williams") + 50)})`);
  await eventually(async () => !(await pageText(A, planId)).includes(ANCHOR3) && !(await pageText(C, planId)).includes(ANCHOR3), "[A] [C] A and C get B's deletion", { ms: 120000, kick: [A, B, C] });
  for (const J of [B, A, C]) {
    if (J !== B) await openShared(J, spaceId, PAGES.plan);
    await openCommentsPanel(J);
    await J.waitUntil(`(${CARD(ANCHOR3)})?.classList.contains("is-orphan")`, `the thread of the deleted words is an orphan on ${J.tag}`, 30000);
    const f3 = await cardFacts(J, ANCHOR3);
    check(f3.orphan && f3.orphanText === "The text this comment was about is gone." && f3.rows.length === 1 && f3.rows[0].body === COMMENT.a5, `[${J.tag}] it reads as a page-level thread: the quote, "${f3.orphanText}", and the comment`);
    const hl3 = await highlightsOf(J);
    check(!hl3.some((h) => h.text === ANCHOR3) && hl3.some((h) => h.text === ANCHOR2), `[${J.tag}] its highlight is gone and the neighbouring thread's stays (${JSON.stringify(hl3.map((h) => h.text))})`);
    const f2 = await cardFacts(J, ANCHOR2);
    check(f2 !== null && !f2.orphan, `[${J.tag}] the thread next to the deleted words is not an orphan`);
  }
  await A.capture("orphan-a");
  threads = await threadsOf(A, planId);
  check(threads.length === 3 && threads.find((t) => t.anchor?.quote === ANCHOR3)?.comments[0].body[0].text === COMMENT.a5, "[A] the core still holds the thread with its anchor and quote (it knows nothing of the page text)");
  check((await unreadCount(A)) === 1 && (await unreadCount(B)) === 0 && (await unreadCount(C)) === 0, "[A] [B] [C] the third thread and its orphaning notified nobody");
  result.details = { threads: threads.length, notifications: { a: await unreadCount(A), b: await unreadCount(B), c: await unreadCount(C) } };
  mark.done = true;
}


// ---------------------------------------------------------------------------------------------------------------
// phase: mentions
// ---------------------------------------------------------------------------------------------------------------

/** Types `trigger` + `query` (the `@` menu of the editor), moves to the entry titled `label` and presses Enter. */
async function pickFromMenu(i, trigger, query, label) {
  await i.type(`${trigger}${query}`);
  await wait(500);
  await i.waitFor(Q("[role=listbox]"), `the menu for ${trigger}${query}`);
  // The menu follows the typed words a moment later (the people are read by the core): wait until it offers the entry.
  await i.waitUntil(`[...document.querySelectorAll("[role=listbox] [role=option] .suggestion-title")].some((e) => e.textContent.trim() === ${JSON.stringify(label)})`, `the menu for ${trigger}${query} offers "${label}"`, 20000);
  for (let k = 0; k < 25; k++) {
    const seen = await i.evaluate(`(() => {
      const options = [...document.querySelectorAll("[role=listbox] [role=option]")];
      const title = (o) => o.querySelector(".suggestion-title")?.textContent.trim() ?? "";
      return { selected: title(options.find((o) => o.getAttribute("aria-selected") === "true") ?? options[0] ?? document.body), all: options.map(title) };
    })()`);
    if (!seen.all.includes(label)) must(false, `[${i.tag}] the menu for ${trigger}${query} offers "${label}" (${seen.all.join(", ")})`);
    if (seen.selected === label) break;
    await i.press("ArrowDown");
    await wait(80);
  }
  await i.press("Enter");
  await wait(700);
}

/** The card of the thread that holds `text` (a page-level thread has no quote). */
const CARD_WITH = (text) => `[...document.querySelectorAll(".comments-panel .comment-thread")].find((li) => norm(li.innerText).includes(${JSON.stringify(text)}))`;

/** A comment with an `@` mention: the field is filled with the start, the picker of people is used, the rest is typed, then it is sent. */
async function writeCommentWithMention(i, scope, label, { before = "", query, person, after }) {
  const field = `${scope} textarea[aria-label=${JSON.stringify(label)}]`;
  await i.clickEl(`document.querySelector(${JSON.stringify(field)})`, `the "${label}" field`);
  if (before !== "") await i.insertText(before);
  await i.insertText(`@${query}`);
  await i.waitFor(Q('[role="listbox"][aria-label="People to mention"]'), "the picker of people");
  const names = await i.evaluate(`[...document.querySelectorAll('[role="listbox"][aria-label="People to mention"] [role=option]')].map((o) => norm(o.textContent))`);
  must(names.some((n) => n.includes(person)), `[${i.tag}] the picker of people offers ${person} (${names.join(", ")})`);
  for (let k = 0; k < 8; k++) {
    const active = await i.evaluate(`norm(document.querySelector('[role="listbox"][aria-label="People to mention"] [role=option][aria-selected="true"]')?.textContent ?? "")`);
    if (active.includes(person)) break;
    await i.press("ArrowDown");
    await wait(80);
  }
  await i.press("Enter");
  await wait(300);
  await i.insertText(after);
  await wait(200);
  const value = await i.run(`return document.querySelector(${JSON.stringify(field)})?.value ?? null;`);
  must(value === `${before}@${person} ${after}`, `[${i.tag}] the comment field holds the mention and the words after it (${value})`);
  await i.clickEl(`document.querySelector(${JSON.stringify(field)}).closest("form").querySelector('button[type="submit"]')`, `the send button of "${label}"`);
  await wait(500);
}

/** The block of a page that holds a person mention of `accountId`. */
async function mentionBlock(i, pageId, accountId) {
  const blocks = await blocksOf(i, pageId);
  return blocks.find((b) => b.inline.some((n) => n.type === "personMention" && n.attrs.accountId === accountId)) ?? null;
}

/** Waits until a core of `i` lists a page-level thread whose first comment includes `text`. */
const threadWithText = async (i, pageId, text) => (await threadsOf(i, pageId)).find((t) => t.comments.some((c) => c.body.some((b) => (b.text ?? "").includes(text))));

/** The account id of a person in the members of the space, as `i` reads them. */
async function accountOf(i, spaceId, name) {
  const found = (await membersOf(i, spaceId)).members.find((m) => m.name === name);
  must(found !== undefined, `[${i.tag}] the member list names ${name}`);
  return found.accountId;
}

async function phaseMentions() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId, pages } = readState();
  const planId = pages.plan;
  const harvestId = pages.harvest;
  const accA = await accountOf(A, spaceId, PEOPLE.a);
  const accB = await accountOf(A, spaceId, PEOPLE.b);
  const accC = await accountOf(A, spaceId, PEOPLE.c);
  writeState({ accounts: { a: accA, b: accB, c: accC } });
  const baseA = await unreadCount(A);
  check(baseA === 1 && (await unreadCount(B)) === 0 && (await unreadCount(C)) === 0, `[A] [B] [C] the inboxes are as the comments phase left them (A ${baseA}, B ${await unreadCount(B)}, C ${await unreadCount(C)})`);

  // ---- 1. A types @ in a page and picks B: B is told, and the notification opens the page at the block ---------------
  const os1 = osMark();
  await openShared(A, spaceId, PAGES.harvest);
  await newLine(A);
  await A.type("Crates are counted by ");
  await pickFromMenu(A, "@", "Bor", PEOPLE.b);
  await A.type("every evening.");
  await A.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  const chip = await A.evaluate(`[...document.querySelectorAll(".ProseMirror .person-mention")].map((e) => ({ text: norm(e.textContent), account: e.getAttribute("data-account-id"), me: e.classList.contains("is-me") }))`);
  check(chip.length === 1 && chip[0].text === `@${PEOPLE.b}` && chip[0].account === accB && chip[0].me === false, `[A] the page shows a chip for B (${JSON.stringify(chip)})`);
  const blockB = await mentionBlock(A, harvestId, accB);
  check(blockB !== null && blockB.text.includes("Crates are counted by"), "[A] the core holds a personMention of B in the paragraph");
  await A.capture("mention-a");
  const toastB = await waitToast(B, /Alma Quillfeather mentioned you on Harvest log/, "a toast says Alma mentioned B on Harvest log", { kick: [A, B] });
  check(toastB === `${PEOPLE.a} mentioned you on ${PAGES.harvest}`, `[B] the toast says who mentioned B and on which page (${toastB})`);
  await waitUnread(B, 1, "B has one unread notification", { kick: [A, B] });
  check((await badgeText(B)) === "1", "[B] the bell's badge counts one");
  const rowB = (await inboxRowsCore(B))[0];
  check(rowB.kind === "PageMention" && rowB.pageId === harvestId && rowB.blockId === blockB.id && rowB.actor === accA && rowB.commentId === undefined, "[B] the row is a PageMention of the Harvest log page, at the block that holds the mention");
  const osB = await waitOsNotification(/Alma Quillfeather mentioned you/, { since: os1, ms: 20000 });
  checkOsNotification(osB, { what: "[B] the page mention", title: PAGES.harvest, bodyPattern: /^Alma Quillfeather mentioned you$/, forbidden: [TEXT.harvest, "Crates are counted"] });
  check((await unreadCount(A)) === baseA, "[A] A has no new notification for mentioning B");
  // The row opens the page at the block (B is on another page).
  await openShared(B, spaceId, PAGES.plan);
  const rowsInbox = await openInbox(B);
  check(rowsInbox.length === 2 && rowsInbox.filter((r) => r.unread).length === 1 && rowsInbox.some((r) => r.unread && r.text === `${PEOPLE.a} mentioned you on ${PAGES.harvest}`), `[B] the inbox lists it, unread, above the read reply of the comments phase (${JSON.stringify(rowsInbox)})`);
  check(rowsInbox[0].unread, "[B] the newest row is first");
  await B.capture("inbox-b");
  await B.clickEl(Q(".inbox .inbox-open"), "the inbox row");
  await B.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(PAGES.harvest)}`, "the page of the notification opens on B", 20000);
  let target = null;
  for (let k = 0; k < 40 && target === null; k++) {
    target = await B.evaluate(`(() => { const e = document.querySelector(".ProseMirror .is-target"); return e ? norm(e.textContent) : null; })()`);
    if (target === null) await wait(150);
  }
  check(target !== null && target.includes("Crates are counted by"), `[B] the page opened at the block: the paragraph with the mention is the target (${target})`);
  await waitUnread(B, 0, "opening the row marked it read", { kick: [B] });
  check((await badgeText(B)) === "" && (await bellLabel(B)) === "Notifications", "[B] the badge is gone once the row is read");

  // ---- 2. @ in a comment notifies C ------------------------------------------------------------------------------
  const os2 = osMark();
  await openShared(A, spaceId, PAGES.plan);
  await openCommentsPanel(A);
  await A.clickEl(T(".comments-panel button", "New comment on the page"), "New comment on the page");
  await A.waitFor(Q(".comments-panel .comment-draft"), "the draft");
  await writeCommentWithMention(A, ".comments-panel .comment-draft", "Write a comment", { query: "Cla", person: PEOPLE.c, after: COMMENT.aMention });
  await A.waitFor(`(${CARD_WITH(COMMENT.aMention)})`, "the new thread on A");
  const mentionChip = await A.evaluate(`norm((${CARD_WITH(COMMENT.aMention)})?.querySelector(".person-mention")?.textContent ?? "")`);
  check(mentionChip === `@${PEOPLE.c}`, `[A] the comment shows the mention as a chip (${mentionChip})`);
  let threads = await threadsOf(A, planId);
  const pageThread = threads.find((t) => t.comments.some((c) => c.body.some((b) => b.type === "mention")));
  check(pageThread !== undefined && pageThread.anchor === undefined && pageThread.comments[0].body.some((b) => b.type === "mention" && b.accountId === accC), "[A] the core holds a page-level thread whose body has a mention item for C");
  const toastC = await waitToast(C, /Alma Quillfeather mentioned you in a comment on Orchard plan/, "a toast says Alma mentioned C in a comment", { kick: [A, C] });
  check(toastC === `${PEOPLE.a} mentioned you in a comment on ${PAGES.plan}`, `[C] the toast says who mentioned C, in a comment, on which page (${toastC})`);
  await waitUnread(C, 1, "C has one unread notification", { kick: [A, C] });
  const rowC = (await inboxRowsCore(C))[0];
  check(rowC.kind === "CommentMention" && rowC.pageId === planId && typeof rowC.commentId === "string" && rowC.actor === accA, "[C] the row is a CommentMention of the plan page with the comment's id");
  check(!JSON.stringify(rowC).includes(COMMENT.aMention), "[C] the row holds no comment text");
  const osC = await waitOsNotification(/Alma Quillfeather mentioned you in a comment/, { since: os2, ms: 20000 });
  checkOsNotification(osC, { what: "[C] the comment mention", title: PAGES.plan, bodyPattern: /^Alma Quillfeather mentioned you in a comment$/, forbidden: [COMMENT.aMention, "measure the cellar"] });
  check((await unreadCount(B)) === 0 && (await unreadCount(A)) === baseA, "[A] [B] nobody else is told about a comment that mentions C");
  // The notification opens the page and the comment's thread.
  await openShared(C, spaceId, PAGES.harvest);
  await closeCommentsPanel(C);
  await openInbox(C);
  await C.capture("inbox-c");
  await C.clickEl(Q(".inbox .inbox-open"), "the inbox row of C");
  await C.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(PAGES.plan)}`, "the plan page opens on C", 20000);
  await C.waitFor(Q(".comments-panel"), "the comments panel opens with the notification");
  await C.waitUntil(`(${CARD_WITH(COMMENT.aMention)})?.classList.contains("is-focused")`, "the thread of the comment is in focus on C", 20000);
  const mineChip = await C.evaluate(`!!(${CARD_WITH(COMMENT.aMention)})?.querySelector(".person-mention.is-me")`);
  check(mineChip === true, "[C] the comment's chip for C is marked as C's own");
  await waitUnread(C, 0, "opening the row marked it read", { kick: [C] });
  await C.capture("comment-mention-c");

  // ---- 3. nothing arrives for one's own changes -----------------------------------------------------------------------
  const quietBefore = { a: await unreadCount(A), toasts: new Set(await noteToasts(A)) };
  await openShared(A, spaceId, PAGES.harvest);
  await newLine(A);
  await A.type("A note for my own list: ");
  await pickFromMenu(A, "@", "Alm", PEOPLE.a);
  await A.type("remember the ladders.");
  await A.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  check((await mentionBlock(A, harvestId, accA)) !== null, "[A] A mentioned A in a page");
  await openShared(A, spaceId, PAGES.plan);
  await openCommentsPanel(A);
  await A.clickEl(T(".comments-panel button", "New comment on the page"), "New comment on the page");
  await A.waitFor(Q(".comments-panel .comment-draft"), "the draft");
  await writeCommentWithMention(A, ".comments-panel .comment-draft", "Write a comment", { query: "Alm", person: PEOPLE.a, after: COMMENT.self });
  await A.waitFor(`(${CARD_WITH(COMMENT.self)})`, "A's note thread");
  // Everything reached the others (so the changes did make the round trip) and A has been told nothing.
  await eventually(async () => (await threadWithText(B, planId, COMMENT.self)) !== undefined && (await mentionBlock(B, harvestId, accA)) !== null, "[B] B reads A's self-mention in the page and in a comment", { ms: 120000, kick: [A, B] });
  await syncNow(A);
  await wait(6000);
  await syncNow(A);
  await wait(4000);
  const seenA = await noteToasts(A);
  check((await unreadCount(A)) === quietBefore.a, `[A] A mentioning A (a page and a comment) raised no notification on A (${await unreadCount(A)})`);
  check([...seenA].every((text) => quietBefore.toasts.has(text)), `[A] no toast on A for A's own changes (${[...seenA].filter((x) => !quietBefore.toasts.has(x)).join(" | ")})`);
  check((await unreadCount(B)) === 0 && (await unreadCount(C)) === 0, "[B] [C] A's note to self notifies nobody else");

  // ---- 4. B mentions A, in a page and in a comment: A is told (a control that notifications do come the other way) ----------
  const os4 = osMark();
  await openShared(B, spaceId, PAGES.harvest);
  await newLine(B);
  await B.type("Alma, the cider press needs ");
  await pickFromMenu(B, "@", "Alm", PEOPLE.a);
  await B.type("a look.");
  await B.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  await openShared(B, spaceId, PAGES.plan);
  await openCommentsPanel(B);
  await B.clickEl(T(".comments-panel button", "New comment on the page"), "New comment on the page");
  await B.waitFor(Q(".comments-panel .comment-draft"), "the draft");
  // B writes the comment without the mention first: nothing reaches A but the page mention.
  await writeComment(B, ".comments-panel .comment-draft", "Write a comment", COMMENT.bMention, "button");
  await B.waitFor(`(${CARD_WITH(COMMENT.bMention)})`, "B's thread");
  await waitToast(A, /Boris Thistledown mentioned you on Harvest log/, "a toast says Boris mentioned A on Harvest log", { kick: [A, B] });
  await waitUnread(A, quietBefore.a + 1, "A has the page mention", { kick: [A, B] });
  await eventually(async () => (await threadWithText(A, planId, COMMENT.bMention)) !== undefined, "[A] A's core holds B's comment", { ms: 120000, kick: [A, B] });
  await wait(4000);
  check((await unreadCount(A)) === quietBefore.a + 1, "[A] a comment that mentions nobody is no notification");
  // B edits it to add the mention: that is a mention of A.
  const editField = '.comments-panel textarea[aria-label="Edit the comment"]';
  await B.clickEl(`byText("button", "Edit", true, ${CARD_WITH(COMMENT.bMention)})`, "Edit on B's comment");
  await B.clickEl(Q(editField), "the edit field");
  await B.press("Home");
  await B.insertText("@Alm");
  await B.waitFor(Q('[role="listbox"][aria-label="People to mention"]'), "the picker of people");
  for (let k = 0; k < 8; k++) {
    const active = await B.evaluate(`norm(document.querySelector('[role="listbox"][aria-label="People to mention"] [role=option][aria-selected="true"]')?.textContent ?? "")`);
    if (active.includes(PEOPLE.a)) break;
    await B.press("ArrowDown");
    await wait(80);
  }
  await B.press("Enter");
  await wait(300);
  check((await B.run(`return document.querySelector(${JSON.stringify(editField)}).value;`)) === `@${PEOPLE.a} ${COMMENT.bMention}`, "[B] the edit field holds the mention and the old words");
  await B.clickEl(`byText("button", "Save", true, ${CARD_WITH(COMMENT.bMention)})`, "Save");
  await B.waitUntil(`!!(${CARD_WITH(COMMENT.bMention)})?.querySelector(".person-mention")`, "the mention shows as a chip in B's edited comment");
  await waitToast(A, /Boris Thistledown mentioned you in a comment on Orchard plan/, "a toast says Boris mentioned A in a comment", { kick: [A, B] });
  await waitUnread(A, quietBefore.a + 2, "A has two more unread notifications", { kick: [A, B] });
  // A second edit that keeps the mention adds none: A is not told again.
  await B.clickEl(`byText("button", "Edit", true, ${CARD_WITH(COMMENT.bMention)})`, "Edit on B's comment again");
  await B.clickEl(Q(editField), "the edit field");
  await B.press("End");
  await B.insertText(" Thanks.");
  await B.clickEl(`byText("button", "Save", true, ${CARD_WITH(COMMENT.bMention)})`, "Save");
  await eventually(async () => (await threadWithText(A, planId, "Thanks.")) !== undefined, "[A] A's core holds the second edit", { ms: 120000, kick: [A, B] });
  await wait(4000);
  check((await unreadCount(A)) === quietBefore.a + 2, "[A] an edit that keeps a mention is not a second notification");
  const kinds = (await inboxRowsCore(A)).map((r) => r.kind).sort();
  check(JSON.stringify(kinds) === JSON.stringify(["CommentMention", "PageMention", "ThreadReply"]), `[A] A's inbox holds the three kinds that reached A (${kinds.join(", ")})`);
  const osA = await waitOsNotification(/Boris Thistledown mentioned you/, { since: os4, ms: 20000, count: 2 });
  checkOsNotification(osA, { what: "[A] B's mentions", title: PAGES.harvest, bodyPattern: /^Boris Thistledown mentioned you( in a comment)?$/, forbidden: [COMMENT.bMention, "Alma, the cider press"] });
  check((await unreadCount(B)) === 0 && (await unreadCount(C)) === 0, "[B] [C] nobody else is told about B's mentions of A");
  await A.capture("inbox-a-mentions");

  // ---- 5. E, A's second device (a kit recovery), reads the whole history and announces none of it ----------------------------
  const expectThreads = (await threadsOf(A, planId)).length;
  const kitA = readState().needles?.kit?.a;
  must(Array.isArray(kitA) && kitA.length === 24, "A's kit words are known");
  const os5 = osMark();
  await launchInstance("e", { fresh: true });
  const E = await inst("e");
  await E.clickEl(T("button", "Recover with my kit"), `E's "Recover with my kit" button`, { ms: 90000 });
  await E.fill(L("Sync server address"), "E's server address field", SERVER_URL);
  await E.clickEl(Q("textarea.kit-input"), "E's kit field");
  await E.insertText(kitA.join(" "));
  await wait(300);
  await E.clickEl(T("button", "Recover"), "E's Recover button");
  await E.waitFor(Q(".tab-strip"), "E's recovered workspace", 300000);
  await markDocument(E);
  // The whole history is read while this watches the toasts and the inbox of E: nothing may be announced.
  let announced = 0;
  await eventually(
    async () => {
      await noteToasts(E);
      announced = Math.max(announced, (await E.ask({ type: "NotificationList", data: { limit: 200 } })).length);
      const space = await sharedSpaceOf(E);
      if (space === undefined) return false;
      const a = await threadsOf(E, planId).catch(() => []);
      const block = await mentionBlock(E, harvestId, accB).catch(() => null);
      const mine = await mentionBlock(E, harvestId, accA).catch(() => null);
      return a.length >= expectThreads && block !== null && mine !== null;
    },
    "[E] E reads the shared space's history: the threads and the mentions",
    { ms: 300000, kick: [A, E] },
  );
  await wait(8000);
  await syncNow(E);
  await wait(4000);
  const eThreads = await threadsOf(E, planId);
  check(eThreads.length === expectThreads && expectThreads === 6 && eThreads.some((t) => t.anchor?.quote === ANCHOR1) && eThreads.some((t) => t.comments.some((c) => c.body.some((b) => b.type === "mention"))), `[E] E holds all the comment threads of the history (${eThreads.length})`);
  check(announced === 0 && (await unreadCount(E)) === 0 && (await E.ask({ type: "NotificationList", data: { limit: 200 } })).length === 0, `[E] reading the history raised no notification: the inbox of E is empty (${announced} rows seen while it read)`);
  check((await noteToasts(E)).size === 0, "[E] reading the history showed no toast");
  const osRead = await waitOsNotification(/mentioned you|replied to your thread|assigned you/, { since: os5, ms: 4000 });
  if (osRead.verifiable) check(osRead.rows.length === 0, `no OS notification was shown while E read the history (${osRead.rows.length})`);
  else notVerifiable(`no OS notification while E read the history: ${osRead.why}`);
  await E.capture("first-read-e");
  // E is not deaf: a new mention of A reaches A's two devices.
  const os6 = osMark();
  await openShared(B, spaceId, PAGES.harvest);
  await newLine(B);
  await B.type("One more thing for ");
  await pickFromMenu(B, "@", "Alm", PEOPLE.a);
  await B.type("tomorrow.");
  await B.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  await waitToast(E, /Boris Thistledown mentioned you on Harvest log/, "a toast on E says Boris mentioned A", { kick: [B, E, A] });
  await waitUnread(E, 1, "E has the one new notification", { kick: [B, E, A] });
  await waitUnread(A, quietBefore.a + 3, "A has it too", { kick: [B, E, A] });
  const osBoth = await waitOsNotification(/Boris Thistledown mentioned you$/, { since: os6, ms: 20000, count: 2 });
  if (osBoth.verifiable && osBoth.rows.length >= 2) check(true, "the system showed the notification twice: A's two devices each raised it");
  else if (osBoth.verifiable) notVerifiable(`the system's notification database shows ${osBoth.rows.length} of the 2 OS notifications of A's two devices`);
  else notVerifiable(osBoth.why);
  check((await unreadCount(E)) === 1, "[E] E's inbox holds the one notification that arrived after the history was read");
  result.details = { notifications: { a: await unreadCount(A), b: await unreadCount(B), c: await unreadCount(C), e: await unreadCount(E) }, threadsOnE: eThreads.length };
}


// ---------------------------------------------------------------------------------------------------------------
// phase: person
// ---------------------------------------------------------------------------------------------------------------

const PROP = "Assigned";

/** The rows of the table view of the open page: the text of each row. */
const tableRows = (i) => i.evaluate(`[...document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row')].map((r) => norm(r.innerText))`);

/** The person chips of a property row of the open page: the names. */
const chipsOf = (i, name) => i.evaluate(`[...document.querySelectorAll('.properties .property-row[aria-label=${JSON.stringify(name)}] .person-chip-name')].map((e) => norm(e.textContent))`);

/** Picks a person in the picker of a Person property row that is open (or opens it with the row's add button first). */
async function pickPerson(i, name, person) {
  const open = await i.run(`return !!document.querySelector(".person-picker");`);
  if (!open) {
    await i.clickEl(`document.querySelector('.properties .property-row[aria-label=${JSON.stringify(name)}] button.property-chip')`, `the add button of "${name}"`);
    await i.waitFor(Q(".person-picker"), "the person picker");
  }
  const names = await i.evaluate(`[...document.querySelectorAll('.person-picker [role=option]')].map((o) => norm(o.textContent))`);
  must(names.some((n) => n.includes(person)), `[${i.tag}] the person picker offers ${person} (${names.join(", ")})`);
  await i.clickEl(`[...document.querySelectorAll('.person-picker [role=option]')].find((o) => norm(o.textContent).includes(${JSON.stringify(person)}))`, `${person} in the picker`);
  await i.waitUntil(`!document.querySelector(".person-picker")`, "the person picker closed", 10000);
  await wait(800);
}

async function phasePerson() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const E = await inst("e");
  const { spaceId, accounts } = readState();
  must(accounts !== undefined, "the mentions phase recorded the accounts");
  const before = { a: await unreadCount(A), b: await unreadCount(B), c: await unreadCount(C), e: await unreadCount(E) };

  // ---- 1. A makes a page with a table view of its sub-pages and two tasks ------------------------------------------------
  await inSharedSpace(A, spaceId);
  await newPage(A, PAGES.tasks);
  await writeLine(A, TEXT.tasks);
  await newLine(A);
  await slash(A, "table", "Table view");
  must((await A.box(Q('.ProseMirror [data-type="pageList"][data-layout="table"]'))) !== null, "[A] the page has a table view");
  await A.evaluate(`document.activeElement?.blur?.()`);
  await wait(1500);
  const tasksId = await pageIdByTitle(A, spaceId, PAGES.tasks);
  const oneId = await newChild(A, spaceId, tasksId, PAGES.taskOne);
  const twoId = await newChild(A, spaceId, tasksId, PAGES.taskTwo);
  writeState({ pages: { ...readState().pages, tasks: tasksId, taskOne: oneId, taskTwo: twoId } });
  await openShared(A, spaceId, PAGES.tasks);
  await A.waitUntil(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row').length === 2`, "the table view lists the two tasks", 30000);

  // ---- 2. A adds a Person property to the first task and assigns B ----------------------------------------------------------
  const os1 = osMark();
  await openShared(A, spaceId, PAGES.taskOne);
  await A.clickEl(Q("button.properties-add"), "the Add a property button");
  await A.waitFor(Q('[role="menu"]'), "the add-property menu");
  await chooseMenuItem(A, "New property");
  await A.waitFor(Q('[role="dialog"] input[type="text"]'), "the new property dialog");
  await A.clickEl(Q('[role="dialog"] input[type="text"]'), "the property name field");
  await A.insertText(PROP);
  const kinds = await A.evaluate(`[...document.querySelectorAll('[role="dialog"] label.new-property-kind .new-property-kind-name')].map((e) => e.textContent.trim())`);
  check(kinds.includes("Person") && kinds.length === 20, `[A] the new property dialog lists twenty kinds, Person among them (${kinds.length}: ${kinds.includes("Person")})`);
  const kindBox = await A.evaluate(`(() => {
    const l = [...document.querySelectorAll('[role="dialog"] label.new-property-kind')].find((e) => e.querySelector(".new-property-kind-name")?.textContent.trim() === "Person");
    if (!l) return null;
    l.scrollIntoView({ block: "center" });
    const r = l.getBoundingClientRect();
    return { cx: r.x + 20, cy: r.y + r.height / 2 };
  })()`);
  must(kindBox !== null, "[A] the dialog offers the Person kind");
  await A.mouse(kindBox.cx, kindBox.cy);
  check((await A.evaluate(`document.querySelector('[role="dialog"] input[aria-label="Person"]')?.checked ?? null`)) === true, "[A] the Person kind is chosen");
  await A.capture("person-new-property");
  await A.clickEl(`byText("button", "Create property", true, dialog())`, "Create property");
  await A.waitUntil(`!document.querySelector('[role="dialog"]')`, "the new property dialog closed");
  await A.waitFor(Q(`.properties .property-row[aria-label=${JSON.stringify(PROP)}]`), `the row of "${PROP}"`);
  const rowKind = await A.evaluate(`document.querySelector('.properties .property-row[aria-label=${JSON.stringify(PROP)}]')?.getAttribute("data-kind")`);
  check(rowKind === "person", `[A] the new property is a person property (${rowKind})`);
  await pickPerson(A, PROP, PEOPLE.b);
  const chips = await chipsOf(A, PROP);
  check(chips.length === 1 && chips[0] === PEOPLE.b, `[A] the property shows B as a chip (${chips.join(", ")})`);
  await A.capture("person-assigned-a");
  const schema = (await A.ask({ type: "PropertySchemaGet", data: { spaceId } })).properties;
  const def = schema.find((p) => p.name === PROP);
  check(def !== undefined && def.kind === "person", "[A] the schema has the Person property");
  writeState({ propertyId: def.id });
  const values = (await A.ask({ type: "PagePropertiesGet", data: { pageId: oneId } })).values;
  check(values.length === 1 && values[0][0] === def.id && values[0][1].kind === "person" && JSON.stringify(values[0][1].value) === JSON.stringify([accounts.b]), "[A] the core holds B's account, and no other, in the Person value");

  // B is told: toast, count, OS notification; the toast's Open button goes to the page.
  const toastB = await waitToast(B, /Alma Quillfeather assigned you on Graft the plum/, "a toast says Alma assigned B on Graft the plum", { kick: [A, B] });
  check(toastB === `${PEOPLE.a} assigned you on ${PAGES.taskOne}`, `[B] the toast says who assigned B and where (${toastB})`);
  await waitUnread(B, before.b + 1, "B has one more unread notification", { kick: [A, B] });
  const rowB = (await inboxRowsCore(B)).find((r) => r.kind === "PersonAssigned");
  check(rowB !== undefined && rowB.pageId === oneId && rowB.actor === accounts.a && rowB.read === false, "[B] the row is a PersonAssigned of the task page by A");
  const osB = await waitOsNotification(/Alma Quillfeather assigned you/, { since: os1, ms: 20000 });
  checkOsNotification(osB, { what: "[B] the assignment", title: PAGES.taskOne, bodyPattern: /^Alma Quillfeather assigned you$/, forbidden: [] });
  const rowsB = await openInbox(B);
  check(rowsB.some((r) => r.unread && r.text === `${PEOPLE.a} assigned you on ${PAGES.taskOne}`), `[B] the inbox lists the assignment (${JSON.stringify(rowsB.map((r) => r.text))})`);
  await B.capture("inbox-b-person");
  await B.clickEl(`[...document.querySelectorAll(".inbox .inbox-row")].find((li) => li.classList.contains("is-unread") && norm(li.textContent).includes("assigned you"))?.querySelector(".inbox-open")`, "the assignment's inbox row");
  await B.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(PAGES.taskOne)}`, "the task page opens on B", 20000);
  await B.waitFor(Q(`.properties .property-row[aria-label=${JSON.stringify(PROP)}]`), "the property row on B");
  const chipsB = await chipsOf(B, PROP);
  check(chipsB.length === 1 && chipsB[0] === PEOPLE.b, `[B] the task page shows B in the property (${chipsB.join(", ")})`);
  const chipMine = await B.evaluate(`!!document.querySelector('.properties .property-row[aria-label=${JSON.stringify(PROP)}] .person-chip.is-me')`);
  check(chipMine === true, "[B] B's own chip is marked as B's");
  await waitUnread(B, before.b, "opening the row marked it read", { kick: [B] });
  check((await unreadCount(A)) === before.a && (await unreadCount(C)) === before.c, "[A] [C] nobody else is told about the assignment");

  // A Commenter sees the property and cannot change it.
  await openShared(C, spaceId, PAGES.taskOne);
  await C.waitFor(Q(`.properties .property-row[aria-label=${JSON.stringify(PROP)}]`), "the property row on C", 60000);
  const factsC = await C.evaluate(`(() => {
    const row = document.querySelector('.properties .property-row[aria-label=${JSON.stringify(PROP)}]');
    return { chips: [...row.querySelectorAll(".person-chip-name")].map((e) => norm(e.textContent)), remove: !!row.querySelector(".option-chip-remove"), add: !!row.querySelector("button.property-chip") };
  })()`);
  check(factsC.chips.join(",") === PEOPLE.b && factsC.remove === false && factsC.add === false, `[C] a Commenter sees the person and has no control to change it (${JSON.stringify(factsC)})`);
  const forced = await C.core({ type: "PagePropertySet", data: { pageId: oneId, propertyId: def.id, value: { kind: "person", value: [accounts.c] } } });
  check(forced.ok === false && forced.error?.code === "Forbidden", `[C] a forced PagePropertySet is refused with Forbidden (${forced.ok ? "ACCEPTED" : forced.error?.code})`);

  // A assigns A to the second task: the change is A's own, nobody is told.
  const quiet = { a: await unreadCount(A), e: await unreadCount(E), b: await unreadCount(B), c: await unreadCount(C) };
  await openShared(A, spaceId, PAGES.taskTwo);
  await A.clickEl(Q("button.properties-add"), "the Add a property button");
  await A.waitFor(Q('[role="menu"]'), "the add-property menu");
  await chooseMenuItem(A, PROP);
  await A.waitFor(Q(`.properties .property-row[aria-label=${JSON.stringify(PROP)}]`), `the row of "${PROP}" on the second task`);
  await pickPerson(A, PROP, PEOPLE.a);
  const chipsA2 = await chipsOf(A, PROP);
  check(chipsA2.length === 1 && chipsA2[0] === PEOPLE.a, `[A] the second task has A (${chipsA2.join(", ")})`);
  await eventually(async () => JSON.stringify(await B.ask({ type: "PagePropertiesGet", data: { pageId: twoId } })).includes(accounts.a), "[B] B reads A on the second task", { ms: 120000, kick: [A, B, E] });
  await eventually(async () => JSON.stringify(await E.ask({ type: "PagePropertiesGet", data: { pageId: twoId } })).includes(accounts.a), "[E] E, A's other device, reads it too", { ms: 120000, kick: [A, B, E] });
  await wait(3000);
  check((await unreadCount(A)) === quiet.a && (await unreadCount(E)) === quiet.e, `[A] [E] assigning oneself raised no notification on A's two devices (A ${await unreadCount(A)}, E ${await unreadCount(E)})`);
  check((await unreadCount(B)) === quiet.b && (await unreadCount(C)) === quiet.c, "[B] [C] it told nobody else");

  // ---- 3. The "me" filter ------------------------------------------------------------------------------------------------
  // PagesByProperty: "me" is the account asking.
  const pagesFor = (i) => i.ask({ type: "PagesByProperty", data: { spaceId, propertyId: def.id, filter: { type: "ContainsMe" } } });
  check(JSON.stringify(await pagesFor(B)) === JSON.stringify([oneId]), `[B] the core's "me" filter lists the task assigned to B (${(await pagesFor(B)).length} page)`);
  check(JSON.stringify(await pagesFor(A)) === JSON.stringify([twoId]), `[A] the same filter on A lists A's task (${(await pagesFor(A)).length} page)`);
  check(JSON.stringify(await pagesFor(C)) === "[]", "[C] and nothing on C");
  check(JSON.stringify(await A.ask({ type: "PagesByProperty", data: { spaceId, propertyId: def.id, filter: { type: "ContainsPerson", data: accounts.b } } })) === JSON.stringify([oneId]), "[A] ContainsPerson for B lists B's task as well");
  // On screen: B filters the table view of the tasks page by "is me".
  await openShared(B, spaceId, PAGES.tasks);
  await B.waitUntil(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row').length === 2`, "the table view lists both tasks on B", 60000);
  await B.clickEl(`byText("button", "Filter", false, document.querySelector(".page-list-toolbar"))`, "the view's Filter button");
  await B.waitFor(Q(".page-filters"), "the filters popover");
  await B.clickEl(Q(".page-filters .page-filter-add"), "Add a filter");
  await B.waitFor(Q('.page-filters select[aria-label="Property"]'), "the filter's property field");
  await chooseOption(B, `document.querySelector('.page-filters select[aria-label="Property"]')`, PROP, "the filter's property");
  const op = await B.evaluate(`(() => { const s = document.querySelector('.page-filters select[aria-label="Condition"]'); return s ? norm(s.options[s.selectedIndex].textContent) : null; })()`);
  if (op !== "is me") await chooseOption(B, `document.querySelector('.page-filters select[aria-label="Condition"]')`, "is me", "the filter's condition");
  check((await B.evaluate(`(() => { const s = document.querySelector('.page-filters select[aria-label="Condition"]'); return norm(s.options[s.selectedIndex].textContent); })()`)) === "is me", "[B] the condition is \"is me\"");
  await B.capture("person-filter-b");
  await B.press("Escape");
  await B.waitUntil(`!document.querySelector(".page-filters")`, "the filters popover closed", 10000).catch(() => null);
  await B.waitUntil(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row').length === 1`, "the filtered table has one row", 30000);
  let rowsT = await tableRows(B);
  check(rowsT.length === 1 && rowsT[0].includes(PAGES.taskOne), `[B] with "is me" the table view shows the page assigned to B (${JSON.stringify(rowsT)})`);
  await B.capture("person-me-b");
  // The filter is stored in the view, which is shared; "me" is each viewer's own: A reads the same view and sees A's task.
  await eventually(async () => {
    await openShared(A, spaceId, PAGES.tasks);
    const r = await tableRows(A);
    return r.length === 1;
  }, "[A] the shared view reaches A with its filter", { ms: 120000, kick: [A, B] });
  rowsT = await tableRows(A);
  check(rowsT.length === 1 && rowsT[0].includes(PAGES.taskTwo), `[A] the same view on A shows A's task, not B's (${JSON.stringify(rowsT)})`);
  await A.capture("person-me-a");
  result.details = { propertyId: "set", notifications: { a: await unreadCount(A), b: await unreadCount(B), c: await unreadCount(C), e: await unreadCount(E) } };
}


// ---------------------------------------------------------------------------------------------------------------
// phase: reminders
// ---------------------------------------------------------------------------------------------------------------

/** The select of a field of the date picker (its label is the first word of the field). */
const pickerSelect = (label) => `[...document.querySelectorAll(".date-picker label.date-picker-field")].find((l) => norm(l.querySelector("span")?.textContent ?? "") === ${JSON.stringify(label)})?.querySelector("select")`;

/** Sets a value on an input as a person's typing would, for the controlled field React keeps (the browser's own setter and an `input` event). */
async function setInputValue(i, expr, value) {
  await i.run(`const el = (${expr});
    const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(el, ${JSON.stringify(value)});
    el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));`);
  await wait(300);
}

/** The reminder toasts of the page (the `ReminderDue` events), their texts. */
const reminderToasts = (i) => i.evaluate(`[...document.querySelectorAll(".reminder-toast .reminder-toast-text")].map((e) => norm(e.textContent))`);

/** The device's clock as the core's reminders read it: seconds since 1970 on the local wall clock. */
const localSeconds = (plusSeconds = 0) => Math.floor((Date.now() - new Date().getTimezoneOffset() * 60000) / 1000) + plusSeconds;

const pad2 = (n) => String(n).padStart(2, "0");

async function phaseReminders() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const E = await inst("e");
  const { spaceId, pages, accounts } = readState();
  const harvestId = pages.harvest;
  const os0 = osMark();

  // ---- A sets a date with a reminder for B in the shared space ------------------------------------------------------------
  await openShared(A, spaceId, PAGES.harvest);
  await newLine(A);
  await A.type("Frost check at the north rows ");
  await pickFromMenu(A, "@", "pick", "Pick a date…");
  await A.waitFor(Q(".date-picker"), "the date picker");
  const fireAt = new Date(Date.now() + 2 * 60000);
  const time = `${pad2(fireAt.getHours())}:${pad2(fireAt.getMinutes())}`;
  const day = `${fireAt.getFullYear()}-${pad2(fireAt.getMonth() + 1)}-${pad2(fireAt.getDate())}`;
  await setInputValue(A, `document.querySelector('.date-picker input[type="time"]')`, time);
  check((await A.evaluate(`document.querySelector('.date-picker input[type="time"]')?.value ?? null`)) === time, `[A] the time field holds ${time}`);
  const hasRemindWho = await A.evaluate(`!!(${pickerSelect("Remind")})`);
  check(hasRemindWho === false, "[A] the Remind choice shows once there is a reminder (not before)");
  await chooseOption(A, pickerSelect("Reminder"), "At time", "the reminder");
  await A.waitUntil(`!!(${pickerSelect("Remind")})`, "the Remind choice appears", 10000);
  const choices = await A.evaluate(`[...(${pickerSelect("Remind")}).options].map((o) => norm(o.textContent))`);
  check(JSON.stringify(choices) === JSON.stringify(["Space Owners", "Me", PEOPLE.b, PEOPLE.c]), `[A] the Remind choice lists the Owners, A and the other members (${choices.join(", ")})`);
  const initial = await A.evaluate(`(() => { const s = ${pickerSelect("Remind")}; return norm(s.options[s.selectedIndex].textContent); })()`);
  check(initial === "Space Owners", `[A] the Remind choice starts on the Owners (${initial})`);
  await chooseOption(A, pickerSelect("Remind"), PEOPLE.b, "who is reminded");
  await A.capture("reminder-picker");
  await A.press("Escape");
  await A.waitUntil(`!document.querySelector(".date-picker")`, "the date picker closed", 10000);
  await A.type(" please.");
  await A.evaluate(`document.activeElement?.blur?.()`);
  await wait(2000);
  const content = JSON.stringify((await A.ask({ type: "PageContentGet", data: { pageId: harvestId } })).content);
  check(content.includes('"reminderFor":"' + accounts.b + '"') && content.includes(`"time":"${time}"`) && content.includes(`"date":"${day}"`), `[A] the date mention holds ${day} ${time}, the reminder and B as who is reminded`);
  const mentionText = await A.evaluate(`norm([...document.querySelectorAll(".ProseMirror .date-mention")].map((e) => e.textContent).join(" | "))`);
  info(`[A] the date mention reads: ${mentionText}`);

  // ---- before it is due: who would fire it (the core's own rule) ----------------------------------------------------------
  const ahead = localSeconds(86400);
  await eventually(async () => (await B.ask({ type: "RemindersDue", data: { until: ahead } })).some((r) => r.pageId === harvestId && r.time === time), "[B] B's core lists the reminder", { ms: 120000, kick: [A, B, C, E] });
  for (const J of [A, C, E]) {
    // The page and the mention reach them through the same sync; "due" is what their own core would fire.
    await eventually(async () => (await pageText(J, harvestId)).includes("Frost check at the north rows"), `[${J.tag}] ${J.tag} has the page with the date`, { ms: 120000, kick: [A, B, C, E] });
  }
  const due = {};
  for (const [name, J] of Object.entries({ a: A, b: B, c: C, e: E })) due[name] = (await J.ask({ type: "RemindersDue", data: { until: ahead } })).filter((r) => r.pageId === harvestId && r.time === time).length;
  check(due.b === 1 && due.a === 0 && due.c === 0 && due.e === 0, `the core's rule: only B's device would fire it (A ${due.a}, B ${due.b}, C ${due.c}, E ${due.e})`);

  // ---- when it is due: only B shows it --------------------------------------------------------------------------------------
  await openShared(B, spaceId, PAGES.plan);
  const seen = { a: new Set(), b: new Set(), c: new Set(), e: new Set() };
  const end = Date.now() + 5 * 60000;
  let bShown = null;
  while (Date.now() < end && bShown === null) {
    for (const [name, J] of Object.entries({ a: A, b: B, c: C, e: E })) for (const text of await reminderToasts(J).catch(() => [])) seen[name].add(text);
    bShown = [...seen.b].find((text) => text.includes(PAGES.harvest)) ?? null;
    if (bShown === null) await wait(2000);
  }
  must(bShown !== null, `[B] B showed the reminder at ${time} (none within 5 minutes)`);
  info(`[B] the reminder toast reads: ${bShown}`);
  check(/Frost check at the north rows/.test(bShown), `[B] B's reminder toast names the page and holds the block's text (${bShown})`);
  // B's toast opens the page at the block (while it is on screen: it goes after 15 s).
  await B.clickEl(`[...document.querySelectorAll(".reminder-toast")].find((t) => norm(t.textContent).includes(${JSON.stringify(PAGES.harvest)}))?.querySelector("button")`, "the toast's Open button");
  await B.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(PAGES.harvest)}`, "the page opens on B", 20000);
  let target = null;
  for (let k = 0; k < 40 && target === null; k++) {
    target = await B.evaluate(`(() => { const e = document.querySelector(".ProseMirror .is-target"); return e ? norm(e.textContent) : null; })()`);
    if (target === null) await wait(150);
  }
  check(target !== null && target.includes("Frost check at the north rows"), `[B] the toast opened the page at the block (${target})`);
  // The others tick every 30 s as well: wait for two more ticks, then look.
  const quietUntil = Date.now() + 70000;
  while (Date.now() < quietUntil) {
    for (const [name, J] of Object.entries({ a: A, b: B, c: C, e: E })) for (const text of await reminderToasts(J).catch(() => [])) seen[name].add(text);
    await wait(2500);
  }
  check(seen.a.size === 0 && seen.c.size === 0 && seen.e.size === 0, `[A] [C] [E] no other device showed a reminder (A ${seen.a.size}, C ${seen.c.size}, E ${seen.e.size}), B showed ${seen.b.size}`);
  await B.capture("reminder-b");
  const osRem = await waitOsNotification(/Frost check/, { since: os0, ms: 20000 });
  if (osRem.verifiable && osRem.rows.length > 0) {
    check(osRem.rows.length === 1 && osRem.rows[0].title === PAGES.harvest, `the system showed the reminder once, titled with the page (${osRem.rows.length} notification(s))`);
  } else {
    notVerifiable(osRem.verifiable ? "the system's notification database shows no reminder notification (notifications may be off)" : osRem.why);
  }
  // The shell ticks the other devices' cores too: nothing remains due for them either.
  const nowDue = {};
  for (const [name, J] of Object.entries({ a: A, b: B, c: C, e: E })) nowDue[name] = (await J.ask({ type: "RemindersDue", data: { until: localSeconds(0) } })).filter((r) => r.pageId === harvestId && r.time === time).length;
  info(`still due after the ticks: ${JSON.stringify(nowDue)}`);
  result.details = { time, due, shown: { a: seen.a.size, b: seen.b.size, c: seen.c.size, e: seen.e.size } };
}


// ---------------------------------------------------------------------------------------------------------------
// phase: search
// ---------------------------------------------------------------------------------------------------------------

/** Opens the search dialog with a trusted Ctrl+K and returns once its field is there. */
async function openSearch(i) {
  await normalize(i);
  await i.key("k", "KeyK", 75, MOD.ctrl);
  await i.waitFor(Q(".search-dialog .search-input"), "the search dialog");
}

/** The rows of the search results: the group (page) title, the label, the snippet. */
const searchHits = (i) =>
  i.evaluate(`[...document.querySelectorAll(".search-results .search-group")].flatMap((g) => {
    const title = norm(g.querySelector(".search-group-title")?.textContent ?? "");
    return [...g.querySelectorAll('[role="option"]')].map((o) => ({ title, label: norm(o.querySelector(".search-hit-label")?.textContent ?? ""), text: norm(o.textContent) }));
  })`);

/** Every text file of an export of a page or a space, with the planned names. */
async function exportEntries(i, spaceId, pageId, format) {
  const plan = await i.ask({ type: "ExportBegin", data: { spaceId, ...(pageId === undefined ? {} : { pageId }), includeChildren: pageId === undefined, format } });
  const entries = [];
  try {
    for (let k = 0; k < 20; k++) {
      const batch = await i.ask({ type: "ExportNext", data: { exportId: plan.exportId, maxEntries: 100 } });
      entries.push(...batch.entries);
      if (batch.done) break;
    }
  } finally {
    await i.core({ type: "ExportEnd", data: { exportId: plan.exportId } });
  }
  return { plan, entries, text: entries.map((e) => (e.content.type === "Text" ? e.content.data.text : "")).join("\n") };
}

/** Exports the open page through its own "Export..." dialog and the native Save As dialog; returns the file's text. */
async function exportPageUi(i, spaceId, title, file) {
  await openShared(i, spaceId, title);
  rmSync(file, { force: true });
  await normalize(i);
  await i.clickEl(Q('button[aria-label="Page options"]'), "the page options button");
  await i.waitFor(Q('[role="menu"]'), "the page menu");
  await chooseMenuItem(i, "Export\u2026");
  await i.waitFor(Q('[role="dialog"] button[type="submit"]'), "the Export dialog");
  const formats = await i.evaluate(`[...document.querySelectorAll('[role="dialog"] .export-format label.radio')].map((l) => ({ text: l.textContent.trim(), checked: l.querySelector("input").checked }))`);
  check(formats.find((f) => f.checked)?.text === "Markdown", `[${i.tag}] the Export dialog starts on Markdown (${formats.map((f) => f.text).join(", ")})`);
  await i.clickEl(Q('[role="dialog"] button[type="submit"]'), "the Export button");
  const answer = await helper("dialog", { pid: i.rec.pid, title: "Save As", steps: [file], timeoutMs: 30000 });
  check(answer.ok === true, `[${i.tag}] the Save As dialog took the file name and closed${answer.error ? ` (${answer.error})` : ""}`);
  const base = file.split(/[\\/]/).pop();
  await i.waitUntil(`[...document.querySelectorAll(".toasts .toast-text")].some((t) => t.textContent.trim().startsWith("Exported") && t.textContent.includes(${JSON.stringify(base)}))`, `the export's toast for ${base}`, 60000);
  await i.waitUntil(`!document.querySelector('[role="dialog"]')`, "the Export dialog closed", 20000);
  check(existsSync(file), `[${i.tag}] the file ${base} exists`);
  return existsSync(file) ? readFileSync(file, "utf8") : "";
}

async function phaseSearch() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId, pages, accounts } = readState();
  const planId = pages.plan;
  const word = COMMENT.searchWord;

  // ---- a word of a comment is found, labelled "in comment" ---------------------------------------------------------------
  const hits = await B.ask({ type: "Search", data: { query: word, spaceId } });
  const hit = hits.find((h) => h.commentId !== undefined);
  check(hit !== undefined && hit.pageId === planId && hit.blockId === undefined && hit.snippet.toLowerCase().includes(word), `[B] the core finds "${word}" in a comment of the plan page (a hit with a comment id and no block)`);
  await openShared(B, spaceId, PAGES.harvest);
  await openSearch(B);
  await B.clickEl(Q(".search-dialog .search-input"), "the search field");
  await B.insertText(word);
  await B.waitUntil(`!!document.querySelector(".search-hit-label")`, "a result with a label", 20000);
  const shown = await searchHits(B);
  const labelled = shown.filter((h) => h.label === "in comment");
  check(labelled.length >= 1 && labelled.every((h) => h.title === PAGES.plan && h.text.includes(word)), `[B] the search lists the comment with the label "in comment" under its page (${JSON.stringify(shown.map((h) => [h.title, h.label]))})`);
  check(shown.every((h) => h.title === PAGES.plan), "[B] nothing else in the workspace holds the word: no page text, no block");
  await B.capture("search-comment-b");
  // The hit opens the page, and the comment's thread.
  await B.clickEl(`[...document.querySelectorAll('.search-results [role="option"]')].find((o) => o.querySelector(".search-hit-label"))`, "the comment hit");
  await B.waitUntil(`document.querySelector(".content-body h1")?.textContent.trim() === ${JSON.stringify(PAGES.plan)}`, "the plan page opens on B", 20000);
  await B.waitFor(Q(".comments-panel"), "the comments panel opens with the hit");
  await B.waitUntil(`(${CARD(ANCHOR2)})?.classList.contains("is-focused")`, "the thread of the comment is in focus", 20000);
  check(true, "[B] the hit opened the page and put the comment's thread in focus");
  // A word of a person mention is found too (the label is indexed).
  const mentionHits = await B.ask({ type: "Search", data: { query: "Thistledown", spaceId } });
  check(mentionHits.some((h) => h.pageId === pages.harvest && h.commentId === undefined), "[B] the label of a person mention is indexed: the page that mentions B is found by B's name");
  // A word that is in no comment finds no comment.
  const none = await C.ask({ type: "Search", data: { query: "zzzquuxzzz", spaceId } });
  check(none.length === 0, "[C] a word nobody wrote finds nothing");
  const onC = await C.ask({ type: "Search", data: { query: word, spaceId } });
  check(onC.some((h) => h.commentId !== undefined), "[C] a Commenter finds the same comment");

  // ---- the export: @label, and no comment --------------------------------------------------------------------------------
  const md = await A.ask({ type: "PageExportMarkdown", data: { pageId: pages.harvest } });
  const markdown = typeof md === "string" ? md : (md.markdown ?? md.text ?? JSON.stringify(md));
  check(markdown.includes(`@${PEOPLE.b}`) && markdown.includes(`@${PEOPLE.a}`), `[A] the Harvest log page's Markdown has @label for each person mention (${markdown.includes(`@${PEOPLE.b}`)}, ${markdown.includes(`@${PEOPLE.a}`)})`);
  check(!markdown.includes(accounts.b) && !markdown.includes(accounts.a), "[A] the Markdown holds no account id");
  const commentWords = Object.values(COMMENT).filter((t) => t.length > 12);
  const mdPlan = await A.ask({ type: "PageExportMarkdown", data: { pageId: planId } });
  const planMarkdown = typeof mdPlan === "string" ? mdPlan : (mdPlan.markdown ?? mdPlan.text ?? JSON.stringify(mdPlan));
  check(planMarkdown.includes("Early: Prune the pear trees") && commentWords.every((t) => !planMarkdown.includes(t)), "[A] the Markdown of the page that has the comments holds the page's text and none of the comments");
  for (const [format, name] of [["markdown", "Markdown"], ["html", "HTML"]]) {
    const one = await exportEntries(A, spaceId, planId, format);
    check(one.entries.length >= 1 && commentWords.every((t) => !one.text.includes(t)), `[A] the ${name} export of the plan page has none of the ${commentWords.length} comment texts (${one.entries.length} files)`);
    const all = await exportEntries(A, spaceId, undefined, format);
    check(all.plan.pages >= 4 && commentWords.every((t) => !all.text.includes(t)), `[A] the ${name} export of the whole space has none of the comment texts (${all.plan.pages} pages, ${all.entries.length} files)`);
    if (format === "markdown") {
      check(all.text.includes(`@${PEOPLE.b}`), "[A] the whole-space Markdown export keeps @label");
      check(!all.entries.some((e) => /comment/i.test(e.path)), `[A] no file of the export is a comment file (${all.entries.map((e) => e.path).join(", ")})`);
    } else {
      check(all.text.includes(`@${PEOPLE.b}`) && !all.text.includes(accounts.b), "[A] the HTML export writes @label and no account id");
    }
  }
  // The same through the page's own Export dialog and the native Save As: the files on disk.
  const dir = join(OUT, "m13-comments-export");
  mkdirSync(dir, { recursive: true });
  const fileHarvest = await exportPageUi(A, spaceId, PAGES.harvest, join(dir, "harvest.md"));
  check(fileHarvest.includes(`@${PEOPLE.b}`) && fileHarvest.includes(`@${PEOPLE.a}`) && fileHarvest.includes(TEXT.harvest), "[A] the exported Harvest log file has the page's text and @label for each mention");
  const filePlan = await exportPageUi(A, spaceId, PAGES.plan, join(dir, "plan.md"));
  check(filePlan.includes("Early: Prune the pear trees") && commentWords.every((t) => !filePlan.includes(t)), "[A] the exported plan file has the page's text and none of its comments");
  check(!filePlan.includes(accounts.a) && !filePlan.includes(accounts.b) && !fileHarvest.includes(accounts.b), "[A] the exported files hold no account id");
  result.details = { hits: shown.length };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: languages
// ---------------------------------------------------------------------------------------------------------------

const messagesOf = (language) => JSON.parse(readFileSync(join(HERE, "..", "..", "packages", "ui", "src", "i18n", `${language}.json`), "utf8"));
const fill = (template, vars) => template.replace(/\{(\w+)\}/g, (_, key) => String(vars[key] ?? ""));
const KIND_KEYS = { ThreadReply: "threadReply", CommentMention: "commentMention", PageMention: "pageMention", PersonAssigned: "personAssigned" };

async function phaseLanguages() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId, pages } = readState();
  const planId = pages.plan;
  const fr = messagesOf("fr");

  // ---- B chooses French --------------------------------------------------------------------------------------------------
  await goSettings(B);
  await B.clickEl(`document.querySelector('input[name="language"][value="fr"]')`, "the French choice");
  await B.waitUntil(`document.documentElement.lang === "fr"`, "the page's language follows to French", 20000);
  await wait(800);
  const nav = await B.evaluate(`[...document.querySelectorAll("nav.sidebar button")].map((b) => norm(b.textContent))`);
  check(nav.includes(fr["nav.home"]) && nav.includes(fr["nav.settings"]), `[B] the navigation is French (${nav.join(" | ")})`);
  await B.capture("languages-settings-b");
  await openShared(B, spaceId, PAGES.plan);

  // ---- the rows B already has read in French ----------------------------------------------------------------------------
  let rows = await openInbox(B);
  const title = await B.evaluate(`norm(document.querySelector(".inbox .inbox-title")?.textContent ?? "")`);
  const markAll = await B.evaluate(`norm(document.querySelector(".inbox .inbox-header button")?.textContent ?? "")`);
  check(title === fr["inbox.title"] && markAll === fr["inbox.markAll"], `[B] the inbox's title and button are French (${title}; ${markAll})`);
  const core = await inboxRowsCore(B);
  check(core.length >= 3, `[B] B has the notifications of the earlier phases (${core.length})`);
  const expected = core.map((r) => fill(fr[`comments.notify.${KIND_KEYS[r.kind]}`], { actor: r.actorName, page: r.pageTitle ?? "" })).sort();
  check(JSON.stringify(rows.map((r) => r.text).sort()) === JSON.stringify(expected), `[B] every row of the inbox is in French, as the translated phrase of its kind: ${expected.slice(0, 3).join(" | ")}`);
  await B.capture("inbox-b-fr");
  await closeInbox(B);

  // ---- a new notification, in French --------------------------------------------------------------------------------------
  const os = osMark();
  await openShared(A, spaceId, PAGES.plan);
  await openCommentsPanel(A);
  await A.clickEl(T(".comments-panel button", "New comment on the page"), "New comment on the page");
  await A.waitFor(Q(".comments-panel .comment-draft"), "the draft");
  await writeCommentWithMention(A, ".comments-panel .comment-draft", "Write a comment", { query: "Bor", person: PEOPLE.b, after: COMMENT.aToB });
  const toast = await waitToast(B, /mentionné/, "a French toast says Alma mentioned B in a comment", { kick: [A, B] });
  check(toast === fill(fr["comments.notify.commentMention"], { actor: PEOPLE.a, page: PAGES.plan }), `[B] the toast is in French (${toast})`);
  const action = await B.evaluate(`norm([...document.querySelectorAll(".toasts .toast")].find((t) => norm(t.textContent).includes("mentionné"))?.querySelector(".toast-action")?.textContent ?? "")`);
  check(action === fr["comments.notify.open"], `[B] the toast's button is French (${action})`);
  await waitUnread(B, 1, "B has one unread notification", { kick: [A, B] });
  const label = await bellLabel(B);
  check(label === fill(fr["inbox.openUnread.one"], { count: 1 }), `[B] the bell says it in French (${label})`);
  rows = await openInbox(B);
  const unreadRow = rows.find((r) => r.unread);
  check(unreadRow?.text === fill(fr["comments.notify.commentMention"], { actor: PEOPLE.a, page: PAGES.plan }), `[B] the new row is in French (${unreadRow?.text})`);
  const markLabel = await B.evaluate(`norm(document.querySelector(".inbox .inbox-row.is-unread .inbox-mark")?.textContent ?? "")`);
  check(markLabel === fr["inbox.markRead"], `[B] the row's own button is French (${markLabel})`);
  await closeInbox(B);
  const osFr = await waitOsNotification(/mentionné/, { since: os, ms: 20000 });
  checkOsNotification(osFr, { what: "[B] the French OS notification", title: PAGES.plan, bodyPattern: new RegExp(`^${escapeRe(fill(fr["shellDialog.notifyCommentMention"], { actor: PEOPLE.a }))}$`), forbidden: [COMMENT.aToB] });
  // The comments panel and the search label are French as well.
  await openCommentsPanel(B);
  const panelTitle = await B.evaluate(`norm(document.querySelector(".comments-panel .comments-title")?.textContent ?? "")`);
  const resolvedLabel = await B.evaluate(`norm(document.querySelector(".comments-panel .comments-show-resolved")?.textContent ?? "")`);
  check(panelTitle === fr["comments.title"] && resolvedLabel === fr["comments.showResolved"], `[B] the comments panel is French (${panelTitle}; ${resolvedLabel})`);
  await openSearch(B);
  await B.clickEl(Q(".search-dialog .search-input"), "the search field");
  await B.insertText(COMMENT.searchWord);
  await B.waitUntil(`!!document.querySelector(".search-hit-label")`, "a result with a label", 20000);
  const hitLabel = await B.evaluate(`norm(document.querySelector(".search-hit-label")?.textContent ?? "")`);
  check(hitLabel === fr["search.inComment"], `[B] the search result's label is French (${hitLabel})`);
  await B.press("Escape");
  await normalize(B);
  // The other device is still English.
  const langA = await A.evaluate(`document.documentElement.lang`);
  check(langA === "en", `[A] A's language is still English (${langA})`);
  result.details = { rows: core.length, osVerified: osFr.verifiable && osFr.rows.length > 0 };
}


// ---------------------------------------------------------------------------------------------------------------
// phase: restart
// ---------------------------------------------------------------------------------------------------------------

/** What an instance holds of this pass, comparable before and after a restart. */
async function snapshotOf(i, ids) {
  const snap = { threads: null, inbox: null, mentions: null, values: null };
  const threads = await threadsOf(i, ids.plan).catch(() => null);
  snap.threads = threads === null ? null : threads.map((t) => ({ q: t.anchor?.quote ?? null, resolved: t.resolved, resolvedBy: t.resolvedBy ?? null, comments: t.comments.map((c) => ({ a: c.authorName, deleted: c.deleted, edited: c.editedAt !== undefined, body: JSON.stringify(c.body) })) }));
  const inbox = await inboxRowsCore(i);
  snap.inbox = inbox.map((r) => ({ id: r.id, kind: r.kind, read: r.read, page: r.pageTitle ?? null, actor: r.actorName, block: r.blockId ?? null })).sort((x, y) => x.id - y.id);
  const blocks = await blocksOf(i, ids.harvest).catch(() => []);
  snap.mentions = blocks.flatMap((b) => b.inline.filter((n) => n.type === "personMention").map((n) => `${n.attrs.accountId}:${b.text.slice(0, 24)}`)).sort();
  const values = {};
  for (const key of ["taskOne", "taskTwo"]) values[key] = JSON.stringify(await i.ask({ type: "PagePropertiesGet", data: { pageId: ids[key] } }).catch(() => null));
  snap.values = values;
  return snap;
}

async function phaseRestart() {
  const names = Object.entries(readState().instances ?? {}).filter(([, rec]) => typeof rec.pid === "number").map(([name]) => name);
  must(names.length === 4, `the four instances of the pass are running (${names.join(", ")})`);
  const { spaceId, pages } = readState();
  const ids = { plan: pages.plan, harvest: pages.harvest, tasks: pages.tasks, taskOne: pages.taskOne, taskTwo: pages.taskTwo };
  const before = {};
  for (const name of names) before[name] = await snapshotOf(await inst(name), ids);
  info(`before the restart: ${names.map((n) => `${n.toUpperCase()} ${before[n].threads?.length ?? 0} threads, ${before[n].inbox.length} rows (${before[n].inbox.filter((r) => !r.read).length} unread)`).join("; ")}`);
  const unreadBefore = Object.fromEntries(names.map((n) => [n, before[n].inbox.filter((r) => !r.read).length]));
  check(unreadBefore.a === 4 && unreadBefore.b === 1 && unreadBefore.c === 0 && unreadBefore.e === 1, `the unread counts are what the earlier phases left (${JSON.stringify(unreadBefore)})`);
  check(names.every((n) => before[n].threads !== null && before[n].threads.length >= 7), "every instance holds the comment threads before the restart");

  for (const name of names) {
    await stopInstance(name);
    await launchInstance(name);
    const i = await inst(name);
    await i.waitFor(Q(".tab-strip"), "the tab strip", 120000);
    await markDocument(i);
  }
  // Nothing may be announced again at the start (the rows were raised before): watch every toast for a while.
  const quiet = Date.now() + 30000;
  while (Date.now() < quiet) {
    for (const name of names) await noteToasts(await inst(name));
    await wait(2000);
  }
  for (const name of names) {
    const i = await inst(name);
    let after = null;
    await eventually(async () => {
      after = await snapshotOf(i, ids);
      return JSON.stringify(after) === JSON.stringify(before[name]);
    }, `[${i.tag}] the comments, notifications, mentions and Person values are as before the restart`, { ms: 120000, kick: [i] });
    check(JSON.stringify(after) === JSON.stringify(before[name]), `[${i.tag}] comments (${after.threads.length} threads), notifications (${after.inbox.length} rows, ${after.inbox.filter((r) => !r.read).length} unread, the read state as it was), mentions (${after.mentions.length}) and Person values are as before the restart`);
  }
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const E = await inst("e");
  const announced = names.map((n) => [...(toastLog[n] ?? [])]).flat();
  check(announced.length === 0, `no toast of a notification after the restart: the history is not announced again (${announced.length})`);

  // ---- on screen ----------------------------------------------------------------------------------------------------------------
  check((await badgeText(A)) === "4" && (await bellLabel(A)) === "Notifications (4 unread)", `[A] the bell still counts the four unread (${await badgeText(A)})`);
  check((await badgeText(C)) === "", "[C] C's bell has no badge");
  await openShared(A, spaceId, PAGES.plan);
  await openCommentsPanel(A);
  await A.waitFor(`(${CARD(ANCHOR2)})`, "C's thread in A's panel");
  await wait(1000);
  let hl = await highlightsOf(A);
  check(hl.some((h) => h.text === ANCHOR2 && !h.resolved) && !hl.some((h) => h.text === ANCHOR1), `[A] after the restart the open thread is highlighted and the resolved one is not (${JSON.stringify(hl.map((h) => h.text))})`);
  const factsRow = await cardFacts(A, ANCHOR2);
  check(factsRow.rows.length >= 2 && factsRow.rows[0].author === PEOPLE.c && factsRow.rows.some((r) => r.author === PEOPLE.a && r.body.includes(COMMENT.a2)), "[A] C's thread has C's comment and A's reply");
  await A.clickEl(`document.querySelector('.comments-panel .comments-show-resolved input')`, "Show resolved");
  await A.waitFor(`(${CARD(ANCHOR1)})`, "the resolved thread");
  const resolvedFacts = await cardFacts(A, ANCHOR1);
  check(resolvedFacts.resolved && resolvedFacts.resolvedText === `Resolved by ${PEOPLE.b}` && resolvedFacts.rows.length === 3 && resolvedFacts.rows[1].deleted, `[A] the resolved thread, its placeholder and "${resolvedFacts.resolvedText}" are there after the restart`);
  hl = await highlightsOf(A);
  check(hl.some((h) => h.text === ANCHOR1 && h.resolved) && hl.some((h) => h.text === ANCHOR2), "[A] both highlights are on the same words after the restart");
  await A.capture("restart-comments-a");
  await openShared(A, spaceId, PAGES.harvest);
  await A.waitUntil(`document.querySelectorAll(".ProseMirror .person-mention").length >= 4`, "the mention chips of the Harvest log page", 30000);
  const chips = await A.evaluate(`[...document.querySelectorAll(".ProseMirror .person-mention")].map((e) => ({ text: norm(e.textContent), me: e.classList.contains("is-me") }))`);
  check(chips.filter((c) => c.text === `@${PEOPLE.b}`).length === 1 && chips.filter((c) => c.text === `@${PEOPLE.a}` && c.me).length === 3, `[A] the mentions are chips after the restart: B once, A three times and marked as A's own (${JSON.stringify(chips.map((c) => c.text))})`);
  await openShared(A, spaceId, PAGES.taskTwo);
  await A.waitFor(Q(`.properties .property-row[aria-label=${JSON.stringify(PROP)}]`), "the Person property row");
  check(JSON.stringify(await chipsOf(A, PROP)) === JSON.stringify([PEOPLE.a]), "[A] the Person property shows A after the restart");
  await openShared(B, spaceId, PAGES.tasks);
  await B.waitUntil(`document.querySelectorAll('.ProseMirror [data-type="pageList"][data-layout="table"] .page-table-body .page-table-row').length === 1`, "the table view's \"me\" filter on B", 60000);
  const rowsT = await tableRows(B);
  check(rowsT.length === 1 && rowsT[0].includes(PAGES.taskOne), `[B] the "me" filter of the table view still shows B's task after the restart (${JSON.stringify(rowsT)})`);
  const fr = messagesOf("fr");
  const langB = await B.evaluate(`document.documentElement.lang`);
  check(langB === "fr", `[B] the language is still French after the restart (${langB})`);
  check((await bellLabel(B)) === fill(fr["inbox.openUnread.one"], { count: 1 }), `[B] the bell is French and counts the one unread (${await bellLabel(B)})`);
  const rowsB = await openInbox(B);
  const coreB = await inboxRowsCore(B);
  check(rowsB.length === coreB.length && rowsB.filter((r) => r.unread).length === 1, `[B] the inbox lists the same rows with the same read state (${rowsB.length} rows, ${rowsB.filter((r) => r.unread).length} unread)`);
  await B.capture("restart-inbox-b");
  await closeInbox(B);

  // ---- the notifications still arrive after the restart ----------------------------------------------------------------------
  await openShared(A, spaceId, PAGES.plan);
  await openCommentsPanel(A);
  await A.waitFor(`(${CARD(ANCHOR2)})`, "C's thread on A");
  await A.clickEl(`byText("button", "Reply", true, ${CARD(ANCHOR2)})`, "the Reply link on C's thread");
  await writeComment(A, ".comments-panel", "Reply to the thread", COMMENT.a4, "enter");
  await waitToast(C, /Alma Quillfeather replied to your thread on Orchard plan/, "a toast on C: Alma replied again", { kick: [A, C] });
  await waitUnread(C, 1, "C has the new notification", { kick: [A, C] });
  check((await unreadCount(B)) === 1 && (await unreadCount(A)) === 4 && (await unreadCount(E)) === 1, "[A] [B] [E] nobody else is told");
  // B goes back to English (the rest of the repository's passes run in English).
  await B.clickEl(T("nav.sidebar button", fr["nav.settings"]), "the Settings button (French)");
  await B.waitFor(Q('input[name="language"][value="en"]'), "the language choices");
  await B.clickEl(`document.querySelector('input[name="language"][value="en"]')`, "the English choice");
  await B.waitUntil(`document.documentElement.lang === "en"`, "the page's language follows to English", 20000);
  result.details = { restarted: names, unread: unreadBefore };
}


// ---------------------------------------------------------------------------------------------------------------
// phase: former (A removes B: what B wrote reads "Former member")
// ---------------------------------------------------------------------------------------------------------------

/** The member row of a person in the open panel, as a page-side expression. */
const rowOf = (name) => `[...document.querySelectorAll(".members-dialog .member-item")].find((li) => li.querySelector(".member-name")?.textContent.trim() === ${JSON.stringify(name)})`;

async function phaseFormer() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const E = await inst("e");
  const { spaceId, pages, accounts } = readState();
  const planId = pages.plan;
  const former = `Former member (${PEOPLE.b})`;
  const before = { a: await unreadCount(A), c: await unreadCount(C), e: await unreadCount(E) };

  // ---- A removes B ------------------------------------------------------------------------------------------------------------
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  await A.clickEl(`byText("button", "Remove", true, ${rowOf(PEOPLE.b)})`, "B's Remove button");
  await A.waitFor(T("h2", `Remove ${PEOPLE.b}?`), "the removal question");
  await A.clickEl(TD("button", `Remove ${PEOPLE.b}`), "the Remove button of the question");
  await waitMembers(A, "the member list no longer has B", (r) => r.length === 2 && !r.some((m) => m.name === PEOPLE.b), 90000, [A]);
  await closeDialog(A, "Close");
  const info = await membersOf(A, spaceId);
  check(info.formerMembers?.some((f) => f.accountId === accounts.b && f.name === PEOPLE.b) === true, "[A] the core lists B among the former members, with the last name it knew");
  const mentionable = await A.ask({ type: "PeopleMentionable", data: { spaceId } });
  check(mentionable.length === 2 && !mentionable.some((p) => p.accountId === accounts.b), `[A] B is no longer offered by the @ menu (${mentionable.map((p) => p.name).join(", ")})`);
  const forced = await A.core({ type: "PagePropertySet", data: { pageId: pages.taskTwo, propertyId: readState().propertyId, value: { kind: "person", value: [accounts.b] } } });
  check(forced.ok === false && forced.error?.code === "InvalidInput" && forced.error?.details?.field === "value", `[A] a Person value naming a person who is no member is refused with InvalidInput for value (${forced.ok ? "ACCEPTED" : `${forced.error?.code} ${JSON.stringify(forced.error?.details ?? {})}`})`);
  await eventually(async () => (await membersOf(C, spaceId)).members.every((m) => m.name !== PEOPLE.b), "[C] C reads that B is gone", { ms: 180000, kick: [A, C, E] });
  await eventually(async () => (await sharedSpaceOf(B))?.shareState === "Archived", "[B] B's space became a read-only copy", { ms: 240000, kick: [A, B] });

  // ---- what B wrote reads "Former member (Boris Thistledown)" on A and on C -----------------------------------------------------
  for (const J of [A, C]) {
    await openShared(J, spaceId, PAGES.plan);
    await openCommentsPanel(J);
    // The resolved thread (with B's deleted reply) is listed while "Show resolved" is on: a restarted app starts with it off.
    if (!(await J.evaluate(`document.querySelector(".comments-panel .comments-show-resolved input")?.checked === true`))) {
      await J.clickEl(`document.querySelector('.comments-panel .comments-show-resolved input')`, "the Show resolved checkbox");
    }
    await J.waitFor(`(${CARD_WITH(COMMENT.bMention)})`, `B's thread on ${J.tag}`);
    await J.waitUntil(`norm((${CARD_WITH(COMMENT.bMention)})?.querySelector(".comment-author")?.textContent ?? "") === ${JSON.stringify(former)}`, `the author of B's thread reads "${former}" on ${J.tag}`, 60000);
    const mark = await J.evaluate(`!!(${CARD_WITH(COMMENT.bMention)})?.querySelector(".person-mark.is-former")`);
    check(mark === true, `[${J.tag}] B's comment is marked as a former member's`);
    const placeholder = await J.evaluate(`norm([...document.querySelectorAll(".comments-panel .comment.is-deleted .comment-author")].map((e) => e.textContent).join("|"))`);
    check(placeholder.includes(former), `[${J.tag}] the placeholder of B's deleted reply names "${former}" (${placeholder})`);
  }
  await A.capture("former-comments-a");
  // The mention chips and the Person value.
  await openShared(A, spaceId, PAGES.harvest);
  await A.waitFor(Q(`.ProseMirror .person-mention[data-account-id="${accounts.b}"]`), "the chip of B in the Harvest log page");
  const chip = await A.evaluate(`(() => { const e = document.querySelector('.ProseMirror .person-mention[data-account-id="${accounts.b}"]'); return { text: norm(e.textContent), former: e.classList.contains("is-former"), title: e.getAttribute("title") }; })()`);
  check(chip.former === true && chip.text === `@${former}` && /no longer a member/.test(chip.title ?? ""), `[A] B's chip in the page reads "${chip.text}" and says why (${chip.title})`);
  const chipOk = await A.evaluate(`[...document.querySelectorAll(".ProseMirror .person-mention.is-me")].length`);
  check(chipOk === 3, `[A] A's own chips are still A's (${chipOk})`);
  await openShared(A, spaceId, PAGES.taskOne);
  await A.waitFor(Q(`.properties .property-row[aria-label=${JSON.stringify(PROP)}]`), "the Person property row");
  const personChip = await A.evaluate(`(() => { const e = document.querySelector('.properties .property-row[aria-label=${JSON.stringify(PROP)}] .person-chip'); return e ? { text: norm(e.textContent), former: e.classList.contains("is-former") } : null; })()`);
  check(personChip !== null && personChip.former === true && personChip.text.includes(former), `[A] the Person value reads "${personChip?.text}" and is marked as a former member's`);
  await A.capture("former-person-a");
  // The inbox: the people who acted are named as the space names them now.
  const rows = await openInbox(A);
  const byB = rows.filter((r) => r.text.includes(PEOPLE.b));
  check(byB.length >= 3 && byB.every((r) => r.text.startsWith(former)), `[A] every row of B's actions names "${former}" (${byB.length} rows: ${byB.map((r) => r.text.slice(0, 40)).join(" | ")})`);
  await A.capture("former-inbox-a");
  await closeInbox(A);
  check((await unreadCount(A)) === before.a && (await unreadCount(C)) === before.c && (await unreadCount(E)) === before.e, "[A] [C] [E] removing B raised no notification anywhere");
  result.details = { former: true };
}


// ---------------------------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------------------------

const PHASE_FUNCS = {
  setup: phaseSetup,
  comments: phaseComments,
  mentions: phaseMentions,
  person: phasePerson,
  reminders: phaseReminders,
  search: phaseSearch,
  languages: phaseLanguages,
  restart: phaseRestart,
  former: phaseFormer,
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
  if (phase !== "setup") {
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
// The report file has every note; the console has what failed (or, with M13C_VERBOSE, everything).
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
      ...(process.env.M13C_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
for (const i of Object.values(insts)) i.close();
process.exit(result.ok ? 0 : 1);
