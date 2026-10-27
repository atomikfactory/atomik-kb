// The dictation bar and what ends a dictation in the app (ADR-0053, `docs/architecture/dictation.md` section 6): the level,
// the timer, the state text, the engine's name and Stop; Esc, Stop and a page that turns read-only stop it, a page or tab
// change and a lock drop it (the page's editor goes, so a phrase still being recognised has nowhere to go; a page or tab
// change says so in a toast), and a destroyed editor cancels it.

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { resetLanguage, t } from "../../i18n";
import { row } from "../../test/sidebar";
import { CaptureError } from "../ai/capture";
import { removeMicrophone } from "../ai/recorder-kit";
import { setPageLock } from "../page-lock";
import { DictationBar } from "./DictationBar";
import { leftThePage } from "./EditorDictation";
import {
  bareController,
  bareEditor,
  barGone,
  blocksOf,
  controllerOf,
  fakePcm,
  findBar,
  kick,
  manual,
  openNotes,
  queryBar,
  readyDictation,
  speak,
  spyOnSessions,
  startAndListen,
} from "./dictation-test-kit";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  removeMicrophone();
  resetLanguage();
});

describe("the bar", () => {
  it("shows nothing while idle, then the state of the engine, its name, the timer, the level and Stop", async () => {
    const pcm = fakePcm();
    const { api, emit } = manual(await readyDictation().status());
    const editor = bareEditor("Hello");
    const controller = bareController(editor, api, pcm);
    render(<DictationBar controller={controller} />);
    expect(queryBar()).toBeNull();

    await act(async () => {
      await controller.start();
    });
    const bar = await findBar();
    // The microphone runs and the engine loads: the timer and the meter are there from the first sample.
    expect(within(bar).getByText(t("dictation.loading"))).toBeInTheDocument();
    expect(within(bar).getByRole("timer", { name: t("dictation.time") })).toHaveTextContent("00:00");
    expect(within(bar).getByRole("meter", { name: t("dictation.level") })).toHaveAttribute("aria-valuenow", "0");
    expect(within(bar).getByText(t("dictation.engine.light"))).toBeInTheDocument();
    expect(within(bar).getByRole("button", { name: t("dictation.stop") })).toBeEnabled();

    act(() => {
      pcm.feed(1_600);
    });
    expect(within(bar).getByRole("meter")).toHaveAttribute("aria-valuenow", "50");

    act(() => {
      emit({ type: "state", state: "listening" });
    });
    expect(within(bar).getByText(t("dictation.listening"))).toBeInTheDocument();
    act(() => {
      emit({ type: "state", state: "recognising" });
    });
    expect(within(bar).getByText(t("dictation.recognising"))).toBeInTheDocument();
    act(() => {
      emit({ type: "state", state: "listening" });
    });

    // Stop: "Finishing…", Stop disabled, the level at zero; `ended` takes the bar away.
    await act(async () => {
      fireEvent.click(within(bar).getByRole("button", { name: t("dictation.stop") }));
      await Promise.resolve();
    });
    expect(within(bar).getByText(t("dictation.stopping"))).toBeInTheDocument();
    expect(within(bar).getByRole("button", { name: t("dictation.stop") })).toBeDisabled();
    expect(within(bar).getByRole("meter")).toHaveAttribute("aria-valuenow", "0");
    act(() => {
      emit({ type: "ended" });
    });
    await barGone();
  });

  it("says that the microphone is being asked for, with the hint about the permission window after a while", async () => {
    const pcm = fakePcm();
    pcm.capture.start.mockImplementation(() => new Promise<void>(() => undefined));
    const controller = bareController(bareEditor("Hello"), readyDictation(), pcm, { hintMs: 150 });
    render(<DictationBar controller={controller} />);
    await kick(controller);
    const bar = await findBar();
    expect(within(bar).getByText(t("dictation.starting"))).toBeInTheDocument();
    expect(within(bar).queryByRole("timer")).toBeNull(); // nothing has been heard yet
    expect(within(bar).queryByText(t("recorder.permissionHint"))).toBeNull();
    expect(await within(bar).findByText(t("recorder.permissionHint"))).toBeInTheDocument();
    // Stop gives up on the request.
    fireEvent.click(within(bar).getByRole("button", { name: t("dictation.stop") }));
    await barGone();
  });

  it("shows a denied microphone with the recorder's text until it is dismissed", async () => {
    const pcm = fakePcm();
    pcm.capture.start.mockRejectedValue(new CaptureError("denied", "Permission denied"));
    const controller = bareController(bareEditor("Hello"), readyDictation(), pcm);
    render(<DictationBar controller={controller} />);
    await act(async () => {
      await controller.start();
    });
    const bar = await findBar();
    expect(within(bar).getByRole("alert")).toHaveTextContent(t("recorder.denied"));
    expect(within(bar).queryByRole("button", { name: t("dictation.stop") })).toBeNull();
    fireEvent.click(within(bar).getByRole("button", { name: t("dictation.dismiss") }));
    await barGone();
  });
});

describe("in the page", () => {
  it("stops with Stop: the phrase the engine owed goes in, the microphone is let go and the bar comes down", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    await startAndListen(editor);
    speak(microphone, 0.1); // half a phrase: the engine finishes it when stopped
    const bar = await findBar();
    expect(blocksOf(editor)).toEqual(["Hello"]);
    fireEvent.click(within(bar).getByRole("button", { name: t("dictation.stop") }));
    await barGone();
    expect(blocksOf(editor)).toEqual(["Hello first phrase."]);
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(controllerOf(editor).state.get().phase).toBe("idle");
  });

  it("stops on Esc", async () => {
    const { editor, microphone, dictation } = await openNotes({ texts: ["Hello"] });
    const sessions = spyOnSessions(dictation());
    await startAndListen(editor);
    await findBar();
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await barGone();
    expect(sessions[0]?.stop).toHaveBeenCalledTimes(1);
    expect(sessions[0]?.cancel).not.toHaveBeenCalled();
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
  });

  it("is dropped when another page comes to the front, with a notice, and the phrase owed goes nowhere", async () => {
    const { editor, microphone, dictation, user } = await openNotes({ texts: ["Hello"] });
    const sessions = spyOnSessions(dictation());
    await startAndListen(editor);
    speak(microphone, 0.1); // half a phrase: the engine would owe one if it were stopped
    expect(screen.queryByText(t("dictation.leftPage"))).toBeNull();
    await user.click(row("Other"));
    await barGone();
    // The editor goes with the page, so the session is cancelled at once, not finished.
    expect(sessions[0]?.cancel).toHaveBeenCalledTimes(1);
    expect(sessions[0]?.stop).not.toHaveBeenCalled();
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(dictation().runningSession).toBeUndefined();
    expect(editor.isDestroyed).toBe(true);
    // The person is told that the rest was dropped, and nothing of it landed in the page in front.
    expect(await screen.findByText(t("dictation.leftPage"))).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: t("dictation.label") })).toBeNull();
    expect(document.body.textContent).not.toContain("First phrase");
    expect(document.body.textContent).toContain("Elsewhere");
  });

  it("says nothing when the person leaves a page where no dictation runs", async () => {
    const { user } = await openNotes({ texts: ["Hello"] });
    await user.click(row("Other"));
    await screen.findByText("Elsewhere");
    expect(screen.queryByText(t("dictation.leftPage"))).toBeNull();
  });

  it("stops when the page is locked", async () => {
    const { editor } = await openNotes({ texts: ["Hello"] });
    const controller = await startAndListen(editor);
    act(() => {
      setPageLock(editor, true);
    });
    await barGone();
    expect(controller.state.get().phase).toBe("idle");
  });

  it("cancels when the editor is destroyed: the session is cancelled, not stopped, and the microphone let go", async () => {
    const { editor, microphone, dictation, unmount } = await openNotes({ texts: ["Hello"] });
    const sessions = spyOnSessions(dictation());
    await startAndListen(editor);
    unmount();
    await waitFor(() => {
      expect(editor.isDestroyed).toBe(true);
    });
    expect(sessions[0]?.cancel).toHaveBeenCalled();
    expect(sessions[0]?.stop).not.toHaveBeenCalled();
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(dictation().runningSession).toBeUndefined();
  });
});

describe("what takes the person away", () => {
  const where = { selectedPageId: "p1", activeTabId: "t1", phase: "ready" } as const;

  it("is another page in front, another tab in front, and the app no longer open (a lock)", () => {
    expect(leftThePage(where, where)).toBe(false);
    expect(leftThePage({ ...where, selectedPageId: "p2" }, where)).toBe(true);
    expect(leftThePage({ ...where, selectedPageId: null }, where)).toBe(true);
    expect(leftThePage({ ...where, activeTabId: "t2" }, where)).toBe(true);
    expect(leftThePage({ ...where, phase: "locked" }, where)).toBe(true);
    expect(leftThePage({ ...where, phase: "loading" }, where)).toBe(true);
    expect(leftThePage({ ...where, phase: "fatal" }, where)).toBe(true);
  });

  it("is not the app becoming ready again, which is what an unlock ends with", () => {
    expect(leftThePage(where, { ...where, phase: "locked" })).toBe(false);
  });
});
