import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { PERSONAL_OUTLINE, outline, renderSidebar, row } from "./test/sidebar";
import type { User } from "./test/render";

const ITEMS = [
  t("page.openInNewTab"),
  t("tree.menu.moveUp"),
  t("tree.menu.moveDown"),
  t("tree.menu.indent"),
  t("tree.menu.outdent"),
  t("tree.menu.sortTitle"),
  t("tree.menu.sortCreated"),
  t("page.duplicate"),
  t("moveToSpace.menu"),
  t("tree.menu.rename"),
  t("tree.menu.setIcon"),
  t("favorites.add"),
  t("template.newSubPageFrom"),
  t("template.turnInto"),
  t("tree.menu.trash"),
];

/** Opens the context menu of a page with a right click. */
async function openMenu(user: User, title: string): Promise<HTMLElement> {
  await user.pointer({ keys: "[MouseRight]", target: row(title) });
  return screen.findByRole("menu", { name: t("tree.actions") });
}

async function choose(user: User, title: string, item: string): Promise<void> {
  const menu = await openMenu(user, title);
  await user.click(within(menu).getByRole("menuitem", { name: item }));
}

describe("page context menu", () => {
  it("opens on a right click with the commands in order, at the pointer", async () => {
    const { user } = await renderSidebar();
    await user.pointer({ keys: "[MouseRight]", target: row("Journal"), coords: { clientX: 40, clientY: 90 } });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(ITEMS);
    expect(menu).toHaveStyle({ left: "40px", top: "90px" });
    // The first command that can run has focus.
    expect(within(menu).getByRole("menuitem", { name: t("page.openInNewTab") })).toHaveFocus();
  });

  it("opens from the actions button of a row, and the same button closes it again", async () => {
    const { user } = await renderSidebar();
    const button = within(row("Journal")).getByRole("button", { name: t("tree.actions") });
    await user.click(button);
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    await user.click(button);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    // The click on the button did not open the page.
    expect(row("Journal")).toHaveAttribute("aria-selected", "false");
  });

  it("disables the commands that do not apply to where the page is", async () => {
    const { user } = await renderSidebar();
    const disabledAt = async (title: string): Promise<string[]> => {
      const menu = await openMenu(user, title);
      const disabled = within(menu)
        .getAllByRole("menuitem")
        .filter((item) => (item as HTMLButtonElement).disabled)
        .map((item) => item.textContent ?? "");
      await user.keyboard("{Escape}");
      return disabled;
    };
    const [, up, down, indent, outdent, sortTitle, sortCreated] = ITEMS;
    // The first page: nothing above, no previous sibling to move into, already at the root. It has two subpages: both sorts apply.
    expect(await disabledAt("Projects")).toEqual([up, indent, outdent]);
    // In the middle of the root: only outdent does not apply (and no sort: no subpages to put in order).
    expect(await disabledAt("Journal")).toEqual([outdent, sortTitle, sortCreated]);
    expect(await disabledAt("Reading list")).toEqual([down, outdent, sortTitle, sortCreated]);
    // One subpage is not enough to sort either.
    expect(await disabledAt("Roadmap")).toEqual([up, indent, sortTitle, sortCreated]);
    expect(await disabledAt("Ideas")).toEqual([down, sortTitle, sortCreated]);
    // An only child: nothing to move past and nothing to move into.
    expect(await disabledAt("Q1")).toEqual([up, down, indent, sortTitle, sortCreated]);
  });

  it("moves a page up and down among its siblings", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.moveUp"));
    await waitFor(() => {
      expect(outline()).toEqual(["Journal", "Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
    });
    await choose(user, "Journal", t("tree.menu.moveDown"));
    await choose(user, "Journal", t("tree.menu.moveDown"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list", "Journal"]);
    });
  });

  it("indents a page into the previous sibling, as its last child, and shows it there", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.indent"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "  Journal", "Reading list"]);
    });
  });

  it("expands the previous sibling when a page is indented into a collapsed one", async () => {
    const { user } = await renderSidebar();
    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.collapse") }));
    await choose(user, "Journal", t("tree.menu.indent"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "  Journal", "Reading list"]);
    });
    expect(row("Projects")).toHaveAttribute("aria-expanded", "true");
  });

  it("outdents a page to be the sibling right after its parent", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Ideas", t("tree.menu.outdent"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "Ideas", "Journal", "Reading list"]);
    });
    await choose(user, "Q1", t("tree.menu.outdent"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "  Q1", "Ideas", "Journal", "Reading list"]);
    });
  });

  it("sorts the subpages A to Z and by date created, once, and one undo takes a sort back", async () => {
    let clock = 1_800_000_000_000;
    const { user } = await renderSidebar({ now: () => (clock += 1) });
    await choose(user, "Projects", t("tree.menu.sortTitle"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Ideas", "  Roadmap", "    Q1", "Journal", "Reading list"]);
    });
    // The pages were created in the order of the seed: Roadmap before Ideas.
    await choose(user, "Projects", t("tree.menu.sortCreated"));
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
    await choose(user, "Projects", t("tree.menu.sortTitle"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Ideas", "  Roadmap", "    Q1", "Journal", "Reading list"]);
    });
    await user.click(screen.getByRole("button", { name: t("sidebar.undo") }));
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
    // The sort left no mark: the order is free to change by hand afterwards.
    await choose(user, "Ideas", t("tree.menu.moveUp"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Ideas", "  Roadmap", "    Q1", "Journal", "Reading list"]);
    });
  });

  it("shows the sorted subpages of a collapsed page", async () => {
    const { user } = await renderSidebar();
    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Projects", "Journal", "Reading list"]);
    await choose(user, "Projects", t("tree.menu.sortTitle"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Ideas", "  Roadmap", "    Q1", "Journal", "Reading list"]);
    });
    expect(row("Projects")).toHaveAttribute("aria-expanded", "true");
  });

  it("explains why a sort was refused", async () => {
    const { user, memory } = await renderSidebar();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await choose(user, "Projects", t("tree.menu.sortTitle"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.sort", { message: "database error: disk I/O error" }),
    );
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("renames from the menu", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.rename"));
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(input).toHaveFocus();
    expect(input).toHaveValue("Journal");
  });

  it("moves a page to the trash from the menu", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.trash"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
    });
    expect(screen.getByRole("button", { name: t("trash.title", { count: 1 }) })).toBeInTheDocument();
  });

  it("moves between enabled commands with the arrow keys, wrapping around", async () => {
    const { user } = await renderSidebar();
    const menu = await openMenu(user, "Projects");
    const item = (name: string) => within(menu).getByRole("menuitem", { name });
    // Move up, indent and outdent are disabled for the first page.
    expect(item(t("page.openInNewTab"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("tree.menu.moveDown"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("tree.menu.sortTitle"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("tree.menu.sortCreated"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("page.duplicate"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("moveToSpace.menu"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("tree.menu.rename"))).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(item(t("template.newSubPageFrom"))).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(item(t("tree.menu.trash"))).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(item(t("page.openInNewTab"))).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(item(t("tree.menu.trash"))).toHaveFocus();
    await user.keyboard("{Home}");
    expect(item(t("page.openInNewTab"))).toHaveFocus();
    await user.keyboard("{End}");
    expect(item(t("tree.menu.trash"))).toHaveFocus();
  });

  it("closes on Escape, on a click elsewhere and on Tab, without doing anything", async () => {
    const { user } = await renderSidebar();
    await openMenu(user, "Journal");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await openMenu(user, "Journal");
    await user.click(screen.getByRole("heading", { level: 1 }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await openMenu(user, "Journal");
    await user.tab();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("explains why a move was refused", async () => {
    const { user, memory } = await renderSidebar();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await choose(user, "Journal", t("tree.menu.moveUp"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.move", { message: "database error: disk I/O error" }),
    );
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("takes focus and gives it back also when effects run twice (StrictMode)", async () => {
    const { user } = await renderSidebar({ strict: true });
    const menu = await openMenu(user, "Journal");
    expect(within(menu).getByRole("menuitem", { name: t("page.openInNewTab") })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(row("Journal")).toHaveFocus();

    await choose(user, "Journal", t("tree.menu.setIcon"));
    const picker = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(row("Journal")).toHaveFocus();
  });

  it("also opens from a plain contextmenu event", async () => {
    await renderSidebar();
    act(() => {
      fireEvent.contextMenu(row("Reading list"), { clientX: 10, clientY: 20 });
    });
    expect(await screen.findByRole("menu", { name: t("tree.actions") })).toBeInTheDocument();
  });
});

describe("page icons", () => {
  it("sets an icon from the suggestions and shows it in the tree", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.setIcon"));
    const picker = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveFocus();
    await user.click(within(picker).getByRole("button", { name: "💡" }));
    await waitFor(() => {
      expect(within(row("Journal")).getByText("💡")).toBeInTheDocument();
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("sets a typed icon, shows it for the open page, and removes it again", async () => {
    const { user } = await renderSidebar();
    await user.click(row("Journal"));
    await choose(user, "Journal", t("tree.menu.setIcon"));
    await user.type(await screen.findByRole("textbox", { name: t("icon.label") }), "🚀{Enter}");
    await waitFor(() => {
      expect(within(row("Journal")).getByText("🚀")).toBeInTheDocument();
    });
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("🚀");

    await choose(user, "Journal", t("tree.menu.setIcon"));
    const picker = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveValue("🚀");
    await user.click(within(picker).getByRole("button", { name: t("icon.remove") }));
    await waitFor(() => {
      expect(within(row("Journal")).queryByText("🚀")).not.toBeInTheDocument();
    });
  });

  it("shows the core's reason when an icon is refused and stays open", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.setIcon"));
    const input = await screen.findByRole("textbox", { name: t("icon.label") });
    await user.type(input, "🙂🙂🙂🙂🙂🙂🙂🙂🙂{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Icon must be at most 32 bytes.");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("dialog", { name: t("icon.title") })).toBeInTheDocument();
    // Editing clears the message again.
    await user.type(input, "{Backspace}");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("does not offer to set an empty icon, and closes on Escape", async () => {
    const { user } = await renderSidebar();
    await choose(user, "Journal", t("tree.menu.setIcon"));
    const picker = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(picker).getByRole("button", { name: t("icon.apply") })).toBeDisabled();
    expect(within(picker).queryByRole("button", { name: t("icon.remove") })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(row("Journal")).toHaveFocus();
  });
});
