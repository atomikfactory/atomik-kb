// Files and checklists in `MemoryTransport` (ADR-0026, amendment of 2026-10-07): the values and
// their rules, the attachments a files value references (retention, trash, purge, deleting the
// property, undo, backups), the text index, the filters and the seeds. Ported from the Rust tests
// of `kb-core` (`tests_properties.rs`) and `kb-model` with the same codes, fields and messages.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { ApiError } from "./generated/ApiError";
import type { ChecklistItem } from "./generated/ChecklistItem";
import type { Event } from "./generated/Event";
import type { PropertyFilter } from "./generated/PropertyFilter";
import type { PropertyValue } from "./generated/PropertyValue";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";

const DAY = 24 * 60 * 60 * 1000;
const T0 = 1_700_000_000_000;
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index * 5);
const PATH = "C:/backups/files.atomikkb";

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

const FILE_A = "a7000000-0000-4000-8000-0000000000a1";
const FILE_B = "a7000000-0000-4000-8000-0000000000a2";
const FILE_C = "a7000000-0000-4000-8000-0000000000a3";
const ABSENT = "a7000000-0000-4000-8000-0000000000ff";

const bytes = (text: string): Uint8Array<ArrayBuffer> => new TextEncoder().encode(text);

const files = (...ids: string[]): PropertyValue => ({ kind: "files", value: ids });
const item = (n: number, text: string, done = false): ChecklistItem => ({
  id: `c1000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
  text,
  done,
});
const checklist = (...items: ChecklistItem[]): PropertyValue => ({ kind: "checklist", value: items });

function must<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("a value the test needs is missing");
  }
  return value;
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

async function expectApiError(promise: Promise<unknown>, expected: ApiError): Promise<void> {
  expect((await failure(promise)).toApiError()).toEqual(expected);
}

const invalidInput = (field: string, message: string): ApiError => ({
  code: "InvalidInput",
  message,
  details: { field },
});

/**
 * A space "Personal" with a files property "Attached", a checklist "To do" and a text "Notes", the
 * pages "Trip", "Other" and "Plain", and three attachments (`FILE_A` to `FILE_C`); a clock the test
 * moves.
 */
async function demo(options: MemoryTransportOptions = {}) {
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
          { name: "Attached", kind: "files" },
          { name: "To do", kind: "checklist" },
          { name: "Notes", kind: "text" },
        ],
        pages: [{ title: "Trip" }, { title: "Other" }, { title: "Plain" }],
      },
    ],
    attachments: [
      { attachmentId: FILE_A, name: "Quarterly report.pdf", mime: "application/pdf", bytes: bytes("plan") },
      { attachmentId: FILE_B, name: "Route map.png", mime: "image/png", bytes: bytes("map") },
      { attachmentId: FILE_C, name: "notes.txt", mime: "text/plain", bytes: bytes("notes") },
    ],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const space = must((await client.spaceList())[0]);
  const schema = await client.propertySchemaGet(space.id);
  const property = (name: string): string => must(schema.find((def) => def.name === name)).id;
  const roots = (await client.treeGet(space.id)).roots;
  const page = (title: string): string => must(roots.find((node) => node.page.title === title)).page.id;
  const info = (attachmentId: string) => client.attachmentGet(attachmentId);
  return {
    transport,
    client,
    events,
    clock,
    spaceId: space.id,
    attached: property("Attached"),
    todo: property("To do"),
    notes: property("Notes"),
    trip: page("Trip"),
    other: page("Other"),
    plain: page("Plain"),
    info,
    take: (): Event[] => events.splice(0),
  };
}

describe("values", () => {
  it("are set, read in schema order through PageInfo, written with canonical ids and cleared", async () => {
    const { client, trip, attached, todo } = await demo();
    const list = checklist(item(1, "Pack", true), item(2, "Call Ada"));
    await client.pagePropertySet(trip, attached, files(FILE_B.toUpperCase(), FILE_A.replaceAll("-", "")));
    const answered = await client.pagePropertySet(trip, todo, list);
    expect(answered).toEqual([
      [attached, files(FILE_B, FILE_A)],
      [todo, list],
    ]);
    expect((await client.pageGet(trip)).properties).toEqual(answered);
    expect(await client.pagePropertiesGet(trip)).toEqual(answered);

    // Answers share nothing with the stored values.
    const value = must(answered[1])[1];
    (value as { value: ChecklistItem[] }).value[0] = item(9, "tampered");
    expect(await client.pagePropertiesGet(trip)).toEqual([
      [attached, files(FILE_B, FILE_A)],
      [todo, list],
    ]);

    // An empty list is no value.
    await client.pagePropertySet(trip, attached, files());
    expect(await client.pagePropertiesGet(trip)).toEqual([[todo, list]]);
    await client.pagePropertySet(trip, todo, null);
    expect(await client.pagePropertiesGet(trip)).toEqual([]);
    await client.pagePropertySet(trip, todo, checklist());
    expect(await client.pagePropertiesGet(trip)).toEqual([]);
  });

  it("accept an attachment that does not exist and refuse what breaks a rule, with the core's messages", async () => {
    const { client, trip, attached, todo, notes, take } = await demo();
    await client.pagePropertySet(trip, attached, files(ABSENT, FILE_A));
    expect(await client.pagePropertiesGet(trip)).toEqual([[attached, files(ABSENT, FILE_A)]]);
    take();

    const refuse = (property: string, value: PropertyValue, message: string): Promise<void> =>
      expectApiError(client.pagePropertySet(trip, property, value), invalidInput("value", message));
    await refuse(attached, files(FILE_A, FILE_B, FILE_A), "attachment ids must be distinct");
    await refuse(attached, files(FILE_A, FILE_A.toUpperCase()), "attachment ids must be distinct");
    const many = Array.from({ length: 51 }, (_, n) => `a7000000-0000-4000-8000-${String(n).padStart(12, "0")}`);
    await refuse(attached, files(...many), "must list at most 50 files");
    await client.pagePropertySet(trip, attached, files(...many.slice(0, 50)));
    await refuse(attached, checklist(item(1, "x")), "the value does not match the kind of the property");
    await refuse(todo, files(FILE_A), "the value does not match the kind of the property");
    await refuse(notes, files(FILE_A), "the value does not match the kind of the property");
    await refuse(todo, checklist(item(1, "a"), item(1, "b")), "item ids must be distinct");
    await refuse(todo, checklist(item(1, "x".repeat(501))), "an item must be at most 500 characters");
    await refuse(todo, checklist(item(1, "two\nlines")), "an item must not contain control characters");
    const items = Array.from({ length: 201 }, (_, n) => item(n, "x"));
    await refuse(todo, checklist(...items), "must hold at most 200 items");
    await client.pagePropertySet(trip, todo, checklist(...items.slice(0, 200)));
    // A blank text is a valid item.
    await client.pagePropertySet(trip, todo, checklist(item(1, "", true)));
    expect(must((await client.pagePropertiesGet(trip))[1])[1]).toEqual(checklist(item(1, "", true)));
    // A malformed value is malformed, never a crash.
    await expectApiError(
      client.pagePropertySet(trip, todo, { kind: "checklist", value: [{ id: "nope", text: "a", done: false }] }),
      invalidInput("value", "the value is malformed"),
    );
    await expectApiError(
      client.pagePropertySet(trip, attached, { kind: "files", value: ["not an id"] }),
      invalidInput("value", "the value is malformed"),
    );
  });

  it("survive a backup, with the references and the names they bring", async () => {
    const files_ = new Map<string, Uint8Array>();
    const source = await demo({ files: files_, security: { kitCreatedAt: T0 - 10_000, kitKey: KEY } });
    const { client, trip, attached, todo, spaceId } = source;
    const list = checklist(item(1, "Pack", true), item(2, "Call Ada"));
    await client.pagePropertySet(trip, attached, files(FILE_C, FILE_A));
    await client.pagePropertySet(trip, todo, list);
    const expected = await client.pagePropertiesGet(trip);
    await client.backupExport(PATH);

    const fresh = new CoreClient(new MemoryTransport({ files: files_, now: () => T0 + 5 * DAY }));
    await fresh.backupImport(PATH, (await kitWordsFromEntropy(KEY)).join(" "));
    await expect(fresh.pagePropertiesGet(trip)).resolves.toEqual(expected);
    await expect(fresh.pagesByProperty(spaceId, attached, { type: "HasValue" })).resolves.toEqual([trip]);
    await expect(fresh.attachmentGet(FILE_A)).resolves.toMatchObject({ references: 1 });
    expect(await fresh.search("quarterly")).toHaveLength(1);
    expect(await fresh.search("ada")).toHaveLength(1);
  });
});

describe("the attachments a files value references", () => {
  it("are kept while a value lists them, and become collectable when a change drops them", async () => {
    const { client, clock, info, trip, attached, spaceId, take } = await demo();
    // Seeded and unreferenced: the clock of both started when the transport did.
    await expect(info(FILE_A)).resolves.toMatchObject({ references: 0, unreferencedSince: T0 });
    take();

    await client.pagePropertySet(trip, attached, files(FILE_A, FILE_B));
    for (const attachment of [FILE_A, FILE_B]) {
      const seen = await info(attachment);
      expect(seen.references, attachment).toBe(1);
      expect(seen.unreferencedSince, attachment).toBeUndefined();
    }
    // The bookkeeping announces the attachments before the structure.
    expect(take()).toEqual([
      { type: "AttachmentsChanged", data: { spaceId } },
      { type: "StructureChanged", data: { spaceId } },
    ]);

    // Far past the retention: referenced files stay, an unreferenced one goes.
    clock.now = T0 + 40 * DAY;
    expect(await client.attachmentGc()).toMatchObject({ purged: 1 });
    await expect(info(FILE_C)).rejects.toBeInstanceOf(CoreError);
    expect(await info(FILE_A)).toMatchObject({ references: 1 });

    // Taking one out of the value starts its clock; putting it back stops it.
    await client.pagePropertySet(trip, attached, files(FILE_A));
    await expect(info(FILE_B)).resolves.toMatchObject({ references: 0, unreferencedSince: T0 + 40 * DAY });
    await client.pagePropertySet(trip, attached, files(FILE_A, FILE_B));
    expect((await info(FILE_B)).unreferencedSince).toBeUndefined();
    await client.pagePropertySet(trip, attached, files(FILE_A));
    clock.now = T0 + 70 * DAY;
    expect(await client.attachmentGc()).toMatchObject({ purged: 0 });
    clock.now = T0 + 71 * DAY + 1;
    expect(await client.attachmentGc()).toMatchObject({ purged: 1 });
    await expect(info(FILE_B)).rejects.toBeInstanceOf(CoreError);
    expect(await info(FILE_A)).toMatchObject({ references: 1 });

    // Clearing the value releases the last file.
    await client.pagePropertySet(trip, attached, null);
    await expect(info(FILE_A)).resolves.toMatchObject({ references: 0, unreferencedSince: T0 + 71 * DAY + 1 });
  });

  it("count once per page and property, together with the blocks, and stay while the page is in the trash", async () => {
    const { client, info, trip, other, attached, spaceId } = await demo();
    const second = await client.propertyCreate(spaceId, "More files", "files");
    await client.pagePropertySet(trip, attached, files(FILE_A, FILE_B));
    await client.pagePropertySet(trip, second.id, files(FILE_A));
    await client.pagePropertySet(other, attached, files(FILE_A));
    expect((await info(FILE_A)).references).toBe(3);
    expect((await info(FILE_B)).references).toBe(1);
    await client.pageTrash(other);
    expect((await info(FILE_A)).references).toBe(3);
    await client.pagePurge(other);
    expect((await info(FILE_A)).references).toBe(2);
    // A purge that is undone brings the references back with the page.
    await client.structureUndo(spaceId);
    expect((await info(FILE_A)).references).toBe(3);
  });

  it("are released at once when the page is purged or the property is deleted", async () => {
    const { client, clock, info, trip, other, attached, spaceId } = await demo();
    const second = await client.propertyCreate(spaceId, "More files", "files");
    await client.pagePropertySet(trip, attached, files(FILE_A, FILE_C));
    await client.pagePropertySet(other, second.id, files(FILE_B, FILE_C));
    clock.now = T0 + DAY;
    await client.pageTrash(other);
    await client.pagePurge(other);
    await expect(info(FILE_B)).resolves.toMatchObject({ references: 0, unreferencedSince: T0 + DAY });
    expect(await info(FILE_C)).toMatchObject({ references: 1 });
    clock.now = T0 + 2 * DAY;
    await client.propertyDelete(spaceId, attached);
    for (const attachment of [FILE_A, FILE_C]) {
      await expect(info(attachment)).resolves.toMatchObject({ references: 0, unreferencedSince: T0 + 2 * DAY });
    }
    clock.now = T0 + 40 * DAY;
    expect(await client.attachmentGc()).toMatchObject({ purged: 3 });
  });

  it("are rebuilt by IndexRebuild without change, and the seeded ones count from the start", async () => {
    const client = new CoreClient(
      new MemoryTransport({
        now: () => T0,
        workspaceName: "Home",
        spaces: [
          {
            name: "Personal",
            properties: [{ name: "Attached", kind: "files" }],
            pages: [{ title: "Trip", properties: { Attached: [FILE_B] } }],
          },
        ],
        attachments: [
          { attachmentId: FILE_A, name: "a.bin", mime: "application/octet-stream", bytes: bytes("a") },
          { attachmentId: FILE_B, name: "b.bin", mime: "application/octet-stream", bytes: bytes("b") },
        ],
      }),
    );
    const seeded = await client.attachmentGet(FILE_B);
    expect(seeded.references).toBe(1);
    expect(seeded.unreferencedSince).toBeUndefined();
    expect((await client.attachmentGet(FILE_A)).unreferencedSince).toBe(T0);
    await client.indexRebuild();
    const rebuilt = await client.attachmentGet(FILE_B);
    expect(rebuilt.references).toBe(1);
    expect(rebuilt.unreferencedSince).toBeUndefined();
  });
});

describe("search", () => {
  it("finds a page by the name of a file it lists, as a page-level hit, and forgets a purged file", async () => {
    const { client, trip, attached } = await demo();
    await client.pagePropertySet(trip, attached, files(FILE_B, FILE_A));
    const hits = await client.search("quarterly");
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({ pageId: trip, snippet: "Route map.png, Quarterly report.pdf" });
    expect(hits[0]?.blockId).toBeUndefined();
    expect(await client.search("quarterly", { scope: "Titles" })).toEqual([]);
    expect(await client.search("route")).toHaveLength(1);

    await client.attachmentDelete(FILE_A);
    expect(await client.search("quarterly")).toEqual([]);
    expect(await client.search("route")).toHaveLength(1);
    expect(await client.pagePropertiesGet(trip)).toEqual([[attached, files(FILE_B, FILE_A)]]);

    await client.pagePropertySet(trip, attached, null);
    expect(await client.search("route")).toEqual([]);
  });

  it("finds a page by the text of a checklist item and follows the list", async () => {
    const { client, trip, todo } = await demo();
    const list = checklist(item(1, "Pack the charger", true), item(2, ""), item(3, "Call Ada"));
    await client.pagePropertySet(trip, todo, list);
    const hits = await client.search("charger");
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({ pageId: trip, snippet: "Pack the charger, Call Ada" });
    expect(hits[0]?.blockId).toBeUndefined();
    expect(await client.search("ada")).toHaveLength(1);
    await client.pagePropertySet(trip, todo, checklist(item(3, "Call Ada", true)));
    expect(await client.search("charger")).toEqual([]);
    expect(await client.search("ada")).toHaveLength(1);
    await client.pagePropertySet(trip, todo, null);
    expect(await client.search("ada")).toEqual([]);
  });
});

describe("PagesByProperty", () => {
  it("answers HasValue for files and checklists and ChecklistComplete for checklists", async () => {
    const { client, spaceId, trip, other, attached, todo } = await demo();
    await client.pagePropertySet(trip, todo, checklist(item(1, "a", true), item(2, "b", true)));
    await client.pagePropertySet(other, todo, checklist(item(1, "a", true), item(2, "b")));
    await client.pagePropertySet(trip, attached, files(FILE_A));
    const trashed = await client.pageCreate({ spaceId, title: "Trashed" });
    await client.pagePropertySet(trashed.id, todo, checklist(item(1, "a", true)));
    await client.pageTrash(trashed.id);

    const has: PropertyFilter = { type: "HasValue" };
    const complete = (data: boolean): PropertyFilter => ({ type: "ChecklistComplete", data });
    await expect(client.pagesByProperty(spaceId, todo, has)).resolves.toEqual([other, trip]);
    await expect(client.pagesByProperty(spaceId, attached, has)).resolves.toEqual([trip]);
    await expect(client.pagesByProperty(spaceId, todo, complete(true))).resolves.toEqual([trip]);
    await expect(client.pagesByProperty(spaceId, todo, complete(false))).resolves.toEqual([other]);
    await client.pagePropertySet(other, todo, checklist(item(2, "b", true)));
    await expect(client.pagesByProperty(spaceId, todo, complete(true))).resolves.toEqual([other, trip]);
  });

  it("refuses the filters that do not fit, with the core's messages", async () => {
    const { client, spaceId, attached, todo, notes } = await demo();
    const refused = (property: string, filter: PropertyFilter, message: string): Promise<void> =>
      expectApiError(client.pagesByProperty(spaceId, property, filter), invalidInput("filter", message));
    const completion = "completion can only be searched in a checklist property";
    await refused(notes, { type: "ChecklistComplete", data: true }, completion);
    await refused(attached, { type: "ChecklistComplete", data: false }, completion);
    const textOnly = "text can only be searched in a text, URL, email, phone or place property";
    await refused(todo, { type: "TextContains", data: "a" }, textOnly);
    await refused(attached, { type: "TextContains", data: "a" }, textOnly);
    await refused(todo, { type: "Checked", data: true }, "a checked state can only be searched in a checkbox property");
    await refused(todo, { type: "ContainsOption", data: id(1) }, "this kind of property has no options");
    const cannot = "a list of files or items cannot be compared; use HasValue (or ChecklistComplete for a checklist)";
    await refused(todo, { type: "Equals", data: checklist(item(1, "a")) }, cannot);
    await refused(attached, { type: "Equals", data: files(FILE_A) }, cannot);
    // An empty list is no value: nothing equals it (the value is checked before the filter).
    await expect(client.pagesByProperty(spaceId, attached, { type: "Equals", data: files() })).resolves.toEqual([]);
    await refused(attached, { type: "Equals", data: files(FILE_A, FILE_A) }, "attachment ids must be distinct");
    const mismatch = "the value does not match the kind of the property";
    await refused(attached, { type: "Equals", data: checklist(item(1, "a")) }, mismatch);
  });
});

describe("seeding", () => {
  const SPACE = {
    name: "Personal",
    properties: [
      { name: "Attached", kind: "files" },
      { name: "To do", kind: "checklist" },
    ],
  } as const;
  const ATTACHMENTS = [
    { attachmentId: FILE_A, name: "a.bin", mime: "application/octet-stream", bytes: bytes("a") },
    { attachmentId: FILE_C, name: "c.bin", mime: "application/octet-stream", bytes: bytes("c") },
  ];

  it("creates files and checklists without events, and an item draws an id unless it has one", async () => {
    let counter = 0x2000;
    const transport = new MemoryTransport({
      newId: () => id(++counter),
      now: () => T0,
      workspaceName: "Home",
      spaces: [
        {
          ...SPACE,
          pages: [
            {
              title: "Trip",
              properties: {
                Attached: [FILE_A, FILE_C.toUpperCase()],
                "To do": [{ text: "Pack", done: true }, { text: "Call Ada", id: "c1000000-0000-4000-8000-000000000042" }],
              },
            },
          ],
        },
      ],
      attachments: ATTACHMENTS,
    });
    const client = new CoreClient(transport);
    const events: Event[] = [];
    transport.subscribe((event) => events.push(event));
    const space = must((await client.spaceList())[0]);
    const trip = must((await client.treeGet(space.id)).roots[0]).page;
    const values = trip.properties ?? [];
    expect(values.map(([, value]) => value.kind)).toEqual(["files", "checklist"]);
    expect(values[0]?.[1]).toEqual(files(FILE_A, FILE_C));
    const list = must(values[1])[1];
    if (list.kind !== "checklist") {
      throw new Error("a checklist was seeded");
    }
    expect(list.value.map((entry) => [entry.text, entry.done])).toEqual([
      ["Pack", true],
      ["Call Ada", false],
    ]);
    expect(list.value[1]?.id).toBe("c1000000-0000-4000-8000-000000000042");
    expect(list.value[0]?.id.startsWith("00000000-0000-4000-8000-")).toBe(true);
    expect(events).toEqual([]);
    expect((await client.attachmentGet(FILE_A)).references).toBe(1);
  });

  it("refuses a seed that does not fit the kind, when the transport is built", () => {
    const build = (properties: Record<string, unknown>): (() => unknown) => () =>
      new MemoryTransport({
        spaces: [{ ...SPACE, pages: [{ title: "P", properties: properties as never }] }],
      });
    expect(build({ Attached: FILE_A })).toThrow(/does not fit its kind \(files\)/u);
    expect(build({ Attached: ["nope"] })).toThrow(/does not fit its kind \(files\)/u);
    expect(build({ "To do": [{ done: true }] })).toThrow(/does not fit its kind \(checklist\)/u);
    expect(build({ "To do": "x" })).toThrow(/does not fit its kind \(checklist\)/u);
    expect(build({ Attached: [FILE_A, FILE_A] })).toThrow(/distinct/u);
  });
});
