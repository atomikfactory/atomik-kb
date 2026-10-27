// Settings -> AI -> MCP server (ADR-0037, `docs/architecture/ai.md` sections 7 and 8.2): a loopback server that
// lets outside AI tools (Claude Desktop, Claude Code, scripts) search and read the notes, and write when allowed.
// This is its switch, port and status, the clients (each with its own token, shown once), what each may do in each
// space (a grid of Read and Write; write implies read; the shell asks in a native dialog before access is added),
// and the audit log of the last 100 calls.
//
// A token is shown once, in the state of this component only: it is never in the store, so leaving the screen or
// locking the app forgets it.

import { isMcpPort, toCoreError } from "@atomik-kb/core-client";
import type { McpAuditEntry, McpClientCreated, McpClientInfo, McpGrant, McpStatus, SpaceInfo } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { FormEvent } from "react";

import { writeClipboard } from "../clipboard";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { detailOf, isFieldError } from "../errors";
import { formatRelativeTime, formatTimestamp } from "../format";
import { t } from "../i18n";
import { spaceAiOn } from "../sharing";
import { selectMcpSettings } from "../store-ai";
import { useAppStore } from "../store-context";

/** The name the app gives itself in the configuration of an outside tool. */
const SERVER_NAME = "atomik-kb";
/** What the Claude Desktop configuration says when the bridge program was not found next to the app. */
const BRIDGE_PLACEHOLDER = "<path to kb-mcp-bridge>";

/**
 * The configuration to paste into Claude Desktop (`claude_desktop_config.json`): the stdio bridge, which talks to
 * the server on `port` with the client's token (section 7).
 */
export function claudeDesktopConfig(bridgePath: string | undefined, port: number, token: string): string {
  return JSON.stringify(
    {
      mcpServers: {
        [SERVER_NAME]: {
          command: bridgePath ?? BRIDGE_PLACEHOLDER,
          args: ["--port", String(port)],
          env: { ATOMIK_KB_MCP_TOKEN: token },
        },
      },
    },
    null,
    2,
  );
}

/** The command that adds the server to Claude Code, which speaks HTTP itself (section 7). */
export function claudeCodeCommand(url: string, token: string): string {
  return `claude mcp add --transport http ${SERVER_NAME} ${url} --header "Authorization: Bearer ${token}"`;
}

/** The names of spaces for a sentence: "A, B". */
function spaceNames(ids: readonly string[], spaces: readonly SpaceInfo[]): string {
  return ids.map((id) => spaces.find((space) => space.id === id)?.name ?? id).join(", ");
}

/** What a client may do, for people. */
function accessSummary(client: McpClientInfo, spaces: readonly SpaceInfo[]): string {
  const writes = client.grants.filter((grant) => grant.write).map((grant) => grant.spaceId);
  const reads = client.grants.filter((grant) => grant.read && !grant.write).map((grant) => grant.spaceId);
  const parts: string[] = [];
  if (reads.length > 0) {
    parts.push(t("mcp.access.read", { spaces: spaceNames(reads, spaces) }));
  }
  if (writes.length > 0) {
    parts.push(t("mcp.access.write", { spaces: spaceNames(writes, spaces) }));
  }
  return parts.length === 0 ? t("mcp.access.none") : parts.join(" ");
}

function statusLine(enabled: boolean, status: McpStatus | null): string {
  if (!enabled) {
    return t("mcp.status.off");
  }
  if (status === null) {
    return t("mcp.status.checking");
  }
  if (status.running) {
    return t("mcp.status.running", { address: `127.0.0.1:${String(status.port)}` });
  }
  return status.error === undefined ? t("mcp.status.stopped") : t("mcp.status.error", { message: status.error });
}

function outcomeLabel(outcome: McpAuditEntry["outcome"]): string {
  switch (outcome) {
    case "ok":
      return t("mcp.audit.outcome.ok");
    case "denied":
      return t("mcp.audit.outcome.denied");
    case "invalid":
      return t("mcp.audit.outcome.invalid");
    case "error":
      return t("mcp.audit.outcome.error");
  }
}

/** A button that copies `text`, and says whether it did. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  return (
    <>
      <button
        type="button"
        className="button button-small"
        onClick={() => {
          writeClipboard(text).then(
            () => {
              setState("copied");
            },
            () => {
              setState("failed");
            },
          );
        }}
      >
        {label}
      </button>
      <span className="muted mcp-copy-state" role="status">
        {state === "copied" ? t("mcp.copied") : state === "failed" ? t("mcp.copyFailed") : ""}
      </span>
    </>
  );
}

/** The token of a client just made (shown once) with the two ways to give it to a tool. */
function NewClient({ created, status, port, onDone }: { created: McpClientCreated; status: McpStatus | null; port: number; onDone: () => void }) {
  const titleId = useId();
  const url = status?.url ?? `http://127.0.0.1:${String(port)}/mcp`;
  const desktop = claudeDesktopConfig(status?.bridgePath, status?.port ?? port, created.token);
  const command = claudeCodeCommand(url, created.token);
  return (
    <section className="mcp-new" aria-labelledby={titleId}>
      <h4 className="mcp-new-title" id={titleId}>
        {t("mcp.new.title", { name: created.client.name })}
      </h4>
      <p className="note">{t("mcp.new.once")}</p>
      <div className="mcp-new-item">
        <span className="mcp-new-label">{t("mcp.new.token")}</span>
        <code className="mono mcp-token">{created.token}</code>
        <CopyButton text={created.token} label={t("mcp.new.copyToken")} />
      </div>
      <div className="mcp-new-item">
        <span className="mcp-new-label">{t("mcp.new.desktop")}</span>
        {status?.bridgePath === undefined && <p className="muted">{t("mcp.new.noBridge")}</p>}
        <pre className="mcp-config" aria-label={t("mcp.new.desktop")}>
          {desktop}
        </pre>
        <CopyButton text={desktop} label={t("mcp.new.copyConfig")} />
      </div>
      <div className="mcp-new-item">
        <span className="mcp-new-label">{t("mcp.new.code")}</span>
        <pre className="mcp-config" aria-label={t("mcp.new.code")}>
          {command}
        </pre>
        <CopyButton text={command} label={t("mcp.new.copyCommand")} />
      </div>
      <p className="muted">{t("mcp.new.noAccess")}</p>
      <div className="row-actions">
        <button type="button" className="button" onClick={onDone}>
          {t("mcp.new.done")}
        </button>
      </div>
    </section>
  );
}

/** The grants of a client by space (write implies read). */
function grantMap(grants: readonly McpGrant[]): ReadonlyMap<string, { read: boolean; write: boolean }> {
  return new Map(grants.map((grant) => [grant.spaceId, { read: grant.read || grant.write, write: grant.write }]));
}

/** What one client may do per space: a grid of Read and Write. */
function GrantsEditor({ client, spaces, onClose }: { client: McpClientInfo; spaces: readonly SpaceInfo[]; onClose: () => void }) {
  const setMcpGrants = useAppStore((state) => state.setMcpGrants);
  const [grants, setGrants] = useState<ReadonlyMap<string, { read: boolean; write: boolean }>>(() => grantMap(client.grants));
  // The grid follows what is stored (a change from elsewhere, or the one just saved) without being rebuilt, so the
  // notice of a save stays: adjusting state while rendering, as the other forms of Settings do.
  const stored = JSON.stringify(client.grants);
  const [knownStored, setKnownStored] = useState(stored);
  if (stored !== knownStored) {
    setKnownStored(stored);
    setGrants(grantMap(client.grants));
  }
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ kind: "saved" | "error"; text: string } | null>(null);
  const titleId = useId();

  function change(spaceId: string, patch: Partial<{ read: boolean; write: boolean }>): void {
    setMessage(null);
    setGrants((current) => {
      const before = current.get(spaceId) ?? { read: false, write: false };
      let next = { ...before, ...patch };
      // Write implies read: allowing it allows reading, and not reading allows no writing.
      if (patch.write === true) {
        next = { ...next, read: true };
      }
      if (patch.read === false) {
        next = { ...next, write: false };
      }
      return new Map(current).set(spaceId, next);
    });
  }

  async function save(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    setSaving(true);
    setMessage(null);
    const list: McpGrant[] = [];
    for (const space of spaces) {
      const grant = grants.get(space.id);
      // A space whose AI is off cannot be granted (ADR-0047): its boxes are off, and nothing is sent for it.
      if (grant !== undefined && (grant.read || grant.write) && spaceAiOn(space)) {
        list.push({ spaceId: space.id, read: grant.read || grant.write, write: grant.write });
      }
    }
    try {
      await setMcpGrants(client.id, list);
      setMessage({ kind: "saved", text: t("mcp.grants.saved") });
    } catch (failure) {
      const reason = toCoreError(failure);
      setMessage({
        kind: "error",
        text:
          isFieldError(reason, "grants") && detailOf(reason, "reason") === "declined"
            ? t("mcp.grants.declined")
            : t("mcp.grants.error", { message: reason.message }),
      });
    }
    setSaving(false);
  }

  return (
    <form
      className="mcp-grants"
      aria-labelledby={titleId}
      onSubmit={(event) => {
        void save(event);
      }}
    >
      <h4 className="mcp-grants-title" id={titleId}>
        {t("mcp.grants.title", { name: client.name })}
      </h4>
      {spaces.length === 0 ? (
        <p className="muted">{t("mcp.grants.noSpaces")}</p>
      ) : (
        <table className="mcp-grants-table">
          <thead>
            <tr>
              <th scope="col">{t("mcp.grants.space")}</th>
              <th scope="col">{t("mcp.grants.read")}</th>
              <th scope="col">{t("mcp.grants.write")}</th>
            </tr>
          </thead>
          <tbody>
            {spaces.map((space) => {
              const aiOn = spaceAiOn(space);
              const grant = aiOn ? (grants.get(space.id) ?? { read: false, write: false }) : { read: false, write: false };
              return (
                <tr key={space.id}>
                  <th scope="row">
                    {space.name}
                    {!aiOn && <span className="muted mcp-space-off"> {t("sharing.mcp.aiOff")}</span>}
                  </th>
                  <td>
                    <input
                      type="checkbox"
                      checked={grant.read}
                      disabled={!aiOn}
                      aria-label={t("mcp.grants.readIn", { space: space.name })}
                      onChange={(event) => {
                        change(space.id, { read: event.target.checked });
                      }}
                    />
                  </td>
                  <td>
                    <input
                      type="checkbox"
                      checked={grant.write}
                      disabled={!aiOn}
                      aria-label={t("mcp.grants.writeIn", { space: space.name })}
                      onChange={(event) => {
                        change(space.id, { write: event.target.checked });
                      }}
                    />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      <p className="muted">{t("mcp.grants.hint")}</p>
      <div className="row-actions">
        <button type="submit" className="button button-primary" disabled={saving || spaces.length === 0}>
          {saving ? t("mcp.grants.saving") : t("mcp.grants.save")}
        </button>
        <button type="button" className="button" onClick={onClose}>
          {t("mcp.grants.close")}
        </button>
      </div>
      {message !== null && (
        <p className={message.kind === "error" ? "error-text" : "muted"} role={message.kind === "error" ? "alert" : "status"}>
          {message.text}
        </p>
      )}
    </form>
  );
}

/** The clients, with "Add client", their permissions and "Delete". */
function Clients({ spaces, status, port }: { spaces: readonly SpaceInfo[]; status: McpStatus | null; port: number }) {
  const clients = useAppStore((state) => state.mcpClients);
  const createMcpClient = useAppStore((state) => state.createMcpClient);
  const deleteMcpClient = useAppStore((state) => state.deleteMcpClient);
  const [name, setName] = useState("");
  const [created, setCreated] = useState<McpClientCreated | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<McpClientInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const titleId = useId();
  const nameId = useId();
  const errorId = useId();

  async function add(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (name.trim() === "") {
      setError(t("mcp.add.empty"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setCreated(await createMcpClient(name));
      setName("");
    } catch (failure) {
      const reason = toCoreError(failure);
      setError(isFieldError(reason, "name") ? t("form.fieldError", { message: reason.message }) : t("mcp.add.error", { message: reason.message }));
    }
    setBusy(false);
  }

  return (
    <div className="mcp-clients" role="group" aria-labelledby={titleId}>
      <h4 className="mcp-heading" id={titleId}>
        {t("mcp.clients.title")}
      </h4>
      {created !== null && (
        <NewClient
          created={created}
          status={status}
          port={port}
          onDone={() => {
            setCreated(null);
          }}
        />
      )}
      {clients === null ? (
        <p className="muted">{t("mcp.clients.loading")}</p>
      ) : clients.length === 0 ? (
        <p className="muted">{t("mcp.clients.none")}</p>
      ) : (
        <ul className="mcp-client-list">
          {clients.map((client) => (
            <li key={client.id} className="mcp-client">
              <div className="mcp-client-head">
                <span className="mcp-client-name">{client.name}</span>
                <span className="mcp-client-actions">
                  <button
                    type="button"
                    className="button button-small"
                    aria-expanded={editing === client.id}
                    aria-label={t("mcp.permissions", { name: client.name })}
                    onClick={() => {
                      setEditing((current) => (current === client.id ? null : client.id));
                    }}
                  >
                    {t("mcp.permissionsButton")}
                  </button>
                  <button
                    type="button"
                    className="button button-small button-danger"
                    aria-label={t("mcp.delete", { name: client.name })}
                    onClick={() => {
                      setDeleting(client);
                    }}
                  >
                    {t("mcp.deleteButton")}
                  </button>
                </span>
              </div>
              <p className="muted mcp-client-access">{accessSummary(client, spaces)}</p>
              <p className="muted mcp-client-meta">
                {t("mcp.created", { when: formatTimestamp(client.createdAt)?.text ?? "" })}
                {" · "}
                {client.lastUsedAt === undefined
                  ? t("mcp.neverUsed")
                  : t("mcp.lastUsed", { when: formatRelativeTime(client.lastUsedAt) ?? "" })}
              </p>
              {editing === client.id && (
                <GrantsEditor
                  key={client.id}
                  client={client}
                  spaces={spaces}
                  onClose={() => {
                    setEditing(null);
                  }}
                />
              )}
            </li>
          ))}
        </ul>
      )}
      <form
        className="field mcp-add"
        noValidate
        onSubmit={(event) => {
          void add(event);
        }}
      >
        <label htmlFor={nameId}>{t("mcp.add.label")}</label>
        <div className="row">
          <input
            id={nameId}
            className="input"
            type="text"
            value={name}
            maxLength={60}
            autoComplete="off"
            placeholder={t("mcp.add.placeholder")}
            aria-invalid={error !== null}
            aria-describedby={error === null ? undefined : errorId}
            onChange={(event) => {
              setName(event.target.value);
              setError(null);
            }}
          />
          <button type="submit" className="button" disabled={busy}>
            {t("mcp.add.button")}
          </button>
        </div>
        {error !== null && (
          <p className="error-text" id={errorId} role="alert">
            {error}
          </p>
        )}
      </form>
      {deleting !== null && (
        <ConfirmDialog
          title={t("mcp.delete.title", { name: deleting.name })}
          body={t("mcp.delete.body")}
          confirmLabel={t("mcp.delete.confirm")}
          onConfirm={async () => {
            try {
              await deleteMcpClient(deleting.id);
              setEditing((current) => (current === deleting.id ? null : current));
            } catch (failure) {
              setError(t("mcp.delete.error", { message: toCoreError(failure).message }));
            }
          }}
          onClose={() => {
            setDeleting(null);
          }}
        />
      )}
    </div>
  );
}

/** The last calls of the clients: when, who, what, on what and how it ended. */
function Audit({ spaces }: { spaces: readonly SpaceInfo[] }) {
  const audit = useAppStore((state) => state.mcpAudit);
  const clients = useAppStore((state) => state.mcpClients);
  const filter = useAppStore((state) => state.mcpAuditClient);
  const setMcpAuditClient = useAppStore((state) => state.setMcpAuditClient);
  const clearMcpAudit = useAppStore((state) => state.clearMcpAudit);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const filterId = useId();

  return (
    <div className="mcp-audit" role="group" aria-labelledby={titleId}>
      <h4 className="mcp-heading" id={titleId}>
        {t("mcp.audit.title")}
      </h4>
      <div className="row mcp-audit-controls">
        <label htmlFor={filterId} className="visually-hidden">
          {t("mcp.audit.filter")}
        </label>
        <select
          id={filterId}
          className="input select"
          value={filter ?? ""}
          onChange={(event) => {
            setError(null);
            setMcpAuditClient(event.target.value === "" ? null : event.target.value).catch((failure: unknown) => {
              setError(t("mcp.audit.error", { message: toCoreError(failure).message }));
            });
          }}
        >
          <option value="">{t("mcp.audit.allClients")}</option>
          {(clients ?? []).map((client) => (
            <option key={client.id} value={client.id}>
              {client.name}
            </option>
          ))}
        </select>
        <button
          type="button"
          className="button"
          disabled={audit === null || audit.length === 0}
          onClick={() => {
            setError(null);
            clearMcpAudit().catch((failure: unknown) => {
              setError(t("mcp.audit.error", { message: toCoreError(failure).message }));
            });
          }}
        >
          {t("mcp.audit.clear")}
        </button>
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {audit === null ? (
        <p className="muted">{t("mcp.audit.loading")}</p>
      ) : audit.length === 0 ? (
        <p className="muted">{t("mcp.audit.none")}</p>
      ) : (
        <div className="mcp-audit-scroll">
          <table className="mcp-audit-table">
            <thead>
              <tr>
                <th scope="col">{t("mcp.audit.time")}</th>
                <th scope="col">{t("mcp.audit.client")}</th>
                <th scope="col">{t("mcp.audit.tool")}</th>
                <th scope="col">{t("mcp.audit.target")}</th>
                <th scope="col">{t("mcp.audit.outcome")}</th>
              </tr>
            </thead>
            <tbody>
              {audit.map((entry) => {
                const when = formatTimestamp(entry.at);
                const space = entry.spaceId === undefined ? undefined : spaces.find((candidate) => candidate.id === entry.spaceId);
                return (
                  <tr key={entry.id} data-outcome={entry.outcome}>
                    <td>{when === null ? "" : <time dateTime={when.iso}>{when.text}</time>}</td>
                    <td>{entry.clientName}</td>
                    <td className="mono">{entry.tool}</td>
                    <td>
                      {entry.summary === "" ? <span className="muted">—</span> : entry.summary}
                      {space !== undefined && <span className="muted mcp-audit-space"> {space.name}</span>}
                    </td>
                    <td>
                      <span className={entry.outcome === "ok" ? "mcp-outcome" : "mcp-outcome mcp-outcome-problem"}>{outcomeLabel(entry.outcome)}</span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** Settings -> AI -> MCP server; nothing where the shell has no MCP server. */
export function McpSettings() {
  const available = useAppStore((state) => state.aiCapabilities.mcp);
  const settings = useAppStore(selectMcpSettings);
  const status = useAppStore((state) => state.mcpStatus);
  const spaces = useAppStore((state) => state.spaces);
  const loadMcp = useAppStore((state) => state.loadMcp);
  const updateMcpSettings = useAppStore((state) => state.updateMcpSettings);
  const [portDraft, setPortDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const titleId = useId();
  const portId = useId();
  const portHintId = useId();

  useEffect(() => {
    if (available) {
      void loadMcp();
    }
  }, [available, loadMcp]);

  if (!available) {
    return null;
  }

  function change(patch: { enabled?: boolean; port?: number }): void {
    setError(null);
    setSaving(true);
    updateMcpSettings(patch)
      .catch((failure: unknown) => {
        setError(t("mcp.error", { message: toCoreError(failure).message }));
      })
      .finally(() => {
        setSaving(false);
      });
  }

  function commitPort(): void {
    if (portDraft === null) {
      return;
    }
    const port = /^\d+$/.test(portDraft.trim()) ? Number(portDraft.trim()) : Number.NaN;
    setPortDraft(null);
    if (!isMcpPort(port)) {
      setError(t("mcp.port.error"));
    } else if (port !== settings.port) {
      change({ port });
    }
  }

  return (
    <section className="subpanel mcp-settings" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("mcp.title")}
      </h3>
      <p className="muted">{t("mcp.intro")}</p>
      <label className="checkbox-label mcp-enabled">
        <input
          type="checkbox"
          role="switch"
          checked={settings.enabled}
          disabled={saving}
          onChange={(event) => {
            change({ enabled: event.target.checked });
          }}
        />
        <span>{t("mcp.enabled")}</span>
      </label>
      <div className="field mcp-port">
        <label htmlFor={portId}>{t("mcp.port")}</label>
        <input
          id={portId}
          className="input"
          type="number"
          inputMode="numeric"
          min={1024}
          max={65535}
          step={1}
          value={portDraft ?? String(settings.port)}
          aria-describedby={portHintId}
          onChange={(event) => {
            setPortDraft(event.target.value);
            setError(null);
          }}
          onBlur={commitPort}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitPort();
            } else if (event.key === "Escape") {
              setPortDraft(null);
            }
          }}
        />
        <p className="muted" id={portHintId}>
          {t("mcp.port.hint")}
        </p>
      </div>
      <p className="mcp-status" role="status">
        {statusLine(settings.enabled, status)}
      </p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      <Clients spaces={spaces} status={status} port={settings.port} />
      <Audit spaces={spaces} />
    </section>
  );
}
