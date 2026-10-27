// A fake microphone for the tests of the recorder: a `MediaStream`, a `MediaRecorder` and an
// `AudioContext` that jsdom lacks, and a way to feed the audio graph samples as a real device would.

import { vi } from "vitest";

import type { CaptureEnv } from "./capture";

export class FakeTrack {
  readonly stop = vi.fn();
}

export class FakeStream {
  readonly tracks = [new FakeTrack()];

  getTracks(): FakeTrack[] {
    return this.tracks;
  }
}

type RecorderState = "inactive" | "recording" | "paused";

export class FakeMediaRecorder {
  static readonly instances: FakeMediaRecorder[] = [];
  static isTypeSupported = (): boolean => true;
  state: RecorderState = "inactive";
  mimeType = "audio/webm;codecs=opus";
  ondataavailable: ((event: { data: Blob }) => void) | null = null;
  onstop: (() => void) | null = null;
  readonly started: number[] = [];

  constructor(
    readonly stream: FakeStream,
    readonly options?: { mimeType?: string },
  ) {
    FakeMediaRecorder.instances.push(this);
  }

  start(timeslice?: number): void {
    this.state = "recording";
    this.started.push(timeslice ?? 0);
  }

  pause(): void {
    this.state = "paused";
  }

  resume(): void {
    this.state = "recording";
  }

  stop(): void {
    this.state = "inactive";
    this.ondataavailable?.({ data: new Blob([new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 1, 2, 3, 4])], { type: this.mimeType }) });
    this.onstop?.();
  }
}

interface FakeProcessor {
  onaudioprocess: ((event: { inputBuffer: { getChannelData: (channel: number) => Float32Array } }) => void) | null;
  connect: () => void;
  disconnect: () => void;
}

export class FakeAudioContext {
  static readonly instances: FakeAudioContext[] = [];
  sampleRate: number;
  state = "running";
  readonly destination = {};
  processor: FakeProcessor | null = null;
  audioWorklet: { addModule: (url: URL) => Promise<void> } | undefined = undefined;
  readonly close = vi.fn(() => {
    this.state = "closed";
    return Promise.resolve();
  });

  constructor(options?: { sampleRate?: number }) {
    this.sampleRate = options?.sampleRate ?? 48000;
    FakeAudioContext.instances.push(this);
  }

  resume(): Promise<void> {
    this.state = "running";
    return Promise.resolve();
  }

  createMediaStreamSource(): { connect: () => void; disconnect: () => void } {
    return { connect: () => undefined, disconnect: () => undefined };
  }

  createGain(): { gain: { value: number }; connect: () => void; disconnect: () => void } {
    return { gain: { value: 1 }, connect: () => undefined, disconnect: () => undefined };
  }

  createScriptProcessor(): FakeProcessor {
    this.processor = { onaudioprocess: null, connect: () => undefined, disconnect: () => undefined };
    return this.processor;
  }
}

/** What a test of the recorder installs and reads back. */
export interface FakeMicrophone {
  env: CaptureEnv;
  getUserMedia: ReturnType<typeof vi.fn>;
  streams: FakeStream[];
  /** The context the capture built (the last one). */
  context: () => FakeAudioContext;
  /** Feeds `seconds` of a tone, as blocks of 4096 samples at the context's rate. */
  feed: (seconds: number) => void;
  /** The recorder the capture made (the last one). */
  recorder: () => FakeMediaRecorder;
  /** With `pending`: answers the request that is open, as the person would in the permission window. */
  answer: (answer: "allow" | "block") => void;
}

/**
 * A fake microphone; `getUserMedia` rejects with `failure` when given. With `pending` it does not answer until
 * `answer` is called (WebView2 asks the person in a window of its own).
 */
export function fakeMicrophone(failure?: { name: string; message?: string }, { pending = false } = {}): FakeMicrophone {
  FakeMediaRecorder.instances.length = 0;
  FakeAudioContext.instances.length = 0;
  const streams: FakeStream[] = [];
  const waiting: ((answer: "allow" | "block") => void)[] = [];
  const getUserMedia = vi.fn(() => {
    if (pending) {
      return new Promise<MediaStream>((resolve, reject) => {
        waiting.push((answer) => {
          if (answer === "block") {
            reject(Object.assign(new Error("Permission denied"), { name: "NotAllowedError" }));
            return;
          }
          const stream = new FakeStream();
          streams.push(stream);
          resolve(stream as unknown as MediaStream);
        });
      });
    }
    if (failure !== undefined) {
      return Promise.reject(Object.assign(new Error(failure.message ?? failure.name), { name: failure.name }));
    }
    const stream = new FakeStream();
    streams.push(stream);
    return Promise.resolve(stream as unknown as MediaStream);
  });
  const env: CaptureEnv = {
    getUserMedia,
    MediaRecorder: FakeMediaRecorder as unknown as typeof MediaRecorder,
    AudioContext: FakeAudioContext as unknown as typeof AudioContext,
  };
  let fed = 0;
  return {
    env,
    getUserMedia,
    streams,
    context: () => {
      const context = FakeAudioContext.instances.at(-1);
      if (context === undefined) {
        throw new Error("no audio context was made");
      }
      return context;
    },
    recorder: () => {
      const recorder = FakeMediaRecorder.instances.at(-1);
      if (recorder === undefined) {
        throw new Error("no recorder was made");
      }
      return recorder;
    },
    answer(answer) {
      for (const respond of waiting.splice(0)) {
        respond(answer);
      }
    },
    feed(seconds) {
      const context = FakeAudioContext.instances.at(-1);
      const onaudioprocess = context?.processor?.onaudioprocess;
      if (context === undefined || onaudioprocess === null || onaudioprocess === undefined) {
        throw new Error("the capture is not listening");
      }
      const rate = context.sampleRate;
      const total = Math.round(seconds * rate);
      for (let done = 0; done < total; done += 4096) {
        const block = new Float32Array(Math.min(4096, total - done));
        for (let i = 0; i < block.length; i += 1) {
          block[i] = 0.2 * Math.sin((2 * Math.PI * 220 * (fed + done + i)) / rate);
        }
        onaudioprocess({ inputBuffer: { getChannelData: () => block } });
      }
      fed += total;
    },
  };
}

/** Installs `microphone` as the page's (`navigator.mediaDevices`, `MediaRecorder`, `AudioContext`); `vi.unstubAllGlobals()` removes it. */
export function installMicrophone(microphone: FakeMicrophone): void {
  vi.stubGlobal("MediaRecorder", FakeMediaRecorder);
  vi.stubGlobal("AudioContext", FakeAudioContext);
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: { getUserMedia: microphone.getUserMedia },
  });
}

/** Removes what `installMicrophone` put there. */
export function removeMicrophone(): void {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "mediaDevices");
}
