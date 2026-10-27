// The recorder (ADR-0037, `docs/architecture/ai.md` section 8.6) over a fake microphone: the bar, the
// live transcript, Pause, Stop (the file saved as an attachment, the audio block and the transcript in
// one transaction, then the summary), Cancel, a denied microphone, and the capture itself.

import type { AttachmentInfo } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { t } from "../../i18n";
import { stubObjectUrls } from "../../test/attachments";
import { paragraphs } from "../../test/editor";
import { openPage } from "../../test/page";
import { AI_ON, outline, placeCursor, renderAiPages, resetAi } from "./ai-test-kit";
import { createSchemaExtensions } from "../schema";
import { createCapture } from "./capture";
import { RecorderController } from "./recorder";
import { RecorderBar } from "./RecorderBar";
import { fakeMicrophone, FakeAudioContext, FakeMediaRecorder, installMicrophone, removeMicrophone } from "./recorder-kit";
import type { FakeMicrophone } from "./recorder-kit";

beforeEach(() => {
  // The inserted audio block shows a player; jsdom plays nothing and has no object URLs.
  stubObjectUrls();
  HTMLMediaElement.prototype.pause = vi.fn() as () => void;
  HTMLMediaElement.prototype.load = vi.fn() as () => void;
});

afterEach(() => {
  resetAi();
  removeMicrophone();
  vi.restoreAllMocks();
});

const space = (...texts: string[]) => [{ name: "Personal", pages: [{ title: "Notes", content: paragraphs(...texts) }] }];

/** Opens a page whose last block is the empty line `/record` is typed on, and starts the recorder from the `/` menu. */
async function startRecording(
  microphone: FakeMicrophone,
  { texts = ["Meeting notes", ""], ...options }: { texts?: string[] } & Parameters<typeof renderAiPages>[1] = {},
) {
  installMicrophone(microphone);
  const app = await renderAiPages(space(...texts), options);
  const editor = await openPage(app.user, "Notes");
  placeCursor(editor, editor.state.doc.content.size - 1);
  act(() => {
    editor.commands.insertContent("/record");
  });
  const menu = await screen.findByRole("listbox", { name: t("slash.label") });
  await waitFor(() => {
    expect(within(menu).getAllByRole("option")[0]?.querySelector(".suggestion-title")?.textContent).toBe(t("slash.record"));
  });
  await app.user.click(within(menu).getByText(t("slash.record")));
  return { ...app, editor, microphone };
}

const bar = () => screen.findByRole("region", { name: t("recorder.label") });

describe("the entry", () => {
  it("is listed while AI is on and the browser can record, and not otherwise", async () => {
    installMicrophone(fakeMicrophone());
    const on = await renderAiPages(space(""));
    const editor = await openPage(on.user, "Notes");
    placeCursor(editor, 1);
    act(() => {
      editor.commands.insertContent("/record");
    });
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    expect(within(menu).getByText(t("slash.record"))).toBeInTheDocument();
    on.unmount();
    resetAi();

    removeMicrophone();
    const cannot = await renderAiPages(space(""));
    const second = await openPage(cannot.user, "Notes");
    placeCursor(second, 1);
    act(() => {
      second.commands.insertContent("/record");
    });
    const unavailable = await screen.findByRole("listbox", { name: t("slash.label") });
    const option = within(unavailable).getByText(t("slash.record")).closest("[role=option]");
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(option).toHaveTextContent(t("recorder.unsupported"));
    cannot.unmount();
    resetAi();

    installMicrophone(fakeMicrophone());
    const off = await renderAiPages(space(""), { aiSettings: { ...AI_ON, enabled: false } });
    const third = await openPage(off.user, "Notes");
    placeCursor(third, 1);
    act(() => {
      third.commands.insertContent("/record");
    });
    // The `/audio` entry still finds the word.
    await waitFor(() => {
      const found = screen.queryByRole("listbox", { name: t("slash.label") });
      expect(found === null || within(found).queryByText(t("slash.record")) === null).toBe(true);
    });
  });
});

describe("recording", () => {
  it("opens the bar with a timer and a level meter, and records opus in webm", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    const region = await bar();
    expect(await within(region).findByRole("timer")).toHaveTextContent("00:00");
    expect(within(region).getByRole("meter", { name: t("recorder.level") })).toHaveAttribute("aria-valuenow", "0");
    expect(within(region).getByRole("button", { name: t("recorder.pause") })).toBeInTheDocument();
    expect(within(region).getByRole("button", { name: t("recorder.stop") })).toBeInTheDocument();
    expect(within(region).getByRole("button", { name: t("recorder.cancel") })).toBeInTheDocument();

    expect(microphone.getUserMedia).toHaveBeenCalledWith({ audio: true });
    expect(microphone.recorder().options).toEqual({ mimeType: "audio/webm;codecs=opus" });
    expect(microphone.recorder().state).toBe("recording");
    // The text `/record` was typed is gone, and the page stays usable.
    expect(app.editor.state.doc.textContent).toBe("Meeting notes");
    expect(app.editor.isEditable).toBe(true);

    act(() => {
      microphone.feed(0.5);
    });
    await waitFor(() => {
      expect(Number(within(region).getByRole("meter").getAttribute("aria-valuenow"))).toBeGreaterThan(0);
    });
  });

  it("pauses and resumes the recorder, and hears nothing while paused", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    const region = await bar();
    await app.user.click(await within(region).findByRole("button", { name: t("recorder.pause") }));
    expect(microphone.recorder().state).toBe("paused");
    expect(within(region).getByText(t("recorder.paused"))).toBeInTheDocument();
    act(() => {
      microphone.feed(40);
    });
    await app.user.click(within(region).getByRole("button", { name: t("recorder.resume") }));
    expect(microphone.recorder().state).toBe("recording");
    // 40 s were fed while paused: not one segment was sent.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(app.ai.calls.filter((call) => call.kind === "transcribe")).toHaveLength(0);
  });

  it("shows the last two lines of the transcript as the speech is heard, in order", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    let heard = 0;
    app.ai.setTranscriber(() => {
      heard += 1;
      return `words ${String(heard)}`;
    });
    const region = await bar();
    await within(region).findByRole("timer");
    act(() => {
      microphone.feed(75);
    });
    await waitFor(() => {
      expect(within(region).getByLabelText(t("recorder.transcript"))).toHaveTextContent("[00:00] words 1");
    });
    await waitFor(() => {
      const lines = within(region).getAllByText(/^\[\d\d:\d\d\] words \d/).map((line) => line.textContent);
      expect(lines).toHaveLength(2);
    });
    const lines = within(region).getAllByText(/^\[\d\d:\d\d\] words \d/).map((line) => line.textContent ?? "");
    expect(lines[0]).toMatch(/words 1$/);
    expect(lines[1]).toMatch(/words 2$/);
  });
});

describe("Stop", () => {
  it("saves the recording, then adds the audio block with the transcript in one transaction, then the summary", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    let heard = 0;
    app.ai.setTranscriber(() => {
      heard += 1;
      return `spoken ${String(heard)}`;
    });
    const region = await bar();
    await within(region).findByRole("timer");
    act(() => {
      microphone.feed(50);
    });

    // Every change the page gets while the recording is added, as the kinds of its blocks.
    const steps: string[][] = [];
    app.editor.on("transaction", ({ transaction }) => {
      if (transaction.docChanged) {
        steps.push(app.editor.state.doc.children.map((node) => node.type.name));
      }
    });
    await app.user.click(within(region).getByRole("button", { name: t("recorder.stop") }));

    await waitFor(() => {
      expect(outline(app.editor).some((line) => line.startsWith("taskList:"))).toBe(true);
    });
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();

    // The audio block never stood without its transcript; the summary came in a transaction of its own.
    const withAudio = steps.filter((kinds) => kinds.includes("file"));
    expect(withAudio.every((kinds) => kinds.includes("details"))).toBe(true);
    expect(steps).toHaveLength(2);

    expect(app.editor.state.doc.children.map((node) => node.type.name)).toEqual([
      "paragraph",
      "file",
      "details",
      "heading",
      "paragraph",
      "heading",
      "bulletList",
      "heading",
      "taskList",
    ]);
    const file = app.editor.state.doc.child(1);
    expect(file.attrs.view).toBe("preview");
    const info: AttachmentInfo = await app.client.attachmentGet(String(file.attrs.attachmentId));
    expect(info.name).toMatch(/^Recording \d{4}-\d{2}-\d{2} \d{2}\.\d{2}\.webm$/);
    expect(info.mime).toBe("audio/webm");
    expect(Number(info.size)).toBeGreaterThan(0);

    const transcript = app.editor.state.doc.child(2);
    expect(transcript.child(0).textContent).toBe(t("transcribe.toggleTitle"));
    expect(transcript.child(1).childCount).toBe(2);
    expect(transcript.child(1).child(0).textContent).toBe("[00:00] spoken 1");
    expect(transcript.child(1).child(1).textContent).toMatch(/^\[\d\d:\d\d\] spoken 2$/);

    // The summary was asked for the words, without timestamps, and not streamed into the page.
    const summary = app.ai.calls.filter((call) => call.kind === "generate").map((call) => call.input as { action: string; text: string });
    expect(summary).toHaveLength(1);
    expect(summary[0]?.action).toBe("meetingSummary");
    expect(summary[0]?.text).toBe("spoken 1\n\nspoken 2");

    // The microphone is released.
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(microphone.context().close).toHaveBeenCalled();
  });

  it("waits for the last segment, saves without a transcript when no model is set, and offers Transcribe later", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone, { aiSettings: { ...AI_ON, transcriptionModel: undefined } });
    const region = await bar();
    const timer = await within(region).findByRole("timer");
    expect(timer).toBeInTheDocument();
    act(() => {
      microphone.feed(10);
    });
    expect(within(region).queryByLabelText(t("recorder.transcript"))).toBeNull();
    await app.user.click(within(region).getByRole("button", { name: t("recorder.stop") }));
    await waitFor(() => {
      expect(app.editor.state.doc.children.map((node) => node.type.name)).toEqual(["paragraph", "file"]);
    });
    expect(app.ai.calls.filter((call) => call.kind === "transcribe" || call.kind === "generate")).toHaveLength(0);
  });

  it("saves the recording with a notice when the transcription fails", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    app.ai.failNext({ code: "InvalidInput", message: "the model is not installed", details: { field: "endpoint", reason: "model" } }, 5);
    const region = await bar();
    await within(region).findByRole("timer");
    act(() => {
      microphone.feed(20);
    });
    await app.user.click(within(region).getByRole("button", { name: t("recorder.stop") }));
    await waitFor(() => {
      expect(app.editor.state.doc.children.map((node) => node.type.name)).toEqual(["paragraph", "file"]);
    });
    const notice = await screen.findByRole("alert");
    expect(notice).toHaveTextContent(t("recorder.transcriptFailed", { message: "the model is not installed" }));
    await app.user.click(within(await bar()).getByRole("button", { name: t("recorder.dismiss") }));
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();
  });

  it("keeps the transcript when the summary fails, with a notice", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    app.ai.setTranscriber(() => "something said");
    app.ai.failNextStream({ code: "InvalidInput", message: "no model", details: { field: "endpoint", reason: "model" } });
    const region = await bar();
    await within(region).findByRole("timer");
    act(() => {
      microphone.feed(20);
    });
    await app.user.click(within(region).getByRole("button", { name: t("recorder.stop") }));
    expect(await screen.findByText(t("recorder.summaryFailed", { message: "no model" }))).toBeInTheDocument();
    expect(app.editor.state.doc.children.map((node) => node.type.name)).toEqual(["paragraph", "file", "details"]);
  });
});

describe("Cancel and the page going away", () => {
  it("throws everything away: nothing is saved, nothing is added, the microphone is released", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    const region = await bar();
    await within(region).findByRole("timer");
    act(() => {
      microphone.feed(20);
    });
    await app.user.click(within(region).getByRole("button", { name: t("recorder.cancel") }));

    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();
    expect(app.editor.state.doc.textContent).toBe("Meeting notes");
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(microphone.recorder().state).toBe("inactive");
    expect(await app.client.attachmentList((await app.client.spaceList())[0]?.id ?? "")).toEqual([]);
  });

  it("releases the microphone when the page is closed during a recording", async () => {
    const microphone = fakeMicrophone();
    const app = await startRecording(microphone);
    await within(await bar()).findByRole("timer");
    app.unmount();
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();
  });
});

describe("a microphone that cannot be used", () => {
  it("tells that the microphone is blocked and leaves the page alone", async () => {
    const microphone = fakeMicrophone({ name: "NotAllowedError", message: "Permission denied" });
    const app = await startRecording(microphone);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("recorder.denied"));
    // A choice of Block is remembered by the app's webview: Windows settings alone do not undo it.
    expect(alert.textContent).toMatch(/remembered/);
    expect(alert.textContent).not.toMatch(/system's privacy settings/);
    expect(app.editor.state.doc.textContent).toBe("Meeting notes");
    await app.user.click(within(await bar()).getByRole("button", { name: t("recorder.dismiss") }));
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();
  });

  it("tells when there is no microphone, and when the browser fails for another reason", async () => {
    const none = await startRecording(fakeMicrophone({ name: "NotFoundError" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("recorder.noDevice"));
    none.unmount();
    resetAi();
    removeMicrophone();

    const other = await startRecording(fakeMicrophone({ name: "AbortError", message: "device busy" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("recorder.failed", { message: "device busy" }));
    other.unmount();
  });
});

describe("the capture", () => {
  it("loads the worklet module from the app's own files and takes the samples it posts", async () => {
    const microphone = fakeMicrophone();
    const addModule = vi.fn(() => Promise.resolve());
    const nodes: { port: { onmessage: ((message: { data: Float32Array[] }) => void) | null } }[] = [];
    class FakeWorkletNode {
      readonly port: { onmessage: ((message: { data: Float32Array[] }) => void) | null } = { onmessage: null };
      constructor() {
        nodes.push(this);
      }
      connect(): void {}
      disconnect(): void {}
    }
    vi.stubGlobal("AudioWorkletNode", FakeWorkletNode);
    class WorkletContext extends FakeAudioContext {
      override audioWorklet = { addModule };
    }
    const env = { ...microphone.env, AudioContext: WorkletContext as unknown as typeof AudioContext };
    const blocks: Float32Array[] = [];
    const capture = createCapture({ onPcm: (samples) => blocks.push(samples), onLevel: () => undefined }, env);
    await capture.start();

    expect(addModule).toHaveBeenCalledTimes(1);
    const [url] = addModule.mock.calls[0] as unknown as [URL];
    expect(url.pathname).toMatch(/pcm-worklet\.js$/);
    nodes[0]?.port.onmessage?.({ data: [new Float32Array([0.1, 0.2, 0.3, 0.4])] });
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toHaveLength(4);
    capture.cancel();
    expect(nodes[0]?.port.onmessage).toBeNull();
  });

  it("resamples from the device's rate when it cannot run a context at 16 kHz", async () => {
    const microphone = fakeMicrophone();
    class Picky extends FakeAudioContext {
      constructor(options?: { sampleRate?: number }) {
        if (options?.sampleRate !== undefined) {
          throw new Error("sample rate not supported");
        }
        super();
      }
    }
    const env = { ...microphone.env, AudioContext: Picky as unknown as typeof AudioContext };
    const blocks: Float32Array[] = [];
    const capture = createCapture({ onPcm: (samples) => blocks.push(samples), onLevel: () => undefined }, env);
    await capture.start();
    const context = FakeAudioContext.instances.at(-1);
    expect(context?.sampleRate).toBe(48000);
    context?.processor?.onaudioprocess?.({ inputBuffer: { getChannelData: () => new Float32Array(4800).fill(0.1) } });
    expect(blocks[0]).toHaveLength(1600); // 4800 samples at 48 kHz are 100 ms: 1600 at 16 kHz
    const blob = await capture.stop();
    expect(blob.type).toBe("audio/webm;codecs=opus");
  });
});

/** A bare editor and a recorder over it, drawn with its bar. */
function bareRecorder(microphone: FakeMicrophone) {
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({
    element,
    extensions: createSchemaExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  const controller = new RecorderController({
    editor,
    client: () => null,
    capabilities: () => ({ enabled: true, writing: false, transcription: false, chatModel: null, transcriptionModel: null }),
    upload: () => null,
    env: () => microphone.env,
    startHintMs: 30,
  });
  render(<RecorderBar controller={controller} />);
  return {
    controller,
    end: () => {
      controller.cancel();
      editor.destroy();
      element.remove();
    },
  };
}

describe("a permission request that is still open", () => {
  it("lets Cancel close the bar at once, and lets go of the microphone when it arrives late", async () => {
    const microphone = fakeMicrophone(undefined, { pending: true });
    const app = await startRecording(microphone);
    const region = await bar();
    expect(within(region).getByText(t("recorder.starting"))).toBeInTheDocument();

    await app.user.click(within(region).getByRole("button", { name: t("recorder.cancel") }));
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();

    // The person answers Allow afterwards: the stream is stopped on the spot, and nothing else is done with it.
    microphone.answer("allow");
    await waitFor(() => {
      expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
    });
    expect(FakeMediaRecorder.instances).toHaveLength(0);
    expect(FakeAudioContext.instances).toHaveLength(0);
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();
    expect(app.editor.state.doc.textContent).toBe("Meeting notes");
  });

  it("shows no notice when the request is answered Block after Cancel", async () => {
    const microphone = fakeMicrophone(undefined, { pending: true });
    const app = await startRecording(microphone);
    await app.user.click(within(await bar()).getByRole("button", { name: t("recorder.cancel") }));
    microphone.answer("block");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(screen.queryByRole("region", { name: t("recorder.label") })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says after a few seconds that a request may be waiting, and takes the hint away once it is answered", async () => {
    const microphone = fakeMicrophone(undefined, { pending: true });
    const { controller, end } = bareRecorder(microphone);
    act(() => {
      void controller.start();
    });
    expect(await screen.findByText(t("recorder.starting"))).toBeInTheDocument();
    expect(screen.queryByText(t("recorder.permissionHint"))).toBeNull();
    expect(await screen.findByText(t("recorder.permissionHint"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("recorder.cancel") })).toBeInTheDocument();

    await act(async () => {
      microphone.answer("allow");
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    await screen.findByRole("timer");
    expect(screen.queryByText(t("recorder.permissionHint"))).toBeNull();
    end();
  });

  it("does not show the hint for a microphone that answers quickly", async () => {
    const { controller, end } = bareRecorder(fakeMicrophone());
    await act(async () => {
      await controller.start();
    });
    await screen.findByRole("timer");
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(screen.queryByText(t("recorder.permissionHint"))).toBeNull();
    end();
  });
});
