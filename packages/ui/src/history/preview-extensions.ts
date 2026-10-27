// The extensions of the version preview's editor (ADR-0057, `docs/architecture/page-history.md` section 8): the page's own schema
// (`createSchemaExtensions`, the editor's nodes and marks) with views that need no live session, and the marks of the
// differences. No Loro binding, no menus, no comments, no presence, no ids written: a read-only ProseMirror view on the
// portable JSON that `diffPages` (or the version alone) gives.
//
// **The differences.** A block carries `attrs.historyDiff` (a global attribute of every node, never rendered); a decoration
// puts the class `history-diff history-diff-<status>` on the outermost block of each added, removed and changed part (what
// lies inside a wholly added or removed block shows through it), and `data-history-diff` beside it. An inline run is a
// `historyDiff` mark rendered as `<ins>` (in this version only) or `<del>` (on the page now only). Decorations, not
// attributes in the DOM, because a node view (the callout, the template button) owns its element and a decoration reaches it.
//
// **Code and equations** are drawn as text: the editor's code block and math block views have a language picker, a drawn
// diagram and typeset formulas, and would draw the merged text of a changed block as one garbled source. The code block keeps
// its highlighting (a plugin, not the view). Both allow marks, so that their changed words can carry the run marks.

import { Extension, getSchema, Mark } from "@tiptap/core";
import type { AnyExtension, JSONContent } from "@tiptap/core";
import type { Node as PmNode, Schema } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { EditorView } from "@tiptap/pm/view";
import { ReactNodeViewRenderer } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import type { ComponentType } from "react";

import { CodeBlock } from "../editor/code-block";
import { MathBlock } from "../editor/math-block";
import { CalloutView } from "../editor/node-views";
import { createSchemaExtensions, ID_BEARING_NODES } from "../editor/schema";
import { HISTORY_DIFF_ATTRIBUTE, HISTORY_DIFF_MARK } from "./page-diff";
import type { PortableNode } from "./page-diff";
import {
  BreadcrumbPlaceholder,
  GalleryPlaceholder,
  PageListPlaceholder,
  PreviewFile,
  PreviewImage,
  PreviewPageLink,
  PreviewPageRef,
  PreviewTemplateButton,
  TableOfContentsPlaceholder,
  WhiteboardPlaceholder,
} from "./preview-views";

/** The attribute that holds the status of a block: on every node of the schema, and never rendered. */
const HistoryDiffAttribute = Extension.create({
  name: "historyDiffAttribute",

  addGlobalAttributes() {
    return [
      {
        types: [...ID_BEARING_NODES],
        attributes: { [HISTORY_DIFF_ATTRIBUTE]: { default: null, rendered: false, keepOnSplit: false } },
      },
    ];
  },
});

/** An inline run that is only in the version (`ins`) or only on the page now (`del`). */
const HistoryDiffMark = Mark.create({
  name: HISTORY_DIFF_MARK,
  inclusive: false,

  addAttributes() {
    return { status: { default: "added", rendered: false } };
  },

  renderHTML({ mark }) {
    const removed = mark.attrs.status === "removed";
    const status = removed ? "removed" : "added";
    return [removed ? "del" : "ins", { class: `history-run history-run-${status}`, "data-history-diff": status }, 0];
  },
});

const historyDiffKey = new PluginKey<DecorationSet>("historyDiff");

/** The decorations of the added, removed and changed blocks of a document; not inside a block that is wholly added or removed. */
function decorate(doc: PmNode): DecorationSet {
  const decorations: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (!node.isBlock) {
      return false; // text and inline atoms carry marks, not statuses
    }
    const status: unknown = node.attrs[HISTORY_DIFF_ATTRIBUTE];
    if (status === "added" || status === "removed" || status === "changed") {
      decorations.push(
        Decoration.node(pos, pos + node.nodeSize, { class: `history-diff history-diff-${status}`, "data-history-diff": status }),
      );
      return status === "changed";
    }
    return true;
  });
  return DecorationSet.create(doc, decorations);
}

const HistoryDiffDecorations = Extension.create({
  name: "historyDiffDecorations",

  addProseMirrorPlugins() {
    return [
      new Plugin<DecorationSet>({
        key: historyDiffKey,
        state: {
          init: (_config, { doc }) => decorate(doc),
          apply: (tr, decorations) => (tr.docChanged ? decorate(tr.doc) : decorations),
        },
        props: {
          decorations: (state) => historyDiffKey.getState(state),
        },
      }),
    ];
  },
});

/** A click on a link of the page: nothing happens (the webview would otherwise leave the app for the address). */
function keepLink(view: EditorView, event: Event): boolean {
  const link = event.target instanceof Element ? event.target.closest("a[href]") : null;
  if (link === null || !view.dom.contains(link)) {
    return false;
  }
  event.preventDefault();
  return true;
}

/** Links in a version are text: they go nowhere (the page editor opens them through the shell; a preview does not). */
const InertLinks = Extension.create({
  name: "inertLinks",

  addProseMirrorPlugins() {
    return [new Plugin({ key: new PluginKey("inertLinks"), props: { handleDOMEvents: { click: keepLink, auxclick: keepLink } } })];
  },
});

/** Rendered attributes as strings (a node view's wrapper sets them on its element). */
function stringAttributes(attributes: Record<string, unknown>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(attributes)) {
    if (value !== null && value !== undefined && value !== false) {
      result[key] = typeof value === "string" ? value : JSON.stringify(value);
    }
  }
  return result;
}

/** The code block and the math block as the preview draws them: the same nodes allowing marks, with no view of their own. */
const TextCodeBlock = CodeBlock.extend({ marks: "_", addNodeView: () => null });
const TextMathBlock = MathBlock.extend({ marks: "_", addNodeView: () => null });

/** The extensions of the preview's editor. */
export function createPreviewExtensions(): AnyExtension[] {
  const view = (component: ComponentType<ReactNodeViewProps>, className: string, type: string, tag?: "span") =>
    ReactNodeViewRenderer(component, {
      ...(tag === undefined ? {} : { as: tag }),
      className,
      attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": type }),
    });
  const schema = createSchemaExtensions({
    assignIds: false,
    pageRef: {
      nodeView: ReactNodeViewRenderer(PreviewPageRef, {
        as: "span",
        className: "page-ref-node",
        attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-page-ref": "" }),
      }),
    },
    pageLink: { nodeView: view(PreviewPageLink, "page-link-node", "pageLink") },
    callout: {
      nodeView: ReactNodeViewRenderer(CalloutView, {
        className: "callout",
        attrs: ({ HTMLAttributes }) => {
          const attributes = stringAttributes(HTMLAttributes);
          return { ...attributes, "data-type": "callout", "data-color": attributes["data-color"] ?? "" };
        },
      }),
    },
    templateButton: { nodeView: view(PreviewTemplateButton, "template-button", "templateButton") },
    image: { nodeView: view(PreviewImage, "attachment-node", "image") },
    file: { nodeView: view(PreviewFile, "attachment-node", "file") },
    gallery: {
      nodeView: ReactNodeViewRenderer(GalleryPlaceholder, {
        className: "attachment-node",
        attrs: ({ HTMLAttributes }) => {
          const attributes = stringAttributes(HTMLAttributes);
          delete attributes["data-items"]; // a long JSON text no one reads
          return { ...attributes, "data-type": "gallery" };
        },
      }),
    },
    whiteboard: { nodeView: view(WhiteboardPlaceholder, "attachment-node", "whiteboard") },
    tableOfContents: { nodeView: view(TableOfContentsPlaceholder, "toc-node", "tableOfContents") },
    breadcrumb: { nodeView: view(BreadcrumbPlaceholder, "breadcrumb-node", "breadcrumb") },
    pageList: { nodeView: view(PageListPlaceholder, "page-list-node", "pageList") },
  });
  const drawn = schema.map((extension) => {
    switch (extension.name) {
      case "codeBlock":
        return TextCodeBlock;
      case "mathBlock":
        return TextMathBlock;
      default:
        return extension;
    }
  });
  return [...drawn, HistoryDiffAttribute, HistoryDiffMark, HistoryDiffDecorations, InertLinks];
}

let cached: Schema | undefined;

/** The preview's schema (built once). */
export function previewSchema(): Schema {
  cached ??= getSchema(createPreviewExtensions());
  return cached;
}

/** Whether the schema can hold the document (an unknown node or mark of a newer version cannot be drawn). */
export function isShowable(doc: PortableNode): boolean {
  try {
    previewSchema().nodeFromJSON(doc);
    return true;
  } catch {
    return false;
  }
}

/** What the editor opens on when a page has no blocks: one empty paragraph. */
export const EMPTY_CONTENT: JSONContent = { type: "doc", content: [{ type: "paragraph" }] };

/** The statuses of a block that show a difference. */
function differs(status: unknown): boolean {
  return status === "added" || status === "removed" || status === "changed";
}

/** The node with every toggle that holds a difference opened (a closed one would hide it), and whether it holds one. */
function openChanged(node: PortableNode): { node: PortableNode; changed: boolean } {
  let changed = differs(node.attrs?.[HISTORY_DIFF_ATTRIBUTE]) || (node.marks ?? []).some((mark) => mark.type === HISTORY_DIFF_MARK);
  let content = node.content;
  if (content !== undefined) {
    const children = content.map(openChanged);
    changed = changed || children.some((child) => child.changed);
    if (children.some((child, index) => child.node !== content?.[index])) {
      content = children.map((child) => child.node);
    }
  }
  if (!changed) {
    return { node, changed };
  }
  const opened = node.type === "details" ? { attrs: { ...node.attrs, open: true } } : {};
  return { node: { ...node, ...opened, ...(content === undefined ? {} : { content }) }, changed };
}

/**
 * The content the editor opens on: the document (merged or the version alone), a page without blocks as one empty paragraph,
 * and, when it shows differences, the toggles that hold one opened.
 */
export function previewContent(doc: PortableNode, withChanges: boolean): PortableNode {
  if ((doc.content?.length ?? 0) === 0) {
    return EMPTY_CONTENT as PortableNode;
  }
  return withChanges ? openChanged(doc).node : doc;
}
