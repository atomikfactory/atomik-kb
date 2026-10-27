// A minimal Chrome DevTools Protocol client for the real-app passes (Node 22+, no dependencies).
//
// The built desktop app is started with
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333
// and this module talks to the page of its WebView2 over that port: trusted mouse and key input
// (Input.dispatch*), JavaScript evaluation, and a recording of everything the page asks the network
// for (Network.*), what the console prints (Runtime.consoleAPICalled, Log.entryAdded) and the CSP
// violations it reports (Log with source "security" and the securitypolicyviolation event).

const PORT = Number(process.env.CDP_PORT ?? 9333);

export async function connect() {
  let target;
  for (let i = 0; i < 60 && target === undefined; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      target = list.find((t) => t.type === "page" && !t.url.startsWith("devtools:"));
    } catch {
      // not up yet
    }
    if (target === undefined) {
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
  if (target === undefined) {
    throw new Error("no page target on the DevTools port");
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
    async drag(x1, y1, x2, y2, steps = 12) {
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x1, y: y1 });
      await send("Input.dispatchMouseEvent", { type: "mousePressed", x: x1, y: y1, button: "left", buttons: 1, clickCount: 1 });
      for (let i = 1; i <= steps; i++) {
        const x = x1 + ((x2 - x1) * i) / steps;
        const y = y1 + ((y2 - y1) * i) / steps;
        await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, button: "left", buttons: 1 });
        await new Promise((r) => setTimeout(r, 15));
      }
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x2, y: y2, button: "left", buttons: 0, clickCount: 1 });
    },
    /** Types text as key events (what a keyboard produces). */
    async type(text) {
      for (const ch of text) {
        await send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch, unmodifiedText: ch });
        await send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
      }
    },
    async key(key, code = key, windowsVirtualKeyCode = 0) {
      await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code, windowsVirtualKeyCode });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode });
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
