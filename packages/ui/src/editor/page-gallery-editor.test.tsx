// The gallery in the editor (ADR-0054, section 6): a `pageList` block with `layout: "gallery"` on a page of the whole app draws the
// pages under it as cards (a cover, the first image the core finds, or a plain card), the card size select of the view settings menu writes the view's
// `cardSize` and its mirror in one change, the "New page" card makes a page under the host page, and a read-only editor still
// opens a page from a card. The first image comes from a fake `PageFirstImages` (the core's index is agent A's).

import { CoreClient, demoImagePng } from "@atomik-kb/core-client";
import type { JsonValue, PageId, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { stubObjectUrls } from "../test/attachments";
import { pid } from "../test/editor";
import { renderInterop } from "../test/interop";
import { idsOf, openPage } from "../test/page";
import { viewSelect } from "../test/page-list";
import { parseViews } from "./page-list-views";

afterEach(() => {
  vi.restoreAllMocks();
});

const PHOTO = "a0000000-0000-4000-8000-0000000000b1";

const LEGACY = { source: { kind: "children" }, layout: "gallery", properties: [] as string[], sort: { by: "title", dir: "asc" } };

function hubContent(list: { [key: string]: JsonValue } = {}, extra: JsonValue[] = []): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      { type: "pageList", attrs: { id: pid(2), ...LEGACY, ...list } },
      { type: "paragraph", attrs: { id: pid(3) } },
      ...extra,
    ],
  };
}

function space(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Alpha", cover: "gradient:sunset", icon: "🌱" },
          { title: "Beta", cover: "color:blue" },
          { title: "Photo" },
          { title: "Plain" },
        ],
      },
    ],
  };
}

/** Opens the Hub page of a core where "Photo" has a first image and the other pages have none. */
async function openHub(content: JsonValue = hubContent()) {
  stubObjectUrls();
  let photo: PageId | null = null;
  const app = await renderInterop(
    { spaces: [space(content)], attachments: [{ attachmentId: PHOTO, name: "photo.png", mime: "image/png", bytes: demoImagePng() }] },
    {
      intercept: (request) => {
        if (request.type !== "PageFirstImages") {
          return undefined;
        }
        const data = request.data.pageIds.filter((id) => id === photo).map((id) => ({ pageId: id, attachmentId: PHOTO }));
        return Promise.resolve({ type: "PageFirstImages" as const, data });
      },
    },
  );
  const ids = await idsOf(app.memory);
  photo = ids.page("Photo");
  const editor = await openPage(app.user, "Hub");
  await screen.findByRole("group", { name: t("pageGallery.label") });
  return { ...app, editor, ids };
}

const block = (): HTMLElement => screen.getByRole("group", { name: t("pageList.label") });
const grid = (): HTMLElement => within(block()).getByRole("list", { name: t("pageGallery.grid") });
const cardOf = (title: string): HTMLElement => {
  const found = within(grid())
    .getAllByRole("listitem")
    .find((item) => within(item).queryByRole("button", { name: t("pageGallery.open", { title }) }) !== null);
  if (found === undefined) {
    throw new Error(`no card for ${title}`);
  }
  return found;
};

function attrsOf(editor: Editor): Record<string, unknown> {
  let found: Record<string, unknown> = {};
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageList") {
      found = node.attrs;
    }
    return true;
  });
  return found;
}

describe("the gallery block", () => {
  it("draws a card for each page under the host page, in the view's order, with the right preview on each", async () => {
    await openHub();
    expect(within(grid()).getAllByRole("listitem")).toHaveLength(5); // four pages and "New page"
    expect(
      within(grid())
        .getAllByRole("button")
        .map((button) => button.getAttribute("aria-label") ?? button.textContent),
    ).toEqual([
      ...["Alpha", "Beta", "Photo", "Plain"].map((title) => t("pageGallery.open", { title })),
      t("pageGallery.newCard"),
      // The menu next to "New page" (ADR-0056).
      t("databaseTemplate.menu"),
    ]);
    // A gradient and a colour of the cover, as the page banner draws them.
    expect(cardOf("Alpha").querySelector(".page-gallery-preview")).toHaveAttribute("data-cover-gradient", "sunset");
    expect(cardOf("Alpha")).toHaveTextContent("🌱");
    expect(cardOf("Beta").querySelector(".page-gallery-preview")).toHaveAttribute("data-cover-color", "blue");
    // The first image of a page with no cover, found by the core.
    await waitFor(() => {
      expect(cardOf("Photo").querySelector("img.page-gallery-image")).not.toBeNull();
    });
    expect(cardOf("Photo").querySelector("img.page-gallery-image")?.getAttribute("src")).toMatch(/^blob:test\//);
    // And a plain card, the icon in the picture's place, for the page that has neither.
    expect(cardOf("Plain").querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "icon");
    expect(within(cardOf("Plain")).getByRole("button", { name: t("pageGallery.open", { title: "Plain" }) })).toBeInTheDocument();
  });

  it("changes the card size through the toolbar, writing `cardSize` into the view and keeping the mirror", async () => {
    const { user, editor } = await openHub();
    expect(grid().style.getPropertyValue("--gallery-card-width")).toBe("240px");
    // The card size is the gallery's section of the view settings menu.
    const select = await viewSelect(user, block(), t("pageGallery.size"));
    await user.selectOptions(select, t("pageGallery.size.l"));
    await waitFor(() => {
      expect(grid().style.getPropertyValue("--gallery-card-width")).toBe("320px");
    });
    const attrs = attrsOf(editor);
    expect(attrs.layout).toBe("gallery");
    const [view, ...rest] = parseViews(attrs);
    expect(rest).toEqual([]);
    expect(view).toMatchObject({ layout: "gallery", cardSize: "l" });
    await user.selectOptions(select, t("pageGallery.size.m"));
    await waitFor(() => {
      expect(grid().style.getPropertyValue("--gallery-card-width")).toBe("240px");
    });
    expect(parseViews(attrsOf(editor))[0]?.cardSize).toBeUndefined();
  });

  it("makes a page under the host page from the New page card", async () => {
    const { user, memory, ids } = await openHub();
    const client = new CoreClient(memory);
    const childrenOfHub = async () => (await client.treeGet(ids.space("Personal"))).roots.find((node) => node.page.id === ids.page("Hub"))?.children.length;
    expect(await childrenOfHub()).toBe(4);
    await user.click(within(grid()).getByRole("button", { name: t("pageGallery.newCard") }));
    await waitFor(async () => {
      expect(await childrenOfHub()).toBe(5);
    });
  });

  it("opens a page from a card in a read-only editor, which has no toolbar and no New page card", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = hubContent({ views: [{ id: "v1", name: "", layout: "gallery", properties: [], sort: { by: "title", dir: "asc" }, openIn: "page" }] }, [
      { type: "kanban", attrs: { id: "k0", columns: 3 } },
    ]);
    const { user, store, ids } = await openHub(content);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(within(block()).queryByRole("toolbar")).not.toBeInTheDocument();
    expect(within(block()).queryByRole("button", { name: t("pageGallery.newCard") })).not.toBeInTheDocument();
    expect(within(grid()).getAllByRole("listitem")).toHaveLength(4);
    await user.click(within(cardOf("Beta")).getByRole("button", { name: t("pageGallery.open", { title: "Beta" }) }));
    await waitFor(() => {
      expect(store.getState().selectedPageId).toBe(ids.page("Beta"));
    });
  });
});
