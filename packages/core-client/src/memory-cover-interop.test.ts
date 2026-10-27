// Covers in the import and export of `MemoryTransport` (ADR-0040): front matter `cover` and `cover_y`
// written by the export (Markdown and HTML) and read back by the import (named, or a picture of the source),
// what the plan lists and warns about, and the AppFlowy reader's mapping of a view's `cover`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { afId, appFlowyDocument } from "./demo-import";
import { demoImagePng } from "./demo-image";
import type { Event } from "./generated/Event";
import type { ExportEntry } from "./generated/ExportEntry";
import type { ImportReport } from "./generated/ImportReport";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { frontMatter } from "./memory-export-text";
import { paletteBackground } from "./memory-import-appflowy";
import { namedCover, planImport } from "./memory-import-plan";
import type { ImportPlan, PlannedPage } from "./memory-import-plan";

const T0 = 1_700_000_000_000;
const BANNER = "aaaaaaaa-0000-4000-8000-000000000001";

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0,
    workspaceName: "Workspace",
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}

type Fixture = ReturnType<typeof setup>;

async function homeOf(client: CoreClient): Promise<string> {
  return (await client.spaceList())[0]?.id ?? "";
}

// ----- export ----------------------------------------------------------------------------------

async function exportEntries(fixture: Fixture, format: "markdown" | "html"): Promise<ExportEntry[]> {
  const spaceId = await homeOf(fixture.client);
  const begun = await fixture.transport.dispatch({ type: "ExportBegin", data: { spaceId, includeChildren: true, format } });
  if (begun.type !== "ExportPlanned") {
    throw new Error("unexpected response");
  }
  const entries: ExportEntry[] = [];
  for (;;) {
    const batch = await fixture.transport.dispatch({ type: "ExportNext", data: { exportId: begun.data.exportId, maxEntries: 500 } });
    if (batch.type !== "ExportBatch") {
      throw new Error("unexpected response");
    }
    entries.push(...batch.data.entries);
    if (batch.data.done) {
      return entries;
    }
  }
}

function textOf(entries: readonly ExportEntry[], suffix: string): string {
  const entry = entries.find((candidate) => candidate.path.endsWith(suffix));
  if (entry?.content.type !== "Text") {
    throw new Error(`no text entry ${suffix} in ${entries.map((candidate) => candidate.path).join(", ")}`);
  }
  return entry.content.data.text;
}

const COVERED = {
  attachments: [{ attachmentId: BANNER, name: "banner.png", mime: "image/png", bytes: demoImagePng() }],
  spaces: [
    {
      name: "Home",
      pages: [
        { title: "Named", cover: "color:purple", coverY: 20 },
        { title: "Gradient", cover: "gradient:ocean" },
        { title: "Picture", cover: `attachment:${BANNER}`, coverY: 90 },
        { title: "Plain" },
      ],
    },
  ],
} satisfies MemoryTransportOptions;

describe("the export writes covers", () => {
  it("as front matter: a named cover as it is, a picture as the path of its file, the position unless it is 50", async () => {
    const entries = await exportEntries(setup(COVERED), "markdown");
    expect(textOf(entries, "Named.md")).toContain('cover: "color:purple"\ncover_y: 20\ncreated:');
    const gradient = textOf(entries, "Gradient.md");
    expect(gradient).toContain('cover: "gradient:ocean"\ncreated:');
    expect(gradient).not.toContain("cover_y");
    const picture = textOf(entries, "Picture.md");
    expect(picture).toMatch(/cover: "[^"]*banner\.png"\ncover_y: 90\n/u);
    expect(textOf(entries, "Plain.md")).not.toContain("cover");
    // The picture is a file of the archive.
    expect(entries.some((entry) => entry.path.endsWith("banner.png"))).toBe(true);
  });

  it("as a banner in HTML: an image for a picture, a block for a colour or a gradient", async () => {
    const entries = await exportEntries(setup(COVERED), "html");
    expect(textOf(entries, "Named.html")).toContain('<div class="page-cover" data-color="purple"></div>');
    expect(textOf(entries, "Gradient.html")).toContain('<div class="page-cover" data-gradient="ocean"></div>');
    expect(textOf(entries, "Picture.html")).toMatch(/<img class="page-cover" src="[^"]*banner\.png" alt="" style="object-position:50% 90%">/u);
    expect(textOf(entries, "Plain.html")).not.toContain('class="page-cover"');
  });

  it("leaves a picture out with its position when the file is not in the archive", () => {
    const text = frontMatter({
      title: "T",
      icon: undefined,
      cover: undefined,
      coverY: 30,
      createdAt: T0,
      updatedAt: T0,
      values: [],
    });
    expect(text).not.toContain("cover");
    expect(
      frontMatter({ title: "T", icon: undefined, cover: "../Notes/banner.png", coverY: 50, createdAt: T0, updatedAt: T0, values: [] }),
    ).toContain('cover: "../Notes/banner.png"\ncreated:');
  });
});

// ----- the plan --------------------------------------------------------------------------------

function plan(files: Record<string, string>, binary: string[] = []): ImportPlan {
  return planImport({ files: new Map(Object.entries(files)), binaryPaths: binary, sourceName: undefined, maxPages: 10_000 });
}

function page(result: ImportPlan, title: string): PlannedPage {
  const found = result.pages.find((candidate) => candidate.title === title);
  if (found === undefined) {
    throw new Error(`no page titled ${title}`);
  }
  return found;
}

const withFront = (title: string, front: string): string => `---\ntitle: ${title}\n${front}---\n${title} text\n`;

describe("the plan reads covers from front matter", () => {
  it("a named cover and a position", () => {
    const result = plan({
      "Named.md": withFront("Named", 'cover: "color:green"\ncover_y: 10\n'),
      "Gradient.md": withFront("Gradient", "cover: gradient:sunset\n"),
    });
    expect(page(result, "Named")).toMatchObject({ cover: { kind: "named", value: "color:green" }, coverY: 10 });
    expect(page(result, "Gradient").cover).toEqual({ kind: "named", value: "gradient:sunset" });
    expect(page(result, "Gradient").coverY).toBeUndefined();
    expect(result.neededFiles).toEqual([]);
    expect(result.pages.every((entry) => entry.values.length === 0)).toBe(true);
  });

  it("a picture of the source by a path relative to the page, listed once as a needed file", () => {
    const result = plan(
      {
        "notes/Picture.md": withFront("Picture", "cover: img/banner.png\ncover_y: 75\n"),
        "notes/Same.md": withFront("Same", "cover: ./img/banner.png\n"),
        "notes/Up.md": withFront("Up", 'cover: "../shared/wide.jpg"\n'),
      },
      ["notes/img/banner.png", "shared/wide.jpg"],
    );
    expect(page(result, "Picture")).toMatchObject({ cover: { kind: "file", path: "notes/img/banner.png" }, coverY: 75 });
    expect(page(result, "Same").cover).toEqual({ kind: "file", path: "notes/img/banner.png" });
    expect(page(result, "Up").cover).toEqual({ kind: "file", path: "shared/wide.jpg" });
    expect(result.neededFiles.sort()).toEqual(["notes/img/banner.png", "shared/wide.jpg"]);
  });

  it("anything else stays a property, and a position out of range is dropped", () => {
    const result = plan({
      "Link.md": withFront("Link", "cover: https://example.com/a.png\n"),
      "Missing.md": withFront("Missing", "cover: img/gone.png\n"),
      "Bad.md": withFront("Bad", "cover: color:teal\n"),
      "Far.md": withFront("Far", "cover: color:red\ncover_y: 101\n"),
      "Odd.md": withFront("Odd", "cover: color:red\ncover_y: 30.5\n"),
    });
    for (const title of ["Link", "Missing", "Bad"]) {
      expect(page(result, title).cover, title).toBeUndefined();
      expect(page(result, title).values.length, title).toBe(1);
    }
    for (const title of ["Far", "Odd"]) {
      expect(page(result, title).cover, title).toEqual({ kind: "named", value: "color:red" });
      expect(page(result, title).coverY, title).toBeUndefined();
      expect(page(result, title).values, title).toEqual([]);
    }
    expect(namedCover(" gradient:a-1 ")).toBe("gradient:a-1");
    expect(namedCover("gradient:A")).toBeUndefined();
    expect(namedCover(`gradient:${"x".repeat(33)}`)).toBeUndefined();
    expect(namedCover("color:Blue")).toBeUndefined();
  });
});

// ----- the commit ------------------------------------------------------------------------------

async function importAll(fixture: Fixture, files: Record<string, string>, binaries: Record<string, { bytes: Uint8Array; mime: string }>): Promise<ImportReport> {
  const spaceId = await homeOf(fixture.client);
  const begun = await fixture.transport.dispatch({ type: "ImportBegin", data: { spaceId, sourceName: "" } });
  if (begun.type !== "ImportStarted") {
    throw new Error("unexpected response");
  }
  const importId = begun.data;
  for (const [path, text] of Object.entries(files)) {
    await fixture.transport.dispatch({ type: "ImportAddFile", data: { importId, path, text } });
  }
  const planned = await fixture.transport.dispatch({ type: "ImportPlan", data: { importId, binaryPaths: Object.keys(binaries) } });
  if (planned.type !== "ImportPlanned") {
    throw new Error("unexpected response");
  }
  for (const path of planned.data.neededFiles) {
    const file = binaries[path];
    if (file === undefined) {
      throw new Error(`the plan needs ${path}`);
    }
    const info = await fixture.client.attachmentUpload(spaceId, path.split("/").at(-1) ?? path, file.mime, [file.bytes]);
    await fixture.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path, attachmentId: info.attachmentId } });
  }
  for (;;) {
    const progress = await fixture.transport.dispatch({ type: "ImportCommit", data: { importId, maxPages: 50 } });
    if (progress.type !== "ImportProgress") {
      throw new Error("unexpected response");
    }
    if (progress.data.report !== undefined) {
      return progress.data.report;
    }
  }
}

async function coversOf(client: CoreClient): Promise<Record<string, [string | undefined, number | undefined]>> {
  const out: Record<string, [string | undefined, number | undefined]> = {};
  const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): void => {
    for (const node of nodes) {
      out[node.page.title] = [node.page.cover, node.page.coverY];
      visit(node.children);
    }
  };
  visit((await client.treeGet(await homeOf(client))).roots);
  return out;
}

describe("the import makes covers", () => {
  const PNG = { bytes: demoImagePng(), mime: "image/png" };

  it("named covers as they are and a picture as an uploaded image, with the position", async () => {
    const fixture = setup({ spaces: [{ name: "Home" }] });
    const report = await importAll(
      fixture,
      {
        "notes/Named.md": withFront("Named", 'cover: "color:green"\ncover_y: 10\n'),
        "notes/Gradient.md": withFront("Gradient", "cover: gradient:sunset\n"),
        "notes/Picture.md": withFront("Picture", "cover: img/banner.png\ncover_y: 75\n"),
        "notes/Plain.md": withFront("Plain", ""),
      },
      { "notes/img/banner.png": PNG },
    );
    expect(report.pages).toBe(4);
    expect(report.attachments).toBe(1);
    expect(report.warnings ?? []).toEqual([]);
    const covers = await coversOf(fixture.client);
    expect(covers.Named).toEqual(["color:green", 10]);
    expect(covers.Gradient).toEqual(["gradient:sunset", undefined]);
    expect(covers.Plain).toEqual([undefined, undefined]);
    const [picture, y] = covers.Picture ?? [];
    expect(y).toBe(75);
    const attachmentId = picture?.replace("attachment:", "") ?? "";
    expect(await fixture.client.attachmentGet(attachmentId)).toMatchObject({ name: "banner.png", references: 1 });
  });

  it("reports a picture that is not an image and drops the cover", async () => {
    const fixture = setup({ spaces: [{ name: "Home" }] });
    const report = await importAll(
      fixture,
      { "Doc.md": withFront("Doc", "cover: files/notes.png\n") },
      { "files/notes.png": { bytes: new Uint8Array([1, 2, 3]), mime: "application/octet-stream" } },
    );
    expect(report.warnings).toEqual([{ path: "Doc.md", code: "unsupportedContent", detail: "cover" }]);
    expect((await coversOf(fixture.client)).Doc).toEqual([undefined, undefined]);
  });
});

// ----- AppFlowy --------------------------------------------------------------------------------

const WORKSPACE = afId("a", 0);

function appFlowy(views: { id: string; name: string; cover?: unknown }[], binaries: Record<string, Uint8Array> = {}): ImportPlan {
  const map: Record<string, unknown> = {
    space: {
      view_id: "space",
      name: "Space",
      layout: 0,
      parent_id: WORKSPACE,
      children: views.map((view) => view.id),
      collab_object_id: "space",
      extra: JSON.stringify({ is_space: true }),
      icon: null,
    },
  };
  const files = new Map<string, string>([
    ["metadata.json", JSON.stringify({ export_info: { workspace_id: WORKSPACE, version: "1.0" } })],
    ["relation_map.json", ""],
  ]);
  for (const view of views) {
    map[view.id] = {
      view_id: view.id,
      name: view.name,
      layout: 0,
      parent_id: "space",
      children: [],
      collab_object_id: view.id,
      extra: JSON.stringify({ cover: view.cover ?? null }),
      icon: null,
    };
    files.set(`collab_jsons/documents/${view.id}.json`, appFlowyDocument(view.id, []));
  }
  files.set("relation_map.json", JSON.stringify({ workspace_id: WORKSPACE, views: map, workspace_database_meta: [] }));
  return planImport({ files, binaryPaths: Object.keys(binaries), sourceName: undefined, maxPages: 10_000 });
}

describe("the AppFlowy reader maps a view's cover", () => {
  it("a colour to the nearest palette colour, a file of the archive or a picture on a server to a file cover", () => {
    const result = appFlowy(
      [
        { id: "blue", name: "Blue", cover: { type: 1, value: "0xFF3B82F6" } },
        { id: "red", name: "Red", cover: { type: "CoverType.color", value: "#E5484D" } },
        { id: "green", name: "Green", cover: { type: "color", value: "0xff2e8b57" } },
        { id: "file", name: "File", cover: { type: 2, value: "abc123.png" } },
        { id: "remote", name: "Remote", cover: { type: "CoverType.file", value: "https://example.com/banner.jpg" } },
        { id: "remote2", name: "Remote again", cover: { type: 2, value: "https://example.com/banner.jpg" } },
        { id: "plain", name: "Plain" },
      ],
      { "local_files/abc123.png": demoImagePng() },
    );
    const cover = (title: string) => page(result, title).cover;
    expect(cover("Blue")).toEqual({ kind: "named", value: "color:blue" });
    expect(cover("Red")).toEqual({ kind: "named", value: "color:red" });
    expect(cover("Green")).toEqual({ kind: "named", value: "color:green" });
    expect(cover("File")).toEqual({ kind: "file", path: "local_files/abc123.png" });
    expect(cover("Remote")).toEqual({ kind: "file", path: "remote/0" });
    expect(cover("Remote again")).toEqual({ kind: "file", path: "remote/0" });
    expect(cover("Plain")).toBeUndefined();
    expect(result.neededFiles).toEqual(["local_files/abc123.png"]);
    expect(result.remoteFiles).toEqual([{ key: "remote/0", url: "https://example.com/banner.jpg", host: "example.com" }]);
    expect(result.warnings.filter((warning) => warning.detail === "cover")).toEqual([]);
  });

  it("reports the covers that cannot be kept once and leaves no cover", () => {
    const result = appFlowy([
      { id: "a", name: "Built in", cover: { type: 3, value: "assets/images/cover_1.jpg" } },
      { id: "b", name: "Gradient", cover: { type: "gradient", value: "gradient_2" } },
      { id: "c", name: "Unknown", cover: { type: 9, value: "x" } },
      { id: "d", name: "White", cover: { type: 1, value: "0xFFFFFFFF" } },
      { id: "e", name: "Not in the archive", cover: { type: 2, value: "gone.png" } },
      { id: "f", name: "Plain http", cover: { type: 2, value: "http://example.com/a.png" } },
      { id: "g", name: "No value", cover: { type: 1 } },
    ]);
    for (const entry of result.pages) {
      expect(entry.cover, entry.title).toBeUndefined();
    }
    expect(result.warnings.filter((warning) => warning.detail === "cover")).toEqual([
      { path: "relation_map.json", code: "unsupportedContent", detail: "cover" },
    ]);
    expect(result.neededFiles).toEqual([]);
    expect(result.remoteFiles).toEqual([]);
  });

  it("maps a colour by hue like the Rust reader", () => {
    expect(paletteBackground("0xFF3B82F6")).toBe("blue");
    expect(paletteBackground("#E5484D")).toBe("red");
    expect(paletteBackground("0xff2e8b57")).toBe("green");
    expect(paletteBackground("0xFFF59E0B")).toBe("orange");
    expect(paletteBackground("0xFFEAB308")).toBe("yellow");
    expect(paletteBackground("0xFF8B5CF6")).toBe("purple");
    expect(paletteBackground("0xFFEC4899")).toBe("pink");
    expect(paletteBackground("0xFF7C4A2D")).toBe("brown");
    expect(paletteBackground("0xFF9CA3AF")).toBe("gray");
    expect(paletteBackground("0xFFFFFFFF")).toBeUndefined();
    expect(paletteBackground("0x00FF0000")).toBeUndefined();
    expect(paletteBackground("blue")).toBeUndefined();
    expect(paletteBackground("0x12345")).toBeUndefined();
  });
});
