// Block schema v1 (ADR-0016, `docs/development/milestone-03.md`, "Block schema v1") as
// Tiptap extensions. Everything that shapes the ProseMirror schema lives here, so the
// editor, the binding tests and the fixture generator (`schema.fixtures.ts`, which writes
// `fixtures/doc/block-schema-v1.json` for the Rust parity test) build the same schema.
// Behaviour that does not touch the schema (placeholder, menus, the Loro binding) is
// added by `extensions.ts`.

import { getSchema } from "@tiptap/core";
import type { AnyExtension, Mark, NodeViewRendererProps } from "@tiptap/core";
import { Details, DetailsContent } from "@tiptap/extension-details";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Table, TableCell, TableHeader, TableRow } from "@tiptap/extension-table";
import { UniqueID } from "@tiptap/extension-unique-id";
import type { Schema } from "@tiptap/pm/model";
import { Plugin } from "@tiptap/pm/state";
import type { Transaction } from "@tiptap/pm/state";
import { tableEditingKey } from "@tiptap/pm/tables";
import { StarterKit } from "@tiptap/starter-kit";

import { FileBlock, ImageBlock } from "./attachment-nodes";
import type { AttachmentNodeOptions } from "./attachment-nodes";
import { Breadcrumb } from "./breadcrumb";
import type { BreadcrumbOptions } from "./breadcrumb";
import { Callout } from "./callout";
import type { CalloutOptions } from "./callout";
import { CodeBlock } from "./code-block";
import { Column, ColumnList } from "./columns";
import { DateMention } from "./date-mention";
import { Gallery } from "./gallery-nodes";
import { Whiteboard } from "./whiteboard-nodes";
import type { WhiteboardOptions } from "./whiteboard-nodes";
import type { GalleryOptions } from "./gallery-nodes";
import type { DateMentionOptions } from "./date-mention";
import { loroSyncPluginKey } from "./loro-binding";
import { MathBlock } from "./math-block";
import { MathInline } from "./math-inline";
import type { MathInlineOptions } from "./math-inline";
import { PageLink } from "./page-link";
import type { PageLinkOptions } from "./page-link";
import { PageList } from "./page-list";
import type { PageListOptions } from "./page-list";
import { PageRef } from "./page-ref";
import type { PageRefOptions } from "./page-ref";
import { PersonMention } from "./person-mention";
import type { PersonMentionOptions } from "./person-mention";
import { TableOfContents } from "./table-of-contents";
import type { TableOfContentsOptions } from "./table-of-contents";
import { TemplateButton } from "./template-button";
import type { TemplateButtonOptions } from "./template-button";
import { Highlight, TextColor } from "./text-color";
import { ToggleSummary } from "./toggle-heading";

/**
 * The nodes that carry a block `id` (a random UUID): every node of the schema except
 * `doc`, `hardBreak` and `text`. `UniqueID` assigns it on creation and renames
 * duplicates (paste, duplicate); the first half of a split block keeps it.
 */
export const ID_BEARING_NODES: readonly string[] = [
  "paragraph",
  "heading",
  "bulletList",
  "orderedList",
  "listItem",
  "taskList",
  "taskItem",
  "blockquote",
  "codeBlock",
  "mathBlock",
  "horizontalRule",
  "details",
  "detailsSummary",
  "detailsContent",
  "callout",
  "pageRef",
  "pageLink",
  "image",
  "file",
  "gallery",
  "whiteboard",
  "table",
  "tableRow",
  "tableCell",
  "tableHeader",
  "columnList",
  "column",
  "tableOfContents",
  "breadcrumb",
  "pageList",
  "dateMention",
  "mathInline",
  "templateButton",
];

/** The attribute holding a block's id; UniqueID renders it as `data-id` in the DOM. */
export const BLOCK_ID_ATTRIBUTE = "id";

/** Marks that must not expand over text typed at their end (shape v1: expand `none`). */
const NON_INCLUSIVE_MARKS = new Set(["code", "link"]);

/**
 * True for the transactions the Loro binding dispatches itself: the initial load and
 * remote changes. Their ids come from the document and must never be rewritten.
 */
export function isBindingTransaction(tr: Transaction): boolean {
  const meta: unknown = tr.getMeta(loroSyncPluginKey);
  if (typeof meta !== "object" || meta === null || !("type" in meta)) {
    return false;
  }
  return meta.type === "non-local-updates" || meta.type === "update-state";
}

/**
 * StarterKit with `code` and `link` non-inclusive, so the expansion the binding derives
 * from the schema (`inclusive` -> Loro `expand`) is the shape v1 table: bold, italic,
 * underline and strike expand after, code and link do not (the colour marks are inclusive
 * too, see `text-color.ts`).
 */
const Kit = StarterKit.extend({
  addExtensions() {
    const extensions = this.parent?.() ?? [];
    return extensions.map((extension) =>
      NON_INCLUSIVE_MARKS.has(extension.name) ? (extension as Mark).extend({ inclusive: false }) : extension,
    );
  },
});

/**
 * Toggles keep their block id. Tiptap's own toggle button rewrites the node's attributes
 * as `{ open }` alone, which drops the `id` (and UniqueID then invents a new one); this
 * node view changes `open` only, and leaves read-only editors to Tiptap (no write there).
 */
const StableDetails = Details.extend({
  addNodeView() {
    const createView = this.parent?.();
    if (createView === null || createView === undefined) {
      return null;
    }
    return (props: NodeViewRendererProps) => {
      const view = createView(props);
      const dom = view.dom;
      if (!(dom instanceof HTMLElement)) {
        return view;
      }
      const toggle = dom.querySelector(":scope > button");
      dom.addEventListener(
        "click",
        (event) => {
          const { editor, getPos } = props;
          if (!(toggle instanceof HTMLElement) || !(event.target instanceof Node) || !toggle.contains(event.target)) {
            return;
          }
          if (!editor.isEditable) {
            return; // Tiptap only toggles the view then
          }
          event.stopPropagation(); // capture phase: the button's own handler never runs
          const pos = getPos();
          const node = pos === undefined ? null : editor.state.doc.nodeAt(pos);
          if (pos === undefined || node?.type.name !== "details") {
            return;
          }
          editor.view.dispatch(editor.state.tr.setNodeAttribute(pos, "open", node.attrs.open !== true));
        },
        true,
      );
      return view;
    };
  },
});

/**
 * Tables are repaired in place by prosemirror-tables after every transaction (`fixTables`
 * adds the cells a short row lacks), remote changes included. Concurrent row and column edits
 * can leave a row short in a merged document, and then every replica writes the same repair
 * at once: the repairs merge into more cells than were missing, which are repaired in turn,
 * and the table grows with every exchange. So a change that comes from the document is never
 * repaired (the table shows as the document holds it, short row included); the next local
 * edit of the table repairs it, on that replica alone.
 */
const SyncSafeTable = Table.extend({
  addProseMirrorPlugins() {
    return (this.parent?.() ?? []).map((plugin) => {
      const { appendTransaction } = plugin.spec;
      if (plugin.spec.key !== tableEditingKey || appendTransaction === undefined) {
        return plugin;
      }
      return new Plugin({
        ...plugin.spec,
        appendTransaction: (transactions, oldState, newState) =>
          transactions.some(isBindingTransaction) ? null : appendTransaction.call(plugin, transactions, oldState, newState),
      });
    });
  },
});

export interface SchemaExtensionOptions {
  /** Options of the page reference node (its chip and label). */
  pageRef?: Partial<PageRefOptions>;
  /** Options of the page link block (its card and label). */
  pageLink?: Partial<PageLinkOptions>;
  /** Options of the date mention node (its chip). */
  dateMention?: Partial<DateMentionOptions>;
  /** Options of the person mention node (its chip, the space whose members it names). */
  personMention?: Partial<PersonMentionOptions>;
  /** Options of the inline equation node (its chip and popover). */
  mathInline?: Partial<MathInlineOptions>;
  /** Options of the callout node (its view). */
  callout?: Partial<CalloutOptions>;
  /** Options of the template button node (its view, the people "me" is found among). */
  templateButton?: Partial<TemplateButtonOptions>;
  /** Options of the image block (its view). */
  image?: Partial<AttachmentNodeOptions>;
  /** Options of the file block (its view, the file name for copies). */
  file?: Partial<AttachmentNodeOptions>;
  /** Options of the photo gallery block (its view). */
  gallery?: Partial<GalleryOptions>;
  /** Options of the whiteboard block (its view). */
  whiteboard?: Partial<WhiteboardOptions>;
  /** Options of the table of contents block (its view). */
  tableOfContents?: Partial<TableOfContentsOptions>;
  /** Options of the breadcrumb block (its view, the page it shows the chain of). */
  breadcrumb?: Partial<BreadcrumbOptions>;
  /** Options of the list view block (its view, the page whose children it lists). */
  pageList?: Partial<PageListOptions>;
  /**
   * Whether `UniqueID` writes ids into the document (default true). Read-only editors
   * turn it off; the `id` attribute is part of the schema either way.
   */
  assignIds?: boolean;
}

/** The extensions that define block schema v1. */
export function createSchemaExtensions(options: SchemaExtensionOptions = {}): AnyExtension[] {
  return [
    Kit.configure({
      undoRedo: false, // undo and redo are Loro's (ADR-0014)
      trailingNode: false, // an automatic trailing paragraph would be a write on every replica
      heading: { levels: [1, 2, 3] },
      codeBlock: false, // replaced by `CodeBlock` below: same node, plus highlighting and a language picker
      link: {
        openOnClick: false, // the editor's own click handler opens links through the shell
        // Only `href` is stored (shape v1): no default target, rel or class.
        HTMLAttributes: { target: null, rel: null, class: null },
      },
    }),
    CodeBlock,
    // The colour marks (ADR-0016, amendment of 2026-10-07): `textColor` and `highlight`, one `color`
    // attribute each (a palette name), expanding after like bold, one colour at a time.
    TextColor,
    Highlight,
    // The math equation block (ADR-0016, amendment of 2026-10-06): LaTeX as text, typeset in its view.
    MathBlock,
    TaskList,
    TaskItem.configure({ nested: true }),
    StableDetails.configure({ persist: true }),
    // The summary of a toggle, with the optional `level` that makes it a toggle heading (ADR-0016,
    // amendment of 2026-10-14; ADR-0040).
    ToggleSummary,
    DetailsContent,
    Callout.configure(options.callout ?? {}),
    // The template button (ADR-0056): a block that holds blocks and inserts a fresh copy of them, below or above itself, when
    // it is clicked; content `block+`, the attributes `label` and `position`.
    TemplateButton.configure(options.templateButton ?? {}),
    PageRef.configure(options.pageRef ?? {}),
    // The date mention (ADR-0016, amendment of 2026-10-07): an inline atom with a floating local
    // `date` (`YYYY-MM-DD`) and an optional `time` (`HH:mm`); it reads as its ISO text.
    DateMention.configure(options.dateMention ?? {}),
    // The person mention (ADR-0049, M13 slice 2): an inline atom with an `accountId` and a fallback `label`; no
    // block id. It shows the member's current name, or "Former member (label)".
    PersonMention.configure(options.personMention ?? {}),
    // The inline equation (ADR-0016, amendment of 2026-10-07): an inline atom with a `latex` source
    // (a string), typeset in its view; it reads as its source, and as `$latex$` in Markdown.
    MathInline.configure(options.mathInline ?? {}),
    // The card that links to a page (ADR-0016, amendment of 2026-10-06): a leaf block with
    // `pageId` and `blockId`, the block twin of the `pageRef` chip.
    PageLink.configure(options.pageLink ?? {}),
    ImageBlock.configure(options.image ?? {}),
    FileBlock.configure(options.file ?? {}),
    // The photo gallery (ADR-0016, amendment of 2026-10-09; ADR-0030): a leaf block with a list of
    // pictures (`items`, each an attachment and an optional caption) and a column count.
    Gallery.configure(options.gallery ?? {}),
    // The whiteboard (ADR-0016, amendment of 2026-10-10; ADR-0033): a leaf block with a `height`; its
    // elements are not in the node but in the root map `whiteboards` of the page document.
    Whiteboard.configure(options.whiteboard ?? {}),
    // Simple tables (ADR-0016, amendment of 2026-10-05): Tiptap's own nodes with their default
    // content and attributes (`colspan`, `rowspan`, `colwidth`, `align` stay at their defaults:
    // no resize and no merge UI), each carrying a block id.
    SyncSafeTable.configure({ resizable: false }),
    TableRow,
    TableHeader,
    TableCell,
    // Columns layout (ADR-0016, amendment of 2026-10-06): a column list of two to five columns,
    // each holding blocks, each node carrying a block id. No column list inside a column: the
    // commands refuse and the list rejects a local transaction that adds one (binding
    // transactions pass).
    ColumnList.configure({ isBindingTransaction }),
    Column,
    // The derived blocks (ADR-0016, amendment of 2026-10-06): leaves with an `id` and nothing else;
    // their views draw the page's headings and its place in the page tree, and write nothing.
    TableOfContents.configure(options.tableOfContents ?? {}),
    Breadcrumb.configure(options.breadcrumb ?? {}),
    // The list view (ADR-0016, amendment of 2026-10-07): a derived leaf that also holds the
    // configuration of its view (`source`, `layout`, `properties`, `sort`); its view draws the pages.
    PageList.configure(options.pageList ?? {}),
    UniqueID.configure({
      attributeName: BLOCK_ID_ATTRIBUTE,
      types: [...ID_BEARING_NODES],
      generateID: () => crypto.randomUUID(),
      filterTransaction: (tr) => !isBindingTransaction(tr),
      updateDocument: options.assignIds ?? true,
    }),
  ];
}

/** The ProseMirror schema of block schema v1, built from the live extensions. */
export function createEditorSchema(): Schema {
  return getSchema(createSchemaExtensions());
}
