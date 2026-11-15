// The transport of the Android app's editor WebView (`apps/android/ARCHITECTURE.md`, "Contract 2: WebView bridge").
// The Kotlin side registers `WebViewCompat.addWebMessageListener(webView, "kbHost", ..)`, which gives the page
// `window.kbHost`: `postMessage(string)` to the app and `onmessage` for what the app sends. Every message is a JSON
// string; bytes travel as base64 (`b64`). A call carries an `id` and is answered by `{ id, ok }` or `{ id, err }`; the
// app's `{ event }` messages are core events; every other message (`init`, `insert`, `theme`, `dictation`) is for the
// editor shell and is handed to `onHostMessage`. There is no media URL: the UI falls back to blobs made from `read`.

import { CoreError, internalError, isApiError, toCoreError } from "./errors";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { ByteRange } from "./generated/ByteRange";
import type { Event } from "./generated/Event";
import type { PageId } from "./generated/PageId";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { AttachmentSaveResult, AttachmentTransport, DocTransport, Transport } from "./transport";

/** The object `addWebMessageListener` puts on `window`. */
export interface KbHost {
  postMessage(message: string): void;
  onmessage: ((event: { data: unknown }) => void) | null;
}

/** A message from the app that is neither a reply nor a core event. */
export type HostMessage = Record<string, unknown>;

const CHUNK = 0x8000;

/** Base64 text of bytes. */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + CHUNK));
  }
  return btoa(binary);
}

/** The bytes of base64 text. */
export function base64ToBytes(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
}

function unsupported(what: string): CoreError {
  return new CoreError({ code: "InvalidInput", message: `${what} is not available in the Android editor` });
}

export class AndroidTransport implements Transport {
  readonly docs: DocTransport;
  readonly attachments: AttachmentTransport;

  readonly #host: KbHost;
  readonly #pending = new Map<number, Pending>();
  readonly #listeners = new Set<(event: Event) => void>();
  readonly #hostListeners = new Set<(message: HostMessage) => void>();
  #nextId = 1;

  constructor(host?: KbHost) {
    const resolved = host ?? (globalThis as { kbHost?: KbHost }).kbHost;
    if (resolved === undefined) {
      throw internalError("window.kbHost is missing: this page must run inside the Android app");
    }
    this.#host = resolved;
    this.#host.onmessage = (event) => {
      this.#receive(event.data);
    };

    this.docs = {
      open: async (pageId: PageId) => this.#bytes(await this.#call({ op: "docOpen", pageId })),
      import: async (pageId: PageId, update: Uint8Array) => {
        await this.#call({ op: "docImport", pageId, b64: bytesToBase64(update) });
      },
      export: async (pageId: PageId, since?: Uint8Array) =>
        this.#bytes(await this.#call({ op: "docExport", pageId, b64: bytesToBase64(since ?? new Uint8Array()) })),
      close: async (pageId: PageId) => {
        await this.#call({ op: "docClose", pageId });
      },
    };

    this.attachments = {
      write: async (attachmentId: AttachmentId, bytes: Uint8Array) => {
        await this.#call({ op: "attWrite", attachmentId, b64: bytesToBase64(bytes) });
      },
      read: async (attachmentId: AttachmentId, range?: ByteRange) =>
        this.#bytes(
          await this.#call(
            range === undefined
              ? { op: "attRead", attachmentId }
              : { op: "attRead", attachmentId, start: range.start, end: range.end },
          ),
        ),
      importPath: (): Promise<AttachmentInfo> =>
        Promise.reject(unsupported("Importing a file by path")),
      saveAs: (): Promise<AttachmentSaveResult> =>
        Promise.reject(unsupported("Saving an attachment to a file")),
      open: (): Promise<string> => Promise.reject(unsupported("Opening an attachment")),
    };
  }

  async dispatch(request: Request): Promise<Response> {
    return (await this.#call({ op: "dispatch", request })) as Response;
  }

  subscribe(onEvent: (event: Event) => void): () => void {
    this.#listeners.add(onEvent);
    return () => {
      this.#listeners.delete(onEvent);
    };
  }

  /** Opens an `http`, `https` or `mailto` link: the app does it. */
  openExternal(url: string): Promise<void> {
    this.notify({ host: "openExternal", url });
    return Promise.resolve();
  }

  /** Listens to the app's messages that are not replies or core events (`init`, `insert`, `theme`, `dictation`). */
  onHostMessage(listener: (message: HostMessage) => void): () => void {
    this.#hostListeners.add(listener);
    return () => {
      this.#hostListeners.delete(listener);
    };
  }

  /** Tells the app something (`{ host: "ready" }`, ...); no reply. */
  notify(message: HostMessage): void {
    this.#host.postMessage(JSON.stringify(message));
  }

  #call(message: Record<string, unknown>): Promise<unknown> {
    const id = this.#nextId;
    this.#nextId += 1;
    return new Promise<unknown>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      try {
        this.#host.postMessage(JSON.stringify({ id, ...message }));
      } catch (error) {
        this.#pending.delete(id);
        reject(toCoreError(error));
      }
    });
  }

  #bytes(reply: unknown): Uint8Array {
    if (isRecord(reply) && typeof reply.b64 === "string") {
      try {
        return base64ToBytes(reply.b64);
      } catch (error) {
        throw internalError("The Android app answered with invalid base64", { cause: error });
      }
    }
    throw internalError("The Android app answered without bytes");
  }

  #receive(data: unknown): void {
    let message: unknown = data;
    if (typeof data === "string") {
      try {
        message = JSON.parse(data);
      } catch {
        return;
      }
    }
    if (!isRecord(message)) {
      return;
    }
    if (typeof message.id === "number" && ("ok" in message || "err" in message)) {
      const pending = this.#pending.get(message.id);
      if (pending === undefined) {
        return;
      }
      this.#pending.delete(message.id);
      if ("err" in message && message.err !== undefined && message.err !== null) {
        pending.reject(isApiError(message.err) ? new CoreError(message.err) : toCoreError(message.err));
      } else {
        pending.resolve(message.ok ?? null);
      }
      return;
    }
    if ("event" in message) {
      const event = message.event as Event;
      for (const listener of [...this.#listeners]) {
        listener(event);
      }
      return;
    }
    for (const listener of [...this.#hostListeners]) {
      listener(message);
    }
  }
}
