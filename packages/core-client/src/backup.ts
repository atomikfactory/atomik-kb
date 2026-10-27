// The backup file of `MemoryTransport` (`backupExport`/`backupImport`). This is a mirror
// format, NOT the core's `KBB1` file of ADR-0017, and nothing in it is encrypted. It keeps
// what the UI can observe of the real file: a plaintext header naming the account and the
// kit that opens it (the mirror's recovery id, a hash of the kit's key, never the key), a
// body that is checked completely before anything is restored, and the core's errors:
// `Crypto` for any damage, `Storage` for a newer format version.
//
// The file is UTF-8 text of four lines, each ending in `\n`:
//
//   ATOMIK-KB-MEMORY-BACKUP 1
//   {"accountId":"…","createdAt":1700000000000,"recoveryId":"…"}      the header, JSON
//   eyJ3b3Jrc3BhY2UiOnsi…                                             the state: base64 of JSON
//   5d41…                                                              SHA-256 (hex) of lines 1-3
//
// so a changed, missing or extra byte anywhere breaks the layout or the checksum.

import type { AttachmentState, AttachmentsState } from "./attachment-mirror";
import { sha256, toHex } from "./digest";
import { CoreError } from "./errors";
import type { DocumentState } from "./content";
import type { AccountId } from "./generated/AccountId";
import type { BackupSummary } from "./generated/BackupSummary";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyOption } from "./generated/PropertyOption";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RecoveryKitInfo } from "./generated/RecoveryKitInfo";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { Workspace } from "./generated/Workspace";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { OPTION_COLORS, PROPERTY_KINDS, STATUS_GROUPS } from "./properties";
import { ROLLUP_FUNCTIONS } from "./rollup";
import type { DeviceRecord, SecurityState } from "./security";
import type { PageState, SpaceState } from "./structure";

/** First line of a backup file: magic and format version. */
const FIRST_LINE = "ATOMIK-KB-MEMORY-BACKUP 1";
const MAGIC = "ATOMIK-KB-MEMORY-BACKUP ";

const utf8 = new TextEncoder();

/** The plaintext header: what a restore checks before it reads the state. */
export interface BackupHeader {
  accountId: AccountId;
  createdAt: number;
  /** The mirror's recovery id of the kit that opens the backup (see `recoveryIdOf`). */
  recoveryId: string;
}

/** Everything a backup restores. Derived data (search and link indexes) is rebuilt instead. */
export interface BackupState {
  workspace: Workspace;
  settings: { key: string; value: JsonValue }[];
  /** Spaces in display order, with every page, live and trashed. */
  spaces: SpaceState[];
  /** The page documents that exist, as portable JSON. */
  documents: DocumentState[];
  security: SecurityState;
  /** Attachments with their chunks, and the attachment tombstones (files written before Milestone 5 have none). */
  attachments: AttachmentsState;
}

/** `Crypto` with the core's wording for a file that is not a backup or does not parse. */
function badFile(what: string): CoreError {
  return new CoreError({ code: "Crypto", message: `not a valid backup file: ${what}` });
}

/** `Crypto` with the core's wording for a backup that does not authenticate. */
function damaged(): CoreError {
  return new CoreError({ code: "Crypto", message: "the backup is damaged or the key is wrong" });
}

/** Base64 of bytes (also the form of attachment chunks inside a backup's state). */
export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let start = 0; start < bytes.length; start += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000));
  }
  return btoa(binary);
}

/** How many bytes base64 text decodes to (without decoding it). */
function base64Length(text: string): number {
  const padding = text.endsWith("==") ? 2 : text.endsWith("=") ? 1 : 0;
  return (text.length / 4) * 3 - padding;
}

/** Bytes of base64 text; throws on text that is not base64. */
export function fromBase64(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (char) => char.charCodeAt(0));
}

/** The file for `state`. */
export async function encodeBackup(header: BackupHeader, state: BackupState): Promise<Uint8Array> {
  const signed = `${FIRST_LINE}\n${JSON.stringify(header)}\n${toBase64(utf8.encode(JSON.stringify(state)))}\n`;
  return utf8.encode(`${signed}${toHex(await sha256(signed))}\n`);
}

/** What a backup export or import reports: the counts of the restored state, the file size. */
export function backupSummary(header: BackupHeader, state: BackupState, bytes: number): BackupSummary {
  return {
    bytes,
    // One structure document per space plus the page documents.
    documents: state.spaces.length + state.documents.length,
    settings: state.settings.length,
    createdAt: header.createdAt,
    accountId: header.accountId,
    // The bytes of the chunks as stored (plaintext in the mirror, ciphertext in the core).
    chunks: state.attachments.attachments.reduce((sum, attachment) => sum + attachment.chunks.length, 0),
    chunkBytes: state.attachments.attachments.reduce(
      (sum, attachment) => attachment.chunks.reduce((bytes, chunk) => bytes + base64Length(chunk.data), sum),
      0,
    ),
  };
}

/**
 * Checks a file's layout, version and checksum and parses its header; the state stays
 * encoded until {@link readBackupState}. Failures use the core's codes and wording: `Crypto`
 * ("not a valid backup file: …", or "the backup is damaged or the key is wrong" when the
 * checksum fails), and `Storage` for a format version this build does not know.
 */
export async function readBackupFile(bytes: Uint8Array): Promise<{ header: BackupHeader; body: string }> {
  if (bytes.length === 0) {
    throw badFile("empty file");
  }
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    throw badFile("bad magic");
  }
  const lines = text.split("\n");
  const [first = "", headerLine = "", body = "", checksum = "", rest] = lines;
  if (first !== FIRST_LINE) {
    if (first.startsWith(MAGIC)) {
      const format = first.slice(MAGIC.length);
      throw new CoreError({ code: "Storage", message: `this backup needs a newer app version (format ${format})` });
    }
    throw badFile("bad magic");
  }
  if (lines.length !== 5 || rest !== "") {
    throw badFile("layout");
  }
  if (checksum !== toHex(await sha256(`${first}\n${headerLine}\n${body}\n`))) {
    throw damaged();
  }
  try {
    const header = record(JSON.parse(headerLine));
    return {
      header: {
        accountId: string(header.accountId),
        createdAt: number(header.createdAt),
        recoveryId: string(header.recoveryId),
      },
      body,
    };
  } catch {
    throw badFile("header");
  }
}

/** Decodes and parses the state of a file {@link readBackupFile} accepted; `Crypto` when it does not parse. */
export function readBackupState(body: string): BackupState {
  try {
    const state = record(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(fromBase64(body))));
    return {
      workspace: readWorkspace(state.workspace),
      settings: array(state.settings).map((item) => {
        const setting = record(item);
        return { key: string(setting.key), value: json(setting.value) };
      }),
      spaces: array(state.spaces).map(readSpace),
      documents: array(state.documents).map((item) => {
        const document = record(item);
        return { pageId: string(document.pageId), content: json(document.content) };
      }),
      security: readSecurity(state.security),
      attachments:
        state.attachments === undefined ? { attachments: [], tombstones: [] } : readAttachments(state.attachments),
    };
  } catch {
    throw badFile("records");
  }
}

// ----- readers: each returns the typed value or throws (reported as damage above) -------

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new TypeError("expected an object");
  }
  return value as Record<string, unknown>;
}

function array(value: unknown): unknown[] {
  if (!Array.isArray(value)) {
    throw new TypeError("expected an array");
  }
  return value as unknown[];
}

function string(value: unknown): string {
  if (typeof value !== "string") {
    throw new TypeError("expected a string");
  }
  return value;
}

function number(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError("expected a number");
  }
  return value;
}

function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") {
    throw new TypeError("expected a boolean");
  }
  return value;
}

/** Any JSON value (it came out of `JSON.parse`); only a missing one is refused. */
function json(value: unknown): JsonValue {
  if (value === undefined) {
    throw new TypeError("expected a JSON value");
  }
  return value as JsonValue;
}

function readWorkspace(value: unknown): Workspace {
  const workspace = record(value);
  return { id: string(workspace.id), name: string(workspace.name), createdAt: number(workspace.createdAt) };
}

function readPage(value: unknown): PageState {
  const page = record(value);
  const state: PageState = {
    id: string(page.id),
    title: string(page.title),
    createdAt: number(page.createdAt),
    updatedAt: number(page.updatedAt),
    parentId: page.parentId === null ? null : string(page.parentId),
    children: array(page.children).map(string),
  };
  // Backups written before the devices were recorded (ADR-0026, amendment of 2026-10-08) have none.
  if (page.createdBy !== undefined) {
    state.createdBy = string(page.createdBy);
  }
  if (page.editedBy !== undefined) {
    state.editedBy = string(page.editedBy);
  }
  if (page.icon !== undefined) {
    state.icon = string(page.icon);
  }
  // Backups written before covers (ADR-0040) have none.
  if (page.cover !== undefined) {
    state.cover = string(page.cover);
  }
  if (page.coverY !== undefined) {
    state.coverY = number(page.coverY);
  }
  // Backups written before the page look (ADR-0040) have none. A font that is not one of the two reads as the
  // default, a flag that is not `true` as off, like the core reads the node meta.
  if (page.font === "serif" || page.font === "mono") {
    state.font = page.font;
  }
  if (page.smallText === true) {
    state.smallText = true;
  }
  if (page.locked === true) {
    state.locked = true;
  }
  // Backups written before templates (ADR-0056) have neither. A flag that is not `true` reads as off, and a placeholder
  // this version does not know is dropped, like the core reads the node meta.
  if (page.template === true) {
    state.template = true;
  }
  if (typeof page.templateValues === "object" && page.templateValues !== null && !Array.isArray(page.templateValues)) {
    const values: Record<PropertyId, TemplatePlaceholder> = {};
    for (const [propertyId, placeholder] of Object.entries(record(page.templateValues))) {
      if (placeholder === "today" || placeholder === "now" || placeholder === "me") {
        values[propertyId] = placeholder;
      }
    }
    if (Object.keys(values).length > 0) {
      state.templateValues = values;
    }
  }
  if (page.trashedAt !== undefined) {
    state.trashedAt = number(page.trashedAt);
  }
  if (page.trash !== undefined) {
    const trash = record(page.trash);
    state.trash = { seq: number(trash.seq), from: trash.from === null ? null : string(trash.from) };
  }
  // Backups written before properties (ADR-0026) have no values.
  if (page.props !== undefined) {
    state.props = array(page.props).map((item): [PropertyId, PropertyValue] => {
      const [id, value, ...rest] = array(item);
      if (rest.length > 0) {
        throw new TypeError("expected a property id and a value");
      }
      return [string(id), readPropertyValue(value)];
    });
  }
  return state;
}

/** One of `choices`, as the string a backup stores it. */
function oneOf<T extends string>(choices: readonly T[], value: unknown): T {
  const text = string(value);
  const found = choices.find((choice) => choice === text);
  if (found === undefined) {
    throw new TypeError("expected one of the known names");
  }
  return found;
}

/** A property value as a backup stores it: `{ kind, value }`, the value shaped by the kind. */
function readPropertyValue(value: unknown): PropertyValue {
  const stored = record(value);
  const kind = oneOf(PROPERTY_KINDS, stored.kind);
  switch (kind) {
    case "number":
      return { kind, value: number(stored.value) };
    case "checkbox":
      return { kind, value: boolean(stored.value) };
    case "multi_select":
    case "files":
    case "relation":
    case "person":
      return { kind, value: array(stored.value).map(string) };
    case "checklist":
      return {
        kind,
        value: array(stored.value).map((entry) => {
          const item = record(entry);
          return { id: string(item.id), text: string(item.text), done: boolean(item.done) };
        }),
      };
    case "place": {
      const place = record(stored.value);
      const out: { name: string; lat?: number; lon?: number } = { name: string(place.name) };
      if (place.lat !== undefined && place.lon !== undefined) {
        out.lat = number(place.lat);
        out.lon = number(place.lon);
      }
      return { kind, value: out };
    }
    case "rollup":
    case "formula":
    case "created_time":
    case "last_edited_time":
    case "created_by":
    case "last_edited_by":
      throw new TypeError("a computed property has no stored value");
    case "date": {
      const day = record(stored.value);
      return {
        kind,
        value: day.time === undefined ? { date: string(day.date) } : { date: string(day.date), time: string(day.time) },
      };
    }
    case "text":
    case "select":
    case "url":
    case "status":
    case "email":
    case "phone":
      return { kind, value: string(stored.value) };
  }
}

function readPropertyOption(value: unknown): PropertyOption {
  const option = record(value);
  const state: PropertyOption = {
    id: string(option.id),
    name: string(option.name),
    color: oneOf(OPTION_COLORS, option.color),
  };
  if (option.group !== undefined) {
    state.group = oneOf(STATUS_GROUPS, option.group);
  }
  return state;
}

/**
 * The configuration of a relation, rollup or formula as a backup stores it (ADR-0027, ADR-0055). A formula keeps its
 * expression only: the result type is derived on read and never stored.
 */
function readPropertyConfig(value: unknown): PropertyConfig {
  const config = record(value);
  if (config.kind === "relation") {
    const out: PropertyConfig = { kind: "relation", twoWay: boolean(config.twoWay) };
    if (config.pairedPropertyId !== undefined) {
      out.pairedPropertyId = string(config.pairedPropertyId);
    }
    return out;
  }
  if (config.kind === "formula") {
    return { kind: "formula", expression: string(config.expression) };
  }
  return {
    kind: "rollup",
    relationPropertyId: string(config.relationPropertyId),
    target: string(config.target),
    function: oneOf(ROLLUP_FUNCTIONS, config.function),
  };
}

function readPropertyDef(value: unknown): PropertyDef {
  const property = record(value);
  const def: PropertyDef = {
    id: string(property.id),
    name: string(property.name),
    kind: oneOf(PROPERTY_KINDS, property.kind),
    options: array(property.options).map(readPropertyOption),
    createdAt: number(property.createdAt),
  };
  // Backups written before relations and rollups (ADR-0027) have no configuration.
  if (property.config !== undefined) {
    def.config = readPropertyConfig(property.config);
  }
  return def;
}

function readSpace(value: unknown): SpaceState {
  const space = record(value);
  const state: SpaceState = {
    id: string(space.id),
    name: string(space.name),
    createdAt: number(space.createdAt),
    roots: array(space.roots).map(string),
    pages: array(space.pages).map(readPage),
  };
  if (space.icon !== undefined) {
    state.icon = string(space.icon);
  }
  // Backups written before properties (ADR-0026) have no schema.
  if (space.properties !== undefined) {
    state.properties = array(space.properties).map(readPropertyDef);
  }
  // Backups written before favourites (ADR-0029) have none.
  if (space.favorites !== undefined) {
    state.favorites = array(space.favorites).map(string);
  }
  return state;
}

function readKitInfo(value: unknown): RecoveryKitInfo {
  const kit = record(value);
  // Backups written before Milestone 7 have no flag: not stale.
  const info: RecoveryKitInfo = {
    createdAt: number(kit.createdAt),
    stale: kit.stale === undefined ? false : boolean(kit.stale),
  };
  if (kit.verifiedAt !== undefined) {
    info.verifiedAt = number(kit.verifiedAt);
  }
  if (kit.reissuedAt !== undefined) {
    info.reissuedAt = number(kit.reissuedAt);
  }
  return info;
}

function readDevice(value: unknown): DeviceRecord {
  const device = record(value);
  const state: DeviceRecord = {
    deviceId: string(device.deviceId),
    name: string(device.name),
    addedAt: number(device.addedAt),
    addedViaKit: boolean(device.addedViaKit),
    signingKey: string(device.signingKey),
  };
  if (device.revokedAt !== undefined) {
    state.revokedAt = number(device.revokedAt);
  }
  if (device.addedBy !== undefined) {
    state.addedBy = string(device.addedBy);
  }
  if (device.revokedBy !== undefined) {
    state.revokedBy = string(device.revokedBy);
  }
  return state;
}

function readSecurity(value: unknown): SecurityState {
  const security = record(value);
  const state: SecurityState = {
    kit: readKitInfo(security.kit),
    devices: array(security.devices).map(readDevice),
    tombstones: array(security.tombstones).map((item) => {
      const tombstone = record(item);
      return { spaceId: string(tombstone.spaceId), pageIds: array(tombstone.pageIds).map(string) };
    }),
  };
  // Backups written before Milestone 7 have no generation (0).
  if (security.rootGeneration !== undefined) {
    state.rootGeneration = number(security.rootGeneration);
  }
  return state;
}

function readAttachment(value: unknown): AttachmentState {
  const attachment = record(value);
  const state: AttachmentState = {
    attachmentId: string(attachment.attachmentId),
    spaceId: string(attachment.spaceId),
    name: string(attachment.name),
    mime: string(attachment.mime),
    size: number(attachment.size),
    createdAt: number(attachment.createdAt),
    chunks: array(attachment.chunks).map((item) => {
      const chunk = record(item);
      return { id: string(chunk.id), data: string(chunk.data) };
    }),
  };
  return state;
}

function readAttachments(value: unknown): AttachmentsState {
  const attachments = record(value);
  return {
    attachments: array(attachments.attachments).map(readAttachment),
    tombstones: array(attachments.tombstones).map((item) => {
      const tombstone = record(item);
      return { spaceId: string(tombstone.spaceId), count: number(tombstone.count) };
    }),
  };
}
