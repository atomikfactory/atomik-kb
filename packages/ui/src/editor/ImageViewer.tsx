// The full-size viewer of an image (ADR-0028, item 6): a modal dialog over the whole window
// with the picture fitted to it, a click that toggles the actual size (scrollable), Arrow
// Left and Right to step through the page's images in document order, and Escape, the close
// button or a click on the backdrop to close it (focus goes back to where it came from). It
// reads the bytes like the block does (a `blob:` URL that lives while the picture is shown),
// so nothing new is needed in the core.

import { MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { useId, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode, RefObject } from "react";

import { ChevronIcon } from "../components/icons";
import { Modal } from "../components/Modal";
import { formatBytes } from "../format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { attachmentIdOf, IMAGE_NODE, parseCaption } from "./attachment-nodes";
import { useAttachment, useImagePreview } from "./AttachmentViews";

/** An image of the page, as the viewer lists it. */
export interface ViewerImage {
  /** Where its block is in the document. */
  pos: number;
  attachmentId: string;
  caption: string | null;
  alt: string;
}

/** The images of the page with an attachment, in document order (columns and toggles included). */
export function pageImages(editor: Editor): ViewerImage[] {
  const images: ViewerImage[] = [];
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name !== IMAGE_NODE) {
      return true;
    }
    const attachmentId = attachmentIdOf(node);
    if (attachmentId !== null) {
      images.push({
        pos,
        attachmentId,
        caption: parseCaption(node.attrs.caption),
        alt: typeof node.attrs.alt === "string" ? node.attrs.alt : "",
      });
    }
    return false;
  });
  return images;
}

export interface ImageViewerProps {
  images: readonly ViewerImage[];
  /** The index in `images` of the picture to show first. */
  start: number;
  /** Gets focus back when the viewer closes. */
  returnFocus: RefObject<HTMLElement | null>;
  onClose: () => void;
}

/** A modal view of one image of the page at a time. */
export function ImageViewer({ images, start, returnFocus, onClose }: ImageViewerProps) {
  const labelId = useId();
  const stageRef = useRef<HTMLDivElement>(null);
  const [index, setIndex] = useState(start);
  const [actualSize, setActualSize] = useState(false);
  const [brokenUrl, setBrokenUrl] = useState<string | null>(null);
  const current = images[index];
  const lookup = useAttachment(current?.attachmentId ?? null);
  const canRead = useAppStore((state) => state.attachmentCapabilities.read);
  const info = lookup.status === "found" ? lookup.info : null;
  const previewable = info !== null && info.availability === "Complete" && canRead && Number(info.size) <= MAX_PREVIEW_BYTES;
  const preview = useImagePreview(previewable ? info : null);

  if (current === undefined) {
    return null;
  }

  const alt = current.alt;
  const label = current.caption ?? (alt === "" ? null : alt) ?? info?.name ?? t("attachment.image.viewer.untitled");

  function step(by: number): void {
    if (images.length < 2) {
      return;
    }
    setIndex((at) => (at + by + images.length) % images.length);
    setActualSize(false);
  }

  let shown = false;
  let content: ReactNode;
  if (info === null) {
    content = (
      <p className="image-viewer-message" role="status">
        {lookup.status === "missing"
          ? t("attachment.deleted.image")
          : lookup.status === "error"
            ? t("attachment.error.read", { message: lookup.message })
            : t("attachment.loading")}
      </p>
    );
  } else if (!previewable) {
    content = (
      <p className="image-viewer-message" role="status">
        {t("attachment.preview.cannotShow", { size: formatBytes(info.size) })}
      </p>
    );
  } else if (preview === null || preview.status === "loading") {
    content = (
      <p className="image-viewer-message" role="status">
        {t("attachment.preview.loading")}
      </p>
    );
  } else if (preview.status === "error") {
    content = (
      <p className="image-viewer-message" role="alert">
        {t("attachment.error.read", { message: preview.message })}
      </p>
    );
  } else if (brokenUrl === preview.url) {
    content = (
      <p className="image-viewer-message" role="status">
        {t("attachment.preview.cannotShow", { size: formatBytes(info.size) })}
      </p>
    );
  } else {
    const url = preview.url;
    shown = true;
    content = (
      <img
        className={actualSize ? "image-viewer-img is-actual" : "image-viewer-img"}
        src={url}
        alt={alt}
        draggable={false}
        onClick={() => {
          setActualSize((actual) => !actual);
        }}
        onError={() => {
          setBrokenUrl(url);
        }}
      />
    );
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
      return;
    }
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      step(event.key === "ArrowLeft" ? -1 : 1);
    }
  }

  const many = images.length > 1;
  return (
    <Modal labelledBy={labelId} onDismiss={onClose} className="image-viewer" initialFocus={stageRef} returnFocus={returnFocus}>
      <div className="image-viewer-body" onKeyDown={onKeyDown}>
        <div
          ref={stageRef}
          className={actualSize && shown ? "image-viewer-stage is-scrollable" : "image-viewer-stage"}
          tabIndex={-1}
          onClick={(event) => {
            if (event.target === event.currentTarget) {
              onClose();
            }
          }}
        >
          {content}
        </div>
        <div className="image-viewer-bar">
          <p id={labelId} className="image-viewer-caption">
            {label}
          </p>
          {many && (
            <span className="image-viewer-position" aria-live="polite">
              {t("attachment.image.viewer.position", { index: index + 1, total: images.length })}
            </span>
          )}
          {shown && (
            <button
              type="button"
              className="button image-viewer-zoom"
              aria-pressed={actualSize}
              onClick={() => {
                setActualSize((actual) => !actual);
              }}
            >
              {t("attachment.image.viewer.actualSize")}
            </button>
          )}
          <button type="button" className="button" onClick={onClose}>
            {t("attachment.image.viewer.close")}
          </button>
        </div>
        {many && (
          <>
            <button
              type="button"
              className="icon-button image-viewer-step is-previous"
              aria-label={t("attachment.image.viewer.previous")}
              title={t("attachment.image.viewer.previous")}
              onClick={() => {
                step(-1);
              }}
            >
              <ChevronIcon />
            </button>
            <button
              type="button"
              className="icon-button image-viewer-step is-next"
              aria-label={t("attachment.image.viewer.next")}
              title={t("attachment.image.viewer.next")}
              onClick={() => {
                step(1);
              }}
            >
              <ChevronIcon />
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}
