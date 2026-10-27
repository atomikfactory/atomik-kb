// `CoreClient.pageFirstImages` (ADR-0054): the request it sends and the answer it returns; the twin of the core is in memory-content.test.ts.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { PageFirstImage } from "./generated/PageFirstImage";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import { MemoryTransport } from "./memory";
import type { DocTransport, Transport } from "./transport";

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const OTHER = "33333333-3333-4333-8333-333333333333";
const ATTACHMENT = "44444444-4444-4444-8444-444444444444";

const docs: DocTransport = {
  open: () => Promise.resolve(new Uint8Array([1])),
  import: () => Promise.resolve(),
  export: () => Promise.resolve(new Uint8Array()),
  close: () => Promise.resolve(),
};

function stub(response: Response) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs,
  };
  return { requests, client: new CoreClient(transport) };
}

describe("CoreClient.pageFirstImages", () => {
  it("sends the space and a copy of the page ids and returns what the core answers", async () => {
    const answer: PageFirstImage[] = [{ pageId: PAGE, attachmentId: ATTACHMENT }];
    const { client, requests } = stub({ type: "PageFirstImages", data: answer });
    const ids = [PAGE, OTHER];
    await expect(client.pageFirstImages(SPACE, ids)).resolves.toEqual(answer);
    expect(requests).toStrictEqual([{ type: "PageFirstImages", data: { spaceId: SPACE, pageIds: [PAGE, OTHER] } }]);
    expect((requests[0] as Extract<Request, { type: "PageFirstImages" }>).data.pageIds).not.toBe(ids);
  });

  it("answers an empty list when no page has an image", async () => {
    const { client } = stub({ type: "PageFirstImages", data: [] });
    await expect(client.pageFirstImages(SPACE, [PAGE])).resolves.toEqual([]);
  });

  it("rejects when the core answers another kind of response", async () => {
    const { client } = stub({ type: "Ok" });
    await expect(client.pageFirstImages(SPACE, [PAGE])).rejects.toBeInstanceOf(CoreError);
  });

  it("is answered by the in-memory core (a page nobody gave an image has none)", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [{ name: "Personal", pages: [{ title: "Hub" }] }] });
    const client = new CoreClient(memory);
    const [space] = await client.spaceList();
    const [root] = (await client.treeGet(space?.id ?? "")).roots;
    await expect(client.pageFirstImages(space?.id ?? "", [root?.page.id ?? ""])).resolves.toEqual([]);
  });
});
