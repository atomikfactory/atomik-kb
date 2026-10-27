// Settings -> AI (ADR-0037, `docs/architecture/ai.md` section 8.2): the switch for the AI features (Ollama), the
// endpoint with its Local or Remote badge and the connection status, the models (chat, embedding, transcription,
// each limited to the models that can do it) and "think", the semantic index with its progress and "Rebuild", and
// the MCP server (`McpSettings.tsx`). Shown only where the shell has an AI (`CoreClient.hasAi()`); MCP shows where it
// has MCP.
//
// The settings are stored through the store (`updateAiSettings`): turning AI on fills empty models from what
// Ollama lists, turning AI or the index off deletes the vectors. The endpoint is written by the shell alone (a
// remote host asks for consent in a native dialog the page cannot fake): "Save" calls `aiEndpointSet`.

import { DEFAULT_AI_ENDPOINT, toCoreError } from "@atomik-kb/core-client";
import type { AiModelInfo, AiSettings as AiSettingsValue } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { FormEvent } from "react";

import { detailOf, isFieldError } from "../errors";
import { activeLocale, t, tp } from "../i18n";
import { describeAiError, selectAiAvailable, selectAiSettings } from "../store-ai";
import { useAppStore } from "../store-context";
import { McpSettings } from "./McpSettings";

/** While pages wait for the index, the status is read again this often, in milliseconds (the indexer's state is in it). */
const STATUS_REFRESH_MS = 5000;

/** "1,024": a count with the thousands separators of the language. */
function formatCount(count: number): string {
  return count.toLocaleString(activeLocale());
}

/** The models that can do `capability`, and for chat the ones that are not embedding models. */
function modelsFor(models: readonly AiModelInfo[], role: "chat" | "embedding" | "audio"): AiModelInfo[] {
  switch (role) {
    case "chat":
      return models.filter((model) => model.capabilities.includes("completion") && !model.capabilities.includes("embedding"));
    case "embedding":
      return models.filter((model) => model.capabilities.includes("embedding"));
    case "audio":
      return models.filter((model) => model.capabilities.includes("audio"));
  }
}

interface ModelPickerProps {
  label: string;
  hint?: string;
  value: string | undefined;
  models: readonly AiModelInfo[];
  /** The text of the option that stands for no model. */
  emptyLabel: string;
  disabled: boolean;
  onChange: (model: string | undefined) => void;
}

/** A choice among the models of one kind. A saved model that Ollama does not list stays choosable, marked as missing. */
function ModelPicker({ label, hint, value, models, emptyLabel, disabled, onChange }: ModelPickerProps) {
  const id = useId();
  const hintId = useId();
  const missing = value !== undefined && !models.some((model) => model.name === value);
  return (
    <div className="field ai-model">
      <label htmlFor={id}>{label}</label>
      <select
        id={id}
        className="input select"
        value={value ?? ""}
        disabled={disabled}
        aria-describedby={hint === undefined ? undefined : hintId}
        onChange={(event) => {
          onChange(event.target.value === "" ? undefined : event.target.value);
        }}
      >
        <option value="">{emptyLabel}</option>
        {missing && <option value={value}>{t("settings.ai.model.missing", { model: value })}</option>}
        {models.map((model) => (
          <option key={model.name} value={model.name}>
            {model.parameterSize === undefined ? model.name : `${model.name} (${model.parameterSize})`}
          </option>
        ))}
      </select>
      {hint !== undefined && (
        <p className="muted" id={hintId}>
          {hint}
        </p>
      )}
    </div>
  );
}

/** The endpoint: the address, "Save", the Local or Remote badge and what the connection says. */
function EndpointForm() {
  const status = useAppStore((state) => state.aiStatus);
  const loading = useAppStore((state) => state.aiStatusLoading);
  const statusError = useAppStore((state) => state.aiStatusError);
  const loadAiStatus = useAppStore((state) => state.loadAiStatus);
  const saveAiEndpoint = useAppStore((state) => state.saveAiEndpoint);
  const stored = status?.endpoint ?? DEFAULT_AI_ENDPOINT;
  const [draft, setDraft] = useState(stored);
  const [knownStored, setKnownStored] = useState(stored);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputId = useId();
  const errorId = useId();

  // Follow an endpoint stored elsewhere (adjusting state while rendering, as DeviceNameForm does).
  if (stored !== knownStored) {
    setKnownStored(stored);
    setDraft(stored);
  }

  async function save(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (saving) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await saveAiEndpoint(draft);
    } catch (failure) {
      const reason = toCoreError(failure);
      if (isFieldError(reason, "url") && detailOf(reason, "reason") === "declined") {
        setError(t("settings.ai.endpoint.declined"));
      } else if (isFieldError(reason, "url")) {
        // One sentence of ours: the core's own wording is a predicate that would read doubled here.
        setError(t("settings.ai.endpoint.invalid"));
      } else {
        setError(t("settings.ai.endpoint.error", { message: describeAiError(reason.toApiError()) }));
      }
    }
    setSaving(false);
  }

  let line: string | null;
  if (loading && status === null) {
    line = t("settings.ai.status.checking");
  } else if (status === null) {
    line = statusError === null ? null : t("settings.ai.status.unreachable", { message: statusError });
  } else if (status.reachable) {
    line = tp("settings.ai.status.connected", status.models.length, { version: status.version ?? "?" });
  } else {
    line = t("settings.ai.status.unreachable", {
      message: status.error === undefined ? t("ai.error.unreachable") : describeAiError(status.error),
    });
  }

  return (
    <form
      className="field ai-endpoint"
      noValidate
      onSubmit={(event) => {
        void save(event);
      }}
    >
      <label htmlFor={inputId}>{t("settings.ai.endpoint")}</label>
      <div className="row">
        <input
          id={inputId}
          className="input"
          type="text"
          inputMode="url"
          value={draft}
          autoComplete="off"
          spellCheck={false}
          aria-invalid={error !== null}
          aria-describedby={error === null ? undefined : errorId}
          onChange={(event) => {
            setDraft(event.target.value);
            setError(null);
          }}
        />
        <button type="submit" className="button" disabled={saving || draft.trim() === "" || draft.trim() === stored}>
          {saving ? t("settings.ai.endpoint.saving") : t("settings.ai.endpoint.save")}
        </button>
      </div>
      {status !== null && (
        <p className="ai-endpoint-badge">
          <span className={status.local ? "ai-badge ai-badge-local" : "ai-badge ai-badge-remote"}>
            {status.local ? t("settings.ai.local") : t("settings.ai.remote")}
          </span>
        </p>
      )}
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
      <div className="ai-status-line">
        <p role="status">{line}</p>
        <button
          type="button"
          className="link-button"
          disabled={loading}
          onClick={() => {
            void loadAiStatus();
          }}
        >
          {t("settings.ai.status.refresh")}
        </button>
      </div>
    </form>
  );
}

/** The semantic index: the switch, how many pages are indexed and "Rebuild". */
function IndexSection({ settings }: { settings: AiSettingsValue }) {
  const index = useAppStore((state) => state.aiIndex);
  const indexer = useAppStore((state) => state.aiStatus?.indexer);
  const updateAiSettings = useAppStore((state) => state.updateAiSettings);
  const rebuildAiIndex = useAppStore((state) => state.rebuildAiIndex);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const active = settings.enabled && settings.semanticIndex;

  async function rebuild(): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      await rebuildAiIndex();
    } catch (failure) {
      setError(t("settings.ai.index.error", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  let progress: string;
  if (!active) {
    progress = t("settings.ai.index.off");
  } else if (index === null) {
    progress = t("settings.ai.index.unknown");
  } else {
    progress = t("settings.ai.index.progress", { done: formatCount(index.pagesIndexed), total: formatCount(index.pagesTotal) });
  }
  let state: string | null = null;
  if (active && indexer?.phase === "error") {
    state = t("settings.ai.index.state.error", { message: indexer.error ?? t("ai.error.unreachable") });
  } else if (active && index !== null && index.dirty > 0) {
    state = t("settings.ai.index.state.indexing");
  } else if (active && index !== null) {
    state = t("settings.ai.index.state.idle");
  }

  return (
    <section className="subpanel ai-index" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("settings.ai.index.title")}
      </h3>
      <label className="checkbox-label">
        <input
          type="checkbox"
          role="switch"
          checked={settings.semanticIndex}
          disabled={!settings.enabled}
          onChange={(event) => {
            updateAiSettings({ semanticIndex: event.target.checked }).catch((failure: unknown) => {
              setError(t("settings.ai.error", { message: toCoreError(failure).message }));
            });
          }}
        />
        <span>{t("settings.ai.index.switch")}</span>
      </label>
      <p className="muted">{t("settings.ai.index.hint")}</p>
      <p className="ai-index-progress" role="status">
        {progress}
        {state !== null && <span className="muted"> {state}</span>}
      </p>
      <div className="row-actions">
        <button
          type="button"
          className="button"
          disabled={!active || busy}
          onClick={() => {
            void rebuild();
          }}
        >
          {busy ? t("settings.ai.index.rebuilding") : t("settings.ai.index.rebuild")}
        </button>
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** Settings -> AI. */
export function AiSettings() {
  const available = useAppStore(selectAiAvailable);
  const settings = useAppStore(selectAiSettings);
  const models = useAppStore((state) => state.aiStatus?.models);
  const dirty = useAppStore((state) => state.aiIndex?.dirty ?? 0);
  const loadAiStatus = useAppStore((state) => state.loadAiStatus);
  const updateAiSettings = useAppStore((state) => state.updateAiSettings);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const titleId = useId();
  const modelsTitleId = useId();

  useEffect(() => {
    if (available) {
      void loadAiStatus();
    }
  }, [available, loadAiStatus]);

  // While pages wait to be indexed the status is read again now and then: it holds the indexer's state.
  useEffect(() => {
    if (!available || !settings.enabled || dirty === 0) {
      return;
    }
    const timer = setInterval(() => {
      void loadAiStatus();
    }, STATUS_REFRESH_MS);
    return () => {
      clearInterval(timer);
    };
  }, [available, settings.enabled, dirty, loadAiStatus]);

  function change(patch: Partial<AiSettingsValue>): void {
    setError(null);
    setSaving(true);
    updateAiSettings(patch)
      .catch((failure: unknown) => {
        setError(t("settings.ai.error", { message: toCoreError(failure).message }));
      })
      .finally(() => {
        setSaving(false);
      });
  }

  if (!available) {
    return <McpSettings />;
  }

  const installed = models ?? [];
  const chatModels = modelsFor(installed, "chat");
  const thinks = installed.find((model) => model.name === settings.chatModel)?.capabilities.includes("thinking") === true;

  return (
    <>
      <section className="subpanel ai-settings" aria-labelledby={titleId}>
        <h3 className="subheading" id={titleId}>
          {t("settings.ai.title")}
        </h3>
        <p className="muted">{t("settings.ai.intro")}</p>
        <label className="checkbox-label ai-enabled">
          <input
            type="checkbox"
            role="switch"
            checked={settings.enabled}
            disabled={saving}
            onChange={(event) => {
              change({ enabled: event.target.checked });
            }}
          />
          <span>{t("settings.ai.enabled")}</span>
        </label>
        <EndpointForm />
        {error !== null && (
          <p className="error-text" role="alert">
            {error}
          </p>
        )}
      </section>

      <section className="subpanel ai-models" aria-labelledby={modelsTitleId}>
        <h3 className="subheading" id={modelsTitleId}>
          {t("settings.ai.models.title")}
        </h3>
        <ModelPicker
          label={t("settings.ai.model.chat")}
          hint={t("settings.ai.model.chat.hint")}
          value={settings.chatModel}
          models={chatModels}
          emptyLabel={t("settings.ai.model.choose")}
          disabled={!settings.enabled}
          onChange={(model) => {
            change({ chatModel: model });
          }}
        />
        <ModelPicker
          label={t("settings.ai.model.embedding")}
          hint={t("settings.ai.model.embedding.hint")}
          value={settings.embeddingModel}
          models={modelsFor(installed, "embedding")}
          emptyLabel={t("settings.ai.model.choose")}
          disabled={!settings.enabled}
          onChange={(model) => {
            change({ embeddingModel: model });
          }}
        />
        <ModelPicker
          label={t("settings.ai.model.transcription")}
          hint={t("settings.ai.model.transcription.hint")}
          value={settings.transcriptionModel}
          models={modelsFor(installed, "audio")}
          emptyLabel={t("settings.ai.model.none")}
          disabled={!settings.enabled}
          onChange={(model) => {
            change({ transcriptionModel: model });
          }}
        />
        <label className="checkbox-label ai-think">
          <input
            type="checkbox"
            role="switch"
            checked={settings.think}
            disabled={!settings.enabled}
            onChange={(event) => {
              change({ think: event.target.checked });
            }}
          />
          <span>{t("settings.ai.think")}</span>
        </label>
        <p className="muted">{settings.chatModel !== undefined && !thinks && models !== undefined ? t("settings.ai.think.unsupported") : t("settings.ai.think.hint")}</p>
      </section>

      <IndexSection settings={settings} />
      <McpSettings />
    </>
  );
}
