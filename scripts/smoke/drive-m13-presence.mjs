// The in-page half of the presence pass (drive-m13-presence.ps1, Milestone 13 slice 3, realtime and presence): trusted mouse and key
// input, the DOM, the core (through the pages' own IPC), the processes, the server and the logs, over the DevTools ports of several
// app instances at once (the calls of cdp.mjs, one connection per instance; the infrastructure is that of drive-m13-comments.mjs and
// drive-m13-share.mjs). Phases:
//
//   node drive-m13-presence.mjs setup     <outfile>  a fresh server, A, B and C on fresh data, A shares a space, invites B (Editor) and C (Viewer) by code
//   node drive-m13-presence.mjs realtime  <outfile>  A and B on one page: text typed in one appears in the other in under a second (20 times each way),
//                                                    with no pull in between
//   node drive-m13-presence.mjs presence  <outfile>  carets, selections, names, colours and avatars of A, B and C (a Viewer too); a moved selection
//                                                    follows; A leaves the page and goes
//   node drive-m13-presence.mjs removal   <outfile>  A removes C (its presence goes, it gets no more entries); a second device of B's account is
//                                                    paired and revoked (its socket is closed, it cannot reconnect, its presence goes)
//   node drive-m13-presence.mjs reconnect <outfile>  the server is stopped and started while A and B have the page open: both reconnect and
//                                                    resubscribe, the edits made meanwhile arrive once, realtime and presence are back
//   node drive-m13-presence.mjs restart   <outfile>  A is killed (its presence times out on B), started again (the counter is seeded from the
//                                                    clock: B accepts it), then every instance is started again: presence comes back, nothing stale
//   node drive-m13-presence.mjs logs      <outfile>  the logs of the server and of every instance
//   node drive-m13-presence.mjs verify    <outfile>  the summary of every phase
//   node drive-m13-presence.mjs stop      <outfile>  stops the instances this pass started (by process id)
//
// Every page phase writes what each page asked the network for, what failed, the console and the content-security-policy violations
// it saw, as JSON, and exits 1 when a check fails, when a page asked another host for something (the pages never talk to the server:
// the shell does), when a violation, a failed request or a console error was seen, or when the document of a webview was replaced
// (the marker `window.__m13pMarker` is lost by a reload). The invite codes and the kit words are kept in the state file for the log
// scan, and never written to a report.

import { spawn } from "node:child_process";
import { appendFileSync, copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const [phase, outFile] = process.argv.slice(2);
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = process.env.M13P_OUT_DIR ?? join(HERE, "out");
const SHARE_PS1 = process.env.M13P_SHARE_PS1 ?? join(HERE, "drive-m13-share.ps1");
const SERVER_PS1 = join(HERE, "server.ps1");
const BASE_PORT = Number(process.env.M13P_BASE_PORT ?? 9533);
const STATE_FILE = join(OUT, "m13-presence-state.json");
const SERVER_URL = process.env.M13P_SERVER_URL ?? "http://127.0.0.1:8807";
const SERVER_LISTEN = SERVER_URL.replace(/^https?:\/\//, "");
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
const SPACE = "Meadow Studio";
const PAGES = { plan: "Meadow plan", notes: "Meadow notes" };
const PEOPLE = { a: "Ines Larkspur", b: "Bram Ashdown", c: "Cora Willowmere" };
const INITIALS = { a: "IL", b: "BA", c: "CW" };
const WORKSPACES = { a: "Larkspur Alpha", b: "Larkspur Beta", c: "Larkspur Gamma", d: "Larkspur Delta", e: "Larkspur Epsilon" };
const SECOND_DEVICE = "Bram second laptop";
/** The palette index of a member is their join order (A 0, B 1, C 2), and the page draws the palette's colour (`packages/ui/src/presence.ts`). */
const COLOURS = { a: "rgb(194, 65, 12)", b: "rgb(29, 78, 216)", c: "rgb(21, 128, 61)" };
const TEXT = {
  p1: "Lanterns hang above the wooden table at dusk.",
  p2: "Guests arrive after the market closes.",
  p3: "The kitchen opens before the doors do.",
  notes: "Remember the folding chairs from the barn.",
  duringA: "while the server was away Ines kept typing",
  duringB: "while the server was away Bram kept typing",
  afterRemoval: "a line written after Cora was removed",
};
const SEL1 = "wooden table";
const SEL2 = "Guests arrive";
const SEL3 = "kitchen opens";
const SEL4 = "market closes";
/** The limits of the spec: a token typed in one window is in the other in under a second; presence times out after 30 s. */
const LATENCY_LIMIT_MS = 1000;
const PRESENCE_TIMEOUT_S = 30;
/** A repeat of a phase on the same data (a debugging aid): its tokens must not be those of the first run. */
const RUN = Number(process.env.M13P_RUN ?? 0);


// ---------------------------------------------------------------------------------------------------------------
// helper steps of the .ps1 files (native dialogs, captures, onboarding, launching, killing)
// ---------------------------------------------------------------------------------------------------------------

let helperCount = 0;
/** Runs one of drive-m13-share.ps1's helper steps (dialog, capture, onboard, spawn, kill, ...); its parameters go in a JSON file, its answer comes back as JSON. */
function helper(step, params = {}, extraEnv = {}) {
  helperCount += 1;
  const argFile = join(OUT, `m13-presence-helper-${process.pid}-${helperCount}.arg.json`);
  const resultFile = join(OUT, `m13-presence-helper-${process.pid}-${helperCount}.result.json`);
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
// stop: the instances this pass started
// ---------------------------------------------------------------------------------------------------------------

async function stopAll() {
  const state = readState();
  // The headless Edge of the view link check, if one was left behind (a process this pass started, by id).
  if (typeof state.edge?.pid === "number") {
    spawn("taskkill", ["/PID", String(state.edge.pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
    writeState({ edge: null });
  }
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
/** The log files of the pass: every instance's (`m13-presence-a.err.log`, `-r1`, and the stdout files) and the server's. */
const logFiles = () => [
  ...readdirSync(OUT).filter((n) => /^m13-presence-[a-e](-r\d+)?\.(err|out)\.log$/.test(n)).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })),
  ...readdirSync(OUT).filter((n) => /^m13-presence-server-.*\.log$/.test(n)),
  ...["server.log", "server.err.log"].filter((n) => existsSync(join(OUT, n))),
];

function needleList() {
  const state = readState();
  const named = [
    SPACE,
    ...Object.values(PAGES),
    ...Object.values(PEOPLE),
    ...Object.values(WORKSPACES),
    "Larkspur",
    "Ashdown",
    "Willowmere",
    "Ines",
    "Bram",
    "Cora",
    SECOND_DEVICE,
    ...Object.values(TEXT).flatMap((text) => [text, text.split(" ").slice(0, 4).join(" ")]),
    SEL1,
    SEL2,
    SEL3,
    SEL4,
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
  { label: "a run of 16 or more byte values (a dump of a frame)", re: /(?:\b\d{1,3}, ){16,}/ },
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
  result.notes.push(`${log.leaks.length === 0 ? "ok" : "FAIL"}: none of the page texts, titles, space names, display names (the names presence shows), workspace names, device names, invite codes or kit words is in a log (${log.leaks.length} leaks${log.leaks.length ? `: ${log.leaks.map((l) => `${l.needle} x${l.count} in ${l.file}`).join(", ")}` : ""}; ${log.needles} needles)`);
  result.notes.push(`${log.idHits.length === 0 ? "ok" : "FAIL"}: no full id, hash or key bytes in a log, only 8-hex ids (${log.idHits.length} kinds of hit${log.idHits.length ? `: ${log.idHits.map((h) => `${h.kind} x${h.count} in ${h.file}`).join(", ")}` : ""})`);
  const control =
    /(?<![A-Za-z0-9_])Meadow plan(?![A-Za-z0-9_])/.test("INFO a made-up line that says Meadow plan in it") &&
    !/(?<![A-Za-z0-9_])Ines(?![A-Za-z0-9_])/.test("INFO a line about lines and Inesa") &&
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

const PHASES = ["setup", "realtime", "presence", "removal", "reconnect", "restart", "logs"];
if (phase === "verify") {
  const summary = { ok: true, phases: {}, captures: [], failedNotes: [], notVerifiable: [], foreignRequests: 0, cspViolations: 0, failedRequests: 0, consoleErrors: 0, bugs: [], details: {}, log: null };
  for (const name of PHASES) {
    const file = join(OUT, `m13-presence-${name}.json`);
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
    const path = join(OUT, `m13-presence-${label}.png`);
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
const dirsOf = (name) => ({ data: join(OUT, `m13-presence-${name}-data`), webview: join(OUT, `m13-presence-${name}-webview`) });

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
  const stem = launches === 0 ? `m13-presence-${name}` : `m13-presence-${name}-r${launches}`;
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
  const href = await i.evaluate(`(() => { window.__m13pMarker = ${JSON.stringify(marker)}; return location.href; })()`);
  patchInstance(i.name, { marker, href });
}

/** The webview's own document: nothing may have replaced it. */
async function intact(i, when) {
  const rec = i.rec;
  const now = await i.evaluate(`(() => ({ marker: window.__m13pMarker ?? null, href: location.href }))()`).catch(() => null);
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

const kitFileOf = (name) => join(OUT, `m13-presence-kit-${name}.txt`);

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
// what this pass adds to the shared infrastructure: drags and selections, opening pages
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
  // Nothing of this pass needs the real keyboard or mouse: every key and click is a DevTools event, and UI Automation (the onboarding) is checked by what it did.
  if (session.locked) info("the Windows session is locked (LogonUI.exe runs): no check of this pass uses a real key, a real click or the clipboard (they are DevTools events), so none is NOT VERIFIABLE on that account");

  const server = await fetch(`${SERVER_URL}/v2/server`).then((r) => r.json());
  check(server.protocolVersion === 2, `the server answers on protocol v2 (${server.protocolVersion}) at ${SERVER_URL}`);
  check(typeof server.serverEpoch === "string" && server.serverEpoch.length > 0, "the server has an epoch (it started fresh)");

  // M13P_NO_C=1 (the short smoke pass): only A and B, and no Viewer.
  const withC = process.env.M13P_NO_C !== "1";
  const names = withC ? ["a", "b", "c"] : ["a", "b"];
  for (const name of names) await launchInstance(name, { fresh: true });
  await Promise.all(names.map((name) => onboard(name)));
  for (const name of names) {
    const i = await inst(name);
    await i.waitFor(Q(".tab-strip"), "the tab strip", 60000);
    await markDocument(i);
    check((await heading(i)) === WORKSPACES[name], `[${i.tag}] the main area shows the workspace's Home (${await heading(i)})`);
    check((await syncStatus(i)).configured === false, `[${i.tag}] sync is not configured yet`);
    await i.capture(`launched-${name}`);
  }
  const A = await inst("a");
  const B = await inst("b");
  const C = withC ? await inst("c") : null;

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
  await writeLine(A, TEXT.p3);
  await newPage(A, PAGES.notes);
  await writeLine(A, TEXT.notes);
  const planId = await pageIdByTitle(A, spaceId, PAGES.plan);
  const notesId = await pageIdByTitle(A, spaceId, PAGES.notes);
  must(planId !== null && notesId !== null, "[A] the core has both pages");
  writeState({ pages: { plan: planId, notes: notesId } });
  const planText = await pageText(A, planId);
  check(planText.includes(TEXT.p1) && planText.includes(TEXT.p2) && planText.includes(TEXT.p3), "[A] the plan page holds its three paragraphs");

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

  // B joins as an Editor and C as a Viewer, each with a code.
  await inviteByCode(A, B, spaceId, { role: "Editor", name: PEOPLE.b, codeKey: "codeB" });
  await B.capture("joined-b");
  if (withC) {
    await inviteByCode(A, C, spaceId, { role: "Viewer", name: PEOPLE.c, codeKey: "codeC" });
    await C.capture("joined-c");
  }

  // Both read the pages; the roles are what the invitations said.
  const treeA = await treeOf(A, spaceId);
  for (const J of withC ? [B, C] : [B]) {
    await eventually(async () => sameShape(shapeOf(await treeOf(J, spaceId)), shapeOf(treeA)), `[${J.tag}] ${J.tag}'s tree equals A's`, { ms: 180000, kick: [A, J] });
    const planJ = await pageIdByTitle(J, spaceId, PAGES.plan);
    const text = await pageText(J, planJ);
    check(text.includes(TEXT.p1) && text.includes(TEXT.p3), `[${J.tag}] reads the plan page's text`);
  }
  await openShared(B, spaceId, PAGES.plan);
  const factsB = await readOnlyFacts(B);
  check(factsB.editable === "true" && factsB.pill === "", `[B] as Editor the page is editable (${factsB.editable}, "${factsB.pill}")`);
  if (withC) {
    await openShared(C, spaceId, PAGES.plan);
    const factsC = await readOnlyFacts(C);
    check(factsC.editable === "false" && factsC.pill !== "", `[C] as Viewer the page is not editable and says so ("${factsC.pill}", contenteditable ${factsC.editable})`);
    await C.capture("viewer-page");
  }
  await openMembers(A);
  const rows = await waitMembers(A, "A's list shows the people with their roles", (r) => r.length === (withC ? 3 : 2) && r.some((m) => m.name === PEOPLE.b && m.role === "Editor") && (!withC || r.some((m) => m.name === PEOPLE.c && m.role === "Viewer")), 90000, withC ? [A, B, C] : [A, B]);
  check(rows.find((m) => m.you)?.role === "Owner", "[A] the list shows A as Owner");
  await closeDialog(A, "Close");
  // The device ids (a short form is what the server's log shows), to tell whose request a log line is.
  const devices = {};
  for (const i of withC ? [A, B, C] : [A, B]) devices[i.name] = (await i.ask({ type: "DeviceInfo" })).deviceId;
  writeState({ devices });
  check(Object.values(devices).every((d) => typeof d === "string" && d.length >= 8), "every device has an id");
  result.details = { members: withC ? 3 : 2, roles: withC ? { a: "Owner", b: "Editor", c: "Viewer" } : { a: "Owner", b: "Editor" }, server: SERVER_URL };
}



// ---------------------------------------------------------------------------------------------------------------
// the server, its log and the apps' logs (what happened on the wire, from outside)
// ---------------------------------------------------------------------------------------------------------------

/** Runs kb-server's script (`server.ps1 -Action stop|start`: start keeps the data; it only ever stops the binary of this checkout). */
function serverCtl(action) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", SERVER_PS1, "-Action", action, "-Listen", SERVER_LISTEN], { stdio: "ignore", windowsHide: true });
    child.on("error", reject);
    child.on("exit", (code) => resolvePromise(code));
  });
}

/** The server's request log: `{ at (ms), method, path, account, device, feed, status }` per line (the log holds requests only, never bodies). */
function serverRequests(file = "server.log") {
  const path = join(OUT, file);
  if (!existsSync(path)) return [];
  const rows = [];
  for (const line of stripAnsi(readFileSync(path, "utf8")).split(/\r?\n/)) {
    const m = /^(\S+Z)\s+INFO request\{method=(\w+) path=(\S+)(?: account="([0-9a-f]+)")?(?: device="([0-9a-f]+)")?(?: feed="([0-9a-f]+)")?\}: .*?status=(\d+)/.exec(line);
    if (m) rows.push({ at: Date.parse(m[1]), method: m[2], path: m[3], account: m[4] ?? null, device: m[5] ?? null, feed: m[6] ?? null, status: Number(m[7]) });
  }
  return rows;
}

/** The server's lines that are not requests (the realtime sockets' closing counts, warnings): `{ at, text }`. */
function serverOtherLines(pattern, file = "server.log") {
  const path = join(OUT, file);
  if (!existsSync(path)) return [];
  return stripAnsi(readFileSync(path, "utf8"))
    .split(/\r?\n/)
    .map((line) => {
      const m = /^(\S+Z)\s+\w+\s+(.*)$/.exec(line);
      return m && pattern.test(m[2]) ? { at: Date.parse(m[1]), text: m[2] } : null;
    })
    .filter((x) => x !== null);
}

/** Whether a short device id of the server's log (8 hex digits) is this device. */
const isDevice = (short, deviceId) => typeof short === "string" && typeof deviceId === "string" && (deviceId.startsWith(short) || deviceId.replace(/-/g, "").startsWith(short));

const isPull = (r) => r.method === "GET" && /^\/v2\/feeds\/[0-9a-f.]+\/changes/.test(r.path);
const isPush = (r) => r.method === "POST" && /^\/v2\/feeds\/[0-9a-f.]+\/entries/.test(r.path);
const isSocket = (r) => r.method === "GET" && r.path === "/v2/ws";

/** The lines of an instance's own log (every file of its launches) that match `pattern`, with their time: `{ at, text }`. */
function appLogLines(name, pattern) {
  const found = [];
  const files = readdirSync(OUT).filter((n) => new RegExp(`^m13-presence-${name}(-r\\d+)?\\.err\\.log$`).test(n));
  for (const file of files) {
    for (const line of stripAnsi(readFileSync(join(OUT, file), "utf8")).split(/\r?\n/)) {
      const m = /^(\d{4}-\d\d-\d\dT[\d:.]+Z)\s+\w+\s+(.*)$/.exec(line);
      if (m && pattern.test(m[2])) found.push({ at: Date.parse(m[1]), text: m[2], file });
    }
  }
  return found.sort((a, b) => a.at - b.at);
}

const inWindow = (rows, from, to) => rows.filter((r) => r.at >= from && r.at <= to);

/** Median and 95th percentile (nearest rank) of a list of numbers. */
function stats(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const n = sorted.length;
  if (n === 0) return { n: 0, median: null, p95: null, min: null, max: null };
  const median = n % 2 === 1 ? sorted[(n - 1) / 2] : (sorted[n / 2 - 1] + sorted[n / 2]) / 2;
  return { n, median: round(median), p95: round(sorted[Math.ceil(0.95 * n) - 1]), min: round(sorted[0]), max: round(sorted[n - 1]) };
}

// ---------------------------------------------------------------------------------------------------------------
// presence as the pages show it: avatars, carets, selections; and as the core lists it
// ---------------------------------------------------------------------------------------------------------------

const presenceList = (i, spaceId) => i.ask({ type: "PresenceList", data: { spaceId } });

/** The avatars of the open page's header: who is on the page (title = the name, `idle` when dimmed). */
const pageAvatars = (i) =>
  i.evaluate(`[...document.querySelectorAll(".presence-page .presence-avatar:not(.presence-avatar-more)")].map((e) => ({ name: (e.getAttribute("title") ?? "").replace(/ \\(idle\\)$/, ""), initials: norm(e.querySelector("[aria-hidden]")?.textContent ?? ""), colour: getComputedStyle(e).backgroundColor, idle: e.classList.contains("is-idle") }))`);

/** The avatars of the space switcher's button: who is in the space. */
const spaceAvatars = (i) =>
  i.evaluate(`[...document.querySelectorAll(".space-button .presence-avatars .presence-avatar:not(.presence-avatar-more)")].map((e) => ({ name: (e.getAttribute("title") ?? "").replace(/ \\(idle\\)$/, ""), initials: norm(e.querySelector("[aria-hidden]")?.textContent ?? ""), colour: getComputedStyle(e).backgroundColor, idle: e.classList.contains("is-idle") }))`);

/** The carets and the selections of other members drawn in the open page's editor. */
const cursorsOf = (i) =>
  i.evaluate(`(() => {
    const pm = document.querySelector(".ProseMirror");
    if (!pm) return { carets: [], selections: [] };
    const carets = [...pm.querySelectorAll(".loro-cursor")].map((c) => { const label = c.querySelector(".loro-cursor-label"); return { name: norm(label?.textContent ?? ""), caret: getComputedStyle(c).borderLeftColor, label: label ? getComputedStyle(label).backgroundColor : null }; });
    const selections = [...pm.querySelectorAll(".loro-cursor-selection")].map((s) => ({ text: s.textContent, colour: s.style.backgroundColor }));
    return { carets, selections };
  })()`);

const namesOf = (list) => list.map((p) => p.name);
const sameNames = (list, names) => JSON.stringify(namesOf(list)) === JSON.stringify(names);
const rgbOf = (c) => String(c ?? "").replace(/^rgba?\(|\)$/g, "").split(",").slice(0, 3).map((x) => x.trim()).join(",");
/** The `rgba(r, g, b, a)` of a selection and the `rgb(r, g, b)` of a caret are the same colour. */
const sameColour = (a, b) => rgbOf(a) === rgbOf(b);

/** Waits until a predicate of the open page's cursors holds; returns the time it took (ms) and the last reading. */
async function waitCursors(i, what, predicate, ms = 20000) {
  const started = Date.now();
  let last = { carets: [], selections: [] };
  while (Date.now() - started < ms) {
    last = await cursorsOf(i).catch(() => last);
    if (predicate(last)) return { ms: Date.now() - started, last };
    await wait(50);
  }
  must(false, `[${i.tag}] ${what} (timed out after ${round(ms / 1000)} s; carets ${JSON.stringify(last.carets.map((c) => c.name))}, selections ${JSON.stringify(last.selections.map((s) => s.text))})`);
  return { ms: Date.now() - started, last };
}

/** Waits until the page's avatars (header) or the space's (switcher) are exactly `names`, in order; returns the time it took. */
async function waitAvatars(i, what, kind, names, ms = 30000) {
  const read = kind === "page" ? pageAvatars : spaceAvatars;
  const started = Date.now();
  let last = [];
  while (Date.now() - started < ms) {
    last = await read(i).catch(() => last);
    if (sameNames(last, names)) return { ms: Date.now() - started, last };
    await wait(100);
  }
  must(false, `[${i.tag}] ${what} (timed out after ${round(ms / 1000)} s; ${kind} avatars ${JSON.stringify(namesOf(last))}, wanted ${JSON.stringify(names)})`);
  return { ms: Date.now() - started, last };
}

/** The devices of a list that belong to a member name (a PresenceList reading). */
const devicesOf = (list, name) => list.filter((d) => d.name === name);

// ---------------------------------------------------------------------------------------------------------------
// typing and measuring: the probe records, with the page's own clock, when a token first shows in the editor's text
// ---------------------------------------------------------------------------------------------------------------

/** Installs the probe in an instance's page (once): `window.__lat[token]` = `Date.now()` when the token first showed in the open editor. */
async function installProbe(i) {
  await i.evaluate(`(() => {
    if (window.__latObserver) return true;
    window.__lat = {};
    const scan = () => {
      const pm = document.querySelector(".ProseMirror");
      if (!pm) return;
      const now = Date.now();
      for (const m of pm.textContent.matchAll(/zq[abcd]\\d{2,3}x/g)) if (window.__lat[m[0]] === undefined) window.__lat[m[0]] = now;
    };
    window.__latObserver = new MutationObserver(scan);
    window.__latObserver.observe(document.body, { subtree: true, childList: true, characterData: true });
    return true;
  })()`);
}

/** When a token first showed in an instance's editor (ms since the epoch on the machine's clock), or null. */
const seenAt = (i, token) => i.evaluate(`window.__lat?.[${JSON.stringify(token)}] ?? null`);

/** Types ` token` at the end of the open page in `from` (a trusted insertion) and waits until `to` shows it. */
async function typeToken(from, to, token, ms = 10000) {
  await from.evaluate(`document.querySelector(".ProseMirror").editor.commands.focus("end")`);
  await from.insertText(` ${token}`);
  const end = Date.now() + ms;
  let tTo = null;
  while (Date.now() < end) {
    tTo = await seenAt(to, token);
    if (tTo !== null) break;
    await wait(10);
  }
  const tFrom = await seenAt(from, token);
  return { token, tFrom, tTo, ms: tFrom !== null && tTo !== null ? tTo - tFrom : null };
}

/** The text of the open editor, from the page, without the names above other members' carets (they are in the editor's DOM, not in the page). */
const EDITOR_TEXT = `(() => { const pm = document.querySelector(".ProseMirror"); if (!pm) return ""; const copy = pm.cloneNode(true); for (const c of copy.querySelectorAll(".loro-cursor")) c.remove(); return copy.textContent ?? ""; })()`;
const editorText = (i) => i.evaluate(EDITOR_TEXT);

/** How many times each token of the list is in an instance's open editor text. */
const tokenCounts = (i, tokens) =>
  i.evaluate(`(() => { const text = ${EDITOR_TEXT}; const out = {}; for (const t of ${JSON.stringify(tokens)}) out[t] = text.split(t).length - 1; return out; })()`);

/** The pull cycles in a list of pull requests: requests less than 5 s apart are one cycle (a cycle pulls the personal feed and each shared feed). */
function pullCycles(pulls) {
  const sorted = [...pulls].sort((a, b) => a.at - b.at);
  const cycles = [];
  for (const p of sorted) {
    const last = cycles[cycles.length - 1];
    if (last !== undefined && p.at - last.end < 5000) last.end = p.at;
    else cycles.push({ start: p.at, end: p.at });
  }
  return cycles;
}

/** Opens a page of the shared space in an instance and waits until its editor is there. */
async function openPlan(i, spaceId, title = PAGES.plan) {
  const id = await openShared(i, spaceId, title);
  await i.waitFor(Q(".ProseMirror"), `[${i.tag}] the editor of "${title}"`, 30000);
  return id;
}

/** The window as the page's presence sees it: shown and in use (the pass emulates the focus; the window may still be hidden behind others). */
const windowFacts = (i) => i.evaluate(`({ visible: document.visibilityState, focus: document.hasFocus() })`);



// ---------------------------------------------------------------------------------------------------------------
// phase: realtime
// ---------------------------------------------------------------------------------------------------------------

/** Waits until an instance's presence list holds a device of the other instance (both sockets are up and subscribed). */
async function waitPeer(i, spaceId, deviceId, what, ms = 90000) {
  await eventually(async () => (await presenceList(i, spaceId)).some((d) => d.deviceId === deviceId), `[${i.tag}] ${what}`, { ms });
}

/** One direction of the measurement: `count` tokens typed in `from`, each waited for in `to`, 1.5 s apart (each is a push of its own). */
async function measureDirection(from, to, prefix, count, deviceTo) {
  const trials = [];
  const t0 = Date.now();
  for (let k = 1; k <= count; k++) {
    const token = `zq${prefix}${String(RUN * 100 + k).padStart(2, "0")}x`;
    trials.push(await typeToken(from, to, token));
    await wait(1500);
  }
  const t1 = Date.now();
  return { trials, t0, t1, deviceTo };
}

async function phaseRealtime() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId, devices } = readState();
  must(typeof devices?.a === "string" && typeof devices?.b === "string", "the setup kept the device ids");
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  for (const i of [A, B]) {
    const facts = await windowFacts(i);
    info(`[${i.tag}] the window: visibility ${facts.visible}, focus ${facts.focus}`);
    check(facts.visible === "visible" && facts.focus === true, `[${i.tag}] the window is the one in use (shown and focused: presence follows it)`);
  }
  // Both sockets are up and subscribed when each sees the other's device in the core's list of who is where.
  await waitPeer(A, spaceId, devices.b, "A's list of who is where has B's device (the realtime sockets are up)");
  await waitPeer(B, spaceId, devices.a, "B's list of who is where has A's device");
  check(true, "[A] [B] each one's presence list holds the other's device: the sockets are up and the frames are relayed");
  await A.capture("realtime-a");
  await B.capture("realtime-b");

  // A paragraph of its own for the tokens, made by A, read by B.
  await newLine(A);
  await A.insertText("Latency log:");
  await wait(2500);
  await eventually(async () => (await editorText(B)).includes("Latency log:"), "[B] the latency paragraph arrived", { ms: 30000 });
  await installProbe(A);
  await installProbe(B);

  const aToB = await measureDirection(A, B, "a", 20, devices.b);
  const bToA = await measureDirection(B, A, "b", 20, devices.a);
  const report = {};
  for (const [label, run, receiver, receiverName] of [["A to B", aToB, devices.b, "b"], ["B to A", bToA, devices.a, "a"]]) {
    const arrived = run.trials.filter((t) => t.ms !== null);
    check(arrived.length === run.trials.length, `${label}: all ${run.trials.length} tokens arrived (${arrived.length})`);
    const s = stats(arrived.map((t) => t.ms));
    info(`${label}: latency of ${s.n} tokens: median ${s.median} ms, 95th percentile ${s.p95} ms, min ${s.min} ms, max ${s.max} ms (limit ${LATENCY_LIMIT_MS} ms)`);
    check(s.median !== null && s.median < LATENCY_LIMIT_MS, `${label}: the median latency is ${s.median} ms (under ${LATENCY_LIMIT_MS} ms)`);
    check(s.p95 !== null && s.p95 < LATENCY_LIMIT_MS, `${label}: the 95th percentile of the latency is ${s.p95} ms (under ${LATENCY_LIMIT_MS} ms)`);
    // No sync cycle in between: the receiving device made no pull while a token was on its way, and few at all (the pull interval is 60 s).
    const pulls = serverRequests().filter((r) => isPull(r) && isDevice(r.device, receiver));
    const during = inWindow(pulls, run.t0, run.t1 + 500);
    let overlapping = 0;
    for (const t of arrived) overlapping += inWindow(pulls, t.tFrom - 20, t.tTo + 100).length > 0 ? 1 : 0;
    const cycles = inWindow(appLogLines(receiverName, /sync cycle/), run.t0, run.t1 + 500);
    const feedPulls = inWindow(appLogLines(receiverName, /pulled one feed/), run.t0, run.t1 + 500);
    const seconds = round((run.t1 - run.t0) / 1000);
    const pullCyclesDuring = pullCycles(during).length;
    info(`${label}: in the ${seconds} s of the run the receiver made ${during.length} pull request(s) to the server in ${pullCyclesDuring} cycle(s) (${overlapping} of the ${arrived.length} tokens were in flight at one), and its own log shows ${cycles.length} sync cycle(s) and ${feedPulls.length} pulled feed(s)`);
    check(overlapping === 0, `${label}: no pull of the receiving device was under way while a token was in flight (${overlapping} of ${arrived.length} overlapped one): they came through the socket`);
    check(pullCyclesDuring <= 1 + Math.floor(seconds / 60), `${label}: the receiver pulled in ${pullCyclesDuring} cycle(s) in ${seconds} s (the periodic pull is every 60 s, so at most ${1 + Math.floor(seconds / 60)}): nothing like one pull per token`);
    report[label] = { ...s, tokens: run.trials.map((t) => t.ms), serverPulls: during.length, pullCycles: pullCyclesDuring, syncCycles: cycles.length, pulledFeeds: feedPulls.length, seconds };
  }
  // Both type at the same moment (information, not a criterion): the driver that applies what the socket relays also runs the
  // device's own push cycles, one after the other, so a window that has just typed may hear the other a cycle late.
  const both = [];
  for (let k = 1; k <= 10; k++) {
    const ta = `zqc${String(RUN * 100 + k).padStart(2, "0")}x`;
    const tb = `zqd${String(RUN * 100 + k).padStart(2, "0")}x`;
    const [x, y] = await Promise.all([typeToken(A, B, ta), typeToken(B, A, tb)]);
    both.push(x, y);
    await wait(1500);
  }
  const simultaneous = stats(both.filter((t) => t.ms !== null).map((t) => t.ms));
  info(`both windows typing at once: ${simultaneous.n} of ${both.length} tokens arrived, median ${simultaneous.median} ms, 95th percentile ${simultaneous.p95} ms, max ${simultaneous.max} ms`);
  check(both.every((t) => t.ms !== null), "every token typed by both windows at once arrived (none lost to the collision of two pushes)");
  report["both at once"] = { ...simultaneous };
  // The text is the same in both windows, every token once.
  const tokens = [...aToB.trials, ...bToA.trials, ...both].map((t) => t.token);
  const countsA = await tokenCounts(A, tokens);
  const countsB = await tokenCounts(B, tokens);
  check(tokens.every((t) => countsA[t] === 1 && countsB[t] === 1), "every token is in both pages exactly once (no duplicate from the inline path and the pull)");
  check((await editorText(A)) === (await editorText(B)), "A's and B's pages have the same text");
  await eventually(
    async () => {
      const idA = await pageIdByTitle(A, spaceId, PAGES.plan);
      const textA = await pageText(A, idA);
      const textB = await pageText(B, await pageIdByTitle(B, spaceId, PAGES.plan));
      return tokens.every((t) => textA.includes(t) && textB.includes(t));
    },
    "the core of A and of B hold every token",
    { ms: 60000, kick: [A, B] },
  );
  result.details = { latencyMs: report, limitMs: LATENCY_LIMIT_MS };
}



// ---------------------------------------------------------------------------------------------------------------
// phase: presence
// ---------------------------------------------------------------------------------------------------------------

/** Records, with the page's clock, every change of this page's own selection (`__selLog`) and of the selection another member's cursor draws (`__curLog`). */
async function installCursorProbe(i) {
  await i.evaluate(`(() => {
    if (window.__curObserver) return true;
    window.__selLog = [];
    window.__curLog = [];
    document.addEventListener("selectionchange", () => {
      const text = String(window.getSelection()?.toString() ?? "");
      const last = window.__selLog[window.__selLog.length - 1];
      if (text !== "" && (!last || last.text !== text)) window.__selLog.push({ at: Date.now(), text });
    });
    const scan = () => {
      const pm = document.querySelector(".ProseMirror");
      if (!pm) return;
      const carets = [...pm.querySelectorAll(".loro-cursor-label")].map((e) => e.textContent).join("|");
      const text = [...pm.querySelectorAll(".loro-cursor-selection")].map((e) => e.textContent).join("");
      const last = window.__curLog[window.__curLog.length - 1];
      if (!last || last.text !== text || last.carets !== carets) window.__curLog.push({ at: Date.now(), text, carets });
    };
    window.__curObserver = new MutationObserver(scan);
    window.__curObserver.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
    return true;
  })()`);
}

/** When this page's selection became `text` (the last time it did), or null. */
const selectedAt = (i, text) => i.evaluate(`[...(window.__selLog ?? [])].reverse().find((e) => e.text === ${JSON.stringify(text)})?.at ?? null`);
/** When the cursors drawn in this page first (after `since`) drew exactly `text` as their selection, or null. */
const drawnAt = (i, text, since) => i.evaluate(`(window.__curLog ?? []).find((e) => e.text === ${JSON.stringify(text)} && e.at >= ${since})?.at ?? null`);

/** The cursors of one member (by name) in a reading: the caret(s) and the selection text(s) of that name's colour. */
function ofMember(reading, who) {
  const carets = reading.carets.filter((c) => c.name === PEOPLE[who]);
  const selections = reading.selections.filter((s) => sameColour(s.colour, COLOURS[who]));
  return { carets, selections, text: selections.map((s) => s.text).join("") };
}

async function phasePresence() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId, devices } = readState();
  const peers = { a: [B, C], b: [A, C], c: [A, B] };
  const byName = { a: A, b: B, c: C };
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  await openPlan(C, spaceId);
  for (const i of [A, B, C]) {
    const facts = await windowFacts(i);
    check(facts.visible === "visible" && facts.focus === true, `[${i.tag}] the window is the one in use (shown and focused: ${facts.visible}, ${facts.focus})`);
    await installCursorProbe(i);
  }

  // ---- avatars: who is on the page (header) and who is in the space (switcher), never yourself ----------------------------------------
  const expected = { a: ["b", "c"], b: ["a", "c"], c: ["a", "b"] };
  const avatarTimes = {};
  for (const who of ["a", "b", "c"]) {
    const i = byName[who];
    const names = expected[who].map((w) => PEOPLE[w]);
    const page = await waitAvatars(i, `${i.tag}'s page header shows ${names.join(" and ")} (not itself)`, "page", names, 60000);
    const space = await waitAvatars(i, `${i.tag}'s space switcher shows ${names.join(" and ")}`, "space", names, 60000);
    avatarTimes[who] = { page: page.ms, space: space.ms };
    for (const w of expected[who]) {
      const one = page.last.find((p) => p.name === PEOPLE[w]);
      check(one !== undefined && one.initials === INITIALS[w] && sameColour(one.colour, COLOURS[w]) && one.idle === false, `[${i.tag}] the page avatar of ${w.toUpperCase()} has the initials ${INITIALS[w]} and the colour of join order ${["a", "b", "c"].indexOf(w)} (${one?.initials}, ${one?.colour})`);
    }
    check(true, `[${i.tag}] shows ${names.join(" and ")} on the page and in the space, and not itself`);
  }
  await A.capture("presence-avatars-a");
  await B.capture("presence-avatars-b");
  await C.capture("presence-avatars-c");
  // The core's own list agrees: the devices of the others, their names and their places in join order.
  for (const who of ["a", "b", "c"]) {
    const i = byName[who];
    const list = await presenceList(i, spaceId);
    const others = list.filter((d) => d.deviceId !== devices[who]);
    check(others.length === 2 && expected[who].every((w) => devicesOf(list, PEOPLE[w]).length === 1), `[${i.tag}] PresenceList names the two other members, one device each (${list.map((d) => d.name).join(", ")})`);
    for (const w of expected[who]) {
      const d = devicesOf(list, PEOPLE[w])[0];
      check(d !== undefined && d.color === ["a", "b", "c"].indexOf(w) && d.deviceId === devices[w], `[${i.tag}] ${w.toUpperCase()}'s device comes with the colour index of its join order (${d?.color}) and its own id`);
    }
  }

  // ---- A selects words: B and C draw the caret and the selection with A's name and colour ----------------------------------------------
  const t0 = Date.now();
  await selectPhrase(A, SEL1);
  const tA = await selectedAt(A, SEL1);
  const seen = {};
  for (const J of [B, C]) {
    const { last } = await waitCursors(J, `${J.tag} draws A's selection "${SEL1}"`, (r) => ofMember(r, "a").text === SEL1, 20000);
    const a = ofMember(last, "a");
    const at = await drawnAt(J, SEL1, t0);
    seen[J.name] = at !== null && tA !== null ? at - tA : null;
    check(a.carets.length === 1 && sameColour(a.carets[0].caret, COLOURS.a) && sameColour(a.carets[0].label, COLOURS.a), `[${J.tag}] A's caret carries A's name and A's colour (${a.carets.map((c) => `${c.name} ${c.caret}`).join(", ")})`);
    check(a.selections.length >= 1 && a.selections.every((s) => sameColour(s.colour, COLOURS.a)), `[${J.tag}] A's selection is drawn in A's colour (${a.selections.map((s) => s.colour).join(", ")})`);
    check(a.text === SEL1, `[${J.tag}] the selection is the words A selected ("${a.text}")`);
  }
  info(`A's selection of "${SEL1}" was drawn ${seen.b} ms (B) and ${seen.c} ms (C) after A made it`);
  check(Object.values(seen).every((ms) => ms !== null && ms < 2000), `A's selection is drawn on B and on C within 2 s (B ${seen.b} ms, C ${seen.c} ms)`);
  const listAfter = await presenceList(B, spaceId);
  check(devicesOf(listAfter, PEOPLE.a)[0]?.state === "Editing", `[B] A's device is Editing in B's list now that it has a selection (${devicesOf(listAfter, PEOPLE.a)[0]?.state})`);
  check(devicesOf(listAfter, PEOPLE.c)[0]?.state === "Viewing", `[B] C's device (a Viewer) is Viewing, never Editing (${devicesOf(listAfter, PEOPLE.c)[0]?.state})`);
  await B.capture("presence-a-selection-b");
  await C.capture("presence-a-selection-c");

  // ---- B selects words: A and C draw B's cursors with B's colour ------------------------------------------------------------------------
  await selectPhrase(B, SEL3);
  for (const J of [A, C]) {
    const { last } = await waitCursors(J, `${J.tag} draws B's selection "${SEL3}"`, (r) => ofMember(r, "b").text === SEL3, 20000);
    const b = ofMember(last, "b");
    check(b.carets.length === 1 && sameColour(b.carets[0].caret, COLOURS.b), `[${J.tag}] B's caret carries B's name and B's colour (${b.carets.map((c) => `${c.name} ${c.caret}`).join(", ")})`);
  }
  const readingC = await cursorsOf(C);
  check(ofMember(readingC, "a").text === SEL1 && ofMember(readingC, "b").text === SEL3, `[C] a Viewer sees both members' selections at once ("${ofMember(readingC, "a").text}" and "${ofMember(readingC, "b").text}")`);

  // ---- C, a Viewer, selects words in the read-only page: it is on the page, and its caret is drawn when the read-only editor lets it --------
  await selectPhrase(C, SEL4);
  const softCursors = async (i, predicate, ms) => {
    const until = Date.now() + ms;
    while (Date.now() < until) {
      if (predicate(await cursorsOf(i).catch(() => ({ carets: [], selections: [] })))) return true;
      await wait(100);
    }
    return false;
  };
  const viewerCaret = (await softCursors(A, (r) => ofMember(r, "c").text === SEL4, 8000)) && (await softCursors(B, (r) => ofMember(r, "c").text === SEL4, 8000));
  if (viewerCaret) check(true, "[A] [B] a Viewer's selection in the read-only page is drawn, in C's name and colour");
  else {
    const viewerFocus = await C.evaluate(`document.querySelector(".ProseMirror").editor.view.hasFocus()`);
    info(`DESIGN GAP: a Viewer's selection in the read-only page is not drawn on the others' pages: the cursor plugin writes a selection only while the editor has the focus (view.hasFocus() is ${viewerFocus} on C after the drag), and a read-only editor never has it. The Viewer is shown by its avatar and its Viewing state`);
    bug("a Viewer's (read-only) selection is never shared: the vendored cursor plugin publishes only while view.hasFocus(), which is false for a non-editable editor (design gap, avatars still show the Viewer)");
    check((await pageAvatars(A)).some((p) => p.name === PEOPLE.c), "[A] a Viewer is on the page all the same (its avatar)");
  }
  await A.capture("presence-all-a");

  // ---- A moves its selection: B's decoration follows ----------------------------------------------------------------------------------------
  const tMove = Date.now();
  await selectPhrase(A, SEL2);
  const tA2 = await selectedAt(A, SEL2);
  const moved = await waitCursors(B, `B's drawing of A's selection follows to "${SEL2}"`, (r) => ofMember(r, "a").text === SEL2, 20000);
  const followAt = await drawnAt(B, SEL2, tMove);
  info(`A's selection moved to "${SEL2}": B drew it ${followAt !== null && tA2 !== null ? followAt - tA2 : "?"} ms after`);
  check(ofMember(moved.last, "a").text === SEL2 && ofMember(moved.last, "a").carets.length === 1, `[B] A's selection moved: only "${SEL2}" is drawn for A, and one caret`);
  check(ofMember(await cursorsOf(C), "a").text === SEL2, "[C] A's selection moved on C as well");
  // The selection grows by keys (a trusted Shift+ArrowRight, six times): the head moves, the anchor stays.
  await A.press("ArrowRight", MOD.shift, 6);
  const grown = await waitCursors(B, "B's drawing of A's selection grows with the keys", (r) => ofMember(r, "a").text === `${SEL2} after`, 20000);
  check(ofMember(grown.last, "a").text === `${SEL2} after`, `[B] A's selection grew with Shift+ArrowRight ("${ofMember(grown.last, "a").text}")`);
  // A collapses the selection with a click: the caret stays, the selection goes.
  const box = await phraseBox(A, SEL3);
  await A.mouse(box.x0 + 2, box.y0);
  const collapsed = await waitCursors(B, "B's drawing of A's selection is a caret only", (r) => ofMember(r, "a").carets.length === 1 && ofMember(r, "a").selections.length === 0, 20000);
  check(ofMember(collapsed.last, "a").selections.length === 0, "[B] a click in A's page left a caret and no selection");
  await selectPhrase(A, SEL2);
  await waitCursors(B, "B draws A's selection again", (r) => ofMember(r, "a").text === SEL2, 20000);

  // ---- A leaves the page: the avatar and the caret go from B within the timeout, then A goes idle ----------------------------------------------
  const tLeave = Date.now();
  await openPlan(A, spaceId, PAGES.notes);
  const gone = await waitAvatars(B, "A's avatar is gone from B's page header", "page", [PEOPLE.c], PRESENCE_TIMEOUT_S * 1000);
  const goneCaret = await waitCursors(B, "A's caret and selection are gone from B's page", (r) => ofMember(r, "a").carets.length === 0 && ofMember(r, "a").selections.length === 0, PRESENCE_TIMEOUT_S * 1000);
  info(`A left the page: its avatar was gone from B's header ${Date.now() - tLeave - (goneCaret.ms > 0 ? 0 : 0)} ms later (header ${gone.ms} ms after the check began, caret ${goneCaret.ms} ms)`);
  check(Date.now() - tLeave < PRESENCE_TIMEOUT_S * 1000, `A's avatar and caret went from B within the ${PRESENCE_TIMEOUT_S} s timeout (${round((Date.now() - tLeave) / 1000)} s)`);
  const space = await waitAvatars(B, "B's space switcher still shows A and C: A is in the space, on another page", "space", [PEOPLE.a, PEOPLE.c], 30000);
  check(space.last.every((p) => p.idle === false), "[B] A is not marked idle while it is on another page");
  const listB = await presenceList(B, spaceId);
  const notesId = readState().pages.notes;
  check(devicesOf(listB, PEOPLE.a)[0]?.pageId === notesId, "[B] PresenceList puts A's device on the notes page now");
  // No page at all (Settings): the device goes idle (its last frame says so).
  await goSettings(A);
  const idle = await eventually(async () => (await spaceAvatars(B)).find((p) => p.name === PEOPLE.a)?.idle === true, "[B] A shows as idle in the space switcher once it is on no page", { ms: 30000 });
  check(idle, "[B] A went idle in the space switcher when it left every page (one Idle frame, not the timeout)");
  await B.capture("presence-a-idle-b");
  // Back on the plan page: the avatar and the caret come back.
  await openPlan(A, spaceId);
  await waitAvatars(B, "A's avatar is back on B's page header", "page", [PEOPLE.a, PEOPLE.c], 30000);
  await selectPhrase(A, SEL1);
  await waitCursors(B, "A's selection is drawn again on B", (r) => ofMember(r, "a").text === SEL1, 20000);
  check(true, "[B] A came back to the page: avatar and selection are back");
  result.details = { avatarMs: avatarTimes, selectionDrawnMs: seen, aLeftToGoneMs: Date.now() - tLeave, viewerCaretDrawn: viewerCaret };
}



// ---------------------------------------------------------------------------------------------------------------
// phase: removal (A removes C; a second device of B's account is paired and revoked)
// ---------------------------------------------------------------------------------------------------------------

/** The row of a member in the open member panel. */
const rowOf = (name) => `[...document.querySelectorAll(".members-dialog .member-item")].find((li) => li.querySelector(".member-name")?.textContent.trim() === ${JSON.stringify(name)})`;

/** Keeps a kit's words for the log scan (never in a report). */
function keepKit(owner, words) {
  const state = readState();
  writeState({ needles: { ...(state.needles ?? {}), kit: { ...(state.needles?.kit ?? {}), [owner]: words } } });
}

/** The new recovery kit of the removal dialog: its words are read and kept for the log scan, the three words asked are typed back, Done. */
async function finishKitFlow(i, owner) {
  await i.waitFor(Q(".kit-words"), "the new recovery kit's words", 120000);
  // The words are listed twice in the page (the dialog's list and the hidden print sheet): the first list is the dialog's.
  const words = await i.evaluate(`[...document.querySelectorAll(".kit-words")[0].querySelectorAll(".kit-word-text")].map((e) => e.textContent.trim())`);
  check(words.length === 24, `[${i.tag}] the new kit has 24 words (${words.length})`);
  keepKit(owner, words);
  await i.clickEl(TD("button", "I saved my kit"), "the I saved my kit button");
  await i.waitFor(Q(".kit-confirm-fields"), "the word check");
  const labels = await i.evaluate(`[...document.querySelectorAll(".kit-confirm-field label")].map((l) => norm(l.textContent))`);
  for (const label of labels) {
    const position = Number(/\d+/.exec(label)?.[0] ?? 0);
    await i.fill(L(label, true), `the field "${label}"`, words[position - 1]);
  }
  await i.clickEl(TD("button", "Check"), "the Check button");
  await i.clickEl(TD("button", "Done"), "the Done button", { ms: 60000 });
  await i.waitUntil(`!document.querySelector('[role="dialog"], [role="alertdialog"]')`, "the kit dialog closed", 30000);
}

/** Polls a predicate in the background; resolves with the time it first held, or null after `ms`. */
function watchUntil(predicate, ms, every = 200) {
  const end = Date.now() + ms;
  return (async () => {
    while (Date.now() < end) {
      try {
        if (await predicate()) return Date.now();
      } catch {
        // not yet
      }
      await wait(every);
    }
    return null;
  })();
}

/** The device ids a server log line may name: the short form of an id. */
const requestsOf = (deviceId, since = 0) => serverRequests().filter((r) => isDevice(r.device, deviceId) && r.at >= since);

async function phaseRemoval() {
  const A = await inst("a");
  const B = await inst("b");
  const C = await inst("c");
  const { spaceId, devices } = readState();
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  await openPlan(C, spaceId);
  for (const i of [A, B, C]) await installProbe(i);
  await installCursorProbe(C);
  await waitAvatars(A, "A's page shows B and C before the removal", "page", [PEOPLE.b, PEOPLE.c], 60000);
  await waitAvatars(B, "B's page shows A and C before the removal", "page", [PEOPLE.a, PEOPLE.c], 60000);
  await selectPhrase(C, SEL4);
  await A.capture("removal-before-a");

  // ---- A removes C: C's avatar goes from A and B at once ------------------------------------------------------------------------------------
  await inSharedSpace(A, spaceId);
  await openMembers(A);
  await A.clickEl(`byText("button", "Remove", true, ${rowOf(PEOPLE.c)})`, "C's Remove button");
  await A.waitFor(T("h2", `Remove ${PEOPLE.c}?`), "the removal question");
  const tRemove = Date.now();
  await A.clickEl(TD("button", `Remove ${PEOPLE.c}`), "the Remove button of the question");
  const goneA = await waitAvatars(A, "C's avatar is gone from A's page header", "page", [PEOPLE.b], 30000);
  const goneB = await waitAvatars(B, "C's avatar is gone from B's page header", "page", [PEOPLE.a], 30000);
  const goneSpaceA = await waitAvatars(A, "C is gone from A's space switcher", "space", [PEOPLE.b], 30000);
  const goneSpaceB = await waitAvatars(B, "C is gone from B's space switcher", "space", [PEOPLE.a], 30000);
  const t = { a: Date.now() - tRemove };
  info(`A removed C: the avatar was gone from A's header after ${goneA.ms} ms, from B's after ${goneB.ms} ms (B read the removal from the feed); the space switchers after ${goneSpaceA.ms} and ${goneSpaceB.ms} ms (clock from the click: ${t.a} ms in all)`);
  check(Date.now() - tRemove < 20000, `C's presence was gone from A and from B within 20 s of the removal (${round((Date.now() - tRemove) / 1000)} s)`);
  check(!(await presenceList(A, spaceId)).some((d) => d.name === PEOPLE.c), "[A] PresenceList has no device of C");
  check(!(await presenceList(B, spaceId)).some((d) => d.name === PEOPLE.c), "[B] PresenceList has no device of C");
  check(ofMember(await cursorsOf(A), "c").carets.length === 0 && ofMember(await cursorsOf(B), "c").carets.length === 0, "[A] [B] no caret of C is drawn");
  await waitMembers(A, "the member list no longer has C", (r) => r.length === 2 && !r.some((m) => m.name === PEOPLE.c), 90000, [A]);
  await closeDialog(A, "Close");
  await A.capture("removal-after-a");

  // C stops receiving entries: A writes, B gets it through the socket, C never does (it learns of its removal within a few cycles).
  const late = await typeToken(A, B, "zqa90x");
  check(late.ms !== null && late.ms < 3000, `[B] still gets A's text through the socket after the removal (${late.ms} ms, within 3 s: the Rekey of the removal is being pushed and pulled at the same time): the other members' sockets are untouched`);
  await eventually(async () => (await sharedSpaceOf(C))?.shareState === "Archived", "[C] C's space became an archive (it learned of the removal)", { ms: 240000, kick: [A, C] });
  const spaceC = await sharedSpaceOf(C);
  check(spaceC.archiveReason === "Removed", `[C] the archive's reason is Removed (${spaceC.archiveReason})`);
  const second = await typeToken(A, B, "zqa91x");
  check(second.ms !== null, "[B] a second token after C became an archive reached B");
  await wait(8000);
  const planC = await pageIdByTitle(C, spaceId, PAGES.plan);
  const textC = await pageText(C, planC);
  check(!textC.includes("zqa90x") && !textC.includes("zqa91x"), "[C] C's copy never got what A wrote after the removal (no entry, inline or pulled)");
  check(!(await editorText(C)).includes("zqa90x"), "[C] nothing of it is in C's open page either");
  const listC = await C.core({ type: "PresenceList", data: { spaceId } });
  check(listC.ok === false || (listC.value?.data ?? []).length === 0, `[C] C lists nobody present in an archive (${listC.ok ? (listC.value?.data ?? []).length + " devices" : listC.error?.code})`);
  check((await pageAvatars(C)).length === 0, "[C] C's page header shows no avatar");
  // A frame C's page forces by the shell's own command goes nowhere: nothing of C ever shows again.
  const forced = await C.evaluate(`(async () => { try { await window.__TAURI_INTERNALS__.invoke("presence_send", { spaceId: ${JSON.stringify(spaceId)}, pageId: ${JSON.stringify(planC)}, state: "Editing", update: [1, 2, 3] }); return "sent"; } catch (e) { return "refused: " + (e?.code ?? e?.message ?? e); } })()`);
  info(`[C] a forced presence_send from the archive answered: ${forced}`);
  await wait(3000);
  check(!(await presenceList(A, spaceId)).some((d) => d.name === PEOPLE.c) && !(await presenceList(B, spaceId)).some((d) => d.name === PEOPLE.c), "[A] [B] a frame forced from C's page after the removal shows nothing");
  await C.capture("removal-c-archive");

  // ---- a second device of B's account: paired, shown on the page, revoked ---------------------------------------------------------------------------
  await launchInstance("d", { fresh: true });
  const D = await inst("d");
  await D.clickEl(T("button", "Add this device to my account"), `D's "Add this device to my account" button`, { ms: 90000 });
  await D.fill(L("Sync server address"), "D's server address field", SERVER_URL);
  await D.clickEl(T("button", "Connect"), "D's Connect button");
  await D.waitFor(Q(".pair-code"), "D's pairing code", 90000);
  const pairingCode = await D.evaluate(`document.querySelector(".pair-code").textContent.replace(/\\s/g, "")`);
  check(/^[0-9A-Z]{12}$/.test(pairingCode), "[D] the pairing code has 12 characters");
  addNeedles({ pairingCode });
  await goSettings(B);
  await B.clickEl(T("button", "Add a device..."), `B's "Add a device..." button`);
  await B.fill(L("Code shown on the new device", true), "the code field", pairingCode);
  await B.clickEl(TD("button", "Continue"), "B's Continue button");
  let digitsB = null;
  let digitsD = null;
  await eventually(async () => {
    digitsB = await digitsOf(B);
    digitsD = await digitsOf(D);
    return digitsB !== null && digitsD !== null;
  }, "both windows show the six digits of the pairing", { ms: 120000 });
  check(/^\d{6}$/.test(digitsB ?? "") && digitsB === digitsD, "the six digits of B and of D are the same (device pairing on v2)");
  await D.clickEl(TD("button", "Confirm"), "D's Confirm button");
  await B.waitUntil(`${DIALOG_TEXT}.includes("You are adding")`, "B's dialog names the new device", 90000);
  await B.clickEl(TD("button", "Confirm"), "B's Confirm button");
  await B.waitUntil(`${DIALOG_TEXT}.includes("was added to your account")`, "B's dialog says the device was added", 180000);
  await closeDialog(B, "Done");
  await D.waitFor(Q(".tab-strip"), "D's workspace", 240000);
  await markDocument(D);
  check((await heading(D)) === WORKSPACES.b, `[D] D has B's workspace (${await heading(D)})`);
  await eventually(async () => (await sharedSpaceOf(D)) !== undefined, "[D] D has the shared space", { ms: 300000, kick: [B, D] });
  const sharedD = await sharedSpaceOf(D);
  check(sharedD.shareState === "Shared" && sharedD.role === "editor" && sharedD.id === spaceId, `[D] D has the shared space as B's account does (${sharedD.shareState}, ${sharedD.role})`);
  const deviceD = (await D.ask({ type: "DeviceInfo" })).deviceId;
  writeState({ devices: { ...readState().devices, d: deviceD } });
  await D.ask({ type: "DeviceRename", data: { name: SECOND_DEVICE } });
  await installProbe(D);
  await installCursorProbe(D);
  await openPlan(B, spaceId);
  await openPlan(D, spaceId);
  await installCursorProbe(B);
  // One member with two devices is one avatar and two carets.
  await waitAvatars(A, "A's page header shows B once, whatever the number of B's devices", "page", [PEOPLE.b], 90000);
  await eventually(async () => devicesOf(await presenceList(A, spaceId), PEOPLE.b).length === 2, "[A] PresenceList holds two devices of B", { ms: 90000 });
  check(true, "[A] one avatar for B and two devices of B in the list");
  await selectPhrase(B, SEL3);
  await selectPhrase(D, SEL1);
  const both = await waitCursors(A, "A draws the carets of both of B's devices", (r) => r.carets.filter((c) => c.name === PEOPLE.b).length === 2 && ofMember(r, "b").selections.length >= 2, 30000);
  check(sameColour(both.last.carets[0].caret, COLOURS.b) && sameColour(both.last.carets[1].caret, COLOURS.b), "[A] both carets carry B's name and B's colour");
  check(ofMember(await cursorsOf(B), "a").carets.length <= 1 && ofMember(await cursorsOf(B), "b").carets.length === 0, "[B] B is not shown to itself: no caret of its own account's other device");
  check(ofMember(await cursorsOf(D), "b").carets.length === 0, "[D] D does not draw B's cursor (same account)");
  await A.capture("removal-two-devices-a");
  await D.capture("removal-d");

  // Revoke D from B's Settings -> Devices (the new kit is issued and checked). A watches its list of who is where, and the carets it draws
  // for B's two devices, from the moment of the click.
  const socketsClosedBefore = serverOtherLines(/realtime connection closed/).length;
  await goSettings(B);
  await B.clickEl(Q(`button[aria-label="Remove ${SECOND_DEVICE}"]`), `the Remove button of "${SECOND_DEVICE}"`, { ms: 60000 });
  await B.waitFor(T("h2", `Remove ${SECOND_DEVICE}?`), "the removal question");
  const watchList = watchUntil(async () => !(await presenceList(A, spaceId)).some((d) => d.deviceId === deviceD), 120000);
  const watchCaret = watchUntil(async () => (await cursorsOf(A)).carets.filter((c) => c.name === PEOPLE.b).length === 0, 120000);
  const tRevoke = Date.now();
  await B.clickEl(TD("button", "Remove device"), "the Remove device button");
  await finishKitFlow(B, "b2");
  info(`D was revoked and the new kit was issued ${round((Date.now() - tRevoke) / 1000)} s after the click`);
  // Its socket is closed: the server logs the end of the connection a moment after the revocation.
  await eventually(async () => serverOtherLines(/realtime connection closed/).length > socketsClosedBefore, "the server closed a realtime connection (D's)", { ms: 60000 });
  const closedAt = serverOtherLines(/realtime connection closed/).slice(socketsClosedBefore)[0].at;
  info(`the server closed D's realtime connection ${round((closedAt - tRevoke) / 1000)} s after the revocation click`);
  check(closedAt - tRevoke < 15000, `D's realtime socket was closed by the server ${round((closedAt - tRevoke) / 1000)} s after the revocation`);
  // D's presence goes from A: the entry of its device, and its caret (B itself left the page for Settings, so its own caret went too).
  const listGoneAt = await watchList;
  const caretGoneAt = await watchCaret;
  check(listGoneAt !== null, "[A] D's device is no longer in A's list of who is where");
  check(caretGoneAt !== null, "[A] D's caret is no longer drawn on A's page");
  if (listGoneAt !== null) info(`D's entry was gone from A's list ${round((listGoneAt - tRevoke) / 1000)} s after the revocation click${caretGoneAt === null ? "" : `, its caret ${round((caretGoneAt - tRevoke) / 1000)} s`}`);
  check(listGoneAt !== null && listGoneAt - tRevoke < (PRESENCE_TIMEOUT_S + 10) * 1000, `D's presence disappeared from A within the ${PRESENCE_TIMEOUT_S} s timeout of the revocation (${listGoneAt === null ? "never" : round((listGoneAt - tRevoke) / 1000) + " s"})`);
  if (listGoneAt !== null && listGoneAt - tRevoke > 10000) {
    info("OBSERVATION: the entry of a revoked device stays in the others' list until its timeout (nothing prunes the presence table when a device log revokes a device, only a removal or an archive does)");
  }
  // B is back on the page: one avatar, one caret of B, and nothing of D.
  await openPlan(B, spaceId);
  await selectPhrase(B, SEL3);
  const backB = await waitCursors(A, "A draws B's own device's caret again", (r) => r.carets.filter((c) => c.name === PEOPLE.b).length === 1, 30000);
  check(sameColour(backB.last.carets.find((c) => c.name === PEOPLE.b).caret, COLOURS.b), "[A] one caret of B is drawn: its own device");
  await waitAvatars(A, "A's page header still shows B (its other device stays)", "page", [PEOPLE.b], 30000);
  // D learns it was removed, and cannot reconnect: no request of D's device is answered after the revocation, even when it is asked to sync.
  await eventually(async () => /removed from the account/i.test(await bodyText(D)), "[D] D shows that it was removed from the account", { ms: 240000, kick: [D] });
  await D.capture("removal-d-removed");
  const afterRevoke = requestsOf(deviceD, tRevoke + 3000);
  const answered = afterRevoke.filter((r) => r.status >= 200 && r.status < 300);
  info(`after the revocation D's device made ${afterRevoke.length} request(s) to the server, ${answered.length} answered 2xx, statuses ${[...new Set(afterRevoke.map((r) => r.status))].join(",")}`);
  check(answered.length === 0, `D's device got no successful answer from the server after the revocation (${answered.length})`);
  await syncNow(D);
  await wait(15000);
  const later = requestsOf(deviceD, tRevoke + 3000).filter((r) => r.status >= 200 && r.status < 300);
  const refused = requestsOf(deviceD, tRevoke + 3000).filter((r) => r.status === 401 || r.status === 403);
  info(`D was asked to sync: it made ${requestsOf(deviceD, tRevoke + 3000).length} request(s), ${refused.length} refused (401/403), ${later.length} answered 2xx`);
  check(later.length === 0, "D's device cannot reconnect: even a forced sync gets no successful answer (no socket, no pull, no push)");
  check(!serverRequests().some((r) => isSocket(r) && isDevice(r.device, deviceD) && r.status === 101 && r.at > tRevoke), "the server upgraded no new socket for D's device after the revocation");
  // The others keep their sockets.
  await wait(5000); // the new keys of the account are being pulled and pushed
  const after = await typeToken(A, B, "zqa92x");
  check(after.ms !== null && after.ms < 3000, `[B] the other sockets were untouched: A's token reached B in ${after.ms} ms`);
  result.details = { removalAvatarGoneMs: { a: goneA.ms, b: goneB.ms }, revokeToListGoneS: listGoneAt === null ? null : round((listGoneAt - tRevoke) / 1000), revokeToSocketClosedS: round((closedAt - tRevoke) / 1000) };
}



// ---------------------------------------------------------------------------------------------------------------
// phase: reconnect (the server goes away and comes back while A and B have the page open)
// ---------------------------------------------------------------------------------------------------------------

async function phaseReconnect() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId, devices } = readState();
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  for (const i of [A, B]) await installProbe(i);
  await waitAvatars(A, "A's page shows B before the server stops", "page", [PEOPLE.b], 60000);
  await waitAvatars(B, "B's page shows A before the server stops", "page", [PEOPLE.a], 60000);
  const opened = { a: appLogLines("a", /realtime socket open/).length, b: appLogLines("b", /realtime socket open/).length };
  const warm = await typeToken(A, B, "zqa60x");
  check(warm.ms !== null && warm.ms < LATENCY_LIMIT_MS, `before the stop A's token reaches B through the socket (${warm.ms} ms)`);

  // ---- the server stops; both write meanwhile (nothing can arrive) --------------------------------------------------------------------------
  await serverCtl("stop");
  copyFileSync(join(OUT, "server.log"), join(OUT, "m13-presence-server-before-reconnect.log"));
  const tStop = Date.now();
  check((await fetch(`${SERVER_URL}/v2/server`).then(() => true).catch(() => false)) === false, "the server is down");
  await wait(6000);
  await A.evaluate(`document.querySelector(".ProseMirror").editor.commands.focus("end")`);
  await A.insertText(" zqa71x");
  await B.evaluate(`document.querySelector(".ProseMirror").editor.commands.focus("end")`);
  await B.insertText(" zqb71x");
  await wait(12000);
  check((await tokenCounts(B, ["zqa71x"])).zqa71x === 0 && (await tokenCounts(A, ["zqb71x"])).zqb71x === 0, "with the server down neither token reached the other window");
  const pending = { a: ((await syncStatus(A)).feeds ?? []).find((f) => f.spaceId === spaceId)?.pendingEntries ?? 0, b: ((await syncStatus(B)).feeds ?? []).find((f) => f.spaceId === spaceId)?.pendingEntries ?? 0 };
  info(`with the server down: A holds ${pending.a} entries to send, B ${pending.b}; sync states ${(await syncStatus(A)).state} and ${(await syncStatus(B)).state}`);
  await A.capture("reconnect-down-a");

  // ---- the server comes back with its data -------------------------------------------------------------------------------------------------
  await serverCtl("start");
  const tStart = Date.now();
  const epoch = await fetch(`${SERVER_URL}/v2/server`).then((r) => r.json());
  check(typeof epoch.serverEpoch === "string", "the server is back with its data");
  await eventually(
    async () => {
      const counts = await tokenCounts(B, ["zqa71x"]);
      const counts2 = await tokenCounts(A, ["zqb71x"]);
      return counts.zqa71x === 1 && counts2.zqb71x === 1;
    },
    "the edits made while the server was away arrived in both windows",
    { ms: 240000 },
  );
  const tArrive = Date.now();
  info(`both edits arrived ${round((tArrive - tStart) / 1000)} s after the server came back`);
  // Both reconnected and subscribed: a socket of each on the server (101), and the clients' own logs say so.
  const sockets = serverRequests().filter((r) => isSocket(r) && r.at >= tStart - 2000);
  const socketOf = (deviceId) => sockets.filter((r) => isDevice(r.device, deviceId) && r.status === 101);
  info(`the server's log after the restart: ${sockets.length} socket request(s) (${socketOf(devices.a).length} of A, ${socketOf(devices.b).length} of B answered 101)`);
  await eventually(async () => socketOf(devices.a).length >= 1 && socketOf(devices.b).length >= 1, "both devices opened a new realtime socket", { ms: 120000 });
  check(appLogLines("a", /realtime socket open/).length > opened.a && appLogLines("b", /realtime socket open/).length > opened.b, "[A] [B] each client logged a new realtime socket after the server came back");
  // Presence is back, and so is the inline path: a token typed now arrives in under a second, without a pull between.
  await waitAvatars(A, "A's page shows B again", "page", [PEOPLE.b], 120000);
  await waitAvatars(B, "B's page shows A again", "page", [PEOPLE.a], 120000);
  const tAvatar = Date.now();
  info(`avatars were back ${round((tAvatar - tStart) / 1000)} s after the server came back`);
  // The first tokens right after the reconnect (both drivers are still catching up with cycles of their own), then, once it has settled, the
  // steady state: under a second both ways, through the socket.
  const early = [];
  early.push(await typeToken(A, B, "zqa72x"));
  await wait(1500);
  early.push(await typeToken(B, A, "zqb72x"));
  await wait(1500);
  check(early.every((x) => x.ms !== null && x.ms < 5000), `right after the reconnect the first tokens arrive within 5 s (${early.map((x) => x.ms).join(", ")} ms): the sockets resubscribed to the feed and no minute-long pull is waited for`);
  await wait(10000);
  const after = [];
  for (let k = 3; k <= 5; k++) {
    after.push(await typeToken(A, B, `zqa7${k}x`));
    await wait(1500);
    after.push(await typeToken(B, A, `zqb7${k}x`));
    await wait(1500);
  }
  check(after.every((x) => x.ms !== null && x.ms < LATENCY_LIMIT_MS), `once settled every token arrives in under a second both ways (${after.map((x) => x.ms).join(", ")} ms)`);
  const pullsOfB = serverRequests().filter((r) => isPull(r) && r.at >= tAvatar + 12000 && isDevice(r.device, devices.b));
  info(`${pullCycles(pullsOfB).length} pull cycle(s) of B while the settled tokens were typed (they came through the socket)`);
  await selectPhrase(A, SEL1);
  await waitCursors(B, "B draws A's selection after the reconnect", (r) => ofMember(r, "a").text === SEL1, 30000);
  // No duplicates: every token once in both pages, the same text, the same text in the cores.
  const tokens = ["zqa60x", "zqa71x", "zqb71x", ...early.map((x) => x.token), ...after.map((x) => x.token)];
  const cA = await tokenCounts(A, tokens);
  const cB = await tokenCounts(B, tokens);
  check(tokens.every((t) => cA[t] === 1 && cB[t] === 1), `every token typed before, during and after the outage is in both pages exactly once (${tokens.map((t) => `${cA[t]}${cB[t]}`).join(" ")})`);
  check((await editorText(A)) === (await editorText(B)), "A's and B's pages have the same text");
  await eventually(
    async () => {
      const tA = await pageText(A, await pageIdByTitle(A, spaceId, PAGES.plan));
      const tB = await pageText(B, await pageIdByTitle(B, spaceId, PAGES.plan));
      return tA === tB && tokens.every((t) => tA.split(t).length === 2);
    },
    "the cores of A and of B hold the same text, every token once",
    { ms: 60000, kick: [A, B] },
  );
  const listA = await presenceList(A, spaceId);
  check(listA.length === 1 && listA[0].deviceId === devices.b, `[A] PresenceList holds B's one device, once (${listA.length})`);
  const listB = await presenceList(B, spaceId);
  check(listB.length === 1 && listB[0].deviceId === devices.a, `[B] PresenceList holds A's one device, once (${listB.length})`);
  result.details = { arrivedAfterRestartS: round((tArrive - tStart) / 1000), avatarsBackS: round((tAvatar - tStart) / 1000), latencyEarlyMs: early.map((x) => x.ms), latencySettledMs: after.map((x) => x.ms) };
}

// ---------------------------------------------------------------------------------------------------------------
// phase: restart (A is killed, then every instance is started again)
// ---------------------------------------------------------------------------------------------------------------

/** Kills an instance's process now (not through a polite exit), keeps the DevTools connection list tidy; returns when the kill was sent. */
async function killInstance(name) {
  const rec = readState().instances?.[name] ?? {};
  const at = Date.now();
  if (typeof rec.pid === "number") await helper("kill", { pids: [rec.pid] }).catch(() => {});
  patchInstance(name, { pid: null });
  if (insts[name] !== undefined) {
    insts[name].close();
    delete insts[name];
  }
  return at;
}

/** Starts an instance again on its data and waits for its workspace (a removed device shows the removed screen instead). */
async function bringBack(name, removed = false) {
  await launchInstance(name);
  const i = await inst(name);
  if (removed) await i.waitUntil(`/removed from the account/i.test(document.body.innerText)`, "the removed screen", 120000);
  else await i.waitFor(Q(".tab-strip"), "the tab strip", 120000);
  await markDocument(i);
  return i;
}

async function phaseRestart() {
  let A = await inst("a");
  let B = await inst("b");
  const { spaceId, devices } = readState();
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  await installProbe(A);
  await installProbe(B);
  await installCursorProbe(B);
  await selectPhrase(A, SEL1);
  await waitCursors(B, "B draws A's selection before A is killed", (r) => ofMember(r, "a").text === SEL1, 30000);
  await waitAvatars(B, "B's page shows A before A is killed", "page", [PEOPLE.a], 60000);
  const lastCounter = (await presenceList(B, spaceId)).find((d) => d.deviceId === devices.a);
  check(lastCounter !== undefined, "[B] A's device is in B's list before the kill");

  // ---- A dies without a word: its avatar and caret time out on B ----------------------------------------------------------------------------------
  const tKill = await killInstance("a");
  info(`A was killed at ${new Date(tKill).toISOString()}`);
  const timedOut = await waitAvatars(B, "A's avatar timed out on B's page header", "page", [], (PRESENCE_TIMEOUT_S + 20) * 1000);
  const timedCaret = await waitCursors(B, "A's caret and selection timed out on B's page", (r) => ofMember(r, "a").carets.length === 0, 20000);
  const goneAfter = (Date.now() - tKill) / 1000;
  info(`A's avatar timed out ${round(goneAfter)} s after the kill (the timeout is ${PRESENCE_TIMEOUT_S} s; B checks every ~5 s); header ${timedOut.ms} ms into the wait, caret ${timedCaret.ms} ms`);
  check(goneAfter >= 5 && goneAfter <= PRESENCE_TIMEOUT_S + 15, `a killed device's presence timed out after ${round(goneAfter)} s (within ${PRESENCE_TIMEOUT_S} s plus the expiry tick)`);
  check(!(await presenceList(B, spaceId)).some((d) => d.deviceId === devices.a), "[B] A's device is no longer in B's list");
  await waitAvatars(B, "B's space switcher no longer shows A", "space", [], 30000);
  check(true, "[B] nothing of A is left on B: no avatar (page or space), no caret, no entry in the list");
  await B.capture("restart-a-timed-out-b");

  // ---- A alone is started again: B still remembers its old counter, and accepts the new frames (the counter is seeded from the clock) -----------
  const tBack = Date.now();
  A = await bringBack("a");
  await openPlan(A, spaceId);
  await installProbe(A);
  await waitAvatars(B, "B's page shows A again after A restarted alone", "page", [PEOPLE.a], 120000);
  await selectPhrase(A, SEL2);
  await waitCursors(B, "B draws A's selection again after A restarted alone", (r) => ofMember(r, "a").text === SEL2, 30000);
  check(true, `[B] accepts the frames of the restarted device (its counter is seeded from the clock): avatar and selection are back ${round((Date.now() - tBack) / 1000)} s after the start`);
  const aliveAfterRestart = await typeToken(A, B, "zqa81x");
  check(aliveAfterRestart.ms !== null && aliveAfterRestart.ms < LATENCY_LIMIT_MS, `[B] a token typed in the restarted A reaches B in ${aliveAfterRestart.ms} ms`);

  // ---- every instance is started again --------------------------------------------------------------------------------------------------------
  for (const name of ["a", "b", "c", "d"]) await killInstance(name);
  await wait(3000);
  const tAll = Date.now();
  const back = {};
  for (const name of ["a", "b", "c", "d"]) back[name] = await bringBack(name, name === "d");
  A = back.a;
  B = back.b;
  const C = back.c;
  const D = back.d;
  info(`every instance was started again (${round((Date.now() - tAll) / 1000)} s)`);
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  await installProbe(A);
  await installProbe(B);
  await installCursorProbe(A);
  await installCursorProbe(B);
  const tReady = Date.now();
  await waitAvatars(A, "A's page shows B after every instance was started again", "page", [PEOPLE.b], 150000);
  await waitAvatars(B, "B's page shows A after every instance was started again", "page", [PEOPLE.a], 150000);
  await waitAvatars(A, "A's space switcher shows B", "space", [PEOPLE.b], 60000);
  await waitAvatars(B, "B's space switcher shows A", "space", [PEOPLE.a], 60000);
  info(`presence was back ${round((Date.now() - tReady) / 1000)} s after both pages were open`);
  await selectPhrase(A, SEL3);
  await selectPhrase(B, SEL4);
  await waitCursors(B, "B draws A's selection after the restart", (r) => ofMember(r, "a").text === SEL3, 30000);
  await waitCursors(A, "A draws B's selection after the restart", (r) => ofMember(r, "b").text === SEL4, 30000);
  check(true, "[A] [B] avatars (page and space) and selections are back after every instance was started again");
  // Right after a start both drivers are still catching up (a full cycle of each: it also serves the socket, one after the other), so the
  // first tokens may take a few seconds; once it has settled the path is the steady one.
  const early = [await typeToken(A, B, "zqa82x"), await typeToken(B, A, "zqb82x")];
  check(early.every((x) => x.ms !== null && x.ms < 8000), `right after the restart the first tokens arrive through the socket within 8 s (${early.map((x) => x.ms).join(", ")} ms)`);
  await wait(10000);
  const latency = [];
  for (let k = 3; k <= 5; k++) {
    latency.push(await typeToken(A, B, `zqa8${k}x`));
    await wait(1500);
    latency.push(await typeToken(B, A, `zqb8${k}x`));
    await wait(1500);
  }
  check(latency.every((x) => x.ms !== null && x.ms < LATENCY_LIMIT_MS), `once settled the realtime path works both ways in under a second (${latency.map((x) => x.ms).join(", ")} ms)`);
  // Nothing stale: only live devices, all heard since this start; nobody removed or revoked is listed.
  const listA = await presenceList(A, spaceId);
  const listB = await presenceList(B, spaceId);
  check(listA.length === 1 && listA[0].deviceId === devices.b && listA[0].updatedAt >= tAll, `[A] PresenceList holds B's one device, heard since the restart (${listA.length} device(s))`);
  check(listB.length === 1 && listB[0].deviceId === devices.a && listB[0].updatedAt >= tAll, `[B] PresenceList holds A's one device, heard since the restart (${listB.length} device(s))`);
  check(![...listA, ...listB].some((d) => d.name === PEOPLE.c || d.deviceId === devices.d), "no device of C (removed) or of D (revoked) is listed");
  check(sameNames(await pageAvatars(A), [PEOPLE.b]) && sameNames(await pageAvatars(B), [PEOPLE.a]), "A's and B's page avatars are one person each, and live");
  check(ofMember(await cursorsOf(A), "c").carets.length === 0 && ofMember(await cursorsOf(B), "c").carets.length === 0, "no caret of C on either page");
  // C and D are what they were: an archive, a removed device.
  check((await sharedSpaceOf(C))?.shareState === "Archived", "[C] C's copy is still an archive after the restart");
  check((await pageAvatars(C)).length === 0, "[C] it shows nobody");
  await eventually(async () => /removed from the account/i.test(await bodyText(D)), "[D] D still shows that it was removed from the account", { ms: 120000, kick: [D] });
  await wait(15000);
  const dAfter = serverRequests().filter((r) => isDevice(r.device, devices.d) && r.at >= tAll && r.status >= 200 && r.status < 300);
  check(dAfter.length === 0, `[D] the revoked device's restart made no successful request (${dAfter.length})`);
  await A.capture("restart-a");
  await B.capture("restart-b");
  void C;
  result.details = { killToGoneS: round(goneAfter), latencyEarlyMs: early.map((x) => x.ms), latencySettledMs: latency.map((x) => x.ms) };
}




// ---------------------------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------------------------
// the short pass of slices 3 and 4 (two instances, A and B; M13P_NO_C=1 for `setup`):
//   node drive-m13-presence.mjs setup      (with M13P_NO_C=1)   A shares a space and invites B by code
//   node drive-m13-presence.mjs smokecursor <outfile>           A types and moves its selection: B shows A's caret and avatar; A leaves: both go
//   node drive-m13-presence.mjs smokelink   <outfile>           A makes a view link (a deleted secret word first), a headless Edge opens it, A edits, A revokes
// The link is read from the dialog into this process's memory and goes nowhere else (no file, no report, no log).
// ---------------------------------------------------------------------------------------------------------------

async function phaseSmokeCursor() {
  const A = await inst("a");
  const B = await inst("b");
  const { spaceId, devices } = readState();
  await openPlan(A, spaceId);
  await openPlan(B, spaceId);
  for (const i of [A, B]) {
    const facts = await windowFacts(i);
    check(facts.visible === "visible" && facts.focus === true, `[${i.tag}] the window is the one in use (${facts.visible}, ${facts.focus})`);
    await installProbe(i);
    await installCursorProbe(i);
  }
  await waitPeer(A, spaceId, devices.b, "A's presence list has B's device (sockets up)");
  await waitPeer(B, spaceId, devices.a, "B's presence list has A's device");
  // B shows A's avatar on the page, with A's initials and colour.
  const avatar = await waitAvatars(B, "B's page header shows A", "page", [PEOPLE.a], 60000);
  const one = avatar.last[0];
  check(one.initials === INITIALS.a && sameColour(one.colour, COLOURS.a) && one.idle === false, `[B] A's avatar has the initials ${INITIALS.a} and A's colour (${one.initials}, ${one.colour})`);
  info(`B showed A's avatar ${avatar.ms} ms after both had the page open`);
  await B.capture("smoke-avatar-b");

  // A types (5 tokens, 1.5 s apart): the latency of each, A's own clock of its token to B's.
  const trials = [];
  for (let k = 1; k <= 5; k++) {
    trials.push(await typeToken(A, B, `zqa${String(k).padStart(2, "0")}x`));
    await wait(1500);
  }
  check(trials.every((t) => t.ms !== null), `all ${trials.length} typed tokens reached B (${trials.map((t) => t.ms).join(", ")} ms)`);
  const s = stats(trials.filter((t) => t.ms !== null).map((t) => t.ms));
  info(`A to B typing latency: first ${trials[0].ms} ms, median ${s.median} ms, max ${s.max} ms over ${s.n} tokens`);
  // A's caret, after the typing, with A's name.
  const caret = await waitCursors(B, "B draws A's caret with A's name after A typed", (r) => ofMember(r, "a").carets.length === 1, 20000);
  const caretA = ofMember(caret.last, "a").carets[0];
  check(caretA.name === PEOPLE.a && sameColour(caretA.caret, COLOURS.a), `[B] A's caret after typing carries A's name and colour (${caretA.name}, ${caretA.caret}) after ${caret.ms} ms of waiting`);
  await B.capture("smoke-caret-b");

  // A moves its selection: B draws it.
  const t0 = Date.now();
  await selectPhrase(A, SEL1);
  const tA = await selectedAt(A, SEL1);
  const sel = await waitCursors(B, `B draws A's selection "${SEL1}"`, (r) => ofMember(r, "a").text === SEL1, 20000);
  const at = await drawnAt(B, SEL1, t0);
  const drawn = at !== null && tA !== null ? at - tA : null;
  info(`A's selection was drawn on B ${drawn} ms after A made it`);
  check(drawn !== null && drawn < 2000, `A's selection is drawn on B within 2 s (${drawn} ms)`);
  check(ofMember(sel.last, "a").carets.length === 1 && ofMember(sel.last, "a").carets[0].name === PEOPLE.a, "[B] the selection comes with A's caret and name");
  await selectPhrase(A, SEL2);
  await waitCursors(B, `B's drawing follows A's selection to "${SEL2}"`, (r) => ofMember(r, "a").text === SEL2, 20000);
  check(true, "[B] A's moved selection is followed");

  // A leaves the page: avatar and caret go from B.
  const tLeave = Date.now();
  await openPlan(A, spaceId, PAGES.notes);
  await waitAvatars(B, "A's avatar is gone from B's page header", "page", [], PRESENCE_TIMEOUT_S * 1000);
  await waitCursors(B, "A's caret and selection are gone from B's page", (r) => ofMember(r, "a").carets.length === 0 && ofMember(r, "a").selections.length === 0, PRESENCE_TIMEOUT_S * 1000);
  const gone = Date.now() - tLeave;
  check(gone < PRESENCE_TIMEOUT_S * 1000, `A's avatar and caret went from B ${gone} ms after A left the page`);
  await B.capture("smoke-gone-b");
  result.details = { latencyMs: trials.map((t) => t.ms), median: s.median, avatarMs: avatar.ms, caretAfterTypingMs: caret.ms, selectionDrawnMs: drawn, leftToGoneMs: gone };
}

const EDGE_EXE = process.env.M13P_EDGE ?? "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const EDGE_PORT = BASE_PORT + 20;
const SECRET_WORD = "quincunxzebra";
const SECRET_LINE = `The passphrase is ${SECRET_WORD} today.`;
const EDIT_LINE = "A later edit that the viewer must pick up.";

/** Removes anything that looks like a link key (the fragment of a view link) from a text before it is reported. */
const scrub = (text) => String(text).replace(/#[A-Za-z0-9_-]{16,}/g, "#<key>").replace(/\/view\S*/g, "/view<...>");

/** A minimal DevTools connection to the headless Edge: `send`, the events seen, `close`. */
async function openEdge() {
  const profile = join(OUT, "m13-edge-profile");
  rmSync(profile, { recursive: true, force: true });
  const child = spawn(EDGE_EXE, ["--headless=new", `--remote-debugging-port=${EDGE_PORT}`, "--remote-allow-origins=*", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-sync", "about:blank"], { stdio: "ignore", windowsHide: true });
  writeState({ edge: { pid: child.pid, profile } });
  let target;
  for (let k = 0; k < 60 && target === undefined; k++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${EDGE_PORT}/json`)).json();
      target = list.find((t) => t.type === "page");
    } catch {
      // not up yet
    }
    if (target === undefined) await wait(500);
  }
  must(target !== undefined, "Edge (headless) answers on its DevTools port");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  const waiting = new Map();
  const events = [];
  let n = 0;
  ws.addEventListener("message", (message) => {
    const data = JSON.parse(String(message.data));
    if (data.id !== undefined) {
      const entry = waiting.get(data.id);
      waiting.delete(data.id);
      if (data.error) entry?.reject(new Error(String(data.error.message)));
      else entry?.resolve(data.result);
    } else events.push(data);
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++n;
      waiting.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  for (const domain of ["Page", "Runtime", "Log", "Network"]) await send(`${domain}.enable`);
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `window.__csp = []; document.addEventListener("securitypolicyviolation", (e) => window.__csp.push({ directive: e.violatedDirective, blocked: e.blockedURI })); ` });
  const evaluate = async (expression) => {
    const answer = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (answer.exceptionDetails) throw new Error(scrub(answer.exceptionDetails.exception?.description ?? answer.exceptionDetails.text));
    return answer.result.value;
  };
  const close = () => {
    try {
      ws.close();
    } catch {
      // closed
    }
    spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  };
  return { send, evaluate, events, close, profile };
}

/** What the viewer page shows now: title, text, whether the editor is locked, the policy violations, and whether the secret is anywhere in the DOM. */
const viewerReading = (edge) =>
  edge.evaluate(`(() => {
    const pm = document.querySelector(".ProseMirror");
    const html = document.documentElement.outerHTML;
    return { title: document.title, text: document.body.innerText, editable: pm ? pm.getAttribute("contenteditable") : null, htmlHasSecret: html.toLowerCase().includes(${JSON.stringify(SECRET_WORD)}), violations: window.__csp ?? [] };
  })()`);

/** Polls the viewer until `predicate(reading)`; returns the milliseconds it took and the reading. */
async function viewerUntil(edge, what, predicate, ms) {
  const started = Date.now();
  let last = null;
  while (Date.now() - started < ms) {
    last = await viewerReading(edge).catch(() => last);
    if (last !== null && predicate(last)) return { ms: Date.now() - started, last };
    await wait(500);
  }
  must(false, `the viewer ${what} (timed out after ${round(ms / 1000)} s; it shows "${scrub(norm((last?.text ?? "").slice(0, 120)))}")`);
  return { ms: Date.now() - started, last };
}

async function openViewLinkDialog(A) {
  await normalize(A);
  await A.clickEl(Q('button[aria-label="Page options"]'), "the page options button");
  await chooseMenuItem(A, "Share a view link");
  await A.waitFor(T("h2", "Share a view link"), "the Share a view link dialog");
}

async function phaseSmokeLink() {
  const A = await inst("a");
  const { spaceId } = readState();
  await openPlan(A, spaceId);
  // A secret word, typed and deleted before the link is made: it is in the page's history, and must not be in the viewer.
  await writeLine(A, SECRET_LINE);
  check((await editorText(A)).includes(SECRET_WORD), "[A] the secret word is in the page");
  await selectPhrase(A, SECRET_LINE);
  await A.press("Backspace");
  await wait(1500);
  await A.evaluate(`document.activeElement?.blur?.()`);
  await wait(1800);
  check(!(await editorText(A)).includes(SECRET_WORD), "[A] the secret word is deleted from the page");
  const planId = readState().pages.plan;
  check(!(await pageText(A, planId)).includes(SECRET_WORD), "[A] the core's page text no longer has the secret word");

  await openViewLinkDialog(A);
  await A.clickEl(TD("button", "Create the link"), "the Create the link button");
  await A.waitFor(Q(".invite-dialog .invite-link-box"), "the view link", 60000);
  // The URL stays in this variable.
  let url = await A.evaluate(`document.querySelector(".invite-dialog .invite-link-box").value`);
  check(typeof url === "string" && url.startsWith(`${SERVER_URL}/view#`), "[A] the dialog shows a link of the server's /view page with a key in its fragment");
  await A.waitUntil(`${DIALOG_TEXT}.includes("Live")`, "the dialog lists the link as Live", 120000).catch(() => info("the dialog did not list the link as Live within 120 s"));
  await A.capture("smoke-link-dialog-a");
  await closeDialog(A, "Close");

  const edge = await openEdge();
  try {
    await edge.send("Page.navigate", { url });
    const opened = await viewerUntil(edge, "shows the page's title and text", (r) => r.text.includes(PAGES.plan) && r.text.includes(TEXT.p1) && r.text.includes(TEXT.p3), 90000);
    url = null;
    const r = opened.last;
    check(r.text.includes(PAGES.plan) && r.text.includes(TEXT.p1) && r.text.includes(TEXT.p2) && r.text.includes(TEXT.p3), `the viewer shows the title and the three paragraphs (after ${opened.ms} ms)`);
    check(r.editable === "false", `the viewer's page is read-only (contenteditable ${r.editable})`);
    check(!r.text.toLowerCase().includes(SECRET_WORD) && r.htmlHasSecret === false, "the viewer's DOM (text and markup) holds no trace of the deleted secret word");
    const shot = await edge.send("Page.captureScreenshot", { format: "png" });
    writeFileSync(join(OUT, "m13-presence-smoke-viewer.png"), Buffer.from(shot.data, "base64"));
    result.captures.push(join(OUT, "m13-presence-smoke-viewer.png"));

    // An edit in A reaches the viewer (its poll is every 60 s, the publication follows A's edit after a debounce).
    await writeLine(A, EDIT_LINE);
    const tEdit = Date.now();
    const edited = await viewerUntil(edge, "shows A's later edit", (x) => x.text.includes(EDIT_LINE), 180000);
    info(`the viewer showed A's edit ${Date.now() - tEdit} ms after A made it`);
    check(Date.now() - tEdit < 130000, `the viewer picked up A's edit ${round((Date.now() - tEdit) / 1000)} s after A made it (limit about 2 min)`);
    check(edited.last.htmlHasSecret === false, "the refreshed page still holds no trace of the secret word");

    // Revoke in A: the viewer says so at its next poll.
    await openViewLinkDialog(A);
    await A.clickEl(Q(".invite-dialog button.invite-revoke"), "the Revoke button of the link");
    await A.clickEl(TD("button", "Revoke the link"), "the confirm button");
    // The revoked link leaves the list ("This page has no view links"); a status tag "Revoked" would also do.
    await A.waitUntil(`${DIALOG_TEXT}.includes("Revoked") || ${DIALOG_TEXT}.includes("This page has no view links")`, "the dialog no longer lists the link as Live", 60000);
    const tRevoke = Date.now();
    await closeDialog(A, "Close");
    const gone = await viewerUntil(edge, "says the link no longer works", (x) => x.text.includes("This link no longer works"), 180000);
    check(!gone.last.text.includes(PAGES.plan) && !gone.last.text.includes(EDIT_LINE), "the revoked viewer no longer shows the page");
    check(true, `the viewer showed "This link no longer works" ${round((Date.now() - tRevoke) / 1000)} s after the revoke`);

    // The console and the policy, over the whole visit.
    const violations = await edge.evaluate(`window.__csp ?? []`);
    const consoleErrors = edge.events
      .filter((e) => (e.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(e.params.type)) || (e.method === "Log.entryAdded" && ["error", "warning"].includes(e.params.entry.level)) || e.method === "Runtime.exceptionThrown")
      .map((e) => scrub(e.method === "Runtime.consoleAPICalled" ? e.params.args.map((a) => a.value ?? a.description ?? "").join(" ") : e.method === "Log.entryAdded" ? `${e.params.entry.level}:${e.params.entry.source}: ${e.params.entry.text}` : (e.params.exceptionDetails.exception?.description ?? e.params.exceptionDetails.text)).slice(0, 300));
    const cspConsole = consoleErrors.filter((t) => /content security policy|Refused to/i.test(t));
    const hosts = [...new Set(edge.events.filter((e) => e.method === "Network.requestWillBeSent" && /^https?:/.test(e.params.request.url)).map((e) => new URL(e.params.request.url).origin))];
    info(`the viewer's requests went to: ${hosts.join(", ")}`);
    check(violations.length === 0 && cspConsole.length === 0, `no content-security-policy violation in the viewer (${violations.length} events, ${cspConsole.length} console lines)`);
    info(`the viewer's console warnings and errors: ${consoleErrors.length === 0 ? "none" : consoleErrors.join(" | ")}`);
    check(consoleErrors.filter((t) => !/404|Failed to load resource/i.test(t)).length === 0, `no console error in the viewer (${consoleErrors.length}; the 404 of a revoked link is expected)`);
    check(hosts.every((h) => h === SERVER_URL), `the viewer talked to no origin but the server's (${hosts.join(", ")})`);
    result.details = { viewerOpenedMs: opened.ms, editShownMs: Date.now() - tEdit, consoleLines: consoleErrors.length };
  } finally {
    edge.close();
    await wait(1500);
    rmSync(edge.profile, { recursive: true, force: true });
    writeState({ edge: null });
  }
}

const PHASE_FUNCS = {
  smokecursor: phaseSmokeCursor,
  smokelink: phaseSmokeLink,
  setup: phaseSetup,
  realtime: phaseRealtime,
  presence: phasePresence,
  removal: phaseRemoval,
  reconnect: phaseReconnect,
  restart: phaseRestart,
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
// The report file has every note; the console has what failed (or, with M13P_VERBOSE, everything).
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
      ...(process.env.M13P_VERBOSE ? { notes: result.notes } : {}),
    },
    null,
    2,
  ),
);
for (const i of Object.values(insts)) i.close();
process.exit(result.ok ? 0 : 1);
