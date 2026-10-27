// The in-memory twin of the core's MCP bookkeeping (`kb-core/src/mcp.rs`, ADR-0037, `docs/architecture/ai.md`
// section 7): clients with bearer tokens, grants per space (write implies read) and the audit log. The protocol
// and the tools are not here (the mirror has no server; `mcp_status` reports it as not running): a test or the
// dev page records calls with {@link MemoryMcp.recordCall}.
//
// Reduced fidelity: tokens are random (`kbm_` + 43 base64url characters) but nothing stores their hash.

import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { McpAuditEntry } from "./generated/McpAuditEntry";
import type { McpClientCreated } from "./generated/McpClientCreated";
import type { McpClientInfo } from "./generated/McpClientInfo";
import type { McpGrant } from "./generated/McpGrant";
import type { McpOutcome } from "./generated/McpOutcome";
import type { PageId } from "./generated/PageId";
import type { SpaceId } from "./generated/SpaceId";
import { invalid } from "./validate";

/** Most characters of a client name (after trimming). */
export const MAX_MCP_CLIENT_NAME_CHARS = 60;
/** Most audit rows kept (older ones go on insert). */
export const MAX_AUDIT_ROWS = 10_000;
/** Most audit rows one list answers, and how many by default. */
const MAX_AUDIT_LIMIT = 500;
const DEFAULT_AUDIT_LIMIT = 100;
/** Longest audit summary, in characters. */
const MAX_SUMMARY_CHARS = 200;

/** What {@link MemoryMcp} needs from its owner. */
export interface McpDeps {
  now: () => number;
  /** Id generator (UUID text); a client's id is its 32 hexadecimal digits. */
  newId: () => string;
  emit: (event: Event) => void;
  /** Whether a space exists (a grant of a deleted space is ignored). */
  spaceExists: (spaceId: SpaceId) => boolean;
}

/** The fields of an audit row the caller chooses. */
export interface McpCallRecord {
  clientId: string;
  tool: string;
  outcome: McpOutcome;
  summary?: string;
  spaceId?: SpaceId;
  pageId?: PageId;
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** 32 random bytes as `kbm_` and 43 base64url characters. */
function newToken(): string {
  const bytes = new Uint8Array(32);
  globalThis.crypto.getRandomValues(bytes);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return `kbm_${btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")}`;
}

/** An id of 32 lower-case hexadecimal digits from a UUID text (or random digits when the text is no UUID). */
function clientIdFrom(text: string): string {
  const hex = text.replace(/-/g, "").toLowerCase();
  if (/^[0-9a-f]{32}$/.test(hex)) {
    return hex;
  }
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** MCP clients, their grants and the audit log. Every change emits `McpChanged`. */
export class MemoryMcp {
  readonly #deps: McpDeps;
  readonly #clients = new Map<string, McpClientInfo>();
  #audit: McpAuditEntry[] = [];
  #nextAuditId = 1;

  constructor(deps: McpDeps) {
    this.#deps = deps;
  }

  /** `McpClientList`: the clients, oldest first. */
  list(): McpClientInfo[] {
    return [...this.#clients.values()].map(copyClient);
  }

  /** `McpClientCreate`: a client without grants and its token (shown once; the mirror keeps no copy). */
  create(rawName: string): McpClientCreated {
    const name = rawName.trim();
    if (name === "") {
      throw invalid("name", "must not be empty");
    }
    if (Array.from(name).length > MAX_MCP_CLIENT_NAME_CHARS) {
      throw invalid("name", `must be at most ${String(MAX_MCP_CLIENT_NAME_CHARS)} characters`);
    }
    if (/\p{Cc}/u.test(name)) {
      throw invalid("name", "must not contain control characters");
    }
    const client: McpClientInfo = { id: clientIdFrom(this.#deps.newId()), name, grants: [], createdAt: this.#deps.now() };
    this.#clients.set(client.id, client);
    this.#deps.emit({ type: "McpChanged" });
    return { client: copyClient(client), token: newToken() };
  }

  /** `McpClientDelete`: forgets the client (its audit rows stay, under the name it had); `NotFound` for an unknown one. */
  delete(clientId: string): void {
    if (!this.#clients.delete(clientId)) {
      throw notFound("client not found");
    }
    this.#deps.emit({ type: "McpChanged" });
  }

  /**
   * The grants change of the shell's `mcp_client_grants_set` (the native confirmation is the shell's): replaces
   * the client's grants. Write implies read, a grant of a space that no longer exists is ignored, a grant that
   * allows nothing is dropped, a space listed twice keeps its last grant. `NotFound` for an unknown client.
   */
  grantsSet(clientId: string, grants: readonly McpGrant[]): McpClientInfo {
    const client = this.#clients.get(clientId);
    if (client === undefined) {
      throw notFound("client not found");
    }
    const bySpace = new Map<SpaceId, McpGrant>();
    for (const grant of grants) {
      if (!this.#deps.spaceExists(grant.spaceId)) {
        continue;
      }
      const write = grant.write;
      const read = grant.read || write;
      if (read) {
        bySpace.set(grant.spaceId, { spaceId: grant.spaceId, read, write });
      } else {
        bySpace.delete(grant.spaceId);
      }
    }
    client.grants = [...bySpace.values()];
    this.#deps.emit({ type: "McpChanged" });
    return copyClient(client);
  }

  /**
   * `McpAuditList`: the newest rows first, of one client when `clientId` is given, only those with an id below
   * `before`, at most `limit` (1 to 500, default 100).
   */
  auditList(options: { clientId?: string; before?: number; limit?: number } = {}): McpAuditEntry[] {
    const limit = Math.min(MAX_AUDIT_LIMIT, Math.max(1, options.limit ?? DEFAULT_AUDIT_LIMIT));
    const rows: McpAuditEntry[] = [];
    for (let i = this.#audit.length - 1; i >= 0 && rows.length < limit; i -= 1) {
      const row = this.#audit[i];
      if (row === undefined) {
        continue;
      }
      if (options.clientId !== undefined && row.clientId !== options.clientId) {
        continue;
      }
      if (options.before !== undefined && row.id >= options.before) {
        continue;
      }
      rows.push({ ...row });
    }
    return rows;
  }

  /** `McpAuditClear`: deletes every row. */
  auditClear(): void {
    this.#audit = [];
    this.#deps.emit({ type: "McpChanged" });
  }

  /**
   * Test hook and dev page: records a call of a client as the real server would (one audit row, the client's
   * last use), then emits `McpChanged`. `NotFound` for an unknown client.
   */
  recordCall(call: McpCallRecord): McpAuditEntry {
    const client = this.#clients.get(call.clientId);
    if (client === undefined) {
      throw notFound("client not found");
    }
    const at = this.#deps.now();
    client.lastUsedAt = at;
    const row: McpAuditEntry = {
      id: this.#nextAuditId,
      at,
      clientId: client.id,
      clientName: client.name,
      tool: call.tool,
      summary: (call.summary ?? "").slice(0, MAX_SUMMARY_CHARS),
      outcome: call.outcome,
    };
    if (call.spaceId !== undefined) {
      row.spaceId = call.spaceId;
    }
    if (call.pageId !== undefined) {
      row.pageId = call.pageId;
    }
    this.#nextAuditId += 1;
    this.#audit.push(row);
    if (this.#audit.length > MAX_AUDIT_ROWS) {
      this.#audit = this.#audit.slice(this.#audit.length - MAX_AUDIT_ROWS);
    }
    this.#deps.emit({ type: "McpChanged" });
    return { ...row };
  }

  /** The spaces of the clients' grants lose a deleted space (the real core ignores such grants everywhere). */
  spaceDeleted(spaceId: SpaceId): void {
    for (const client of this.#clients.values()) {
      client.grants = client.grants.filter((grant) => grant.spaceId !== spaceId);
    }
  }
}

function copyClient(client: McpClientInfo): McpClientInfo {
  const copy: McpClientInfo = {
    id: client.id,
    name: client.name,
    grants: client.grants.map((grant) => ({ ...grant })),
    createdAt: client.createdAt,
  };
  if (client.lastUsedAt !== undefined) {
    copy.lastUsedAt = client.lastUsedAt;
  }
  return copy;
}
