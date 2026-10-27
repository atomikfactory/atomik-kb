// What `dev.tsx` turns its query parameters into: the starting situation of the
// in-memory core. Kept apart from the entry point so it can be tested without rendering.

import {
  AI_SETTINGS_SETTING,
  CHUNK_SIZE,
  CoreClient,
  DEFAULT_AI_ENDPOINT,
  DOWNLOAD_POLICY_ON_DEMAND,
  DRIVE_ERROR_REASONS,
  MemoryTransport,
  demoImagePng,
  demoPdf,
  demoWav,
} from "@atomik-kb/core-client";
import type {
  DriveErrorReason,
  JsonValue,
  MemoryAttachmentsOptions,
  MemoryDriveBackup,
  MemoryDriveOperation,
  MemoryDriveOptions,
  MemoryNotificationSeed,
  MemoryPresencePeer,
  MemoryRemoteAttachmentSeed,
  MemorySharedSpaceSeed,
  MemorySharingOptions,
  MemorySyncMode,
  MemoryThreadSeed,
  MemoryTransportOptions,
  RemoteImage,
  SeedPage,
  SeedSpace,
  Transport,
} from "@atomik-kb/core-client";

/** Looks a demo page's id up by its title (`undefined` while the ids are not known yet). */
type PageIdOf = (title: string) => string | undefined;

/** Ids of the demo blocks: UUIDs, as the core indexes only those. */
function blockId(n: number): string {
  return `de000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function text(value: string, mark?: string): JsonValue {
  return mark === undefined ? { type: "text", text: value } : { type: "text", text: value, marks: [{ type: mark }] };
}

function paragraph(id: number, ...content: JsonValue[]): JsonValue {
  return content.length === 0
    ? { type: "paragraph", attrs: { id: blockId(id) } }
    : { type: "paragraph", attrs: { id: blockId(id) }, content };
}

function heading(id: number, level: number, value: string): JsonValue {
  return { type: "heading", attrs: { id: blockId(id), level }, content: [text(value)] };
}

/** The demo's attachments, in the first space ("Personal"), shown on "Welcome". */
export const DEMO_IMAGE_ID = "de000000-0000-4000-8000-00000000a001";
export const DEMO_FILE_ID = "de000000-0000-4000-8000-00000000a002";
/** A three-page PDF the demo shows as a preview (ADR-0031). */
export const DEMO_PDF_ID = "de000000-0000-4000-8000-00000000a003";
/** A three-second tone the demo shows as a player (ADR-0032). */
export const DEMO_AUDIO_ID = "de000000-0000-4000-8000-00000000a004";

const DEMO_ATTACHMENTS: MemoryTransportOptions["attachments"] = [
  { attachmentId: DEMO_IMAGE_ID, name: "atomik.png", mime: "image/png", bytes: demoImagePng() },
  {
    attachmentId: DEMO_FILE_ID,
    name: "packing-list.txt",
    mime: "text/plain",
    bytes: new TextEncoder().encode("Toothbrush\nCharger\nA good book\n"),
  },
  { attachmentId: DEMO_PDF_ID, name: "atomik-demo.pdf", mime: "application/pdf", bytes: demoPdf(3) },
  { attachmentId: DEMO_AUDIO_ID, name: "atomik-tone.wav", mime: "audio/wav", bytes: demoWav(3) },
];

/** The day `offset` days from today on this machine's clock, `YYYY-MM-DD` (a date mention's floating local date). */
export function localDay(offset = 0): string {
  const day = new Date();
  day.setDate(day.getDate() + offset);
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${String(day.getFullYear())}-${pad(day.getMonth() + 1)}-${pad(day.getDate())}`;
}

/**
 * A date mention (an inline atom): a day, a time of day when `time` is given and a reminder (minutes
 * before its moment, ADR-0024) when `reminder` is.
 */
function dateMention(id: number, date: string, time?: string, reminder?: number): JsonValue {
  const attrs: { [key: string]: JsonValue } = { id: blockId(id), date };
  if (time !== undefined) {
    attrs.time = time;
  }
  if (reminder !== undefined) {
    attrs.reminder = reminder;
  }
  return { type: "dateMention", attrs };
}

/**
 * A moment two minutes from `now` on this machine's clock, rounded up to the minute (so it is
 * between two and three minutes away): its day and its `HH:mm`. The demo's reminder fires then.
 */
export function inTwoMinutes(now: Date = new Date()): { date: string; time: string } {
  const moment = new Date(Math.ceil((now.getTime() + 120_000) / 60_000) * 60_000);
  const pad = (value: number): string => String(value).padStart(2, "0");
  return {
    date: `${String(moment.getFullYear())}-${pad(moment.getMonth() + 1)}-${pad(moment.getDate())}`,
    time: `${pad(moment.getHours())}:${pad(moment.getMinutes())}`,
  };
}

/** An inline equation (an inline atom): LaTeX, typeset by the editor. */
function mathInline(id: number, latex: string): JsonValue {
  return { type: "mathInline", attrs: { id: blockId(id), latex } };
}

/** A paragraph `label` + a reference to the page titled `title`, when its id is known. */
function reference(id: number, label: string, title: string, pageIdOf: PageIdOf): JsonValue[] {
  const pageId = pageIdOf(title);
  if (pageId === undefined) {
    return [];
  }
  return [paragraph(id, text(label), { type: "pageRef", attrs: { id: blockId(id + 500), pageId } })];
}

/** A card linking to the page titled `title` (a `pageLink` block), when its id is known. */
function pageCard(id: number, title: string, pageIdOf: PageIdOf): JsonValue[] {
  const pageId = pageIdOf(title);
  return pageId === undefined ? [] : [{ type: "pageLink", attrs: { id: blockId(id), pageId } }];
}

/** "Welcome": every kind of block, to look at and edit, and links to two other pages. */
function welcome(pageIdOf: PageIdOf): JsonValue {
  const soon = inTwoMinutes();
  return {
    type: "doc",
    content: [
      heading(1, 1, "Welcome to atomik-kb"),
      // A table of contents of this page's headings (derived: the document stores the block only).
      { type: "tableOfContents", attrs: { id: blockId(70) } },
      paragraph(
        2,
        text("Write with "),
        text("bold", "bold"),
        text(", "),
        text("italic", "italic"),
        text(", "),
        text("underline", "underline"),
        text(", "),
        text("strike", "strike"),
        text(", "),
        text("code", "code"),
        text(" and "),
        { type: "text", text: "links", marks: [{ type: "link", attrs: { href: "https://example.com" } }] },
        text(". Type / for blocks and [[ to link a page."),
      ),
      // Dates (ids 90 to 92): `@` and `/date` add more; "Today" and the day after next week follow the clock.
      paragraph(
        90,
        text("Type @ for a day or a page, or /date: today is "),
        dateMention(91, localDay()),
        text(", the review is on "),
        dateMention(92, localDay(7), "10:00"),
        text("."),
      ),
      // Inline equations (ids 110 to 112): `$x$` and `/inline equation` add more; a click edits the LaTeX.
      paragraph(
        110,
        text("Type $E=mc^2$ or /inline equation for a formula inside a line: energy is "),
        mathInline(111, "E=mc^2"),
        text(" and a circle covers "),
        mathInline(112, String.raw`A = \pi r^2`),
        text("."),
      ),
      // Reminders (ids 100 to 102): the first fires in two to three minutes (a toast shows in this page, 15 seconds
      // at most after the moment); the second reminds a day before 9:00 in three days. Open a date to change either.
      paragraph(
        100,
        text("Reminder: this one fires in about two minutes, "),
        dateMention(101, soon.date, soon.time, 0),
        text(", and this one a day ahead of 9:00 in three days, "),
        dateMention(102, localDay(3), undefined, 1440),
        text("."),
      ),
      heading(3, 2, "Lists"),
      {
        type: "bulletList",
        attrs: { id: blockId(4) },
        content: [
          {
            type: "listItem",
            attrs: { id: blockId(5) },
            content: [
              paragraph(6, text("Bullets nest with Tab")),
              {
                type: "bulletList",
                attrs: { id: blockId(7) },
                content: [{ type: "listItem", attrs: { id: blockId(8) }, content: [paragraph(9, text("and Shift+Tab"))] }],
              },
            ],
          },
        ],
      },
      {
        type: "orderedList",
        attrs: { id: blockId(10), start: 1 },
        content: [
          { type: "listItem", attrs: { id: blockId(11) }, content: [paragraph(12, text("First"))] },
          { type: "listItem", attrs: { id: blockId(13) }, content: [paragraph(14, text("Second"))] },
        ],
      },
      {
        type: "taskList",
        attrs: { id: blockId(15) },
        content: [
          { type: "taskItem", attrs: { id: blockId(16), checked: true }, content: [paragraph(17, text("Try the editor"))] },
          {
            type: "taskItem",
            attrs: { id: blockId(18), checked: false },
            content: [paragraph(19, text("Drag a block by its handle"))],
          },
        ],
      },
      heading(20, 2, "More blocks"),
      {
        type: "details",
        attrs: { id: blockId(21), open: false },
        content: [
          { type: "detailsSummary", attrs: { id: blockId(22) }, content: [text("A toggle")] },
          { type: "detailsContent", attrs: { id: blockId(23) }, content: [paragraph(24, text("Hidden until opened."))] },
        ],
      },
      { type: "callout", attrs: { id: blockId(25), icon: "💡" }, content: [paragraph(26, text("Callouts make a note stand out."))] },
      { type: "blockquote", attrs: { id: blockId(27) }, content: [paragraph(28, text("A quoted line."))] },
      {
        type: "codeBlock",
        attrs: { id: blockId(29), language: "rust" },
        content: [text('fn main() {\n    println!("hello");\n}')],
      },
      {
        type: "codeBlock",
        attrs: { id: blockId(37), language: "mermaid" },
        content: [text("flowchart LR\n  Write[Write a page] --> Sync[Sync, encrypted] --> Read[Read anywhere]")],
      },
      {
        type: "mathBlock",
        attrs: { id: blockId(38) },
        content: [text("\\int_0^1 x^2\\,dx = \\frac{1}{3}")],
      },
      {
        // Two columns side by side (ids 60 to 64: 40 to 50 belong to the other demo pages).
        type: "columnList",
        attrs: { id: blockId(60) },
        content: [
          {
            type: "column",
            attrs: { id: blockId(61) },
            content: [paragraph(62, text("Left column: type /columns to put blocks side by side."))],
          },
          {
            type: "column",
            attrs: { id: blockId(63) },
            content: [paragraph(64, text("Right column: the toolbar adds, removes or unwraps columns."))],
          },
        ],
      },
      { type: "horizontalRule", attrs: { id: blockId(30) } },
      heading(33, 2, "Images and files"),
      paragraph(34, text("Drop or paste files here, or type /image or /file.")),
      { type: "image", attrs: { id: blockId(35), attachmentId: DEMO_IMAGE_ID, alt: "A tiny demo picture", width: 96 } },
      { type: "file", attrs: { id: blockId(36), attachmentId: DEMO_FILE_ID } },
      // A PDF read in the page (ADR-0031): a file block with `view: "preview"`, lazily drawn page by page.
      paragraph(162, text("A PDF viewer: /pdf picks a PDF and shows it here; the block's menu switches it to a card.")),
      { type: "file", attrs: { id: blockId(163), attachmentId: DEMO_PDF_ID, view: "preview" } },
      // A sound played in the page (ADR-0032): /audio and /video pick a file; the menu switches to a card.
      paragraph(164, text("A player: /audio and /video pick a file and play it here; the block's menu switches it to a card.")),
      { type: "file", attrs: { id: blockId(165), attachmentId: DEMO_AUDIO_ID, view: "preview" } },
      // A photo gallery (ADR-0030): four pictures of the one demo image, told apart by their captions.
      paragraph(160, text("A photo gallery: /gallery adds one; drop or paste pictures on it, drag them to reorder.")),
      {
        type: "gallery",
        attrs: {
          id: blockId(161),
          columns: 2,
          items: [
            { attachmentId: DEMO_IMAGE_ID, caption: "Morning" },
            { attachmentId: DEMO_IMAGE_ID, caption: "Noon" },
            { attachmentId: DEMO_IMAGE_ID },
            { attachmentId: DEMO_IMAGE_ID, caption: "Night" },
          ],
        },
      },
      // A whiteboard (ADR-0033): its drawing is not in the block but in the page document's root map
      // `whiteboards`, under this block's id (the portable form of a page carries it as a top-level key).
      paragraph(170, text("A whiteboard: /whiteboard adds one; Edit opens the drawing full screen.")),
      { type: "whiteboard", attrs: { id: blockId(171), height: 280 } },
      ...reference(31, "Where this is going: ", "Roadmap", pageIdOf),
      ...pageCard(80, "Roadmap", pageIdOf),
      ...reference(32, "Work lives in another space: ", "Handbook", pageIdOf),
    ],
    whiteboards: { [blockId(171)]: demoDrawing() },
  };
}

/** The shapes and the text of the demo whiteboard: two boxes, an arrow between them and a label, as Excalidraw writes them. */
function demoDrawing(): JsonValue {
  const base = {
    angle: 0,
    strokeColor: "#1e1e1e",
    fillStyle: "solid",
    strokeWidth: 2,
    strokeStyle: "solid",
    roughness: 1,
    opacity: 100,
    groupIds: [],
    frameId: null,
    roundness: { type: 3 },
    isDeleted: false,
    link: null,
    locked: false,
    updated: 1760000000000,
    versionNonce: 1,
  } satisfies { [key: string]: JsonValue };
  const box = (id: string, index: string, x: number, color: string): { [key: string]: JsonValue } => ({
    ...base,
    id,
    type: "rectangle",
    index,
    x,
    y: 60,
    width: 180,
    height: 90,
    backgroundColor: color,
    seed: x + 7,
    version: 2,
    boundElements: null,
  });
  return {
    "demo-box-a": box("demo-box-a", "a0", 40, "#a5d8ff"),
    "demo-box-b": box("demo-box-b", "a1", 380, "#b2f2bb"),
    "demo-arrow": {
      ...base,
      id: "demo-arrow",
      type: "arrow",
      index: "a2",
      x: 222,
      y: 105,
      width: 156,
      height: 0,
      backgroundColor: "transparent",
      seed: 11,
      version: 2,
      points: [
        [0, 0],
        [156, 0],
      ],
      lastCommittedPoint: null,
      startBinding: null,
      endBinding: null,
      startArrowhead: null,
      endArrowhead: "arrow",
      boundElements: null,
      roundness: { type: 2 },
    },
    "demo-label": {
      ...base,
      id: "demo-label",
      type: "text",
      index: "a3",
      x: 40,
      y: 170,
      width: 330,
      height: 25,
      backgroundColor: "transparent",
      seed: 13,
      version: 2,
      text: "Idea to plan, in two boxes",
      originalText: "Idea to plan, in two boxes",
      fontSize: 20,
      fontFamily: 5,
      textAlign: "left",
      verticalAlign: "top",
      containerId: null,
      autoResize: true,
      lineHeight: 1.25,
      roundness: null,
      boundElements: null,
    },
  };
}

function roadmap(pageIdOf: PageIdOf): JsonValue {
  return {
    type: "doc",
    content: [
      // Where this page sits in the tree (derived: the document stores the block only).
      { type: "breadcrumb", attrs: { id: blockId(71) } },
      heading(40, 2, "Milestones"),
      {
        type: "taskList",
        attrs: { id: blockId(41) },
        content: [
          { type: "taskItem", attrs: { id: blockId(42), checked: true }, content: [paragraph(43, text("Page tree and spaces"))] },
          {
            type: "taskItem",
            attrs: { id: blockId(44), checked: false },
            content: [paragraph(45, text("Page editor, links and search"))],
          },
        ],
      },
      ...reference(46, "Decisions: ", "Design notes", pageIdOf),
    ],
  };
}

/** The ids of the demo's Status and Due properties, fixed so that the list view of "Projects" can name them. */
export const DEMO_STATUS_PROPERTY_ID = "de000000-0000-4000-8000-00000000b001";
export const DEMO_DUE_PROPERTY_ID = "de000000-0000-4000-8000-00000000b002";

/** More fixed ids, for the table and the board of "Tasks" and "Release plan" (relations and rollups, ADR-0027). */
export const DEMO_PRIORITY_PROPERTY_ID = "de000000-0000-4000-8000-00000000b003";
export const DEMO_CREATED_PROPERTY_ID = "de000000-0000-4000-8000-00000000b004";
export const DEMO_EDITED_BY_PROPERTY_ID = "de000000-0000-4000-8000-00000000b005";
export const DEMO_TASKS_PROPERTY_ID = "de000000-0000-4000-8000-00000000b006";
export const DEMO_TASK_COUNT_PROPERTY_ID = "de000000-0000-4000-8000-00000000b007";
export const DEMO_EFFORT_PROPERTY_ID = "de000000-0000-4000-8000-00000000b008";

/** The demo's formulas (ADR-0055): a score from the priority (shown in the table of "Tasks") and the days to a due date. */
export const DEMO_SCORE_PROPERTY_ID = "de000000-0000-4000-8000-00000000b009";
export const DEMO_DUE_IN_PROPERTY_ID = "de000000-0000-4000-8000-00000000b00a";

/**
 * "Tasks": a table and a board (grouped by Status) over the pages under it (block ids 140 and up),
 * with the created time and the last editing device, which the app computes.
 */
function tasks(): JsonValue {
  return {
    type: "doc",
    content: [
      paragraph(140, text("The table of the tasks:")),
      {
        type: "pageList",
        attrs: {
          id: blockId(141),
          source: { kind: "children" },
          layout: "table",
          properties: [
            DEMO_STATUS_PROPERTY_ID,
            DEMO_PRIORITY_PROPERTY_ID,
            DEMO_SCORE_PROPERTY_ID,
            DEMO_CREATED_PROPERTY_ID,
            DEMO_EDITED_BY_PROPERTY_ID,
          ],
          sort: { by: DEMO_PRIORITY_PROPERTY_ID, dir: "desc" },
        },
      },
      paragraph(142, text("The same tasks as a board, by status:")),
      {
        type: "pageList",
        attrs: {
          id: blockId(143),
          source: { kind: "children" },
          layout: "board",
          properties: [DEMO_PRIORITY_PROPERTY_ID],
          sort: { by: "title", dir: "asc" },
          groupBy: DEMO_STATUS_PROPERTY_ID,
        },
      },
      paragraph(144),
    ],
  };
}

/**
 * "Release plan": a table of the pages under it with their tasks (a two-way relation, shown as
 * chips), how many there are and the effort they add up to (rollups over the relation).
 */
function milestones(): JsonValue {
  return {
    type: "doc",
    content: [
      paragraph(150, text("Each milestone with its tasks, how many and their effort:")),
      {
        type: "pageList",
        attrs: {
          id: blockId(151),
          source: { kind: "children" },
          layout: "table",
          properties: [DEMO_STATUS_PROPERTY_ID, DEMO_TASKS_PROPERTY_ID, DEMO_TASK_COUNT_PROPERTY_ID, DEMO_EFFORT_PROPERTY_ID],
          sort: { by: DEMO_EFFORT_PROPERTY_ID, dir: "desc" },
        },
      },
      paragraph(152),
    ],
  };
}

/**
 * "Projects": a list view of the pages under it (the list view block, ids 130 and up), showing Status
 * and Due and sorted by Due, soonest first. The document stores the configuration of the view only;
 * the rows come from the page tree and the values of the pages.
 */
function projects(): JsonValue {
  return {
    type: "doc",
    content: [
      paragraph(130, text("Everything under this page, soonest first:")),
      {
        type: "pageList",
        attrs: {
          id: blockId(131),
          source: { kind: "children" },
          layout: "list",
          properties: [DEMO_STATUS_PROPERTY_ID, DEMO_DUE_PROPERTY_ID],
          sort: { by: DEMO_DUE_PROPERTY_ID, dir: "asc" },
        },
      },
      paragraph(132),
    ],
  };
}

/** The demo spaces; references between pages are left out while `pageIdOf` does not know the ids. */
function demoSpaces(pageIdOf: PageIdOf): readonly SeedSpace[] {
  const page = (title: string, extra: Omit<SeedPage, "title"> = {}): SeedPage => ({ title, ...extra });
  return [
    {
      name: "Personal",
      icon: "🏠",
      // The page properties (ADR-0026): three tags and a property of several kinds, to look at and edit.
      tagOptions: [
        { name: "idea", color: "green" },
        { name: "project", color: "blue" },
        { name: "reading", color: "orange" },
      ],
      properties: [
        { name: "Status", kind: "status", id: DEMO_STATUS_PROPERTY_ID },
        { name: "Due", kind: "date", id: DEMO_DUE_PROPERTY_ID },
        { name: "Done", kind: "checkbox" },
        { name: "Link", kind: "url" },
        { name: "Owner email", kind: "email" },
        { name: "Priority", kind: "number", id: DEMO_PRIORITY_PROPERTY_ID },
        { name: "Files", kind: "files" },
        { name: "Launch checklist", kind: "checklist" },
        // Place and the computed kinds (ADR-0026, amendment of 2026-10-08).
        { name: "Place", kind: "place" },
        { name: "Created", kind: "created_time", id: DEMO_CREATED_PROPERTY_ID },
        { name: "Last edited by", kind: "last_edited_by", id: DEMO_EDITED_BY_PROPERTY_ID },
        // A two-way relation (the tasks of a milestone, and the milestone of a task) and rollups over it (ADR-0027).
        { name: "Tasks", kind: "relation", id: DEMO_TASKS_PROPERTY_ID, twoWay: true, backName: "Milestone" },
        { name: "Task count", kind: "rollup", id: DEMO_TASK_COUNT_PROPERTY_ID, rollup: { relation: "Tasks", target: "title", function: "count_all" } },
        { name: "Effort", kind: "rollup", id: DEMO_EFFORT_PROPERTY_ID, rollup: { relation: "Tasks", target: "Priority", function: "sum" } },
        // Formulas (ADR-0055), written with the names of the properties they read (the seed turns them into ids).
        { name: "Score", kind: "formula", id: DEMO_SCORE_PROPERTY_ID, formula: 'prop("Priority") * 10' },
        {
          name: "Due in",
          kind: "formula",
          id: DEMO_DUE_IN_PROPERTY_ID,
          formula: 'if(empty(prop("Due")), "", format(dateBetween(prop("Due"), today(), "days")) + " days")',
        },
      ],
      pages: [
        page("Welcome", {
          icon: "👋",
          // Two favourites (ADR-0029), so the sidebar shows its Favourites section.
          favorite: true,
          content: welcome(pageIdOf),
          tags: ["idea"],
          properties: {
            Status: "In progress",
            Due: { date: localDay(7) },
            Link: "https://example.com/atomik-kb",
            "Owner email": "owner@example.com",
            Priority: 2,
            // Files and media (ADR-0026, amendment of 2026-10-07): the two demo attachments, as chips.
            Files: [DEMO_IMAGE_ID, DEMO_FILE_ID],
            // A place with a position, and one that is only a name.
            Place: { name: "Café de Flore, Paris", lat: 48.854, lon: 2.333 },
          },
        }),
        page("Projects", {
          icon: "📁",
          content: projects(),
          children: [
            page("atomik-kb", {
              icon: "🧠",
              properties: { Status: "In progress", Due: { date: localDay(14) } },
              children: [
                page("Roadmap", {
                  icon: "🗺️",
                  content: roadmap(pageIdOf),
                  tags: ["project"],
                  properties: {
                    Status: "Not started",
                    Due: { date: localDay(30) },
                    Done: false,
                    Priority: 1,
                    // A checklist with its progress: three items, one done.
                    "Launch checklist": [
                      { text: "Write the release notes", done: true },
                      { text: "Test the installers" },
                      { text: "Announce it" },
                    ],
                  },
                }),
                page("Design notes", {
                  content: {
                    type: "doc",
                    content: [paragraph(50, text("Search finds words in titles and in page text, across spaces."))],
                  },
                }),
              ],
            }),
            page("Garden", { icon: "🌱", properties: { Status: "Not started", Due: { date: localDay(3) } } }),
          ],
        }),
        page("Tasks", {
          icon: "✅",
          content: tasks(),
          children: [
            page("Write release notes", { properties: { Status: "In progress", Priority: 3 } }),
            page("Test the installers", { properties: { Status: "Not started", Priority: 5 } }),
            page("Announce it", { properties: { Status: "Not started", Priority: 1 } }),
            page("Update the docs", { properties: { Status: "Done", Priority: 2 } }),
          ],
        }),
        page("Release plan", {
          icon: "🏁",
          content: milestones(),
          children: [
            page("Beta", {
              properties: {
                Status: "In progress",
                Due: { date: localDay(10) },
                Tasks: ["Write release notes", "Test the installers"],
              },
            }),
            page("Release", {
              properties: {
                Status: "Not started",
                Due: { date: localDay(30) },
                Tasks: ["Announce it", "Update the docs", "Write release notes"],
              },
            }),
          ],
        }),
        page("Reading list", {
          icon: "📚",
          favorite: true,
          tags: ["reading", "idea"],
          properties: {
            Status: "In progress",
            Link: "https://example.com/books",
            Done: false,
            Priority: 3,
            Place: { name: "Public library" },
          },
        }),
        page("Journal", { children: [page("2026")] }),
      ],
    },
    {
      name: "Work",
      icon: "💼",
      // A Person property (ADR-0049): the members of the space (in a personal space, this account alone, "me").
      properties: [{ name: "Owner", kind: "person" }],
      pages: [
        page("Meetings", { children: [page("Weekly sync"), page("Retro")], properties: { Owner: ["me"] } }),
        page("Handbook", { content: { type: "doc", content: [paragraph(60, text("How we work."))] } }),
      ],
    },
    { name: "Archive" },
  ];
}

/** `?seed=demo`: two spaces with nested pages and content, enough to try the sidebar and the editor. */
export const DEMO_SPACES: readonly SeedSpace[] = demoSpaces(() => undefined);

/** Ids that are the same on every run (`00000000-0000-4000-8000-000000000001`, ...). */
function sequentialIds(): () => string {
  let next = 0;
  return () => {
    next += 1;
    return `00000000-0000-4000-8000-${next.toString(16).padStart(12, "0")}`;
  };
}

/** The app-lock passphrase of `?security=lock` and `?security=locked`. */
export const DEV_PASSPHRASE = "correct horse battery";

/** The values of `?sync=`: the simulated sync server the dev page starts connected to. */
const SYNC_MODES: readonly MemorySyncMode[] = ["idle", "offline", "forked", "error"];

/** The server the dev page's `?sync=` connects to. */
export const DEV_SYNC_SERVER = "https://sync.example.com";

/** Whether `value` is one of the `?sync=` modes. */
function isSyncMode(value: string | null): value is MemorySyncMode {
  return SYNC_MODES.some((mode) => mode === value);
}

/** `now` dates the recovery kit of `?security=...` (Unix ms). */
export function optionsFromQuery(query: URLSearchParams, now: number = Date.now()): MemoryTransportOptions {
  const options: MemoryTransportOptions = {
    status: {
      platform: "windows",
      dataDir: "C:\\Users\\you\\AppData\\Roaming\\com.atomik-kb.desktop",
      coreVersion: "0.1.0",
    },
  };
  const workspace = query.get("workspace");
  if (workspace !== null) {
    options.workspaceName = workspace;
  }
  if (query.get("seed") === "demo") {
    applyDemoSeed(options);
  }
  const theme = query.get("theme");
  if (theme !== null) {
    options.settings = { "ui.theme": theme };
  }
  // The stored UI language (`system`, `en`, `fr` or `es`); the app reads anything else as `system`.
  const language = query.get("language");
  if (language !== null) {
    options.settings = { ...options.settings, "ui.language": language };
  }
  const core = query.get("core");
  if (core === "opening") {
    options.coreState = "opening";
  } else if (core === "failed") {
    options.coreState = {
      failed: {
        code: "KeystoreUnavailable",
        message: "no usable keystore: no secret service is running",
      },
    };
  }
  const api = Number(query.get("api"));
  if (Number.isInteger(api) && api > 0) {
    options.status = { ...options.status, apiVersion: api };
  }
  if (query.get("encrypted") === "1") {
    options.status = { ...options.status, dbEncrypted: true, dbCipher: "SQLCipher 4.14.0 community" };
  }
  const security = query.get("security");
  if (security === "kit" || security === "lock" || security === "locked") {
    // A recovery kit belongs to a workspace.
    options.workspaceName ??= "Demo workspace";
    options.security = { kitCreatedAt: now };
    if (security !== "kit") {
      options.security.appLockPassphrase = DEV_PASSPHRASE;
    }
    // While the core is still "opening", `dev.tsx` locks it when it is done instead.
    if (security === "locked" && options.coreState !== "opening") {
      options.security.startLocked = true;
    }
  }
  const sync = query.get("sync");
  if (isSyncMode(sync)) {
    // Sync belongs to a workspace; the mirror's own messages explain Offline and Error.
    options.workspaceName ??= "Demo workspace";
    options.sync = { state: sync, serverUrl: DEV_SYNC_SERVER };
  } else if (sync === "invite") {
    // Sync off, and a server that does not know this account: connecting asks for a token.
    options.workspaceName ??= "Demo workspace";
    options.sync = { accountKnown: false };
  }
  applyAttachmentsMode(options, query.get("attachments"));
  applySharingMode(options, query, now);
  applyPresenceMode(options, query);
  applyDevicesMode(options, query.get("devices"), now);
  applyAiMode(options, query);
  applyDictationMode(options, query);
  applyDriveMode(options, query, now);
  options.remoteImages = devRemoteImages();
  return options;
}

/** The bridge's path the fake MCP status reports on the dev page (the real one is next to the app). */
export const DEV_MCP_BRIDGE_PATH = "C:\\Program Files\\atomik-kb\\kb-mcp-bridge.exe";

/** How often the dev page's fake index embeds a batch of pages, in milliseconds (so the progress shows). */
export const DEV_INDEX_STEP_MS = 700;

/** The AI settings `?ai=fake&aiOn=1` starts with: on, with the models the fake Ollama lists. */
export const DEV_AI_SETTINGS: JsonValue = {
  enabled: true,
  chatModel: "qwen3:8b",
  embeddingModel: "nomic-embed-text:latest",
  transcriptionModel: "r2t2:latest",
  semanticIndex: true,
  think: false,
};

/**
 * `?ai=fake` or `?ai=ollama[&aiUrl=...]` (ADR-0037): the shell's local AI and MCP commands, in a workspace.
 * `fake` answers deterministically with a made-up list of models (`&aiOn=1` also turns AI on with them, to look
 * at the writing panel and the chat at once); `ollama` calls a real Ollama from the browser, at `aiUrl`
 * (default `http://127.0.0.1:11434`; its `OLLAMA_ORIGINS` must allow this page, which the default does for
 * `http://localhost:*`). Both fill the semantic index a few pages at a time, so the progress shows in
 * Settings -> AI. Without it the transport has no AI and the UI hides every AI entry point.
 */
function applyAiMode(options: MemoryTransportOptions, query: URLSearchParams): void {
  const mode = query.get("ai");
  if (mode !== "fake" && mode !== "ollama") {
    return;
  }
  options.workspaceName ??= "Demo workspace";
  if (mode === "fake") {
    options.ai = { mode, indexStepMs: DEV_INDEX_STEP_MS, bridgePath: DEV_MCP_BRIDGE_PATH };
    if (query.get("aiOn") === "1") {
      options.settings = { ...options.settings, [AI_SETTINGS_SETTING]: DEV_AI_SETTINGS };
    }
  } else {
    options.ai = {
      mode,
      baseUrl: query.get("aiUrl") ?? DEFAULT_AI_ENDPOINT,
      indexStepMs: DEV_INDEX_STEP_MS,
      bridgePath: DEV_MCP_BRIDGE_PATH,
    };
  }
}

/**
 * `?dictation=fake` (ADR-0053): the shell's local dictation as a fake: nothing installed or chosen (Settings -> Dictation
 * and the first-use dialog of `/microphone` offer the download, ten steps of progress), then a session that "recognises"
 * a scripted phrase for every two seconds of audio pushed (and one more when it is stopped), whatever is said. The browser's
 * microphone is the real one (the page asks for permission). Without it the transport has no dictation and the UI hides
 * `/microphone` and the Settings section. Add `&dictationInstalled=light` (or `accurate`, or both, comma-separated) to
 * start with a model installed and the first of them chosen: `/microphone` then starts at once.
 */
function applyDictationMode(options: MemoryTransportOptions, query: URLSearchParams): void {
  if (query.get("dictation") !== "fake") {
    return;
  }
  options.workspaceName ??= "Demo workspace";
  const installed = (query.get("dictationInstalled") ?? "")
    .split(",")
    .filter((engine): engine is "accurate" | "light" => engine === "accurate" || engine === "light");
  const [first] = installed;
  options.dictation = first === undefined ? "fake" : { installed, selected: first };
}

/** The operations of the fake Drive that `&driveFail=` can fail. */
const DRIVE_OPERATIONS: readonly MemoryDriveOperation[] = ["connect", "list", "download", "deliver"];

/** The recovery kit's key of the backups `?driveBackups=demo` puts in the fake Drive (the words are in `dev.tsx`). */
export const DEV_DRIVE_KIT_KEY = Uint8Array.from({ length: 32 }, (_, index) => index + 1);

/** `&driveFail=connect:denied,deliver:quota`: the operations to fail and why; pairs that are not both known are left out. */
function driveFailures(text: string | null): NonNullable<MemoryDriveOptions["fail"]> {
  const fail: NonNullable<MemoryDriveOptions["fail"]> = {};
  for (const pair of (text ?? "").split(",")) {
    const [operation, reason] = pair.split(":");
    const known = DRIVE_OPERATIONS.find((candidate) => candidate === operation);
    const why = DRIVE_ERROR_REASONS.find((candidate): candidate is DriveErrorReason => candidate === reason);
    if (known !== undefined && why !== undefined) {
      fail[known] = why;
    }
  }
  return fail;
}

/** A name of a scheduled backup for `ms` (local time), as the app writes it. */
function scheduledName(ms: number): string {
  const when = new Date(ms);
  const two = (n: number): string => String(n).padStart(2, "0");
  return `atomik-kb-auto-${String(when.getFullYear())}-${two(when.getMonth() + 1)}-${two(when.getDate())}-${two(when.getHours())}${two(when.getMinutes())}.atomikkb`;
}

/**
 * Three scheduled backups of the demo workspace for the fake Drive, a day apart, that a restore accepts with the recovery kit
 * of {@link DEV_DRIVE_KIT_KEY}: a source installation exports once, when the first is downloaded.
 */
function demoDriveBackups(now: number): MemoryDriveBackup[] {
  let exported: Promise<Uint8Array> | undefined;
  const bytes = (): Promise<Uint8Array> => {
    exported ??= (async () => {
      const source = new MemoryTransport({
        workspaceName: "Demo workspace",
        spaces: DEMO_SPACES,
        newId: sequentialIds(),
        now: () => now,
        security: { kitCreatedAt: now, kitVerifiedAt: now, kitKey: DEV_DRIVE_KIT_KEY },
      });
      const { path } = await source.backupExport("/drive-demo/source.atomikkb");
      return source.files.get(path) ?? new Uint8Array();
    })();
    return exported;
  };
  return [0, 1, 2].map((daysAgo) => ({ name: scheduledName(now - daysAgo * 86_400_000), bytes, size: 48_000 + daysAgo * 1_024 }));
}

/**
 * `?drive=fake` (ADR-0059): Google Drive as a backup target and a restore source, on a fake of the shell's commands: the consent
 * (accepted), the sign-in (a moment), a fake account, a folder URL and downloads into the fake disk. It does not start a
 * workspace (alone it is onboarding, where "Restore from a backup" lists Drive); with `&seed=demo` or `&workspace=` Settings ->
 * Security -> Backup has the Drive target. Without it the transport has no Drive and the UI hides both. More:
 * `&driveConnected=1` starts connected; `&driveConsent=no` refuses the consent dialog; `&driveClient=none` is a build with no
 * Google client; `&driveFail=connect:denied,deliver:quota` fails the operations (`connect`, `list`, `download`, `deliver`) with
 * a reason of `DRIVE_ERROR_REASONS`; `&driveBackups=demo` puts three restorable backups in the folder (the recovery kit of
 * {@link DEV_DRIVE_KIT_KEY}), `&driveBackups=N` puts N that are not backups (restoring them fails).
 */
function applyDriveMode(options: MemoryTransportOptions, query: URLSearchParams, now: number): void {
  if (query.get("drive") !== "fake") {
    return;
  }
  const drive: MemoryDriveOptions = {};
  if (query.get("driveConnected") === "1") {
    drive.connected = true;
  }
  if (query.get("driveConsent") === "no") {
    drive.consent = false;
  }
  if (query.get("driveClient") === "none") {
    drive.noClient = true;
  }
  const fail = driveFailures(query.get("driveFail"));
  if (Object.keys(fail).length > 0) {
    drive.fail = fail;
  }
  const backups = query.get("driveBackups");
  if (backups === "demo") {
    drive.backups = demoDriveBackups(now);
  } else if (backups !== null && Number.isInteger(Number(backups)) && Number(backups) > 0) {
    drive.backups = Array.from({ length: Math.min(Number(backups), 50) }, (_, index) => ({
      name: scheduledName(now - index * 86_400_000),
    }));
  }
  options.drive = Object.keys(drive).length === 0 ? "fake" : drive;
}

/**
 * `?mcp=demo` (with `?ai=`): two MCP clients, one with access to the first space and a few calls in the audit
 * log, so that Settings -> AI -> MCP has something to show. The calls are made through the in-memory core.
 */
export async function seedMcpDemo(memory: MemoryTransport): Promise<void> {
  const client = new CoreClient(memory);
  const [first, second] = await client.spaceList();
  const desktop = await client.mcpClientCreate("Claude Desktop");
  await client.mcpClientCreate("Scripts");
  if (first !== undefined) {
    await client.mcpGrantsSet(desktop.client.id, [{ spaceId: first.id, read: true, write: false }]);
    const tree = await client.treeGet(first.id);
    const page = tree.roots[0]?.page;
    memory.recordMcpCall({ clientId: desktop.client.id, tool: "list_spaces", outcome: "ok", summary: "" });
    memory.recordMcpCall({ clientId: desktop.client.id, tool: "search", outcome: "ok", summary: "roadmap", spaceId: first.id });
    if (page !== undefined) {
      memory.recordMcpCall({ clientId: desktop.client.id, tool: "read_page", outcome: "ok", summary: page.title, spaceId: first.id, pageId: page.id });
    }
  }
  if (second !== undefined) {
    memory.recordMcpCall({ clientId: desktop.client.id, tool: "read_page", outcome: "denied", summary: "Handbook", spaceId: second.id });
  }
}

/**
 * The one address "Image from URL" (ADR-0028) finds a picture at on the dev page: the in-memory
 * core has no network, so `/Image from URL` with this link (the demo picture, `image/png`) shows
 * the whole flow; any other link ends in the error card ("the server could not be reached").
 */
export const DEMO_IMAGE_URL = "https://example.com/demo.png";

/** The canned answers of the dev page's "Image from URL": {@link DEMO_IMAGE_URL} and nothing else. */
export function devRemoteImages(): Map<string, RemoteImage> {
  return new Map([[DEMO_IMAGE_URL, { bytes: demoImagePng(), contentType: "image/png" }]]);
}

/** `?seed=demo`: the demo spaces, pages, image and file, in a workspace. */
function applyDemoSeed(options: MemoryTransportOptions): void {
  options.spaces = DEMO_SPACES;
  options.attachments = DEMO_ATTACHMENTS;
  options.workspaceName ??= "Demo workspace";
  // The same ids on every run, so that `withDemoLinks` can know them in advance.
  options.newId = sequentialIds();
}

/** The values of `?attachments=` that seed attachment sync (`none` is handled by `dev.tsx`). */
export type AttachmentsMode = "downloading" | "waiting" | "on-demand";

const ATTACHMENTS_MODES: readonly AttachmentsMode[] = ["downloading", "waiting", "on-demand"];

function attachmentsMode(value: string | null): AttachmentsMode | null {
  return ATTACHMENTS_MODES.find((mode) => mode === value) ?? null;
}

/** One step of the in-memory attachment transfers on the dev page (one chunk), in milliseconds. */
export const DEV_TRANSFER_STEP_MS = 1500;

/**
 * The demo's image and file as another device made them (`?attachments=`), under the ids of
 * `?seed=demo`'s, so that "Welcome" shows them: the picture followed by padding (browsers ignore
 * what follows a PNG's end), two chunks; a PDF-named file of three chunks. With `waiting` the
 * server holds none of their chunks yet.
 */
export function remoteDemoAttachments(waiting: boolean): MemoryRemoteAttachmentSeed[] {
  const png = demoImagePng();
  const image = new Uint8Array(CHUNK_SIZE + png.length);
  image.set(png);
  return [
    { attachmentId: DEMO_IMAGE_ID, name: "atomik.png", mime: "image/png", bytes: image, waiting },
    {
      attachmentId: DEMO_FILE_ID,
      name: "site-survey.pdf",
      mime: "application/pdf",
      bytes: new Uint8Array(2 * CHUNK_SIZE + 512 * 1024),
      waiting,
    },
    { attachmentId: DEMO_PDF_ID, name: "atomik-demo.pdf", mime: "application/pdf", bytes: demoPdf(3), waiting },
    { attachmentId: DEMO_AUDIO_ID, name: "atomik-tone.wav", mime: "audio/wav", bytes: demoWav(3), waiting },
  ];
}

/**
 * `?attachments=downloading|waiting|on-demand`: the demo (implied) whose image and file on
 * "Welcome" exist on the sync server but not on this device, in the state the value names: being
 * downloaded, waiting for the device that made them to finish uploading, or not downloaded under
 * the on-demand policy ("Download" fetches one). Transfers move one chunk every
 * `DEV_TRANSFER_STEP_MS`. Sync is connected to `DEV_SYNC_SERVER` unless `?sync=` says otherwise,
 * since the attachments came through it.
 */
function applyAttachmentsMode(options: MemoryTransportOptions, value: string | null): void {
  const mode = attachmentsMode(value);
  if (mode === null) {
    return;
  }
  if (options.spaces !== DEMO_SPACES) {
    applyDemoSeed(options);
  }
  // The demo's image and file are the remote ones.
  delete options.attachments;
  const attachmentSync: MemoryAttachmentsOptions = {
    remoteAttachments: remoteDemoAttachments(mode === "waiting"),
    transferStepMs: DEV_TRANSFER_STEP_MS,
  };
  if (mode === "on-demand") {
    attachmentSync.downloadPolicy = DOWNLOAD_POLICY_ON_DEMAND;
  }
  options.attachmentSync = attachmentSync;
  options.sync ??= { state: "idle", serverUrl: DEV_SYNC_SERVER };
}

/** The values of `?devices=`. */
export type DevicesMode = "pair-joiner" | "pair-sponsor" | "recover" | "revoked" | "rotation";

const DEVICES_MODES: readonly DevicesMode[] = ["pair-joiner", "pair-sponsor", "recover", "revoked", "rotation"];

function devicesMode(value: string | null): DevicesMode | null {
  return DEVICES_MODES.find((mode) => mode === value) ?? null;
}

/**
 * A recovery kit the in-memory core accepts for `?devices=recover` (the 24 words of an
 * all-zero key; any valid kit works, and one containing the word "wrong" stands for a kit the
 * server does not know).
 */
export const DEV_RECOVERY_KIT = `${"abandon ".repeat(23)}art`;

/** One step of the in-memory pairing and recovery on the dev page, in milliseconds. */
export const DEV_PAIRING_STEP_MS = 2500;

/**
 * `?devices=`: pairing and recovery start on a fresh installation (the onboarding screen,
 * whatever else asks for a workspace); the sponsor needs the account's kit and sync on (and
 * opens on Settings, see `initialRouteFromQuery`); a removed device and a pending rotation
 * (with the app lock, whose passphrase finishes it) need the kit. Three other devices make the
 * device list worth looking at.
 */
function applyDevicesMode(options: MemoryTransportOptions, value: string | null, now: number): void {
  const mode = devicesMode(value);
  if (mode === null) {
    return;
  }
  // Slow enough to read each step (the code, the digits, "Setting up...") before the next.
  options.pairingStepMs = DEV_PAIRING_STEP_MS;
  if (mode === "pair-joiner" || mode === "recover") {
    delete options.workspaceName;
    delete options.spaces;
    delete options.attachments;
    delete options.attachmentSync;
    delete options.security;
    delete options.sync;
    return;
  }
  options.workspaceName ??= "Demo workspace";
  options.security = { ...options.security, kitCreatedAt: options.security?.kitCreatedAt ?? now };
  if (mode === "pair-sponsor") {
    options.sync = { state: "idle", serverUrl: DEV_SYNC_SERVER };
    options.devices = { devices: 2 };
  } else if (mode === "revoked") {
    options.devices = { devices: 3, thisDeviceRevoked: true };
  } else {
    options.security.appLockPassphrase = DEV_PASSPHRASE;
    options.devices = { devices: 3, rotationPending: true };
  }
}

/** The values of `?sharing=` (Milestone 13): who this account is in the shared space, or the session the page starts with. */
export type SharingMode =
  | "owner"
  | "editor"
  | "commenter"
  | "viewer"
  | "removed"
  | "closed"
  | "disputed"
  | "kit-notice"
  | "link-preview"
  | "code-inviter"
  | "code-invitee";

const SHARING_MODES: readonly SharingMode[] = [
  "owner",
  "editor",
  "commenter",
  "viewer",
  "removed",
  "closed",
  "disputed",
  "kit-notice",
  "link-preview",
  "code-inviter",
  "code-invitee",
];

/** `?sharing=<mode>`, or `?shared=1` for `owner`; `null` for neither. */
export function sharingMode(query: URLSearchParams): SharingMode | null {
  const value = query.get("sharing");
  return SHARING_MODES.find((mode) => mode === value) ?? (query.get("shared") === "1" ? "owner" : null);
}

/**
 * `?comments=` (Milestone 13, slice 2; ADR-0048): a shared space of the dev page starts with comment threads on its pages
 * (Carol, Bob and this account on "Handbook" and "Weekly sync": one anchored to the words "How we work.", one resolved, one
 * page-level) and two rows in the inbox. `none` starts it without them; `live` adds people who act a few seconds after the
 * page opens (a reply that mentions this account, a new thread: toasts, the bell's count), see `dev.tsx`.
 */
export type CommentsMode = "none" | "live";

/** `?comments=none|live`; `null` for neither (the default: the seeded threads, nobody acts). */
export function commentsMode(query: URLSearchParams): CommentsMode | null {
  const value = query.get("comments");
  return value === "none" || value === "live" ? value : null;
}

/** The threads a shared space of the dev page starts with. */
export const DEV_COMMENT_THREADS: readonly MemoryThreadSeed[] = [
  {
    page: "Handbook",
    quote: "How we work.",
    comments: [
      { author: "Carol", body: "Can we add the on-call rota here?", minutesAgo: 180 },
      { author: "Bob", body: ["Good idea. ", { mention: "Maya" }, " can you draft it?"], minutesAgo: 120 },
    ],
  },
  {
    page: "Handbook",
    resolved: "Bob",
    comments: [
      { author: "Carol", body: "This page needs an owner.", minutesAgo: 1_500 },
      { author: "Bob", body: "Added Maya as the owner.", minutesAgo: 1_400 },
    ],
  },
  { page: "Weekly sync", comments: [{ author: "Bob", body: "Agenda for Monday: roadmap and hiring.", minutesAgo: 50 }] },
];

/** The rows of the inbox a shared space of the dev page starts with. */
export const DEV_NOTIFICATIONS: readonly MemoryNotificationSeed[] = [
  { kind: "CommentMention", actor: "Bob", page: "Handbook", threadQuote: "How we work.", minutesAgo: 120 },
  { kind: "PersonAssigned", actor: "Carol", page: "Meetings", read: true, minutesAgo: 1_300 },
];

/** One step of the in-memory invite sessions and of a share on the dev page, in milliseconds. */
export const DEV_SHARING_STEP_MS = 1500;

/** The name this account shows to co-members on the dev page. */
export const DEV_PROFILE_NAME = "Maya";

/** The space of the demo that `?sharing=` shares. */
export const DEV_SHARED_SPACE = "Work";

/**
 * `?shared=1` and `?sharing=` (Milestone 13): the demo's "Work" space starts shared, with Bob (Editor, verified), Carol (Commenter) and
 * Dan (Viewer) and this account as its Owner, or as the role the mode names; `removed` and `closed` start it as a read-only copy,
 * `disputed` with an incomplete snapshot and refused changes, `kit-notice` with a member who added a device with the recovery kit (and
 * the notice arrives a moment after the page opened, see `dev.tsx`); `link-preview`, `code-invitee` and `code-inviter` open with a
 * session under way. Sharing needs a workspace, the account (the kit) and a sync server, which the mode adds; it implies the demo
 * seed. Every step of a session takes `DEV_SHARING_STEP_MS`.
 */
function applySharingMode(options: MemoryTransportOptions, query: URLSearchParams, now: number): void {
  const mode = sharingMode(query);
  if (mode === null) {
    return;
  }
  if (options.spaces !== DEMO_SPACES) {
    applyDemoSeed(options);
  }
  options.workspaceName ??= "Demo workspace";
  options.security = { ...options.security, kitCreatedAt: options.security?.kitCreatedAt ?? now };
  options.sync ??= { state: "idle", serverUrl: DEV_SYNC_SERVER };
  const sharing: MemorySharingOptions = { profileName: DEV_PROFILE_NAME, stepMs: DEV_SHARING_STEP_MS };
  const withComments = commentsMode(query) !== "none";
  const work = (seed: Partial<MemorySharedSpaceSeed> = {}): MemorySharedSpaceSeed => ({
    space: DEV_SHARED_SPACE,
    ...(withComments ? { comments: DEV_COMMENT_THREADS, notifications: DEV_NOTIFICATIONS } : {}),
    ...seed,
  });
  switch (mode) {
    case "owner":
      sharing.spaces = [work()];
      break;
    case "editor":
    case "commenter":
    case "viewer":
      sharing.spaces = [work({ role: mode })];
      break;
    case "removed":
      sharing.spaces = [work({ role: "editor", archive: "Removed" })];
      break;
    case "closed":
      sharing.spaces = [work({ archive: "Closed" })];
      break;
    case "disputed":
      sharing.spaces = [
        work({
          members: [
            { name: "Bob", role: "editor", verified: true },
            { name: "Carol", role: "commenter", refusedChanges: 3 },
            { name: "Dan", role: "viewer" },
          ],
          disputes: [{ member: "Bob" }],
          invites: [{ role: "viewer" }],
        }),
      ];
      break;
    case "kit-notice":
      sharing.spaces = [
        work({
          members: [
            { name: "Bob", role: "editor", verified: true, viaKitAt: now - 2 * 86_400_000 },
            { name: "Carol", role: "commenter" },
            { name: "Dan", role: "viewer" },
          ],
        }),
      ];
      break;
    case "link-preview":
      sharing.start = "link-preview";
      break;
    case "code-invitee":
      sharing.start = "code-invitee";
      break;
    case "code-inviter":
      sharing.spaces = [work()];
      sharing.start = "code-inviter";
      break;
  }
  options.sharing = sharing;
}

/**
 * `?presence=demo` (Milestone 13, slice 3; ADR-0050): with a shared space (`?shared=1`, or `?sharing=` for a role), its other
 * members are on its pages: Bob edits "Handbook" with a caret that walks through its words and leaves for "Weekly sync" now and
 * then, Carol reads "Handbook" and goes idle, Dan visits "Meetings" and "Handbook": avatars in the page header and the space
 * switcher, a moving cursor with a name label, the 30-second timeout when a script ends. `null` for anything else.
 */
export type PresenceMode = "demo";

/** `?presence=demo`; `null` for anything else. */
export function presenceMode(query: URLSearchParams): PresenceMode | null {
  return query.get("presence") === "demo" ? "demo" : null;
}

/** One step of the scripted people of `?presence=demo`, in milliseconds. */
export const DEV_PRESENCE_STEP_MS = 1200;

/** The people of `?presence=demo`; they are members of the shared space of `DEV_SHARED_SPACE`. */
export const DEV_PRESENCE_PEERS: readonly MemoryPresencePeer[] = [
  {
    space: DEV_SHARED_SPACE,
    member: "Bob",
    route: [
      { page: "Handbook", does: "edit", steps: 12 },
      { page: "Weekly sync", steps: 4 },
    ],
  },
  { space: DEV_SHARED_SPACE, member: "Carol", route: [{ page: "Handbook", steps: 20 }, { steps: 6 }] },
  { space: DEV_SHARED_SPACE, member: "Dan", route: [{ page: "Meetings", steps: 8 }, { page: "Handbook", steps: 8 }] },
];

/** `?presence=demo`: the scripted people, when the shared space is live (a read-only copy and the sessions have none to be in). */
function applyPresenceMode(options: MemoryTransportOptions, query: URLSearchParams): void {
  const seed = options.sharing?.spaces?.find((space) => space.space === DEV_SHARED_SPACE);
  if (presenceMode(query) === "demo" && seed !== undefined && seed.archive === undefined) {
    options.presence = { stepMs: DEV_PRESENCE_STEP_MS, peers: DEV_PRESENCE_PEERS };
  }
}

/** The screen the app opens on: Settings for `?devices=pair-sponsor` (where "Add a device..." is). */
export function initialRouteFromQuery(query: URLSearchParams): "settings" | undefined {
  return devicesMode(query.get("devices")) === "pair-sponsor" ? "settings" : undefined;
}

/**
 * `?attachments=none`: the in-memory core behind a transport without the attachment part
 * (bytes, file dialogs, "Open"), like a shell that has none, to look at what the UI does
 * then: the Image and File commands say why they are unavailable, images show no preview.
 */
export function withoutAttachments(memory: MemoryTransport): Transport {
  return {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
    backupExport: (path) => memory.backupExport(path),
    backupImport: (path, kit) => memory.backupImport(path, kit),
    ...(memory.ai === undefined ? {} : { ai: memory.ai }),
    ...(memory.mcp === undefined ? {} : { mcp: memory.mcp }),
    ...(memory.dictation === undefined ? {} : { dictation: memory.dictation }),
    backupFolderList: (path) => memory.backupFolderList(path),
    ...(memory.drive === undefined ? {} : { drive: memory.drive }),
    presence: memory.presence,
  };
}

/**
 * The demo with its page references: "Welcome" links "Roadmap" and "Handbook" (another
 * space), "Roadmap" links "Design notes", so backlinks show. Page ids exist only once the
 * core made the pages, so a first in-memory core made from the same options tells them
 * (the ids are sequential, the same for the same seed), and the content of the second
 * one uses them. Options without the demo seed come back as they are.
 */
export async function withDemoLinks(options: MemoryTransportOptions): Promise<MemoryTransportOptions> {
  if (options.spaces !== DEMO_SPACES) {
    return options;
  }
  // Ready and unlocked whatever the real one starts as, so it can be read (a seeded kit and
  // devices take their ids after the pages, so they do not change theirs). Without the remote
  // attachments (fixed ids, after the pages too): no megabytes and no transfer timer for it.
  const probe = new MemoryTransport({
    ...options,
    coreState: undefined,
    security: undefined,
    devices: undefined,
    attachmentSync: undefined,
    ai: undefined,
    dictation: undefined,
    drive: undefined,
    sharing: undefined,
    presence: undefined,
    newId: sequentialIds(),
  });
  const client = new CoreClient(probe);
  const ids = new Map<string, string>();
  for (const space of await client.spaceList()) {
    const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): void => {
      for (const node of nodes) {
        ids.set(node.page.title, node.page.id);
        visit(node.children);
      }
    };
    visit((await client.treeGet(space.id)).roots);
  }
  return { ...options, spaces: demoSpaces((title) => ids.get(title)), newId: sequentialIds() };
}
