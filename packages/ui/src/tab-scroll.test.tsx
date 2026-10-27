// The scroll position of the entries of the tabs (ADR-0034 invariant 4): followed while an entry
// shows, saved when it stops showing (a tab switch, back or forward, the screen going), put back when
// it shows again once the page is tall enough, and the cases where it is not (a new entry, a block
// to scroll to, the reader taking over). The first blocks run the hook on its own over a stand-in
// scrolling body and a frame clock the test turns by hand; the last runs the whole layout.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { StrictMode, useRef } from "react";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";

import { createAppStore } from "./store";
import { StoreContext } from "./store-context";
import type { TabEntry } from "./store-tabs";
import { RESTORE_FRAMES, ScrollContainerContext, useEntryScroll } from "./tab-scroll";
import { t } from "./i18n";
import { idsOf } from "./test/page";
import { NOW } from "./test/render";
import { PERSONAL, row } from "./test/sidebar";
import { makeTab, renderLayout, seedTabs, tabNamed } from "./test/tabs";

afterEach(() => {
  vi.unstubAllGlobals();
});

/** An animation frame clock the test turns by hand. */
function stubFrames() {
  const queue = new Map<number, FrameRequestCallback>();
  let next = 1;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback): number => {
    const id = next;
    next += 1;
    queue.set(id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number): void => {
    queue.delete(id);
  });
  return {
    /** Runs the frames asked for so far, `count` times over. */
    step(count = 1): void {
      for (let n = 0; n < count; n += 1) {
        const due = [...queue.values()];
        queue.clear();
        act(() => {
          for (const callback of due) {
            callback(0);
          }
        });
      }
    },
    pending: (): number => queue.size,
  };
}

/** A started store over a space with pages, and the ids of what it holds. */
async function startStore() {
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [PERSONAL] });
  const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
  onTestFinished(store.getState().start());
  await waitFor(() => {
    expect(store.getState().tabs.length).toBeGreaterThan(0);
    expect(store.getState().tree).not.toBeNull();
  });
  return { store, memory, ids: await idsOf(memory) };
}

/** A stand-in for the scrolling body of the main area, whose height the test sets. */
function setHeights(body: HTMLElement, scrollHeight: number, clientHeight = 500): void {
  Object.defineProperty(body, "scrollHeight", { configurable: true, value: scrollHeight });
  Object.defineProperty(body, "clientHeight", { configurable: true, value: clientHeight });
}

function Scroller({ entry, shown }: { entry: TabEntry | null; shown: boolean }) {
  useEntryScroll(entry, shown);
  return null;
}

function Body({ entry, shown = true }: { entry: TabEntry | null; shown?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <ScrollContainerContext value={ref}>
      <div ref={ref} data-testid="body" />
      <Scroller entry={entry} shown={shown} />
    </ScrollContainerContext>
  );
}

/** Mounts the hook over a stand-in body, with two entries (`a` and `b`) in tabs of the store. */
async function mountScroller(options: { strict?: boolean } = {}) {
  const { store, ids } = await startStore();
  const space = ids.space("Personal");
  const a = makeTab("a", space, ids.page("Journal"));
  const b = makeTab("b", space, ids.page("Reading list"));
  act(() => {
    seedTabs(store, [a, b], a.id);
  });
  const entryA = a.entries[0] as TabEntry;
  const entryB = b.entries[0] as TabEntry;
  const ui = (entry: TabEntry | null, shown = true) => {
    const body = (
      <StoreContext value={store}>
        <Body entry={entry} shown={shown} />
      </StoreContext>
    );
    return options.strict === true ? <StrictMode>{body}</StrictMode> : body;
  };
  const view = render(ui(entryA));
  const body = screen.getByTestId("body");
  return {
    store,
    ids,
    body,
    entryA,
    entryB,
    tabA: a,
    tabB: b,
    show: (entry: TabEntry | null, shown = true) => {
      view.rerender(ui(entry, shown));
    },
    unmount: view.unmount,
  };
}

/** Scrolls the body the way a reader does: the position changes and the event follows. */
function scrollTo(body: HTMLElement, top: number): void {
  body.scrollTop = top;
  fireEvent.scroll(body);
}

describe("keeping the position of an entry", () => {
  it("saves the position when another entry takes the area, and puts it back when the entry shows again", async () => {
    const frames = stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 300);
    show(entryB);
    expect(store.getState().savedScroll(entryA.id)).toBe(300);
    expect(body.scrollTop).toBe(0); // a new entry starts at the top
    scrollTo(body, 80);
    show(entryA);
    expect(body.scrollTop).toBe(300);
    expect(store.getState().savedScroll(entryB.id)).toBe(80);
    expect(frames.pending()).toBe(0); // the page was tall enough: no waiting
    show(entryB);
    expect(body.scrollTop).toBe(80);
    expect(store.getState().savedScroll(entryA.id)).toBe(300);
  });

  it("writes nothing to the store while the body scrolls", async () => {
    stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    const saveScroll = vi.fn();
    act(() => {
      store.setState({ saveScroll });
    });
    for (const top of [10, 40, 90, 160, 250]) {
      scrollTo(body, top);
    }
    expect(saveScroll).not.toHaveBeenCalled();
    show(entryB);
    expect(saveScroll).toHaveBeenCalledTimes(1);
    expect(saveScroll).toHaveBeenCalledWith(entryA.id, 250);
  });

  it("saves the position when the screen goes", async () => {
    stubFrames();
    const { store, body, entryA, unmount } = await mountScroller();
    scrollTo(body, 120);
    unmount();
    expect(store.getState().savedScroll(entryA.id)).toBe(120);
  });

  it("starts an entry it has not seen at the top, whatever the body was left at", async () => {
    stubFrames();
    const { store, body, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 700);
    show(entryB);
    expect(body.scrollTop).toBe(0);
    expect(store.getState().savedScroll(entryB.id)).toBeUndefined();
  });

  it("starts at the top again for a position of nothing", async () => {
    stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    scrollTo(body, 0);
    show(entryB);
    scrollTo(body, 50);
    show(entryA);
    expect(store.getState().savedScroll(entryA.id)).toBe(0);
    expect(body.scrollTop).toBe(0);
  });

  it("forgets an entry that is in no tab, nor in a closed one", async () => {
    stubFrames();
    const { store, body, entryA, entryB, tabA, tabB, show } = await mountScroller();
    scrollTo(body, 200);
    act(() => {
      seedTabs(store, [tabB], tabB.id);
    });
    show(entryB);
    expect(store.getState().savedScroll(entryA.id)).toBeUndefined();
    // A closed tab can come back with its history: its entries keep their position.
    scrollTo(body, 90);
    act(() => {
      seedTabs(store, [tabA], tabA.id, [tabB]);
    });
    show(entryA);
    expect(store.getState().savedScroll(entryB.id)).toBe(90);
  });

  it("does nothing without an entry (no tab yet)", async () => {
    stubFrames();
    const { store, body, entryA, show } = await mountScroller();
    scrollTo(body, 200);
    show(null);
    expect(store.getState().savedScroll(entryA.id)).toBe(200);
    scrollTo(body, 50);
    expect(body.scrollTop).toBe(50);
  });
});

describe("putting the position back", () => {
  it("waits, frame by frame, until the page is tall enough for it", async () => {
    const frames = stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 800);
    show(entryB);
    setHeights(body, 700); // the page of the entry is still opening
    show(entryA);
    expect(body.scrollTop).toBe(0);
    expect(frames.pending()).toBe(1);
    frames.step(3);
    expect(body.scrollTop).toBe(0);
    setHeights(body, 1200); // 1200 - 500 = 700 of scroll: short of 800
    frames.step();
    expect(body.scrollTop).toBe(0);
    setHeights(body, 1400);
    frames.step();
    expect(body.scrollTop).toBe(800);
    expect(frames.pending()).toBe(0);
    // Followed from there as always.
    show(entryB);
    expect(store.getState().savedScroll(entryA.id)).toBe(800);
  });

  it("gives up waiting after a bound, and asks for the position anyway", async () => {
    const frames = stubFrames();
    const { body, entryA, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 800);
    show(entryB);
    setHeights(body, 100); // never tall enough
    show(entryA);
    frames.step(RESTORE_FRAMES - 2);
    expect(body.scrollTop).toBe(0);
    expect(frames.pending()).toBe(1);
    frames.step();
    expect(body.scrollTop).toBe(800); // a real body keeps what it can of it
    expect(frames.pending()).toBe(0);
  });

  it("leaves the reader where they are when they scroll, touch, press or type while it waits", async () => {
    for (const type of ["wheel", "touchstart", "pointerdown", "keydown"]) {
      const frames = stubFrames();
      const { store, body, entryA, entryB, show, unmount } = await mountScroller();
      setHeights(body, 3000);
      scrollTo(body, 800);
      show(entryB);
      setHeights(body, 100);
      show(entryA);
      expect(frames.pending()).toBe(1);
      fireEvent(body, new Event(type, { bubbles: true }));
      expect(frames.pending()).toBe(0);
      setHeights(body, 3000);
      frames.step(2);
      expect(body.scrollTop, type).toBe(0);
      // And what they did from there is the position that is saved.
      scrollTo(body, 40);
      show(entryB);
      expect(store.getState().savedScroll(entryA.id), type).toBe(40);
      unmount();
      vi.unstubAllGlobals();
    }
  });

  it("keeps the remembered position when the entry goes before it could be put back", async () => {
    stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 800);
    show(entryB);
    setHeights(body, 100);
    show(entryA); // waiting: the body is at 0 now
    show(entryB);
    expect(store.getState().savedScroll(entryA.id)).toBe(800);
  });

  it("lets a block that something points at win over the remembered position", async () => {
    const frames = stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 800);
    show(entryB);
    act(() => {
      store.setState({ scrollTarget: { pageId: entryA.pageId ?? "", blockId: "b0000000-0000-4000-8000-000000000001", seq: 1 } });
    });
    show(entryA);
    expect(body.scrollTop).toBe(0);
    expect(frames.pending()).toBe(0);
    // A target for another page does not.
    show(entryB);
    act(() => {
      store.setState({ scrollTarget: { pageId: entryB.pageId ?? "", blockId: "b0000000-0000-4000-8000-000000000001", seq: 2 } });
    });
    scrollTo(body, 30);
    show(entryA);
    expect(body.scrollTop).toBe(0);
    expect(store.getState().savedScroll(entryB.id)).toBe(30);
  });

  it("puts a position back once, in the strict mode that runs effects twice", async () => {
    stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller({ strict: true });
    setHeights(body, 3000);
    scrollTo(body, 300);
    show(entryB);
    show(entryA);
    expect(body.scrollTop).toBe(300);
    show(entryB);
    expect(store.getState().savedScroll(entryA.id)).toBe(300);
  });

  it("holds the position of an entry whose page is not there yet at the top, and waits for the page", async () => {
    const frames = stubFrames();
    const { store, body, entryA, entryB, show } = await mountScroller();
    setHeights(body, 3000);
    scrollTo(body, 600);
    show(entryB);
    // The entry's page is not in the tree yet: a Home stands in, which has no position to keep.
    show(entryA, false);
    expect(body.scrollTop).toBe(0);
    expect(frames.pending()).toBe(0);
    show(entryB);
    expect(store.getState().savedScroll(entryA.id)).toBe(600);
    show(entryA, false);
    show(entryA, true);
    expect(body.scrollTop).toBe(600);
  });
});

describe("the positions in the whole layout", () => {
  /** The scrolling body of the main area, tall enough for any position the tests use. */
  function tallBody(): HTMLElement {
    const body = document.querySelector<HTMLElement>(".content-body");
    if (body === null) {
      throw new Error("the main area has no body");
    }
    setHeights(body, 5000);
    return body;
  }

  it("puts a page back where it was scrolled when back returns to it", async () => {
    const { user } = await renderLayout();
    const body = tallBody();
    await user.click(row("Journal"));
    await screen.findByRole("textbox", { name: t("editor.label") });
    scrollTo(body, 420);
    await user.click(row("Reading list"));
    await waitFor(() => {
      expect(body.scrollTop).toBe(0);
    });
    await user.click(screen.getByRole("button", { name: t("tabs.back") }));
    await waitFor(() => {
      expect(screen.getByRole("tabpanel", { name: "Journal" })).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(body.scrollTop).toBe(420);
    });
  });

  it("keeps the position of each tab: switching tabs puts each page back where it was left", async () => {
    const { user, store, ids } = await renderLayout();
    const body = tallBody();
    await user.click(row("Journal"));
    await screen.findByRole("textbox", { name: t("editor.label") });
    scrollTo(body, 250);
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Projects"), spaceId: ids.space("Personal"), disposition: "foreground" });
    });
    await waitFor(() => {
      expect(tabNamed("Projects")).toHaveAttribute("aria-selected", "true");
    });
    await waitFor(() => {
      expect(body.scrollTop).toBe(0);
    });
    scrollTo(body, 90);
    await user.click(tabNamed("Journal"));
    await waitFor(() => {
      expect(body.scrollTop).toBe(250);
    });
    await user.click(tabNamed("Projects"));
    await waitFor(() => {
      expect(body.scrollTop).toBe(90);
    });
  });

  it("keeps the position through a visit to Settings, which unmounts the page", async () => {
    const { user } = await renderLayout();
    const body = tallBody();
    await user.click(row("Journal"));
    await screen.findByRole("textbox", { name: t("editor.label") });
    scrollTo(body, 175);
    await user.click(screen.getByRole("button", { name: t("nav.settings") }));
    expect(await screen.findByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
    await user.click(tabNamed("Journal"));
    await waitFor(() => {
      expect(screen.getByRole("tabpanel", { name: "Journal" })).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(body.scrollTop).toBe(175);
    });
  });

  it("shows the Home of a tab while the page of another tab is not there, and keeps the key per page", async () => {
    const { user, store, ids } = await renderLayout();
    const space = ids.space("Personal");
    await user.click(row("Journal"));
    const home = makeTab("home", space, null);
    act(() => {
      seedTabs(store, [...store.getState().tabs, home], home.id);
    });
    expect(await screen.findByRole("heading", { name: "Research" })).toBeInTheDocument();
    await user.click(tabNamed("Journal"));
    expect(await screen.findByRole("textbox", { name: t("editor.label") })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Research" })).not.toBeInTheDocument();
  });
});
