// Settings -> Dictation (ADR-0053, screens/DictationSettings.tsx): the section is there only where the transport can dictate,
// lists the engines as radios ("Accurate" first, recommended) with status, size, languages and credits, chooses the
// engine of this device, downloads a model with progress and Cancel, removes it, takes model files from a folder, and
// disables Ollama with the reason and the way to the AI section.

import { CoreError } from "@atomik-kb/core-client";
import type { MemoryDictationOptions } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fakeOf } from "./editor/dictation/dictation-test-kit";
import { formatSize } from "./editor/dictation/text";
import { resetLanguage, t } from "./i18n";
import { AI_SPACES } from "./test/ai";
import { openScreen, renderApp } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
  resetLanguage();
});

async function openSettings(dictation: MemoryDictationOptions | "fake" | null = {}, extra: Parameters<typeof renderApp>[0] = {}) {
  const options: Parameters<typeof renderApp>[0] = { workspaceName: "Research", spaces: AI_SPACES, ...extra };
  if (dictation !== null) {
    options.dictation = dictation === "fake" ? "fake" : { installStepMs: 20, ...dictation };
  }
  const app = renderApp(options);
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  return app;
}

const section = () => screen.getByRole("heading", { level: 2, name: t("settings.dictation") }).closest("section") as HTMLElement;
const engineRow = (name: RegExp) => within(section()).getByRole("radio", { name }).closest("li") as HTMLElement;
const button = (row: HTMLElement, name: string) => within(row).getByRole("button", { name });

describe("the section", () => {
  it("is not there where the transport cannot dictate", async () => {
    await openSettings(null);
    expect(screen.queryByRole("heading", { name: t("settings.dictation") })).not.toBeInTheDocument();
  });

  it("lists the engines as radios, Accurate first and recommended, none chosen to begin with", async () => {
    await openSettings({});
    expect(await screen.findByRole("heading", { level: 2, name: t("settings.dictation") })).toBeInTheDocument();
    expect(within(section()).getByRole("heading", { level: 3, name: t("settings.dictation.engine.title") })).toBeInTheDocument();
    await within(section()).findByRole("radio", { name: /Light/ });
    const radios = within(section()).getAllByRole("radio");
    expect(radios.map((item) => item.getAttribute("value"))).toEqual(["accurate", "light", "ollama"]);
    expect(radios.some((item) => (item as HTMLInputElement).checked)).toBe(false);
    expect(within(section()).getAllByText(t("dictation.recommended"))).toHaveLength(1);
    expect(radios[0]?.closest("li")).toHaveTextContent(t("dictation.recommended"));
  });

  it("shows for each engine its status, download size in MB, languages by name and credits", async () => {
    await openSettings({ installed: ["light"] });
    await within(section()).findByRole("radio", { name: /Light/ });
    const accurate = engineRow(/Accurate/);
    const light = engineRow(/Light/);
    expect(within(accurate).getByText(t("dictation.engine.notInstalled"))).toBeInTheDocument();
    expect(within(accurate).getByText(t("dictation.size", { size: formatSize(670_478_772) }))).toBeInTheDocument();
    expect(within(accurate).getByText(/^Languages: .*Bulgarian.*Spanish.*Ukrainian/)).toBeInTheDocument();
    expect(within(accurate).getByText(/CC-BY-4\.0/)).toBeInTheDocument(); // Parakeet's licence asks for the credit
    expect(within(accurate).getByText(t("settings.dictation.credits"))).toBeInTheDocument();
    expect(within(light).getByText(t("dictation.engine.installed"))).toBeInTheDocument();
    expect(within(light).getByText(t("dictation.languages", { languages: "English" }))).toBeInTheDocument();
    expect(within(light).getByText(/Moonshine tiny by Useful Sensors, MIT licence/)).toBeInTheDocument();
    const ollama = engineRow(/Ollama/);
    expect(within(ollama).getByText(t("dictation.languages.model"))).toBeInTheDocument();
    expect(within(ollama).queryByText(t("settings.dictation.credits"))).toBeNull();
  });

  it("is drawn again, with the languages named, in the language the person switches to", async () => {
    const { user } = await openSettings({});
    await within(section()).findByRole("radio", { name: /Light/ });
    expect(within(section()).getByText(/^Languages: .*Bulgarian/)).toBeInTheDocument();
    const group = screen.getByRole("group", { name: t("settings.language.legend") });
    await user.click(within(group).getByRole("radio", { name: "Français" }));
    await screen.findByRole("heading", { level: 2, name: "Dictée" });
    // "Langues : bulgare, croate, ..." (a no-break space before the colon) and the sizes in Mo.
    expect(within(section()).getByText(/^Langues\s: .*bulgare/u)).toBeInTheDocument();
    expect(within(section()).getByText(/^Téléchargement\s: 671\sMo$/u)).toBeInTheDocument();
    expect(within(section()).getAllByText("Recommandé")).toHaveLength(1);
  });
});

describe("the choice of engine", () => {
  it("is the engine of this device, kept by the shell: clicking a radio chooses it", async () => {
    const { user, memory } = await openSettings({ installed: ["light"] });
    await within(section()).findByRole("radio", { name: /Light/ });
    await user.click(within(section()).getByRole("radio", { name: /Light/ }));
    await waitFor(() => {
      expect(within(section()).getByRole("radio", { name: /Light/ })).toBeChecked();
    });
    expect((await fakeOf(memory).status()).selected).toBe("light");
    // Any engine can be chosen, installed or not: the first use then offers the download.
    await user.click(within(section()).getByRole("radio", { name: /Accurate/ }));
    await waitFor(() => {
      expect(within(section()).getByRole("radio", { name: /Accurate/ })).toBeChecked();
    });
    expect((await fakeOf(memory).status()).selected).toBe("accurate");
  });

  it("shows the choice that was already made", async () => {
    await openSettings({ installed: ["accurate"], selected: "accurate" });
    await within(section()).findByRole("radio", { name: /Accurate/ });
    expect(within(section()).getByRole("radio", { name: /Accurate/ })).toBeChecked();
  });
});

describe("downloading a model", () => {
  it("shows the bar and Cancel while it runs, then Installed and Remove; the first model becomes the choice", async () => {
    const { user, memory } = await openSettings({});
    await within(section()).findByRole("radio", { name: /Light/ });
    const light = engineRow(/Light/);
    await user.click(button(light, t("settings.dictation.download")));
    const bar = await within(light).findByRole("progressbar", { name: t("dictation.download.progress") });
    expect(bar).toBeInTheDocument();
    await waitFor(() => {
      expect(within(light).getByRole("status")).toHaveTextContent(/%/);
    });
    expect(within(light).getByRole("button", { name: t("settings.dictation.cancel") })).toBeInTheDocument();
    // One download at a time: the other buttons wait.
    expect(button(engineRow(/Accurate/), t("settings.dictation.download"))).toBeDisabled();
    await within(light).findByText(t("dictation.download.verifying"));

    await waitFor(() => {
      expect(within(engineRow(/Light/)).getByText(t("dictation.engine.installed"))).toBeInTheDocument();
    });
    expect(within(engineRow(/Light/)).queryByRole("progressbar")).toBeNull();
    expect(button(engineRow(/Light/), t("settings.dictation.remove"))).toBeEnabled();
    expect(within(engineRow(/Light/)).queryByRole("button", { name: t("settings.dictation.download") })).toBeNull();
    expect(fakeOf(memory).installedEngines).toEqual(["light"]);
    // Nothing was chosen yet, so the model that was just asked for is.
    expect((await fakeOf(memory).status()).selected).toBe("light");
    expect(within(section()).getByRole("radio", { name: /Light/ })).toBeChecked();
  });

  it("keeps the choice that was made when another model arrives", async () => {
    const { user, memory } = await openSettings({ installed: ["light"], selected: "light" });
    await within(section()).findByRole("radio", { name: /Accurate/ });
    await user.click(button(engineRow(/Accurate/), t("settings.dictation.download")));
    await waitFor(() => {
      expect(within(engineRow(/Accurate/)).getByText(t("dictation.engine.installed"))).toBeInTheDocument();
    });
    expect((await fakeOf(memory).status()).selected).toBe("light");
  });

  it("goes back to Download, with nothing said, when it is cancelled", async () => {
    const { user, memory } = await openSettings({ installStepMs: 80 });
    await within(section()).findByRole("radio", { name: /Light/ });
    const light = engineRow(/Light/);
    await user.click(button(light, t("settings.dictation.download")));
    await user.click(await within(light).findByRole("button", { name: t("settings.dictation.cancel") }));
    await waitFor(() => {
      expect(within(engineRow(/Light/)).getByRole("button", { name: t("settings.dictation.download") })).toBeEnabled();
    });
    expect(within(section()).queryByRole("alert")).toBeNull();
    expect(fakeOf(memory).installedEngines).toEqual([]);
  });

  it("says nothing when the shell's own dialog was declined", async () => {
    const { user } = await openSettings({ consent: false });
    await within(section()).findByRole("radio", { name: /Light/ });
    await user.click(button(engineRow(/Light/), t("settings.dictation.download")));
    await waitFor(() => {
      expect(button(engineRow(/Light/), t("settings.dictation.download"))).toBeEnabled();
    });
    expect(within(section()).queryByRole("alert")).toBeNull();
    expect(within(engineRow(/Light/)).queryByRole("progressbar")).toBeNull();
  });

  it("says why it failed, in the person's language", async () => {
    const { user } = await openSettings({ failInstallWith: "hashMismatch" });
    await within(section()).findByRole("radio", { name: /Light/ });
    await user.click(button(engineRow(/Light/), t("settings.dictation.download")));
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("dictation.error.hashMismatch"));
    expect(button(engineRow(/Light/), t("settings.dictation.download"))).toBeEnabled();
  });
});

describe("model files and removal", () => {
  it("takes model files from a folder (the shell's dialog) and then offers Remove", async () => {
    const { user, memory } = await openSettings({});
    await within(section()).findByRole("radio", { name: /Light/ });
    await user.click(button(engineRow(/Light/), t("settings.dictation.useFiles")));
    await waitFor(() => {
      expect(within(engineRow(/Light/)).getByText(t("dictation.engine.installed"))).toBeInTheDocument();
    });
    expect(fakeOf(memory).installedEngines).toEqual(["light"]);
    expect(button(engineRow(/Light/), t("settings.dictation.remove"))).toBeInTheDocument();
  });

  it("removes a model and goes back to Download", async () => {
    const { user, memory } = await openSettings({ installed: ["light"], selected: "light" });
    await within(section()).findByRole("radio", { name: /Light/ });
    await user.click(button(engineRow(/Light/), t("settings.dictation.remove")));
    await waitFor(() => {
      expect(within(engineRow(/Light/)).getByText(t("dictation.engine.notInstalled"))).toBeInTheDocument();
    });
    expect(fakeOf(memory).installedEngines).toEqual([]);
    expect(button(engineRow(/Light/), t("settings.dictation.download"))).toBeEnabled();
    // The choice stays, as in the shell.
    expect((await fakeOf(memory).status()).selected).toBe("light");
  });

  it("says why a removal was refused", async () => {
    const { user, memory } = await openSettings({ installed: ["light"], selected: "light" });
    await within(section()).findByRole("radio", { name: /Light/ });
    vi.spyOn(fakeOf(memory), "remove").mockRejectedValue(
      new CoreError({ code: "InvalidInput", message: "a session uses this engine", details: { field: "engine", reason: "busy" } }),
    );
    await user.click(button(engineRow(/Light/), t("settings.dictation.remove")));
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("dictation.error.busy"));
    // The model is still there.
    expect(within(engineRow(/Light/)).getByText(t("dictation.engine.installed"))).toBeInTheDocument();
  });
});

describe("Ollama", () => {
  it.each([
    ["aiOff", "dictation.unavailable.aiOff"],
    ["noTranscriptionModel", "dictation.unavailable.noTranscriptionModel"],
    ["consentRequired", "dictation.unavailable.consentRequired"],
  ] as const)("is disabled when %s, says which step is missing and links to the AI section", async (reason, key) => {
    const { user } = await openSettings({ ollamaUnavailable: reason }, { ai: { mode: "fake" } });
    await within(section()).findByRole("radio", { name: /Ollama/ });
    const ollama = engineRow(/Ollama/);
    expect(within(ollama).getByRole("radio")).toBeDisabled();
    expect(within(ollama).getByText(t(key))).toBeInTheDocument();
    const link = within(ollama).getByRole("button", { name: t("dictation.openAiSettings") });
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    await user.click(link);
    await waitFor(() => {
      expect(scroll).toHaveBeenCalled();
    });
    // It scrolled the AI section into view.
    expect((scroll.mock.contexts[0] as Element).id).toBe("settings-ai");
  });

  it("can be chosen when AI transcription is set up, with nothing to download", async () => {
    const { user, memory } = await openSettings({ ollamaUnavailable: null });
    await within(section()).findByRole("radio", { name: /Ollama/ });
    const ollama = engineRow(/Ollama/);
    expect(within(ollama).getByRole("radio")).toBeEnabled();
    expect(within(ollama).getByText(t("dictation.engine.ollama.ready"))).toBeInTheDocument();
    expect(within(ollama).queryByRole("button")).toBeNull();
    await user.click(within(ollama).getByRole("radio"));
    await waitFor(() => {
      expect(within(engineRow(/Ollama/)).getByRole("radio")).toBeChecked();
    });
    expect((await fakeOf(memory).status()).selected).toBe("ollama");
  });
});

describe("the fake of the dev page", () => {
  it('starts with "fake": nothing installed, nothing chosen, and a download that works', async () => {
    const { user, memory } = await openSettings("fake");
    await within(section()).findByRole("radio", { name: /Light/ });
    expect(fakeOf(memory).installedEngines).toEqual([]);
    await user.click(button(engineRow(/Light/), t("settings.dictation.download")));
    await waitFor(
      () => {
        expect(fakeOf(memory).installedEngines).toEqual(["light"]);
      },
      { timeout: 3000 },
    );
  });
});
