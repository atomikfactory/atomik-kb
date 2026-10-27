// Page history in the in-memory core (ADR-0057, `docs/architecture/page-history.md` section 6): a journal of editing sessions
// kept in memory, the versions of a page from the journal and the document's history, the content at a version, restore.
//
// What it mirrors of the core (`kb-core`'s `history.rs`), with the reduced fidelity of the twin:
//
// - **A version** is a state of a page's document: its Loro frontiers, each `ID(peer, counter)` written `"<peer>@<counter>"`
//   with the peer in decimal, sorted by peer (numerically), joined with `,` (`versionId`). The same text on every replica that has
//   those changes. An id that does not parse, or names changes the document does not have, is `InvalidInput`.
// - **The journal** has one row per editing session of a page, written at every write of the memory transport to a page
//   document (`documentWritten` of `./content`: the editor's updates, the core's own writes, and changes that arrive from elsewhere
//   through `MemoryTransport.changePage`): a write continues the latest row when it is an edit by the same author at most
//   ten minutes after the row's end (`SESSION_GAP_MS`), else it starts a row; a restore always starts one. The author of a local
//   write is this device (its device id, this account's id when it has a kit); a `"Remote"` change has no author here (the
//   memory transport has no sync entries to read one from), so it is a row of its own without one. Times are the transport's
//   clock, the title is the structure's at the time. The journal is nobody's data: not in a backup, not synced.
// - **The list** is the journal's rows newest first (dated), then the older history as undated versions: the runs of changes
//   (consecutive changes of one peer, in lamport order) from before the first journal row's write, newest first. The twin knows
//   where the first row's session began (its `before`, the version vector from before the write), so a run that the first
//   session shares a peer with is still cut at that point.
// - **Content at a version** forks the document there; **restore** reverts the memory document to it as one new change, records
//   a `restore` row and renames the page when the version's title is known and differs. Events, held back until everything is
//   done: `PageContentChanged` (source `Core`), `BacklinksChanged`, `StructureChanged` on a rename.

import type { LoroDoc, OpId } from "loro-crdt";

import { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { ChangeSource } from "./generated/ChangeSource";
import type { DeviceId } from "./generated/DeviceId";
import type { PageHistory } from "./generated/PageHistory";
import type { PageId } from "./generated/PageId";
import type { PageInfo } from "./generated/PageInfo";
import type { PageVersion } from "./generated/PageVersion";
import type { PageVersionContent } from "./generated/PageVersionContent";
import type { PageVersionKind } from "./generated/PageVersionKind";
import type { VersionAuthor } from "./generated/VersionAuthor";
import type { MemoryContent } from "./content";
import type { MemoryStructure } from "./structure";
import { invalid } from "./validate";

/** A write continues the journal's latest row when it comes at most this long after the row's end (ms). */
export const SESSION_GAP_MS = 600_000;
/** Versions in an answer when `limit` is absent. */
const DEFAULT_LIMIT = 100;
/** Most versions in an answer. */
const MAX_LIMIT = 500;
/** Most ids in one version id (a document rarely has more than a couple of heads). */
const MAX_HEADS = 256;
/** `<peer>@<counter>`: the peer is a `u64` in decimal, the counter an `i32`. */
const OP_ID = /^(\d{1,20})@(\d{1,10})$/u;
const MAX_PEER = 2n ** 64n - 1n;
const MAX_COUNTER = 2 ** 31 - 1;

/** What the history needs from its owner. */
export interface HistoryDeps {
  /** Clock in Unix milliseconds: the times of the journal. */
  now: () => number;
  /** Runs `run`, then delivers the events it caused once each (in order of first occurrence). */
  collect: <T>(run: () => T) => T;
  /** This device and this account (`undefined` before the recovery kit): the author of a local write. */
  device: () => { deviceId: DeviceId; accountId: AccountId | undefined };
  structure: MemoryStructure;
  content: MemoryContent;
}

/** One row of the journal: an editing session, or a restore. */
interface JournalRow {
  kind: PageVersionKind;
  startedAt: number;
  endedAt: number;
  /** The document's version at the session's end: {@link versionId} of `frontiers`. */
  version: string;
  frontiers: readonly OpId[];
  /** The document's version vector from before the session's first write (peer to end counter). */
  before: ReadonlyMap<string, number>;
  /** `undefined` when nobody is known. */
  author: VersionAuthor | undefined;
  /** The page's title at the end of the session, when it had one. */
  title: string | undefined;
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

function compareBig(a: bigint, b: bigint): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** The id of a version (spec 2.1): `"<peer>@<counter>"` each, peers in decimal sorted numerically, joined with `,`. */
export function versionId(frontiers: readonly OpId[]): string {
  return frontiers
    .map(({ peer, counter }) => ({ peer: BigInt(peer), counter }))
    .sort((a, b) => compareBig(a.peer, b.peer) || a.counter - b.counter)
    .map(({ peer, counter }) => `${peer.toString()}@${String(counter)}`)
    .join(",");
}

/** `limit` is a `u32` in the contract: 100 by default, clamped to 1..=500. */
function historyLimit(raw: number | undefined): number {
  if (raw === undefined) {
    return DEFAULT_LIMIT;
  }
  if (!Number.isInteger(raw) || raw < 0) {
    throw invalid("limit", "must be a non-negative integer");
  }
  return Math.min(MAX_LIMIT, Math.max(1, raw));
}

/**
 * The version a text names in `doc`: its frontiers (reduced to the minimal ones) and its canonical id. `InvalidInput` on
 * `field` when the text does not parse or names changes the document does not have.
 */
function parseVersion(doc: LoroDoc, text: string, field: string): { frontiers: OpId[]; id: string } {
  const raw: unknown = text;
  const parts = typeof raw === "string" && raw !== "" ? raw.split(",") : [];
  if (parts.length === 0 || parts.length > MAX_HEADS) {
    throw invalid(field, "is not a version id");
  }
  const known = doc.oplogVersion();
  const ids: OpId[] = [];
  for (const part of parts) {
    const match = OP_ID.exec(part);
    const peer = match?.[1] === undefined ? undefined : BigInt(match[1]);
    const counter = match?.[2] === undefined ? undefined : Number(match[2]);
    if (peer === undefined || counter === undefined || peer > MAX_PEER || counter > MAX_COUNTER) {
      throw invalid(field, "is not a version id");
    }
    const end = known.get(peer.toString() as `${number}`);
    if (end === undefined || counter >= end) {
      throw invalid(field, "names changes the page does not have");
    }
    ids.push({ peer: peer.toString() as `${number}`, counter });
  }
  try {
    const frontiers = doc.vvToFrontiers(doc.frontiersToVV(ids));
    return { frontiers, id: versionId(frontiers) };
  } catch {
    throw invalid(field, "names changes the page does not have");
  }
}

/**
 * The undated versions of a document (spec 2.3): the runs of its changes that lie within `before` (a version vector, peer to
 * end counter: the history from before the first journal row's session), each run being consecutive changes of one peer in
 * lamport order, cut at `before`. Answers the run's version ids newest first (the highest lamport first): the version after a
 * run is its last id, `[peer@counter]`.
 */
export function undatedVersions(doc: LoroDoc, before: ReadonlyMap<string, number>): string[] {
  const pieces: { peer: string; lamport: number; last: number }[] = [];
  for (const [peer, changes] of doc.getAllChanges()) {
    const end = before.get(peer) ?? 0;
    for (const change of changes) {
      const length = Math.min(change.length, end - change.counter);
      if (length > 0) {
        pieces.push({ peer, lamport: change.lamport, last: change.counter + length - 1 });
      }
    }
  }
  pieces.sort((a, b) => a.lamport - b.lamport || compareBig(BigInt(a.peer), BigInt(b.peer)));
  const runs: { peer: string; last: number }[] = [];
  for (const piece of pieces) {
    const run = runs.at(-1);
    if (run?.peer === piece.peer) {
      run.last = piece.last;
    } else {
      runs.push({ peer: piece.peer, last: piece.last });
    }
  }
  return runs.reverse().map((run) => `${run.peer}@${String(run.last)}`);
}

/** Who a row names, for the rule that a session has one author: the device, else the account, else nobody. */
function authorKey(author: VersionAuthor | undefined): string {
  return author?.deviceId ?? author?.accountId ?? "";
}

/** The in-memory page history. */
export class MemoryHistory {
  readonly #deps: HistoryDeps;
  /** The journal by page: rows in the order they began. Purged pages keep theirs, like their documents. */
  readonly #rows = new Map<PageId, JournalRow[]>();

  constructor(deps: HistoryDeps) {
    this.#deps = deps;
  }

  /**
   * A write to a page's document (`MemoryContent`'s `documentWritten`): continues the latest row or starts one. A write that
   * left the document's version as the latest row has it (a repeated update) records nothing.
   */
  record(pageId: PageId, source: ChangeSource, before: ReadonlyMap<string, number>): void {
    const doc = this.#deps.content.docOf(pageId);
    if (doc === undefined) {
      return;
    }
    const frontiers = doc.oplogFrontiers();
    const version = versionId(frontiers);
    const rows = this.#rowsOf(pageId);
    const last = rows.at(-1);
    if (last?.version === version) {
      return;
    }
    const now = this.#deps.now();
    const author = this.#author(source);
    const title = this.#deps.structure.pageMeta(pageId)?.title;
    if (
      last !== undefined &&
      last.kind === "edit" &&
      authorKey(last.author) === authorKey(author) &&
      now - last.endedAt <= SESSION_GAP_MS
    ) {
      last.endedAt = now;
      last.version = version;
      last.frontiers = frontiers;
      last.title = title;
      return;
    }
    rows.push({ kind: "edit", startedAt: now, endedAt: now, version, frontiers, before, author, title });
  }

  /** Forgets the journal: the documents were replaced (a backup restore), so its versions are gone. */
  reset(): void {
    this.#rows.clear();
  }

  /**
   * `PageHistoryList`: the versions of a page, dated rows newest first, then the undated runs of the older history; `current`
   * marks the version that is the page's content now. A session that changed nothing is listed once. `before` resumes after
   * a version of an earlier answer (`InvalidInput` for one the page's list does not have), `limit` is 100 by default, at most
   * 500. `NotFound` for an unknown page; a page in the trash is listed; a page without a document has no versions.
   */
  list(pageId: PageId, before: string | undefined, limit: number | undefined): PageHistory {
    this.#requirePage(pageId);
    const size = historyLimit(limit);
    const doc = this.#deps.content.docOf(pageId);
    const versions = doc === undefined ? [] : this.#versions(pageId, doc);
    let start = 0;
    if (before !== undefined) {
      const id = doc === undefined ? undefined : parseVersion(doc, before, "before").id;
      const at = versions.findIndex((version) => version.id === id);
      if (at < 0) {
        throw invalid("before", "is not a version in the page's history");
      }
      start = at + 1;
    }
    return { pageId, versions: versions.slice(start, start + size), more: start + size < versions.length };
  }

  /**
   * `PageHistoryGet`: the page's content (portable JSON) at a version, and its title then: that of the newest journal row with
   * that version, none for an undated one. `NotFound` for an unknown page; `InvalidInput` for a version the page does not
   * have. A page in the trash is answered.
   */
  get(pageId: PageId, version: string): PageVersionContent {
    this.#requirePage(pageId);
    const { frontiers, id } = this.#resolve(pageId, version);
    const result: PageVersionContent = {
      pageId,
      version: id,
      content: this.#deps.content.contentAt(pageId, frontiers),
    };
    const title = this.#titleAt(pageId, id);
    if (title !== undefined) {
      result.title = title;
    }
    return result;
  }

  /**
   * `PageHistoryRestore` (the Editor role is checked by the transport): makes the page's content what it was at the version, as
   * one new change on the memory document, records a `restore` row, renames the page when the version's title is known and
   * differs, and answers the page. Events: `PageContentChanged` (source `Core`), `BacklinksChanged`, then `StructureChanged`
   * on a rename. Restoring the current version (or a version whose state is the current one, with the same title) changes and
   * announces nothing. `NotFound` for an unknown page; `InvalidInput` for a page in the trash and for a version the page does
   * not have.
   */
  restore(pageId: PageId, version: string): PageInfo {
    const { structure, content } = this.#deps;
    const meta = structure.pageMeta(pageId);
    if (meta === undefined) {
      throw notFound("page not found");
    }
    if (meta.trashed) {
      throw invalid("pageId", "the page is in the trash");
    }
    const { frontiers, id } = this.#resolve(pageId, version);
    const doc = content.docOf(pageId);
    if (doc === undefined || id === versionId(doc.oplogFrontiers())) {
      return structure.pageGet(pageId);
    }
    const title = this.#titleAt(pageId, id);
    return this.#deps.collect(() => {
      const { changed, before } = content.revertTo(pageId, frontiers);
      const renamed = title !== undefined && title !== meta.title;
      if (renamed) {
        structure.pageRename(pageId, title);
      }
      if (changed || renamed) {
        this.#restored(pageId, doc, before);
      }
      return structure.pageGet(pageId);
    });
  }

  // ----- plumbing ---------------------------------------------------------------------

  /** A known page (live or in the trash): unknown and purged pages are `NotFound`. */
  #requirePage(pageId: PageId): void {
    if (this.#deps.structure.pageMeta(pageId) === undefined) {
      throw notFound("page not found");
    }
  }

  #rowsOf(pageId: PageId): JournalRow[] {
    let rows = this.#rows.get(pageId);
    if (rows === undefined) {
      rows = [];
      this.#rows.set(pageId, rows);
    }
    return rows;
  }

  /** The author of a write: this device, unless the change arrived from elsewhere (nobody is known then). */
  #author(source: ChangeSource): VersionAuthor | undefined {
    if (source === "Remote") {
      return undefined;
    }
    const { deviceId, accountId } = this.#deps.device();
    const author: VersionAuthor = { deviceId, thisDevice: true };
    if (accountId !== undefined) {
      author.accountId = accountId;
    }
    return author;
  }

  /** The `restore` row of a restore that is done: always a row of its own, titled as the page is now. */
  #restored(pageId: PageId, doc: LoroDoc, before: ReadonlyMap<string, number>): void {
    const now = this.#deps.now();
    const frontiers = doc.oplogFrontiers();
    this.#rowsOf(pageId).push({
      kind: "restore",
      startedAt: now,
      endedAt: now,
      version: versionId(frontiers),
      frontiers,
      before,
      author: this.#author("Core"),
      title: this.#deps.structure.pageMeta(pageId)?.title,
    });
  }

  /** The version a text names in the page's document (`InvalidInput` on `version` when there is none). */
  #resolve(pageId: PageId, version: string): { frontiers: OpId[]; id: string } {
    const doc = this.#deps.content.docOf(pageId);
    if (doc === undefined) {
      throw invalid("version", "names changes the page does not have");
    }
    return parseVersion(doc, version, "version");
  }

  /** The rows whose versions the document still has, in the order they began (a replaced document leaves none). */
  #validRows(pageId: PageId, doc: LoroDoc): JournalRow[] {
    const known = doc.oplogVersion();
    return (this.#rows.get(pageId) ?? []).filter((row) =>
      row.frontiers.every(({ peer, counter }) => counter < (known.get(peer) ?? 0)),
    );
  }

  /** The title of the newest journal row with this version. */
  #titleAt(pageId: PageId, id: string): string | undefined {
    const doc = this.#deps.content.docOf(pageId);
    const rows = doc === undefined ? [] : this.#validRows(pageId, doc);
    return rows.findLast((row) => row.version === id)?.title;
  }

  /** The page's whole list, newest first: the rows, then the undated runs, each id once. */
  #versions(pageId: PageId, doc: LoroDoc): PageVersion[] {
    const current = versionId(doc.oplogFrontiers());
    const rows = this.#validRows(pageId, doc);
    const ordered = rows
      .map((row, index) => ({ row, index }))
      .sort((a, b) => b.row.endedAt - a.row.endedAt || b.index - a.index);
    const seen = new Set<string>();
    const versions: PageVersion[] = [];
    const add = (version: PageVersion): void => {
      if (!seen.has(version.id)) {
        seen.add(version.id);
        if (version.id === current) {
          version.current = true;
        }
        versions.push(version);
      }
    };
    for (const { row } of ordered) {
      const version: PageVersion = { id: row.version, kind: row.kind, startedAt: row.startedAt, endedAt: row.endedAt };
      if (row.author !== undefined) {
        version.author = { ...row.author };
      }
      if (row.title !== undefined) {
        version.title = row.title;
      }
      add(version);
    }
    // The history before the first row's session; without any row, all of it.
    const first = rows[0];
    for (const id of undatedVersions(doc, first === undefined ? doc.oplogVersion().toJSON() : first.before)) {
      add({ id, kind: "edit" });
    }
    return versions;
  }
}

