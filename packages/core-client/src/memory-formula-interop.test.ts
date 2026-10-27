// Formulas (ADR-0055) meeting the rest of the in-memory core: the export (the CSV of a formula column, the sidecar's
// display form, no value in the front matter), the import of a sidecar (names made ids, formulas after what they read,
// one that does not resolve kept as written) and the move to another space (the formula, what it reads and the
// widgets of a dashboard follow). The mirror of `kb-core`'s `tests_formulas.rs` and `space_move/mapping.rs` tests.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import type { ImportReport } from "./generated/ImportReport";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { readMemoryArchive } from "./memory-export";
import { exportValue, valueText } from "./memory-export-text";
import type { ValueLookups } from "./memory-export-text";
import { remapView, viewPropertyIds } from "./memory-move-values";
import type { ViewMap } from "./memory-move-values";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const OCT_7_1430 = 1_791_383_400_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

function must<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("a value the test needs is missing");
  }
  return value;
}

function make(spaces: readonly SeedSpace[], options: MemoryTransportOptions = {}) {
  let counter = 0x1000;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => OCT_7_1430,
    localOffsetSeconds: () => 0,
    workspaceName: "Workspace",
    security: { kitCreatedAt: T0 },
    spaces,
    ...options,
  });
  return { transport, client: new CoreClient(transport) };
}

// ----- export ---------------------------------------------------------------------------------

describe("the export of a formula", () => {
  const SPACE: SeedSpace = {
    name: "Home",
    properties: [
      { name: "Price", kind: "number" },
      { name: "Tasks", kind: "relation" },
      { name: "Double", kind: "formula", formula: 'prop("Price") * 2' },
      { name: "Shout", kind: "formula", formula: 'prop("title") + "!"' },
      { name: "Cheap", kind: "formula", formula: 'prop("Price") < 5' },
      { name: "Linked", kind: "formula", formula: 'prop("Tasks")' },
      { name: "Both", kind: "formula", formula: '[prop("Price"), prop("Price") * 2]' },
      { name: "Today", kind: "formula", formula: "today()" },
    ],
    pages: [
      { title: "Alpha", properties: { Price: 3, Tasks: ["Task A", "Task B"] } },
      { title: "Task A" },
      { title: "Task B" },
    ],
  };

  it("writes the result in the CSV like the kind it is", async () => {
    const { client } = make([SPACE]);
    const space = must((await client.spaceList())[0]);
    const schema = await client.propertySchemaGet(space.id);
    const column = (name: string) => ({ type: "Property" as const, data: { propertyId: must(schema.find((def) => def.name === name)).id } });
    const roots = (await client.treeGet(space.id)).roots;
    const alpha = must(roots.find((node) => node.page.title === "Alpha")).page.id;
    const csv = await client.exportCsv(
      space.id,
      [alpha],
      ["Double", "Shout", "Cheap", "Linked", "Both", "Today"].map(column),
    );
    expect(csv.split("\r\n")).toEqual([
      "\u{FEFF}Double,Shout,Cheap,Linked,Both,Today",
      '6,Alpha!,Yes,"Task A, Task B","3, 6",2026-10-07',
      "",
    ]);
  });

  it("leaves a formula out of the front matter and writes its expression in display form in the sidecar", async () => {
    const { client, transport } = make([SPACE]);
    const space = must((await client.spaceList())[0]);
    await client.exportRun({ spaceId: space.id, includeChildren: true, format: "markdown" });
    const files = readMemoryArchive(transport.files.get("/downloads/Home.zip") ?? new Uint8Array());
    const page = String(files?.["Alpha.md"]);
    expect(page).toContain('"Price": 3');
    expect(page).not.toContain("Double");
    const sidecar = JSON.parse(String(files?.["atomik-export.json"])) as {
      properties: { name: string; kind: string; config?: { expression?: string } }[];
    };
    const named = (name: string) => must(sidecar.properties.find((property) => property.name === name));
    expect(named("Double")).toEqual({ name: "Double", kind: "formula", config: { expression: 'prop("Price") * 2' } });
    expect(named("Shout").config).toEqual({ expression: 'prop("title") + "!"' });
    expect(JSON.stringify(sidecar)).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}-4/u);
  });

  it("is a number, text, checkbox, date, pages or names as the exporters write them", () => {
    const look: ValueLookups = {
      optionName: () => undefined,
      pageTitle: (page) => ({ [id(1)]: "Roadmap", [id(2)]: "Budget" })[page],
      fileName: () => undefined,
      filePath: () => undefined,
      deviceName: () => undefined,
      personName: (account) => ({ [id(3)]: "Ada" })[account],
    };
    const text = (result: JsonValue) => {
      const exported = exportValue({ kind: "formula", value: result as never }, look);
      return exported === undefined ? undefined : valueText(exported);
    };
    expect(text({ type: "number", data: 1.5 })).toBe("1.5");
    expect(text({ type: "boolean", data: false })).toBe("No");
    expect(text({ type: "date", data: { date: "2026-10-07", time: "14:30" } })).toBe("2026-10-07 14:30");
    expect(text({ type: "page", data: id(1) })).toBe("Roadmap");
    expect(text({ type: "page", data: id(9) })).toBeUndefined();
    expect(text({ type: "person", data: id(3) })).toBe("Ada");
    expect(text({ type: "person", data: id(9) })).toBe("Former member");
    expect(
      text({ type: "list", data: [{ type: "page", data: id(1) }, { type: "page", data: id(9) }, { type: "page", data: id(2) }] }),
    ).toBe("Roadmap, Budget");
    expect(text({ type: "list", data: [{ type: "person", data: id(3) }, { type: "person", data: id(9) }] })).toBe("Ada, Former member");
    expect(text({ type: "list", data: [{ type: "number", data: 1 }, { type: "number", data: 2 }] })).toBe("1, 2");
  });
});

// ----- import ---------------------------------------------------------------------------------

describe("the import of a formula", () => {
  async function importSidecar(sidecar: unknown, pages: Record<string, string> = {}) {
    const { client, transport } = make([{ name: "Home", properties: [{ name: "Estimate", kind: "number" }], pages: [{ title: "Existing" }] }]);
    const space = must((await client.spaceList())[0]);
    const started = await transport.dispatch({ type: "ImportBegin", data: { spaceId: space.id, sourceName: "" } });
    if (started.type !== "ImportStarted") {
      throw new Error("unexpected response");
    }
    const importId = started.data;
    const files = { "atomik-export.json": JSON.stringify(sidecar), ...pages };
    for (const [path, text] of Object.entries(files)) {
      await transport.dispatch({ type: "ImportAddFile", data: { importId, path, text } });
    }
    await transport.dispatch({ type: "ImportPlan", data: { importId, binaryPaths: [] } });
    let report: ImportReport | undefined;
    while (report === undefined) {
      const progress = await transport.dispatch({ type: "ImportCommit", data: { importId, maxPages: 50 } });
      if (progress.type !== "ImportProgress") {
        throw new Error("unexpected response");
      }
      report = progress.data.report ?? undefined;
    }
    return { client, space, report, schema: await client.propertySchemaGet(space.id) };
  }

  it("makes the names of the sidecar the ids of the space, each formula after the ones it reads", async () => {
    const { schema, report } = await importSidecar({
      format: 1,
      properties: [
        // The formulas come first in the file, and Total reads Score: the importer orders them.
        { name: "Total", kind: "formula", config: { expression: 'prop("Score") + length(prop("title"))' } },
        { name: "Score", kind: "formula", config: { expression: 'prop("Priority") * 10' } },
        { name: "Priority", kind: "number" },
      ],
      pages: ["A.md"],
    }, { "A.md": "---\nPriority: 2\n---\nx" });
    const priority = must(schema.find((def) => def.name === "Priority"));
    const score = must(schema.find((def) => def.name === "Score"));
    const total = must(schema.find((def) => def.name === "Total"));
    expect(score.config).toEqual({
      kind: "formula",
      expression: `prop("${priority.id}") * 10`,
      resultType: { base: "number", list: false },
    });
    expect(total.config).toEqual({
      kind: "formula",
      expression: `prop("${score.id}") + length(prop("title"))`,
      resultType: { base: "number", list: false },
    });
    expect(report.propertiesCreated).toBe(3);
  });

  it("keeps an expression that does not resolve as written, and it stays broken", async () => {
    const { client, space, schema } = await importSidecar({
      format: 1,
      properties: [
        { name: "Ghost", kind: "formula", config: { expression: 'prop("Nothing") * 2' } },
        { name: "Bad", kind: "formula", config: { expression: "1 +" } },
        // The English label of the editor is not the title in a file: only `title` is.
        { name: "Legacy", kind: "formula", config: { expression: 'prop("Name") + "!"' } },
        { name: "Empty", kind: "formula", config: { expression: "  " } },
        { name: "None", kind: "formula" },
      ],
      pages: ["A.md"],
    }, { "A.md": "x" });
    expect(schema.map((def) => def.name)).toEqual(["Tags", "Estimate", "Ghost", "Bad", "Legacy"]);
    expect(must(schema.find((def) => def.name === "Legacy")).config).toEqual({ kind: "formula", expression: 'prop("Name") + "!"' });
    expect(must(schema.find((def) => def.name === "Ghost")).config).toEqual({ kind: "formula", expression: 'prop("Nothing") * 2' });
    expect(must(schema.find((def) => def.name === "Bad")).config).toEqual({ kind: "formula", expression: "1 +" });
    // No page has a value, and the person fixes the formula in the settings.
    const page = must((await client.treeGet(space.id)).roots.find((node) => node.page.title === "A")).page;
    expect((page.properties ?? []).some(([propertyId]) => schema.find((def) => def.id === propertyId)?.kind === "formula")).toBe(false);
    const fixed = await client.propertyConfigSet(space.id, must(schema.find((def) => def.name === "Ghost")).id, {
      kind: "formula",
      expression: `prop("${must(schema.find((def) => def.name === "Estimate")).id}") * 2`,
    });
    expect(fixed.config).toMatchObject({ resultType: { base: "number", list: false } });
  });
});

// ----- move -----------------------------------------------------------------------------------

describe("the move of pages whose views name formulas", () => {
  const P = { priority: id(0x101), score: id(0x102), total: id(0x103), stage: id(0x104), unused: id(0x105) };
  const O = { todo: id(0x201), done: id(0x202) };
  const BOARD = id(0x301);

  const attrs = (): Record<string, JsonValue> => ({
    id: BOARD,
    layout: "table",
    source: { kind: "children" },
    properties: [P.score],
    views: [
      { id: "t", name: "", layout: "table", properties: [P.total] },
      {
        id: "d",
        name: "Overview",
        layout: "dashboard",
        properties: [],
        widgets: [
          { id: "w0000001", kind: "number", title: "", w: 1, h: "s", calc: { by: P.total, fn: "sum" } },
          {
            id: "w0000002",
            kind: "chart",
            title: "",
            w: 2,
            h: "m",
            chart: { type: "bar", groupBy: P.stage, measure: { fn: "sum", propertyId: P.score } },
            filter: { op: "and", items: [{ propertyId: P.stage, op: "hasOption", value: O.done }] },
          },
          { id: "w0000003", kind: "number", title: "", w: 1, h: "s", calc: { by: P.unused, fn: "sum" } },
        ],
      },
    ],
  });

  const SPACES: readonly SeedSpace[] = [
    {
      name: "Home",
      properties: [
        { id: P.priority, name: "Priority", kind: "number" },
        { id: P.score, name: "Score", kind: "formula", formula: 'prop("Priority") * 10' },
        { id: P.total, name: "Total", kind: "formula", formula: 'prop("Score") + length(prop("title"))' },
        {
          id: P.stage,
          name: "Stage",
          kind: "select",
          options: [
            { id: O.todo, name: "Todo" },
            { id: O.done, name: "Done" },
          ],
        },
        { id: P.unused, name: "Unused", kind: "number" },
      ],
      pages: [
        {
          title: "Board",
          content: { type: "doc", content: [{ type: "pageList", attrs: attrs() as never }] },
          children: [{ title: "Card", properties: { Priority: 3, Stage: "Done" } }],
        },
      ],
    },
    { name: "Work" },
  ];

  it("carries the formulas the views name with what they read, and the widgets follow", async () => {
    const { client } = make(SPACES);
    const [home, work] = await client.spaceList();
    const board = must((await client.treeGet(must(home).id)).roots[0]).page.id;
    const summary = await client.pageMoveToSpacePlan(board, must(work).id, null);
    expect(summary.propertiesCreated).toBe(4);
    await client.pageMoveToSpace(board, must(work).id, null, null);

    const schema = await client.propertySchemaGet(must(work).id);
    // What a formula reads is made first, then the formulas, each after the ones it reads; Unused has no value and no formula
    // reads it, so it stays behind.
    expect(schema.map((def) => def.name)).toEqual(["Tags", "Priority", "Stage", "Score", "Total"]);
    const target = (name: string) => must(schema.find((def) => def.name === name));
    expect(target("Score").config).toMatchObject({
      expression: `prop("${target("Priority").id}") * 10`,
      resultType: { base: "number", list: false },
    });
    expect(target("Total").config).toMatchObject({
      expression: `prop("${target("Score").id}") + length(prop("title"))`,
      resultType: { base: "number", list: false },
    });
    // (The title a formula reads is no property: it stays the title and needs nothing of the target.)
    // The moved card has the values of its formulas in the target.
    const targetRoots = (await client.treeGet(must(work).id)).roots;
    const card = must(targetRoots.flatMap((node) => [node, ...node.children]).find((node) => node.page.title === "Card")).page;
    expect(card.properties).toContainEqual([target("Score").id, { kind: "formula", value: { type: "number", data: 30 } }]);
    expect(card.properties).toContainEqual([target("Total").id, { kind: "formula", value: { type: "number", data: 34 } }]);

    const content = (await client.pageContentGet(board)).content as { content: { attrs: Record<string, JsonValue> }[] };
    const block = must(content.content[0]).attrs;
    expect(block.properties).toEqual([target("Score").id]);
    const views = block.views as Record<string, JsonValue>[];
    expect(must(views[0]).properties).toEqual([target("Total").id]);
    const widgets = must(views[1]).widgets as Record<string, JsonValue>[];
    expect(must(widgets[0]).calc).toEqual({ by: target("Total").id, fn: "sum" });
    const done = must(target("Stage").options.find((option) => option.name === "Done")).id;
    expect(must(widgets[1]).chart).toEqual({
      type: "bar",
      groupBy: target("Stage").id,
      measure: { fn: "sum", propertyId: target("Score").id },
    });
    expect(must(widgets[1]).filter).toEqual({ op: "and", items: [{ propertyId: target("Stage").id, op: "hasOption", value: done }] });
    // Unused has no counterpart: the number counts all pages again.
    expect(must(widgets[2]).calc).toEqual({ by: "title", fn: "count_all" });
  });

  it("uses the formula the target has by name, and the widgets follow the mapping", () => {
    // `remapView` and `viewPropertyIds` on their own: what a view names, and what it becomes.
    expect([...viewPropertyIds(attrs())].sort()).toEqual([P.score, P.stage, P.total, P.unused].sort());
    const map: ViewMap = {
      property: (source) => ({ [P.score]: id(0x901), [P.total]: id(0x902), [P.stage]: id(0x903) })[source],
      option: (property, option) => (property === P.stage && option === O.done ? id(0xa01) : undefined),
    };
    const out = remapView(attrs(), map);
    const widgets = (out.views as Record<string, JsonValue>[])[1]?.widgets as Record<string, JsonValue>[];
    expect(widgets[0]?.calc).toEqual({ by: id(0x902), fn: "sum" });
    expect(widgets[1]?.chart).toEqual({ type: "bar", groupBy: id(0x903), measure: { fn: "sum", propertyId: id(0x901) } });
    expect(widgets[2]?.calc).toEqual({ by: "title", fn: "count_all" });
    // A dashboard that maps to itself is unchanged.
    const identity: ViewMap = { property: (source) => source, option: (_, option) => option };
    expect(remapView(attrs(), identity)).toEqual(attrs());
  });
});
