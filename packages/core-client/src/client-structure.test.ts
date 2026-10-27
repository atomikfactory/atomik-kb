import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { PageInfo } from "./generated/PageInfo";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SpaceInfo } from "./generated/SpaceInfo";
import type { TreeView } from "./generated/TreeView";
import type { Workspace } from "./generated/Workspace";
import type { DocTransport, Transport } from "./transport";

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const PARENT = "33333333-3333-4333-8333-333333333333";

// Records as the shell sends them: JSON text, so timestamps are plain numbers.
const space = JSON.parse(
  `{"id":"${SPACE}","name":"Personal","createdAt":1700000000000,"pageCount":3}`,
) as SpaceInfo;
const page = JSON.parse(
  `{"id":"${PAGE}","spaceId":"${SPACE}","title":"Plan","createdAt":1700000000000,"updatedAt":1700000000001}`,
) as PageInfo;
const tree: TreeView = {
  spaceId: SPACE,
  roots: [{ page, children: [] }],
  trashed: [],
  undo: { canUndo: true, canRedo: false },
};
const workspace = JSON.parse(
  '{"id":"44444444-4444-4444-8444-444444444444","name":"Home","createdAt":1700000000000}',
) as Workspace;

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
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

describe("CoreClient workspaceCreate with a first space", () => {
  it("sends firstSpaceName when given and leaves it out otherwise", async () => {
    const withSpace = stubTransport({ type: "Workspace", data: workspace });
    await expect(withSpace.client.workspaceCreate("Home", "Personal")).resolves.toEqual(workspace);
    expect(withSpace.requests).toEqual([
      { type: "WorkspaceCreate", data: { name: "Home", firstSpaceName: "Personal" } },
    ]);

    const without = stubTransport({ type: "Workspace", data: workspace });
    await without.client.workspaceCreate("Home");
    expect(without.requests[0]).toStrictEqual({ type: "WorkspaceCreate", data: { name: "Home" } });
  });
});

describe("CoreClient space helpers", () => {
  it("spaceList sends SpaceList and returns the spaces", async () => {
    const { client, requests } = stubTransport({ type: "Spaces", data: [space] });
    await expect(client.spaceList()).resolves.toEqual([space]);
    expect(requests).toEqual([{ type: "SpaceList" }]);
  });

  it("spaceCreate sends the name, and the icon only when given", async () => {
    const plain = stubTransport({ type: "Space", data: space });
    await expect(plain.client.spaceCreate("Personal")).resolves.toEqual(space);
    expect(plain.requests[0]).toStrictEqual({ type: "SpaceCreate", data: { name: "Personal" } });

    const withIcon = stubTransport({ type: "Space", data: space });
    await withIcon.client.spaceCreate("Personal", "🏠");
    expect(withIcon.requests).toEqual([{ type: "SpaceCreate", data: { name: "Personal", icon: "🏠" } }]);
  });

  it("spaceRename and spaceSetIcon send the space id and return the space", async () => {
    const rename = stubTransport({ type: "Space", data: space });
    await expect(rename.client.spaceRename(SPACE, "Work")).resolves.toEqual(space);
    expect(rename.requests).toEqual([{ type: "SpaceRename", data: { spaceId: SPACE, name: "Work" } }]);

    const setIcon = stubTransport({ type: "Space", data: space });
    await expect(setIcon.client.spaceSetIcon(SPACE, "💼")).resolves.toEqual(space);
    expect(setIcon.requests).toEqual([{ type: "SpaceSetIcon", data: { spaceId: SPACE, icon: "💼" } }]);

    const clear = stubTransport({ type: "Space", data: space });
    await clear.client.spaceSetIcon(SPACE, null);
    expect(clear.requests).toEqual([{ type: "SpaceSetIcon", data: { spaceId: SPACE, icon: null } }]);
  });

  it("spaceMove sends the index and returns the new order", async () => {
    const { client, requests } = stubTransport({ type: "Spaces", data: [space] });
    await expect(client.spaceMove(SPACE, 2)).resolves.toEqual([space]);
    expect(requests).toEqual([{ type: "SpaceMove", data: { spaceId: SPACE, index: 2 } }]);
  });

  it("spaceDelete sends the space id and resolves with nothing", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.spaceDelete(SPACE)).resolves.toBeUndefined();
    expect(requests).toEqual([{ type: "SpaceDelete", data: { spaceId: SPACE } }]);
  });
});

describe("CoreClient page helpers", () => {
  it("treeGet sends the space id and returns the tree view", async () => {
    const { client, requests } = stubTransport({ type: "Tree", data: tree });
    await expect(client.treeGet(SPACE)).resolves.toEqual(tree);
    expect(requests).toEqual([{ type: "TreeGet", data: { spaceId: SPACE } }]);
  });

  it("pageGet sends the page id and returns the page", async () => {
    const { client, requests } = stubTransport({ type: "Page", data: page });
    await expect(client.pageGet(PAGE)).resolves.toEqual(page);
    expect(requests).toEqual([{ type: "PageGet", data: { pageId: PAGE } }]);
  });

  it("pageCreate sends only the fields that were given", async () => {
    const root = stubTransport({ type: "Page", data: page });
    await expect(root.client.pageCreate({ spaceId: SPACE, title: "Plan" })).resolves.toEqual(page);
    expect(root.requests[0]).toStrictEqual({ type: "PageCreate", data: { spaceId: SPACE, title: "Plan" } });

    const child = stubTransport({ type: "Page", data: page });
    await child.client.pageCreate({ spaceId: SPACE, parentId: PARENT, index: 0, title: "" });
    expect(child.requests).toEqual([
      { type: "PageCreate", data: { spaceId: SPACE, parentId: PARENT, index: 0, title: "" } },
    ]);

    const nullParent = stubTransport({ type: "Page", data: page });
    await nullParent.client.pageCreate({ spaceId: SPACE, parentId: null, title: "Plan" });
    expect(nullParent.requests[0]).toStrictEqual({
      type: "PageCreate",
      data: { spaceId: SPACE, title: "Plan" },
    });
  });

  it("pageRename and pageSetIcon send the page id and return the page", async () => {
    const rename = stubTransport({ type: "Page", data: page });
    await expect(rename.client.pageRename(PAGE, "New")).resolves.toEqual(page);
    expect(rename.requests).toEqual([{ type: "PageRename", data: { pageId: PAGE, title: "New" } }]);

    const setIcon = stubTransport({ type: "Page", data: page });
    await expect(setIcon.client.pageSetIcon(PAGE, "📝")).resolves.toEqual(page);
    expect(setIcon.requests).toEqual([{ type: "PageSetIcon", data: { pageId: PAGE, icon: "📝" } }]);

    const clear = stubTransport({ type: "Page", data: page });
    await clear.client.pageSetIcon(PAGE, null);
    expect(clear.requests).toEqual([{ type: "PageSetIcon", data: { pageId: PAGE, icon: null } }]);
  });

  it("pageMove sends the parent only when there is one", async () => {
    const under = stubTransport({ type: "Ok" });
    await expect(under.client.pageMove(PAGE, PARENT, 1)).resolves.toBeUndefined();
    expect(under.requests).toEqual([{ type: "PageMove", data: { pageId: PAGE, parentId: PARENT, index: 1 } }]);

    const root = stubTransport({ type: "Ok" });
    await root.client.pageMove(PAGE, null, 0);
    expect(root.requests[0]).toStrictEqual({ type: "PageMove", data: { pageId: PAGE, index: 0 } });
  });

  it("pageReorderChildren sends the space, the order and the parent only when there is one", async () => {
    const under = stubTransport({ type: "Ok" });
    await expect(under.client.pageReorderChildren(SPACE, PARENT, [PAGE, PARENT])).resolves.toBeUndefined();
    expect(under.requests).toEqual([
      { type: "PageReorderChildren", data: { spaceId: SPACE, parentId: PARENT, order: [PAGE, PARENT] } },
    ]);

    const root = stubTransport({ type: "Ok" });
    await root.client.pageReorderChildren(SPACE, null, [PAGE]);
    expect(root.requests[0]).toStrictEqual({
      type: "PageReorderChildren",
      data: { spaceId: SPACE, order: [PAGE] },
    });
  });

  it("pageTrash, pageRestore and pagePurge send the page id", async () => {
    const trash = stubTransport({ type: "Ok" });
    await expect(trash.client.pageTrash(PAGE)).resolves.toBeUndefined();
    expect(trash.requests).toEqual([{ type: "PageTrash", data: { pageId: PAGE } }]);

    const restore = stubTransport({ type: "Page", data: page });
    await expect(restore.client.pageRestore(PAGE)).resolves.toEqual(page);
    expect(restore.requests).toEqual([{ type: "PageRestore", data: { pageId: PAGE } }]);

    const purge = stubTransport({ type: "Ok" });
    await expect(purge.client.pagePurge(PAGE)).resolves.toBeUndefined();
    expect(purge.requests).toEqual([{ type: "PagePurge", data: { pageId: PAGE } }]);
  });

  it("pageDuplicate sends the page id and returns the copy of the page", async () => {
    const copy: PageInfo = { ...page, id: PARENT, title: "Plan (copy)" };
    const { client, requests } = stubTransport({ type: "Page", data: copy });
    await expect(client.pageDuplicate(PAGE)).resolves.toEqual(copy);
    expect(requests).toEqual([{ type: "PageDuplicate", data: { pageId: PAGE } }]);
  });

  it("pageFavoriteSet sends the page id and the flag", async () => {
    const add = stubTransport({ type: "Ok" });
    await expect(add.client.pageFavoriteSet(PAGE, true)).resolves.toBeUndefined();
    expect(add.requests).toEqual([{ type: "PageFavoriteSet", data: { pageId: PAGE, favorite: true } }]);

    const remove = stubTransport({ type: "Ok" });
    await remove.client.pageFavoriteSet(PAGE, false);
    expect(remove.requests).toEqual([{ type: "PageFavoriteSet", data: { pageId: PAGE, favorite: false } }]);
  });

  it("pageFavoriteMove sends `before` only when there is one", async () => {
    const front = stubTransport({ type: "Ok" });
    await expect(front.client.pageFavoriteMove(PAGE, PARENT)).resolves.toBeUndefined();
    expect(front.requests).toEqual([{ type: "PageFavoriteMove", data: { pageId: PAGE, before: PARENT } }]);

    const last = stubTransport({ type: "Ok" });
    await last.client.pageFavoriteMove(PAGE);
    expect(last.requests[0]).toStrictEqual({ type: "PageFavoriteMove", data: { pageId: PAGE } });

    const nullBefore = stubTransport({ type: "Ok" });
    await nullBefore.client.pageFavoriteMove(PAGE, null);
    expect(nullBefore.requests[0]).toStrictEqual({ type: "PageFavoriteMove", data: { pageId: PAGE } });
  });

  it("treeGet returns the favourites the core lists", async () => {
    const withFavorites: TreeView = { ...tree, favorites: [PAGE, PARENT] };
    const { client } = stubTransport({ type: "Tree", data: withFavorites });
    await expect(client.treeGet(SPACE)).resolves.toEqual(withFavorites);
  });

  it("structureUndo and structureRedo return the tree as it is afterwards", async () => {
    const undo = stubTransport({ type: "Tree", data: tree });
    await expect(undo.client.structureUndo(SPACE)).resolves.toEqual(tree);
    expect(undo.requests).toEqual([{ type: "StructureUndo", data: { spaceId: SPACE } }]);

    const redo = stubTransport({ type: "Tree", data: tree });
    await expect(redo.client.structureRedo(SPACE)).resolves.toEqual(tree);
    expect(redo.requests).toEqual([{ type: "StructureRedo", data: { spaceId: SPACE } }]);
  });
});

describe("CoreClient response narrowing for the structure requests", () => {
  it("rejects with an Internal error naming the request when the variant is unexpected", async () => {
    const calls: [string, Response, (client: CoreClient) => Promise<unknown>][] = [
      ["SpaceList", { type: "Ok" }, (c) => c.spaceList()],
      ["SpaceCreate", { type: "Ok" }, (c) => c.spaceCreate("x")],
      ["SpaceRename", { type: "Ok" }, (c) => c.spaceRename(SPACE, "x")],
      ["SpaceSetIcon", { type: "Ok" }, (c) => c.spaceSetIcon(SPACE, null)],
      ["SpaceMove", { type: "Ok" }, (c) => c.spaceMove(SPACE, 0)],
      ["SpaceDelete", { type: "Spaces", data: [] }, (c) => c.spaceDelete(SPACE)],
      ["TreeGet", { type: "Ok" }, (c) => c.treeGet(SPACE)],
      ["PageGet", { type: "Ok" }, (c) => c.pageGet(PAGE)],
      ["PageCreate", { type: "Ok" }, (c) => c.pageCreate({ spaceId: SPACE, title: "x" })],
      ["PageRename", { type: "Ok" }, (c) => c.pageRename(PAGE, "x")],
      ["PageSetIcon", { type: "Ok" }, (c) => c.pageSetIcon(PAGE, null)],
      ["PageMove", { type: "Page", data: page }, (c) => c.pageMove(PAGE, null, 0)],
      ["PageReorderChildren", { type: "Page", data: page }, (c) => c.pageReorderChildren(SPACE, null, [PAGE])],
      ["PageDuplicate", { type: "Ok" }, (c) => c.pageDuplicate(PAGE)],
      ["PageTrash", { type: "Page", data: page }, (c) => c.pageTrash(PAGE)],
      ["PageRestore", { type: "Ok" }, (c) => c.pageRestore(PAGE)],
      ["PagePurge", { type: "Page", data: page }, (c) => c.pagePurge(PAGE)],
      ["PageFavoriteSet", { type: "Page", data: page }, (c) => c.pageFavoriteSet(PAGE, true)],
      ["PageFavoriteMove", { type: "Page", data: page }, (c) => c.pageFavoriteMove(PAGE)],
      ["StructureUndo", { type: "Ok" }, (c) => c.structureUndo(SPACE)],
      ["StructureRedo", { type: "Ok" }, (c) => c.structureRedo(SPACE)],
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

  it("does not mix up the Space, Spaces, Tree and Page variants", async () => {
    const { client } = stubTransport({ type: "Space", data: space });
    await expect(client.spaceList()).rejects.toMatchObject({
      message: expect.stringContaining("expected Spaces, got Space") as unknown,
    });
    await expect(client.treeGet(SPACE)).rejects.toMatchObject({
      message: expect.stringContaining("expected Tree, got Space") as unknown,
    });
    await expect(client.pageGet(PAGE)).rejects.toMatchObject({
      message: expect.stringContaining("expected Page, got Space") as unknown,
    });
  });
});
