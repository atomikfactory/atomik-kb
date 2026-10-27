// The first-use dialog of dictation (ADR-0053, `docs/architecture/dictation.md` section 6): `/microphone` with no engine
// chosen, or one that is not installed, lists the engines as Settings does ("Accurate" first and recommended, with size,
// languages and, for Ollama, why it cannot be used), and the button chooses the engine, downloads its model with
// progress and "Verifying…", and starts dictating. A declined consent returns to the list quietly.

import type { MemoryDictationOptions } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { resetLanguage, t } from "../../i18n";
import { loadLanguage } from "../../i18n";
import { removeMicrophone } from "../ai/recorder-kit";
import { controllerOf, findBar, kick, openNotes } from "./dictation-test-kit";
import { formatSize, languageList } from "./text";

afterEach(() => {
  vi.restoreAllMocks();
  removeMicrophone();
  resetLanguage();
});

/** Nothing installed or chosen yet; the downloads take a moment (10 steps), and Ollama is off. */
const FIRST_USE: MemoryDictationOptions = {
  installed: [],
  selected: null,
  loadMs: 0,
  samplesPerPhrase: 3_200,
  installStepMs: 25,
  ollamaUnavailable: "aiOff",
};

/** Opens a page, runs `/microphone` and waits for the dialog. */
async function openDialog(dictation: MemoryDictationOptions = FIRST_USE) {
  const app = await openNotes({ dictation, texts: ["Hello"] });
  void controllerOf(app.editor).start();
  const dialog = await screen.findByRole("dialog", { name: t("dictation.setup.title") });
  await within(dialog).findByRole("radio", { name: /Light/ });
  return { ...app, dialog };
}

const radio = (dialog: HTMLElement, name: RegExp) => within(dialog).getByRole("radio", { name });
const go = (dialog: HTMLElement, name: string) => within(dialog).getByRole("button", { name });

describe("the dialog", () => {
  it("opens when no engine is chosen, without asking for the microphone, listing the engines as Settings does", async () => {
    const { dialog, microphone, editor } = await openDialog();
    expect(microphone.getUserMedia).not.toHaveBeenCalled();
    expect(editor.state.doc.textContent).toBe("Hello");

    const radios = within(dialog).getAllByRole("radio");
    expect(radios.map((item) => item.getAttribute("value"))).toEqual(["accurate", "light", "ollama"]);
    // Accurate first, recommended, and chosen to begin with.
    expect(radios[0]).toBeChecked();
    expect(within(dialog).getByText(t("dictation.recommended"))).toBeInTheDocument();
    expect(within(dialog).getAllByText(t("dictation.recommended"))).toHaveLength(1);

    // Sizes in MB, the languages in the active language, what each is.
    const accurate = radios[0]?.closest("li") as HTMLElement;
    const light = radios[1]?.closest("li") as HTMLElement;
    expect(within(accurate).getByText(t("dictation.size", { size: formatSize(670_478_772) }))).toBeInTheDocument();
    expect(formatSize(670_478_772)).toBe("671 MB");
    expect(within(light).getByText(t("dictation.size", { size: "124 MB" }))).toBeInTheDocument();
    expect(within(accurate).getByText(/^Languages: .*Bulgarian.*Spanish.*Ukrainian/)).toBeInTheDocument();
    expect(within(light).getByText(t("dictation.languages", { languages: "English" }))).toBeInTheDocument();
    expect(within(accurate).getByText(t("dictation.engine.accurate.about"))).toBeInTheDocument();
    expect(within(accurate).getByText(t("dictation.engine.notInstalled"))).toBeInTheDocument();

    // Ollama cannot be chosen, and says why, with the way to the AI settings.
    const ollama = radios[2];
    expect(ollama).toBeDisabled();
    const ollamaRow = ollama?.closest("li") as HTMLElement;
    expect(within(ollamaRow).getByText(t("dictation.unavailable.aiOff"))).toBeInTheDocument();
    expect(within(ollamaRow).getByText(t("dictation.languages.model"))).toBeInTheDocument();
    expect(within(ollamaRow).getByRole("button", { name: t("dictation.openAiSettings") })).toBeInTheDocument();

    // Choosing a downloadable engine shows what the button downloads.
    expect(go(dialog, t("dictation.setup.download", { size: "671 MB" }))).toBeEnabled();
    fireEvent.click(radio(dialog, /Light/));
    expect(go(dialog, t("dictation.setup.download", { size: "124 MB" }))).toBeEnabled();
    expect(radio(dialog, /Light/)).toBeChecked();
  });

  it("starts at the engine that is installed when nothing is chosen, and downloads nothing", async () => {
    const { dialog, dictation, microphone } = await openDialog({ ...FIRST_USE, installed: ["light"] });
    expect(radio(dialog, /Light/)).toBeChecked();
    expect(within(dialog).getByText(t("dictation.engine.installed"))).toBeInTheDocument();
    const install = vi.spyOn(dictation(), "install");
    fireEvent.click(go(dialog, t("dictation.setup.start")));
    await findBar();
    await waitFor(() => {
      expect(screen.getByText(t("dictation.listening"))).toBeInTheDocument();
    });
    expect(install).not.toHaveBeenCalled();
    expect((await dictation().status()).selected).toBe("light");
    expect(microphone.getUserMedia).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("downloads the chosen model with progress and Verifying…, chooses it, and starts dictating when it is in", async () => {
    const { dialog, dictation, microphone, editor } = await openDialog();
    fireEvent.click(radio(dialog, /Light/));
    fireEvent.click(go(dialog, t("dictation.setup.download", { size: "124 MB" })));

    // The bar of progress, the percentage, and no way to change the choice while it runs.
    const bar = await within(dialog).findByRole("progressbar", { name: t("dictation.download.progress") });
    await waitFor(() => {
      expect(within(dialog).getByRole("status")).toHaveTextContent(/%/);
    });
    expect(Number((bar as HTMLProgressElement).value)).toBeGreaterThan(0);
    expect(radio(dialog, /Accurate/)).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("dictation.setup.cancelDownload") })).toBeInTheDocument();
    await within(dialog).findByText(t("dictation.download.verifying"));

    // Then the dialog goes and the session starts, with Light chosen on this device.
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    const region = await findBar();
    await waitFor(() => {
      expect(within(region).getByText(t("dictation.listening"))).toBeInTheDocument();
    });
    expect(within(region).getByText(t("dictation.engine.light"))).toBeInTheDocument();
    expect(dictation().installedEngines).toEqual(["light"]);
    expect((await dictation().status()).selected).toBe("light");
    expect(microphone.getUserMedia).toHaveBeenCalledTimes(1);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("returns to the list with nothing said when the download is declined in the shell's own dialog", async () => {
    const { dialog, dictation } = await openDialog({ ...FIRST_USE, consent: false });
    fireEvent.click(radio(dialog, /Light/));
    fireEvent.click(go(dialog, t("dictation.setup.download", { size: "124 MB" })));
    await waitFor(() => {
      expect(radio(dialog, /Light/)).toBeEnabled();
    });
    // No error, no progress, the dialog is still there with the choice kept, and nothing was downloaded.
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(within(dialog).queryByRole("progressbar")).toBeNull();
    expect(radio(dialog, /Light/)).toBeChecked();
    expect(go(dialog, t("dictation.setup.download", { size: "124 MB" }))).toBeEnabled();
    expect(dictation().installedEngines).toEqual([]);
  });

  it("says why when the download fails, and stays on the list", async () => {
    const { dialog, dictation } = await openDialog({ ...FIRST_USE, failInstallWith: "downloadFailed" });
    fireEvent.click(radio(dialog, /Light/));
    fireEvent.click(go(dialog, t("dictation.setup.download", { size: "124 MB" })));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("dictation.error.downloadFailed"));
    expect(within(dialog).queryByRole("progressbar")).toBeNull();
    expect(radio(dialog, /Light/)).toBeEnabled();
    expect(dictation().installedEngines).toEqual([]);
  });

  it("goes back to the list, quietly, when the download is cancelled with its button", async () => {
    const { dialog, dictation } = await openDialog({ ...FIRST_USE, installStepMs: 60 });
    fireEvent.click(radio(dialog, /Light/));
    fireEvent.click(go(dialog, t("dictation.setup.download", { size: "124 MB" })));
    fireEvent.click(await within(dialog).findByRole("button", { name: t("dictation.setup.cancelDownload") }));
    await waitFor(() => {
      expect(within(dialog).queryByRole("progressbar")).toBeNull();
    });
    expect(within(dialog).queryByRole("alert")).toBeNull();
    expect(radio(dialog, /Light/)).toBeEnabled();
    expect(dictation().installedEngines).toEqual([]);
    expect(screen.queryByRole("region", { name: t("dictation.label") })).toBeNull();
  });

  it("closes with Cancel and with Esc, and dictation is idle again", async () => {
    const first = await openDialog();
    fireEvent.click(go(first.dialog, t("dialog.cancel")));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(controllerOf(first.editor).state.get().phase).toBe("idle");
    expect(first.microphone.getUserMedia).not.toHaveBeenCalled();

    await act(async () => {
      void controllerOf(first.editor).start();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    const dialog = await screen.findByRole("dialog", { name: t("dictation.setup.title") });
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(controllerOf(first.editor).state.get().phase).toBe("idle");
  });

  it("offers Ollama when it is set up, and starts with it with nothing to download", async () => {
    const { dialog, dictation, editor } = await openDialog({ ...FIRST_USE, ollamaUnavailable: null });
    const ollama = radio(dialog, /Ollama/);
    expect(ollama).toBeEnabled();
    expect(within(ollama.closest("li") as HTMLElement).getByText(t("dictation.engine.ollama.ready"))).toBeInTheDocument();
    expect(within(ollama.closest("li") as HTMLElement).queryByRole("button")).toBeNull();
    fireEvent.click(ollama);
    const install = vi.spyOn(dictation(), "install");
    fireEvent.click(go(dialog, t("dictation.setup.start")));
    await waitFor(() => {
      expect(controllerOf(editor).state.get().phase).toBe("listening");
    });
    expect(install).not.toHaveBeenCalled();
    expect(controllerOf(editor).state.get().engine).toBe("ollama");
    expect((await dictation().status()).selected).toBe("ollama");
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("goes to the AI settings from Ollama's reason, leaving the dialog", async () => {
    const app = await openNotes({ dictation: FIRST_USE, texts: ["Hello"], ai: true });
    void controllerOf(app.editor).start();
    const dialog = await screen.findByRole("dialog", { name: t("dictation.setup.title") });
    fireEvent.click(await within(dialog).findByRole("button", { name: t("dictation.openAiSettings") }));
    await screen.findByRole("heading", { name: t("settings.title") });
    expect(screen.queryByRole("dialog")).toBeNull();
    // The page is gone with the dialog; nothing was started.
    expect(app.dictation().runningSession).toBeUndefined();
    expect(app.microphone.getUserMedia).not.toHaveBeenCalled();
  });

  it("starts at once when the chosen engine is installed: no dialog", async () => {
    const { editor } = await openNotes({ dictation: { ...FIRST_USE, installed: ["light"], selected: "light" }, texts: ["Hello"] });
    const controller = controllerOf(editor);
    await kick(controller);
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => {
      expect(controller.state.get().phase).toBe("listening");
    });
    await act(async () => {
      await controller.stop();
    });
  });
});

describe("the figures it shows", () => {
  it("name the languages in the language of the app, by Intl.DisplayNames", async () => {
    expect(languageList(["fr", "en", "es"])).toBe("English, French, and Spanish");
    await loadLanguage("fr");
    expect(languageList(["fr", "en", "es"])).toBe("anglais, espagnol et français");
    expect(formatSize(670_478_772)).toMatch(/^671\s*Mo$/u); // a narrow no-break space in between
    await loadLanguage("es");
    expect(languageList(["fr", "en", "es"])).toBe("español, francés e inglés");
    expect(formatSize(123_967_539)).toBe("124 MB");
  });
});
