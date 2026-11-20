// The dev harness (`pnpm dev`): a fake `window.kbHost` that answers the editor's calls from core-client's `MemoryTransport`, so the
// editor can be tried in a desktop browser at a phone viewport (390x844). It speaks the same messages the Android app does
// (apps/android/ARCHITECTURE.md, Contract 2) and sends `init` once the editor says `ready`.
//
// Query parameters: `?theme=dark|light`, `?focus=title|end`, `?readonly=1`, `?keyboard=<px>` (a pretend soft keyboard).
// The page's replies to the app are logged to the console (`[kbHost] ...`).

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { base64ToBytes, bytesToBase64 } from "@atomik-kb/core-client/android";
import type { KbHost } from "@atomik-kb/core-client/android";

const uuid = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

function text(value: string, ...marks: string[]): JsonValue {
  return { type: "text", text: value, ...(marks.length > 0 ? { marks: marks.map((type) => ({ type })) } : {}) };
}

function block(n: number, type: string, content: JsonValue[], attrs: Record<string, JsonValue> = {}): JsonValue {
  return { type, attrs: { id: uuid(n), ...attrs }, content };
}

const SAMPLE: JsonValue = {
  type: "doc",
  content: [
    block(1, "heading", [text("Field notes")], { level: 2 }),
    block(2, "paragraph", [
      text("The editor of the phone app: "),
      text("bold", "bold"),
      text(", "),
      text("italic", "italic"),
      text(" and plain text, with a toolbar above the keyboard."),
    ]),
    {
      type: "bulletList",
      attrs: { id: uuid(3) },
      content: [
        { type: "listItem", attrs: { id: uuid(4) }, content: [block(5, "paragraph", [text("Capture a thought")])] },
        { type: "listItem", attrs: { id: uuid(6) }, content: [block(7, "paragraph", [text("Sync it later")])] },
      ],
    },
    {
      type: "taskList",
      attrs: { id: uuid(8) },
      content: [
        {
          type: "taskItem",
          attrs: { id: uuid(9), checked: true },
          content: [block(10, "paragraph", [text("Pair the phone")])],
        },
        {
          type: "taskItem",
          attrs: { id: uuid(11), checked: false },
          content: [block(12, "paragraph", [text("Write something")])],
        },
      ],
    },
    block(13, "paragraph", []),
  ],
};

export async function installDevHost(): Promise<void> {
  const params = new URLSearchParams(location.search);
  const memory = new MemoryTransport({
    workspaceName: "Dev",
    spaces: [{ name: "Notes", pages: [{ title: "Field notes", content: SAMPLE }] }],
  });
  const client = new CoreClient(memory);
  const [space] = await client.spaceList();
  if (space === undefined) {
    throw new Error("the dev core has no space");
  }
  const tree = await client.treeGet(space.id);
  const page = tree.roots[0]?.page;
  if (page === undefined) {
    throw new Error("the dev core has no page");
  }

  const host: KbHost = {
    onmessage: null,
    postMessage(raw: string) {
      const message = JSON.parse(raw) as Record<string, unknown>;
      if (typeof message.host === "string") {
        console.info("[kbHost]", raw);
        if (message.host === "ready") {
          deliver({
            init: {
              pageId: page.id,
              spaceId: space.id,
              theme: params.get("theme") ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"),
              locale: navigator.language,
              readOnly: params.get("readonly") === "1",
              focus: params.get("focus"),
            },
          });
        }
        return;
      }
      void answer(message);
    },
  };

  function deliver(message: unknown): void {
    queueMicrotask(() => {
      host.onmessage?.({ data: JSON.stringify(message) });
    });
  }

  async function answer(message: Record<string, unknown>): Promise<void> {
    const id = message.id;
    const pageId = String(message.pageId);
    const attachmentId = String(message.attachmentId);
    const b64 = typeof message.b64 === "string" ? message.b64 : "";
    try {
      let ok: unknown = null;
      switch (message.op) {
        case "dispatch":
          ok = await memory.dispatch(message.request as Parameters<MemoryTransport["dispatch"]>[0]);
          break;
        case "docOpen":
          ok = { b64: bytesToBase64(await memory.docs.open(pageId)) };
          break;
        case "docImport":
          await memory.docs.import(pageId, base64ToBytes(b64));
          break;
        case "docExport":
          ok = { b64: bytesToBase64(await memory.docs.export(pageId, base64ToBytes(b64))) };
          break;
        case "docClose":
          await memory.docs.close(pageId);
          break;
        case "attRead":
          ok = {
            b64: bytesToBase64(
              await memory.attachments.read(
                attachmentId,
                typeof message.start === "number" && typeof message.end === "number"
                  ? { start: message.start, end: message.end }
                  : undefined,
              ),
            ),
          };
          break;
        case "attWrite":
          await memory.attachments.write(attachmentId, base64ToBytes(b64));
          break;
        default:
          throw new Error(`unknown op ${String(message.op)}`);
      }
      deliver({ id, ok });
    } catch (error) {
      const plain =
        typeof error === "object" && error !== null && "code" in error && "message" in error
          ? error
          : { code: "Internal", message: String(error) };
      deliver({ id, err: plain });
    }
  }

  memory.subscribe((event) => {
    deliver({ event });
  });
  (window as { kbHost?: KbHost }).kbHost = host;
  // Lets the console play the app: `kbDev({ insert: { text: "dictated words" } })`.
  (window as { kbDev?: (message: unknown) => void }).kbDev = deliver;
}
