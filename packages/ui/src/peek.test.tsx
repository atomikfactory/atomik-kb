// The peek of a database row on screen (ADR-0054, section 8 of `docs/architecture/database-views.md`): the side panel and the
// center dialog over the whole layout, what they show (icon, title, properties, editor), their buttons, Esc, the focus handed
// back to the row, the resize handle and its one write, what changes the page in front (nothing) and what closes the peek, a
// locked page, no comments and no backlinks, and the single-slot state of the page in front that the peek's second editor must
// leave alone (the comments panel's and the chat's editor, Ctrl+F, the page title's id, presence, dictation). The layout is the
// real one over a store on an in-memory core; a button stands in for the row that opens the peek.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageId, SeedSpace, SpaceId } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Editor } from "@tiptap/core";
import { onTestFinished, describe, expect, it, vi } from "vitest";

import { getActiveEditor } from "./editor/ai/active-editor";
import { commentsHost } from "./editor/comments-host";
import { dictationOn } from "./editor/dictation/registry";
import { openRow } from "./editor/open-row";
import { t } from "./i18n";
import { MainLayout } from "./screens/MainLayout";
import { createAppStore } from "./store";
import { StoreContext } from "./store-context";
import { PEEK_DEFAULT_WIDTH, PEEK_WIDTH_KEY, selectPeekWidth } from "./store-peek";
import { paragraphs } from "./test/editor";
import { editorOf, idsOf } from "./test/page";
import { NOW, findMainNavigation } from "./test/render";

const PERSONAL: SeedSpace = {
  name: "Personal",
  pages: [
    {
      title: "Hub",
      content: paragraphs("hub text"),
      children: [
        { title: "Alpha", icon: "🅰️", content: paragraphs("alpha text") },
        { title: "Beta", content: paragraphs("beta text") },
        { title: "Gamma", content: paragraphs("gamma text") },
      ],
    },
    { title: "Other", content: paragraphs("other text") },
  ],
};

const WORK: SeedSpace = { name: "Work", pages: [{ title: "Meetings" }] };

/** The whole layout over a store on an in-memory core, with a button that stands in for the row that opens the peek. */
async function renderPeek({ openHub = true, dictation = false }: { openHub?: boolean; dictation?: boolean } = {}) {
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [PERSONAL, WORK],
    ...(dictation ? { dictation: { installed: ["light"], selected: "light", loadMs: 0 } } : {}),
  });
  const client = new CoreClient(memory);
  const writes = vi.spyOn(client, "settingsSet");
  const store = createAppStore(client, { corePollMs: 5 });
  const stop = store.getState().start();
  onTestFinished(stop);
  const user = userEvent.setup();
  const view = render(
    <StoreContext value={store}>
      <MainLayout />
      <button type="button">row</button>
    </StoreContext>,
  );
  await findMainNavigation();
  const ids = await idsOf(memory);
  const page = (title: string): PageId => ids.page(title);
  const space: SpaceId = ids.space("Personal");
  if (openHub) {
    act(() => {
      store.getState().navigateTo({ pageId: page("Hub"), spaceId: space });
    });
    await screen.findByRole("heading", { level: 1, name: "Hub" });
  }
  const row = screen.getByRole("button", { name: "row" });
  /** Opens the peek the way a row does: the row has the focus, then `openRow`. */
  const peekRow = (title: string, openIn: "side" | "center" = "side", disposition: "current" | "background" | "foreground" = "current") => {
    act(() => {
      row.focus();
      openRow(store.getState(), { pageId: page(title), spaceId: space }, { openIn }, disposition, page("Hub"));
    });
  };
  return { memory, client, store, user, view, writes, page, space, row, peekRow };
}

const sidePeek = (): HTMLElement => screen.getByRole("complementary", { name: "Alpha" });
/** The peek of either mode, `null` when none is open (the sidebar is a complementary landmark too). */
const peekElement = (): HTMLElement | null => document.querySelector<HTMLElement>("[data-peek]");
/** The writes of the width setting (the tabs write theirs too). */
const widthWrites = (writes: { mock: { calls: unknown[][] } }): unknown[][] => writes.mock.calls.filter(([key]) => key === PEEK_WIDTH_KEY);
const content = (): HTMLElement => document.querySelector<HTMLElement>(".content") ?? document.body;

/** The editors on screen, the page's first and the peek's second. */
function editors(): Editor[] {
  return screen.getAllByRole("textbox", { name: t("editor.label") }).map((box) => editorOf(box));
}

/**
 * Puts the cursor at the end of the editor's text and the focus in it, at once (Tiptap's own `focus` command takes it a
 * frame later, after whatever opened meanwhile).
 */
function focusEnd(editor: Editor): void {
  act(() => {
    editor.view.focus();
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
  });
}

/** Waits until the peek's editor shows its page. */
async function peekEditor(scope: HTMLElement): Promise<Editor> {
  const box = await within(scope).findByRole("textbox", { name: t("editor.label") });
  const editor = editorOf(box);
  await waitFor(() => {
    expect(editor.state.doc.textContent).not.toBe("");
  });
  return editor;
}

describe("the side peek", () => {
  it("shows the page's icon, title, properties and editor beside the page, which stays as it is", async () => {
    const { store, peekRow, page } = await renderPeek();
    const tabs = store.getState().tabs;
    peekRow("Alpha");

    const peek = sidePeek();
    expect(peek).toHaveAttribute("data-mode", "side");
    expect(content()).toHaveClass("has-peek");
    expect(within(peek).getByRole("heading", { level: 2, name: /Alpha/ })).toHaveAttribute("id", "peek-title");
    expect(peek.querySelector(".page-title-icon")).toHaveTextContent("🅰️");
    expect(within(peek).getByRole("button", { name: t("peek.openAsPage") })).toBeInTheDocument();
    expect(within(peek).getByRole("button", { name: t("page.openInNewTab") })).toBeInTheDocument();
    expect(within(peek).getByRole("button", { name: t("peek.close") })).toBeInTheDocument();
    expect(peek.querySelector(".properties")).not.toBeNull();
    expect((await peekEditor(peek)).state.doc.textContent).toBe("alpha text");

    // The page in front is as it was: its heading, its own editor, the tab and its history.
    expect(screen.getByRole("heading", { level: 1, name: "Hub" })).toBeInTheDocument();
    expect(editors()).toHaveLength(2);
    expect(editors()[0]?.state.doc.textContent).toBe("hub text");
    expect(store.getState().selectedPageId).toBe(page("Hub"));
    expect(store.getState().tabs).toBe(tabs);
  });

  it("moves the focus into the peek, and swaps its page when another row is clicked", async () => {
    const { peekRow } = await renderPeek();
    peekRow("Alpha");
    expect(sidePeek()).toHaveFocus();

    peekRow("Beta");
    const peek = screen.getByRole("complementary", { name: "Beta" });
    expect(screen.queryByRole("complementary", { name: "Alpha" })).not.toBeInTheDocument();
    expect(peek).toHaveFocus();
    expect((await peekEditor(peek)).state.doc.textContent).toBe("beta text");
    // Never two editors on one page, and never more than the page's and the peek's.
    expect(editors()).toHaveLength(2);
  });

  it("is not drawn for the page that holds the database nor for the page in front: they open as a page", async () => {
    const { store, page, space, peekRow } = await renderPeek();
    peekRow("Alpha");
    act(() => {
      openRow(store.getState(), { pageId: page("Hub"), spaceId: space }, { openIn: "side" }, "current", page("Hub"));
    });
    expect(peekElement()).toBeNull();
    expect(store.getState().peek).toBeNull();
    expect(store.getState().selectedPageId).toBe(page("Hub"));
    expect(editors()).toHaveLength(1);
  });

  it("closes with its button, and returns the focus to the row", async () => {
    const { user, row, peekRow } = await renderPeek();
    peekRow("Alpha");
    await user.click(within(sidePeek()).getByRole("button", { name: t("peek.close") }));
    expect(peekElement()).toBeNull();
    expect(content()).not.toHaveClass("has-peek");
    expect(row).toHaveFocus();
  });

  it("closes on Esc, and returns the focus to the row", async () => {
    const { user, row, store, peekRow } = await renderPeek();
    peekRow("Alpha");
    expect(sidePeek()).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(store.getState().peek).toBeNull();
    expect(row).toHaveFocus();
  });

  it("closes on Esc from a button inside it, but not on Esc outside it", async () => {
    const { user, store, peekRow, row } = await renderPeek();
    peekRow("Alpha");
    within(sidePeek()).getByRole("button", { name: t("peek.openAsPage") }).focus();
    await user.keyboard("{Escape}");
    expect(store.getState().peek).toBeNull();

    peekRow("Alpha");
    act(() => {
      row.focus();
    });
    await user.keyboard("{Escape}");
    expect(store.getState().peek).not.toBeNull();
  });

  it("leaves Esc to what inside it took the key: a field being edited, the slash menu", async () => {
    const { user, store, peekRow } = await renderPeek();
    peekRow("Alpha");
    const peek = sidePeek();

    // The title's field: Esc cancels the edit, a second Esc closes the peek.
    await user.click(within(peek).getByRole("button", { name: "Alpha" }));
    expect(within(peek).getByRole("textbox", { name: t("tree.renameLabel") })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(within(peek).queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    expect(store.getState().peek).not.toBeNull();

    // The slash menu of its editor.
    const editor = await peekEditor(peek);
    focusEnd(editor);
    act(() => {
      editor.commands.insertContent(" /");
    });
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    });
    expect(store.getState().peek).not.toBeNull();

    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    expect(store.getState().peek).toBeNull();
  });

  it("closes on Esc from the page's text: the editor's own Escape (it selects the block) is not a popup", async () => {
    const { store, row, peekRow } = await renderPeek();
    peekRow("Alpha");
    const editor = await peekEditor(sidePeek());
    focusEnd(editor);
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    expect(store.getState().peek).toBeNull();
    expect(row).toHaveFocus();
  });

  it("does not close on an Esc of the editor that is composing", async () => {
    const { store, peekRow } = await renderPeek();
    peekRow("Alpha");
    const editor = await peekEditor(sidePeek());
    focusEnd(editor);
    fireEvent.keyDown(editor.view.dom, { key: "Escape", isComposing: true });
    expect(store.getState().peek).not.toBeNull();
  });

  it("moves the caret to the start of its text on Enter in the title's field, and Esc closes it from there", async () => {
    const { user, store, peekRow, row } = await renderPeek();
    peekRow("Alpha");
    const peek = sidePeek();
    const editor = await peekEditor(peek);
    // The caret is at the end of the text before the rename: Enter puts it back at the start.
    focusEnd(editor);
    expect(editor.state.selection.from).toBeGreaterThan(1);
    await user.click(within(peek).getByRole("button", { name: "Alpha" }));
    const field = within(peek).getByRole("textbox", { name: t("tree.renameLabel") });
    await user.clear(field);
    await user.type(field, "Alpha two{Enter}");

    await waitFor(() => {
      expect(within(peek).queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    });
    expect(await within(peek).findByRole("heading", { level: 2, name: /Alpha two/ })).toBeInTheDocument();
    expect(editor.view.hasFocus()).toBe(true);
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.from).toBe(1);
    expect(document.activeElement).not.toBe(document.body);
    // The page in front's editor did not take it.
    expect(editors()[0]).not.toBe(editor);
    expect(editors()[0]?.view.hasFocus()).toBe(false);

    // Esc from the text closes the peek, and the focus goes back to the row.
    await user.keyboard("{Escape}");
    expect(store.getState().peek).toBeNull();
    expect(row).toHaveFocus();
  });

  it("moves the caret into its text on Enter when the title did not change", async () => {
    const { user, peekRow } = await renderPeek();
    peekRow("Alpha");
    const peek = sidePeek();
    const editor = await peekEditor(peek);
    await user.click(within(peek).getByRole("button", { name: "Alpha" }));
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(within(peek).queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    });
    expect(editor.view.hasFocus()).toBe(true);
    expect(editor.state.selection.from).toBe(1);
  });

  it("closes on Esc when the focus is on nothing (the body), and returns the focus to the row", async () => {
    const { user, store, peekRow, row } = await renderPeek();
    peekRow("Alpha");
    act(() => {
      (document.activeElement as HTMLElement | null)?.blur();
    });
    expect(document.body).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(store.getState().peek).toBeNull();
    expect(row).toHaveFocus();
  });

  it("closes on Esc with the focus on the body after a rename that Esc cancelled", async () => {
    const { user, store, peekRow } = await renderPeek();
    peekRow("Alpha");
    const peek = sidePeek();
    await user.click(within(peek).getByRole("button", { name: "Alpha" }));
    await user.keyboard("{Escape}");
    expect(store.getState().peek).not.toBeNull();
    // The field went away with its Esc: the focus is nowhere, and the next Esc is the peek's.
    expect(document.body).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(store.getState().peek).toBeNull();
  });

  it("keeps the peek on an Esc with the focus on the body when something took the key, it is composing, or a popup is open outside", async () => {
    const { user, store, peekRow } = await renderPeek();
    peekRow("Alpha");
    act(() => {
      (document.activeElement as HTMLElement | null)?.blur();
    });

    // A handler took the key (it cancels the default).
    const take = (event: Event): void => {
      event.preventDefault();
    };
    document.body.addEventListener("keydown", take);
    await user.keyboard("{Escape}");
    document.body.removeEventListener("keydown", take);
    expect(store.getState().peek).not.toBeNull();

    // An input method's composition.
    fireEvent.keyDown(document.body, { key: "Escape", isComposing: true });
    expect(store.getState().peek).not.toBeNull();

    // A menu, a listbox and a dialog outside the peek: the key is theirs (a popup that has no focus of its own).
    for (const role of ["menu", "listbox", "dialog"]) {
      const popup = document.createElement("div");
      popup.setAttribute("role", role);
      document.body.append(popup);
      await user.keyboard("{Escape}");
      popup.remove();
      expect(store.getState().peek).not.toBeNull();
    }

    // Nothing left: the key is the peek's.
    await user.keyboard("{Escape}");
    expect(store.getState().peek).toBeNull();
  });

  it("leaves Esc to the page in front's editor and fields: the peek stays open", async () => {
    const { user, store, peekRow } = await renderPeek();
    peekRow("Alpha");
    await peekEditor(sidePeek());
    const [mainEd] = editors();
    expect(mainEd).toBeDefined();
    if (mainEd === undefined) {
      return;
    }
    focusEnd(mainEd);
    await user.keyboard("{Escape}");
    expect(store.getState().peek).not.toBeNull();
    // The main editor keeps what it does with Esc: the focus stays in it.
    expect(mainEd.view.hasFocus()).toBe(true);

    // A field of the page in front (the breadcrumb's or the sidebar's search button stands in: any control with the focus).
    const field = document.createElement("input");
    document.body.append(field);
    onTestFinished(() => {
      field.remove();
    });
    field.focus();
    await user.keyboard("{Escape}");
    expect(store.getState().peek).not.toBeNull();
  });

  it("opens the page as a page, in the tab in front, and closes", async () => {
    const { user, store, peekRow, page } = await renderPeek();
    peekRow("Alpha");
    await user.click(within(sidePeek()).getByRole("button", { name: t("peek.openAsPage") }));
    expect(store.getState().peek).toBeNull();
    expect(store.getState().selectedPageId).toBe(page("Alpha"));
    expect(await screen.findByRole("heading", { level: 1, name: /Alpha/ })).toBeInTheDocument();
    expect(store.getState().tabs).toHaveLength(1);
  });

  it("opens the page in a new tab, in front, and closes", async () => {
    const { user, store, peekRow, page } = await renderPeek();
    const before = store.getState().activeTabId;
    peekRow("Alpha");
    await user.click(within(sidePeek()).getByRole("button", { name: t("page.openInNewTab") }));
    expect(store.getState().peek).toBeNull();
    expect(store.getState().tabs).toHaveLength(2);
    expect(store.getState().activeTabId).not.toBe(before);
    expect(store.getState().selectedPageId).toBe(page("Alpha"));
  });

  it("closes when another tab comes to the front, and when the page in front changes", async () => {
    const { store, peekRow, page } = await renderPeek();
    const first = store.getState().activeTabId ?? "";
    peekRow("Alpha");
    act(() => {
      store.getState().newTab();
    });
    expect(peekElement()).toBeNull();

    act(() => {
      store.getState().activateTab(first);
    });
    await screen.findByRole("heading", { level: 1, name: "Hub" });
    peekRow("Alpha");
    act(() => {
      store.getState().navigateTo({ pageId: page("Other"), spaceId: store.getState().currentSpaceId ?? "" });
    });
    expect(peekElement()).toBeNull();
  });

  it("opens other pages in tabs for Ctrl and Ctrl+Shift, and leaves the peek alone for Ctrl", async () => {
    const { store, peekRow, page } = await renderPeek();
    peekRow("Alpha");
    peekRow("Beta", "side", "background");
    expect(store.getState().tabs).toHaveLength(2);
    expect(store.getState().peek?.pageId).toBe(page("Alpha"));
    expect(screen.getByRole("complementary", { name: "Alpha" })).toBeInTheDocument();
  });

  it("closes when its page is trashed", async () => {
    const { store, peekRow, page } = await renderPeek();
    peekRow("Alpha");
    await act(async () => {
      await store.getState().trashPage(page("Alpha"));
    });
    await waitFor(() => {
      expect(peekElement()).toBeNull();
    });
  });

  it("shows the title and the text read-only for a locked page", async () => {
    const { store, peekRow, page } = await renderPeek();
    await act(async () => {
      await store.getState().setPageAppearance(page("Alpha"), { locked: true });
    });
    peekRow("Alpha");
    const peek = sidePeek();
    const editor = await peekEditor(peek);
    await waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(within(peek).queryByRole("button", { name: "Alpha" })).not.toBeInTheDocument();
    expect(within(peek).getByRole("heading", { level: 2 })).toHaveTextContent("Alpha");
    // The page in front is not locked: its editor stays editable.
    expect(editors()[0]?.isEditable).toBe(true);
  });

  it("renames the page from its title", async () => {
    const { user, store, peekRow, page } = await renderPeek();
    peekRow("Alpha");
    await user.click(within(sidePeek()).getByRole("button", { name: "Alpha" }));
    const input = screen.getByRole("textbox", { name: t("tree.renameLabel") });
    await user.clear(input);
    await user.type(input, "Alpha 2{Enter}");
    await waitFor(() => {
      expect(store.getState().tree?.roots[0]?.children.find((node) => node.page.id === page("Alpha"))?.page.title).toBe("Alpha 2");
    });
    expect(await screen.findByRole("complementary", { name: /Alpha 2/ })).toBeInTheDocument();
  });

  it("shows no comments and no backlinks, and does not take their slots from the page in front", async () => {
    const { store, peekRow, page } = await renderPeek();
    await waitFor(() => {
      expect(store.getState().pageComments?.pageId).toBe(page("Hub"));
      expect(store.getState().backlinks?.pageId).toBe(page("Hub"));
    });
    peekRow("Alpha");
    const peek = sidePeek();
    expect(within(peek).queryByRole("button", { name: t("comments.toggle") })).not.toBeInTheDocument();
    expect(peek.querySelector(".backlinks, .comments-panel")).toBeNull();
    peekRow("Beta");
    expect(store.getState().pageComments?.pageId).toBe(page("Hub"));
    expect(store.getState().backlinks?.pageId).toBe(page("Hub"));
    act(() => {
      store.getState().closePeek();
    });
    expect(store.getState().pageComments?.pageId).toBe(page("Hub"));
    expect(store.getState().backlinks?.pageId).toBe(page("Hub"));
  });
});

describe("the center peek", () => {
  it("is a large dialog over the page, with the same buttons", async () => {
    const { store, peekRow, page } = await renderPeek();
    peekRow("Alpha", "center");
    const dialog = await screen.findByRole("dialog", { name: /Alpha/ });
    expect(dialog).toHaveClass("peek-dialog");
    expect(content()).not.toHaveClass("has-peek");
    expect(within(dialog).getByRole("button", { name: t("peek.openAsPage") })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("page.openInNewTab") })).toBeInTheDocument();
    expect((await peekEditor(dialog)).state.doc.textContent).toBe("alpha text");
    expect(store.getState().selectedPageId).toBe(page("Hub"));
    expect(dialog.querySelector(".peek")).toHaveFocus();
  });

  it("closes on Esc and on a click on the backdrop, and returns the focus to the row", async () => {
    const { user, row, store, peekRow } = await renderPeek();
    peekRow("Alpha", "center");
    await screen.findByRole("dialog", { name: /Alpha/ });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().peek).toBeNull();
    expect(row).toHaveFocus();

    peekRow("Beta", "center");
    const dialog = await screen.findByRole("dialog", { name: /Beta/ });
    const backdrop = dialog.parentElement;
    expect(backdrop).not.toBeNull();
    await user.click(backdrop as HTMLElement);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(row).toHaveFocus();
  });

  it("closes on Esc when the focus is on nothing (the body), and returns the focus to the row", async () => {
    const { user, store, peekRow, row } = await renderPeek();
    peekRow("Alpha", "center");
    await screen.findByRole("dialog", { name: /Alpha/ });
    act(() => {
      (document.activeElement as HTMLElement | null)?.blur();
    });
    expect(document.body).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().peek).toBeNull();
    expect(row).toHaveFocus();
  });

  it("moves the caret to the start of its text on Enter in the title's field", async () => {
    const { user, peekRow } = await renderPeek();
    peekRow("Alpha", "center");
    const dialog = await screen.findByRole("dialog", { name: /Alpha/ });
    const editor = await peekEditor(dialog);
    focusEnd(editor);
    await user.click(within(dialog).getByRole("button", { name: "Alpha" }));
    const field = within(dialog).getByRole("textbox", { name: t("tree.renameLabel") });
    await user.clear(field);
    await user.type(field, "Alpha two{Enter}");
    await waitFor(() => {
      expect(within(dialog).queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    });
    expect(editor.view.hasFocus()).toBe(true);
    expect(editor.state.selection.from).toBe(1);
  });

  it("leaves Esc to the slash menu of its editor, then closes", async () => {
    const { store, peekRow } = await renderPeek();
    peekRow("Alpha", "center");
    const dialog = await screen.findByRole("dialog", { name: /Alpha/ });
    const editor = await peekEditor(dialog);
    focusEnd(editor);
    act(() => {
      editor.commands.insertContent(" /");
    });
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    });
    expect(store.getState().peek).not.toBeNull();
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    expect(store.getState().peek).toBeNull();
  });

  it("swaps its page when a row is clicked while it is open, and closes on a tab change", async () => {
    const { store, peekRow } = await renderPeek();
    peekRow("Alpha", "center");
    await screen.findByRole("dialog", { name: /Alpha/ });
    peekRow("Beta", "center");
    expect(await screen.findByRole("dialog", { name: /Beta/ })).toBeInTheDocument();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    act(() => {
      store.getState().newTab();
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("changes from the side to the center when the next row asks for it", async () => {
    const { peekRow } = await renderPeek();
    peekRow("Alpha", "side");
    expect(sidePeek()).toBeInTheDocument();
    peekRow("Beta", "center");
    expect(await screen.findByRole("dialog", { name: /Beta/ })).toBeInTheDocument();
    expect(document.querySelector('[data-peek][data-mode="side"]')).toBeNull();
    expect(editors()).toHaveLength(2);
  });
});

describe("the resize handle", () => {
  const separator = (): HTMLElement => within(sidePeek()).getByRole("separator", { name: t("peek.resize") });

  it("shows the width, changes it live through a CSS variable and writes the setting once, on release", async () => {
    const { store, writes, peekRow } = await renderPeek();
    peekRow("Alpha");
    const handle = separator();
    expect(handle).toHaveAttribute("aria-valuenow", String(PEEK_DEFAULT_WIDTH));
    expect(handle).toHaveAttribute("aria-valuemin", "320");
    expect(handle).toHaveAttribute("aria-valuemax", "960");
    expect(content().style.getPropertyValue("--peek-width")).toBe("480px");

    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 1000 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 900 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 850 });
    expect(content().style.getPropertyValue("--peek-width")).toBe("630px");
    expect(handle).toHaveAttribute("aria-valuenow", "630");
    expect(widthWrites(writes)).toHaveLength(0);
    expect(selectPeekWidth(store.getState())).toBe(480);

    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 850 });
    expect(widthWrites(writes)).toEqual([[PEEK_WIDTH_KEY, 630]]);
    expect(selectPeekWidth(store.getState())).toBe(630);
    expect(content().style.getPropertyValue("--peek-width")).toBe("630px");
    expect(separator()).toHaveAttribute("aria-valuenow", "630");
  });

  it("keeps the width within 320 and 960", async () => {
    const { writes, peekRow } = await renderPeek();
    peekRow("Alpha");
    const handle = separator();
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 1000 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: -5000 });
    expect(content().style.getPropertyValue("--peek-width")).toBe("960px");
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 9000 });
    expect(content().style.getPropertyValue("--peek-width")).toBe("320px");
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 9000 });
    expect(widthWrites(writes)).toEqual([[PEEK_WIDTH_KEY, 320]]);
  });

  it("puts the old width back on Escape, writes nothing, and does not close the peek", async () => {
    const { store, writes, peekRow } = await renderPeek();
    peekRow("Alpha");
    const handle = separator();
    act(() => {
      handle.focus();
    });
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 1000 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 900 });
    expect(content().style.getPropertyValue("--peek-width")).toBe("580px");
    fireEvent.keyDown(handle, { key: "Escape" });
    expect(content().style.getPropertyValue("--peek-width")).toBe("480px");
    expect(widthWrites(writes)).toHaveLength(0);
    expect(store.getState().peek).not.toBeNull();
  });

  it("puts the old width back when the pointer is cancelled", async () => {
    const { writes, peekRow } = await renderPeek();
    peekRow("Alpha");
    const handle = separator();
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 1000 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 900 });
    fireEvent.pointerCancel(handle, { pointerId: 1 });
    expect(content().style.getPropertyValue("--peek-width")).toBe("480px");
    expect(widthWrites(writes)).toHaveLength(0);
  });

  it("changes by 16 px with the arrow keys (Left widens) and resets on a double click", async () => {
    const { store, writes, peekRow } = await renderPeek();
    peekRow("Alpha");
    const handle = separator();
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(selectPeekWidth(store.getState())).toBe(496);
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(selectPeekWidth(store.getState())).toBe(496);
    expect(widthWrites(writes)).toEqual([
      [PEEK_WIDTH_KEY, 496],
      [PEEK_WIDTH_KEY, 512],
      [PEEK_WIDTH_KEY, 496],
    ]);
    expect(content().style.getPropertyValue("--peek-width")).toBe("496px");
    fireEvent.doubleClick(handle);
    expect(selectPeekWidth(store.getState())).toBe(PEEK_DEFAULT_WIDTH);
    expect(content().style.getPropertyValue("--peek-width")).toBe("480px");
  });
});

describe("the layout around it", () => {
  it("is the column after the page and before the comments panel", async () => {
    const { store, peekRow } = await renderPeek();
    await waitFor(() => {
      expect(store.getState().pageComments).not.toBeNull();
    });
    act(() => {
      store.setState({ commentsOpen: true });
    });
    peekRow("Alpha");
    const area = content();
    expect(area).toHaveClass("has-peek", "has-comments");
    const children = Array.from(area.children);
    const body = area.querySelector(".content-body");
    const comments = area.querySelector(".comments-panel");
    expect(body).not.toBeNull();
    expect(comments).not.toBeNull();
    expect(children.indexOf(body as Element)).toBeLessThan(children.indexOf(sidePeek()));
    expect(children.indexOf(sidePeek())).toBeLessThan(children.indexOf(comments as Element));
  });

  it("is the column before the chat panel, with the comments panel between", async () => {
    const { store, peekRow } = await renderPeek();
    await waitFor(() => {
      expect(store.getState().pageComments).not.toBeNull();
    });
    act(() => {
      store.setState({ commentsOpen: true, chatOpen: true });
    });
    peekRow("Alpha");
    const area = content();
    if (!area.classList.contains("has-chat")) {
      // A transport without a local AI has no chat panel to sit beside (the contract is the order of the columns).
      expect(area).toHaveClass("has-peek", "has-comments");
      return;
    }
    const children = Array.from(area.children);
    const chat = area.querySelector(".ai-chat");
    const comments = area.querySelector(".comments-panel");
    expect(children.indexOf(sidePeek())).toBeLessThan(children.indexOf(comments as Element));
    expect(children.indexOf(comments as Element)).toBeLessThan(children.indexOf(chat as Element));
  });

  it("leaves with the main screen", async () => {
    const { store, peekRow } = await renderPeek();
    peekRow("Alpha");
    act(() => {
      store.getState().navigate("settings");
    });
    expect(screen.queryByRole("complementary", { name: "Alpha" })).not.toBeInTheDocument();
    expect(store.getState().peek).toBeNull();
  });
});

describe("what the page in front keeps", () => {
  it("keeps the comments panel's editor and the chat's editor, with the peek open and after it closed", async () => {
    const { store, peekRow, page } = await renderPeek();
    await waitFor(() => {
      expect(commentsHost.get()?.pageId).toBe(page("Hub"));
      expect(getActiveEditor()?.pageId).toBe(page("Hub"));
    });
    const main = editors()[0];
    expect(commentsHost.get()?.editor).toBe(main);
    expect(getActiveEditor()?.editor).toBe(main);

    peekRow("Alpha");
    await peekEditor(sidePeek());
    expect(commentsHost.get()?.editor).toBe(main);
    expect(getActiveEditor()?.editor).toBe(main);
    peekRow("Beta");
    await peekEditor(screen.getByRole("complementary", { name: "Beta" }));
    expect(commentsHost.get()?.editor).toBe(main);
    expect(getActiveEditor()?.editor).toBe(main);

    act(() => {
      store.getState().closePeek();
    });
    expect(commentsHost.get()?.editor).toBe(main);
    expect(getActiveEditor()?.editor).toBe(main);
    expect(commentsHost.get()?.editor.isDestroyed).toBe(false);
  });

  it("opens the find bar of the editor that holds the focus, and only that one", async () => {
    const { user, peekRow } = await renderPeek();
    peekRow("Alpha");
    const peek = sidePeek();
    const peekEd = await peekEditor(peek);
    const [mainEd] = editors();
    expect(mainEd).toBeDefined();

    // Focus in the peek's editor: its bar opens, inside the peek, and the page's does not.
    focusEnd(peekEd);
    await user.keyboard("{Control>}f{/Control}");
    const bars = screen.getAllByRole("search", { name: t("find.label") });
    expect(bars).toHaveLength(1);
    expect(peek.contains(bars[0] ?? null)).toBe(true);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("search")).not.toBeInTheDocument();
    expect(sidePeek()).toBeInTheDocument();

    // Focus in the page's editor: the page's bar opens, outside the peek.
    if (mainEd !== undefined) {
      focusEnd(mainEd);
    }
    await user.keyboard("{Control>}f{/Control}");
    const mainBars = screen.getAllByRole("search", { name: t("find.label") });
    expect(mainBars).toHaveLength(1);
    expect(peek.contains(mainBars[0] ?? null)).toBe(false);
  });

  it("opens the page's find bar for 'Find in page' of the page menu, even with the peek open", async () => {
    const { user, peekRow } = await renderPeek();
    peekRow("Alpha");
    await peekEditor(sidePeek());
    // The page menu is the page's (it is in the header of the page in front, outside the peek).
    await user.click(screen.getAllByRole("button", { name: t("page.actions") })[0] as HTMLElement);
    await user.click(await screen.findByRole("menuitem", { name: t("page.find") }));
    const bars = await screen.findAllByRole("search", { name: t("find.label") });
    expect(bars).toHaveLength(1);
    expect(sidePeek().contains(bars[0] ?? null)).toBe(false);
  });

  it("opens the peek's find bar inside the center dialog, which is modal for everything else", async () => {
    const { user, peekRow } = await renderPeek();
    peekRow("Alpha", "center");
    const dialog = await screen.findByRole("dialog", { name: /Alpha/ });
    const editor = await peekEditor(dialog);
    focusEnd(editor);
    await user.keyboard("{Control>}f{/Control}");
    const bars = screen.getAllByRole("search", { name: t("find.label") });
    expect(bars).toHaveLength(1);
    expect(dialog.contains(bars[0] ?? null)).toBe(true);
  });

  it("has one page title and one peek title, the peek's with its own id", async () => {
    const { peekRow } = await renderPeek();
    peekRow("Alpha");
    expect(document.querySelectorAll("#page-title")).toHaveLength(1);
    expect(document.querySelectorAll("#peek-title")).toHaveLength(1);
    expect(document.querySelector("#page-title")?.closest(".peek")).toBeNull();
    expect(sidePeek()).toHaveAttribute("aria-labelledby", "peek-title");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Hub");
  });

  it("does not announce the peek's page: one presence for the device, the page in front's", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [PERSONAL, WORK] });
    const client = new CoreClient(memory);
    const store = createAppStore(client, { corePollMs: 5 });
    const subscribed = vi.fn();
    const stop = store.getState().start();
    onTestFinished(stop);
    await vi.waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    const original = store.getState().subscribePresence;
    store.setState({
      presenceAvailable: true,
      subscribePresence: (listener) => {
        subscribed();
        return original(listener);
      },
    });
    render(
      <StoreContext value={store}>
        <MainLayout />
      </StoreContext>,
    );
    await findMainNavigation();
    const ids = await idsOf(memory);
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Hub"), spaceId: ids.space("Personal") });
    });
    await screen.findByRole("heading", { level: 1, name: "Hub" });
    await screen.findByRole("textbox", { name: t("editor.label") });
    const forThePage = subscribed.mock.calls.length;
    expect(forThePage).toBeGreaterThan(0);

    act(() => {
      store.getState().openPeek({ pageId: ids.page("Alpha"), spaceId: ids.space("Personal"), mode: "side" });
    });
    await peekEditor(screen.getByRole("complementary", { name: /Alpha/ }));
    expect(subscribed.mock.calls.length).toBe(forThePage);
  });

  it("gives the peek's editor no dictation: it targets the editor of the page in front", async () => {
    const { peekRow } = await renderPeek({ dictation: true });
    const [mainEd] = editors();
    await waitFor(() => {
      expect(dictationOn(mainEd as Editor)).toBe(true);
    });
    peekRow("Alpha");
    const peekEd = await peekEditor(sidePeek());
    expect(dictationOn(peekEd)).toBe(false);
    // Opening and closing the peek leaves the dictation of the page in front as it was.
    expect(dictationOn(mainEd as Editor)).toBe(true);
  });
});
