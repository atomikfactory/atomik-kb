// "Move to…" of a page between spaces (ADR-0038): the entries of the row menu and the page menu (hidden with one
// space), the dialog's three steps (the other spaces, the target's live tree as a picker, the confirmation from the
// plan), what the move asks of the core and what the sidebar does when it is done, and the store following
// `PagesMovedSpace` (here or from another device): tabs, recent pages, selection and both spaces' trees.
//
// The core answers the two requests with canned summaries (`intercept`): what the move does to the pages is the core's
// business and is tested there; these tests are about what the UI asks and shows.

import { CoreClient, CoreError } from "@atomik-kb/core-client";
import type { MemoryTransport, PageId, Request, Response, SeedSpace, SpaceId, TreeView } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t, tp } from "./i18n";
import { COLLAPSED_KEY_PREFIX } from "./store-structure";
import type { SpaceMoveSummary } from "./store-structure";
import { renderInterop } from "./test/interop";
import { idsOf } from "./test/page";
import type { User } from "./test/render";
import { PERSONAL, WORK, outline, renderSidebar, row } from "./test/sidebar";

const ARCHIVE: SeedSpace = {
  name: "Archive",
  icon: "🗄️",
  pages: [
    { title: "2024", children: [{ title: "Q1 notes" }, { title: "Q2 notes", children: [{ title: "Deep" }] }] },
    { title: "Old ideas" },
  ],
};

const SPACES = [PERSONAL, WORK, ARCHIVE];

function summary(overrides: Partial<SpaceMoveSummary> = {}): SpaceMoveSummary {
  return {
    pageIds: ["p1"],
    attachmentsMoved: 0,
    attachmentsLeft: 0,
    propertiesCreated: 0,
    valuesDropped: 0,
    relationLinksRemoved: 0,
    ...overrides,
  };
}

type Answer = SpaceMoveSummary | CoreError | Promise<SpaceMoveSummary>;

/** What the core says to the plan and to the move; a test changes it between steps. */
interface Answers {
  plan: Answer;
  move: Answer;
  /** When set, the tree of that space is read as this (a space whose pages left): a test sets it before the event. */
  tree?: TreeView;
}

function reply(answer: Answer, type: "SpaceMovePlanned" | "PagesMovedToSpace"): Response | Promise<Response> {
  if (answer instanceof CoreError) {
    throw answer;
  }
  return answer instanceof Promise ? answer.then((data) => ({ type, data })) : { type, data: answer };
}

function canned(answers: Answers) {
  return (request: Request): Response | Promise<Response> | undefined => {
    if (request.type === "PageMoveToSpacePlan") {
      return reply(answers.plan, "SpaceMovePlanned");
    }
    if (request.type === "PageMoveToSpace") {
      return reply(answers.move, "PagesMovedToSpace");
    }
    if (request.type === "TreeGet" && answers.tree?.spaceId === request.data.spaceId) {
      return { type: "Tree", data: answers.tree };
    }
    return undefined;
  };
}

/** The app over Personal, Work and Archive, with the core's answers to the move under the test's control. */
async function setup(spaces: readonly SeedSpace[] = SPACES, answers: Answers = { plan: summary(), move: summary() }) {
  const app = await renderInterop({ spaces: [...spaces] }, { intercept: canned(answers) });
  const ids = await idsOf(app.memory);
  return { ...app, ids, answers };
}

function requestsOf<T extends Request["type"]>(requests: readonly Request[], type: T): Extract<Request, { type: T }>[] {
  return requests.filter((request): request is Extract<Request, { type: T }> => request.type === type);
}

async function openRowMenu(user: User, title: string): Promise<HTMLElement> {
  await user.pointer({ keys: "[MouseRight]", target: row(title) });
  return screen.findByRole("menu", { name: t("tree.actions") });
}

/** Opens the dialog from the row menu of a page of the sidebar. */
async function openMoveDialog(user: User, title: string): Promise<HTMLElement> {
  const menu = await openRowMenu(user, title);
  await user.click(within(menu).getByRole("menuitem", { name: t("moveToSpace.menu") }));
  return screen.findByRole("dialog", { name: t("moveToSpace.title", { name: title }) });
}

/** Presses Next once the picker has the tree (it is read again whenever the step is entered). */
async function pressNext(user: User, dialog: HTMLElement): Promise<void> {
  const next = within(dialog).getByRole("button", { name: t("moveToSpace.next") });
  await waitFor(() => {
    expect(next).toBeEnabled();
  });
  await user.click(next);
}

async function chooseSpace(user: User, dialog: HTMLElement, name: string): Promise<void> {
  await user.click(within(dialog).getByRole("button", { name: new RegExp(`^${name}`) }));
  await within(dialog).findByRole("radio", { name: t("moveToSpace.topLevel") });
}

/** The places the picker offers, in order: the title of each, with its depth. */
function places(dialog: HTMLElement): string[] {
  return within(dialog)
    .getAllByRole("radio")
    .map((radio) => {
      const holder = radio.closest(".move-option");
      const label = (radio as HTMLInputElement).labels?.[0]?.textContent ?? "";
      return `${"  ".repeat(Number(holder?.getAttribute("data-depth") ?? "0"))}${label}`;
    });
}

describe("the entries", () => {
  it("offers Move to… in the row menu of a page", async () => {
    const { user } = await renderSidebar();
    const menu = await openRowMenu(user, "Journal");
    expect(within(menu).getByRole("menuitem", { name: t("moveToSpace.menu") })).toBeEnabled();
  });

  it("does not offer Move to… in the row menu with one space", async () => {
    const { user } = await renderSidebar({ spaces: [PERSONAL] });
    const menu = await openRowMenu(user, "Journal");
    expect(within(menu).queryByRole("menuitem", { name: t("moveToSpace.menu") })).not.toBeInTheDocument();
  });

  it("offers Move to… in the page menu of the open page", async () => {
    const { user } = await renderSidebar();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    expect(await screen.findByRole("menuitem", { name: t("moveToSpace.menu") })).toBeInTheDocument();
  });

  it("does not offer Move to… in the page menu with one space", async () => {
    const { user } = await renderSidebar({ spaces: [PERSONAL] });
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await screen.findByRole("menu", { name: t("page.actions") });
    expect(screen.queryByRole("menuitem", { name: t("moveToSpace.menu") })).not.toBeInTheDocument();
  });

  it("appears once a second space exists, and goes when it is deleted", async () => {
    const { user, memory, store } = await renderInterop({ spaces: [PERSONAL] });
    const client = new CoreClient(memory);
    const first = await openRowMenu(user, "Journal");
    expect(within(first).queryByRole("menuitem", { name: t("moveToSpace.menu") })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");

    const work = await client.spaceCreate("Work");
    await waitFor(() => {
      expect(store.getState().spaces).toHaveLength(2);
    });
    // The new space is current: go back to the first one to see its pages.
    store.getState().selectSpace(store.getState().spaces[0]?.id ?? "");
    await waitFor(() => {
      expect(outline()).toContain("Journal");
    });
    const second = await openRowMenu(user, "Journal");
    expect(within(second).getByRole("menuitem", { name: t("moveToSpace.menu") })).toBeInTheDocument();
    await user.keyboard("{Escape}");

    await client.spaceDelete(work.id);
    await waitFor(() => {
      expect(store.getState().spaces).toHaveLength(1);
    });
    const third = await openRowMenu(user, "Journal");
    expect(within(third).queryByRole("menuitem", { name: t("moveToSpace.menu") })).not.toBeInTheDocument();
  });
});

describe("step 1: the other spaces", () => {
  it("lists the other spaces with their icon and number of pages, and not the one the page is in", async () => {
    const { user } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    const list = within(dialog).getByRole("list", { name: t("moveToSpace.spacesLabel") });
    const items = within(list).getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual([
      `Work${tp("spaces.pageCount", 1)}`,
      `🗄️Archive${tp("spaces.pageCount", 5)}`,
    ]);
    expect(within(list).queryByText("Personal")).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("moveToSpace.step.space"))).toBeInTheDocument();
    // The dialog is named for the page, and described by what it asks.
    expect(dialog).toHaveAccessibleDescription(t("moveToSpace.step.space"));
  });

  it("names an untitled page Untitled", async () => {
    const { user } = await setup([{ name: "Personal", pages: [{ title: "" }] }, WORK]);
    const menu = await openRowMenu(user, t("page.untitled"));
    await user.click(within(menu).getByRole("menuitem", { name: t("moveToSpace.menu") }));
    expect(await screen.findByRole("dialog", { name: t("moveToSpace.title", { name: t("page.untitled") }) })).toBeInTheDocument();
  });

  it("closes with Cancel and with Escape, sending nothing, and gives focus back to the row", async () => {
    const { user, requests } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(row("Journal")).toHaveFocus();

    await openMoveDialog(user, "Journal");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(requestsOf(requests, "PageMoveToSpacePlan")).toHaveLength(0);
    expect(requestsOf(requests, "PageMoveToSpace")).toHaveLength(0);
  });

  it("says so when there is no other space any more", async () => {
    const { user, memory } = await setup([PERSONAL, { name: "Empty", pages: [] }]);
    const dialog = await openMoveDialog(user, "Journal");
    const empty = new CoreClient(memory);
    const spaces = await empty.spaceList();
    await empty.spaceDelete(spaces.find((space) => space.name === "Empty")?.id ?? "");
    expect(await within(dialog).findByText(t("moveToSpace.noSpaces"))).toBeInTheDocument();
  });
});

describe("step 2: where it goes", () => {
  it("offers the top level first, chosen, then the live tree of the space in order", async () => {
    const { user } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    expect(within(dialog).getByText(t("moveToSpace.step.place", { space: "Archive" }))).toBeInTheDocument();
    expect(places(dialog)).toEqual([t("moveToSpace.topLevel"), "2024", "  Q1 notes", "  Q2 notes", "    Deep", "Old ideas"]);
    expect(within(dialog).getByRole("radio", { name: t("moveToSpace.topLevel") })).toBeChecked();
    // The filter box has focus, and Next is there.
    expect(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") })).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.next") })).toBeEnabled();
  });

  it("does not offer the pages in the trash", async () => {
    const { user, memory, ids } = await setup();
    await new CoreClient(memory).pageTrash(ids.page("Old ideas"));
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    expect(places(dialog)).toEqual([t("moveToSpace.topLevel"), "2024", "  Q1 notes", "  Q2 notes", "    Deep"]);
  });

  it("filters the pages by title, wherever they are, with where each one is", async () => {
    const { user } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    await user.type(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") }), "NOTES");
    expect(places(dialog)).toEqual([t("moveToSpace.topLevel"), "Q1 notes", "Q2 notes"]);
    expect(within(dialog).getByRole("radio", { name: "Q2 notes" })).toHaveAccessibleDescription("2024");

    await user.clear(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") }));
    await user.type(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") }), "deep");
    expect(places(dialog)).toEqual([t("moveToSpace.topLevel"), "Deep"]);
    expect(within(dialog).getByRole("radio", { name: "Deep" })).toHaveAccessibleDescription("2024 / Q2 notes");

    await user.clear(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") }));
    await user.type(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") }), "zzz");
    expect(places(dialog)).toEqual([t("moveToSpace.topLevel")]);
    expect(within(dialog).getByText(t("moveToSpace.noMatches"))).toBeInTheDocument();
  });

  it("does not go on when Enter is pressed in the filter box", async () => {
    const { user } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    await user.type(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") }), "deep{Enter}");
    expect(within(dialog).getByRole("textbox", { name: t("moveToSpace.filterLabel") })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("moveToSpace.move") })).not.toBeInTheDocument();
  });

  it("goes back to the spaces and on to another one", async () => {
    const { user } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    await user.click(within(dialog).getByRole("radio", { name: "Old ideas" }));
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    expect(within(dialog).getByText(t("moveToSpace.step.space"))).toBeInTheDocument();
    await chooseSpace(user, dialog, "Work");
    expect(places(dialog)).toEqual([t("moveToSpace.topLevel"), "Meetings"]);
    // A choice made in the other space does not carry over.
    expect(within(dialog).getByRole("radio", { name: t("moveToSpace.topLevel") })).toBeChecked();
  });

  it("reads the tree again every time the step is entered", async () => {
    const { user, memory, ids } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Work");
    await pressNext(user, dialog);
    await within(dialog).findByText(t("moveToSpace.where.top"));

    // A page made meanwhile (here or on another device) is there when the person comes back.
    await new CoreClient(memory).pageCreate({ spaceId: ids.space("Work"), parentId: null, title: "Fresh" });
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    await waitFor(() => {
      expect(places(dialog)).toEqual([t("moveToSpace.topLevel"), "Meetings", "Fresh"]);
    });
  });

  it("forgets a parent that was deleted meanwhile and offers the top level", async () => {
    const { user, memory, ids } = await setup();
    const client = new CoreClient(memory);
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    await user.click(within(dialog).getByRole("radio", { name: "Old ideas" }));
    await pressNext(user, dialog);
    await within(dialog).findByText(t("moveToSpace.where.under", { parent: "Old ideas" }));
    await client.pageTrash(ids.page("Old ideas"));
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    await waitFor(() => {
      expect(within(dialog).getByRole("radio", { name: t("moveToSpace.topLevel") })).toBeChecked();
    });
    expect(within(dialog).queryByRole("radio", { name: "Old ideas" })).not.toBeInTheDocument();
  });

  it("shows why the pages of the space could not be read, and does not go on", async () => {
    const { user, memory } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    memory.failNext({ code: "Storage", message: "disk I/O error" });
    await user.click(within(dialog).getByRole("button", { name: /^Archive/ }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      t("moveToSpace.treeError", { space: "Archive", message: "disk I/O error" }),
    );
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.next") })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    expect(within(dialog).getByText(t("moveToSpace.step.space"))).toBeInTheDocument();
  });
});

describe("step 3: the confirmation", () => {
  async function toConfirmation(user: User, dialog: HTMLElement, space = "Work"): Promise<void> {
    await chooseSpace(user, dialog, space);
    await pressNext(user, dialog);
  }

  it("asks the core what the move does, with the page, the space and no parent for the top level", async () => {
    const { user, requests, ids } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await toConfirmation(user, dialog);
    await within(dialog).findByText(t("moveToSpace.where.top"));
    const plans = requestsOf(requests, "PageMoveToSpacePlan");
    expect(plans).toHaveLength(1);
    expect(plans[0]?.data).toEqual({ pageId: ids.page("Journal"), spaceId: ids.space("Work") });
  });

  it("names the parent when one is chosen, and sends it", async () => {
    const { user, requests, ids } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    await user.click(within(dialog).getByRole("radio", { name: "Q2 notes" }));
    await pressNext(user, dialog);
    expect(await within(dialog).findByText(t("moveToSpace.where.under", { parent: "Q2 notes" }))).toBeInTheDocument();
    expect(requestsOf(requests, "PageMoveToSpacePlan")[0]?.data).toEqual({
      pageId: ids.page("Journal"),
      spaceId: ids.space("Archive"),
      parentId: ids.page("Q2 notes"),
    });
  });

  it("says how many pages move to which space, in the singular and the plural, with no other line when all are zero", async () => {
    const answers: Answers = { plan: summary({ pageIds: ["a"] }), move: summary() };
    const { user } = await setup(SPACES, answers);
    const dialog = await openMoveDialog(user, "Journal");
    await toConfirmation(user, dialog);
    expect(await within(dialog).findByText(tp("moveToSpace.confirm", 1, { space: "Work" }))).toBeInTheDocument();
    expect(within(dialog).queryByRole("list")).not.toBeInTheDocument();
    expect(dialog).toHaveAccessibleDescription(tp("moveToSpace.confirm", 1, { space: "Work" }));

    answers.plan = summary({ pageIds: ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"] });
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    await pressNext(user, dialog);
    expect(await within(dialog).findByText(tp("moveToSpace.confirm", 12, { space: "Work" }))).toBeInTheDocument();
    expect(tp("moveToSpace.confirm", 12, { space: "Work" })).toBe("Move 12 pages to “Work”");
  });

  it("lists only what is not zero: properties created, values that cannot follow, relation links, attachments that stay", async () => {
    const answers: Answers = {
      plan: summary({ pageIds: ["a", "b"], propertiesCreated: 3, valuesDropped: 1, relationLinksRemoved: 0, attachmentsLeft: 2, attachmentsMoved: 4 }),
      move: summary(),
    };
    const { user } = await setup(SPACES, answers);
    const dialog = await openMoveDialog(user, "Journal");
    await toConfirmation(user, dialog);
    const list = await within(dialog).findByRole("list");
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      tp("moveToSpace.properties", 3, { space: "Work" }),
      tp("moveToSpace.values", 1),
      tp("moveToSpace.attachments", 2, { space: "Personal" }),
    ]);

    answers.plan = summary({ propertiesCreated: 1, relationLinksRemoved: 5, attachmentsLeft: 1 });
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    await pressNext(user, dialog);
    await waitFor(() => {
      expect(within(dialog).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
        tp("moveToSpace.properties", 1, { space: "Work" }),
        tp("moveToSpace.relations", 5),
        tp("moveToSpace.attachments", 1, { space: "Personal" }),
      ]);
    });
  });

  it("waits for the plan before Move can be pressed", async () => {
    let release: (value: SpaceMoveSummary) => void = () => undefined;
    const answers: Answers = {
      plan: new Promise<SpaceMoveSummary>((resolve) => {
        release = resolve;
      }),
      move: summary(),
    };
    const { user } = await setup(SPACES, answers);
    const dialog = await openMoveDialog(user, "Journal");
    await toConfirmation(user, dialog);
    expect(await within(dialog).findByRole("status")).toHaveTextContent(t("moveToSpace.checking"));
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.move") })).toBeDisabled();
    await act(async () => {
      release(summary({ pageIds: ["a", "b"] }));
      await Promise.resolve();
    });
    expect(await within(dialog).findByText(tp("moveToSpace.confirm", 2, { space: "Work" }))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.move") })).toBeEnabled();
  });

  it("shows why the move cannot be done, keeps Move off, and lets the person go back", async () => {
    const answers: Answers = {
      plan: new CoreError({ code: "InvalidInput", message: "the page is in the trash", details: { field: "pageId" } }),
      move: summary(),
    };
    const { user } = await setup(SPACES, answers);
    const dialog = await openMoveDialog(user, "Journal");
    await toConfirmation(user, dialog);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      t("moveToSpace.planError", { message: "the page is in the trash" }),
    );
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.move") })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    expect(within(dialog).getByRole("radio", { name: t("moveToSpace.topLevel") })).toBeInTheDocument();
  });

  it("asks again, for the new place, when the person goes back and picks another parent", async () => {
    const { user, requests, ids } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await chooseSpace(user, dialog, "Archive");
    await pressNext(user, dialog);
    await within(dialog).findByText(t("moveToSpace.where.top"));
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.back") }));
    await user.click(await within(dialog).findByRole("radio", { name: "2024" }));
    await pressNext(user, dialog);
    await within(dialog).findByText(t("moveToSpace.where.under", { parent: "2024" }));
    const plans = requestsOf(requests, "PageMoveToSpacePlan").map((request) => request.data.parentId);
    expect(plans).toEqual([undefined, ids.page("2024")]);
  });
});

describe("moving", () => {
  async function toMove(user: User, dialog: HTMLElement, space: string, parent?: string): Promise<void> {
    await chooseSpace(user, dialog, space);
    if (parent !== undefined) {
      await user.click(within(dialog).getByRole("radio", { name: parent }));
    }
    await pressNext(user, dialog);
    await within(dialog).findByText(/^Move \d+ pages? to/);
  }

  it("moves the page last in the top level of the space, then shows it there", async () => {
    const { user, store, requests, ids } = await setup();
    const dialog = await openMoveDialog(user, "Journal");
    await toMove(user, dialog, "Work");
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    const moves = requestsOf(requests, "PageMoveToSpace");
    expect(moves).toHaveLength(1);
    // No parent for the top level and no index: last.
    expect(moves[0]?.data).toEqual({ pageId: ids.page("Journal"), spaceId: ids.space("Work") });
    // The sidebar is in the space the page went to, with the page selected.
    expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    expect(store.getState().selectedPageId).toBe(ids.page("Journal"));
    expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent("Work");
  });

  it("sends the parent chosen, and expands it and its parents in the target's tree", async () => {
    const { user, store, memory, requests, ids } = await setup();
    const client = new CoreClient(memory);
    // 2024 and Q2 notes are collapsed in Archive.
    await client.settingsSet(`${COLLAPSED_KEY_PREFIX}.${ids.space("Archive")}`, [ids.page("2024"), ids.page("Q2 notes"), ids.page("Old ideas")]);
    const dialog = await openMoveDialog(user, "Journal");
    await toMove(user, dialog, "Archive", "Deep");
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Archive"));
    });
    expect(requestsOf(requests, "PageMoveToSpace")[0]?.data).toEqual({
      pageId: ids.page("Journal"),
      spaceId: ids.space("Archive"),
      parentId: ids.page("Deep"),
    });
    // The chain down to the parent is open; a page that is collapsed for another reason stays so.
    await waitFor(() => {
      expect(outline()).toEqual(["2024", "  Q1 notes", "  Q2 notes", "    Deep", "Old ideas"]);
    });
    expect(await client.settingsGet(`${COLLAPSED_KEY_PREFIX}.${ids.space("Archive")}`)).toEqual([ids.page("Old ideas")]);
  });

  it("is busy while the core works: the buttons are off and the dialog does not close on Escape", async () => {
    let finish: (value: SpaceMoveSummary) => void = () => undefined;
    const answers: Answers = {
      plan: summary(),
      move: new Promise<SpaceMoveSummary>((resolve) => {
        finish = resolve;
      }),
    };
    const { user } = await setup(SPACES, answers);
    const dialog = await openMoveDialog(user, "Journal");
    await toMove(user, dialog, "Work");
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    const moving = await within(dialog).findByRole("button", { name: t("moveToSpace.moving") });
    expect(moving).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("dialog.cancel") })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.back") })).toBeDisabled();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBe(dialog);
    await act(async () => {
      finish(summary());
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });

  it("shows why the core refused in the dialog, stays where it is, and can be tried again", async () => {
    const answers: Answers = {
      plan: summary(),
      move: new CoreError({ code: "InvalidInput", message: "that space has too many pages", details: { field: "pageId" } }),
    };
    const { user, store, ids } = await setup(SPACES, answers);
    const dialog = await openMoveDialog(user, "Journal");
    await toMove(user, dialog, "Work");
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      t("moveToSpace.error", { message: "that space has too many pages" }),
    );
    expect(screen.getByRole("dialog")).toBe(dialog);
    expect(within(dialog).getByRole("button", { name: t("moveToSpace.move") })).toBeEnabled();
    // Nothing happened to the sidebar.
    expect(store.getState().currentSpaceId).toBe(ids.space("Personal"));

    answers.move = summary();
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
  });

  it("moves the open page from its page menu", async () => {
    const { user, store, requests, ids } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("moveToSpace.menu") }));
    const dialog = await screen.findByRole("dialog", { name: t("moveToSpace.title", { name: "Journal" }) });
    await toMove(user, dialog, "Archive", "Old ideas");
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Archive"));
    });
    expect(requestsOf(requests, "PageMoveToSpace")[0]?.data).toEqual({
      pageId: ids.page("Journal"),
      spaceId: ids.space("Archive"),
      parentId: ids.page("Old ideas"),
    });
    expect(store.getState().selectedPageId).toBe(ids.page("Journal"));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });
});

describe("the store follows PagesMovedSpace", () => {
  function moved(from: SpaceId, to: SpaceId, ...pageIds: PageId[]) {
    return { type: "PagesMovedSpace", data: { fromSpaceId: from, toSpaceId: to, pageIds } } as const;
  }

  async function settingOf(memory: MemoryTransport, key: string): Promise<unknown> {
    return new CoreClient(memory).settingsGet(key);
  }

  it("takes the main area into the new space when the open page moved, and keeps the page open", async () => {
    const { user, store, emit, ids, memory } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    const tabsBefore = store.getState().tabs.length;

    act(() => {
      emit(moved(ids.space("Personal"), ids.space("Work"), ids.page("Journal")));
    });

    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    });
    expect(store.getState().selectedPageId).toBe(ids.page("Journal"));
    expect(store.getState().tabs).toHaveLength(tabsBefore);
    const active = store.getState().tabs.find((tab) => tab.id === store.getState().activeTabId);
    expect(active?.entries.at(-1)).toMatchObject({ spaceId: ids.space("Work"), pageId: ids.page("Journal") });
    // The stored tabs say the same.
    await waitFor(async () => {
      expect(JSON.stringify(await settingOf(memory, "ui.tabs"))).toContain(`"spaceId":"${ids.space("Work")}","pageId":"${ids.page("Journal")}"`);
    });
  });

  it("moves the entries of tabs that are not in front, and leaves the front one where it is", async () => {
    const { user, store, emit, ids } = await setup();
    await user.click(row("Journal"));
    // Another tab on Reading list, opened in the background; Journal stays in front.
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Reading list"), spaceId: ids.space("Personal"), disposition: "background" });
    });
    await waitFor(() => {
      expect(store.getState().tabs).toHaveLength(2);
    });

    act(() => {
      emit(moved(ids.space("Personal"), ids.space("Work"), ids.page("Reading list")));
    });

    await waitFor(() => {
      const entries = store.getState().tabs.map((tab) => tab.entries.at(-1));
      expect(entries.map((entry) => [entry?.pageId, entry?.spaceId])).toEqual([
        [ids.page("Journal"), ids.space("Personal")],
        [ids.page("Reading list"), ids.space("Work")],
      ]);
    });
    expect(store.getState().currentSpaceId).toBe(ids.space("Personal"));
    // The page left the old space's tree, and the tab that shows it is still there.
    expect(store.getState().tabs).toHaveLength(2);
  });

  it("keeps the tab of a page that moved when the tree of the old space no longer lists it", async () => {
    const { user, store, emit, ids, memory, answers } = await setup();
    await user.click(row("Journal"));
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Reading list"), spaceId: ids.space("Personal"), disposition: "background" });
    });
    await waitFor(() => {
      expect(store.getState().tabs).toHaveLength(2);
    });
    // From now on the core reads the old space without the page, as it does once the page left it.
    const before = await new CoreClient(memory).treeGet(ids.space("Personal"));
    answers.tree = { ...before, roots: before.roots.filter((node) => node.page.id !== ids.page("Reading list")) };

    act(() => {
      emit(moved(ids.space("Personal"), ids.space("Work"), ids.page("Reading list")));
    });
    await waitFor(() => {
      expect(outline()).not.toContain("Reading list");
    });
    // A page that left a tree is gone, unless it moved: the tab is still there, now in the new space.
    const entries = store.getState().tabs.map((tab) => tab.entries.at(-1));
    expect(entries.map((entry) => [entry?.pageId, entry?.spaceId])).toEqual([
      [ids.page("Journal"), ids.space("Personal")],
      [ids.page("Reading list"), ids.space("Work")],
    ]);
  });

  it("gives the recent pages that moved the new space, and remembers it", async () => {
    const { user, store, emit, ids, memory } = await setup();
    await user.click(row("Journal"));
    await user.click(row("Reading list"));
    await waitFor(async () => {
      expect(JSON.stringify(await settingOf(memory, "ui.recent_pages"))).toContain(ids.page("Journal"));
    });

    act(() => {
      emit(moved(ids.space("Personal"), ids.space("Work"), ids.page("Journal")));
    });

    await waitFor(() => {
      const recent = store.getState().settings["ui.recent_pages"];
      expect(recent).toEqual([
        expect.objectContaining({ pageId: ids.page("Reading list"), spaceId: ids.space("Personal") }),
        expect.objectContaining({ pageId: ids.page("Journal"), spaceId: ids.space("Work") }),
      ]);
    });
    await waitFor(async () => {
      expect(await settingOf(memory, "ui.recent_pages")).toEqual(store.getState().settings["ui.recent_pages"]);
    });
  });

  it("reads both spaces again: the list of spaces, and the tree of the one that is open", async () => {
    const { user, emit, ids, requests } = await setup();
    await user.click(row("Journal"));
    const listsBefore = requestsOf(requests, "SpaceList").length;
    const treesBefore = requestsOf(requests, "TreeGet").length;

    // Another page of the open space moved away: the open space's tree is read again.
    act(() => {
      emit(moved(ids.space("Personal"), ids.space("Archive"), ids.page("Reading list")));
    });
    await waitFor(() => {
      expect(requestsOf(requests, "SpaceList").length).toBeGreaterThan(listsBefore);
      expect(requestsOf(requests, "TreeGet").length).toBeGreaterThan(treesBefore);
    });
    expect(requestsOf(requests, "TreeGet").slice(treesBefore).every((request) => request.data.spaceId === ids.space("Personal"))).toBe(true);
  });

  it("leaves everything alone for a move between other spaces", async () => {
    const { user, store, emit, ids, requests } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    const treesBefore = requestsOf(requests, "TreeGet").length;
    const tabsBefore = store.getState().tabs;

    act(() => {
      emit(moved(ids.space("Work"), ids.space("Archive"), ids.page("Meetings")));
    });
    await waitFor(() => {
      expect(requestsOf(requests, "SpaceList").length).toBeGreaterThan(1);
    });
    expect(store.getState().tabs).toBe(tabsBefore);
    expect(requestsOf(requests, "TreeGet")).toHaveLength(treesBefore);
    expect(store.getState().currentSpaceId).toBe(ids.space("Personal"));
  });
});
