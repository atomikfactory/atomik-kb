// The photo gallery block (ADR-0030): several pictures in a grid. Each picture is an attachment
// shown from its bytes like an image block's (a `blob:` URL that lives as long as the tile shows
// it; a picture above the 32 MiB preview limit, or one that is not here yet, is a placeholder
// with its name), read only once the tile is near the window. A click opens the full-size viewer
// (ImageViewer.tsx), which steps through this gallery's pictures. On a page that can be edited a
// small toolbar adds pictures (a file chooser that takes several, and the paste and drop of the
// page, see gallery-uploads.ts) and sets the column count; each picture has a caption field, a
// button to remove it and buttons to move it earlier and later, and can be dragged to another
// place. Every change is one write of the block's `items` (or `columns`) attribute.

import { MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import { NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { ClipboardEvent, ReactNode, RefObject } from "react";

import { PlusIcon, TrashIcon } from "../components/icons";
import { formatBytes } from "../format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { BLOCK_ID_ATTRIBUTE } from "./schema";
import { useAttachment, useImagePreview } from "./AttachmentViews";
import { CAPTION_MAX_LENGTH } from "./attachment-nodes";
import {
  GALLERY_COLUMNS,
  galleryItemsOf,
  MAX_GALLERY_ITEMS,
  moveItem,
  readGalleryColumns,
  withCaption,
} from "./gallery-nodes";
import type { GalleryItem } from "./gallery-nodes";
import { addImagesToGallery, canAddToGallery, galleryTransfers } from "./gallery-uploads";
import { ImageViewer } from "./ImageViewer";
import type { ViewerImage } from "./ImageViewer";

/** The drag type of a picture being moved inside a gallery: only these drags are the gallery's own. */
export const GALLERY_DRAG_TYPE = "application/x-atomik-gallery-item";

/**
 * The node view's `stopEvent` (what ProseMirror must leave alone): the drags of the pictures
 * inside a gallery, and the events of the controls in it (inputs and buttons), as Tiptap's own
 * default does. Everything else (a click that selects the block, copy and paste, a drop of files
 * from outside) is the editor's.
 */
export function galleryStopsEvent(event: Event): boolean {
  const target = event.target;
  if (!(target instanceof Element) || target.closest(".gallery-block") === null) {
    return false;
  }
  if (event.type.startsWith("drag") || event.type === "drop") {
    if (target.closest(".gallery-thumb-wrap") !== null && (event.type === "dragstart" || event.type === "dragend")) {
      return true;
    }
    const types = (event as globalThis.DragEvent).dataTransfer?.types;
    return types !== undefined && Array.from(types).includes(GALLERY_DRAG_TYPE);
  }
  return target.closest("input, button, textarea, select") !== null;
}

/** True once the element has been near the window (or where nothing can tell: always). */
function useNearViewport(ref: RefObject<HTMLElement | null>): boolean {
  const [near, setNear] = useState(typeof IntersectionObserver === "undefined");
  useEffect(() => {
    const element = ref.current;
    if (near || element === null) {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setNear(true);
          observer.disconnect();
        }
      },
      { rootMargin: "400px" },
    );
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [near, ref]);
  return near;
}

interface ThumbProps {
  item: GalleryItem;
  /** 1-based place in the gallery, for the names of the controls. */
  n: number;
  onOpen: (opener: HTMLElement) => void;
}

/** The picture of one tile, or a placeholder saying why it is not shown. */
function Thumb({ item, n, onOpen }: ThumbProps) {
  const holder = useRef<HTMLButtonElement>(null);
  // Print mode (print.ts) reads every picture: paper has no scrolling.
  const printing = useAppStore((state) => state.printing);
  const near = useNearViewport(holder) || printing;
  const lookup = useAttachment(item.attachmentId);
  const canRead = useAppStore((state) => state.attachmentCapabilities.read);
  const info = lookup.status === "found" ? lookup.info : null;
  const previewable = info !== null && info.availability === "Complete" && canRead && Number(info.size) <= MAX_PREVIEW_BYTES;
  const preview = useImagePreview(near && previewable ? info : null);
  const [brokenUrl, setBrokenUrl] = useState<string | null>(null);

  let body: ReactNode;
  let shown = false;
  if (lookup.status === "missing") {
    body = <Placeholder icon="🚫" text={t("attachment.deleted.image")} />;
  } else if (lookup.status === "error") {
    body = <Placeholder icon="⚠️" text={t("attachment.error.read", { message: lookup.message })} />;
  } else if (info === null) {
    body = <Placeholder icon="⏳" text={t("attachment.loading")} />;
  } else if (info.availability !== "Complete") {
    body = <Placeholder icon="⏳" name={info.name} text={t("attachment.transfer.waiting")} />;
  } else if (!canRead) {
    body = <Placeholder icon="🖼️" name={info.name} text={t("attachment.preview.unavailable", { size: formatBytes(info.size) })} />;
  } else if (!previewable) {
    body = <Placeholder icon="🖼️" name={info.name} text={t("attachment.preview.tooLarge", { size: formatBytes(info.size) })} />;
  } else if (preview === null || preview.status === "loading") {
    body = <Placeholder icon="⏳" name={info.name} text={t("attachment.preview.loading")} />;
  } else if (preview.status === "error") {
    body = <Placeholder icon="⚠️" name={info.name} text={t("attachment.error.read", { message: preview.message })} />;
  } else if (brokenUrl === preview.url) {
    body = <Placeholder icon="🖼️" name={info.name} text={t("attachment.preview.cannotShow", { size: formatBytes(info.size) })} />;
  } else {
    const url = preview.url;
    shown = true;
    body = (
      <img
        className="gallery-img"
        src={url}
        alt={item.caption ?? ""}
        draggable={false}
        onError={() => {
          setBrokenUrl(url);
        }}
      />
    );
  }

  // What print mode waits for: a picture whose bytes are on their way.
  const loading = lookup.status === "loading" || (previewable && (preview === null || preview.status === "loading"));

  return (
    <button
      ref={holder}
      type="button"
      data-state={loading ? "loading" : undefined}
      className={shown ? "gallery-thumb" : "gallery-thumb is-placeholder"}
      aria-label={t("gallery.open", { n })}
      title={t("gallery.open", { n })}
      onClick={(event) => {
        onOpen(event.currentTarget);
      }}
    >
      {body}
    </button>
  );
}

function Placeholder({ icon, name, text }: { icon: string; name?: string; text: string }) {
  return (
    <span className="gallery-placeholder">
      <span className="gallery-placeholder-icon" aria-hidden="true">
        {icon}
      </span>
      {name !== undefined && <span className="gallery-placeholder-name">{name}</span>}
      <span className="gallery-placeholder-text">{text}</span>
    </span>
  );
}

/** The caption under a picture: a text field on a page that can be edited (Enter or leaving it saves, Escape undoes), else text. */
function Caption({ item, n, editable, onCommit }: { item: GalleryItem; n: number; editable: boolean; onCommit: (text: string) => void }) {
  const caption = item.caption ?? "";
  if (!editable) {
    return caption === "" ? null : <figcaption className="gallery-caption">{caption}</figcaption>;
  }
  return (
    <input
      // A new key when the stored caption changes, so that an edit from elsewhere replaces the draft.
      key={caption}
      className="gallery-caption-edit"
      type="text"
      defaultValue={caption}
      maxLength={CAPTION_MAX_LENGTH}
      aria-label={t("gallery.caption.label", { n })}
      placeholder={t("gallery.caption.placeholder")}
      autoComplete="off"
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) {
          return;
        }
        if (event.key === "Enter") {
          event.preventDefault();
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          event.currentTarget.value = caption;
          event.currentTarget.blur();
        }
      }}
      onBlur={(event) => {
        if (event.currentTarget.value !== caption) {
          onCommit(event.currentTarget.value);
        }
      }}
    />
  );
}

/** A file chooser that takes several pictures: the hidden input behind the "Add images" button. */
function usePicker(onPick: (files: File[]) => void) {
  const input = useRef<HTMLInputElement>(null);
  const element = (
    <input
      ref={input}
      className="gallery-file-input"
      type="file"
      accept="image/*"
      multiple
      hidden
      tabIndex={-1}
      aria-hidden="true"
      data-testid="gallery-file-input"
      onChange={(event) => {
        const files = Array.from(event.currentTarget.files ?? []);
        event.currentTarget.value = ""; // the same pictures can be chosen again
        if (files.length > 0) {
          onPick(files);
        }
      }}
    />
  );
  return {
    element,
    open: () => {
      input.current?.click();
    },
  };
}

/** The `gallery` block. */
export function GalleryView(props: ReactNodeViewProps) {
  const { node, editor, getPos, updateAttributes } = props;
  const items = useMemo(() => galleryItemsOf(node), [node]);
  const columns = readGalleryColumns(node.attrs.columns);
  const blockId = typeof node.attrs[BLOCK_ID_ATTRIBUTE] === "string" ? node.attrs[BLOCK_ID_ATTRIBUTE] : "";
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const canAdd = editable && blockId !== "" && canAddToGallery(editor);
  const store = galleryTransfers(editor);
  const transfers = useSyncExternalStore(store.subscribe, () => store.of(blockId));
  const [viewer, setViewer] = useState<{ images: ViewerImage[]; start: number } | null>(null);
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  const [dragOver, setDragOver] = useState<number | null>(null);
  const viewerFocus = useRef<HTMLElement | null>(null);
  const full = items.length >= MAX_GALLERY_ITEMS;

  const picker = usePicker((files) => {
    addImagesToGallery(editor, blockId, files);
  });

  function write(next: GalleryItem[]): void {
    updateAttributes({ items: next });
  }

  function openViewer(start: number, opener: HTMLElement): void {
    const at = getPos() ?? 0;
    viewerFocus.current = opener;
    setViewer({
      images: items.map((item) => ({ pos: at, attachmentId: item.attachmentId, caption: item.caption ?? null, alt: "" })),
      start,
    });
  }

  /** Pictures pasted while focus is in the block (the page's own paste handler sees the selected block). */
  function onPaste(event: ClipboardEvent<HTMLElement>): void {
    const files = Array.from(event.clipboardData.files);
    if (!canAdd || files.length === 0 || event.clipboardData.getData("text/plain") !== "") {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    addImagesToGallery(editor, blockId, files);
  }

  function endDrag(): void {
    setDragFrom(null);
    setDragOver(null);
  }

  const dragging = dragFrom !== null;
  const tiles = items.map((item, index) => {
    const n = index + 1;
    const occurrence = items.slice(0, index).filter((other) => other.attachmentId === item.attachmentId).length;
    return (
      <li
        key={`${item.attachmentId}:${String(occurrence)}`}
        className={["gallery-item", dragFrom === index ? "is-dragged" : "", dragOver === index && dragFrom !== index ? "is-drop-target" : ""]
          .filter((name) => name !== "")
          .join(" ")}
        onDragOver={(event) => {
          if (dragging && event.dataTransfer.types.includes(GALLERY_DRAG_TYPE)) {
            event.preventDefault();
            event.dataTransfer.dropEffect = "move";
            setDragOver(index);
          }
        }}
        onDrop={(event) => {
          if (dragFrom !== null && event.dataTransfer.types.includes(GALLERY_DRAG_TYPE)) {
            event.preventDefault();
            event.stopPropagation();
            const from = dragFrom;
            endDrag();
            if (from !== index) {
              write(moveItem(items, from, index));
            }
          }
        }}
      >
        <figure className="gallery-figure">
          <div
            className="gallery-thumb-wrap"
            draggable={editable}
            onDragStart={
              editable
                ? (event) => {
                    event.dataTransfer.setData(GALLERY_DRAG_TYPE, String(index));
                    event.dataTransfer.effectAllowed = "move";
                    setDragFrom(index);
                  }
                : undefined
            }
            onDragEnd={endDrag}
          >
            <Thumb
              item={item}
              n={n}
              onOpen={(opener) => {
                openViewer(index, opener);
              }}
            />
            {editable && (
              <span className="gallery-item-tools">
                <button
                  type="button"
                  className="icon-button gallery-tool"
                  aria-label={t("gallery.moveEarlier", { n })}
                  title={t("gallery.moveEarlier", { n })}
                  disabled={index === 0}
                  onClick={() => {
                    write(moveItem(items, index, index - 1));
                  }}
                >
                  <span aria-hidden="true">‹</span>
                </button>
                <button
                  type="button"
                  className="icon-button gallery-tool"
                  aria-label={t("gallery.moveLater", { n })}
                  title={t("gallery.moveLater", { n })}
                  disabled={index === items.length - 1}
                  onClick={() => {
                    write(moveItem(items, index, index + 1));
                  }}
                >
                  <span aria-hidden="true">›</span>
                </button>
                <button
                  type="button"
                  className="icon-button gallery-tool"
                  aria-label={t("gallery.remove", { n })}
                  title={t("gallery.remove", { n })}
                  onClick={() => {
                    write(items.filter((_, at) => at !== index));
                  }}
                >
                  <TrashIcon />
                </button>
              </span>
            )}
          </div>
          <Caption
            item={item}
            n={n}
            editable={editable}
            onCommit={(text) => {
              write(items.map((other, at) => (at === index ? withCaption(other, text) : other)));
            }}
          />
        </figure>
      </li>
    );
  });

  return (
    <NodeViewWrapper
      className="attachment-block gallery-block"
      data-gallery-block={blockId}
      data-columns={String(columns)}
      role="group"
      aria-label={t("gallery.label")}
      onPaste={onPaste}
    >
      {editable && (
        <div className="gallery-toolbar" role="toolbar" aria-label={t("gallery.toolbar")}>
          {canAdd && (
            <button type="button" className="button gallery-add" aria-label={t("gallery.addLabel")} disabled={full} onClick={picker.open}>
              <PlusIcon />
              <span>{t("gallery.add")}</span>
            </button>
          )}
          <span className="gallery-columns" role="group" aria-label={t("gallery.columns.group")}>
            {GALLERY_COLUMNS.map((count) => (
              <button
                key={count}
                type="button"
                className="icon-button gallery-columns-button"
                aria-label={t("gallery.columns", { n: count })}
                title={t("gallery.columns", { n: count })}
                aria-pressed={columns === count}
                onClick={() => {
                  if (count !== columns) {
                    updateAttributes({ columns: count });
                  }
                }}
              >
                {count}
              </button>
            ))}
          </span>
        </div>
      )}
      {items.length === 0 ? (
        <p className="gallery-empty muted">{editable ? t("gallery.empty") : t("gallery.empty.readOnly")}</p>
      ) : (
        <ul className="gallery-grid" role="list" style={{ gridTemplateColumns: `repeat(${String(columns)}, minmax(0, 1fr))` }}>
          {tiles}
        </ul>
      )}
      {transfers.length > 0 && (
        <ul className="gallery-transfers" role="list">
          {transfers.map((transfer) => (
            <li key={transfer.id} className={transfer.error === null ? "gallery-transfer" : "gallery-transfer is-error"}>
              {transfer.error === null ? (
                <>
                  <span className="gallery-transfer-name">{t("gallery.uploading", { name: transfer.name })}</span>
                  <progress
                    className="attachment-upload-bar"
                    max={Math.max(1, transfer.total ?? 1)}
                    value={transfer.total === null ? undefined : Math.min(transfer.sent, Math.max(1, transfer.total))}
                    aria-label={t("gallery.uploading", { name: transfer.name })}
                  />
                </>
              ) : (
                <>
                  <span className="gallery-transfer-name" role="alert">
                    {transfer.error}
                  </span>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      store.remove(blockId, transfer.id);
                    }}
                  >
                    {t("attachment.dismiss")}
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {picker.element}
      {viewer !== null && (
        <ImageViewer
          images={viewer.images}
          start={viewer.start}
          returnFocus={viewerFocus}
          onClose={() => {
            setViewer(null);
          }}
        />
      )}
    </NodeViewWrapper>
  );
}
