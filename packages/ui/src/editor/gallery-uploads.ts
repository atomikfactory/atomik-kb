// Adding pictures to a gallery (ADR-0030): the pictures a person picks (the file chooser of the
// view, several at once), pastes with the gallery selected, or drops on the block are uploaded one
// after the other through the same service as the pasted and dropped files of the page
// (`AttachmentService.upload`), and each one is appended to the gallery's `items` as soon as it is
// stored, so that a failure further on keeps the pictures already added. The gallery is found again
// by its block id after every upload (the page may have moved on); one that has gone leaves the
// attachment unreferenced, for the retention collector.
//
// What is under way (progress, refusals) lives outside the document, in a small store per editor
// that the view subscribes to; nothing of it is written to the page.

import type { Editor } from "@tiptap/core";

import { t } from "../i18n";
import { describeUploadError, serviceOf } from "./attachments";
import { GALLERY_NODE, MAX_GALLERY_ITEMS, readGalleryItems } from "./gallery-nodes";
import { BLOCK_ID_ATTRIBUTE } from "./schema";

/** A picture on its way into a gallery, or the reason one was refused. */
export interface GalleryTransfer {
  id: number;
  name: string;
  /** Bytes sent and the total, while it uploads. */
  sent: number;
  total: number | null;
  /** Why it was refused or failed; the entry stays until it is dismissed. */
  error: string | null;
}

const NONE: readonly GalleryTransfer[] = [];

/** What is under way in the galleries of one editor, by block id. */
export class GalleryTransfers {
  readonly #byBlock = new Map<string, readonly GalleryTransfer[]>();
  readonly #listeners = new Set<() => void>();
  #next = 0;

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The transfers of a gallery (a stable array until they change). */
  of(blockId: string): readonly GalleryTransfer[] {
    return this.#byBlock.get(blockId) ?? NONE;
  }

  add(blockId: string, name: string, total: number | null, error: string | null = null): number {
    this.#next += 1;
    const id = this.#next;
    this.#set(blockId, [...this.of(blockId), { id, name, sent: 0, total, error }]);
    return id;
  }

  update(blockId: string, id: number, change: Partial<GalleryTransfer>): void {
    this.#set(
      blockId,
      this.of(blockId).map((transfer) => (transfer.id === id ? { ...transfer, ...change } : transfer)),
    );
  }

  remove(blockId: string, id: number): void {
    this.#set(
      blockId,
      this.of(blockId).filter((transfer) => transfer.id !== id),
    );
  }

  /** How many pictures are still uploading or waiting to (refusals not counted). */
  pending(blockId: string): number {
    return this.of(blockId).filter((transfer) => transfer.error === null).length;
  }

  #set(blockId: string, next: readonly GalleryTransfer[]): void {
    if (next.length === 0) {
      this.#byBlock.delete(blockId);
    } else {
      this.#byBlock.set(blockId, next);
    }
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

const stores = new WeakMap<Editor, GalleryTransfers>();

/** The transfers store of an editor (made on first use). */
export function galleryTransfers(editor: Editor): GalleryTransfers {
  let store = stores.get(editor);
  if (store === undefined) {
    store = new GalleryTransfers();
    stores.set(editor, store);
  }
  return store;
}

/** Whether the editor can take pictures into a gallery now: editable, with an upload service. */
export function canAddToGallery(editor: Editor): boolean {
  return !editor.isDestroyed && editor.isEditable && serviceOf(editor)?.canUpload === true;
}

/** Appends `attachmentId` to the gallery with block id `blockId`; `false` when it is gone or full. */
function appendItem(editor: Editor, blockId: string, attachmentId: string): boolean {
  let found: { pos: number; items: ReturnType<typeof readGalleryItems> } | null = null;
  editor.state.doc.descendants((node, pos) => {
    if (found !== null) {
      return false;
    }
    if (node.type.name === GALLERY_NODE && node.attrs[BLOCK_ID_ATTRIBUTE] === blockId) {
      found = { pos, items: readGalleryItems(node.attrs.items) };
    }
    return found === null;
  });
  const target = found as { pos: number; items: ReturnType<typeof readGalleryItems> } | null;
  if (target === null || target.items.length >= MAX_GALLERY_ITEMS) {
    return false;
  }
  editor.view.dispatch(editor.state.tr.setNodeAttribute(target.pos, "items", [...target.items, { attachmentId }]));
  return true;
}

/** How many pictures the gallery with this block id holds now (0 when it is gone). */
function countOf(editor: Editor, blockId: string): number {
  let count = 0;
  editor.state.doc.descendants((node) => {
    if (node.type.name === GALLERY_NODE && node.attrs[BLOCK_ID_ATTRIBUTE] === blockId) {
      count = readGalleryItems(node.attrs.items).length;
      return false;
    }
    return true;
  });
  return count;
}

/** True for a file the browser calls an image (the core checks the bytes again when it stores them). */
function looksLikeImage(file: File): boolean {
  return file.type.toLowerCase().startsWith("image/");
}

/**
 * Uploads `files` into the gallery with block id `blockId`, in order, appending each picture to the
 * gallery when it is stored. A file that is not an image, one beyond the limit of
 * {@link MAX_GALLERY_ITEMS} and one the core refuses leave a line saying so (dismissable) and the
 * others go on. Does nothing where pictures cannot be added ({@link canAddToGallery}).
 */
export function addImagesToGallery(editor: Editor, blockId: string, files: readonly File[]): void {
  const service = serviceOf(editor);
  if (service === null || !canAddToGallery(editor) || files.length === 0) {
    return;
  }
  const transfers = galleryTransfers(editor);
  const room = Math.max(0, MAX_GALLERY_ITEMS - countOf(editor, blockId) - transfers.pending(blockId));
  let accepted = 0;
  const jobs: { file: File; id: number }[] = [];
  for (const file of files) {
    if (!looksLikeImage(file)) {
      transfers.add(blockId, file.name, null, t("gallery.error.notImage", { name: file.name }));
    } else if (accepted >= room) {
      transfers.add(blockId, file.name, null, t("gallery.error.full", { name: file.name, max: MAX_GALLERY_ITEMS }));
    } else {
      accepted += 1;
      jobs.push({ file, id: transfers.add(blockId, file.name, file.size) });
    }
  }
  void (async () => {
    for (const { file, id } of jobs) {
      if (editor.isDestroyed) {
        return;
      }
      try {
        const info = await service.upload(file, (sent, total) => {
          transfers.update(blockId, id, { sent, total: total ?? file.size });
        });
        if (editor.isDestroyed) {
          return;
        }
        if (!info.mime.toLowerCase().startsWith("image/")) {
          transfers.update(blockId, id, { error: t("gallery.error.notImage", { name: file.name }) });
        } else {
          // Appended, or the gallery is gone (the attachment is then unreferenced, as any upload nobody uses).
          appendItem(editor, blockId, info.attachmentId);
          transfers.remove(blockId, id);
        }
      } catch (error) {
        if (editor.isDestroyed) {
          return;
        }
        transfers.update(blockId, id, { error: describeUploadError(error, file.name) });
      }
    }
  })();
}
