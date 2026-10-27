// Where `MemoryTransport`'s `attachments.importUrl` gets a picture from (ADR-0028): a map of
// canned answers given to its constructor (`MemoryTransportOptions.remoteImages`), checked
// first, else the browser's `fetch` with the shell's checks (no credentials, no `Referer`, an
// `https` address after every redirect, at most `MAX_PREVIEW_BYTES`, 60 seconds). A web page can
// reach only servers that allow it (CORS), so against a real address this mostly answers
// `network`; the map is what tests and the in-browser dev UI use.

import { MAX_PREVIEW_BYTES } from "./attachments";
import { CoreError } from "./errors";
import { URL_MESSAGES, statusFailure, urlFailure } from "./url-images";

/** What a canned address answers: the bytes and the `Content-Type` the server would send. */
export interface RemoteImage {
  bytes: Uint8Array;
  contentType: string;
}

/** The whole download may take this long, in milliseconds (the shell's 60 seconds). */
const TOTAL_TIMEOUT_MS = 60_000;

/** `InvalidInput` for the field `size`, with `details.limit`: a picture larger than the app can show. */
export function tooLargeFailure(limit: number = MAX_PREVIEW_BYTES): CoreError {
  return new CoreError({
    code: "InvalidInput",
    message: URL_MESSAGES.tooLarge,
    details: { field: "size", limit },
  });
}

function isHttps(address: string): boolean {
  try {
    const url = new URL(address);
    return url.protocol === "https:" && url.username === "" && url.password === "";
  } catch {
    return false;
  }
}

/** The bytes of a response, never more than `MAX_PREVIEW_BYTES`. */
async function readBody(response: Response): Promise<Uint8Array> {
  const reader = response.body?.getReader();
  if (reader === undefined) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length > MAX_PREVIEW_BYTES) {
      throw tooLargeFailure();
    }
    return bytes;
  }
  const pieces: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    total += value.length;
    if (total > MAX_PREVIEW_BYTES) {
      await reader.cancel();
      throw tooLargeFailure();
    }
    pieces.push(value);
  }
  const bytes = new Uint8Array(total);
  let at = 0;
  for (const piece of pieces) {
    bytes.set(piece, at);
    at += piece.length;
  }
  return bytes;
}

/** One `GET` with the browser's `fetch`, with the shell's rules. */
async function fetchImage(url: URL): Promise<RemoteImage> {
  if (typeof fetch !== "function") {
    throw urlFailure("network", URL_MESSAGES.network);
  }
  const controller = new AbortController();
  const timer = setTimeout(() => {
    controller.abort();
  }, TOTAL_TIMEOUT_MS);
  try {
    let response: Response;
    try {
      response = await fetch(url.href, {
        headers: { Accept: "image/*" },
        credentials: "omit",
        referrerPolicy: "no-referrer",
        redirect: "follow",
        signal: controller.signal,
      });
    } catch {
      throw controller.signal.aborted
        ? urlFailure("timeout", URL_MESSAGES.timeout)
        : urlFailure("network", URL_MESSAGES.network);
    }
    if (response.redirected && !isHttps(response.url)) {
      throw urlFailure("redirect", URL_MESSAGES.redirect);
    }
    if (!response.ok) {
      throw statusFailure(response.status);
    }
    const length = Number(response.headers.get("content-length") ?? Number.NaN);
    if (Number.isFinite(length) && length > MAX_PREVIEW_BYTES) {
      throw tooLargeFailure();
    }
    let bytes: Uint8Array;
    try {
      bytes = await readBody(response);
    } catch (error) {
      if (error instanceof CoreError) {
        throw error;
      }
      throw controller.signal.aborted
        ? urlFailure("timeout", URL_MESSAGES.timeout)
        : urlFailure("network", URL_MESSAGES.network);
    }
    return { bytes, contentType: response.headers.get("content-type") ?? "" };
  } finally {
    clearTimeout(timer);
  }
}

/**
 * The picture at `url`: the canned answer for it (looked up by the address as the person typed
 * it, trimmed, or as parsed), else the browser's `fetch`. A canned answer larger than
 * `MAX_PREVIEW_BYTES` is refused like a download is.
 */
export async function loadRemoteImage(
  url: URL,
  typed: string,
  remote: ReadonlyMap<string, RemoteImage> | undefined,
): Promise<RemoteImage> {
  const canned = remote?.get(typed.trim()) ?? remote?.get(url.href);
  if (canned !== undefined) {
    if (canned.bytes.length > MAX_PREVIEW_BYTES) {
      throw tooLargeFailure();
    }
    return canned;
  }
  return fetchImage(url);
}
