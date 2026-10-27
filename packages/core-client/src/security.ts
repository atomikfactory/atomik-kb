// The in-memory twin of the core's Milestone 4 key and identity side (`kb-core`'s
// `security.rs`, `docs/development/milestone-04.md`, "Core"): the recovery kit, the app
// lock, the device log and the keyring counts, used by `MemoryTransport`; with Milestone 7's
// revocation, root key generations and rotations (`devices.rs`, ADR-0022). Nothing is
// encrypted. A kit is generated and encoded like the core does it (32 random bytes, 24
// BIP-39 words and the code form of `./kit`) and then forgotten: only a hash of its key
// (the mirror's recovery id) is kept to verify a re-typed kit, and only a hash of the
// app-lock passphrase is kept to check it. Errors (codes, fields, messages, order of the
// checks) follow `kb-core`'s `security.rs`.

import { sha256, toHex } from "./digest";
import { CoreError, internalError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { DeviceEntry } from "./generated/DeviceEntry";
import type { DeviceId } from "./generated/DeviceId";
import type { DeviceInfo } from "./generated/DeviceInfo";
import type { Event } from "./generated/Event";
import type { KeyringInfo } from "./generated/KeyringInfo";
import type { PageId } from "./generated/PageId";
import type { RecoveryKit } from "./generated/RecoveryKit";
import type { RecoveryKitInfo } from "./generated/RecoveryKitInfo";
import type { SecurityInfo } from "./generated/SecurityInfo";
import type { SpaceId } from "./generated/SpaceId";
import type { MemoryContent } from "./content";
import { KIT_KEY_BYTES, kitCodeFromEntropy, kitWordsFromEntropy, parseKit } from "./kit";
import type { MemoryStructure } from "./structure";
import {
  PASSPHRASE_MAX_BYTES,
  PASSPHRASE_MIN_BYTES,
  invalid,
  passphraseLengthOk,
  passphrase as validPassphrase,
} from "./validate";

/** The Argon2id parameters the core reports for the app lock (ADR-0018 defaults and minimums). */
const ARGON2 = { memoryKib: 65_536, iterations: 3, parallelism: 4 } as const;
/** Domains of the mirror's hashes (stand-ins for the core's HKDF labels, not the same values). */
const RECOVERY_ID_DOMAIN = "atomik-kb/memory/recovery-id";
const PASSPHRASE_DOMAIN = "atomik-kb/memory/passphrase";

/**
 * The mirror's recovery id of a kit's key: hex SHA-256 under a mirror-only domain. Like the
 * core's `HKDF(RcK, recovery-id)` it is not secret and identifies the kit that opens a backup.
 */
export async function recoveryIdOf(key: Uint8Array): Promise<string> {
  return toHex(await sha256(RECOVERY_ID_DOMAIN, key));
}

function passphraseHash(passphrase: string): Promise<string> {
  return sha256(PASSPHRASE_DOMAIN, passphrase).then(toHex);
}

/** A fixed fake Ed25519 public key for a device: its id's hex digits, repeated to 64. */
function fakeSigningKey(deviceId: DeviceId): string {
  const hex = deviceId.toLowerCase().replace(/[^0-9a-f]/gu, "") || "0";
  return hex.repeat(Math.ceil(64 / hex.length)).slice(0, 64);
}

/** Hashes that nobody may be waiting for yet must not end as unhandled rejections. */
function quiet<T>(promise: Promise<T>): Promise<T> {
  promise.catch(() => undefined);
  return promise;
}

/** What the core answers `lock` and the lock settings while the app lock is off. */
export function lockNotEnabled(): CoreError {
  return invalid("lock", "the app lock is not enabled");
}

/** A wrong app-lock passphrase, for the request field that carried it. */
export function wrongPassphrase(field = "passphrase"): CoreError {
  return invalid(field, "wrong passphrase");
}

function noKit(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** A device log entry as the mirror keeps it and a backup carries it. */
export interface DeviceRecord {
  deviceId: DeviceId;
  /** The name the log recorded when the device joined (the log is append-only). */
  name: string;
  addedAt: number;
  addedViaKit: boolean;
  revokedAt?: number;
  /** Fake Ed25519 public key, lowercase hex. */
  signingKey: string;
  /** The device that signed the addition (none for the first device and for kit additions). */
  addedBy?: DeviceId;
  /** The device that signed the revocation. */
  revokedBy?: DeviceId;
}

/** What a backup carries of the security side; the account and recovery ids travel in its header. */
export interface SecurityState {
  kit: RecoveryKitInfo;
  devices: DeviceRecord[];
  /** Keyring tombstones (purged pages), by space. */
  tombstones: { spaceId: SpaceId; pageIds: PageId[] }[];
  /** The root key's generation (absent in backups written before Milestone 7: 0). */
  rootGeneration?: number;
}

/** Names of the other devices a seeded device log holds, in order (then "Device 6", ...). */
const OTHER_DEVICE_NAMES = ["Laptop", "Work desktop", "Phone", "Tablet"];

/**
 * The devices side of the starting situation (`MemoryTransportOptions.devices`, for the dev
 * page's `?devices=revoked|rotation`). Every option needs a seeded kit (`security.kitCreatedAt`):
 * a device log belongs to an account.
 */
export interface MemoryDevicesOptions {
  /**
   * Devices in the log, this one included (default 1). The others are active, added by this
   * device when the kit was created, named "Laptop", "Work desktop", "Phone", "Tablet", then
   * "Device 6" and so on.
   */
  devices?: number;
  /**
   * This device was removed from the account by the first other device (one is added when
   * `devices` leaves none): `SecurityInfo.thisDeviceRevoked`, and sync is off (a `sync` seed is
   * ignored).
   */
  thisDeviceRevoked?: boolean;
  /** A rotation made on another device waits here for `RotationApply` (as `security.rotationPending`). */
  rotationPending?: boolean;
}

/**
 * Changes to the device log that arrive from other devices through sync
 * (`MemoryTransport.receiveDeviceLog`), applied as the core applies them.
 */
export interface DeviceLogChange {
  /** Names of devices another device added. */
  added?: readonly string[];
  /** Other active devices another device revoked. */
  revoked?: readonly DeviceId[];
  /** Another device revoked this one. */
  revokeThisDevice?: boolean;
  /**
   * Another device rotated the root key: applied at once while the app lock is off, pending
   * until `RotationApply` while it is on (never for a revoked device, which gets no new key).
   */
  rotate?: boolean;
}

/** The data of a `DevicesChanged` event. */
export type DevicesChange = Extract<Event, { type: "DevicesChanged" }>["data"];

/** The account and kit a backup belongs to (its header). */
export interface BackupIdentity {
  accountId: AccountId;
  recoveryId: string;
}

/**
 * The starting situation of the security side (`MemoryTransportOptions.security`), so UI
 * tests can start after onboarding issued a kit, with the app lock on, or locked.
 */
export interface MemorySecurityOptions {
  /**
   * Start with a recovery kit issued at this time (Unix ms), the device log holding this
   * device. Needs `MemoryTransportOptions.workspaceName`: a kit belongs to a workspace.
   */
  kitCreatedAt?: number;
  /** With `kitCreatedAt`: when the kit was last verified (default: never). */
  kitVerifiedAt?: number;
  /**
   * With `kitCreatedAt`: the kit's 32-byte key (default: random), so a test can type the
   * kit, for example `(await kitWordsFromEntropy(kitKey)).join(" ")`.
   */
  kitKey?: Uint8Array;
  /**
   * The account. With `kitCreatedAt` it is the kit's account (default: a fresh id from
   * `newId`, taken after the seeded spaces and pages). Without a kit it stands for an
   * installation that still holds that account's root key (its keystore survived while the
   * data went): `backupImport` then restores that account's backups without the kit.
   */
  accountId?: AccountId;
  /** Start with the app lock enabled under this passphrase (8 to 256 UTF-8 bytes). */
  appLockPassphrase?: string;
  /** Start in the `"locked"` core state, as an app opening a locked workspace; needs `appLockPassphrase`. */
  startLocked?: boolean;
  /**
   * With `kitCreatedAt`: a root key rotation made on another device waits for `RotationApply`
   * (`SecurityInfo.rotationPending`), as on a device whose app lock kept it from switching.
   */
  rotationPending?: boolean;
}

/** What the security mirror needs from its owner. */
export interface SecurityDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Id generator (UUID text), for the account id. */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Spaces and pages, for the keyring counts. */
  structure: MemoryStructure;
  /** Page documents, for the keyring counts. */
  content: MemoryContent;
  /** This device as `DeviceInfo` reports it now (the current name). */
  device: () => DeviceInfo;
  /** Whether the workspace exists: a kit needs one. */
  hasWorkspace: () => boolean;
  /** Attachment key records and attachment tombstones, for the keyring counts; none without it. */
  attachments?: () => { keys: number; tombstones: number };
}

interface KitState {
  createdAt: number;
  verifiedAt: number | undefined;
  reissuedAt: number | undefined;
  /**
   * The kit opened this installation (a recovery from the server, a restore with the kit): it
   * may be in other hands, so the UI asks for a re-issue. A re-issue clears it.
   */
  stale: boolean;
  /** The current key's recovery id (a promise: a seeded kit is hashed in the background). */
  recoveryId: Promise<string>;
}

interface Issued {
  words: string[];
  code: string;
  recoveryId: string;
}

function kitInfo(kit: KitState): RecoveryKitInfo {
  // Optional fields are left out when unset, as the core's serialisation does.
  const info: RecoveryKitInfo = { createdAt: kit.createdAt, stale: kit.stale };
  if (kit.verifiedAt !== undefined) {
    info.verifiedAt = kit.verifiedAt;
  }
  if (kit.reissuedAt !== undefined) {
    info.reissuedAt = kit.reissuedAt;
  }
  return info;
}

/**
 * The recovery kit, the app lock, the device log and the keyring of the mirror. Methods
 * throw (or reject with) `CoreError`s where the core answers with an `ApiError`, and emit
 * `SecurityChanged` after every change.
 */
export class MemorySecurity {
  readonly #deps: SecurityDeps;
  #accountId: AccountId | undefined;
  #kit: KitState | undefined;
  /** Hash of the app-lock passphrase while the lock is enabled. */
  #lock: Promise<string> | undefined;
  #devices: DeviceRecord[] = [];
  /** The keyring's tombstones: purged pages by space. */
  #tombstones = new Map<SpaceId, Set<PageId>>();
  /** Generation of the root key (0 until a revocation or a rotation). */
  #rootGeneration = 0;
  /** Rotations received from other devices that wait for `RotationApply`. */
  #pendingRotations = 0;
  /** The current kit's recovery blob is on the sync server. */
  #recoveryPublished = false;

  constructor(deps: SecurityDeps) {
    this.#deps = deps;
  }

  /**
   * Applies `MemoryTransportOptions.security` and `.devices`; throws on combinations the core
   * cannot be in.
   */
  seed(options: MemorySecurityOptions, devices: MemoryDevicesOptions = {}): void {
    const { kitCreatedAt, kitVerifiedAt, kitKey, appLockPassphrase } = options;
    if (kitCreatedAt === undefined && (kitVerifiedAt !== undefined || kitKey !== undefined)) {
      throw internalError("MemoryTransport: security.kitVerifiedAt and security.kitKey need security.kitCreatedAt");
    }
    const rotationPending = options.rotationPending === true || devices.rotationPending === true;
    const seedsLog = devices.devices !== undefined || devices.thisDeviceRevoked === true || rotationPending;
    if (kitCreatedAt === undefined && seedsLog) {
      throw internalError("MemoryTransport: the devices options and security.rotationPending need security.kitCreatedAt");
    }
    if (devices.devices !== undefined && (!Number.isInteger(devices.devices) || devices.devices < 1)) {
      throw internalError("MemoryTransport: devices.devices must be a whole number of at least 1");
    }
    if (kitCreatedAt !== undefined && !this.#deps.hasWorkspace()) {
      throw internalError("MemoryTransport: security.kitCreatedAt needs workspaceName (a kit belongs to a workspace)");
    }
    if (kitKey !== undefined && kitKey.length !== KIT_KEY_BYTES) {
      throw internalError(`MemoryTransport: security.kitKey must be ${String(KIT_KEY_BYTES)} bytes`);
    }
    if (appLockPassphrase !== undefined) {
      this.#lock = quiet(passphraseHash(validPassphrase(appLockPassphrase)));
    }
    this.#accountId = options.accountId;
    if (kitCreatedAt !== undefined) {
      const key = kitKey ?? globalThis.crypto.getRandomValues(new Uint8Array(KIT_KEY_BYTES));
      this.#accountId ??= this.#deps.newId();
      this.#kit = {
        createdAt: kitCreatedAt,
        verifiedAt: kitVerifiedAt,
        reissuedAt: undefined,
        stale: false,
        recoveryId: quiet(recoveryIdOf(key)),
      };
      const me = this.#thisDevice(kitCreatedAt, false);
      this.#devices = [me];
      const total = Math.max(devices.devices ?? 1, devices.thisDeviceRevoked === true ? 2 : 1);
      for (let index = 1; index < total; index += 1) {
        const name = OTHER_DEVICE_NAMES[index - 1] ?? `Device ${String(index + 1)}`;
        this.#devices.push(this.#otherDevice(name, kitCreatedAt, me.deviceId));
      }
      if (devices.thisDeviceRevoked === true) {
        me.revokedAt = this.#deps.now();
        me.revokedBy = this.#devices[1]?.deviceId;
      }
      this.#pendingRotations = rotationPending ? 1 : 0;
    }
  }

  /** The account this installation belongs to (from the kit, a restored backup or the seed). */
  get accountId(): AccountId | undefined {
    return this.#accountId;
  }

  /** Whether the app lock is enabled. */
  get lockEnabled(): boolean {
    return this.#lock !== undefined;
  }

  /** Whether another device removed this one from the account. */
  get thisDeviceRevoked(): boolean {
    const me = this.#deps.device().deviceId;
    return this.#devices.some((device) => device.deviceId === me && device.revokedAt !== undefined);
  }

  /** Whether `candidate` is the app-lock passphrase (always `false` while the lock is off). */
  async passphraseMatches(candidate: string): Promise<boolean> {
    const lock = this.#lock;
    if (lock === undefined) {
      return false;
    }
    const [expected, actual] = await Promise.all([lock, passphraseHash(candidate)]);
    return expected === actual;
  }

  // ----- requests -------------------------------------------------------------------

  /** `SecurityStatus`. */
  status(): SecurityInfo {
    const info: SecurityInfo = {
      appLock: { enabled: this.lockEnabled, ...ARGON2 },
      thisDevice: this.#deps.device().deviceId,
      devices: this.#devices.length,
      keyring: this.#keyring(),
      rootGeneration: this.#rootGeneration,
      rotationPending: this.#pendingRotations > 0,
      thisDeviceRevoked: this.thisDeviceRevoked,
      recoveryPublished: this.#recoveryPublished,
    };
    if (this.#kit !== undefined) {
      info.accountId = this.#account();
      info.recoveryKit = kitInfo(this.#kit);
    }
    return info;
  }

  /**
   * `RecoveryKitCreate`: a workspace is needed (`InvalidInput` for `workspace` otherwise) and
   * no kit may exist (`AlreadyExists`). Sets the account id and writes the device log's first
   * entry (this device) unless a log exists.
   */
  async kitCreate(): Promise<RecoveryKit> {
    this.#requireNoKit();
    const issued = await this.#issue();
    this.#requireNoKit();
    const createdAt = this.#deps.now();
    this.#accountId ??= this.#deps.newId();
    this.#kit = {
      createdAt,
      verifiedAt: undefined,
      reissuedAt: undefined,
      stale: false,
      recoveryId: Promise.resolve(issued.recoveryId),
    };
    if (this.#devices.length === 0) {
      this.#devices.push(this.#thisDevice(createdAt, false));
    }
    this.#recoveryPublished = false;
    this.#deps.emit({ type: "SecurityChanged" });
    return { words: issued.words, code: issued.code, accountId: this.#account(), createdAt };
  }

  /**
   * `RecoveryKitVerify`: `NotFound` without a kit; then the typed kit (either form, see
   * `parseKit`) must decode (`InvalidInput` for `kit` with kb-crypto's message) and be the
   * current kit ("the kit does not match this workspace"). Records `verifiedAt`.
   */
  async kitVerify(text: string): Promise<void> {
    const kit = this.#kit;
    if (kit === undefined) {
      throw noKit("no recovery kit exists yet");
    }
    const key = await parseKit(text);
    const candidate = await recoveryIdOf(key);
    key.fill(0);
    // A kit re-issued while this was checked does not count as verified either.
    if (candidate !== (await kit.recoveryId) || this.#kit !== kit) {
      throw invalid("kit", "the kit does not match this workspace");
    }
    kit.verifiedAt = this.#deps.now();
    this.#deps.emit({ type: "SecurityChanged" });
  }

  /**
   * `RecoveryKitReissue`: a new kit replaces the current one (`NotFound` without a kit), which
   * stops verifying. While the app lock is on, `passphrase` must be given and right
   * (`InvalidInput` for `passphrase`); otherwise it is ignored. Keeps the account id and
   * `createdAt`, sets `reissuedAt`, and clears `verifiedAt` (the new kit is unverified).
   */
  async kitReissue(passphrase: string | undefined): Promise<RecoveryKit> {
    const previous = this.#kit;
    if (previous === undefined) {
      throw noKit("no recovery kit exists yet; create one");
    }
    await this.#requirePassphrase(passphrase, "the app-lock passphrase is required to re-issue the kit");
    const issued = await this.#issue();
    const reissuedAt = this.#replaceKit(previous, issued);
    this.#deps.emit({ type: "SecurityChanged" });
    return { words: issued.words, code: issued.code, accountId: this.#account(), createdAt: reissuedAt };
  }

  // ----- devices (Milestone 7) --------------------------------------------------------

  /**
   * `DeviceRevoke`: removes another active device from the account. This device is refused
   * (`InvalidInput` for `deviceId`), as are unknown and already revoked devices and a revoked
   * device's own attempts; while the app lock is on the passphrase must be given and right
   * (`InvalidInput` for `passphrase`). The device is marked revoked by this one, the root key
   * rotates (generation + 1) and a new kit replaces the old one, which stops verifying.
   * Emits `SecurityChanged`, then `DevicesChanged` with the device and `rotated`.
   */
  async revoke(deviceId: DeviceId, passphrase: string | undefined): Promise<RecoveryKit> {
    const me = this.#deps.device().deviceId;
    const target = this.#revocable(deviceId, me);
    await this.#requirePassphrase(passphrase, "the app-lock passphrase is required to remove a device");
    const issued = await this.#issue();
    // Checked again: the log or the kit may have changed while the passphrase was hashed.
    this.#revocable(deviceId, me);
    const previous = this.#kit;
    if (previous === undefined) {
      throw noKit("no recovery kit exists yet; create one");
    }
    const at = this.#replaceKit(previous, issued);
    target.revokedAt = at;
    target.revokedBy = me;
    this.#rootGeneration += 1;
    this.#deps.emit({ type: "SecurityChanged" });
    this.#deps.emit({
      type: "DevicesChanged",
      data: { added: [], revoked: [deviceId], thisDeviceRevoked: false, rotated: true },
    });
    return { words: issued.words, code: issued.code, accountId: this.#account(), createdAt: at };
  }

  /**
   * `RotationApply`: finishes the rotations another device made while the app lock kept this one
   * from storing the new root key (the passphrase is required while the lock is on:
   * `InvalidInput` for `passphrase`). Nothing pending: the status, unchanged. Emits
   * `SecurityChanged` when it applied something.
   */
  async rotationApply(passphrase: string | undefined): Promise<SecurityInfo> {
    if (this.#pendingRotations === 0) {
      return this.status();
    }
    await this.#requirePassphrase(passphrase, "the app-lock passphrase is required to apply the new keys");
    if (this.#pendingRotations > 0) {
      this.#rootGeneration += this.#pendingRotations;
      this.#pendingRotations = 0;
      this.#deps.emit({ type: "SecurityChanged" });
    }
    return this.status();
  }

  /**
   * Makes this fresh installation a device of an existing account, as a finished pairing or
   * recovery does: the account, its kit (`recoveryId` identifies it; unverified), the other
   * devices of the log, and this device added by `addedBy` (pairing) or via the kit. Emits
   * `SecurityChanged`.
   */
  joinAccount(join: {
    accountId: AccountId;
    recoveryId: string;
    others: readonly DeviceRecord[];
    addedBy?: DeviceId;
    viaKit: boolean;
  }): void {
    const now = this.#deps.now();
    this.#accountId = join.accountId;
    this.#kit = {
      createdAt: now,
      verifiedAt: undefined,
      reissuedAt: undefined,
      // A recovery opened the account with the kit; a pairing handed it over (still the account's own).
      stale: join.viaKit,
      recoveryId: Promise.resolve(join.recoveryId),
    };
    this.#devices = [...join.others.map((device) => ({ ...device })), this.#thisDevice(now, join.viaKit, join.addedBy)];
    this.#tombstones = new Map();
    this.#rootGeneration = 0;
    this.#pendingRotations = 0;
    this.#recoveryPublished = false;
    this.#deps.emit({ type: "SecurityChanged" });
  }

  /** A device of the account other than this one, as a log entry records it (for joins and seeds). */
  otherDevice(name: string, addedAt: number, addedBy?: DeviceId): DeviceRecord {
    return this.#otherDevice(name, addedAt, addedBy);
  }

  /**
   * The sponsor confirmed a pairing: the joiner, named `name`, joins the log, added by this
   * device. Emits `SecurityChanged`, then `DevicesChanged` with the joiner, as the core does.
   */
  deviceAdded(name: string): void {
    const me = this.#deps.device().deviceId;
    const joiner = this.#otherDevice(name, this.#deps.now(), me);
    this.#devices.push(joiner);
    this.#deps.emit({ type: "SecurityChanged" });
    this.#deps.emit({
      type: "DevicesChanged",
      data: { added: [this.#entry(joiner, me)], revoked: [], thisDeviceRevoked: false, rotated: false },
    });
  }

  /**
   * Applies device log entries that arrived from other devices (see `DeviceLogChange`; the
   * signer is the first other active device) and emits `SecurityChanged`, then `DevicesChanged`
   * with what changed. A test hook's misuse (no kit, a device that cannot be revoked) is
   * `Internal`.
   */
  receiveLog(change: DeviceLogChange): DevicesChange {
    if (this.#kit === undefined) {
      throw internalError("MemoryTransport: a device log arrives only for an account (create the kit first)");
    }
    const me = this.#deps.device().deviceId;
    const revokedIds = new Set(change.revoked ?? []);
    const signer = this.#devices.find(
      (device) => device.deviceId !== me && device.revokedAt === undefined && !revokedIds.has(device.deviceId),
    )?.deviceId;
    const now = this.#deps.now();
    const targets = [...revokedIds].map((deviceId) => {
      const target = this.#devices.find((device) => device.deviceId === deviceId);
      if (target === undefined || target.revokedAt !== undefined || deviceId === me) {
        throw internalError(`MemoryTransport: ${deviceId} is not another active device of the log`);
      }
      return target;
    });
    const added = (change.added ?? []).map((name) => this.#otherDevice(name, now, signer));
    this.#devices.push(...added);
    for (const target of targets) {
      target.revokedAt = now;
      if (signer !== undefined) {
        target.revokedBy = signer;
      }
    }
    const revokeThisDevice = change.revokeThisDevice === true && !this.thisDeviceRevoked;
    if (revokeThisDevice) {
      const record = this.#devices.find((device) => device.deviceId === me);
      if (record !== undefined) {
        record.revokedAt = now;
        if (signer !== undefined) {
          record.revokedBy = signer;
        }
      }
    }
    const rotated = change.rotate === true;
    if (rotated && !this.thisDeviceRevoked) {
      if (this.lockEnabled) {
        this.#pendingRotations += 1;
      } else {
        this.#rootGeneration += 1;
      }
    }
    const data: DevicesChange = {
      added: added.map((device) => this.#entry(device, me)),
      revoked: targets.map((target) => target.deviceId),
      thisDeviceRevoked: revokeThisDevice,
      rotated,
    };
    this.#deps.emit({ type: "SecurityChanged" });
    this.#deps.emit({ type: "DevicesChanged", data });
    return data;
  }

  /**
   * The sync driver uploaded the recovery blob of the current kit (a healthy push with a kit):
   * `recoveryPublished` becomes `true`, with `SecurityChanged`. Any new kit unpublishes it.
   */
  recoveryUploaded(): void {
    if (this.#kit !== undefined && !this.#recoveryPublished) {
      this.#recoveryPublished = true;
      this.#deps.emit({ type: "SecurityChanged" });
    }
  }

  /** `AppLockEnable`: 8 to 256 bytes (`InvalidInput` for `passphrase`), `AlreadyExists` when on. */
  async lockEnable(passphrase: string): Promise<void> {
    validPassphrase(passphrase);
    this.#requireLockOff();
    const hash = await passphraseHash(passphrase);
    this.#requireLockOff();
    this.#lock = Promise.resolve(hash);
    this.#deps.emit({ type: "SecurityChanged" });
  }

  /** `AppLockDisable`: the current passphrase (`InvalidInput` for `passphrase` when wrong). */
  async lockDisable(passphrase: string): Promise<void> {
    if (this.#lock === undefined) {
      throw lockNotEnabled();
    }
    if (!(await this.passphraseMatches(passphrase))) {
      throw wrongPassphrase();
    }
    this.#lock = undefined;
    this.#deps.emit({ type: "SecurityChanged" });
  }

  /**
   * `AppLockChange`, checked in the core's order: the lock must be on (`InvalidInput` for
   * `lock`), the new passphrase 8 to 256 bytes (`InvalidInput` for `new`) and the current one
   * right (`InvalidInput` for `current`).
   */
  async lockChange(current: string, next: string): Promise<void> {
    if (this.#lock === undefined) {
      throw lockNotEnabled();
    }
    if (!passphraseLengthOk(next)) {
      throw invalid("new", `must be ${String(PASSPHRASE_MIN_BYTES)} to ${String(PASSPHRASE_MAX_BYTES)} bytes`);
    }
    if (!(await this.passphraseMatches(current))) {
      throw wrongPassphrase("current");
    }
    this.#lock = Promise.resolve(await passphraseHash(next));
    this.#deps.emit({ type: "SecurityChanged" });
  }

  /** `DeviceList`: the log in order; this device under its current name. Empty before the kit. */
  deviceList(): DeviceEntry[] {
    const me = this.#deps.device().deviceId;
    return this.#devices.map((device) => this.#entry(device, me));
  }

  // ----- keyring hooks --------------------------------------------------------------

  /** `PagePurge` writes a document tombstone for every purged page (crypto-erase). */
  pagesPurged(spaceId: SpaceId, pageIds: readonly PageId[]): void {
    const tombstones = this.#tombstones.get(spaceId) ?? new Set<PageId>();
    for (const pageId of pageIds) {
      tombstones.add(pageId);
    }
    this.#tombstones.set(spaceId, tombstones);
  }

  /** `SpaceDelete` drops the space's keyring rows, tombstones included. */
  spaceDeleted(spaceId: SpaceId): void {
    this.#tombstones.delete(spaceId);
  }

  // ----- backups --------------------------------------------------------------------

  /** The account, the kit's recovery id and the state a backup carries; `undefined` without a kit. */
  async backupState(): Promise<{ identity: BackupIdentity; state: SecurityState } | undefined> {
    const kit = this.#kit;
    if (kit === undefined) {
      return undefined;
    }
    return {
      identity: { accountId: this.#account(), recoveryId: await kit.recoveryId },
      state: {
        kit: kitInfo(kit),
        devices: this.#devices.map((device) => ({ ...device })),
        tombstones: [...this.#tombstones].map(([spaceId, pageIds]) => ({ spaceId, pageIds: [...pageIds] })),
        rootGeneration: this.#rootGeneration,
      },
    };
  }

  /**
   * Returns the function that makes a restored backup's account, kit, device log and
   * tombstones this installation's, with the app lock off. When the kit opened the backup
   * (`viaKit`) and this device is not in the log, it joins it "added via kit".
   */
  prepareRestore(identity: BackupIdentity, state: SecurityState, viaKit: boolean): () => void {
    const devices = state.devices.map((device) => ({ ...device }));
    const tombstones = new Map(state.tombstones.map(({ spaceId, pageIds }) => [spaceId, new Set(pageIds)]));
    const kit: KitState = {
      createdAt: state.kit.createdAt,
      verifiedAt: state.kit.verifiedAt,
      reissuedAt: state.kit.reissuedAt,
      // Restoring with the kit makes it stale; without it the backup's own flag stands.
      stale: viaKit || state.kit.stale,
      recoveryId: Promise.resolve(identity.recoveryId),
    };
    return () => {
      const me = this.#deps.device();
      if (viaKit && !devices.some((device) => device.deviceId === me.deviceId)) {
        devices.push(this.#thisDevice(this.#deps.now(), true));
      }
      this.#accountId = identity.accountId;
      this.#kit = kit;
      this.#lock = undefined;
      this.#devices = devices;
      this.#tombstones = tombstones;
      this.#rootGeneration = state.rootGeneration ?? 0;
      this.#pendingRotations = 0;
      this.#recoveryPublished = false;
    };
  }

  // ----- plumbing -------------------------------------------------------------------

  #requireNoKit(): void {
    if (!this.#deps.hasWorkspace()) {
      throw invalid("workspace", "create the workspace first");
    }
    if (this.#kit !== undefined) {
      throw new CoreError({
        code: "AlreadyExists",
        message: "a recovery kit already exists; re-issue it instead",
      });
    }
  }

  /** While the app lock is on, `passphrase` must be given (`missing` otherwise) and right. */
  async #requirePassphrase(passphrase: string | undefined, missing: string): Promise<void> {
    if (this.#lock === undefined) {
      return;
    }
    if (passphrase === undefined) {
      throw invalid("passphrase", missing);
    }
    if (!(await this.passphraseMatches(passphrase))) {
      throw wrongPassphrase();
    }
  }

  /** Replaces the kit with `issued` (same account and creation time, unverified, unpublished); answers the time. */
  #replaceKit(previous: KitState, issued: Issued): number {
    const reissuedAt = this.#deps.now();
    this.#kit = {
      createdAt: previous.createdAt,
      verifiedAt: undefined,
      reissuedAt,
      stale: false,
      recoveryId: Promise.resolve(issued.recoveryId),
    };
    this.#recoveryPublished = false;
    return reissuedAt;
  }

  /** The record of a device this one may revoke; `InvalidInput` for `deviceId` otherwise. */
  #revocable(deviceId: DeviceId, me: DeviceId): DeviceRecord {
    if (this.thisDeviceRevoked) {
      throw invalid("deviceId", "this device was removed from the account");
    }
    if (deviceId === me) {
      throw invalid("deviceId", "this device cannot remove itself; remove it from another device");
    }
    const target = this.#devices.find((device) => device.deviceId === deviceId && device.revokedAt === undefined);
    if (target === undefined) {
      throw invalid("deviceId", "no active device of this account has this id");
    }
    return target;
  }

  /** A log record as `DeviceList` and `DevicesChanged` report it; this device under its current name. */
  #entry(device: DeviceRecord, me: DeviceId): DeviceEntry {
    const isThisDevice = device.deviceId === me;
    const entry: DeviceEntry = {
      deviceId: device.deviceId,
      name: isThisDevice ? this.#deps.device().name : device.name,
      addedAt: device.addedAt,
      addedViaKit: device.addedViaKit,
      isThisDevice,
      signingKey: device.signingKey,
    };
    if (device.revokedAt !== undefined) {
      entry.revokedAt = device.revokedAt;
    }
    if (device.addedBy !== undefined) {
      entry.addedBy = device.addedBy;
    }
    if (device.revokedBy !== undefined) {
      entry.revokedBy = device.revokedBy;
    }
    return entry;
  }

  #otherDevice(name: string, addedAt: number, addedBy: DeviceId | undefined): DeviceRecord {
    const deviceId = this.#deps.newId();
    const record: DeviceRecord = { deviceId, name, addedAt, addedViaKit: false, signingKey: fakeSigningKey(deviceId) };
    if (addedBy !== undefined) {
      record.addedBy = addedBy;
    }
    return record;
  }

  #requireLockOff(): void {
    if (this.#lock !== undefined) {
      throw new CoreError({ code: "AlreadyExists", message: "the app lock is already enabled" });
    }
  }

  #account(): AccountId {
    if (this.#accountId === undefined) {
      throw internalError("a recovery kit exists without an account id");
    }
    return this.#accountId;
  }

  /** A fresh kit: 32 random bytes, encoded and hashed, then erased. */
  async #issue(): Promise<Issued> {
    const key = globalThis.crypto.getRandomValues(new Uint8Array(KIT_KEY_BYTES));
    try {
      const [words, code, recoveryId] = await Promise.all([
        kitWordsFromEntropy(key),
        kitCodeFromEntropy(key),
        recoveryIdOf(key),
      ]);
      return { words, code, recoveryId };
    } finally {
      key.fill(0);
    }
  }

  #thisDevice(addedAt: number, addedViaKit: boolean, addedBy?: DeviceId): DeviceRecord {
    const me = this.#deps.device();
    const record: DeviceRecord = {
      deviceId: me.deviceId,
      name: me.name,
      addedAt,
      addedViaKit,
      signingKey: fakeSigningKey(me.deviceId),
    };
    if (addedBy !== undefined) {
      record.addedBy = addedBy;
    }
    return record;
  }

  /**
   * Keyring counts as the core's hooks would leave them: one space key per space; one document
   * key per structure document (one per space) and per page document created so far, unless
   * the page was purged; one attachment key per finished attachment; one tombstone per purged
   * page and per purged attachment of a live space.
   */
  #keyring(): KeyringInfo {
    const { structure, content } = this.#deps;
    const attachments = this.#deps.attachments?.() ?? { keys: 0, tombstones: 0 };
    const spaces = structure.spaceIds().length;
    const purged = new Set<PageId>();
    for (const pageIds of this.#tombstones.values()) {
      pageIds.forEach((pageId) => purged.add(pageId));
    }
    const pageDocuments = content
      .documentPageIds()
      .filter((pageId) => structure.pageMeta(pageId) !== undefined && !purged.has(pageId)).length;
    return {
      spaces,
      documents: spaces + pageDocuments,
      attachments: attachments.keys,
      tombstones: purged.size + attachments.tombstones,
    };
  }
}
