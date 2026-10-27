// The `place` kind and the computed kinds (created and last edited time and device) in
// `MemoryTransport` (ADR-0026, amendment of 2026-10-08): the value, its rules and messages, the
// computed values read from the page, the throttled stamp of a content edit, the filters and the
// seeds. Ported from the Rust tests of `kb-model` (`tests/authorship.rs`) and `kb-core`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { ApiError } from "./generated/ApiError";
import type { PropertyValue } from "./generated/PropertyValue";
import { MemoryTransport } from "./memory";
import { EDIT_STAMP_INTERVAL_MS } from "./structure";

const T0 = 1_700_000_000_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function must<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("a value the test needs is missing");
  }
  return value;
}

async function expectApiError(promise: Promise<unknown>, expected: ApiError): Promise<void> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  expect((error as CoreError).toApiError()).toEqual(expected);
}

const invalidInput = (field: string, message: string): ApiError => ({
  code: "InvalidInput",
  message,
  details: { field },
});

const place = (name: string, lat?: number, lon?: number): PropertyValue => ({
  kind: "place",
  value: lat === undefined || lon === undefined ? { name } : { name, lat, lon },
});

async function demo() {
  let counter = 0x1000;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    workspaceName: "Home",
    spaces: [
      {
        name: "Personal",
        properties: [
          { name: "Where", kind: "place" },
          { name: "Created", kind: "created_time" },
          { name: "Edited", kind: "last_edited_time" },
          { name: "Author", kind: "created_by" },
          { name: "Editor", kind: "last_edited_by" },
        ],
        pages: [
          { title: "Cafe", properties: { Where: { name: "Café de Flore, Paris", lat: 48.854, lon: 2.333 } } },
          { title: "Home", properties: { Where: { name: "Home office" } } },
          { title: "Plain" },
        ],
      },
    ],
  });
  const client = new CoreClient(transport);
  const space = must((await client.spaceList())[0]);
  const schema = await client.propertySchemaGet(space.id);
  const property = (name: string): string => must(schema.find((def) => def.name === name)).id;
  const roots = (await client.treeGet(space.id)).roots;
  const page = (title: string): string => must(roots.find((node) => node.page.title === title)).page.id;
  const device = (await client.appStatus()).deviceId;
  return {
    transport,
    client,
    clock,
    spaceId: space.id,
    device,
    where: property("Where"),
    created: property("Created"),
    edited: property("Edited"),
    author: property("Author"),
    editor: property("Editor"),
    cafe: page("Cafe"),
    home: page("Home"),
    plain: page("Plain"),
  };
}

describe("place", () => {
  it("is stored with or without coordinates, read back through PageInfo and cleared", async () => {
    const { client, cafe, home, plain, where } = await demo();
    expect((await client.pageGet(cafe)).properties).toContainEqual([where, place("Café de Flore, Paris", 48.854, 2.333)]);
    expect(await client.pagePropertiesGet(home)).toContainEqual([where, place("Home office")]);
    await client.pagePropertySet(plain, where, place("South pole", -90, -180));
    expect(await client.pagePropertiesGet(plain)).toContainEqual([where, place("South pole", -90, -180)]);
    await client.pagePropertySet(plain, where, null);
    expect(await client.pagePropertiesGet(plain)).not.toContainEqual([where, expect.anything()]);
  });

  it("follows the rules of the core, with its messages", async () => {
    const { client, plain, where } = await demo();
    const refuse = (value: PropertyValue, message: string): Promise<void> =>
      expectApiError(client.pagePropertySet(plain, where, value), invalidInput("value", message));
    await refuse(place(""), "must not be empty");
    await refuse(place("   "), "must not be empty");
    await refuse(place("x".repeat(501)), "must be at most 500 characters");
    await refuse(place("a\nb"), "must not contain control characters");
    await refuse({ kind: "place", value: { name: "x", lat: 1 } }, "the latitude and the longitude go together");
    await refuse({ kind: "place", value: { name: "x", lon: 1 } }, "the latitude and the longitude go together");
    await refuse(place("x", 90.5, 0), "the latitude must be between -90 and 90");
    await refuse(place("x", -91, 0), "the latitude must be between -90 and 90");
    await refuse(place("x", Number.NaN, 0), "the latitude must be between -90 and 90");
    await refuse(place("x", 0, 180.5), "the longitude must be between -180 and 180");
    await refuse(place("x", 0, Number.NEGATIVE_INFINITY), "the longitude must be between -180 and 180");
    await refuse({ kind: "place", value: "x" } as unknown as PropertyValue, "the value is malformed");
    await client.pagePropertySet(plain, where, place("é".repeat(500), 90, 180));
  });

  it("is searched by its name, filtered by text and HasValue, and cannot be compared", async () => {
    const { client, spaceId, where, cafe, home, plain } = await demo();
    expect((await client.search("Flore")).map((hit) => hit.pageId)).toEqual([cafe]);
    expect(await client.pagesByProperty(spaceId, where, { type: "TextContains", data: "paris" })).toEqual([cafe]);
    expect(await client.pagesByProperty(spaceId, where, { type: "TextContains", data: "OFFICE" })).toEqual([home]);
    expect(await client.pagesByProperty(spaceId, where, { type: "HasValue" })).toEqual([cafe, home]);
    await expectApiError(
      client.pagesByProperty(spaceId, where, { type: "Equals", data: place("Home office") }),
      invalidInput("filter", "a place cannot be compared; use TextContains or HasValue"),
    );
    await client.pagePropertySet(plain, where, place("Paris Gare du Nord"));
    expect(await client.pagesByProperty(spaceId, where, { type: "TextContains", data: "paris" })).toEqual([cafe, plain]);
  });
});

describe("computed kinds", () => {
  it("are read from the page, in schema order, in PageInfo and the tree", async () => {
    const { client, spaceId, cafe, created, edited, author, editor, device, where } = await demo();
    const expected = [
      [where, place("Café de Flore, Paris", 48.854, 2.333)],
      [created, { kind: "created_time", value: T0 }],
      [edited, { kind: "last_edited_time", value: T0 }],
      [author, { kind: "created_by", value: device }],
      [editor, { kind: "last_edited_by", value: device }],
    ];
    expect((await client.pageGet(cafe)).properties).toEqual(expected);
    expect(await client.pagePropertiesGet(cafe)).toEqual(expected);
    const node = must((await client.treeGet(spaceId)).roots.find((n) => n.page.id === cafe));
    expect(node.page.properties).toEqual(expected);
  });

  it("are also named in PageInfo, without a property: createdBy and editedBy, in the tree too (ADR-0040)", async () => {
    const { client, clock, spaceId, plain, device } = await demo();
    const info = await client.pageGet(plain);
    expect(info.createdBy).toBe(device);
    expect(info.editedBy).toBe(device);
    clock.now = T0 + 5_000;
    await client.pageRename(plain, "Plain, renamed");
    const renamed = await client.pageGet(plain);
    expect(renamed).toMatchObject({ createdBy: device, editedBy: device, createdAt: T0, updatedAt: T0 + 5_000 });
    const node = must((await client.treeGet(spaceId)).roots.find((n) => n.page.id === plain));
    expect(node.page).toEqual(renamed);
    // A page made later is attributed to the device too.
    const made = await client.pageCreate({ spaceId, title: "Made later" });
    expect(made).toMatchObject({ createdBy: device, editedBy: device });
  });

  it("refuse values (InvalidInput for the property) and add no search text", async () => {
    const { client, plain, created, author } = await demo();
    for (const [property, value] of [
      [created, { kind: "created_time", value: 5 }],
      [author, { kind: "created_by", value: id(1) }],
      [created, null],
    ] as const) {
      await expectApiError(
        client.pagePropertySet(plain, property, value),
        invalidInput("propertyId", "this property is computed from the page and cannot be set"),
      );
    }
    expect((await client.search("created")).length).toBe(0);
  });

  it("follow the last edit: a rename moves the time, the device is the editor", async () => {
    const { client, clock, plain, edited, created } = await demo();
    clock.now = T0 + 5_000;
    await client.pageRename(plain, "Plain, renamed");
    const values = await client.pagePropertiesGet(plain);
    expect(values).toContainEqual([edited, { kind: "last_edited_time", value: T0 + 5_000 }]);
    expect(values).toContainEqual([created, { kind: "created_time", value: T0 }]);
  });

  it("are filtered by HasValue, Equals and InstantBetween", async () => {
    const { client, clock, spaceId, plain, home, cafe, created, edited, author, editor, device } = await demo();
    clock.now = T0 + 10_000;
    await client.pageRename(plain, "Plain");
    await client.pageRename(plain, "Plain 2");
    const find = (property: string, filter: Parameters<typeof client.pagesByProperty>[2]) =>
      client.pagesByProperty(spaceId, property, filter);
    expect(await find(created, { type: "HasValue" })).toEqual([cafe, home, plain]);
    expect(await find(author, { type: "Equals", data: { kind: "created_by", value: device } })).toEqual([
      cafe,
      home,
      plain,
    ]);
    expect(await find(editor, { type: "Equals", data: { kind: "last_edited_by", value: id(99) } })).toEqual([]);
    expect(await find(edited, { type: "InstantBetween", data: { from: T0 + 1 } })).toEqual([plain]);
    expect(await find(edited, { type: "InstantBetween", data: { to: T0 } })).toEqual([cafe, home]);
    expect(await find(created, { type: "InstantBetween", data: { from: T0, to: T0 } })).toEqual([cafe, home, plain]);
    await expectApiError(
      find(edited, { type: "InstantBetween", data: { from: 5, to: 4 } }),
      invalidInput("filter", "from must not be after to"),
    );
    await expectApiError(
      find(author, { type: "InstantBetween", data: {} }),
      invalidInput("filter", "instants can only be searched in a created or last edited time property"),
    );
    await expectApiError(
      find(edited, { type: "DateBetween", data: { from: "2026-10-01" } }),
      invalidInput("filter", "dates can only be searched in a date property"),
    );
    await expectApiError(
      find(edited, { type: "Equals", data: { kind: "created_by", value: device } }),
      invalidInput("filter", "the value does not match the kind of the property"),
    );
  });

  it("cannot be seeded with a value", () => {
    expect(
      () =>
        new MemoryTransport({
          spaces: [
            {
              name: "S",
              properties: [{ name: "Created", kind: "created_time" }],
              pages: [{ title: "P", properties: { Created: "x" } }],
            },
          ],
        }),
    ).toThrow(/computed/u);
  });
});

describe("backups", () => {
  it("keep a place, the schema with the computed kinds and the device that made a page", async () => {
    const key = Uint8Array.from({ length: 32 }, (_, index) => index * 7);
    const files = new Map<string, Uint8Array>();
    const spaces = [
      {
        name: "Personal",
        properties: [
          { name: "Where", kind: "place" as const },
          { name: "Author", kind: "created_by" as const },
          { name: "Created", kind: "created_time" as const },
        ],
        pages: [{ title: "Cafe", properties: { Where: { name: "Café", lat: 48.85, lon: 2.35 } } }],
      },
    ];
    let counter = 0x2000;
    const source = new MemoryTransport({
      newId: () => id(++counter),
      now: () => T0,
      workspaceName: "Home",
      spaces,
      files,
      security: { kitCreatedAt: T0 - 10_000, kitKey: key },
    });
    const client = new CoreClient(source);
    const space = must((await client.spaceList())[0]);
    const cafe = must((await client.treeGet(space.id)).roots[0]).page.id;
    const before = await client.pagePropertiesGet(cafe);
    expect(before).toHaveLength(3);
    await client.backupExport("C:/backups/place.atomikkb");

    const fresh = new CoreClient(new MemoryTransport({ files, now: () => T0 + 1_000 }));
    const { kitWordsFromEntropy } = await import("./kit");
    await fresh.backupImport("C:/backups/place.atomikkb", (await kitWordsFromEntropy(key)).join(" "));
    expect(await fresh.pagePropertiesGet(cafe)).toEqual(before);
    expect(await fresh.pagesByProperty(space.id, must(before[0])[0], { type: "TextContains", data: "caf" })).toEqual([cafe]);
  });
});

describe("the stamp of a content edit", () => {
  it("moves the last edit at most once per interval per device and announces the structure", async () => {
    const { transport, client, clock, spaceId, plain, edited } = await demo();
    const events: string[] = [];
    transport.subscribe((event) => events.push(event.type));
    const editedAt = async (): Promise<unknown> =>
      (await client.pagePropertiesGet(plain)).find(([property]) => property === edited)?.[1];

    // The first edit by this device is stamped at once: the page was made here, so within the
    // interval of its creation it is not.
    clock.now = T0 + 1_000;
    transport.changePage(plain, (doc) => doc.getMap("x").set("a", 1), "Editor");
    expect(await editedAt()).toEqual({ kind: "last_edited_time", value: T0 });
    expect(events).not.toContain("StructureChanged");

    clock.now = T0 + EDIT_STAMP_INTERVAL_MS;
    transport.changePage(plain, (doc) => doc.getMap("x").set("a", 2), "Editor");
    expect(await editedAt()).toEqual({ kind: "last_edited_time", value: T0 + EDIT_STAMP_INTERVAL_MS });
    expect(events.filter((type) => type === "StructureChanged")).toHaveLength(1);
    expect(events.indexOf("PageContentChanged")).toBeLessThan(events.indexOf("StructureChanged"));

    // Inside the interval of the last stamp: nothing.
    clock.now = T0 + EDIT_STAMP_INTERVAL_MS + 59_999;
    transport.changePage(plain, (doc) => doc.getMap("x").set("a", 3), "Editor");
    expect(await editedAt()).toEqual({ kind: "last_edited_time", value: T0 + EDIT_STAMP_INTERVAL_MS });
    expect(events.filter((type) => type === "StructureChanged")).toHaveLength(1);

    // A remote change is not a local edit: it stamps nothing (the other device did).
    clock.now = T0 + 10 * EDIT_STAMP_INTERVAL_MS;
    transport.changePage(plain, (doc) => doc.getMap("x").set("a", 4), "Remote");
    expect(await editedAt()).toEqual({ kind: "last_edited_time", value: T0 + EDIT_STAMP_INTERVAL_MS });
    expect((await client.pageGet(plain)).updatedAt).toBe(T0 + EDIT_STAMP_INTERVAL_MS);
    expect(spaceId).toBeDefined();
  });
});
