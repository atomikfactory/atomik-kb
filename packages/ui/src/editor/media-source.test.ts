// Where a player's bytes come from (ADR-0032): the shell's streaming URL when there is one, else a blob.

import { CHUNK_SIZE, MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { stubObjectUrls } from "../test/attachments";
import { MediaTooLargeError, openMediaSource } from "./media-source";

const ID = "12345678-9abc-4def-8123-456789abcdef";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("openMediaSource", () => {
  it("uses the streaming URL as it is and reads nothing, whatever the size", async () => {
    const read = vi.fn(() => Promise.resolve(new Uint8Array()));
    const source = await openMediaSource(
      { attachmentId: ID, mime: "video/mp4", size: 5 * MAX_PREVIEW_BYTES },
      { mediaUrl: (id) => `http://kbmedia.localhost/${id}`, read },
    );
    expect(source).toMatchObject({ url: `http://kbmedia.localhost/${ID}`, streamed: true });
    expect(read).not.toHaveBeenCalled();
    expect(() => {
      source.release();
    }).not.toThrow();
  });

  it("makes a blob of the whole file where there is no streaming URL, and revokes it on release", async () => {
    const urls = stubObjectUrls();
    const read = vi.fn(() => Promise.resolve(new Uint8Array([1, 2, 3])));
    const source = await openMediaSource({ attachmentId: ID, mime: "audio/wav", size: 3 }, { mediaUrl: () => null, read });
    expect(read).toHaveBeenCalledExactlyOnceWith(ID);
    expect(source).toMatchObject({ url: "blob:test/1", streamed: false });
    const blob = urls.create.mock.calls[0]?.[0];
    expect(blob?.type).toBe("audio/wav");
    expect(blob?.size).toBe(3);
    expect(urls.revoke).not.toHaveBeenCalled();
    source.release();
    expect(urls.revoke).toHaveBeenCalledExactlyOnceWith("blob:test/1");
  });

  it("plays up to MAX_PREVIEW_BYTES as a blob and refuses beyond that without reading", async () => {
    stubObjectUrls();
    const read = vi.fn(() => Promise.resolve(new Uint8Array(CHUNK_SIZE)));
    const deps = { mediaUrl: () => null, read };
    await expect(openMediaSource({ attachmentId: ID, mime: "video/webm", size: MAX_PREVIEW_BYTES }, deps)).resolves.toMatchObject({ streamed: false });
    read.mockClear();
    await expect(openMediaSource({ attachmentId: ID, mime: "video/webm", size: MAX_PREVIEW_BYTES + 1 }, deps)).rejects.toBeInstanceOf(MediaTooLargeError);
    expect(read).not.toHaveBeenCalled();
  });

  it("passes a failed read on and leaves no blob", async () => {
    const urls = stubObjectUrls();
    const failure = new Error("chunk missing");
    await expect(
      openMediaSource({ attachmentId: ID, mime: "audio/ogg", size: 10 }, { mediaUrl: () => null, read: () => Promise.reject(failure) }),
    ).rejects.toBe(failure);
    expect(urls.create).not.toHaveBeenCalled();
  });
});
