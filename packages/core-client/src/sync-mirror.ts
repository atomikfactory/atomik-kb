// The in-memory twin of the core's Milestone 6 sync side (`kb-core`'s `sync.rs`,
// `docs/development/milestone-06.md`, "Contract" and "TypeScript client"), used by
// `MemoryTransport`, together with a fake of the shell's sync driver (`kb-net`'s `SyncDriver`).
// Nothing goes over a network and there is no server: the fake driver only walks the status
// through the states a real cycle shows, on short timers, so the UI can be built and tested
// against it. The simulated server is chosen with `MemorySyncOptions` (healthy, unreachable,
// failing, with a history that does not match this device's, or not knowing the account yet).

import type { Event } from "./generated/Event";
import type { SyncInfo } from "./generated/SyncInfo";
import type { SyncState } from "./generated/SyncState";
import type { TransferInfo } from "./generated/TransferInfo";
import { inviteToken, serverUrl } from "./validate";

/**
 * How the simulated server behaves (`MemorySyncOptions.state`, `MemoryTransport.setSyncMode`):
 *
 * - `"idle"`: healthy; a cycle goes `Pushing` → `Pulling` → `Idle`;
 * - `"offline"`: unreachable; a cycle ends `Offline` with `lastError` (default "cannot reach the
 *   server"), local edits stay pending;
 * - `"forked"`: its history does not match this device's; a cycle only pulls (`Pulling` →
 *   `Forked`, `forkDetected: true`), local edits stay pending (uploads are paused);
 * - `"error"`: failing; a cycle ends `Error` with `lastError` (default "the server refused the
 *   request"), local edits stay pending.
 */
export type MemorySyncMode = "idle" | "offline" | "forked" | "error";

/**
 * The starting situation of sync (`MemoryTransportOptions.sync`, for the dev page's
 * `?sync=idle|offline|forked|error`). With `state` or `serverUrl` the transport starts
 * connected to `serverUrl` (default `https://sync.example.com`), as a device that has synced
 * with it before: the server epoch known, the last push and pull at start, and the status the
 * mode shows between cycles (`Idle`, `Offline`, `Forked` with `forkDetected`, or `Error`).
 * Without either it starts `Disabled`. The simulated server stays the same across a disconnect
 * and a reconnect (until `MemoryTransport.setSyncMode`).
 */
export interface MemorySyncOptions {
  /** The simulated server (default `"idle"`, a healthy one). */
  state?: MemorySyncMode;
  /** The configured server address (validated like `SyncConfigure`'s). */
  serverUrl?: string;
  /** The failure message of the `"offline"` and `"error"` modes, instead of their defaults. */
  lastError?: string;
  /**
   * Whether the simulated server knows this device's account (default `true`). When `false`, a
   * cycle that reaches the server without an invite token ends `Error` ("the server does not
   * know this account; an invite token is required") with `needsInviteToken: true`; a
   * `syncConfigure` with a token creates the account (its cycle then runs normally), except the
   * token `"bad"`, which the server refuses (`Error`, "the server refused the invite token",
   * `needsInviteToken` still `true`). Starting connected with `false` (and a reachable server)
   * shows the status of such a failed cycle, without a last push or pull.
   */
  accountKnown?: boolean;
}

/** Default length of one step of the fake driver, in milliseconds (`MemoryTransportOptions.syncStepMs`). */
export const DEFAULT_SYNC_STEP_MS = 150;
/** The simulated server's storage epoch (16 bytes, hex), as `SyncInfo.serverEpoch` reports it. */
export const MEMORY_SERVER_EPOCH = "5e7e0e90c4a14b2f9d3c6a8b1e0f2d47";
/** The server a seeded configuration points at when `MemorySyncOptions.serverUrl` is left out. */
export const DEFAULT_MEMORY_SERVER_URL = "https://sync.example.com";
/** The invite token the simulated server refuses, so the UI's retry path can be exercised. */
export const BAD_INVITE_TOKEN = "bad";

const OFFLINE_ERROR = "cannot reach the server";
const SERVER_ERROR = "the server refused the request";
/** What the core reports when the driver found the account unknown to the server (the core's text). */
const UNKNOWN_ACCOUNT_ERROR = "the server does not know this account; an invite token is required";
const TOKEN_REFUSED_ERROR = "the server refused the invite token";
/** The state each simulated server leaves the status in between cycles. */
const RESTING_STATE: Record<MemorySyncMode, SyncState> = {
  idle: "Idle",
  offline: "Offline",
  forked: "Forked",
  error: "Error",
};

/**
 * Whether an event announces a local change the core would upload: a page edited here (not by
 * another device), a page tree, the spaces or a space's attachments. Each one counts as one
 * pending entry. Settings are local to the device and never synced. A change of the property
 * schema (ADR-0026) is announced by `PropertySchemaChanged` followed by `StructureChanged`: the
 * structure document is one entry, so the second counts and the first does not.
 */
export function isLocalEdit(event: Event): boolean {
  switch (event.type) {
    case "PageContentChanged":
      return event.data.source !== "Remote";
    case "StructureChanged":
    case "SpacesChanged":
    case "AttachmentsChanged":
      return true;
    case "CommentsChanged":
      // A comment of this account is one record, one entry of the page's comment stream (M13 slice 2).
      return true;
    case "PropertySchemaChanged":
      return false;
    default:
      return false;
  }
}

/** What the sync mirror needs from its owner. */
export interface SyncDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Whether the core is ready; the fake driver stops when it is not (locked, failed). */
  ready: () => boolean;
  /** Length of one step of the fake driver, in milliseconds. */
  stepMs: number;
  /**
   * A healthy cycle finished pushing (the driver also uploads the recovery blob then, and the
   * mirror counts the attachment chunks to upload as sent with it).
   */
  pushed?: () => void;
  /** Sync was disabled (the core clears its chunk transfer table). */
  disconnected?: () => void;
  /** `SyncInfo.transfers` (Milestone 8); nothing to transfer without it. */
  transfers?: () => TransferInfo;
}

/** Where the fake driver is in a cycle: waiting for its first step (the debounce), or in one. */
type Phase = "waiting" | "pushing" | "pulling";

/**
 * The sync status (`SyncConfigure`, `SyncStatus`, `SyncNow`) and the fake driver.
 *
 * - `configure(url, token)` validates like the core (`InvalidInput` for `serverUrl` or for a
 *   malformed `inviteToken`), stores the address (a different one forgets what the old server
 *   taught: cursor, epoch, times, fork, `needsInviteToken`), hands the token to the driver for
 *   its next cycle (never reported; a token clears `needsInviteToken`), answers `Idle` (`Forked`
 *   when the same server's fork is still detected: only a disconnect or another server clears it)
 *   and emits `SyncChanged` then `SyncRequested`. `configure(undefined)` disables: `Disabled`,
 *   cursor 0, no epoch, nothing pending, `SyncChanged`.
 * - Every local edit while configured (see `localEdit`) counts as one pending entry and emits
 *   `SyncRequested`, like the core's persist hooks; `syncNow` emits `SyncRequested`.
 * - The fake driver answers `SyncRequested` (debounced: requests while it waits merge, requests
 *   during a cycle run one more cycle after it) with a cycle, one step of `stepMs` per state,
 *   each state change emitting `SyncChanged`. An unreachable or failing server ends the cycle at
 *   once (`Offline`, `Error`). Reaching the server makes its epoch known; an unknown account
 *   without an acceptable token ends it `Error` with `needsInviteToken`. Then a forked device
 *   only pulls (`Pulling` → `Forked`); a healthy cycle goes `Pushing` (the pending entries are the
 *   ones being pushed, plus edits made meanwhile), `Pulling` (`lastPushAt`; pushed entries are no
 *   longer pending), `Idle` (`lastPullAt`; `remoteCursor` grows by the entries pushed, which come
 *   back through the feed). `devicesSeen` stays 0: the simulated server has no other device.
 * - `transfers` (Milestone 8) comes from the owner (`SyncDeps.transfers`): the attachment
 *   mirror's counts. Chunks move through the attachment mirror's own transfer steps, not
 *   through these cycles, except that a healthy push also sends the chunks to upload.
 * - `stop` (a lock) cancels the cycle in progress: what it was pushing is pending again and the
 *   state returns to the one before the cycle, without events. `resume` (an unlock, when the
 *   shell restarts the driver) runs a cycle when configured.
 */
export class MemorySync {
  readonly #deps: SyncDeps;
  #mode: MemorySyncMode = "idle";
  #modeError: string | undefined;
  /** The simulated server knows the account (it does once a token created it). */
  #accountKnown = true;
  #serverUrl: string | undefined;
  #state: SyncState = "Disabled";
  #remoteCursor = 0;
  #serverEpoch: string | undefined;
  #lastPushAt: number | undefined;
  #lastPullAt: number | undefined;
  #lastError: string | undefined;
  #forkDetected = false;
  #needsInviteToken = false;
  /** The invite token of the last `configure`, held for the driver's next cycle that reaches the server. */
  #token: string | undefined;
  /** Local edits not yet taken by a cycle. */
  #edits = 0;
  /** Edits the current cycle is pushing. */
  #inFlight = 0;
  /** Edits pushed whose entries have not come back through a pull yet. */
  #pushed = 0;
  #phase: Phase | undefined;
  /** The state before the cycle in progress, restored by `stop`. */
  #before: SyncState = "Disabled";
  #rerun = false;
  #timer: ReturnType<typeof setTimeout> | undefined;
  /** Remote changes are being applied: their events are not local edits. */
  #applyingRemote = 0;

  constructor(deps: SyncDeps) {
    this.#deps = deps;
  }

  /** Applies `MemoryTransportOptions.sync`, without events and without starting a cycle. */
  seed(options: MemorySyncOptions): void {
    this.#mode = options.state ?? "idle";
    this.#modeError = options.lastError;
    this.#accountKnown = options.accountKnown ?? true;
    if (options.state === undefined && options.serverUrl === undefined) {
      return;
    }
    this.#serverUrl = serverUrl(options.serverUrl ?? DEFAULT_MEMORY_SERVER_URL);
    const failure = this.#failure();
    if (failure !== undefined) {
      this.#state = RESTING_STATE[this.#mode];
      this.#lastError = failure;
    } else if (!this.#accountKnown) {
      this.#serverEpoch = MEMORY_SERVER_EPOCH;
      this.#accountUnknown(UNKNOWN_ACCOUNT_ERROR);
      return;
    } else {
      this.#state = RESTING_STATE[this.#mode];
      this.#forkDetected = this.#mode === "forked";
    }
    if (this.#accountKnown) {
      this.#serverEpoch = MEMORY_SERVER_EPOCH;
      const now = this.#deps.now();
      this.#lastPushAt = now;
      this.#lastPullAt = now;
    }
  }

  get configured(): boolean {
    return this.#serverUrl !== undefined;
  }

  /** `SyncStatus`. Optional fields are left out when unset, as the core's serialisation does. */
  info(): SyncInfo {
    const info: SyncInfo = {
      configured: this.configured,
      state: this.#state,
      pendingEntries: this.#edits + this.#inFlight,
      parkedItems: 0,
      transfers: this.#deps.transfers?.() ?? { uploads: 0, downloads: 0, waiting: 0 },
      remoteCursor: this.#remoteCursor,
      forkDetected: this.#forkDetected,
      devicesSeen: 0,
      needsInviteToken: this.#needsInviteToken,
    };
    if (this.#serverUrl !== undefined) {
      info.serverUrl = this.#serverUrl;
    }
    if (this.#serverEpoch !== undefined) {
      info.serverEpoch = this.#serverEpoch;
    }
    if (this.#lastPushAt !== undefined) {
      info.lastPushAt = this.#lastPushAt;
    }
    if (this.#lastPullAt !== undefined) {
      info.lastPullAt = this.#lastPullAt;
    }
    if (this.#lastError !== undefined) {
      info.lastError = this.#lastError;
    }
    return info;
  }

  /** `SyncConfigure`: `rawUrl` undefined disables. See the class documentation. */
  configure(rawUrl: string | undefined, rawToken: string | undefined): SyncInfo {
    const url = rawUrl === undefined ? undefined : serverUrl(rawUrl);
    const token = rawToken === undefined ? undefined : inviteToken(rawToken);
    this.#cancelCycle();
    if (url === undefined) {
      this.#serverUrl = undefined;
      this.#token = undefined;
      this.#forget();
      this.#edits = 0;
      this.#state = "Disabled";
      this.#deps.disconnected?.();
      this.#deps.emit({ type: "SyncChanged" });
      return this.info();
    }
    if (url !== this.#serverUrl) {
      this.#forget();
    }
    this.#serverUrl = url;
    this.#token = token;
    if (token !== undefined) {
      this.#needsInviteToken = false;
    }
    this.#lastError = undefined;
    this.#state = this.#forkDetected ? "Forked" : "Idle";
    this.#deps.emit({ type: "SyncChanged" });
    this.#deps.emit({ type: "SyncRequested" });
    this.#request();
    return this.info();
  }

  /** `SyncNow`: emits `SyncRequested`; the fake driver runs a cycle when configured. */
  syncNow(): void {
    this.#deps.emit({ type: "SyncRequested" });
    this.#request();
  }

  /**
   * A local change was persisted (the transport calls this for its page, structure, space and
   * attachment events, see `isLocalEdit`): while configured it is one more pending entry and
   * emits `SyncRequested`. Changes applied inside `remote` do not count.
   */
  localEdit(): void {
    if (!this.configured || this.#applyingRemote > 0) {
      return;
    }
    this.#edits += 1;
    this.#deps.emit({ type: "SyncRequested" });
    this.#request();
  }

  /** Runs `apply` as a change that came from another device: its events are not local edits. */
  remote<T>(apply: () => T): T {
    this.#applyingRemote += 1;
    try {
      return apply();
    } finally {
      this.#applyingRemote -= 1;
    }
  }

  /** Switches the simulated server; the next cycle shows it. */
  setMode(mode: MemorySyncMode, lastError?: string): void {
    this.#mode = mode;
    this.#modeError = lastError;
  }

  /** The core locked (or failed): the driver stops, silently. */
  stop(): void {
    this.#cancelCycle();
  }

  /** The core is ready again and the shell restarts the driver, which runs a cycle when configured. */
  resume(): void {
    this.#request();
  }

  // ----- the fake driver --------------------------------------------------------------

  /** A cycle was asked for: start one after a step, merge into the waiting one, or run one more. */
  #request(): void {
    if (!this.configured || !this.#deps.ready()) {
      return;
    }
    if (this.#phase === undefined) {
      this.#before = this.#state;
      this.#phase = "waiting";
      this.#schedule(() => {
        this.#firstStep();
      });
    } else if (this.#phase !== "waiting") {
      this.#rerun = true;
    }
  }

  /** The cycle's first step: the epoch check and the account, then the push or the pull. */
  #firstStep(): void {
    this.#phase = undefined;
    const failure = this.#failure();
    if (failure !== undefined) {
      // The server cannot be reached or fails: the token waits for a cycle that reaches it.
      this.#lastError = failure;
      this.#show(RESTING_STATE[this.#mode]);
      return;
    }
    this.#serverEpoch = MEMORY_SERVER_EPOCH;
    const token = this.#token;
    this.#token = undefined;
    if (!this.#accountKnown) {
      if (token === undefined || token === BAD_INVITE_TOKEN) {
        this.#accountUnknown(token === undefined ? UNKNOWN_ACCOUNT_ERROR : TOKEN_REFUSED_ERROR);
        this.#deps.emit({ type: "SyncChanged" });
        return;
      }
      // The server accepted the token and created the account.
      this.#accountKnown = true;
    }
    this.#needsInviteToken = false;
    this.#lastError = undefined;
    if (this.#forkDetected || this.#mode === "forked") {
      // Uploads stay paused while forked: the driver only pulls.
      this.#phase = "pulling";
      this.#show("Pulling");
      this.#schedule(() => {
        this.#forkDetected = true;
        this.#finishPull("Forked");
      });
      return;
    }
    this.#phase = "pushing";
    this.#inFlight += this.#edits;
    this.#edits = 0;
    this.#show("Pushing");
    this.#schedule(() => {
      this.#pushed += this.#inFlight;
      this.#inFlight = 0;
      this.#lastPushAt = this.#deps.now();
      this.#phase = "pulling";
      this.#show("Pulling");
      this.#deps.pushed?.();
      this.#schedule(() => {
        this.#finishPull("Idle");
      });
    });
  }

  /** The server does not know the account (or refused the token): `Error`, waiting for a token. */
  #accountUnknown(message: string): void {
    this.#state = "Error";
    this.#lastError = message;
    this.#needsInviteToken = true;
  }

  /** The pull ends: our pushed entries came back through the feed. */
  #finishPull(state: SyncState): void {
    this.#remoteCursor += this.#pushed;
    this.#pushed = 0;
    this.#lastPullAt = this.#deps.now();
    this.#phase = undefined;
    this.#show(state);
    if (this.#rerun) {
      this.#rerun = false;
      this.#request();
    }
  }

  /** Runs `step` after `stepMs`, unless the core stopped being ready meanwhile. */
  #schedule(step: () => void): void {
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      if (!this.#deps.ready()) {
        this.#cancelCycle();
        return;
      }
      step();
    }, this.#deps.stepMs);
  }

  /** Cancels the cycle in progress: what it was pushing is pending again, the state is the one before it. */
  #cancelCycle(): void {
    if (this.#timer !== undefined) {
      clearTimeout(this.#timer);
      this.#timer = undefined;
    }
    if (this.#phase !== undefined && this.#phase !== "waiting") {
      this.#state = this.#before;
    }
    this.#edits += this.#inFlight;
    this.#inFlight = 0;
    this.#phase = undefined;
    this.#rerun = false;
  }

  /** Forgets everything learnt from a server (another server, or sync disabled). */
  #forget(): void {
    this.#remoteCursor = 0;
    this.#serverEpoch = undefined;
    this.#lastPushAt = undefined;
    this.#lastPullAt = undefined;
    this.#lastError = undefined;
    this.#forkDetected = false;
    this.#needsInviteToken = false;
    this.#pushed = 0;
  }

  /** The failure the simulated server answers in the `"offline"` and `"error"` modes. */
  #failure(): string | undefined {
    switch (this.#mode) {
      case "offline":
        return this.#modeError ?? OFFLINE_ERROR;
      case "error":
        return this.#modeError ?? SERVER_ERROR;
      case "idle":
      case "forked":
        return undefined;
    }
  }

  #show(state: SyncState): void {
    this.#state = state;
    this.#deps.emit({ type: "SyncChanged" });
  }
}
