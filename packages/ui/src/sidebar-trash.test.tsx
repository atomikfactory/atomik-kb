import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t, tp } from "./i18n";
import { PERSONAL, PERSONAL_OUTLINE, outline, renderSidebar, row } from "./test/sidebar";
import { renderWith } from "./test/render";
import type { User } from "./test/render";

const toggle = (count: number): HTMLElement => screen.getByRole("button", { name: t("trash.title", { count }) });

async function trash(user: User, title: string): Promise<void> {
  act(() => {
    row(title).focus();
  });
  await user.keyboard("{Delete}");
  await waitFor(() => {
    expect(outline()).not.toContain(title);
  });
}

/** The entries of the open trash panel, as `[title, restore button, delete button]`. */
function entries(): { title: string; item: HTMLElement }[] {
  return within(screen.getByRole("list", { name: t("trash.listLabel") }))
    .getAllByRole("listitem")
    .map((item) => ({ title: item.querySelector(".trash-title")?.textContent ?? "", item }));
}

describe("trash", () => {
  it("starts closed with the number of trashed pages, and says when it is empty", async () => {
    const { user } = await renderSidebar();
    expect(toggle(0)).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle(0));
    expect(toggle(0)).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(t("trash.empty"))).toBeInTheDocument();
    await user.click(toggle(0));
    expect(screen.queryByText(t("trash.empty"))).not.toBeInTheDocument();
  });

  it("lists trashed pages, most recent first, with how many pages went with them", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    await trash(user, "Roadmap");
    await trash(user, "Projects");
    expect(outline()).toEqual(["Reading list"]);
    await user.click(toggle(3));
    expect(entries().map((entry) => entry.title)).toEqual(["Projects", "Roadmap", "Journal"]);
    // Projects took Ideas along (Roadmap had already left); Roadmap took Q1.
    const [projects, roadmap, journal] = entries();
    expect(projects?.item).toHaveTextContent(tp("trash.inside", 1));
    expect(roadmap?.item).toHaveTextContent(tp("trash.inside", 1));
    expect(journal?.item).not.toHaveTextContent("inside");
  });

  it("shows pages without a title as Untitled", async () => {
    const { user } = await renderSidebar({ spaces: [{ name: "Personal", pages: [{ title: "" }, { title: "Keep" }] }] });
    await trash(user, t("page.untitled"));
    await user.click(toggle(1));
    expect(entries().map((entry) => entry.title)).toEqual([t("page.untitled")]);
  });

  it("restores a page to where it came from, after its siblings", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Roadmap");
    await user.click(toggle(1));
    const [entry] = entries();
    await user.click(within(entry?.item as HTMLElement).getByRole("button", { name: t("trash.restore") }));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Ideas", "  Roadmap", "    Q1", "Journal", "Reading list"]);
    });
    expect(toggle(0)).toBeInTheDocument();
    expect(screen.getByText(t("trash.empty"))).toBeInTheDocument();
  });

  it("restores to the end of the root when the previous parent is in the trash too", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Roadmap");
    await trash(user, "Projects");
    await user.click(toggle(2));
    expect(entries().map((entry) => entry.title)).toEqual(["Projects", "Roadmap"]);
    const roadmap = entries()[1];
    await user.click(within(roadmap?.item as HTMLElement).getByRole("button", { name: t("trash.restore") }));
    await waitFor(() => {
      expect(outline()).toEqual(["Journal", "Reading list", "Roadmap", "  Q1"]);
    });
  });

  it("keeps the trash of each space apart", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    expect(toggle(1)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Personal" }));
    await user.click(within(screen.getByRole("list", { name: t("spaces.listLabel") })).getByRole("button", { name: "Work" }));
    await waitFor(() => {
      expect(outline()).toEqual(["Meetings"]);
    });
    expect(toggle(0)).toBeInTheDocument();
  });

  it("explains why a page could not be restored", async () => {
    const { user, memory } = await renderSidebar();
    await trash(user, "Journal");
    await user.click(toggle(1));
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(screen.getByRole("button", { name: t("trash.restore") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.restore", { message: "database error: disk I/O error" }),
    );
    expect(outline()).not.toContain("Journal");
  });
});

describe("trash: deleting permanently", () => {
  /** Trashes `title`, opens the trash and presses "Delete permanently" on its entry. */
  async function startPurge(user: User, title: string): Promise<HTMLElement> {
    await trash(user, title);
    await user.click(toggle(1));
    await user.click(screen.getByRole("button", { name: t("trash.purge") }));
    return screen.findByRole("alertdialog", { name: t("trash.purgeTitle") });
  }

  it("asks first, says it cannot be undone, and keeps the page when cancelled", async () => {
    const { user, memory } = await renderSidebar();
    const dialog = await startPurge(user, "Journal");
    expect(dialog).toHaveAccessibleDescription(t("trash.purgeBody.none", { title: "Journal" }));
    expect(dialog).toHaveTextContent("This cannot be undone.");
    expect(within(dialog).getByRole("button", { name: t("dialog.cancel") })).toHaveFocus();

    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("trash.purge") })).toHaveFocus();
    expect(toggle(1)).toBeInTheDocument();
    const client = new CoreClient(memory);
    const [space] = await client.spaceList();
    expect((await client.treeGet(space?.id ?? "")).trashed.map((entry) => entry.page.title)).toEqual(["Journal"]);
  });

  it("starts on the cancel button and gives focus back, also when effects run twice (StrictMode)", async () => {
    const { user } = await renderSidebar({ strict: true });
    const dialog = await startPurge(user, "Journal");
    expect(within(dialog).getByRole("button", { name: t("dialog.cancel") })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("button", { name: t("trash.purge") })).toHaveFocus();
  });

  it("keeps Tab inside the dialog", async () => {
    const { user } = await renderSidebar();
    const dialog = await startPurge(user, "Journal");
    const cancel = within(dialog).getByRole("button", { name: t("dialog.cancel") });
    const confirm = within(dialog).getByRole("button", { name: t("trash.purge") });
    expect(cancel).toHaveFocus();
    await user.tab();
    expect(confirm).toHaveFocus();
    await user.tab();
    expect(cancel).toHaveFocus();
    await user.tab({ shift: true });
    expect(confirm).toHaveFocus();
  });

  it("cancels on Escape and on a click on the backdrop", async () => {
    const { user } = await renderSidebar();
    await startPurge(user, "Journal");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("trash.purge") }));
    const backdrop = (await screen.findByRole("alertdialog")).parentElement as HTMLElement;
    await user.click(backdrop);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(toggle(1)).toBeInTheDocument();
  });

  it("deletes the page for good once confirmed", async () => {
    const { user, memory } = await renderSidebar();
    const client = new CoreClient(memory);
    const [space] = await client.spaceList();
    const before = await client.treeGet(space?.id ?? "");
    const journalId = before.roots.find((node) => node.page.title === "Journal")?.page.id ?? "";

    const dialog = await startPurge(user, "Journal");
    await user.click(within(dialog).getByRole("button", { name: t("trash.purge") }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(toggle(0)).toBeInTheDocument();
    });
    await expect(client.pageGet(journalId)).rejects.toMatchObject({ code: "NotFound" });
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
  });

  it("names the pages that go with it", async () => {
    const { user } = await renderSidebar();
    let dialog = await startPurge(user, "Roadmap");
    expect(dialog).toHaveAccessibleDescription(tp("trash.purgeBody", 1, { title: "Roadmap" }));
    await user.keyboard("{Escape}");

    await trash(user, "Projects");
    const projects = within(screen.getByRole("list", { name: t("trash.listLabel") }))
      .getAllByRole("listitem")
      .find((item) => item.textContent.includes("Projects"));
    await user.click(within(projects as HTMLElement).getByRole("button", { name: t("trash.purge") }));
    dialog = await screen.findByRole("alertdialog", { name: t("trash.purgeTitle") });
    // Roadmap had left Projects already, so only Ideas is inside it.
    expect(dialog).toHaveAccessibleDescription(tp("trash.purgeBody", 1, { title: "Projects" }));
  });

  it("names several pages with a count", async () => {
    const { user } = await renderSidebar({
      spaces: [{ name: "Personal", pages: [{ title: "Big", children: [{ title: "A" }, { title: "B" }, { title: "C" }] }] }],
    });
    const dialog = await startPurge(user, "Big");
    expect(dialog).toHaveAccessibleDescription(tp("trash.purgeBody", 3, { title: "Big" }));
  });

  it("explains why a page could not be deleted and keeps it in the trash", async () => {
    const { user, memory } = await renderSidebar();
    const dialog = await startPurge(user, "Journal");
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(within(dialog).getByRole("button", { name: t("trash.purge") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.purge", { message: "database error: disk I/O error" }),
    );
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(toggle(1)).toBeInTheDocument();
  });
});

describe("undo and redo", () => {
  const undoButton = (): HTMLElement => screen.getByRole("button", { name: t("sidebar.undo") });
  const redoButton = (): HTMLElement => screen.getByRole("button", { name: t("sidebar.redo") });

  it("starts with both buttons disabled and says which shortcuts they have", async () => {
    await renderSidebar();
    expect(undoButton()).toBeDisabled();
    expect(redoButton()).toBeDisabled();
    expect(undoButton()).toHaveAttribute("title", t("sidebar.undoTitle", { shortcut: "Ctrl+Z" }));
    expect(redoButton()).toHaveAttribute("title", t("sidebar.redoTitle", { shortcut: "Ctrl+Shift+Z" }));
  });

  it("enables undo after a change, and redo after an undo, following the core's undo state", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    await waitFor(() => {
      expect(undoButton()).toBeEnabled();
    });
    expect(redoButton()).toBeDisabled();

    await user.click(undoButton());
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
    expect(undoButton()).toBeDisabled();
    expect(redoButton()).toBeEnabled();

    await user.click(redoButton());
    await waitFor(() => {
      expect(outline()).not.toContain("Journal");
    });
    expect(undoButton()).toBeEnabled();
    expect(redoButton()).toBeDisabled();
  });

  it("undoes moves and renames, one change at a time", async () => {
    const { user } = await renderSidebar();
    await user.pointer({ keys: "[MouseRight]", target: row("Journal") });
    await user.click(await screen.findByRole("menuitem", { name: t("tree.menu.moveUp") }));
    await user.dblClick(row("Reading list"));
    await user.type(await screen.findByRole("textbox", { name: t("tree.renameLabel") }), "!{Enter}");
    await waitFor(() => {
      expect(outline()).toEqual(["Journal", "Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list!"]);
    });
    await user.click(undoButton());
    await waitFor(() => {
      expect(outline()).toContain("Reading list");
    });
    expect(outline()[0]).toBe("Journal");
    await user.click(undoButton());
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
  });

  it("undoes with Ctrl+Z and redoes with Ctrl+Shift+Z while no text field has focus", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    await waitFor(() => {
      expect(undoButton()).toBeEnabled();
    });
    await user.keyboard("{Control>}z{/Control}");
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
    await user.keyboard("{Control>}{Shift>}z{/Shift}{/Control}");
    await waitFor(() => {
      expect(outline()).not.toContain("Journal");
    });
  });

  it("answers to the Meta key too (Cmd on macOS)", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    await waitFor(() => {
      expect(undoButton()).toBeEnabled();
    });
    await user.keyboard("{Meta>}z{/Meta}");
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
  });

  it("leaves the shortcut to the text field while a title is being edited", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    await waitFor(() => {
      expect(undoButton()).toBeEnabled();
    });
    await user.dblClick(row("Reading list"));
    const input = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await user.keyboard("{Control>}z{/Control}");
    expect(input).toBeInTheDocument();
    expect(outline()).not.toContain("Journal");
    expect(undoButton()).toBeEnabled();
  });

  it("does nothing when there is nothing to undo or redo", async () => {
    const { user } = await renderSidebar();
    await user.keyboard("{Control>}z{/Control}{Control>}{Shift>}z{/Shift}{/Control}");
    expect(outline()).toEqual(PERSONAL_OUTLINE);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("is not triggered from inside a dialog", async () => {
    const { user } = await renderSidebar();
    await trash(user, "Journal");
    await trash(user, "Reading list");
    await user.click(toggle(2));
    await user.click(screen.getAllByRole("button", { name: t("trash.purge") })[0] as HTMLElement);
    await screen.findByRole("alertdialog");
    await user.keyboard("{Control>}z{/Control}");
    await waitFor(() => {
      expect(undoButton()).toBeEnabled();
    });
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas"]);
  });

  it("explains why an undo failed", async () => {
    const { user, memory } = await renderSidebar();
    await trash(user, "Journal");
    await waitFor(() => {
      expect(undoButton()).toBeEnabled();
    });
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(undoButton());
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.undo", { message: "database error: disk I/O error" }),
    );
  });

  it("keeps the history of each space apart", async () => {
    const memory = new MemoryTransport({
      workspaceName: "Research",
      spaces: [PERSONAL, { name: "Work", pages: [{ title: "Meetings" }] }],
    });
    const client = new CoreClient(memory);
    const [personal, work] = await client.spaceList();
    await client.pageCreate({ spaceId: work?.id ?? "", title: "Standup" });
    renderWith(memory);
    await screen.findByRole("tree");
    // The first space has nothing to undo, although the second one does.
    expect(undoButton()).toBeDisabled();
    expect(personal?.name).toBe("Personal");
  });
});
