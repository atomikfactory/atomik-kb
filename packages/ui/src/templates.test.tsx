// Template pages in the UI (ADR-0056, `docs/architecture/templates.md` section 6): turning a page into a template and back (the
// sidebar row menu, the page menu, the banner), the banner and "Use template", the placeholders of the properties panel, relative
// mentions in a template, the Templates panel, a page made from a template (the button beside New page, the row menu, `/template`),
// and the places that leave templates out (the sidebar, recents, Home, Ctrl+K, the pickers that list from the tree).
//
// It runs over the in-memory core with the request log of `test/templates.tsx`. The starters are the starters module's (another
// part of the batch): the module is replaced here by two starters that make pages through the client, so that what the UI asks of
// `instantiateStarter` and what it does with the answer are tested without the real files.

import { CoreError, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { CoreClient, PageInfo, PageNode, SeedSpace, TreeView } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { RECENT_PAGES_KEY, parseRecentPages } from "./store-layout";
import { templateGroups, templateRootOf, toFullTarget, withoutTemplates } from "./template-tree";
import { findEditor } from "./test/page";
import type { User } from "./test/render";
import { renderSharing } from "./test/sharing";
import { outline, row, sidebarButton } from "./test/sidebar";
import { OTHER_SPACE, TEMPLATE_SPACE, pageIdOf, pagesByTitle, renderTemplateApp } from "./test/templates";
import { isInTemplate, templatePageIds, templatesOfSpace } from "./templates";

const starters = vi.hoisted(() => ({ instantiate: vi.fn() }));

vi.mock("./starter-templates", () => ({
  STARTER_TEMPLATES: [
    {
      id: "meeting-notes",
      icon: "📝",
      title: (language: string) => `Starter meeting notes (${language})`,
      description: () => "Agenda, notes and actions",
    },
    { id: "daily-journal", icon: "📅", title: () => "Starter journal", description: () => "A page a day" },
  ],
  instantiateStarter: starters.instantiate,
}));

beforeEach(() => {
  starters.instantiate.mockReset();
  // A starter makes a page (a template page with `asTemplate`) through the client, as the real one does through the import session.
  starters.instantiate.mockImplementation(
    async (
      client: CoreClient,
      id: string,
      options: { spaceId: string; parentId?: string; asTemplate?: boolean },
    ): Promise<PageInfo> => {
      const page = await client.pageCreate({
        spaceId: options.spaceId,
        parentId: options.parentId ?? null,
        title: `Made from ${id}`,
      });
      return options.asTemplate === true ? client.pageSetTemplate(page.id, true) : page;
    },
  );
});

// ----- helpers -----------------------------------------------------------------------------------------------

/** Right-clicks a row of the sidebar and chooses an item of its menu. */
async function chooseFromRow(user: User, title: string, item: string): Promise<void> {
  await user.pointer({ keys: "[MouseRight]", target: row(title) });
  const menu = await screen.findByRole("menu", { name: t("tree.actions") });
  await user.click(within(menu).getByRole("menuitem", { name: item }));
}

/** Opens the page menu of the page in front. */
async function openPageMenu(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  return screen.findByRole("menu", { name: t("page.actions") });
}

/** Opens a page by id (a template is not in the sidebar) and waits for its title. */
async function openByTitle(
  store: Awaited<ReturnType<typeof renderTemplateApp>>["store"],
  memory: Awaited<ReturnType<typeof renderTemplateApp>>["memory"],
  title: string,
): Promise<string> {
  const id = await pageIdOf(memory, title);
  act(() => {
    store.getState().selectPage(id);
  });
  await screen.findByRole("heading", { level: 1, name: title });
  return id;
}

/** The banner above the title of a template page. */
function banner(): HTMLElement {
  return screen.getByRole("region", { name: t("template.banner.label") });
}

/** The Templates panel, opened from the sidebar. */
async function openPanel(user: User): Promise<HTMLElement> {
  await user.click(sidebarButton(t("template.panel.open")));
  return screen.findByRole("dialog", { name: t("template.panel.title") });
}

/** The line of the panel (or of the picker) that holds `title`. */
function lineOf(container: HTMLElement, title: string): HTMLElement {
  const line = within(container).getByText(title).closest("li");
  if (line === null) {
    throw new Error(`no line for ${title}`);
  }
  return line;
}

/** The titles of the top-level pages of the core, in order, templates included. */
async function topLevel(client: CoreClient): Promise<string[]> {
  const [space] = await client.spaceList();
  return (await client.treeGet(space?.id ?? "")).roots.map((node) => node.page.title);
}

function info(id: string, extra: Partial<PageInfo> = {}): PageInfo {
  return { id, spaceId: "s", title: id, createdAt: 0, updatedAt: 0, ...extra };
}

function node(page: PageInfo, children: PageNode[] = []): PageNode {
  return { page, children };
}

function viewOf(roots: PageNode[]): TreeView {
  return { spaceId: "s", roots, trashed: [], undo: { canUndo: false, canRedo: false } };
}

// ----- the helpers over the tree ---------------------------------------------------------------------------------

describe("the helpers over the tree", () => {
  // a, T (template: t1, t2 inside), b, and c with a template child u and a normal child v.
  const tree = viewOf([
    node(info("a")),
    node(info("T", { template: true }), [node(info("t1", { parentId: "T" }), [node(info("t2", { parentId: "t1" }))])]),
    node(info("b")),
    node(info("c"), [node(info("u", { template: true, parentId: "c" })), node(info("v", { parentId: "c" }))]),
  ]);

  it("finds the pages that are templates or inside one", () => {
    expect([...templatePageIds(tree)].sort()).toEqual(["T", "t1", "t2", "u"]);
    expect(isInTemplate(tree, "t2")).toBe(true);
    expect(isInTemplate(tree, "v")).toBe(false);
    expect(templatesOfSpace(tree, "s").map((page) => page.id)).toEqual(["T", "u"]);
  });

  it("draws the tree without templates and what is inside them, and leaves a tree with none as it is", () => {
    const shown = withoutTemplates(tree.roots);
    expect(shown.map((entry) => entry.page.id)).toEqual(["a", "b", "c"]);
    expect(shown[2]?.children.map((entry) => entry.page.id)).toEqual(["v"]);
    const plain = [node(info("x"), [node(info("y"))])];
    expect(withoutTemplates(plain)).toBe(plain);
  });

  it("names the template a page belongs to: itself, or its outermost ancestor with the flag", () => {
    expect(templateRootOf(tree, "T")?.id).toBe("T");
    expect(templateRootOf(tree, "t2")?.id).toBe("T");
    expect(templateRootOf(tree, "v")).toBeNull();
    expect(templateRootOf(null, "v")).toBeNull();
  });

  it("puts a move computed without the templates back in terms of all the pages", () => {
    // Under c the visible children are [v]; u (a template) comes first, so "before v" is position 1 and "last" is 2.
    expect(toFullTarget(tree.roots, "z", { parentId: "c", index: 0 })).toEqual({ parentId: "c", index: 1 });
    expect(toFullTarget(tree.roots, "z", { parentId: "c", index: 1 })).toEqual({ parentId: "c", index: 2 });
    // At the top level the visible pages are [a, b, c]: before b is position 2 (T sits between), the end is 4.
    expect(toFullTarget(tree.roots, "z", { parentId: null, index: 1 })).toEqual({ parentId: null, index: 2 });
    expect(toFullTarget(tree.roots, "z", { parentId: null, index: 3 })).toEqual({ parentId: null, index: 4 });
    // The page that moves is not among the siblings (b moving: [a, T, c] with [a, c] showing), and a parent with nothing hidden is left alone.
    expect(toFullTarget(tree.roots, "b", { parentId: null, index: 1 })).toEqual({ parentId: null, index: 2 });
    expect(toFullTarget(tree.roots, "b", { parentId: null, index: 2 })).toEqual({ parentId: null, index: 3 });
    expect(toFullTarget(tree.roots, "z", { parentId: "t1", index: 0 })).toEqual({ parentId: "t1", index: 0 });
  });

  it("groups the templates by where they live, the top level first", () => {
    const groups = templateGroups(tree, "s");
    expect(groups.map((group) => [group.parentId, group.title, group.templates.map((page) => page.id)])).toEqual([
      [null, "", ["T"]],
      ["c", "c", ["u"]],
    ]);
    expect(templateGroups(null, "s")).toEqual([]);
  });
});

// ----- hiding templates -----------------------------------------------------------------------------------------

describe("hiding templates", () => {
  it("leaves templates, and the pages inside them, out of the sidebar", async () => {
    await renderTemplateApp();
    expect(outline()).toEqual(["Journal", "Projects", "  Roadmap", "Reading list"]);
  });

  it("shows no expand button on a page whose only sub-page is a template", async () => {
    await renderTemplateApp([{ name: "Personal", pages: [{ title: "Tasks", children: [{ title: "Task form", template: true }] }] }]);
    expect(outline()).toEqual(["Tasks"]);
    expect(within(row("Tasks")).queryByRole("button", { name: t("tree.collapse") })).not.toBeInTheDocument();
  });

  it("moves a page among the visible ones and puts the move back in terms of the whole tree", async () => {
    const space: SeedSpace = { name: "Personal", pages: [{ title: "A" }, { title: "Hidden", template: true }, { title: "B" }, { title: "C" }] };
    const { user, client } = await renderTemplateApp([space]);
    expect(outline()).toEqual(["A", "B", "C"]);
    // A is the first page that shows: nothing to move it up past, although a template stands after it.
    await user.pointer({ keys: "[MouseRight]", target: row("A") });
    let menu = await screen.findByRole("menu", { name: t("tree.actions") });
    expect(within(menu).getByRole("menuitem", { name: t("tree.menu.moveUp") })).toBeDisabled();
    await user.keyboard("{Escape}");

    // B up: before A.
    await chooseFromRow(user, "B", t("tree.menu.moveUp"));
    await waitFor(() => {
      expect(outline()).toEqual(["B", "A", "C"]);
    });
    expect(await topLevel(client)).toEqual(["B", "A", "Hidden", "C"]);

    // A down: after C, which is the end of everything.
    await user.pointer({ keys: "[MouseRight]", target: row("A") });
    menu = await screen.findByRole("menu", { name: t("tree.actions") });
    await user.click(within(menu).getByRole("menuitem", { name: t("tree.menu.moveDown") }));
    await waitFor(() => {
      expect(outline()).toEqual(["B", "C", "A"]);
    });
    expect(await topLevel(client)).toEqual(["B", "Hidden", "C", "A"]);
  });

  it("does not record a template as a recent page, and lists none on Home or in Ctrl+K", async () => {
    const { store, memory } = await renderTemplateApp();
    const journal = await pageIdOf(memory, "Journal");
    const meeting = await pageIdOf(memory, "Meeting notes");
    const brief = await pageIdOf(memory, "Project brief");
    const briefNotes = await pageIdOf(memory, "Brief notes");
    const space = store.getState().currentSpaceId ?? "";
    // Opening a template or a page inside one leaves no entry; a page does.
    act(() => {
      store.getState().selectPage(meeting);
    });
    await screen.findByRole("heading", { level: 1, name: "Meeting notes" });
    act(() => {
      store.getState().selectPage(briefNotes);
    });
    await screen.findByRole("heading", { level: 1, name: "Brief notes" });
    act(() => {
      store.getState().selectPage(journal);
    });
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await waitFor(() => {
      expect(parseRecentPages(store.getState().settings[RECENT_PAGES_KEY]).map((entry) => entry.pageId)).toEqual([journal]);
    });

    // Entries that were stored before the page became a template are left out when they are listed.
    await act(async () => {
      await store.getState().setSetting(RECENT_PAGES_KEY, [
        { pageId: meeting, spaceId: space, openedAt: 3 },
        { pageId: brief, spaceId: space, openedAt: 2 },
        { pageId: journal, spaceId: space, openedAt: 1 },
      ]);
    });
    act(() => {
      store.getState().selectPage(null);
    });
    const home = await screen.findByRole("list", { name: t("recent.visitedList") });
    expect(within(home).getAllByRole("listitem").map((item) => item.textContent)).toEqual([expect.stringContaining("Journal")]);
    expect(within(home).queryByText("Meeting notes")).not.toBeInTheDocument();

    act(() => {
      store.getState().openSearch();
    });
    const results = await screen.findByRole("listbox", { name: t("search.results") });
    await waitFor(() => {
      expect(within(results).getAllByRole("option").map((option) => option.getAttribute("aria-label"))).toEqual(["Journal"]);
    });
  });

  it("keeps a favourite template in the favourites, with a template mark", async () => {
    await renderTemplateApp([
      {
        name: "Personal",
        pages: [{ title: "Journal", favorite: true }, { title: "Weekly review", template: true, favorite: true }],
      },
    ]);
    const list = await screen.findByRole("list", { name: t("favorites.title") });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    const template = lineOf(list, "Weekly review");
    expect(within(template).getByRole("img", { name: t("template.mark") })).toBeInTheDocument();
    expect(within(lineOf(list, "Journal")).queryByRole("img", { name: t("template.mark") })).not.toBeInTheDocument();
  });

  it("leaves templates out of a search limited to a tag", async () => {
    const space: SeedSpace = {
      name: "Personal",
      tagOptions: [{ name: "idea", color: "green" }],
      pages: [{ title: "Garden", tags: ["idea"] }, { title: "Idea form", tags: ["idea"], template: true }],
    };
    const { store, client } = await renderTemplateApp([space]);
    const [info] = await client.spaceList();
    const spaceId = info?.id ?? "";
    const optionId = (await client.propertySchemaGet(spaceId))
      .find((def) => def.id === TAGS_PROPERTY_ID)
      ?.options.find((option) => option.name === "idea")?.id;
    act(() => {
      store.getState().openSearchWithTag({ spaceId, propertyId: TAGS_PROPERTY_ID, optionId: optionId ?? "" });
    });
    const dialog = await screen.findByRole("dialog", { name: t("search.label") });
    await waitFor(() => {
      expect(
        within(dialog)
          .getAllByRole("group")
          .filter((group) => group.classList.contains("search-group"))
          .map((group) => group.getAttribute("aria-label")),
      ).toEqual(["Garden"]);
    });
  });

  it("leaves templates out of the pages a picker lists from the tree", async () => {
    const { store } = await renderTemplateApp();
    const choices = await store.getState().findPages("");
    expect(choices.map((choice) => choice.title)).toEqual(["Journal", "Projects", "Roadmap", "Reading list"]);
  });

  it("leaves the templates of the target space out of the places the move dialog offers", async () => {
    const work: SeedSpace = {
      name: "Work",
      pages: [{ title: "Meetings" }, { title: "Meeting form", template: true, children: [{ title: "Inner" }] }],
    };
    const { user } = await renderTemplateApp([TEMPLATE_SPACE, work]);
    await user.pointer({ keys: "[MouseRight]", target: row("Journal") });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    await user.click(within(menu).getByRole("menuitem", { name: t("moveToSpace.menu") }));
    const dialog = await screen.findByRole("dialog", { name: t("moveToSpace.title", { name: "Journal" }) });
    await user.click(within(dialog).getByRole("button", { name: /^Work/u }));
    await within(dialog).findByRole("radio", { name: t("moveToSpace.topLevel") });
    const places = within(dialog)
      .getAllByRole("radio")
      .map((radio) => (radio as HTMLInputElement).labels?.[0]?.textContent ?? "");
    expect(places).toEqual([t("moveToSpace.topLevel"), "Meetings"]);
  });
});

// ----- turning a page into a template and back ---------------------------------------------------------------------

describe("turning a page into a template", () => {
  it("is in the row menu of the sidebar: the page leaves the tree and the core is asked", async () => {
    const { user, requests, memory } = await renderTemplateApp();
    const journal = await pageIdOf(memory, "Journal");
    await chooseFromRow(user, "Journal", t("template.turnInto"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "Reading list"]);
    });
    expect(requests).toContainEqual({ type: "PageSetTemplate", data: { pageId: journal, template: true } });
    expect((await pagesByTitle(memory)).get("Journal")?.template).toBe(true);
  });

  it("is in the page menu, which then offers to stop, and the banner shows", async () => {
    const { user, requests, memory } = await renderTemplateApp();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    expect(screen.queryByRole("region", { name: t("template.banner.label") })).not.toBeInTheDocument();
    let menu = await openPageMenu(user);
    expect(within(menu).queryByRole("menuitem", { name: t("template.stopUsing") })).not.toBeInTheDocument();
    await user.click(within(menu).getByRole("menuitem", { name: t("template.turnInto") }));

    await within(await screen.findByRole("region", { name: t("template.banner.label") })).findByText(t("template.banner.text"));
    expect(requests).toContainEqual({
      type: "PageSetTemplate",
      data: { pageId: await pageIdOf(memory, "Journal"), template: true },
    });
    // The page is a template now: the menu offers the way back and not the way in.
    menu = await openPageMenu(user);
    expect(within(menu).getByRole("menuitem", { name: t("template.stopUsing") })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("template.turnInto") })).not.toBeInTheDocument();
  });

  it("is not offered for a page inside a template, which names the template in its banner", async () => {
    const { user, store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Brief notes");
    expect(banner()).toHaveTextContent(t("template.banner.inside", { title: "Project brief" }));
    const menu = await openPageMenu(user);
    expect(within(menu).queryByRole("menuitem", { name: t("template.turnInto") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("template.stopUsing") })).not.toBeInTheDocument();
  });

  it("is undone by 'Stop using as template' of the banner: the page is in the sidebar again", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const id = await openByTitle(store, memory, "Meeting notes");
    expect(outline()).not.toContain("Meeting notes");
    await user.click(within(banner()).getByRole("button", { name: t("template.stopUsing") }));
    await waitFor(() => {
      expect(outline()).toContain("Meeting notes");
    });
    expect(requests).toContainEqual({ type: "PageSetTemplate", data: { pageId: id, template: false } });
    expect(screen.queryByRole("region", { name: t("template.banner.label") })).not.toBeInTheDocument();
  });

  it("is undone by 'Stop using as template' of the page menu", async () => {
    const { user, store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Meeting notes");
    const menu = await openPageMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: t("template.stopUsing") }));
    await waitFor(() => {
      expect(outline()).toContain("Meeting notes");
    });
    expect((await pagesByTitle(memory)).get("Meeting notes")?.template).toBeUndefined();
  });

  it("shows the reason when the core refuses, in the banner, and changes nothing", async () => {
    const { user, store, memory } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageSetTemplate") {
          throw new CoreError({ code: "InvalidInput", message: "the page is in the trash", details: { field: "pageId" } });
        }
        return undefined;
      },
    });
    await openByTitle(store, memory, "Meeting notes");
    await user.click(within(banner()).getByRole("button", { name: t("template.stopUsing") }));
    expect(await within(banner()).findByRole("alert")).toHaveTextContent(
      t("template.error.change", { message: "the page is in the trash" }),
    );
    expect(outline()).not.toContain("Meeting notes");
  });
});

// ----- the banner ---------------------------------------------------------------------------------------------------

describe("the template banner", () => {
  it("is above the title of a template, with Use template and Stop using as template", async () => {
    const { store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Meeting notes");
    expect(banner()).toHaveTextContent(t("template.banner.text"));
    expect(within(banner()).getByRole("button", { name: t("template.banner.use") })).toBeEnabled();
    expect(within(banner()).getByRole("button", { name: t("template.stopUsing") })).toBeEnabled();
    const heading = screen.getByRole("heading", { level: 1, name: "Meeting notes" });
    expect(banner().compareDocumentPosition(heading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("is not on a page that is not in a template", async () => {
    const { user } = await renderTemplateApp();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    expect(screen.queryByRole("region", { name: t("template.banner.label") })).not.toBeInTheDocument();
  });

  it("makes a page of a top-level template at the top level and opens it", async () => {
    const { user, store, memory, requests, client } = await renderTemplateApp();
    const template = await openByTitle(store, memory, "Meeting notes");
    await user.click(within(banner()).getByRole("button", { name: t("template.banner.use") }));
    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBe(template);
    });
    expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: template } });
    const made = await client.pageGet(store.getState().selectedPageId ?? "");
    expect(made.title).toBe("Meeting notes");
    expect(made.template).toBeUndefined();
    // The new page is in the sidebar, the template is not.
    expect(await screen.findByRole("heading", { level: 1, name: "Meeting notes" })).toBeInTheDocument();
    expect(outline()).toContain("Meeting notes");
    expect(screen.queryByRole("region", { name: t("template.banner.label") })).not.toBeInTheDocument();
  });

  it("makes a page of a template that lives under a page beside the template, under that page", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const template = await openByTitle(store, memory, "Project brief");
    const projects = await pageIdOf(memory, "Projects");
    await user.click(within(banner()).getByRole("button", { name: t("template.banner.use") }));
    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBe(template);
    });
    expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: template, parentId: projects } });
    await waitFor(() => {
      expect(outline()).toContain("  Project brief");
    });
  });

  it("makes a page from the template when the banner is on a page inside it", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const root = await pageIdOf(memory, "Project brief");
    const projects = await pageIdOf(memory, "Projects");
    await openByTitle(store, memory, "Brief notes");
    await user.click(within(banner()).getByRole("button", { name: t("template.banner.use") }));
    await waitFor(() => {
      expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: root, parentId: projects } });
    });
  });

  it("shows the reason when the core refuses, and stays on the template", async () => {
    const { user, store, memory } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageCreateFromTemplate") {
          throw new CoreError({ code: "InvalidInput", message: "the template is too big", details: { field: "templateId" } });
        }
        return undefined;
      },
    });
    const template = await openByTitle(store, memory, "Meeting notes");
    await user.click(within(banner()).getByRole("button", { name: t("template.banner.use") }));
    expect(await within(banner()).findByRole("alert")).toHaveTextContent(
      t("template.error.use", { message: "the template is too big" }),
    );
    expect(store.getState().selectedPageId).toBe(template);
  });
});

// ----- the banner in the peek ---------------------------------------------------------------------------------------

describe("the template banner in the peek", () => {
  /** The peek of either mode, once it is open. */
  async function peekElement(): Promise<HTMLElement> {
    await waitFor(() => {
      expect(document.querySelector("[data-peek]")).not.toBeNull();
    });
    return document.querySelector<HTMLElement>("[data-peek]") as HTMLElement;
  }

  it("shows a template opened in the side peek as one, compact, with Use template and Stop using as template", async () => {
    const { store, memory } = await renderTemplateApp();
    const template = await pageIdOf(memory, "Project brief");
    act(() => {
      store.getState().openPeek({ pageId: template, spaceId: store.getState().currentSpaceId ?? "", mode: "side" });
    });
    const peek = await peekElement();
    const peekBanner = await within(peek).findByRole("region", { name: t("template.banner.label") });
    expect(peekBanner).toHaveClass("is-compact");
    expect(peekBanner).toHaveTextContent(t("template.banner.text"));
    expect(within(peekBanner).getByRole("button", { name: t("template.banner.use") })).toBeEnabled();
    expect(within(peekBanner).getByRole("button", { name: t("template.stopUsing") })).toBeEnabled();
    // It is above the title of the peeked page.
    const heading = within(peek).getByRole("heading", { level: 2, name: "Project brief" });
    expect(peekBanner.compareDocumentPosition(heading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows it in the center peek too", async () => {
    const { store, memory } = await renderTemplateApp();
    const template = await pageIdOf(memory, "Meeting notes");
    act(() => {
      store.getState().openPeek({ pageId: template, spaceId: store.getState().currentSpaceId ?? "", mode: "center" });
    });
    const peek = await peekElement();
    expect(peek).toHaveAttribute("data-mode", "center");
    expect(await within(peek).findByRole("region", { name: t("template.banner.label") })).toBeInTheDocument();
  });

  it("is not on the peek of a page that is not in a template", async () => {
    const { store, memory } = await renderTemplateApp();
    const page = await pageIdOf(memory, "Roadmap");
    act(() => {
      store.getState().openPeek({ pageId: page, spaceId: store.getState().currentSpaceId ?? "", mode: "side" });
    });
    const peek = await peekElement();
    await within(peek).findByRole("heading", { level: 2, name: "Roadmap" });
    expect(within(peek).queryByRole("region", { name: t("template.banner.label") })).not.toBeInTheDocument();
  });

  it("makes the page under the template's parent from 'Use template' and shows it in the peek, as a click on a row would", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const template = await pageIdOf(memory, "Project brief");
    const projects = await pageIdOf(memory, "Projects");
    const spaceId = store.getState().currentSpaceId ?? "";
    act(() => {
      store.getState().openPeek({ pageId: template, spaceId, mode: "side" });
    });
    const peek = await peekElement();
    await user.click(await within(peek).findByRole("button", { name: t("template.banner.use") }));
    await waitFor(() => {
      expect(store.getState().peek?.pageId).not.toBe(template);
    });
    expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: template, parentId: projects } });
    // The peek shows the new page, still at the side, which is no template; the tab and its history did not move.
    const made = store.getState().peek;
    expect(made?.mode).toBe("side");
    expect(made?.pageId).not.toBeUndefined();
    expect(store.getState().selectedPageId).toBeNull();
    await waitFor(() => {
      expect(within(peek).queryByRole("region", { name: t("template.banner.label") })).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(outline()).toContain("  Project brief");
    });
  });

  it("shows the reason in the peek when the core refuses, and keeps the template open", async () => {
    const { user, store, memory } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageCreateFromTemplate") {
          throw new CoreError({ code: "InvalidInput", message: "too many pages", details: { field: "templateId" } });
        }
        return undefined;
      },
    });
    const template = await pageIdOf(memory, "Meeting notes");
    act(() => {
      store.getState().openPeek({ pageId: template, spaceId: store.getState().currentSpaceId ?? "", mode: "side" });
    });
    const peek = await peekElement();
    await user.click(await within(peek).findByRole("button", { name: t("template.banner.use") }));
    expect(await within(peek).findByRole("alert")).toHaveTextContent(t("template.error.use", { message: "too many pages" }));
    expect(store.getState().peek?.pageId).toBe(template);
  });
});

// ----- placeholders in the properties panel -------------------------------------------------------------------------

describe("placeholders in the properties panel", () => {
  async function panelOf(): Promise<HTMLElement> {
    return screen.findByRole("region", { name: t("properties.title") });
  }

  /** Opens the "Fill in … when created" menu of a property row and chooses one. */
  async function fillIn(user: User, rowName: string, choice: string): Promise<void> {
    const line = within(await panelOf()).getByRole("listitem", { name: rowName });
    await user.click(within(line).getByRole("button", { name: t("template.placeholder.menu", { name: rowName }) }));
    const menu = await screen.findByRole("menu", { name: t("template.placeholder.menuLabel", { name: rowName }) });
    await user.click(within(menu).getByRole("menuitem", { name: choice }));
  }

  it("offers Today and Now for a date property of a template, and shows the chosen one as a filled-in chip", async () => {
    const { user, store, memory, requests, client } = await renderTemplateApp();
    const page = await openByTitle(store, memory, "Meeting notes");
    const [space] = await client.spaceList();
    const due = (await client.propertySchemaGet(space?.id ?? "")).find((def) => def.name === "Due");
    const line = within(await panelOf()).getByRole("listitem", { name: "Due" });
    await user.click(within(line).getByRole("button", { name: t("template.placeholder.menu", { name: "Due" }) }));
    const menu = await screen.findByRole("menu", { name: t("template.placeholder.menuLabel", { name: "Due" }) });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("template.placeholder.today"),
      t("template.placeholder.now"),
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: t("template.placeholder.now") }));

    expect(requests).toContainEqual({
      type: "PageSetTemplatePlaceholder",
      data: { pageId: page, propertyId: due?.id, placeholder: "now" },
    });
    await waitFor(() => {
      const chip = within(within(screen.getByRole("region", { name: t("properties.title") })).getByRole("listitem", { name: "Due" }));
      expect(chip.getByText(t("template.placeholder.now"))).toBeInTheDocument();
      expect(chip.getByText(t("template.placeholder.filledIn"))).toBeInTheDocument();
    });
    // The value editor (the date picker's button) is replaced by the chip.
    const filled = within(await panelOf()).getByRole("listitem", { name: "Due" });
    expect(filled.querySelector('[aria-haspopup="dialog"]')).toBeNull();
  });

  it("clears a placeholder with Clear, and the value editor is back", async () => {
    const { user, store, memory, requests, client } = await renderTemplateApp();
    const page = await openByTitle(store, memory, "Meeting notes");
    const [space] = await client.spaceList();
    const due = (await client.propertySchemaGet(space?.id ?? "")).find((def) => def.name === "Due");
    await fillIn(user, "Due", t("template.placeholder.today"));
    await waitFor(() => {
      expect(screen.getByText(t("template.placeholder.filledIn"))).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: t("template.placeholder.clear", { name: "Due" }) }));
    await waitFor(() => {
      expect(screen.queryByText(t("template.placeholder.filledIn"))).not.toBeInTheDocument();
    });
    expect(requests).toContainEqual({ type: "PageSetTemplatePlaceholder", data: { pageId: page, propertyId: due?.id } });
    const line = within(await panelOf()).queryByRole("listitem", { name: "Due" });
    // With nothing left on the page the property row is gone until it is added again.
    expect(line === null || within(line).queryByText(t("template.placeholder.today")) === null).toBe(true);
  });

  it("offers Me for a person property", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const page = await openByTitle(store, memory, "Meeting notes");
    await user.click(within(await panelOf()).getByRole("button", { name: t("properties.add") }));
    const add = await screen.findByRole("menu", { name: t("properties.add") });
    await user.click(within(add).getByRole("menuitem", { name: /^Owner/u }));
    await fillIn(user, "Owner", t("template.placeholder.me"));
    await waitFor(() => {
      expect(
        requests.some(
          (request) =>
            request.type === "PageSetTemplatePlaceholder" && request.data.pageId === page && request.data.placeholder === "me",
        ),
      ).toBe(true);
    });
    expect(await screen.findByText(t("template.placeholder.filledIn"))).toBeInTheDocument();
  });

  it("is not offered on a page that is not in a template", async () => {
    const { user } = await renderTemplateApp();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await user.click(within(await panelOf()).getByRole("button", { name: t("properties.add") }));
    const add = await screen.findByRole("menu", { name: t("properties.add") });
    await user.click(within(add).getByRole("menuitem", { name: /^Due/u }));
    const line = within(await panelOf()).getByRole("listitem", { name: "Due" });
    expect(within(line).queryByRole("button", { name: t("template.placeholder.menu", { name: "Due" }) })).not.toBeInTheDocument();
  });

  it("is offered on a page inside a template as well", async () => {
    const { user, store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Brief notes");
    await user.click(within(await panelOf()).getByRole("button", { name: t("properties.add") }));
    const add = await screen.findByRole("menu", { name: t("properties.add") });
    await user.click(within(add).getByRole("menuitem", { name: /^Due/u }));
    const line = within(await panelOf()).getByRole("listitem", { name: "Due" });
    expect(within(line).getByRole("button", { name: t("template.placeholder.menu", { name: "Due" }) })).toBeInTheDocument();
  });

  it("shows the reason when the core refuses", async () => {
    const { user, store, memory } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageSetTemplatePlaceholder") {
          throw new CoreError({ code: "InvalidInput", message: "no such placeholder", details: { field: "placeholder" } });
        }
        return undefined;
      },
    });
    await openByTitle(store, memory, "Meeting notes");
    await fillIn(user, "Due", t("template.placeholder.today"));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("template.error.placeholder", { message: "no such placeholder" }));
  });
});

// ----- relative mentions in a template -------------------------------------------------------------------------------

describe("relative mentions in a template", () => {
  function placeCursorAtEnd(editor: Editor): void {
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, editor.state.doc.content.size - 1)));
    });
  }

  function mentionsOf(editor: Editor, name: string): Record<string, unknown>[] {
    const found: Record<string, unknown>[] = [];
    editor.state.doc.descendants((child) => {
      if (child.type.name === name) {
        found.push({ ...child.attrs });
      }
      return true;
    });
    return found;
  }

  it("offers Today (when created) and Now (when created) in the date picker of a mention, and marks the mention", async () => {
    const { user, store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Meeting notes");
    const editor = await findEditor();
    placeCursorAtEnd(editor);
    act(() => {
      editor.commands.insertDateMention({ date: "2026-10-03", openPicker: true });
    });
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    const group = within(picker).getByRole("group", { name: t("template.picker.relativeLabel") });
    expect(within(group).getAllByRole("button").map((button) => button.textContent)).toEqual([
      t("template.placeholder.today"),
      t("template.placeholder.now"),
    ]);
    await user.click(within(group).getByRole("button", { name: t("template.placeholder.today") }));
    await waitFor(() => {
      expect(mentionsOf(editor, "dateMention")[0]?.relative).toBe("today");
    });
    expect(screen.queryByRole("dialog", { name: t("date.pickerTitle") })).not.toBeInTheDocument();
    const chip = editor.view.dom.querySelector(".date-mention");
    expect(chip).toHaveClass("is-relative");
    expect(chip).toHaveTextContent(t("date.today"));
    // The chip is called by when it is filled in.
    expect(within(editor.view.dom).getByRole("button", { name: t("date.change", { date: t("template.placeholder.today") }) })).toBe(chip);
  });

  it("turns a relative mention back into a fixed day when the day is picked by hand", async () => {
    const { user, store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Meeting notes");
    const editor = await findEditor();
    placeCursorAtEnd(editor);
    act(() => {
      editor.commands.insertDateMention({ date: "2026-10-03", relative: "now", openPicker: true });
    });
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    expect(within(picker).getByRole("button", { name: t("template.placeholder.now") })).toHaveAttribute("aria-pressed", "true");
    await user.click(within(picker).getByRole("button", { name: /October 12, 2026|12 October 2026|12\/10\/2026|Monday, October 12/u }));
    await waitFor(() => {
      const [mention] = mentionsOf(editor, "dateMention");
      expect(mention?.date).toBe("2026-10-12");
      expect(mention?.relative).toBeNull();
    });
  });

  it("does not offer it in a page that is not in a template", async () => {
    const { user } = await renderTemplateApp();
    await user.click(row("Journal"));
    const editor = await findEditor();
    placeCursorAtEnd(editor);
    act(() => {
      editor.commands.insertDateMention({ date: "2026-10-03", openPicker: true });
    });
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    expect(within(picker).queryByRole("group", { name: t("template.picker.relativeLabel") })).not.toBeInTheDocument();
  });

  it("offers Me (whoever creates) in the @ menu and inserts a person mention marked relative", async () => {
    const { store, memory } = await renderTemplateApp();
    await openByTitle(store, memory, "Meeting notes");
    const editor = await findEditor();
    placeCursorAtEnd(editor);
    act(() => {
      editor.commands.insertContent(" @me");
    });
    const menu = await screen.findByRole("listbox", { name: t("mention.label") });
    const me = await within(menu).findByRole("option", { name: new RegExp(t("template.mention.me").replace(/[()]/gu, "\\$&"), "u") });
    expect(me).toHaveTextContent(t("template.mention.meHint"));
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await waitFor(() => {
      expect(mentionsOf(editor, "personMention")).toHaveLength(1);
    });
    const [mention] = mentionsOf(editor, "personMention");
    expect(mention?.relative).toBe("me");
    expect(typeof mention?.accountId).toBe("string");
    expect(String(mention?.label)).not.toBe("");
    const chip = editor.view.dom.querySelector(".person-mention");
    expect(chip).toHaveClass("is-relative");
    expect(chip).toHaveTextContent(`@${t("template.relative.me")}`);
  });

  it("does not offer Me (whoever creates) in a page that is not in a template", async () => {
    const { user } = await renderTemplateApp();
    await user.click(row("Journal"));
    const editor = await findEditor();
    placeCursorAtEnd(editor);
    act(() => {
      editor.commands.insertContent(" @me");
    });
    await screen.findByRole("listbox", { name: t("mention.label") });
    await waitFor(() => {
      expect(screen.queryByText(t("template.mention.me"))).not.toBeInTheDocument();
    });
  });
});

// ----- the Templates panel -------------------------------------------------------------------------------------------

describe("the Templates panel", () => {
  it("is an entry of the sidebar next to the trash that opens a dialog", async () => {
    const { user } = await renderTemplateApp();
    const entry = sidebarButton(t("template.panel.open"));
    const trash = sidebarButton(t("trash.title", { count: 0 }));
    expect(entry.compareDocumentPosition(trash) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: t("template.panel.title") })).not.toBeInTheDocument();
    const dialog = await openPanel(user);
    await user.keyboard("{Escape}");
    expect(dialog).not.toBeInTheDocument();
  });

  it("lists the templates by where they live, and the starters", async () => {
    const { user } = await renderTemplateApp();
    const dialog = await openPanel(user);
    expect(within(dialog).getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent)).toEqual([
      t("template.panel.groupTop"),
      "Projects",
      t("template.panel.starters"),
    ]);
    const top = within(dialog).getByRole("region", { name: t("template.panel.groupTop") });
    expect(within(top).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      expect.stringContaining("Meeting notes"),
    ]);
    const projects = within(dialog).getByRole("region", { name: "Projects" });
    expect(within(projects).getAllByRole("listitem")).toHaveLength(1);
    expect(within(projects).getByText("Project brief")).toBeInTheDocument();
    const starterList = within(dialog).getByRole("region", { name: t("template.panel.starters") });
    expect(within(starterList).getByText("Starter meeting notes (en)")).toBeInTheDocument();
    expect(within(starterList).getByText("Agenda, notes and actions")).toBeInTheDocument();
    expect(within(starterList).getByText("Starter journal")).toBeInTheDocument();
  });

  it("gives a template Use, Edit, Duplicate and Delete, and a starter Use and Add to my templates", async () => {
    const { user } = await renderTemplateApp();
    const dialog = await openPanel(user);
    const template = lineOf(dialog, "Meeting notes");
    expect(within(template).getAllByRole("button").map((button) => button.textContent)).toEqual([
      t("template.use"),
      t("template.edit"),
      t("template.duplicate"),
      t("template.delete"),
    ]);
    const starter = lineOf(dialog, "Starter journal");
    expect(within(starter).getAllByRole("button").map((button) => button.textContent)).toEqual([
      t("template.use"),
      t("template.addToMine"),
    ]);
  });

  it("uses a template: a page is made from it where the template lives, opened, and the panel closes", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const meeting = await pageIdOf(memory, "Meeting notes");
    const brief = await pageIdOf(memory, "Project brief");
    const projects = await pageIdOf(memory, "Projects");
    let dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Meeting notes")).getByRole("button", { name: t("template.use") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("template.panel.title") })).not.toBeInTheDocument();
    });
    expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: meeting } });
    expect(store.getState().selectedPageId).not.toBe(meeting);
    expect(store.getState().selectedPageId).not.toBeNull();

    dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Project brief")).getByRole("button", { name: t("template.use") }));
    await waitFor(() => {
      expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: brief, parentId: projects } });
    });
  });

  it("edits a template: it opens in the active tab with its banner, and the panel closes", async () => {
    const { user, store, memory } = await renderTemplateApp();
    const meeting = await pageIdOf(memory, "Meeting notes");
    const dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Meeting notes")).getByRole("button", { name: t("template.edit") }));
    expect(await screen.findByRole("heading", { level: 1, name: "Meeting notes" })).toBeInTheDocument();
    expect(store.getState().selectedPageId).toBe(meeting);
    expect(screen.queryByRole("dialog", { name: t("template.panel.title") })).not.toBeInTheDocument();
    expect(banner()).toBeInTheDocument();
  });

  it("duplicates a template without leaving the panel", async () => {
    const { user, store, requests, memory } = await renderTemplateApp();
    const meeting = await pageIdOf(memory, "Meeting notes");
    const dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Meeting notes")).getByRole("button", { name: t("template.duplicate") }));
    await waitFor(() => {
      expect(requests).toContainEqual({ type: "PageDuplicate", data: { pageId: meeting } });
    });
    expect(screen.getByRole("dialog", { name: t("template.panel.title") })).toBeInTheDocument();
    expect(store.getState().selectedPageId).not.toBe(await pageIdOf(memory, "Meeting notes (copy)"));
    expect(store.getState().selectedPageId).toBeNull();
  });

  it("deletes a template to the trash", async () => {
    const { user, requests, memory, client } = await renderTemplateApp();
    const meeting = await pageIdOf(memory, "Meeting notes");
    const dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Meeting notes")).getByRole("button", { name: t("template.delete") }));
    await waitFor(() => {
      expect(within(dialog).queryByText("Meeting notes")).not.toBeInTheDocument();
    });
    expect(requests).toContainEqual({ type: "PageTrash", data: { pageId: meeting } });
    const [space] = await client.spaceList();
    expect((await client.treeGet(space?.id ?? "")).trashed.map((entry) => entry.page.title)).toEqual(["Meeting notes"]);
  });

  it("uses a starter: it makes the page in the language of the app, with the day and the time, and opens it", async () => {
    const { user, store } = await renderTemplateApp();
    const dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Starter meeting notes (en)")).getByRole("button", { name: t("template.use") }));
    await waitFor(() => {
      expect(starters.instantiate).toHaveBeenCalledTimes(1);
    });
    const [, id, options] = starters.instantiate.mock.calls[0] as [unknown, string, Record<string, unknown>];
    expect(id).toBe("meeting-notes");
    expect(Object.keys(options).sort()).toEqual(["language", "now", "spaceId", "today"]);
    expect(options).toMatchObject({ spaceId: store.getState().currentSpaceId, language: "en" });
    expect(String(options.today)).toMatch(/^\d{4}-\d{2}-\d{2}$/u);
    expect(String(options.now)).toMatch(/^\d{2}:\d{2}$/u);
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("template.panel.title") })).not.toBeInTheDocument();
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Made from meeting-notes" })).toBeInTheDocument();
  });

  it("adds a starter to the templates of the space: a template page, listed at once, nothing opens", async () => {
    const { user, store } = await renderTemplateApp();
    const dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Starter journal")).getByRole("button", { name: t("template.addToMine") }));
    await waitFor(() => {
      expect(starters.instantiate).toHaveBeenCalledTimes(1);
    });
    const [, id, options] = starters.instantiate.mock.calls[0] as [unknown, string, Record<string, unknown>];
    expect(id).toBe("daily-journal");
    expect(options).toMatchObject({ spaceId: store.getState().currentSpaceId, asTemplate: true, language: "en" });
    const top = within(dialog).getByRole("region", { name: t("template.panel.groupTop") });
    expect(await within(top).findByText("Made from daily-journal")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: t("template.panel.title") })).toBeInTheDocument();
    expect(store.getState().selectedPageId).toBeNull();
    expect(outline()).not.toContain("Made from daily-journal");
  });

  it("makes a new template: a top-level page named New template with the flag, opened with its banner", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const dialog = await openPanel(user);
    await user.click(within(dialog).getByRole("button", { name: t("template.panel.new") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("template.panel.title") })).not.toBeInTheDocument();
    });
    const made = (await pagesByTitle(memory)).get(t("template.newTitle"));
    expect(made?.template).toBe(true);
    expect(made?.parentId).toBeUndefined();
    expect(requests).toContainEqual({ type: "PageSetTemplate", data: { pageId: made?.id, template: true } });
    expect(store.getState().selectedPageId).toBe(made?.id);
    expect(await screen.findByRole("heading", { level: 1, name: t("template.newTitle") })).toBeInTheDocument();
    expect(banner()).toBeInTheDocument();
    expect(outline()).not.toContain(t("template.newTitle"));
  });

  it("shows the reason when the core refuses, in the panel", async () => {
    const { user } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageDuplicate") {
          throw new CoreError({ code: "InvalidInput", message: "the page is in the trash", details: { field: "pageId" } });
        }
        return undefined;
      },
    });
    const dialog = await openPanel(user);
    await user.click(within(lineOf(dialog, "Meeting notes")).getByRole("button", { name: t("template.duplicate") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("template.error.panel", { message: "the page is in the trash" }));
  });

  it("says there are no templates yet, and still lists the starters", async () => {
    const { user } = await renderTemplateApp([{ name: "Personal", pages: [{ title: "Journal" }] }]);
    const dialog = await openPanel(user);
    expect(within(dialog).getByText(t("template.panel.empty"))).toBeInTheDocument();
    expect(within(dialog).getByText("Starter journal")).toBeInTheDocument();
  });

  it("follows the space that is open", async () => {
    const { user, store } = await renderTemplateApp([TEMPLATE_SPACE, OTHER_SPACE]);
    store.getState().selectSpace(store.getState().spaces[1]?.id ?? "");
    await waitFor(() => {
      expect(outline()).toEqual(["Meetings"]);
    });
    const dialog = await openPanel(user);
    expect(within(dialog).queryByText("Meeting notes")).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("template.panel.empty"))).toBeInTheDocument();
  });
});

// ----- a page made from a template -------------------------------------------------------------------------------------

describe("making a page from a template", () => {
  it("has a button beside New page that picks a template or a starter", async () => {
    const { user } = await renderTemplateApp();
    const button = screen.getByRole("button", { name: t("template.newPageFrom") });
    const newPage = screen.getByRole("button", { name: t("tree.newPage") });
    expect(newPage.nextElementSibling).toBe(button);
    await user.click(button);
    const picker = await screen.findByRole("menu", { name: t("template.picker.label") });
    const templates = within(picker).getByRole("group", { name: t("template.picker.templates") });
    // A page icon that is an emoji is part of the text; the template that lives under Projects says so.
    expect(within(templates).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "📝Meeting notes",
      "Project briefProjects",
    ]);
    const choices = within(picker).getByRole("group", { name: t("template.picker.starters") });
    expect(within(choices).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "📝Starter meeting notes (en)Agenda, notes and actions",
      "📅Starter journalA page a day",
    ]);
    // The button that opened it closes it.
    await user.click(button);
    expect(screen.queryByRole("menu", { name: t("template.picker.label") })).not.toBeInTheDocument();
  });

  it("makes a top-level page of the chosen template and opens it", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const meeting = await pageIdOf(memory, "Meeting notes");
    await user.click(screen.getByRole("button", { name: t("template.newPageFrom") }));
    const picker = await screen.findByRole("menu", { name: t("template.picker.label") });
    await user.click(within(picker).getByRole("menuitem", { name: /^Meeting notes/u }));
    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBeNull();
    });
    expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: meeting } });
    expect(screen.queryByRole("menu", { name: t("template.picker.label") })).not.toBeInTheDocument();
    await waitFor(() => {
      expect(outline()).toContain("Meeting notes");
    });
  });

  it("makes a page of a starter in the current space", async () => {
    const { user, store } = await renderTemplateApp();
    await user.click(screen.getByRole("button", { name: t("template.newPageFrom") }));
    const picker = await screen.findByRole("menu", { name: t("template.picker.label") });
    await user.click(within(picker).getByRole("menuitem", { name: /^Starter journal/u }));
    await waitFor(() => {
      expect(starters.instantiate).toHaveBeenCalledTimes(1);
    });
    const [, id, options] = starters.instantiate.mock.calls[0] as [unknown, string, Record<string, unknown>];
    expect(id).toBe("daily-journal");
    expect(options).toMatchObject({ spaceId: store.getState().currentSpaceId, language: "en" });
    expect(options).not.toHaveProperty("parentId");
    expect(await screen.findByRole("heading", { level: 1, name: "Made from daily-journal" })).toBeInTheDocument();
  });

  it("is in the row menu as a sub-page of that page, which opens expanded", async () => {
    const { user, store, memory, requests } = await renderTemplateApp();
    const journal = await pageIdOf(memory, "Journal");
    const meeting = await pageIdOf(memory, "Meeting notes");
    await chooseFromRow(user, "Journal", t("template.newSubPageFrom"));
    const picker = await screen.findByRole("menu", { name: t("template.picker.label") });
    await user.click(within(picker).getByRole("menuitem", { name: /^Meeting notes/u }));
    await waitFor(() => {
      expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: meeting, parentId: journal } });
    });
    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBeNull();
    });
    await waitFor(() => {
      // The new page comes with the copy of what is inside the template.
      expect(outline()).toEqual(["Journal", "  Meeting notes", "    Agenda", "Projects", "  Roadmap", "Reading list"]);
    });
  });

  it("makes a sub-page of a starter from the row menu too", async () => {
    const { user, memory } = await renderTemplateApp();
    const reading = await pageIdOf(memory, "Reading list");
    await chooseFromRow(user, "Reading list", t("template.newSubPageFrom"));
    const picker = await screen.findByRole("menu", { name: t("template.picker.label") });
    await user.click(within(picker).getByRole("menuitem", { name: /^Starter journal/u }));
    await waitFor(() => {
      expect(starters.instantiate).toHaveBeenCalledTimes(1);
    });
    const [, , options] = starters.instantiate.mock.calls[0] as [unknown, string, Record<string, unknown>];
    expect(options.parentId).toBe(reading);
  });

  it("closes with Escape and makes nothing", async () => {
    const { user, requests } = await renderTemplateApp();
    await user.click(screen.getByRole("button", { name: t("template.newPageFrom") }));
    await screen.findByRole("menu", { name: t("template.picker.label") });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu", { name: t("template.picker.label") })).not.toBeInTheDocument();
    expect(requests.filter((request) => request.type === "PageCreateFromTemplate")).toEqual([]);
    expect(starters.instantiate).not.toHaveBeenCalled();
  });

  it("shows the core's reason in the sidebar when it refuses", async () => {
    const { user } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageCreateFromTemplate") {
          throw new CoreError({ code: "InvalidInput", message: "too many pages", details: { field: "templateId" } });
        }
        return undefined;
      },
    });
    await user.click(screen.getByRole("button", { name: t("template.newPageFrom") }));
    const picker = await screen.findByRole("menu", { name: t("template.picker.label") });
    await user.click(within(picker).getByRole("menuitem", { name: /^Meeting notes/u }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("template.error.use", { message: "too many pages" }));
  });
});

// ----- /template -----------------------------------------------------------------------------------------------------------

describe("/template in the slash menu", () => {
  /** Types `/template` at the start of the empty page and chooses the entry. */
  async function chooseSlashTemplate(editor: Editor, user: User): Promise<HTMLElement> {
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
      editor.commands.insertContent("/template");
    });
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await user.click(within(menu).getByText(t("template.slash")));
    return screen.findByRole("menu", { name: t("template.picker.label") });
  }

  it("makes a sub-page of the open page from the chosen template, puts its card in the page and opens it", async () => {
    const { user, store, memory, requests, client } = await renderTemplateApp();
    const journal = await pageIdOf(memory, "Journal");
    const meeting = await pageIdOf(memory, "Meeting notes");
    await user.click(row("Journal"));
    const editor = await findEditor();
    const picker = await chooseSlashTemplate(editor, user);
    // The typed command is gone while the picker asks.
    expect(editor.getText()).not.toContain("/template");
    await user.click(within(picker).getByRole("menuitem", { name: /^Meeting notes/u }));

    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBe(journal);
    });
    expect(requests).toContainEqual({ type: "PageCreateFromTemplate", data: { templateId: meeting, parentId: journal } });
    const made = store.getState().selectedPageId ?? "";
    expect((await client.pageGet(made)).parentId).toBe(journal);
    // The card to the new page is in the page that was open, saved before the new page opened.
    expect(JSON.stringify((await client.pageContentGet(journal)).content)).toContain(`"pageId":"${made}"`);
    expect(JSON.stringify((await client.pageContentGet(journal)).content)).toContain('"type":"pageLink"');
  });

  it("makes the sub-page from a starter", async () => {
    const { user, store, memory } = await renderTemplateApp();
    const journal = await pageIdOf(memory, "Journal");
    await user.click(row("Journal"));
    const editor = await findEditor();
    const picker = await chooseSlashTemplate(editor, user);
    await user.click(within(picker).getByRole("menuitem", { name: /^Starter journal/u }));
    await waitFor(() => {
      expect(starters.instantiate).toHaveBeenCalledTimes(1);
    });
    const [, id, options] = starters.instantiate.mock.calls[0] as [unknown, string, Record<string, unknown>];
    expect(id).toBe("daily-journal");
    expect(options).toMatchObject({ parentId: journal, language: "en" });
    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBe(journal);
    });
  });

  it("inserts nothing and makes nothing when the picker is dismissed", async () => {
    const { user, requests, memory, client } = await renderTemplateApp();
    const journal = await pageIdOf(memory, "Journal");
    await user.click(row("Journal"));
    const editor = await findEditor();
    await chooseSlashTemplate(editor, user);
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu", { name: t("template.picker.label") })).not.toBeInTheDocument();
    });
    expect(requests.filter((request) => request.type === "PageCreateFromTemplate")).toEqual([]);
    expect(JSON.stringify((await client.pageContentGet(journal)).content)).not.toContain("pageLink");
    expect(screen.getByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
  });

  it("shows the reason in the page and inserts nothing when the page cannot be made", async () => {
    const { user, memory, client } = await renderTemplateApp([TEMPLATE_SPACE], {
      intercept: (request) => {
        if (request.type === "PageCreateFromTemplate") {
          throw new CoreError({ code: "InvalidInput", message: "the disk is full", details: { field: "templateId" } });
        }
        return undefined;
      },
    });
    const journal = await pageIdOf(memory, "Journal");
    await user.click(row("Journal"));
    const editor = await findEditor();
    const picker = await chooseSlashTemplate(editor, user);
    await user.click(within(picker).getByRole("menuitem", { name: /^Meeting notes/u }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("template.error.use", { message: "the disk is full" }));
    expect(JSON.stringify((await client.pageContentGet(journal)).content)).not.toContain("pageLink");
    expect(screen.getByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
  });
});

// ----- a role that cannot edit the space -----------------------------------------------------------------------------------

describe("a Viewer", () => {
  it("opens the templates of the space, and makes, changes and unmakes nothing", async () => {
    const work: SeedSpace = {
      name: "Work",
      pages: [{ title: "Plan" }, { title: "Standup", template: true }],
    };
    const { user } = await renderSharing({ role: "viewer", memory: { spaces: [work] } });
    // The sidebar has no way to make a page from a template either.
    await waitFor(() => {
      expect(outline()).toEqual(["Plan"]);
    });
    expect(screen.queryByRole("button", { name: t("template.newPageFrom") })).not.toBeInTheDocument();

    const dialog = await openPanel(user);
    expect(within(dialog).queryByRole("button", { name: t("template.panel.new") })).not.toBeInTheDocument();
    expect(within(dialog).queryByText("Starter journal")).not.toBeInTheDocument();
    const line = lineOf(dialog, "Standup");
    expect(within(line).getAllByRole("button").map((button) => button.textContent)).toEqual([t("template.open")]);
    await user.click(within(line).getByRole("button", { name: t("template.open") }));

    await screen.findByRole("heading", { level: 1, name: "Standup" });
    expect(banner()).toHaveTextContent(t("template.banner.text"));
    expect(within(banner()).queryByRole("button")).not.toBeInTheDocument();
    const menu = await openPageMenu(user);
    expect(within(menu).queryByRole("menuitem", { name: t("template.stopUsing") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("template.turnInto") })).not.toBeInTheDocument();
  });
});
