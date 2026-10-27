// Where the player of a video or audio block gets its bytes from (ADR-0032), behind one small
// function so that the way they arrive can be swapped, as `PdfByteSource` does for the PDF viewer.
//
// - With a transport that has a media URL (the desktop shell's `kbmedia` protocol), the player's
//   `src` is that URL: the webview streams the file itself, range by range, and the shell
//   decrypts only the chunks a range needs, whatever the size. Nothing is read here.
// - Without one (the web build, the in-memory transport, the dev page), the whole attachment is
//   read and wrapped in a blob URL, which is possible up to `MAX_PREVIEW_BYTES`; a larger file is
//   refused with `MediaTooLargeError` and the block keeps its card.
//
// A source is released when the player goes (the blob URL is revoked); a streamed one holds nothing.

import { MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import type { AttachmentId } from "@atomik-kb/core-client";

/** A URL a media element can play, and how to let go of it. */
export interface MediaSource {
  readonly url: string;
  /** `true` for the shell's streaming protocol, `false` for a blob made from the whole file. */
  readonly streamed: boolean;
  release: () => void;
}

/** What the source needs of the app. */
export interface MediaSourceDeps {
  /** The streaming URL, or `null` when this transport has none. */
  mediaUrl: (attachmentId: AttachmentId) => string | null;
  /** The whole plaintext (at most `MAX_PREVIEW_BYTES`). */
  read: (attachmentId: AttachmentId) => Promise<Uint8Array>;
}

/** The file is too large to play without the shell's streaming protocol. */
export class MediaTooLargeError extends Error {
  constructor() {
    super("the attachment is too large to play here");
    this.name = "MediaTooLargeError";
  }
}

/** The attachment as something a `<video>` or `<audio>` element can play. */
export async function openMediaSource(
  attachment: { attachmentId: AttachmentId; mime: string; size: number | bigint },
  deps: MediaSourceDeps,
): Promise<MediaSource> {
  const streaming = deps.mediaUrl(attachment.attachmentId);
  if (streaming !== null) {
    return { url: streaming, streamed: true, release: () => undefined };
  }
  if (Number(attachment.size) > MAX_PREVIEW_BYTES) {
    throw new MediaTooLargeError();
  }
  const bytes = await deps.read(attachment.attachmentId);
  // `slice` copies into a plain ArrayBuffer, which is what a Blob wants.
  const url = URL.createObjectURL(new Blob([bytes.slice()], { type: attachment.mime }));
  return {
    url,
    streamed: false,
    release: () => {
      URL.revokeObjectURL(url);
    },
  };
}
