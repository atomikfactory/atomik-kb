// The inline player of a video or audio attachment (ADR-0032): what a `file` block with
// `view: "preview"` shows for a `video/*` or `audio/*` file. A native `<video controls>` or
// `<audio controls>` over a source from `openMediaSource` (the shell's streaming protocol, or a
// blob where there is none). Nothing is written to the page.
//
// The player never outlives the unlocked app: when the app locks (and the whole editor goes) or the
// block is removed, the element is paused and its `src` dropped, so the webview stops requesting
// bytes and holds no blob. A format the webview cannot decode (WebView2 plays MP4/H.264, WebM,
// MP3, AAC, Ogg and WAV; WebKitGTK depends on the GStreamer plugins installed) sends the block back
// to its card with a line that says so.

import { toCoreError } from "@atomik-kb/core-client";
import type { AttachmentInfo } from "@atomik-kb/core-client";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { t } from "../i18n";
import { useAppStore } from "../store-context";
import type { MediaKind } from "./attachment-nodes";
import { MediaTooLargeError, openMediaSource } from "./media-source";
import type { MediaSource } from "./media-source";

/** The state of the opening of a source. */
export type MediaLoad =
  | { status: "loading" }
  | { status: "ready"; source: MediaSource }
  | { status: "error"; message: string };

/** The line that says why a file is not played. */
export function describeMediaFailure(error: unknown): string {
  if (error instanceof MediaTooLargeError) {
    return t("media.error.tooLarge");
  }
  return t("media.error.unreadable", { message: toCoreError(error).message });
}

/** The line for a media element's error: code 4 is a format or a source the webview does not play. */
export function describeElementError(code: number | undefined): string {
  return code === 4 ? t("media.error.unsupported") : t("media.error.failed");
}

/**
 * Opens the source of `info` (a complete attachment) once per attachment and releases it when the
 * block goes. `null` for no `info`.
 */
export function useMediaSource(info: AttachmentInfo | null): MediaLoad | null {
  const mediaUrl = useAppStore((state) => state.attachmentMediaUrl);
  const read = useAppStore((state) => state.readAttachment);
  const id = info?.attachmentId ?? null;
  const mime = info?.mime ?? "";
  const size = info === null ? 0 : Number(info.size);
  const key = id === null ? null : `${id} ${String(size)}`;
  const [loaded, setLoaded] = useState<{ key: string; load: MediaLoad } | null>(null);

  useEffect(() => {
    if (id === null || key === null) {
      return;
    }
    let active = true;
    let opened: MediaSource | null = null;
    openMediaSource({ attachmentId: id, mime, size }, { mediaUrl, read }).then(
      (source) => {
        if (!active) {
          source.release();
          return;
        }
        opened = source;
        setLoaded({ key, load: { status: "ready", source } });
      },
      (error: unknown) => {
        if (active) {
          setLoaded({ key, load: { status: "error", message: describeMediaFailure(error) } });
        }
      },
    );
    return () => {
      active = false;
      opened?.release();
    };
  }, [id, key, mime, size, mediaUrl, read]);

  if (key === null) {
    return null;
  }
  return loaded?.key === key ? loaded.load : { status: "loading" };
}

export interface MediaPreviewProps {
  info: AttachmentInfo;
  kind: MediaKind;
  /** The block's menu button, shown next to the name. */
  menuButton: ReactNode;
  /** The file cannot be played: the block falls back to its card with this line. */
  onFailure: (message: string) => void;
}

/** Stops a media element: pauses it, drops its source and stops the fetching. */
function stop(element: HTMLMediaElement): void {
  element.pause();
  element.removeAttribute("src");
  element.load();
}

/** The preview of a video or audio attachment, or `null` after it reported through `onFailure` why it cannot play. */
export function MediaPreview({ info, kind, menuButton, onFailure }: MediaPreviewProps) {
  const load = useMediaSource(info);
  const unlocked = useAppStore((state) => state.phase === "ready");
  const element = useRef<HTMLMediaElement | null>(null);
  const attach = useCallback((node: HTMLMediaElement | null) => {
    element.current = node;
  }, []);
  const failure = load?.status === "error" ? load.message : null;
  // While the app is not unlocked the element has no source at all.
  const url = load?.status === "ready" && unlocked ? load.source.url : null;

  useEffect(() => {
    if (failure !== null) {
      onFailure(failure);
    }
  }, [failure, onFailure]);

  // When the source goes (the app locks) the element lets go of what it was playing; removing the
  // attribute alone would leave the old media playing.
  useEffect(() => {
    if (url === null && element.current !== null) {
      stop(element.current);
    }
  }, [url]);

  // When the block goes (removed, or the whole editor with the lock screen) the same.
  useEffect(() => {
    const current = element.current;
    return () => {
      if (current !== null) {
        stop(current);
      }
    };
  }, []);

  if (load?.status === "error") {
    return null;
  }
  const props = {
    ref: attach,
    className: "media-element",
    controls: true,
    preload: "metadata" as const,
    src: url ?? undefined,
    "aria-label": t("media.label", { name: info.name }),
    onError: () => {
      // Dropping the source on purpose also raises an error event: only a real source counts.
      const current = element.current;
      if (current !== null && current.getAttribute("src") !== null) {
        onFailure(describeElementError(current.error?.code));
      }
    },
  };
  return (
    <div className="media-preview" data-kind={kind}>
      <div className="media-bar">
        <span className="media-title" title={info.name}>
          {info.name}
        </span>
        {load === null || load.status === "loading" ? <span className="media-message">{t("media.loading")}</span> : null}
        {menuButton}
      </div>
      {kind === "video" ? <video {...props} playsInline /> : <audio {...props} />}
    </div>
  );
}
