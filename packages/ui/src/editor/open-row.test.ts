// How a click on a row opens its page (ADR-0054, `docs/architecture/database-views.md` section 8): the table of
// dispositions and `openIn`, the pages that are never peeked, and the guards (a page in the trash or gone, a space that was
// left, a page of another space). Pure: a fake of the store's members.

import type { PageInfo } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { openRow } from "./open-row";
import type { OpenRowStore } from "./open-row";
import type { OpenIn } from "./page-list-views";

const SPACE = "space-1";
const ROW = { pageId: "page-row", spaceId: SPACE };
const HOST = "page-host";
const FRONT = "page-front";

function fake(selectedPageId: string | null = FRONT, more: Partial<OpenRowStore> = {}) {
  const navigateTo = vi.fn();
  const openPeek = vi.fn();
  const closePeek = vi.fn();
  const store = { navigateTo, openPeek, closePeek, selectedPageId, ...more } as unknown as OpenRowStore;
  return { store, navigateTo, openPeek, closePeek };
}

/** What the store knows when a page is open in `SPACE`, the account is in it and one more space, and nothing is wrong with the row. */
const KNOWN = { currentSpaceId: SPACE, spaces: [{ id: SPACE }, { id: "space-2" }] } as unknown as Partial<OpenRowStore>;

function lookupOf(page: Partial<PageInfo> | null): Partial<OpenRowStore> {
  const entry = page === null ? { status: "missing" } : { status: "found", page: { id: ROW.pageId, spaceId: SPACE, title: "Row", ...page } };
  return { ...KNOWN, pageLookup: { [ROW.pageId]: entry } } as unknown as Partial<OpenRowStore>;
}

describe("a plain click", () => {
  it("opens the side peek by default", () => {
    const { store, navigateTo, openPeek } = fake();
    openRow(store, ROW, {}, "current", HOST);
    expect(openPeek).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: SPACE, mode: "side" });
    expect(navigateTo).not.toHaveBeenCalled();
  });

  it.each<[OpenIn, string]>([
    ["side", "side"],
    ["center", "center"],
  ])("opens the %s peek when the view says so", (openIn, mode) => {
    const { store, navigateTo, openPeek } = fake();
    openRow(store, ROW, { openIn }, "current", HOST);
    expect(openPeek).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: SPACE, mode });
    expect(navigateTo).not.toHaveBeenCalled();
  });

  it("navigates the tab in front, and closes the peek, for `page`", () => {
    const { store, navigateTo, openPeek, closePeek } = fake();
    openRow(store, ROW, { openIn: "page" }, "current", HOST);
    expect(navigateTo).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: SPACE, disposition: "current" });
    expect(closePeek).toHaveBeenCalledOnce();
    expect(openPeek).not.toHaveBeenCalled();
  });

  it("opens the page that holds the database, and the page in front, as a page whatever the view says", () => {
    for (const openIn of ["side", "center"] as const) {
      const host = fake();
      openRow(host.store, { pageId: HOST, spaceId: SPACE }, { openIn }, "current", HOST);
      expect(host.openPeek).not.toHaveBeenCalled();
      expect(host.navigateTo).toHaveBeenCalledExactlyOnceWith({ pageId: HOST, spaceId: SPACE, disposition: "current" });
      expect(host.closePeek).toHaveBeenCalledOnce();

      const front = fake();
      openRow(front.store, { pageId: FRONT, spaceId: SPACE }, { openIn }, "current", HOST);
      expect(front.openPeek).not.toHaveBeenCalled();
      expect(front.navigateTo).toHaveBeenCalledExactlyOnceWith({ pageId: FRONT, spaceId: SPACE, disposition: "current" });
    }
  });

  it("peeks any page when no page is in front and the host is not known", () => {
    const { store, openPeek } = fake(null);
    openRow(store, ROW, {}, "current", null);
    expect(openPeek).toHaveBeenCalledOnce();
  });
});

describe("a gesture that asks for a tab", () => {
  it.each<["background" | "foreground"]>([["background"], ["foreground"]])("opens a %s tab, whatever `openIn` says, and leaves the peek alone", (disposition) => {
    for (const openIn of ["side", "center", "page"] as const) {
      const { store, navigateTo, openPeek, closePeek } = fake();
      openRow(store, ROW, { openIn }, disposition, HOST);
      expect(navigateTo, openIn).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: SPACE, disposition });
      expect(openPeek).not.toHaveBeenCalled();
      expect(closePeek).not.toHaveBeenCalled();
    }
  });
});

describe("the guards", () => {
  it("peeks as before when the store knows the space and nothing about the page", () => {
    const { store, openPeek, navigateTo } = fake(FRONT, KNOWN);
    openRow(store, ROW, {}, "current", HOST);
    expect(openPeek).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: SPACE, mode: "side" });
    expect(navigateTo).not.toHaveBeenCalled();
  });

  it("peeks a page the lookup knows is alive", () => {
    const { store, openPeek } = fake(FRONT, lookupOf({}));
    openRow(store, ROW, { openIn: "center" }, "current", HOST);
    expect(openPeek).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: SPACE, mode: "center" });
  });

  it.each<[string, Partial<OpenRowStore>]>([
    ["in the trash", lookupOf({ trashedAt: 1_700_000_000_000 })],
    ["gone", lookupOf(null)],
  ])("does nothing for a page that is %s, whatever the click", (_name, more) => {
    for (const disposition of ["current", "background", "foreground"] as const) {
      for (const openIn of ["side", "center", "page"] as const) {
        const { store, navigateTo, openPeek, closePeek } = fake(FRONT, more);
        openRow(store, ROW, { openIn }, disposition, HOST);
        expect(navigateTo).not.toHaveBeenCalled();
        expect(openPeek).not.toHaveBeenCalled();
        expect(closePeek).not.toHaveBeenCalled();
      }
    }
  });

  it("does nothing for a space this account left, whatever the click", () => {
    const gone = { spaceId: "space-left" };
    for (const disposition of ["current", "background", "foreground"] as const) {
      const { store, navigateTo, openPeek, closePeek } = fake(FRONT, KNOWN);
      openRow(store, { pageId: "page-row", ...gone }, { openIn: "page" }, disposition, HOST);
      expect(navigateTo).not.toHaveBeenCalled();
      expect(openPeek).not.toHaveBeenCalled();
      expect(closePeek).not.toHaveBeenCalled();
    }
  });

  it("opens a page of another space as a page: the peek shows the space in front", () => {
    const { store, navigateTo, openPeek, closePeek } = fake(FRONT, KNOWN);
    openRow(store, { pageId: "page-row", spaceId: "space-2" }, { openIn: "side" }, "current", HOST);
    expect(openPeek).not.toHaveBeenCalled();
    expect(closePeek).toHaveBeenCalledOnce();
    expect(navigateTo).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: "space-2", disposition: "current" });
  });

  it("still opens a tab for a page of another space", () => {
    const { store, navigateTo, openPeek } = fake(FRONT, KNOWN);
    openRow(store, { pageId: "page-row", spaceId: "space-2" }, {}, "background", HOST);
    expect(navigateTo).toHaveBeenCalledExactlyOnceWith({ pageId: "page-row", spaceId: "space-2", disposition: "background" });
    expect(openPeek).not.toHaveBeenCalled();
  });

  it("does not guard what it cannot know (no spaces read yet)", () => {
    const { store, openPeek } = fake(FRONT, { currentSpaceId: null, spaces: [] });
    openRow(store, ROW, {}, "current", HOST);
    expect(openPeek).toHaveBeenCalledOnce();
  });
});
