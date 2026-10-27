// Decoding an audio or video file for the transcription (ADR-0037, `docs/architecture/ai.md` section
// 8.6): the webview's own decoder (`decodeAudioData`) on an `OfflineAudioContext` at 16 kHz, which gives
// the file's audio already at the rate the model wants; the channels are then mixed down to mono. Any
// format the webview plays can be transcribed (the audio of an MP4, a WebM recording, MP3, WAV...).

import { downmixAndResample, TRANSCRIPTION_RATE } from "@atomik-kb/core-client";

/** The file's audio could not be decoded (a format the webview does not know, or damaged data). */
export class AudioDecodeError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "AudioDecodeError";
  }
}

type OfflineContextConstructor = new (channels: number, length: number, sampleRate: number) => OfflineAudioContext;

/** 16 kHz mono samples of the audio in `bytes`. Takes the buffer of `bytes` (it is detached by the decoder). */
export async function decodeToMono16k(bytes: Uint8Array): Promise<Float32Array> {
  const Offline =
    (globalThis as { OfflineAudioContext?: OfflineContextConstructor }).OfflineAudioContext ??
    (globalThis as { webkitOfflineAudioContext?: OfflineContextConstructor }).webkitOfflineAudioContext;
  if (Offline === undefined) {
    throw new AudioDecodeError("this webview cannot decode audio");
  }
  const whole = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength;
  const data = whole ? bytes.buffer : bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  let decoded: AudioBuffer;
  try {
    decoded = await new Offline(1, 1, TRANSCRIPTION_RATE).decodeAudioData(data as ArrayBuffer);
  } catch (error) {
    throw new AudioDecodeError(error instanceof Error ? error.message : "the audio could not be decoded", { cause: error });
  }
  const channels: Float32Array[] = [];
  for (let channel = 0; channel < decoded.numberOfChannels; channel += 1) {
    channels.push(decoded.getChannelData(channel));
  }
  return downmixAndResample(channels, decoded.sampleRate, TRANSCRIPTION_RATE);
}
