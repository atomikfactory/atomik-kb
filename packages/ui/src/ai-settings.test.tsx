// Settings -> AI (ADR-0037, screens/AiSettings.tsx): the section is there only where the shell has an AI, the
// switch fills models on, the endpoint with its Local or Remote badge and the status line, the model pickers
// limited by capability, "think", and the semantic index with its progress and "Rebuild".

import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t, tp } from "./i18n";
import { AI_ON, AI_SETTING, AI_SPACES, aiOf } from "./test/ai";
import { openScreen, renderApp } from "./test/render";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function openSettings(options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({ workspaceName: "Research", spaces: AI_SPACES, ai: { mode: "fake" }, ...options });
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  await screen.findByRole("heading", { name: t("settings.ai.title") });
  return app;
}

const enabledSwitch = () => screen.getByRole("switch", { name: t("settings.ai.enabled") });
const picker = (name: Parameters<typeof t>[0]) => screen.getByRole("combobox", { name: t(name) });
const optionNames = (select: HTMLElement) => within(select).getAllByRole("option").map((option) => option.textContent);

/** The stored AI settings. */
async function stored(app: Awaited<ReturnType<typeof openSettings>>) {
  const response = await app.memory.dispatch({ type: "SettingsGet", data: { key: AI_SETTING } });
  return response.type === "Setting" ? response.data : null;
}

describe("the section", () => {
  it("is not there where the shell has no AI", async () => {
    const app = renderApp({ workspaceName: "Research", spaces: AI_SPACES });
    await openScreen(app.user, "nav.settings");
    await screen.findByRole("heading", { name: t("settings.title") });
    expect(screen.queryByRole("heading", { name: t("settings.ai") })).not.toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: t("settings.ai.enabled") })).not.toBeInTheDocument();
  });

  it("is there with an AI: off by default, with the connection and the Local badge", async () => {
    await openSettings();
    expect(screen.getByRole("heading", { level: 2, name: t("settings.ai") })).toBeInTheDocument();
    expect(enabledSwitch()).not.toBeChecked();
    expect(await screen.findByText(tp("settings.ai.status.connected", 5, { version: "0.35.0" }))).toBeInTheDocument();
    expect(screen.getByLabelText(t("settings.ai.endpoint"))).toHaveValue("http://127.0.0.1:11434");
    expect(screen.getByText(t("settings.ai.local"))).toBeInTheDocument();
    expect(screen.queryByText(t("settings.ai.remote"))).not.toBeInTheDocument();
  });

  it("says 1 model in the singular", async () => {
    const app = await openSettings();
    aiOf(app.memory).setModels([{ name: "qwen3:8b", capabilities: ["completion"], size: 1 }]);
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.status.refresh") }));
    expect(await screen.findByText(tp("settings.ai.status.connected", 1, { version: "0.35.0" }))).toBeInTheDocument();
  });

  it("says Ollama cannot be reached, and checks again on request", async () => {
    const app = await openSettings();
    await screen.findByText(tp("settings.ai.status.connected", 5, { version: "0.35.0" }));
    aiOf(app.memory).setReachable(false, "connection refused");
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.status.refresh") }));
    expect(await screen.findByText(t("settings.ai.status.unreachable", { message: t("ai.error.unreachable") }))).toBeInTheDocument();
    aiOf(app.memory).setReachable(true);
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.status.refresh") }));
    expect(await screen.findByText(tp("settings.ai.status.connected", 5, { version: "0.35.0" }))).toBeInTheDocument();
  });
});

describe("turning AI on", () => {
  it("fills the models from what Ollama lists and stores the settings", async () => {
    const app = await openSettings();
    await screen.findByText(tp("settings.ai.status.connected", 5, { version: "0.35.0" }));
    expect(picker("settings.ai.model.chat")).toBeDisabled();
    await app.user.click(enabledSwitch());
    await waitFor(() => {
      expect(enabledSwitch()).toBeChecked();
    });
    await waitFor(() => {
      expect(picker("settings.ai.model.chat")).toHaveValue("qwen3:8b");
    });
    expect(picker("settings.ai.model.embedding")).toHaveValue("nomic-embed-text:latest");
    // Transcription is left for the person to choose.
    expect(picker("settings.ai.model.transcription")).toHaveValue("");
    expect(picker("settings.ai.model.chat")).toBeEnabled();
    expect(await stored(app)).toEqual({
      enabled: true,
      chatModel: "qwen3:8b",
      embeddingModel: "nomic-embed-text:latest",
      semanticIndex: true,
      think: false,
    });
  });

  it("keeps the models already chosen", async () => {
    const app = await openSettings({
      settings: { [AI_SETTING]: { enabled: false, chatModel: "llama3.2:3b", semanticIndex: true, think: false } },
    });
    await app.user.click(enabledSwitch());
    await waitFor(() => {
      expect(picker("settings.ai.model.embedding")).toHaveValue("nomic-embed-text:latest");
    });
    expect(picker("settings.ai.model.chat")).toHaveValue("llama3.2:3b");
  });

  it("turns off again, and the vectors go", async () => {
    const app = await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(app.memory).advanceIndex();
    await screen.findByText(t("settings.ai.index.progress", { done: 4, total: 4 }));
    await app.user.click(enabledSwitch());
    await waitFor(() => {
      expect(enabledSwitch()).not.toBeChecked();
    });
    expect(await screen.findByText(t("settings.ai.index.off"))).toBeInTheDocument();
    const response = await app.memory.dispatch({ type: "AiIndexStatus" });
    expect(response).toMatchObject({ data: { pagesIndexed: 0 } });
  });

  it("says so when the setting cannot be saved", async () => {
    const app = await openSettings();
    app.memory.failNext({ code: "Storage", message: "disk full" });
    await app.user.click(enabledSwitch());
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.ai.error", { message: "disk full" }));
    expect(enabledSwitch()).not.toBeChecked();
  });
});

describe("the models", () => {
  it("lists the models that can do the job: chat without embedding models, embedding, audio", async () => {
    await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    await screen.findByText(tp("settings.ai.status.connected", 5, { version: "0.35.0" }));
    const chat = picker("settings.ai.model.chat");
    await waitFor(() => {
      expect(optionNames(chat)).toContain("llama3.2:3b (3.2B)");
    });
    expect(optionNames(chat)).toEqual([
      t("settings.ai.model.choose"),
      "qwen3:8b (8.2B)",
      "llama3.2:3b (3.2B)",
      "gemma4:12b (12B)",
      "r2t2:latest (2B)",
    ]);
    expect(optionNames(picker("settings.ai.model.embedding"))).toEqual([t("settings.ai.model.choose"), "nomic-embed-text:latest (137M)"]);
    expect(optionNames(picker("settings.ai.model.transcription"))).toEqual([t("settings.ai.model.none"), "gemma4:12b (12B)", "r2t2:latest (2B)"]);
  });

  it("stores a chosen model, and no model for transcription", async () => {
    const app = await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    await waitFor(() => {
      expect(optionNames(picker("settings.ai.model.chat"))).toContain("llama3.2:3b (3.2B)");
    });
    await app.user.selectOptions(picker("settings.ai.model.chat"), "llama3.2:3b");
    await waitFor(async () => {
      expect(await stored(app)).toMatchObject({ chatModel: "llama3.2:3b" });
    });
    await app.user.selectOptions(picker("settings.ai.model.transcription"), "gemma4:12b");
    await waitFor(async () => {
      expect(await stored(app)).toMatchObject({ transcriptionModel: "gemma4:12b" });
    });
    await app.user.selectOptions(picker("settings.ai.model.transcription"), t("settings.ai.model.none"));
    await waitFor(async () => {
      expect((await stored(app)) as Record<string, unknown>).not.toHaveProperty("transcriptionModel");
    });
  });

  it("keeps a model that is no longer installed choosable, marked as missing", async () => {
    await openSettings({ settings: { [AI_SETTING]: { ...(AI_ON as object), chatModel: "old-model:7b" } } });
    const chat = picker("settings.ai.model.chat");
    await waitFor(() => {
      expect(chat).toHaveValue("old-model:7b");
    });
    expect(screen.getByRole("option", { name: t("settings.ai.model.missing", { model: "old-model:7b" }) })).toBeInTheDocument();
  });

  it("explains the transcription model", async () => {
    await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    expect(screen.getByText(t("settings.ai.model.transcription.hint"))).toBeInTheDocument();
  });

  it("stores the think switch, and says it has no effect on a model that cannot think", async () => {
    const app = await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    const think = screen.getByRole("switch", { name: t("settings.ai.think") });
    await waitFor(() => {
      expect(screen.getByText(t("settings.ai.think.hint"))).toBeInTheDocument();
    });
    await app.user.click(think);
    await waitFor(async () => {
      expect(await stored(app)).toMatchObject({ think: true });
    });
    await waitFor(() => {
      expect(optionNames(picker("settings.ai.model.chat"))).toContain("llama3.2:3b (3.2B)");
    });
    await app.user.selectOptions(picker("settings.ai.model.chat"), "llama3.2:3b");
    expect(await screen.findByText(t("settings.ai.think.unsupported"))).toBeInTheDocument();
  });
});

describe("the endpoint", () => {
  it("saves a local address and keeps the Local badge; the button waits for a change", async () => {
    const app = await openSettings();
    const field = await screen.findByLabelText(t("settings.ai.endpoint"));
    const save = screen.getByRole("button", { name: t("settings.ai.endpoint.save") });
    expect(save).toBeDisabled();
    await app.user.clear(field);
    await app.user.type(field, "http://localhost:11500");
    expect(save).toBeEnabled();
    await app.user.click(save);
    await waitFor(() => {
      expect(screen.getByLabelText(t("settings.ai.endpoint"))).toHaveValue("http://localhost:11500");
    });
    expect(screen.getByText(t("settings.ai.local"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("settings.ai.endpoint.save") })).toBeDisabled();
    const response = await app.memory.dispatch({ type: "SettingsGet", data: { key: "app.ai.endpoint" } });
    expect(response).toMatchObject({ data: { url: "http://localhost:11500", remoteConsent: false } });
  });

  it("marks a remote address, where text leaves this device", async () => {
    const app = await openSettings();
    const field = await screen.findByLabelText(t("settings.ai.endpoint"));
    await app.user.clear(field);
    await app.user.type(field, "https://ollama.example.com:8443");
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.endpoint.save") }));
    expect(await screen.findByText(t("settings.ai.remote"))).toBeInTheDocument();
    expect(screen.queryByText(t("settings.ai.local"))).not.toBeInTheDocument();
  });

  it("says nothing changed when the native dialog was declined", async () => {
    const app = await openSettings();
    aiOf(app.memory).setDeclineConsent(true);
    const field = await screen.findByLabelText(t("settings.ai.endpoint"));
    await app.user.clear(field);
    await app.user.type(field, "http://192.168.1.5:11434");
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.endpoint.save") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.ai.endpoint.declined"));
    expect(screen.getByText(t("settings.ai.local"))).toBeInTheDocument();
    expect(field).toHaveAttribute("aria-invalid", "true");
  });

  it("says what is wrong with an address", async () => {
    const app = await openSettings();
    const field = await screen.findByLabelText(t("settings.ai.endpoint"));
    await app.user.clear(field);
    await app.user.type(field, "ftp://nope");
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.endpoint.save") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.ai.endpoint.invalid"));
    expect(screen.getByRole("alert").textContent).not.toMatch(/address.*address/i);
    // Typing again clears the message.
    await app.user.type(field, "x");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("the semantic index", () => {
  it("shows how many pages are indexed, follows the indexer, and is off with AI off", async () => {
    const app = await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    expect(await screen.findByText(t("settings.ai.index.progress", { done: 0, total: 4 }))).toBeInTheDocument();
    expect(screen.getByText(t("settings.ai.index.state.indexing"))).toBeInTheDocument();
    await aiOf(app.memory).advanceIndex();
    expect(await screen.findByText(t("settings.ai.index.progress", { done: 4, total: 4 }))).toBeInTheDocument();
    expect(screen.getByText(t("settings.ai.index.state.idle"))).toBeInTheDocument();
  });

  it("rebuilds: deletes the vectors, which the indexer then makes again", async () => {
    const app = await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(app.memory).advanceIndex();
    await screen.findByText(t("settings.ai.index.progress", { done: 4, total: 4 }));
    await app.user.click(screen.getByRole("button", { name: t("settings.ai.index.rebuild") }));
    expect(await screen.findByText(t("settings.ai.index.progress", { done: 0, total: 4 }))).toBeInTheDocument();
  });

  it("turns off with its switch: the vectors go and the controls stop", async () => {
    const app = await openSettings({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(app.memory).advanceIndex();
    await screen.findByText(t("settings.ai.index.progress", { done: 4, total: 4 }));
    await app.user.click(screen.getByRole("switch", { name: t("settings.ai.index.switch") }));
    expect(await screen.findByText(t("settings.ai.index.off"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("settings.ai.index.rebuild") })).toBeDisabled();
    expect(await stored(app)).toMatchObject({ semanticIndex: false });
    const response = await app.memory.dispatch({ type: "AiIndexStatus" });
    expect(response).toMatchObject({ data: { pagesIndexed: 0 } });
  });

  it("is disabled while AI is off", async () => {
    await openSettings();
    expect(screen.getByRole("switch", { name: t("settings.ai.index.switch") })).toBeDisabled();
    expect(screen.getByText(t("settings.ai.index.off"))).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: t("settings.ai.think") })).toBeDisabled();
  });
});
