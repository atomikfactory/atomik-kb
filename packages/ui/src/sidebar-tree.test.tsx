import { CoreClient } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { recordingTransport, renderWith } from "./test/render";
import { MemoryTransport } from "@atomik-kb/core-client";
import { PERSONAL, PERSONAL_OUTLINE, WORK, outline, renderSidebar, row } from "./test/sidebar";

const untitled = t("page.untitled");

/** Ids of the pages of the first space, by title, read from the core. */
async function pageIds(memory: MemoryTransport): Promise<{ spaceId: string; id: (title: string) => string }> {
  const client = new CoreClient(memory);
  const [space] = await client.spaceList();
  const tree = await client.treeGet(space?.id ?? "");
  const found = new Map<string, string>();
  const walk = (nodes: typeof tree.roots): void => {
    for (const node of nodes) {
      found.set(node.page.title, node.page.id);
      walk(node.children);
    }
  };
  walk(tree.roots);
  return { spaceId: tree.spaceId, id: (title) => found.get(title) ?? "" };
}

describe("page tree: rendering", () => {
  it("shows the pages of the first space as a tree, in order, with their depth", async () => {
    await renderSidebar();
    expect(outline()).toEqual(PERSONAL_OUTLINE);

    const projects = row("Projects");
    expect(projects).toHaveAttribute("aria-level", "1");
    expect(projects).toHaveAttribute("aria-expanded", "true");
    expect(projects).toHaveAttribute("aria-posinset", "1");
    expect(projects).toHaveAttribute("aria-setsize", "3");
    expect(row("Q1")).toHaveAttribute("aria-level", "3");
    expect(row("Q1")).not.toHaveAttribute("aria-expanded");
    expect(row("Ideas")).toHaveAttribute("aria-posinset", "2");
    expect(row("Ideas")).toHaveAttribute("aria-setsize", "2");
    expect(within(projects).getByText("📁")).toBeInTheDocument();
    for (const item of screen.getAllByRole("treeitem")) {
      expect(item).toHaveAttribute("aria-selected", "false");
    }
  });

  it("shows pages without a title as Untitled", async () => {
    await renderSidebar({ spaces: [{ name: "Personal", pages: [{ title: "" }] }] });
    expect(outline()).toEqual([untitled]);
  });

  it("shows a hint instead of a tree for a space without pages", async () => {
    await renderSidebar({ spaces: [{ name: "Empty" }] });
    expect(screen.queryByRole("tree")).not.toBeInTheDocument();
    expect(screen.getByText(t("tree.empty"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("tree.newPage") })).toBeInTheDocument();
  });

  it("opens a page in the main area when it is clicked, with its editor", async () => {
    const { user } = await renderSidebar();
    expect(screen.getByRole("heading", { level: 1, name: "Research" })).toBeInTheDocument();
    expect(screen.getByText(t("home.hint.select"))).toBeInTheDocument();

    await user.click(row("Roadmap"));
    expect(screen.getByRole("heading", { level: 1, name: "Roadmap" })).toBeInTheDocument();
    expect(await screen.findByRole("textbox", { name: t("editor.label") })).toBeInTheDocument();
    expect(row("Roadmap")).toHaveAttribute("aria-selected", "true");

    await user.click(row("Projects"));
    expect(screen.getByRole("heading", { level: 1, name: "Projects" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("📁");
    expect(row("Roadmap")).toHaveAttribute("aria-selected", "false");
  });

  it("shows nothing as open when the open page leaves the tree, and does not reopen it when it returns (ADR-0034 section 3)", async () => {
    const { user, memory } = await renderSidebar();
    const { id } = await pageIds(memory);
    await user.click(row("Journal"));
    await memory.dispatch({ type: "PageTrash", data: { pageId: id("Journal") } });
    await waitFor(() => {
      expect(outline()).not.toContain("Journal");
    });
    expect(screen.getByRole("heading", { level: 1, name: "Research" })).toBeInTheDocument();
    await memory.dispatch({ type: "PageRestore", data: { pageId: id("Journal") } });
    await waitFor(() => {
      expect(outline()).toContain("Journal");
    });
    // The tab dropped the page when it went; undoing the trash does not reopen it.
    expect(screen.getByRole("heading", { level: 1, name: "Research" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: "Journal" })).not.toBeInTheDocument();
  });

  it("follows changes made elsewhere through the StructureChanged event", async () => {
    const { memory } = await renderSidebar();
    const { spaceId, id } = await pageIds(memory);
    await memory.dispatch({ type: "PageCreate", data: { spaceId, parentId: id("Journal"), title: "Entry" } });
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal", "  Entry", "Reading list"]);
    });
  });
});

describe("page tree: creating pages", () => {
  it("creates a root page at the end, opens it and starts renaming it", async () => {
    const { user, memory } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));

    expect(await screen.findByRole("treeitem", { name: untitled })).toBeInTheDocument();
    expect(outline()).toEqual([...PERSONAL_OUTLINE, untitled]);
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(input).toHaveFocus();
    expect(input).toHaveValue("");
    expect(screen.getByRole("heading", { level: 1, name: untitled })).toBeInTheDocument();

    await user.type(input, "Inbox{Enter}");
    await waitFor(() => {
      expect(outline().at(-1)).toBe("Inbox");
    });
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Inbox" })).toBeInTheDocument();
    const { spaceId } = await pageIds(memory);
    const view = await new CoreClient(memory).treeGet(spaceId);
    expect(view.roots.map((node) => node.page.title)).toEqual(["Projects", "Journal", "Reading list", "Inbox"]);
  });

  it("keeps an untitled page when the name is left empty", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.keyboard("{Escape}");
    expect(outline().at(-1)).toBe(untitled);
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
  });

  it("creates a child page with the plus button of a row, and shows it under its parent", async () => {
    const { user } = await renderSidebar();
    await user.click(within(row("Journal")).getByRole("button", { name: t("tree.addChild") }));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal", `  ${untitled}`, "Reading list"]);
    expect(row("Journal")).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("Entry{Enter}");
    await waitFor(() => {
      expect(outline()).toContain("  Entry");
    });
  });

  it("expands a collapsed parent when a child is added, and remembers that", async () => {
    const { user, memory } = await renderSidebar();
    const { spaceId } = await pageIds(memory);
    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Projects", "Journal", "Reading list"]);

    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.addChild") }));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(row("Projects")).toHaveAttribute("aria-expanded", "true");
    expect(outline().slice(0, 6)).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", `  ${untitled}`, "Journal"]);
    await expect(
      memory.dispatch({ type: "SettingsGet", data: { key: `ui.sidebar.collapsed.${spaceId}` } }),
    ).resolves.toEqual({ type: "Setting", data: [] });
  });

  it("explains why a page could not be created, and offers to dismiss it", async () => {
    const { user, memory } = await renderSidebar();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("tree.error.create", { message: "database error: disk I/O error" }));
    expect(outline()).toEqual(PERSONAL_OUTLINE);
    await user.click(within(alert).getByRole("button", { name: t("sidebar.dismissError") }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("page tree: renaming in place", () => {
  it("renames with a double click: Enter commits and focus returns to the row", async () => {
    const { user } = await renderSidebar();
    await user.dblClick(row("Journal"));
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(input).toHaveValue("Journal");
    expect(input).toHaveFocus();

    await user.clear(input);
    await user.type(input, "Diary{Enter}");
    await waitFor(() => {
      expect(outline()).toContain("Diary");
    });
    expect(outline()).not.toContain("Journal");
    expect(row("Diary")).toHaveFocus();
  });

  it("renames with F2, and Escape cancels without asking the core", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL, WORK] });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("tree");

    act(() => {
      row("Journal").focus();
    });
    await user.keyboard("{F2}");
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.type(input, " changed{Escape}");
    expect(outline()).toEqual(PERSONAL_OUTLINE);
    expect(row("Journal")).toHaveFocus();
    expect(recorder.requests.filter((request) => request.type === "PageRename")).toEqual([]);
  });

  it("leaves Enter and Escape to an input method that is composing text", async () => {
    await renderSidebar();
    fireEvent.keyDown(row("Journal"), { key: "F2" });
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    fireEvent.change(input, { target: { value: "ジャーナル" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    fireEvent.keyDown(input, { key: "Escape", isComposing: true });
    expect(input).toBeInTheDocument();
    expect(input).toHaveValue("ジャーナル");
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => {
      expect(outline()).toContain("ジャーナル");
    });
  });

  it("does not bother the core when the title did not change", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("tree");
    await user.dblClick(row("Journal"));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    expect(recorder.requests.filter((request) => request.type === "PageRename")).toEqual([]);
  });

  it("commits when the field loses focus", async () => {
    const { user } = await renderSidebar();
    await user.dblClick(row("Journal"));
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.type(input, "!");
    await user.click(screen.getByRole("heading", { level: 1 }));
    await waitFor(() => {
      expect(outline()).toContain("Journal!");
    });
  });

  it("allows an empty title and shows it as Untitled", async () => {
    const { user } = await renderSidebar();
    await user.dblClick(row("Journal"));
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.clear(input);
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(outline()).toContain(untitled);
    });
  });

  it("shows the core's reason when a title is refused and keeps the old title", async () => {
    const { user } = await renderSidebar();
    await user.dblClick(row("Journal"));
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.click(input);
    await user.paste("x".repeat(501));
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.rename", { message: "must be at most 500 characters" }),
    );
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });
});

describe("page tree: expanding and collapsing", () => {
  it("collapses and expands a page with its chevron", async () => {
    const { user } = await renderSidebar();
    await user.click(within(row("Roadmap")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Projects", "  Roadmap", "  Ideas", "Journal", "Reading list"]);
    expect(row("Roadmap")).toHaveAttribute("aria-expanded", "false");

    await user.click(within(row("Roadmap")).getByRole("button", { name: t("tree.expand") }));
    expect(outline()).toEqual(PERSONAL_OUTLINE);
    // Pages without children have no chevron.
    expect(within(row("Journal")).queryByRole("button", { name: t("tree.collapse") })).not.toBeInTheDocument();
  });

  it("remembers collapsed pages per space in the ui.sidebar.collapsed.<space> setting", async () => {
    const { user, memory } = await renderSidebar();
    const { spaceId, id } = await pageIds(memory);
    const key = `ui.sidebar.collapsed.${spaceId}`;

    await expect(memory.dispatch({ type: "SettingsGet", data: { key } })).resolves.toEqual({
      type: "Setting",
      data: null,
    });

    await user.click(within(row("Roadmap")).getByRole("button", { name: t("tree.collapse") }));
    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.collapse") }));
    await expect(memory.dispatch({ type: "SettingsGet", data: { key } })).resolves.toEqual({
      type: "Setting",
      data: [id("Roadmap"), id("Projects")],
    });

    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.expand") }));
    await expect(memory.dispatch({ type: "SettingsGet", data: { key } })).resolves.toEqual({
      type: "Setting",
      data: [id("Roadmap")],
    });
  });

  it("starts with the pages collapsed that were collapsed last time", async () => {
    const { user, memory, unmount } = await renderSidebar();
    await user.click(within(row("Roadmap")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Projects", "  Roadmap", "  Ideas", "Journal", "Reading list"]);
    unmount();

    renderWith(memory);
    await screen.findByRole("tree");
    expect(outline()).toEqual(["Projects", "  Roadmap", "  Ideas", "Journal", "Reading list"]);
  });

  it("keeps the collapsed pages of each space apart", async () => {
    const memory = new MemoryTransport({
      workspaceName: "Research",
      spaces: [
        { name: "One", pages: [{ title: "Parent", children: [{ title: "Child" }] }] },
        { name: "Two", pages: [{ title: "Parent", children: [{ title: "Child" }] }] },
      ],
    });
    const { user } = renderWith(memory);
    await screen.findByRole("tree");
    await user.click(within(row("Parent")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Parent"]);

    await user.click(screen.getByRole("button", { name: "One" }));
    await user.click(within(screen.getByRole("list", { name: t("spaces.listLabel") })).getByRole("button", { name: "Two" }));
    await waitFor(() => {
      expect(outline()).toEqual(["Parent", "  Child"]);
    });
  });

  it("ignores a stored value that is not a list of page ids", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const { spaceId } = await pageIds(memory);
    await memory.dispatch({ type: "SettingsSet", data: { key: `ui.sidebar.collapsed.${spaceId}`, value: { nope: true } } });
    renderWith(memory);
    await screen.findByRole("tree");
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });
});

describe("page tree: keyboard", () => {
  it("puts one row in the Tab order and moves focus with the arrow keys, Home and End", async () => {
    const { user } = await renderSidebar();
    expect(row("Projects")).toHaveAttribute("tabindex", "0");
    expect(row("Roadmap")).toHaveAttribute("tabindex", "-1");

    act(() => {
      row("Projects").focus();
    });
    await user.keyboard("{ArrowDown}");
    expect(row("Roadmap")).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(row("Ideas")).toHaveFocus();
    expect(row("Ideas")).toHaveAttribute("tabindex", "0");
    expect(row("Projects")).toHaveAttribute("tabindex", "-1");
    await user.keyboard("{End}");
    expect(row("Reading list")).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(row("Reading list")).toHaveFocus();
    await user.keyboard("{Home}");
    expect(row("Projects")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(row("Projects")).toHaveFocus();
  });

  it("starts the Tab order at the open page", async () => {
    const { user } = await renderSidebar();
    await user.click(row("Journal"));
    expect(row("Journal")).toHaveAttribute("tabindex", "0");
    expect(row("Projects")).toHaveAttribute("tabindex", "-1");
  });

  it("opens the focused page with Enter", async () => {
    const { user } = await renderSidebar();
    act(() => {
      row("Roadmap").focus();
    });
    await user.keyboard("{Enter}");
    expect(screen.getByRole("heading", { level: 1, name: "Roadmap" })).toBeInTheDocument();
    expect(row("Roadmap")).toHaveAttribute("aria-selected", "true");
  });

  it("collapses with Left and expands with Right, moving to the parent or first child otherwise", async () => {
    const { user } = await renderSidebar();
    act(() => {
      row("Q1").focus();
    });
    await user.keyboard("{ArrowLeft}");
    expect(row("Roadmap")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(outline()).toEqual(["Projects", "  Roadmap", "  Ideas", "Journal", "Reading list"]);
    expect(row("Roadmap")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(row("Projects")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(row("Roadmap")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(outline()).toEqual(PERSONAL_OUTLINE);
    await user.keyboard("{ArrowRight}");
    expect(row("Q1")).toHaveFocus();
  });

  it("moves a page to the trash with Delete and focuses the next row", async () => {
    const { user } = await renderSidebar();
    act(() => {
      row("Journal").focus();
    });
    await user.keyboard("{Delete}");
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
    });
    expect(row("Reading list")).toHaveFocus();
  });

  it("focuses the previous row when the last page goes to the trash", async () => {
    const { user } = await renderSidebar();
    act(() => {
      row("Reading list").focus();
    });
    await user.keyboard("{Delete}");
    await waitFor(() => {
      expect(outline()).not.toContain("Reading list");
    });
    expect(row("Journal")).toHaveFocus();
  });

  it("opens the context menu with the menu key and leaves other keys alone while renaming", async () => {
    const { user } = await renderSidebar();
    act(() => {
      row("Journal").focus();
    });
    await user.keyboard("{ContextMenu}");
    expect(await screen.findByRole("menu", { name: t("tree.actions") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(row("Journal")).toHaveFocus();

    await user.keyboard("{F2}");
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.type(input, "{ArrowDown}{Delete}");
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });
});

describe("page tree: requests", () => {
  it("reads the tree once after each change, and nothing else", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL, WORK] });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("tree");
    const count = (type: string): number => recorder.requests.filter((request) => request.type === type).length;
    recorder.requests.length = 0;

    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.keyboard("Inbox{Enter}");
    await waitFor(() => {
      expect(outline().at(-1)).toBe("Inbox");
    });
    // One create and one rename, each followed by exactly one read of the tree.
    expect(count("PageCreate")).toBe(1);
    expect(count("PageRename")).toBe(1);
    expect(count("TreeGet")).toBe(2);
    expect(count("SpaceList")).toBe(0);
  });

  it("renders the sidebar correctly when effects run twice (StrictMode)", async () => {
    await renderSidebar({ strict: true });
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });
});
