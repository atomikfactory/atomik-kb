// The import of `MemoryTransport` (ADR-0036, 3 and 7): sessions (`ImportBegin`, `ImportAddFile`,
// `ImportPlan`, `ImportAddAttachment`, `ImportCommit`, `ImportAbort`), the commit (properties,
// pages, documents, values, events, the report) and the shell's `importRun` on canned sources.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { demoImagePng } from "./demo-image";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { ImportPlanInfo } from "./generated/ImportPlanInfo";
import type { ImportProgress } from "./generated/ImportProgress";
import type { ImportReport } from "./generated/ImportReport";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { decodeSourceText, isSafeRelativePath, isSkippedPath, isTextFile } from "./memory-import";
import type { MemoryImportSource } from "./memory-import";
import { TAGS_PROPERTY_ID } from "./properties";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

// Ids in creation order: the device (1), the workspace (2), "Home" (3), "Existing" (4), "Child" (5),
// "Trashed" (6), then "Other space" (7) and "Far" (8).
const HOME = id(3);
const EXISTING = id(4);
const TRASHED = id(6);
const FAR = id(8);

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    pages: [{ title: "Existing", children: [{ title: "Child" }] }, { title: "Trashed" }],
    properties: [
      { name: "Status", kind: "status" },
      { name: "Estimate", kind: "number" },
    ],
  },
  { name: "Other space", pages: [{ title: "Far" }] },
];

async function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0,
    workspaceName: "Workspace",
    spaces: SPACES,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  await client.pageTrash(TRASHED);
  events.length = 0;
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

async function begin({ transport }: Fixture, parentId?: string, sourceName = "", spaceId = HOME): Promise<number> {
  const response = await transport.dispatch({ type: "ImportBegin", data: { spaceId, ...(parentId === undefined ? {} : { parentId }), sourceName } });
  if (response.type !== "ImportStarted") {
    throw new Error("unexpected response");
  }
  return response.data;
}

async function addFile({ transport }: Fixture, importId: number, path: string, text: string): Promise<void> {
  await transport.dispatch({ type: "ImportAddFile", data: { importId, path, text } });
}

async function plan({ transport }: Fixture, importId: number, binaryPaths: string[] = []): Promise<ImportPlanInfo> {
  const response = await transport.dispatch({ type: "ImportPlan", data: { importId, binaryPaths } });
  if (response.type !== "ImportPlanned") {
    throw new Error("unexpected response");
  }
  return response.data;
}

async function commit({ transport }: Fixture, importId: number, maxPages = 50): Promise<ImportProgress> {
  const response = await transport.dispatch({ type: "ImportCommit", data: { importId, maxPages } });
  if (response.type !== "ImportProgress") {
    throw new Error("unexpected response");
  }
  return response.data;
}

/** Runs a whole import through the raw requests; the report. */
async function importAll(
  fixture: Fixture,
  files: Record<string, string>,
  options: { binary?: string[]; parentId?: string; sourceName?: string; uploads?: Record<string, Uint8Array> } = {},
): Promise<ImportReport> {
  const importId = await begin(fixture, options.parentId, options.sourceName ?? "");
  for (const [path, text] of Object.entries(files)) {
    await addFile(fixture, importId, path, text);
  }
  const planned = await plan(fixture, importId, options.binary ?? []);
  for (const path of planned.neededFiles) {
    const bytes = options.uploads?.[path] ?? demoImagePng();
    const info = await fixture.client.attachmentUpload(HOME, path.split("/").at(-1) ?? path, "application/octet-stream", [bytes]);
    await fixture.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path, attachmentId: info.attachmentId } });
  }
  for (;;) {
    const progress = await commit(fixture, importId);
    if (progress.report !== undefined) {
      return progress.report;
    }
  }
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

async function outline(client: CoreClient, spaceId = HOME): Promise<string[]> {
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

async function findPage(client: CoreClient, title: string, spaceId = HOME): Promise<string> {
  const search = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): string | undefined => {
    for (const node of nodes) {
      if (node.page.title === title) {
        return node.page.id;
      }
      const inner = search(node.children);
      if (inner !== undefined) {
        return inner;
      }
    }
    return undefined;
  };
  const found = search((await client.treeGet(spaceId)).roots);
  if (found === undefined) {
    throw new Error(`no page titled ${title}`);
  }
  return found;
}

describe("the helpers of the shell", () => {
  it("accepts only relative paths of / separated names", () => {
    for (const good of ["a.md", "a/b.md", "Ünï/Çode 日本.md", "a b/c"]) {
      expect(isSafeRelativePath(good)).toBe(true);
    }
    for (const bad of ["", "/abs", "a//b", "a/./b", "./a", "../a", "a/../b", "a/..", "C:\\x", "C:/x", "a\\b", "a\0b", `${"x".repeat(1025)}`]) {
      expect(isSafeRelativePath(bad)).toBe(false);
    }
    expect(isSafeRelativePath("x".repeat(1024))).toBe(true);
  });

  it("reads files with the text extensions, and the sidecar at the root, as text", () => {
    for (const text of ["a.md", "A.MARKDOWN", "b.txt", "c.html", "d.HTM", "e.csv", "f.excalidraw", "atomik-export.json", "dir/g.md"]) {
      expect(isTextFile(text)).toBe(true);
    }
    for (const binary of ["a.png", "b.pdf", "c.json", "dir/atomik-export.json", "notes"]) {
      expect(isTextFile(binary)).toBe(false);
    }
  });

  it("skips hidden entries, __MACOSX and node_modules", () => {
    for (const skipped of [".DS_Store", "a/.hidden/b.md", "__MACOSX/a.md", "a/node_modules/b.js", "a/.git"]) {
      expect(isSkippedPath(skipped)).toBe(true);
    }
    expect(isSkippedPath("a/b.md")).toBe(false);
    expect(isSkippedPath("a.b/c.md")).toBe(false);
  });

  it("decodes UTF-8 (byte order mark removed), UTF-16 with a mark, and flags other bytes as lossy", () => {
    const encoder = new TextEncoder();
    expect(decodeSourceText(encoder.encode("héllo"))).toEqual({ text: "héllo", lossy: false });
    expect(decodeSourceText(Uint8Array.from([0xef, 0xbb, 0xbf, ...encoder.encode("bom")]))).toEqual({ text: "bom", lossy: false });
    expect(decodeSourceText(Uint8Array.from([0xff, 0xfe, 0x68, 0x00, 0x69, 0x00]))).toEqual({ text: "hi", lossy: false });
    expect(decodeSourceText(Uint8Array.from([0xfe, 0xff, 0x00, 0x68, 0x00, 0x69]))).toEqual({ text: "hi", lossy: false });
    expect(decodeSourceText(Uint8Array.from([0x61, 0xff, 0x62]))).toEqual({ text: "a\u{FFFD}b", lossy: true });
  });
});

describe("ImportBegin", () => {
  it("counts up from 1, never reusing an id, and a new session drops the old one", async () => {
    const fixture = await setup();
    expect(await begin(fixture)).toBe(1);
    expect(await begin(fixture)).toBe(2);
    const dropped = await failure(fixture.transport.dispatch({ type: "ImportAddFile", data: { importId: 1, path: "a.md", text: "" } }));
    expect(dropped.code).toBe("NotFound");
    await fixture.transport.dispatch({ type: "ImportAbort", data: { importId: 2 } });
    expect(await begin(fixture)).toBe(3);
  });

  it("checks the space and the destination", async () => {
    const fixture = await setup();
    expect((await failure(begin(fixture, undefined, "", id(99)))).code).toBe("NotFound");
    expect((await failure(begin(fixture, id(99)))).code).toBe("NotFound");
    const elsewhere = await failure(begin(fixture, FAR));
    expect(elsewhere.code).toBe("InvalidInput");
    expect(elsewhere.details).toEqual({ field: "parentId" });
    const trashed = await failure(begin(fixture, TRASHED));
    expect(trashed.details).toEqual({ field: "parentId" });
    expect(await begin(fixture, EXISTING)).toBeGreaterThan(0);
  });
});

describe("ImportAddFile", () => {
  it("refuses paths that are not relative paths of safe names", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    for (const path of ["", "/etc/passwd", "a//b.md", "a/../b.md", "./a.md", "C:\\x.md", "a\\b.md", "x".repeat(1025)]) {
      const error = await failure(addFile(fixture, importId, path, "x"));
      expect(error.code).toBe("InvalidInput");
      expect(error.details).toMatchObject({ field: "path" });
    }
    await addFile(fixture, importId, "a/b.md", "x");
  });

  it("refuses a file above 16 MiB, and more than 10,000 files", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    const big = await failure(addFile(fixture, importId, "big.md", "x".repeat(16 * 1024 * 1024 + 1)));
    expect(big.details).toEqual({ field: "text", limit: 16 * 1024 * 1024 });
    await addFile(fixture, importId, "ok.md", "x".repeat(16 * 1024 * 1024));
    for (let n = 0; n < 9_999; n += 1) {
      await addFile(fixture, importId, `f${String(n)}.md`, "");
    }
    const many = await failure(addFile(fixture, importId, "one-too-many.md", ""));
    expect(many.details).toEqual({ field: "path", limit: 10_000 });
    // A path given again replaces the file and is no new file.
    await addFile(fixture, importId, "f0.md", "again");
  });

  it("refuses unknown sessions and files after the plan", async () => {
    const fixture = await setup();
    expect((await failure(addFile(fixture, 99, "a.md", ""))).code).toBe("NotFound");
    const importId = await begin(fixture);
    await plan(fixture, importId);
    const late = await failure(addFile(fixture, importId, "a.md", ""));
    expect(late.code).toBe("InvalidInput");
    expect(late.details).toEqual({ field: "importId" });
  });
});

describe("ImportPlan", () => {
  it("answers the pages to create, the files they need and the warnings, and keeps the plan", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    await addFile(fixture, importId, "Notes/A.md", "![pic](img/a.png)\n\n[gone](nothing.md)");
    await addFile(fixture, importId, "Notes/B.txt", "text");
    await addFile(fixture, importId, "Notes/c.json", "{}");
    const planned = await plan(fixture, importId, ["Notes/img/a.png", "Notes/img/unused.png"]);
    expect(planned).toEqual({
      pages: 2,
      neededFiles: ["Notes/img/a.png"],
      warnings: [
        { path: "Notes/c.json", code: "unsupportedFile" },
        { path: "Notes/A.md", code: "unresolvedLink", detail: "nothing.md" },
      ],
    });
    const again = await failure(plan(fixture, importId));
    expect(again.details).toEqual({ field: "importId" });
  });

  it("omits the warnings when there are none", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    await addFile(fixture, importId, "A.md", "x");
    expect(await plan(fixture, importId)).toEqual({ pages: 1, neededFiles: [] });
  });

  it("refuses unsafe paths of other files", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    const error = await failure(plan(fixture, importId, ["../outside.png"]));
    expect(error.details).toEqual({ field: "binaryPaths" });
  });
});

describe("ImportAddAttachment", () => {
  async function planned(fixture: Fixture) {
    const importId = await begin(fixture);
    await addFile(fixture, importId, "A.md", "![](pic.png)");
    await plan(fixture, importId, ["pic.png", "other.png"]);
    const info = await fixture.client.attachmentUpload(HOME, "pic.png", "image/png", [demoImagePng()]);
    return { importId, attachmentId: info.attachmentId };
  }

  it("takes a complete attachment of the space for a needed path", async () => {
    const fixture = await setup();
    const { importId, attachmentId } = await planned(fixture);
    expect(await fixture.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path: "pic.png", attachmentId } })).toEqual({ type: "Ok" });
  });

  it("refuses a path that is not needed, an attachment that is unknown, of another space or not complete, and a session without a plan", async () => {
    const fixture = await setup({
      attachmentSync: {
        downloadPolicy: "on_demand",
        remoteAttachments: [{ attachmentId: "aaaaaaaa-0000-4000-8000-000000000001", name: "far.png", mime: "image/png", bytes: new Uint8Array([1]) }],
      },
    });
    const { importId, attachmentId } = await planned(fixture);
    const add = (path: string, id: string) => fixture.transport.dispatch({ type: "ImportAddAttachment", data: { importId, path, attachmentId: id } });
    expect((await failure(add("other.png", attachmentId))).details).toEqual({ field: "path" });
    expect((await failure(add("pic.png", id(99)))).code).toBe("NotFound");
    const otherSpace = (await fixture.client.spaceList())[1]?.id ?? "";
    const abroad = await fixture.client.attachmentUpload(otherSpace, "x.png", "image/png", [new Uint8Array([1])]);
    expect((await failure(add("pic.png", abroad.attachmentId))).details).toEqual({ field: "attachmentId" });
    expect((await failure(add("pic.png", "aaaaaaaa-0000-4000-8000-000000000001"))).details).toEqual({ field: "attachmentId" });
    const early = await begin(fixture);
    expect((await failure(fixture.transport.dispatch({ type: "ImportAddAttachment", data: { importId: early, path: "pic.png", attachmentId } }))).details).toEqual({ field: "importId" });
  });
});

describe("ImportCommit", () => {
  it("writes the pages in batches, under the destination after its children, and answers the report on the last call", async () => {
    const fixture = await setup();
    const importId = await begin(fixture, EXISTING);
    await addFile(fixture, importId, "One.md", "# One\n\nfirst");
    await addFile(fixture, importId, "Two.md", "second");
    await addFile(fixture, importId, "Three.txt", "third");
    await plan(fixture, importId);
    expect(await commit(fixture, importId, 2)).toEqual({ created: 2, total: 3, done: false });
    // Every page exists in the tree after the first call, under the existing children.
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "  One", "  Three", "  Two"]);
    const last = await commit(fixture, importId, 2);
    expect(last.created).toBe(3);
    expect(last.done).toBe(true);
    expect(last.report).toMatchObject({ pages: 3, attachments: 0, propertiesCreated: 0, moreWarnings: 0 });
    expect(last.report?.rootPageIds).toHaveLength(3);
    const gone = await failure(commit(fixture, importId));
    expect(gone.code).toBe("NotFound");
  });

  it("emits the events of the writes once each, then ImportProgress, after every call", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    await addFile(fixture, importId, "One.md", "first");
    await addFile(fixture, importId, "Two.md", "---\nStatus: Done\n---\nsecond");
    await plan(fixture, importId);
    fixture.events.length = 0;
    await commit(fixture, importId, 1);
    const types = fixture.events.map((event) => event.type);
    expect(types.filter((type) => type === "StructureChanged")).toHaveLength(1);
    expect(types.filter((type) => type === "PageContentChanged")).toHaveLength(1);
    expect(fixture.events.at(-1)).toEqual({ type: "ImportProgress", data: { importId, done: 1, total: 2 } });
    expect(fixture.events.find((event) => event.type === "PageContentChanged")).toMatchObject({ data: { source: "Core" } });
    fixture.events.length = 0;
    await commit(fixture, importId, 1);
    expect(fixture.events.map((event) => event.type)).toEqual(["PageContentChanged", "StructureChanged", "ImportProgress"]);
    expect(fixture.events.at(-1)).toEqual({ type: "ImportProgress", data: { importId, done: 2, total: 2 } });
  });

  it("answers done at once for an import with nothing in it", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    await plan(fixture, importId);
    const progress = await commit(fixture, importId);
    expect(progress).toEqual({ created: 0, total: 0, done: true, report: { rootPageIds: [], pages: 0, attachments: 0, propertiesCreated: 0, moreWarnings: 0 } });
  });

  it("checks maxPages and the session, and wants the plan first", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    const early = await failure(commit(fixture, importId));
    expect(early.details).toEqual({ field: "importId" });
    await plan(fixture, importId);
    const zero = await failure(commit(fixture, importId, 0));
    expect(zero.details).toEqual({ field: "maxPages" });
    expect((await failure(commit(fixture, 99))).code).toBe("NotFound");
  });

  it("keeps the pages already written when the import is aborted, and drops the session", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    await addFile(fixture, importId, "One.md", "first");
    await addFile(fixture, importId, "Two.md", "second");
    await plan(fixture, importId);
    await commit(fixture, importId, 1);
    expect(await fixture.transport.dispatch({ type: "ImportAbort", data: { importId } })).toEqual({ type: "Ok" });
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "One", "Two"]);
    expect((await failure(commit(fixture, importId))).code).toBe("NotFound");
    // Aborting what is gone is no error.
    expect(await fixture.transport.dispatch({ type: "ImportAbort", data: { importId } })).toEqual({ type: "Ok" });
  });

  it("drops the session when the app locks", async () => {
    const fixture = await setup({ security: { kitCreatedAt: T0, appLockPassphrase: "correct horse battery" } });
    const importId = await begin(fixture);
    await fixture.transport.lock();
    await fixture.transport.unlock("correct horse battery");
    expect((await failure(plan(fixture, importId))).code).toBe("NotFound");
  });

  it("is one step of structural undo", async () => {
    const fixture = await setup();
    await importAll(fixture, { "One.md": "1", "Two.md": "2" });
    expect(await outline(fixture.client)).toContain("One");
    await fixture.client.structureUndo(HOME);
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child"]);
  });
});

describe("the pages written", () => {
  it("creates the tree with titles and icons, and gives every block of the content an id", async () => {
    const fixture = await setup();
    const report = await importAll(fixture, {
      "Notes/Plan.md": '---\ntitle: "My plan"\nicon: "📝"\n---\n# Heading\n\nSome **bold** text.\n\n- one\n- two\n\n> 💡 A callout\n',
      "Notes/Plan/Sub.md": "sub",
      "Notes/Other.md": "other",
    });
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "Other", "My plan", "  Sub"]);
    const plan = await findPage(fixture.client, "My plan");
    expect((await fixture.client.pageGet(plan)).icon).toBe("📝");
    expect(report.rootPageIds).toEqual([await findPage(fixture.client, "Other"), plan]);
    const content = (await fixture.client.pageContentGet(plan)).content as { content: { type: string; attrs?: { id?: string } }[] };
    expect(content.content.map((block) => block.type)).toEqual(["heading", "paragraph", "bulletList", "callout"]);
    const ids: string[] = [];
    const collect = (node: JsonValue): void => {
      if (typeof node === "object" && node !== null && !Array.isArray(node)) {
        const attrs = node.attrs;
        if (typeof attrs === "object" && attrs !== null && !Array.isArray(attrs) && typeof attrs.id === "string") {
          ids.push(attrs.id);
        }
        collect(node.content ?? null);
      } else if (Array.isArray(node)) {
        node.forEach(collect);
      }
    };
    collect(content);
    expect(ids.length).toBeGreaterThan(6);
    expect(new Set(ids).size).toBe(ids.length);
    for (const blockId of ids) {
      expect(blockId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u);
    }
  });

  it("indexes the pages like any page: search finds the text, links make backlinks", async () => {
    const fixture = await setup();
    await importAll(fixture, { "A.md": "The zebra crossing and [B](B.md).", "B.md": "target" });
    const hits = await fixture.client.search("zebra");
    expect(hits.map((hit) => hit.title)).toEqual(["A"]);
    const backlinks = await fixture.client.backlinksGet(await findPage(fixture.client, "B"));
    expect(backlinks.map((link) => link.title)).toEqual(["A"]);
    expect(fixture.events.filter((event) => event.type === "BacklinksChanged")).toHaveLength(1);
  });

  it("resolves links between the pages of the import, wiki links by title, and an existing page of the space by its title", async () => {
    const fixture = await setup();
    const report = await importAll(fixture, {
      "A.md": "See [B](B.md), [[B]], [[Existing]], [[Nobody]] and [gone](gone.md).\n\n[B](B.md)\n",
      "B.md": "x",
    });
    const a = await findPage(fixture.client, "A");
    const b = await findPage(fixture.client, "B");
    const existing = await findPage(fixture.client, "Existing");
    expect(await contentOf(fixture.client, a)).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "See " },
            { type: "pageRef", attrs: { pageId: b } },
            { type: "text", text: ", " },
            { type: "pageRef", attrs: { pageId: b } },
            { type: "text", text: ", " },
            { type: "pageRef", attrs: { pageId: existing } },
            { type: "text", text: ", [[Nobody]] and gone." },
          ],
        },
        { type: "pageLink", attrs: { pageId: b } },
      ],
    });
    expect(report.warnings).toEqual([
      { path: "A.md", code: "unresolvedLink", detail: "gone.md" },
      { path: "A.md", code: "unresolvedLink", detail: "Nobody" },
    ]);
  });

  it("looks a title up among the live pages of the space only when exactly one has it", async () => {
    const fixture = await setup();
    await fixture.client.pageCreate({ spaceId: HOME, title: "Twin" });
    await fixture.client.pageCreate({ spaceId: HOME, title: "Twin" });
    await fixture.client.pageCreate({ spaceId: HOME, title: "Single" });
    const report = await importAll(fixture, { "A.md": "[[Twin]] and [[Single]] and [[Trashed]]" });
    const a = await findPage(fixture.client, "A");
    const single = await findPage(fixture.client, "Single");
    expect(await contentOf(fixture.client, a)).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "[[Twin]] and " },
            { type: "pageRef", attrs: { pageId: single } },
            { type: "text", text: " and [[Trashed]]" },
          ],
        },
      ],
    });
    expect(report.warnings?.map((warning) => warning.detail)).toEqual(["Twin", "Trashed"]);
  });

  it("makes a page of every file of the source, a table view of a CSV and a child page per row with its values", async () => {
    const fixture = await setup();
    const report = await importAll(fixture, {
      "Tasks.csv": "Name,Done,Estimate,Status,Labels\nWrite,Yes,3,Done,\"a, b\"\nTest,No,x,Blocked,b\nShip,Yes,2.5,Done,\"b, c\"\nPlan,No,1,Blocked,a\n",
    });
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "Tasks", "  Write", "  Test", "  Ship", "  Plan"]);
    const schema = await fixture.client.propertySchemaGet(HOME);
    const named = (name: string) => schema.find((def) => def.name === name);
    // Matched by name whatever the kind: Status and Estimate exist, the others are created.
    expect(named("Status")?.kind).toBe("status");
    expect(named("Estimate")?.kind).toBe("number");
    expect(named("Done")?.kind).toBe("checkbox");
    expect(named("Labels")).toMatchObject({ kind: "multi_select", options: [{ name: "a" }, { name: "b" }, { name: "c" }] });
    expect(report.propertiesCreated).toBe(2);
    const write = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "Write"));
    const byName = (values: typeof write, name: string) => values.find(([propertyId]) => propertyId === named(name)?.id)?.[1];
    expect(byName(write, "Done")).toEqual({ kind: "checkbox", value: true });
    expect(byName(write, "Estimate")).toEqual({ kind: "number", value: 3 });
    expect(byName(write, "Labels")).toMatchObject({ kind: "multi_select" });
    // "Done" is an option of the status property; "Blocked" is created.
    const status = named("Status");
    expect(status?.options.map((option) => option.name)).toEqual(["Not started", "In progress", "Done", "Blocked"]);
    expect(byName(write, "Status")).toEqual({ kind: "status", value: status?.options[2]?.id });
    // "x" does not fit a number: a warning, no value.
    const test = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "Test"));
    expect(byName(test, "Estimate")).toBeUndefined();
    expect(report.warnings).toEqual([{ path: "", code: "invalidValue", detail: "Estimate" }]);
    const table = (await fixture.client.pageContentGet(await findPage(fixture.client, "Tasks"))).content as { content: { type: string; attrs: { properties: string[]; layout: string } }[] };
    expect(table.content[0]?.type).toBe("pageList");
    expect(table.content[0]?.attrs.layout).toBe("table");
    expect(table.content[0]?.attrs.properties).toEqual(["Done", "Estimate", "Status", "Labels"].map((name) => named(name)?.id));
  });

  it("sends tags to the built-in Tags property and creates its options", async () => {
    const fixture = await setup();
    const report = await importAll(fixture, { "A.md": "---\ntags: [work, review]\n---\nx", "B.md": "---\ntags: work\n---\ny" });
    const schema = await fixture.client.propertySchemaGet(HOME);
    const tags = schema.find((def) => def.id === TAGS_PROPERTY_ID);
    expect(tags?.options.map((option) => option.name)).toEqual(["work", "review"]);
    expect(report.propertiesCreated).toBe(0);
    const values = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "A"));
    expect(values[0]?.[1]).toEqual({ kind: "multi_select", value: tags?.options.map((option) => option.id) });
  });

  it("creates properties of the kinds the front matter shows, and counts them in the report", async () => {
    const fixture = await setup();
    const report = await importAll(fixture, {
      "A.md": `---
Rating: 4
Reviewed: true
Due: 2026-10-07 14:30
Labels: [x, y]
Notes: free text
Related: "[[B]]"
Checklist:
  - "[x] a"
  - "[ ] b"
Place:
  name: Paris
  lat: 48.8566
  lon: 2.3522
---
body`,
      "B.md": "b",
    });
    const schema = await fixture.client.propertySchemaGet(HOME);
    expect(Object.fromEntries(schema.map((def) => [def.name, def.kind]))).toMatchObject({
      Rating: "number",
      Reviewed: "checkbox",
      Due: "date",
      Labels: "multi_select",
      Notes: "text",
      Related: "relation",
      Checklist: "checklist",
      Place: "place",
    });
    expect(report.propertiesCreated).toBe(8);
    const values = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "A"));
    const valueOf = (name: string) => values.find(([propertyId]) => propertyId === schema.find((def) => def.name === name)?.id)?.[1];
    expect(valueOf("Rating")).toEqual({ kind: "number", value: 4 });
    expect(valueOf("Reviewed")).toEqual({ kind: "checkbox", value: true });
    expect(valueOf("Due")).toEqual({ kind: "date", value: { date: "2026-10-07", time: "14:30" } });
    expect(valueOf("Notes")).toEqual({ kind: "text", value: "free text" });
    expect(valueOf("Related")).toEqual({ kind: "relation", value: [await findPage(fixture.client, "B")] });
    expect(valueOf("Checklist")).toMatchObject({ kind: "checklist", value: [{ text: "a", done: true }, { text: "b", done: false }] });
    expect(valueOf("Place")).toEqual({ kind: "place", value: { name: "Paris", lat: 48.8566, lon: 2.3522 } });
  });

  it("matches properties by name with Unicode case folding and reuses the existing one whatever its kind", async () => {
    const fixture = await setup();
    await fixture.client.propertyCreate(HOME, "Ärger", "text");
    const report = await importAll(fixture, { "A.md": "---\nÄRGER: 5\nSTATUS: Blocked\n---\nx" });
    expect(report.propertiesCreated).toBe(0);
    const schema = await fixture.client.propertySchemaGet(HOME);
    const values = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "A"));
    expect(values.find(([propertyId]) => propertyId === schema.find((def) => def.name === "Ärger")?.id)?.[1]).toEqual({ kind: "text", value: "5" });
    const status = schema.find((def) => def.name === "Status");
    expect(status?.options.map((option) => option.name)).toContain("Blocked");
  });

  it("turns the properties the space cannot hold into warnings, and drops their values", async () => {
    const fixture = await setup();
    for (let n = 0; n < 96; n += 1) {
      await fixture.client.propertyCreate(HOME, `P${String(n)}`, "text");
    }
    // Tags, Status, Estimate and 96 more: 99 of 100.
    const report = await importAll(fixture, { "A.md": "---\nOne: 1\nTwo: 2\n---\nx" });
    expect(report.propertiesCreated).toBe(1);
    expect(report.warnings).toEqual([{ path: "", code: "propertyLimit", detail: "Two" }]);
    const schema = await fixture.client.propertySchemaGet(HOME);
    expect(schema).toHaveLength(100);
    expect(await fixture.client.pagePropertiesGet(await findPage(fixture.client, "A"))).toHaveLength(1);
  });

  it("creates relations two-way with the planned name of the pair, and rollups once their relation exists", async () => {
    const fixture = await setup();
    const sidecar = JSON.stringify({
      format: 1,
      properties: [
        { name: "Related", kind: "relation", config: { twoWay: true, pairName: "Back" } },
        { name: "Estimate", kind: "number" },
        { name: "Total", kind: "rollup", config: { relation: "Related", target: { property: "Estimate" }, function: "sum" } },
      ],
      pages: [],
    });
    const report = await importAll(fixture, { "atomik-export.json": sidecar, "A.md": '---\nRelated: "[[B]]"\nEstimate: 2\n---\nx', "B.md": "---\nEstimate: 3\n---\ny" });
    const schema = await fixture.client.propertySchemaGet(HOME);
    const related = schema.find((def) => def.name === "Related");
    expect(related?.config).toMatchObject({ kind: "relation", twoWay: true });
    expect(schema.find((def) => def.name === "Back")?.kind).toBe("relation");
    expect(schema.find((def) => def.name === "Related (back)")).toBeUndefined();
    expect(schema.find((def) => def.name === "Total")).toMatchObject({ kind: "rollup", config: { kind: "rollup", function: "sum" } });
    // Pair (2) and rollup (1); Estimate exists already.
    expect(report.propertiesCreated).toBe(3);
    const b = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "B"));
    expect(b.find(([propertyId]) => propertyId === schema.find((def) => def.name === "Back")?.id)?.[1]).toEqual({
      kind: "relation",
      value: [await findPage(fixture.client, "A")],
    });
    const a = await fixture.client.pagePropertiesGet(await findPage(fixture.client, "A"));
    expect(a.find(([propertyId]) => propertyId === schema.find((def) => def.name === "Total")?.id)?.[1]).toMatchObject({
      kind: "rollup",
      value: { result: { type: "number", data: 3 } },
    });
  });

  it("orders the warnings: planning, resolving, then coercion, at most 500, the rest counted", async () => {
    const fixture = await setup();
    const files: Record<string, string> = { "a.md": "[[Nobody]] and [x](missing.md)", "b.json": "{}" };
    // 600 pages with a value that does not fit the number property Estimate.
    for (let n = 0; n < 600; n += 1) {
      files[`bad${String(n).padStart(3, "0")}.md`] = "---\nEstimate: soon\n---\nx";
    }
    const report = await importAll(fixture, files);
    const codes = (report.warnings ?? []).map((warning) => warning.code);
    expect(codes).toHaveLength(500);
    // Planning first (the file it does not read, then the link it cannot follow), then resolving, then the values.
    expect(codes.slice(0, 3)).toEqual(["unsupportedFile", "unresolvedLink", "unresolvedLink"]);
    expect((report.warnings ?? [])[2]).toEqual({ path: "a.md", code: "unresolvedLink", detail: "Nobody" });
    expect(codes.slice(3).every((code) => code === "invalidValue")).toBe(true);
    expect(report.moreWarnings).toBe(3 + 600 - 500);
  });
});

describe("images and files", () => {
  it("turns the files the pages use into image and file blocks on the uploaded attachments, and counts them once", async () => {
    const fixture = await setup();
    const report = await importAll(
      fixture,
      {
        "Notes/A.md": "![First](img/a.png)\n\n[Report](docs/report.pdf)\n",
        "Notes/B.md": "---\nAttachments:\n  - docs/report.pdf\n---\n![Again](img/a.png)\n",
      },
      { binary: ["Notes/img/a.png", "Notes/docs/report.pdf", "Notes/img/unused.png"] },
    );
    expect(report.attachments).toBe(2);
    const attachments = await fixture.client.attachmentList(HOME);
    expect(attachments.map((entry) => entry.name).sort()).toEqual(["a.png", "report.pdf"]);
    const a = await findPage(fixture.client, "A");
    const image = attachments.find((entry) => entry.name === "a.png");
    const file = attachments.find((entry) => entry.name === "report.pdf");
    expect(await contentOf(fixture.client, a)).toEqual({
      type: "doc",
      content: [
        { type: "image", attrs: { alt: "First", attachmentId: image?.attachmentId } },
        { type: "file", attrs: { attachmentId: file?.attachmentId } },
      ],
    });
    // The attachments are referenced: nothing is unreferenced, and the references are counted.
    expect(attachments.every((entry) => entry.references > 0 && entry.unreferencedSince === undefined)).toBe(true);
  });

  it("turns a file the shell could not upload into text with a warning", async () => {
    const fixture = await setup();
    const importId = await begin(fixture);
    await addFile(fixture, importId, "A.md", "![alt text](img/a.png)\n\n[Report](report.pdf)");
    await plan(fixture, importId, ["img/a.png", "report.pdf"]);
    // Nothing is uploaded.
    let report: ImportReport | undefined;
    while (report === undefined) {
      report = (await commit(fixture, importId)).report;
    }
    expect(report.attachments).toBe(0);
    expect(report.warnings).toEqual([
      { path: "A.md", code: "missingFile", detail: "img/a.png" },
      { path: "A.md", code: "missingFile", detail: "report.pdf" },
    ]);
    expect(await contentOf(fixture.client, await findPage(fixture.client, "A"))).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "alt text" }] },
        { type: "paragraph", content: [{ type: "text", text: "report.pdf" }] },
      ],
    });
  });

  it("checks an attachment named by a link of the source (attachment://) belongs to the space", async () => {
    const fixture = await setup();
    const mine = await fixture.client.attachmentUpload(HOME, "mine.png", "image/png", [demoImagePng()]);
    const report = await importAll(fixture, {
      "A.md": `![mine](attachment://${mine.attachmentId})\n\n![foreign](attachment://aaaaaaaa-0000-4000-8000-000000000009)\n`,
    });
    const blocks = ((await contentOf(fixture.client, await findPage(fixture.client, "A"))) as { content: { type: string }[] }).content;
    expect(blocks.map((block) => block.type)).toEqual(["image", "paragraph"]);
    expect(report.warnings).toEqual([{ path: "A.md", code: "missingFile", detail: "aaaaaaaa-0000-4000-8000-000000000009" }]);
    expect(report.attachments).toBe(1);
  });
});

describe("importRun", () => {
  const SOURCES: Record<string, MemoryImportSource> = {
    "Notion export.zip": {
      kind: "zip",
      files: {
        "Getting Started 0123456789abcdef0123456789abcdef.md": "# Getting Started\n\n![pic](pic.png)\n\nSee [Tasks](Tasks.md).",
        "Tasks.md": "tasks",
        "pic.png": demoImagePng(),
        ".DS_Store": "junk",
        "__MACOSX/pic.png": "junk",
        "node_modules/x/index.md": "junk",
      },
    },
    "My notes": { kind: "folder", files: { "a.md": "A", "b.txt": "B", "notes/c.md": "C", "/abs.md": "x", "x//y.md": "x", "a b/c.docx": "x" } },
    "": { kind: "files", files: { "one.md": "one" } },
  };

  it("picks the first source of the kind asked for and runs the session like the shell", async () => {
    const fixture = await setup({ importSources: SOURCES });
    const report = await fixture.client.importRun({ spaceId: HOME, source: "zip" });
    // The container carries the zip's name without .zip: the source has two top-level pages.
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "Notion export", "  Getting Started", "  Tasks"]);
    expect(report.pages).toBe(3);
    expect(report.attachments).toBe(1);
    expect(report.rootPageIds).toEqual([await findPage(fixture.client, "Notion export")]);
    expect(report.warnings).toBeUndefined();
    const attachments = await fixture.client.attachmentList(HOME);
    expect(attachments).toMatchObject([{ name: "pic.png", mime: "image/png", size: demoImagePng().length }]);
    expect(await fixture.client.attachmentRead(attachments[0]?.attachmentId ?? "")).toEqual(demoImagePng());
    expect(fixture.events.filter((event) => event.type === "ImportProgress").length).toBeGreaterThan(0);
  });

  it("names the container after a folder, none for files, puts the pages under the destination, skips what the shell skips", async () => {
    const fixture = await setup({ importSources: SOURCES });
    const report = await fixture.client.importRun({ spaceId: HOME, source: "folder", parentId: EXISTING });
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "  My notes", "    a", "    b", "    notes", "      c"]);
    // Unsafe paths are left out; a file the importer does not read as text and no page uses is not an error.
    expect(report.warnings).toEqual([
      { path: "/abs.md", code: "unsupportedFile" },
      { path: "x//y.md", code: "unsupportedFile" },
    ]);
    const files = await fixture.client.importRun({ spaceId: HOME, source: "files" });
    expect(files.pages).toBe(1);
    expect(await findPage(fixture.client, "one")).toBeDefined();
  });

  it("is a cancelled dialog when no source has the kind asked for, or the dialogs are cancelled", async () => {
    const none = await setup({ importSources: { only: { kind: "folder", files: { "a.md": "" } } } });
    const missing = await failure(none.client.importRun({ spaceId: HOME, source: "zip" }));
    expect(missing.code).toBe("InvalidInput");
    expect(missing.details).toEqual({ field: "path" });
    const cancelled = await setup({ importSources: SOURCES, cancelDialogs: true });
    expect((await failure(cancelled.client.importRun({ spaceId: HOME, source: "zip" }))).details).toEqual({ field: "path" });
    const nothing = await setup();
    expect((await failure(nothing.client.importRun({ spaceId: HOME, source: "files" }))).details).toEqual({ field: "path" });
  });

  it("decodes text files and warns about the ones that are not UTF-8 or too large", async () => {
    const fixture = await setup({
      importSources: {
        s: {
          kind: "files",
          files: {
            "utf16.md": Uint8Array.from([0xff, 0xfe, ...[..."héllo"].flatMap((char) => [char.charCodeAt(0), 0])]),
            "latin.md": Uint8Array.from([0x63, 0x61, 0x66, 0xe9]),
            "huge.md": new Uint8Array(16 * 1024 * 1024 + 1).fill(0x61),
          },
        },
      },
    });
    const report = await fixture.client.importRun({ spaceId: HOME, source: "files" });
    expect(report.warnings).toEqual([
      { path: "latin.md", code: "notUtf8" },
      { path: "huge.md", code: "tooLarge" },
    ]);
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child", "latin", "utf16"]);
    expect(await contentOf(fixture.client, await findPage(fixture.client, "utf16"))).toEqual({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "héllo" }] }],
    });
  });

  it("reports a file it could not upload as attachmentFailed, and the page keeps its text", async () => {
    const fixture = await setup({
      importSources: { s: { kind: "files", files: { "a.md": "![alt](pic.png)", "pic.png": demoImagePng() } } },
    });
    // The shell's upload fails like any attachment call (see `failNextAttachments`).
    fixture.transport.failNextAttachments({ code: "Storage", message: "disk full" });
    const report = await fixture.client.importRun({ spaceId: HOME, source: "files" });
    expect(report.attachments).toBe(0);
    expect(report.warnings).toEqual([
      { path: "a.md", code: "missingFile", detail: "pic.png" },
      { path: "pic.png", code: "attachmentFailed" },
    ]);
  });

  it("fails before it writes anything when the destination cannot take pages", async () => {
    const fixture = await setup({ importSources: SOURCES });
    const error = await failure(fixture.client.importRun({ spaceId: HOME, source: "folder", parentId: TRASHED }));
    expect(error.details).toEqual({ field: "parentId" });
    expect(await outline(fixture.client)).toEqual(["Existing", "  Child"]);
    const unknown = await failure(fixture.client.importRun({ spaceId: id(99), source: "zip" }));
    expect(unknown.code).toBe("NotFound");
  });

  it("needs the core to be ready", async () => {
    const fixture = await setup({ importSources: SOURCES });
    fixture.transport.setCoreState("locked");
    expect((await failure(fixture.client.importRun({ spaceId: HOME, source: "zip" }))).code).toBe("Locked");
  });
});

describe("a <time> element of an exported file (ADR-0036 file flavour, ADR-0056)", () => {
  it("becomes a date mention in the imported page, with its time, reminder and what it is filled in with", async () => {
    const fixture = await setup();
    await importAll(fixture, {
      "Due.md": [
        'Today is <time datetime="2026-10-18">2026-10-18</time>.',
        "",
        'Call at <time datetime="2026-10-19T14:30" data-reminder="15" data-relative="now">2026-10-19 14:30</time>, not <time datetime="2026-02-30">2026-02-30</time>.',
      ].join("\n"),
    });
    const content = await contentOf(fixture.client, await findPage(fixture.client, "Due"));
    expect(content).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "Today is " },
            { type: "dateMention", attrs: { date: "2026-10-18" } },
            { type: "text", text: "." },
          ],
        },
        {
          type: "paragraph",
          content: [
            { type: "text", text: "Call at " },
            { type: "dateMention", attrs: { date: "2026-10-19", time: "14:30", reminder: 15, relative: "now" } },
            // Not a day: the element stays as text, in the run around it.
            { type: "text", text: ', not <time datetime="2026-02-30">2026-02-30</time>.' },
          ],
        },
      ],
    });
  });
});
