// The in-page half of the whiteboard pass (drive-whiteboard.ps1): trusted mouse and key input and
// the page's own state, over the DevTools port of the app's WebView2 (see cdp.mjs). Phases:
//
//   node drive-whiteboard.mjs insert <outfile>   type /whiteboard in "Smoke page" and choose the entry
//   node drive-whiteboard.mjs draw <outfile>     open the editor, draw a rectangle and a text (left open)
//   node drive-whiteboard.mjs finish <outfile>   close the editor: the picture in the page shows the drawing
//   node drive-whiteboard.mjs verify <outfile>   check the picture in the page holds the drawing
//   node drive-whiteboard.mjs view <outfile>     open the editor on a drawing and close it (no change)
//   node drive-whiteboard.mjs open|locked|unlocked <outfile>   the lock pass: editor left open with a change just made; after "Lock
//                                                nothing of the drawing is in the page; after the unlock both rectangles are there
//
// Every phase writes what the page asked the network for, what failed, the console and the
// content-security-policy violations it saw, as JSON, and exits 1 when its own check fails.

import { writeFileSync } from "node:fs";

import { connect } from "./cdp.mjs";

const [phase, outFile] = process.argv.slice(2);
const TEXT = "Hello atomik";
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const page = await connect();
// CSP violations are reported to the page as events; collect them from here on.
await page.evaluate(`(() => {
  window.__cspViolations = [];
  document.addEventListener("securitypolicyviolation", (e) => {
    window.__cspViolations.push({ directive: e.violatedDirective, blocked: e.blockedURI, source: e.sourceFile });
  });
})()`);

const result = { phase, ok: true, notes: [] };
function check(condition, note) {
  result.notes.push(`${condition ? "ok" : "FAIL"}: ${note}`);
  if (!condition) {
    result.ok = false;
  }
}

/** The box of the first element matching `selector`, in page coordinates, or null. */
async function box(selector) {
  return page.evaluate(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
  })()`);
}

async function waitFor(selector, what, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const found = await box(selector);
    if (found !== null && found.w > 0 && found.h > 0) {
      return found;
    }
    await wait(250);
  }
  check(false, `${what} did not appear (${selector})`);
  return null;
}

async function clickOn(selector, what) {
  const found = await waitFor(selector, what);
  if (found === null) {
    return false;
  }
  await page.click(found.cx, found.cy);
  return true;
}

/** The text and the shapes of the inline picture of the first whiteboard. */
async function picture() {
  return page.evaluate(`(() => {
    const svg = document.querySelector(".whiteboard-block .whiteboard-svg > svg");
    if (!svg) return null;
    return { text: svg.textContent, shapes: svg.querySelectorAll("path, rect, ellipse, polygon").length, texts: svg.querySelectorAll("text").length };
  })()`);
}

async function openEditor() {
  await clickOn(".whiteboard-block .whiteboard-open", "the Edit button");
  const stage = await waitFor(".whiteboard-modal-stage .excalidraw canvas", "Excalidraw's canvas", 60000);
  await wait(1500);
  return stage;
}

async function closeEditor() {
  await clickOn(".whiteboard-modal-bar button", "the Done button");
  await wait(800);
  check((await box(".whiteboard-modal")) === null, "the editor closed");
}

try {
  if (phase === "insert") {
    // A new empty line at the end of the page, then the slash menu, typed with real key events.
    await page.evaluate(`(() => { const el = document.querySelector(".ProseMirror"); el.editor.commands.focus("end"); })()`);
    await page.key("Enter", "Enter", 13);
    await page.type("/whiteboard");
    await wait(800);
    check((await box("[role=listbox]")) !== null, "the slash menu is open");
    const label = await page.evaluate(`(() => [...document.querySelectorAll("[role=option]")].map((o) => o.textContent.trim().slice(0, 40))[0] ?? "")()`);
    check(label.startsWith("Whiteboard"), `the first entry is the whiteboard (${label})`);
    await page.key("Enter", "Enter", 13);
    await wait(1000);
    const block = await waitFor(".whiteboard-block", "the whiteboard block");
    check(block !== null, "a whiteboard block is in the page");
    const state = await page.evaluate(`(() => ({ hint: document.querySelector(".whiteboard-hint")?.textContent ?? "", mounted: !!document.querySelector(".excalidraw") }))()`);
    check(state.hint.includes("empty whiteboard"), `the empty block shows its hint (${state.hint})`);
    check(!state.mounted, "no Excalidraw is mounted for an empty drawing");
  } else if (phase === "draw") {
    const stage = await openEditor();
    check(stage !== null, "the editor shows Excalidraw's canvas");
    if (stage !== null) {
      const { x, y, w, h } = stage;
      // The rectangle tool (R), then a drag.
      await page.click(x + w / 2, y + h / 2); // focus the canvas
      await page.key("r", "KeyR", 82);
      await page.drag(x + w * 0.3, y + h * 0.3, x + w * 0.5, y + h * 0.5);
      await wait(500);
      // The text tool (T), a click, the text, Escape.
      await page.key("t", "KeyT", 84);
      await page.click(x + w * 0.55, y + h * 0.35);
      await wait(500);
      await page.type(TEXT);
      await wait(300);
      await page.key("Escape", "Escape", 27);
      await wait(1500); // longer than the 400 ms pause before the elements are written
      const drawn = await page.evaluate(`(() => ({ dialog: !!document.querySelector(".whiteboard-modal"), tools: document.querySelectorAll(".excalidraw .App-toolbar [data-testid^=toolbar-]").length }))()`);
      check(drawn.dialog, "the editor is still open");
      result.tools = drawn.tools;
    }
  } else if (phase === "finish") {
    await closeEditor();
    await wait(1500);
    const pic = await picture();
    check(pic !== null, "the page shows a picture of the drawing");
    check(pic !== null && pic.text.includes(TEXT), `the picture holds the text (${pic?.text ?? ""})`);
    check(pic !== null && pic.shapes >= 1, `the picture holds the rectangle (${pic?.shapes ?? 0} shapes)`);
    result.picture = pic;
  } else if (phase === "verify") {
    await waitFor(".whiteboard-block .whiteboard-svg > svg", "the picture of the drawing", 40000);
    const pic = await picture();
    check(pic !== null && pic.text.includes(TEXT), `the picture holds the text (${pic?.text ?? ""})`);
    check(pic !== null && pic.shapes >= 1, `the picture holds the rectangle (${pic?.shapes ?? 0} shapes)`);
    result.picture = pic;
    const stage = await openEditor();
    check(stage !== null, "the editor opens on the saved drawing");
    const inEditor = await page.evaluate(`(() => ({ canvases: document.querySelectorAll(".whiteboard-modal canvas").length }))()`);
    check(inEditor.canvases >= 1, "the editor draws the saved drawing");
    await closeEditor();
  } else if (phase === "open") {
    // The editor on the saved drawing, one more rectangle drawn, and left open (for the lock pass).
    const stage = await openEditor();
    check(stage !== null, "the editor opens");
    if (stage !== null) {
      await page.click(stage.x + stage.w * 0.5, stage.y + stage.h * 0.8);
      await page.key("r", "KeyR", 82);
      await page.drag(stage.x + stage.w * 0.6, stage.y + stage.h * 0.65, stage.x + stage.w * 0.8, stage.y + stage.h * 0.8);
    }
  } else if (phase === "locked") {
    // The app is locked: nothing of the drawing is in the page any more.
    await wait(1500);
    const left = await page.evaluate(`(() => ({
      editor: !!document.querySelector(".excalidraw, .whiteboard-modal, .whiteboard-block"),
      canvases: document.querySelectorAll("canvas").length,
      text: document.body.innerText.includes(${JSON.stringify(TEXT)}),
      svg: !!document.querySelector(".whiteboard-svg svg"),
    }))()`);
    check(!left.editor, "no whiteboard, editor or overlay is left in the page");
    check(left.canvases === 0, `no canvas is left (${left.canvases})`);
    check(!left.text, "the text of the drawing is not in the page");
    check(!left.svg, "no picture of the drawing is left");
  } else if (phase === "unlocked") {
    await waitFor(".whiteboard-block .whiteboard-svg > svg", "the picture of the drawing", 40000);
    const pic = await picture();
    check(pic !== null && pic.text.includes(TEXT), `the picture holds the text (${pic?.text ?? ""})`);
    check(pic !== null && pic.shapes >= 2, `the picture holds both rectangles (${pic?.shapes ?? 0} shapes)`);
    result.picture = pic;
  } else if (phase === "view") {
    await openEditor();
    await closeEditor();
  } else {
    throw new Error(`unknown phase ${phase}`);
  }
} catch (error) {
  check(false, `exception: ${error instanceof Error ? error.message : String(error)}`);
}

await wait(500);
const report = page.report();
const violations = await page.evaluate("window.__cspViolations ?? []").catch(() => []);
// The origins the app itself talks to: its own pages and assets, the IPC bridge, the media scheme.
const ALLOWED = /^(?:https?:\/\/(?:tauri\.localhost|ipc\.localhost|kbmedia\.localhost)|tauri:\/\/localhost|ipc:\/\/localhost|kbmedia:\/\/localhost|data:|blob:|about:)/u;
const foreign = report.requests.filter((r) => !ALLOWED.test(r.url));
result.requests = report.requests.length;
result.scripts = report.requests.filter((r) => r.type === "Script").map((r) => r.url.replace(/^https?:\/\/tauri\.localhost/u, ""));
result.fonts = report.requests.filter((r) => r.type === "Font").map((r) => r.url.replace(/^https?:\/\/tauri\.localhost/u, ""));
result.foreignRequests = foreign;
result.failures = report.failures;
result.cspViolations = violations;
result.console = report.console.filter((c) => c.type !== "log" && c.type !== "info" && c.type !== "debug");
check(foreign.length === 0, `no request to another host (${foreign.length})`);
check(violations.length === 0, `no content-security-policy violation (${violations.length})`);
check(report.failures.length === 0, `no failed request (${report.failures.length})`);
const errors = report.console.filter((c) => c.type === "error" || c.type === "exception" || c.type.startsWith("log:error"));
check(errors.length === 0, `no console error (${errors.length})`);
if (outFile) {
  writeFileSync(outFile, JSON.stringify(result, null, 2));
}
console.log(JSON.stringify({ phase, ok: result.ok, notes: result.notes }, null, 2));
page.close();
process.exit(result.ok ? 0 : 1);
