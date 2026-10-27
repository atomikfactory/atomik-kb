// The controller of a dictation (ADR-0053, `docs/architecture/dictation.md` sections 5 and 6) against the fake dictation,
// a fake microphone and a schema-only editor: the order of the phases, the pushes (1,600 to 3,200 samples), what stops
// it (Esc, Stop, a page that turns read-only) and what cancels it (the editor goes, the person leaves the page, and the
// speech it hears counts as activity for the auto-lock), waiting for `ended`, the sentences
// of every error code, the permission hint, the first-use dialog's hand-over, and the recorder's exclusion.

import { DICTATION_ERROR_CODES, DICTATION_MAX_PUSH_SAMPLES, MemoryDictation } from "@atomik-kb/core-client";
import { act, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ACTIVITY_EVENT } from "../../activity";
import { resetLanguage, t } from "../../i18n";
import { fakeMicrophone, FakeMediaRecorder, installMicrophone, removeMicrophone } from "../ai/recorder-kit";
import { IDLE_DICTATION } from "./controller";
import type { DictationController } from "./controller";
import { MAX_PUSH_SAMPLES, MIN_PUSH_SAMPLES } from "./chunker";
import { bareController, bareEditor, blocksOf, fakePcm, kick, manual, placeCursor, readyDictation, watched } from "./dictation-test-kit";
import { ERROR_KEYS } from "./text";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  removeMicrophone();
  resetLanguage();
});

/** Starts `controller` and waits until its microphone runs. */
async function begin(controller: DictationController): Promise<void> {
  await act(async () => {
    await controller.start();
  });
}

const phase = (controller: DictationController) => controller.state.get().phase;

describe("starting", () => {
  it("asks for the microphone, starts the session of the chosen engine and goes starting, loading, listening", async () => {
    const phases: string[] = [];
    const dictation = readyDictation({ loadMs: 10 });
    const start = vi.spyOn(dictation, "start");
    const pcm = fakePcm();
    const editor = bareEditor("Hello");
    const controller = bareController(editor, dictation, pcm);
    controller.state.subscribe(() => {
      const current = phase(controller);
      if (phases.at(-1) !== current) {
        phases.push(current);
      }
    });
    await begin(controller);
    expect(pcm.capture.start).toHaveBeenCalledTimes(1);
    expect(start).toHaveBeenCalledWith("light", expect.any(Function));
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    expect(phases).toEqual(["starting", "loading", "listening"]);
    expect(controller.state.get().engine).toBe("light");
    await controller.stop();
    expect(phase(controller)).toBe("idle");
  });

  it("does nothing without a dictation, on a read-only page, or while the recorder runs", async () => {
    const pcm = fakePcm();
    const none = bareController(bareEditor("Hello"), null, pcm);
    await begin(none);
    expect(phase(none)).toBe("idle");

    const locked = bareEditor("Hello");
    locked.setEditable(false);
    const readOnly = bareController(locked, readyDictation(), pcm);
    await begin(readOnly);
    expect(phase(readOnly)).toBe("idle");

    const recording = bareController(bareEditor("Hello"), readyDictation(), pcm, { recording: () => true });
    await begin(recording);
    expect(phase(recording)).toBe("idle");
    expect(pcm.capture.start).not.toHaveBeenCalled();
  });

  it("says so when the page's browser has no microphone API", async () => {
    const editor = bareEditor("Hello");
    const controller = bareController(editor, readyDictation(), fakePcm(), { capture: undefined, env: () => null });
    await begin(controller);
    expect(phase(controller)).toBe("error");
    expect(controller.state.get().notice).toBe(t("recorder.unsupported"));
  });

  it("starts nothing twice", async () => {
    const dictation = readyDictation();
    const start = vi.spyOn(dictation, "start");
    const controller = bareController(bareEditor("Hello"), dictation, fakePcm());
    await Promise.all([begin(controller), begin(controller)]);
    expect(start).toHaveBeenCalledTimes(1);
    await controller.stop();
  });

  it("waits in `setup` for the first-use dialog when no engine is ready, and starts nothing", async () => {
    const dictation = readyDictation({ installed: [], selected: null });
    const start = vi.spyOn(dictation, "start");
    const pcm = fakePcm();
    const controller = bareController(bareEditor("Hello"), dictation, pcm);
    await begin(controller);
    expect(phase(controller)).toBe("setup");
    expect(controller.busy).toBe(true);
    expect(pcm.capture.start).not.toHaveBeenCalled();
    expect(start).not.toHaveBeenCalled();
    controller.dismissSetup();
    expect(phase(controller)).toBe("idle");

    // A chosen engine whose model is gone is the same: the dialog.
    const gone = readyDictation({ installed: [], selected: "accurate" });
    const second = bareController(bareEditor("Hello"), gone, pcm);
    await begin(second);
    expect(phase(second)).toBe("setup");
  });

  it("starts the session when the dialog hands over an engine that is installed", async () => {
    const dictation = readyDictation({ installed: ["accurate"], selected: null });
    const start = vi.spyOn(dictation, "start");
    const pcm = fakePcm();
    const controller = bareController(bareEditor("Hello"), dictation, pcm);
    await begin(controller);
    expect(phase(controller)).toBe("setup");
    await act(async () => {
      await controller.proceed("accurate");
    });
    expect(start).toHaveBeenCalledWith("accurate", expect.any(Function));
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    await controller.stop();
  });

  it("shows the hint about the permission window when the microphone does not answer for a while", async () => {
    const pcm = fakePcm();
    let answer: () => void = () => undefined;
    pcm.capture.start.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          answer = resolve;
        }),
    );
    const controller = bareController(bareEditor("Hello"), readyDictation(), pcm, { hintMs: 20 });
    await kick(controller);
    await vi.waitFor(() => {
      expect(controller.state.get().permissionHint).toBe(true);
    });
    expect(phase(controller)).toBe("starting");
    act(() => {
      answer();
    });
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    expect(controller.state.get().permissionHint).toBe(false);
    await controller.stop();
  });

  it("lets go of a microphone that answers after the person gave up", async () => {
    const pcm = fakePcm();
    let answer: () => void = () => undefined;
    pcm.capture.start.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          answer = resolve;
        }),
    );
    const dictation = readyDictation();
    const start = vi.spyOn(dictation, "start");
    const controller = bareController(bareEditor("Hello"), dictation, pcm);
    await kick(controller);
    await vi.waitFor(() => {
      expect(pcm.capture.start).toHaveBeenCalled();
    });
    await controller.stop(); // Stop while the request is open: nothing to finish
    expect(phase(controller)).toBe("idle");
    act(() => {
      answer();
    });
    expect(pcm.capture.cancel).toHaveBeenCalled();
    expect(start).not.toHaveBeenCalled();
    expect(phase(controller)).toBe("idle");
  });
});

describe("the audio", () => {
  it("is pushed in chunks of 1,600 to 3,200 samples, in order, and the rest goes with Stop", async () => {
    const { api, sessions } = watched(readyDictation({ samplesPerPhrase: 1_000_000 }));
    const pcm = fakePcm();
    const controller = bareController(bareEditor("Hello"), api, pcm);
    await begin(controller);
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    for (const size of [683, 683, 683, 4_096, 683, 2_048, 9_999]) {
      pcm.feed(size);
    }
    const seen = sessions[0];
    expect(seen?.pushes.length).toBeGreaterThan(3);
    for (const length of seen?.pushes ?? []) {
      expect(length).toBeGreaterThanOrEqual(MIN_PUSH_SAMPLES);
      expect(length).toBeLessThanOrEqual(MAX_PUSH_SAMPLES);
      expect(length).toBeLessThanOrEqual(DICTATION_MAX_PUSH_SAMPLES);
    }
    const fed = [683, 683, 683, 4_096, 683, 2_048, 9_999].reduce((sum, size) => sum + size, 0);
    const pushed = (seen?.pushes ?? []).reduce((sum, length) => sum + length, 0);
    expect(pushed).toBeLessThan(fed); // a little is still held
    await controller.stop();
    expect((seen?.pushes ?? []).reduce((sum, length) => sum + length, 0)).toBe(fed); // and goes with Stop
    expect(seen?.stops).toBe(1);
  });

  it("is kept while the session is being registered, and pushed first once it is", async () => {
    const { api, sessions } = watched(readyDictation());
    const pcm = fakePcm();
    // The microphone hands over samples as soon as it starts, before `start` of the session answers.
    pcm.capture.start.mockImplementation(() => {
      pcm.feed(3_200);
      return Promise.resolve();
    });
    const controller = bareController(bareEditor("Hello"), api, pcm);
    await begin(controller);
    expect(sessions[0]?.pushes).toEqual([3_200]);
    await controller.stop();
  });

  it("shows the level the microphone reports, and counts the seconds from its first sample", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
    const pcm = fakePcm();
    const controller = bareController(bareEditor("Hello"), readyDictation({ loadMs: 0 }), pcm);
    await begin(controller);
    await vi.advanceTimersByTimeAsync(10);
    pcm.feed(1_600);
    expect(controller.state.get().level).toBe(0.5);
    expect(controller.state.get().seconds).toBe(0);
    await vi.advanceTimersByTimeAsync(2_600);
    expect(controller.state.get().seconds).toBe(2);
    await controller.stop();
  });
});

describe("the phrases", () => {
  it("go into the page at the cursor, as the engine recognises them", async () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const pcm = fakePcm();
    const controller = bareController(editor, readyDictation({ phrases: ["one.", "two"] }), pcm);
    await begin(controller);
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    act(() => {
      pcm.feed(3_200);
    });
    expect(blocksOf(editor)).toEqual(["Hello one."]);
    expect(phase(controller)).toBe("listening");
    act(() => {
      pcm.feed(3_200);
    });
    expect(blocksOf(editor)).toEqual(["Hello one. two"]);
    await controller.stop();
  });

  it("report the speech that is heard as activity for the auto-lock timer, and an open microphone alone is none", async () => {
    const activity = vi.fn();
    window.addEventListener(ACTIVITY_EVENT, activity);
    try {
      const pcm = fakePcm();
      const { api, emit } = manual(await readyDictation().status());
      const controller = bareController(bareEditor("Hello"), api, pcm);
      await begin(controller);
      act(() => {
        emit({ type: "state", state: "loading" });
        emit({ type: "state", state: "listening" });
        pcm.feed(3_200); // the microphone is open and loud, nothing was recognised
      });
      expect(activity).not.toHaveBeenCalled();
      act(() => {
        emit({ type: "state", state: "recognising" });
      });
      expect(activity).toHaveBeenCalledTimes(1);
      act(() => {
        emit({ type: "phrase", text: "something said." });
      });
      expect(activity).toHaveBeenCalledTimes(2);
      act(() => {
        emit({ type: "state", state: "listening" });
      });
      expect(activity).toHaveBeenCalledTimes(2);
      const stopping = controller.stop();
      act(() => {
        emit({ type: "ended" });
      });
      await stopping;
    } finally {
      window.removeEventListener(ACTIVITY_EVENT, activity);
    }
  });
});

describe("stopping", () => {
  it("lets the microphone go at once, finishes with the engine, and waits for `ended` before the bar comes down", async () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const pcm = fakePcm();
    const { api, session, emit } = manual(await readyDictation().status());
    const controller = bareController(editor, api, pcm);
    await begin(controller);
    act(() => {
      emit({ type: "state", state: "listening" });
    });
    expect(phase(controller)).toBe("listening");
    pcm.feed(1_000); // less than a push: it is held

    let settled = false;
    const stopping = controller.stop().then(() => {
      settled = true;
    });
    await act(async () => {
      await Promise.resolve();
    });
    // Stop: the held audio went out, the microphone was let go, the engine was told to finish, and the bar says so.
    expect(phase(controller)).toBe("stopping");
    expect(pcm.capture.stop).toHaveBeenCalledTimes(1);
    expect(session.push).toHaveBeenCalledTimes(1);
    expect(session.push.mock.calls[0]?.[0]).toHaveLength(1_000);
    expect(session.stop).toHaveBeenCalledTimes(1);
    expect(session.cancel).not.toHaveBeenCalled();
    expect(settled).toBe(false);

    // The last phrases come before `ended`, and are put into the page; the bar's own state does not move.
    act(() => {
      emit({ type: "state", state: "recognising" });
      emit({ type: "phrase", text: "last words." });
    });
    expect(blocksOf(editor)).toEqual(["Hello last words."]);
    expect(phase(controller)).toBe("stopping");
    expect(settled).toBe(false);

    act(() => {
      emit({ type: "ended" });
    });
    await stopping;
    expect(settled).toBe(true);
    expect(controller.state.get()).toEqual(IDLE_DICTATION);
    // Stopping again is fine, and so is a second Stop while it is under way (the same wait).
    await controller.stop();
  });

  it("gives up waiting when `ended` never comes", async () => {
    const pcm = fakePcm();
    const { api, session } = manual(await readyDictation().status());
    const controller = bareController(bareEditor("Hello"), api, pcm, { stopWaitMs: 20 });
    await begin(controller);
    await controller.stop();
    expect(session.cancel).toHaveBeenCalled();
    expect(phase(controller)).toBe("idle");
  });

  it("stops on Esc, unless it came from a menu, a dialog or a text field outside the page", async () => {
    const editor = bareEditor("Hello");
    const pcm = fakePcm();
    const { api, sessions } = watched(readyDictation());
    const controller = bareController(editor, api, pcm);
    const detach = controller.attach();
    await begin(controller);
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });

    // The Esc of a menu, a dialog or a field of its own closes that, and the dictation goes on.
    const menu = document.createElement("div");
    menu.setAttribute("role", "listbox");
    const option = document.createElement("div");
    menu.append(option);
    const field = document.createElement("input");
    document.body.append(menu, field);
    fireEvent.keyDown(option, { key: "Escape" });
    fireEvent.keyDown(field, { key: "Escape" });
    expect(phase(controller)).toBe("listening");
    fireEvent.keyDown(window, { key: "a" });
    expect(phase(controller)).toBe("listening");
    menu.remove();
    field.remove();

    // From the page itself (its own keymaps take Esc too), or from nowhere in particular: stop.
    expect(phase(controller)).toBe("listening");

    fireEvent.keyDown(window, { key: "Escape" });
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("idle");
    });
    expect(sessions[0]?.stops).toBe(1);
    expect(sessions[0]?.cancels).toBe(0);
    expect(pcm.capture.stop).toHaveBeenCalled();
    detach();
  });

  it("stops when the page turns read-only, and puts nothing more into it", async () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const pcm = fakePcm();
    const { api, sessions } = watched(readyDictation());
    const controller = bareController(editor, api, pcm);
    const detach = controller.attach();
    await begin(controller);
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    pcm.feed(1_600); // half a phrase heard: the engine owes one when it is stopped
    act(() => {
      editor.setEditable(false);
    });
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("idle");
    });
    expect(sessions[0]?.stops).toBe(1);
    expect(blocksOf(editor)).toEqual(["Hello"]);
    detach();
  });

  it("cancels when the editor is destroyed: no last phrases, the microphone and the session let go", async () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const pcm = fakePcm();
    const { api, sessions } = watched(readyDictation());
    const controller = bareController(editor, api, pcm);
    const detach = controller.attach();
    await begin(controller);
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    pcm.feed(1_600);
    detach();
    editor.destroy();
    expect(phase(controller)).toBe("idle");
    expect(sessions[0]?.cancels).toBe(1);
    expect(sessions[0]?.stops).toBe(0);
    expect(pcm.capture.cancel).toHaveBeenCalled();
    // What the old run still delivers is dropped: more samples reach nobody.
    const pushed = sessions[0]?.pushes.length;
    pcm.feed(3_200);
    expect(sessions[0]?.pushes.length).toBe(pushed);
  });
});

describe("leaving the page", () => {
  it("drops everything: the session is cancelled, not stopped, and the phrase still being recognised goes nowhere", async () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const pcm = fakePcm();
    const { api, session, emit } = manual(await readyDictation().status());
    const controller = bareController(editor, api, pcm);
    expect(controller.leave()).toBe(false); // nothing runs: nothing can be lost
    await begin(controller);
    act(() => {
      emit({ type: "state", state: "listening" });
    });
    pcm.feed(1_600); // half a phrase heard
    expect(controller.leave()).toBe(true);
    expect(phase(controller)).toBe("idle");
    expect(session.cancel).toHaveBeenCalledTimes(1);
    expect(session.stop).not.toHaveBeenCalled();
    expect(pcm.capture.cancel).toHaveBeenCalled();
    // What the engine owed arrives too late for a page that is gone: it is dropped, and `ended` finds nothing to end.
    act(() => {
      emit({ type: "phrase", text: "late words." });
      emit({ type: "ended" });
    });
    expect(blocksOf(editor)).toEqual(["Hello"]);
    expect(controller.state.get()).toEqual(IDLE_DICTATION);
  });

  it("says that speech may be lost while the session finishes, and ends the wait of Stop", async () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const { api, session, emit } = manual(await readyDictation().status());
    const controller = bareController(editor, api, fakePcm());
    await begin(controller);
    const stopping = controller.stop();
    await act(async () => {
      await Promise.resolve();
    });
    expect(phase(controller)).toBe("stopping");
    expect(controller.leave()).toBe(true);
    await stopping;
    expect(session.cancel).toHaveBeenCalledTimes(1);
    act(() => {
      emit({ type: "phrase", text: "late words." });
    });
    expect(blocksOf(editor)).toEqual(["Hello"]);
  });
});

describe("failures", () => {
  it.each(DICTATION_ERROR_CODES.map((code) => [code] as const))(
    "says %s in the person's language when the session cannot start, and lets the microphone go",
    async (code) => {
      const pcm = fakePcm();
      const controller = bareController(bareEditor("Hello"), readyDictation({ failStartWith: code }), pcm);
      await begin(controller);
      expect(phase(controller)).toBe("error");
      const sentence = t(ERROR_KEYS[code]);
      expect(sentence).not.toBe(ERROR_KEYS[code]);
      expect(sentence.length).toBeGreaterThan(10);
      expect(controller.state.get().notice).toBe(sentence);
      expect(pcm.capture.cancel).toHaveBeenCalled();
      controller.dismiss();
      expect(phase(controller)).toBe("idle");
    },
  );

  it("has a sentence for every code of the shell", () => {
    expect(Object.keys(ERROR_KEYS).sort()).toEqual([...DICTATION_ERROR_CODES].sort());
  });

  it("shows the sentence of an engine error when `ended` follows it, and not before the bar is down", async () => {
    const pcm = fakePcm();
    const { api, emit } = manual(await readyDictation().status());
    const controller = bareController(bareEditor("Hello"), api, pcm);
    await begin(controller);
    act(() => {
      emit({ type: "error", code: "engineCrashed" });
    });
    // The error is on the bar at once; the session is over at `ended`.
    expect(controller.state.get().notice).toBe(t("dictation.error.engineCrashed"));
    expect(phase(controller)).toBe("loading");
    act(() => {
      emit({ type: "ended" });
    });
    expect(phase(controller)).toBe("error");
    expect(controller.state.get().notice).toBe(t("dictation.error.engineCrashed"));
    expect(pcm.capture.cancel).toHaveBeenCalled();
    // A new dictation can start over it.
    await begin(controller);
    expect(controller.state.get().notice).toBeNull();
    controller.cancel();
  });

  it("goes quietly idle when the session ends on its own with no error (a lock in the shell)", async () => {
    const pcm = fakePcm();
    const { api, emit } = manual(await readyDictation().status());
    const controller = bareController(bareEditor("Hello"), api, pcm);
    await begin(controller);
    act(() => {
      emit({ type: "ended" });
    });
    expect(controller.state.get()).toEqual(IDLE_DICTATION);
    expect(pcm.capture.cancel).toHaveBeenCalled();
  });

  it("says when the status of the engines cannot be read", async () => {
    const dictation = readyDictation();
    vi.spyOn(dictation, "status").mockRejectedValue(new Error("the shell is gone"));
    const controller = bareController(bareEditor("Hello"), dictation, fakePcm());
    await begin(controller);
    expect(phase(controller)).toBe("error");
    expect(controller.state.get().notice).toContain("the shell is gone");
  });
});

describe("with the real capture (a fake microphone and Web Audio)", () => {
  beforeEach(() => {
    FakeMediaRecorder.instances.length = 0;
  });

  it("records no file: no MediaRecorder is made, the same constraints as the recorder, and the audio reaches the session", async () => {
    const microphone = fakeMicrophone();
    installMicrophone(microphone);
    const { api, sessions } = watched(readyDictation({ samplesPerPhrase: 1_000_000 }));
    const controller = bareController(bareEditor("Hello"), api, fakePcm(), { capture: undefined });
    await begin(controller);
    await vi.waitFor(() => {
      expect(phase(controller)).toBe("listening");
    });
    expect(microphone.getUserMedia).toHaveBeenCalledWith({ audio: true });
    expect(FakeMediaRecorder.instances).toHaveLength(0);
    act(() => {
      microphone.feed(0.5);
    });
    expect((sessions[0]?.pushes ?? []).length).toBeGreaterThan(0);
    expect(controller.state.get().level).toBeGreaterThan(0);
    await controller.stop();
    // The microphone was let go: the tracks stopped.
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(FakeMediaRecorder.instances).toHaveLength(0);
  });
});

describe("the fake dictation it is tested against", () => {
  it("is the one of core-client", () => {
    expect(readyDictation()).toBeInstanceOf(MemoryDictation);
  });
});
