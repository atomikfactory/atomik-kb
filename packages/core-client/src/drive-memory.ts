// An in-memory `DriveApi` (ADR-0059, `docs/architecture/drive-backup.md` sections 3 and 4): what the dev page (`?drive=fake`) and
// the UI tests use instead of Google. It behaves like the desktop shell as far as the contract goes: a connection that starts
// with a (pretend) native consent dialog and waits for a (pretend) browser, the same refusals with the same `details.reason`
// (`DRIVE_ERROR_REASONS`), a list of fake backups, a download with progress into the `MemoryTransport`'s fake disk (so
// `backupImport(kit?, path)` restores it), `cancel`, and the connection kept in the setting `app.backup.drive` through the
// transport's shell-setting path (so `SettingChanged` fires). It sends and stores nothing outside the page.
//
// The scheduled run of the transport calls `connected` and `deliver` (the shell's `drive_host::deliver`): a backup uploaded
// joins the list, the newest `keep` scheduled ones stay, and the outcome is `last` in the setting.

import type { BackupFileInfo, DriveApi, DriveClientInput, DriveErrorReason, DriveProgress, DriveStatus } from "./drive";
import { CoreError, toCoreError } from "./errors";
import type { ErrorCode } from "./generated/ErrorCode";
import type { BackupRunStatus } from "./generated/BackupRunStatus";
import type { DriveConnection } from "./generated/DriveConnection";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { BACKUP_DRIVE_SETTING, backupsToPrune, isAutoBackupName } from "./memory-backup-schedule";

/** The address of the fake account. */
export const FAKE_DRIVE_ACCOUNT = "fake.user@example.com";
/** The id of the fake "atomik-kb backups" folder. */
export const FAKE_DRIVE_FOLDER_ID = "fake-atomik-kb-backups";
/** Where the fake folder is "in Drive" (what `status().folderUrl` says). */
export const FAKE_DRIVE_FOLDER_URL = `https://drive.google.com/drive/folders/${FAKE_DRIVE_FOLDER_ID}`;
/** Where `download` puts a backup on the fake disk (the shell's `<app data>/restore-staging/`). */
export const FAKE_RESTORE_STAGING = "/restore-staging";
/** The most backups `listBackups` answers (`kb-drive`'s `MAX_LISTED`). */
const MAX_LISTED = 200;
const DAY_MS = 86_400_000;

/** What can fail, for {@link MemoryDriveOptions.fail} and {@link MemoryDrive.setFailure}. */
export type MemoryDriveOperation = "connect" | "list" | "download" | "deliver";

/** A backup in the fake Drive folder. */
export interface MemoryDriveBackup {
  /** The file's name (a scheduled backup's name is `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb`). */
  name: string;
  /**
   * What a download writes to the fake disk: bytes, or a function that makes them when the file is downloaded (so a restorable
   * backup can be exported then). Default: a few bytes that are not a backup (restoring them fails with `Crypto`).
   */
  bytes?: Uint8Array | (() => Uint8Array | Promise<Uint8Array>);
  /** The size listed, in bytes; default the bytes' length (a function or no bytes: 4096). */
  size?: number;
  /** When it was modified, Unix ms; default a day earlier for each position, so the first of the list is the newest. */
  modifiedAt?: number;
}

/** How the fake starts; every field is optional. */
export interface MemoryDriveOptions {
  /** Whether the person agrees in the (pretend) native consent dialog; default true. */
  consent?: boolean;
  /** This build has no Google client and none was set: the status says `none` and connecting fails with `noClient`. */
  noClient?: boolean;
  /** The Google account's address; default {@link FAKE_DRIVE_ACCOUNT}. */
  account?: string;
  /** Starts connected (the setting `app.backup.drive` is written without an event, unless it is already stored). */
  connected?: boolean;
  /** The backups already in the Drive folder; default none. */
  backups?: readonly MemoryDriveBackup[];
  /** Operations that fail, each with the reason a failure of the shell carries (`DRIVE_ERROR_REASONS`), until it is reset. */
  fail?: Partial<Record<MemoryDriveOperation, DriveErrorReason>>;
  /** Milliseconds the sign-in in the browser takes; default 30. `Infinity` waits for `cancel`. */
  connectMs?: number;
  /** Milliseconds between the progress calls of a download; default 20. `Infinity` waits for `cancel`. */
  stepMs?: number;
  /** How many progress calls a download makes; default 4 (the last one is complete). */
  steps?: number;
}

/**
 * What the fake needs from the transport it lives in (a `MemoryTransport` hands it its own): the fake disk, the clock and the
 * settings. Alone, the fake keeps its own.
 */
export interface MemoryDriveHost {
  /** The fake disk of the transport (path to bytes). */
  readonly files: Map<string, Uint8Array>;
  /** The clock, Unix ms. */
  now(): number;
  /** A stored setting; `undefined` when it is not set. */
  setting(key: string): JsonValue | undefined;
  /** Stores a setting the shell owns, without an event (the starting situation). */
  seedSetting(key: string, value: JsonValue): void;
  /** Stores a setting the shell owns and announces it (`SettingChanged`). */
  writeSetting(key: string, value: JsonValue): void;
}

/** The error codes of the failures; a guess at the shell's, which is not part of the contract (`driveReasonOf` is). */
const CODES: Record<DriveErrorReason, ErrorCode> = {
  consentDeclined: "InvalidInput",
  cancelled: "InvalidInput",
  noClient: "InvalidInput",
  browserTimeout: "InvalidInput",
  denied: "InvalidInput",
  reconnect: "InvalidInput",
  quota: "Storage",
  notConnected: "InvalidInput",
  notFound: "NotFound",
  network: "Internal",
  server: "Internal",
};

/** Generic messages, as the shell's: never a token, an id or a name. */
const MESSAGES: Record<DriveErrorReason, string> = {
  consentDeclined: "Google Drive was not connected: the notice was declined",
  cancelled: "the operation was cancelled",
  noClient: "this build has no Google client: set your own one first",
  browserTimeout: "the sign-in in the browser did not finish in time",
  denied: "Google Drive access was refused",
  reconnect: "Google Drive must be connected again",
  quota: "Google Drive has no room left",
  notConnected: "Google Drive is not connected",
  notFound: "the backup is not in Google Drive",
  network: "Google Drive could not be reached",
  server: "Google Drive answered with an error",
};

/** The failure of a Drive command: `details.reason` says why (see `driveReasonOf`). */
export function driveFailure(reason: DriveErrorReason): CoreError {
  return new CoreError({ code: CODES[reason], message: MESSAGES[reason], details: { field: "drive", reason } });
}

/** What the host of a lone fake keeps. */
function loneHost(): MemoryDriveHost {
  const settings = new Map<string, JsonValue>();
  return {
    files: new Map<string, Uint8Array>(),
    now: () => Date.now(),
    setting: (key) => settings.get(key),
    seedSetting: (key, value) => {
      settings.set(key, value);
    },
    writeSetting: (key, value) => {
      settings.set(key, value);
    },
  };
}

interface StoredBackup {
  id: string;
  name: string;
  bytes: Uint8Array | (() => Uint8Array | Promise<Uint8Array>);
  size: number;
  modifiedAt: number;
}

/** The text of an error as `BackupRunStatus.error` carries it: the code and the message. */
function errorText(error: CoreError): string {
  return `${error.code}: ${error.message}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The connection a stored `app.backup.drive` describes; what is not understood is left out. */
function connectionOf(stored: JsonValue | undefined): DriveConnection {
  const raw: Record<string, unknown> = isRecord(stored) ? stored : {};
  const connection: DriveConnection = { needsReconnect: raw.needsReconnect === true };
  if (typeof raw.account === "string") {
    connection.account = raw.account;
  }
  if (typeof raw.folderId === "string") {
    connection.folderId = raw.folderId;
  }
  if (typeof raw.connectedAt === "number") {
    connection.connectedAt = raw.connectedAt;
  }
  if (typeof raw.ownClientId === "string") {
    connection.ownClientId = raw.ownClientId;
  }
  if (isRecord(raw.last) && typeof raw.last.at === "number" && typeof raw.last.ok === "boolean") {
    connection.last = raw.last as unknown as BackupRunStatus;
  }
  return connection;
}

/** The fake Google Drive target. */
export class MemoryDrive implements DriveApi {
  readonly #options: MemoryDriveOptions;
  readonly #host: MemoryDriveHost;
  readonly #fail = new Map<MemoryDriveOperation, DriveErrorReason>();
  readonly #backups: StoredBackup[] = [];
  /** What ends the sign-in or the download that waits (see `cancel`). */
  readonly #cancelers = new Set<() => void>();
  #consent: boolean;
  #nextId = 1;
  #staged: string | undefined;

  constructor(options: MemoryDriveOptions = {}, host: MemoryDriveHost = loneHost()) {
    this.#options = options;
    this.#host = host;
    this.#consent = options.consent ?? true;
    for (const [operation, reason] of Object.entries(options.fail ?? {}) as [MemoryDriveOperation, DriveErrorReason][]) {
      this.#fail.set(operation, reason);
    }
    const now = host.now();
    (options.backups ?? []).forEach((backup, position) => {
      this.#backups.push(this.#stored(backup, now - position * DAY_MS));
    });
    if (options.connected === true && host.setting(BACKUP_DRIVE_SETTING) === undefined) {
      host.seedSetting(BACKUP_DRIVE_SETTING, this.#asJson(this.#connect(connectionOf(undefined), now)));
    }
  }

  // ----- what a test can look at and steer -------------------------------------------------------------------------------

  /** Whether a connection is stored (what the scheduled run asks). A connection that needs reconnecting is still one. */
  get connected(): boolean {
    return this.#connection().connectedAt !== undefined;
  }

  /**
   * The name of the newest scheduled backup Drive got, by its record in `app.backup.drive` (its last success), when it has had
   * one: what the scheduled run asks before it delivers a file again.
   */
  get newestDelivered(): string | undefined {
    const last = this.#connection().last;
    return last?.lastSuccessAt === undefined ? undefined : last.file;
  }

  /** The backups in the fake Drive folder, newest first (every one, not only the 200 a listing answers). */
  get backups(): readonly BackupFileInfo[] {
    return this.#sorted().map((backup) => this.#info(backup));
  }

  /** The file `download` last wrote on the fake disk, while it is still there. */
  get stagedPath(): string | undefined {
    return this.#staged;
  }

  /** Makes an operation fail with `reason` from now on, or work again with `undefined`. */
  setFailure(operation: MemoryDriveOperation, reason: DriveErrorReason | undefined): void {
    if (reason === undefined) {
      this.#fail.delete(operation);
    } else {
      this.#fail.set(operation, reason);
    }
  }

  /** Whether the person agrees in the pretend consent dialog from now on. */
  setConsent(agree: boolean): void {
    this.#consent = agree;
  }

  // ----- DriveApi ----------------------------------------------------------------------------------------------------------

  status(): Promise<DriveStatus> {
    return Promise.resolve(this.#status());
  }

  async connect(): Promise<DriveStatus> {
    if (this.#client() === "none") {
      throw driveFailure("noClient");
    }
    if (!this.#consent) {
      throw driveFailure("consentDeclined");
    }
    await this.#wait(this.#options.connectMs ?? 30);
    const failure = this.#fail.get("connect");
    if (failure !== undefined) {
      throw driveFailure(failure);
    }
    // The next connection clears a refused token; the own client and the last upload stay.
    const kept = this.#connection();
    const base: DriveConnection = { needsReconnect: false };
    if (kept.ownClientId !== undefined) {
      base.ownClientId = kept.ownClientId;
    }
    if (kept.last !== undefined) {
      base.last = kept.last;
    }
    this.#store(this.#connect(base, this.#host.now()));
    return this.#status();
  }

  disconnect(): Promise<DriveStatus> {
    this.#store(this.#forgotten(this.#connection()));
    return Promise.resolve(this.#status());
  }

  setClient(client: DriveClientInput | null): Promise<DriveStatus> {
    let connection = this.#forgotten(this.#connection());
    if (client === null) {
      delete connection.ownClientId;
    } else {
      const clientId = client.clientId.trim();
      if (clientId === "") {
        return Promise.reject(new CoreError({ code: "InvalidInput", message: "the client id is empty", details: { field: "clientId" } }));
      }
      connection = { ...connection, ownClientId: clientId };
    }
    this.#store(connection);
    return Promise.resolve(this.#status());
  }

  listBackups(): Promise<BackupFileInfo[]> {
    try {
      this.#usable("list");
    } catch (error) {
      return Promise.reject(toCoreError(error));
    }
    return Promise.resolve(this.#sorted().slice(0, MAX_LISTED).map((backup) => this.#info(backup)));
  }

  async download(fileId: string, onProgress: (progress: DriveProgress) => void): Promise<string> {
    this.#usable("download");
    const backup = this.#backups.find((candidate) => candidate.id === fileId);
    if (backup === undefined) {
      throw driveFailure("notFound");
    }
    const steps = Math.max(1, Math.floor(this.#options.steps ?? 4));
    for (let step = 1; step <= steps; step += 1) {
      await this.#wait(this.#options.stepMs ?? 20);
      onProgress({ received: Math.round((backup.size * step) / steps), total: backup.size });
    }
    const bytes = typeof backup.bytes === "function" ? await backup.bytes() : backup.bytes;
    // A previous staging file is deleted first.
    if (this.#staged !== undefined) {
      this.#host.files.delete(this.#staged);
    }
    this.#staged = `${FAKE_RESTORE_STAGING}/${String(this.#nextId++)}-${backup.name}`;
    this.#host.files.set(this.#staged, bytes);
    return this.#staged;
  }

  cancel(): Promise<void> {
    for (const cancel of [...this.#cancelers]) {
      cancel();
    }
    return Promise.resolve();
  }

  // ----- the scheduled run ---------------------------------------------------------------------------------------------

  /**
   * What the scheduled run does with the backup it exported (the shell's `drive_host::deliver`): uploads it, keeps the newest
   * `keep` scheduled backups of the folder, records the outcome in `app.backup.drive` (`last`). Resolves `false` when Drive is
   * not connected (nothing is sent, no error); rejects with the failure the shell would report (a refused token sets
   * `needsReconnect`).
   */
  deliver(name: string, bytes: Uint8Array, keep: number): Promise<boolean> {
    try {
      return Promise.resolve(this.#deliver(name, bytes, keep));
    } catch (error) {
      return Promise.reject(toCoreError(error));
    }
  }

  #deliver(name: string, bytes: Uint8Array, keep: number): boolean {
    if (!this.connected) {
      return false;
    }
    try {
      this.#usable("deliver");
    } catch (error) {
      if (error instanceof CoreError) {
        this.#recordFailure(error);
      }
      throw error;
    }
    const at = this.#host.now();
    this.#backups.push({ id: `fake-file-${String(this.#nextId++)}`, name, bytes: bytes.slice(), size: bytes.length, modifiedAt: at });
    const scheduled = this.#backups.filter((backup) => isAutoBackupName(backup.name)).map((backup) => backup.name);
    for (const old of backupsToPrune(scheduled, Math.max(1, keep))) {
      const index = this.#backups.findIndex((backup) => backup.name === old);
      if (index >= 0) {
        this.#backups.splice(index, 1);
      }
    }
    const last: BackupRunStatus = { at, ok: true, lastSuccessAt: at, file: name };
    this.#store({ ...this.#connection(), last });
    return true;
  }

  // ----- private -------------------------------------------------------------------------------------------------------------

  #client(): "builtIn" | "own" | "none" {
    if (this.#connection().ownClientId !== undefined) {
      return "own";
    }
    return this.#options.noClient === true ? "none" : "builtIn";
  }

  #status(): DriveStatus {
    const connection = this.#connection();
    const status: DriveStatus = {
      client: this.#client(),
      connected: this.connected,
      needsReconnect: connection.needsReconnect,
    };
    if (this.connected) {
      if (connection.account !== undefined) {
        status.account = connection.account;
      }
      status.folderUrl = FAKE_DRIVE_FOLDER_URL;
    }
    if (connection.ownClientId !== undefined) {
      status.ownClientId = connection.ownClientId;
    }
    if (connection.last !== undefined) {
      status.last = connection.last;
    }
    return status;
  }

  /** Throws what an operation on the connection fails with: not connected, a refused token, or the failure that was set. */
  #usable(operation: MemoryDriveOperation): void {
    if (!this.connected) {
      throw driveFailure("notConnected");
    }
    if (this.#connection().needsReconnect) {
      throw driveFailure("reconnect");
    }
    const failure = this.#fail.get(operation);
    if (failure !== undefined) {
      throw driveFailure(failure);
    }
  }

  /** The failure of a delivery, in `last`; a refused token also sets `needsReconnect`. */
  #recordFailure(error: CoreError): void {
    const connection = this.#connection();
    const reason = error.details !== undefined && isRecord(error.details) ? error.details.reason : undefined;
    const at = this.#host.now();
    const last: BackupRunStatus = { at, ok: false, error: errorText(error) };
    if (connection.last?.lastSuccessAt !== undefined) {
      last.lastSuccessAt = connection.last.lastSuccessAt;
    }
    if (connection.last?.file !== undefined) {
      last.file = connection.last.file;
    }
    this.#store({ ...connection, needsReconnect: connection.needsReconnect || reason === "reconnect", last });
  }

  #connection(): DriveConnection {
    return connectionOf(this.#host.setting(BACKUP_DRIVE_SETTING));
  }

  /** `connection` as a connected one: the account, the folder and the time. */
  #connect(connection: DriveConnection, at: number): DriveConnection {
    return {
      ...connection,
      account: this.#options.account ?? FAKE_DRIVE_ACCOUNT,
      folderId: FAKE_DRIVE_FOLDER_ID,
      connectedAt: at,
    };
  }

  /** `connection` without the connection itself: the own client and the last upload stay. */
  #forgotten(connection: DriveConnection): DriveConnection {
    const kept: DriveConnection = { needsReconnect: false };
    if (connection.ownClientId !== undefined) {
      kept.ownClientId = connection.ownClientId;
    }
    if (connection.last !== undefined) {
      kept.last = connection.last;
    }
    return kept;
  }

  #store(connection: DriveConnection): void {
    this.#host.writeSetting(BACKUP_DRIVE_SETTING, this.#asJson(connection));
  }

  #asJson(connection: DriveConnection): JsonValue {
    return JSON.parse(JSON.stringify(connection)) as JsonValue;
  }

  #stored(backup: MemoryDriveBackup, defaultTime: number): StoredBackup {
    const bytes = backup.bytes ?? new TextEncoder().encode(`not a backup: ${backup.name}`);
    const size = backup.size ?? (bytes instanceof Uint8Array ? bytes.length : 4096);
    return {
      id: `fake-file-${String(this.#nextId++)}`,
      name: backup.name,
      bytes,
      size,
      modifiedAt: backup.modifiedAt ?? defaultTime,
    };
  }

  /** Newest first; a tie goes to the one that came later. */
  #sorted(): StoredBackup[] {
    return this.#backups
      .map((backup, position) => ({ backup, position }))
      .sort((a, b) => b.backup.modifiedAt - a.backup.modifiedAt || b.position - a.position)
      .map(({ backup }) => backup);
  }

  #info(backup: StoredBackup): BackupFileInfo {
    return { id: backup.id, name: backup.name, size: backup.size, modifiedAt: backup.modifiedAt };
  }

  /** Waits `ms` (for ever when it is not finite); `cancel` ends it with `cancelled`. */
  #wait(ms: number): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const cancel = (): void => {
        if (timer !== undefined) {
          clearTimeout(timer);
        }
        this.#cancelers.delete(cancel);
        reject(driveFailure("cancelled"));
      };
      this.#cancelers.add(cancel);
      if (Number.isFinite(ms)) {
        timer = setTimeout(() => {
          this.#cancelers.delete(cancel);
          resolve();
        }, Math.max(0, ms));
      }
    });
  }
}
