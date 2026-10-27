// The canned import sources of the dev page: each imports cleanly through the in-memory core.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { demoImagePng } from "./demo-image";
import { demoImportSources } from "./demo-import";
import { MemoryTransport } from "./memory";
import type { MemoryImportSource } from "./memory-import";

async function setup() {
  const transport = new MemoryTransport({
    workspaceName: "Demo",
    spaces: [{ name: "Home" }],
    importSources: demoImportSources(),
  });
  const client = new CoreClient(transport);
  const space = (await client.spaceList())[0]?.id ?? "";
  return { client, transport, space };
}

async function outline(client: CoreClient, space: string): Promise<string[]> {
  const lines: string[] = [];
  const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"], depth: number): void => {
    for (const node of nodes) {
      lines.push(`${"  ".repeat(depth)}${node.page.title}`);
      visit(node.children, depth + 1);
    }
  };
  visit((await client.treeGet(space)).roots, 0);
  return lines;
}

describe("demoImportSources", () => {
  it("offers a zip, a folder, loose files and an AppFlowy backup, a fresh copy every time", () => {
    const sources = demoImportSources();
    expect(Object.entries(sources).map(([name, source]) => [name, source.kind])).toEqual([
      ["Notion export.zip", "zip"],
      ["Markdown notes", "folder"],
      ["Loose files", "files"],
      ["AppFlowy backup.zip", "zip"],
    ]);
    delete sources["Loose files"];
    expect(Object.keys(demoImportSources())).toContain("Loose files");
    expect(demoImportSources()["Notion export.zip"]?.files["Getting Started 0123456789abcdef0123456789abcdef/picture.png"]).toEqual(demoImagePng());
  });

  it("imports the Notion export: a page with its sub-page and picture, a database of two rows, no warning", async () => {
    const { client, space } = await setup();
    const report = await client.importRun({ spaceId: space, source: "zip" });
    expect(await outline(client, space)).toEqual([
      "Notion export",
      "  Getting Started",
      "    Notes",
      "  Task List",
      "    Write the report",
      "    Send it",
    ]);
    expect(report).toMatchObject({ pages: 6, attachments: 1, propertiesCreated: 3, moreWarnings: 0 });
    expect(report.warnings).toBeUndefined();
    const schema = await client.propertySchemaGet(space);
    expect(schema.filter((def) => def.name !== "Tags").map((def) => `${def.name}:${def.kind}`)).toEqual(["Status:select", "Done:checkbox", "Due:date"]);
    const attachments = await client.attachmentList(space);
    expect(attachments.map((attachment) => attachment.name)).toEqual(["picture.png"]);
    expect(attachments[0]?.references).toBe(1);
  });

  it("imports the Markdown folder: front matter as properties and tags, wiki links as references", async () => {
    const { client, space } = await setup();
    const report = await client.importRun({ spaceId: space, source: "folder" });
    expect(await outline(client, space)).toEqual(["Markdown notes", "  Ideas", "  Projects", "    Plan", "  Welcome"]);
    expect(report.warnings).toBeUndefined();
    const welcome = (await client.treeGet(space)).roots[0]?.children.find((node) => node.page.title === "Welcome")?.page.id ?? "";
    const ideas = (await client.treeGet(space)).roots[0]?.children.find((node) => node.page.title === "Ideas")?.page.id ?? "";
    expect((await client.backlinksGet(ideas)).map((link) => link.title)).toEqual(["Welcome"]);
    const schema = await client.propertySchemaGet(space);
    expect(schema.map((def) => `${def.name}:${def.kind}`)).toEqual(["Tags:multi_select", "status:text", "rating:number"]);
    expect((await client.pagePropertiesGet(welcome)).length).toBe(3);
  });

  it("imports the loose files as two pages", async () => {
    const { client, space } = await setup();
    const report = await client.importRun({ spaceId: space, source: "files" });
    expect(await outline(client, space)).toEqual(["Meeting notes", "Shopping list"]);
    expect(report.pages).toBe(2);
    expect(report.warnings).toBeUndefined();
  });

  it("imports the AppFlowy backup into new spaces: two spaces, nine pages, one picture, the remote image left as a link", async () => {
    const { client, space } = await setup();
    const report = await client.importRun({ destination: "newSpaces", spaceId: space, source: "zip" });
    expect(report).toMatchObject({ pages: 9, attachments: 1, propertiesCreated: 2, moreWarnings: 0 });
    expect(report.warnings).toEqual([
      { path: "collab_jsons/documents/b0000000-0000-4000-8000-000000000004.json", code: "remoteImage", detail: "https://files.appflowy.example/workspace/diagram.png" },
    ]);
    const spaces = await client.spaceList();
    expect(spaces.map((entry) => [entry.name, entry.icon, entry.pageCount])).toEqual([
      ["Home", undefined, 0],
      ["Personal", "🏠", 7],
      ["Work", "💼", 2],
    ]);
    expect(report.spaceIds).toEqual([spaces[1]?.id, spaces[2]?.id]);
    expect(await outline(client, spaces[1]?.id ?? "")).toEqual([
      "Welcome",
      "Projects",
      "  Roadmap",
      "  Tasks board",
      "    Write the report",
      "    Review the draft",
      "    Send it",
    ]);
    expect(await outline(client, spaces[2]?.id ?? "")).toEqual(["Meeting notes", "Reading list"]);
    // The picture went into the space of its page, not the first space.
    expect((await client.attachmentList(spaces[2]?.id ?? "")).map((info) => info.name)).toEqual(["mK3qWf0dYV4j2u9c8gH7nQe1RZpTbXsLhA6oiNyD5Ew.png"]);
    expect(await client.attachmentList(space)).toEqual([]);
  });

  it("imports the AppFlowy backup into a page as container pages, one per space", async () => {
    const { client, space } = await setup();
    // The first zip is the Notion export; a source that is only the backup picks it.
    const transport = new MemoryTransport({
      workspaceName: "Demo",
      spaces: [{ name: "Home" }],
      importSources: { "AppFlowy backup.zip": demoImportSources()["AppFlowy backup.zip"] as MemoryImportSource },
    });
    const own = new CoreClient(transport);
    const home = (await own.spaceList())[0]?.id ?? "";
    void client;
    void space;
    const report = await own.importRun({ spaceId: home, source: "zip" });
    expect(report.spaceIds).toBeUndefined();
    expect(await outline(own, home)).toEqual([
      "Personal",
      "  Welcome",
      "  Projects",
      "    Roadmap",
      "    Tasks board",
      "      Write the report",
      "      Review the draft",
      "      Send it",
      "Work",
      "  Meeting notes",
      "  Reading list",
    ]);
    expect(report).toMatchObject({ pages: 11, attachments: 1, propertiesCreated: 2 });
    expect(report.rootPageIds).toHaveLength(2);
  });
});
