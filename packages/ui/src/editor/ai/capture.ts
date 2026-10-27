// Microphone capture for the recorder (ADR-0037, `docs/architecture/ai.md` section 8.6): one stream, two
// consumers. A `MediaRecorder` writes `audio/webm;codecs=opus`, the file that is stored. In parallel the
// Web Audio API hands the same samples to the page as PCM (an `AudioWorklet` loaded from the app's own
// origin, `ScriptProcessorNode` where worklets are missing), downmixed and resampled to 16 kHz for the
// transcription. Pausing pauses both; nothing is kept while paused.
//
// Dictation (ADR-0053, `docs/architecture/dictation.md` section 6) takes the same 16 kHz tap with no
// `MediaRecorder` and nothing stored: `createPcmCapture`.
//
// The browser objects come in through `CaptureEnv` (the globals by default), so that tests give the
// recorder a fake microphone.

import { downmixAndResample, TRANSCRIPTION_RATE } from "@atomik-kb/core-client";

/** The browser APIs a capture of samples alone uses: the microphone and the Web Audio API. */
export interface PcmCaptureEnv {
  getUserMedia: (constraints: MediaStreamConstraints) => Promise<MediaStream>;
  AudioContext: typeof AudioContext;
}

/** The browser APIs the recorder's capture uses. */
export interface CaptureEnv extends PcmCaptureEnv {
  MediaRecorder: typeof MediaRecorder;
}

/** The environment of the page for samples alone, or `null` where there is no microphone API or no Web Audio. */
export function browserPcmEnv(): PcmCaptureEnv | null {
  const devices = (globalThis.navigator as Navigator | undefined)?.mediaDevices;
  const context =
    (globalThis as { AudioContext?: typeof AudioContext }).AudioContext ??
    (globalThis as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (typeof devices?.getUserMedia !== "function" || context === undefined) {
    return null;
  }
  return {
    getUserMedia: (constraints) => devices.getUserMedia(constraints),
    AudioContext: context,
  };
}

/** The environment of the page, or `null` where a microphone cannot be recorded (no media devices, no recorder). */
export function browserCaptureEnv(): CaptureEnv | null {
  const pcm = browserPcmEnv();
  const recorder = (globalThis as { MediaRecorder?: typeof MediaRecorder }).MediaRecorder;
  return pcm === null || recorder === undefined ? null : { ...pcm, MediaRecorder: recorder };
}

/** What a capture reports. */
export interface CaptureEvents {
  /** A block of 16 kHz mono samples (only while recording, not while paused). */
  onPcm: (samples: Float32Array) => void;
  /** The loudness of the last block, 0 to 1. */
  onLevel: (level: number) => void;
}

/** Why a capture could not start. */
export type CaptureFailure = "denied" | "noDevice" | "failed";

export class CaptureError extends Error {
  readonly reason: CaptureFailure;

  constructor(reason: CaptureFailure, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "CaptureError";
    this.reason = reason;
  }
}

/** The kind of a `getUserMedia` failure, from the error's name. */
export function failureOf(error: unknown): CaptureFailure {
  const name = typeof error === "object" && error !== null && "name" in error ? String(error.name) : "";
  if (name === "NotAllowedError" || name === "SecurityError" || name === "PermissionDeniedError") {
    return "denied";
  }
  if (name === "NotFoundError" || name === "DevicesNotFoundError" || name === "OverconstrainedError") {
    return "noDevice";
  }
  return "failed";
}

/** The mime type the stored file is recorded in, where the browser knows it. */
const PREFERRED_TYPE = "audio/webm;codecs=opus";

/** The loudness of a block, 0 to 1 (its RMS, scaled so that speech moves the meter). */
export function levelOf(samples: Float32Array): number {
  if (samples.length === 0) {
    return 0;
  }
  let sum = 0;
  for (const sample of samples) {
    sum += sample * sample;
  }
  return Math.min(1, Math.sqrt(sum / samples.length) * 4);
}

/** The microphone, recording. */
export interface Capture {
  /** Asks for the microphone (rejects with a `CaptureError`) and starts recording. */
  start: () => Promise<void>;
  pause: () => void;
  resume: () => void;
  /** Stops and resolves with the recorded file's bytes (`audio/webm`). */
  stop: () => Promise<Blob>;
  /** Stops and drops everything. */
  cancel: () => void;
}

/** The microphone as 16 kHz samples alone (dictation): nothing is recorded to a file. */
export interface PcmCapture {
  /** Asks for the microphone (rejects with a `CaptureError`) and starts handing out samples. */
  start: () => Promise<void>;
  /** Lets the microphone go (the samples that were handed out stay delivered). */
  stop: () => Promise<void>;
  /** Stops and drops everything; a request for the microphone that is still open is let go of when it is answered. */
  cancel: () => void;
}

export function createCapture(events: CaptureEvents, env: CaptureEnv): Capture {
  return buildCapture(events, env, env.MediaRecorder);
}

/**
 * The 16 kHz tap of the recorder's capture with no `MediaRecorder`: the same `getUserMedia` constraints, the same
 * worklet (the script processor where there is none), the same failure kinds, and nothing kept: `onPcm` gets the blocks
 * and that is all of the audio there is.
 */
export function createPcmCapture(events: CaptureEvents, env: PcmCaptureEnv): PcmCapture {
  const capture = buildCapture(events, env, null);
  return {
    start: () => capture.start(),
    stop: async () => {
      await capture.stop();
    },
    cancel: () => {
      capture.cancel();
    },
  };
}

function buildCapture(events: CaptureEvents, env: PcmCaptureEnv, recorderType: typeof MediaRecorder | null): Capture {
  let stream: MediaStream | null = null;
  let recorder: MediaRecorder | null = null;
  let context: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let tap: AudioNode | null = null;
  let mute: GainNode | null = null;
  let paused = false;
  let closed = false;
  const chunks: Blob[] = [];

  /** A block of the stream's samples, one array per channel (while paused they are dropped). */
  function onBlock(channels: Float32Array[]): void {
    if (paused || closed || context === null) {
      return;
    }
    const samples = downmixAndResample(channels, context.sampleRate, TRANSCRIPTION_RATE);
    events.onLevel(levelOf(samples));
    events.onPcm(samples);
  }

  async function startTap(audio: AudioContext, input: MediaStreamAudioSourceNode): Promise<void> {
    mute = audio.createGain();
    mute.gain.value = 0; // the nodes must reach the destination to run; they must not play
    mute.connect(audio.destination);
    if (typeof AudioWorkletNode !== "undefined" && audio.audioWorklet !== undefined) {
      try {
        await audio.audioWorklet.addModule(new URL("./pcm-worklet.js?no-inline", import.meta.url));
        const node = new AudioWorkletNode(audio, "atomik-pcm-capture", { numberOfInputs: 1, numberOfOutputs: 1 });
        node.port.onmessage = (message: MessageEvent<Float32Array[]>) => {
          onBlock(message.data);
        };
        input.connect(node);
        node.connect(mute);
        tap = node;
        return;
      } catch {
        // No worklet here (or the module did not load): the script processor does the same, on the main thread.
      }
    }
    const processor = audio.createScriptProcessor(4096, 1, 1);
    processor.onaudioprocess = (event: AudioProcessingEvent) => {
      onBlock([new Float32Array(event.inputBuffer.getChannelData(0))]);
    };
    input.connect(processor);
    processor.connect(mute);
    tap = processor;
  }

  /** Takes the audio graph down (the stream and the recorder stay). */
  function teardownGraph(): void {
    if (tap !== null) {
      if ("port" in tap) {
        (tap as AudioWorkletNode).port.onmessage = null;
      }
      if ("onaudioprocess" in tap) {
        (tap as ScriptProcessorNode).onaudioprocess = null;
      }
      tap.disconnect();
    }
    source?.disconnect();
    mute?.disconnect();
    if (context !== null && context.state !== "closed") {
      context.close().catch(() => undefined);
    }
    tap = null;
    source = null;
    mute = null;
    context = null;
  }

  /** Takes everything down: the graph, then the microphone. */
  function release(): void {
    closed = true;
    teardownGraph();
    for (const track of stream?.getTracks() ?? []) {
      track.stop();
    }
    stream = null;
  }

  /** The audio graph at `rate` (the context's own when `undefined`): microphone, tap, mute. */
  async function buildGraph(rate: number | undefined): Promise<void> {
    if (stream === null) {
      return;
    }
    context = rate === undefined ? new env.AudioContext() : new env.AudioContext({ sampleRate: rate });
    source = context.createMediaStreamSource(stream);
    await startTap(context, source);
    if (context.state === "suspended") {
      await context.resume();
    }
  }

  return {
    async start() {
      let acquired: MediaStream;
      try {
        acquired = await env.getUserMedia({ audio: true });
      } catch (error) {
        const reason = failureOf(error);
        throw new CaptureError(reason, error instanceof Error ? error.message : String(error), { cause: error });
      }
      if (closed) {
        // Cancelled while the permission request was open (WebView2 asks in a window of its own): the
        // microphone arrives late and is let go of at once, nothing else is done with it.
        for (const track of acquired.getTracks()) {
          track.stop();
        }
        return;
      }
      stream = acquired;
      try {
        if (recorderType !== null) {
          const options = recorderType.isTypeSupported(PREFERRED_TYPE) ? { mimeType: PREFERRED_TYPE } : undefined;
          recorder = new recorderType(stream, options);
          recorder.ondataavailable = (event: BlobEvent) => {
            if (event.data.size > 0) {
              chunks.push(event.data);
            }
          };
          recorder.start(1000);
        }
        try {
          await buildGraph(TRANSCRIPTION_RATE);
        } catch {
          // A browser that cannot run a context at 16 kHz (or connect one to the microphone): the context
          // runs at the device's rate and the blocks are resampled as they arrive.
          teardownGraph();
          await buildGraph(undefined);
        }
      } catch (error) {
        release();
        throw new CaptureError("failed", error instanceof Error ? error.message : String(error), { cause: error });
      }
    },

    pause() {
      paused = true;
      if (recorder?.state === "recording") {
        recorder.pause();
      }
    },

    resume() {
      paused = false;
      if (recorder?.state === "paused") {
        recorder.resume();
      }
    },

    stop() {
      return new Promise<Blob>((resolve) => {
        const finish = (): void => {
          const type = recorder?.mimeType === undefined || recorder.mimeType === "" ? "audio/webm" : recorder.mimeType;
          release();
          resolve(new Blob(chunks, { type }));
        };
        if (recorder === null || recorder.state === "inactive") {
          finish();
          return;
        }
        recorder.onstop = finish;
        recorder.stop();
      });
    },

    cancel() {
      if (recorder !== null) {
        recorder.ondataavailable = null;
        recorder.onstop = null;
        if (recorder.state !== "inactive") {
          try {
            recorder.stop();
          } catch {
            // already stopping
          }
        }
      }
      chunks.length = 0;
      release();
    },
  };
}
