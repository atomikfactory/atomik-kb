import { toCoreError } from "@atomik-kb/core-client";
import type { DeviceInfo } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { RecoveryKitBanner } from "../components/RecoveryKitBanner";
import { RotationBanner, StaleKitBanner } from "../components/DeviceBanners";
import { SyncForkBanner } from "../components/SyncForkBanner";
import { isFieldError } from "../errors";
import { formatTimestamp } from "../format";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { THEME_KEY, selectTheme } from "../store";
import type { Theme } from "../store";
import { useAppStore } from "../store-context";
import { RECENT_PAGES_KEY, parseRecentPages } from "../store-layout";
import { AccentChoice } from "./AccentChoice";
import { AiSettings } from "./AiSettings";
import { DictationSettings } from "./DictationSettings";
import { AttachmentSettings } from "./AttachmentSettings";
import { DesktopSettings } from "./DesktopSettings";
import { LanguagePicker } from "./LanguagePicker";
import { SecuritySettings } from "./SecuritySettings";
import { SyncSettings } from "./SyncSettings";
import { TrashSettings } from "./TrashSettings";

const THEME_OPTIONS: readonly { value: Theme; label: MessageKey }[] = [
  { value: "light", label: "settings.theme.light" },
  { value: "dark", label: "settings.theme.dark" },
  { value: "system", label: "settings.theme.system" },
];

type DeviceFormState =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved" }
  | { kind: "error"; message: string };

function describeRenameError(error: unknown): string {
  const failure = toCoreError(error);
  if (isFieldError(failure, "name")) {
    return t("form.fieldError", { message: failure.message });
  }
  return t("settings.device.error.generic", { message: failure.message });
}

function ThemeChoice() {
  const theme = useAppStore(selectTheme);
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);

  function choose(next: Theme): void {
    setError(null);
    setSetting(THEME_KEY, next).catch((failure: unknown) => {
      setError(t("settings.theme.error", { message: toCoreError(failure).message }));
    });
  }

  return (
    <fieldset className="fieldset">
      <legend>{t("settings.theme.legend")}</legend>
      <div className="radio-group">
        {THEME_OPTIONS.map(({ value, label }) => (
          <label className="radio" key={value}>
            <input
              type="radio"
              name="theme"
              value={value}
              checked={theme === value}
              onChange={() => {
                choose(value);
              }}
            />
            <span>{t(label)}</span>
          </label>
        ))}
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </fieldset>
  );
}

/** "Clear recent pages" (ADR-0029): the list kept on this device for the search and the Home screen. */
function RecentPagesChoice() {
  const hasRecent = useAppStore((state) => parseRecentPages(state.settings[RECENT_PAGES_KEY]).length > 0);
  const clearRecentPages = useAppStore((state) => state.clearRecentPages);
  const [state, setState] = useState<{ kind: "idle" } | { kind: "cleared" } | { kind: "error"; message: string }>({
    kind: "idle",
  });

  function clear(): void {
    setState({ kind: "idle" });
    clearRecentPages().then(
      () => {
        setState({ kind: "cleared" });
      },
      (failure: unknown) => {
        setState({ kind: "error", message: t("recent.settings.error", { message: toCoreError(failure).message }) });
      },
    );
  }

  return (
    <fieldset className="fieldset">
      <legend>{t("recent.settings.legend")}</legend>
      <p className="muted">{t("recent.settings.hint")}</p>
      <div className="row">
        <button type="button" className="button" disabled={!hasRecent} onClick={clear}>
          {t("recent.settings.clear")}
        </button>
      </div>
      {state.kind === "error" && (
        <p className="error-text" role="alert">
          {state.message}
        </p>
      )}
      <p className="muted" role="status">
        {state.kind === "cleared" ? t("recent.settings.cleared") : ""}
      </p>
    </fieldset>
  );
}

function DeviceNameForm({ device }: { device: DeviceInfo }) {
  const renameDevice = useAppStore((state) => state.renameDevice);
  const [name, setName] = useState(device.name);
  const [state, setState] = useState<DeviceFormState>({ kind: "idle" });
  const inputId = useId();
  const errorId = useId();

  // Follow renames that did not come from this form (adjusting state during render,
  // as the React docs recommend, instead of an effect).
  const [knownName, setKnownName] = useState(device.name);
  if (device.name !== knownName) {
    setKnownName(device.name);
    setName(device.name);
  }

  const saving = state.kind === "saving";
  const unchanged = name.trim() === device.name;

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (saving) {
      return;
    }
    if (name.trim() === "") {
      setState({ kind: "error", message: t("settings.device.error.empty") });
      return;
    }
    setState({ kind: "saving" });
    try {
      await renameDevice(name);
      setState({ kind: "saved" });
    } catch (failure) {
      setState({ kind: "error", message: describeRenameError(failure) });
    }
  }

  return (
    <form
      className="form"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      <label htmlFor={inputId}>{t("settings.device.nameLabel")}</label>
      <div className="row">
        <input
          id={inputId}
          className="input"
          type="text"
          value={name}
          autoComplete="off"
          aria-invalid={state.kind === "error"}
          aria-describedby={state.kind === "error" ? errorId : undefined}
          onChange={(event) => {
            setName(event.target.value);
            setState({ kind: "idle" });
          }}
        />
        <button type="submit" className="button" disabled={saving || unchanged}>
          {saving ? t("settings.device.saving") : t("settings.device.save")}
        </button>
      </div>
      {state.kind === "error" && (
        <p className="error-text" id={errorId} role="alert">
          {state.message}
        </p>
      )}
      <p className="muted" role="status">
        {state.kind === "saved" ? t("settings.device.saved") : ""}
      </p>
    </form>
  );
}

/** Theme, accent colour, language, device name, the desktop shell's tray and shortcut, security (kit, app lock, devices, backup), sync, AI and MCP, dictation, where the data lives, attachments and the trash. */
export function SettingsScreen() {
  const device = useAppStore((state) => state.device);
  // AI and the MCP server need the desktop shell (or the in-memory fake): the section is left out without them.
  const hasAiSection = useAppStore((state) => state.aiCapabilities.ai || state.aiCapabilities.mcp);
  // Dictation needs the desktop shell's engines (or the in-memory fake, `?dictation=fake`) (ADR-0053).
  const hasDictationSection = useAppStore((state) => state.aiClient.hasDictation());
  // The tray and the quick-capture shortcut exist in the desktop shell only (ADR-0040): not in the in-memory dev UI or the web build.
  const hasDesktopSection = useAppStore((state) => state.desktopShell);
  const status = useAppStore((state) => state.status);
  const firstLaunch = device === null ? null : formatTimestamp(device.createdAt);

  return (
    <section aria-labelledby="settings-title">
      <h1 className="title" id="settings-title">
        {t("settings.title")}
      </h1>
      <RotationBanner />
      <SyncForkBanner />
      <StaleKitBanner />
      <RecoveryKitBanner />

      <section className="panel" aria-labelledby="settings-appearance">
        <h2 className="subtitle" id="settings-appearance">
          {t("settings.appearance")}
        </h2>
        <ThemeChoice />
        <AccentChoice />
        <LanguagePicker />
        <RecentPagesChoice />
      </section>

      {device !== null && (
        <section className="panel" aria-labelledby="settings-device">
          <h2 className="subtitle" id="settings-device">
            {t("settings.device")}
          </h2>
          <DeviceNameForm device={device} />
          <dl className="facts">
            <dt>{t("settings.device.platform")}</dt>
            <dd>{device.platform}</dd>
            {firstLaunch !== null && (
              <>
                <dt>{t("settings.device.firstLaunch")}</dt>
                <dd>
                  <time dateTime={firstLaunch.iso}>{firstLaunch.text}</time>
                </dd>
              </>
            )}
          </dl>
        </section>
      )}

      {hasDesktopSection && (
        <section className="panel" aria-labelledby="settings-desktop">
          <h2 className="subtitle" id="settings-desktop">
            {t("settings.desktop")}
          </h2>
          <DesktopSettings />
        </section>
      )}

      <section className="panel" aria-labelledby="settings-security">
        <h2 className="subtitle" id="settings-security">
          {t("settings.security")}
        </h2>
        <SecuritySettings />
      </section>

      <section className="panel" aria-labelledby="settings-sync">
        <h2 className="subtitle" id="settings-sync">
          {t("settings.sync")}
        </h2>
        <SyncSettings />
      </section>

      {hasAiSection && (
        <section className="panel" aria-labelledby="settings-ai">
          <h2 className="subtitle" id="settings-ai">
            {t("settings.ai")}
          </h2>
          <AiSettings />
        </section>
      )}

      {hasDictationSection && (
        <section className="panel" aria-labelledby="settings-dictation">
          <h2 className="subtitle" id="settings-dictation">
            {t("settings.dictation")}
          </h2>
          <DictationSettings />
        </section>
      )}

      {status !== null && (
        <section className="panel" aria-labelledby="settings-data">
          <h2 className="subtitle" id="settings-data">
            {t("settings.data")}
          </h2>
          <dl className="facts">
            <dt>{t("settings.data.location")}</dt>
            <dd className="mono">{status.dataDir}</dd>
            <dt>{t("settings.data.encrypted")}</dt>
            <dd>{status.dbEncrypted ? t("common.yes") : t("common.no")}</dd>
          </dl>
          <AttachmentSettings />
          <TrashSettings />
        </section>
      )}
    </section>
  );
}
