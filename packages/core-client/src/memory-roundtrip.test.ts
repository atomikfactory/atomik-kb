// What an export brings back: the in-memory export of a space imported as a `.zip` into another
// space (ADR-0036, 7). The fidelity is that of the mirror (clipboard-flavour Markdown, no
// annotations): titles, the tree and its order, icons, text and inline formatting, links between
// pages, images and files, the schema and every value come back; block ids are new, and what
// Markdown cannot say (callout colours, column layout, image layout) is lost, as documented.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { demoImagePng } from "./demo-image";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyValue } from "./generated/PropertyValue";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import { readMemoryArchive } from "./memory-export";
import type { MemoryImportSource } from "./memory-import";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const PHOTO = "aaaaaaaa-0000-4000-8000-000000000001";
const REPORT = "aaaaaaaa-0000-4000-8000-000000000002";

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

const text = (value: string, ...marks: JsonValue[]): JsonValue => (marks.length === 0 ? { type: "text", text: value } : { type: "text", text: value, marks });
const para = (n: number, ...content: JsonValue[]): JsonValue => ({ type: "paragraph", attrs: { id: blockId(n) }, content });

// Ids: the device (1), the workspace (2), "Home" (3), Projects (4), Beta (5), Alpha (6), Archive (7), "Copy" (8).
const HOME = id(3);
const COPY = id(8);
const ALPHA = id(6);

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    tagOptions: [{ name: "work", color: "blue" }, { name: "review", color: "red" }],
    properties: [
      { name: "Status", kind: "status" },
      { name: "Estimate", kind: "number" },
      { name: "Done", kind: "checkbox" },
      { name: "Due", kind: "date" },
      { name: "Labels", kind: "multi_select", options: [{ name: "a", color: "green" }, { name: "b", color: "pink" }] },
      { name: "Kind", kind: "select", options: [{ name: "Big", color: "orange" }] },
      { name: "Notes", kind: "text" },
      { name: "Link", kind: "url" },
      { name: "Contact", kind: "email" },
      { name: "Related", kind: "relation", twoWay: true, backName: "Back" },
      { name: "Total", kind: "rollup", rollup: { relation: "Related", target: "Estimate", function: "sum" } },
      { name: "Place", kind: "place" },
      { name: "Checks", kind: "checklist" },
      { name: "Attachments", kind: "files" },
    ],
    pages: [
      {
        title: "Projects",
        icon: "📁",
        tags: ["work", "review"],
        properties: {
          Status: "In progress",
          Estimate: 12.5,
          Done: false,
          Due: { date: "2026-10-07", time: "14:30" },
          Labels: ["a", "b"],
          Kind: "Big",
          Notes: "Line with \"quotes\", commas, and a #hash",
          Link: "https://example.com/path?q=1",
          Contact: "ann@example.com",
          Place: { name: "Paris", lat: 48.8566, lon: 2.3522 },
          Checks: [{ text: "Buy milk", done: true }, { text: "Call Ann" }],
          Attachments: [REPORT],
        },
        content: {
          type: "doc",
          content: [
            { type: "heading", attrs: { id: blockId(1), level: 2 }, content: [text("Plan")] },
            para(2, text("Some "), text("bold", { type: "bold" }), text(" and "), text("italic", { type: "italic" }), text(", a "), text("link", { type: "link", attrs: { href: "https://example.com" } }), text(" and a reference to "), { type: "pageRef", attrs: { id: blockId(3), pageId: ALPHA } }, text(".")),
            {
              type: "bulletList",
              attrs: { id: blockId(4) },
              content: [
                { type: "listItem", attrs: { id: blockId(5) }, content: [para(6, text("one"))] },
                { type: "listItem", attrs: { id: blockId(7) }, content: [para(8, text("two"))] },
              ],
            },
            { type: "codeBlock", attrs: { id: blockId(9), language: "rust" }, content: [text("fn main() {}")] },
            { type: "blockquote", attrs: { id: blockId(10) }, content: [para(11, text("quoted"))] },
            { type: "image", attrs: { id: blockId(12), attachmentId: PHOTO, alt: "A photo", caption: "Cap" } },
            { type: "file", attrs: { id: blockId(13), attachmentId: REPORT } },
            { type: "horizontalRule", attrs: { id: blockId(14) } },
          ],
        },
        // Beta before Alpha, and Archive after Projects: not the natural order of their names.
        children: [
          { title: "Beta", icon: "icon:book:blue", properties: { Estimate: 4 } },
          { title: "Alpha", properties: { Status: "Done", Estimate: 3.5, Related: ["Beta"] }, content: { type: "doc", content: [para(15, text("Alpha text"))] } },
        ],
      },
      { title: "Archive", content: { type: "doc", content: [para(16, text("Archived"))] } },
    ],
  },
  { name: "Copy" },
];

function setup() {
  let counter = 0;
  const sources: Record<string, MemoryImportSource> = {};
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0,
    workspaceName: "Workspace",
    spaces: SPACES,
    attachments: [
      { attachmentId: PHOTO, name: "photo.png", mime: "image/png", bytes: demoImagePng() },
      { attachmentId: REPORT, name: "report.pdf", mime: "application/pdf", bytes: new Uint8Array([37, 80, 68, 70, 1, 2, 3]) },
    ],
    importSources: sources,
  });
  const client = new CoreClient(transport);
  return { transport, client, sources };
}

type PageSummary = { title: string; icon: string | undefined; path: string };

async function pages(client: CoreClient, spaceId: string): Promise<(PageSummary & { id: string })[]> {
  const found: (PageSummary & { id: string })[] = [];
  const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"], path: string): void => {
    for (const node of nodes) {
      const here = path === "" ? node.page.title : `${path}/${node.page.title}`;
      found.push({ id: node.page.id, title: node.page.title, icon: node.page.icon, path: here });
      visit(node.children, here);
    }
  };
  visit((await client.treeGet(spaceId)).roots, "");
  return found;
}

/** A page's values by property name, with options, pages and files as names and titles. */
async function valuesByName(client: CoreClient, spaceId: string, pageId: string): Promise<Record<string, unknown>> {
  const schema = await client.propertySchemaGet(spaceId);
  const titles = new Map((await pages(client, spaceId)).map((page) => [page.id, page.title]));
  const names = new Map((await client.attachmentList(spaceId)).map((attachment) => [attachment.attachmentId, attachment.name]));
  const optionName = (def: PropertyDef | undefined, optionId: string): string => def?.options.find((option) => option.id === optionId)?.name ?? optionId;
  const readable = (def: PropertyDef | undefined, value: PropertyValue): unknown => {
    switch (value.kind) {
      case "select":
      case "status":
        return optionName(def, value.value);
      case "multi_select":
        return value.value.map((optionId) => optionName(def, optionId));
      case "relation":
        return value.value.map((target) => titles.get(target) ?? target);
      case "files":
        return value.value.map((attachmentId) => names.get(attachmentId) ?? attachmentId);
      case "checklist":
        return value.value.map((item) => ({ text: item.text, done: item.done }));
      case "rollup":
        return value.value;
      default:
        return value.value;
    }
  };
  const result: Record<string, unknown> = {};
  for (const [propertyId, value] of await client.pagePropertiesGet(pageId)) {
    const def = schema.find((candidate) => candidate.id === propertyId);
    if (def !== undefined && value.kind !== "created_time" && value.kind !== "last_edited_time" && value.kind !== "created_by" && value.kind !== "last_edited_by") {
      result[def.name] = readable(def, value);
    }
  }
  return result;
}

/** A page as Markdown with the attachment ids made alike (they are new in the copy). */
async function markdownOf(client: CoreClient, pageId: string): Promise<string> {
  return (await client.pageExportMarkdown(pageId)).replace(/attachment:\/\/[0-9a-f-]{36}/gu, "attachment://ID");
}

describe("export then import", () => {
  async function roundTrip() {
    const fixture = setup();
    const exported = await fixture.client.exportRun({ spaceId: HOME, includeChildren: true, format: "markdown" });
    const archive = readMemoryArchive(fixture.transport.files.get(exported.path) ?? new Uint8Array());
    if (archive === undefined) {
      throw new Error("not an archive");
    }
    fixture.sources["Home.zip"] = { kind: "zip", files: archive };
    const report = await fixture.client.importRun({ spaceId: COPY, source: "zip" });
    return { ...fixture, report, archive };
  }

  it("brings back the tree, its order (the sidecar's, not the alphabet's) and the titles and icons", async () => {
    const { client, report } = await roundTrip();
    const original = await pages(client, HOME);
    const copy = await pages(client, COPY);
    // The archive has several top-level pages and the zip a name: they sit under a container.
    expect(copy[0]).toMatchObject({ title: "Home", path: "Home" });
    expect(copy.slice(1).map(({ title, icon, path }) => ({ title, icon, path: path.replace(/^Home\//u, "") }))).toEqual(
      original.map(({ title, icon, path }) => ({ title, icon, path })),
    );
    expect(report.pages).toBe(original.length + 1);
    expect(report.warnings).toBeUndefined();
    expect(report.rootPageIds).toEqual([copy[0]?.id]);
  });

  it("brings back the text, inline formatting, links between pages, images and files", async () => {
    const { client } = await roundTrip();
    const original = await pages(client, HOME);
    const copy = await pages(client, COPY);
    for (const page of original) {
      const twin = copy.find((candidate) => candidate.path === `Home/${page.path}`);
      expect(twin, page.path).toBeDefined();
      expect(await markdownOf(client, twin?.id ?? ""), page.path).toBe(await markdownOf(client, page.id));
    }
    // The links are real references to the copies, not text.
    const projects = copy.find((page) => page.path === "Home/Projects");
    const alpha = copy.find((page) => page.path === "Home/Projects/Alpha");
    const backlinks = await client.backlinksGet(alpha?.id ?? "");
    expect(backlinks.map((link) => link.pageId)).toEqual([projects?.id]);
  });

  it("brings back the attachments with their names, types and bytes", async () => {
    const { client } = await roundTrip();
    const attachments = (await client.attachmentList(COPY)).sort((a, b) => (a.name < b.name ? -1 : 1));
    expect(attachments.map((attachment) => [attachment.name, attachment.mime, attachment.size])).toEqual([
      ["photo.png", "image/png", demoImagePng().length],
      ["report.pdf", "application/pdf", 7],
    ]);
    expect(await client.attachmentRead(attachments[0]?.attachmentId ?? "")).toEqual(demoImagePng());
    expect(await client.attachmentRead(attachments[1]?.attachmentId ?? "")).toEqual(new Uint8Array([37, 80, 68, 70, 1, 2, 3]));
    // Every one is used by a page: none is waiting for the collector.
    expect(attachments.every((attachment) => attachment.references > 0)).toBe(true);
  });

  it("brings back the schema and every value, matched by name", async () => {
    const { client } = await roundTrip();
    const schemaOf = async (spaceId: string) =>
      (await client.propertySchemaGet(spaceId)).map((def) => ({
        name: def.name,
        kind: def.kind,
        options: def.options.map((option) => ({ name: option.name, color: option.color, group: option.group })),
      }));
    expect(await schemaOf(COPY)).toEqual(await schemaOf(HOME));
    const original = await pages(client, HOME);
    const copy = await pages(client, COPY);
    for (const page of original) {
      const twin = copy.find((candidate) => candidate.path === `Home/${page.path}`);
      expect(await valuesByName(client, COPY, twin?.id ?? ""), page.path).toEqual(await valuesByName(client, HOME, page.id));
    }
    const projects = copy.find((page) => page.path === "Home/Projects");
    expect(await valuesByName(client, COPY, projects?.id ?? "")).toMatchObject({
      Notes: 'Line with "quotes", commas, and a #hash',
      Place: { name: "Paris", lat: 48.8566, lon: 2.3522 },
      Checks: [{ text: "Buy milk", done: true }, { text: "Call Ann", done: false }],
      Attachments: ["report.pdf"],
      Due: { date: "2026-10-07", time: "14:30" },
    });
  });

  it("brings back two-way relations and rollups", async () => {
    const { client } = await roundTrip();
    const copy = await pages(client, COPY);
    const beta = copy.find((page) => page.path === "Home/Projects/Beta");
    const back = await valuesByName(client, COPY, beta?.id ?? "");
    expect(back.Back).toEqual(["Alpha"]);
    const alpha = copy.find((page) => page.path === "Home/Projects/Alpha");
    expect((await valuesByName(client, COPY, alpha?.id ?? "")).Total).toMatchObject({ function: "sum", result: { type: "number", data: 4 } });
  });
});
