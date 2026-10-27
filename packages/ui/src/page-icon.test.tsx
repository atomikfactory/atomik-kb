// Page and space icons (ADR-0029, section 1): the grammar the reader follows, the bundled
// icon set, the `PageIcon` component that shows emoji, icons of the set and uploaded pictures
// (and the default glyph for what it cannot show), and the cache that shares one `blob:` URL
// per picture.

import { CoreClient, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentTransport, MemoryTransportOptions, Transport } from "@atomik-kb/core-client";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PageIcon, parseIcon } from "./components/PageIcon";
import { PALETTE_COLORS } from "./editor/palette";
import { IconImageCache } from "./icons/icon-images";
import { imageIconValue, setIconValue } from "./icons/icon-grammar";
import { ICON_CATEGORIES, ICON_LIST, ICON_SET, iconByName, iconLabel, searchIcons } from "./icons/icon-set";
import { createAppStore } from "./store";
import type { AppStore } from "./store";
import { StoreContext } from "./store-context";
import { stubObjectUrls } from "./test/attachments";
import { NOW, recordingTransport } from "./test/render";

afterEach(async () => {
  // A picture is let go a turn after its last place went (icons/icon-images.ts): let that turn
  // pass, so that no clean-up of this test runs in the next one.
  cleanup();
  await new Promise((resolve) => setTimeout(resolve, 0));
  vi.restoreAllMocks();
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const NOTE = "a0000000-0000-4000-8000-00000000000a";
const GONE = "a0000000-0000-4000-8000-0000000000ff";

describe("parseIcon", () => {
  it("reads the three forms, and nothing for no icon", () => {
    const cases: [string | null | undefined, ReturnType<typeof parseIcon>][] = [
      [undefined, null],
      [null, null],
      ["", null],
      ["   ", null],
      ["🌱", { kind: "emoji", text: "🌱" }],
      ["  🌱 ", { kind: "emoji", text: "🌱" }],
      ["ok", { kind: "emoji", text: "ok" }],
      ["icon:book-open", { kind: "set", name: "book-open" }],
      ["icon:book-open:blue", { kind: "set", name: "book-open", color: "blue" }],
      ["icon:a", { kind: "set", name: "a" }],
      ["icon:x9:red", { kind: "set", name: "x9", color: "red" }],
      [`attachment:${CAT}`, { kind: "image", attachmentId: CAT }],
    ];
    for (const [icon, expected] of cases) {
      expect(parseIcon(icon), String(icon)).toEqual(expected);
    }
  });

  it("knows each colour of the palette", () => {
    for (const color of PALETTE_COLORS) {
      expect(parseIcon(`icon:star:${color}`)).toEqual({ kind: "set", name: "star", color });
    }
  });

  it("answers nothing for a value of the set or image form that breaks the grammar", () => {
    const broken = [
      "icon:",
      "icon::blue",
      "icon:Book-Open",
      "icon:book open",
      "icon:-book",
      "icon:book-",
      "icon:book_open",
      `icon:${"a".repeat(49)}`,
      "icon:book:chartreuse",
      "icon:book:Blue",
      "icon:book:blue:extra",
      "icon:book:",
      "attachment:",
      "attachment:5f0c1d2e",
      `attachment:${CAT.toUpperCase()}`,
      `attachment:${CAT}x`,
      `attachment: ${CAT}`,
    ];
    for (const icon of broken) {
      expect(parseIcon(icon), icon).toBeNull();
    }
    // The longest name is fine.
    expect(parseIcon(`icon:${"a".repeat(48)}`)).toEqual({ kind: "set", name: "a".repeat(48) });
  });

  it("writes what it reads", () => {
    expect(setIconValue("book-open")).toBe("icon:book-open");
    expect(setIconValue("book-open", null)).toBe("icon:book-open");
    expect(setIconValue("book-open", "blue")).toBe("icon:book-open:blue");
    expect(imageIconValue(CAT)).toBe(`attachment:${CAT}`);
    for (const value of [setIconValue("star", "pink"), setIconValue("star"), imageIconValue(CAT)]) {
      expect(parseIcon(value), value).not.toBeNull();
    }
  });
});

describe("the icon set", () => {
  it("has about two hundred icons, with names the grammar accepts and no duplicate", () => {
    expect(ICON_LIST.length).toBeGreaterThanOrEqual(190);
    expect(ICON_LIST.length).toBeLessThanOrEqual(240);
    const names = ICON_LIST.map((entry) => entry.name);
    expect(new Set(names).size).toBe(names.length);
    for (const name of names) {
      expect(parseIcon(`icon:${name}`), name).toEqual({ kind: "set", name });
    }
    // Every entry draws something, and finds itself by its own name.
    for (const entry of ICON_LIST) {
      expect(entry.keywords.length, entry.name).toBeGreaterThan(0);
      expect(entry.component, entry.name).toBeDefined();
      expect(iconByName(entry.name)).toBe(entry);
    }
  });

  it("lists every category, none empty", () => {
    expect(ICON_CATEGORIES).toHaveLength(10);
    for (const category of ICON_CATEGORIES) {
      expect(ICON_SET[category].length, category).toBeGreaterThanOrEqual(10);
    }
  });

  it("keeps the names stored in documents (a name is never removed or renamed)", () => {
    for (const name of ["book-open", "file-text", "house", "star", "lightbulb", "calendar", "rocket", "settings", "folder"]) {
      expect(iconByName(name), name).toBeDefined();
    }
  });

  it("knows no name it does not hold", () => {
    expect(iconByName("no-such-icon")).toBeUndefined();
    expect(iconByName("")).toBeUndefined();
    expect(iconByName("constructor")).toBeUndefined();
  });

  it("searches names and keywords, names first, every word of the query", () => {
    expect(searchIcons("")).toEqual([]);
    expect(searchIcons("   ")).toEqual([]);
    const book = searchIcons("book").map((entry) => entry.name);
    expect(book.slice(0, 3)).toEqual(["book", "book-open", "book-bookmark"]);
    expect(book).toContain("notebook"); // by its keyword
    expect(searchIcons("BOOK OPEN").map((entry) => entry.name)).toContain("book-open");
    expect(searchIcons("book open").map((entry) => entry.name)[0]).toBe("book-open");
    expect(searchIcons("book-open").map((entry) => entry.name)[0]).toBe("book-open");
    expect(searchIcons("doctor").map((entry) => entry.name)).toContain("stethoscope"); // keyword
    expect(searchIcons("zzzq")).toEqual([]);
    expect(searchIcons("book zzzq")).toEqual([]);
  });

  it("labels a name as words", () => {
    expect(iconLabel("book-open")).toBe("book open");
    expect(iconLabel("star")).toBe("star");
  });
});

// ----- PageIcon ----------------------------------------------------------------------------------

/** A started store over `memory`, to give to the components below it. */
async function storeOver(transport: Transport): Promise<{ store: AppStore; stop: () => void }> {
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  return { store, stop };
}

const NOTE_BYTES = new TextEncoder().encode("not a picture");

function memoryWith(extra: Partial<MemoryTransportOptions> = {}): MemoryTransport {
  return new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Notes" }] }],
    attachments: [
      { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() },
      { attachmentId: NOTE, name: "note.txt", mime: "text/plain", bytes: NOTE_BYTES },
    ],
    ...extra,
  });
}

function icons(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(".page-icon"));
}

describe("PageIcon: emoji and the icon set", () => {
  it("shows an emoji as text in one decorative span that takes the className", () => {
    const { container } = render(<PageIcon icon="🌱" className="tree-icon" />);
    expect(container.children).toHaveLength(1);
    const span = container.firstElementChild;
    expect(span).toHaveClass("page-icon", "tree-icon");
    expect(span).toHaveAttribute("aria-hidden", "true");
    expect(span).toHaveAttribute("data-icon-kind", "emoji");
    expect(span).toHaveTextContent("🌱");
    expect(span?.querySelector("svg")).toBeNull();
  });

  it("shows an icon of the set, in the text colour or in a colour of the palette", () => {
    const { container, rerender } = render(<PageIcon icon="icon:book-open" />);
    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "set");
    const plain = container.querySelector("svg");
    expect(plain).toHaveClass("page-icon-set", "lucide-book-open");
    expect(plain?.getAttribute("class")).not.toMatch(/page-icon-color/);
    expect(container.textContent).toBe("");

    rerender(<PageIcon icon="icon:book-open:blue" />);
    expect(container.querySelector("svg")).toHaveClass("page-icon-set", "lucide-book-open", "page-icon-color-blue");

    for (const color of PALETTE_COLORS) {
      rerender(<PageIcon icon={`icon:star:${color}`} />);
      expect(container.querySelector("svg"), color).toHaveClass(`page-icon-color-${color}`);
    }
  });

  it("shows the default glyph of its kind for no icon", () => {
    const page = render(<PageIcon icon={undefined} />);
    expect(page.container.firstElementChild).toHaveAttribute("data-icon-kind", "default");
    expect(page.container.querySelector("svg path")).not.toBeNull(); // the sheet of paper
    expect(page.container.querySelector("svg rect")).toBeNull();
    page.unmount();

    const space = render(<PageIcon icon={null} kind="space" />);
    expect(space.container.querySelector("svg rect")).not.toBeNull(); // the rounded square
  });

  it("shows the default glyph, never the raw string, for what it cannot make sense of", () => {
    const unusable = [
      "icon:no-such-icon", // a name of a newer set
      "icon:no-such-icon:blue",
      "icon:book-open:chartreuse", // a colour it does not know
      "icon:Book-Open",
      "icon:",
      "attachment:not-an-id",
    ];
    for (const icon of unusable) {
      const { container, unmount } = render(<PageIcon icon={icon} />);
      expect(container.firstElementChild, icon).toHaveAttribute("data-icon-kind", "default");
      expect(container.querySelector("svg"), icon).not.toBeNull();
      expect(container.textContent, icon).toBe("");
      unmount();
    }
  });

  it("shows an uploaded picture's default glyph where there is no store to read it from", () => {
    const { container } = render(<PageIcon icon={`attachment:${CAT}`} />);
    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "default");
    expect(container.querySelector("img")).toBeNull();
  });
});

describe("PageIcon: an uploaded picture", () => {
  function inStore(store: AppStore, ...icon: string[]) {
    return render(
      <StoreContext.Provider value={store}>
        {icon.map((value, index) => (
          <PageIcon key={index} icon={value} className={`at-${String(index)}`} />
        ))}
      </StoreContext.Provider>,
    );
  }

  it("shows the glyph while the picture is read, then the picture from a blob URL", async () => {
    const urls = stubObjectUrls();
    const { store, stop } = await storeOver(memoryWith());
    const { container } = inStore(store, `attachment:${CAT}`);

    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "default");
    await waitFor(() => {
      expect(container.querySelector("img.page-icon-image")).toHaveAttribute("src", "blob:test/1");
    });
    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "image");
    expect(container.querySelector("img")).toHaveAttribute("alt", "");
    expect(container.querySelector("svg")).toBeNull();
    expect(urls.create).toHaveBeenCalledTimes(1);
    const blob = urls.create.mock.calls[0]?.[0];
    expect(blob?.type).toBe("image/png");
    expect(blob?.size).toBe(demoImagePng().length);
    stop();
  });

  it("reads one picture once and shares its URL between the places that show it, revoking it with the last", async () => {
    const urls = stubObjectUrls();
    const recording = recordingTransport(memoryWith());
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { store, stop } = await storeOver(recording.transport);
    const first = inStore(store, `attachment:${CAT}`, `attachment:${CAT}`);
    await waitFor(() => {
      expect(first.container.querySelectorAll("img")).toHaveLength(2);
    });
    const second = inStore(store, `attachment:${CAT}`);
    await waitFor(() => {
      expect(second.container.querySelector("img")).not.toBeNull();
    });
    const sources = [...first.container.querySelectorAll("img"), ...second.container.querySelectorAll("img")].map((img) =>
      img.getAttribute("src"),
    );
    expect(sources).toEqual(["blob:test/1", "blob:test/1", "blob:test/1"]);
    expect(read).toHaveBeenCalledTimes(1);
    expect(urls.create).toHaveBeenCalledTimes(1);

    first.unmount();
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(urls.revoke).not.toHaveBeenCalled(); // one place still shows it
    second.unmount();
    await waitFor(() => {
      expect(urls.revoke).toHaveBeenCalledWith("blob:test/1");
    });
    expect(urls.revoke).toHaveBeenCalledTimes(1);
    stop();
  });

  it("keeps the URL for a place the tree draws again in the same turn", async () => {
    const urls = stubObjectUrls();
    const recording = recordingTransport(memoryWith());
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { store, stop } = await storeOver(recording.transport);
    const first = inStore(store, `attachment:${CAT}`);
    await waitFor(() => {
      expect(first.container.querySelector("img")).not.toBeNull();
    });
    // One row goes and another one that shows the same picture comes, as when a page moves.
    first.unmount();
    const second = inStore(store, `attachment:${CAT}`);
    expect(second.container.querySelector("img")).toHaveAttribute("src", "blob:test/1");
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(urls.revoke).not.toHaveBeenCalled();
    expect(read).toHaveBeenCalledTimes(1);
    second.unmount();
    stop();
  });

  it("shows the glyph for a picture that is missing, not an image, or cannot be decoded", async () => {
    stubObjectUrls();
    const recording = recordingTransport(memoryWith());
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { store, stop } = await storeOver(recording.transport);
    const { container } = inStore(store, `attachment:${GONE}`, `attachment:${NOTE}`, `attachment:${CAT}`);

    await waitFor(() => {
      expect(container.querySelector(".at-2")).toHaveAttribute("data-icon-kind", "image");
    });
    expect(container.querySelector(".at-0")).toHaveAttribute("data-icon-kind", "default"); // no such attachment
    expect(container.querySelector(".at-1")).toHaveAttribute("data-icon-kind", "default"); // text/plain
    expect(read).toHaveBeenCalledTimes(1); // only the image was read

    // The webview refuses to decode the bytes: the glyph again.
    fireEvent.error(container.querySelector(".at-2 img") as HTMLImageElement);
    expect(container.querySelector(".at-2")).toHaveAttribute("data-icon-kind", "default");
    expect(container.querySelector(".at-2 img")).toBeNull();
    stop();
  });

  it("never shows a picture as the icon of a space, and does not read it", async () => {
    stubObjectUrls();
    const recording = recordingTransport(memoryWith());
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { store, stop } = await storeOver(recording.transport);
    const { container } = render(
      <StoreContext.Provider value={store}>
        <PageIcon icon={`attachment:${CAT}`} kind="space" />
      </StoreContext.Provider>,
    );
    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "default");
    expect(container.querySelector("svg rect")).not.toBeNull(); // the space glyph
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(read).not.toHaveBeenCalled();
    stop();
  });

  it("shows the glyph when the shell cannot read attachments", async () => {
    stubObjectUrls();
    const { transport } = recordingTransport(memoryWith(), { attachments: false });
    const { store, stop } = await storeOver(transport);
    expect(store.getState().attachmentCapabilities.read).toBe(false);
    const { container } = inStore(store, `attachment:${CAT}`);
    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "default");
    expect(container.querySelector("img")).toBeNull();
    stop();
  });

  it("asks once for an icon this device has not downloaded, and shows it when it is complete", async () => {
    stubObjectUrls();
    const memory = new MemoryTransport({
      now: () => NOW,
      workspaceName: "Research",
      spaces: [{ name: "Personal", pages: [{ title: "Notes" }] }],
      attachmentSync: {
        downloadPolicy: "on_demand",
        remoteAttachments: [{ attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() }],
      },
    });
    const recording = recordingTransport(memory);
    const { store, stop } = await storeOver(recording.transport);
    const { container } = inStore(store, `attachment:${CAT}`, `attachment:${CAT}`);

    const downloads = () => recording.requests.filter((request) => request.type === "AttachmentDownload").length;
    await waitFor(() => {
      expect(downloads()).toBe(1);
    });
    // Nothing more is asked for however often the places draw again, and the glyph shows.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(downloads()).toBe(1);
    expect(icons(container).map((icon) => icon.dataset.iconKind)).toEqual(["default", "default"]);

    act(() => {
      memory.advanceTransfers();
    });
    await waitFor(() => {
      expect(container.querySelectorAll("img")).toHaveLength(2);
    });
    expect(downloads()).toBe(1);
    stop();
  });

  it("does not read a picture that is too big for an icon", async () => {
    stubObjectUrls();
    const recording = recordingTransport(
      memoryWith({
        attachments: [{ attachmentId: CAT, name: "big.png", mime: "image/png", bytes: new Uint8Array(3 * 1024 * 1024) }],
      }),
    );
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { store, stop } = await storeOver(recording.transport);
    const { container } = inStore(store, `attachment:${CAT}`);
    await waitFor(() => {
      expect(store.getState().attachmentLookup[CAT]?.status).toBe("found");
    });
    expect(container.firstElementChild).toHaveAttribute("data-icon-kind", "default");
    expect(read).not.toHaveBeenCalled();
    stop();
  });
});

describe("IconImageCache", () => {
  /** A source whose reads the test settles itself. */
  function source() {
    const reads: { id: string; resolve: (bytes: Uint8Array) => void; reject: (error: Error) => void }[] = [];
    const download = vi.fn<(id: string) => Promise<unknown>>(() => Promise.resolve());
    const cache = new IconImageCache({
      read: (id) =>
        new Promise<Uint8Array>((resolve, reject) => {
          reads.push({ id, resolve, reject });
        }),
      download,
    });
    return { cache, reads, download };
  }

  it("reads on the first use, shares the answer, and lets go a turn after the last use", async () => {
    const urls = stubObjectUrls();
    const { cache, reads } = source();
    const listener = vi.fn();
    const first = cache.subscribe(CAT, "image/png", listener);
    const second = cache.subscribe(CAT, "image/png", listener); // the same listener twice is two uses
    expect(reads).toHaveLength(1);
    expect(cache.snapshot(CAT)).toEqual({ status: "loading" });

    reads[0]?.resolve(new Uint8Array([1, 2, 3]));
    await waitFor(() => {
      expect(cache.snapshot(CAT)).toEqual({ status: "ready", url: "blob:test/1" });
    });
    expect(listener).toHaveBeenCalledTimes(2);

    first();
    first(); // a second release of the same use counts for nothing
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(urls.revoke).not.toHaveBeenCalled();
    expect(cache.size).toBe(1);
    second();
    expect(urls.revoke).not.toHaveBeenCalled(); // not within the turn
    await waitFor(() => {
      expect(urls.revoke).toHaveBeenCalledWith("blob:test/1");
    });
    expect(cache.size).toBe(0);
    expect(cache.snapshot(CAT)).toEqual({ status: "loading" });
  });

  it("reads again for a use that comes after the last went", async () => {
    stubObjectUrls();
    const { cache, reads } = source();
    const stop = cache.subscribe(CAT, "image/png", vi.fn());
    reads[0]?.resolve(new Uint8Array([1]));
    await waitFor(() => {
      expect(cache.snapshot(CAT).status).toBe("ready");
    });
    stop();
    await waitFor(() => {
      expect(cache.size).toBe(0);
    });
    cache.subscribe(CAT, "image/png", vi.fn());
    expect(reads).toHaveLength(2);
  });

  it("makes no URL for bytes that arrive after the picture was let go", async () => {
    const urls = stubObjectUrls();
    const { cache, reads } = source();
    const stop = cache.subscribe(CAT, "image/png", vi.fn());
    stop();
    await waitFor(() => {
      expect(cache.size).toBe(0);
    });
    reads[0]?.resolve(new Uint8Array([1]));
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(urls.create).not.toHaveBeenCalled();
    expect(cache.snapshot(CAT)).toEqual({ status: "loading" });
  });

  it("says failed for a read that fails, and does not read again while it is shown", async () => {
    stubObjectUrls();
    const { cache, reads } = source();
    const listener = vi.fn();
    cache.subscribe(CAT, "image/png", listener);
    reads[0]?.reject(new Error("unreadable"));
    await waitFor(() => {
      expect(cache.snapshot(CAT)).toEqual({ status: "failed" });
    });
    expect(listener).toHaveBeenCalledTimes(1);
    cache.subscribe(CAT, "image/png", listener);
    expect(reads).toHaveLength(1);
  });

  it("requests a download once per attachment, and again after the core refused", async () => {
    const { cache, download } = source();
    cache.requestDownload(CAT);
    cache.requestDownload(CAT);
    expect(download).toHaveBeenCalledTimes(1);
    cache.requestDownload(NOTE);
    expect(download).toHaveBeenCalledTimes(2);

    download.mockReturnValueOnce(Promise.reject(new Error("refused")));
    cache.requestDownload(GONE);
    await new Promise((resolve) => setTimeout(resolve, 0));
    cache.requestDownload(GONE);
    expect(download).toHaveBeenCalledTimes(4);
  });
});
