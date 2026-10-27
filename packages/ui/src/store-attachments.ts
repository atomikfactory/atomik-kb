// The attachment part of the app store (Milestone 5, ADR-0020): what the blocks of the open
// page know about their attachments (name, type, size, or that they are gone), uploads and
// the shell's file actions, and the numbers of Settings -> Data. `store.ts` composes it with
// the rest.
//
// As in the other slices, reads are "latest request wins" and `AttachmentsChanged { spaceId }`
// refreshes what it names: the attachments of that space the UI has looked up, and the
// statistics when they are about that space. Since Milestone 8 (ADR-0023) the event also fires
// as chunks arrive and leave, and a manifest can reach this device after the block that uses
// it, so the lookups that answered "missing" or failed are read again on every such event too
// (they do not know their space); a lookup that comes back unchanged leaves the store as it
// was, so a card does not re-render for every chunk of another attachment. Attachment bytes
// never enter the store: a block reads them itself and keeps them only as long as it shows
// them.

import { toCoreError } from "@atomik-kb/core-client";
import type {
  AttachmentId,
  AttachmentInfo,
  ByteRange,
  CoreClient,
  Event as CoreEvent,
  GcSummary,
  SpaceId,
  UploadProgress,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/** What the UI knows about one attachment. */
export type AttachmentLookup =
  | { status: "loading" }
  | { status: "found"; info: AttachmentInfo }
  /** Unknown or purged: its blocks show a "deleted" card. */
  | { status: "missing" }
  | { status: "error"; message: string };

/** Count and total size of the attachments of one space (Settings -> Data). */
export interface AttachmentStats {
  spaceId: SpaceId;
  status: "loading" | "ready" | "error";
  count: number;
  totalBytes: number;
}

/**
 * What the transport can do with attachment bytes and files. The client offers all of it or
 * nothing today (`hasAttachments`); the flags stay apart so that the UI hides each action on
 * its own.
 */
export interface AttachmentCapabilities {
  /** Pasted and dropped files become attachments (`attachmentUpload`). */
  upload: boolean;
  /** Images show from their bytes (`attachmentRead`). */
  read: boolean;
  /** The shell's file picker (the "Image" and "File" commands). */
  importPath: boolean;
  /**
   * The shell downloads a picture from a link (the "Image from URL" command, ADR-0028). Apart
   * from `importPath`: the web build will have a picker and no way to fetch a link (CORS).
   */
  importUrl: boolean;
  saveAs: boolean;
  open: boolean;
}

export interface AttachmentsData {
  attachmentLookup: Readonly<Record<AttachmentId, AttachmentLookup>>;
  /** Statistics of the space Settings shows; `null` before they were asked for. */
  attachmentStats: AttachmentStats | null;
}

export interface AttachmentsActions {
  attachmentCapabilities: AttachmentCapabilities;
  /** Makes sure `attachmentLookup` has the attachment; a no-op once known. */
  lookupAttachment: (attachmentId: AttachmentId) => void;
  /**
   * Uploads a pasted or dropped file into a space (an empty MIME type becomes
   * `application/octet-stream`) and returns the new attachment; rejects with a `CoreError`.
   */
  uploadAttachment: (spaceId: SpaceId, file: File, onProgress?: UploadProgress) => Promise<AttachmentInfo>;
  /**
   * Lets the user pick a file in the shell's dialog and uploads it into the space; resolves
   * `null` when the dialog was cancelled. Rejects with a `CoreError` otherwise.
   */
  importAttachment: (spaceId: SpaceId) => Promise<AttachmentInfo | null>;
  /**
   * Has the shell download the picture an https link points to, once, and store it as an
   * attachment of the space (ADR-0028). The link goes to the shell and nowhere else: the page
   * never holds it. Rejects with a `CoreError`: `InvalidInput` for the field `url` with
   * `details.reason` `status`, `network`, `timeout`, `redirect` or `notImage`, or for `size`.
   */
  importAttachmentFromUrl: (spaceId: SpaceId, url: string) => Promise<AttachmentInfo>;
  /** The whole plaintext (at most `MAX_PREVIEW_BYTES`); rejects with a `CoreError`. */
  readAttachment: (attachmentId: AttachmentId) => Promise<Uint8Array>;
  /**
   * The half-open byte `range` of the plaintext, at most `MAX_PREVIEW_BYTES` long: only the
   * chunks it touches are read and decrypted, whatever the size of the attachment (the PDF
   * viewer, ADR-0031); rejects with a `CoreError`.
   */
  readAttachmentRange: (attachmentId: AttachmentId, range: ByteRange) => Promise<Uint8Array>;
  /**
   * The URL a `<video>` or `<audio>` element streams the attachment from (the desktop shell's
   * media protocol, ADR-0032), or `null` where there is none and the player makes a blob from
   * {@link AttachmentsActions.readAttachment} instead.
   */
  attachmentMediaUrl: (attachmentId: AttachmentId) => string | null;
  /**
   * Asks the core for the chunks of the attachment this device lacks, whatever the download
   * policy (`AttachmentDownload`, ADR-0023), and remembers the answer (usually `Downloading`);
   * rejects with a `CoreError`.
   */
  downloadAttachment: (attachmentId: AttachmentId) => Promise<AttachmentInfo>;
  /** Writes the attachment to a file the user picks; resolves `false` when cancelled. */
  saveAttachmentAs: (attachmentId: AttachmentId) => Promise<boolean>;
  /** Opens the attachment with the system's default application. */
  openAttachment: (attachmentId: AttachmentId) => Promise<void>;
  /** Loads the statistics of a space, or forgets them with `null`. */
  loadAttachmentStats: (spaceId: SpaceId | null) => void;
  /** Collects the attachments unreferenced past the retention now; rejects with a `CoreError`. */
  collectAttachments: () => Promise<GcSummary>;
}

export type AttachmentsState = AttachmentsData & AttachmentsActions;

export const EMPTY_ATTACHMENTS: AttachmentsData = { attachmentLookup: {}, attachmentStats: null };

/** The type stored for a file whose type the browser does not know. */
export const FALLBACK_MIME = "application/octet-stream";

/** True for the error a cancelled file dialog answers: `InvalidInput` for the field `path`. */
export function isCancelled(error: unknown): boolean {
  const failure = toCoreError(error);
  const { details } = failure;
  return (
    failure.code === "InvalidInput" &&
    typeof details === "object" &&
    details !== null &&
    !Array.isArray(details) &&
    details.field === "path"
  );
}

/** Two answers about the same attachment say the same thing (every field is a plain value). */
function sameInfo(a: AttachmentInfo, b: AttachmentInfo): boolean {
  const fields = new Set([...Object.keys(a), ...Object.keys(b)]) as Set<keyof AttachmentInfo>;
  for (const field of fields) {
    if (a[field] !== b[field]) {
      return false;
    }
  }
  return true;
}

function sameLookup(a: AttachmentLookup | undefined, b: AttachmentLookup): boolean {
  switch (b.status) {
    case "found":
      return a?.status === "found" && sameInfo(a.info, b.info);
    case "error":
      return a?.status === "error" && a.message === b.message;
    default:
      return a?.status === b.status;
  }
}

export interface AttachmentsSlice {
  initial: AttachmentsState;
  onEvent: (event: CoreEvent) => void;
}

export function createAttachmentsSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): AttachmentsSlice {
  const supported = client.hasAttachments();
  const lookupTickets = new Map<AttachmentId, number>();
  let statsTicket = 0;

  function remember(info: AttachmentInfo): void {
    lookupTickets.set(info.attachmentId, (lookupTickets.get(info.attachmentId) ?? 0) + 1);
    set((state) => ({
      attachmentLookup: { ...state.attachmentLookup, [info.attachmentId]: { status: "found", info } },
    }));
  }

  async function fetchLookup(attachmentId: AttachmentId): Promise<void> {
    const ticket = (lookupTickets.get(attachmentId) ?? 0) + 1;
    lookupTickets.set(attachmentId, ticket);
    let lookup: AttachmentLookup;
    try {
      lookup = { status: "found", info: await client.attachmentGet(attachmentId) };
    } catch (error) {
      const failure = toCoreError(error);
      lookup = failure.code === "NotFound" ? { status: "missing" } : { status: "error", message: failure.message };
    }
    if (lookupTickets.get(attachmentId) === ticket && !sameLookup(get().attachmentLookup[attachmentId], lookup)) {
      set((state) => ({ attachmentLookup: { ...state.attachmentLookup, [attachmentId]: lookup } }));
    }
  }

  async function fetchStats(spaceId: SpaceId): Promise<void> {
    const ticket = (statsTicket += 1);
    try {
      const list = await client.attachmentList(spaceId);
      if (ticket === statsTicket) {
        const totalBytes = list.reduce((sum, info) => sum + Number(info.size), 0);
        set({ attachmentStats: { spaceId, status: "ready", count: list.length, totalBytes } });
      }
    } catch (error) {
      if (ticket === statsTicket) {
        set((state) => ({
          attachmentStats: {
            spaceId,
            status: "error",
            count: state.attachmentStats?.spaceId === spaceId ? state.attachmentStats.count : 0,
            totalBytes: state.attachmentStats?.spaceId === spaceId ? state.attachmentStats.totalBytes : 0,
          },
        }));
      }
      throw error;
    }
  }

  const initial: AttachmentsState = {
    ...EMPTY_ATTACHMENTS,
    attachmentCapabilities: {
      upload: supported,
      read: supported,
      importPath: supported,
      importUrl: client.canImportUrl(),
      saveAs: supported,
      open: supported,
    },

    lookupAttachment: (attachmentId) => {
      if (get().attachmentLookup[attachmentId] !== undefined) {
        return;
      }
      set((state) => ({ attachmentLookup: { ...state.attachmentLookup, [attachmentId]: { status: "loading" } } }));
      refreshQuietly(() => fetchLookup(attachmentId));
    },

    uploadAttachment: async (spaceId, file, onProgress) => {
      const mime = file.type === "" ? FALLBACK_MIME : file.type;
      const info = await client.attachmentUpload(spaceId, file.name, mime, file, onProgress);
      remember(info);
      return info;
    },

    importAttachment: async (spaceId) => {
      let info: AttachmentInfo;
      try {
        info = await client.attachmentImportPath(spaceId);
      } catch (error) {
        if (isCancelled(error)) {
          return null;
        }
        throw error;
      }
      remember(info);
      return info;
    },

    importAttachmentFromUrl: async (spaceId, url) => {
      const info = await client.attachmentImportUrl(spaceId, url);
      remember(info);
      return info;
    },

    readAttachment: (attachmentId) => client.attachmentRead(attachmentId),

    readAttachmentRange: (attachmentId, range) => client.attachmentRead(attachmentId, range),

    attachmentMediaUrl: (attachmentId) => client.attachmentMediaUrl(attachmentId),

    downloadAttachment: async (attachmentId) => {
      const info = await client.attachmentDownload(attachmentId);
      remember(info);
      return info;
    },

    saveAttachmentAs: async (attachmentId) => {
      try {
        await client.attachmentSaveAs(attachmentId);
        return true;
      } catch (error) {
        if (isCancelled(error)) {
          return false;
        }
        throw error;
      }
    },

    openAttachment: async (attachmentId) => {
      await client.attachmentOpen(attachmentId);
    },

    loadAttachmentStats: (spaceId) => {
      if (spaceId === null) {
        statsTicket += 1;
        set({ attachmentStats: null });
        return;
      }
      set((state) => ({
        attachmentStats:
          state.attachmentStats?.spaceId === spaceId
            ? state.attachmentStats
            : { spaceId, status: "loading", count: 0, totalBytes: 0 },
      }));
      refreshQuietly(() => fetchStats(spaceId));
    },

    collectAttachments: () => client.attachmentGc(),
  };

  return {
    initial,
    onEvent: (event) => {
      if (event.type !== "AttachmentsChanged") {
        return;
      }
      const { spaceId } = event.data;
      for (const [attachmentId, lookup] of Object.entries(get().attachmentLookup)) {
        // A lookup still loading may have been answered before the change: it is read again too.
        if (lookup.status !== "found" || lookup.info.spaceId === spaceId) {
          refreshQuietly(() => fetchLookup(attachmentId));
        }
      }
      if (get().attachmentStats?.spaceId === spaceId) {
        refreshQuietly(() => fetchStats(spaceId));
      }
    },
  };
}
