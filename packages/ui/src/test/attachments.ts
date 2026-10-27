// Helpers for tests of attachment blocks: files to paste and drop, uploads made directly
// through a client (as another window would), and the object URLs jsdom lacks.

import { CoreClient, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentInfo, MemoryTransport, SpaceId } from "@atomik-kb/core-client";
import { act } from "@testing-library/react";
import { vi } from "vitest";

export const MIB = 1024 * 1024;

/** A small valid PNG. */
export function pngFile(name = "photo.png"): File {
  return new File([demoImagePng()], name, { type: "image/png" });
}

/** A text file. */
export function textFile(name = "notes.txt", text = "hello"): File {
  return new File([text], name, { type: "text/plain" });
}

/** A file of `size` zero bytes. */
export function sizedFile(name: string, size: number, type = "application/octet-stream"): File {
  return new File([new Uint8Array(size)], name, { type });
}

/** Uploads a file into a space through a client of its own (not through the UI). */
export function upload(memory: MemoryTransport, spaceId: SpaceId, file: File): Promise<AttachmentInfo> {
  return new CoreClient(memory).attachmentUpload(spaceId, file.name, file.type, file);
}

/** `URL.createObjectURL` and `URL.revokeObjectURL` (jsdom has neither) as spies: `blob:test/1`, ... */
export function stubObjectUrls() {
  let next = 0;
  const create = vi.fn<(blob: Blob) => string>(() => {
    next += 1;
    return `blob:test/${String(next)}`;
  });
  const revoke = vi.fn<(url: string) => void>();
  Object.defineProperty(URL, "createObjectURL", { configurable: true, writable: true, value: create });
  Object.defineProperty(URL, "revokeObjectURL", { configurable: true, writable: true, value: revoke });
  return { create, revoke };
}

/** What a paste or a drop of files carries (jsdom has no `DataTransfer`). */
function transferOf(files: readonly File[], text: string) {
  return {
    files,
    types: [...(text === "" ? [] : ["text/plain"]), ...(files.length === 0 ? [] : ["Files"])],
    getData: (type: string) => (type === "text/plain" ? text : ""),
    dropEffect: "none",
    effectAllowed: "all",
  };
}

/** Drops files on `target` (inside the editor); returns whether the drop was taken. */
export function dropFiles(target: Element, files: readonly File[]): boolean {
  const event = new Event("drop", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: transferOf(files, "") });
  let taken = false;
  act(() => {
    taken = !target.dispatchEvent(event);
  });
  return taken;
}

/** Pastes files (and optionally text) into `target`. */
export function pasteFiles(target: Element, files: readonly File[], text = ""): void {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: transferOf(files, text) });
  act(() => {
    target.dispatchEvent(event);
  });
}
