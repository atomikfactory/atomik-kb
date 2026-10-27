// Settings -> Data, attachments (milestone-05.md, "UI"): how many files the current space
// holds and how much room they take, whether this device downloads the files of synced pages
// at once or when asked (`app.attachments.download`, milestone-08.md), how long files no block
// uses any more are kept (`app.attachments.retention_days`), and "Clean up now", which
// collects them at once.

import {
  DEFAULT_RETENTION_DAYS,
  DOWNLOAD_POLICY_EAGER,
  DOWNLOAD_POLICY_ON_DEMAND,
  DOWNLOAD_POLICY_SETTING,
  RETENTION_SETTING,
  downloadPolicyOf,
  toCoreError,
} from "@atomik-kb/core-client";
import type { DownloadPolicy, GcSummary, JsonValue } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";

import { formatBytes } from "../format";
import { t, tp } from "../i18n";
import { useAppStore } from "../store-context";

/** The retention periods offered, in days; `0` keeps unused files forever. */
export const RETENTION_CHOICES: readonly number[] = [7, 30, 90, 0];

/** Reads the stored retention: a whole number of days, the default when unset or unusable. */
export function parseRetentionDays(value: JsonValue | null | undefined): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : DEFAULT_RETENTION_DAYS;
}

/**
 * The download policies offered (`DOWNLOAD_POLICY_SETTING`, ADR-0023), the default first:
 * every file of every synced page comes at once, or only when asked for.
 */
export const DOWNLOAD_POLICIES: readonly DownloadPolicy[] = [DOWNLOAD_POLICY_EAGER, DOWNLOAD_POLICY_ON_DEMAND];

function downloadPolicyLabel(policy: DownloadPolicy): string {
  return policy === DOWNLOAD_POLICY_EAGER
    ? t("settings.attachments.download.eager")
    : t("settings.attachments.download.onDemand");
}

function DownloadPolicyChoice() {
  const stored = useAppStore((state) => state.settings[DOWNLOAD_POLICY_SETTING]);
  const loadSetting = useAppStore((state) => state.loadSetting);
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);
  const id = useId();
  const hintId = useId();
  const unread = stored === undefined;

  useEffect(() => {
    if (unread) {
      loadSetting(DOWNLOAD_POLICY_SETTING).catch((failure: unknown) => {
        console.error("could not read the attachment download policy", toCoreError(failure).code);
      });
    }
  }, [unread, loadSetting]);

  return (
    <div className="field attachment-download">
      <label htmlFor={id}>{t("settings.attachments.download")}</label>
      <select
        id={id}
        className="input select"
        value={downloadPolicyOf(stored)}
        disabled={unread}
        aria-describedby={hintId}
        onChange={(event) => {
          setError(null);
          setSetting(DOWNLOAD_POLICY_SETTING, downloadPolicyOf(event.target.value)).catch((failure: unknown) => {
            setError(t("settings.attachments.download.error", { message: toCoreError(failure).message }));
          });
        }}
      >
        {DOWNLOAD_POLICIES.map((policy) => (
          <option key={policy} value={policy}>
            {downloadPolicyLabel(policy)}
          </option>
        ))}
      </select>
      <p className="muted" id={hintId}>
        {t("settings.attachments.download.hint")}
      </p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function retentionLabel(days: number): string {
  return days === 0 ? t("settings.attachments.retention.never") : t("settings.attachments.retention.days", { days });
}

/** The line after "Clean up now". */
export function describeCollection(summary: GcSummary): string {
  if (summary.purged === 0 && summary.chunksDeleted === 0) {
    return t("settings.attachments.clean.nothing");
  }
  return t("settings.attachments.clean.done", {
    count: summary.purged,
    size: formatBytes(summary.bytesFreed),
  });
}

function RetentionChoice() {
  const stored = useAppStore((state) => state.settings[RETENTION_SETTING]);
  const loadSetting = useAppStore((state) => state.loadSetting);
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);
  const id = useId();
  const days = parseRetentionDays(stored);
  const choices = RETENTION_CHOICES.includes(days) ? RETENTION_CHOICES : [...RETENTION_CHOICES, days];
  const unread = stored === undefined;

  useEffect(() => {
    if (unread) {
      loadSetting(RETENTION_SETTING).catch((failure: unknown) => {
        console.error("could not read the attachment retention", toCoreError(failure).code);
      });
    }
  }, [unread, loadSetting]);

  return (
    <div className="field attachment-retention">
      <label htmlFor={id}>{t("settings.attachments.retention")}</label>
      <select
        id={id}
        className="input select"
        value={String(days)}
        disabled={unread}
        onChange={(event) => {
          setError(null);
          setSetting(RETENTION_SETTING, Number(event.target.value)).catch((failure: unknown) => {
            setError(t("settings.attachments.retention.error", { message: toCoreError(failure).message }));
          });
        }}
      >
        {choices.map((choice) => (
          <option key={choice} value={String(choice)}>
            {retentionLabel(choice)}
          </option>
        ))}
      </select>
      <p className="muted">{t("settings.attachments.retention.hint")}</p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

type CleanState = { kind: "idle" } | { kind: "running" } | { kind: "done"; text: string } | { kind: "error"; text: string };

/** The attachments part of Settings -> Data. */
export function AttachmentSettings() {
  const spaceId = useAppStore((state) => state.currentSpaceId);
  const spaceName = useAppStore((state) => state.spaces.find((space) => space.id === state.currentSpaceId)?.name ?? null);
  const stats = useAppStore((state) => state.attachmentStats);
  const loadAttachmentStats = useAppStore((state) => state.loadAttachmentStats);
  const collectAttachments = useAppStore((state) => state.collectAttachments);
  const [clean, setClean] = useState<CleanState>({ kind: "idle" });
  const titleId = useId();

  useEffect(() => {
    loadAttachmentStats(spaceId);
    return () => {
      loadAttachmentStats(null);
    };
  }, [spaceId, loadAttachmentStats]);

  async function cleanUp(): Promise<void> {
    setClean({ kind: "running" });
    try {
      setClean({ kind: "done", text: describeCollection(await collectAttachments()) });
    } catch (error) {
      setClean({ kind: "error", text: t("settings.attachments.clean.error", { message: toCoreError(error).message }) });
    }
  }

  let usage: string | null = null;
  if (spaceId !== null && stats?.spaceId === spaceId) {
    if (stats.status === "loading") {
      usage = t("settings.attachments.loading");
    } else if (stats.status === "error") {
      usage = t("settings.attachments.error");
    } else if (stats.count === 0) {
      usage = t("settings.attachments.none");
    } else {
      usage = tp("settings.attachments.usage", stats.count, { size: formatBytes(stats.totalBytes) });
    }
  }

  return (
    <section className="subpanel attachment-settings" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("settings.attachments.title")}
      </h3>
      {usage !== null && (
        <dl className="facts">
          <dt>{spaceName === null ? t("settings.attachments.space") : t("settings.attachments.inSpace", { space: spaceName })}</dt>
          <dd role="status">{usage}</dd>
        </dl>
      )}
      <DownloadPolicyChoice />
      <RetentionChoice />
      <div className="row-actions">
        <button
          type="button"
          className="button"
          disabled={clean.kind === "running"}
          onClick={() => {
            void cleanUp();
          }}
        >
          {clean.kind === "running" ? t("settings.attachments.clean.running") : t("settings.attachments.clean")}
        </button>
      </div>
      {clean.kind === "done" && (
        <p className="muted attachment-clean-result" role="status">
          {clean.text}
        </p>
      )}
      {clean.kind === "error" && (
        <p className="error-text" role="alert">
          {clean.text}
        </p>
      )}
    </section>
  );
}
