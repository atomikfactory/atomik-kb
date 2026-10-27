// The list view: block schema v1's `pageList` (a leaf atom, group `block`; ADR-0016, amendments of
// 2026-10-07 and 2026-10-08; ADR-0054 for several views and the calendar and gallery layouts). A view is a block in a page document, so it syncs with the page; it is
// *derived*: the document stores the configuration of the view and nothing of the pages it shows,
// which its React view (`PageListView.tsx`) reads from the app's page tree (and from the property
// values of the space for a tag source). The layouts are the list, the table, the board, the calendar and the gallery.
//
// Attributes, besides the block `id`:
//
// - `source`: `{ "kind": "children" }` (the pages under the page that holds the block, the
//   default) or `{ "kind": "tag", "optionId": "<id>" }` (the pages of the space that carry that
//   option of Tags);
// - `layout`: `"list"`, `"table"`, `"board"`, `"calendar"` or `"gallery"` (another value shows as the list);
// - `properties`: the ids of the properties shown on each row (the columns of the table), in order
//   (default none);
// - `sort`: `{ "by": "title" | "updated" | "created" | "<property id>", "dir": "asc" | "desc" }`
//   (default title, ascending);
// - `filters`: `[{ "propertyId", "op", "value"? }]`, combined with AND (default none);
// - `columnWidths`: `{ "<property id>|title": px }` of the table (default none: default widths);
// - `groupBy`: the id of the select or status property the board takes its columns from (default none);
// - `hiddenGroups`: the option ids (and `"none"`) of the board columns that are hidden (default none);
// - `calculations`: `{ "<property id>|title": "<function>" }`, the function the table's footer shows under
//   each column (default none; ADR-0016, amendment of 2026-10-14; `page-table-calculations.ts`);
// - `views`: the list of views of the block (ADR-0054, `page-list-views.ts`), each with its own layout, properties, sort,
//   filter tree and layout settings, all sharing `source`. The attributes above are the **mirror** of the first view
//   (what an older build shows); a block without `views` is one view made from them (default none).
//
// In HTML it is `<div data-type="pageList" data-source data-layout data-properties data-sort
// data-filters data-column-widths data-group-by data-hidden-groups data-calculations data-views>`, the structured
// attributes as JSON text. Markdown renders it as
// nothing (the pages are derived at run time, so a line per page would be stale text in a file).

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";

import { dashboardBlockAttrs } from "./dashboard/dashboard-insert";
import { insertLeafBlock, PAGE_LIST_NODE } from "./derived-blocks";
import { readGroupBy, readHiddenGroups } from "./page-board-model";
import { readFilters } from "./page-list-filters";
import {
  DEFAULT_PAGE_LIST_LAYOUT,
  DEFAULT_PAGE_LIST_SORT,
  DEFAULT_PAGE_LIST_SOURCE,
  readColumnWidths,
  readProperties,
  readSort,
} from "./page-list-model";
import { readCalculations } from "./page-table-calculations";

export { PAGE_LIST_NODE } from "./derived-blocks";

/** The layouts `/table view`, `/board view`, `/calendar view`, `/gallery view` and `/dashboard view` insert (the list is `/list view`). */
export type InsertedLayout = "table" | "board" | "calendar" | "gallery" | "dashboard";

export interface PageListOptions {
  /** How the editor shows the block; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
  /** The page the editor shows: the `children` source lists the pages under it. `null` lists none. */
  pageId: string | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    pageList: {
      /**
       * Inserts a view of the children of the page, by title, where the selection is, the caret in
       * the block after it: a list, or the table, board, calendar or gallery layout when `layout` names one
       * (`/table view`, `/board view`, `/calendar view`, `/gallery view`; a board then asks for its group
       * property and a calendar for its date). `"dashboard"` (`/dashboard view`, ADR-0055) inserts a block whose only view is a
       * dashboard with a count and a chart.
       */
      insertPageList: (layout?: InsertedLayout) => ReturnType;
    };
  }
}

/** A JSON attribute of the HTML form, or `fallback` when the text is missing or is not JSON. */
function parseJson(text: string | null, fallback: unknown): unknown {
  if (text === null) {
    return fallback;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return fallback;
  }
}

/** A source of the HTML form: a source of a kind this version does not know is kept as it is (a copy keeps it). */
function parseSource(text: string | null): unknown {
  const parsed = parseJson(text, DEFAULT_PAGE_LIST_SOURCE);
  return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed) ? parsed : DEFAULT_PAGE_LIST_SOURCE;
}

/**
 * The list view (ADR-0016, amendments of 2026-10-07 and 2026-10-08; ADR-0054): a leaf block with the
 * attributes `source`, `layout`, `properties`, `sort`, `filters`, `columnWidths`, `groupBy`,
 * `hiddenGroups`, `calculations` and `views`. The pages it lists are not in the document.
 */
export const PageList = Node.create<PageListOptions>({
  name: PAGE_LIST_NODE,
  group: "block",
  atom: true,
  selectable: true,

  addOptions() {
    return { nodeView: null, pageId: null };
  },

  addAttributes() {
    return {
      source: {
        default: DEFAULT_PAGE_LIST_SOURCE,
        parseHTML: (element) => parseSource(element.getAttribute("data-source")),
        renderHTML: (attributes) => ({ "data-source": JSON.stringify(attributes.source) }),
      },
      layout: {
        default: DEFAULT_PAGE_LIST_LAYOUT,
        parseHTML: (element) => element.getAttribute("data-layout") ?? DEFAULT_PAGE_LIST_LAYOUT,
        renderHTML: (attributes) => ({ "data-layout": String(attributes.layout) }),
      },
      properties: {
        default: [],
        parseHTML: (element) => readProperties(parseJson(element.getAttribute("data-properties"), [])),
        renderHTML: (attributes) => ({ "data-properties": JSON.stringify(attributes.properties) }),
      },
      sort: {
        default: DEFAULT_PAGE_LIST_SORT,
        parseHTML: (element) => readSort(parseJson(element.getAttribute("data-sort"), null)),
        renderHTML: (attributes) => ({ "data-sort": JSON.stringify(attributes.sort) }),
      },
      // `filters` and `columnWidths` are absent from the document until a control sets them: their
      // default is `null`, which the Loro binding does not write, so that the first change of any
      // other attribute of a block written before them records that attribute and nothing else.
      filters: {
        default: null,
        parseHTML: (element) => {
          const text = element.getAttribute("data-filters");
          return text === null ? undefined : readFilters(parseJson(text, []));
        },
        renderHTML: (attributes) => (attributes.filters === null ? {} : { "data-filters": JSON.stringify(attributes.filters) }),
      },
      columnWidths: {
        default: null,
        parseHTML: (element) => {
          const text = element.getAttribute("data-column-widths");
          return text === null ? undefined : readColumnWidths(parseJson(text, {}));
        },
        renderHTML: (attributes) =>
          attributes.columnWidths === null ? {} : { "data-column-widths": JSON.stringify(attributes.columnWidths) },
      },
      groupBy: {
        default: null,
        parseHTML: (element) => {
          const text = element.getAttribute("data-group-by");
          return text === null ? undefined : readGroupBy(text);
        },
        renderHTML: (attributes) => (typeof attributes.groupBy === "string" ? { "data-group-by": attributes.groupBy } : {}),
      },
      hiddenGroups: {
        default: null,
        parseHTML: (element) => {
          const text = element.getAttribute("data-hidden-groups");
          return text === null ? undefined : readHiddenGroups(parseJson(text, []));
        },
        renderHTML: (attributes) =>
          attributes.hiddenGroups === null ? {} : { "data-hidden-groups": JSON.stringify(attributes.hiddenGroups) },
      },
      calculations: {
        default: null,
        parseHTML: (element) => {
          const text = element.getAttribute("data-calculations");
          return text === null ? undefined : readCalculations(parseJson(text, {}));
        },
        renderHTML: (attributes) =>
          attributes.calculations === null ? {} : { "data-calculations": JSON.stringify(attributes.calculations) },
      },
      // The views of the block (ADR-0054). The raw list is kept as it is (an entry this version cannot read is
      // not dropped here: `parseViews` reads leniently where the view is drawn), and absent until a control writes
      // it (default `null`, not written by the binding).
      views: {
        default: null,
        parseHTML: (element) => {
          const text = element.getAttribute("data-views");
          const parsed = text === null ? undefined : parseJson(text, undefined);
          return Array.isArray(parsed) ? (parsed as unknown[]) : undefined;
        },
        renderHTML: (attributes) => (attributes.views === null ? {} : { "data-views": JSON.stringify(attributes.views) }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="pageList"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": PAGE_LIST_NODE, class: "page-list-block" }, HTMLAttributes)];
  },

  addCommands() {
    return {
      insertPageList:
        (layout) =>
        (props) =>
          insertLeafBlock(
            this.name,
            props,
            layout === undefined ? undefined : layout === "dashboard" ? dashboardBlockAttrs() : { layout },
          ),
    };
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
