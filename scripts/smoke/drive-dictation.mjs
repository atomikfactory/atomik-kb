// The in-page half of drive-dictation.ps1: dictation (/microphone, the real kb-speech.exe sidecar, the Light engine) and
// sorting (tree menu, structure undo, /sort list) in the dev app, over the WebView2 DevTools port (cdp.mjs: trusted mouse and
// key events, DOM reads). The native consent dialog of the download is accepted by the .ps1's "message-dialog" helper
// (UI Automation / BM_CLICK). Never SendKeys.
//
//   node drive-dictation.mjs <report.json> <wav>
//
// Writes the report (every check: id, ok, detail) and out\dict-*.png; exits 1 when a check failed.

import { execFile, execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { connect } from "./cdp.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "out");
mkdirSync(out, { recursive: true });
const reportPath = process.argv[2] ?? join(out, "dict-report.json");
const ps1 = join(here, "drive-dictation.ps1");

const tag = process.env.DICT_TAG ?? ""; // appended to the page titles of a second pass in the same data directory
const report = { startedAt: new Date().toISOString(), checks: [], notes: {} };
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

/** A helper step of the .ps1 (reads a JSON argument file, writes a JSON result file). */
function helper(step, args) {
  const argFile = join(out, `dict-${step}-arg.json`);
  const resFile = join(out, `dict-${step}-res.json`);
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

/** The ids of the running kb-speech.exe processes. */
function speechPids() {
  try {
    const text = execFileSync("tasklist", ["/FI", "IMAGENAME eq kb-speech.exe", "/FO", "CSV", "/NH"], { encoding: "utf8" });
    return text.split(/\r?\n/).filter((l) => l.includes("kb-speech.exe")).map((l) => Number(l.split('","')[1]));
  } catch {
    return [];
  }
}

const cdp = await connect();

// ---- DOM helpers ------------------------------------------------------------------------------------------------------
const ev = (expression) => cdp.evaluate(expression);
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
  await sleep(250);
}
const MOD_CTRL = 2;
const MOD_SHIFT = 8;
async function press(key, code, vk, modifiers = 0, text = undefined) {
  await cdp.send("Input.dispatchKeyEvent", { type: text === undefined ? "rawKeyDown" : "keyDown", key, code, windowsVirtualKeyCode: vk, modifiers, ...(text === undefined ? {} : { text }) });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(120);
}
const enter = () => press("Enter", "Enter", 13, 0, "\r");
const tab = () => press("Tab", "Tab", 9);
const shiftTab = () => press("Tab", "Tab", 9, MOD_SHIFT);
const escape = () => press("Escape", "Escape", 27);
const ctrlZ = () => press("z", "KeyZ", 90, MOD_CTRL);
async function typeText(text) {
  await cdp.send("Input.insertText", { text });
  await sleep(150);
}

const editorSelector = '.ProseMirror[contenteditable="true"], [contenteditable="true"].ProseMirror, main [contenteditable="true"]';
const editorText = () => ev(`(document.querySelector(${JSON.stringify(editorSelector)})?.innerText ?? "").replace(/\\u200b/g, "")`);
async function focusEditorEnd() {
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(editorSelector)}); if (!e) return false; e.focus(); const s = getSelection(); s.selectAllChildren(e); s.collapseToEnd(); return true; })()`);
  await sleep(150);
}
const treeItems = () => ev(`[...document.querySelectorAll('[role="treeitem"]')].map((e) => ({ t: e.getAttribute("aria-label"), l: Number(e.getAttribute("aria-level")) }))`);
const childTitles = async (parent) => {
  await ev(`(() => { const row = [...document.querySelectorAll('[role="treeitem"]')].find((e) => e.getAttribute("aria-label") === ${JSON.stringify(parent)}); if (row?.getAttribute("aria-expanded") === "false") row.querySelector("button.tree-chevron")?.click(); })()`);
  await sleep(300);
  const rows = await treeItems();
  const i = rows.findIndex((r) => r.t === parent);
  if (i < 0) return null;
  const out_ = [];
  for (let j = i + 1; j < rows.length && rows[j].l > rows[i].l; j++) if (rows[j].l === rows[i].l + 1) out_.push(rows[j].t);
  return out_;
};

// ---- onboarding -------------------------------------------------------------------------------------------------------
async function onboard() {
  const has = await where('input[aria-label="Workspace name"], input[placeholder="Personal"]');
  if (has) {
    await cdp.click(has.x, has.y);
    await typeText("Dictation workspace");
    await clickOn("button", "Create workspace");
    await sleep(2500);
  }
  const later = await where("button, a", "Do this later");
  if (later) {
    await cdp.click(later.x, later.y);
    await sleep(2000);
  }
}

async function newPage(title) {
  await clickOn("button.tree-new");
  await sleep(900);
  if (title) {
    await clickOn("button.page-title-button");
    await sleep(200);
    await typeText(title);
    await enter();
    await sleep(500);
  }
}

// ---- A. dictation -----------------------------------------------------------------------------------------------------
async function phaseDictation() {
  await newPage(`Dictation page${tag}`);
  await focusEditorEnd();
  await typeText("/microphone");
  await sleep(500);
  const menu = await ev(`(() => { const m = document.querySelector('[role="listbox"]'); return m ? [...m.querySelectorAll('[role="option"]')].map((o) => o.textContent) : null; })()`);
  check("A0 slash /microphone ranks Microphone first", Array.isArray(menu) && /Microphone/.test(menu[0] ?? ""), menu);
  await enter();
  await sleep(1200);

  // The first-use dialog: choose Light (Moonshine), then Download -> the native consent -> progress -> dictation starts.
  const dialogUp = await where(".dictation-setup");
  const barAtOnce = dialogUp ? null : await where(".dictation-bar");
  check("A1 first-use dialog shown (or, with the model already installed, dictation starts at once)", Boolean(dialogUp) || Boolean(barAtOnce), dialogUp ? "dictation-setup" : barAtOnce ? "no dialog: the model is installed (second pass), the bar is up" : "neither");
  let installed = false;
  if (barAtOnce) installed = true;
  if (dialogUp) {
    const options = await ev(`[...document.querySelectorAll('.dictation-setup input[type=radio]')].map((r) => ({ label: r.closest('label')?.innerText.replace(/\\s+/g, ' ').slice(0, 120), checked: r.checked, disabled: r.disabled }))`);
    note("engines", options);
    await ev(`(() => { const r = [...document.querySelectorAll('.dictation-setup input[type=radio]')].find((x) => /Light/.test(x.closest('label')?.innerText ?? '')); if (r) r.click(); })()`);
    await sleep(300);
    const goLabel = await ev(`document.querySelector('.dictation-setup .button-primary')?.textContent`);
    note("goButton", goLabel);
    if (/Download/.test(goLabel ?? "")) {
      await shot("dict-1-dialog");
      await ev(`document.querySelector('.dictation-setup .button-primary').click()`);
      const dl = Date.now();
      const consent = await helper("message-dialog", { title: "Download a speech model?", press: "Download", capture: join(out, "dict-2-consent.png"), timeoutMs: 40000 });
      note("consent", { ok: consent.ok, method: consent.method, waitedMs: consent.waitedMs, texts: (consent.texts ?? []).map((s) => s.slice(0, 140)), buttons: consent.buttons });
      check("A2 native consent dialog accepted", consent.ok, `method=${consent.method} waited=${consent.waitedMs}ms`);
      // Progress until the dialog closes (done -> dictation starts).
      let last = "";
      const end = Date.now() + 240000;
      while (Date.now() < end) {
        const state = await ev(`(() => ({ dlg: !!document.querySelector('.dictation-setup'), status: document.querySelector('.dictation-setup [role=status]')?.textContent ?? '', alert: document.querySelector('.dictation-setup [role=alert]')?.textContent ?? '', bar: document.querySelector('.dictation-bar')?.innerText ?? '' }))()`);
        if (state.status !== last) {
          last = state.status;
          console.log(`  download [${((Date.now() - dl) / 1000).toFixed(1)}s] ${state.status} ${state.alert}`);
        }
        if (state.alert) break;
        if (!state.dlg) {
          installed = true;
          break;
        }
        await sleep(500);
      }
      note("downloadSeconds", ((Date.now() - dl) / 1000).toFixed(1));
      check("A3 Light model downloaded and verified (real download)", installed, last);
    } else {
      note("noDownload", "button says: " + goLabel + " (already installed or another state)");
      await ev(`document.querySelector('.dictation-setup .button-primary').click()`);
      installed = true;
    }
  }
  if (!installed) {
    return;
  }

  // Dictation runs: wait for the bar, then for phrases.
  const barSeen = await waitWhere(".dictation-bar", null, 60000).then(() => true, () => false);
  check("A4 dictation bar shown", barSeen, await ev(`document.querySelector('.dictation-bar')?.innerText?.replace(/\\s+/g, ' ')`));
  const duringPids = speechPids();
  note("kbSpeechDuring", duringPids);
  const startedAt = Date.now();
  let prev = "";
  const timeline = [];
  let base = "";
  base = await editorText();
  const endPhrases = Date.now() + 90000;
  while (Date.now() < endPhrases) {
    const text = (await editorText()).trim();
    if (text !== prev) {
      timeline.push({ s: Number(((Date.now() - startedAt) / 1000).toFixed(1)), text });
      console.log(`  text [${((Date.now() - startedAt) / 1000).toFixed(1)}s] ${text.replace(/\n/g, " / ")}`);
      prev = text;
    }
    // Three sentences in the file (it loops): stop once the third phrase has landed.
    if (timeline.length >= 3) break;
    await sleep(250);
  }
  const state = await ev(`document.querySelector('.dictation-bar')?.innerText?.replace(/\\s+/g, ' ')`);
  note("barDuring", state);
  await shot("dict-3-dictating");
  const before = (await editorText()).trim();
  note("dictatedText", before);
  note("phraseTimeline", timeline);
  const lower = before.toLowerCase().replace(/[^a-z ]/g, " ").replace(/\s+/g, " ");
  const ref = "Peden has authored three books. He was commonly referred to as the blacksmith of Ballinalee. A massive state effort was undertaken to follow up on their cases.";
  const refWords = ref.toLowerCase().replace(/[^a-z ]/g, " ").split(/\s+/).filter(Boolean);
  const got = new Set(lower.split(" "));
  const hits = refWords.filter((w) => got.has(w));
  check("A5 dictated text matches the reference (roughly)", hits.length / refWords.length >= 0.6, `${hits.length}/${refWords.length} reference words; text: ${before.replace(/\n/g, " / ")}`);
  const phraseCount = timeline.length;
  note("pageChanges", phraseCount);

  // Esc stops; the bar goes; nothing more is inserted.
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(editorSelector)}); e?.focus(); })()`);
  const stopAt = Date.now();
  await escape();
  let barGone = false;
  for (let i = 0; i < 40 && !barGone; i++) {
    barGone = !(await where(".dictation-bar"));
    if (!barGone) await sleep(250);
  }
  const barGoneMs = Date.now() - stopAt;
  const afterStop = (await editorText()).trim();
  if (afterStop !== prev) {
    timeline.push({ s: Number(((Date.now() - startedAt) / 1000).toFixed(1)), text: afterStop, atStop: true });
    console.log(`  a phrase landed while stopping: ${afterStop.slice(prev.length).trim()}`);
  }
  await sleep(6000);
  const later = (await editorText()).trim();
  check("A6 Esc stops: bar gone, nothing more inserted for 6 s", barGone && later === afterStop, `barGone=${barGone} in ${barGoneMs}ms; text same=${later === afterStop}; (text at stop ${afterStop.length} chars, 6 s later ${later.length})`);
  await shot("dict-4-stopped");

  // Ctrl+Z removes only the last phrase.
  await focusEditorEnd();
  const beforeUndo = (await editorText()).trim();
  await ctrlZ();
  await sleep(400);
  const afterUndo = (await editorText()).trim();
  const lastSeen = timeline.length >= 2 ? timeline[timeline.length - 1].text : null;
  const wantLeft = timeline.length >= 2 ? timeline[timeline.length - 2].text : null;
  const removed = beforeUndo.startsWith(afterUndo) ? beforeUndo.slice(afterUndo.length).trim() : null;
  const okUndo = wantLeft !== null && afterUndo === wantLeft && beforeUndo === lastSeen;
  check("A7 Ctrl+Z removes only the last phrase", okUndo, `removed=${JSON.stringify(removed)}; left=${JSON.stringify(afterUndo)}; expected left=${JSON.stringify(wantLeft)}`);
  await shot("dict-5-undo");

  // The shell sends Quit when the session ends (the sidecar's own idle exit is 60 s); either way the process must be gone soon.
  const exitStart = stopAt;
  let gone = speechPids().length === 0;
  let exitMs = gone ? Date.now() - exitStart : null;
  while (!gone && Date.now() - exitStart < 100000) {
    await sleep(250);
    gone = speechPids().length === 0;
    if (gone) exitMs = Date.now() - exitStart;
  }
  const errLog = readFileSync(join(out, "dict.err.log"), "utf8");
  const ended = [...errLog.matchAll(/the dictation sidecar ended reason="([^"]*)"/g)].map((m) => m[1]);
  note("sidecarEndedReasons", ended);
  check("A8 kb-speech.exe was running during the session and exits within ~70 s of the stop", duringPids.length > 0 && gone && exitMs <= 75000, `running during: pids ${JSON.stringify(duringPids)}; gone ${exitMs}ms after Esc; log reasons: ${JSON.stringify(ended.slice(-2))}`);
}

// ---- B. sorting -------------------------------------------------------------------------------------------------------
async function renameRow(current, next) {
  const p = await waitWhere('[role="treeitem"]', current);
  await cdp.click(p.x, p.y, "left", 2);
  await sleep(500);
  const editing = await where('.tree-rename input, input.tree-rename');
  if (!editing) return false;
  await ev(`(() => { const i = document.querySelector('.tree-rename input, input.tree-rename'); i.select(); })()`);
  await typeText(next);
  await enter();
  await sleep(500);
  return true;
}
async function openMenuOf(title) {
  const p = await waitWhere('[role="treeitem"]', title);
  await cdp.click(p.x, p.y, "right");
  await sleep(400);
  let open = await where('[role="menu"]');
  if (!open) {
    // The "more" button of the row opens the same menu.
    await ev(`(() => { const row = [...document.querySelectorAll('[role="treeitem"]')].find((e) => e.getAttribute('aria-label') === ${JSON.stringify(title)}); row?.querySelector('button[aria-haspopup="menu"]')?.click(); })()`);
    await sleep(400);
    open = await where('[role="menu"]');
    note("menuOpenedBy", "the row's actions button (right-click gave no menu)");
  }
  return Boolean(open);
}

async function phaseSort() {
  const parent = `Sorting${tag}`;
  await newPage(parent);
  // Four subpages, created in this order.
  const names = ["Page 10", "page 2", "Écho", "alpha"];
  for (let i = 0; i < names.length; i++) {
    const row = await waitWhere('[role="treeitem"]', parent);
    await ev(`(() => { const row = [...document.querySelectorAll('[role="treeitem"]')].find((e) => e.getAttribute('aria-label') === ${JSON.stringify(parent)}); row?.querySelector('button[aria-label="Add a page inside"]')?.click(); })()`);
    await sleep(900);
    // The new subpage opens, untitled: name it from its page title.
    await clickOn("button.page-title-button");
    await sleep(200);
    await typeText(names[i]);
    await enter();
    await sleep(500);
  }
  const created = await childTitles(parent);
  check("B0 four subpages created in order", JSON.stringify(created) === JSON.stringify(names), created);

  if (!(await openMenuOf(parent))) {
    check("B1 tree menu opens", false, "no menu");
    return;
  }
  await clickOn('[role="menuitem"]', "Sort subpages A–Z");
  await sleep(1200);
  const byTitle = await childTitles(parent);
  check("B1 Sort subpages A-Z: alpha, Écho, page 2, Page 10", JSON.stringify(byTitle) === JSON.stringify(["alpha", "Écho", "page 2", "Page 10"]), byTitle);

  await openMenuOf(parent);
  await clickOn('[role="menuitem"]', "Sort subpages by date created");
  await sleep(1200);
  const byDate = await childTitles(parent);
  check("B2 Sort by date created restores the creation order", JSON.stringify(byDate) === JSON.stringify(names), byDate);

  // Structure undo: Ctrl+Z with the focus on a tree row (outside any text field).
  await ev(`[...document.querySelectorAll('[role="treeitem"]')].find((e) => e.getAttribute('aria-label') === ${JSON.stringify(parent)})?.focus()`);
  await sleep(200);
  const focusOn = await ev(`(document.activeElement?.getAttribute('role') ?? document.activeElement?.tagName)`);
  await ctrlZ();
  await sleep(1200);
  const afterUndo = await childTitles(parent);
  check("B3 Ctrl+Z outside a text field: one undo restores the previous order (A-Z)", JSON.stringify(afterUndo) === JSON.stringify(["alpha", "Écho", "page 2", "Page 10"]), { focusOn, afterUndo });

  // /sort list in a page.
  await newPage(`Sorted list${tag}`);
  await focusEditorEnd();
  await cdp.type("- ");
  await typeText("banana");
  await enter();
  await tab();
  await typeText("banana child");
  await enter();
  await shiftTab();
  await typeText("Apple");
  await enter();
  await typeText("cherry");
  await sleep(300);
  const listBefore = await ev(`(document.querySelector(${JSON.stringify(editorSelector)})?.innerHTML ?? "").length`);
  const dom = () => ev(`(() => { const ul = document.querySelector(${JSON.stringify(editorSelector)})?.querySelector('ul'); if (!ul) return null; const walk = (list) => [...list.children].filter((c) => c.tagName === 'LI').map((li) => { const own = li.querySelector(':scope > p, :scope > div > p'); const sub = li.querySelector(':scope > ul'); return sub ? { t: (own ?? li).textContent.trim(), c: walk(sub) } : (own ?? li).textContent.trim(); }); return walk(ul); })()`);
  const listTypedStructure = await dom();
  note("listBeforeSort", listTypedStructure);
  await typeText(" /sort"); // a space first: the slash menu opens at a block start or after a space
  await sleep(500);
  const slash = await ev(`(() => { const m = document.querySelector('[role="listbox"]'); return m ? [...m.querySelectorAll('[role="option"]')].map((o) => o.textContent) : null; })()`);
  note("slashSortOptions", slash);
  await enter();
  await sleep(800);
  const sorted = await dom();
  const expect = ["Apple", { t: "banana", c: ["banana child"] }, "cherry"];
  check("B4 /sort list: Apple, banana (with its child), cherry", JSON.stringify(sorted) === JSON.stringify(expect), { listTypedStructure, sorted, slash: slash?.slice(0, 3), htmlLen: listBefore });
  await shot("dict-6-sorted");
}

try {
  const phases = (process.env.DICT_PHASES ?? "dictation,sort").split(",");
  await onboard();
  if (phases.includes("dictation")) await phaseDictation();
  if (phases.includes("sort")) await phaseSort();
} catch (error) {
  check("script", false, String(error?.stack ?? error).slice(0, 600));
  await shot("dict-error");
}

const net = cdp.report();
const consoleErrors = net.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
note("consoleErrors", consoleErrors.slice(0, 8));
note("kbSpeechLeft", speechPids());
report.finishedAt = new Date().toISOString();
writeFileSync(reportPath, JSON.stringify(report, null, 2));
cdp.close();
const failed = report.checks.filter((c) => !c.ok);
console.log(`${report.checks.length - failed.length}/${report.checks.length} checks passed; wall ${stamp()}`);
process.exit(failed.length === 0 ? 0 : 1);
