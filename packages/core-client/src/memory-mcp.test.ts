// MCP clients, grants and the audit log in `MemoryTransport` (ADR-0037, `docs/architecture/ai.md` section 7).

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import { MemoryTransport } from "./memory";

async function setup() {
  let clock = 1_000;
  const memory = new MemoryTransport({
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: [{ title: "A" }] }, { name: "Work" }],
    ai: { mode: "fake" },
    now: () => (clock += 10),
  });
  const client = new CoreClient(memory);
  const events: Event[] = [];
  memory.subscribe((event) => events.push(event));
  const [personal, work] = await client.spaceList();
  return { memory, client, events, personal: personal?.id ?? "", work: work?.id ?? "" };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

describe("MCP clients", () => {
  it("creates a client without grants and shows the token once", async () => {
    const { client, events } = await setup();
    const created = await client.mcpClientCreate("  Claude Desktop ");
    expect(created.client).toMatchObject({ name: "Claude Desktop", grants: [] });
    expect(created.client.id).toMatch(/^[0-9a-f]{32}$/);
    expect(created.client.lastUsedAt).toBeUndefined();
    expect(created.token).toMatch(/^kbm_[A-Za-z0-9_-]{43}$/);
    expect(events).toContainEqual({ type: "McpChanged" });
    // The list never has a token.
    const [listed] = await client.mcpClientList();
    expect(listed).toEqual(created.client);
    expect(JSON.stringify(await client.mcpClientList())).not.toContain("kbm_");
  });

  it("gives every client its own id and token", async () => {
    const { client } = await setup();
    const first = await client.mcpClientCreate("One");
    const second = await client.mcpClientCreate("Two");
    expect(first.client.id).not.toBe(second.client.id);
    expect(first.token).not.toBe(second.token);
    expect((await client.mcpClientList()).map((entry) => entry.name)).toEqual(["One", "Two"]);
  });

  it("checks the name: 1 to 60 characters after trimming, no control characters", async () => {
    const { client } = await setup();
    expect(await failure(client.mcpClientCreate("   "))).toMatchObject({ code: "InvalidInput", details: { field: "name" } });
    expect(await failure(client.mcpClientCreate("x".repeat(61)))).toMatchObject({ details: { field: "name" } });
    expect(await failure(client.mcpClientCreate("a\u0007b"))).toMatchObject({ details: { field: "name" } });
    await expect(client.mcpClientCreate("x".repeat(60))).resolves.toBeDefined();
  });

  it("deletes a client, and an unknown one is NotFound", async () => {
    const { client, events } = await setup();
    const { client: created } = await client.mcpClientCreate("Gone");
    events.length = 0;
    await client.mcpClientDelete(created.id);
    expect(await client.mcpClientList()).toEqual([]);
    expect(events).toContainEqual({ type: "McpChanged" });
    expect(await failure(client.mcpClientDelete(created.id))).toMatchObject({ code: "NotFound" });
  });
});

describe("MCP grants (the shell's mcp_client_grants_set)", () => {
  it("replaces the grants; write implies read", async () => {
    const { client, personal, work, events } = await setup();
    const { client: created } = await client.mcpClientCreate("Agent");
    events.length = 0;
    const updated = await client.mcpGrantsSet(created.id, [
      { spaceId: personal, read: false, write: true },
      { spaceId: work, read: true, write: false },
    ]);
    expect(updated.grants).toEqual([
      { spaceId: personal, read: true, write: true },
      { spaceId: work, read: true, write: false },
    ]);
    expect((await client.mcpClientList())[0]?.grants).toEqual(updated.grants);
    expect(events).toContainEqual({ type: "McpChanged" });
    // Replaced, not merged.
    const fewer = await client.mcpGrantsSet(created.id, [{ spaceId: work, read: true, write: false }]);
    expect(fewer.grants).toEqual([{ spaceId: work, read: true, write: false }]);
  });

  it("ignores grants of unknown spaces, drops grants that allow nothing, and keeps the last of a space listed twice", async () => {
    const { client, personal, work } = await setup();
    const { client: created } = await client.mcpClientCreate("Agent");
    const updated = await client.mcpGrantsSet(created.id, [
      { spaceId: "deleted-space", read: true, write: true },
      { spaceId: personal, read: true, write: true },
      { spaceId: personal, read: true, write: false },
      { spaceId: work, read: false, write: false },
    ]);
    expect(updated.grants).toEqual([{ spaceId: personal, read: true, write: false }]);
  });

  it("forgets the grants of a deleted space", async () => {
    const { client, work } = await setup();
    const { client: created } = await client.mcpClientCreate("Agent");
    await client.mcpGrantsSet(created.id, [{ spaceId: work, read: true, write: true }]);
    await client.spaceDelete(work);
    expect((await client.mcpClientList())[0]?.grants).toEqual([]);
  });

  it("answers NotFound for an unknown client", async () => {
    const { client } = await setup();
    expect(await failure(client.mcpGrantsSet("nope", []))).toMatchObject({ code: "NotFound" });
  });
});

describe("the MCP audit log", () => {
  it("lists the newest first, with the client's name at the time and the last use", async () => {
    const { client, memory, personal } = await setup();
    const { client: a } = await client.mcpClientCreate("Alpha");
    const { client: b } = await client.mcpClientCreate("Beta");
    memory.recordMcpCall({ clientId: a.id, tool: "search", outcome: "ok", summary: "tomato", spaceId: personal });
    memory.recordMcpCall({ clientId: b.id, tool: "read_page", outcome: "denied", summary: "A" });
    memory.recordMcpCall({ clientId: a.id, tool: "create_page", outcome: "error", summary: "New" });
    const rows = await client.mcpAuditList();
    expect(rows.map((row) => row.tool)).toEqual(["create_page", "read_page", "search"]);
    expect(rows.map((row) => row.id)).toEqual([3, 2, 1]);
    expect(rows[2]).toMatchObject({ clientId: a.id, clientName: "Alpha", outcome: "ok", summary: "tomato", spaceId: personal });
    expect(rows[0]?.at).toBeGreaterThan(rows[2]?.at ?? 0);
    expect((await client.mcpClientList()).find((entry) => entry.id === a.id)?.lastUsedAt).toBe(rows[0]?.at);
    // The row keeps the name after the client is deleted.
    await client.mcpClientDelete(a.id);
    expect((await client.mcpAuditList()).find((row) => row.tool === "search")?.clientName).toBe("Alpha");
  });

  it("filters by client, pages with `before` and limits", async () => {
    const { client, memory } = await setup();
    const { client: a } = await client.mcpClientCreate("Alpha");
    const { client: b } = await client.mcpClientCreate("Beta");
    for (let i = 0; i < 6; i += 1) {
      memory.recordMcpCall({ clientId: i % 2 === 0 ? a.id : b.id, tool: "search", outcome: "ok", summary: `q${String(i)}` });
    }
    expect((await client.mcpAuditList({ clientId: a.id })).map((row) => row.summary)).toEqual(["q4", "q2", "q0"]);
    expect((await client.mcpAuditList({ limit: 2 })).map((row) => row.summary)).toEqual(["q5", "q4"]);
    expect((await client.mcpAuditList({ before: 4, limit: 2 })).map((row) => row.summary)).toEqual(["q2", "q1"]);
    expect(await client.mcpAuditList({ clientId: "nobody" })).toEqual([]);
  });

  it("keeps the newest 10,000 rows, shows 100 by default and at most 500", async () => {
    const { client, memory } = await setup();
    const { client: a } = await client.mcpClientCreate("Alpha");
    for (let i = 0; i < 10_050; i += 1) {
      memory.recordMcpCall({ clientId: a.id, tool: "search", outcome: "ok" });
    }
    expect(await client.mcpAuditList()).toHaveLength(100);
    expect(await client.mcpAuditList({ limit: 1000 })).toHaveLength(500);
    // Ids 1 to 50 were pruned: 51 is the oldest row left.
    const last = await client.mcpAuditList({ limit: 500, before: 552 });
    expect(last.at(-1)?.id).toBe(52);
    expect((await client.mcpAuditList({ before: 52 })).map((row) => row.id)).toEqual([51]);
    expect(await client.mcpAuditList({ before: 51 })).toEqual([]);
  });

  it("cuts a summary at 200 characters and clears", async () => {
    const { client, memory, events } = await setup();
    const { client: a } = await client.mcpClientCreate("Alpha");
    memory.recordMcpCall({ clientId: a.id, tool: "search", outcome: "invalid", summary: "x".repeat(500) });
    expect((await client.mcpAuditList())[0]?.summary).toHaveLength(200);
    events.length = 0;
    await client.mcpAuditClear();
    expect(await client.mcpAuditList()).toEqual([]);
    expect(events).toContainEqual({ type: "McpChanged" });
  });

  it("answers NotFound when a test records a call for an unknown client", async () => {
    const { memory } = await setup();
    expect(() => memory.recordMcpCall({ clientId: "nope", tool: "search", outcome: "ok" })).toThrow(CoreError);
  });
});
