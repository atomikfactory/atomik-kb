// The node views of the version preview (ADR-0057, `docs/architecture/page-history.md` section 8): what the page editor
// draws with a live session, drawn without one. Nothing here writes, opens a menu, navigates or reads a Loro document:
// a page reference is a chip that goes nowhere, an image is read through the attachment store, and the blocks that need
// the page's live state or a heavy library (a whiteboard, a gallery, the table of contents, the breadcrumb, a list view)
// are a labelled box. The callout is the editor's own view (its pickers open only in an editable editor).

import { MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import { NodeViewContent, NodeViewWrapper } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useState } from "react";
import type { CSSProperties, ReactNode } from "react";

import { ChevronIcon, PlusIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { attachmentIdOf, parseAlign, parseCaption, parseWidth, parseWidthPercent } from "../editor/attachment-nodes";
import { fileIcon, useAttachment, useImagePreview } from "../editor/AttachmentViews";
import { labelOf, positionOf } from "../editor/template-button";
import { useReferencedPage } from "../editor/node-views";
import { formatBytes } from "../format";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import type { AttachmentLookup } from "../store-attachments";
import { useAppStore } from "../store-context";

// ----- references ---------------------------------------------------------------------------------------------------

/** The title a reference shows: the page's own, "Untitled", or why there is none. */
function referenceTitle(page: ReturnType<typeof useReferencedPage>, missing: MessageKey): { title: string; state: string } {
  if (page.status === "missing") {
    return { title: t(missing), state: " is-deleted" };
  }
  if (page.status === "found") {
    return { title: page.page.title === "" ? t("page.untitled") : page.page.title, state: page.page.trashedAt === undefined ? "" : " is-trashed" };
  }
  return { title: t("pageRef.loading"), state: "" };
}

/** The `pageRef` chip: the page's icon and live title, and no link. */
export function PreviewPageRef({ node }: ReactNodeViewProps) {
  const page = useReferencedPage(String(node.attrs.pageId));
  const { title, state } = referenceTitle(page, "pageRef.deleted");
  return (
    <NodeViewWrapper as="span" className={`page-ref${state}`}>
      <PageIcon icon={page.status === "found" ? page.page.icon : undefined} className="page-ref-icon" />
      <span className="page-ref-title">{title}</span>
    </NodeViewWrapper>
  );
}

/** The `pageLink` card: the page's icon and live title, and no link. */
export function PreviewPageLink({ node }: ReactNodeViewProps) {
  const page = useReferencedPage(String(node.attrs.pageId));
  const { title, state } = referenceTitle(page, "pageLink.unknown");
  return (
    <NodeViewWrapper className={`page-link${state.replace("is-deleted", "is-unknown")}`}>
      <PageIcon icon={page.status === "found" ? page.page.icon : undefined} className="page-link-icon" />
      <span className="page-link-title">{title}</span>
      <span className="page-link-chevron" aria-hidden="true">
        <ChevronIcon />
      </span>
    </NodeViewWrapper>
  );
}

// ----- attachments --------------------------------------------------------------------------------------------------

/** A card in place of a picture or a file: an icon, a title and a line of detail. */
function Card({ icon, title, detail, className }: { icon: string; title: string; detail?: string; className?: string }) {
  return (
    <div className={className === undefined ? "attachment-card" : `attachment-card ${className}`}>
      <span className="attachment-icon" aria-hidden="true">
        {icon}
      </span>
      <span className="attachment-text">
        <span className="attachment-name">{title}</span>
        {detail !== undefined && <span className="attachment-detail">{detail}</span>}
      </span>
    </div>
  );
}

/** The card of an attachment that is not there (yet): loading, deleted, unreadable; `null` once it is found. */
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

/** The `image` block: the picture (read through the attachment store) with its width, alignment and caption; else a card. */
export function PreviewImage({ node }: ReactNodeViewProps) {
  const lookup = useAttachment(attachmentIdOf(node));
  const canRead = useAppStore((state) => state.attachmentCapabilities.read);
  const info = lookup.status === "found" ? lookup.info : null;
  const previewable = info !== null && info.availability === "Complete" && canRead && Number(info.size) <= MAX_PREVIEW_BYTES;
  const preview = useImagePreview(previewable ? info : null);
  const [broken, setBroken] = useState<string | null>(null);
  const alt = typeof node.attrs.alt === "string" ? node.attrs.alt : "";
  const width = parseWidth(node.attrs.width);
  const widthPercent = parseWidthPercent(node.attrs.widthPercent);
  const align = parseAlign(node.attrs.align) ?? "center";
  const caption = parseCaption(node.attrs.caption);

  const url = preview?.status === "ready" && preview.url !== broken ? preview.url : null;
  let body: ReactNode = lookupCard(lookup, "image");
  if (body === null && info !== null) {
    const size = formatBytes(info.size);
    if (url === null) {
      let detail = t("attachment.preview.loading");
      if (info.availability !== "Complete") {
        detail = t("attachment.transfer.waiting");
      } else if (!canRead) {
        detail = t("attachment.preview.unavailable", { size });
      } else if (!previewable) {
        detail = t("attachment.preview.tooLarge", { size });
      } else if (preview?.status === "error") {
        detail = t("attachment.error.read", { message: preview.message });
      } else if (preview?.status === "ready") {
        detail = t("attachment.preview.cannotShow", { size });
      }
      body = <Card icon="🖼️" title={info.name} detail={detail} />;
    }
  }

  let frameStyle: CSSProperties | undefined;
  if (url !== null && widthPercent !== null) {
    frameStyle = { width: `${String(widthPercent)}%` };
  } else if (url !== null && width !== null) {
    frameStyle = { width: `${String(width)}px` };
  }
  return (
    <NodeViewWrapper className="attachment-block image-block">
      <div className={url === null ? "image-layout is-card" : "image-layout"} data-align={url === null ? undefined : align}>
        <div className={url === null ? "attachment-frame" : `attachment-frame image-frame${frameStyle === undefined ? "" : " is-sized"}`} style={frameStyle}>
          {url === null ? (
            body
          ) : (
            <img
              className="image-block-img"
              src={url}
              alt={alt}
              draggable={false}
              onError={() => {
                setBroken(url);
              }}
            />
          )}
        </div>
      </div>
      {caption !== null && (
        <p className="image-caption" data-align={align}>
          {caption}
        </p>
      )}
    </NodeViewWrapper>
  );
}

/** The `file` block: a card with the file's type, name and size. */
export function PreviewFile({ node }: ReactNodeViewProps) {
  const lookup = useAttachment(attachmentIdOf(node));
  const info = lookup.status === "found" ? lookup.info : null;
  const body =
    lookupCard(lookup, "file") ??
    (info === null ? null : <Card icon={fileIcon(info.mime)} title={info.name} detail={formatBytes(info.size)} className="file-card" />);
  return (
    <NodeViewWrapper className="attachment-block file-block">
      <div className="attachment-frame">{body}</div>
    </NodeViewWrapper>
  );
}

// ----- blocks without a live view ------------------------------------------------------------------------------------

/** The view of a block the preview draws as a labelled box (a whiteboard, a gallery, the derived blocks). */
function placeholderView(label: MessageKey): (props: ReactNodeViewProps) => ReactNode {
  return function PlaceholderView() {
    return (
      <NodeViewWrapper className="history-placeholder">
        <span>{t(label)}</span>
      </NodeViewWrapper>
    );
  };
}

export const WhiteboardPlaceholder = placeholderView("whiteboard.title");
export const GalleryPlaceholder = placeholderView("gallery.label");
export const TableOfContentsPlaceholder = placeholderView("toc.title");
export const BreadcrumbPlaceholder = placeholderView("breadcrumb.label");
export const PageListPlaceholder = placeholderView("pageList.label");

/** The template button: its label as a button that does nothing, and the blocks it holds, shown (the page editor hides them). */
export function PreviewTemplateButton({ node }: ReactNodeViewProps) {
  const label = labelOf(node.attrs.label);
  const shown = label === "" ? t("templateButton.defaultLabel") : label;
  const position = positionOf(node.attrs.position);
  return (
    <NodeViewWrapper className="template-button-body" data-position={position}>
      <div className="template-button-bar" contentEditable={false}>
        <button type="button" className="template-button-trigger" disabled>
          <span className="template-button-plus" aria-hidden="true">
            <PlusIcon />
          </span>
          <span className="template-button-label">{shown}</span>
        </button>
      </div>
      <div className="template-button-frame" role="group" aria-label={t("templateButton.contentHint")}>
        <NodeViewContent className="template-button-content" />
      </div>
    </NodeViewWrapper>
  );
}
