import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Backlink } from "./generated/Backlink";
import type { PageContent } from "./generated/PageContent";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SearchHit } from "./generated/SearchHit";
import type { DocTransport, Transport } from "./transport";

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const SOURCE = "33333333-3333-4333-8333-333333333333";

const hit: SearchHit = {
  pageId: PAGE,
  spaceId: SPACE,
  title: "Plan",
  blockId: "44444444-4444-4444-8444-444444444444",
  snippet: "the plan",
  highlights: [{ start: 4, end: 8 }],
};
const backlink: Backlink = {
  pageId: SOURCE,
  spaceId: SPACE,
  title: "Source",
  blockId: "55555555-5555-4555-8555-555555555555",
  text: "see Plan",
};
const content: PageContent = { pageId: PAGE, content: { type: "doc", content: [{ type: "paragraph" }] } };

const docs: DocTransport = {
  open: () => Promise.resolve(new Uint8Array([1])),
  import: () => Promise.resolve(),
  export: () => Promise.resolve(new Uint8Array()),
  close: () => Promise.resolve(),
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response, extras: Partial<Transport> = {}) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs,
    ...extras,
  };
  return { requests, client: new CoreClient(transport) };
}

describe("CoreClient search", () => {
  it("sends the query and only the options that were given", async () => {
    const plain = stubTransport({ type: "Search", data: [hit] });
    await expect(plain.client.search("plan")).resolves.toEqual([hit]);
    expect(plain.requests[0]).toStrictEqual({ type: "Search", data: { query: "plan" } });

    const scoped = stubTransport({ type: "Search", data: [] });
    await scoped.client.search("plan", { spaceId: SPACE, scope: "Titles", limit: 8 });
    expect(scoped.requests).toEqual([
      { type: "Search", data: { query: "plan", spaceId: SPACE, scope: "Titles", limit: 8 } },
    ]);

    // The time filter goes out as `editedAfter`, in Unix milliseconds (ADR-0040).
    const recent = stubTransport({ type: "Search", data: [] });
    await recent.client.search("plan", { editedAfter: 1_791_383_400_000 });
    expect(recent.requests[0]).toStrictEqual({ type: "Search", data: { query: "plan", editedAfter: 1_791_383_400_000 } });

    const empty = stubTransport({ type: "Search", data: [] });
    await empty.client.search("plan", { spaceId: undefined, limit: undefined, editedAfter: undefined });
    expect(empty.requests[0]).toStrictEqual({ type: "Search", data: { query: "plan" } });
  });
});

describe("CoreClient page content helpers", () => {
  it("backlinksGet sends the page id and returns the backlinks", async () => {
    const { client, requests } = stubTransport({ type: "Backlinks", data: [backlink] });
    await expect(client.backlinksGet(PAGE)).resolves.toEqual([backlink]);
    expect(requests).toEqual([{ type: "BacklinksGet", data: { pageId: PAGE } }]);
  });

  it("pageContentGet returns the portable content", async () => {
    const { client, requests } = stubTransport({ type: "PageContent", data: content });
    await expect(client.pageContentGet(PAGE)).resolves.toEqual(content);
    expect(requests).toEqual([{ type: "PageContentGet", data: { pageId: PAGE } }]);
  });

  it("pageExportMarkdown returns the Markdown text", async () => {
    const { client, requests } = stubTransport({ type: "Markdown", data: "# Plan\n" });
    await expect(client.pageExportMarkdown(PAGE)).resolves.toBe("# Plan\n");
    expect(requests).toEqual([{ type: "PageExportMarkdown", data: { pageId: PAGE } }]);
  });

  it("indexRebuild sends the space only when given", async () => {
    const all = stubTransport({ type: "Ok" });
    await expect(all.client.indexRebuild()).resolves.toBeUndefined();
    expect(all.requests[0]).toStrictEqual({ type: "IndexRebuild", data: {} });

    const one = stubTransport({ type: "Ok" });
    await one.client.indexRebuild(SPACE);
    expect(one.requests).toEqual([{ type: "IndexRebuild", data: { spaceId: SPACE } }]);
  });

  it("rejects with an Internal error naming the request when the variant is unexpected", async () => {
    const calls: [string, Response, (client: CoreClient) => Promise<unknown>][] = [
      ["Search", { type: "Ok" }, (c) => c.search("x")],
      ["BacklinksGet", { type: "Search", data: [] }, (c) => c.backlinksGet(PAGE)],
      ["PageContentGet", { type: "Markdown", data: "" }, (c) => c.pageContentGet(PAGE)],
      ["PageExportMarkdown", { type: "PageContent", data: content }, (c) => c.pageExportMarkdown(PAGE)],
      ["IndexRebuild", { type: "Markdown", data: "" }, (c) => c.indexRebuild()],
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
});

describe("CoreClient docs and openExternal", () => {
  it("docs is the transport's document session API", async () => {
    const { client } = stubTransport({ type: "Ok" });
    expect(client.docs).toBe(docs);
    await expect(client.docs.open(PAGE)).resolves.toEqual(new Uint8Array([1]));
  });

  it("openExternal forwards to the transport and reports whether it could", async () => {
    const openExternal = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());
    const able = stubTransport({ type: "Ok" }, { openExternal });
    await expect(able.client.openExternal("https://example.com")).resolves.toBe(true);
    expect(openExternal).toHaveBeenCalledExactlyOnceWith("https://example.com");

    const unable = stubTransport({ type: "Ok" });
    await expect(unable.client.openExternal("https://example.com")).resolves.toBe(false);
  });

  it("openExternal passes the transport's failure through", async () => {
    const refused = new CoreError({ code: "InvalidInput", message: "unsupported scheme", details: { field: "url" } });
    const { client } = stubTransport({ type: "Ok" }, { openExternal: () => Promise.reject(refused) });
    await expect(client.openExternal("file:///etc/passwd")).rejects.toBe(refused);
  });
});
