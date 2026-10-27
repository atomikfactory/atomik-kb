// The feed layout drawn with props (ADR-0058, `docs/architecture/database-power-ups.md` section 6.1): the cards (icon, title, shown
// properties, excerpt), the excerpts asked of the store and kept up to date, the 25 cards at a time with "Show more" and the last
// card that adds the next page when it comes into view, how a card opens its page, and the "New page" card, over a memory core whose
// `PageExcerpts` answers the test controls.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { PageId, PageInfo, PropertyDef, Response, SpaceId } from "@atomik-kb/core-client";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";

import { t } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { interopTransport } from "../test/interop";
import { NOW } from "../test/render";
import { EMPTY_FILTER_GROUP } from "./page-list-filter-tree";
import { PageListEmptyContext } from "./page-list-empty";
import type { PageListLayoutProps } from "./page-list-layout-types";
import type { PageListViewSpec } from "./page-list-views";
import { FEED_PAGE_SIZE, FEED_REFRESH_DELAY_MS } from "./page-feed-model";
import { NEAR_MARGIN_PX } from "./page-gallery-images";
import { PageFeedView } from "./PageFeedView";

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const OTHER_SPACE: SpaceId = "11111111-1111-4111-8111-222222222222";

const pageId = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;

function page(n: number, extra: Partial<PageInfo> = {}): PageInfo {
  return { id: pageId(n), spaceId: SPACE, title: `Page ${String(n)}`, createdAt: 1, updatedAt: 1, ...extra };
}

const pages = (count: number): PageInfo[] => Array.from({ length: count }, (_, n) => page(n + 1));

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
  layout: "feed",
  properties: [],
  sort: { by: "created", dir: "desc" },
  sorts: [{ by: "created", dir: "desc" }],
  filter: EMPTY_FILTER_GROUP,
};

/** The excerpt the fake core answers for a page; set before the feed asks. */
type Excerpts = Map<PageId, string>;

/** A memory core, a wrapped transport that answers `PageExcerpts` from `excerpts`, and a store over it. */
function setup(excerpts: Excerpts = new Map()) {
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Hub" }] }],
  });
  const control: { gate: Promise<void> | null; fail: CoreError | null } = { gate: null, fail: null };
  const wrapped = interopTransport(memory, {
    // A request other than `PageExcerpts` goes to the memory core: `undefined` has to come back at once, not in a promise.
    intercept: (request) => {
      if (request.type !== "PageExcerpts") {
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
          const text = excerpts.get(id);
          return text === undefined ? [] : [{ pageId: id, text }];
        });
        return { type: "PageExcerpts", data };
      };
      return answer();
    },
  });
  onTestFinished(() => {
    wrapped.detach();
  });
  const store = createAppStore(new CoreClient(wrapped.transport), { corePollMs: 5 });
  const excerptRequests = (): PageId[][] =>
    wrapped.requests.flatMap((request) => (request.type === "PageExcerpts" ? [request.data.pageIds] : []));
  return { memory, store, excerpts, control, excerptRequests, ...wrapped };
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

function renderFeed(harness: Harness, extra: Partial<PageListLayoutProps> = {}, empty: string | null = null) {
  const ui = (next: Partial<PageListLayoutProps>) => (
    <StoreContext value={harness.store}>
      <PageListEmptyContext value={empty}>
        <PageFeedView {...props({ ...extra, ...next })} />
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
const hasCard = (n: number): boolean => document.querySelector(`[data-page="${pageId(n)}"]`) !== null;
const cardCount = (): number => document.querySelectorAll(".page-feed-list > .page-feed-card").length;
const excerptOf = (n: number): string | null => card(n).querySelector(".page-feed-excerpt")?.textContent ?? null;
const titleButton = (title: string): HTMLElement => screen.getByRole("button", { name: t("pageList.feed.open", { title }) });
const showMore = (): HTMLElement => screen.getByRole("button", { name: t("pageList.feed.showMore") });
const ids = (from: number, to: number): PageId[] => Array.from({ length: to - from + 1 }, (_, n) => pageId(from + n));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("the cards", () => {
  it("draws a card for each row, in the order given, each a button named for its title, with the page's icon", () => {
    renderFeed(setup(), { rows: [page(2, { title: "Beta", icon: "🌱" }), page(1, { title: "Alpha" }), page(3, { title: "" })] });
    const list = screen.getByRole("list", { name: t("pageList.feed.list") });
    const cards = within(list).getAllByRole("listitem");
    expect(cards).toHaveLength(3);
    expect(cards.map((item) => within(item).getByRole("button").getAttribute("aria-label"))).toEqual([
      t("pageList.feed.open", { title: "Beta" }),
      t("pageList.feed.open", { title: "Alpha" }),
      t("pageList.feed.open", { title: t("page.untitled") }),
    ]);
    expect(within(card(2)).getByText("🌱")).toBeInTheDocument();
    expect(card(1).querySelector(".page-feed-title .page-icon")).toHaveAttribute("data-icon-kind", "default");
    expect(screen.getByRole("group", { name: t("pageList.feed.label") })).toBeInTheDocument();
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
    renderFeed(setup(), { rows, columns: [STATUS, COST, NOTES] });
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

  it("says what the node view says when there are no rows, and draws only the New page card then", () => {
    renderFeed(setup(), { rows: [] }, "No pages yet");
    expect(screen.getByText("No pages yet")).toBeInTheDocument();
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.getByRole("button", { name: t("pageList.feed.newCard") })).toBeInTheDocument();
  });

  it("draws nothing at all for no rows in a read-only editor, and no message while the node view says none", () => {
    renderFeed(setup(), { rows: [], editable: false, onCreatePage: null }, null);
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByText(t("pageList.feed.newCard"))).toBeNull();
  });

  it("asks the core for nothing when there are no rows", async () => {
    const harness = setup();
    renderFeed(harness, { rows: [] });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(harness.excerptRequests()).toEqual([]);
  });
});

describe("the excerpts", () => {
  it("shows the excerpt under the title and the properties, for the pages that have one", async () => {
    const harness = setup(
      new Map([
        [pageId(1), "The first lines of page one"],
        [pageId(3), "Third page"],
      ]),
    );
    renderFeed(harness, {
      rows: [page(1, { properties: [[COST.id, { kind: "number", value: 4.5 }]] }), page(2), page(3)],
      columns: [COST],
    });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("The first lines of page one");
    });
    expect(excerptOf(3)).toBe("Third page");
    // A page with no excerpt shows none: no empty paragraph either.
    expect(excerptOf(2)).toBeNull();
    // Title, properties, excerpt: in that order.
    const title = card(1).querySelector(".page-feed-title");
    const props = card(1).querySelector(".page-feed-props");
    const excerpt = card(1).querySelector(".page-feed-excerpt");
    expect(title !== null && props !== null && excerpt !== null).toBe(true);
    expect((title as Element).compareDocumentPosition(props as Element) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect((props as Element).compareDocumentPosition(excerpt as Element) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps the line breaks of an excerpt in its text (the style folds the other white space)", async () => {
    const harness = setup(new Map([[pageId(1), "First line\nSecond line\nThird line…"]]));
    renderFeed(harness, { rows: [page(1)] });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("First line\nSecond line\nThird line…");
    });
    expect(card(1).querySelector(".page-feed-excerpt")?.tagName).toBe("P");
  });

  it("asks the core once for the cards drawn, in the order of the rows, and for no others", async () => {
    const harness = setup();
    renderFeed(harness, { rows: pages(60) });
    await waitFor(() => {
      expect(harness.excerptRequests()).toHaveLength(1);
    });
    expect(harness.excerptRequests()[0]).toEqual(ids(1, FEED_PAGE_SIZE));
  });

  it("asks for the space the feed is in", async () => {
    const harness = setup();
    renderFeed(harness, { rows: [page(1)] });
    await waitFor(() => {
      expect(harness.requests.filter((request) => request.type === "PageExcerpts")).toHaveLength(1);
    });
    const request = harness.requests.find((entry) => entry.type === "PageExcerpts");
    expect(request?.type === "PageExcerpts" ? request.data.spaceId : null).toBe(SPACE);
  });

  it("does not ask again when the same rows are drawn again", async () => {
    const harness = setup(new Map([[pageId(1), "One"]]));
    const { again } = renderFeed(harness, { rows: pages(3) });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("One");
    });
    again({ rows: pages(3) });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(harness.excerptRequests()).toHaveLength(1);
    expect(excerptOf(1)).toBe("One");
  });

  it("asks again, after a short delay and once, when the content of a page it asked about changes, and only for that page", async () => {
    const harness = setup(new Map([[pageId(1), "Before"]]));
    renderFeed(harness, { rows: pages(3) });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("Before");
    });
    // Page 2 gets its first words, and a burst of events says so.
    harness.excerpts.set(pageId(2), "Now it has text");
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(2), source: "Remote" } });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(2), source: "Remote" } });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(2), source: "Editor" } });
    expect(harness.excerptRequests()).toHaveLength(1);
    await waitFor(() => {
      expect(excerptOf(2)).toBe("Now it has text");
    });
    expect(harness.excerptRequests()).toEqual([ids(1, 3), [pageId(2)]]);
    expect(FEED_REFRESH_DELAY_MS).toBeGreaterThan(0);
    // The other cards kept theirs.
    expect(excerptOf(1)).toBe("Before");
  });

  it("changes the excerpt of a page whose text changed, and drops it when the text is gone", async () => {
    const harness = setup(new Map([[pageId(1), "Old words"]]));
    renderFeed(harness, { rows: [page(1)] });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("Old words");
    });
    harness.excerpts.set(pageId(1), "New words");
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(1), source: "Editor" } });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("New words");
    });
    harness.excerpts.delete(pageId(1));
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(1), source: "Editor" } });
    await waitFor(() => {
      expect(excerptOf(1)).toBeNull();
    });
  });

  it("ignores the content changes of pages it did not ask about", async () => {
    const harness = setup(new Map([[pageId(1), "One"]]));
    renderFeed(harness, { rows: [page(1)] });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("One");
    });
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(99), source: "Editor" } });
    await new Promise((resolve) => setTimeout(resolve, FEED_REFRESH_DELAY_MS + 80));
    expect(harness.excerptRequests()).toHaveLength(1);
  });

  it("keeps the last answer while the next is on its way", async () => {
    const harness = setup(new Map([[pageId(1), "First answer"]]));
    renderFeed(harness, { rows: [page(1)] });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("First answer");
    });
    let release: () => void = () => undefined;
    harness.control.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    harness.excerpts.set(pageId(1), "Second answer");
    harness.emit({ type: "PageContentChanged", data: { pageId: pageId(1), source: "Remote" } });
    await waitFor(() => {
      expect(harness.excerptRequests()).toHaveLength(2);
    });
    expect(excerptOf(1)).toBe("First answer");
    release();
    await waitFor(() => {
      expect(excerptOf(1)).toBe("Second answer");
    });
  });

  it("shows nothing of another space's answer, and the new space's once it arrives", async () => {
    const harness = setup(
      new Map([
        [pageId(1), "In the first space"],
        [pageId(5), "In the other space"],
      ]),
    );
    const { again } = renderFeed(harness, { rows: [page(1)] });
    await waitFor(() => {
      expect(excerptOf(1)).toBe("In the first space");
    });
    let release: () => void = () => undefined;
    harness.control.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    again({ rows: [page(5, { spaceId: OTHER_SPACE })], spaceId: OTHER_SPACE });
    expect(excerptOf(5)).toBeNull();
    release();
    await waitFor(() => {
      expect(excerptOf(5)).toBe("In the other space");
    });
  });

  it("leaves the cards without an excerpt when the lookup fails, and still opens them", async () => {
    const harness = setup(new Map([[pageId(1), "One"]]));
    harness.control.fail = new CoreError({ code: "Internal", message: "the index is busy" });
    const onOpen = vi.fn();
    renderFeed(harness, { rows: [page(1, { title: "Alpha" })], onOpen });
    await waitFor(() => {
      expect(harness.excerptRequests()).toHaveLength(1);
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(excerptOf(1)).toBeNull();
    await userEvent.click(titleButton("Alpha"));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("draws the cards without a store, with no excerpt", () => {
    render(<PageFeedView {...props({ rows: [page(1, { title: "Alpha" })] })} />);
    expect(titleButton("Alpha")).toBeInTheDocument();
    expect(document.querySelector(".page-feed-excerpt")).toBeNull();
  });
});

describe("paging", () => {
  it("draws 25 cards of 60 rows, then \"Show more\" and the New page card", () => {
    renderFeed(setup(), { rows: pages(60) });
    expect(cardCount()).toBe(25);
    expect(hasCard(25)).toBe(true);
    expect(hasCard(26)).toBe(false);
    expect(showMore()).toBeInTheDocument();
    // "Show more" is under the cards and above the New page card, which is last.
    const group = screen.getByRole("group", { name: t("pageList.feed.label") });
    expect(Array.from(group.children).map((child) => child.className.split(" ")[0])).toEqual([
      "page-feed-list",
      "page-feed-more",
      "page-feed-card",
    ]);
  });

  it("adds 25 cards for each press, asks for the excerpts of the new ones only, and goes when all are drawn", async () => {
    const harness = setup(new Map([[pageId(30), "A card of the second batch"]]));
    renderFeed(harness, { rows: pages(60) });
    await waitFor(() => {
      expect(harness.excerptRequests()).toHaveLength(1);
    });
    await userEvent.click(showMore());
    expect(cardCount()).toBe(50);
    expect(hasCard(50)).toBe(true);
    await waitFor(() => {
      expect(excerptOf(30)).toBe("A card of the second batch");
    });
    expect(harness.excerptRequests()).toEqual([ids(1, 25), ids(26, 50)]);
    await userEvent.click(showMore());
    expect(cardCount()).toBe(60);
    expect(screen.queryByRole("button", { name: t("pageList.feed.showMore") })).toBeNull();
    await waitFor(() => {
      expect(harness.excerptRequests()).toHaveLength(3);
    });
    expect(harness.excerptRequests()[2]).toEqual(ids(51, 60));
    // The first batches' excerpts are still there.
    expect(excerptOf(30)).toBe("A card of the second batch");
  });

  it("offers no \"Show more\" for 25 rows or fewer", () => {
    renderFeed(setup(), { rows: pages(25) });
    expect(cardCount()).toBe(25);
    expect(screen.queryByRole("button", { name: t("pageList.feed.showMore") })).toBeNull();
  });

  it("offers \"Show more\" in a read-only editor too, with no New page card", async () => {
    renderFeed(setup(), { rows: pages(30), editable: false, onCreatePage: null });
    await userEvent.click(showMore());
    expect(cardCount()).toBe(30);
    expect(screen.queryByRole("button", { name: t("pageList.feed.newCard") })).toBeNull();
  });

  it("keeps the cards it drew when the rows are drawn again, and shows what is left when the rows get fewer", async () => {
    const { again } = renderFeed(setup(), { rows: pages(60) });
    await userEvent.click(showMore());
    expect(cardCount()).toBe(50);
    again({ rows: pages(60) });
    expect(cardCount()).toBe(50);
    again({ rows: pages(30) });
    expect(cardCount()).toBe(30);
    expect(screen.queryByRole("button", { name: t("pageList.feed.showMore") })).toBeNull();
  });

  it("adds a page to the cards drawn when the rows came back after getting fewer", async () => {
    const { again } = renderFeed(setup(), { rows: pages(200) });
    await userEvent.click(showMore());
    await userEvent.click(showMore());
    await userEvent.click(showMore());
    expect(cardCount()).toBe(100);
    again({ rows: pages(30) });
    expect(cardCount()).toBe(30);
    again({ rows: pages(200) });
    // 100 were asked for; the list is long again and shows them, then 25 more with the next press.
    expect(cardCount()).toBe(100);
    await userEvent.click(showMore());
    expect(cardCount()).toBe(125);
  });

  it("starts again at 25 cards for another view, and again when the first one is shown once more", async () => {
    const { again } = renderFeed(setup(), { rows: pages(60) });
    await userEvent.click(showMore());
    expect(cardCount()).toBe(50);
    again({ rows: pages(60), view: { ...VIEW, id: "v2" } });
    expect(cardCount()).toBe(25);
    again({ rows: pages(60), view: VIEW });
    expect(cardCount()).toBe(25);
  });
});

/** An `IntersectionObserver` the test drives: elements show when told to. */
class FakeObserver {
  static all: FakeObserver[] = [];
  readonly observed = new Set<Element>();
  disconnected = false;
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
    this.disconnected = true;
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

/** The observer that watches something now (the others were disconnected). */
const liveObserver = (): FakeObserver | undefined => FakeObserver.all.find((observer) => !observer.disconnected);

describe("the last card", () => {
  beforeEach(() => {
    FakeObserver.all = [];
    vi.stubGlobal("IntersectionObserver", FakeObserver);
  });

  it("is watched while rows are left, and adds the next page of cards when it comes into view", async () => {
    const harness = setup();
    renderFeed(harness, { rows: pages(60) });
    const observer = liveObserver();
    expect(observer?.observed.size).toBe(1);
    expect(observer?.observed.has(card(25))).toBe(true);
    observer?.hide(card(25));
    expect(cardCount()).toBe(25);
    observer?.show(card(25));
    expect(cardCount()).toBe(50);
    // The new last card is the one watched now, and the old observer is gone.
    expect(observer?.disconnected).toBe(true);
    const next = liveObserver();
    expect(next).not.toBe(observer);
    expect(next?.observed.size).toBe(1);
    expect(next?.observed.has(card(50))).toBe(true);
    await waitFor(() => {
      expect(harness.excerptRequests()).toEqual([ids(1, 25), ids(26, 50)]);
    });
    next?.show(card(50));
    expect(cardCount()).toBe(60);
    // Everything is drawn: nothing is watched any more.
    expect(liveObserver()).toBeUndefined();
  });

  it("watches the page's scrolling body, with a margin, and lets go of it on unmount", () => {
    const body = document.createElement("div");
    body.style.overflowY = "auto";
    document.body.appendChild(body);
    const harness = setup();
    const view = render(
      <StoreContext value={harness.store}>
        <PageFeedView {...props({ rows: pages(40) })} />
      </StoreContext>,
      { container: body.appendChild(document.createElement("div")) },
    );
    const observer = liveObserver();
    expect(observer?.options?.root).toBe(body);
    expect(observer?.options?.rootMargin).toBe(`${String(NEAR_MARGIN_PX)}px 0px`);
    view.unmount();
    expect(observer?.observed.size).toBe(0);
    body.remove();
  });

  it("is not watched when 25 rows or fewer are all drawn", () => {
    renderFeed(setup(), { rows: pages(25) });
    expect(liveObserver()).toBeUndefined();
  });

  it("is left to the button where there is no IntersectionObserver", async () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    renderFeed(setup(), { rows: pages(60) });
    expect(cardCount()).toBe(25);
    await userEvent.click(showMore());
    expect(cardCount()).toBe(50);
  });
});

describe("opening a page", () => {
  it("opens through the title button once, with the disposition of the gesture", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    renderFeed(setup(), { rows: [page(1, { title: "Alpha" })], onOpen });
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
    renderFeed(setup(), { rows: [page(1, { title: "Alpha" })], onOpen });
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

  it("opens from anywhere on the card, the excerpt included, once, with the gesture's disposition", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    const harness = setup(new Map([[pageId(1), "Some words of the page"]]));
    renderFeed(harness, {
      rows: [page(1, { properties: [[COST.id, { kind: "number", value: 4.5 }]] })],
      columns: [COST],
      onOpen,
    });
    await waitFor(() => {
      expect(excerptOf(1)).not.toBeNull();
    });
    await user.click(card(1).querySelector(".page-feed-excerpt") as HTMLElement);
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
    renderFeed(setup(), { rows: [page(1, { title: "Alpha" })], editable: false, onCreatePage: null, onOpen });
    await userEvent.click(titleButton("Alpha"));
    await userEvent.click(card(1));
    expect(onOpen).toHaveBeenCalledTimes(2);
  });

  it("opens the card that was drawn after \"Show more\" like the others", async () => {
    const onOpen = vi.fn();
    renderFeed(setup(), { rows: pages(30), onOpen });
    await userEvent.click(showMore());
    await userEvent.click(titleButton("Page 30"));
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: pageId(30) }), "current");
  });

  it("does not open from the New page card", async () => {
    const onOpen = vi.fn();
    renderFeed(setup(), { rows: [], onOpen });
    await userEvent.click(screen.getByRole("button", { name: t("pageList.feed.newCard") }));
    expect(onOpen).not.toHaveBeenCalled();
  });
});

describe("the New page card", () => {
  it("is the last card and asks for a page with no initial values", async () => {
    const onCreatePage = vi.fn();
    renderFeed(setup(), { rows: [page(1), page(2)], onCreatePage });
    const group = screen.getByRole("group", { name: t("pageList.feed.label") });
    const last = group.lastElementChild;
    expect(last).not.toBeNull();
    const button = within(last as HTMLElement).getByRole("button", { name: t("pageList.feed.newCard") });
    await userEvent.click(button);
    expect(onCreatePage).toHaveBeenCalledTimes(1);
    expect(onCreatePage).toHaveBeenCalledWith();
  });

  it("is disabled while a page is being made", () => {
    renderFeed(setup(), { rows: [page(1)], creating: true });
    expect(screen.getByRole("button", { name: t("pageList.feed.newCard") })).toBeDisabled();
  });

  it("is not offered when the layout may not make pages (a read-only editor, no host page)", () => {
    renderFeed(setup(), { rows: [page(1)], editable: false, onCreatePage: null });
    expect(screen.queryByRole("button", { name: t("pageList.feed.newCard") })).toBeNull();
    expect(within(screen.getByRole("list", { name: t("pageList.feed.list") })).getAllByRole("listitem")).toHaveLength(1);
  });
});
