// The version preview of the page history (`VersionPreview.tsx`, ADR-0057): a version drawn read-only, its differences from the page
// now highlighted, the "Show changes" toggle, the version alone while the page loads, and the blocks that need the store
// (pictures, references) or draw as a labelled box.

import { CoreClient, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { RenderResult } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { createAppStore } from "../store";
import type { AppStore } from "../store";
import { StoreContext } from "../store-context";
import { stubObjectUrls } from "../test/attachments";
import { idsOf } from "../test/page";
import { NOW } from "../test/render";
import type { PortableNode } from "./page-diff";
import { VersionPreview } from "./VersionPreview";
import type { VersionPreviewProps } from "./VersionPreview";

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const FILE = "a0000000-0000-4000-8000-0000000000f1";
const GONE = "a0000000-0000-4000-8000-0000000000ff";
const SPACE = "space-1";
const PAGE = "page-1";

/** What stops the stores of the tests, called when each test is over. */
const stops: (() => void)[] = [];

afterEach(() => {
  for (const stop of stops.splice(0)) {
    stop();
  }
  vi.restoreAllMocks();
});

// ----- documents ---------------------------------------------------------------------------------------------------------

function id(index: number): string {
  return `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
}

function para(n: number, value: string): PortableNode {
  return { type: "paragraph", attrs: { id: id(n) }, content: [{ type: "text", text: value }] };
}

function doc(...content: PortableNode[]): PortableNode {
  return { type: "doc", content };
}

/** The version: three paragraphs, one of which is not on the page now, and a changed one. */
const VERSION = doc(para(1, "Kept as it is"), para(2, "Only in the version"), para(3, "The quick brown fox"));
/** The page now: the first paragraph, another one that the version lacks, and the changed one. */
const CURRENT = doc(para(1, "Kept as it is"), para(4, "Only on the page now"), para(3, "The slow brown fox"));

// ----- rendering ---------------------------------------------------------------------------------------------------------

function memory(): MemoryTransport {
  return new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Notes" }] }],
    attachments: [
      { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() },
      { attachmentId: FILE, name: "report.pdf", mime: "application/pdf", bytes: new Uint8Array([1, 2, 3]) },
    ],
  });
}

async function storeOver(transport: MemoryTransport): Promise<{ store: AppStore; stop: () => void }> {
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  return { store, stop };
}

function props(overrides: Partial<VersionPreviewProps> = {}): VersionPreviewProps {
  return { spaceId: SPACE, pageId: PAGE, version: VERSION, current: CURRENT, ...overrides };
}

/** Renders the preview in the app's store (its views read pictures and titles through it); returns the render and a way to change props. */
async function show(overrides: Partial<VersionPreviewProps> = {}, transport = memory(), { ready = true }: { ready?: boolean } = {}) {
  const { store, stop } = await storeOver(transport);
  stops.push(stop);
  const view = (next: Partial<VersionPreviewProps>) => (
    <StoreContext.Provider value={store}>
      <VersionPreview {...props(next)} />
    </StoreContext.Provider>
  );
  const rendered = render(view(overrides));
  if (ready) {
    await screen.findByRole("document", { name: t("history.preview.label") });
  }
  return {
    ...rendered,
    transport,
    store,
    rerenderWith: (next: Partial<VersionPreviewProps>) => {
      rendered.rerender(view(next));
    },
  };
}

/** The element that holds the page: the preview's editor. */
function content(view: RenderResult): HTMLElement {
  const element = view.container.querySelector<HTMLElement>(".version-preview-content");
  if (element === null) {
    throw new Error("the preview has no editor");
  }
  return element;
}

function toggle(): HTMLInputElement {
  return screen.getByRole("checkbox", { name: t("history.preview.showChanges") });
}

/** The text of the elements with `selector` inside the preview. */
function texts(view: RenderResult, selector: string): string[] {
  return Array.from(content(view).querySelectorAll(selector)).map((element) => element.textContent ?? "");
}

// ----- the version ----------------------------------------------------------------------------------------------------------

describe("VersionPreview: the version alone", () => {
  it("draws the version's blocks read-only while the page now is loading, with the toggle held and a note", async () => {
    const view = await show({ current: null });
    const editor = content(view);
    expect(editor).toHaveAttribute("contenteditable", "false");
    expect(editor).toHaveClass("page-editor-content"); // the page's own styles draw it
    expect(Array.from(editor.querySelectorAll("p")).map((p) => p.textContent)).toEqual([
      "Kept as it is",
      "Only in the version",
      "The quick brown fox",
    ]);
    expect(editor.querySelector("[data-history-diff]")).toBeNull();
    expect(toggle()).toBeChecked();
    expect(toggle()).toBeDisabled();
    expect(screen.getByText(t("history.preview.comparing"))).toBeInTheDocument();
    expect(screen.queryByText(t("history.preview.legend.added"))).toBeNull();
  });

  it("draws nothing that can be used to write: no editing, no menus, no comments, no block handle", async () => {
    const view = await show({ current: null });
    const editor = content(view);
    expect(editor).toHaveAttribute("aria-readonly", "true");
    expect(document.querySelector(".block-handle, .slash-menu, .format-menu, .comment-button")).toBeNull();
    // Typing into the page does nothing.
    fireEvent.keyDown(editor, { key: "x" });
    fireEvent.input(editor, { data: "x" });
    expect(editor.textContent).not.toContain("x ");
    expect(Array.from(editor.querySelectorAll("p")).map((p) => p.textContent)).toHaveLength(3);
  });

  it("does not follow a link of the page: a click on it goes nowhere", async () => {
    const link: PortableNode = { type: "text", text: "the docs", marks: [{ type: "link", attrs: { href: "https://example.com/docs" } }] };
    const version = doc({ type: "paragraph", attrs: { id: id(1) }, content: [{ type: "text", text: "read " }, link] });
    const view = await show({ version, current: null });
    const anchor = within(content(view)).getByText("the docs");
    expect(anchor.closest("a")).toHaveAttribute("href", "https://example.com/docs");
    expect(fireEvent.click(anchor)).toBe(false); // the event's default was prevented
    expect(fireEvent(anchor, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }))).toBe(false);
  });

  it("shows the version's title when it is known, and 'Untitled' for an empty one", async () => {
    const view = await show({ current: null, title: "My plan" });
    expect(screen.getByRole("heading", { name: "My plan" })).toBeInTheDocument();
    view.rerenderWith({ current: null, title: "" });
    expect(screen.getByRole("heading", { name: t("page.untitled") })).toBeInTheDocument();
    view.rerenderWith({ current: null });
    expect(screen.queryByRole("heading")).toBeNull();
  });

  it("shows a page without blocks as one empty paragraph", async () => {
    const view = await show({ version: { type: "doc" }, current: null });
    expect(Array.from(content(view).querySelectorAll("p"))).toHaveLength(1);
  });

  it("says so when a version has content that this version of the app cannot draw", async () => {
    await show({ version: doc({ type: "futureBlock", attrs: { id: id(1) } }), current: null }, memory(), { ready: false });
    expect(await screen.findByRole("alert")).toHaveTextContent(t("history.preview.error"));
    expect(screen.queryByRole("document")).toBeNull();
  });
});

// ----- the differences ---------------------------------------------------------------------------------------------------------

describe("VersionPreview: the differences", () => {
  it("decorates the added, removed and changed blocks, and their words, with the legend and the summary", async () => {
    const view = await show();
    const editor = content(view);

    const added = editor.querySelector('[data-history-diff="added"]');
    expect(added).toHaveClass("history-diff", "history-diff-added");
    expect(added).toHaveTextContent("Only in the version");
    const removed = editor.querySelector('[data-history-diff="removed"]');
    expect(removed).toHaveClass("history-diff", "history-diff-removed");
    expect(removed).toHaveTextContent("Only on the page now");
    const changed = editor.querySelector('[data-history-diff="changed"]');
    expect(changed).toHaveClass("history-diff", "history-diff-changed");
    // The unchanged block is not decorated.
    expect(editor.querySelectorAll(".history-diff")).toHaveLength(3);
    expect(Array.from(editor.querySelectorAll("p")).map((p) => p.className.includes("history-diff"))).toEqual([false, true, true, true]);

    // The words: the version's are insertions, the page's own are deletions.
    expect(texts(view, "ins.history-run-added")).toEqual(["quick"]);
    expect(texts(view, "del.history-run-removed")).toEqual(["slow"]);

    // The legend and the summary.
    const legend = screen.getByRole("list", { name: t("history.preview.legend") });
    expect(within(legend).getByText(t("history.preview.legend.added"))).toBeInTheDocument();
    expect(within(legend).getByText(t("history.preview.legend.removed"))).toBeInTheDocument();
    expect(within(legend).getByText(t("history.preview.legend.changed"))).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("1 block added, 1 removed, 1 changed");
    expect(toggle()).toBeChecked();
    expect(toggle()).toBeEnabled();
  });

  it("puts the page's own blocks in their place between the version's", async () => {
    const view = await show();
    expect(Array.from(content(view).querySelectorAll("p")).map((p) => p.textContent)).toEqual([
      "Kept as it is",
      "Only on the page now",
      "Only in the version",
      "The slowquick brown fox", // the removed run and the added one sit side by side (a margin in the stylesheet parts them)
    ]);
  });

  it("counts several blocks in the summary", async () => {
    const version = doc(para(1, "a"), para(2, "b"), para(3, "c"), para(4, "d"), para(5, "e"));
    const current = doc(para(1, "a"));
    await show({ version, current });
    expect(screen.getByRole("status")).toHaveTextContent("4 blocks added");
  });

  it("says that nothing differs when the version is the page now", async () => {
    const view = await show({ version: VERSION, current: VERSION });
    expect(screen.getByRole("status")).toHaveTextContent(t("history.preview.noChanges"));
    expect(content(view).querySelector(".history-diff")).toBeNull();
    expect(content(view).querySelector("ins, del")).toBeNull();
  });

  it("outlines a block that is not text and whose attributes differ", async () => {
    const heading = (level: number): PortableNode => ({ type: "heading", attrs: { id: id(1), level }, content: [{ type: "text", text: "Same words" }] });
    const view = await show({ version: doc(heading(2)), current: doc(heading(1)) });
    expect(content(view).querySelector("h2")).toHaveClass("history-diff-changed");
    expect(screen.getByRole("status")).toHaveTextContent("1 changed");
  });

  it("opens a toggle that holds a difference, so that the difference is seen", async () => {
    const toggleBlock = (open: boolean, ...inner: PortableNode[]): PortableNode => ({
      type: "details",
      attrs: { id: id(1), open },
      content: [
        { type: "detailsSummary", attrs: { id: id(2) }, content: [{ type: "text", text: "More" }] },
        { type: "detailsContent", attrs: { id: id(3) }, content: inner },
      ],
    });
    const view = await show({
      version: doc(toggleBlock(false, para(4, "kept"), para(5, "hidden gem"))),
      current: doc(toggleBlock(false, para(4, "kept"))),
    });
    const details = content(view).querySelector('[data-type="details"]');
    expect(details).toHaveClass("is-open");
    expect(content(view).querySelector('[data-history-diff="added"]')).toHaveTextContent("hidden gem");
  });

  it("leaves a toggle without differences as the version has it", async () => {
    const toggleBlock = (open: boolean): PortableNode => ({
      type: "details",
      attrs: { id: id(1), open },
      content: [
        { type: "detailsSummary", attrs: { id: id(2) }, content: [{ type: "text", text: "More" }] },
        { type: "detailsContent", attrs: { id: id(3) }, content: [para(4, "kept")] },
      ],
    });
    const view = await show({ version: doc(toggleBlock(false)), current: doc(toggleBlock(true)) });
    expect(content(view).querySelector('[data-type="details"]')).not.toHaveClass("is-open");
    expect(screen.getByRole("status")).toHaveTextContent(t("history.preview.noChanges"));
  });

  it("draws the changed words of a code block as text with the runs marked", async () => {
    const code = (value: string): PortableNode => ({ type: "codeBlock", attrs: { id: id(1), language: "mermaid" }, content: [{ type: "text", text: value }] });
    const view = await show({ version: doc(code("graph TD; A-->B")), current: doc(code("graph TD; A-->C")) });
    const block = content(view).querySelector("pre");
    expect(block).not.toBeNull();
    expect(texts(view, "pre ins")).toEqual(["B"]);
    expect(texts(view, "pre del")).toEqual(["C"]);
    // No language picker and no drawn diagram: the merged text would only be one garbled source.
    expect(content(view).querySelector("select, .code-block-preview")).toBeNull();
  });

  it("draws an equation as its LaTeX, with the changed words marked", async () => {
    const math = (value: string): PortableNode => ({ type: "mathBlock", attrs: { id: id(1) }, content: [{ type: "text", text: value }] });
    const view = await show({ version: doc(math("x^2 + 1")), current: doc(math("x^2 + 2")) });
    expect(texts(view, '[data-type="mathBlock"] ins')).toEqual(["1"]);
    expect(texts(view, '[data-type="mathBlock"] del')).toEqual(["2"]);
  });
});

// ----- the toggle --------------------------------------------------------------------------------------------------------------

describe("VersionPreview: Show changes", () => {
  it("shows the version alone when it is off, and the changes again when it is on", async () => {
    const view = await show();
    expect(content(view).querySelector(".history-diff")).not.toBeNull();

    fireEvent.click(toggle());
    await waitFor(() => {
      expect(content(view).querySelector(".history-diff")).toBeNull();
    });
    expect(toggle()).not.toBeChecked();
    expect(Array.from(content(view).querySelectorAll("p")).map((p) => p.textContent)).toEqual([
      "Kept as it is",
      "Only in the version",
      "The quick brown fox",
    ]);
    expect(content(view).querySelector("ins, del")).toBeNull();
    expect(screen.queryByRole("list", { name: t("history.preview.legend") })).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();

    fireEvent.click(toggle());
    await waitFor(() => {
      expect(content(view).querySelector(".history-diff-added")).not.toBeNull();
    });
    expect(screen.getByRole("status")).toHaveTextContent("1 block added, 1 removed, 1 changed");
  });

  it("follows the version and the page as they change, and keeps the toggle where the person left it", async () => {
    const view = await show({ current: null });
    expect(content(view).querySelector(".history-diff")).toBeNull();

    // The page now arrives: the differences appear.
    view.rerenderWith({ current: CURRENT });
    await waitFor(() => {
      expect(content(view).querySelector(".history-diff-removed")).not.toBeNull();
    });
    expect(toggle()).toBeEnabled();

    // The person turns them off and picks another version: still off.
    fireEvent.click(toggle());
    view.rerenderWith({ version: doc(para(1, "Kept as it is"), para(9, "Another version")), current: CURRENT });
    await waitFor(() => {
      expect(content(view)).toHaveTextContent("Another version");
    });
    expect(toggle()).not.toBeChecked();
    expect(content(view).querySelector(".history-diff")).toBeNull();

    // Turned on, the other version is compared too.
    fireEvent.click(toggle());
    await waitFor(() => {
      expect(content(view).querySelector(".history-diff-added")).toHaveTextContent("Another version");
    });
  });
});

// ----- blocks that need the store -----------------------------------------------------------------------------------------------

describe("VersionPreview: pictures, files and references", () => {
  it("reads a picture through the attachment store and shows it with its caption and width", async () => {
    const urls = stubObjectUrls();
    const image: PortableNode = {
      type: "image",
      attrs: { id: id(1), attachmentId: CAT, alt: "A cat", widthPercent: 40, align: "left", caption: "The cat" },
    };
    const view = await show({ version: doc(image), current: null });
    const picture = await waitFor(() => {
      const found = content(view).querySelector<HTMLImageElement>("img.image-block-img");
      expect(found).not.toBeNull();
      return found as HTMLImageElement;
    });
    expect(picture).toHaveAttribute("alt", "A cat");
    expect(picture.src).toBe("blob:test/1");
    expect(urls.create).toHaveBeenCalledTimes(1);
    expect(picture.closest(".attachment-frame")).toHaveStyle({ width: "40%" });
    expect(content(view).querySelector(".image-layout")).toHaveAttribute("data-align", "left");
    expect(within(content(view)).getByText("The cat")).toHaveClass("image-caption");
    // Nothing to edit: no toolbar, no menu button, no resize handle.
    expect(content(view).querySelector(".image-toolbar, .attachment-menu-button, .image-handle")).toBeNull();
  });

  it("shows a card for a picture whose attachment is gone, and the file's name and size for a file", async () => {
    stubObjectUrls();
    const version = doc(
      { type: "image", attrs: { id: id(1), attachmentId: GONE, alt: "" } },
      { type: "file", attrs: { id: id(2), attachmentId: FILE } },
    );
    const view = await show({ version, current: null });
    await waitFor(() => {
      expect(within(content(view)).getByText(t("attachment.deleted.image"))).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(within(content(view)).getByText("report.pdf")).toBeInTheDocument();
    });
    expect(content(view).querySelector("img")).toBeNull();
  });

  it("marks an image that is added or removed as such, picture and all", async () => {
    stubObjectUrls();
    const image = (n: number): PortableNode => ({ type: "image", attrs: { id: id(n), attachmentId: CAT, alt: "" } });
    const view = await show({ version: doc(para(1, "a"), image(2)), current: doc(para(1, "a"), image(3)) });
    await waitFor(() => {
      expect(content(view).querySelectorAll("img.image-block-img")).toHaveLength(2);
    });
    expect(content(view).querySelector(".history-diff-added img")).not.toBeNull();
    expect(content(view).querySelector(".history-diff-removed img")).not.toBeNull();
  });

  it("shows a page reference as a chip with the page's title, and no link", async () => {
    const transport = memory();
    const ids = await idsOf(transport);
    const ref: PortableNode = { type: "pageRef", attrs: { id: id(2), pageId: ids.page("Notes") } };
    const missing: PortableNode = { type: "pageRef", attrs: { id: id(3), pageId: "00000000-0000-4000-8000-00000000dead" } };
    const version = doc({
      type: "paragraph",
      attrs: { id: id(1) },
      content: [{ type: "text", text: "see " }, ref, { type: "text", text: " and " }, missing],
    });
    const view = await show({ version, current: null }, transport);
    await waitFor(() => {
      expect(within(content(view)).getByText("Notes")).toHaveClass("page-ref-title");
    });
    await waitFor(() => {
      expect(within(content(view)).getByText(t("pageRef.deleted"))).toBeInTheDocument();
    });
    expect(content(view).querySelector("[role='link'], a")).toBeNull();
    // A click does not go anywhere.
    const navigate = vi.spyOn(view.store.getState(), "navigateTo");
    fireEvent.click(within(content(view)).getByText("Notes"));
    expect(navigate).not.toHaveBeenCalled();
  });

  it("draws the blocks that need a live page as a labelled box, and a callout and a template button with their blocks", async () => {
    const version = doc(
      { type: "whiteboard", attrs: { id: id(1), height: 300 } },
      { type: "gallery", attrs: { id: id(2), items: [{ attachmentId: CAT }], columns: 3 } },
      { type: "tableOfContents", attrs: { id: id(3) } },
      { type: "breadcrumb", attrs: { id: id(4) } },
      { type: "callout", attrs: { id: id(5), icon: "💡", color: "blue" }, content: [para(6, "Inside the callout")] },
      { type: "templateButton", attrs: { id: id(7), label: "Add task", position: "below" }, content: [para(8, "Inside the button")] },
    );
    const view = await show({ version, current: null });
    const editor = content(view);
    for (const label of [t("whiteboard.title"), t("gallery.label"), t("toc.title"), t("breadcrumb.label")]) {
      expect(within(editor).getByText(label).closest(".history-placeholder")).not.toBeNull();
    }
    expect(within(editor).getByText("Inside the callout")).toBeInTheDocument();
    expect(editor.querySelector('[data-type="callout"]')).toHaveAttribute("data-color", "blue");
    expect(editor.querySelector(".callout-color-button")).toBeNull();
    expect(within(editor).getByRole("button", { name: "Add task" })).toBeDisabled();
    expect(within(editor).getByText("Inside the button")).toBeVisible();
    expect(editor.querySelector(".template-button-fold, .template-button-settings")).toBeNull();
  });
});

// ----- the work around it ---------------------------------------------------------------------------------------------------------

describe("VersionPreview: updating", () => {
  it("lets go of its editor when it goes", async () => {
    const view = await show({ current: null });
    const editor = content(view);
    expect(editor.isConnected).toBe(true);
    view.unmount();
    expect(editor.isConnected).toBe(false);
    await act(async () => {
      await Promise.resolve();
    });
  });
});
