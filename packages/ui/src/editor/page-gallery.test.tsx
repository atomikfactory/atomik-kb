// The gallery layout drawn with props (ADR-0054, section 6): the cards (preview, icon, title, shown properties), the card sizes,
// the "New page" card, how a card opens its page, the previews (cover, first image, plain card, a cover that cannot be drawn),
// the laziness of the pictures and the release of their blob URLs, the settings section, and the first-image lookup hook with a
// fake `PageFirstImages` that the test controls.

import { CoreClient, CoreError, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentId, PageId, PageInfo, PropertyDef, Response, SpaceId } from "@atomik-kb/core-client";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

import { t } from "../i18n";
import { iconImagesOf } from "../icons/icon-images";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { stubObjectUrls } from "../test/attachments";
import { interopTransport } from "../test/interop";
import { NOW } from "../test/render";
import { EMPTY_FILTER_GROUP } from "./page-list-filter-tree";
import { PageListEmptyContext } from "./page-list-empty";
import type { PageListLayoutProps } from "./page-list-layout-types";
import type { PageListViewSpec } from "./page-list-views";
import { NearViewport, REFRESH_DELAY_MS, usePageFirstImages } from "./page-gallery-images";
import { PageGalleryView, PageGallerySettings } from "./PageGalleryView";

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const OTHER_SPACE: SpaceId = "11111111-1111-4111-8111-222222222222";
const PNG_A: AttachmentId = "a0000000-0000-4000-8000-0000000000a1";
const PNG_B: AttachmentId = "a0000000-0000-4000-8000-0000000000a2";
const NOTE: AttachmentId = "a0000000-0000-4000-8000-0000000000a3";

const pageId = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;

function page(n: number, extra: Partial<PageInfo> = {}): PageInfo {
  return { id: pageId(n), spaceId: SPACE, title: `Page ${String(n)}`, createdAt: 1, updatedAt: 1, ...extra };
}

const STATUS: PropertyDef = {
  id: "de000000-0000-4000-8000-00000000d001",
  name: "Status",
  kind: "select",
  options: [{ id: "opt-todo", name: "Todo", color: "blue" }],
  createdAt: 1,
};
const COST: PropertyDef = { id: "de000000-0000-4000-8000-00000000d002", name: "Cost", kind: "number", options: [], createdAt: 1 };
const NOTES: PropertyDef = { id: "de000000-0000-4000-8000-00000000d003", name: "Notes", kind: "text", options: [], createdAt: 1 };

const VIEW: PageListViewSpec = {
  id: "v1",
  name: "",
  layout: "gallery",
  properties: [],
  sort: { by: "title", dir: "asc" },
  sorts: [{ by: "title", dir: "asc" }],
  filter: EMPTY_FILTER_GROUP,
};

/** The first image the fake core answers for a page; set before the gallery asks. */
type Answers = Map<PageId, AttachmentId>;

/** A memory core with three attachments, a wrapped transport that answers `PageFirstImages` from `answers`, and a store over it. */
function setup(answers: Answers = new Map()) {
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Hub" }] }],
    attachments: [
      { attachmentId: PNG_A, name: "a.png", mime: "image/png", bytes: demoImagePng() },
      { attachmentId: PNG_B, name: "b.png", mime: "image/png", bytes: demoImagePng() },
      { attachmentId: NOTE, name: "notes.txt", mime: "text/plain", bytes: new TextEncoder().encode("not a picture") },
    ],
  });
  const control: { gate: Promise<void> | null; fail: CoreError | null } = { gate: null, fail: null };
  const wrapped = interopTransport(memory, {
    // A request other than `PageFirstImages` goes to the memory core: `undefined` has to come back at once, not in a promise.
    intercept: (request) => {
      if (request.type !== "PageFirstImages") {
        return undefined;
      }
      const ids = request.data.pageIds;
      const answer = async (): Promise<Response> => {
        if (control.gate !== null) {
          await control.gate;
        }
        if (control.fail !== null) {
          throw control.fail;
        }
        const data = ids.flatMap((id) => {
          const attachmentId = answers.get(id);
          return attachmentId === undefined ? [] : [{ pageId: id, attachmentId }];
        });
        return { type: "PageFirstImages", data };
      };
      return answer();
    },
  });
  onTestFinished(() => {
    wrapped.detach();
  });
  const store = createAppStore(new CoreClient(wrapped.transport), { corePollMs: 5 });
  const firstImageRequests = () =>
    wrapped.requests.flatMap((request) => (request.type === "PageFirstImages" ? [request.data.pageIds] : []));
  return { memory, store, answers, control, firstImageRequests, ...wrapped };
}

type Harness = ReturnType<typeof setup>;

function props(extra: Partial<PageListLayoutProps> = {}): PageListLayoutProps {
  return {
    rows: [],
    view: VIEW,
    schema: [],
    columns: [],
    spaceId: SPACE,
    editable: true,
    today: "2026-10-03",
    onOpen: vi.fn(),
    onUpdateView: vi.fn(),
    onCreatePage: vi.fn(),
    creating: false,
    ...extra,
  };
}

function renderGallery(harness: Harness, extra: Partial<PageListLayoutProps> = {}, empty: string | null = null) {
  const ui = (next: Partial<PageListLayoutProps>) => (
    <StoreContext value={harness.store}>
      <PageListEmptyContext value={empty}>
        <PageGalleryView {...props({ ...extra, ...next })} />
      </PageListEmptyContext>
    </StoreContext>
  );
  const view = render(ui({}));
  return { ...view, again: (next: Partial<PageListLayoutProps>) => view.rerender(ui(next)) };
}

const card = (n: number): HTMLElement => {
  const found = document.querySelector<HTMLElement>(`[data-page="${pageId(n)}"]`);
  if (found === null) {
    throw new Error(`no card for page ${String(n)}`);
  }
  return found;
};
const pictureOf = (n: number): HTMLImageElement | null => card(n).querySelector("img.page-gallery-image");
const titleButton = (title: string): HTMLElement => screen.getByRole("button", { name: t("pageGallery.open", { title }) });

let urls: ReturnType<typeof stubObjectUrls>;

beforeEach(() => {
  urls = stubObjectUrls();
});

afterEach(async () => {
  cleanup();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  // The icon-image cache revokes a blob URL a turn after its last use: let that happen while this test's spies are in place.
  await new Promise((resolve) => setTimeout(resolve, 0));
});

describe("the cards", () => {
  it("draws a card for each row, in the order given, each a button named for its title, with the page's icon", () => {
    renderGallery(setup(), { rows: [page(2, { title: "Beta", icon: "🌱" }), page(1, { title: "Alpha" }), page(3, { title: "" })] });
    const grid = screen.getByRole("list", { name: t("pageGallery.grid") });
    const cards = within(grid).getAllByRole("listitem");
    // Three cards and the "New page" card.
    expect(cards).toHaveLength(4);
    expect(cards.slice(0, 3).map((item) => within(item).getByRole("button").getAttribute("aria-label"))).toEqual([
      t("pageGallery.open", { title: "Beta" }),
      t("pageGallery.open", { title: "Alpha" }),
      t("pageGallery.open", { title: t("page.untitled") }),
    ]);
    // The icon is in the title and, on a card with no picture, large in the preview area too.
    expect(within(card(2)).getAllByText("🌱")).toHaveLength(2);
    expect(card(1).querySelector(".page-gallery-title .page-icon")).toHaveAttribute("data-icon-kind", "default");
    expect(screen.getByRole("group", { name: t("pageGallery.label") })).toBeInTheDocument();
  });

  it("shows the properties the view shows, read-only, only the ones the page has", () => {
    const rows = [
      page(1, {
        properties: [
          [STATUS.id, { kind: "select", value: "opt-todo" }],
          [COST.id, { kind: "number", value: 4.5 }],
        ],
      }),
      page(2, { properties: [[COST.id, { kind: "number", value: 7 }]] }),
      page(3),
    ];
    renderGallery(setup(), { rows, columns: [STATUS, COST, NOTES] });
    const first = within(card(1)).getAllByRole("group");
    expect(first.map((group) => group.getAttribute("aria-label"))).toEqual(["Status", "Cost"]);
    expect(first[0]).toHaveTextContent("Todo");
    expect(first[1]).toHaveTextContent("4.5");
    expect(within(card(2)).getAllByRole("group").map((group) => group.getAttribute("aria-label"))).toEqual(["Cost"]);
    expect(within(card(3)).queryAllByRole("group")).toHaveLength(0);
    // Nothing to edit in a card: only the title (and no inputs).
    expect(within(card(1)).getAllByRole("button")).toHaveLength(1);
    expect(card(1).querySelector("input, textarea, select")).toBeNull();
  });

  it("is a grid that sizes its cards by the view: 240 px (medium) by default, 160 (small), 320 (large)", () => {
    const harness = setup();
    const { again } = renderGallery(harness, { rows: [page(1)] });
    const gallery = screen.getByRole("group", { name: t("pageGallery.label") });
    const grid = screen.getByRole("list", { name: t("pageGallery.grid") });
    expect(gallery).toHaveAttribute("data-size", "m");
    expect(grid.style.getPropertyValue("--gallery-card-width")).toBe("240px");
    again({ rows: [page(1)], view: { ...VIEW, cardSize: "s" } });
    expect(gallery).toHaveAttribute("data-size", "s");
    expect(grid.style.getPropertyValue("--gallery-card-width")).toBe("160px");
    again({ rows: [page(1)], view: { ...VIEW, cardSize: "l" } });
    expect(grid.style.getPropertyValue("--gallery-card-width")).toBe("320px");
  });

  it("says what the node view says when there are no rows, and draws only the New page card then", () => {
    renderGallery(setup(), { rows: [] }, "No pages yet");
    expect(screen.getByText("No pages yet")).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: t("pageGallery.grid") })).getAllByRole("listitem")).toHaveLength(1);
  });

  it("draws nothing at all for no rows in a read-only editor, and no message while the node view says none", () => {
    renderGallery(setup(), { rows: [], editable: false, onCreatePage: null }, null);
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByText(t("pageGallery.newCard"))).toBeNull();
  });
});

describe("the New page card", () => {
  it("is the last card and asks for a page with no initial values", async () => {
    const onCreatePage = vi.fn();
    renderGallery(setup(), { rows: [page(1), page(2)], onCreatePage });
    const items = within(screen.getByRole("list", { name: t("pageGallery.grid") })).getAllByRole("listitem");
    const last = items[items.length - 1];
    expect(last).toBeDefined();
    const button = within(last as HTMLElement).getByRole("button", { name: t("pageGallery.newCard") });
    await userEvent.click(button);
    expect(onCreatePage).toHaveBeenCalledTimes(1);
    expect(onCreatePage).toHaveBeenCalledWith();
  });

  it("is disabled while a page is being made", () => {
    renderGallery(setup(), { rows: [page(1)], creating: true });
    expect(screen.getByRole("button", { name: t("pageGallery.newCard") })).toBeDisabled();
  });

  it("is not offered when the layout may not make pages (a read-only editor, no host page)", () => {
    renderGallery(setup(), { rows: [page(1)], editable: false, onCreatePage: null });
    expect(screen.queryByRole("button", { name: t("pageGallery.newCard") })).toBeNull();
    expect(within(screen.getByRole("list", { name: t("pageGallery.grid") })).getAllByRole("listitem")).toHaveLength(1);
  });
});

describe("opening a page", () => {
  it("opens through the title button once, with the disposition of the gesture", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    renderGallery(setup(), { rows: [page(1, { title: "Alpha" })], onOpen });
    await user.click(titleButton("Alpha"));
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenLastCalledWith(expect.objectContaining({ id: pageId(1) }), "current");
    await user.keyboard("{Control>}");
    await user.click(titleButton("Alpha"));
    await user.keyboard("{/Control}");
    expect(onOpen).toHaveBeenCalledTimes(2);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "background");
    await user.keyboard("{Control>}{Shift>}");
    await user.click(titleButton("Alpha"));
    await user.keyboard("{/Shift}{/Control}");
    expect(onOpen).toHaveBeenCalledTimes(3);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "foreground");
    fireEvent(titleButton("Alpha"), new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    expect(onOpen).toHaveBeenCalledTimes(4);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "background");
  });

  it("opens from the keyboard: Enter on the focused title, Ctrl+Enter in a tab in front, Space like a click", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    renderGallery(setup(), { rows: [page(1, { title: "Alpha" })], onOpen });
    await user.tab();
    expect(titleButton("Alpha")).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenLastCalledWith(expect.objectContaining({ id: pageId(1) }), "current");
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(onOpen).toHaveBeenCalledTimes(2);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "foreground");
    await user.keyboard(" ");
    expect(onOpen).toHaveBeenCalledTimes(3);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "current");
  });

  it("opens from anywhere on the card, once, with the gesture's disposition", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    renderGallery(setup(), {
      rows: [page(1, { cover: "color:blue", properties: [[COST.id, { kind: "number", value: 4.5 }]] })],
      columns: [COST],
      onOpen,
    });
    await user.click(card(1).querySelector(".page-gallery-preview") as HTMLElement);
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenLastCalledWith(expect.objectContaining({ id: pageId(1) }), "current");
    await user.click(within(card(1)).getByRole("group", { name: "Cost" }));
    expect(onOpen).toHaveBeenCalledTimes(2);
    await user.keyboard("{Control>}");
    await user.click(card(1));
    await user.keyboard("{/Control}");
    expect(onOpen).toHaveBeenCalledTimes(3);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "background");
    fireEvent(card(1), new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    expect(onOpen).toHaveBeenCalledTimes(4);
    expect(onOpen).toHaveBeenLastCalledWith(expect.anything(), "background");
  });

  it("opens in a read-only editor too", async () => {
    const onOpen = vi.fn();
    renderGallery(setup(), { rows: [page(1, { title: "Alpha" })], editable: false, onCreatePage: null, onOpen });
    await userEvent.click(titleButton("Alpha"));
    await userEvent.click(card(1));
    expect(onOpen).toHaveBeenCalledTimes(2);
  });

  it("does not open from the New page card", async () => {
    const onOpen = vi.fn();
    renderGallery(setup(), { rows: [], onOpen });
    await userEvent.click(screen.getByRole("button", { name: t("pageGallery.newCard") }));
    expect(onOpen).not.toHaveBeenCalled();
  });
});

describe("the preview of a card", () => {
  it("is a colour or a gradient of the page's cover, drawn like the page banner, and no picture is read", async () => {
    const harness = setup();
    renderGallery(harness, { rows: [page(1, { cover: "color:green" }), page(2, { cover: "gradient:ocean" })] });
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-cover-color", "green");
    expect(card(2).querySelector(".page-gallery-preview")).toHaveAttribute("data-cover-gradient", "ocean");
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "cover");
    expect(card(1).querySelector("img")).toBeNull();
    // A colour or a bundled gradient never needs the first image: nothing to ask the core.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(harness.firstImageRequests()).toEqual([]);
  });

  it("is the picture of an uploaded cover, placed by the cover's position", async () => {
    renderGallery(setup(), { rows: [page(1, { cover: `attachment:${PNG_A}`, coverY: 30 })] });
    await waitFor(() => {
      expect(pictureOf(1)).not.toBeNull();
    });
    const image = pictureOf(1);
    expect(image?.getAttribute("src")).toMatch(/^blob:test\//);
    expect(image).toHaveAttribute("alt", "");
    expect(image?.style.objectPosition).toBe("50% 30%");
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "cover");
  });

  it("is the first image of the page when it has no cover, placed in the middle", async () => {
    const harness = setup(new Map([[pageId(1), PNG_B]]));
    renderGallery(harness, { rows: [page(1), page(2)] });
    await waitFor(() => {
      expect(pictureOf(1)).not.toBeNull();
    });
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "image");
    expect(pictureOf(1)?.style.objectPosition).toBe("50% 50%");
    // The other page has no image: a plain card, its icon in the picture's place.
    expect(card(2).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "icon");
    expect(card(2).querySelector(".page-gallery-preview .page-icon")).toHaveAttribute("data-icon-kind", "default");
    expect(harness.firstImageRequests()).toEqual([[pageId(1), pageId(2)]]);
  });

  it("prefers the cover to the first image", async () => {
    const harness = setup(new Map([[pageId(1), PNG_B]]));
    renderGallery(harness, { rows: [page(1, { cover: `attachment:${PNG_A}` })] });
    await waitFor(() => {
      expect(pictureOf(1)).not.toBeNull();
    });
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "cover");
    // Only the cover's blob URL was made.
    expect(urls.create).toHaveBeenCalledTimes(1);
  });

  it("asks the core for the first image of a page whose cover is a picture or not valid, and not of a colour's", async () => {
    const harness = setup();
    renderGallery(harness, {
      rows: [
        page(1),
        page(2, { cover: `attachment:${PNG_A}` }),
        page(3, { cover: "color:red" }),
        page(4, { cover: "not a cover" }),
        page(5, { cover: "gradient:sunset" }),
        page(6, { cover: "gradient:from-a-newer-version" }),
      ],
    });
    await waitFor(() => {
      expect(harness.firstImageRequests()).toHaveLength(1);
    });
    expect(harness.firstImageRequests()[0]).toEqual([pageId(1), pageId(2), pageId(4), pageId(6)]);
  });

  it("goes on to the first image when the cover is not an image, and when the webview cannot decode it", async () => {
    const harness = setup(
      new Map([
        [pageId(1), PNG_B],
        [pageId(2), PNG_B],
      ]),
    );
    renderGallery(harness, { rows: [page(1, { cover: `attachment:${NOTE}` }), page(2, { cover: `attachment:${PNG_A}` })] });
    await waitFor(() => {
      expect(card(1).querySelector("img")).not.toBeNull();
    });
    await waitFor(() => {
      expect(card(2).querySelector("img")).not.toBeNull();
    });
    const decoded = pictureOf(2);
    expect(decoded).not.toBeNull();
    // The cover of page 2 is a picture the webview cannot decode.
    fireEvent.error(decoded as HTMLImageElement);
    await waitFor(() => {
      expect(card(2).querySelector("img")).not.toBe(decoded);
    });
    expect(card(2).querySelector("img")).not.toBeNull();
    // Both cards now show the first image (the same attachment, so the same blob URL).
    expect(pictureOf(1)?.getAttribute("src")).toBe(pictureOf(2)?.getAttribute("src"));
  });

  it("is a plain card when the cover cannot be drawn and the page has no image, and for covers that do not parse or are not bundled", async () => {
    renderGallery(setup(), {
      rows: [
        page(1, { cover: `attachment:${NOTE}` }),
        page(2, { cover: "gradient:from-a-newer-version" }),
        page(3, { cover: "color:chartreuse" }),
        page(4, { cover: `attachment:${"a0000000-0000-4000-8000-0000000000ff"}` }),
      ],
    });
    await waitFor(() => {
      expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "icon");
    });
    await waitFor(() => {
      expect(card(4).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "icon");
    });
    expect(card(2).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "icon");
    expect(card(3).querySelector(".page-gallery-preview")).toHaveAttribute("data-preview", "icon");
    for (const n of [1, 2, 3, 4]) {
      expect(within(card(n)).getByRole("button")).toBeInTheDocument();
    }
  });

  it("shows the page's icon large and centred in the preview area of a plain card, so every card has the same area", async () => {
    const harness = setup(new Map([[pageId(3), PNG_B]]));
    const onOpen = vi.fn();
    renderGallery(harness, {
      rows: [page(1, { icon: "🌱" }), page(2, { icon: "icon:star:blue" }), page(3), page(4, { cover: "color:green" })],
      onOpen,
    });
    const preview = (n: number): HTMLElement => {
      const found = card(n).querySelector<HTMLElement>(".page-gallery-preview");
      if (found === null) {
        throw new Error(`no preview area on card ${String(n)}`);
      }
      return found;
    };

    // An emoji and an icon of the set are drawn in the preview area, on the card's neutral ground (no cover attribute).
    expect(preview(1)).toHaveAttribute("data-preview", "icon");
    expect(preview(1)).not.toHaveAttribute("data-cover-color");
    const emoji = preview(1).querySelector(".page-gallery-preview-icon");
    expect(emoji).toHaveTextContent("🌱");
    expect(emoji).toHaveAttribute("data-icon-kind", "emoji");
    expect(preview(2).querySelector(".page-gallery-preview-icon")).toHaveAttribute("data-icon-kind", "set");

    // A page with a cover or an image keeps its picture and no icon in the picture's area.
    expect(preview(4)).toHaveAttribute("data-preview", "cover");
    expect(preview(4).querySelector(".page-icon")).toBeNull();
    await waitFor(() => {
      expect(pictureOf(3)).not.toBeNull();
    });
    expect(preview(3)).toHaveAttribute("data-preview", "image");
    expect(preview(3).querySelector(".page-icon")).toBeNull();

    // The title's own icon stays, the preview is not a second button, and a click on it opens the page like the card does.
    expect(card(1).querySelector(".page-gallery-title .page-icon")).toHaveTextContent("🌱");
    expect(within(card(1)).getAllByRole("button")).toHaveLength(1);
    await userEvent.setup().click(preview(1));
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenLastCalledWith(expect.objectContaining({ id: pageId(1) }), "current");
  });

  it("shows the same picture on two cards with one blob URL", async () => {
    renderGallery(setup(), { rows: [page(1, { cover: `attachment:${PNG_A}` }), page(2, { cover: `attachment:${PNG_A}` })] });
    await waitFor(() => {
      expect(pictureOf(2)).not.toBeNull();
    });
    expect(pictureOf(1)?.getAttribute("src")).toBe(pictureOf(2)?.getAttribute("src"));
    expect(urls.create).toHaveBeenCalledTimes(1);
  });

  it("releases a blob URL when the last card showing it is gone", async () => {
    const view = renderGallery(setup(), { rows: [page(1, { cover: `attachment:${PNG_A}` }), page(2, { cover: `attachment:${PNG_A}` })] });
    await waitFor(() => {
      expect(pictureOf(2)).not.toBeNull();
    });
    const url = pictureOf(1)?.getAttribute("src") ?? "";
    view.again({ rows: [page(2, { cover: `attachment:${PNG_A}` })] });
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(urls.revoke).not.toHaveBeenCalled();
    view.unmount();
    await waitFor(() => {
      expect(urls.revoke).toHaveBeenCalledWith(url);
    });
    expect(urls.revoke).toHaveBeenCalledTimes(1);
  });
});

/** An `IntersectionObserver` the test drives: elements show when told to. */
class FakeObserver {
  static all: FakeObserver[] = [];
  readonly observed = new Set<Element>();
  constructor(
    private readonly callback: IntersectionObserverCallback,
    readonly options: IntersectionObserverInit | undefined,
  ) {
    FakeObserver.all.push(this);
  }
  observe(element: Element): void {
    this.observed.add(element);
  }
  unobserve(element: Element): void {
    this.observed.delete(element);
  }
  disconnect(): void {
    this.observed.clear();
  }
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
  show(element: Element): void {
    act(() => {
      this.callback([{ isIntersecting: true, target: element } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
    });
  }
  hide(element: Element): void {
    act(() => {
      this.callback([{ isIntersecting: false, target: element } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
    });
  }
}

describe("pictures far from the viewport", () => {
  beforeEach(() => {
    FakeObserver.all = [];
    vi.stubGlobal("IntersectionObserver", FakeObserver);
  });

  it("are not read until their card is near the viewport, and stay once they are", async () => {
    const harness = setup();
    renderGallery(harness, {
      rows: [page(1, { cover: `attachment:${PNG_A}` }), page(2, { cover: `attachment:${PNG_A}` }), page(3, { cover: `attachment:${PNG_B}` })],
    });
    expect(FakeObserver.all).toHaveLength(1);
    const observer = FakeObserver.all[0];
    expect(observer?.observed.size).toBe(3);
    // The places of the pictures are there, nothing is read.
    for (const n of [1, 2, 3]) {
      expect(card(n).querySelector(".page-gallery-preview")).toHaveAttribute("data-state", "far");
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(harness.requests.filter((request) => request.type === "AttachmentGet")).toHaveLength(0);
    expect(iconImagesOf(harness.store).size).toBe(0);
    expect(urls.create).not.toHaveBeenCalled();

    observer?.show(card(3));
    await waitFor(() => {
      expect(pictureOf(3)).not.toBeNull();
    });
    expect(harness.requests.filter((request) => request.type === "AttachmentGet")).toHaveLength(1);
    expect(iconImagesOf(harness.store).size).toBe(1);
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-state", "far");
    expect(card(2).querySelector(".page-gallery-preview")).toHaveAttribute("data-state", "far");
    // A card that was near stays drawn when the observer says it left.
    observer?.hide(card(3));
    expect(pictureOf(3)).not.toBeNull();
    expect(observer?.observed.has(card(3))).toBe(false);

    observer?.show(card(1));
    observer?.show(card(2));
    await waitFor(() => {
      expect(pictureOf(2)).not.toBeNull();
    });
    expect(pictureOf(1)).not.toBeNull();
  });

  it("are read for the whole gallery when the page is printed, near or not", async () => {
    const harness = setup();
    renderGallery(harness, { rows: [page(1, { cover: `attachment:${PNG_A}` }), page(2, { cover: `attachment:${PNG_B}` })] });
    expect(card(2).querySelector(".page-gallery-preview")).toHaveAttribute("data-state", "far");
    act(() => {
      harness.store.setState({ printing: true });
    });
    await waitFor(() => {
      expect(pictureOf(1)).not.toBeNull();
    });
    await waitFor(() => {
      expect(pictureOf(2)).not.toBeNull();
    });
  });

  it("makes the observer watch the page's scrolling body with a margin, and lets go of it on unmount", () => {
    const body = document.createElement("div");
    body.style.overflowY = "auto";
    document.body.appendChild(body);
    const harness = setup();
    const view = render(
      <StoreContext value={harness.store}>
        <PageGalleryView {...props({ rows: [page(1, { cover: `attachment:${PNG_A}` })] })} />
      </StoreContext>,
      { container: body.appendChild(document.createElement("div")) },
    );
    const observer = FakeObserver.all[0];
    expect(observer?.options?.root).toBe(body);
    expect(observer?.options?.rootMargin).toBe("400px 0px");
    view.unmount();
    expect(observer?.observed.size).toBe(0);
    body.remove();
  });

  it("do not make a colour cover wait: it needs no file", () => {
    renderGallery(setup(), { rows: [page(1, { cover: "color:blue" })] });
    expect(card(1).querySelector(".page-gallery-preview")).toHaveAttribute("data-state", "ready");
  });
});

describe("NearViewport", () => {
  it("calls a watcher once, when its element shows, and stops watching it", () => {
    FakeObserver.all = [];
    vi.stubGlobal("IntersectionObserver", FakeObserver);
    const viewport = new NearViewport();
    const element = document.createElement("div");
    const near = vi.fn();
    const stop = viewport.watch(element, near);
    const observer = FakeObserver.all[0];
    expect(observer?.options?.root).toBeNull();
    observer?.hide(element);
    expect(near).not.toHaveBeenCalled();
    observer?.show(element);
    observer?.show(element);
    expect(near).toHaveBeenCalledTimes(1);
    stop();
    viewport.dispose();
  });

  it("starts again after it was disposed", () => {
    FakeObserver.all = [];
    vi.stubGlobal("IntersectionObserver", FakeObserver);
    const viewport = new NearViewport();
    const element = document.createElement("div");
    viewport.watch(element, () => undefined);
    viewport.dispose();
    const near = vi.fn();
    viewport.watch(element, near);
    expect(FakeObserver.all).toHaveLength(2);
    FakeObserver.all[1]?.show(element);
    expect(near).toHaveBeenCalledTimes(1);
  });

  it("answers at once where there is no IntersectionObserver", () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    const near = vi.fn();
    new NearViewport().watch(document.createElement("div"), near);
    expect(near).toHaveBeenCalledTimes(1);
    expect(NearViewport.supported).toBe(false);
  });
});

describe("the settings section", () => {
  it("is a select of the three card sizes, showing the view's", () => {
    render(<PageGallerySettings view={{ ...VIEW, cardSize: "l" }} schema={[]} onUpdateView={vi.fn()} />);
    const select = screen.getByRole<HTMLSelectElement>("combobox", { name: t("pageGallery.size") });
    expect(Array.from(select.options).map((option) => [option.value, option.textContent])).toEqual([
      ["s", t("pageGallery.size.s")],
      ["m", t("pageGallery.size.m")],
      ["l", t("pageGallery.size.l")],
    ]);
    expect(select.value).toBe("l");
  });

  it("shows medium for a view with no size", () => {
    render(<PageGallerySettings view={VIEW} schema={[]} onUpdateView={vi.fn()} />);
    expect(screen.getByRole<HTMLSelectElement>("combobox", { name: t("pageGallery.size") }).value).toBe("m");
  });

  it("writes the size through the view, and removes the setting for the default", async () => {
    const onUpdateView = vi.fn();
    const user = userEvent.setup();
    render(<PageGallerySettings view={VIEW} schema={[]} onUpdateView={onUpdateView} />);
    const select = screen.getByRole("combobox", { name: t("pageGallery.size") });
    await user.selectOptions(select, t("pageGallery.size.s"));
    expect(onUpdateView).toHaveBeenLastCalledWith({ cardSize: "s" });
    await user.selectOptions(select, t("pageGallery.size.l"));
    expect(onUpdateView).toHaveBeenLastCalledWith({ cardSize: "l" });
    await user.selectOptions(select, t("pageGallery.size.m"));
    expect(onUpdateView).toHaveBeenLastCalledWith({ cardSize: null });
  });
});

describe("usePageFirstImages", () => {
  function Probe({ spaceId, ids }: { spaceId: SpaceId; ids: readonly PageId[] }) {
    const images = usePageFirstImages(spaceId, ids);
    return <output aria-label="images">{[...images].map(([id, attachmentId]) => `${id}=${attachmentId}`).join(",")}</output>;
  }

  const FIRST = [pageId(1), pageId(2), pageId(3)] as const;

  function renderProbe(harness: Harness, spaceId: SpaceId, ids: readonly PageId[]) {
    const ui = (next: { spaceId: SpaceId; ids: readonly PageId[] }) => (
      <StoreContext value={harness.store}>
        <Probe {...next} />
      </StoreContext>
    );
    const view = render(ui({ spaceId, ids }));
    return { ...view, again: (next: { spaceId: SpaceId; ids: readonly PageId[] }) => view.rerender(ui(next)) };
  }

  const output = (): string => screen.getByLabelText("images").textContent;

  it("asks the store once for the pages it is given and answers the ones that have an image", async () => {
    const harness = setup(
      new Map([
        [pageId(1), PNG_A],
        [pageId(3), PNG_B],
      ]),
    );
    renderProbe(harness, SPACE, FIRST);
    expect(output()).toBe("");
    await waitFor(() => {
      expect(output()).toBe(`${pageId(1)}=${PNG_A},${pageId(3)}=${PNG_B}`);
    });
    expect(harness.firstImageRequests()).toEqual([[...FIRST]]);
  });

  it("asks nothing for no pages", async () => {
    const harness = setup();
    renderProbe(harness, SPACE, []);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(harness.firstImageRequests()).toEqual([]);
    expect(output()).toBe("");
  });

  it("asks in requests of at most 500 ids", async () => {
    const harness = setup();
    const ids = Array.from({ length: 1100 }, (_, n) => pageId(n + 1));
    renderProbe(harness, SPACE, ids);
    await waitFor(() => {
      expect(harness.firstImageRequests()).toHaveLength(3);
    });
    expect(harness.firstImageRequests().map((request) => request.length)).toEqual([500, 500, 100]);
  });

  it("does not ask again when the same pages are given again", async () => {
    const harness = setup(new Map([[pageId(1), PNG_A]]));
    const { again } = renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).not.toBe("");
    });
    again({ spaceId: SPACE, ids: [...FIRST] });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(harness.firstImageRequests()).toHaveLength(1);
    expect(output()).toBe(`${pageId(1)}=${PNG_A}`);
  });

  it("asks only for the pages it does not know yet when one is added", async () => {
    const harness = setup(
      new Map([
        [pageId(1), PNG_A],
        [pageId(4), PNG_B],
      ]),
    );
    const { again } = renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).not.toBe("");
    });
    again({ spaceId: SPACE, ids: [...FIRST, pageId(4)] });
    await waitFor(() => {
      expect(output()).toBe(`${pageId(1)}=${PNG_A},${pageId(4)}=${PNG_B}`);
    });
    expect(harness.firstImageRequests()).toEqual([[...FIRST], [pageId(4)]]);
  });

  it("asks again, after a short delay and once, when the content of a page it asked about changes", async () => {
    const harness = setup(new Map([[pageId(1), PNG_A]]));
    renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).toBe(`${pageId(1)}=${PNG_A}`);
    });
    // The page's first image changes, and a burst of events says so.
    harness.answers.set(pageId(2), PNG_B);
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(2), source: "Remote" } });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(2), source: "Remote" } });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(2), source: "Editor" } });
    expect(harness.firstImageRequests()).toHaveLength(1);
    await waitFor(() => {
      expect(output()).toBe(`${pageId(1)}=${PNG_A},${pageId(2)}=${PNG_B}`);
    });
    expect(harness.firstImageRequests()).toEqual([[...FIRST], [pageId(2)]]);
    expect(REFRESH_DELAY_MS).toBeGreaterThan(0);
  });

  it("asks again when the tree of the space changes, and ignores the events of other pages and spaces", async () => {
    const harness = setup(new Map([[pageId(1), PNG_A]]));
    renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).not.toBe("");
    });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(99), source: "Editor" } });
    harness.emit({ type: "StructureChanged", data: { spaceId: OTHER_SPACE } });
    await new Promise((resolve) => setTimeout(resolve, REFRESH_DELAY_MS + 80));
    expect(harness.firstImageRequests()).toHaveLength(1);
    harness.answers.delete(pageId(1));
    harness.emit({ type: "StructureChanged", data: { spaceId: SPACE } });
    await waitFor(() => {
      expect(output()).toBe("");
    });
    expect(harness.firstImageRequests()).toHaveLength(2);
  });

  it("keeps the last answer while the next is on its way", async () => {
    const harness = setup(new Map([[pageId(1), PNG_A]]));
    renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).toBe(`${pageId(1)}=${PNG_A}`);
    });
    let release: () => void = () => undefined;
    harness.control.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    harness.answers.set(pageId(1), PNG_B);
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(1), source: "Remote" } });
    await waitFor(() => {
      expect(harness.firstImageRequests()).toHaveLength(2);
    });
    expect(output()).toBe(`${pageId(1)}=${PNG_A}`);
    release();
    await waitFor(() => {
      expect(output()).toBe(`${pageId(1)}=${PNG_B}`);
    });
  });

  it("shows nothing of another space's answer, and the new space's once it arrives", async () => {
    const harness = setup(new Map([[pageId(1), PNG_A]]));
    const { again } = renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).not.toBe("");
    });
    let release: () => void = () => undefined;
    harness.control.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    harness.answers.set(pageId(5), PNG_B);
    again({ spaceId: OTHER_SPACE, ids: [pageId(5)] });
    expect(output()).toBe("");
    release();
    await waitFor(() => {
      expect(output()).toBe(`${pageId(5)}=${PNG_B}`);
    });
  });

  it("leaves the cards on what they showed when the lookup fails", async () => {
    const harness = setup(new Map([[pageId(1), PNG_A]]));
    renderProbe(harness, SPACE, FIRST);
    await waitFor(() => {
      expect(output()).not.toBe("");
    });
    harness.control.fail = new CoreError({ code: "Internal", message: "the index is busy" });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(1), source: "Remote" } });
    await waitFor(() => {
      expect(harness.firstImageRequests()).toHaveLength(2);
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(output()).toBe(`${pageId(1)}=${PNG_A}`);
  });

  it("answers nothing without a store", () => {
    render(<Probe spaceId={SPACE} ids={FIRST} />);
    expect(output()).toBe("");
  });
});
