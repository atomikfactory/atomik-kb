// The Upload tab of the cover picker (ADR-0040): a picture from a file, a drop or a paste, with the
// icon picker's flow (`IconUploadTab`) and the same accepted types, but not scaled down to an icon's
// size: a picture larger than 2560 pixels on a side is redrawn at that size (icons/resize-image.ts), a
// smaller one is stored as it is. It becomes an attachment of the page's space, and the cover becomes
// `attachment:<id>`. A refusal at any step (not an image, unreadable, the core's own) is shown in the
// picker and leaves the cover as it was.

import { toCoreError } from "@atomik-kb/core-client";
import type { SpaceId } from "@atomik-kb/core-client";
import { useContext, useRef, useState } from "react";
import type { ClipboardEvent, DragEvent } from "react";

import { t } from "../i18n";
import { imageCoverValue } from "../icons/cover-grammar";
import type { ImageResizer } from "../icons/resize-image";
import { StoreContext } from "../store-context";

export interface CoverUploadTabProps {
  /** The space of the page: where the picture is stored. */
  spaceId: SpaceId;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  setError: (message: string | null) => void;
  resize: ImageResizer;
  /** Stores the cover (`attachment:<id>`) and closes the picker. */
  apply: (cover: string) => Promise<void>;
}

/** The first file of a list (a drop, a paste, the file picker), if there is one. */
function firstFile(files: FileList | readonly File[] | null | undefined): File | null {
  return files === null || files === undefined ? null : (Array.from(files)[0] ?? null);
}

export function CoverUploadTab({ spaceId, busy, setBusy, setError, resize, apply }: CoverUploadTabProps) {
  const store = useContext(StoreContext);
  const input = useRef<HTMLInputElement>(null);
  const [dragging, setDragging] = useState(false);
  const [progress, setProgress] = useState<{ sent: number; total: number | undefined } | null>(null);

  async function take(file: File): Promise<void> {
    if (busy || store === null) {
      return;
    }
    setError(null);
    if (!file.type.toLowerCase().startsWith("image/")) {
      setError(t("icon.upload.error.notImage"));
      return;
    }
    setBusy(true);
    setProgress({ sent: 0, total: undefined });
    let picture: File;
    try {
      picture = await resize(file);
    } catch {
      setError(t("icon.upload.error.decode"));
      setBusy(false);
      setProgress(null);
      return;
    }
    let attachmentId: string;
    try {
      const info = await store.getState().uploadAttachment(spaceId, picture, (sent, total) => {
        setProgress({ sent, total });
      });
      attachmentId = info.attachmentId;
    } catch (failure) {
      setError(t("icon.upload.error", { message: toCoreError(failure).message }));
      setBusy(false);
      setProgress(null);
      return;
    }
    setProgress(null);
    await apply(imageCoverValue(attachmentId));
  }

  function takeFirst(files: FileList | readonly File[] | null | undefined): boolean {
    const file = firstFile(files);
    if (file === null) {
      return false;
    }
    void take(file);
    return true;
  }

  function onDrop(event: DragEvent<HTMLDivElement>): void {
    event.preventDefault();
    setDragging(false);
    takeFirst(event.dataTransfer.files);
  }

  function onPaste(event: ClipboardEvent<HTMLDivElement>): void {
    if (takeFirst(event.clipboardData.files)) {
      event.preventDefault();
    }
  }

  return (
    <div
      className={dragging ? "icon-upload is-dragging" : "icon-upload"}
      onDragOver={(event) => {
        event.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => {
        setDragging(false);
      }}
      onDrop={onDrop}
      onPaste={onPaste}
    >
      <p className="icon-upload-drop">{t("icon.upload.drop")}</p>
      <button
        type="button"
        className="button"
        disabled={busy}
        data-autofocus=""
        onClick={() => {
          input.current?.click();
        }}
      >
        {t("icon.upload.choose")}
      </button>
      <input
        ref={input}
        type="file"
        accept="image/*"
        hidden
        tabIndex={-1}
        onChange={(event) => {
          takeFirst(event.target.files);
          // The same file can be chosen again.
          event.target.value = "";
        }}
      />
      <p className="muted icon-upload-hint">{t("cover.upload.hint")}</p>
      {progress !== null && (
        <p className="icon-upload-status" role="status">
          <span>{t("icon.upload.uploading")}</span>
          <progress
            className="icon-upload-bar"
            aria-label={t("icon.upload.progress")}
            max={progress.total ?? 1}
            value={progress.total === undefined ? undefined : Math.min(progress.sent, progress.total)}
          />
        </p>
      )}
    </div>
  );
}
