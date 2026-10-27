// The pictures of uploaded page icons (ADR-0029, section 1): an icon `attachment:<id>` is an
// ordinary attachment, small, that the sidebar, the breadcrumbs, the search and every link to
// the page show at once. So the bytes are read once per attachment and become one `blob:` URL
// that every place showing it shares, reference-counted: it is revoked when the last place that
// showed it is gone (after a turn of the event loop, so that a row the tree draws again, a page
// moved under another, does not read its picture a second time).
//
// On a device with the on-demand download policy the attachment may not be here yet: an icon is
// small, so the first place that shows it asks the core for it (`AttachmentDownload`, once), and
// shows the default glyph until it is complete.
//
// There is one cache per app store (`iconImagesOf`): two `<App>` mounted in one page, as the
// tests do, never share a picture or a request.

import { useCallback, useEffect, useSyncExternalStore } from "react";
import { useStore } from "zustand";

import type { AppStore } from "../store";

/**
 * Icons are small by construction (the picker makes them 256 x 256 at most); a bigger image
 * that another client stored as an icon is not read for every row of the sidebar.
 */
export const MAX_ICON_BYTES = 2 * 1024 * 1024;

/** What the cache knows about the picture of one attachment. */
export type IconImage = { status: "loading" } | { status: "ready"; url: string } | { status: "failed" };

const LOADING: IconImage = { status: "loading" };

interface Entry {
  /** Places showing the picture now. */
  users: number;
  image: IconImage;
  listeners: Set<() => void>;
  /** The pending clean-up of an entry nobody uses. */
  teardown: ReturnType<typeof setTimeout> | null;
}

export interface IconImageSource {
  /** The whole bytes of an attachment; rejects when it cannot be read. */
  read: (attachmentId: string) => Promise<Uint8Array>;
  /** Asks for the chunks of an attachment this device lacks; rejects when the core refuses. */
  download: (attachmentId: string) => Promise<unknown>;
}

export class IconImageCache {
  private readonly entries = new Map<string, Entry>();
  private readonly requested = new Set<string>();

  constructor(private readonly source: IconImageSource) {}

  /** The picture of the attachment as it is now (`loading` for one nobody shows). */
  snapshot(attachmentId: string): IconImage {
    return this.entries.get(attachmentId)?.image ?? LOADING;
  }

  /** How many attachments have a picture (or a read on its way) held by the cache. */
  get size(): number {
    return this.entries.size;
  }

  /**
   * Starts showing the picture of a complete attachment: reads it when nobody shows it yet, and
   * calls `listener` whenever `snapshot` changes. Returns what ends the use.
   */
  subscribe(attachmentId: string, mime: string, listener: () => void): () => void {
    let entry = this.entries.get(attachmentId);
    if (entry === undefined) {
      entry = { users: 0, image: LOADING, listeners: new Set(), teardown: null };
      this.entries.set(attachmentId, entry);
      this.load(attachmentId, mime, entry);
    }
    if (entry.teardown !== null) {
      clearTimeout(entry.teardown);
      entry.teardown = null;
    }
    entry.users += 1;
    // One function per use, so that the same listener given twice counts twice.
    const mine = (): void => {
      listener();
    };
    entry.listeners.add(mine);
    const held = entry;
    let released = false;
    return () => {
      if (released) {
        return;
      }
      released = true;
      held.listeners.delete(mine);
      held.users -= 1;
      if (held.users === 0) {
        held.teardown = setTimeout(() => {
          this.drop(attachmentId, held);
        }, 0);
      }
    };
  }

  /** Asks the core for an attachment this device lacks, once (again after a refusal). */
  requestDownload(attachmentId: string): void {
    if (this.requested.has(attachmentId)) {
      return;
    }
    this.requested.add(attachmentId);
    this.source.download(attachmentId).catch(() => {
      this.requested.delete(attachmentId);
    });
  }

  private load(attachmentId: string, mime: string, entry: Entry): void {
    const settle = (image: IconImage): void => {
      // The entry may be gone (and a new one made for the id) while the bytes were on their way.
      if (this.entries.get(attachmentId) !== entry) {
        return;
      }
      entry.image = image;
      for (const listener of [...entry.listeners]) {
        listener();
      }
    };
    this.source.read(attachmentId).then(
      (bytes) => {
        if (this.entries.get(attachmentId) !== entry) {
          return;
        }
        try {
          const blob = new Blob([bytes as Uint8Array<ArrayBuffer>], { type: mime });
          settle({ status: "ready", url: URL.createObjectURL(blob) });
        } catch {
          settle({ status: "failed" });
        }
      },
      () => {
        settle({ status: "failed" });
      },
    );
  }

  private drop(attachmentId: string, entry: Entry): void {
    if (entry.users > 0 || this.entries.get(attachmentId) !== entry) {
      return;
    }
    this.entries.delete(attachmentId);
    if (entry.image.status === "ready") {
      try {
        URL.revokeObjectURL(entry.image.url);
      } catch {
        // An environment without object URLs made none either.
      }
    }
  }
}

const caches = new WeakMap<AppStore, IconImageCache>();

/** The cache of a store: its attachments are read and downloaded through the store's own actions. */
export function iconImagesOf(store: AppStore): IconImageCache {
  let cache = caches.get(store);
  if (cache === undefined) {
    cache = new IconImageCache({
      read: (attachmentId) => store.getState().readAttachment(attachmentId),
      download: (attachmentId) => store.getState().downloadAttachment(attachmentId),
    });
    caches.set(store, cache);
  }
  return cache;
}

/**
 * The most the banner of a page cover reads (ADR-0040): the largest picture the UI previews. A cover is a picture
 * of its own, not a glyph, and the upload keeps it within 2560 pixels, which is a few megabytes.
 */
export const MAX_COVER_BYTES = 32 * 1024 * 1024;

/**
 * The `blob:` URL of the picture of an uploaded icon, `null` while it cannot be shown: not looked
 * up yet, not an image, too big (more than `maxBytes`), not all here (a download is asked for when
 * this device has none of it), not readable. The caller shows the default glyph then. A page cover
 * reads its picture the same way, with a larger limit.
 */
export function useIconImage(store: AppStore, attachmentId: string, maxBytes: number = MAX_ICON_BYTES): string | null {
  const cache = iconImagesOf(store);
  const lookup = useStore(store, (state) => state.attachmentLookup[attachmentId]);
  const lookupAttachment = useStore(store, (state) => state.lookupAttachment);
  const canRead = useStore(store, (state) => state.attachmentCapabilities.read);

  const needsLookup = lookup === undefined;
  useEffect(() => {
    if (needsLookup) {
      lookupAttachment(attachmentId);
    }
  }, [needsLookup, lookupAttachment, attachmentId]);

  const info = lookup?.status === "found" ? lookup.info : null;
  const usable =
    canRead && info !== null && info.mime.toLowerCase().startsWith("image/") && Number(info.size) <= maxBytes;
  const complete = usable && info.availability === "Complete";
  const notDownloaded = usable && info.availability === "NotDownloaded";
  const mime = info?.mime ?? "";

  useEffect(() => {
    if (notDownloaded) {
      cache.requestDownload(attachmentId);
    }
  }, [notDownloaded, cache, attachmentId]);

  const subscribe = useCallback(
    (listener: () => void) => (complete ? cache.subscribe(attachmentId, mime, listener) : () => undefined),
    [cache, complete, attachmentId, mime],
  );
  const getSnapshot = useCallback(() => (complete ? cache.snapshot(attachmentId) : LOADING), [cache, complete, attachmentId]);
  const image = useSyncExternalStore(subscribe, getSnapshot);
  return image.status === "ready" ? image.url : null;
}
