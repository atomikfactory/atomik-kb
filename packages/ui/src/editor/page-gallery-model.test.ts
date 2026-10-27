// The pure rules of the gallery (ADR-0054, section 6): the order of a card's preview (cover, first image, none), which covers
// this version can draw, the candidates a card falls back through, which pages may need their first image, the card size, and
// whether a picture can be drawn.

import type { AttachmentId, PageId, PageInfo } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { MAX_COVER_BYTES } from "../icons/icon-images";
import type { AttachmentLookup } from "../store-attachments";
import {
  cardSizeOf,
  GALLERY_CARD_WIDTHS,
  needsFirstImage,
  pictureStatus,
  previewCandidates,
  previewOf,
  readCardSize,
} from "./page-gallery-model";
import type { PictureFacts } from "./page-gallery-model";

const SPACE = "11111111-1111-4111-8111-111111111111";
const ID = "22222222-2222-4222-8222-222222222222";
const COVER = "a0000000-0000-4000-8000-0000000000c0";
const PICTURE = "a0000000-0000-4000-8000-0000000000c1";

function page(extra: Partial<PageInfo> = {}): PageInfo {
  return { id: ID, spaceId: SPACE, title: "Alpha", createdAt: 1, updatedAt: 1, ...extra };
}

function images(attachmentId: AttachmentId = PICTURE, pageId: PageId = ID): ReadonlyMap<PageId, AttachmentId> {
  return new Map([[pageId, attachmentId]]);
}

const NONE: ReadonlyMap<PageId, AttachmentId> = new Map();

describe("previewOf", () => {
  it("is the cover first, whatever the page's first image is", () => {
    expect(previewOf(page({ cover: `attachment:${COVER}`, coverY: 20 }), images())).toEqual({
      kind: "cover",
      cover: { kind: "image", attachmentId: COVER },
      y: 20,
    });
    expect(previewOf(page({ cover: "color:blue" }), images())).toEqual({ kind: "cover", cover: { kind: "color", color: "blue" }, y: 50 });
    expect(previewOf(page({ cover: "gradient:sunset" }), images())).toEqual({
      kind: "cover",
      cover: { kind: "gradient", name: "sunset" },
      y: 50,
    });
  });

  it("is the first image of the page when there is no cover", () => {
    expect(previewOf(page(), images())).toEqual({ kind: "image", attachmentId: PICTURE });
  });

  it("is none for a page with neither, and for a first image that belongs to another page", () => {
    expect(previewOf(page(), NONE)).toEqual({ kind: "none" });
    expect(previewOf(page(), images(PICTURE, "33333333-3333-4333-8333-333333333333"))).toEqual({ kind: "none" });
  });

  it("skips a cover that is not valid, as the grammar says, and goes on to the first image", () => {
    for (const cover of ["", "   ", "color:chartreuse", "gradient:Not Valid", "attachment:nope", "https://example.com/x.png", "sunset"]) {
      expect(previewOf(page({ cover }), images()), cover).toEqual({ kind: "image", attachmentId: PICTURE });
      expect(previewOf(page({ cover }), NONE), cover).toEqual({ kind: "none" });
    }
  });

  it("skips a gradient this version does not bundle: it cannot be drawn", () => {
    expect(previewOf(page({ cover: "gradient:from-a-newer-version" }), images())).toEqual({ kind: "image", attachmentId: PICTURE });
    expect(previewOf(page({ cover: "gradient:from-a-newer-version" }), NONE)).toEqual({ kind: "none" });
  });

  it("clamps the cover's vertical position to 0 to 100 and rounds it", () => {
    const y = (coverY: number | undefined) => {
      const preview = previewOf(page({ cover: `attachment:${COVER}`, coverY }), NONE);
      return preview.kind === "cover" ? preview.y : null;
    };
    expect(y(undefined)).toBe(50);
    expect(y(0)).toBe(0);
    expect(y(100)).toBe(100);
    expect(y(130)).toBe(100);
    expect(y(-5)).toBe(0);
    expect(y(33.4)).toBe(33);
    expect(y(Number.NaN)).toBe(50);
  });
});

describe("previewCandidates", () => {
  it("lists a picture cover and then the first image, the cover being the one tried first", () => {
    expect(previewCandidates(page({ cover: `attachment:${COVER}` }), images())).toEqual([
      { kind: "cover", cover: { kind: "image", attachmentId: COVER }, y: 50 },
      { kind: "image", attachmentId: PICTURE },
    ]);
  });

  it("stops at a colour or a bundled gradient, which always draw", () => {
    expect(previewCandidates(page({ cover: "color:red" }), images())).toHaveLength(1);
    expect(previewCandidates(page({ cover: "gradient:ocean" }), images())).toHaveLength(1);
  });

  it("is empty for a plain card and has one entry for a page with only a first image", () => {
    expect(previewCandidates(page(), NONE)).toEqual([]);
    expect(previewCandidates(page(), images())).toEqual([{ kind: "image", attachmentId: PICTURE }]);
  });
});

describe("needsFirstImage", () => {
  it("is false for a colour and a bundled gradient and true for everything else", () => {
    expect(needsFirstImage(page())).toBe(true);
    expect(needsFirstImage(page({ cover: `attachment:${COVER}` }))).toBe(true);
    expect(needsFirstImage(page({ cover: "color:green" }))).toBe(false);
    expect(needsFirstImage(page({ cover: "gradient:mint" }))).toBe(false);
    expect(needsFirstImage(page({ cover: "gradient:from-a-newer-version" }))).toBe(true);
    expect(needsFirstImage(page({ cover: "garbage" }))).toBe(true);
  });
});

describe("the card size", () => {
  it("is medium unless the view says otherwise, and each size has its width", () => {
    expect(cardSizeOf({})).toBe("m");
    expect(cardSizeOf({ cardSize: "s" })).toBe("s");
    expect(cardSizeOf({ cardSize: "l" })).toBe("l");
    expect(GALLERY_CARD_WIDTHS).toEqual({ s: 160, m: 240, l: 320 });
  });

  it("reads a select's value, medium for anything else", () => {
    expect(readCardSize("s")).toBe("s");
    expect(readCardSize("l")).toBe("l");
    expect(readCardSize("m")).toBe("m");
    expect(readCardSize("xl")).toBe("m");
    expect(readCardSize("")).toBe("m");
  });
});

describe("pictureStatus", () => {
  const found = (mime: string, size: number, availability: "Complete" | "NotDownloaded" | "Downloading" | "Waiting" = "Complete"): AttachmentLookup => ({
    status: "found",
    info: { attachmentId: PICTURE, spaceId: SPACE, name: "x", mime, size, chunkCount: 1, createdAt: 1, references: 1, localChunks: 1, availability },
  });
  const facts = (extra: Partial<PictureFacts> = {}): PictureFacts => ({
    url: null,
    broken: null,
    canRead: true,
    lookup: undefined,
    readFailed: false,
    ...extra,
  });

  it("is ready with a URL, until the webview cannot decode it", () => {
    expect(pictureStatus(facts({ url: "blob:a", lookup: found("image/png", 10) }))).toBe("ready");
    expect(pictureStatus(facts({ url: "blob:a", broken: "blob:a" }))).toBe("failed");
    expect(pictureStatus(facts({ url: "blob:b", broken: "blob:a" }))).toBe("ready");
  });

  it("is loading while the attachment is looked up, read or downloaded", () => {
    expect(pictureStatus(facts())).toBe("loading");
    expect(pictureStatus(facts({ lookup: { status: "loading" } }))).toBe("loading");
    expect(pictureStatus(facts({ lookup: found("image/png", 10) }))).toBe("loading");
    for (const availability of ["NotDownloaded", "Downloading", "Waiting"] as const) {
      expect(pictureStatus(facts({ lookup: found("image/jpeg", 10, availability) })), availability).toBe("loading");
    }
  });

  it("is failed when it never will be a picture: no way to read files, unknown, not an image, too big, unreadable", () => {
    expect(pictureStatus(facts({ canRead: false }))).toBe("failed");
    expect(pictureStatus(facts({ lookup: { status: "missing" } }))).toBe("failed");
    expect(pictureStatus(facts({ lookup: { status: "error", message: "io" } }))).toBe("failed");
    expect(pictureStatus(facts({ lookup: found("application/pdf", 10) }))).toBe("failed");
    expect(pictureStatus(facts({ lookup: found("text/plain", 10) }))).toBe("failed");
    expect(pictureStatus(facts({ lookup: found("image/png", MAX_COVER_BYTES + 1) }))).toBe("failed");
    expect(pictureStatus(facts({ lookup: found("image/png", MAX_COVER_BYTES) }))).toBe("loading");
    expect(pictureStatus(facts({ lookup: found("image/png", 10), readFailed: true }))).toBe("failed");
  });

  it("reads the MIME type without regard to case", () => {
    expect(pictureStatus(facts({ lookup: found("IMAGE/PNG", 10) }))).toBe("loading");
  });
});
