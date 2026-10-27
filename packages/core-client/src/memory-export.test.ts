// The export of `MemoryTransport` (ADR-0036, 2 and 7): sessions (`ExportBegin`, `ExportNext`,
// `ExportEnd`), layout, entries, warnings, events, the CSV of a view and the shell's `exportRun`
// on the fake disk.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { demoImagePng } from "./demo-image";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { ExportBatch } from "./generated/ExportBatch";
import type { ExportEntry } from "./generated/ExportEntry";
import type { ExportFormat } from "./generated/ExportFormat";
import type { ExportPlan } from "./generated/ExportPlan";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { readMemoryArchive } from "./memory-export";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const PHOTO = "aaaaaaaa-0000-4000-8000-000000000001";
const REPORT = "aaaaaaaa-0000-4000-8000-000000000002";
const MISSING = "aaaaaaaa-0000-4000-8000-0000000000ff";

/** The ids `setup` hands out, in order: the device (1), the workspace (2), "Home" (3), then its pages depth first. */
function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

const HOME = id(3);
const PROJECTS = id(4);
const ALPHA = id(5);
const BETA = id(6);
const TODO = id(7);
const ELSEWHERE = id(9);

const text = (value: string): JsonValue => ({ type: "text", text: value });
const doc = (...content: JsonValue[]): JsonValue => ({ type: "doc", content });
const paragraph = (n: number, ...content: JsonValue[]): JsonValue => ({ type: "paragraph", attrs: { id: blockId(n) }, content });

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    pages: [
      {
        title: "Projects",
        icon: "📁",
        content: doc(
          paragraph(1, text("See "), { type: "pageRef", attrs: { id: blockId(2), pageId: ALPHA } }, text(" now.")),
          { type: "image", attrs: { id: blockId(3), attachmentId: PHOTO, alt: "A photo" } },
          { type: "file", attrs: { id: blockId(4), attachmentId: REPORT } },
        ),
        children: [
          { title: "Alpha", content: doc(paragraph(5, text("Alpha text"))) },
          {
            title: "Beta",
            content: doc(
              paragraph(6, { type: "pageRef", attrs: { id: blockId(7), pageId: PROJECTS } }),
              { type: "image", attrs: { id: blockId(8), attachmentId: PHOTO } },
              { type: "file", attrs: { id: blockId(9), attachmentId: MISSING } },
            ),
          },
        ],
      },
      { title: "Todo" },
    ],
  },
  { name: "Elsewhere", pages: [{ title: "Far" }] },
];

function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0,
    workspaceName: "Workspace",
    spaces: SPACES,
    attachments: [{ attachmentId: PHOTO, name: "photo.png", mime: "image/png", bytes: demoImagePng() }],
    attachmentSync: {
      downloadPolicy: "on_demand",
      remoteAttachments: [{ attachmentId: REPORT, name: "report.pdf", mime: "application/pdf", bytes: new Uint8Array([1, 2, 3]) }],
    },
    security: { kitCreatedAt: T0 },
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}

type Fixture = Awaited<ReturnType<typeof setup>>;

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

async function begin(
  { transport }: Fixture,
  pageId?: string,
  includeChildren = true,
  format: ExportFormat = "markdown",
  spaceId = HOME,
): Promise<ExportPlan> {
  const response = await transport.dispatch({ type: "ExportBegin", data: { spaceId, ...(pageId === undefined ? {} : { pageId }), includeChildren, format } });
  if (response.type !== "ExportPlanned") {
    throw new Error("unexpected response");
  }
  return response.data;
}

async function next({ transport }: Fixture, exportId: number, maxEntries = 500): Promise<ExportBatch> {
  const response = await transport.dispatch({ type: "ExportNext", data: { exportId, maxEntries } });
  if (response.type !== "ExportBatch") {
    throw new Error("unexpected response");
  }
  return response.data;
}

/** Every entry of an export in one go. */
async function everything(fixture: Fixture, exportId: number): Promise<{ entries: ExportEntry[]; warnings: NonNullable<ExportBatch["warnings"]> }> {
  const entries: ExportEntry[] = [];
  const warnings: NonNullable<ExportBatch["warnings"]> = [];
  for (;;) {
    const batch = await next(fixture, exportId, 500);
    entries.push(...batch.entries);
    warnings.push(...(batch.warnings ?? []));
    if (batch.done) {
      return { entries, warnings };
    }
  }
}

const textOf = (entry: ExportEntry | undefined): string => {
  if (entry?.content.type !== "Text") {
    throw new Error("not a text entry");
  }
  return entry.content.data.text;
};

describe("ExportBegin", () => {
  it("plans the whole space: pages in tree order, the available attachments, a zip named after the space", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    expect(plan).toEqual({ exportId: 1, pages: 4, attachments: 1, singleFile: false, suggestedName: "Home.zip" });
  });

  it("plans one page with its descendants, or alone", async () => {
    const fixture = setup();
    expect((await begin(fixture, PROJECTS, true)).pages).toBe(3);
    expect((await begin(fixture, PROJECTS, false)).pages).toBe(1);
    expect((await begin(fixture, BETA, true)).pages).toBe(1);
  });

  it("makes one page without attachments or whiteboards a single file", async () => {
    const fixture = setup();
    expect(await begin(fixture, TODO)).toEqual({ exportId: 1, pages: 1, attachments: 0, singleFile: true, suggestedName: "Todo.md" });
    expect(await begin(fixture, TODO, true, "html")).toMatchObject({ singleFile: true, suggestedName: "Todo.html" });
    // Alpha has no attachment; Beta's are an available and an unknown one: the available one counts.
    expect(await begin(fixture, ALPHA)).toMatchObject({ singleFile: true });
    expect(await begin(fixture, BETA, false)).toMatchObject({ singleFile: false, attachments: 1, suggestedName: "Beta.zip" });
  });

  it("counts up from 1, never reusing an id, and a new session drops the old one", async () => {
    const fixture = setup();
    expect((await begin(fixture)).exportId).toBe(1);
    expect((await begin(fixture)).exportId).toBe(2);
    expect((await failure(next(fixture, 1))).code).toBe("NotFound");
    await fixture.transport.dispatch({ type: "ExportEnd", data: { exportId: 2 } });
    expect((await begin(fixture)).exportId).toBe(3);
  });

  it("refuses unknown spaces and pages, pages of another space and pages in the trash", async () => {
    const fixture = setup();
    expect((await failure(begin(fixture, undefined, true, "markdown", id(99)))).code).toBe("NotFound");
    expect((await failure(begin(fixture, id(99)))).code).toBe("NotFound");
    const elsewhere = await failure(begin(fixture, ELSEWHERE));
    expect(elsewhere.code).toBe("InvalidInput");
    expect(elsewhere.details).toEqual({ field: "pageId" });
    await fixture.client.pageTrash(TODO);
    const trashed = await failure(begin(fixture, TODO));
    expect(trashed.code).toBe("InvalidInput");
    expect(trashed.details).toEqual({ field: "pageId" });
  });

  it("leaves trashed descendants out", async () => {
    const fixture = setup();
    await fixture.client.pageTrash(BETA);
    expect((await begin(fixture)).pages).toBe(3);
    expect((await begin(fixture, PROJECTS, true)).pages).toBe(2);
  });

  it("rejects a format it does not know", async () => {
    const fixture = setup();
    const error = await failure(fixture.transport.dispatch({ type: "ExportBegin", data: { spaceId: HOME, includeChildren: true, format: "pdf" as ExportFormat } }));
    expect(error.details).toEqual({ field: "format" });
  });
});

describe("ExportNext", () => {
  it("delivers each page's file, then the attachments laid out with it, then the sidecar", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    const { entries } = await everything(fixture, plan.exportId);
    expect(entries.map((entry) => entry.path)).toEqual([
      "Projects.md",
      "Projects/photo.png",
      "Projects/Alpha.md",
      "Projects/Beta.md",
      "Todo.md",
      "atomik-export.json",
    ]);
    expect(entries[1]?.content).toEqual({ type: "Attachment", data: { attachmentId: PHOTO, size: demoImagePng().length } });
    const sidecar = JSON.parse(textOf(entries[5])) as { format: number; name: string; pages: string[]; properties: { name: string }[] };
    expect(sidecar.format).toBe(1);
    expect(sidecar.name).toBe("Home");
    expect(sidecar.pages).toEqual(["Projects.md", "Projects/Alpha.md", "Projects/Beta.md", "Todo.md"]);
    expect(sidecar.properties.map((property) => property.name)).toEqual(["Tags"]);
  });

  it("writes the front matter and the Markdown body with relative links between files", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    const { entries } = await everything(fixture, plan.exportId);
    const projects = textOf(entries[0]);
    expect(projects).toBe(
      [
        "---",
        'title: "Projects"',
        'icon: "📁"',
        'created: "2023-11-14T22:13:20Z"',
        'updated: "2023-11-14T22:13:20Z"',
        "---",
        "",
        "See [Alpha](Projects/Alpha.md) now.",
        "",
        "![A photo](Projects/photo.png)",
        "",
        `[report.pdf](attachment://${REPORT})`,
        "",
      ].join("\n"),
    );
    const beta = textOf(entries[3]);
    // Beta lives in the folder Projects/: its links go up and down from there.
    expect(beta).toContain("[Projects](../Projects.md)");
    expect(beta).toContain("![](photo.png)");
    expect(beta).toContain(`[${MISSING}](attachment://${MISSING})`);
  });

  it("exports a page that has no stored document as an empty page, and creates no document", async () => {
    const fixture = setup();
    const before = (await fixture.transport.backupExport("/backups/before.atomikkb")).summary.documents;
    const plan = await begin(fixture);
    const { entries } = await everything(fixture, plan.exportId);
    expect(textOf(entries[4])).toBe('---\ntitle: "Todo"\ncreated: "2023-11-14T22:13:20Z"\nupdated: "2023-11-14T22:13:20Z"\n---\n\n\n');
    expect((await fixture.transport.backupExport("/backups/after.atomikkb")).summary.documents).toBe(before);
    expect(fixture.events.some((event) => event.type === "PageContentChanged")).toBe(false);
  });

  it("warns once per attachment that is not on this device or is unknown, and never writes its file", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    const { entries, warnings } = await everything(fixture, plan.exportId);
    expect(entries.some((entry) => entry.path.endsWith("report.pdf"))).toBe(false);
    expect(warnings).toEqual([
      { code: "attachmentNotDownloaded", pageId: PROJECTS, attachmentId: REPORT },
      { code: "attachmentMissing", pageId: BETA, attachmentId: MISSING },
    ]);
  });

  it("splits a page's group over batches and never exceeds maxEntries", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    const batches: ExportBatch[] = [];
    for (;;) {
      const batch = await next(fixture, plan.exportId, 1);
      batches.push(batch);
      if (batch.done) {
        break;
      }
    }
    expect(batches.map((batch) => batch.entries.map((entry) => entry.path))).toEqual([
      ["Projects.md"],
      ["Projects/photo.png"],
      ["Projects/Alpha.md"],
      ["Projects/Beta.md"],
      ["Todo.md"],
      ["atomik-export.json"],
    ]);
    expect(batches.map((batch) => batch.done)).toEqual([false, false, false, false, false, true]);
    expect(batches.every((batch) => batch.entries.length <= 1)).toBe(true);
    // The warnings of a page come with the call that rendered it.
    expect(batches.flatMap((batch) => batch.warnings ?? [])).toHaveLength(2);
  });

  it("emits ExportProgress with the pages rendered so far after every call", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    fixture.events.length = 0;
    await next(fixture, plan.exportId, 2);
    await next(fixture, plan.exportId, 2);
    await next(fixture, plan.exportId, 2);
    expect(fixture.events).toEqual([
      { type: "ExportProgress", data: { exportId: 1, done: 1, total: 4 } },
      { type: "ExportProgress", data: { exportId: 1, done: 3, total: 4 } },
      { type: "ExportProgress", data: { exportId: 1, done: 4, total: 4 } },
    ]);
  });

  it("answers an empty finished batch when called again after the end", async () => {
    const fixture = setup();
    const plan = await begin(fixture, TODO);
    expect((await next(fixture, plan.exportId)).done).toBe(true);
    expect(await next(fixture, plan.exportId)).toEqual({ entries: [], done: true });
  });

  it("checks maxEntries (1 to 500) and the session", async () => {
    const fixture = setup();
    const plan = await begin(fixture);
    for (const bad of [0, 501, -1, 1.5]) {
      const error = await failure(next(fixture, plan.exportId, bad));
      expect(error.code).toBe("InvalidInput");
      expect(error.details).toMatchObject({ field: "maxEntries", limit: 500 });
    }
    expect((await failure(next(fixture, 99))).code).toBe("NotFound");
    await fixture.transport.dispatch({ type: "ExportEnd", data: { exportId: plan.exportId } });
    expect((await failure(next(fixture, plan.exportId))).code).toBe("NotFound");
    // Ending an export that is gone is not an error.
    expect(await fixture.transport.dispatch({ type: "ExportEnd", data: { exportId: plan.exportId } })).toEqual({ type: "Ok" });
  });

  it("drops the session when the app locks", async () => {
    const fixture = setup({ security: { kitCreatedAt: T0, appLockPassphrase: "correct horse battery" } });
    const plan = await begin(fixture);
    await fixture.transport.lock();
    await fixture.transport.unlock("correct horse battery");
    expect((await failure(next(fixture, plan.exportId))).code).toBe("NotFound");
  });
});

describe("export formats", () => {
  it("writes HTML pages with links to each other, no sidecar", async () => {
    const fixture = setup();
    const plan = await begin(fixture, undefined, true, "html");
    const { entries } = await everything(fixture, plan.exportId);
    expect(entries.map((entry) => entry.path)).toEqual([
      "Projects.html",
      "Projects/photo.png",
      "Projects/Alpha.html",
      "Projects/Beta.html",
      "Todo.html",
    ]);
    const projects = textOf(entries[0]);
    expect(projects.startsWith("<!doctype html>")).toBe(true);
    expect(projects).toContain('<h1 class="page-title">Projects</h1>');
    expect(projects).toContain('<div class="page-header-icon">📁</div>');
    expect(projects).toContain('<a class="page-ref" href="Projects/Alpha.html">Alpha</a>');
    expect(projects).toContain('<img src="Projects/photo.png" alt="A photo">');
    expect(textOf(entries[3])).toContain('<a class="page-ref" href="../Projects.html">Projects</a>');
    expect(textOf(entries[3])).toContain('<img src="photo.png" alt="">');
  });

  it("exports a page alone with links to pages outside the export as plain references", async () => {
    const fixture = setup();
    const plan = await begin(fixture, PROJECTS, false);
    const { entries } = await everything(fixture, plan.exportId);
    expect(entries.map((entry) => entry.path)).toEqual(["Projects.md", "Projects/photo.png", "atomik-export.json"]);
    expect(textOf(entries[0])).toContain("See [[Alpha]] now.");
  });

  it("lays out whiteboards as .excalidraw files and counts them against the single-file rule", async () => {
    const WB = "cccccccc-0000-4000-8000-000000000001";
    const content: JsonValue = {
      type: "doc",
      content: [paragraph(1, text("Board:")), { type: "whiteboard", attrs: { id: WB, height: 360 } }],
      whiteboards: {
        [WB]: {
          e1: { id: "e1", type: "text", index: "a0", text: "Hello", originalText: "Hello" },
          e2: { id: "e2", type: "rectangle", index: "a1", isDeleted: true },
        },
      },
    };
    const fixture = setup({ spaces: [{ name: "Home", pages: [{ title: "Board", content }] }] });
    const plan = await begin(fixture);
    expect(plan).toMatchObject({ pages: 1, singleFile: false, suggestedName: "Home.zip" });
    const { entries } = await everything(fixture, plan.exportId);
    expect(entries.map((entry) => entry.path)).toEqual(["Board.md", "Board/Whiteboard.excalidraw", "atomik-export.json"]);
    const drawing = JSON.parse(textOf(entries[1])) as { elements: { id: string }[] };
    expect(drawing.elements.map((element) => element.id)).toEqual(["e1"]);
  });
});

describe("property values in the front matter", () => {
  it("writes the tags and the properties by name, in schema order", async () => {
    const fixture = setup({
      spaces: [
        {
          name: "Home",
          tagOptions: [{ name: "work" }, { name: "review" }],
          properties: [
            { name: "Status", kind: "status" },
            { name: "Estimate", kind: "number" },
            { name: "Done", kind: "checkbox" },
            { name: "Due", kind: "date" },
          ],
          pages: [
            {
              title: "Weekly review",
              tags: ["work", "review"],
              properties: { Status: "Done", Estimate: 3.5, Done: true, Due: { date: "2026-10-07", time: "14:30" } },
            },
          ],
        },
      ],
    });
    const plan = await begin(fixture, undefined, true, "markdown");
    const { entries } = await everything(fixture, plan.exportId);
    expect(entries[0]?.path).toBe("Weekly review.md");
    expect(textOf(entries[0])).toBe(`---
title: "Weekly review"
created: "2023-11-14T22:13:20Z"
updated: "2023-11-14T22:13:20Z"
tags:
  - "work"
  - "review"
"Status": "Done"
"Estimate": 3.5
"Done": true
"Due": "2026-10-07 14:30"
---


`);
  });
});

describe("ExportCsv", () => {
  async function csvFixture() {
    const fixture = setup({
      spaces: [
        {
          name: "Home",
          properties: [
            { name: "Estimate", kind: "number" },
            { name: "Done", kind: "checkbox" },
            { name: "Due", kind: "date" },
            { name: "Status", kind: "status" },
            { name: "Labels", kind: "multi_select", options: [{ name: "a" }, { name: "b" }] },
            { name: "Notes", kind: "text" },
            { name: "Place", kind: "place" },
            { name: "Checks", kind: "checklist" },
            { name: "Edited", kind: "last_edited_time" },
            { name: "By", kind: "created_by" },
          ],
          pages: [
            {
              title: "=SUM(1,2)",
              properties: {
                Estimate: -5,
                Done: true,
                Due: { date: "2026-10-07" },
                Status: "In progress",
                Labels: ["a", "b"],
                Notes: "+1 and, \"quoted\"\nsecond line",
                Place: { name: "Paris", lat: 48.8566, lon: 2.3522 },
                Checks: [{ text: "x", done: true }, { text: "y" }],
              },
            },
            { title: "Plain", properties: { Estimate: 0.5, Done: false } },
          ],
        },
        { name: "Elsewhere", pages: [{ title: "Far" }] },
      ],
      attachments: [],
      attachmentSync: undefined,
    });
    const schema = await fixture.client.propertySchemaGet(HOME);
    const property = (name: string): string => schema.find((entry) => entry.name === name)?.id ?? "";
    const pages = (await fixture.client.treeGet(HOME)).roots.map((node) => node.page.id);
    return { ...fixture, property, pages };
  }

  it("writes the header, a byte order mark, CRLF, quoting and the value text forms", async () => {
    const { client, property, pages } = await csvFixture();
    const csv = await client.exportCsv(HOME, pages, [
      { type: "Title" },
      ...["Estimate", "Done", "Due", "Status", "Labels", "Notes", "Place", "Checks", "Edited", "By"].map((name) => ({
        type: "Property" as const,
        data: { propertyId: property(name) },
      })),
    ]);
    const local = (ms: number): string => {
      const when = new Date(ms);
      const two = (n: number): string => String(n).padStart(2, "0");
      return `${String(when.getFullYear())}-${two(when.getMonth() + 1)}-${two(when.getDate())} ${two(when.getHours())}:${two(when.getMinutes())}`;
    };
    expect(csv).toBe(
      [
        "\u{FEFF}Name,Estimate,Done,Due,Status,Labels,Notes,Place,Checks,Edited,By",
        [
          '"\'=SUM(1,2)"',
          "-5",
          "Yes",
          "2026-10-07",
          "In progress",
          '"a, b"',
          '"\'+1 and, ""quoted""\nsecond line"',
          '"Paris (48.8566, 2.3522)"',
          "[x] x; [ ] y",
          local(T0),
          "memory device",
        ].join(","),
        `Plain,0.5,No,,,,,,,${local(T0)},memory device`,
        "",
      ].join("\r\n"),
    );
  });

  it("keeps the order of the rows and columns it is given, and writes only what is asked", async () => {
    const { client, property, pages } = await csvFixture();
    const reversed = [...pages].reverse();
    const csv = await client.exportCsv(HOME, reversed, [{ type: "Property", data: { propertyId: property("Estimate") } }, { type: "Title" }]);
    expect(csv).toBe("\u{FEFF}Estimate,Name\r\n0.5,Plain\r\n-5,\"'=SUM(1,2)\"\r\n");
    expect(await client.exportCsv(HOME, [], [{ type: "Title" }])).toBe("\u{FEFF}Name\r\n");
  });

  it("never guards a number cell", async () => {
    const { client, property, pages } = await csvFixture();
    const csv = await client.exportCsv(HOME, pages.slice(0, 1), [{ type: "Property", data: { propertyId: property("Estimate") } }]);
    expect(csv).toBe("\u{FEFF}Estimate\r\n-5\r\n");
  });

  it("refuses unknown spaces, pages and properties, pages of another space and too many columns", async () => {
    const { client, pages } = await csvFixture();
    expect((await failure(client.exportCsv(id(99), [], []))).code).toBe("NotFound");
    expect((await failure(client.exportCsv(HOME, [id(99)], [{ type: "Title" }]))).code).toBe("NotFound");
    expect((await failure(client.exportCsv(HOME, pages, [{ type: "Property", data: { propertyId: id(99) } }]))).code).toBe("NotFound");
    const far = (await client.treeGet((await client.spaceList()).find((space) => space.name === "Elsewhere")?.id ?? "")).roots[0]?.page.id ?? "";
    const other = await failure(client.exportCsv(HOME, [far], [{ type: "Title" }]));
    expect(other.details).toEqual({ field: "pageIds" });
    const columns = Array.from({ length: 102 }, () => ({ type: "Title" as const }));
    const tooMany = await failure(client.exportCsv(HOME, pages, columns));
    expect(tooMany.details).toEqual({ field: "columns", limit: 101 });
    expect((await client.exportCsv(HOME, pages, columns.slice(0, 101))).startsWith("\u{FEFF}Name,")).toBe(true);
  });
});

describe("exportRun", () => {
  it("writes the archive to the fake disk as a JSON listing of its entries, attachments included", async () => {
    const fixture = setup();
    const result = await fixture.client.exportRun({ spaceId: HOME, includeChildren: true, format: "markdown" });
    expect(result.path).toBe("/downloads/Home.zip");
    expect(result.pages).toBe(4);
    expect(result.attachments).toBe(1);
    expect(result.warnings).toEqual([
      { code: "attachmentNotDownloaded", pageId: PROJECTS, attachmentId: REPORT },
      { code: "attachmentMissing", pageId: BETA, attachmentId: MISSING },
    ]);
    const files = readMemoryArchive(fixture.transport.files.get("/downloads/Home.zip") ?? new Uint8Array());
    expect(Object.keys(files ?? {})).toEqual([
      "Projects.md",
      "Projects/photo.png",
      "Projects/Alpha.md",
      "Projects/Beta.md",
      "Todo.md",
      "atomik-export.json",
    ]);
    expect(files?.["Projects/photo.png"]).toEqual(demoImagePng());
  });

  it("writes a single page as a plain file", async () => {
    const fixture = setup();
    const result = await fixture.client.exportRun({ spaceId: HOME, pageId: ALPHA, includeChildren: false, format: "markdown" });
    expect(result).toEqual({ path: "/downloads/Alpha.md", pages: 1, attachments: 0, warnings: [] });
    const written = new TextDecoder().decode(fixture.transport.files.get("/downloads/Alpha.md"));
    expect(written.startsWith('---\ntitle: "Alpha"\n')).toBe(true);
    expect(written).toContain("Alpha text");
    const html = await fixture.client.exportRun({ spaceId: HOME, pageId: ALPHA, includeChildren: false, format: "html" });
    expect(html.path).toBe("/downloads/Alpha.html");
    expect(new TextDecoder().decode(fixture.transport.files.get("/downloads/Alpha.html")).startsWith("<!doctype html>")).toBe(true);
  });

  it("emits progress events and ends its session", async () => {
    const fixture = setup();
    await fixture.client.exportRun({ spaceId: HOME, includeChildren: true, format: "markdown" });
    expect(fixture.events.filter((event) => event.type === "ExportProgress").length).toBeGreaterThan(0);
    expect((await failure(next(fixture, 1))).code).toBe("NotFound");
  });

  it("is cancelled like the other dialogs: InvalidInput for path, the session ends", async () => {
    const fixture = setup({ cancelDialogs: true });
    const error = await failure(fixture.client.exportRun({ spaceId: HOME, includeChildren: true, format: "markdown" }));
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "path" });
    expect(fixture.transport.files.size).toBe(0);
    expect((await failure(next(fixture, 1))).code).toBe("NotFound");
    // The next session has the next id.
    fixture.transport.setCancelDialogs(false);
    expect((await begin(fixture)).exportId).toBe(2);
  });

  it("leaves an attachment it cannot read out, with a warning", async () => {
    const fixture = setup();
    fixture.transport.blobs.clear();
    const result = await fixture.client.exportRun({ spaceId: HOME, includeChildren: true, format: "markdown" });
    expect(result.attachments).toBe(0);
    expect(result.warnings).toContainEqual({ code: "attachmentUnreadable", attachmentId: PHOTO });
    const files = readMemoryArchive(fixture.transport.files.get("/downloads/Home.zip") ?? new Uint8Array());
    expect(Object.keys(files ?? {})).not.toContain("Projects/photo.png");
  });

  it("refuses a space that does not exist and needs the core to be ready", async () => {
    const fixture = setup();
    expect((await failure(fixture.client.exportRun({ spaceId: id(99), includeChildren: true, format: "markdown" }))).code).toBe("NotFound");
    fixture.transport.setCoreState("locked");
    expect((await failure(fixture.client.exportRun({ spaceId: HOME, includeChildren: true, format: "markdown" }))).code).toBe("Locked");
  });
});
