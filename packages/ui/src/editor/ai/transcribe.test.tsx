// "Transcribe" on an audio or video block (ADR-0037, `docs/architecture/ai.md` section 8.6): the menu entry,
// the progress in the block with its Cancel, the transcript and the summary under the block, and the
// rules of reading the file (ranges, 512 MiB) and decoding it (a fake `OfflineAudioContext`).

import { demoWav, MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransportOptions } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { t } from "../../i18n";
import { stubObjectUrls } from "../../test/attachments";
import { pid } from "../../test/editor";
import { openPage } from "../../test/page";
import { AI_ON, renderAiPages, resetAi } from "./ai-test-kit";
import { decodeToMono16k } from "./audio-decode";
import { readFileBytes, MAX_TRANSCRIBE_BYTES, TranscribeJobs, transcribeFile } from "./transcribe-jobs";
import { AudioDecodeError } from "./audio-decode";

const SOUND = "a0000000-0000-4000-8000-0000000000a1";
const MOVIE = "a0000000-0000-4000-8000-0000000000a2";
const TEXT = "a0000000-0000-4000-8000-0000000000a3";

function seed(): MemoryTransportOptions["attachments"] {
  return [
    { attachmentId: SOUND, name: "talk.wav", mime: "audio/wav", bytes: demoWav(3) },
    { attachmentId: MOVIE, name: "clip.mp4", mime: "video/mp4", bytes: new Uint8Array([1, 2, 3]) },
    { attachmentId: TEXT, name: "notes.txt", mime: "text/plain", bytes: new TextEncoder().encode("hello") },
  ];
}

/** `seconds` of a tone at 16 kHz, with a quiet moment at 31 s. */
function tone(seconds: number): Float32Array {
  const rate = 16000;
  const samples = new Float32Array(seconds * rate);
  for (let i = 0; i < samples.length; i += 1) {
    const at = i / rate;
    samples[i] = at > 30.85 && at < 31.15 ? 0 : 0.2 * Math.sin((2 * Math.PI * 220 * i) / rate);
  }
  return samples;
}

/** What `decodeAudioData` answers in the tests, set by each test. */
let decoded: { channels: Float32Array[]; sampleRate: number } | Error = { channels: [tone(70)], sampleRate: 16000 };
const decodeCalls: { contexts: number[]; bytes: number[] } = { contexts: [], bytes: [] };

class FakeOfflineAudioContext {
  constructor(channels: number, length: number, sampleRate: number) {
    decodeCalls.contexts.push(channels, length, sampleRate);
  }

  decodeAudioData(data: ArrayBuffer): Promise<unknown> {
    decodeCalls.bytes.push(data.byteLength);
    if (decoded instanceof Error) {
      return Promise.reject(decoded);
    }
    const { channels, sampleRate } = decoded;
    return Promise.resolve({
      numberOfChannels: channels.length,
      sampleRate,
      getChannelData: (index: number) => channels[index],
    });
  }
}

beforeEach(() => {
  decoded = { channels: [tone(70)], sampleRate: 16000 };
  decodeCalls.contexts = [];
  decodeCalls.bytes = [];
  vi.stubGlobal("OfflineAudioContext", FakeOfflineAudioContext);
  stubObjectUrls();
  HTMLMediaElement.prototype.pause = vi.fn() as () => void;
  HTMLMediaElement.prototype.load = vi.fn() as () => void;
});

afterEach(() => {
  resetAi();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function pageWith(attachmentId: string): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Listen:" }] },
      { type: "file", attrs: { id: pid(1), attachmentId, view: "preview" } },
      { type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "After" }] },
    ],
  };
}

async function openFile(attachmentId: string, options: Parameters<typeof renderAiPages>[1] = {}) {
  const app = await renderAiPages([{ name: "Personal", pages: [{ title: "Notes", content: pageWith(attachmentId) }] }], {
    attachments: seed(),
    ...options,
  });
  const editor = await openPage(app.user, "Notes");
  return { ...app, editor };
}

async function menu(app: Awaited<ReturnType<typeof openFile>>) {
  await app.user.click(within(app.editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
  return screen.findByRole("menu");
}

const kinds = (editor: Awaited<ReturnType<typeof openFile>>["editor"]) => editor.state.doc.children.map((node) => node.type.name);

describe("the menu entry", () => {
  it("is in the menu of a complete audio or video block while a transcription model is set", async () => {
    for (const id of [SOUND, MOVIE]) {
      const app = await openFile(id);
      expect(await within(await menu(app)).findByRole("menuitem", { name: t("transcribe.menu") })).toBeInTheDocument();
      app.unmount();
      resetAi();
    }
  });

  it("is not there for another file, without a transcription model, with AI off, or in a read-only page", async () => {
    const text = await openFile(TEXT);
    expect(within(await menu(text)).queryByRole("menuitem", { name: t("transcribe.menu") })).toBeNull();
    text.unmount();
    resetAi();

    const noModel = await openFile(SOUND, { aiSettings: { ...AI_ON, transcriptionModel: undefined } });
    expect(within(await menu(noModel)).queryByRole("menuitem", { name: t("transcribe.menu") })).toBeNull();
    noModel.unmount();
    resetAi();

    const off = await openFile(SOUND, { aiSettings: { ...AI_ON, enabled: false } });
    expect(within(await menu(off)).queryByRole("menuitem", { name: t("transcribe.menu") })).toBeNull();
    off.unmount();
    resetAi();

    const readOnly = await openFile(SOUND);
    act(() => {
      readOnly.editor.setEditable(false);
    });
    expect(within(await menu(readOnly)).queryByRole("menuitem", { name: t("transcribe.menu") })).toBeNull();
  });
});

describe("transcribing a block", () => {
  it("reads and decodes the file, shows 'Transcribing n of N' with Cancel, then adds the transcript and the summary", async () => {
    const app = await openFile(SOUND);
    let heard = 0;
    app.ai.setTranscriber(() => {
      heard += 1;
      return `part ${String(heard)}`;
    });
    // Hold the transcription until the test has looked at the progress.
    const original = app.ai.transcribe.bind(app.ai);
    let open: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      open = resolve;
    });
    app.ai.transcribe = async (wav, model) => {
      await gate;
      return original(wav, model);
    };

    await app.user.click(within(await menu(app)).getByRole("menuitem", { name: t("transcribe.menu") }));
    const status = await screen.findByText(t("transcribe.progress", { done: 1, total: 3 }));
    expect(status).toBeInTheDocument();
    const block = status.closest(".transcribe-status") as HTMLElement;
    expect(within(block).getByRole("button", { name: t("transcribe.cancel") })).toBeInTheDocument();
    expect(within(block).getByRole("progressbar", { name: t("transcribe.progress", { done: 1, total: 3 }) })).toBeInTheDocument();
    expect(kinds(app.editor)).toEqual(["paragraph", "file", "paragraph"]);

    // The file was decoded at 16 kHz, mono, from every byte of it.
    expect(decodeCalls.contexts).toEqual([1, 1, 16000]);
    expect(decodeCalls.bytes[0]).toBe(demoWav(3).length);

    const steps: string[][] = [];
    app.editor.on("transaction", ({ transaction }) => {
      if (transaction.docChanged) {
        steps.push(kinds(app.editor));
      }
    });
    open();
    await waitFor(() => {
      expect(kinds(app.editor)).toContain("taskList");
    });
    // The toggle came in one transaction, the summary in another, both right under the block.
    expect(steps).toHaveLength(2);
    expect(steps[0]).toEqual(["paragraph", "file", "details", "paragraph"]);
    expect(kinds(app.editor)).toEqual([
      "paragraph",
      "file",
      "details",
      "heading",
      "paragraph",
      "heading",
      "bulletList",
      "heading",
      "taskList",
      "paragraph",
    ]);
    const toggle = app.editor.state.doc.child(2);
    expect(toggle.child(1).children.map((node) => node.textContent)).toEqual(["[00:00] part 1", expect.stringMatching(/^\[\d\d:\d\d\] part 2$/), expect.stringMatching(/^\[\d\d:\d\d\] part 3$/)]);
    expect(screen.queryByText(/Transcribing/)).toBeNull();
    expect(app.ai.calls.filter((call) => call.kind === "transcribe")).toHaveLength(3);
    expect(app.ai.calls.filter((call) => call.kind === "generate")).toMatchObject([{ input: { action: "meetingSummary" } }]);
  });

  it("adds only the transcript when no chat model is set", async () => {
    const app = await openFile(SOUND, { aiSettings: { ...AI_ON, chatModel: undefined } });
    await app.user.click(within(await menu(app)).getByRole("menuitem", { name: t("transcribe.menu") }));
    await waitFor(() => {
      expect(kinds(app.editor)).toEqual(["paragraph", "file", "details", "paragraph"]);
    });
    expect(app.ai.calls.filter((call) => call.kind === "generate")).toHaveLength(0);
  });

  it("stops at Cancel: nothing is added and the rest is not sent", async () => {
    const app = await openFile(SOUND);
    const original = app.ai.transcribe.bind(app.ai);
    let open: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      open = resolve;
    });
    app.ai.transcribe = async (wav, model) => {
      await gate;
      return original(wav, model);
    };
    await app.user.click(within(await menu(app)).getByRole("menuitem", { name: t("transcribe.menu") }));
    await app.user.click(await screen.findByRole("button", { name: t("transcribe.cancel") }));
    expect(screen.queryByText(/Transcribing/)).toBeNull();
    open();
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(kinds(app.editor)).toEqual(["paragraph", "file", "paragraph"]);
    expect(app.ai.calls.filter((call) => call.kind === "transcribe")).toHaveLength(1);
    expect(app.ai.calls.filter((call) => call.kind === "generate")).toHaveLength(0);
  });

  it("says when the audio cannot be decoded, and when nobody speaks", async () => {
    decoded = new Error("Unable to decode audio data");
    const first = await openFile(SOUND);
    await first.user.click(within(await menu(first)).getByRole("menuitem", { name: t("transcribe.menu") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("transcribe.decodeFailed", { message: "Unable to decode audio data" }));
    await first.user.click(screen.getByRole("button", { name: t("transcribe.dismiss") }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(kinds(first.editor)).toEqual(["paragraph", "file", "paragraph"]);
    first.unmount();
    resetAi();

    decoded = { channels: [tone(5)], sampleRate: 16000 };
    const second = await openFile(SOUND);
    second.ai.setTranscriber(() => "");
    await second.user.click(within(await menu(second)).getByRole("menuitem", { name: t("transcribe.menu") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("transcribe.noSpeech"));
    expect(kinds(second.editor)).toEqual(["paragraph", "file", "paragraph"]);
  });

  it("shows why a failing transcription added nothing", async () => {
    decoded = { channels: [tone(5)], sampleRate: 16000 };
    const app = await openFile(SOUND);
    app.ai.failNext({ code: "InvalidInput", message: "the model is not installed", details: { field: "endpoint", reason: "model" } }, 3);
    await app.user.click(within(await menu(app)).getByRole("menuitem", { name: t("transcribe.menu") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("transcribe.failed", { message: "the model is not installed" }));
    expect(kinds(app.editor)).toEqual(["paragraph", "file", "paragraph"]);
  });

  it("goes with the page: a job still running when the editor goes is cancelled", async () => {
    const app = await openFile(SOUND);
    const original = app.ai.transcribe.bind(app.ai);
    app.ai.transcribe = async (wav, model) => {
      await new Promise((resolve) => setTimeout(resolve, 20));
      return original(wav, model);
    };
    await app.user.click(within(await menu(app)).getByRole("menuitem", { name: t("transcribe.menu") }));
    await screen.findByText(t("transcribe.progress", { done: 1, total: 3 }));
    app.unmount();
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(app.ai.calls.filter((call) => call.kind === "transcribe").length).toBeLessThanOrEqual(2);
  });
});

describe("reading and decoding", () => {
  it("reads a file in ranges of at most the preview limit and puts the pieces in place", async () => {
    const size = 40 * 1024 * 1024;
    const ranges: { start: number; end: number }[] = [];
    const bytes = await readFileBytes(
      (range) => {
        ranges.push(range);
        return Promise.resolve(new Uint8Array(range.end - range.start).fill(ranges.length));
      },
      size,
      new AbortController().signal,
      () => undefined,
    );
    expect(ranges).toEqual([
      { start: 0, end: 16 * 1024 * 1024 },
      { start: 16 * 1024 * 1024, end: 32 * 1024 * 1024 },
      { start: 32 * 1024 * 1024, end: size },
    ]);
    for (const range of ranges) {
      expect(range.end - range.start).toBeLessThanOrEqual(MAX_PREVIEW_BYTES);
    }
    expect(bytes).toHaveLength(size);
    expect([bytes[0], bytes[16 * 1024 * 1024], bytes[size - 1]]).toEqual([1, 2, 3]);
  });

  it("refuses a file beyond 512 MiB before reading any of it", async () => {
    const readRange = vi.fn(() => Promise.resolve(new Uint8Array(0)));
    const jobs = new TranscribeJobs({
      editor: {} as never,
      client: () => ({ hasAi: () => true, aiGenerate: vi.fn(), aiCancel: vi.fn(), aiTranscribe: vi.fn() }),
      capabilities: () => ({ enabled: true, writing: true, transcription: true, chatModel: "m", transcriptionModel: "r" }),
      readRange,
    });
    expect(MAX_TRANSCRIBE_BYTES).toBe(512 * 1024 * 1024);
    await jobs.start({ blockId: "b", attachmentId: "a", size: MAX_TRANSCRIBE_BYTES + 1 });
    expect(readRange).not.toHaveBeenCalled();
    expect(jobs.views.get().get("b")).toEqual({ status: "error", message: t("transcribe.tooLarge") });
    jobs.dismiss("b");
    expect(jobs.views.get().size).toBe(0);
  });

  it("transcribes a decoded file in segments, in order, and reports each step", async () => {
    const steps: string[] = [];
    const seen: number[] = [];
    const result = await transcribeFile({
      client: {
        aiTranscribe: (wav) => {
          seen.push(wav.length);
          return Promise.resolve({ text: `text ${String(seen.length)}` });
        },
      },
      model: "r2t2",
      readRange: (range) => Promise.resolve(new Uint8Array(range.end - range.start)),
      decode: () => Promise.resolve(tone(70)),
      size: 100,
      signal: new AbortController().signal,
      onProgress: (view) => {
        steps.push(`${view.step}`);
      },
    });
    expect(result.segments.map((segment) => segment.text)).toEqual(["text 1", "text 2", "text 3"]);
    expect(result.segments.map((segment) => segment.startMs)[0]).toBe(0);
    expect(result.failed).toBe(0);
    expect([...new Set(steps)]).toEqual(["reading", "decoding", "transcribing"]);
    for (const length of seen) {
      expect(length).toBeLessThanOrEqual(44 + 35 * 16000 * 2);
    }
  });

  it("mixes channels down and takes the decoder's rate", async () => {
    decoded = { channels: [new Float32Array(32000).fill(0.5), new Float32Array(32000).fill(-0.5)], sampleRate: 16000 };
    const samples = await decodeToMono16k(new Uint8Array([1, 2, 3, 4]));
    expect(samples).toHaveLength(32000);
    expect(samples[100]).toBeCloseTo(0, 5);
    vi.unstubAllGlobals();
    await expect(decodeToMono16k(new Uint8Array([1]))).rejects.toBeInstanceOf(AudioDecodeError);
  });
});
