// The open page around the editor: title and icon, the page menu's "Copy as Markdown",
// and the backlinks panel.

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t, tp } from "./i18n";
import { paragraphs } from "./test/editor";
import { appendReference, findEditor, idsOf, openPage, renderPages } from "./test/page";
import { row } from "./test/sidebar";

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "clipboard");
});

const NOTES: JsonValue = {
  type: "doc",
  content: [
    { type: "heading", attrs: { id: "h0", level: 1 }, content: [{ type: "text", text: "Plan" }] },
    {
      type: "paragraph",
      attrs: { id: "p0" },
      content: [
        { type: "text", text: "Ship " },
        { type: "text", marks: [{ type: "bold" }], text: "soon" },
      ],
    },
  ],
};

describe("the page header", () => {
  it("renames the page from its title", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("x") }] }]);
    await openPage(user, "Notes");
    await user.click(within(screen.getByRole("heading", { level: 1 })).getByRole("button", { name: "Notes" }));
    const input = screen.getByRole("textbox", { name: t("tree.renameLabel") });
    await user.clear(input);
    await user.type(input, "Plans{Enter}");
    expect(await screen.findByRole("heading", { level: 1, name: "Plans" })).toBeInTheDocument();
    expect(row("Plans")).toBeInTheDocument();
  });

  it("sets the page's icon", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("x") }] }]);
    await openPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.addIcon") }));
    await user.click(within(await screen.findByRole("dialog", { name: t("icon.title") })).getByRole("button", { name: "🎯" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("🎯");
    });
  });
});

describe("Copy as Markdown", () => {
  // user-event puts a clipboard of its own on `navigator` for every test.
  it("writes the page's Markdown to the clipboard", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: NOTES }] }]);
    await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    const writeText = vi.spyOn(navigator.clipboard, "writeText");

    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.copyMarkdown") }));

    const expected = await new CoreClient(memory).pageExportMarkdown(page("Notes"));
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith(expected);
    });
    expect(await navigator.clipboard.readText()).toBe(expected);
    expect(expected).toContain("# Plan");
    expect(expected).toContain("Ship **soon**");
    expect(await screen.findByText(t("page.markdown.copied"))).toBeInTheDocument();
  });

  it("shows the Markdown to copy by hand where the clipboard cannot be written", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: NOTES }] }]);
    await openPage(user, "Notes");
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("not allowed"));

    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.copyMarkdown") }));

    const dialog = await screen.findByRole("dialog", { name: t("page.markdown.title") });
    const text = within(dialog).getByRole("textbox", { name: t("page.markdown.title") });
    expect(text).toHaveAttribute("readonly");
    expect((text as HTMLTextAreaElement).value).toContain("Ship **soon**");
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog", { name: t("page.markdown.title") })).not.toBeInTheDocument();
  });

  it("falls back to the dialog without a clipboard API too", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: NOTES }] }]);
    await openPage(user, "Notes");
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.copyMarkdown") }));
    expect(await screen.findByRole("dialog", { name: t("page.markdown.title") })).toBeInTheDocument();
  });
});

describe("backlinks", () => {
  it("lists the pages and blocks that link here, follows changes and opens them at the block", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Target", content: paragraphs("Linked to") }, { title: "Index" }] },
      { name: "Work", pages: [{ title: "Source", icon: "🧭", content: paragraphs("Intro") }] },
    ]);
    const { page } = await idsOf(memory);
    act(() => {
      memory.changePage(
        page("Source"),
        (doc) => {
          appendReference(doc, "Read ", page("Target"), "11111111-1111-4111-8111-111111111111");
        },
        "Core",
      );
    });
    await openPage(user, "Target");

    const panel = await screen.findByRole("region", { name: tp("backlinks.title", 1) });
    expect(within(panel).getByRole("button", { name: "Source" })).toBeInTheDocument();
    const block = within(panel).getByRole("button", { name: t("backlinks.block", { title: "Source", text: "Read Target" }) });
    expect(block).toHaveTextContent("Read Target");

    // A second page starts linking here while the page is open.
    act(() => {
      memory.changePage(
        page("Index"),
        (doc) => {
          appendReference(doc, "See ", page("Target"), "22222222-2222-4222-8222-222222222222");
        },
        "Core",
      );
    });
    expect(await screen.findByRole("region", { name: tp("backlinks.title", 2) })).toBeInTheDocument();

    // Opening the block switches to its space, opens the page and brings the block into view.
    await user.click(
      screen.getByRole("button", { name: t("backlinks.block", { title: "Source", text: "Read Target" }) }),
    );
    expect(await screen.findByRole("heading", { level: 1, name: "Source" })).toBeInTheDocument();
    expect(screen.getByRole("treeitem", { name: "Source" })).toHaveAttribute("aria-selected", "true");
    const editor = await findEditor();
    await waitFor(() => {
      expect(scrolled).toHaveBeenCalled();
    });
    const target = editor.view.dom.querySelector("[data-id='11111111-1111-4111-8111-111111111111']");
    expect(target).toHaveClass("is-target");
  });

  it("shows nothing while no page links here", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Alone", content: paragraphs("x") }] }]);
    await openPage(user, "Alone");
    expect(screen.queryByRole("region", { name: /linked from/i })).not.toBeInTheDocument();
  });
});
