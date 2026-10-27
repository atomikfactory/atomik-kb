// Settings -> Sync (Milestone 6, ADR-0021): connecting this device to a sync server (and the
// invite token when the server does not know the account yet: refused at once with
// `InvalidInput` for `inviteToken`, or found by the driver, `SyncInfo.needsInviteToken`), the
// status of sync, "Sync now" and disconnecting. The invite token stays in the state of the
// form that asks for it. Since Milestone 8 (ADR-0023) a line counts the attachment chunks
// still to move: to upload, to download, and waiting for the device that has them.

import { toCoreError } from "@atomik-kb/core-client";
import type { SyncInfo, TransferInfo } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { FormEvent, ReactNode } from "react";

import { SYNC_STATE_LABELS, syncTone } from "../components/SyncIndicator";
import { SyncDisconnectDialog } from "../components/SyncDisconnectDialog";
import { isFieldError } from "../errors";
import { formatRelativeTime, formatTimestamp } from "../format";
import { t, tp } from "../i18n";
import { useAppStore } from "../store-context";
import { isSyncBusy } from "../store-sync";

/** How often "last synced 2 min ago" is brought up to date. */
const CLOCK_TICK_MS = 15_000;

/** The current time, renewed every `intervalMs` (a render must not read the clock itself). */
function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => {
      setNow(Date.now());
    }, intervalMs);
    return () => {
      clearInterval(timer);
    };
  }, [intervalMs]);
  return now;
}

/** The last successful exchange with the server, either way. */
export function lastSyncedAt(sync: SyncInfo): number | undefined {
  const times = [sync.lastPullAt, sync.lastPushAt].filter((value) => value !== undefined).map(Number);
  return times.length === 0 ? undefined : Math.max(...times);
}

/**
 * "3 to upload, 2 to download, 1 waiting for another device": the attachment chunks still to
 * move, leaving out the counts at zero; `null` when nothing moves (or an older core does not
 * say).
 */
export function describeTransfers(transfers: TransferInfo | undefined): string | null {
  if (transfers === undefined) {
    return null;
  }
  const parts: string[] = [];
  if (transfers.uploads > 0) {
    parts.push(t("attachment.transfer.uploads", { count: transfers.uploads }));
  }
  if (transfers.downloads > 0) {
    parts.push(t("attachment.transfer.downloads", { count: transfers.downloads }));
  }
  if (transfers.waiting > 0) {
    parts.push(t("attachment.transfer.waitingCount", { count: transfers.waiting }));
  }
  return parts.length === 0 ? null : parts.join(", ");
}

// ----- connecting ---------------------------------------------------------------------

/** A problem with what was typed; `field` says which input it belongs to. */
interface ConnectError {
  field: "url" | "token" | null;
  text: string;
}

interface ConnectFormProps {
  /**
   * The configured server, when sync is on but the server does not know this account
   * (`SyncInfo.needsInviteToken`): the form starts at the invite token for that server, which
   * the status above already shows. Without it, the form starts at an empty address.
   */
  configuredUrl?: string;
}

/**
 * The server address, then "Connect". When the server does not know this account yet (the
 * core answers `InvalidInput` for `inviteToken`, or the driver found out and the status says
 * `needsInviteToken`), the form asks for the invite token the server's operator gave out and
 * connects again with it; "Use another server" goes back to the address.
 */
function ConnectForm({ configuredUrl }: ConnectFormProps) {
  const configureSync = useAppStore((state) => state.configureSync);
  const [url, setUrl] = useState(configuredUrl ?? "");
  const [token, setToken] = useState("");
  const [asksToken, setAsksToken] = useState(configuredUrl !== undefined);
  // Focus moves to the token field only when the user's own "Connect" led there.
  const [focusToken, setFocusToken] = useState(false);
  const [error, setError] = useState<ConnectError | null>(null);
  const [busy, setBusy] = useState(false);
  const urlId = useId();
  const tokenId = useId();
  const tokenIntroId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const serverUrl = url.trim();
    if (serverUrl === "") {
      setAsksToken(false);
      setError({ field: "url", text: t("sync.error.emptyUrl") });
      return;
    }
    const inviteToken = asksToken ? token.trim() : undefined;
    if (inviteToken === "") {
      setError({ field: "token", text: t("sync.token.error.empty") });
      return;
    }
    setError(null);
    setBusy(true);
    try {
      // Once the store has the answer, Settings shows the status instead of this form; while
      // the server still wants a token for this account, the form stays and asks for it.
      const info = await configureSync(serverUrl, inviteToken);
      setToken("");
      setAsksToken(info.needsInviteToken);
    } catch (failure) {
      const reason = toCoreError(failure);
      if (isFieldError(reason, "inviteToken")) {
        if (asksToken) {
          setToken("");
          setError({ field: "token", text: t("sync.token.error.refused", { message: reason.message }) });
        } else {
          setAsksToken(true);
          setFocusToken(true);
        }
      } else if (isFieldError(reason, "serverUrl")) {
        setAsksToken(false);
        setToken("");
        setError({ field: "url", text: t("sync.error.url", { message: reason.message }) });
      } else {
        setError({ field: null, text: t("sync.error.generic", { message: reason.message }) });
      }
    }
    setBusy(false);
  }

  const errorFor = (field: ConnectError["field"]): string | undefined =>
    error !== null && error.field === field ? errorId : undefined;
  const connectLabel = busy ? t("sync.connecting") : t("sync.connect");

  return (
    <form
      className="form sync-connect"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      {configuredUrl === undefined && <p className="muted">{t("sync.intro")}</p>}
      {asksToken ? (
        <>
          {/* The status above names the configured server; another one is named here. */}
          {url.trim() !== configuredUrl && (
            <dl className="facts">
              <dt>{t("sync.server")}</dt>
              <dd className="mono">{url.trim()}</dd>
            </dl>
          )}
          <p id={tokenIntroId}>{t("sync.token.intro")}</p>
          <label htmlFor={tokenId}>{t("sync.token.label")}</label>
          <div className="row">
            <input
              id={tokenId}
              className="input"
              type="text"
              value={token}
              autoComplete="off"
              spellCheck={false}
              autoFocus={focusToken}
              readOnly={busy}
              aria-invalid={error?.field === "token"}
              aria-describedby={[tokenIntroId, errorFor("token")].filter((id) => id !== undefined).join(" ")}
              onChange={(event) => {
                setToken(event.target.value);
                setError(null);
              }}
            />
            <button type="submit" className="button button-primary" disabled={busy}>
              {connectLabel}
            </button>
          </div>
          <div>
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => {
                setAsksToken(false);
                setToken("");
                setError(null);
              }}
            >
              {t("sync.token.changeServer")}
            </button>
          </div>
        </>
      ) : (
        <>
          <label htmlFor={urlId}>{t("sync.urlLabel")}</label>
          <div className="row">
            <input
              id={urlId}
              className="input"
              type="url"
              inputMode="url"
              value={url}
              placeholder={t("sync.urlPlaceholder")}
              autoComplete="url"
              spellCheck={false}
              readOnly={busy}
              aria-invalid={error?.field === "url"}
              aria-describedby={errorFor("url")}
              onChange={(event) => {
                setUrl(event.target.value);
                setError(null);
              }}
            />
            <button type="submit" className="button button-primary" disabled={busy}>
              {connectLabel}
            </button>
          </div>
        </>
      )}
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error.text}
        </p>
      )}
    </form>
  );
}

// ----- connected ----------------------------------------------------------------------

/** "Connected · last synced 2 min ago · 3 changes waiting to upload · 1 other device seen". */
function StatusLine({ sync }: { sync: SyncInfo }) {
  const now = useNow(CLOCK_TICK_MS);
  const last = lastSyncedAt(sync);
  const lastText = last === undefined ? null : formatRelativeTime(last, now);
  const lastFull = last === undefined ? null : formatTimestamp(last);

  const parts: { key: string; node: ReactNode }[] = [
    {
      key: "state",
      node: (
        <span className="sync-state">
          <span className={`sync-mark sync-tone-${syncTone(sync.state)}`} aria-hidden="true" />
          {t(SYNC_STATE_LABELS[sync.state])}
        </span>
      ),
    },
    {
      key: "last",
      node:
        lastText === null || lastFull === null ? (
          t("sync.neverSynced")
        ) : (
          <time dateTime={lastFull.iso} title={lastFull.text}>
            {t("sync.lastSynced", { time: lastText })}
          </time>
        ),
    },
  ];
  if (sync.pendingEntries > 0) {
    parts.push({ key: "pending", node: tp("sync.pending", sync.pendingEntries) });
  }
  parts.push({
    key: "devices",
    node: sync.devicesSeen === 0 ? t("sync.devices.none") : tp("sync.devices", sync.devicesSeen),
  });

  return (
    <>
      {parts.map((part, index) => (
        <span key={part.key}>
          {index > 0 && (
            <span className="sync-sep" aria-hidden="true">
              {" · "}
            </span>
          )}
          {part.node}
        </span>
      ))}
    </>
  );
}

function SyncStatus({ sync }: { sync: SyncInfo }) {
  const syncNow = useAppStore((state) => state.syncNow);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = isSyncBusy(sync.state);
  const transfers = describeTransfers(sync.transfers);

  return (
    <>
      <dl className="facts">
        <dt>{t("sync.server")}</dt>
        <dd className="mono">{sync.serverUrl ?? ""}</dd>
        <dt>{t("sync.status")}</dt>
        <dd className="sync-status">
          <StatusLine sync={sync} />
        </dd>
        {transfers !== null && (
          <>
            <dt>{t("attachment.transfer.label")}</dt>
            <dd className="sync-transfers">{transfers}</dd>
          </>
        )}
      </dl>
      {sync.lastError !== undefined && <p className="note sync-last-error">{t("sync.lastError", { message: sync.lastError })}</p>}
      {/* The server does not know this account: nothing syncs until a token creates it. */}
      {sync.needsInviteToken && <ConnectForm key={sync.serverUrl} configuredUrl={sync.serverUrl ?? ""} />}
      <div className="row-actions">
        {!sync.needsInviteToken && (
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => {
              setError(null);
              syncNow().catch((failure: unknown) => {
                setError(t("sync.now.error", { message: toCoreError(failure).message }));
              });
            }}
          >
            {t("sync.now")}
          </button>
        )}
        <button
          type="button"
          className="button"
          onClick={() => {
            setError(null);
            setConfirming(true);
          }}
        >
          {t("sync.disconnect")}
        </button>
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {confirming && (
        <SyncDisconnectDialog
          onClose={() => {
            setConfirming(false);
          }}
          onError={setError}
        />
      )}
    </>
  );
}

// ----- the section --------------------------------------------------------------------

/** Settings -> Sync. */
export function SyncSettings() {
  const sync = useAppStore((state) => state.sync);
  const refreshSync = useAppStore((state) => state.refreshSync);

  if (sync === null) {
    return (
      <div className="subpanel">
        <p className="muted">{t("sync.unavailable")}</p>
        <div className="row-actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              refreshSync().catch((error: unknown) => {
                console.error("could not read the sync status", toCoreError(error).code);
              });
            }}
          >
            {t("sync.retry")}
          </button>
        </div>
      </div>
    );
  }

  return sync.configured ? <SyncStatus sync={sync} /> : <ConnectForm />;
}
