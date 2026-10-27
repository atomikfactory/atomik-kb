// `/microphone` in the `/` menu (ADR-0053, `docs/architecture/dictation.md` section 6): found first by /mic, /microphone and
// /dictate, hidden where the transport cannot dictate, unavailable on read-only pages, while dictation or a recording
// runs and where there is no microphone API, and exclusive with "Record audio".

import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { loadLanguage, resetLanguage, t } from "../../i18n";
import { stubObjectUrls } from "../../test/attachments";
import { editorAiOf } from "../ai/EditorAi";
import { resetAi } from "../ai/ai-test-kit";
import { fakeMicrophone, removeMicrophone } from "../ai/recorder-kit";
import { setPageLock } from "../page-lock";
import { filterSlashCommands } from "../slash-commands";
import { dictationOf, dictationOn, dictationRunning, dictationUnavailable } from "./registry";
import {
  barGone,
  controllerOf,
  findBar,
  openNotes,
  openSlashMenu,
  optionLabels,
  startAndListen,
} from "./dictation-test-kit";

beforeEach(() => {
  stubObjectUrls();
  HTMLMediaElement.prototype.pause = vi.fn() as () => void;
  HTMLMediaElement.prototype.load = vi.fn() as () => void;
});

afterEach(() => {
  resetAi();
  removeMicrophone();
  resetLanguage();
  vi.restoreAllMocks();
});

/** The option of the menu with that label. */
function optionOf(menu: HTMLElement, label: string): HTMLElement {
  const option = within(menu)
    .getAllByRole("option")
    .find((candidate) => candidate.querySelector(".suggestion-title")?.textContent === label);
  if (option === undefined) {
    throw new Error(`no option "${label}"`);
  }
  return option;
}

describe("the ranking", () => {
  it("puts Microphone first for /mic, /microphone and /dictate, in the three languages, above Record audio", async () => {
    for (const language of ["en", "fr", "es"] as const) {
      await loadLanguage(language);
      for (const query of ["mic", "microphone", "dictate"]) {
        expect(filterSlashCommands(query).map((command) => command.id).slice(0, 2), `${language} /${query}`).toEqual([
          "microphone",
          "record",
        ]);
      }
    }
  });
});

describe("the entry", () => {
  it("is in the menu, first for /mic, when the transport can dictate", async () => {
    const { editor } = await openNotes({ texts: ["Hello", ""] });
    expect(dictationOn(editor)).toBe(true);
    act(() => {
      editor.commands.focus("end");
    });
    const menu = await openSlashMenu(editor, "mic");
    await waitFor(() => {
      expect(optionLabels(menu)[0]).toBe(t("slash.microphone"));
    });
    expect(optionOf(menu, t("slash.microphone")).textContent).toContain(t("slash.microphone.hint"));
    expect(optionOf(menu, t("slash.microphone"))).not.toHaveAttribute("aria-disabled", "true");
  });

  it("is left out of the menu, and has no dictation, where the transport cannot dictate", async () => {
    const { editor } = await openNotes({ dictation: null, texts: ["Hello", ""] });
    expect(dictationOn(editor)).toBe(false);
    expect(dictationOf(editor)).toBeNull();
    act(() => {
      editor.commands.focus("end");
    });
    act(() => {
      editor.commands.insertContent("/mic");
    });
    // Nothing else is found by those letters, so the menu does not even open.
    await new Promise((resolve) => setTimeout(resolve, 50));
    const menu = screen.queryByRole("listbox", { name: t("slash.label") });
    expect(menu === null || !optionLabels(menu).includes(t("slash.microphone"))).toBe(true);
  });

  it("is shown and says why it cannot run where there is no microphone API", async () => {
    const { editor } = await openNotes({ texts: ["Hello", ""] });
    removeMicrophone();
    act(() => {
      editor.commands.focus("end");
    });
    const menu = await openSlashMenu(editor, "mic");
    const option = optionOf(menu, t("slash.microphone"));
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(option).toHaveTextContent(t("slash.microphone.unsupported"));
    const before = JSON.stringify(editor.getJSON());
    fireEvent.click(option);
    expect(JSON.stringify(editor.getJSON())).toBe(before);
    expect(controllerOf(editor).state.get().phase).toBe("idle");
  });

  it("is unavailable on a read-only page", async () => {
    const { editor } = await openNotes({ texts: ["Hello", ""] });
    expect(dictationUnavailable(editor)).toBeNull();
    act(() => {
      setPageLock(editor, true);
    });
    expect(editor.isEditable).toBe(false);
    expect(dictationUnavailable(editor)).toBe("slash.microphone.readOnly");
    // Even asked to, the controller starts nothing on a page it cannot write.
    await act(async () => {
      await controllerOf(editor).start();
    });
    expect(controllerOf(editor).state.get().phase).toBe("idle");
    expect(screen.queryByRole("region", { name: t("dictation.label") })).toBeNull();
  });

  it("runs from the menu: the typed command goes and the bar comes up with the microphone on", async () => {
    const { editor, microphone, dictation } = await openNotes({ texts: ["Hello", ""] });
    act(() => {
      editor.commands.focus("end");
    });
    const menu = await openSlashMenu(editor, "mic");
    await waitFor(() => {
      expect(optionLabels(menu)[0]).toBe(t("slash.microphone"));
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    const bar = await findBar();
    await waitFor(() => {
      expect(within(bar).getByText(t("dictation.listening"))).toBeInTheDocument();
    });
    expect(editor.state.doc.textContent).toBe("Hello");
    expect(microphone.getUserMedia).toHaveBeenCalledWith({ audio: true });
    expect(dictation().runningSession).toBeDefined();
    await act(async () => {
      await controllerOf(editor).stop();
    });
    await barGone();
  });
});

describe("the exclusion with the recorder", () => {
  it("makes Record audio unavailable while dictation runs, and the recorder does not start", async () => {
    const { editor } = await openNotes({ ai: true, texts: ["Hello", ""] });
    const controller = await startAndListen(editor);
    expect(dictationRunning(editor)).toBe(true);
    const recorder = editorAiOf(editor)?.recorder;
    await act(async () => {
      await recorder?.start();
    });
    expect(recorder?.state.get().phase).toBe("idle");

    act(() => {
      editor.commands.focus("end");
    });
    const menu = await openSlashMenu(editor, "mic"); // Microphone, and Record audio by its keyword
    const option = optionOf(menu, t("slash.record"));
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(option).toHaveTextContent(t("slash.record.dictating"));
    // And Microphone itself says dictation is already running.
    expect(optionOf(menu, t("slash.microphone"))).toHaveTextContent(t("slash.microphone.busy"));

    await act(async () => {
      await controller.stop();
    });
    await barGone();
    expect(dictationRunning(editor)).toBe(false);
  });

  it("makes Microphone unavailable while a recording runs, and dictation does not start", async () => {
    const { editor } = await openNotes({ ai: true, texts: ["Hello", ""] });
    const recorder = editorAiOf(editor)?.recorder;
    await act(async () => {
      await recorder?.start();
    });
    await waitFor(() => {
      expect(recorder?.state.get().phase).toBe("recording");
    });
    expect(dictationUnavailable(editor)).toBe("slash.microphone.recording");
    await act(async () => {
      await controllerOf(editor).start();
    });
    expect(controllerOf(editor).state.get().phase).toBe("idle");
    expect(screen.queryByRole("region", { name: t("dictation.label") })).toBeNull();

    act(() => {
      recorder?.cancel();
    });
    expect(dictationUnavailable(editor)).toBeNull();
  });

  it("lets the recorder start again once dictation ended", async () => {
    const { editor } = await openNotes({ ai: true, texts: ["Hello", ""] });
    const controller = await startAndListen(editor);
    await act(async () => {
      await controller.stop();
    });
    await barGone();
    const recorder = editorAiOf(editor)?.recorder;
    await act(async () => {
      await recorder?.start();
    });
    await waitFor(() => {
      expect(recorder?.state.get().phase).toBe("recording");
    });
    act(() => {
      recorder?.cancel();
    });
  });
});

describe("the microphone permission", () => {
  it("is asked for before the session starts: a denied microphone loads no model and the bar says so", async () => {
    const microphone = fakeMicrophone({ name: "NotAllowedError" });
    const { editor, dictation } = await openNotes({ texts: ["Hello", ""], microphone });
    const start = vi.spyOn(dictation(), "start");
    await act(async () => {
      await controllerOf(editor).start();
    });
    const bar = await findBar();
    expect(within(bar).getByRole("alert")).toHaveTextContent(t("recorder.denied"));
    expect(start).not.toHaveBeenCalled();
    expect(controllerOf(editor).state.get().phase).toBe("error");
    // Dismissing clears the bar and dictation can be tried again.
    fireEvent.click(within(bar).getByRole("button", { name: t("dictation.dismiss") }));
    await barGone();
    expect(controllerOf(editor).state.get().phase).toBe("idle");
  });

  it("says when there is no microphone", async () => {
    const microphone = fakeMicrophone({ name: "NotFoundError" });
    const { editor } = await openNotes({ texts: ["Hello", ""], microphone });
    await act(async () => {
      await controllerOf(editor).start();
    });
    expect(within(await findBar()).getByRole("alert")).toHaveTextContent(t("recorder.noDevice"));
  });
});
