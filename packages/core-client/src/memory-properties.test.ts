// Typed page properties (ADR-0026) in `MemoryTransport`: the schema, the values, the queries,
// events, search, undo, backups, seeding and sync, ported from the Rust tests of the core
// (`kb-core`'s `tests_properties.rs` and `kb-model`'s `tests/properties.rs`) with the same codes,
// fields and messages.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readBackupFile, readBackupState } from "./backup";
import { CoreClient } from "./client";
import { sha256, toHex } from "./digest";
import { CoreError } from "./errors";
import type { ApiError } from "./generated/ApiError";
import type { Event } from "./generated/Event";
import type { PageInfo } from "./generated/PageInfo";
import type { PageNode } from "./generated/PageNode";
import type { OptionColor } from "./generated/OptionColor";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyFilter } from "./generated/PropertyFilter";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { StatusGroup } from "./generated/StatusGroup";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { DEFAULT_STATUS_OPTIONS, TAGS_PROPERTY_ID } from "./properties";
import type { SeedPage, SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);
const PATH = "C:/backups/properties.atomikkb";

/** The ids `setup` hands out, in order: the device is 1, then everything created. */
function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

const UNKNOWN_SPACE = id(0xff);
const UNKNOWN_PROPERTY = id(0xfe);
const UNKNOWN_OPTION = id(0xfd);
const UNKNOWN_PAGE = id(0xfc);

/** Deterministic ids and clock (every read of the clock advances it by one millisecond). */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  let clock = T0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock++,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  /** The events so far, forgotten afterwards. */
  const take = (): Event[] => events.splice(0);
  return { transport, client, events, take };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** Awaits a rejection and compares it with the `ApiError` the core would answer. */
async function expectApiError(promise: Promise<unknown>, expected: ApiError): Promise<void> {
  expect((await failure(promise)).toApiError()).toEqual(expected);
}

const invalidInput = (field: string, message: string): ApiError => ({
  code: "InvalidInput",
  message,
  details: { field },
});
const notFound = (message: string): ApiError => ({ code: "NotFound", message });

const SCHEMA_CHANGED = (spaceId: SpaceId): Event => ({ type: "PropertySchemaChanged", data: { spaceId } });
const STRUCTURE_CHANGED = (spaceId: SpaceId): Event => ({ type: "StructureChanged", data: { spaceId } });

const text = (value: string): PropertyValue => ({ kind: "text", value });
const number = (value: number): PropertyValue => ({ kind: "number", value });
const multi = (...ids: string[]): PropertyValue => ({ kind: "multi_select", value: ids });
const select = (value: string): PropertyValue => ({ kind: "select", value });
const status = (value: string): PropertyValue => ({ kind: "status", value });
const checkbox = (value: boolean): PropertyValue => ({ kind: "checkbox", value });
const day = (date: string, time?: string): PropertyValue => ({
  kind: "date",
  value: time === undefined ? { date } : { date, time },
});

function must<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("a value the test needs is missing");
  }
  return value;
}

/** One space "Personal" (no pages) and helpers to make pages in it. */
async function demo(options: MemoryTransportOptions = {}) {
  const ctx = setup({ spaces: [{ name: "Personal" }], ...options });
  const space = must((await ctx.client.spaceList())[0]);
  const makePage = async (title: string, parentId?: string, index?: number): Promise<string> => {
    const page = await ctx.client.pageCreate({
      spaceId: space.id,
      title,
      ...(parentId === undefined ? {} : { parentId }),
      ...(index === undefined ? {} : { index }),
    });
    return page.id;
  };
  return { ...ctx, spaceId: space.id, makePage };
}

const names = (schema: PropertyDef[]): string[] => schema.map((def) => def.name);
const idsOfValues = (values: [PropertyId, PropertyValue][]): string[] => values.map(([property]) => property);

// ----- schema ---------------------------------------------------------------------------------

describe("the schema", () => {
  it("lists Tags first, as a virtual multi-select without options, in a new space", async () => {
    const { client, spaceId } = await demo();
    await expect(client.propertySchemaGet(spaceId)).resolves.toStrictEqual([
      { id: TAGS_PROPERTY_ID, name: "Tags", kind: "multi_select", options: [], createdAt: 0 },
    ]);
    await expectApiError(client.propertySchemaGet(UNKNOWN_SPACE), notFound("space not found"));
  });

  it("creates, renames, moves and deletes properties, saying so before the structure changed", async () => {
    const { client, spaceId, take } = await demo();
    const notes = await client.propertyCreate(spaceId, "  Notes  ", "text");
    expect(notes).toMatchObject({ name: "Notes", kind: "text", options: [] });
    expect(notes.createdAt).toBeGreaterThan(T0);
    expect(take()).toEqual([SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]);

    const stage = await client.propertyCreate(spaceId, "Stage", "status");
    const cost = await client.propertyCreate(spaceId, "Cost", "number");
    take();
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Notes", "Stage", "Cost"]);

    const renamed = await client.propertyRename(spaceId, notes.id, "Remarks");
    expect(renamed).toEqual({ ...notes, name: "Remarks" });
    await client.propertyMove(spaceId, cost.id, stage.id);
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Remarks", "Cost", "Stage"]);
    await client.propertyMove(spaceId, cost.id);
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Remarks", "Stage", "Cost"]);
    expect(take()).toEqual(Array.from({ length: 3 }, () => [SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]).flat());

    await expect(client.propertyDelete(spaceId, stage.id)).resolves.toBeUndefined();
    expect(take()).toEqual([SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]);
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Remarks", "Cost"]);
  });

  it("answers copies: changing an answer changes nothing in the schema", async () => {
    const { client, spaceId } = await demo();
    const kind = await client.propertyCreate(spaceId, "Kind", "select");
    await client.propertyOptionAdd(spaceId, kind.id, "red", "red");
    const answer = await client.propertySchemaGet(spaceId);
    must(answer[1]).name = "tampered";
    must(answer[1]).options.length = 0;
    const again = await client.propertySchemaGet(spaceId);
    expect(must(again[1]).name).toBe("Kind");
    expect(must(again[1]).options).toHaveLength(1);
  });

  it("refuses what does not fit with the core's codes, fields and messages, and emits nothing", async () => {
    const { client, spaceId, take } = await demo();
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const kind = await client.propertyCreate(spaceId, "Kind", "select");
    const red = await client.propertyOptionAdd(spaceId, kind.id, "red", "red");
    take();

    const cases: [string, () => Promise<unknown>, ApiError][] = [
      ["create, empty name", () => client.propertyCreate(spaceId, "", "text"), invalidInput("name", "must not be empty")],
      ["create, blank name", () => client.propertyCreate(spaceId, "   ", "text"), invalidInput("name", "must not be empty")],
      [
        "create, control character",
        () => client.propertyCreate(spaceId, "tab\there", "text"),
        invalidInput("name", "must not contain control characters"),
      ],
      [
        "create, long name",
        () => client.propertyCreate(spaceId, "x".repeat(101), "text"),
        invalidInput("name", "must be at most 100 characters"),
      ],
      [
        "create, a bad name wins over an unknown space",
        () => client.propertyCreate(UNKNOWN_SPACE, " ", "text"),
        invalidInput("name", "must not be empty"),
      ],
      ["create, unknown space", () => client.propertyCreate(UNKNOWN_SPACE, "x", "text"), notFound("space not found")],
      ["rename, blank name", () => client.propertyRename(spaceId, notes.id, " "), invalidInput("name", "must not be empty")],
      ["rename, unknown property", () => client.propertyRename(spaceId, UNKNOWN_PROPERTY, "x"), notFound("property not found")],
      ["rename, unknown space", () => client.propertyRename(UNKNOWN_SPACE, notes.id, "x"), notFound("space not found")],
      ["delete, unknown property", () => client.propertyDelete(spaceId, UNKNOWN_PROPERTY), notFound("property not found")],
      [
        "delete Tags",
        () => client.propertyDelete(spaceId, TAGS_PROPERTY_ID),
        invalidInput("propertyId", "the Tags property cannot be deleted"),
      ],
      [
        "move Tags",
        () => client.propertyMove(spaceId, TAGS_PROPERTY_ID),
        invalidInput("propertyId", "the Tags property always comes first"),
      ],
      ["move, unknown property", () => client.propertyMove(spaceId, UNKNOWN_PROPERTY), notFound("property not found")],
      ["move before an unknown one", () => client.propertyMove(spaceId, notes.id, UNKNOWN_PROPERTY), notFound("property not found")],
      [
        "option on a text property",
        () => client.propertyOptionAdd(spaceId, notes.id, "x", "gray"),
        invalidInput("propertyId", "this kind of property has no options"),
      ],
      [
        "option, unknown property",
        () => client.propertyOptionAdd(spaceId, UNKNOWN_PROPERTY, "x", "gray"),
        notFound("property not found"),
      ],
      [
        "option with a group on a select",
        () => client.propertyOptionAdd(spaceId, kind.id, "x", "gray", "done"),
        invalidInput("group", "only status options have a group"),
      ],
      [
        "option, blank name",
        () => client.propertyOptionAdd(spaceId, kind.id, "  ", "gray"),
        invalidInput("name", "must not be empty"),
      ],
      [
        "option update, unknown option",
        () => client.propertyOptionUpdate(spaceId, kind.id, UNKNOWN_OPTION, { name: "x" }),
        notFound("option not found"),
      ],
      [
        "option update, unknown property",
        () => client.propertyOptionUpdate(spaceId, UNKNOWN_PROPERTY, red.id, { name: "x" }),
        notFound("property not found"),
      ],
      [
        "option update, group on a select",
        () => client.propertyOptionUpdate(spaceId, kind.id, red.id, { group: "done" }),
        invalidInput("group", "only status options have a group"),
      ],
      [
        "option update, blank name",
        () => client.propertyOptionUpdate(spaceId, kind.id, red.id, { name: " " }),
        invalidInput("name", "must not be empty"),
      ],
      [
        "option delete, unknown option",
        () => client.propertyOptionDelete(spaceId, kind.id, UNKNOWN_OPTION),
        notFound("option not found"),
      ],
      [
        "option delete, unknown property",
        () => client.propertyOptionDelete(spaceId, UNKNOWN_PROPERTY, red.id),
        notFound("property not found"),
      ],
    ];
    for (const [label, call, expected] of cases) {
      expect((await failure(call())).toApiError(), label).toEqual(expected);
    }
    expect(take()).toEqual([]);
    await expect(client.propertySchemaGet(spaceId)).resolves.toHaveLength(3);
  });

  it("refuses a kind, colour or group a typed client cannot send, as InvalidInput", async () => {
    const { client, spaceId } = await demo();
    const pick = await client.propertyCreate(spaceId, "Kind", "select");
    await expect(client.propertyCreate(spaceId, "x", "table" as PropertyKind)).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "kind" },
    });
    await expect(client.propertyOptionAdd(spaceId, pick.id, "x", "teal" as OptionColor)).rejects.toMatchObject({
      details: { field: "color" },
    });
    await expect(client.propertyOptionAdd(spaceId, pick.id, "x", "gray", "later" as StatusGroup)).rejects.toMatchObject({
      details: { field: "group" },
    });
  });

  it("keeps a property per space", async () => {
    const { client, spaceId } = await demo();
    const other = await client.spaceCreate("Work");
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await expect(client.propertySchemaGet(spaceId)).resolves.toHaveLength(2);
    await expect(client.propertySchemaGet(other.id)).resolves.toHaveLength(1);
    const elsewhere = (await client.pageCreate({ spaceId: other.id, title: "Elsewhere" })).id;
    await expectApiError(client.pagePropertySet(elsewhere, notes.id, text("x")), notFound("property not found"));
    await expectApiError(
      client.pagesByProperty(other.id, notes.id, { type: "HasValue" }),
      notFound("property not found"),
    );
  });

  it("counts Tags among the 100 properties a space holds", async () => {
    const { client, spaceId, take } = await demo();
    for (let n = 0; n < 99; n += 1) {
      await client.propertyCreate(spaceId, `p${String(n)}`, "text");
    }
    await expect(client.propertySchemaGet(spaceId)).resolves.toHaveLength(100);
    take();
    await expectApiError(
      client.propertyCreate(spaceId, "one too many", "text"),
      invalidInput("spaceId", "a space holds at most 100 properties"),
    );
    expect(take()).toEqual([]);
    // A name of exactly 100 characters is fine.
    const other = await client.spaceCreate("Other");
    await expect(client.propertyCreate(other.id, "x".repeat(100), "text")).resolves.toMatchObject({
      name: "x".repeat(100),
    });
  });

  it("starts a status property with three grouped options that have fresh, distinct ids", async () => {
    const { client, spaceId } = await demo();
    const stage = await client.propertyCreate(spaceId, "Stage", "status");
    expect(stage.options.map(({ name, group, color }) => ({ name, group, color }))).toEqual(DEFAULT_STATUS_OPTIONS);
    expect(new Set(stage.options.map((option) => option.id)).size).toBe(3);
    const pick = await client.propertyCreate(spaceId, "Pick", "select");
    expect(pick.options).toEqual([]);
    const other = await client.propertyCreate(spaceId, "Other stage", "status");
    expect(other.options.map((option) => option.id)).not.toContain(must(stage.options[0]).id);
  });

  it("moves in front of another property, to the end, before Tags and not at all", async () => {
    const { client, spaceId, take } = await demo();
    const created: Record<string, string> = {};
    for (const name of ["A", "B", "C", "D"]) {
      created[name] = (await client.propertyCreate(spaceId, name, "text")).id;
    }
    const [a, b, c, d] = [must(created.A), must(created.B), must(created.C), must(created.D)];
    const order = async (): Promise<string> => (await client.propertySchemaGet(spaceId)).map((def) => def.name).join("");
    expect(await order()).toBe("TagsABCD");
    await client.propertyMove(spaceId, d, b);
    expect(await order()).toBe("TagsADBC");
    await client.propertyMove(spaceId, a);
    expect(await order()).toBe("TagsDBCA");
    await client.propertyMove(spaceId, c, TAGS_PROPERTY_ID);
    expect(await order()).toBe("TagsCDBA");
    take();
    // Already in place, to itself and to the end while last: nothing changes, all are accepted and announced.
    await client.propertyMove(spaceId, c, d);
    await client.propertyMove(spaceId, a);
    await client.propertyMove(spaceId, b, b);
    expect(await order()).toBe("TagsCDBA");
    expect(take()).toEqual(Array.from({ length: 3 }, () => [SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]).flat());
  });

  it("writes Tags on its first option (its creation time is then the write time) and lets it be renamed", async () => {
    const { client, spaceId } = await demo();
    await client.propertyCreate(spaceId, "Notes", "text");
    const virtual = must((await client.propertySchemaGet(spaceId))[0]);
    expect(virtual.createdAt).toBe(0);
    const urgent = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "urgent", "red");
    const written = must((await client.propertySchemaGet(spaceId))[0]);
    expect(written).toMatchObject({ id: TAGS_PROPERTY_ID, name: "Tags", kind: "multi_select", options: [urgent] });
    expect(written.createdAt).toBeGreaterThan(T0);

    const renamed = await client.propertyRename(spaceId, TAGS_PROPERTY_ID, "Labels");
    expect(renamed.name).toBe("Labels");
    expect(renamed.createdAt).toBe(written.createdAt);
    expect(must((await client.propertySchemaGet(spaceId))[0]).id).toBe(TAGS_PROPERTY_ID);
    await expectApiError(
      client.propertyDelete(spaceId, TAGS_PROPERTY_ID),
      invalidInput("propertyId", "the Tags property cannot be deleted"),
    );
  });

  it("writes Tags when a virtual one is renamed, even to the same name, but not when something is refused", async () => {
    const { client, spaceId } = await demo();
    await expectApiError(
      client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, " ", "red"),
      invalidInput("name", "must not be empty"),
    );
    await expectApiError(
      client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "x", "red", "done"),
      invalidInput("group", "only status options have a group"),
    );
    expect(must((await client.propertySchemaGet(spaceId))[0]).createdAt).toBe(0);
    await expectApiError(
      client.propertyOptionUpdate(spaceId, TAGS_PROPERTY_ID, UNKNOWN_OPTION, { name: "x" }),
      notFound("option not found"),
    );
    expect(must((await client.propertySchemaGet(spaceId))[0]).createdAt).toBe(0);
    await client.propertyRename(spaceId, TAGS_PROPERTY_ID, "Tags");
    expect(must((await client.propertySchemaGet(spaceId))[0]).createdAt).toBeGreaterThan(T0);
  });
});

// ----- options --------------------------------------------------------------------------------

describe("options", () => {
  it("are added last, updated in place with their ids stable, and may share a name", async () => {
    const { client, spaceId, take } = await demo();
    const kind = await client.propertyCreate(spaceId, "Kind", "select");
    const one = await client.propertyOptionAdd(spaceId, kind.id, "One", "gray");
    const two = await client.propertyOptionAdd(spaceId, kind.id, "Two", "gray");
    const three = await client.propertyOptionAdd(spaceId, kind.id, "  Three ", "pink");
    expect(three).toEqual({ id: three.id, name: "Three", color: "pink" });
    expect("group" in three).toBe(false);
    const twin = await client.propertyOptionAdd(spaceId, kind.id, "One", "blue");
    expect(twin.id).not.toBe(one.id);
    take();

    const updated = await client.propertyOptionUpdate(spaceId, kind.id, two.id, { name: " Deux ", color: "yellow" });
    expect(updated).toEqual({ id: two.id, name: "Deux", color: "yellow" });
    expect(take()).toEqual([SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]);
    const listed = must((await client.propertySchemaGet(spaceId))[1]).options;
    expect(listed.map((option) => option.name)).toEqual(["One", "Deux", "Three", "One"]);
    // An update that changes nothing is accepted (and announced).
    await expect(client.propertyOptionUpdate(spaceId, kind.id, two.id, {})).resolves.toEqual(updated);
    await expect(client.propertyOptionUpdate(spaceId, kind.id, two.id, { name: "Deux" })).resolves.toEqual(updated);
    expect(take()).toHaveLength(4);
  });

  it("give a status option a group (todo unless given) that can be changed", async () => {
    const { client, spaceId } = await demo();
    const stage = await client.propertyCreate(spaceId, "Stage", "status");
    const blocked = await client.propertyOptionAdd(spaceId, stage.id, "Blocked", "red");
    expect(blocked.group).toBe("todo");
    const moved = await client.propertyOptionUpdate(spaceId, stage.id, blocked.id, { group: "in_progress" });
    expect(moved.group).toBe("in_progress");
    const shipped = await client.propertyOptionAdd(spaceId, stage.id, "Shipped", "green", "done");
    expect(shipped.group).toBe("done");
    expect(must((await client.propertySchemaGet(spaceId))[1]).options).toHaveLength(5);
  });

  it("are limited to 500 per property", async () => {
    const { client, spaceId } = await demo();
    const kind = await client.propertyCreate(spaceId, "Kind", "select");
    for (let n = 0; n < 500; n += 1) {
      await client.propertyOptionAdd(spaceId, kind.id, `o${String(n)}`, "gray");
    }
    await expectApiError(
      client.propertyOptionAdd(spaceId, kind.id, "one too many", "gray"),
      invalidInput("propertyId", "a property holds at most 500 options"),
    );
    expect(must((await client.propertySchemaGet(spaceId))[1]).options).toHaveLength(500);
  });

  it("go from every value that had them, in one change, and set those pages' updatedAt", async () => {
    const { client, spaceId, makePage, take } = await demo();
    const a = await makePage("A");
    const b = await makePage("B");
    const c = await makePage("C");
    const trashed = await makePage("Trashed");
    const untouched = await makePage("Untouched");
    const kind = await client.propertyCreate(spaceId, "Kind", "select");
    const red = await client.propertyOptionAdd(spaceId, kind.id, "red", "red");
    const green = await client.propertyOptionAdd(spaceId, kind.id, "green", "green");
    const x = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "x", "gray");
    const y = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "y", "gray");
    await client.pagePropertySet(a, kind.id, select(red.id));
    await client.pagePropertySet(b, kind.id, select(green.id));
    await client.pagePropertySet(trashed, kind.id, select(red.id));
    await client.pagePropertySet(a, TAGS_PROPERTY_ID, multi(x.id, y.id));
    await client.pagePropertySet(c, TAGS_PROPERTY_ID, multi(x.id));
    await client.pagePropertySet(untouched, TAGS_PROPERTY_ID, multi(y.id));
    await client.pageTrash(trashed);
    const timeOf = async (page: string): Promise<number> => (await client.pageGet(page)).updatedAt;
    const [bBefore, untouchedBefore] = [await timeOf(b), await timeOf(untouched)];
    take();

    // A select option: the pages that had it lose the value, a trashed page too.
    await client.propertyOptionDelete(spaceId, kind.id, red.id);
    expect(take()).toEqual([SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]);
    expect(idsOfValues(await client.pagePropertiesGet(a))).toEqual([TAGS_PROPERTY_ID]);
    await expect(client.pagePropertiesGet(trashed)).resolves.toEqual([]);
    await expect(client.pagePropertiesGet(b)).resolves.toEqual([[kind.id, select(green.id)]]);
    expect(await timeOf(a)).toBeGreaterThan(bBefore);
    expect(await timeOf(trashed)).toBeGreaterThan(bBefore);
    expect(await timeOf(b)).toBe(bBefore);
    expect(must((await client.propertySchemaGet(spaceId))[1]).options).toEqual([
      { id: green.id, name: "green", color: "green" },
    ]);

    // A multi-select option: removed from the list, and a list left empty is cleared.
    const beforeTags = await timeOf(c);
    await client.propertyOptionDelete(spaceId, TAGS_PROPERTY_ID, x.id);
    await expect(client.pagePropertiesGet(a)).resolves.toEqual([[TAGS_PROPERTY_ID, multi(y.id)]]);
    await expect(client.pagePropertiesGet(c)).resolves.toEqual([]);
    expect(await timeOf(c)).toBeGreaterThan(beforeTags);
    await expect(client.pagePropertiesGet(untouched)).resolves.toEqual([[TAGS_PROPERTY_ID, multi(y.id)]]);
    expect(await timeOf(untouched)).toBe(untouchedBefore);
    expect(must((await client.propertySchemaGet(spaceId))[0]).options).toEqual([{ id: y.id, name: "y", color: "gray" }]);
    await expect(
      client.pagesByProperty(spaceId, TAGS_PROPERTY_ID, { type: "ContainsOption", data: x.id }),
    ).resolves.toEqual([]);

    // An option no page has changes the schema only.
    const idle = await client.propertyOptionAdd(spaceId, kind.id, "idle", "gray");
    take();
    await client.propertyOptionDelete(spaceId, kind.id, idle.id);
    expect(take()).toEqual([SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]);
  });
});

// ----- values ---------------------------------------------------------------------------------

/** A page with one property of every kind (and a Tags option), none set yet. */
async function everyKind() {
  const ctx = await demo();
  const { client, spaceId } = ctx;
  const page = await ctx.makePage("Plan");
  const create = async (name: string, kind: Parameters<CoreClient["propertyCreate"]>[2]) =>
    client.propertyCreate(spaceId, name, kind);
  const notes = await create("Notes", "text");
  const cost = await create("Cost", "number");
  const due = await create("Due", "date");
  const done = await create("Done", "checkbox");
  const link = await create("Link", "url");
  const stage = await create("Stage", "status");
  const email = await create("Contact", "email");
  const phone = await create("Phone", "phone");
  const kind = await create("Kind", "select");
  const red = await client.propertyOptionAdd(spaceId, kind.id, "red", "red");
  const tag = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "urgent", "red");
  ctx.take();
  return { ...ctx, page, notes, cost, due, done, link, stage, email, phone, kind, red, tag };
}

describe("values", () => {
  it("are set, read in schema order through every page read, and cleared", async () => {
    const w = await everyKind();
    const { client, spaceId, page } = w;
    await client.pagePropertySet(page, w.notes.id, text("Quarterly plan"));
    await client.pagePropertySet(page, w.cost.id, number(12.5));
    await client.pagePropertySet(page, w.due.id, day("2026-10-07", "14:30"));
    await client.pagePropertySet(page, w.done.id, checkbox(true));
    await client.pagePropertySet(page, w.link.id, { kind: "url", value: "https://example.com" });
    await client.pagePropertySet(page, w.stage.id, status(must(w.stage.options[1]).id));
    await client.pagePropertySet(page, w.email.id, { kind: "email", value: "ada@example.com" });
    const all = await client.pagePropertySet(page, w.phone.id, { kind: "phone", value: "+1 555 0100" });
    expect(all).toHaveLength(8);
    await client.pagePropertySet(page, w.kind.id, select(w.red.id));
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(w.tag.id));

    const read = await client.pagePropertiesGet(page);
    expect(idsOfValues(read)).toEqual([
      TAGS_PROPERTY_ID,
      w.notes.id,
      w.cost.id,
      w.due.id,
      w.done.id,
      w.link.id,
      w.stage.id,
      w.email.id,
      w.phone.id,
      w.kind.id,
    ]);
    expect(read[1]).toEqual([w.notes.id, text("Quarterly plan")]);
    expect(read[3]).toEqual([w.due.id, day("2026-10-07", "14:30")]);

    // PageInfo carries the same list: PageGet and TreeGet.
    const info = await client.pageGet(page);
    expect(info.properties).toEqual(read);
    const tree = await client.treeGet(spaceId);
    expect(must(tree.roots[0]).page.properties).toEqual(read);

    // Clearing, and the empty values.
    await client.pagePropertySet(page, w.cost.id, null);
    await client.pagePropertySet(page, w.notes.id, text(""));
    await client.pagePropertySet(page, w.link.id, { kind: "url", value: "" });
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi());
    const left = await client.pagePropertiesGet(page);
    expect(idsOfValues(left)).toEqual([w.due.id, w.done.id, w.stage.id, w.email.id, w.phone.id, w.kind.id]);
    expect((await client.pageGet(page)).properties).toHaveLength(6);
  });

  it("keep a checkbox that is off and a number that is zero (they are values)", async () => {
    const w = await everyKind();
    await w.client.pagePropertySet(w.page, w.done.id, checkbox(false));
    await w.client.pagePropertySet(w.page, w.cost.id, number(0));
    await expect(w.client.pagePropertiesGet(w.page)).resolves.toEqual([
      [w.cost.id, number(0)],
      [w.done.id, checkbox(false)],
    ]);
  });

  it("keep a multi-select in the order given", async () => {
    const { client, spaceId, makePage } = await demo();
    const page = await makePage("A");
    const added: Record<string, string> = {};
    for (const name of ["one", "two", "three"]) {
      added[name] = (await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, name, "gray")).id;
    }
    const one = must(added.one);
    const two = must(added.two);
    const three = must(added.three);
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(three, one));
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[TAGS_PROPERTY_ID, multi(three, one)]]);
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(one, two, three));
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[TAGS_PROPERTY_ID, multi(one, two, three)]]);
  });

  it("are refused with the field value and the core's message, and nothing is written or announced", async () => {
    const w = await everyKind();
    const { client, page } = w;
    const hundredOne = Array.from({ length: 101 }, (_, n) => id(0x1000 + n));
    const refusals: [string, string, PropertyValue, string][] = [
      ["text gets a number", w.notes.id, number(1), "the value does not match the kind of the property"],
      ["long text", w.notes.id, text("x".repeat(10_001)), "must be at most 10000 characters"],
      [
        "control character",
        w.notes.id,
        text("bell\u0007"),
        "must not contain control characters other than tabs and line breaks",
      ],
      ["unknown option", w.kind.id, select(UNKNOWN_OPTION), "the value names an unknown option"],
      ["status value on a select", w.kind.id, status(w.red.id), "the value does not match the kind of the property"],
      ["multi value on a select", w.kind.id, multi(w.red.id), "the value does not match the kind of the property"],
      ["another property's option on Tags", TAGS_PROPERTY_ID, multi(w.red.id), "the value names an unknown option"],
      ["duplicate options", TAGS_PROPERTY_ID, multi(w.tag.id, w.tag.id), "option ids must be distinct"],
      ["too many options", TAGS_PROPERTY_ID, multi(...hundredOne), "must list at most 100 options"],
      ["not a real day", w.due.id, day("2026-02-30"), "the date must be a real day as YYYY-MM-DD"],
      ["not a real time", w.due.id, day("2026-10-07", "25:00"), "the time must be HH:mm (24 hours)"],
      ["NaN", w.cost.id, number(Number.NaN), "must be a finite number"],
      ["Infinity", w.cost.id, number(Number.POSITIVE_INFINITY), "must be a finite number"],
      [
        "a URL without a scheme",
        w.link.id,
        { kind: "url", value: "example.com" },
        "must be an absolute http, https, mailto or tel URL without spaces",
      ],
      [
        "a javascript URL",
        w.link.id,
        { kind: "url", value: "javascript:alert(1)" },
        "must be an absolute http, https, mailto or tel URL without spaces",
      ],
      ["an empty email", w.email.id, { kind: "email", value: "" }, "must not be empty"],
      ["a blank email", w.email.id, { kind: "email", value: "   " }, "must not be empty"],
      ["a long email", w.email.id, { kind: "email", value: "a".repeat(255) }, "must be at most 254 characters"],
      ["a multi-line email", w.email.id, { kind: "email", value: "a\nb" }, "must not contain control characters"],
      ["a long phone", w.phone.id, { kind: "phone", value: "1".repeat(65) }, "must be at most 64 characters"],
    ];
    for (const [label, property, value, message] of refusals) {
      expect((await failure(client.pagePropertySet(page, property, value))).toApiError(), label).toEqual(
        invalidInput("value", message),
      );
    }
    await expectApiError(client.pagePropertySet(page, UNKNOWN_PROPERTY, text("x")), notFound("property not found"));
    await expectApiError(client.pagePropertySet(UNKNOWN_PAGE, w.notes.id, null), notFound("page not found"));
    await expectApiError(client.pagePropertiesGet(UNKNOWN_PAGE), notFound("page not found"));
    expect(w.take()).toEqual([]);
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([]);
  });

  it("accept the largest values the limits allow", async () => {
    const w = await everyKind();
    const { client, page } = w;
    await client.pagePropertySet(page, w.notes.id, text("\u{1F600}".repeat(10_000)));
    await client.pagePropertySet(page, w.email.id, { kind: "email", value: "a".repeat(254) });
    await client.pagePropertySet(page, w.phone.id, { kind: "phone", value: "1".repeat(64) });
    const url = `https://example.com/${"a".repeat(2048 - 20)}`;
    await client.pagePropertySet(page, w.link.id, { kind: "url", value: url });
    await client.pagePropertySet(page, w.cost.id, number(-1.5e300));
    expect(await client.pagePropertiesGet(page)).toHaveLength(5);
  });

  it("answer values that share nothing with the stored ones", async () => {
    const { client, spaceId, makePage } = await demo();
    const page = await makePage("A");
    const tag = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "t", "gray");
    const sent = multi(tag.id);
    const answer = await client.pagePropertySet(page, TAGS_PROPERTY_ID, sent);
    (sent as { value: string[] }).value.push("tampered");
    must(answer[0])[1] = text("tampered");
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[TAGS_PROPERTY_ID, multi(tag.id)]]);
  });

  it("set the page's updatedAt only when something changed, and announce the structure either way", async () => {
    const { client, spaceId, makePage, take } = await demo();
    const page = await makePage("Plan");
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const other = await client.propertyCreate(spaceId, "Other", "text");
    const created = await client.pageGet(page);
    take();
    await client.pagePropertySet(page, notes.id, text("x"));
    const first = await client.pageGet(page);
    expect(first.updatedAt).toBeGreaterThan(created.updatedAt);
    expect(first.createdAt).toBe(created.createdAt);
    // Only the structure changed: no schema event.
    expect(take()).toEqual([STRUCTURE_CHANGED(spaceId)]);

    // The same value again writes nothing (no new time) but is still announced.
    await client.pagePropertySet(page, notes.id, text("x"));
    expect(take()).toEqual([STRUCTURE_CHANGED(spaceId)]);
    expect((await client.pageGet(page)).updatedAt).toBe(first.updatedAt);

    await client.pagePropertySet(page, notes.id, text("y"));
    expect((await client.pageGet(page)).updatedAt).toBeGreaterThan(first.updatedAt);
    // Clearing a value that is not there changes nothing either.
    const before = (await client.pageGet(page)).updatedAt;
    await client.pagePropertySet(page, other.id, null);
    expect((await client.pageGet(page)).updatedAt).toBe(before);
    // A list is the same value only in the same order.
    const a = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "a", "gray");
    const b = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "b", "gray");
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(a.id, b.id));
    const listed = (await client.pageGet(page)).updatedAt;
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(a.id, b.id));
    expect((await client.pageGet(page)).updatedAt).toBe(listed);
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(b.id, a.id));
    expect((await client.pageGet(page)).updatedAt).toBeGreaterThan(listed);
  });

  it("show up in the PageInfo of every answer that carries one, and nowhere when there are none", async () => {
    const { client, spaceId, makePage } = await demo();
    const parent = await makePage("Parent");
    const child = await makePage("Child", parent);
    const tag = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "t", "gray");
    await client.pagePropertySet(child, TAGS_PROPERTY_ID, multi(tag.id));
    const expected: [PropertyId, PropertyValue][] = [[TAGS_PROPERTY_ID, multi(tag.id)]];

    expect((await client.pageGet(child)).properties).toEqual(expected);
    const view = await client.treeGet(spaceId);
    const root = must(view.roots[0]);
    expect("properties" in root.page).toBe(false);
    expect(must(root.children[0]).page.properties).toEqual(expected);
    expect((await client.pageRename(child, "Renamed")).properties).toEqual(expected);
    expect((await client.pageSetIcon(child, "📌")).properties).toEqual(expected);

    // Inside a trashed subtree, as the top of one, and after a restore.
    await client.pageTrash(parent);
    const trashed = await client.treeGet(spaceId);
    expect(trashed.trashed).toHaveLength(1);
    expect("properties" in must(trashed.trashed[0]).page).toBe(false);
    expect((await client.pageGet(child)).properties).toEqual(expected);
    const restored = await client.pageRestore(parent);
    expect("properties" in restored).toBe(false);
    await client.pageTrash(child);
    const trashedChild = must((await client.treeGet(spaceId)).trashed[0]);
    expect(trashedChild.page.properties).toEqual(expected);
    expect((await client.pageRestore(child)).properties).toEqual(expected);
    // A new page has none, and the key is left out.
    const fresh: PageInfo = await client.pageCreate({ spaceId, title: "Fresh" });
    expect("properties" in fresh).toBe(false);
  });

  it("belong to trashed pages too: they can be edited in the trash and come back with a restore", async () => {
    const { client, spaceId, makePage } = await demo();
    const page = await makePage("Plan");
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await client.pagePropertySet(page, notes.id, text("kept"));
    await client.pageTrash(page);
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[notes.id, text("kept")]]);
    await client.pagePropertySet(page, notes.id, text("edited while trashed"));
    expect((await client.pageGet(page)).properties).toEqual([[notes.id, text("edited while trashed")]]);
    await client.pageRestore(page);
    const view = await client.treeGet(spaceId);
    expect(must(view.roots[0]).page.properties).toEqual([[notes.id, text("edited while trashed")]]);
  });

  it("of a deleted property are unread at once and dropped by the next write of that page's values", async () => {
    const files = new Map<string, Uint8Array>();
    const { client, transport, spaceId, makePage } = await demo({
      files,
      workspaceName: "Home",
      security: { kitCreatedAt: T0 - 1000, kitKey: KEY },
    });
    const a = await makePage("A");
    const b = await makePage("B");
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const cost = await client.propertyCreate(spaceId, "Cost", "number");
    await client.pagePropertySet(a, notes.id, text("old"));
    await client.pagePropertySet(a, cost.id, number(5));
    await client.pagePropertySet(b, notes.id, text("also old"));
    await client.propertyDelete(spaceId, notes.id);

    // Unread: PageInfo, PagePropertiesGet and every query.
    await expect(client.pagePropertiesGet(a)).resolves.toEqual([[cost.id, number(5)]]);
    expect((await client.pageGet(b)).properties).toBeUndefined();
    await expectApiError(
      client.pagesByProperty(spaceId, notes.id, { type: "HasValue" }),
      notFound("property not found"),
    );
    expect(await client.search("old")).toEqual([]);

    // Still stored: a backup carries the dangling values.
    const stored = async (): Promise<string[]> => {
      await client.backupExport(PATH);
      const { body } = await readBackupFile(must(transport.files.get(PATH)));
      const state = readBackupState(body);
      return state.spaces.flatMap((space) =>
        space.pages.flatMap((page) => (page.props ?? []).map(([property]) => `${page.id}:${property}`)),
      );
    };
    expect(await stored()).toEqual([`${a}:${notes.id}`, `${a}:${cost.id}`, `${b}:${notes.id}`]);

    // The next real write of a page's values drops its dangling ones; the other page keeps its.
    await client.pagePropertySet(a, cost.id, number(6));
    expect(await stored()).toEqual([`${a}:${cost.id}`, `${b}:${notes.id}`]);
    await client.pagePropertySet(b, cost.id, number(1));
    expect(await stored()).toEqual([`${a}:${cost.id}`, `${b}:${cost.id}`]);
    // A write that changes nothing cleans nothing.
    await client.pagePropertySet(a, cost.id, number(6));
    expect(await stored()).toEqual([`${a}:${cost.id}`, `${b}:${cost.id}`]);
  });
});

// ----- queries --------------------------------------------------------------------------------

/** Pages "Banana", "apple", "Cherry" and a trashed "Aardvark", with values of every filterable kind. */
async function world() {
  const ctx = await demo();
  const { client, spaceId, makePage } = ctx;
  const banana = await makePage("Banana");
  const apple = await makePage("apple");
  const cherry = await makePage("Cherry");
  const trashed = await makePage("Aardvark");
  const notes = (await client.propertyCreate(spaceId, "Notes", "text")).id;
  const cost = (await client.propertyCreate(spaceId, "Cost", "number")).id;
  const stage = await client.propertyCreate(spaceId, "Stage", "status");
  const kind = await client.propertyCreate(spaceId, "Kind", "select");
  const due = (await client.propertyCreate(spaceId, "Due", "date")).id;
  const done = (await client.propertyCreate(spaceId, "Done", "checkbox")).id;
  const red = (await client.propertyOptionAdd(spaceId, kind.id, "red", "red")).id;
  const urgent = (await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "urgent", "red")).id;
  const later = (await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "later", "gray")).id;
  const rows: [string, string[], string, number, boolean, PropertyValue][] = [
    [banana, [urgent, later], "Hello World", 12, true, day("2026-10-07", "14:30")],
    [apple, [urgent], "hello", 12, false, day("2026-10-07")],
    [cherry, [later], "ÉCOLE", 3.5, true, day("2026-11-01")],
    [trashed, [urgent], "hello trashed", 12, true, day("2026-10-07")],
  ];
  for (const [page, tags, note, amount, flag, when] of rows) {
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(...tags));
    await client.pagePropertySet(page, notes, text(note));
    await client.pagePropertySet(page, cost, number(amount));
    await client.pagePropertySet(page, done, checkbox(flag));
    await client.pagePropertySet(page, due, when);
  }
  await client.pagePropertySet(banana, stage.id, status(must(stage.options[1]).id));
  await client.pagePropertySet(apple, kind.id, select(red));
  await client.pageTrash(trashed);
  ctx.take();
  const query = (property: string, filter: PropertyFilter) => client.pagesByProperty(spaceId, property, filter);
  return { ...ctx, banana, apple, cherry, trashed, notes, cost, stage, kind, red, due, done, urgent, later, query };
}

describe("PagesByProperty", () => {
  it("HasValue lists the live pages by title, ignoring ASCII case, and leaves the trashed page out", async () => {
    const w = await world();
    const has: PropertyFilter = { type: "HasValue" };
    await expect(w.query(w.notes, has)).resolves.toEqual([w.apple, w.banana, w.cherry]);
    await expect(w.query(w.stage.id, has)).resolves.toEqual([w.banana]);
    await expect(w.query(w.kind.id, has)).resolves.toEqual([w.apple]);
    // A checkbox that is off has a value.
    await expect(w.query(w.done, has)).resolves.toEqual([w.apple, w.banana, w.cherry]);
  });

  it("answers every filter", async () => {
    const w = await world();
    const { apple, banana, cherry, query } = w;
    // Options.
    await expect(query(TAGS_PROPERTY_ID, { type: "ContainsOption", data: w.urgent })).resolves.toEqual([apple, banana]);
    await expect(query(TAGS_PROPERTY_ID, { type: "ContainsOption", data: w.later })).resolves.toEqual([banana, cherry]);
    // A set, in any order.
    await expect(query(TAGS_PROPERTY_ID, { type: "Equals", data: multi(w.later, w.urgent) })).resolves.toEqual([banana]);
    await expect(query(TAGS_PROPERTY_ID, { type: "Equals", data: multi(w.urgent) })).resolves.toEqual([apple]);
    const progress = must(w.stage.options[1]).id;
    await expect(query(w.stage.id, { type: "Equals", data: status(progress) })).resolves.toEqual([banana]);
    await expect(query(w.stage.id, { type: "ContainsOption", data: progress })).resolves.toEqual([banana]);
    await expect(
      query(w.stage.id, { type: "ContainsOption", data: must(w.stage.options[0]).id }),
    ).resolves.toEqual([]);
    await expect(query(w.kind.id, { type: "Equals", data: select(w.red) })).resolves.toEqual([apple]);
    await expect(query(w.kind.id, { type: "ContainsOption", data: w.red })).resolves.toEqual([apple]);
    // An option the property does not list matches nothing (it is not an error).
    await expect(query(w.kind.id, { type: "ContainsOption", data: UNKNOWN_OPTION })).resolves.toEqual([]);
    await expect(query(w.kind.id, { type: "Equals", data: select(UNKNOWN_OPTION) })).resolves.toEqual([]);

    // Text: exact and case-sensitive, or a Unicode case-insensitive substring.
    await expect(query(w.notes, { type: "Equals", data: text("hello") })).resolves.toEqual([apple]);
    await expect(query(w.notes, { type: "Equals", data: text("Hello") })).resolves.toEqual([]);
    await expect(query(w.notes, { type: "Equals", data: text("") })).resolves.toEqual([]);
    await expect(query(w.notes, { type: "TextContains", data: "HELLO" })).resolves.toEqual([apple, banana]);
    await expect(query(w.notes, { type: "TextContains", data: "école" })).resolves.toEqual([cherry]);
    await expect(query(w.notes, { type: "TextContains", data: "o w" })).resolves.toEqual([banana]);
    await expect(query(w.notes, { type: "TextContains", data: "x".repeat(200) })).resolves.toEqual([]);

    // Numbers, checkboxes, dates.
    await expect(query(w.cost, { type: "Equals", data: number(12) })).resolves.toEqual([apple, banana]);
    await expect(query(w.cost, { type: "Equals", data: number(3.5) })).resolves.toEqual([cherry]);
    await expect(query(w.done, { type: "Checked", data: true })).resolves.toEqual([banana, cherry]);
    // Off or unset, among the live pages.
    await expect(query(w.done, { type: "Checked", data: false })).resolves.toEqual([apple]);
    await expect(query(w.done, { type: "Equals", data: checkbox(false) })).resolves.toEqual([apple]);
    await expect(query(w.done, { type: "Equals", data: checkbox(true) })).resolves.toEqual([banana, cherry]);
    // No time matches only a date without one; a time matches that time.
    await expect(query(w.due, { type: "Equals", data: day("2026-10-07") })).resolves.toEqual([apple]);
    await expect(query(w.due, { type: "Equals", data: day("2026-10-07", "14:30") })).resolves.toEqual([banana]);
    await expect(query(w.due, { type: "Equals", data: day("2026-10-07", "14:31") })).resolves.toEqual([]);
    const between = (from?: string, to?: string): PropertyFilter => ({
      type: "DateBetween",
      data: { ...(from === undefined ? {} : { from }), ...(to === undefined ? {} : { to }) },
    });
    await expect(query(w.due, between("2026-10-01", "2026-10-31"))).resolves.toEqual([apple, banana]);
    await expect(query(w.due, between("2026-10-07", "2026-10-07"))).resolves.toEqual([apple, banana]);
    await expect(query(w.due, between("2026-10-08"))).resolves.toEqual([cherry]);
    await expect(query(w.due, between(undefined, "2026-10-06"))).resolves.toEqual([]);
    await expect(query(w.due, between())).resolves.toEqual([apple, banana, cherry]);
  });

  it("lists every live page for Checked(false), also the ones that never had the checkbox", async () => {
    const w = await world();
    const extra = await w.makePage("Zed");
    await expect(w.query(w.done, { type: "Checked", data: false })).resolves.toEqual([w.apple, extra]);
    await expect(w.query(w.done, { type: "HasValue" })).resolves.toEqual([w.apple, w.banana, w.cherry]);
  });

  it("refuses a filter that does not fit the property, checking the property first", async () => {
    const w = await world();
    const filterError = (message: string): ApiError => invalidInput("filter", message);
    const noOptions = "this kind of property has no options";
    const textOnly = "text can only be searched in a text, URL, email, phone or place property";
    const cases: [string, string, PropertyFilter, string][] = [
      ["Checked on text", w.notes, { type: "Checked", data: true }, "a checked state can only be searched in a checkbox property"],
      ["ContainsOption on text", w.notes, { type: "ContainsOption", data: w.urgent }, noOptions],
      ["Equals of another kind", w.notes, { type: "Equals", data: number(1) }, "the value does not match the kind of the property"],
      ["empty text", w.notes, { type: "TextContains", data: "" }, "must not be empty"],
      ["long text", w.notes, { type: "TextContains", data: "x".repeat(201) }, "must be at most 200 characters"],
      ["control characters", w.notes, { type: "TextContains", data: "a\nb" }, "must not contain control characters"],
      ["text in a number", w.cost, { type: "TextContains", data: "1" }, textOnly],
      ["text in a select", w.kind.id, { type: "TextContains", data: "red" }, textOnly],
      ["dates in a checkbox", w.done, { type: "DateBetween", data: {} }, "dates can only be searched in a date property"],
      ["a day that does not exist", w.due, { type: "DateBetween", data: { from: "2026-02-30" } }, "a date must be a real day as YYYY-MM-DD"],
      ["a day that does not exist, to", w.due, { type: "DateBetween", data: { to: "2026-13-01" } }, "a date must be a real day as YYYY-MM-DD"],
      [
        "from after to",
        w.due,
        { type: "DateBetween", data: { from: "2026-10-31", to: "2026-10-01" } },
        "from must not be after to",
      ],
      ["options in a number", w.cost, { type: "ContainsOption", data: w.urgent }, noOptions],
      ["Equals a duplicate list", TAGS_PROPERTY_ID, { type: "Equals", data: multi(w.urgent, w.urgent) }, "option ids must be distinct"],
      ["Equals NaN", w.cost, { type: "Equals", data: number(Number.NaN) }, "must be a finite number"],
      ["Equals a day that does not exist", w.due, { type: "Equals", data: day("2026-02-30") }, "the date must be a real day as YYYY-MM-DD"],
    ];
    for (const [label, property, filter, message] of cases) {
      expect((await failure(w.query(property, filter))).toApiError(), label).toEqual(filterError(message));
    }
    await expectApiError(w.query(UNKNOWN_PROPERTY, { type: "HasValue" }), notFound("property not found"));
    // The property is checked before the filter.
    await expectApiError(w.query(UNKNOWN_PROPERTY, { type: "Checked", data: true }), notFound("property not found"));
    await expectApiError(
      w.client.pagesByProperty(UNKNOWN_SPACE, w.notes, { type: "HasValue" }),
      notFound("space not found"),
    );
    await expect(w.query(w.notes, { type: "Equals", data: text("hello") })).resolves.toEqual([w.apple]);
  });

  it("matches an empty Equals of a text, URL or list to nothing, and an email or phone only exactly", async () => {
    const w = await world();
    const link = await w.client.propertyCreate(w.spaceId, "Link", "url");
    const email = await w.client.propertyCreate(w.spaceId, "Contact", "email");
    await w.client.pagePropertySet(w.apple, link.id, { kind: "url", value: "https://Example.com/A" });
    await w.client.pagePropertySet(w.banana, email.id, { kind: "email", value: "Ada@Example.com" });
    await expect(w.query(link.id, { type: "Equals", data: { kind: "url", value: "" } })).resolves.toEqual([]);
    await expect(w.query(link.id, { type: "Equals", data: { kind: "url", value: "https://Example.com/A" } })).resolves.toEqual([w.apple]);
    await expect(w.query(link.id, { type: "Equals", data: { kind: "url", value: "https://example.com/a" } })).resolves.toEqual([]);
    await expect(w.query(link.id, { type: "TextContains", data: "EXAMPLE.com/a" })).resolves.toEqual([w.apple]);
    await expectApiError(
      w.query(email.id, { type: "Equals", data: { kind: "email", value: "" } }),
      invalidInput("filter", "must not be empty"),
    );
    await expect(w.query(email.id, { type: "TextContains", data: "ada@" })).resolves.toEqual([w.banana]);
    await expect(w.query(TAGS_PROPERTY_ID, { type: "Equals", data: multi() })).resolves.toEqual([]);
  });

  it("orders by title with ASCII-only case folding, then the exact title, then the page id, by code point", async () => {
    const { client, spaceId, makePage } = await demo();
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const titles = ["zebra", "Apple", "apple", "e", "É", "é", "E", "\u{1F600} smile", "\uFF5E tilde", "Zulu", "10", "9"];
    const pages = new Map<string, string>();
    for (const title of titles) {
      const page = await makePage(title);
      pages.set(page, title);
      await client.pagePropertySet(page, notes.id, text("v"));
    }
    const listed = (await client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).map((page) => pages.get(page));
    expect(listed).toEqual([
      "10",
      "9",
      // Folded together: "Apple" and "apple" tie, the exact title decides ("A" before "a").
      "Apple",
      "apple",
      // "E" and "e" tie after folding.
      "E",
      "e",
      // "Z" folds to "z": "Zulu" after "zebra"; both before "É" and "é", which are not folded
      // (U+00C9 and U+00E9 are past every ASCII letter).
      "zebra",
      "Zulu",
      "É",
      "é",
      // U+FF5E (one UTF-16 unit) before U+1F600 (a surrogate pair), as UTF-8 bytes order them.
      "\uFF5E tilde",
      "\u{1F600} smile",
    ]);
  });

  it("breaks a tie between equal titles by page id, not by the order of the tree", async () => {
    const { client, spaceId, makePage } = await demo();
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const first = await makePage("Same");
    const second = await makePage("Same", undefined, 0);
    for (const page of [first, second]) {
      await client.pagePropertySet(page, notes.id, text("v"));
    }
    // `second` is first in the tree but has the larger id.
    expect(must((await client.treeGet(spaceId)).roots[0]).page.id).toBe(second);
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([first, second]);
  });

  it("leaves trashed pages out and brings them back with a restore, edits in the trash included", async () => {
    const w = await world();
    const urgent = (): Promise<string[]> => w.query(TAGS_PROPERTY_ID, { type: "ContainsOption", data: w.urgent });
    expect(await urgent()).not.toContain(w.trashed);
    // The trashed page still reads and takes values.
    await expect(w.client.pagePropertiesGet(w.trashed)).resolves.toHaveLength(5);
    await w.client.pagePropertySet(w.trashed, w.notes, text("edited in the trash"));
    await w.client.pageRestore(w.trashed);
    // "Aardvark" sorts first.
    expect(await urgent()).toEqual([w.trashed, w.apple, w.banana]);
    await expect(w.query(w.notes, { type: "Equals", data: text("edited in the trash") })).resolves.toEqual([w.trashed]);
  });

  it("includes the pages of a restored subtree and leaves out those below a trashed page", async () => {
    const { client, spaceId, makePage } = await demo();
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const parent = await makePage("Parent");
    const child = await makePage("Child", parent);
    await client.pagePropertySet(child, notes.id, text("v"));
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([child]);
    await client.pageTrash(parent);
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([]);
    await client.pageRestore(parent);
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([child]);
  });
});

// ----- purge and undo -------------------------------------------------------------------------

describe("purge and structural undo", () => {
  it("drop the values of a purged page from every read, and an undone purge brings them back", async () => {
    const { client, spaceId, makePage } = await demo();
    const page = await makePage("Plan");
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await client.pagePropertySet(page, notes.id, text("survives the undo"));
    await client.pageTrash(page);
    await client.pagePurge(page);
    await expectApiError(client.pagePropertiesGet(page), notFound("page not found"));
    await expectApiError(client.pagePropertySet(page, notes.id, null), notFound("page not found"));
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([]);
    expect(await client.search("survives")).toEqual([]);

    // Undoing the purge brings the page back into the trash, values included.
    await client.structureUndo(spaceId);
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[notes.id, text("survives the undo")]]);
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([]);
    await client.pageRestore(page);
    await expect(client.pagesByProperty(spaceId, notes.id, { type: "HasValue" })).resolves.toEqual([page]);
    // Redo and undo again: still there.
    await client.structureUndo(spaceId);
    await client.structureRedo(spaceId);
    await client.structureUndo(spaceId);
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[notes.id, text("survives the undo")]]);
  });

  it("never undo or redo a schema change or a value", async () => {
    const { client, spaceId, makePage } = await demo();
    const page = await makePage("Plan");
    await client.pageRename(page, "Renamed");
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await client.pagePropertySet(page, notes.id, text("v"));
    await client.pageRename(page, "Renamed again");
    // Property writes are not on the undo stack: undo takes back the rename, nothing else.
    await client.structureUndo(spaceId);
    expect((await client.pageGet(page)).title).toBe("Renamed");
    await expect(client.pagePropertiesGet(page)).resolves.toEqual([[notes.id, text("v")]]);
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Notes"]);
    await client.structureUndo(spaceId);
    await client.structureUndo(spaceId);
    await expectApiError(client.pageGet(page), notFound("page not found"));
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Notes"]);
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: false, canRedo: true });
    // Redo brings the page back as it was when its creation was undone.
    await client.structureRedo(spaceId);
    expect((await client.pageGet(page)).title).toBe("Plan");
    await expect(client.propertySchemaGet(spaceId).then(names)).resolves.toEqual(["Tags", "Notes"]);
  });

  it("put nothing on the undo stack, and clear nothing from the redo stack", async () => {
    const { client, spaceId } = await demo({ spaces: [{ name: "Personal", pages: [{ title: "Plan" }] }] });
    const page = must((await client.treeGet(spaceId)).roots[0]).page.id;
    const idle = { canUndo: false, canRedo: false };
    expect((await client.treeGet(spaceId)).undo).toEqual(idle);
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const tag = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "t", "gray");
    await client.pagePropertySet(page, notes.id, text("v"));
    await client.pagePropertySet(page, TAGS_PROPERTY_ID, multi(tag.id));
    await client.propertyOptionDelete(spaceId, TAGS_PROPERTY_ID, tag.id);
    await client.propertyMove(spaceId, notes.id);
    await client.propertyDelete(spaceId, notes.id);
    expect((await client.treeGet(spaceId)).undo).toEqual(idle);
    // With something to redo, property writes leave it redoable.
    await client.pageRename(page, "Renamed");
    await client.structureUndo(spaceId);
    await client.propertyCreate(spaceId, "More", "text");
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: false, canRedo: true });
  });

  it("go with the space: deleting it drops the schema, and a new space starts with Tags only", async () => {
    const { client, spaceId, makePage } = await demo();
    const other = await client.spaceCreate("Scratch");
    await client.propertyCreate(other.id, "Notes", "text");
    const page = (await client.pageCreate({ spaceId: other.id, title: "P" })).id;
    await client.pagePurge(page);
    await client.spaceDelete(other.id);
    await expectApiError(client.propertySchemaGet(other.id), notFound("space not found"));
    await expect(client.propertySchemaGet(spaceId)).resolves.toHaveLength(1);
    await makePage("still fine");
  });
});

// ----- events ---------------------------------------------------------------------------------

describe("events", () => {
  it("are StructureChanged only for a value, PropertySchemaChanged first for a schema change", async () => {
    const w = await everyKind();
    const { client, spaceId, page, take } = w;
    await client.propertyRename(spaceId, w.notes.id, "Remarks");
    await client.propertyOptionAdd(spaceId, w.kind.id, "green", "green");
    await client.propertyOptionUpdate(spaceId, w.kind.id, w.red.id, { color: "pink" });
    await client.propertyOptionDelete(spaceId, w.kind.id, w.red.id);
    await client.propertyMove(spaceId, w.cost.id);
    await client.propertyDelete(spaceId, w.cost.id);
    expect(take()).toEqual(Array.from({ length: 6 }, () => [SCHEMA_CHANGED(spaceId), STRUCTURE_CHANGED(spaceId)]).flat());
    await client.pagePropertySet(page, w.notes.id, text("v"));
    await client.pagePropertySet(page, w.notes.id, null);
    await client.pagePropertySet(page, w.notes.id, null);
    expect(take()).toEqual(Array.from({ length: 3 }, () => STRUCTURE_CHANGED(spaceId)));
    // Reads and queries emit nothing.
    await client.propertySchemaGet(spaceId);
    await client.pagePropertiesGet(page);
    await client.pagesByProperty(spaceId, w.notes.id, { type: "HasValue" });
    expect(take()).toEqual([]);
  });
});

describe("sync", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T0);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("counts every accepted property request as one pending entry, a schema change not twice", async () => {
    let counter = 0;
    const transport = new MemoryTransport({
      newId: () => id(++counter),
      now: () => Date.now(),
      spaces: [{ name: "Personal", pages: [{ title: "Plan" }] }],
    });
    const client = new CoreClient(transport);
    const space = must((await client.spaceList())[0]);
    const page = must((await client.treeGet(space.id)).roots[0]).page.id;
    await client.syncConfigure("https://sync.example.com");
    const pending = async (): Promise<number> => (await client.syncStatus()).pendingEntries;
    expect(await pending()).toBe(0);

    const notes = await client.propertyCreate(space.id, "Notes", "text");
    expect(await pending()).toBe(1);
    const kind = await client.propertyCreate(space.id, "Kind", "select");
    const option = await client.propertyOptionAdd(space.id, kind.id, "red", "red");
    await client.propertyOptionUpdate(space.id, kind.id, option.id, { name: "crimson" });
    await client.propertyRename(space.id, notes.id, "Remarks");
    await client.propertyMove(space.id, kind.id, notes.id);
    expect(await pending()).toBe(6);
    await client.pagePropertySet(page, notes.id, text("v"));
    expect(await pending()).toBe(7);
    // The same value again is announced (StructureChanged) and counts like the core's persist hook.
    await client.pagePropertySet(page, notes.id, text("v"));
    expect(await pending()).toBe(8);
    await client.propertyOptionDelete(space.id, kind.id, option.id);
    await client.propertyDelete(space.id, kind.id);
    expect(await pending()).toBe(10);
    // Refusals and reads count nothing.
    await expect(client.propertyDelete(space.id, TAGS_PROPERTY_ID)).rejects.toBeInstanceOf(CoreError);
    await client.propertySchemaGet(space.id);
    await client.pagePropertiesGet(page);
    await client.pagesByProperty(space.id, notes.id, { type: "HasValue" });
    expect(await pending()).toBe(10);
  });
});

// ----- search ---------------------------------------------------------------------------------

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

describe("search", () => {
  /** "Plan" has a body that says urgent, "Other" has nothing. */
  async function searchable() {
    const body: JsonValue = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: blockId(1) },
          content: [{ type: "text", text: "the body says urgent matters" }],
        },
      ],
    };
    const ctx = await demo({
      spaces: [{ name: "Personal", pages: [{ title: "Plan", content: body }, { title: "Other" }] }],
    });
    const view = await ctx.client.treeGet(ctx.spaceId);
    const plan = must(view.roots[0]).page.id;
    const other = must(view.roots[1]).page.id;
    return { ...ctx, plan, other };
  }

  it("finds a page by a tag name or a text value as a page-level hit that leaves the title and body rules alone", async () => {
    const { client, spaceId, plan, other } = await searchable();
    const urgent = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "urgent", "red");
    const work = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "work", "blue");
    await client.pagePropertySet(plan, TAGS_PROPERTY_ID, multi(urgent.id, work.id));
    await client.pagePropertySet(other, TAGS_PROPERTY_ID, multi(work.id));
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await client.pagePropertySet(other, notes.id, text("Remember the Quokka"));

    // The body block is a block hit, the tag a page-level hit with its text as the snippet,
    // listed after the block hits.
    const hits = await client.search("urgent");
    expect(hits).toHaveLength(2);
    expect(hits[0]).toMatchObject({ pageId: plan, blockId: blockId(1) });
    const tagHit = must(hits[1]);
    expect(tagHit).toMatchObject({ pageId: plan, spaceId, title: "Plan", snippet: "urgent, work" });
    expect("blockId" in tagHit).toBe(false);
    expect(tagHit.highlights).toEqual([{ start: 0, end: 6 }]);

    expect((await client.search("work")).map((hit) => hit.pageId).sort()).toEqual([other, plan].sort());
    const quokka = await client.search("quokka");
    expect(quokka).toHaveLength(1);
    expect(quokka[0]).toMatchObject({ pageId: other, snippet: "Remember the Quokka" });
    expect(await client.search("plan")).toHaveLength(1);
    // Every word must be inside one text: "urgent" and "quokka" are in different rows.
    expect(await client.search("urgent quokka")).toEqual([]);
    // Prefixes match like in a block.
    expect(await client.search("quok")).toHaveLength(1);

    // A titles-only search never matches a property.
    expect(await client.search("urgent", { scope: "Titles" })).toEqual([]);
    expect(await client.search("plan", { scope: "Titles" })).toHaveLength(1);
    // A space filter and the limit apply to property hits too.
    expect(await client.search("work", { spaceId })).toHaveLength(2);
    expect(await client.search("work", { limit: 1 })).toHaveLength(1);
  });

  it("follows renames and deletions of options, values and properties", async () => {
    const { client, spaceId, plan, other } = await searchable();
    const urgent = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "urgent", "red");
    const work = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "work", "blue");
    await client.pagePropertySet(plan, TAGS_PROPERTY_ID, multi(urgent.id, work.id));
    await client.pagePropertySet(other, TAGS_PROPERTY_ID, multi(work.id));
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await client.pagePropertySet(other, notes.id, text("Remember the Quokka"));

    await client.propertyOptionUpdate(spaceId, TAGS_PROPERTY_ID, work.id, { name: "office" });
    expect(await client.search("work")).toEqual([]);
    expect(await client.search("office")).toHaveLength(2);
    await client.propertyOptionDelete(spaceId, TAGS_PROPERTY_ID, work.id);
    expect(await client.search("office")).toEqual([]);
    // Plan's other tag and body remain.
    expect(await client.search("urgent")).toHaveLength(2);

    // Clearing a value and deleting a property both take their text out.
    await client.pagePropertySet(other, notes.id, null);
    expect(await client.search("quokka")).toEqual([]);
    await client.pagePropertySet(other, notes.id, text("Quokka again"));
    expect(await client.search("quokka")).toHaveLength(1);
    await client.propertyDelete(spaceId, notes.id);
    expect(await client.search("quokka")).toEqual([]);

    // A trashed page is not found, a restored one is.
    await client.pageTrash(plan);
    expect(await client.search("urgent")).toEqual([]);
    await client.pageRestore(plan);
    expect(await client.search("urgent")).toHaveLength(2);
  });

  it("indexes option names, text, URL, email and phone values, and never numbers, checkboxes or dates", async () => {
    const w = await everyKind();
    const { client, page } = w;
    await client.pagePropertySet(page, w.notes.id, text("alphaword"));
    await client.pagePropertySet(page, w.cost.id, number(31337));
    await client.pagePropertySet(page, w.due.id, day("2031-03-04"));
    await client.pagePropertySet(page, w.done.id, checkbox(true));
    await client.pagePropertySet(page, w.link.id, { kind: "url", value: "https://betaword.example.com/path" });
    await client.pagePropertySet(page, w.stage.id, status(must(w.stage.options[1]).id));
    await client.pagePropertySet(page, w.email.id, { kind: "email", value: "gamma.word@example.com" });
    await client.pagePropertySet(page, w.phone.id, { kind: "phone", value: "+1 555 0142" });
    await client.pagePropertySet(page, w.kind.id, select(w.red.id));
    expect(await client.search("alphaword")).toHaveLength(1);
    expect(await client.search("betaword")).toHaveLength(1);
    expect(await client.search("gamma")).toHaveLength(1);
    expect(await client.search("0142")).toHaveLength(1);
    expect(await client.search("progress")).toHaveLength(1);
    expect(await client.search("red")).toHaveLength(1);
    expect(await client.search("31337")).toEqual([]);
    expect(await client.search("2031")).toEqual([]);
    expect(await client.search("true")).toEqual([]);
    // One row per property: no row has the status name and the text, but the page does (ADR-0040), so
    // it is one hit, on the first of the two rows in schema order.
    expect(await client.search("progress alphaword")).toMatchObject([{ pageId: page, snippet: "alphaword" }]);
  });

  it("finds nothing for an option that is no longer listed, and updates when the page or the space changes", async () => {
    const { client, spaceId, plan } = await searchable();
    const tag = await client.propertyOptionAdd(spaceId, TAGS_PROPERTY_ID, "keepme", "gray");
    await client.pagePropertySet(plan, TAGS_PROPERTY_ID, multi(tag.id));
    expect(await client.search("keepme")).toHaveLength(1);
    // The page's own rows do not disturb the property rows.
    await client.pageRename(plan, "Renamed");
    expect(await client.search("keepme")).toHaveLength(1);
    await client.indexRebuild(spaceId);
    expect(await client.search("keepme")).toHaveLength(1);
    await client.indexRebuild();
    expect(await client.search("keepme")).toHaveLength(1);
    await client.pageTrash(plan);
    await client.pagePurge(plan);
    expect(await client.search("keepme")).toEqual([]);
  });
});

// ----- backups --------------------------------------------------------------------------------

/** Re-encodes a backup with `edit` applied to its state, with a valid checksum. */
async function reseal(bytes: Uint8Array, edit: (state: BackupJson) => void): Promise<Uint8Array> {
  const text = (data: Uint8Array): string => new TextDecoder().decode(data);
  const [first = "", headerLine = "", body = ""] = text(bytes).split("\n");
  const state = JSON.parse(text(Uint8Array.from(atob(body), (char) => char.charCodeAt(0)))) as BackupJson;
  edit(state);
  const encoded = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(state))));
  const signed = `${first}\n${headerLine}\n${encoded}\n`;
  return new TextEncoder().encode(`${signed}${toHex(await sha256(signed))}\n`);
}

/** The part of a backup's state these tests edit. */
interface BackupJson {
  spaces: { properties?: unknown[]; pages: { props?: unknown }[] }[];
}

describe("backups", () => {
  /** An installation with a kit (so it can export) and a space with a schema and values. */
  async function installation() {
    const files = new Map<string, Uint8Array>();
    const ctx = setup({
      files,
      workspaceName: "Research",
      spaces: [{ name: "Personal", pages: [{ title: "Plan" }, { title: "Old" }] }],
      security: { kitCreatedAt: T0 - 10_000, kitKey: KEY },
    });
    const { client } = ctx;
    const space = must((await client.spaceList())[0]);
    const view = await client.treeGet(space.id);
    const plan = must(view.roots[0]).page.id;
    const old = must(view.roots[1]).page.id;
    const notes = await client.propertyCreate(space.id, "Notes", "text");
    const stage = await client.propertyCreate(space.id, "Stage", "status");
    const kind = await client.propertyCreate(space.id, "Kind", "multi_select");
    const due = await client.propertyCreate(space.id, "Due", "date");
    const red = await client.propertyOptionAdd(space.id, kind.id, "red", "red");
    const tag = await client.propertyOptionAdd(space.id, TAGS_PROPERTY_ID, "restored tag", "purple");
    await client.pagePropertySet(plan, notes.id, text("in the backup"));
    await client.pagePropertySet(plan, stage.id, status(must(stage.options[2]).id));
    await client.pagePropertySet(plan, kind.id, multi(red.id));
    await client.pagePropertySet(plan, due.id, day("2026-10-07", "09:30"));
    await client.pagePropertySet(plan, TAGS_PROPERTY_ID, multi(tag.id));
    await client.pagePropertySet(old, notes.id, text("trashed value"));
    await client.pageTrash(old);
    return { ...ctx, files, spaceId: space.id, plan, old, notes, stage, kind, due, red, tag };
  }

  const words = async (): Promise<string> => (await kitWordsFromEntropy(KEY)).join(" ");

  it("carry the schema and every value, so a restore brings them back with their ids and times", async () => {
    const source = await installation();
    const { client } = source;
    const schema = await client.propertySchemaGet(source.spaceId);
    const planValues = await client.pagePropertiesGet(source.plan);
    const oldValues = await client.pagePropertiesGet(source.old);
    await client.backupExport(PATH);

    const fresh = setup({ files: source.files });
    await fresh.client.backupImport(PATH, await words());
    await expect(fresh.client.propertySchemaGet(source.spaceId)).resolves.toEqual(schema);
    await expect(fresh.client.pagePropertiesGet(source.plan)).resolves.toEqual(planValues);
    await expect(fresh.client.pagePropertiesGet(source.old)).resolves.toEqual(oldValues);
    expect(planValues).toHaveLength(5);
    // PageInfo carries them, trashed pages too, and the restored derived data answers.
    expect((await fresh.client.pageGet(source.plan)).properties).toEqual(planValues);
    expect(must((await fresh.client.treeGet(source.spaceId)).trashed[0]).page.properties).toEqual(oldValues);
    await expect(
      fresh.client.pagesByProperty(source.spaceId, source.notes.id, { type: "HasValue" }),
    ).resolves.toEqual([source.plan]);
    expect(await fresh.client.search("restored tag")).toHaveLength(1);
    // Tags was written (it has options): its creation time came along.
    expect(must(schema[0]).createdAt).toBeGreaterThan(T0);
    // The restored installation keeps working.
    await fresh.client.pagePropertySet(source.plan, source.notes.id, text("edited after the restore"));
    await expect(fresh.client.pagePropertiesGet(source.plan)).resolves.toHaveLength(5);
    await fresh.client.propertyOptionDelete(source.spaceId, source.kind.id, source.red.id);
    expect(idsOfValues(await fresh.client.pagePropertiesGet(source.plan))).toEqual([
      TAGS_PROPERTY_ID,
      source.notes.id,
      source.stage.id,
      source.due.id,
    ]);
  });

  it("round-trip a space whose Tags was never written, and leave out empty fields", async () => {
    const files = new Map<string, Uint8Array>();
    const { client } = setup({
      files,
      workspaceName: "Research",
      spaces: [{ name: "Plain", pages: [{ title: "P" }] }],
      security: { kitCreatedAt: T0 - 10_000, kitKey: KEY },
    });
    await client.backupExport(PATH);
    const { body } = await readBackupFile(must(files.get(PATH)));
    const state = readBackupState(body);
    const space = must(state.spaces[0]);
    expect("properties" in space).toBe(false);
    expect("props" in must(space.pages[0])).toBe(false);
    const fresh = setup({ files });
    await fresh.client.backupImport(PATH, await words());
    const restored = must((await fresh.client.spaceList())[0]);
    await expect(fresh.client.propertySchemaGet(restored.id)).resolves.toEqual([
      { id: TAGS_PROPERTY_ID, name: "Tags", kind: "multi_select", options: [], createdAt: 0 },
    ]);
  });

  it("restore a backup written before properties as a space with Tags only", async () => {
    const source = await installation();
    await source.client.backupExport(PATH);
    const bytes = must(source.files.get(PATH));
    // The shape of an older backup: no `properties` on spaces and no `props` on pages.
    const older = await reseal(bytes, (state) => {
      for (const space of state.spaces) {
        delete space.properties;
        for (const page of space.pages) {
          delete page.props;
        }
      }
    });
    expect(older.length).toBeLessThan(bytes.length);
    source.files.set(PATH, older);
    const fresh = setup({ files: source.files });
    await fresh.client.backupImport(PATH, await words());
    await expect(fresh.client.propertySchemaGet(source.spaceId)).resolves.toEqual([
      { id: TAGS_PROPERTY_ID, name: "Tags", kind: "multi_select", options: [], createdAt: 0 },
    ]);
    await expect(fresh.client.pagePropertiesGet(source.plan)).resolves.toEqual([]);
    expect((await fresh.client.pageGet(source.plan)).properties).toBeUndefined();
    // And a restored installation takes properties like a new one.
    const notes = await fresh.client.propertyCreate(source.spaceId, "Notes", "text");
    await expect(fresh.client.pagePropertySet(source.plan, notes.id, text("again"))).resolves.toHaveLength(1);
  });

  it("refuse damaged property records as a damaged file, and change nothing", async () => {
    const source = await installation();
    await source.client.backupExport(PATH);
    const bytes = must(source.files.get(PATH));
    const damage: ((state: BackupJson) => void)[] = [
      (state) => {
        must(must(state.spaces[0]).properties)[1] = { id: "x", name: "n", kind: "table", options: [], createdAt: 1 };
      },
      (state) => {
        must(must(state.spaces[0]).properties)[1] = { id: "x", name: "n", kind: "text", options: [{ id: "o" }], createdAt: 1 };
      },
      (state) => {
        const properties = must(must(state.spaces[0]).properties);
        properties.push(properties[1]);
      },
      (state) => {
        must(must(state.spaces[0]).pages[0]).props = [["p", { kind: "number", value: "3" }]];
      },
    ];
    for (const edit of damage) {
      const path = "C:/backups/damaged.atomikkb";
      source.files.set(path, await reseal(bytes, edit));
      const fresh = setup({ files: source.files });
      await expect(fresh.client.backupImport(path, await words())).rejects.toMatchObject({
        code: "Crypto",
        message: expect.stringContaining("not a valid backup file") as unknown,
      });
      await expect(fresh.client.workspaceGet()).resolves.toBeNull();
    }
  });
});

// ----- seeding --------------------------------------------------------------------------------

describe("seeding", () => {
  const SEED: SeedSpace[] = [
    {
      name: "Personal",
      tagOptions: [{ name: "urgent", color: "red" }],
      properties: [
        { name: "Stage", kind: "status" },
        { name: "Kind", kind: "select", options: [{ name: "red", color: "red" }, { name: "green" }] },
        { name: "Notes", kind: "text" },
        { name: "Cost", kind: "number" },
        { name: "Due", kind: "date" },
        { name: "Done", kind: "checkbox" },
        { name: "Link", kind: "url" },
        { name: "Contact", kind: "email" },
        { name: "Phone", kind: "phone" },
        { name: "Areas", kind: "multi_select", options: [{ name: "home" }, { name: "work", color: "blue" }] },
      ],
      pages: [
        {
          title: "Alpha",
          tags: ["urgent", "later"],
          properties: {
            Stage: "In progress",
            Kind: "green",
            Notes: "hello",
            Cost: 3,
            Due: { date: "2026-10-07", time: "09:30" },
            Done: true,
            Link: "https://example.com",
            Contact: "ada@example.com",
            Phone: "+1 555",
            Areas: ["work", "home"],
          },
          children: [{ title: "Beta", tags: ["later"], properties: { Due: "2026-11-01", Notes: "" } }],
        },
        { title: "Gamma" },
      ],
    },
    { name: "Work", pages: [{ title: "Delta", tags: ["x"] }] },
  ];

  it("creates schemas and values without events, undo history or a change of any page's updatedAt", async () => {
    const { client, events } = setup({ spaces: SEED });
    const [personal, work] = await client.spaceList();
    expect(events).toEqual([]);
    const view = await client.treeGet(must(personal).id);
    expect(view.undo).toEqual({ canUndo: false, canRedo: false });
    const alpha = must(view.roots[0]);
    expect(alpha.page.updatedAt).toBe(alpha.page.createdAt);

    const schema = await client.propertySchemaGet(must(personal).id);
    expect(names(schema)).toEqual([
      "Tags",
      "Stage",
      "Kind",
      "Notes",
      "Cost",
      "Due",
      "Done",
      "Link",
      "Contact",
      "Phone",
      "Areas",
    ]);
    const byName = (name: string): PropertyDef => must(schema.find((def) => def.name === name));
    // Tags: the seeded option, then the one a page created by name (gray).
    expect(byName("Tags").options.map(({ name, color }) => ({ name, color }))).toEqual([
      { name: "urgent", color: "red" },
      { name: "later", color: "gray" },
    ]);
    expect(byName("Tags").createdAt).toBeGreaterThan(0);
    // A status property starts with the three default options unless it is given its own.
    expect(byName("Stage").options.map(({ name, group }) => ({ name, group }))).toEqual(
      DEFAULT_STATUS_OPTIONS.map(({ name, group }) => ({ name, group })),
    );
    expect(byName("Kind").options.map(({ name, color }) => ({ name, color }))).toEqual([
      { name: "red", color: "red" },
      { name: "green", color: "gray" },
    ]);

    const values = new Map(alpha.page.properties);
    const tag = (name: string): string => must(byName("Tags").options.find((option) => option.name === name)).id;
    expect(values.get(TAGS_PROPERTY_ID)).toEqual(multi(tag("urgent"), tag("later")));
    expect(values.get(byName("Stage").id)).toEqual(status(must(byName("Stage").options[1]).id));
    expect(values.get(byName("Kind").id)).toEqual(select(must(byName("Kind").options[1]).id));
    expect(values.get(byName("Notes").id)).toEqual(text("hello"));
    expect(values.get(byName("Cost").id)).toEqual(number(3));
    expect(values.get(byName("Due").id)).toEqual(day("2026-10-07", "09:30"));
    expect(values.get(byName("Done").id)).toEqual(checkbox(true));
    expect(values.get(byName("Link").id)).toEqual({ kind: "url", value: "https://example.com" });
    expect(values.get(byName("Contact").id)).toEqual({ kind: "email", value: "ada@example.com" });
    expect(values.get(byName("Phone").id)).toEqual({ kind: "phone", value: "+1 555" });
    const areas = byName("Areas").options.map((option) => option.id);
    expect(values.get(byName("Areas").id)).toEqual(multi(must(areas[1]), must(areas[0])));

    // The child: a date given as a plain string, an empty text is no value.
    const beta = must(alpha.children[0]);
    expect(new Map(beta.page.properties)).toEqual(
      new Map([
        [TAGS_PROPERTY_ID, multi(tag("later"))],
        [byName("Due").id, day("2026-11-01")],
      ]),
    );
    expect("properties" in must(view.roots[1]).page).toBe(false);

    // The second space has its own Tags (and only that).
    const second = await client.propertySchemaGet(must(work).id);
    expect(names(second)).toEqual(["Tags"]);
    expect(must(second[0]).options.map((option) => option.name)).toEqual(["x"]);
    // The seeded values answer queries and search like written ones.
    await expect(
      client.pagesByProperty(must(personal).id, byName("Done").id, { type: "Checked", data: true }),
    ).resolves.toEqual([alpha.page.id]);
    expect(await client.search("urgent, later")).toHaveLength(1);
  });

  it("draws the ids of properties and options after every id the other seeds draw, in a documented order", async () => {
    // The same spaces and pages seeded without any property or tag.
    const bare = (pages: readonly SeedPage[]): SeedPage[] =>
      pages.map((page) => ({ title: page.title, children: bare(page.children ?? []) }));
    const plain = setup({ spaces: SEED.map(({ name, pages }) => ({ name, pages: bare(pages ?? []) })) });
    const withProperties = setup({ spaces: SEED });
    const flat = (nodes: PageNode[]): string[] => nodes.flatMap((node) => [node.page.id, ...flat(node.children)]);
    const treeIds = async (client: CoreClient): Promise<string[]> => {
      const out: string[] = [];
      for (const space of await client.spaceList()) {
        out.push(space.id, ...flat((await client.treeGet(space.id)).roots));
      }
      return out;
    };
    expect(await treeIds(withProperties.client)).toEqual(await treeIds(plain.client));

    // Device 1; space 2, Alpha 3, Beta 4, Gamma 5; space 6, Delta 7. Then the first space's
    // properties: Tags option "urgent" 8, Stage 9 (+ options 10-12), Kind 13 (+ 14, 15), Notes 16,
    // Cost 17, Due 18, Done 19, Link 20, Contact 21, Phone 22, Areas 23 (+ 24, 25), then the Tags
    // option "later" created by Alpha's tags 26. The second space: "x" 27.
    const [personal, work] = await withProperties.client.spaceList();
    const schema = await withProperties.client.propertySchemaGet(must(personal).id);
    expect(must(schema.find((def) => def.name === "Stage")).id).toBe(id(9));
    expect(must(schema.find((def) => def.name === "Stage")).options.map((option) => option.id)).toEqual([id(10), id(11), id(12)]);
    expect(must(schema.find((def) => def.name === "Areas")).id).toBe(id(23));
    expect(must(schema[0]).options.map((option) => option.id)).toEqual([id(8), id(26)]);
    expect(must((await withProperties.client.propertySchemaGet(must(work).id))[0]).options[0]?.id).toBe(id(27));
  });

  it("keeps the ids it is given and draws nothing for them", async () => {
    const propertyId = "aaaaaaaa-0000-4000-8000-000000000001";
    const optionId = "aaaaaaaa-0000-4000-8000-000000000002";
    const { client } = setup({
      spaces: [
        {
          name: "Personal",
          pages: [{ title: "A", properties: { Kind: "red" } }],
          properties: [{ id: propertyId, name: "Kind", kind: "select", options: [{ id: optionId, name: "red" }] }],
        },
      ],
    });
    const space = must((await client.spaceList())[0]);
    const schema = await client.propertySchemaGet(space.id);
    expect(must(schema[1])).toMatchObject({ id: propertyId, options: [{ id: optionId, name: "red" }] });
    await expect(client.pagePropertiesGet(must((await client.treeGet(space.id)).roots[0]).page.id)).resolves.toEqual([
      [propertyId, select(optionId)],
    ]);
    // The ids are the transport's (device 1, space 2, page 3): nothing else was drawn.
    expect(space.id).toBe(id(2));
  });

  it("refuses a seed that could not be created by requests, when the transport is built", () => {
    const build = (space: SeedSpace): (() => MemoryTransport) => () => new MemoryTransport({ spaces: [space] });
    expect(build({ name: "S", pages: [{ title: "P", properties: { Missing: "x" } }] })).toThrow(/no property named "Missing"/u);
    expect(
      build({
        name: "S",
        properties: [{ name: "Kind", kind: "select", options: [{ name: "red" }] }],
        pages: [{ title: "P", properties: { Kind: "blue" } }],
      }),
    ).toThrow(/no option named "blue"/u);
    expect(
      build({ name: "S", properties: [{ name: "N", kind: "number" }], pages: [{ title: "P", properties: { N: "3" } }] }),
    ).toThrow(/does not fit its kind \(number\)/u);
    expect(build({ name: "S", properties: [{ name: " ", kind: "text" }] })).toThrow(/must not be empty/u);
    expect(build({ name: "S", properties: [{ name: "T", kind: "text", options: [{ name: "x" }] }] })).toThrow(
      /no options/u,
    );
    expect(
      build({ name: "S", properties: [{ name: "K", kind: "select", options: [{ name: "x", group: "done" }] }] }),
    ).toThrow(/only status options have a group/u);
    expect(
      build({
        name: "S",
        properties: [{ name: "D", kind: "date" }],
        pages: [{ title: "P", properties: { D: { date: "2026-02-30" } } }],
      }),
    ).toThrow(/the date must be a real day/u);
    expect(
      build({
        name: "S",
        properties: [{ name: "K", kind: "select" }],
        pages: [{ title: "P", properties: { K: "x" } }],
      }),
    ).toThrow(/no option named "x"/u);
    // Only Tags gets a missing option created.
    expect(build({ name: "S", pages: [{ title: "P", properties: { Tags: ["fresh"] } }] })).not.toThrow();
  });

  it("lets a Tags entry of properties replace the tags shorthand of the same page", async () => {
    const { client } = setup({
      spaces: [{ name: "S", pages: [{ title: "P", tags: ["a"], properties: { Tags: ["b"] } }] }],
    });
    const space = must((await client.spaceList())[0]);
    const page = must((await client.treeGet(space.id)).roots[0]).page;
    const tags = must((await client.propertySchemaGet(space.id))[0]);
    expect(tags.options.map((option) => option.name)).toEqual(["a", "b"]);
    expect(page.properties).toEqual([[TAGS_PROPERTY_ID, multi(must(tags.options[1]).id)]]);
  });
});
