import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t, tp } from "./i18n";
import { beginDrag } from "./test/dnd";
import { recordingTransport, renderWith } from "./test/render";
import type { User } from "./test/render";
import { PERSONAL, WORK, outline, renderSidebar } from "./test/sidebar";

/** The button at the top of the sidebar that names the current space. */
const switcher = (): HTMLElement => screen.getByRole("button", { description: t("spaces.switchTitle") });
const spaceList = (): HTMLElement => screen.getByRole("list", { name: t("spaces.listLabel") });
const spaceNames = (): string[] => within(spaceList()).getAllByRole("listitem").map((item) => item.textContent);

async function openList(user: User): Promise<HTMLElement> {
  await user.click(switcher());
  return screen.findByRole("dialog", { name: t("spaces.listLabel") });
}

async function openOptions(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  return screen.findByRole("menu", { name: t("spaces.optionsLabel") });
}

async function chooseOption(user: User, name: string): Promise<void> {
  const menu = await openOptions(user);
  await user.click(within(menu).getByRole("menuitem", { name }));
}

describe("space switcher", () => {
  it("names the current space, the first one, with its icon", async () => {
    await renderSidebar();
    expect(switcher()).toHaveTextContent("Personal");
    expect(within(switcher()).getByText("🏠")).toBeInTheDocument();
    expect(switcher()).toHaveAttribute("aria-expanded", "false");
  });

  it("lists the spaces in order with the current one marked, and closes on Escape", async () => {
    const { user } = await renderSidebar();
    const popover = await openList(user);
    expect(switcher()).toHaveAttribute("aria-expanded", "true");
    expect(spaceNames()).toEqual(["🏠Personal", "Work"]);
    expect(within(popover).getByRole("button", { name: "Personal" })).toHaveAttribute("aria-current", "true");
    expect(within(popover).getByRole("button", { name: "Work" })).not.toHaveAttribute("aria-current");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(switcher()).toHaveFocus();
  });

  it("closes when the switcher is pressed again or the page is pressed elsewhere", async () => {
    const { user } = await renderSidebar();
    await openList(user);
    await user.click(switcher());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await openList(user);
    await user.click(screen.getByRole("heading", { level: 1 }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("switches to another space: its pages show, and no page is open", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("treeitem", { name: "Journal" }));
    expect(screen.getByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();

    const popover = await openList(user);
    await user.click(within(popover).getByRole("button", { name: "Work" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(switcher()).toHaveTextContent("Work");
    await waitFor(() => {
      expect(outline()).toEqual(["Meetings"]);
    });
    expect(screen.getByRole("heading", { level: 1, name: "Research" })).toBeInTheDocument();
  });

  it("goes to the home screen when a space is picked from Settings", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: t("nav.settings") }));
    expect(screen.getByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
    const popover = await openList(user);
    await user.click(within(popover).getByRole("button", { name: "Work" }));
    expect(screen.getByRole("heading", { level: 1, name: "Research" })).toBeInTheDocument();
  });

  it("follows the spaces of the core when they change elsewhere", async () => {
    const { user, memory } = await renderSidebar();
    const client = new CoreClient(memory);
    await client.spaceCreate("Added elsewhere", "🌱");
    const [personal] = await client.spaceList();
    await client.spaceRename(personal?.id ?? "", "Renamed elsewhere");
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Renamed elsewhere");
    });
    await openList(user);
    expect(spaceNames()).toEqual(["🏠Renamed elsewhere", "Work", "🌱Added elsewhere"]);
  });

  it("moves on to the first space when the current one is deleted elsewhere", async () => {
    const { user, memory } = await renderSidebar({ spaces: [PERSONAL, { name: "Empty" }, WORK] });
    const popover = await openList(user);
    await user.click(within(popover).getByRole("button", { name: "Empty" }));
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Empty");
    });
    const client = new CoreClient(memory);
    const [, empty] = await client.spaceList();
    await client.spaceDelete(empty?.id ?? "");
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Personal");
    });
    await waitFor(() => {
      expect(outline()).toContain("Journal");
    });
  });
});

describe("creating spaces", () => {
  it("creates a space from the list, switches to it and shows it empty", async () => {
    const { user, memory } = await renderSidebar();
    await openList(user);
    await user.click(screen.getByRole("button", { name: t("spaces.newButton") }));
    const input = screen.getByRole("textbox", { name: t("spaces.nameLabel") });
    expect(input).toHaveFocus();
    await user.type(input, "  Ideas  {Enter}");

    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Ideas");
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByText(t("tree.empty"))).toBeInTheDocument();
    });
    expect(screen.getByText(t("home.hint.emptySpace"))).toBeInTheDocument();
    const names = (await new CoreClient(memory).spaceList()).map((space) => space.name);
    expect(names).toEqual(["Personal", "Work", "Ideas"]);
  });

  it("asks for a name before it asks the core", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("tree");
    await openList(user);
    await user.click(screen.getByRole("button", { name: t("spaces.newButton") }));
    await user.type(screen.getByRole("textbox", { name: t("spaces.nameLabel") }), "   {Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(t("spaces.error.empty"));
    expect(recorder.requests.filter((request) => request.type === "SpaceCreate")).toEqual([]);
  });

  it("shows the core's reason for a name that is refused and keeps what was typed", async () => {
    const { user } = await renderSidebar();
    await openList(user);
    await user.click(screen.getByRole("button", { name: t("spaces.newButton") }));
    const input = screen.getByRole("textbox", { name: t("spaces.nameLabel") });
    await user.click(input);
    await user.paste("x".repeat(101));
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Name must be at most 100 characters.");
    expect(input).toHaveValue("x".repeat(101));
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("dialog", { name: t("spaces.listLabel") })).toBeInTheDocument();
  });

  it("can be cancelled", async () => {
    const { user } = await renderSidebar();
    await openList(user);
    await user.click(screen.getByRole("button", { name: t("spaces.newButton") }));
    await user.click(screen.getByRole("button", { name: t("spaces.cancel") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(switcher()).toHaveTextContent("Personal");
  });
});

describe("a workspace without spaces", () => {
  it("asks for the first space, and shows the pages of the new space", async () => {
    const { user, memory } = await renderSidebar({ spaces: [] });
    expect(screen.getByText(t("spaces.firstTitle"))).toBeInTheDocument();
    expect(screen.getByText(t("home.hint.noSpaces"))).toBeInTheDocument();
    expect(screen.queryByRole("tree")).not.toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: t("spaces.nameLabel") }), "Personal{Enter}");
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Personal");
    });
    expect(screen.queryByText(t("spaces.firstTitle"))).not.toBeInTheDocument();
    expect(await screen.findByText(t("tree.empty"))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    expect(await screen.findByRole("treeitem", { name: t("page.untitled") })).toBeInTheDocument();
    await expect(new CoreClient(memory).spaceList()).resolves.toHaveLength(1);
  });

  it("returns to that state after the last space is deleted", async () => {
    const { user } = await renderSidebar({ spaces: [{ name: "Only" }] });
    await chooseOption(user, t("spaces.delete"));
    const dialog = await screen.findByRole("alertdialog", { name: t("spaces.deleteTitle") });
    await user.click(within(dialog).getByRole("button", { name: t("spaces.deleteConfirm") }));
    expect(await screen.findByText(t("spaces.firstTitle"))).toBeInTheDocument();
  });
});

describe("managing the current space", () => {
  it("renames in place: Enter commits and focus returns to the switcher", async () => {
    const { user } = await renderSidebar();
    await chooseOption(user, t("spaces.rename"));
    const input = await screen.findByRole("textbox", { name: t("spaces.nameLabel") });
    expect(input).toHaveValue("Personal");
    expect(input).toHaveFocus();
    await user.clear(input);
    await user.type(input, "  Home  {Enter}");
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Home");
    });
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(switcher()).toHaveFocus();
  });

  it("cancels a rename with Escape and ignores an unchanged name", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("tree");
    await chooseOption(user, t("spaces.rename"));
    await user.type(await screen.findByRole("textbox", { name: t("spaces.nameLabel") }), " more{Escape}");
    expect(switcher()).toHaveTextContent("Personal");
    await chooseOption(user, t("spaces.rename"));
    await screen.findByRole("textbox", { name: t("spaces.nameLabel") });
    await user.keyboard("{Enter}");
    expect(recorder.requests.filter((request) => request.type === "SpaceRename")).toEqual([]);
  });

  it("explains why a rename was refused", async () => {
    const { user } = await renderSidebar();
    await chooseOption(user, t("spaces.rename"));
    const input = await screen.findByRole("textbox", { name: t("spaces.nameLabel") });
    await user.clear(input);
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("spaces.error.rename", { message: "must not be empty" }),
    );
    expect(switcher()).toHaveTextContent("Personal");
  });

  it("changes and removes the icon", async () => {
    const { user } = await renderSidebar();
    await chooseOption(user, t("spaces.changeIcon"));
    const picker = await screen.findByRole("dialog", { name: t("icon.title") });
    await user.click(within(picker).getByRole("button", { name: "🧠" }));
    await waitFor(() => {
      expect(within(switcher()).getByText("🧠")).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: t("spaces.optionsLabel") })).toHaveFocus();

    await chooseOption(user, t("spaces.changeIcon"));
    await user.click(await screen.findByRole("button", { name: t("icon.remove") }));
    await waitFor(() => {
      expect(within(switcher()).queryByText("🧠")).not.toBeInTheDocument();
    });
  });

  it("moves the space up and down from the menu, disabling what cannot be done", async () => {
    const { user } = await renderSidebar();
    let menu = await openOptions(user);
    expect(within(menu).getByRole("menuitem", { name: t("spaces.moveUp") })).toBeDisabled();
    await user.click(within(menu).getByRole("menuitem", { name: t("spaces.moveDown") }));
    await openList(user);
    await waitFor(() => {
      expect(spaceNames()).toEqual(["Work", "🏠Personal"]);
    });
    await user.keyboard("{Escape}");

    menu = await openOptions(user);
    expect(within(menu).getByRole("menuitem", { name: t("spaces.moveDown") })).toBeDisabled();
    await user.click(within(menu).getByRole("menuitem", { name: t("spaces.moveUp") }));
    await openList(user);
    await waitFor(() => {
      expect(spaceNames()).toEqual(["🏠Personal", "Work"]);
    });
  });
});

describe("sorting the top-level pages (ADR-0052)", () => {
  it("sorts them A to Z and by date created from the space menu, and offers neither for fewer than two pages", async () => {
    let clock = 1_800_000_000_000;
    const { user } = await renderSidebar({ now: () => (clock += 1) });
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal", "Reading list"]);
    await chooseOption(user, t("spaces.sortTitle"));
    await waitFor(() => {
      expect(outline()).toEqual(["Journal", "Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
    });
    await chooseOption(user, t("spaces.sortCreated"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal", "Reading list"]);
    });

    // Work has a single page: both items are there and disabled.
    const popover = await openList(user);
    await user.click(within(popover).getByRole("button", { name: "Work" }));
    await waitFor(() => {
      expect(outline()).toEqual(["Meetings"]);
    });
    const menu = await openOptions(user);
    expect(within(menu).getByRole("menuitem", { name: t("spaces.sortTitle") })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: t("spaces.sortCreated") })).toBeDisabled();
  });

  it("explains why a sort was refused", async () => {
    const { user, memory } = await renderSidebar();
    const menu = await openOptions(user);
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(within(menu).getByRole("menuitem", { name: t("spaces.sortTitle") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("spaces.error.sort", { message: "database error: disk I/O error" }),
    );
  });
});

describe("reordering spaces by drag and drop", () => {
  it("puts a space before or after another one", async () => {
    const { user } = await renderSidebar({ spaces: [PERSONAL, WORK, { name: "Play" }] });
    await openList(user);
    const items = (): HTMLElement[] => within(spaceList()).getAllByRole("listitem");

    const first = beginDrag(items()[2] as HTMLElement);
    expect(first.over(items()[0] as HTMLElement, "before")).toBe(true);
    expect(items()[0]).toHaveClass("drop-before");
    first.drop(items()[0] as HTMLElement, "before");
    await waitFor(() => {
      expect(spaceNames()).toEqual(["Play", "🏠Personal", "Work"]);
    });

    const second = beginDrag(items()[0] as HTMLElement);
    expect(second.over(items()[2] as HTMLElement, "after")).toBe(true);
    expect(items()[2]).toHaveClass("drop-after");
    second.drop(items()[2] as HTMLElement, "after");
    await waitFor(() => {
      expect(spaceNames()).toEqual(["🏠Personal", "Work", "Play"]);
    });
    for (const item of items()) {
      expect(item.className).toBe("space-row");
    }
  });

  it("does not accept a space on itself and does not ask the core to move it where it is", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL, WORK] });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("tree");
    await openList(user);
    const [personal, work] = within(spaceList()).getAllByRole("listitem") as [HTMLElement, HTMLElement];

    expect(beginDrag(personal).over(personal, "before")).toBe(false);
    const drag = beginDrag(work);
    expect(drag.over(personal, "after")).toBe(true);
    drag.drop(personal, "after");
    expect(recorder.requests.filter((request) => request.type === "SpaceMove")).toEqual([]);
    expect(spaceNames()).toEqual(["🏠Personal", "Work"]);
  });
});

describe("deleting a space", () => {
  it("is offered for a space that has pages", async () => {
    const { user } = await renderSidebar();
    const menu = await openOptions(user);
    expect(within(menu).getByRole("menuitem", { name: t("spaces.delete") })).toBeEnabled();
  });

  it("asks for the space name and deletes the space with its live and trashed pages", async () => {
    const { user, memory } = await renderSidebar({
      spaces: [{ name: "Trashy", pages: [{ title: "Kept" }, { title: "Gone" }] }],
    });
    await user.click(screen.getByRole("treeitem", { name: "Gone" }));
    await user.keyboard("{Delete}");
    await waitFor(() => {
      expect(screen.queryByRole("treeitem", { name: "Gone" })).not.toBeInTheDocument();
    });

    await chooseOption(user, t("spaces.delete"));
    const dialog = await screen.findByRole("alertdialog", { name: t("spaces.deleteTitle") });
    expect(dialog).toHaveAccessibleDescription(tp("spaces.deleteBodyPages", 2, { name: "Trashy" }));
    const confirm = within(dialog).getByRole("button", { name: t("spaces.deleteConfirm") });
    expect(confirm).toBeDisabled();
    const field = within(dialog).getByRole("textbox", { name: t("spaces.deleteTypeLabel") });
    await user.type(field, "Trash");
    expect(confirm).toBeDisabled();
    await user.type(field, "y");
    expect(confirm).toBeEnabled();
    await user.click(confirm);

    await waitFor(async () => {
      const names = (await new CoreClient(memory).spaceList()).map((space) => space.name);
      expect(names).not.toContain("Trashy");
    });
  });

  it("deletes an empty space after a confirmation and moves on to another one", async () => {
    const { user, memory } = await renderSidebar();
    await openList(user);
    await user.click(screen.getByRole("button", { name: t("spaces.newButton") }));
    await user.type(screen.getByRole("textbox", { name: t("spaces.nameLabel") }), "Scratch{Enter}");
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Scratch");
    });
    await screen.findByText(t("tree.empty"));

    await chooseOption(user, t("spaces.delete"));
    const dialog = await screen.findByRole("alertdialog", { name: t("spaces.deleteTitle") });
    expect(dialog).toHaveAccessibleDescription(t("spaces.deleteBody", { name: "Scratch" }));
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(switcher()).toHaveTextContent("Scratch");
    expect(screen.getByRole("button", { name: t("spaces.optionsLabel") })).toHaveFocus();

    await chooseOption(user, t("spaces.delete"));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("spaces.deleteConfirm") }),
    );
    await waitFor(() => {
      expect(switcher()).toHaveTextContent("Personal");
    });
    await waitFor(() => {
      expect(outline()).toContain("Journal");
    });
    const names = (await new CoreClient(memory).spaceList()).map((space) => space.name);
    expect(names).toEqual(["Personal", "Work"]);
  });
});
