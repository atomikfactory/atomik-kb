// The in-memory import of an AppFlowy backup (ADR-0039, `docs/architecture/appflowy-import.md`, 8):
// `ImportBeginSpaces` sessions (spaces made at the plan, pages per space, files per space, remote files,
// the report), the reduced reader (tree, spaces, documents, databases, files, warnings), the shell's reading
// of an archive (`.json` as text, `collab_objects/` skipped) and the same archive imported into a page.
// The archives are synthetic, written for these tests (clean room, ADR-0010).

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { afId, appFlowyDocument, demoImportSources, plain, styled, text } from "./demo-import";
import type { BlockSpec } from "./demo-import";
import { demoImagePng } from "./demo-image";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { ImportPlanInfo } from "./generated/ImportPlanInfo";
import type { ImportReport } from "./generated/ImportReport";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { MemoryImportSource } from "./memory-import";
import { uuidV5 } from "./uuid-v5";

const WORKSPACE = afId("a", 0);

interface ViewSpec {
  id: string;
  name?: string;
  parent?: string;
  children?: string[];
  layout?: number;
  extra?: Record<string, unknown>;
  icon?: { ty: number; value: string };
}

/** The files of an AppFlowy backup: the views, the documents (by id), the database JSON (by view id) and any other file. */
function archive(
  views: readonly ViewSpec[],
  documents: Record<string, readonly BlockSpec[]> = {},
  databases: Record<string, unknown> = {},
  more: Record<string, string | Uint8Array> = {},
): Record<string, string | Uint8Array> {
  const map: Record<string, unknown> = {};
  for (const view of views) {
    map[view.id] = {
      view_id: view.id,
      name: view.name ?? "Untitled",
      layout: view.layout ?? 0,
      parent_id: view.parent ?? WORKSPACE,
      children: view.children ?? [],
      collab_object_id: view.id,
      created_at: 1_760_000_000,
      updated_at: 1_760_000_100,
      extra: JSON.stringify(view.extra ?? {}),
      icon: view.icon ?? null,
    };
  }
  const files: Record<string, string | Uint8Array> = {
    "metadata.json": JSON.stringify({ export_info: { workspace_id: WORKSPACE, version: "1.0" } }),
    "relation_map.json": JSON.stringify({ workspace_id: WORKSPACE, views: map, workspace_database_meta: [] }),
  };
  for (const [id, blocks] of Object.entries(documents)) {
    files[`collab_jsons/documents/${id}.json`] = appFlowyDocument(id, blocks);
  }
  for (const [id, database] of Object.entries(databases)) {
    files[`collab_jsons/databases/${id}.json`] = JSON.stringify(database);
  }
  return { ...files, ...more };
}

const SPACE = { is_space: true };
const S1 = afId("a", 1);
const S2 = afId("a", 2);

/** A database JSON: its fields (the first is the primary one), its view and its rows (`cells` by field id, the title first). */
function database(
  id: string,
  viewId: string,
  fields: { id: string; name: string; type: number; options?: { id: string; name: string; color?: string }[] }[],
  rows: { id: string; cells: Record<string, unknown>; icon?: string; document?: boolean }[],
  view: Record<string, unknown> = {},
): unknown {
  return {
    database_id: id,
    views: [
      {
        id: viewId,
        layout: 1,
        filters: [],
        sorts: [],
        group_settings: [],
        row_orders: rows.map((row) => ({ id: row.id, height: 60 })),
        field_orders: fields.map((field) => ({ id: field.id })),
        field_settings: {},
        ...view,
      },
    ],
    fields: fields.map((field, at) => ({
      id: field.id,
      name: field.name,
      field_type: field.type,
      is_primary: at === 0,
      type_options:
        field.options === undefined
          ? {}
          : { [String(field.type)]: { content: JSON.stringify({ options: field.options, disable_color: false }) } },
    })),
    rows: rows.map((row) => ({
      id: row.id,
      cells: Object.fromEntries(Object.entries(row.cells).map(([field, data]) => [field, { field_type: 0, data }])),
    })),
    row_metas: Object.fromEntries(
      rows.map((row) => [row.id, { icon_url: row.icon ?? "", cover: null, is_document_empty: row.document !== true }]),
    ),
  };
}

function importing(files: Record<string, string | Uint8Array>, options: MemoryTransportOptions = {}) {
  const transport = new MemoryTransport({
    workspaceName: "Workspace",
    spaces: [{ name: "Home", pages: [{ title: "Existing" }] }],
    importSources: { "Backup.zip": { kind: "zip", files } },
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}
type Context = ReturnType<typeof importing>;

/** Runs the shell's import of the backup into new spaces. */
async function intoSpaces(files: Record<string, string | Uint8Array>, options: MemoryTransportOptions = {}) {
  const ctx = importing(files, options);
  const home = (await ctx.client.spaceList())[0]?.id ?? "";
  const report = await ctx.client.importRun({ destination: "newSpaces", spaceId: home, source: "zip" });
  const spaces = (await ctx.client.spaceList()).slice(1);
  return { ...ctx, home, report, spaces };
}

async function outline(client: CoreClient, spaceId: string): Promise<string[]> {
  const lines: string[] = [];
  const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"], depth: number): void => {
    for (const node of nodes) {
      lines.push(`${"  ".repeat(depth)}${node.page.title}`);
      visit(node.children, depth + 1);
    }
  };
  visit((await client.treeGet(spaceId)).roots, 0);
  return lines;
}

/** Every live page of every space by title (the first wins). */
async function pagesByTitle(client: CoreClient): Promise<Map<string, string>> {
  const found = new Map<string, string>();
  for (const space of await client.spaceList()) {
    const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): void => {
      for (const node of nodes) {
        if (!found.has(node.page.title)) {
          found.set(node.page.title, node.page.id);
        }
        visit(node.children);
      }
    };
    visit((await client.treeGet(space.id)).roots);
  }
  return found;
}

/** The portable content of a page without the block ids. */
async function contentOf(client: CoreClient, pageId: string): Promise<JsonValue> {
  const strip = (value: JsonValue): JsonValue => {
    if (Array.isArray(value)) {
      return value.map(strip);
    }
    if (typeof value !== "object" || value === null) {
      return value;
    }
    const copy: { [key: string]: JsonValue } = {};
    for (const [key, child] of Object.entries(value)) {
      if (key === "attrs" && typeof child === "object" && child !== null && !Array.isArray(child)) {
        const { id: _ignored, ...attrs } = child;
        void _ignored;
        if (Object.keys(attrs).length > 0) {
          copy[key] = attrs;
        }
      } else {
        copy[key] = strip(child);
      }
    }
    return copy;
  };
  return strip((await client.pageContentGet(pageId)).content);
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** The text files of an archive as the shell would give them to a session, and its binary paths. */
function split(files: Record<string, string | Uint8Array>): { texts: [string, string][]; binary: string[] } {
  const texts: [string, string][] = [];
  const binary: string[] = [];
  for (const [path, content] of Object.entries(files)) {
    if (path.startsWith("collab_objects/")) {
      continue;
    }
    if (typeof content === "string") {
      texts.push([path, content]);
    } else {
      binary.push(path);
    }
  }
  return { texts, binary };
}

/** Opens an `ImportBeginSpaces` session with the archive's text files and plans it. */
async function planSpaces(ctx: Context, files: Record<string, string | Uint8Array>, sourceName?: string) {
  const { texts, binary } = split(files);
  const begun = await ctx.transport.dispatch({ type: "ImportBeginSpaces", data: sourceName === undefined ? {} : { sourceName } });
  if (begun.type !== "ImportStarted") {
    throw new Error("unexpected response");
  }
  for (const [path, body] of texts) {
    await ctx.transport.dispatch({ type: "ImportAddFile", data: { importId: begun.data, path, text: body } });
  }
  const planned = await ctx.transport.dispatch({ type: "ImportPlan", data: { importId: begun.data, binaryPaths: binary } });
  if (planned.type !== "ImportPlanned") {
    throw new Error("unexpected response");
  }
  return { importId: begun.data, plan: planned.data };
}

async function commitAll(ctx: Context, importId: number): Promise<ImportReport> {
  for (;;) {
    const response = await ctx.transport.dispatch({ type: "ImportCommit", data: { importId, maxPages: 50 } });
    if (response.type !== "ImportProgress") {
      throw new Error("unexpected response");
    }
    if (response.data.report !== undefined) {
      return response.data.report;
    }
  }
}

// ----- the demo backup ----------------------------------------------------------------------------

describe("the demo backup through the requests", () => {
  const demo = (): Record<string, string | Uint8Array> => demoImportSources()["AppFlowy backup.zip"]?.files ?? {};

  it("creates the spaces at the plan, in order, and answers the spaces, the file spaces and the remote files", async () => {
    const ctx = importing(demo());
    const before = (await ctx.client.spaceList()).length;
    ctx.events.length = 0;
    const { plan } = await planSpaces(ctx, demo(), "AppFlowy backup");
    const spaces = await ctx.client.spaceList();
    expect(spaces.map((space) => space.name)).toEqual(["Home", "Personal", "Work"]);
    expect(before).toBe(1);
    expect(ctx.events).toEqual([{ type: "SpacesChanged" }]);
    expect(plan.pages).toBe(9);
    expect(plan.spaces).toEqual([
      { spaceId: spaces[1]?.id, name: "Personal", pages: 7 },
      { spaceId: spaces[2]?.id, name: "Work", pages: 2 },
    ]);
    expect(plan.neededFiles).toEqual(["local_files/mK3qWf0dYV4j2u9c8gH7nQe1RZpTbXsLhA6oiNyD5Ew.png"]);
    // The picture is used by a page of "Work": it is uploaded there.
    expect(plan.fileSpaces).toEqual([spaces[2]?.id]);
    expect(plan.remoteFiles).toEqual([
      { key: "remote/0", url: "https://files.appflowy.example/workspace/diagram.png", host: "files.appflowy.example", spaceId: spaces[2]?.id },
    ]);
    expect(plan.warnings).toBeUndefined();
    // Nothing is a page yet.
    expect(spaces.map((space) => space.pageCount)).toEqual([1, 0, 0]);
  });

  it("creates the pages per space, the properties for the space that uses them, and reports the spaces and their root pages", async () => {
    const ctx = importing(demo());
    const { importId } = await planSpaces(ctx, demo());
    const [, personal, work] = await ctx.client.spaceList();
    const attachment = async (spaceId: string): Promise<string> =>
      (await ctx.client.attachmentUpload(spaceId, "picture.png", "image/png", [demoImagePng()])).attachmentId;
    await ctx.transport.dispatch({
      type: "ImportAddAttachment",
      data: {
        importId,
        path: "local_files/mK3qWf0dYV4j2u9c8gH7nQe1RZpTbXsLhA6oiNyD5Ew.png",
        attachmentId: await attachment(work?.id ?? ""),
      },
    });
    const report = await commitAll(ctx, importId);
    expect(report).toMatchObject({ pages: 9, attachments: 1, propertiesCreated: 2, moreWarnings: 0 });
    expect(report.spaceIds).toEqual([personal?.id, work?.id]);
    const titles = await pagesByTitle(ctx.client);
    expect(report.rootPageIds).toEqual(["Welcome", "Projects", "Meeting notes", "Reading list"].map((title) => titles.get(title)));
    expect(await ctx.client.propertySchemaGet(personal?.id ?? "")).toHaveLength(3);
    expect(await ctx.client.propertySchemaGet(work?.id ?? "")).toHaveLength(1);
    // The remote image was not reported: it stays a link, with its warning.
    expect(report.warnings).toEqual([expect.objectContaining({ code: "remoteImage" })]);
    // The session is closed.
    expect((await failure(ctx.client.importRun({ spaceId: personal?.id ?? "", source: "folder" }))).code).toBe("InvalidInput");
  });

  it("writes the content: headings, nested lists, to-dos, a page mention across the tree, links and the picture", async () => {
    const { client, spaces } = await intoSpaces(demo());
    const titles = await pagesByTitle(client);
    const welcome = await contentOf(client, titles.get("Welcome") ?? "");
    expect(welcome).toEqual({
      type: "doc",
      content: [
        { type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: "Welcome to the backup" }] },
        {
          type: "paragraph",
          content: [
            { type: "text", text: "This page came from an AppFlowy backup, with a " },
            { type: "text", marks: [{ type: "bold" }], text: "bold" },
            { type: "text", text: " word and a look at the " },
            { type: "pageRef", attrs: { pageId: titles.get("Roadmap") } },
            { type: "text", text: "." },
          ],
        },
        {
          type: "bulletList",
          content: [
            {
              type: "listItem",
              content: [
                { type: "paragraph", content: [{ type: "text", text: "First idea" }] },
                {
                  type: "bulletList",
                  content: [{ type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "A nested thought" }] }] }],
                },
              ],
            },
            { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "Second idea" }] }] },
          ],
        },
        {
          type: "taskList",
          content: [
            { type: "taskItem", attrs: { checked: false }, content: [{ type: "paragraph", content: [{ type: "text", text: "Pack the demo" }] }] },
            { type: "taskItem", attrs: { checked: true }, content: [{ type: "paragraph", content: [{ type: "text", text: "Read the backup" }] }] },
          ],
        },
      ],
    });
    expect((await client.backlinksGet(titles.get("Roadmap") ?? "")).map((link) => link.title)).toEqual(["Welcome"]);
    const projects = await contentOf(client, titles.get("Projects") ?? "");
    expect(projects).toMatchObject({ content: [{ type: "paragraph" }, { type: "pageLink", attrs: { pageId: titles.get("Tasks board") } }] });
    const roadmap = JSON.stringify(await contentOf(client, titles.get("Roadmap") ?? ""));
    expect(roadmap).toContain('"marks":[{"type":"link","attrs":{"href":"https://example.com/plan"}}]');
    expect(roadmap).toContain('"type":"horizontalRule"');
    // The picture is an image block on the attachment of the space of its page.
    const meeting = (await contentOf(client, titles.get("Meeting notes") ?? "")) as { content: { type: string; attrs?: { attachmentId?: string; align?: string } }[] };
    const image = meeting.content[1];
    expect(image).toMatchObject({ type: "image", attrs: { align: "center" } });
    expect((await client.attachmentGet(image?.attrs?.attachmentId ?? "")).spaceId).toBe(spaces[1]?.id);
    // The remote image is a link to its address.
    expect(JSON.stringify(meeting.content[2])).toContain("https://files.appflowy.example/workspace/diagram.png");
  });

  it("makes the board a page with a board view, its rows child pages with values and the row document", async () => {
    const { client, spaces } = await intoSpaces(demo());
    const titles = await pagesByTitle(client);
    const schema = new Map((await client.propertySchemaGet(spaces[0]?.id ?? "")).map((def) => [def.name, def]));
    expect([...schema.keys()]).toEqual(["Tags", "Notes", "Status"]);
    expect(schema.get("Status")).toMatchObject({ kind: "select" });
    expect(schema.get("Status")?.options.map((option) => [option.name, option.color])).toEqual([
      ["To do", "purple"],
      ["Doing", "blue"],
      ["Done", "green"],
    ]);
    const board = (await contentOf(client, titles.get("Tasks board") ?? "")) as { content: { type: string; attrs: Record<string, JsonValue> }[] };
    expect(board.content).toEqual([
      {
        type: "pageList",
        attrs: {
          groupBy: schema.get("Status")?.id,
          layout: "board",
          properties: [schema.get("Notes")?.id, schema.get("Status")?.id],
          source: { kind: "children" },
        },
      },
    ]);
    const row = titles.get("Write the report") ?? "";
    expect((await client.pageGet(row)).icon).toBe("📝");
    const values = new Map((await client.pagePropertiesGet(row)).map(([id, value]) => [id, value]));
    expect(values.get(schema.get("Notes")?.id ?? "")).toEqual({ kind: "text", value: "Due Friday" });
    expect(values.get(schema.get("Status")?.id ?? "")).toEqual({
      kind: "select",
      value: schema.get("Status")?.options.find((option) => option.name === "To do")?.id,
    });
    expect(await contentOf(client, row)).toMatchObject({
      content: [{ type: "heading", attrs: { level: 3 } }, { type: "paragraph", content: [{ type: "text", text: "Three sections, one chart." }] }],
    });
    // The row without a document is an empty page; the row documents are no pages of their own.
    expect(await contentOf(client, titles.get("Send it") ?? "")).toEqual({ type: "doc", content: [{ type: "paragraph" }] });
    expect(titles.has("Untitled")).toBe(false);
    const filter = await client.pagesByProperty(spaces[0]?.id ?? "", schema.get("Status")?.id ?? "", {
      type: "ContainsOption",
      data: schema.get("Status")?.options.find((option) => option.name === "Done")?.id ?? "",
    });
    expect(filter).toEqual([titles.get("Send it")]);
  });

  it("reports an image the shell downloaded as an attachment under its key", async () => {
    const ctx = importing(demo());
    const { importId, plan } = await planSpaces(ctx, demo());
    const picture = plan.neededFiles[0] ?? "";
    const space = plan.spaces?.[1]?.spaceId ?? "";
    const upload = async (name: string): Promise<string> =>
      (await ctx.client.attachmentUpload(space, name, "image/png", [demoImagePng()])).attachmentId;
    await ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: picture, attachmentId: await upload("picture.png") } });
    await ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: "remote/0", attachmentId: await upload("diagram.png") } });
    const report = await commitAll(ctx, importId);
    expect(report.attachments).toBe(2);
    expect(report.warnings).toBeUndefined();
    const titles = await pagesByTitle(ctx.client);
    const meeting = JSON.stringify(await contentOf(ctx.client, titles.get("Meeting notes") ?? ""));
    expect(meeting.match(/"type":"image"/gu)).toHaveLength(2);
  });

  it("refuses an attachment of another space, an unknown path, and one before the plan", async () => {
    const ctx = importing(demo());
    const begun = await ctx.transport.dispatch({ type: "ImportBeginSpaces", data: {} });
    const importId = begun.type === "ImportStarted" ? begun.data : 0;
    const early = await failure(
      ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: "x", attachmentId: "00000000-0000-4000-8000-000000000001" } }),
    );
    expect(early.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "importId" } });
    const { texts, binary } = split(demo());
    for (const [path, body] of texts) {
      await ctx.transport.dispatch({ type: "ImportAddFile", data: { importId, path, text: body } });
    }
    const planned = await ctx.transport.dispatch({ type: "ImportPlan", data: { importId, binaryPaths: binary } });
    const plan: ImportPlanInfo = planned.type === "ImportPlanned" ? planned.data : { pages: 0, neededFiles: [] };
    const home = (await ctx.client.spaceList())[0]?.id ?? "";
    const upload = async (space: string): Promise<string> =>
      (await ctx.client.attachmentUpload(space, "p.png", "image/png", [demoImagePng()])).attachmentId;
    const wrong = await failure(
      ctx.transport.dispatch({
        type: "ImportAddAttachment",
        data: { importId, path: plan.neededFiles[0] ?? "", attachmentId: await upload(home) },
      }),
    );
    expect(wrong.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "attachmentId" } });
    const unknown = await failure(
      ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: "local_files/other.png", attachmentId: await upload(home) } }),
    );
    expect(unknown.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "path" } });
  });
});

// ----- sessions: abort, failure, other sources -----------------------------------------------------

describe("sessions that create spaces", () => {
  const demo = (): Record<string, string | Uint8Array> => demoImportSources()["AppFlowy backup.zip"]?.files ?? {};

  it("deletes the spaces that have no page when the session is aborted, with the attachments uploaded into them", async () => {
    const ctx = importing(demo());
    const { importId, plan } = await planSpaces(ctx, demo());
    const work = plan.spaces?.[1]?.spaceId ?? "";
    const info = await ctx.client.attachmentUpload(work, "picture.png", "image/png", [demoImagePng()]);
    await ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: plan.neededFiles[0] ?? "", attachmentId: info.attachmentId } });
    ctx.events.length = 0;
    await ctx.transport.dispatch({ type: "ImportAbort", data: { importId } });
    expect((await ctx.client.spaceList()).map((space) => space.name)).toEqual(["Home"]);
    expect(ctx.events).toContainEqual({ type: "SpacesChanged" });
    expect((await failure(ctx.client.attachmentGet(info.attachmentId))).code).toBe("NotFound");
    // Aborting again, or a session that is gone, is fine.
    await ctx.transport.dispatch({ type: "ImportAbort", data: { importId } });
  });

  it("keeps the spaces that have pages when an abort comes after the pages were created", async () => {
    const ctx = importing(demo());
    const { importId } = await planSpaces(ctx, demo());
    await ctx.transport.dispatch({ type: "ImportCommit", data: { importId, maxPages: 1 } });
    await ctx.transport.dispatch({ type: "ImportAbort", data: { importId } });
    expect((await ctx.client.spaceList()).map((space) => [space.name, space.pageCount])).toEqual([
      ["Home", 1],
      ["Personal", 7],
      ["Work", 2],
    ]);
  });

  it("ends the session and deletes the empty spaces when a commit fails", async () => {
    const ctx = importing(demo());
    const { importId, plan } = await planSpaces(ctx, demo());
    // A space deleted behind the session's back: its pages cannot be created.
    await ctx.client.spaceDelete(plan.spaces?.[1]?.spaceId ?? "");
    const refused = await failure(ctx.transport.dispatch({ type: "ImportCommit", data: { importId, maxPages: 50 } }));
    expect(refused.code).toBe("NotFound");
    expect((await ctx.client.spaceList()).map((space) => space.name)).toEqual(["Home", "Personal"]);
    expect((await failure(ctx.transport.dispatch({ type: "ImportCommit", data: { importId, maxPages: 50 } }))).code).toBe("NotFound");
  });

  it("drops the spaces of an earlier session that never started when a new session begins", async () => {
    const ctx = importing(demo());
    await planSpaces(ctx, demo());
    expect((await ctx.client.spaceList()).map((space) => space.name)).toEqual(["Home", "Personal", "Work"]);
    await ctx.transport.dispatch({ type: "ImportBeginSpaces", data: {} });
    expect((await ctx.client.spaceList()).map((space) => space.name)).toEqual(["Home"]);
  });

  it("puts a source without spaces in one new space named after it, without a container page", async () => {
    const notion = demoImportSources()["Notion export.zip"] as MemoryImportSource;
    const ctx = importing({}, { importSources: { "Notion export.zip": notion } });
    const home = (await ctx.client.spaceList())[0]?.id ?? "";
    const report = await ctx.client.importRun({ destination: "newSpaces", spaceId: home, source: "zip" });
    const spaces = await ctx.client.spaceList();
    expect(spaces.map((space) => space.name)).toEqual(["Home", "Notion export"]);
    expect(report.spaceIds).toEqual([spaces[1]?.id]);
    expect(await outline(ctx.client, spaces[1]?.id ?? "")).toEqual(["Getting Started", "  Notes", "Task List", "  Write the report", "  Send it"]);
    // Its picture is an attachment of the new space, and the properties are the new space's.
    expect((await ctx.client.attachmentList(spaces[1]?.id ?? "")).map((info) => info.name)).toEqual(["picture.png"]);
    expect(await ctx.client.attachmentList(home)).toEqual([]);
    expect((await ctx.client.propertySchemaGet(home)).map((def) => def.name)).toEqual(["Tags"]);
    expect(report).toMatchObject({ pages: 5, attachments: 1, propertiesCreated: 3 });
    expect(report.rootPageIds).toHaveLength(2);
  });

  it("names the space \"Imported\" when the source has no name", async () => {
    const ctx = importing({});
    const begun = await planFiles(ctx, { "Note.md": "# Note\n\nHello\n" });
    expect(begun.plan.spaces?.map((space) => space.name)).toEqual(["Imported"]);
    expect(begun.plan.fileSpaces).toEqual([]);
    const report = await commitAll(ctx, begun.importId);
    expect(report.pages).toBe(1);
  });

  it("keeps the properties and relations of each space apart", async () => {
    const field = afId("f", 1);
    const status = afId("f", 2);
    const fieldB = afId("f", 3);
    const views = [
      { id: S1, name: "One", extra: SPACE, children: [afId("b", 1)] },
      { id: S2, name: "Two", extra: SPACE, children: [afId("b", 2)] },
      { id: afId("b", 1), name: "Board one", parent: S1, layout: 1, extra: { database_id: afId("d", 1) } },
      { id: afId("b", 2), name: "Board two", parent: S2, layout: 1, extra: { database_id: afId("d", 2) } },
    ];
    const files = archive(
      views,
      {},
      {
        [afId("b", 1)]: database(afId("d", 1), afId("b", 1), [{ id: field, name: "Name", type: 0 }, { id: status, name: "Size", type: 1 }], [
          { id: afId("e", 1), cells: { [field]: "A", [status]: "3" } },
        ]),
        [afId("b", 2)]: database(afId("d", 2), afId("b", 2), [{ id: fieldB, name: "Name", type: 0 }, { id: afId("f", 4), name: "Size", type: 1 }, { id: afId("f", 5), name: "Only here", type: 0 }], [
          { id: afId("e", 2), cells: { [fieldB]: "B", [afId("f", 4)]: "5", [afId("f", 5)]: "x" } },
        ]),
      },
    );
    const { client, spaces, report } = await intoSpaces(files);
    expect(report.propertiesCreated).toBe(3);
    expect((await client.propertySchemaGet(spaces[0]?.id ?? "")).map((def) => def.name)).toEqual(["Tags", "Size"]);
    expect((await client.propertySchemaGet(spaces[1]?.id ?? "")).map((def) => def.name)).toEqual(["Tags", "Size", "Only here"]);
  });
});

/** A session of text files that is not an AppFlowy backup (no `.json` is read), planned. */
async function planFiles(ctx: Context, files: Record<string, string>) {
  const begun = await ctx.transport.dispatch({ type: "ImportBeginSpaces", data: {} });
  const importId = begun.type === "ImportStarted" ? begun.data : 0;
  for (const [path, body] of Object.entries(files)) {
    await ctx.transport.dispatch({ type: "ImportAddFile", data: { importId, path, text: body } });
  }
  const planned = await ctx.transport.dispatch({ type: "ImportPlan", data: { importId, binaryPaths: [] } });
  return { importId, plan: planned.type === "ImportPlanned" ? planned.data : ({ pages: 0, neededFiles: [] } as ImportPlanInfo) };
}

// ----- the reader -----------------------------------------------------------------------------------

describe("the tree", () => {
  it("plans the spaces in order, the views in the order of `children`, a view listed twice once, and cuts a cycle", async () => {
    const [a, b, c, d] = [afId("b", 1), afId("b", 2), afId("b", 3), afId("b", 4)];
    const files = archive(
      [
        { id: S1, name: "First", extra: SPACE, children: [b, a, b] },
        { id: S2, name: "Second", extra: SPACE, children: [c] },
        { id: a, name: "A", parent: S1, children: [d] },
        { id: b, name: "B", parent: S1 },
        { id: c, name: "C", parent: S2, children: [d] },
        // D is listed under A and under C, and lists C: a cycle.
        { id: d, name: "D", parent: a, children: [c] },
      ],
      { [a]: [text("paragraph", [plain("a")])] },
    );
    const { client, spaces } = await intoSpaces(files);
    expect(spaces.map((space) => space.name)).toEqual(["First", "Second"]);
    expect(await outline(client, spaces[0]?.id ?? "")).toEqual(["B", "A", "  D", "    C"]);
    expect(await outline(client, spaces[1]?.id ?? "")).toEqual([]);
  });

  it("puts the views no space reaches in one more space, \"Other pages\", and leaves row documents out", async () => {
    const [x, y, z] = [afId("b", 1), afId("b", 2), afId("b", 3)];
    const row = afId("e", 1);
    const document = uuidV5(row, "document_id") ?? "";
    const field = afId("f", 1);
    const files = archive(
      [
        { id: S1, name: "Only space", extra: SPACE, children: [x, afId("b", 5)] },
        { id: x, name: "Inside", parent: S1 },
        // Reachable from nowhere: a loose view and its child, an unknown parent, a row document.
        { id: y, name: "Loose", children: [z] },
        { id: z, name: "Loose child", parent: y },
        { id: afId("b", 4), name: "Orphan", parent: afId("9", 9) },
        { id: afId("b", 5), name: "Table", layout: 1, parent: S1, extra: { database_id: afId("d", 1) } },
        { id: document, name: "Untitled" },
      ],
      {},
      { [afId("b", 5)]: database(afId("d", 1), afId("b", 5), [{ id: field, name: "Name", type: 0 }], [{ id: row, cells: { [field]: "Row" } }]) },
    );
    const { client, spaces, report } = await intoSpaces(files);
    expect(spaces.map((space) => space.name)).toEqual(["Only space", "Other pages"]);
    expect(await outline(client, spaces[1]?.id ?? "")).toEqual(["Loose", "  Loose child", "Orphan"]);
    expect(await outline(client, spaces[0]?.id ?? "")).toEqual(["Inside", "Table", "  Row"]);
    expect(report.pages).toBe(6);
  });

  it("warns for a view without document and plans it empty, and drops other icons and covers once", async () => {
    const [a, b, c] = [afId("b", 1), afId("b", 2), afId("b", 3)];
    const files = archive(
      [
        { id: S1, name: "Space", extra: SPACE, children: [a, b, c] },
        { id: a, name: "No document", parent: S1, icon: { ty: 1, value: "interface_essential/star" } },
        { id: b, name: "Covered", parent: S1, icon: { ty: 2, value: "https://example.com/i.png" }, extra: { cover: { type: "color", value: "#fff" } } },
        { id: c, name: "Emoji", parent: S1, icon: { ty: 0, value: "🚀" } },
      ],
      { [b]: [], [c]: [] },
    );
    const { client, spaces, report } = await intoSpaces(files);
    const tree = (await client.treeGet(spaces[0]?.id ?? "")).roots.map((node) => [node.page.title, node.page.icon]);
    expect(tree).toEqual([
      ["No document", undefined],
      ["Covered", undefined],
      ["Emoji", "🚀"],
    ]);
    // The tree is read first (icons, covers), then the documents.
    expect(report.warnings).toEqual([
      { path: "relation_map.json", code: "unsupportedContent", detail: "icon" },
      { path: "relation_map.json", code: "unsupportedContent", detail: "cover" },
      { path: `collab_jsons/documents/${a}.json`, code: "missingFile", detail: "No document" },
    ]);
  });

  it("makes safe names of hostile ones: control characters, an empty name, very long names", async () => {
    const [a, b] = [afId("b", 1), afId("b", 2)];
    const files = archive([
      { id: S1, name: `Tab\tbed\nspace ${"x".repeat(300)}`, extra: SPACE, children: [a, b] },
      { id: S2, name: "   ", extra: SPACE },
      { id: a, name: "", parent: S1 },
      { id: b, name: `Line\nbreak ${"y".repeat(600)}`, parent: S1 },
    ]);
    const { client, spaces } = await intoSpaces(files);
    expect(spaces[0]?.name.length).toBeLessThanOrEqual(100);
    expect(spaces[0]?.name.startsWith("Tab bed space ")).toBe(true);
    // An empty name is "Untitled", as a page's.
    expect(spaces[1]?.name).toBe("Untitled");
    const titles = (await client.treeGet(spaces[0]?.id ?? "")).roots.map((node) => node.page.title);
    expect(titles[0]).toBe("Untitled");
    expect(titles[1]?.length).toBe(500);
    expect(titles[1]?.startsWith("Line break ")).toBe(true);
  });
});

describe("documents", () => {
  const page = afId("b", 1);
  const files = (blocks: readonly BlockSpec[], more: Record<string, string | Uint8Array> = {}, views: ViewSpec[] = []) =>
    archive(
      [{ id: S1, name: "Space", extra: SPACE, children: [page] }, { id: page, name: "Page", parent: S1 }, ...views],
      { [page]: blocks },
      {},
      more,
    );
  const read = async (blocks: readonly BlockSpec[], more: Record<string, string | Uint8Array> = {}, views: ViewSpec[] = []) => {
    const run = await intoSpaces(files(blocks, more, views));
    const id = (await pagesByTitle(run.client)).get("Page") ?? "";
    return { ...run, content: (await contentOf(run.client, id)) as { content?: JsonValue[] } };
  };

  it("maps the blocks of the table: headings clamped to level 3, quotes, callouts, toggles, code, equations, dividers, outlines", async () => {
    const { content, report } = await read([
      text("heading", [plain("Deep")], { level: 5 }),
      text("quote", [plain("Said")]),
      text("callout", [plain("Careful")], { icon: "💡" }),
      text("toggle_list", [plain("More")], { collapsed: true }, [text("paragraph", [plain("Hidden")])]),
      text("code", [plain("let x = 1;")], { language: "Rust" }),
      { ty: "math_equation", data: { formula: "E=mc^2" } },
      { ty: "divider" },
      { ty: "outline" },
    ]);
    expect(report.warnings).toBeUndefined();
    expect(content.content).toEqual([
      { type: "heading", attrs: { level: 3 }, content: [{ type: "text", text: "Deep" }] },
      { type: "blockquote", content: [{ type: "paragraph", content: [{ type: "text", text: "Said" }] }] },
      { type: "callout", attrs: { icon: "💡" }, content: [{ type: "paragraph", content: [{ type: "text", text: "Careful" }] }] },
      {
        type: "details",
        attrs: { open: false },
        content: [
          { type: "detailsSummary", content: [{ type: "text", text: "More" }] },
          { type: "detailsContent", content: [{ type: "paragraph", content: [{ type: "text", text: "Hidden" }] }] },
        ],
      },
      { type: "codeBlock", attrs: { language: "rust" }, content: [{ type: "text", text: "let x = 1;" }] },
      { type: "mathBlock", content: [{ type: "text", text: "E=mc^2" }] },
      { type: "horizontalRule" },
      { type: "tableOfContents" },
    ]);
  });

  it("maps a toggle heading to a toggle whose summary has its level, clamped to 3, and a toggle list to a plain one (ADR-0040)", async () => {
    const { content, report } = await read([
      text("toggle_heading", [plain("One")], { level: 1 }),
      text("toggle_heading", [plain("Two")], { level: 2, collapsed: true }, [text("paragraph", [plain("inside")])]),
      text("toggle_heading", [plain("None")], {}),
      text("toggle_heading", [plain("Text")], { level: "x" }),
      text("toggle_heading", [plain("Deep")], { level: 9 }),
      text("toggle_list", [plain("List")], { level: 2 }),
    ]);
    expect(report.warnings).toBeUndefined();
    const summaries = (content.content ?? []).map((toggle) => (toggle as { content: { attrs?: JsonValue }[] }).content[0]?.attrs);
    expect(summaries).toEqual([{ level: 1 }, { level: 2 }, { level: 1 }, { level: 1 }, { level: 3 }, undefined]);
    expect((content.content ?? [])[1]).toEqual({
      type: "details",
      attrs: { open: false },
      content: [
        { type: "detailsSummary", attrs: { level: 2 }, content: [{ type: "text", text: "Two" }] },
        { type: "detailsContent", content: [{ type: "paragraph", content: [{ type: "text", text: "inside" }] }] },
      ],
    });
  });

  it("groups consecutive list items of one kind, keeps an ordered list's start, and nests lists in lists", async () => {
    const { content } = await read([
      text("numbered_list", [plain("three")], { number: 3 }),
      text("numbered_list", [plain("four")], { number: 4 }, [text("todo_list", [plain("sub")], { checked: true })]),
      text("bulleted_list", [plain("dot")]),
      text("numbered_list", [plain("again")], { number: 1 }),
    ]);
    expect(content.content).toEqual([
      {
        type: "orderedList",
        attrs: { start: 3 },
        content: [
          { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "three" }] }] },
          {
            type: "listItem",
            content: [
              { type: "paragraph", content: [{ type: "text", text: "four" }] },
              { type: "taskList", content: [{ type: "taskItem", attrs: { checked: true }, content: [{ type: "paragraph", content: [{ type: "text", text: "sub" }] }] }] },
            ],
          },
        ],
      },
      { type: "bulletList", content: [{ type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "dot" }] }] }] },
      { type: "orderedList", content: [{ type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text: "again" }] }] }] },
    ]);
  });

  it("reads the text: marks, links (only web and mail ones), page links by view id, date mentions, equations, line breaks", async () => {
    const other = afId("b", 2);
    const { content, report } = await read(
      [
        text("paragraph", [
          styled("all", { bold: true, italic: true, underline: true, strikethrough: true, code: true }),
          plain(" "),
          styled("web", { href: "https://example.com" }),
          plain(" "),
          styled("script", { href: "javascript:alert(1)" }),
          plain(" "),
          styled("page", { href: other, is_page_link: true }),
          plain(" "),
          styled("$", { mention: { type: "date", date: "2026-10-07T00:00:00.000" } }),
          plain(" "),
          styled("$", { mention: { type: "date", date: 1_760_000_000_000 } }),
          plain(" "),
          styled("x^2", { formula: "x^2" }),
          plain(" two\nlines"),
          plain("\n"),
        ]),
      ],
      { [`collab_jsons/documents/${other}.json`]: appFlowyDocument(other, []) },
      [{ id: other, name: "Other", parent: S1 }],
    );
    expect(report.warnings).toBeUndefined();
    const paragraph = content.content?.[0] as { content: { type: string; text?: string; marks?: { type: string; attrs?: { href: string } }[]; attrs?: Record<string, JsonValue> }[] };
    const names = paragraph.content.map((node) => node.type);
    // Runs of text with the same marks are one text: the link is dropped from "script", which joins its neighbours.
    expect(names).toEqual(["text", "text", "text", "text", "pageRef", "text", "dateMention", "text", "dateMention", "text", "mathInline", "text", "hardBreak", "text"]);
    expect(paragraph.content[0]?.marks?.map((mark) => mark.type).sort()).toEqual(["bold", "code", "italic", "strike", "underline"]);
    expect(paragraph.content[2]?.marks).toEqual([{ type: "link", attrs: { href: "https://example.com" } }]);
    // A link that is not a web or mail address keeps its text and loses the link.
    expect(paragraph.content[3]).toEqual({ type: "text", text: " script " });
    expect(paragraph.content[6]?.attrs).toEqual({ date: "2026-10-07" });
    expect(paragraph.content[8]?.attrs).toEqual({ date: "2025-10-09" });
    expect(paragraph.content[10]?.attrs).toEqual({ latex: "x^2" });
    expect(paragraph.content[4]?.attrs).toBeDefined();
  });

  it("warns once per page and block type for what it does not map, keeping the text and the children", async () => {
    const { content, report } = await read([
      text("simple_table", [plain("T")], {}, [text("paragraph", [plain("cell")])]),
      text("simple_table", [plain("U")]),
      { ty: "ai_meeting" },
      { ty: "grid", data: { view_id: afId("9", 9) } },
      text("paragraph", [styled("$", { mention: { type: "page", page_id: afId("9", 8) } })]),
    ]);
    expect(report.warnings).toEqual([
      { path: `collab_jsons/documents/${page}.json`, code: "unsupportedContent", detail: "simple_table" },
      { path: `collab_jsons/documents/${page}.json`, code: "unsupportedContent", detail: "ai_meeting" },
      { path: `collab_jsons/documents/${page}.json`, code: "unresolvedLink", detail: afId("9", 9) },
      { path: `collab_jsons/documents/${page}.json`, code: "unresolvedLink", detail: afId("9", 8) },
    ]);
    expect(content.content).toEqual([
      { type: "paragraph", content: [{ type: "text", text: "T" }] },
      { type: "paragraph", content: [{ type: "text", text: "cell" }] },
      { type: "paragraph", content: [{ type: "text", text: "U" }] },
      { type: "paragraph" },
    ]);
  });

  it("reads old blocks (the delta inline in `data`, `data` as JSON text) and an empty document as an empty page", async () => {
    const raw = JSON.stringify({
      page_id: page,
      blocks: {
        [page]: { id: page, ty: "page", parent: "", children: "root", external_id: null, external_type: null, data: {} },
        old: { id: "old", ty: "paragraph", parent: page, children: "none", external_id: null, external_type: null, data: JSON.stringify({ delta: [{ insert: "Old block" }] }) },
      },
      meta: { children_map: { root: ["old"] }, text_map: {} },
    });
    const { client } = await intoSpaces({
      ...files([]),
      [`collab_jsons/documents/${page}.json`]: raw,
    });
    const id = (await pagesByTitle(client)).get("Page") ?? "";
    expect(await contentOf(client, id)).toEqual({ type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "Old block" }] }] });
    const { client: second } = await intoSpaces(files([]));
    expect(await contentOf(second, (await pagesByTitle(second)).get("Page") ?? "")).toEqual({ type: "doc", content: [{ type: "paragraph" }] });
  });

  it("warns for a document that is not one", async () => {
    const { report } = await intoSpaces({ ...files([]), [`collab_jsons/documents/${page}.json`]: "{ not json" });
    expect(report.warnings).toEqual([{ path: `collab_jsons/documents/${page}.json`, code: "unsupportedContent", detail: "document" }]);
  });

  it("needs a file the page uses once, in the space of the first page that uses it, and warns for one the archive lacks", async () => {
    const second = afId("b", 2);
    const view = (id: string, name: string, parent: string): ViewSpec => ({ id, name, parent });
    const run = archive(
      [
        { id: S1, name: "First", extra: SPACE, children: [page] },
        { id: S2, name: "Second", extra: SPACE, children: [second] },
        view(page, "Page", S1),
        view(second, "Later", S2),
      ],
      {
        [page]: [{ ty: "image", data: { url: `${page}/pic.png`, url_type: 2 } }, { ty: "file", data: { url: `${page}/report.pdf`, url_type: 2, name: "report.pdf" } }, { ty: "image", data: { url: `${page}/gone.png`, url_type: 2 } }],
        [second]: [{ ty: "image", data: { url: `${second}/pic.png`, url_type: 2 } }, { ty: "file", data: { url: "https://example.com/doc.pdf", name: "Doc" } }, { ty: "link_preview", data: { url: "https://example.com/p" } }],
      },
      {},
      { "local_files/pic.png": demoImagePng(), "local_files/report.pdf": new Uint8Array([37, 80, 68, 70]) },
    );
    const ctx = importing(run);
    const { importId, plan } = await planSpaces(ctx, run);
    const spaces = plan.spaces ?? [];
    expect(plan.neededFiles).toEqual(["local_files/pic.png", "local_files/report.pdf"]);
    expect(plan.fileSpaces).toEqual([spaces[0]?.spaceId, spaces[0]?.spaceId]);
    expect(plan.warnings).toEqual([{ path: `collab_jsons/documents/${page}.json`, code: "missingFile", detail: "gone.png" }]);
    for (const [at, path] of plan.neededFiles.entries()) {
      const info = await ctx.client.attachmentUpload(plan.fileSpaces?.[at] ?? "", path.split("/").at(-1) ?? path, "application/octet-stream", [demoImagePng()]);
      await ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path, attachmentId: info.attachmentId } });
    }
    const report = await commitAll(ctx, importId);
    expect(report.attachments).toBe(2);
    const titles = await pagesByTitle(ctx.client);
    const later = JSON.stringify(await contentOf(ctx.client, titles.get("Later") ?? ""));
    expect(later).toContain('"type":"image"');
    expect(later).toContain("https://example.com/doc.pdf");
    expect(later).toContain("https://example.com/p");
  });
});

describe("databases", () => {
  const view = afId("b", 1);
  const dbId = afId("d", 1);
  const f = { name: afId("f", 1), notes: afId("f", 2), count: afId("f", 3), when: afId("f", 4), tags: afId("f", 5), done: afId("f", 6), link: afId("f", 7), list: afId("f", 8), made: afId("f", 9), pick: afId("f", 10), rel: afId("f", 11) };
  const options = { a: afId("0", 1), b: afId("0", 2), c: afId("0", 3) };
  const fields = [
    { id: f.name, name: "Name", type: 0 },
    { id: f.notes, name: "Notes", type: 0 },
    { id: f.count, name: "Count", type: 1 },
    { id: f.when, name: "When", type: 2 },
    { id: f.tags, name: "Tags here", type: 4, options: [{ id: options.a, name: "Red", color: "LightPink" }, { id: options.b, name: "Cool", color: "Aqua" }, { id: options.c, name: "Odd", color: "Mystery" }] },
    { id: f.done, name: "Done", type: 5 },
    { id: f.link, name: "Site", type: 6 },
    { id: f.list, name: "Steps", type: 7 },
    { id: f.made, name: "Created", type: 9 },
    { id: f.pick, name: "Pick", type: 3, options: [{ id: options.a, name: "Yes" }] },
    { id: f.rel, name: "Related", type: 10 },
  ];
  const withRows = (rows: { id: string; cells: Record<string, unknown>; icon?: string }[], viewSettings: Record<string, unknown> = {}, layout = 1) =>
    archive(
      [{ id: S1, name: "Space", extra: SPACE, children: [view] }, { id: view, name: "Data", parent: S1, layout, extra: { database_id: dbId } }],
      {},
      { [view]: database(dbId, view, fields, rows, viewSettings) },
    );

  it("maps the kinds of fields: text, number, date, multi-select with colours, checkbox, URL, checklist; computed fields and relations are not imported", async () => {
    const files = withRows([
      {
        id: afId("e", 1),
        cells: {
          [f.name]: "Row one",
          [f.notes]: "note",
          [f.count]: "12.5",
          [f.when]: "1760000000",
          [f.tags]: `${options.a}, ${options.b}`,
          [f.done]: "Yes",
          [f.link]: "https://example.com",
          [f.list]: JSON.stringify({ options: [{ id: "i1", name: "Buy" }, { id: "i2", name: "Pay" }], selected_option_ids: ["i2"] }),
          [f.made]: "1760000000",
          [f.rel]: "x",
        },
      },
    ]);
    const { client, spaces, report } = await intoSpaces(files);
    const schema = new Map((await client.propertySchemaGet(spaces[0]?.id ?? "")).map((def) => [def.name, def]));
    expect([...schema.entries()].map(([name, def]) => `${name}:${def.kind}`)).toEqual([
      "Tags:multi_select",
      "Notes:text",
      "Count:number",
      "When:date",
      "Tags here:multi_select",
      "Done:checkbox",
      "Site:url",
      "Steps:checklist",
      "Pick:select",
    ]);
    expect(schema.get("Tags here")?.options.map((option) => [option.name, option.color])).toEqual([
      ["Red", "pink"],
      ["Cool", "blue"],
      ["Odd", expect.any(String)],
    ]);
    const row = (await pagesByTitle(client)).get("Row one") ?? "";
    const values = new Map((await client.pagePropertiesGet(row)).map(([id, value]) => [id, value]));
    const get = (name: string) => values.get(schema.get(name)?.id ?? "");
    expect(get("Notes")).toEqual({ kind: "text", value: "note" });
    expect(get("Count")).toEqual({ kind: "number", value: 12.5 });
    expect(get("When")).toEqual({ kind: "date", value: { date: "2025-10-09" } });
    expect(get("Tags here")).toMatchObject({ kind: "multi_select" });
    expect(get("Done")).toEqual({ kind: "checkbox", value: true });
    expect(get("Site")).toEqual({ kind: "url", value: "https://example.com" });
    expect(get("Steps")).toMatchObject({ kind: "checklist", value: [{ text: "Buy", done: false }, { text: "Pay", done: true }] });
    expect(report.warnings).toEqual([{ path: `collab_jsons/databases/${view}.json`, code: "unsupportedContent", detail: "Related" }]);
  });

  it("warns for a value that does not fit (a number, a date, an option id the field lacks)", async () => {
    const files = withRows([
      { id: afId("e", 1), cells: { [f.name]: "Odd row", [f.count]: "many", [f.when]: "soon", [f.pick]: afId("0", 9), [f.done]: "maybe" } },
    ]);
    const { report } = await intoSpaces(files);
    expect(report.warnings).toEqual([
      { path: `collab_jsons/databases/${view}.json`, code: "unsupportedContent", detail: "Related" },
      ...["Count", "When", "Done", "Pick"].map((name) => ({ path: `collab_jsons/databases/${view}.json`, code: "invalidValue", detail: name })),
    ]);
  });

  it("orders the rows as the view does, then the rest; the primary cell is the title; a row's icon is an emoji", async () => {
    const [r1, r2, r3, r4] = [afId("e", 1), afId("e", 2), afId("e", 3), afId("e", 4)];
    const files = withRows(
      [
        { id: r1, cells: { [f.name]: "First in rows" } },
        { id: r2, cells: { [f.name]: "" }, icon: "https://example.com/icon.png" },
        { id: r3, cells: { [f.name]: "Third" }, icon: "🌟" },
        { id: r4, cells: { [f.name]: "Fourth in rows" } },
      ],
      { row_orders: [{ id: r3 }, { id: r2 }, { id: afId("e", 99) }, { id: r3 }] },
    );
    const { client, spaces } = await intoSpaces(files);
    const nodes = (await client.treeGet(spaces[0]?.id ?? "")).roots[0]?.children ?? [];
    expect(nodes.map((node) => [node.page.title, node.page.icon])).toEqual([
      ["Third", "🌟"],
      ["Untitled", undefined],
      ["First in rows", undefined],
      ["Fourth in rows", undefined],
    ]);
  });

  it("makes grids and other layouts tables, a calendar with a warning, and warns once for sorts and filters", async () => {
    const files = withRows([{ id: afId("e", 1), cells: { [f.name]: "Row" } }], { sorts: [{ id: "s" }], filters: [{ id: "f" }] }, 3);
    const { client, spaces, report } = await intoSpaces(files);
    const board = (await contentOf(client, (await client.treeGet(spaces[0]?.id ?? "")).roots[0]?.page.id ?? "")) as { content: { attrs: { layout: string; properties: string[] } }[] };
    expect(board.content[0]?.attrs.layout).toBe("table");
    // Every field but the title, the created time and the relation.
    expect(board.content[0]?.attrs.properties).toHaveLength(8);
    expect(report.warnings).toEqual(
      expect.arrayContaining([
        { path: `collab_jsons/databases/${view}.json`, code: "unsupportedContent", detail: "calendar" },
        { path: `collab_jsons/databases/${view}.json`, code: "unsupportedContent", detail: "sort or filter" },
      ]),
    );
  });

  it("hides the fields the view hides and groups a board by the view's group field", async () => {
    const files = withRows(
      [{ id: afId("e", 1), cells: { [f.name]: "Row", [f.pick]: options.a } }],
      {
        layout: 2,
        field_settings: { [f.notes]: { visibility: 2 }, [f.count]: { visibility: 0 } },
        group_settings: [{ field_id: f.pick, groups: [{ id: options.a, visible: true }] }],
      },
      2,
    );
    const { client, spaces } = await intoSpaces(files);
    const schema = new Map((await client.propertySchemaGet(spaces[0]?.id ?? "")).map((def) => [def.name, def.id]));
    const page = (await client.treeGet(spaces[0]?.id ?? "")).roots[0]?.page.id ?? "";
    const block = ((await contentOf(client, page)) as { content: { attrs: { layout: string; groupBy: string; properties: string[] } }[] }).content[0]?.attrs;
    expect(block?.layout).toBe("board");
    expect(block?.groupBy).toBe(schema.get("Pick"));
    expect(block?.properties).not.toContain(schema.get("Notes"));
    expect(block?.properties).toContain(schema.get("Count"));
  });

  it("plans a second view of a database as a page that links to the first, and merges same-named fields over the import", async () => {
    const second = afId("b", 2);
    const otherDb = afId("d", 2);
    const files = archive(
      [
        { id: S1, name: "Space", extra: SPACE, children: [view, second, afId("b", 3)] },
        { id: view, name: "Data", parent: S1, layout: 1, extra: { database_id: dbId } },
        { id: second, name: "Data again", parent: S1, layout: 2, extra: { database_id: dbId } },
        { id: afId("b", 3), name: "More data", parent: S1, layout: 1, extra: { database_id: otherDb } },
      ],
      {},
      {
        [view]: database(dbId, view, [{ id: f.name, name: "Name", type: 0 }, { id: f.count, name: "Count", type: 1 }], [{ id: afId("e", 1), cells: { [f.name]: "A", [f.count]: "1" } }]),
        [second]: database(dbId, second, [{ id: f.name, name: "Name", type: 0 }, { id: f.count, name: "Count", type: 1 }], [{ id: afId("e", 1), cells: { [f.name]: "A", [f.count]: "1" } }]),
        [afId("b", 3)]: database(otherDb, afId("b", 3), [{ id: afId("f", 20), name: "Name", type: 0 }, { id: afId("f", 21), name: "count", type: 0 }], [{ id: afId("e", 2), cells: { [afId("f", 20)]: "B", [afId("f", 21)]: "x" } }]),
      },
    );
    const { client, spaces, report } = await intoSpaces(files);
    expect(await outline(client, spaces[0]?.id ?? "")).toEqual(["Data", "  A", "Data again", "More data", "  B"]);
    const titles = await pagesByTitle(client);
    expect(await contentOf(client, titles.get("Data again") ?? "")).toMatchObject({ content: [{ type: "pageLink", attrs: { pageId: titles.get("Data") } }] });
    expect(report.warnings).toEqual([{ path: `collab_jsons/databases/${second}.json`, code: "unsupportedContent", detail: "linked view" }]);
    // Count is a number in one database and text in the other: one property, text.
    const schema = await client.propertySchemaGet(spaces[0]?.id ?? "");
    expect(schema.map((def) => `${def.name}:${def.kind}`)).toEqual(["Tags:multi_select", "Count:text"]);
  });

  it("reads a media field: local files are needed, remote ones dropped with a warning", async () => {
    const media = afId("f", 30);
    const files = archive(
      [{ id: S1, name: "Space", extra: SPACE, children: [view] }, { id: view, name: "Data", parent: S1, layout: 1, extra: { database_id: dbId } }],
      {},
      {
        [view]: database(
          dbId,
          view,
          [{ id: f.name, name: "Name", type: 0 }, { id: media, name: "Files", type: 14 }],
          [
            {
              id: afId("e", 1),
              cells: {
                [f.name]: "Row",
                [media]: JSON.stringify([{ url: `${view}/doc.pdf`, upload_type: 1, name: "doc.pdf" }, { url: "https://example.com/x.png", upload_type: 2, name: "x" }]),
              },
            },
          ],
        ),
      },
      { "local_files/doc.pdf": new Uint8Array([37, 80, 68, 70]) },
    );
    const ctx = importing(files);
    const { importId, plan } = await planSpaces(ctx, files);
    expect(plan.neededFiles).toEqual(["local_files/doc.pdf"]);
    const info = await ctx.client.attachmentUpload(plan.fileSpaces?.[0] ?? "", "doc.pdf", "application/pdf", [new Uint8Array([37, 80, 68, 70])]);
    await ctx.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: "local_files/doc.pdf", attachmentId: info.attachmentId } });
    const report = await commitAll(ctx, importId);
    expect(report.warnings).toEqual([{ path: `collab_jsons/databases/${view}.json`, code: "unsupportedContent", detail: "Files" }]);
    const spaces = (await ctx.client.spaceList()).slice(1);
    const schema = await ctx.client.propertySchemaGet(spaces[0]?.id ?? "");
    const row = (await pagesByTitle(ctx.client)).get("Row") ?? "";
    expect(await ctx.client.pagePropertiesGet(row)).toEqual([[schema[1]?.id, { kind: "files", value: [info.attachmentId] }]]);
  });
});

// ----- the shell's reading of an archive and the other destination -------------------------------------

describe("the shell", () => {
  const demo = (): Record<string, string | Uint8Array> => demoImportSources()["AppFlowy backup.zip"]?.files ?? {};

  it("reads the .json files of an AppFlowy backup as text, skips collab_objects/ silently, and strips a wrapping folder", async () => {
    const wrapped = Object.fromEntries(Object.entries(demo()).map(([path, content]) => [`AppFlowy Backup/${path}`, content]));
    wrapped["AppFlowy Backup/collab_objects/another.bin"] = new Uint8Array([9]);
    wrapped["AppFlowy Backup/collab_objects/also.json"] = "{}";
    const { report, spaces } = await intoSpaces(wrapped);
    expect(spaces.map((space) => space.name)).toEqual(["Personal", "Work"]);
    expect(report).toMatchObject({ pages: 9, attachments: 1 });
    // Nothing is reported for the skipped files, and the warnings name the files as the shell gave them.
    expect(report.warnings).toEqual([
      expect.objectContaining({ path: "AppFlowy Backup/collab_jsons/documents/b0000000-0000-4000-8000-000000000004.json", code: "remoteImage" }),
    ]);
  });

  it("leaves the .json files of any other source as files the importer does not read", async () => {
    const { report } = await intoSpaces({ "Note.md": "# Note\n", "data.json": "{}" });
    // A text file the importer reads is a page; a .json is a binary path nobody needs.
    expect(report.pages).toBe(1);
    expect(report.warnings).toBeUndefined();
  });

  it("picks the AppFlowy backup for new spaces and the first zip otherwise, and a cancelled dialog is a cancelled import", async () => {
    const sources = demoImportSources();
    const ctx = importing({}, { importSources: sources });
    const home = (await ctx.client.spaceList())[0]?.id ?? "";
    const plain = await ctx.client.importRun({ spaceId: home, source: "zip" });
    expect(plain.spaceIds).toBeUndefined();
    expect(await outline(ctx.client, home)).toContain("Notion export");
    const spaces = await ctx.client.importRun({ destination: "newSpaces", spaceId: home, source: "zip" });
    expect(spaces.spaceIds).toHaveLength(2);
    ctx.transport.setCancelDialogs(true);
    const cancelled = await failure(ctx.client.importRun({ destination: "newSpaces", spaceId: home, source: "zip" }));
    expect(cancelled.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "path" } });
  });

  it("imports the backup into a page with one container page per space, files into that page's space and remote files listed", async () => {
    const ctx = importing(demo());
    const home = (await ctx.client.spaceList())[0]?.id ?? "";
    const existing = (await pagesByTitle(ctx.client)).get("Existing") ?? "";
    const { texts, binary } = split(demo());
    const begun = await ctx.transport.dispatch({ type: "ImportBegin", data: { spaceId: home, parentId: existing, sourceName: "AppFlowy backup" } });
    const importId = begun.type === "ImportStarted" ? begun.data : 0;
    for (const [path, body] of texts) {
      await ctx.transport.dispatch({ type: "ImportAddFile", data: { importId, path, text: body } });
    }
    const planned = await ctx.transport.dispatch({ type: "ImportPlan", data: { importId, binaryPaths: binary } });
    const plan = planned.type === "ImportPlanned" ? planned.data : ({ pages: 0, neededFiles: [] } as ImportPlanInfo);
    // The space roots are pages: 11 in all; no spaces, no file spaces; the remote file is listed with the session's space.
    expect(plan.pages).toBe(11);
    expect(plan.spaces).toBeUndefined();
    expect(plan.fileSpaces).toBeUndefined();
    expect(plan.remoteFiles).toEqual([expect.objectContaining({ key: "remote/0", spaceId: home })]);
    const report = await commitAll(ctx, importId);
    expect(report.spaceIds).toBeUndefined();
    expect(report.rootPageIds).toHaveLength(2);
    expect(await outline(ctx.client, home)).toEqual([
      "Existing",
      "  Personal",
      "    Welcome",
      "    Projects",
      "      Roadmap",
      "      Tasks board",
      "        Write the report",
      "        Review the draft",
      "        Send it",
      "  Work",
      "    Meeting notes",
      "    Reading list",
    ]);
    expect((await ctx.client.spaceList()).map((space) => space.name)).toEqual(["Home"]);
  });
});

describe("recognition", () => {
  it("reads only a source with a metadata.json and a relation_map.json that has views as an AppFlowy backup", async () => {
    const page = afId("b", 1);
    const notBackup = (files: Record<string, string | Uint8Array>) => ({ ...files, "Note.md": "# Note\n" });
    const full = archive([{ id: S1, name: "Space", extra: SPACE, children: [page] }, { id: page, name: "Page", parent: S1 }]);
    const { "metadata.json": _metadata, ...withoutMetadata } = full;
    void _metadata;
    // Without metadata.json, or with a relation map without views, the .json files are binary paths and the Markdown file is the page.
    for (const files of [withoutMetadata, { ...full, "relation_map.json": JSON.stringify({ workspace_id: WORKSPACE }) }]) {
      const { report, spaces } = await intoSpaces(notBackup(files));
      expect(spaces.map((space) => space.name)).toEqual(["Backup"]);
      expect(report.pages).toBe(1);
    }
    const { spaces } = await intoSpaces(notBackup(full));
    expect(spaces.map((space) => space.name)).toEqual(["Space"]);
  });

  it("moves an imported board to another space with its view, its values and its select options", async () => {
    const { client, spaces } = await intoSpaces(demoImportSources()["AppFlowy backup.zip"]?.files ?? {});
    const titles = await pagesByTitle(client);
    const [personal, work] = spaces;
    const summary = await client.pageMoveToSpace(titles.get("Tasks board") ?? "", work?.id ?? "", null, null);
    expect(summary).toMatchObject({ propertiesCreated: 2, valuesDropped: 0, relationLinksRemoved: 0, attachmentsMoved: 0 });
    expect(summary.pageIds).toHaveLength(4);
    expect(await outline(client, work?.id ?? "")).toEqual(["Meeting notes", "Reading list", "Tasks board", "  Write the report", "  Review the draft", "  Send it"]);
    const schema = new Map((await client.propertySchemaGet(work?.id ?? "")).map((def) => [def.name, def]));
    expect([...schema.keys()]).toEqual(["Tags", "Notes", "Status"]);
    const block = ((await contentOf(client, titles.get("Tasks board") ?? "")) as { content: { attrs: { groupBy: string; properties: string[] } }[] }).content[0]?.attrs;
    expect(block).toMatchObject({ groupBy: schema.get("Status")?.id, properties: [schema.get("Notes")?.id, schema.get("Status")?.id] });
    const done = schema.get("Status")?.options.find((option) => option.name === "Done")?.id ?? "";
    expect(await client.pagesByProperty(work?.id ?? "", schema.get("Status")?.id ?? "", { type: "ContainsOption", data: done })).toEqual([titles.get("Send it")]);
    // The rest of Personal is where it was.
    expect(await outline(client, personal?.id ?? "")).toEqual(["Welcome", "Projects", "  Roadmap"]);
  });
});
