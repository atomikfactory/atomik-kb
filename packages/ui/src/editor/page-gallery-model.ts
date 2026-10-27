// The pure rules of the gallery layout (ADR-0054, `docs/architecture/database-views.md` section 6): what a card
// previews and how wide a card is. The view (`PageGalleryView.tsx`) draws what they answer.
//
// The preview of a page, in this order: its cover, else the first image of the page (found by the core,
// `PageFirstImages`), else nothing (a plain card: the icon and the title). A cover this version cannot draw is no
// cover: an invalid string (`parseCover` answers `null`) and a gradient it does not bundle. A cover that is a
// picture may still fail once the view tries it (not an image, too big, unreadable, undecodable): the view then
// goes on to the next candidate of `previewCandidates`.

import type { AttachmentId, PageId, PageInfo } from "@atomik-kb/core-client";

import { coverPosition, isBundledGradient, parseCover } from "../icons/cover-grammar";
import type { ParsedCover } from "../icons/cover-grammar";
import { MAX_COVER_BYTES } from "../icons/icon-images";
import type { AttachmentLookup } from "../store-attachments";
import { CARD_SIZES, DEFAULT_CARD_SIZE } from "./page-list-views";
import type { CardSize, PageListViewSpec } from "./page-list-views";

/** The width of a card in pixels for each size (the grid fills the row with as many as fit). */
export const GALLERY_CARD_WIDTHS: Readonly<Record<CardSize, number>> = { s: 160, m: 240, l: 320 };

/** What the top of a card shows. */
export type GalleryPreview =
  /** The page's cover (`PageInfo.cover`, `coverY`): an attachment, a palette colour or a bundled gradient. */
  | { readonly kind: "cover"; readonly cover: ParsedCover; readonly y: number }
  /** The first image in the page, an attachment. */
  | { readonly kind: "image"; readonly attachmentId: AttachmentId }
  /** Neither: the icon on a plain card (the card has no picture area). */
  | { readonly kind: "none" };

/** The card size of a view (`m` when it has none). */
export function cardSizeOf(view: Pick<PageListViewSpec, "cardSize">): CardSize {
  return view.cardSize ?? DEFAULT_CARD_SIZE;
}

/** The card size a `<select>` value names; the default for anything else. */
export function readCardSize(value: string): CardSize {
  return CARD_SIZES.find((size) => size === value) ?? DEFAULT_CARD_SIZE;
}

/** A cover that needs no file: a colour of the palette or a gradient this version bundles. It always draws. */
function isSwatch(cover: ParsedCover): boolean {
  return cover.kind === "color" || (cover.kind === "gradient" && isBundledGradient(cover.name));
}

/**
 * Whether the card of `page` may need the page's first image: not when its cover is a colour or a bundled gradient
 * (always drawn). A page with a picture cover is included, since that picture may fail to show and the first image
 * comes next.
 */
export function needsFirstImage(page: PageInfo): boolean {
  const cover = parseCover(page.cover);
  return cover === null || !isSwatch(cover);
}

/**
 * The previews a card tries, best first: the cover when this version can draw it, then the first image the core found.
 * Nothing after a colour or a gradient (they always draw). Empty: a plain card.
 */
export function previewCandidates(page: PageInfo, firstImages: ReadonlyMap<PageId, AttachmentId>): readonly GalleryPreview[] {
  const cover = parseCover(page.cover);
  const candidates: GalleryPreview[] = [];
  if (cover !== null && (cover.kind !== "gradient" || isBundledGradient(cover.name))) {
    candidates.push({ kind: "cover", cover, y: coverPosition(page.coverY) });
    if (isSwatch(cover)) {
      return candidates;
    }
  }
  const attachmentId = firstImages.get(page.id);
  if (attachmentId !== undefined) {
    candidates.push({ kind: "image", attachmentId });
  }
  return candidates;
}

/**
 * The preview of a page, in this order: its cover when it has one this version can draw (`parseCover`; `y` is `coverY`
 * or 50, from 0 to 100), else the first image the core found for it (`firstImages`, from `PageFirstImages`), else none.
 */
export function previewOf(page: PageInfo, firstImages: ReadonlyMap<PageId, AttachmentId>): GalleryPreview {
  return previewCandidates(page, firstImages)[0] ?? { kind: "none" };
}

/** How far a picture of a card has got: on its way, drawn, or never going to be (the next candidate then). */
export type PictureStatus = "loading" | "ready" | "failed";

export interface PictureFacts {
  /** The `blob:` URL the icon-image cache gives (`useIconImage`), `null` until there is one. */
  readonly url: string | null;
  /** The URL the webview failed to decode (`img` `error`), if any. */
  readonly broken: string | null;
  /** Whether the shell can read attachment bytes at all. */
  readonly canRead: boolean;
  /** What the store knows of the attachment (`undefined`: not asked yet). */
  readonly lookup: AttachmentLookup | undefined;
  /** The cache tried to read the bytes and could not. */
  readonly readFailed: boolean;
}

/**
 * Whether a picture (a cover, a first image) can be drawn: `ready` with its URL, `loading` while it is being looked up, read
 * or downloaded, `failed` when it never will be (the shell cannot read files, the attachment is unknown, is not an image,
 * is bigger than a cover may be, cannot be read or cannot be decoded). A failed cover is not a preview: the card tries the
 * next candidate.
 */
export function pictureStatus({ url, broken, canRead, lookup, readFailed }: PictureFacts): PictureStatus {
  if (url !== null) {
    return url === broken ? "failed" : "ready";
  }
  if (!canRead || readFailed) {
    return "failed";
  }
  if (lookup === undefined || lookup.status === "loading") {
    return "loading";
  }
  if (lookup.status !== "found") {
    return "failed";
  }
  const { mime, size } = lookup.info;
  if (!mime.toLowerCase().startsWith("image/") || Number(size) > MAX_COVER_BYTES) {
    return "failed";
  }
  // Complete: being read now; not downloaded, waiting or downloading: on its way (the icon-image cache asks for it).
  return "loading";
}
