// The editor's replica of an open page document and its sync loop with the core
// (`docs/sync/crdt-strategy.md`, "Two replicas of one document"; milestone-03.md, "UI").
//
// open:    `docs.open` -> snapshot -> new LoroDoc -> import; the editor mounts on it.
// local:   every local Loro commit schedules a flush; a flush runs at most every 250 ms
//          (and on blur, page change, unmount and `pagehide`) and sends
//          `export({ mode: "update", from: lastAcked })` with `docs.import`. `lastAcked`
//          only advances when the core acknowledged, so a failed flush is retried.
// remote:  `PageContentChanged` from anyone but the editor -> `docs.export(since: our
//          version)` -> import, which the binding applies as a remote change (never on
//          the undo stack).
// close:   flush, then `docs.close`.

import { configurePageDoc, toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, CoreError, Event as CoreEvent, PageId } from "@atomik-kb/core-client";
import { LoroDoc, VersionVector } from "loro-crdt";
import type { PeerID } from "loro-crdt";

/** Local changes are sent at most this often. */
export const FLUSH_INTERVAL_MS = 250;

/** From this many consecutive failed flushes on, the editor says that changes are not saved. */
export const FAILED_FLUSHES_NOTICE = 3;

/** Waits before retrying a failed flush on its own (the last one repeats). */
const RETRY_DELAYS_MS = [1000, 2000, 5000, 10_000];

export type PageDocStatus = "opening" | "ready" | "failed";

export interface PageDocSnapshot {
  status: PageDocStatus;
  /** The editor's replica; set once `status` is `"ready"`. */
  doc: LoroDoc | null;
  /** Why opening failed. */
  error: CoreError | null;
  /** Consecutive flushes the core did not acknowledge (0 when everything is saved or pending). */
  failedFlushes: number;
}

export interface PageDocSessionOptions {
  /** See {@link FLUSH_INTERVAL_MS}. */
  flushIntervalMs?: number;
  /** See `RETRY_DELAYS_MS`. */
  retryDelaysMs?: readonly number[];
}

const OPENING: PageDocSnapshot = { status: "opening", doc: null, error: null, failedFlushes: 0 };

/** `a` and `b` merged: for every peer the larger counter. */
function mergeVersions(a: VersionVector, b: VersionVector): VersionVector {
  const merged = a.toJSON();
  for (const [peer, counter] of b.toJSON()) {
    merged.set(peer, Math.max(merged.get(peer) ?? 0, counter));
  }
  return new VersionVector(merged);
}

/** `acked` with every peer but `local` raised to `version` (what came from the core is acknowledged). */
function withRemotePeers(acked: VersionVector, version: VersionVector, local: PeerID): VersionVector {
  const merged = acked.toJSON();
  for (const [peer, counter] of version.toJSON()) {
    if (peer !== local) {
      merged.set(peer, Math.max(merged.get(peer) ?? 0, counter));
    }
  }
  return new VersionVector(merged);
}

/** One open page: its `LoroDoc`, kept in sync with the core. */
export class PageDocSession {
  readonly pageId: PageId;
  readonly #client: CoreClient;
  readonly #flushIntervalMs: number;
  readonly #retryDelaysMs: readonly number[];
  readonly #listeners = new Set<() => void>();
  #snapshot: PageDocSnapshot = OPENING;
  #doc: LoroDoc | null = null;
  /** What the core has acknowledged of this replica (and has sent it). */
  #lastAcked: VersionVector | null = null;
  #flushTimer: ReturnType<typeof setTimeout> | null = null;
  #retryTimer: ReturnType<typeof setTimeout> | null = null;
  #lastFlushAt = Number.NEGATIVE_INFINITY;
  #flushing: Promise<void> | null = null;
  #flushAgain = false;
  #pulling = false;
  #pullAgain = false;
  /** A remote change was announced before the document was open. */
  #pullAfterOpen = false;
  #unsubscribeLocal: (() => void) | null = null;
  #closed = false;
  readonly #unsubscribeEvents: () => void;
  readonly #onPageHide = (): void => {
    void this.flush();
  };

  constructor(client: CoreClient, pageId: PageId, options: PageDocSessionOptions = {}) {
    this.#client = client;
    this.pageId = pageId;
    this.#flushIntervalMs = options.flushIntervalMs ?? FLUSH_INTERVAL_MS;
    this.#retryDelaysMs = options.retryDelaysMs ?? RETRY_DELAYS_MS;
    // Listen first: a change announced while the document opens must not be missed.
    this.#unsubscribeEvents = client.subscribe((event) => {
      this.#onEvent(event);
    });
    void this.#open();
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): PageDocSnapshot => this.#snapshot;

  /** Tries to open the document again after a failure. */
  retry(): void {
    if (this.#snapshot.status === "failed" && !this.#closed) {
      this.#set(OPENING);
      void this.#open();
    }
  }

  /**
   * Sends every local change the core has not acknowledged yet, and resolves when that is
   * done (also when it failed: failures are counted in the snapshot and retried). Calls
   * while a flush runs make it run once more and share its promise.
   */
  flush(): Promise<void> {
    this.#clearFlushTimer();
    if (this.#flushing !== null) {
      this.#flushAgain = true;
      return this.#flushing;
    }
    const running = this.#runFlush().finally(() => {
      this.#flushing = null;
    });
    this.#flushing = running;
    return running;
  }

  /** Sends what is left, then closes the document in the core. Never rejects. */
  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    await this.flush();
    this.#stop();
    if (this.#doc !== null) {
      try {
        await this.#client.docs.close(this.pageId);
      } catch (error) {
        console.error("could not close the page document", toCoreError(error).code);
      }
    }
  }

  /**
   * Stops at once, sends nothing and drops the replica: for when the core closed every
   * document itself (the app was locked). Changes not flushed before are lost.
   */
  discard(): void {
    if (this.#closed) {
      return;
    }
    this.#stop();
    this.#doc = null;
    this.#lastAcked = null;
  }

  #stop(): void {
    this.#closed = true;
    this.#clearFlushTimer();
    this.#clearRetryTimer();
    this.#unsubscribeEvents();
    this.#unsubscribeLocal?.();
    this.#unsubscribeLocal = null;
    window.removeEventListener("pagehide", this.#onPageHide);
  }

  async #open(): Promise<void> {
    let snapshot: Uint8Array;
    try {
      snapshot = await this.#client.docs.open(this.pageId);
    } catch (error) {
      if (!this.#closed) {
        this.#set({ status: "failed", doc: null, error: toCoreError(error), failedFlushes: 0 });
      }
      return;
    }
    if (this.#closed) {
      // Released while opening: the core has it open now, so close it again.
      this.#client.docs.close(this.pageId).catch((error: unknown) => {
        console.error("could not close the page document", toCoreError(error).code);
      });
      return;
    }
    const doc = new LoroDoc();
    configurePageDoc(doc); // the mark table of shape v1, exactly like the core's replica
    try {
      doc.import(snapshot);
    } catch (error) {
      this.#set({ status: "failed", doc: null, error: toCoreError(error), failedFlushes: 0 });
      return;
    }
    this.#doc = doc;
    this.#lastAcked = doc.oplogVersion();
    this.#unsubscribeLocal = doc.subscribeLocalUpdates(() => {
      this.#scheduleFlush();
    });
    window.addEventListener("pagehide", this.#onPageHide);
    this.#set({ status: "ready", doc, error: null, failedFlushes: 0 });
    if (this.#pullAfterOpen) {
      this.#pullAfterOpen = false;
      void this.#pull();
    }
  }

  #onEvent(event: CoreEvent): void {
    if (
      event.type !== "PageContentChanged" ||
      event.data.pageId !== this.pageId ||
      event.data.source === "Editor" ||
      this.#closed
    ) {
      return;
    }
    if (this.#doc === null) {
      this.#pullAfterOpen = true;
      return;
    }
    void this.#pull();
  }

  /** Imports what the core has and this replica lacks. */
  async #pull(): Promise<void> {
    if (this.#pulling) {
      this.#pullAgain = true;
      return;
    }
    this.#pulling = true;
    try {
      do {
        this.#pullAgain = false;
        const doc = this.#doc;
        if (doc === null || this.#closed) {
          return;
        }
        const bytes = await this.#client.docs.export(this.pageId, doc.oplogVersion().encode());
        if (this.#closed || bytes.length === 0) {
          continue;
        }
        doc.import(bytes);
        if (this.#lastAcked !== null) {
          this.#lastAcked = withRemotePeers(this.#lastAcked, doc.oplogVersion(), doc.peerIdStr);
        }
      } while (this.#pullAgain);
    } catch (error) {
      console.error("could not load a change to the page", toCoreError(error).code);
    } finally {
      this.#pulling = false;
    }
  }

  #scheduleFlush(): void {
    if (this.#closed || this.#flushTimer !== null) {
      return;
    }
    const wait = Math.max(0, this.#lastFlushAt + this.#flushIntervalMs - Date.now());
    this.#flushTimer = setTimeout(() => {
      this.#flushTimer = null;
      void this.flush();
    }, wait);
  }

  async #runFlush(): Promise<void> {
    do {
      this.#flushAgain = false;
      const doc = this.#doc;
      const acked = this.#lastAcked;
      if (doc === null || acked === null) {
        return;
      }
      const version = doc.oplogVersion();
      if (version.compare(acked) === 0) {
        return; // nothing new
      }
      const update = doc.export({ mode: "update", from: acked });
      this.#lastFlushAt = Date.now();
      try {
        await this.#client.docs.import(this.pageId, update);
      } catch (error) {
        console.error("could not save changes to the page", toCoreError(error).code);
        this.#set({ ...this.#snapshot, failedFlushes: this.#snapshot.failedFlushes + 1 });
        this.#scheduleRetry();
        return;
      }
      this.#lastAcked = mergeVersions(this.#lastAcked ?? acked, version);
      this.#clearRetryTimer();
      if (this.#snapshot.failedFlushes !== 0) {
        this.#set({ ...this.#snapshot, failedFlushes: 0 });
      }
    } while (this.#flushAgain);
  }

  #scheduleRetry(): void {
    if (this.#closed || this.#retryTimer !== null) {
      return;
    }
    const index = Math.min(this.#snapshot.failedFlushes, this.#retryDelaysMs.length) - 1;
    const delay = this.#retryDelaysMs[Math.max(0, index)] ?? 1000;
    this.#retryTimer = setTimeout(() => {
      this.#retryTimer = null;
      void this.flush();
    }, delay);
  }

  #clearFlushTimer(): void {
    if (this.#flushTimer !== null) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
  }

  #clearRetryTimer(): void {
    if (this.#retryTimer !== null) {
      clearTimeout(this.#retryTimer);
      this.#retryTimer = null;
    }
  }

  #set(snapshot: PageDocSnapshot): void {
    this.#snapshot = snapshot;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

interface SessionEntry {
  session: PageDocSession;
  users: number;
  closing: ReturnType<typeof setTimeout> | null;
}

/**
 * The open page documents of one app, shared by whoever shows a page. A session closes
 * shortly after its last user let go, so React's StrictMode remount and a quick switch
 * back to the page reuse it instead of reopening.
 */
export class PageDocSessions {
  readonly #client: CoreClient;
  readonly #options: PageDocSessionOptions;
  readonly #sessions = new Map<PageId, SessionEntry>();
  readonly #listeners = new Set<() => void>();

  constructor(client: CoreClient, options: PageDocSessionOptions = {}) {
    this.#client = client;
    this.#options = options;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The session of a page, if one is open (or closing); for rendering. */
  peek(pageId: PageId): PageDocSession | null {
    return this.#sessions.get(pageId)?.session ?? null;
  }

  /** Opens (or reuses) the session of a page; give it back with {@link release}. */
  acquire(pageId: PageId): PageDocSession {
    const entry = this.#sessions.get(pageId);
    if (entry !== undefined) {
      entry.users += 1;
      if (entry.closing !== null) {
        clearTimeout(entry.closing);
        entry.closing = null;
      }
      return entry.session;
    }
    const session = new PageDocSession(this.#client, pageId, this.#options);
    this.#sessions.set(pageId, { session, users: 1, closing: null });
    this.#notify();
    return session;
  }

  /** Lets go of a session: it flushes now and closes unless someone takes it again. */
  release(session: PageDocSession): void {
    const entry = this.#sessions.get(session.pageId);
    if (entry?.session !== session) {
      return;
    }
    entry.users -= 1;
    if (entry.users > 0) {
      return;
    }
    void session.flush();
    entry.closing = setTimeout(() => {
      entry.closing = null;
      if (this.#sessions.get(session.pageId) === entry && entry.users === 0) {
        this.#sessions.delete(session.pageId);
        this.#notify();
        void session.close();
      }
    }, 0);
  }

  /** Sends every pending change of every open page. */
  flushAll(): Promise<void> {
    return Promise.all([...this.#sessions.values()].map((entry) => entry.session.flush())).then(() => undefined);
  }

  /**
   * Drops every session without involving the core, which has closed the documents itself
   * (the app was locked; see {@link PageDocSession.discard}). Flush first.
   */
  discardAll(): void {
    if (this.#sessions.size === 0) {
      return;
    }
    for (const entry of this.#sessions.values()) {
      if (entry.closing !== null) {
        clearTimeout(entry.closing);
      }
      entry.session.discard();
    }
    this.#sessions.clear();
    this.#notify();
  }

  #notify(): void {
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}
