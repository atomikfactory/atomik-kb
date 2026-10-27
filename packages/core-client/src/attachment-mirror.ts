// The in-memory twin of the core's Milestone 5 attachments (`kb-core`'s `attachments.rs`,
// ADR-0020, `docs/development/milestone-05.md`, "Core"), used by `MemoryTransport`: uploads
// in progress, chunks in a fake blob store, one manifest per attachment, references derived
// from the page documents, retention-based garbage collection and the binary read. Since
// Milestone 8 (ADR-0023, `docs/development/milestone-08.md`, "TypeScript client") also the
// attachment side of sync (`kb-core`'s `transfers.rs`): attachments that exist on a simulated
// server but not (yet) here, the download policy, `AttachmentDownload`, the availability of
// each attachment, the transfer counts of `SyncInfo` and a fake of the driver's transfer step.
//
// Nothing is encrypted: a chunk holds its plaintext as it is. Everything the UI can observe
// is kept faithful: chunks are cut at `CHUNK_SIZE` like the core cuts them (full chunks as
// they complete, then a final chunk at `finish`, always present and possibly empty, so a
// file has `floor(size / CHUNK_SIZE) + 1` chunks), sizes, chunk counts, reference counts,
// `unreferencedSince`, the GC summary, the events and the errors (codes, fields, messages).

import {
  CHUNK_SIZE,
  DEFAULT_RETENTION_DAYS,
  DOWNLOAD_POLICY_EAGER,
  DOWNLOAD_POLICY_SETTING,
  MAX_ATTACHMENT_BYTES,
  RETENTION_SETTING,
  downloadPolicyOf,
  sizeLimitError,
  validateAttachmentName,
  validateMime,
  validateRange,
} from "./attachments";
import type { DownloadPolicy } from "./attachments";
import { fromBase64, toBase64 } from "./backup";
import type { MemoryContent } from "./content";
import { CoreError, internalError } from "./errors";
import type { AttachmentAvailability } from "./generated/AttachmentAvailability";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { ByteRange } from "./generated/ByteRange";
import type { ChunkId } from "./generated/ChunkId";
import type { Event } from "./generated/Event";
import type { GcSummary } from "./generated/GcSummary";
import type { SpaceId } from "./generated/SpaceId";
import type { TransferInfo } from "./generated/TransferInfo";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { canonicalId } from "./page-doc";
import type { MemoryStructure } from "./structure";
import { invalid } from "./validate";

const MS_PER_DAY = 86_400_000;

/** An attachment present from the start (`MemoryTransportOptions.attachments`). */
export interface MemoryAttachmentSeed {
  /** Owning space; defaults to the first space. */
  spaceId?: SpaceId;
  /**
   * The attachment's id, so that seeded page content can show it (`image` and `file` blocks
   * name it in `attachmentId`); defaults to the next id of `MemoryTransportOptions.newId`.
   */
  attachmentId?: AttachmentId;
  /** File name, checked like `AttachmentBegin` checks it. */
  name: string;
  /** MIME type, checked like `AttachmentBegin` checks it. */
  mime: string;
  /** The content. */
  bytes: Uint8Array;
}

/**
 * An attachment another device of the account made (`MemoryAttachmentsOptions.remoteAttachments`):
 * its manifest reached this device through sync, its chunks are on the simulated server (or not
 * yet, with `waiting`) and none is here. Checked and cut into chunks like
 * {@link MemoryAttachmentSeed}.
 */
export interface MemoryRemoteAttachmentSeed extends MemoryAttachmentSeed {
  /**
   * The uploading device has not finished: the server holds none of the chunks, so the
   * transfers never progress. Once its chunks are wanted the attachment is `Waiting`: at once
   * under the eager policy; under on demand after `AttachmentDownload` (or a read) and one
   * transfer step that finds the server without the chunk. Default `false`: the server holds
   * every chunk.
   */
  waiting?: boolean;
}

/**
 * The attachment side of sync in the mirror (`MemoryTransportOptions.attachmentSync`,
 * Milestone 8, ADR-0023; the dev page's `?attachments=downloading|waiting|on-demand`).
 */
export interface MemoryAttachmentsOptions {
  /**
   * Stored as the setting `DOWNLOAD_POLICY_SETTING` (`app.attachments.download`) at start,
   * replacing a value from `MemoryTransportOptions.settings`; without it the setting is left as
   * it is (absent: eager). The mirror reads the setting whenever it needs the policy, so a later
   * `settingsSet` changes it (to eager: every incomplete attachment becomes wanted).
   */
  downloadPolicy?: DownloadPolicy;
  /**
   * Attachments present on the simulated server but not here (`localChunks: 0`), created after
   * the local seeds (`MemoryTransportOptions.attachments`) without events: `Downloading` under
   * the eager policy (`Waiting` with `waiting`), `NotDownloaded` under on demand. Their chunks
   * arrive one per transfer step (`MemoryTransport.advanceTransfers`, or the timer of
   * `transferStepMs`).
   */
  remoteAttachments?: readonly MemoryRemoteAttachmentSeed[];
  /**
   * When set, a timer runs one transfer step every `transferStepMs` milliseconds while a
   * transfer is due, so downloads progress by themselves (the dev page); a step is skipped while
   * the core is not ready. Without it (tests) transfers move only through
   * `MemoryTransport.advanceTransfers`.
   */
  transferStepMs?: number;
}

/** An attachment as a backup of the mirror carries it: its manifest and its chunks (base64, in order). */
export interface AttachmentState {
  attachmentId: AttachmentId;
  spaceId: SpaceId;
  name: string;
  mime: string;
  size: number;
  createdAt: number;
  chunks: { id: ChunkId; data: string }[];
}

/** Every attachment and the attachment tombstones of the keyring, for a backup. */
export interface AttachmentsState {
  attachments: AttachmentState[];
  tombstones: { spaceId: SpaceId; count: number }[];
}

/** What the attachment mirror needs from its owner. */
export interface AttachmentDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Id generator (UUID text), for attachment ids. */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Spaces, and which pages exist (a purged page references nothing). */
  structure: MemoryStructure;
  /** Page documents, which the references are derived from. */
  content: MemoryContent;
  /** Whether the workspace exists: an upload needs one. */
  hasWorkspace: () => boolean;
  /** A setting's value; `undefined` when it is not set. */
  setting: (key: string) => JsonValue | undefined;
  /**
   * Delivers an event of a transfer (a chunk arrived, an attachment became wanted or waiting,
   * the counts changed): a change of this device's blob store, not a local edit to upload.
   * Defaults to `emit`.
   */
  emitTransfer?: (event: Event) => void;
  /** Pokes the sync driver (`SyncRequested`), as `AttachmentDownload` and a demanding read do. */
  requestSync?: () => void;
  /** Whether sync is configured: the chunks of an attachment finished meanwhile are to upload (default: never). */
  syncConfigured?: () => boolean;
  /** Whether the core is ready; the transfer timer skips its steps while it is not (default: always). */
  ready?: () => boolean;
  /** `MemoryAttachmentsOptions.transferStepMs`. */
  transferStepMs?: number | undefined;
}

/** A finished attachment: its manifest and the index row the core keeps for it. */
interface Manifest {
  attachmentId: AttachmentId;
  spaceId: SpaceId;
  name: string;
  mime: string;
  size: number;
  chunks: ChunkId[];
  createdAt: number;
  /** Since when nothing references it (set at `finish`); `undefined` while referenced. */
  unreferencedSince: number | undefined;
  /**
   * Chunks the bookkeeping counts as here (the core's `chunk_sync` rows in `UPLOAD` or
   * `SYNCED`): the first `local` of `chunks`, which arrive in order. A test that deletes a chunk
   * from `blobs` behind the mirror's back leaves this unchanged, as it leaves the core's table.
   */
  local: number;
  /** Chunks finished here while sync was configured that the server lacks yet (`UPLOAD` rows). */
  toUpload: number;
  /** The missing chunks are wanted (`DOWNLOAD` rows): the policy was eager, or something asked. */
  wanted: boolean;
  /** At the last attempt the server did not hold the next chunk: the uploader has not finished. */
  unavailable: boolean;
}

/** An upload between `AttachmentBegin` and `AttachmentFinish`/`AttachmentAbort`. */
interface Upload {
  spaceId: SpaceId;
  name: string;
  mime: string;
  createdAt: number;
  /** Full chunks stored so far. */
  chunks: ChunkId[];
  /** Plaintext not stored yet: always shorter than one chunk between calls. */
  pending: Uint8Array;
  /** Bytes received so far. */
  written: number;
}

function notFound(): CoreError {
  return new CoreError({ code: "NotFound", message: "attachment not found" });
}

function noUpload(): CoreError {
  return new CoreError({ code: "NotFound", message: "no upload in progress for this attachment" });
}

function spaceNotFound(): CoreError {
  return new CoreError({ code: "NotFound", message: "space not found" });
}

function damaged(): CoreError {
  return new CoreError({ code: "Crypto", message: "the attachment is damaged or its key is wrong" });
}

/** A read of an attachment whose chunks are not all here (ADR-0023). */
function incomplete(manifest: Manifest): CoreError {
  const counts = `${String(manifest.local)} of ${String(manifest.chunks.length)} chunks`;
  return new CoreError({ code: "Storage", message: `the attachment is not complete on this device (${counts})` });
}

function missingChunk(chunk: ChunkId): CoreError {
  return new CoreError({
    code: "Storage",
    message: `an attachment chunk is missing from the store (${chunk.slice(0, 8)})`,
  });
}

function compareStrings(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Oldest first, then by id: the order transfers take attachments in. */
function byAge(a: Manifest, b: Manifest): number {
  return a.createdAt - b.createdAt || compareStrings(a.attachmentId, b.attachmentId);
}

/** The chunks of an attachment that are not here. */
function missing(manifest: Manifest): number {
  return manifest.chunks.length - manifest.local;
}

/**
 * Uploads, chunks, manifests and references. Methods throw `CoreError`s where the core
 * answers with an `ApiError` and emit `AttachmentsChanged` where the core does.
 *
 * Attachment sync (ADR-0023): an attachment finished here, or seeded with
 * `MemoryTransportOptions.attachments`, is `Complete`. A remote one
 * (`MemoryAttachmentsOptions.remoteAttachments`) starts with no chunk here; its chunks are wanted
 * at once under the eager policy (`Downloading`, or `Waiting` when the server does not hold
 * them), under on demand only once `AttachmentDownload` or a read asks (`NotDownloaded` until
 * then). A transfer step ({@link MemoryAttachments.advance}) does one chunk of work, uploads
 * first, then downloads, oldest attachment first: an upload sends a chunk finished here while
 * sync was configured; a download moves the next chunk of a wanted attachment from the
 * simulated server into `blobs` (`Complete` after the last one) or, when the server does not
 * hold it, makes the attachment `Waiting` (not tried again: here the uploader never finishes).
 * A download step emits `AttachmentsChanged` for the attachment's space, every step then
 * `SyncChanged` (the counts of `SyncInfo.transfers` changed); none of these events counts as a
 * local edit. Reads of an incomplete attachment fail with `Storage` ("the attachment is not
 * complete on this device (n of m chunks)").
 */
export class MemoryAttachments {
  /**
   * The fake blob store: chunk id (64 hex characters; a counter here, BLAKE3 of the
   * ciphertext in the core) to the chunk's bytes (plaintext here, the final chunk unpadded).
   */
  readonly blobs = new Map<ChunkId, Uint8Array>();
  readonly #deps: AttachmentDeps;
  readonly #manifests = new Map<AttachmentId, Manifest>();
  readonly #uploads = new Map<AttachmentId, Upload>();
  /** Keyring tombstones of purged attachments (a key shred each), by space. */
  readonly #tombstones = new Map<SpaceId, number>();
  /** The simulated server's chunks of remote attachments (chunk id to bytes). */
  readonly #server = new Map<ChunkId, Uint8Array>();
  #chunkSeq = 0;
  #transferTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(deps: AttachmentDeps) {
    this.#deps = deps;
  }

  /**
   * Creates finished attachments without events, as if they had been there before the app
   * started: unreferenced since now unless the seeded page content shows them.
   */
  seed(seeds: readonly MemoryAttachmentSeed[]): void {
    for (const seed of seeds) {
      this.#seedOne(seed.spaceId ?? this.#deps.structure.spaceIds()[0], seed);
    }
  }

  /**
   * Creates the remote attachments of `MemoryAttachmentsOptions.remoteAttachments` without
   * events: manifests like {@link MemoryAttachments.seed}'s, whose chunks are on the simulated
   * server (nowhere with `waiting`) and not here, wanted or not by the policy read now. Starts
   * the transfer timer when something is due.
   */
  seedRemote(seeds: readonly MemoryRemoteAttachmentSeed[]): void {
    const eager = this.#policy() === DOWNLOAD_POLICY_EAGER;
    for (const seed of seeds) {
      const manifest = this.#seedOne(seed.spaceId ?? this.#deps.structure.spaceIds()[0], seed);
      for (const chunk of manifest.chunks) {
        const bytes = this.blobs.get(chunk);
        this.blobs.delete(chunk);
        if (bytes !== undefined && seed.waiting !== true) {
          this.#server.set(chunk, bytes);
        }
      }
      manifest.local = 0;
      manifest.wanted = eager;
      // Under the eager policy the driver has asked already, and was told to wait.
      manifest.unavailable = eager && seed.waiting === true;
    }
    this.#scheduleTransfers();
  }

  // ----- uploads --------------------------------------------------------------------

  /**
   * `AttachmentBegin`: checks the name, the MIME type, the workspace (`InvalidInput` for
   * `workspace`) and the space (`NotFound`), then starts an upload under a new id
   * (`AlreadyExists` should that id be taken). No event: nothing is listed until `finish`.
   */
  begin(spaceId: SpaceId, name: string, mime: string): AttachmentId {
    return this.#start(spaceId, name, mime, undefined);
  }

  /**
   * `attachment_write`: appends plaintext to an upload and stores every chunk that completes.
   * `NotFound` for an unknown upload, `InvalidInput` for `size` (with `details.limit`) when
   * the upload would pass `MAX_ATTACHMENT_BYTES` (nothing of the piece is taken).
   */
  write(attachmentId: AttachmentId, bytes: Uint8Array): void {
    const upload = this.#uploads.get(attachmentId);
    if (upload === undefined) {
      throw noUpload();
    }
    if (upload.written + bytes.length > MAX_ATTACHMENT_BYTES) {
      throw sizeLimitError();
    }
    const pending = new Uint8Array(upload.pending.length + bytes.length);
    pending.set(upload.pending);
    pending.set(bytes, upload.pending.length);
    upload.written += bytes.length;
    let start = 0;
    while (pending.length - start >= CHUNK_SIZE) {
      upload.chunks.push(this.#store(pending.slice(start, start + CHUNK_SIZE)));
      start += CHUNK_SIZE;
    }
    upload.pending = start === 0 ? pending : pending.slice(start);
  }

  /**
   * `AttachmentFinish`: stores the final chunk (empty when the size is a multiple of the chunk
   * size) and the manifest, unreferenced since now; emits `AttachmentsChanged`. `NotFound` for
   * an unknown upload, or for its space when that was deleted meanwhile (its chunks go).
   */
  finish(attachmentId: AttachmentId): AttachmentInfo {
    const manifest = this.#finish(attachmentId);
    // The core queues every chunk for upload; the mirror counts them while sync is configured.
    if (this.#deps.syncConfigured?.() === true) {
      manifest.toUpload = manifest.chunks.length;
    }
    this.#changed(manifest.spaceId);
    this.#scheduleTransfers();
    return this.#info(manifest, this.#deps.content.attachmentReferenceCounts());
  }

  /** `AttachmentAbort`: drops the upload and its chunks; emits `AttachmentsChanged`. `NotFound` for an unknown upload. */
  abort(attachmentId: AttachmentId): void {
    const upload = this.#uploads.get(attachmentId);
    if (upload === undefined) {
      throw noUpload();
    }
    this.#uploads.delete(attachmentId);
    this.#drop(upload.chunks);
    this.#changed(upload.spaceId);
  }

  // ----- requests ---------------------------------------------------------------------

  /** `AttachmentList`: the space's attachments, newest first, then by id (`NotFound` for an unknown space). */
  list(spaceId: SpaceId): AttachmentInfo[] {
    if (this.#deps.structure.spacePageIds(spaceId) === undefined) {
      throw spaceNotFound();
    }
    const counts = this.#deps.content.attachmentReferenceCounts();
    return [...this.#manifests.values()]
      .filter((manifest) => manifest.spaceId === spaceId)
      .sort((a, b) => b.createdAt - a.createdAt || compareStrings(a.attachmentId, b.attachmentId))
      .map((manifest) => this.#info(manifest, counts));
  }

  /** `AttachmentGet`: `NotFound` for unknown and purged attachments and for uploads in progress. */
  get(attachmentId: AttachmentId): AttachmentInfo {
    return this.#info(this.#manifest(attachmentId), this.#deps.content.attachmentReferenceCounts());
  }

  /** `AttachmentDelete`: purges now, whatever references it (blocks keep their node); emits `AttachmentsChanged`. */
  delete(attachmentId: AttachmentId): void {
    this.#purge(this.#manifest(attachmentId));
  }

  /**
   * `AttachmentGc`, also run at every open: purges the attachments unreferenced since before
   * `now - retention` (the setting `RETENTION_SETTING` read as a number of days, fractions
   * allowed; `DEFAULT_RETENTION_DAYS` when absent or not a number; `<= 0` keeps everything),
   * longest unreferenced first, each emitting `AttachmentsChanged`; then deletes the chunks no
   * manifest and no upload in progress claims.
   */
  gc(): GcSummary {
    const summary: GcSummary = { purged: 0, chunksDeleted: 0, bytesFreed: 0 };
    const retention = this.#retentionMs();
    const candidates = [...this.#manifests.values()].some((manifest) => manifest.unreferencedSince !== undefined);
    // The clock is read only when something could expire (test clocks count their reads).
    if (retention !== undefined && candidates) {
      const cutoff = this.#deps.now() - retention;
      const expired = [...this.#manifests.values()]
        .filter((manifest) => manifest.unreferencedSince !== undefined && manifest.unreferencedSince < cutoff)
        .sort(
          (a, b) =>
            (a.unreferencedSince ?? 0) - (b.unreferencedSince ?? 0) || compareStrings(a.attachmentId, b.attachmentId),
        );
      for (const manifest of expired) {
        const freed = this.#purge(manifest);
        summary.purged += 1;
        summary.chunksDeleted += manifest.chunks.length;
        summary.bytesFreed += freed;
      }
    }
    const claimed = new Set<ChunkId>();
    for (const owner of [...this.#manifests.values(), ...this.#uploads.values()]) {
      owner.chunks.forEach((chunk) => claimed.add(chunk));
    }
    const orphans = [...this.blobs.keys()].filter((chunk) => !claimed.has(chunk));
    summary.chunksDeleted += orphans.length;
    summary.bytesFreed += this.#drop(orphans);
    return summary;
  }

  /**
   * The core's `attachment_read`: the plaintext, whole or the half-open `range`
   * (`start <= end <= size`, else `InvalidInput` for `range`), read from the chunks the range
   * needs. `NotFound` for an unknown or purged attachment. `Storage` for an attachment that is
   * not complete here, checked first ("the attachment is not complete on this device (n of m
   * chunks)"; a read of an attachment nobody asked for, under the on-demand policy, also makes
   * its chunks wanted like `AttachmentDownload`: `AttachmentsChanged`, `SyncRequested`).
   * `Storage` for a chunk missing from `blobs` (the first 8 hex characters of its id), `Crypto`
   * for chunks that cannot back the manifest's size.
   */
  read(attachmentId: AttachmentId, range: ByteRange | undefined): Uint8Array {
    const manifest = this.#manifest(attachmentId);
    if (missing(manifest) > 0) {
      if (this.#want([manifest])) {
        this.#deps.requestSync?.();
      }
      throw incomplete(manifest);
    }
    const { start, end } = range === undefined ? { start: 0, end: manifest.size } : validateRange(range, manifest.size);
    const out = new Uint8Array(end - start);
    const last = manifest.chunks.length - 1;
    const first = Math.floor(start / CHUNK_SIZE);
    const lastNeeded = end === 0 ? 0 : Math.floor((end - 1) / CHUNK_SIZE);
    let delivered = 0;
    for (let index = first; index <= Math.min(lastNeeded, last); index += 1) {
      const chunk = manifest.chunks[index];
      if (chunk === undefined) {
        throw damaged();
      }
      const bytes = this.blobs.get(chunk);
      if (bytes === undefined) {
        throw missingChunk(chunk);
      }
      const chunkStart = index * CHUNK_SIZE;
      if (chunkStart + bytes.length > manifest.size || (index !== last && bytes.length !== CHUNK_SIZE)) {
        throw damaged();
      }
      const from = Math.min(Math.max(start - chunkStart, 0), bytes.length);
      const to = Math.min(Math.max(end - chunkStart, 0), bytes.length);
      if (from < to) {
        out.set(bytes.subarray(from, to), delivered);
        delivered += to - from;
      }
    }
    // The chunks must back every byte the manifest promises in the range.
    if (delivered !== end - start) {
      throw damaged();
    }
    return out;
  }

  // ----- hooks of the other mirrors ---------------------------------------------------

  /** The name of a finished attachment (`file` blocks in Markdown); `undefined` once purged. */
  nameOf(attachmentId: AttachmentId): string | undefined {
    return this.#manifests.get(attachmentId)?.name;
  }

  /**
   * The space and the MIME type of a finished attachment; `undefined` for an unknown one and for
   * one that was purged (its manifest is gone). What `PageSetIcon` checks an image icon against.
   */
  manifestOf(attachmentId: AttachmentId): { spaceId: SpaceId; mime: string } | undefined {
    const manifest = this.#manifests.get(attachmentId);
    return manifest === undefined ? undefined : { spaceId: manifest.spaceId, mime: manifest.mime };
  }

  /**
   * The core's reference bookkeeping after references may have changed (`touched`: the
   * attachments involved; every attachment when left out): an attachment that nothing
   * references (documents of live and trashed pages count, purged pages do not) and that is
   * not marked yet becomes unreferenced since now; a marked one that is referenced again is
   * cleared. Emits `AttachmentsChanged` once per space with a change, in space id order.
   */
  refresh(touched?: Iterable<AttachmentId>): void {
    const counts = this.#deps.content.attachmentReferenceCounts();
    const ids = touched === undefined ? [...this.#manifests.keys()] : [...new Set(touched)];
    const spaces = new Set<SpaceId>();
    let now: number | undefined;
    for (const attachmentId of ids) {
      const manifest = this.#manifests.get(attachmentId);
      if (manifest === undefined) {
        continue;
      }
      const referenced = (counts.get(attachmentId) ?? 0) > 0;
      if (!referenced && manifest.unreferencedSince === undefined) {
        now ??= this.#deps.now();
        manifest.unreferencedSince = now;
        spaces.add(manifest.spaceId);
      } else if (referenced && manifest.unreferencedSince !== undefined) {
        manifest.unreferencedSince = undefined;
        spaces.add(manifest.spaceId);
      }
    }
    for (const spaceId of [...spaces].sort(compareStrings)) {
      this.#changed(spaceId);
    }
  }

  /**
   * A move to another space (ADR-0038): the attachments of `attachmentIds` that belong to `fromSpaceId`
   * become the target's (their manifests and index rows; the chunks stay where they are). Emits
   * `AttachmentsChanged` for the source then the target when any moved.
   */
  moveToSpace(attachmentIds: readonly AttachmentId[], fromSpaceId: SpaceId, toSpaceId: SpaceId): void {
    let moved = 0;
    for (const attachmentId of attachmentIds) {
      const manifest = this.#manifests.get(attachmentId);
      if (manifest?.spaceId === fromSpaceId) {
        manifest.spaceId = toSpaceId;
        moved += 1;
      }
    }
    if (moved > 0) {
      this.#changed(fromSpaceId);
      this.#changed(toSpaceId);
    }
  }

  /**
   * `SpaceDelete`, before the space goes: every attachment of the space is purged (an
   * `AttachmentsChanged` each), then the space's keyring rows go, tombstones included.
   */
  spaceDeleted(spaceId: SpaceId): void {
    const manifests = [...this.#manifests.values()]
      .filter((manifest) => manifest.spaceId === spaceId)
      .sort((a, b) => b.createdAt - a.createdAt || compareStrings(a.attachmentId, b.attachmentId));
    for (const manifest of manifests) {
      this.#purge(manifest);
    }
    this.#tombstones.delete(spaceId);
  }

  /** Keyring counts: one attachment key per finished attachment, one tombstone per purged one. */
  keyring(): { keys: number; tombstones: number } {
    let tombstones = 0;
    for (const count of this.#tombstones.values()) {
      tombstones += count;
    }
    return { keys: this.#manifests.size, tombstones };
  }

  // ----- attachment sync (Milestone 8, ADR-0023) --------------------------------------

  /**
   * `AttachmentDownload`: the chunks of an incomplete attachment are wanted now, whatever the
   * policy (`NotDownloaded` becomes `Downloading` and emits `AttachmentsChanged`; a `Waiting` or
   * `Downloading` one stays as it is), and the driver is poked (`SyncRequested`). A complete
   * attachment answers its info unchanged, without events. `NotFound` for unknown and purged
   * attachments.
   */
  download(attachmentId: AttachmentId): AttachmentInfo {
    const manifest = this.#manifest(attachmentId);
    if (missing(manifest) > 0) {
      this.#want([manifest]);
      this.#deps.requestSync?.();
    }
    return this.#info(manifest, this.#deps.content.attachmentReferenceCounts());
  }

  /**
   * A setting changed (`SettingsSet`): when the download policy is now eager, every incomplete
   * attachment nobody asked for becomes wanted (`AttachmentsChanged` once per space with a
   * change, in space id order, then `SyncRequested`), as the core's reconciliation does. Turning
   * the policy to on demand changes nothing: what is wanted stays wanted.
   */
  settingChanged(key: string): void {
    if (key !== DOWNLOAD_POLICY_SETTING || this.#policy() !== DOWNLOAD_POLICY_EAGER) {
      return;
    }
    if (this.#want([...this.#manifests.values()].sort(byAge))) {
      this.#deps.requestSync?.();
    }
  }

  /**
   * Runs up to `count` transfer steps (see the class documentation) and answers how many did
   * something: an upload, a chunk that arrived, or a download the server could not serve; `0`
   * when nothing is due (everything here and uploaded, or not wanted, or waiting).
   */
  advance(count: number): number {
    let done = 0;
    while (done < count && this.#step()) {
      done += 1;
    }
    return done;
  }

  /** `SyncInfo.transfers`: chunks to upload, wanted chunks the server should serve, wanted chunks it did not hold. */
  transfers(): TransferInfo {
    const info: TransferInfo = { uploads: 0, downloads: 0, waiting: 0 };
    for (const manifest of this.#manifests.values()) {
      info.uploads += manifest.toUpload;
      if (manifest.wanted && manifest.unavailable) {
        info.waiting += missing(manifest);
      } else if (manifest.wanted) {
        info.downloads += missing(manifest);
      }
    }
    return info;
  }

  /**
   * Nothing is to upload any more: a healthy cycle of the fake sync driver pushed (the chunks
   * travel in the same cycle), or sync was disabled (the core clears its table).
   */
  uploadsDone(): void {
    for (const manifest of this.#manifests.values()) {
      manifest.toUpload = 0;
    }
  }

  // ----- backups ----------------------------------------------------------------------

  /** Every finished attachment with its chunks, oldest first, and the tombstones (uploads in progress are not backed up). */
  exportState(): AttachmentsState {
    const attachments = [...this.#manifests.values()]
      .sort((a, b) => a.createdAt - b.createdAt || compareStrings(a.attachmentId, b.attachmentId))
      .map(
        (manifest): AttachmentState => ({
          attachmentId: manifest.attachmentId,
          spaceId: manifest.spaceId,
          name: manifest.name,
          mime: manifest.mime,
          size: manifest.size,
          createdAt: manifest.createdAt,
          // A chunk missing from the store is left out, as the core leaves it out of its backup.
          chunks: manifest.chunks.flatMap((id) => {
            const bytes = this.blobs.get(id);
            return bytes === undefined ? [] : [{ id, data: toBase64(bytes) }];
          }),
        }),
      );
    const tombstones = [...this.#tombstones].map(([spaceId, count]) => ({ spaceId, count }));
    return { attachments, tombstones };
  }

  /**
   * Decodes a backup's attachments and returns the function that makes them the only ones,
   * without events and unmarked (the restore derives `unreferencedSince` again through
   * {@link MemoryAttachments.refresh}, like the core's index rebuild). Throws on chunk text that
   * is not base64. Nothing changes until the function is called.
   */
  prepareRestore(state: AttachmentsState): () => void {
    const manifests = new Map<AttachmentId, Manifest>();
    const blobs = new Map<ChunkId, Uint8Array>();
    for (const attachment of state.attachments) {
      for (const { id, data } of attachment.chunks) {
        blobs.set(id, fromBase64(data));
      }
      manifests.set(attachment.attachmentId, {
        attachmentId: attachment.attachmentId,
        spaceId: attachment.spaceId,
        name: attachment.name,
        mime: attachment.mime,
        size: attachment.size,
        chunks: attachment.chunks.map(({ id }) => id),
        createdAt: attachment.createdAt,
        unreferencedSince: undefined,
        local: attachment.chunks.length,
        toUpload: 0,
        wanted: false,
        unavailable: false,
      });
    }
    return () => {
      this.#manifests.clear();
      this.#uploads.clear();
      this.blobs.clear();
      this.#server.clear();
      this.#tombstones.clear();
      for (const [id, manifest] of manifests) {
        this.#manifests.set(id, manifest);
      }
      for (const [id, bytes] of blobs) {
        this.blobs.set(id, bytes);
      }
      for (const { spaceId, count } of state.tombstones) {
        this.#tombstones.set(spaceId, count);
      }
      // Chunk ids of the mirror are a counter: new ones must not meet restored ones.
      for (const id of blobs.keys()) {
        const n = Number.parseInt(id, 16);
        if (Number.isSafeInteger(n)) {
          this.#chunkSeq = Math.max(this.#chunkSeq, n);
        }
      }
    };
  }

  // ----- plumbing ---------------------------------------------------------------------

  /** One seeded attachment, finished, unreferenced since now unless the seeded content shows it. */
  #seedOne(spaceId: SpaceId | undefined, seed: MemoryAttachmentSeed): Manifest {
    if (spaceId === undefined) {
      throw internalError("MemoryTransport: seeded attachments need a space");
    }
    const attachmentId = this.#start(spaceId, seed.name, seed.mime, seed.attachmentId);
    this.write(attachmentId, seed.bytes);
    const manifest = this.#finish(attachmentId);
    if ((this.#deps.content.attachmentReferenceCounts().get(attachmentId) ?? 0) > 0) {
      manifest.unreferencedSince = undefined;
    }
    return manifest;
  }

  #start(spaceId: SpaceId, rawName: string, rawMime: string, fixedId: AttachmentId | undefined): AttachmentId {
    const name = validateAttachmentName(rawName);
    const mime = validateMime(rawMime);
    if (!this.#deps.hasWorkspace()) {
      throw invalid("workspace", "create the workspace first");
    }
    if (this.#deps.structure.spacePageIds(spaceId) === undefined) {
      throw spaceNotFound();
    }
    const createdAt = this.#deps.now();
    const attachmentId = fixedId ?? this.#deps.newId();
    if (this.#manifests.has(attachmentId) || this.#uploads.has(attachmentId)) {
      throw new CoreError({ code: "AlreadyExists", message: "attachment id already in use" });
    }
    this.#uploads.set(attachmentId, { spaceId, name, mime, createdAt, chunks: [], pending: new Uint8Array(), written: 0 });
    return attachmentId;
  }

  #finish(attachmentId: AttachmentId): Manifest {
    const upload = this.#uploads.get(attachmentId);
    if (upload === undefined) {
      throw noUpload();
    }
    this.#uploads.delete(attachmentId);
    const chunks = [...upload.chunks, this.#store(upload.pending)];
    if (this.#deps.structure.spacePageIds(upload.spaceId) === undefined) {
      this.#drop(chunks);
      throw spaceNotFound();
    }
    const manifest: Manifest = {
      attachmentId,
      spaceId: upload.spaceId,
      name: upload.name,
      mime: upload.mime,
      size: upload.written,
      chunks,
      createdAt: upload.createdAt,
      // Nothing references a fresh upload until a block is inserted.
      unreferencedSince: this.#deps.now(),
      local: chunks.length,
      toUpload: 0,
      wanted: false,
      unavailable: false,
    };
    this.#manifests.set(attachmentId, manifest);
    return manifest;
  }

  /** Stores one chunk under a new id; returns the id. */
  #store(bytes: Uint8Array): ChunkId {
    this.#chunkSeq += 1;
    const chunk = this.#chunkSeq.toString(16).padStart(64, "0");
    this.blobs.set(chunk, bytes);
    return chunk;
  }

  /** Deletes chunks; returns the bytes they held (missing ones count nothing). */
  #drop(chunks: readonly ChunkId[]): number {
    let freed = 0;
    for (const chunk of chunks) {
      freed += this.blobs.get(chunk)?.length ?? 0;
      this.blobs.delete(chunk);
    }
    return freed;
  }

  /** Key shred, manifest, chunks, index row; emits `AttachmentsChanged`. Returns the bytes freed. */
  #purge(manifest: Manifest): number {
    this.#tombstones.set(manifest.spaceId, (this.#tombstones.get(manifest.spaceId) ?? 0) + 1);
    this.#manifests.delete(manifest.attachmentId);
    const freed = this.#drop(manifest.chunks);
    // The core queues a delete of every chunk on the server; the simulated server drops them now.
    for (const chunk of manifest.chunks) {
      this.#server.delete(chunk);
    }
    this.#changed(manifest.spaceId);
    return freed;
  }

  #manifest(attachmentId: AttachmentId): Manifest {
    const manifest = this.#manifests.get(attachmentId);
    if (manifest === undefined) {
      throw notFound();
    }
    return manifest;
  }

  /** The retention in milliseconds; `undefined` keeps everything. */
  #retentionMs(): number | undefined {
    const value = this.#deps.setting(RETENTION_SETTING);
    const days = typeof value === "number" ? value : DEFAULT_RETENTION_DAYS;
    return days <= 0 || !Number.isFinite(days) ? undefined : Math.trunc(days * MS_PER_DAY);
  }

  #changed(spaceId: SpaceId): void {
    this.#deps.emit({ type: "AttachmentsChanged", data: { spaceId } });
  }

  /** Delivers an event of a transfer (not a local edit). */
  #transfer(event: Event): void {
    (this.#deps.emitTransfer ?? this.#deps.emit)(event);
  }

  /** The download policy, read from its setting. */
  #policy(): DownloadPolicy {
    return downloadPolicyOf(this.#deps.setting(DOWNLOAD_POLICY_SETTING));
  }

  #availability(manifest: Manifest): AttachmentAvailability {
    if (missing(manifest) === 0) {
      return "Complete";
    }
    if (!manifest.wanted) {
      return "NotDownloaded";
    }
    return manifest.unavailable ? "Waiting" : "Downloading";
  }

  /**
   * Makes the missing chunks of `manifests` wanted where nobody asked for them yet: emits
   * `AttachmentsChanged` once per space with a change (space id order) and starts the transfer
   * timer. Answers whether anything changed.
   */
  #want(manifests: readonly Manifest[]): boolean {
    const spaces = new Set<SpaceId>();
    for (const manifest of manifests) {
      if (!manifest.wanted && missing(manifest) > 0) {
        manifest.wanted = true;
        spaces.add(manifest.spaceId);
      }
    }
    for (const spaceId of [...spaces].sort(compareStrings)) {
      this.#transfer({ type: "AttachmentsChanged", data: { spaceId } });
    }
    this.#scheduleTransfers();
    return spaces.size > 0;
  }

  /** The attachment whose chunk a transfer step uploads next, if any. */
  #nextUpload(): Manifest | undefined {
    return [...this.#manifests.values()].filter((manifest) => manifest.toUpload > 0).sort(byAge)[0];
  }

  /** The attachment whose chunk a transfer step downloads next, if any (waiting ones are not due). */
  #nextDownload(): Manifest | undefined {
    return [...this.#manifests.values()]
      .filter((manifest) => manifest.wanted && !manifest.unavailable && missing(manifest) > 0)
      .sort(byAge)[0];
  }

  /** One transfer step; answers whether it did something. */
  #step(): boolean {
    const upload = this.#nextUpload();
    if (upload !== undefined) {
      upload.toUpload -= 1;
      this.#transfer({ type: "SyncChanged" });
      return true;
    }
    const download = this.#nextDownload();
    if (download === undefined) {
      return false;
    }
    const chunk = download.chunks[download.local];
    const bytes = chunk === undefined ? undefined : this.#server.get(chunk);
    if (chunk === undefined || bytes === undefined) {
      download.unavailable = true;
    } else {
      this.blobs.set(chunk, bytes);
      download.local += 1;
    }
    this.#transfer({ type: "AttachmentsChanged", data: { spaceId: download.spaceId } });
    this.#transfer({ type: "SyncChanged" });
    return true;
  }

  /** Starts the transfer timer (only with `transferStepMs`) unless it runs already or nothing is due. */
  #scheduleTransfers(): void {
    const stepMs = this.#deps.transferStepMs;
    if (stepMs === undefined || this.#transferTimer !== undefined) {
      return;
    }
    if (this.#nextUpload() === undefined && this.#nextDownload() === undefined) {
      return;
    }
    this.#transferTimer = setTimeout(() => {
      this.#transferTimer = undefined;
      if (this.#deps.ready?.() ?? true) {
        this.#step();
      }
      this.#scheduleTransfers();
    }, stepMs);
  }

  #info(manifest: Manifest, counts: ReadonlyMap<AttachmentId, number>): AttachmentInfo {
    // Optional fields are left out when unset, as the core's serialisation does.
    const info: AttachmentInfo = {
      attachmentId: manifest.attachmentId,
      spaceId: manifest.spaceId,
      name: manifest.name,
      mime: manifest.mime,
      size: manifest.size,
      chunkCount: manifest.chunks.length,
      createdAt: manifest.createdAt,
      references: counts.get(manifest.attachmentId) ?? 0,
      localChunks: manifest.local,
      availability: this.#availability(manifest),
    };
    if (manifest.unreferencedSince !== undefined) {
      info.unreferencedSince = manifest.unreferencedSince;
    }
    return info;
  }
}

/**
 * The attachment id of a raw IPC command, from its `x-kb-attachment` header as the shell
 * parses it: anything but UUID text is `InvalidInput` for the field `x-kb-attachment`.
 */
export function attachmentHeader(attachmentId: string): AttachmentId {
  if (canonicalId(attachmentId) === undefined) {
    throw invalid("x-kb-attachment", "the attachment id must be UUID text");
  }
  return attachmentId;
}
