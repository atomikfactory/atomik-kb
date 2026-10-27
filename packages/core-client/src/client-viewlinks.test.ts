// `CoreClient`'s view link methods (Milestone 13, slice 4 contract): what `viewLinkCreate`, `viewLinkList` and
// `viewLinkRevoke` send and return, and the constants of the viewer. What the in-memory mirror's links do is
// `memory-viewlinks.test.ts`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { ViewLinkInfo } from "./generated/ViewLinkInfo";
import { LINK_EXPIRY_MAX_DAYS, LINK_POLL_SECONDS, LINK_REFRESH_DEBOUNCE_MS, VIEWER_CSP, VIEWER_PATH } from "./link";
import type { DocTransport, Transport } from "./transport";

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const LINK = "cdcdcdcd-cdcd-cdcd-cdcd-cdcdcdcdcdcd";
const ALICE = "33333333-3333-4333-8333-333333333333";

const link: ViewLinkInfo = {
  linkId: LINK,
  pageId: PAGE,
  url: "http://127.0.0.1:8787/view#AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8",
  createdBy: ALICE,
  createdByName: "Alice",
  createdAt: 1_791_383_400_000,
  expiresAt: 1_791_988_200_000,
  status: "Publishing",
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
  };
  return { requests, client: new CoreClient(transport) };
}

describe("CoreClient view links", () => {
  it("creates a link and sends the request as the contract says", async () => {
    const { client, requests } = stubTransport({ type: "ViewLink", data: link });
    expect(await client.viewLinkCreate(SPACE, PAGE, 7)).toEqual(link);
    expect(await client.viewLinkCreate(SPACE, PAGE)).toEqual(link);
    expect(requests).toEqual([
      { type: "ViewLinkCreate", data: { spaceId: SPACE, pageId: PAGE, expiresInDays: 7 } },
      // No expiry: the key is left out, not `undefined`.
      { type: "ViewLinkCreate", data: { spaceId: SPACE, pageId: PAGE } },
    ]);
    expect(Object.keys(requests[1]?.type === "ViewLinkCreate" ? requests[1].data : {})).toEqual(["spaceId", "pageId"]);
  });

  it("lists a space's links and revokes one, getting the links that remain", async () => {
    const list = stubTransport({ type: "ViewLinks", data: [link] });
    expect(await list.client.viewLinkList(SPACE)).toEqual([link]);
    expect(list.requests).toEqual([{ type: "ViewLinkList", data: { spaceId: SPACE } }]);

    const revoke = stubTransport({ type: "ViewLinks", data: [] });
    expect(await revoke.client.viewLinkRevoke(SPACE, LINK)).toEqual([]);
    expect(revoke.requests).toEqual([{ type: "ViewLinkRevoke", data: { spaceId: SPACE, linkId: LINK } }]);
  });

  it("refuses an answer of another type", async () => {
    const { client } = stubTransport({ type: "Ok" });
    await expect(client.viewLinkCreate(SPACE, PAGE)).rejects.toBeInstanceOf(CoreError);
    await expect(client.viewLinkList(SPACE)).rejects.toBeInstanceOf(CoreError);
    await expect(client.viewLinkRevoke(SPACE, LINK)).rejects.toBeInstanceOf(CoreError);
  });

  it("keeps the viewer's constants in step with kb-types", () => {
    expect(LINK_REFRESH_DEBOUNCE_MS).toBe(60_000);
    expect(LINK_POLL_SECONDS).toBe(60);
    expect(LINK_EXPIRY_MAX_DAYS).toBe(365);
    expect(VIEWER_PATH).toBe("/view");
    expect(VIEWER_CSP).toBe(
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:",
    );
    // The policy names no remote origin.
    expect(VIEWER_CSP).not.toMatch(/https?:/u);
  });
});
