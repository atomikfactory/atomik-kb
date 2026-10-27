// The cover banner of a page (ADR-0040), above its header and as wide as the page (ADR-0035): a picture of
// the page's space (`attachment:<id>`, read through the attachment cache the icons use, drawn with
// `object-fit: cover` and `object-position: 50% <coverY>%`), a colour of the palette or a bundled gradient
// (defined in `app.css`). A picture that cannot be shown yet (not downloaded, unreadable, not an image), a
// gradient this version does not bundle and any value that follows no form are a neutral banner, never the
// raw string.
//
// On hover or focus the banner offers "Change cover", "Reposition" (a picture only) and "Remove". Reposition
// is dragging the picture up and down (or the arrow keys: the overlay is a slider), then "Save position" (one
// write) or "Cancel". A read-only page (in the trash, or printing) shows the banner and nothing to press.

import { useContext, useId, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent as ReactPointerEvent } from "react";

import { t } from "../i18n";
import { coverPosition, isBundledGradient, parseCover } from "../icons/cover-grammar";
import { MAX_COVER_BYTES, useIconImage } from "../icons/icon-images";
import type { AppStore } from "../store";
import { StoreContext } from "../store-context";

/** The vertical distance, in pixels, a full 0 to 100 sweep covers when the layout cannot be measured (no layout in tests). */
export const FALLBACK_OVERFLOW_PX = 200;

/** What one press of an arrow key and of Page Up or Page Down move the picture by, in percent. */
const STEP = 5;
const BIG_STEP = 20;

export interface PageCoverProps {
  /** The cover string of the page (`attachment:<id>`, `color:<name>` or `gradient:<name>`). */
  cover: string;
  /** The picture's vertical position in percent (`undefined`: the default 50). */
  coverY: number | undefined;
  /** Shows the banner without anything to press. */
  readOnly: boolean;
  /** "Change cover": opens the picker from this button. */
  onChange: (opener: HTMLElement) => void;
  /**
   * "Save position": writes the new position (once). When the core refuses it rejects, after the caller said so,
   * and the banner stays in reposition mode.
   */
  onSavePosition: (y: number) => Promise<void>;
  /** "Remove": removes the cover. Rejects when the core refuses, after the caller said so. */
  onRemove: () => Promise<void>;
}

/** The picture of an uploaded cover: its `blob:` URL once the bytes are read, nothing until then. */
function CoverImage({
  store,
  attachmentId,
  y,
  onLoaded,
}: {
  store: AppStore;
  attachmentId: string;
  y: number;
  onLoaded: (size: { width: number; height: number }) => void;
}) {
  const url = useIconImage(store, attachmentId, MAX_COVER_BYTES);
  // A picture the webview cannot decode is the neutral banner too.
  const [broken, setBroken] = useState<string | null>(null);
  if (url === null || url === broken) {
    return null;
  }
  return (
    <img
      className="page-cover-image"
      src={url}
      alt=""
      draggable={false}
      style={{ objectPosition: `50% ${String(y)}%` }}
      onLoad={(event) => {
        onLoaded({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight });
      }}
      onError={() => {
        setBroken(url);
      }}
    />
  );
}

export function PageCover({ cover, coverY, readOnly, onChange, onSavePosition, onRemove }: PageCoverProps) {
  const store = useContext(StoreContext);
  const hintId = useId();
  const parsed = parseCover(cover);
  const banner = useRef<HTMLDivElement>(null);
  const natural = useRef<{ width: number; height: number } | null>(null);
  const drag = useRef<{ startY: number; startPosition: number } | null>(null);
  // The position being chosen while repositioning; `null` when not repositioning.
  const [position, setPosition] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const saved = coverPosition(coverY);
  const shown = position ?? saved;
  const repositioning = position !== null && !readOnly;
  const isImage = parsed?.kind === "image";

  /** How far, in pixels, the picture can move up and down inside the banner. */
  function overflowPx(): number {
    const box = banner.current;
    const size = natural.current;
    if (box === null || size === null || box.clientWidth <= 0 || box.clientHeight <= 0 || size.width <= 0 || size.height <= 0) {
      return FALLBACK_OVERFLOW_PX;
    }
    const scale = Math.max(box.clientWidth / size.width, box.clientHeight / size.height);
    return Math.max(1, size.height * scale - box.clientHeight);
  }

  const clamp = (y: number): number => Math.min(100, Math.max(0, Math.round(y)));

  function start(event: ReactPointerEvent<HTMLDivElement>): void {
    if (event.button !== 0 && event.button !== undefined) {
      return;
    }
    drag.current = { startY: event.clientY, startPosition: position ?? saved };
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // No pointer capture (an environment without it): the move events still reach the overlay while over it.
    }
  }

  function move(event: ReactPointerEvent<HTMLDivElement>): void {
    const from = drag.current;
    if (from === null) {
      return;
    }
    // Dragging the picture down shows more of its top: the position goes down.
    setPosition(clamp(from.startPosition - ((event.clientY - from.startY) / overflowPx()) * 100));
  }

  function end(): void {
    drag.current = null;
  }

  function key(event: KeyboardEvent<HTMLDivElement>): void {
    let next: number;
    switch (event.key) {
      case "ArrowUp":
        next = shown - STEP;
        break;
      case "ArrowDown":
        next = shown + STEP;
        break;
      case "PageUp":
        next = shown - BIG_STEP;
        break;
      case "PageDown":
        next = shown + BIG_STEP;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = 100;
        break;
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        setPosition(null);
        return;
      default:
        return;
    }
    event.preventDefault();
    setPosition(clamp(next));
  }

  async function save(): Promise<void> {
    if (position === null || busy) {
      return;
    }
    if (position === saved) {
      setPosition(null);
      return;
    }
    setBusy(true);
    try {
      await onSavePosition(position);
      setPosition(null);
    } catch {
      // The caller said what went wrong; the position stays as chosen, to save again or cancel.
    } finally {
      setBusy(false);
    }
  }

  async function remove(): Promise<void> {
    if (busy) {
      return;
    }
    setBusy(true);
    try {
      await onRemove();
    } catch {
      // The caller said what went wrong; the cover stays.
    } finally {
      setBusy(false);
    }
  }

  const kind = parsed?.kind ?? "none";
  const named =
    parsed?.kind === "color"
      ? { "data-cover-color": parsed.color }
      : parsed?.kind === "gradient" && isBundledGradient(parsed.name)
        ? { "data-cover-gradient": parsed.name }
        : {};

  return (
    <div
      ref={banner}
      className={repositioning ? "page-cover is-repositioning" : "page-cover"}
      role="region"
      aria-label={t("cover.banner")}
      data-cover-kind={kind}
      {...named}
    >
      {parsed?.kind === "image" && store !== null && (
        <CoverImage
          store={store}
          attachmentId={parsed.attachmentId}
          y={shown}
          onLoaded={(size) => {
            natural.current = size;
          }}
        />
      )}
      {repositioning && (
        <div
          className="page-cover-drag"
          role="slider"
          tabIndex={0}
          aria-label={t("cover.position")}
          aria-orientation="vertical"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={shown}
          aria-describedby={hintId}
          data-autofocus=""
          ref={(element) => {
            element?.focus({ preventScroll: true });
          }}
          onPointerDown={start}
          onPointerMove={move}
          onPointerUp={end}
          onPointerCancel={end}
          onKeyDown={key}
        >
          <p className="page-cover-hint" id={hintId}>
            {t("cover.positionHint")}
          </p>
        </div>
      )}
      {!readOnly && (
        <div className="page-cover-actions">
          {repositioning ? (
            <>
              <button
                type="button"
                className="button button-primary"
                disabled={busy}
                onClick={() => {
                  void save();
                }}
              >
                {t("cover.savePosition")}
              </button>
              <button
                type="button"
                className="button"
                disabled={busy}
                onClick={() => {
                  setPosition(null);
                }}
              >
                {t("cover.cancelPosition")}
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                className="button"
                aria-haspopup="dialog"
                disabled={busy}
                onClick={(event) => {
                  onChange(event.currentTarget);
                }}
              >
                {t("cover.change")}
              </button>
              {isImage && (
                <button
                  type="button"
                  className="button"
                  disabled={busy}
                  onClick={() => {
                    setPosition(saved);
                  }}
                >
                  {t("cover.reposition")}
                </button>
              )}
              <button
                type="button"
                className="button"
                disabled={busy}
                onClick={() => {
                  void remove();
                }}
              >
                {t("cover.remove")}
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
