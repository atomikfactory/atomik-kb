// React views of the attachment blocks (milestone-05.md, "UI"): an image shown from its
// bytes (a `blob:` URL that lives exactly as long as the view shows it), a file card with an
// icon for its type, its name and size, and the cards for what can go wrong: too large to
// preview, unreadable, deleted. Each block has a menu: Save as, Open, Copy as Markdown, the
// alt text of an image, Remove. An image also has a layout (ADR-0028): a width in percent of
// its column with a resize handle on each edge, an alignment and a caption, a toolbar with
// them, and a full-size viewer (ImageViewer.tsx).
//
// Since Milestone 8 (ADR-0023) an attachment's content may not be on this device yet
// (`AttachmentInfo.availability`): while it downloads the block shows how far it got, while
// the device that added it has not uploaded it the block says so, and on a device that
// downloads on demand it offers "Download". None of them reads the bytes, and Save as and
// Open wait for the whole file; an image shows itself once its attachment is complete.

import { CHUNK_SIZE, MAX_PREVIEW_BYTES, toCoreError } from "@atomik-kb/core-client";
import type { AttachmentInfo } from "@atomik-kb/core-client";
import type { Node as PmNode } from "@tiptap/pm/model";
import { NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";

import { writeClipboard } from "../clipboard";
import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { MoreIcon } from "../components/icons";
import { InlineEdit } from "../components/InlineEdit";
import { Menu } from "../components/Menu";
import type { MenuItem } from "../components/Menu";
import { TextDialog } from "../components/TextDialog";
import { formatBytes } from "../format";
import { t } from "../i18n";
import type { AttachmentLookup } from "../store-attachments";
import { useAppStore } from "../store-context";
import {
  attachmentIdOf,
  attachmentMarkdown,
  IMAGE_NODE,
  isPdfFile,
  mediaKindOf,
  normalizeCaption,
  parseAlign,
  parseCaption,
  parseFileView,
  parseWidth,
  parseWidthPercent,
} from "./attachment-nodes";
import type { ImageAlign } from "./attachment-nodes";
import { useAiCapabilities } from "./ai/availability";
import { useEditorAi } from "./ai/EditorAi";
import { TranscribeStatus } from "./ai/TranscribeStatus";
import { CaptionEdit, clampPercent, ImageToolbar, ResizeHandle } from "./ImageControls";
import { ImageViewer, pageImages } from "./ImageViewer";
import type { ViewerImage } from "./ImageViewer";
import { MediaPreview } from "./MediaPlayer";
import { PdfPreview } from "./PdfViewer";
import { Observable, useObservable } from "./ai/observable";
import type { JobView } from "./ai/transcribe-jobs";

/** The jobs of an editor without AI features: none. */
const NO_JOBS = new Observable<ReadonlyMap<string, JobView>>(new Map());

/** An icon for a file by the family of its MIME type. */
export function fileIcon(mime: string): string {
  const [family = "", subtype = ""] = mime.toLowerCase().split("/");
  if (family === "image") {
    return "🖼️";
  }
  if (family === "video") {
    return "🎞️";
  }
  if (family === "audio") {
    return "🎵";
  }
  if (subtype === "pdf") {
    return "📕";
  }
  if (/zip|tar|gzip|7z|rar|compressed/.test(subtype)) {
    return "🗜️";
  }
  if (/spreadsheet|excel|csv/.test(subtype)) {
    return "📊";
  }
  if (/presentation|powerpoint/.test(subtype)) {
    return "📽️";
  }
  if (family === "text" || /json|xml|markdown|msword|wordprocessing|opendocument\.text|rtf/.test(subtype)) {
    return "📄";
  }
  return "📎";
}

/** The attachment a block shows, kept fresh: from the store's cache, else read once. */
export function useAttachment(attachmentId: string | null): AttachmentLookup {
  const lookup = useAppStore((state) => (attachmentId === null ? undefined : state.attachmentLookup[attachmentId]));
  const lookupAttachment = useAppStore((state) => state.lookupAttachment);
  const needsLookup = attachmentId !== null && lookup === undefined;
  useEffect(() => {
    if (needsLookup) {
      lookupAttachment(attachmentId);
    }
  }, [needsLookup, lookupAttachment, attachmentId]);
  if (attachmentId === null) {
    return { status: "missing" };
  }
  return lookup ?? { status: "loading" };
}

export type Preview = { status: "loading" } | { status: "ready"; url: string } | { status: "error"; message: string };

/**
 * The bytes of an image as a `blob:` URL, read when `info` is given (a complete image small
 * enough to preview): an image whose content was still on its way is read once it becomes
 * complete, since `info` is `null` until then. The URL is revoked when the attachment changes
 * and when the view goes.
 */
export function useImagePreview(info: AttachmentInfo | null): Preview | null {
  const readAttachment = useAppStore((state) => state.readAttachment);
  const id = info?.attachmentId ?? null;
  const mime = info?.mime ?? "";
  const key = id === null ? null : `${id} ${mime}`;
  const [loaded, setLoaded] = useState<{ key: string; preview: Preview } | null>(null);

  useEffect(() => {
    if (id === null || key === null) {
      return;
    }
    let active = true;
    let url: string | null = null;
    readAttachment(id).then(
      (bytes) => {
        if (!active) {
          return;
        }
        url = URL.createObjectURL(new Blob([bytes as Uint8Array<ArrayBuffer>], { type: mime }));
        setLoaded({ key, preview: { status: "ready", url } });
      },
      (error: unknown) => {
        if (active) {
          setLoaded({ key, preview: { status: "error", message: toCoreError(error).message } });
        }
      },
    );
    return () => {
      active = false;
      if (url !== null) {
        URL.revokeObjectURL(url);
      }
    };
  }, [id, key, mime, readAttachment]);

  if (key === null) {
    return null;
  }
  return loaded?.key === key ? loaded.preview : { status: "loading" };
}

/**
 * A card in place of the image or file: an icon, a title, a line of detail (with `footer`, a
 * progress bar, under it) and actions.
 */
function Card({
  icon,
  title,
  detail,
  footer,
  className,
  children,
}: {
  icon: string;
  title: string;
  detail?: string;
  footer?: ReactNode;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <div className={className === undefined ? "attachment-card" : `attachment-card ${className}`}>
      <span className="attachment-icon" aria-hidden="true">
        {icon}
      </span>
      <span className="attachment-text">
        <span className="attachment-name">{title}</span>
        {detail !== undefined && <span className="attachment-detail">{detail}</span>}
        {footer}
      </span>
      {children}
    </div>
  );
}

/**
 * About how much of an attachment is here: every chunk but the last holds `CHUNK_SIZE` bytes
 * of plaintext, so the present chunks hold at most this much.
 */
export function downloadedBytes(info: AttachmentInfo): number {
  return Math.min(Number(info.size), info.localChunks * CHUNK_SIZE);
}

/**
 * The card of an attachment whose content is not all on this device (ADR-0023), or `null`
 * once it is complete: how far its download got, that it waits for the device that added
 * it, or its size and `download` (a "Download" button) on a device that downloads on demand.
 */
function transferCard(info: AttachmentInfo, icon: string, download: ReactNode): ReactNode {
  switch (info.availability) {
    case "Complete":
      return null;
    case "Downloading": {
      const total = Math.max(1, info.chunkCount);
      return (
        <Card
          icon={icon}
          title={info.name}
          detail={t("attachment.transfer.downloading", {
            done: formatBytes(downloadedBytes(info)),
            total: formatBytes(info.size),
          })}
          footer={
            <progress
              className="attachment-transfer-bar"
              max={total}
              value={Math.min(info.localChunks, total)}
              aria-label={t("attachment.transfer.progress", { name: info.name })}
            />
          }
          className="is-downloading"
        />
      );
    }
    case "Waiting":
      return <Card icon="⏳" title={info.name} detail={t("attachment.transfer.waiting")} className="is-waiting" />;
    case "NotDownloaded":
      return (
        <Card
          icon={icon}
          title={info.name}
          detail={t("attachment.transfer.notDownloaded", { size: formatBytes(info.size) })}
          className="is-not-downloaded"
        >
          {download}
        </Card>
      );
  }
}

/** What an image block adds to its menu. */
interface ImageMenu {
  /** Opens the full-size viewer from `opener`; `null` while the picture is not shown. */
  viewFull: ((opener: HTMLElement | null) => void) | null;
}

/**
 * What a file block adds to its menu when it holds a PDF the preview can show (ADR-0031) or a video
 * or audio file the player can play (ADR-0032).
 */
interface PdfMenu {
  /** The preview is a viewer (a PDF) or a player (media): the name of the entry that turns it on. */
  player: boolean;
  /** The block shows the preview now. */
  previewing: boolean;
  /** Switches between the preview and the card. */
  toggle: () => void;
}

/** What every attachment block shares: its menu, the alt text field, notices. */
function useBlockActions(
  props: ReactNodeViewProps,
  info: AttachmentInfo | null,
  image: ImageMenu | null = null,
  pdf: PdfMenu | null = null,
  transcribe: (() => void) | null = null,
) {
  const { node, editor, deleteNode, updateAttributes } = props;
  const capabilities = useAppStore((state) => state.attachmentCapabilities);
  const saveAttachmentAs = useAppStore((state) => state.saveAttachmentAs);
  const openAttachment = useAppStore((state) => state.openAttachment);
  const downloadAttachment = useAppStore((state) => state.downloadAttachment);
  const [menu, setMenu] = useState<{ anchor: Anchor; opener: Element } | null>(null);
  const [editingAlt, setEditingAlt] = useState(false);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  const [markdown, setMarkdown] = useState<string | null>(null);
  const [requesting, setRequesting] = useState(false);
  const isImage = node.type.name === IMAGE_NODE;
  // The binding can turn the editor read-only after the view was drawn (content from a newer version).
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  // A file that is not all here cannot be saved or opened yet (the core would refuse).
  const complete = info !== null && info.availability === "Complete";
  const canSave = capabilities.saveAs && complete;
  const canOpen = capabilities.open && complete;

  async function download(): Promise<void> {
    if (info === null) {
      return;
    }
    setNotice(null);
    setRequesting(true);
    try {
      await downloadAttachment(info.attachmentId);
    } catch (error) {
      setNotice({ text: t("attachment.transfer.error", { message: toCoreError(error).message }), error: true });
    }
    setRequesting(false);
  }

  async function saveAs(): Promise<void> {
    if (info === null) {
      return;
    }
    setNotice(null);
    try {
      if (await saveAttachmentAs(info.attachmentId)) {
        setNotice({ text: t("attachment.saved"), error: false });
      }
    } catch (error) {
      setNotice({ text: t("attachment.error.save", { message: toCoreError(error).message }), error: true });
    }
  }

  async function open(): Promise<void> {
    if (info === null) {
      return;
    }
    setNotice(null);
    try {
      await openAttachment(info.attachmentId);
    } catch (error) {
      setNotice({ text: t("attachment.error.open", { message: toCoreError(error).message }), error: true });
    }
  }

  async function copyMarkdown(current: PmNode): Promise<void> {
    setNotice(null);
    const text = attachmentMarkdown(current, info?.name ?? null);
    try {
      await writeClipboard(text);
      setNotice({ text: t("attachment.markdown.copied"), error: false });
    } catch {
      setMarkdown(text); // no clipboard here: show the text to copy by hand
    }
  }

  const items: MenuItem[] = [
    ...(image?.viewFull != null ? [{ id: "viewFull", label: t("attachment.image.viewFull") }] : []),
    ...(isImage && editable
      ? [
          { id: "fullWidth", label: t("attachment.image.fullWidth") },
          { id: "originalSize", label: t("attachment.image.originalSize") },
        ]
      : []),
    ...(pdf !== null && editable
      ? [
          {
            id: "togglePdf",
            label: pdf.previewing
              ? t("attachment.pdf.showCard")
              : pdf.player
                ? t("attachment.media.showPlayer")
                : t("attachment.pdf.showPreview"),
          },
        ]
      : []),
    ...(transcribe !== null && editable ? [{ id: "transcribe", label: t("transcribe.menu") }] : []),
    ...(canSave ? [{ id: "saveAs", label: t("attachment.saveAs") }] : []),
    ...(canOpen ? [{ id: "open", label: t("attachment.open") }] : []),
    ...(isImage && editable ? [{ id: "alt", label: t("attachment.alt.edit") }] : []),
    { id: "markdown", label: t("attachment.copyMarkdown") },
    ...(editable ? [{ id: "remove", label: t("attachment.remove"), danger: true }] : []),
  ];

  const menuButton = (
    <button
      type="button"
      className="icon-button attachment-menu-button"
      aria-label={t("attachment.actions")}
      title={t("attachment.actions")}
      aria-haspopup="menu"
      aria-expanded={menu !== null}
      onClick={(event) => {
        const opener = event.currentTarget;
        setMenu((current) => (current?.opener === opener ? null : { anchor: anchorBelow(opener), opener }));
      }}
    >
      <MoreIcon />
    </button>
  );

  const extras = (
    <>
      {editingAlt && (
        <InlineEdit
          className="attachment-alt-edit"
          value={typeof node.attrs.alt === "string" ? node.attrs.alt : ""}
          label={t("attachment.alt.label")}
          placeholder={t("attachment.alt.placeholder")}
          onCommit={(next) => {
            updateAttributes({ alt: next.trim() });
            return Promise.resolve();
          }}
          onDone={() => {
            setEditingAlt(false);
          }}
        />
      )}
      {notice !== null && (
        <p className={notice.error ? "attachment-notice error-text" : "attachment-notice muted"} role={notice.error ? "alert" : "status"}>
          {notice.text}
        </p>
      )}
      {menu !== null && (
        <Menu
          label={t("attachment.actions")}
          items={items}
          anchor={menu.anchor}
          anchorElement={menu.opener}
          onSelect={(id) => {
            switch (id) {
              case "viewFull":
                image?.viewFull?.(menu.opener instanceof HTMLElement ? menu.opener : null);
                break;
              case "fullWidth":
                updateAttributes({ widthPercent: 100, width: null });
                break;
              case "originalSize":
                updateAttributes({ widthPercent: null, width: null });
                break;
              case "togglePdf":
                pdf?.toggle();
                break;
              case "transcribe":
                transcribe?.();
                break;
              case "saveAs":
                void saveAs();
                break;
              case "open":
                void open();
                break;
              case "alt":
                setEditingAlt(true);
                break;
              case "markdown":
                void copyMarkdown(node);
                break;
              case "remove":
                deleteNode();
                break;
            }
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
      {markdown !== null && (
        <TextDialog
          title={t("attachment.markdown.title")}
          body={t("page.markdown.body")}
          text={markdown}
          onClose={() => {
            setMarkdown(null);
          }}
        />
      )}
    </>
  );

  /** Save as and Open as buttons, for cards that show no preview. */
  const fileButtons =
    canSave || canOpen ? (
      <span className="attachment-card-actions">
        {canSave && (
          <button type="button" className="button" onClick={() => void saveAs()}>
            {t("attachment.saveAs")}
          </button>
        )}
        {canOpen && (
          <button type="button" className="button" onClick={() => void open()}>
            {t("attachment.open")}
          </button>
        )}
      </span>
    ) : null;

  /** "Download", for a card whose file this device has not asked for (on-demand policy). */
  const downloadButton = (
    <span className="attachment-card-actions">
      <button type="button" className="button" disabled={requesting} onClick={() => void download()}>
        {t("attachment.transfer.download")}
      </button>
    </span>
  );

  return { menuButton, extras, fileButtons, downloadButton };
}

/** The card of an attachment that is not there (yet): loading, deleted, unreadable. */
function lookupCard(lookup: AttachmentLookup, kind: "image" | "file"): ReactNode {
  switch (lookup.status) {
    case "loading":
      return <Card icon="⏳" title={t("attachment.loading")} className="is-loading" />;
    case "missing":
      return (
        <Card
          icon="🚫"
          title={kind === "image" ? t("attachment.deleted.image") : t("attachment.deleted.file")}
          detail={t("attachment.deleted.detail")}
          className="is-deleted"
        />
      );
    case "error":
      return <Card icon="⚠️" title={t("attachment.error.read", { message: lookup.message })} className="is-error" />;
    case "found":
      return null;
  }
}

/** Where the pointer-driven resize finds the column: the block's width, from its bounding box. */
function widthOf(element: Element | null): number {
  return element === null ? 0 : element.getBoundingClientRect().width;
}

/** The `image` block: the picture with its layout and tools, or a card saying why it cannot be shown. */
export function ImageView(props: ReactNodeViewProps) {
  const { node, editor, getPos, updateAttributes } = props;
  const lookup = useAttachment(attachmentIdOf(node));
  const canRead = useAppStore((state) => state.attachmentCapabilities.read);
  const info = lookup.status === "found" ? lookup.info : null;
  const complete = info !== null && info.availability === "Complete";
  const previewable = complete && canRead && Number(info.size) <= MAX_PREVIEW_BYTES;
  const preview = useImagePreview(previewable ? info : null);
  const [brokenUrl, setBrokenUrl] = useState<string | null>(null);
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const alt = typeof node.attrs.alt === "string" ? node.attrs.alt : "";
  const width = parseWidth(node.attrs.width);
  const widthPercent = parseWidthPercent(node.attrs.widthPercent);
  const align = parseAlign(node.attrs.align);
  const caption = parseCaption(node.attrs.caption);

  const wrapperRef = useRef<HTMLElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  const captionButton = useRef<HTMLButtonElement>(null);
  const viewerFocus = useRef<HTMLElement | null>(null);
  // The width shown while a handle is dragged (nothing is written until it is released), and
  // the picture's width in percent as last measured (for one with no width of its own).
  const [live, setLive] = useState<number | null>(null);
  const [measured, setMeasured] = useState<number | null>(null);
  const [editingCaption, setEditingCaption] = useState(false);
  const [viewer, setViewer] = useState<{ images: ViewerImage[]; start: number } | null>(null);

  /** The picture's width now, in percent of the block's width, `null` where there is no layout. */
  function measure(): number | null {
    const column = widthOf(wrapperRef.current);
    const frame = widthOf(frameRef.current);
    return column > 0 && frame > 0 ? clampPercent(Math.round((frame / column) * 100)) : null;
  }

  /** The width a drag or a key press starts from. */
  function currentPercent(): number {
    return live ?? widthPercent ?? measure() ?? measured ?? 100;
  }

  function resize(percent: number | null): void {
    updateAttributes({ widthPercent: percent, width: null });
  }

  /** Opens the viewer on this image; focus goes back to `opener` (the page when there is none). */
  function openViewer(opener: HTMLElement | null): void {
    const images = pageImages(editor);
    const here = getPos();
    const attachmentId = attachmentIdOf(node);
    let start = images.findIndex((image) => image.pos === here);
    if (start < 0) {
      start = images.findIndex((image) => image.attachmentId === attachmentId);
    }
    if (start < 0) {
      return;
    }
    viewerFocus.current = opener ?? editor.view.dom;
    setViewer({ images, start });
  }

  // The picture is shown once its bytes are read and the webview can decode them.
  const picture = previewable && preview?.status === "ready" && preview.url !== brokenUrl;
  const { menuButton, extras, fileButtons, downloadButton } = useBlockActions(props, info, {
    viewFull: picture
      ? (opener) => {
          openViewer(opener);
        }
      : null,
  });

  let body: ReactNode = lookupCard(lookup, "image");
  if (info !== null) {
    const size = formatBytes(info.size);
    const transfer = transferCard(info, "🖼️", downloadButton);
    if (transfer !== null) {
      body = transfer;
    } else if (!canRead) {
      body = <Card icon="🖼️" title={info.name} detail={t("attachment.preview.unavailable", { size })} />;
    } else if (!previewable) {
      body = (
        <Card icon="🖼️" title={info.name} detail={t("attachment.preview.tooLarge", { size })} className="is-too-large">
          {fileButtons}
        </Card>
      );
    } else if (preview === null || preview.status === "loading") {
      body = <Card icon="⏳" title={info.name} detail={t("attachment.preview.loading")} className="is-loading" />;
    } else if (preview.status === "error") {
      body = (
        <Card icon="⚠️" title={info.name} detail={t("attachment.error.read", { message: preview.message })} className="is-error">
          {fileButtons}
        </Card>
      );
    } else if (brokenUrl === preview.url) {
      body = (
        <Card icon="🖼️" title={info.name} detail={t("attachment.preview.cannotShow", { size })} className="is-error">
          {fileButtons}
        </Card>
      );
    } else {
      const url = preview.url;
      // A double click opens the viewer; where nothing can be edited a single click does.
      body = (
        <img
          className={editable ? "image-block-img" : "image-block-img is-zoomable"}
          src={url}
          alt={alt}
          title={alt === "" ? undefined : alt}
          draggable={false}
          onLoad={() => {
            setMeasured(measure());
          }}
          onError={() => {
            setBrokenUrl(url);
          }}
          onClick={
            editable
              ? undefined
              : () => {
                  openViewer(null);
                }
          }
          onDoubleClick={
            editable
              ? () => {
                  openViewer(null);
                }
              : undefined
          }
        />
      );
    }
  }

  const shownPercent = live ?? widthPercent;
  const sized = picture && (shownPercent !== null || width !== null);
  let frameStyle: CSSProperties | undefined;
  if (picture && shownPercent !== null) {
    frameStyle = { width: `${String(shownPercent)}%` };
  } else if (picture && width !== null) {
    frameStyle = { width: `${String(width)}px` };
  }
  const frameClass = ["attachment-frame", picture ? "image-frame" : "", sized ? "is-sized" : "", live === null ? "" : "is-resizing"]
    .filter((name) => name !== "")
    .join(" ");

  function setAlign(next: ImageAlign): void {
    const wanted = next === "center" ? null : next;
    if (wanted !== align) {
      updateAttributes({ align: wanted });
    }
  }

  function endCaptionEdit(reason: "enter" | "escape" | "blur"): void {
    setEditingCaption(false);
    if (reason !== "blur") {
      // The field is going: give focus back to the button that opened it, or to the page.
      const button = captionButton.current;
      if (button?.isConnected === true) {
        button.focus();
      } else {
        editor.commands.focus();
      }
    }
  }

  return (
    <NodeViewWrapper ref={wrapperRef} className="attachment-block image-block" data-drag-handle="">
      <div className={picture ? "image-layout" : "image-layout is-card"} data-align={picture ? (align ?? "center") : undefined}>
        <div
          ref={frameRef}
          className={frameClass}
          style={frameStyle}
          onPointerEnter={() => {
            if (picture && live === null) {
              setMeasured(measure());
            }
          }}
        >
          {body}
          {picture ? (
            <ImageToolbar
              editable={editable}
              align={align}
              captionButton={captionButton}
              onAlign={setAlign}
              onCaption={() => {
                setEditingCaption(true);
              }}
              onView={(button) => {
                openViewer(button);
              }}
              menuButton={menuButton}
            />
          ) : (
            menuButton
          )}
          {picture &&
            editable &&
            (["left", "right"] as const).map((side) => (
              <ResizeHandle
                key={side}
                side={side}
                value={shownPercent ?? measured ?? 100}
                centered={align === null || align === "center"}
                column={() => widthOf(wrapperRef.current)}
                current={currentPercent}
                onPreview={setLive}
                onCommit={resize}
                onReset={() => {
                  if (widthPercent !== null || width !== null) {
                    resize(null);
                  }
                }}
                onFocus={() => {
                  setMeasured(measure());
                }}
              />
            ))}
        </div>
      </div>
      {editingCaption && editable ? (
        <CaptionEdit
          value={caption ?? ""}
          align={align}
          onCommit={(text) => {
            const next = normalizeCaption(text);
            if (next !== caption) {
              updateAttributes({ caption: next });
            }
          }}
          onDone={endCaptionEdit}
        />
      ) : (
        caption !== null && (
          <p
            className={editable ? "image-caption is-editable" : "image-caption"}
            data-align={align ?? "center"}
            onClick={
              editable
                ? () => {
                    setEditingCaption(true);
                  }
                : undefined
            }
          >
            {caption}
          </p>
        )
      )}
      {extras}
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

/**
 * The `file` block: a card with the file's type, name and size (or how far its download is). A PDF
 * with `view: "preview"` shows as an inline viewer instead (ADR-0031); one that cannot be opened
 * (password protected, damaged, unreadable) goes back to the card with the reason under its name.
 */
export function FileView(props: ReactNodeViewProps) {
  const { node, updateAttributes } = props;
  const attachmentId = attachmentIdOf(node);
  const lookup = useAttachment(attachmentId);
  const canRead = useAppStore((state) => state.attachmentCapabilities.read);
  const info = lookup.status === "found" ? lookup.info : null;
  const view = parseFileView(node.attrs.view);
  const media = info === null ? null : mediaKindOf(info.mime);
  const previewable =
    info !== null && info.availability === "Complete" && canRead && (media !== null || isPdfFile(info.name, info.mime));
  // Why the preview was given up, for the attachment it was about.
  const [failure, setFailure] = useState<{ id: string | null; message: string } | null>(null);
  const failedWith = failure?.id === attachmentId ? failure.message : null;
  // Print mode (print.ts): paper has no viewer or player, the card (name and size) prints instead.
  const printing = useAppStore((state) => state.printing);
  const previewing = view === "preview" && previewable && failedWith === null && !printing;
  const onFailure = useCallback(
    (message: string) => {
      setFailure({ id: attachmentId, message });
    },
    [attachmentId],
  );
  // Local AI (ADR-0037): "Transcribe" in the menu of a complete audio or video file, while AI is on with a
  // transcription model; the progress shows under the block.
  const ai = useEditorAi();
  const aiCapabilities = useAiCapabilities();
  const blockId = typeof node.attrs.id === "string" ? node.attrs.id : null;
  const transcribable =
    ai !== null && blockId !== null && info !== null && info.availability === "Complete" && canRead && media !== null && aiCapabilities.transcription;
  const running = useObservable(ai?.jobs.views ?? NO_JOBS).has(blockId ?? "");
  const { menuButton, extras, downloadButton } = useBlockActions(
    props,
    info,
    null,
    previewable
      ? {
          player: media !== null,
          previewing,
          toggle: () => {
            // After a failure the entry says "Show preview": it tries again, the attribute stays.
            const wanted = failedWith !== null || view !== "preview" ? "preview" : null;
            setFailure(null);
            if (wanted !== view) {
              updateAttributes({ view: wanted });
            }
          },
        }
      : null,
    transcribable && !running
      ? () => {
          void ai.jobs.start({ blockId, attachmentId: info.attachmentId, size: Number(info.size) });
        }
      : null,
  );

  let body: ReactNode = lookupCard(lookup, "file");
  if (info !== null) {
    const icon = fileIcon(info.mime);
    body =
      transferCard(info, icon, downloadButton) ??
      (view === "preview" && failedWith !== null ? (
        <Card icon="⚠️" title={info.name} detail={failedWith} className="file-card is-error" />
      ) : (
        <Card icon={icon} title={info.name} detail={formatBytes(info.size)} className="file-card" />
      ));
  }

  if (previewing && media !== null) {
    return (
      <NodeViewWrapper className="attachment-block file-block is-media-preview">
        <div className="attachment-frame">
          <MediaPreview info={info} kind={media} menuButton={menuButton} onFailure={onFailure} />
        </div>
        <TranscribeStatus blockId={blockId} />
        {extras}
      </NodeViewWrapper>
    );
  }
  if (previewing) {
    return (
      <NodeViewWrapper className="attachment-block file-block is-pdf-preview">
        <div className="attachment-frame">
          <PdfPreview info={info} menuButton={menuButton} onFailure={onFailure} />
        </div>
        {extras}
      </NodeViewWrapper>
    );
  }
  return (
    <NodeViewWrapper className="attachment-block file-block" data-drag-handle="">
      <div className="attachment-frame">
        {body}
        {menuButton}
      </div>
      <TranscribeStatus blockId={blockId} />
      {extras}
    </NodeViewWrapper>
  );
}
