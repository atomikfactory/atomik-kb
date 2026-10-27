// Canned sources for the import dialogs of the in-memory core (`MemoryTransportOptions.importSources`,
// ADR-0036): what the dev page (`/dev.html`) offers when the person picks "Files", "Folder" or "a
// .zip" in the Import dialog, since there is no real disk to pick from. Built in code, so that no
// binary file is checked in.

import { demoImagePng } from "./demo-image";
import type { MemoryImportSource } from "./memory-import";
import { uuidV5 } from "./uuid-v5";

/** The Notion ids of the sample export (32 lowercase hex digits after the name, as Notion writes them). */
const START_ID = "0123456789abcdef0123456789abcdef";
const NOTES_ID = "11111111111111111111111111111111";
const TASKS_ID = "fedcba9876543210fedcba9876543210";
const ROW_ID = "22222222222222222222222222222222";

/** A small Notion "Markdown & CSV" export: a page, a sub-page, a database of two rows and a picture. */
function notionExport(): MemoryImportSource {
  const start = `Getting Started ${START_ID}`;
  const tasks = `Task List ${TASKS_ID}`;
  const database = ["Name,Status,Done,Due", 'Write the report,Todo,No,"October 12, 2026"', "Send it,Done,Yes,2026-10-10", ""].join("\n");
  return {
    kind: "zip",
    files: {
      [`${start}.md`]: [
        "# Getting Started",
        "",
        "Welcome to the **import demo**. This page, its sub-page and the task list below came from a Notion export.",
        "",
        `![A tiny picture](${start.replaceAll(" ", "%20")}/picture.png)`,
        "",
        `See the [notes](${start.replaceAll(" ", "%20")}/Notes%20${NOTES_ID}.md) and the [task list](Task%20List%20${TASKS_ID}.csv).`,
        "",
        "<aside>",
        "💡 Callouts survive too.",
        "</aside>",
        "",
      ].join("\n"),
      [`${start}/Notes ${NOTES_ID}.md`]: "# Notes\n\nA sub-page with a list:\n\n- one\n- two\n",
      [`${start}/picture.png`]: demoImagePng(),
      [`${tasks}.csv`]: database,
      [`${tasks}_all.csv`]: database,
      [`${tasks}/Write the report ${ROW_ID}.md`]: "# Write the report\n\nStatus: Todo\nDone: No\n\nDraft it on Monday, review on Friday.\n",
    },
  };
}

/** A small folder of Markdown notes as Obsidian writes them: front matter, a wiki link, a sub-folder. */
function markdownFolder(): MemoryImportSource {
  return {
    kind: "folder",
    files: {
      "Welcome.md": [
        "---",
        "tags: [demo, notes]",
        "status: active",
        "rating: 4",
        "---",
        "# Welcome",
        "",
        "A note with front matter, a wiki link to [[Ideas]] and a [regular link](Projects/Plan.md).",
        "",
      ].join("\n"),
      "Ideas.md": "# Ideas\n\n- Try the import\n- Export it again\n\nBack to [[Welcome]].\n",
      "Projects/Plan.md": "---\nstatus: planned\nrating: 2\n---\n# Plan\n\nA page in a sub-folder.\n",
    },
  };
}

/** Two loose files, as the "Files" choice of the dialog picks them. */
function looseFiles(): MemoryImportSource {
  return {
    kind: "files",
    files: {
      "Meeting notes.md": "# Meeting notes\n\n- [ ] Send the minutes\n- [x] Book the room\n",
      "Shopping list.txt": "milk\neggs\nbread\n",
    },
  };
}

// ----- an AppFlowy backup (ADR-0039) -------------------------------------------------------------

/** A text operation of an AppFlowy delta: the text, and its attributes when it has any. */
export type Op = { insert: string; attributes?: Record<string, unknown> };

/** A block of a synthetic AppFlowy document: its type, data, text and children. */
export interface BlockSpec {
  ty: string;
  data?: Record<string, unknown>;
  ops?: Op[];
  children?: BlockSpec[];
}

/** Plain text (exported for the tests of the AppFlowy import). */
export function plain(insert: string): Op {
  return { insert };
}

export function styled(insert: string, attributes: Record<string, unknown>): Op {
  return { insert, attributes };
}

/** A block of text: the delta goes into the document's text map, as the backup does. */
export function text(ty: string, ops: Op[], data: Record<string, unknown> = {}, children: BlockSpec[] = []): BlockSpec {
  return { ty, ops, data, children };
}

/** A document of an AppFlowy backup (`collab_jsons/documents/<id>.json`): a block map, a children map and a text map of deltas. */
export function appFlowyDocument(pageId: string, blocks: readonly BlockSpec[]): string {
  const map: Record<string, unknown> = {};
  const childrenMap: Record<string, string[]> = {};
  const textMap: Record<string, string> = {};
  let count = 0;
  const add = (spec: BlockSpec, parent: string): string => {
    count += 1;
    const n = String(count);
    const id = `block-${n}`;
    const hasText = spec.ops !== undefined;
    if (hasText) {
      textMap[`text-${n}`] = JSON.stringify(spec.ops);
    }
    childrenMap[`children-${n}`] = (spec.children ?? []).map((child) => add(child, id));
    map[id] = {
      id,
      ty: spec.ty,
      parent,
      children: `children-${n}`,
      external_id: hasText ? `text-${n}` : null,
      external_type: hasText ? "text" : null,
      data: spec.data ?? {},
    };
    return id;
  };
  childrenMap.root = blocks.map((block) => add(block, pageId));
  map[pageId] = { id: pageId, ty: "page", parent: "", children: "root", external_id: null, external_type: null, data: {} };
  return JSON.stringify({ page_id: pageId, blocks: map, meta: { children_map: childrenMap, text_map: textMap } });
}

/** An id of the synthetic workspace: `prefix` tells the kinds apart, `n` the item. */
export function afId(prefix: string, n: number): string {
  return `${prefix}0000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/**
 * A small synthetic AppFlowy backup ("Back up your workspace", `docs/architecture/appflowy-import.md` 2),
 * written by hand for the dev page: two spaces ("Personal" and "Work"), nested pages with headings,
 * paragraphs, lists, a to-do list and a page mention, a board database (a text and a single-select field,
 * three rows, one with a row document), one local picture and one image kept on a (fake) server. Imported
 * into new spaces it gives 2 spaces and 9 pages with the picture as an attachment, and one warning for the
 * remote image.
 */
function appFlowyBackup(): MemoryImportSource {
  const workspace = afId("a", 0);
  const personal = afId("a", 1);
  const work = afId("a", 2);
  const welcome = afId("b", 1);
  const projects = afId("b", 2);
  const roadmap = afId("b", 3);
  const meeting = afId("b", 4);
  const reading = afId("b", 5);
  const board = afId("c", 1);
  const database = afId("d", 1);
  const rows = [afId("e", 1), afId("e", 2), afId("e", 3)];
  const fields = { name: afId("f", 1), notes: afId("f", 2), status: afId("f", 3) };
  const options = [afId("0", 1), afId("0", 2), afId("0", 3)];
  const rowDocuments = rows.map((row) => uuidV5(row, "document_id") ?? row);
  const picture = "mK3qWf0dYV4j2u9c8gH7nQe1RZpTbXsLhA6oiNyD5Ew.png";
  const server = "https://files.appflowy.example/workspace/diagram.png";

  const view = (id: string, name: string, parent: string, children: string[], extra: Record<string, unknown> = {}, layout = 0, icon?: string) => ({
    view_id: id,
    name,
    layout,
    parent_id: parent,
    children,
    collab_object_id: id,
    created_at: 1_760_000_000,
    updated_at: 1_760_000_100,
    extra: JSON.stringify(extra),
    icon: icon === undefined ? null : { ty: 0, value: icon },
  });
  const views: Record<string, unknown> = {};
  const put = (value: ReturnType<typeof view>): void => {
    views[value.view_id] = value;
  };
  put(view(personal, "Personal", workspace, [welcome, projects], { is_space: true, space_icon: "interface_essential/home-3", space_permission: 0 }, 0, "🏠"));
  put(view(work, "Work", workspace, [meeting, reading], { is_space: true, space_icon: "interface_essential/briefcase", space_permission: 0 }, 0, "💼"));
  put(view(welcome, "Welcome", personal, [], {}, 0, "👋"));
  put(view(projects, "Projects", personal, [roadmap, board]));
  put(view(roadmap, "Roadmap", projects, []));
  put(view(board, "Tasks board", projects, [], { database_id: database }, 2));
  put(view(meeting, "Meeting notes", work, []));
  put(view(reading, "Reading list", work, []));
  // The row documents are views of the workspace, named "Untitled": they are not pages.
  rowDocuments.forEach((id) => {
    put(view(id, "Untitled", workspace, []));
  });

  const documents: Record<string, string> = {
    [welcome]: appFlowyDocument(welcome, [
      text("heading", [plain("Welcome to the backup")], { level: 1 }),
      text("paragraph", [
        plain("This page came from an AppFlowy backup, with a "),
        styled("bold", { bold: true }),
        plain(" word and a look at the "),
        styled("$", { mention: { type: "page", page_id: roadmap } }),
        plain("."),
      ]),
      text("bulleted_list", [plain("First idea")], {}, [text("bulleted_list", [plain("A nested thought")])]),
      text("bulleted_list", [plain("Second idea")]),
      text("todo_list", [plain("Pack the demo")], { checked: false }),
      text("todo_list", [plain("Read the backup")], { checked: true }),
    ]),
    [projects]: appFlowyDocument(projects, [
      text("paragraph", [plain("Everything under this page is a project.")]),
      { ty: "sub_page", data: { view_id: board } },
    ]),
    [roadmap]: appFlowyDocument(roadmap, [
      text("heading", [plain("Roadmap")], { level: 2 }),
      text("paragraph", [plain("Read "), styled("the plan", { href: "https://example.com/plan" }), plain(" first.")]),
      text("numbered_list", [plain("Draft")], { number: 1 }),
      text("numbered_list", [plain("Review")], { number: 2 }),
      { ty: "divider" },
    ]),
    [meeting]: appFlowyDocument(meeting, [
      text("paragraph", [plain("Notes of the Monday meeting.")]),
      { ty: "image", data: { url: `${meeting}/${picture}`, url_type: 2, align: "center" } },
      { ty: "image", data: { url: server, url_type: 1 } },
    ]),
    [reading]: appFlowyDocument(reading, [
      text("numbered_list", [plain("Dune")], { number: 1 }),
      text("numbered_list", [plain("Hyperion")], { number: 2 }),
    ]),
    [rowDocuments[0] ?? ""]: appFlowyDocument(rowDocuments[0] ?? "", [
      text("heading", [plain("Outline")], { level: 3 }),
      text("paragraph", [plain("Three sections, one chart.")]),
    ]),
  };

  const selectOptions = [
    { id: options[0], name: "To do", color: "Purple" },
    { id: options[1], name: "Doing", color: "Blue" },
    { id: options[2], name: "Done", color: "Green" },
  ];
  const cell = (fieldType: number, data: string) => ({ field_type: fieldType, data, created_at: 1_760_000_000 });
  const titles = ["Write the report", "Review the draft", "Send it"];
  const notes = ["Due Friday", "", "After the review"];
  const databaseJson = JSON.stringify({
    database_id: database,
    views: [
      {
        id: board,
        layout: 2,
        layout_settings: {},
        filters: [],
        sorts: [],
        group_settings: [{ field_id: fields.status, groups: selectOptions.map((option) => ({ id: option.id, visible: true })) }],
        row_orders: rows.map((id) => ({ id, height: 60 })),
        field_orders: [{ id: fields.name }, { id: fields.notes }, { id: fields.status }],
        field_settings: {},
      },
    ],
    fields: [
      { id: fields.name, name: "Name", field_type: 0, is_primary: true, type_options: {} },
      { id: fields.notes, name: "Notes", field_type: 0, is_primary: false, type_options: {} },
      {
        id: fields.status,
        name: "Status",
        field_type: 3,
        is_primary: false,
        type_options: { "3": { content: JSON.stringify({ options: selectOptions, disable_color: false }) } },
      },
    ],
    rows: rows.map((id, at) => ({
      id,
      cells: {
        [fields.name]: cell(0, titles[at] ?? ""),
        [fields.notes]: cell(0, notes[at] ?? ""),
        [fields.status]: cell(3, options[at] ?? ""),
      },
      created_at: 1_760_000_000,
      modified_at: 1_760_000_050,
    })),
    row_metas: Object.fromEntries(
      rows.map((id, at) => [id, { icon_url: at === 0 ? "📝" : "", cover: null, is_document_empty: at !== 0, attachment_count: 0 }]),
    ),
  });

  const files: Record<string, string | Uint8Array> = {
    "metadata.json": JSON.stringify({
      export_info: {
        workspace_id: workspace,
        export_timestamp: 1_760_000_200,
        total_views: Object.keys(views).length,
        version: "1.0",
        omitted_item_count: 0,
      },
    }),
    "relation_map.json": JSON.stringify({
      workspace_id: workspace,
      export_timestamp: 1_760_000_200,
      archive_capabilities: ["content_addressed_attachments_v1"],
      views,
      collab_objects: {},
      dependencies: [],
      workspace_database_meta: [{ database_id: database, view_ids: [board] }],
    }),
    "EXPORT_WARNINGS.json": JSON.stringify({ omitted_item_count: 0, samples: [] }),
    [`collab_jsons/databases/${board}.json`]: databaseJson,
    [`collab_objects/${welcome}.bin`]: new Uint8Array([1, 2, 3, 4]),
    [`local_files/${picture}`]: demoImagePng(),
  };
  for (const [id, json] of Object.entries(documents)) {
    files[`collab_jsons/documents/${id}.json`] = json;
  }
  return { kind: "zip", files };
}

/**
 * The canned sources the dev page's Import dialog picks from (see `MemoryTransportOptions.importSources`): a
 * Notion export as a `.zip`, a folder of Markdown notes, two loose files, and a synthetic AppFlowy backup as a
 * `.zip` (the dialog of "Import AppFlowy workspace..." picks it; a plain `.zip` import picks the Notion one). A
 * fresh copy on every call.
 */
export function demoImportSources(): Record<string, MemoryImportSource> {
  return {
    "Notion export.zip": notionExport(),
    "Markdown notes": markdownFolder(),
    "Loose files": looseFiles(),
    "AppFlowy backup.zip": appFlowyBackup(),
  };
}
