// Helpers of the tests of dictation (ADR-0053): the app over the in-memory core with the fake dictation and a fake
// microphone, a page to dictate into, and a schema-only editor with a controller of its own for the cases that need no app.

import { MemoryDictation } from "@atomik-kb/core-client";
import type {
  DictationApi,
  DictationEvent,
  DictationSession,
  DictationStatus,
  MemoryDictationOptions,
  MemoryTransport,
  SeedSpace,
} from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, screen, waitFor, within } from "@testing-library/react";
import { expect, vi } from "vitest";
import type { MockInstance } from "vitest";

import { t } from "../../i18n";
import { paragraphs } from "../../test/editor";
import { openPage, renderPages } from "../../test/page";
import { renderAiPages } from "../ai/ai-test-kit";
import { fakeMicrophone, installMicrophone } from "../ai/recorder-kit";
import type { FakeMicrophone } from "../ai/recorder-kit";
import { createSchemaExtensions } from "../schema";
import type { CaptureEvents, PcmCapture } from "../ai/capture";
import { DictationController } from "./controller";
import type { DictationDeps } from "./controller";
import { dictationOf } from "./registry";

/** What the fake dictation starts with in most tests: Light installed and chosen, no loading time, a phrase per 0.2 s of audio. */
export const READY: MemoryDictationOptions = {
  installed: ["light"],
  selected: "light",
  loadMs: 0,
  samplesPerPhrase: 3_200,
  phrases: ["First phrase.", "and second", "Third."],
};

/** A space with the pages of `texts` as paragraphs of "Notes", and "Other" to leave for. */
export const notesSpace = (...texts: string[]): SeedSpace[] => [
  { name: "Personal", pages: [{ title: "Notes", content: paragraphs(...texts) }, { title: "Other", content: paragraphs("Elsewhere") }] },
];

/** Puts the cursor at `pos` without a selection. */
export function placeCursor(editor: Editor, pos: number): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
  });
}

/** Selects `from` to `to`. */
export function selectRange(editor: Editor, from: number, to: number): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, from, to)));
  });
}

/** The controller of the editor; fails the test when there is none. */
export function controllerOf(editor: Editor): DictationController {
  const controller = dictationOf(editor);
  if (controller === null) {
    throw new Error("the editor has no dictation");
  }
  return controller;
}

/** A page of the app, open, with the cursor at the end of its last paragraph, and a fake microphone installed. */
export async function openNotes(
  options: { dictation?: MemoryDictationOptions | "fake" | null; texts?: string[]; ai?: boolean; microphone?: FakeMicrophone } = {},
) {
  const microphone = options.microphone ?? fakeMicrophone();
  installMicrophone(microphone);
  const dictation = options.dictation === null ? {} : { dictation: options.dictation ?? READY };
  const spaces = notesSpace(...(options.texts ?? ["Hello"]));
  const app = options.ai === true ? await renderAiPages(spaces, dictation) : await renderPages(spaces, dictation);
  const editor = await openPage(app.user, "Notes");
  placeCursor(editor, editor.state.doc.content.size - 1);
  return { ...app, editor, microphone, dictation: () => fakeOf(app.memory) };
}

/** The fake dictation of the transport; fails the test when it has none. */
export function fakeOf(memory: MemoryTransport): MemoryDictation {
  if (memory.dictation === undefined) {
    throw new Error("the transport has no dictation: give it the `dictation` option");
  }
  return memory.dictation;
}

/** Starts `controller` without waiting for it to be up (a microphone that has not answered yet), and lets what is ready run. */
export async function kick(controller: DictationController): Promise<void> {
  void controller.start();
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

/** Starts the dictation of `editor` and waits until the engine listens. */
export async function startAndListen(editor: Editor): Promise<DictationController> {
  const controller = controllerOf(editor);
  await act(async () => {
    await controller.start();
  });
  await waitFor(() => {
    expect(controller.state.get().phase).toBe("listening");
  });
  return controller;
}

/** Feeds `seconds` of audio to the fake microphone (0.2 s is one chunk of 3,200 samples, one phrase of `READY`). */
export function speak(microphone: FakeMicrophone, seconds = 0.2): void {
  act(() => {
    microphone.feed(seconds);
  });
}

/** The text of the page's blocks, one string per block. */
export function blocksOf(editor: Editor): string[] {
  const texts: string[] = [];
  editor.state.doc.forEach((node) => {
    texts.push(node.textContent);
  });
  return texts;
}

/** The dictation bar, once it is up. */
export const findBar = () => screen.findByRole("region", { name: t("dictation.label") });

/** The dictation bar, or `null`. */
export const queryBar = () => screen.queryByRole("region", { name: t("dictation.label") });

/** Waits until the bar is gone. */
export async function barGone(): Promise<void> {
  await waitFor(() => {
    expect(queryBar()).toBeNull();
  });
}

/** Types `/query` at the cursor of the page's last (empty) block and waits for the menu. */
export async function openSlashMenu(editor: Editor, query: string): Promise<HTMLElement> {
  act(() => {
    editor.commands.insertContent(`/${query}`);
  });
  return screen.findByRole("listbox", { name: t("slash.label") });
}

/** The label of the option, as the menu shows it. */
export function optionLabels(menu: HTMLElement): (string | null | undefined)[] {
  return within(menu)
    .getAllByRole("option")
    .map((option) => option.querySelector(".suggestion-title")?.textContent);
}

// ----- a controller on a schema-only editor ----------------------------------------------------------

/** A microphone that hands the test the events of its capture, and counts what the controller asks of it. */
export function fakePcm() {
  let events: CaptureEvents | null = null;
  const capture = {
    start: vi.fn(() => Promise.resolve()),
    stop: vi.fn(() => Promise.resolve()),
    cancel: vi.fn(),
  } satisfies PcmCapture;
  return {
    capture,
    make: (made: CaptureEvents): PcmCapture => {
      events = made;
      return capture;
    },
    /** Hands the controller `samples` samples of a quiet tone, as a block of the capture. */
    feed: (samples: number): void => {
      events?.onPcm(new Float32Array(samples).fill(0.1));
      events?.onLevel(0.5);
    },
  };
}

/** A schema-only editor in the page, holding paragraphs of `texts`. */
export function bareEditor(...texts: string[]): Editor {
  const element = document.createElement("div");
  document.body.append(element);
  return new Editor({
    element,
    extensions: createSchemaExtensions(),
    content: {
      type: "doc",
      content: (texts.length === 0 ? [""] : texts).map((text) => ({
        type: "paragraph",
        ...(text === "" ? {} : { content: [{ type: "text", text }] }),
      })),
    },
  });
}

/** A controller over `api` and `editor` with the fake microphone `pcm`; nothing registers it. */
export function bareController(editor: Editor, api: DictationApi | null, pcm: ReturnType<typeof fakePcm>, extra: Partial<DictationDeps> = {}) {
  return new DictationController({ editor, api: () => api, recording: () => false, capture: pcm.make, ...extra });
}

/** The fake dictation, ready with Light chosen. */
export function readyDictation(options: MemoryDictationOptions = {}): MemoryDictation {
  return new MemoryDictation({ ...READY, ...options });
}


/** What a session of `watched` saw. */
export interface Seen {
  pushes: number[];
  stops: number;
  cancels: number;
}

/** The fake dictation, with a record of what its sessions were given and told. */
export function watched(inner: DictationApi): { api: DictationApi; sessions: Seen[] } {
  const sessions: Seen[] = [];
  const api: DictationApi = {
    status: () => inner.status(),
    select: (engine) => inner.select(engine),
    install: (engine, onEvent) => inner.install(engine, onEvent),
    cancelInstall: () => inner.cancelInstall(),
    importModel: (engine) => inner.importModel(engine),
    remove: (engine) => inner.remove(engine),
    start: async (engine, onEvent) => {
      const session = await inner.start(engine, onEvent);
      const seen: Seen = { pushes: [], stops: 0, cancels: 0 };
      sessions.push(seen);
      return {
        id: session.id,
        push: (pcm) => {
          seen.pushes.push(pcm.length);
          session.push(pcm);
        },
        stop: () => {
          seen.stops += 1;
          return session.stop();
        },
        cancel: () => {
          seen.cancels += 1;
          return session.cancel();
        },
      };
    },
  };
  return { api, sessions };
}

/** A dictation whose events and end the test sends by hand (the shell's `ended` comes whenever it likes). */
export function manual(status: DictationStatus) {
  let emit: (event: DictationEvent) => void = () => undefined;
  const session = {
    id: "manual",
    push: vi.fn<DictationSession["push"]>(),
    stop: vi.fn<DictationSession["stop"]>(() => Promise.resolve()),
    cancel: vi.fn<DictationSession["cancel"]>(() => Promise.resolve()),
  };
  const api: DictationApi = {
    status: () => Promise.resolve(status),
    select: () => Promise.resolve(status),
    install: () => Promise.resolve(),
    cancelInstall: () => Promise.resolve(),
    importModel: () => Promise.resolve(status),
    remove: () => Promise.resolve(status),
    start: (_engine, onEvent) => {
      emit = onEvent;
      return Promise.resolve(session);
    },
  };
  return { api, session, emit: (event: DictationEvent) => emit(event) };
}


/** The spies on the `stop` and `cancel` of a session of the fake dictation. */
export interface SessionSpies {
  stop: MockInstance<DictationSession["stop"]>;
  cancel: MockInstance<DictationSession["cancel"]>;
}

/** Spies on `stop` and `cancel` of every session `dictation` starts from now on, in the order they start. */
export function spyOnSessions(dictation: MemoryDictation): SessionSpies[] {
  const spies: SessionSpies[] = [];
  const start = dictation.start.bind(dictation);
  vi.spyOn(dictation, "start").mockImplementation(async (engine, onEvent) => {
    const session = await start(engine, onEvent);
    spies.push({ stop: vi.spyOn(session, "stop"), cancel: vi.spyOn(session, "cancel") });
    return session;
  });
  return spies;
}
