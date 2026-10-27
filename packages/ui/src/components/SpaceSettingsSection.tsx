// The settings of a shared space an Owner changes (Milestone 13, ADR-0043 and ADR-0047): how long trashed pages stay before the
// Owners' devices delete them for good (the space's own limit; the device setting is for personal spaces), and whether Editors may
// create view links (ADR-0051: the page menu's "Share a view link..." follows it, and so does the core, which refuses an Editor
// while it is off).

import { toCoreError } from "@atomik-kb/core-client";
import type { SpaceId, SpaceSettings } from "@atomik-kb/core-client";
import { useId, useState } from "react";

import { t, tp } from "../i18n";
import { retentionOptions } from "../sharing";
import type { SpaceSettingsChange } from "../store-sharing";
import { useAppStore } from "../store-context";

export function SpaceSettingsSection({ spaceId, settings }: { spaceId: SpaceId; settings: SpaceSettings }) {
  const setSpaceSettings = useAppStore((state) => state.setSpaceSettings);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const retentionId = useId();
  const retentionHintId = useId();
  const linkHintId = useId();
  const days = Number(settings.trashRetentionDays);

  async function change(next: SpaceSettingsChange): Promise<void> {
    setError(null);
    setBusy(true);
    try {
      await setSpaceSettings(spaceId, next);
    } catch (failure) {
      setError(t("sharing.settings.error", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  return (
    <section className="members-section" aria-labelledby={`settings-${spaceId}`}>
      <h3 className="members-heading" id={`settings-${spaceId}`}>
        {t("sharing.settings.title")}
      </h3>
      <div className="field">
        <label htmlFor={retentionId}>{t("sharing.settings.retention")}</label>
        <select
          id={retentionId}
          className="select"
          value={days}
          disabled={busy}
          aria-describedby={retentionHintId}
          onChange={(event) => {
            void change({ trashRetentionDays: Number(event.target.value) });
          }}
        >
          {retentionOptions(days).map((option) => (
            <option key={option} value={option}>
              {option === 0 ? t("sharing.settings.retention.keep") : tp("sharing.days", option)}
            </option>
          ))}
        </select>
        <p className="muted" id={retentionHintId}>
          {t("sharing.settings.retentionHint")}
        </p>
      </div>
      <label className="checkbox-label members-link-setting">
        <input
          type="checkbox"
          checked={settings.editorsMayLink}
          disabled={busy}
          aria-describedby={linkHintId}
          onChange={(event) => {
            void change({ editorsMayLink: event.target.checked });
          }}
        />
        <span>{t("sharing.settings.editorsLink")}</span>
      </label>
      <p className="muted" id={linkHintId}>
        {t("sharing.settings.editorsLinkHint")}
      </p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
