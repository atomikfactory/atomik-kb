// The view of the breadcrumb block: the chain from the space down to the page the editor shows
// (`Breadcrumbs`, which any other screen can use for a page too). The page comes from the
// extension's options (the editor of a page knows its page id); the chain from the app's page
// tree, so the block redraws when a page is renamed or moved. A read-only editor shows it and its
// buttons navigate all the same: nothing here touches the document.

import { NodeViewWrapper } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";

import { Breadcrumbs } from "../components/Breadcrumbs";

/** The `pageId` option of the breadcrumb extension, `null` when it has none. */
function pageIdOf(options: unknown): string | null {
  if (typeof options !== "object" || options === null || !("pageId" in options)) {
    return null;
  }
  return typeof options.pageId === "string" && options.pageId !== "" ? options.pageId : null;
}

/** The breadcrumb block of a page. */
export function BreadcrumbView({ extension }: ReactNodeViewProps) {
  const pageId = pageIdOf(extension.options);
  return <NodeViewWrapper className="breadcrumb-view">{pageId !== null && <Breadcrumbs pageId={pageId} />}</NodeViewWrapper>;
}
