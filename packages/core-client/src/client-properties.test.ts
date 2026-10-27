// The typed property methods of `CoreClient` (ADR-0026): the request each one sends, the payload
// it unwraps, what happens when the transport answers the wrong variant or fails, and the same
// calls end to end over a `MemoryTransport`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { PageProperties } from "./generated/PageProperties";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyOption } from "./generated/PropertyOption";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import { MemoryTransport } from "./memory";
import { TAGS_PROPERTY_ID } from "./properties";
import type { DocTransport, Transport } from "./transport";

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const PROPERTY = "33333333-3333-4333-8333-333333333333";
const OTHER = "44444444-4444-4444-8444-444444444444";
const OPTION = "55555555-5555-4555-8555-555555555555";

const def: PropertyDef = {
  id: PROPERTY,
  name: "Stage",
  kind: "status",
  options: [{ id: OPTION, name: "Done", color: "green", group: "done" }],
  createdAt: 1_700_000_000_000,
};
const tags: PropertyDef = { id: TAGS_PROPERTY_ID, name: "Tags", kind: "multi_select", options: [], createdAt: 0 };
const option: PropertyOption = { id: OPTION, name: "Done", color: "green", group: "done" };
const values: PageProperties["values"] = [[PROPERTY, { kind: "status", value: OPTION }]];

const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response | Error) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return response instanceof Error ? Promise.reject(response) : Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
  };
  return { requests, client: new CoreClient(transport) };
}

describe("CoreClient property helpers", () => {
  it("propertySchemaGet sends the space id and returns the properties of the answer", async () => {
    const { client, requests } = stubTransport({ type: "PropertySchema", data: { properties: [tags, def] } });
    await expect(client.propertySchemaGet(SPACE)).resolves.toEqual([tags, def]);
    expect(requests).toEqual([{ type: "PropertySchemaGet", data: { spaceId: SPACE } }]);
  });

  it("propertyCreate sends the name and the kind and returns the new property", async () => {
    const { client, requests } = stubTransport({ type: "Property", data: def });
    await expect(client.propertyCreate(SPACE, "Stage", "status")).resolves.toEqual(def);
    expect(requests).toEqual([{ type: "PropertyCreate", data: { spaceId: SPACE, name: "Stage", kind: "status" } }]);
  });

  it("propertyRename sends the new name and returns the property", async () => {
    const { client, requests } = stubTransport({ type: "Property", data: def });
    await expect(client.propertyRename(SPACE, PROPERTY, "Phase")).resolves.toEqual(def);
    expect(requests).toEqual([
      { type: "PropertyRename", data: { spaceId: SPACE, propertyId: PROPERTY, name: "Phase" } },
    ]);
  });

  it("propertyDelete sends the ids and resolves with nothing", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.propertyDelete(SPACE, PROPERTY)).resolves.toBeUndefined();
    expect(requests).toEqual([{ type: "PropertyDelete", data: { spaceId: SPACE, propertyId: PROPERTY } }]);
  });

  it("propertyMove sends `before` only when given", async () => {
    const before = stubTransport({ type: "Ok" });
    await expect(before.client.propertyMove(SPACE, PROPERTY, OTHER)).resolves.toBeUndefined();
    expect(before.requests).toEqual([
      { type: "PropertyMove", data: { spaceId: SPACE, propertyId: PROPERTY, before: OTHER } },
    ]);

    const last = stubTransport({ type: "Ok" });
    await last.client.propertyMove(SPACE, PROPERTY);
    expect(last.requests[0]).toStrictEqual({ type: "PropertyMove", data: { spaceId: SPACE, propertyId: PROPERTY } });
  });

  it("propertyOptionAdd sends the colour, and the group only when given, and returns the option", async () => {
    const plain = stubTransport({ type: "PropertyOption", data: option });
    await expect(plain.client.propertyOptionAdd(SPACE, PROPERTY, "Done", "green")).resolves.toEqual(option);
    expect(plain.requests[0]).toStrictEqual({
      type: "PropertyOptionAdd",
      data: { spaceId: SPACE, propertyId: PROPERTY, name: "Done", color: "green" },
    });

    const grouped = stubTransport({ type: "PropertyOption", data: option });
    await grouped.client.propertyOptionAdd(SPACE, PROPERTY, "Done", "green", "done");
    expect(grouped.requests).toEqual([
      {
        type: "PropertyOptionAdd",
        data: { spaceId: SPACE, propertyId: PROPERTY, name: "Done", color: "green", group: "done" },
      },
    ]);
  });

  it("propertyOptionUpdate sends only what the patch changes and returns the option", async () => {
    const some = stubTransport({ type: "PropertyOption", data: option });
    await expect(some.client.propertyOptionUpdate(SPACE, PROPERTY, OPTION, { color: "red" })).resolves.toEqual(option);
    expect(some.requests[0]).toStrictEqual({
      type: "PropertyOptionUpdate",
      data: { spaceId: SPACE, propertyId: PROPERTY, optionId: OPTION, color: "red" },
    });

    const all = stubTransport({ type: "PropertyOption", data: option });
    await all.client.propertyOptionUpdate(SPACE, PROPERTY, OPTION, { name: "Shipped", color: "pink", group: "done" });
    expect(all.requests).toEqual([
      {
        type: "PropertyOptionUpdate",
        data: { spaceId: SPACE, propertyId: PROPERTY, optionId: OPTION, name: "Shipped", color: "pink", group: "done" },
      },
    ]);

    const none = stubTransport({ type: "PropertyOption", data: option });
    await none.client.propertyOptionUpdate(SPACE, PROPERTY, OPTION, {});
    expect(none.requests[0]).toStrictEqual({
      type: "PropertyOptionUpdate",
      data: { spaceId: SPACE, propertyId: PROPERTY, optionId: OPTION },
    });
  });

  it("propertyOptionDelete sends the ids and resolves with nothing", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.propertyOptionDelete(SPACE, PROPERTY, OPTION)).resolves.toBeUndefined();
    expect(requests).toEqual([
      { type: "PropertyOptionDelete", data: { spaceId: SPACE, propertyId: PROPERTY, optionId: OPTION } },
    ]);
  });

  it("pagePropertySet sends the value, or null to clear, and returns the values of the answer", async () => {
    const set = stubTransport({ type: "PageProperties", data: { values } });
    await expect(set.client.pagePropertySet(PAGE, PROPERTY, { kind: "status", value: OPTION })).resolves.toEqual(values);
    expect(set.requests).toEqual([
      {
        type: "PagePropertySet",
        data: { pageId: PAGE, propertyId: PROPERTY, value: { kind: "status", value: OPTION } },
      },
    ]);

    const clear = stubTransport({ type: "PageProperties", data: { values: [] } });
    await expect(clear.client.pagePropertySet(PAGE, PROPERTY, null)).resolves.toEqual([]);
    expect(clear.requests).toEqual([
      { type: "PagePropertySet", data: { pageId: PAGE, propertyId: PROPERTY, value: null } },
    ]);
  });

  it("pagePropertiesGet sends the page id and returns the values", async () => {
    const { client, requests } = stubTransport({ type: "PageProperties", data: { values } });
    await expect(client.pagePropertiesGet(PAGE)).resolves.toEqual(values);
    expect(requests).toEqual([{ type: "PagePropertiesGet", data: { pageId: PAGE } }]);
  });

  it("pagesByProperty sends the filter as given and returns the page ids", async () => {
    const { client, requests } = stubTransport({ type: "PageIds", data: [PAGE, OTHER] });
    await expect(
      client.pagesByProperty(SPACE, PROPERTY, { type: "DateBetween", data: { from: "2026-10-01" } }),
    ).resolves.toEqual([PAGE, OTHER]);
    expect(requests).toEqual([
      {
        type: "PagesByProperty",
        data: { spaceId: SPACE, propertyId: PROPERTY, filter: { type: "DateBetween", data: { from: "2026-10-01" } } },
      },
    ]);
    await client.pagesByProperty(SPACE, PROPERTY, { type: "HasValue" });
    expect(requests[1]).toStrictEqual({
      type: "PagesByProperty",
      data: { spaceId: SPACE, propertyId: PROPERTY, filter: { type: "HasValue" } },
    });
  });
});

describe("CoreClient response narrowing and failures for the property requests", () => {
  it("rejects with an Internal error naming the request when the variant is unexpected", async () => {
    const calls: [string, Response, (client: CoreClient) => Promise<unknown>][] = [
      ["PropertySchemaGet", { type: "Property", data: def }, (c) => c.propertySchemaGet(SPACE)],
      ["PropertyCreate", { type: "Ok" }, (c) => c.propertyCreate(SPACE, "x", "text")],
      ["PropertyRename", { type: "PropertyOption", data: option }, (c) => c.propertyRename(SPACE, PROPERTY, "x")],
      ["PropertyDelete", { type: "Property", data: def }, (c) => c.propertyDelete(SPACE, PROPERTY)],
      ["PropertyMove", { type: "Property", data: def }, (c) => c.propertyMove(SPACE, PROPERTY)],
      ["PropertyOptionAdd", { type: "Property", data: def }, (c) => c.propertyOptionAdd(SPACE, PROPERTY, "x", "gray")],
      ["PropertyOptionUpdate", { type: "Ok" }, (c) => c.propertyOptionUpdate(SPACE, PROPERTY, OPTION, {})],
      ["PropertyOptionDelete", { type: "PropertyOption", data: option }, (c) => c.propertyOptionDelete(SPACE, PROPERTY, OPTION)],
      ["PagePropertySet", { type: "Ok" }, (c) => c.pagePropertySet(PAGE, PROPERTY, null)],
      ["PagePropertiesGet", { type: "PageIds", data: [] }, (c) => c.pagePropertiesGet(PAGE)],
      ["PagesByProperty", { type: "PageProperties", data: { values } }, (c) => c.pagesByProperty(SPACE, PROPERTY, { type: "HasValue" })],
    ];
    for (const [name, wrong, call] of calls) {
      const { client } = stubTransport(wrong);
      const error: unknown = await call(client).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error, name).toBeInstanceOf(CoreError);
      expect(error, name).toMatchObject({ code: "Internal" });
      expect((error as CoreError).message, name).toContain(`Unexpected response to ${name}`);
    }
  });

  it("does not mix up the Property, PropertyOption, PropertySchema, PageProperties and PageIds variants", async () => {
    const { client } = stubTransport({ type: "Property", data: def });
    await expect(client.propertySchemaGet(SPACE)).rejects.toMatchObject({
      message: expect.stringContaining("expected PropertySchema, got Property") as unknown,
    });
    await expect(client.propertyOptionAdd(SPACE, PROPERTY, "x", "gray")).rejects.toMatchObject({
      message: expect.stringContaining("expected PropertyOption, got Property") as unknown,
    });
    await expect(client.pagesByProperty(SPACE, PROPERTY, { type: "HasValue" })).rejects.toMatchObject({
      message: expect.stringContaining("expected PageIds, got Property") as unknown,
    });
  });

  it("lets the transport's failures through as they are", async () => {
    const failure = new CoreError({ code: "NotFound", message: "property not found" });
    const { client } = stubTransport(failure);
    await expect(client.propertyDelete(SPACE, PROPERTY)).rejects.toBe(failure);
    await expect(client.pagePropertySet(PAGE, PROPERTY, null)).rejects.toBe(failure);
    await expect(client.pagesByProperty(SPACE, PROPERTY, { type: "HasValue" })).rejects.toBe(failure);
  });
});

describe("CoreClient property helpers over a MemoryTransport", () => {
  async function setup() {
    let counter = 0;
    const transport = new MemoryTransport({
      newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
      spaces: [{ name: "Personal", pages: [{ title: "Plan" }, { title: "Ideas" }] }],
    });
    const client = new CoreClient(transport);
    const space = (await client.spaceList())[0];
    if (space === undefined) {
      throw new Error("the demo space is missing");
    }
    const [plan, ideas] = (await client.treeGet(space.id)).roots.map((node) => node.page.id);
    if (plan === undefined || ideas === undefined) {
      throw new Error("the demo pages are missing");
    }
    return { transport, client, spaceId: space.id, plan, ideas };
  }

  it("works through the schema, the values and the queries with the shapes the types promise", async () => {
    const { client, spaceId, plan, ideas } = await setup();
    const stage = await client.propertyCreate(spaceId, "Stage", "status");
    expect(stage).toMatchObject({ name: "Stage", kind: "status" });
    expect(stage.options.map((one) => one.name)).toEqual(["Not started", "In progress", "Done"]);
    const kind = await client.propertyCreate(spaceId, "Kind", "select");
    const red = await client.propertyOptionAdd(spaceId, kind.id, "red", "red");
    expect(red).toEqual({ id: red.id, name: "red", color: "red" });
    const blue = await client.propertyOptionUpdate(spaceId, kind.id, red.id, { name: "blue", color: "blue" });
    expect(blue).toEqual({ id: red.id, name: "blue", color: "blue" });

    const doneId = stage.options[2]?.id ?? "";
    const valuesAfter = await client.pagePropertySet(plan, stage.id, { kind: "status", value: doneId });
    expect(valuesAfter).toEqual([[stage.id, { kind: "status", value: doneId }]]);
    await client.pagePropertySet(ideas, kind.id, { kind: "select", value: red.id });
    await expect(client.pagePropertiesGet(plan)).resolves.toEqual(valuesAfter);
    await expect(client.pagesByProperty(spaceId, stage.id, { type: "HasValue" })).resolves.toEqual([plan]);
    await expect(
      client.pagesByProperty(spaceId, stage.id, { type: "ContainsOption", data: doneId }),
    ).resolves.toEqual([plan]);
    await expect(
      client.pagesByProperty(spaceId, kind.id, { type: "Equals", data: { kind: "select", value: red.id } }),
    ).resolves.toEqual([ideas]);

    await client.propertyMove(spaceId, kind.id, stage.id);
    expect((await client.propertySchemaGet(spaceId)).map((one) => one.name)).toEqual(["Tags", "Kind", "Stage"]);
    await client.propertyOptionDelete(spaceId, kind.id, red.id);
    await expect(client.pagePropertiesGet(ideas)).resolves.toEqual([]);
    await client.pagePropertySet(plan, stage.id, null);
    await client.propertyDelete(spaceId, stage.id);
    await client.propertyRename(spaceId, TAGS_PROPERTY_ID, "Labels");
    expect((await client.propertySchemaGet(spaceId)).map((one) => one.name)).toEqual(["Labels", "Kind"]);
  });

  it("rejects with CoreErrors carrying the core's code, message and field", async () => {
    const { client, spaceId, plan } = await setup();
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    const error = (await client.propertyDelete(spaceId, TAGS_PROPERTY_ID).then(
      () => undefined,
      (e: unknown) => e,
    )) as CoreError;
    expect(error).toBeInstanceOf(CoreError);
    expect(error.toApiError()).toEqual({
      code: "InvalidInput",
      message: "the Tags property cannot be deleted",
      details: { field: "propertyId" },
    });
    await expect(client.pagePropertySet(plan, notes.id, { kind: "number", value: 1 })).rejects.toMatchObject({
      code: "InvalidInput",
      message: "the value does not match the kind of the property",
      details: { field: "value" },
    });
    await expect(client.propertySchemaGet("00000000-0000-4000-8000-0000000000ff")).rejects.toMatchObject({
      code: "NotFound",
      message: "space not found",
    });
  });

  it("is refused with the core's error while the transport has a failure queued, and works again after it", async () => {
    const { transport, client, spaceId } = await setup();
    transport.failNext({ code: "Storage", message: "disk full" });
    await expect(client.propertyCreate(spaceId, "Notes", "text")).rejects.toMatchObject({ code: "Storage" });
    await expect(client.propertySchemaGet(spaceId)).resolves.toHaveLength(1);
    await expect(client.propertyCreate(spaceId, "Notes", "text")).resolves.toMatchObject({ name: "Notes" });
  });
});
