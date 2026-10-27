// The in-memory twin of the core's Milestone 7 pairing and recovery sessions (`kb-core`'s
// `pairing.rs` and `recovery.rs`, ADR-0022, `docs/development/milestone-07.md`), used by
// `MemoryTransport`, together with a fake of the shell's pairing and recovery tasks. Nothing
// goes over a network: the other device (an imaginary sponsor for a joiner, an imaginary joiner
// named "Pixel 9" for a sponsor) and the server answer on short timers, so the UI can walk
// through both roles and the recovery. What a finished session installs (the workspace, the
// account, the device log, sync) is the transport's, through the callbacks of the deps.

import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { PairingInfo } from "./generated/PairingInfo";
import type { PairingRole } from "./generated/PairingRole";
import type { PairingState } from "./generated/PairingState";
import type { RecoveryInfo } from "./generated/RecoveryInfo";
import type { RecoveryState } from "./generated/RecoveryState";
import { parseKit } from "./kit";
import { recoveryIdOf } from "./security";
import { formatPairingCode, invalid, pairingCode, serverUrl } from "./validate";

/** Default length of one step of the fake pairing and recovery tasks (`MemoryTransportOptions.pairingStepMs`). */
export const DEFAULT_PAIRING_STEP_MS = 150;
/** The code a joiner mirror shows (`PairingInfo.code` while `Waiting`). */
export const MEMORY_PAIRING_CODE = "ABCD EFGH JKMN";
/** The six digits both sides of a mirrored pairing show. */
export const MEMORY_PAIRING_SAS = "428516";
/** The name of the imaginary joiner a sponsor mirror adds. */
export const MEMORY_JOINER_NAME = "Pixel 9";
/** A code the simulated server no longer knows: a sponsor typing it ends `Failed`, "the code expired". */
export const EXPIRED_PAIRING_CODE = "BADB ADBA DBAD";
/** How long a pairing mailbox lives (`kb_types::devices::PAIRING_TTL_SECONDS`), in milliseconds. */
const PAIRING_TTL_MS = 600 * 1000;

/** What a pairing or a recovery needs from the transport. */
export interface DevicesDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Whether the core is ready; the fake tasks stop when it is not. */
  ready: () => boolean;
  /** Length of one step, in milliseconds. */
  stepMs: number;
  /** Whether this installation has a workspace (joining and recovering need a fresh one). */
  hasWorkspace: () => boolean;
}

/** What pairing needs besides `DevicesDeps`. */
export interface PairingDeps extends DevicesDeps {
  /** Whether this installation has an account (a recovery kit): sponsoring needs one. */
  hasAccount: () => boolean;
  /** The configured sync server: the sponsor pairs through it. */
  syncServer: () => string | undefined;
  /** The joiner's `Done`: installs the imaginary sponsor's bundle and connects to `serverUrl`. */
  installBundle: (serverUrl: string) => void;
  /** The sponsor's confirmation: the joiner, named `name`, joins the device log. */
  deviceAdded: (name: string) => void;
}

/** What recovery needs besides `DevicesDeps`. */
export interface RecoveryDeps extends DevicesDeps {
  /** `Done`: installs the account the kit (`recoveryId`) opens and connects to `serverUrl`. */
  install: (serverUrl: string, recoveryId: string) => void;
}

/** `AlreadyExists`: pairing and recovery set up a fresh installation only. */
function workspaceExists(): CoreError {
  return new CoreError({
    code: "AlreadyExists",
    message: "this installation already has a workspace; joining an account needs a fresh one",
  });
}

/** Runs one step after another on a timer, as long as the core stays ready. */
export class Steps {
  readonly #deps: DevicesDeps;
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor(deps: DevicesDeps) {
    this.#deps = deps;
  }

  /** Runs `step` after one step; `stopped` instead when the core is no longer ready by then. */
  next(step: () => void, stopped: () => void): void {
    this.cancel();
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      if (this.#deps.ready()) {
        step();
      } else {
        stopped();
      }
    }, this.#deps.stepMs);
  }

  cancel(): void {
    if (this.#timer !== undefined) {
      clearTimeout(this.#timer);
      this.#timer = undefined;
    }
  }
}

const PAIRING_ACTIVE: ReadonlySet<PairingState> = new Set([
  "Starting",
  "Waiting",
  "Confirming",
  "PeerConfirming",
  "Transferring",
]);

interface PairingSession {
  role: PairingRole;
  state: PairingState;
  serverUrl: string;
  code?: string;
  sas?: string;
  peerName?: string;
  error?: string;
  expiresAt?: number;
  /** A sponsor typed the expired code: the server will not know the mailbox. */
  expired: boolean;
}

/**
 * The pairing session (`PairingStart`, `PairingConfirm`, `PairingCancel`, `PairingStatus`) and a
 * fake of the shell's pairing task. Every change emits `PairingChanged`; a start also emits
 * `PairingRequested` (the shell starts its task).
 *
 * - Joiner (`serverUrl` required; a workspace is `AlreadyExists`): `Starting` → (step) `Waiting`
 *   with the code `MEMORY_PAIRING_CODE` and `expiresAt` → (step, the imaginary sponsor typed it)
 *   `Confirming` with the six digits → `PairingConfirm` → `PeerConfirming` → (step)
 *   `Transferring` → (step) `Done`: the transport installs the bundle (`WorkspaceCreated`,
 *   `SecurityChanged`, then sync connects: `SyncChanged`, `SyncRequested`) before the last
 *   `PairingChanged`.
 * - Sponsor (`code` required and well-formed; needs the account's kit and sync on, whose server
 *   it pairs through): `Starting` → (step) `Confirming` with the digits → (step, the joiner
 *   confirmed) `peerName` `MEMORY_JOINER_NAME` → `PairingConfirm`: the joiner joins the device log
 *   (`SecurityChanged`, `DevicesChanged` with it), then `Transferring` → (step) `Done`. The code
 *   `EXPIRED_PAIRING_CODE` ends `Failed` with "the code expired" instead of `Confirming`.
 * - `PairingConfirm` outside `Confirming`, or on the sponsor before the joiner confirmed, is
 *   `InvalidInput` for the field `pairing`. `PairingCancel` ends a session in progress as
 *   `Cancelled`; a new `PairingStart` replaces any session. A lock (`stop`) cancels the session
 *   in progress silently.
 */
export class MemoryPairing {
  readonly #deps: PairingDeps;
  readonly #steps: Steps;
  #session: PairingSession | undefined;

  constructor(deps: PairingDeps) {
    this.#deps = deps;
    this.#steps = new Steps(deps);
  }

  /** `PairingStatus`. Optional fields are left out when unset, as the core's serialisation does. */
  info(): PairingInfo {
    const session = this.#session;
    if (session === undefined) {
      return { role: "Joiner", state: "Idle" };
    }
    const info: PairingInfo = { role: session.role, state: session.state, serverUrl: session.serverUrl };
    if (session.code !== undefined) {
      info.code = session.code;
    }
    if (session.sas !== undefined) {
      info.sas = session.sas;
    }
    if (session.peerName !== undefined) {
      info.peerName = session.peerName;
    }
    if (session.error !== undefined) {
      info.error = session.error;
    }
    if (session.expiresAt !== undefined) {
      info.expiresAt = session.expiresAt;
    }
    return info;
  }

  /** `PairingStart`. See the class documentation. */
  start(role: PairingRole, rawUrl: string | undefined, rawCode: string | undefined): PairingInfo {
    let session: PairingSession;
    if (role === "Joiner") {
      if (rawUrl === undefined) {
        throw invalid("serverUrl", "the server address must be http(s)://host[:port]");
      }
      const url = serverUrl(rawUrl);
      if (this.#deps.hasWorkspace()) {
        throw workspaceExists();
      }
      session = { role, state: "Starting", serverUrl: url, expired: false };
    } else {
      if (rawCode === undefined) {
        throw invalid("code", "the code is 12 letters and digits");
      }
      const code = pairingCode(rawCode);
      if (!this.#deps.hasAccount()) {
        throw invalid("kit", "create your recovery kit before adding a device");
      }
      const url = this.#deps.syncServer();
      if (url === undefined) {
        throw invalid("serverUrl", "turn sync on before adding a device");
      }
      session = {
        role,
        state: "Starting",
        serverUrl: url,
        code: formatPairingCode(code),
        expired: code === pairingCode(EXPIRED_PAIRING_CODE),
      };
    }
    this.#steps.cancel();
    this.#session = session;
    this.#deps.emit({ type: "PairingChanged" });
    this.#deps.emit({ type: "PairingRequested" });
    this.#next(session, () => {
      if (session.role === "Joiner") {
        this.#joinerWaits(session);
      } else {
        this.#sponsorFetched(session);
      }
    });
    return this.info();
  }

  /** `PairingConfirm`: the user compared the six digits. See the class documentation. */
  confirm(): PairingInfo {
    const session = this.#session;
    if (session?.state !== "Confirming") {
      throw invalid("pairing", "there is nothing to confirm");
    }
    if (session.role === "Joiner") {
      this.#change(session, { state: "PeerConfirming" });
      this.#next(session, () => {
        this.#change(session, { state: "Transferring" });
        this.#next(session, () => {
          this.#finish(session, () => {
            this.#deps.installBundle(session.serverUrl);
          });
        });
      });
    } else {
      if (session.peerName === undefined) {
        throw invalid("pairing", "the other device has not confirmed yet");
      }
      // The sponsor appends the joiner's `DeviceAdded` once confirmed, before the bundle goes.
      this.#deps.deviceAdded(session.peerName);
      this.#change(session, { state: "Transferring" });
      this.#next(session, () => {
        this.#finish(session, () => undefined);
      });
    }
    return this.info();
  }

  /** `PairingCancel`: a session in progress ends `Cancelled`; otherwise nothing changes. */
  cancel(): PairingInfo {
    const session = this.#session;
    if (session !== undefined && PAIRING_ACTIVE.has(session.state)) {
      this.#steps.cancel();
      this.#change(session, { state: "Cancelled" });
    }
    return this.info();
  }

  /** The core locked: the shell stops the task, and the session in progress ends `Cancelled`, silently. */
  stop(): void {
    this.#steps.cancel();
    const session = this.#session;
    if (session !== undefined && PAIRING_ACTIVE.has(session.state)) {
      session.state = "Cancelled";
    }
  }

  #joinerWaits(session: PairingSession): void {
    this.#change(session, {
      state: "Waiting",
      code: MEMORY_PAIRING_CODE,
      expiresAt: this.#deps.now() + PAIRING_TTL_MS,
    });
    this.#next(session, () => {
      this.#change(session, { state: "Confirming", sas: MEMORY_PAIRING_SAS });
    });
  }

  #sponsorFetched(session: PairingSession): void {
    if (session.expired) {
      this.#change(session, { state: "Failed", error: "the code expired" });
      return;
    }
    this.#change(session, {
      state: "Confirming",
      sas: MEMORY_PAIRING_SAS,
      expiresAt: this.#deps.now() + PAIRING_TTL_MS,
    });
    this.#next(session, () => {
      this.#change(session, { peerName: MEMORY_JOINER_NAME });
    });
  }

  /** `Done` after `install`, unless a workspace appeared meanwhile on a joiner (`Failed`). */
  #finish(session: PairingSession, install: () => void): void {
    if (session.role === "Joiner" && this.#deps.hasWorkspace()) {
      this.#change(session, { state: "Failed", error: "this installation already has a workspace" });
      return;
    }
    install();
    this.#change(session, { state: "Done" });
  }

  /** The next step of `session`, unless it was replaced, cancelled or stopped meanwhile. */
  #next(session: PairingSession, step: () => void): void {
    this.#steps.next(
      () => {
        if (this.#session === session && PAIRING_ACTIVE.has(session.state)) {
          step();
        }
      },
      () => {
        this.stop();
      },
    );
  }

  #change(session: PairingSession, change: Partial<PairingSession>): void {
    Object.assign(session, change);
    this.#deps.emit({ type: "PairingChanged" });
  }
}

const RECOVERY_ACTIVE: ReadonlySet<RecoveryState> = new Set(["Fetching", "Joining"]);

interface RecoverySession {
  state: RecoveryState;
  serverUrl: string;
  error?: string;
  /** The kit's recovery id; `undefined` for a kit the simulated server does not know. */
  recoveryId: string | undefined;
}

/**
 * The recovery with the kit from a server (`RecoveryStart`, `RecoveryStatus`, `RecoveryCancel`)
 * and a fake of the shell's recovery task. Every change emits `RecoveryChanged`; a start also
 * emits `RecoveryRequested`.
 *
 * - `RecoveryStart` checks the address (`InvalidInput` for `serverUrl`) and the kit (it must
 *   decode like `RecoveryKitVerify`'s: `InvalidInput` for `kit`), then wants a fresh
 *   installation (`AlreadyExists`). A kit text containing the word "wrong" stands for a kit the
 *   simulated server does not know: it is not decoded, and the recovery ends `Failed` with "this
 *   server does not know that kit" after the fetch.
 * - `Fetching` → (step) `Joining` → (step) `Done`: the transport installs the account the kit
 *   opens (`WorkspaceCreated`, `SecurityChanged`, then sync connects: `SyncChanged`,
 *   `SyncRequested`), this device "added via kit", before the last `RecoveryChanged`.
 * - `RecoveryCancel` drops the session (`Idle`); a new `RecoveryStart` replaces it; `stop` (a
 *   lock) drops it silently.
 */
export class MemoryRecovery {
  readonly #deps: RecoveryDeps;
  readonly #steps: Steps;
  #session: RecoverySession | undefined;

  constructor(deps: RecoveryDeps) {
    this.#deps = deps;
    this.#steps = new Steps(deps);
  }

  /** `RecoveryStatus`. Optional fields are left out when unset, as the core's serialisation does. */
  info(): RecoveryInfo {
    const session = this.#session;
    if (session === undefined) {
      return { state: "Idle" };
    }
    const info: RecoveryInfo = { state: session.state, serverUrl: session.serverUrl };
    if (session.error !== undefined) {
      info.error = session.error;
    }
    return info;
  }

  /** `RecoveryStart`. See the class documentation. */
  async start(rawUrl: string, kit: string): Promise<RecoveryInfo> {
    const url = serverUrl(rawUrl);
    let recoveryId: string | undefined;
    if (!/\bwrong\b/iu.test(kit)) {
      const key = await parseKit(kit);
      try {
        recoveryId = await recoveryIdOf(key);
      } finally {
        key.fill(0);
      }
    }
    if (this.#deps.hasWorkspace()) {
      throw workspaceExists();
    }
    const session: RecoverySession = { state: "Fetching", serverUrl: url, recoveryId };
    this.#steps.cancel();
    this.#session = session;
    this.#deps.emit({ type: "RecoveryChanged" });
    this.#deps.emit({ type: "RecoveryRequested" });
    this.#next(session, () => {
      if (session.recoveryId === undefined) {
        this.#change(session, { state: "Failed", error: "this server does not know that kit" });
        return;
      }
      const id = session.recoveryId;
      this.#change(session, { state: "Joining" });
      this.#next(session, () => {
        if (this.#deps.hasWorkspace()) {
          this.#change(session, { state: "Failed", error: "this installation already has a workspace" });
          return;
        }
        this.#deps.install(session.serverUrl, id);
        this.#change(session, { state: "Done" });
      });
    });
    return this.info();
  }

  /** `RecoveryCancel`: the session is dropped (`Idle`), with `RecoveryChanged` when there was one. */
  cancel(): RecoveryInfo {
    this.#steps.cancel();
    if (this.#session !== undefined) {
      this.#session = undefined;
      this.#deps.emit({ type: "RecoveryChanged" });
    }
    return this.info();
  }

  /** The core locked or failed: the task stops and the session is dropped, silently. */
  stop(): void {
    this.#steps.cancel();
    if (this.#session !== undefined && RECOVERY_ACTIVE.has(this.#session.state)) {
      this.#session = undefined;
    }
  }

  #next(session: RecoverySession, step: () => void): void {
    this.#steps.next(
      () => {
        if (this.#session === session && RECOVERY_ACTIVE.has(session.state)) {
          step();
        }
      },
      () => {
        this.stop();
      },
    );
  }

  #change(session: RecoverySession, change: Partial<RecoverySession>): void {
    Object.assign(session, change);
    this.#deps.emit({ type: "RecoveryChanged" });
  }
}
