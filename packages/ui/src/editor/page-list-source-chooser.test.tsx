// Linked sources in the whole app (ADR-0058, `docs/architecture/database-power-ups.md` section 2): the rows, the heading and the link of
// a block whose source is another page's sub-pages or the whole space, where "New" and the templates of the menu next to it work, the
// state of a linked view whose page is gone (the message, the chooser for an editor only, nothing else listed), and the "Pages shown"
// chooser of the view settings with its page picker. The pure rules are `page-list-sources.test.ts`'s. Like `database-templates.test.tsx`,
// these tests run over the in-memory core with a wrapper that records what the UI sent, and seed the block with the ids of a build
// that only told them.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, Request, SeedSpace, Transport } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { pid } from "../test/editor";
import { idsOf, openPage } from "../test/page";
import { findMainNavigation, renderWith } from "../test/render";
import type { User } from "../test/render";

const SORT = { by: "title", dir: "asc" };
const TAG = { idea: "de000000-0000-4000-8000-00000000f101" };
const PRIORITY = "de000000-0000-4000-8000-00000000f001";

function ticking(): () => number {
  let now = Date.UTC(2026, 9, 3, 12, 0, 0);
  return () => {
    now += 1000;
    return now;
  };
}

/** Page ids in creation order, the same in every build of the same seeds (so a block can name a page before the core exists). */
function counter(): () => string {
  let next = 0;
  return () => {
    next += 1;
    return pid(100_000 + next);
  };
}

/** A hub whose block (id `pid(2)`) has `source` and one view of `layout`; `more` are further views as stored. */
function hubContent(source: JsonValue, layout = "table", extra: { [key: string]: JsonValue } = {}): JsonValue {
  const stored = { id: "v1", name: "", layout, properties: [] as string[], sort: SORT, ...extra };
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      { type: "pageList", attrs: { id: pid(2), source, layout, properties: [], sort: SORT, views: [stored] } },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

/**
 * Hub holds the block, with the children Alpha and Beta. Tasks has Task A, Task B and Task template (a template with Template notes).
 * Archive has Old stuff; Empty has nothing; Brief is a template at the top level. Alpha and Task A have a Priority. `only` leaves Hub
 * alone in the space.
 */
function space(content: JsonValue, only = false): SeedSpace {
  return {
    name: "Personal",
    tagOptions: [{ id: TAG.idea, name: "idea", color: "green" }],
    properties: [{ id: PRIORITY, name: "Priority", kind: "number" }],
    pages: only
      ? [{ title: "Hub", content }]
      : [
          { title: "Hub", content, children: [{ title: "Alpha", properties: { Priority: 1 } }, { title: "Beta" }] },
          {
            title: "Tasks",
            children: [
              { title: "Task A", properties: { Priority: 1 } },
              { title: "Task B" },
              { title: "Task template", template: true, children: [{ title: "Template notes" }] },
            ],
          },
          { title: "Archive", children: [{ title: "Old stuff" }] },
          { title: "Empty" },
          { title: "Brief", template: true },
        ],
  };
}

type Sent<T extends Request["type"]> = Extract<Request, { type: T }> extends { data: infer D } ? D : never;

interface RecordingCore {
  readonly transport: Transport;
  /** What the UI sent of one request type, in order. */
  readonly sent: <T extends Request["type"]>(type: T) => Sent<T>[];
}

/** The in-memory core as the app's transport, recording every request the UI sends. */
function recordingCore(inner: MemoryTransport): RecordingCore {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch(request) {
      requests.push(request);
      return inner.dispatch(request);
    },
    subscribe: (onEvent) => inner.subscribe(onEvent),
    coreState: () => inner.coreState(),
    docs: inner.docs,
    attachments: inner.attachments,
    lock: () => inner.lock(),
    unlock: (passphrase) => inner.unlock(passphrase),
  };
  return {
    transport,
    sent: <T extends Request["type"]>(type: T) =>
      requests.filter((request) => request.type === type).map((request) => (request as { data?: unknown }).data) as Sent<T>[],
  };
}

type Ids = Awaited<ReturnType<typeof idsOf>>;

interface HubOptions {
  readonly layout?: string;
  readonly extra?: { [key: string]: JsonValue };
  readonly only?: boolean;
}

/** Opens Hub with a block whose source `source(ids)` names pages by id; the first build only tells the ids. */
async function openHub(source: (ids: Ids) => JsonValue, { layout = "table", extra = {}, only = false }: HubOptions = {}) {
  const build = (content: JsonValue) =>
    new MemoryTransport({ now: ticking(), workspaceName: "Research", newId: counter(), spaces: [space(content, only)] });
  const known = await idsOf(build(hubContent({ kind: "children" })));
  const memory = build(hubContent(source(known), layout, extra));
  const core = recordingCore(memory);
  const app = renderWith(core.transport);
  await findMainNavigation();
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const ids = await idsOf(memory);
  return { ...app, memory, core, editor, ids, client: new CoreClient(memory), spaceId: ids.space("Personal") };
}

const blockElement = (): HTMLElement => screen.getByRole("group", { name: t("pageList.label") });

/** The titles of the rows of the table, in order. */
function rowTitles(scope: HTMLElement = blockElement()): string[] {
  return Array.from(scope.querySelectorAll(".page-table-row[data-page] .page-table-title")).map((entry) => entry.textContent ?? "");
}

/** The rows of a view over the whole space: every page but Hub, templates and what is inside them left out, in the view's title order. */
const SPACE_ROWS = ["Alpha", "Archive", "Beta", "Empty", "Old stuff", "Task A", "Task B", "Tasks"];

/** The link under the heading to a linked view's source page, `null` when there is none. */
const sourceLink = (): Element | null => blockElement().querySelector(".page-list-source-link");
const headingText = (): string => blockElement().querySelector(".page-list-heading")?.textContent ?? "";
const emptyText = (): string => blockElement().querySelector(".page-list-empty")?.textContent ?? "";
const newPageButton = (scope: HTMLElement = blockElement()): HTMLElement => within(scope).getByRole("button", { name: t("pageList.newPage") });

/** The source the editor's document holds for the block. */
function sourceOf(editor: Editor): unknown {
  let found: unknown;
  editor.state.doc.descendants((entry) => {
    if (entry.type.name === "pageList") {
      found = entry.attrs.source;
    }
    return true;
  });
  return found;
}

async function openSettings(user: User): Promise<HTMLElement> {
  await user.click(within(blockElement()).getByRole("button", { name: t("pageList.viewSettings") }));
  return screen.findByRole("dialog", { name: t("pageList.viewSettings") });
}

const chooserOf = (scope: HTMLElement): HTMLSelectElement =>
  within(scope).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.source.label") });

const optionTexts = (select: HTMLSelectElement): string[] => Array.from(select.options).map((option) => option.textContent ?? "");

/** The titles the picker lists (the second child of each option is the title, after the icon). */
const pickerTitles = (picker: HTMLElement): string[] =>
  within(picker)
    .getAllByRole("option")
    .map((option) => option.children[1]?.textContent ?? "");

async function openMenu(user: User): Promise<HTMLElement> {
  await user.click(within(blockElement()).getByRole("button", { name: t("databaseTemplate.menu") }));
  return screen.findByRole("menu", { name: t("databaseTemplate.panel") });
}

const menuTemplates = (menu: HTMLElement): string[] =>
  Array.from(menu.querySelectorAll("[data-template] .database-new-title")).map((entry) => entry.textContent ?? "");

describe("a block that lists the sub-pages of another page", () => {
  const tasksSource = (ids: Ids): JsonValue => ({ kind: "page", pageId: ids.page("Tasks") });

  it("draws the sub-pages of that page, without its templates, and says whose they are", async () => {
    await openHub(tasksSource);
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Task A", "Task B"]);
    });
    expect(headingText()).toBe(t("pageList.source.page", { title: "Tasks" }));
  });

  it("links to the source page, which opens as a page", async () => {
    const { user } = await openHub(tasksSource);
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(2);
    });
    await user.click(within(blockElement()).getByRole("button", { name: t("pageList.source.open", { title: "Tasks" }) }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent).toBe("Tasks");
    });
  });

  it("has the link to its source page, which a view of the page's own sub-pages does not", async () => {
    await openHub(() => ({ kind: "children" }));
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Alpha", "Beta"]);
    });
    expect(sourceLink()).toBeNull();
  });

  it("says so when the page has no sub-pages", async () => {
    await openHub((ids) => ({ kind: "page", pageId: ids.page("Empty") }));
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.empty.page"));
    });
  });

  it("makes a new page under the source page, not under the page that holds the block", async () => {
    const { user, core, ids, client, spaceId } = await openHub(tasksSource);
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(2);
    });
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreate")).toHaveLength(1);
    });
    expect(core.sent("PageCreate")[0]?.parentId).toBe(ids.page("Tasks"));
    const tree = await client.treeGet(spaceId);
    const tasks = tree.roots.find((entry) => entry.page.id === ids.page("Tasks"));
    expect(tasks?.children).toHaveLength(4);
    expect(tree.roots.find((entry) => entry.page.id === ids.page("Hub"))?.children.map((entry) => entry.page.title)).toEqual(["Alpha", "Beta"]);
  });

  it("lists the templates of the source page in the menu next to New, and makes a new template under it", async () => {
    const { user, core, ids } = await openHub(tasksSource);
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(2);
    });
    const menu = await openMenu(user);
    expect(menuTemplates(menu)).toEqual(["Task template"]);
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.newTemplate") }));
    await waitFor(() => {
      expect(core.sent("PageSetTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreate")[0]?.parentId).toBe(ids.page("Tasks"));
  });

  it("makes a page from a template of the source page, under the source page", async () => {
    const { user, core, ids } = await openHub(tasksSource);
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(2);
    });
    const menu = await openMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: /^Task template/u }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")[0]).toEqual({ templateId: ids.page("Task template"), parentId: ids.page("Tasks") });
  });

  it("draws the same rows in the list layout", async () => {
    await openHub(tasksSource, { layout: "list" });
    await waitFor(() => {
      expect(Array.from(blockElement().querySelectorAll(".page-list-row .page-list-name")).map((entry) => entry.textContent)).toEqual([
        "Task A",
        "Task B",
      ]);
    });
  });

  it("follows the tree: a page made elsewhere under the source page shows up", async () => {
    const { client, ids } = await openHub(tasksSource);
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Task A", "Task B"]);
    });
    await client.pageCreate({ spaceId: ids.space("Personal"), parentId: ids.page("Tasks"), title: "Task C" });
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Task A", "Task B", "Task C"]);
    });
  });
});

describe("a block that lists every page of the space", () => {
  const space_ = (): JsonValue => ({ kind: "space" });

  it("draws every live page but the one that holds the block, at any depth, without templates or what is inside them", async () => {
    await openHub(space_);
    await waitFor(() => {
      expect(rowTitles()).toEqual(SPACE_ROWS);
    });
    expect(headingText()).toBe(t("pageList.source.space"));
    expect(sourceLink()).toBeNull();
  });

  it("makes a new page under the page that holds the block", async () => {
    const { user, core, ids } = await openHub(space_);
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(8);
    });
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreate")).toHaveLength(1);
    });
    expect(core.sent("PageCreate")[0]?.parentId).toBe(ids.page("Hub"));
  });

  it("lists every template of the space in the menu next to New", async () => {
    const { user } = await openHub(space_);
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(8);
    });
    const menu = await openMenu(user);
    expect(menuTemplates(menu)).toEqual(["Task template", "Brief"]);
  });

  it("says so when the space has no other page", async () => {
    await openHub(space_, { only: true });
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.empty.space"));
    });
  });

  it("is narrowed by the view's filters", async () => {
    const filter = { op: "and", items: [{ propertyId: PRIORITY, op: "isSet" }] };
    await openHub(space_, { extra: { filter } });
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Alpha", "Task A"]);
    });
  });

  it("says so when the filters hide every page", async () => {
    const filter = { op: "and", items: [{ propertyId: PRIORITY, op: "equals", value: 99 }] };
    await openHub(space_, { extra: { filter } });
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.empty.filtered"));
    });
    expect(rowTitles()).toEqual([]);
  });
});

describe("a linked view whose page is no longer available", () => {
  const goneSource = (): JsonValue => ({ kind: "page", pageId: pid(999_999) });

  it("says so, lists nothing and offers no New", async () => {
    await openHub(goneSource);
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.source.missing"));
    });
    expect(rowTitles()).toEqual([]);
    expect(within(blockElement()).queryByRole("button", { name: t("pageList.newPage") })).toBeNull();
    expect(sourceLink()).toBeNull();
    expect(headingText()).toBe(t("pageList.source.pageGone"));
  });

  it.each(["Task template", "Template notes"])("says so for a source page that is a template or inside one (%s)", async (title) => {
    await openHub((ids) => ({ kind: "page", pageId: ids.page(title) }));
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.source.missing"));
    });
    expect(rowTitles()).toEqual([]);
  });

  it("says so when the source page is moved to the trash meanwhile, and lists the pages again once it is restored", async () => {
    const { client, ids } = await openHub((known) => ({ kind: "page", pageId: known.page("Archive") }));
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Old stuff"]);
    });
    await client.pageTrash(ids.page("Archive"));
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.source.missing"));
    });
    expect(rowTitles()).toEqual([]);
    await client.pageRestore(ids.page("Archive"));
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Old stuff"]);
    });
  });

  it("offers an editor the Pages shown chooser in place of the list, and writes the source that is chosen", async () => {
    const { user, editor } = await openHub(goneSource);
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.source.missing"));
    });
    const select = chooserOf(blockElement());
    expect(select).toHaveValue(`page:${pid(999_999)}`);
    expect(optionTexts(select)).toEqual([
      t("pageList.source.children"),
      t("pageList.source.tag", { name: "idea" }),
      t("pageList.source.pageGone"),
      t("pageList.source.pickPage"),
      t("pageList.source.space"),
    ]);
    await user.selectOptions(select, t("pageList.source.space"));
    await waitFor(() => {
      expect(rowTitles()).toEqual(SPACE_ROWS);
    });
    expect(sourceOf(editor)).toEqual({ kind: "space" });
    expect(within(blockElement()).queryByRole("combobox", { name: t("pageList.source.label") })).toBeNull();
  });

  it("picks another page from the chooser in place of the list", async () => {
    const { user, editor, ids } = await openHub(goneSource);
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.source.missing"));
    });
    await user.selectOptions(chooserOf(blockElement()), t("pageList.source.pickPage"));
    const picker = await screen.findByRole("dialog", { name: t("pageList.source.pickerLabel") });
    await user.click(within(picker).getByRole("option", { name: /^Archive$/u }));
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Old stuff"]);
    });
    expect(sourceOf(editor)).toEqual({ kind: "page", pageId: ids.page("Archive") });
    expect(screen.queryByRole("dialog", { name: t("pageList.source.pickerLabel") })).toBeNull();
  });

  it("only says so in a read-only editor, with no chooser", async () => {
    const { editor } = await openHub(goneSource);
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.source.missing"));
    });
    editor.setEditable(false);
    await waitFor(() => {
      expect(within(blockElement()).queryByRole("combobox", { name: t("pageList.source.label") })).toBeNull();
    });
    expect(emptyText()).toBe(t("pageList.source.missing"));
  });
});

describe("the Pages shown chooser of the view settings", () => {
  it("lists the sub-pages of this page, the tags, the sub-pages of another page and every page, in this order", async () => {
    const { user } = await openHub(() => ({ kind: "children" }));
    const menu = await openSettings(user);
    const select = chooserOf(menu);
    expect(select).toHaveValue("children");
    expect(optionTexts(select)).toEqual([
      t("pageList.source.children"),
      t("pageList.source.tag", { name: "idea" }),
      t("pageList.source.pickPage"),
      t("pageList.source.space"),
    ]);
  });

  it("shows the source page of a linked view as its own entry, selected", async () => {
    const { user, ids } = await openHub((known) => ({ kind: "page", pageId: known.page("Tasks") }));
    const select = chooserOf(await openSettings(user));
    expect(select).toHaveValue(`page:${ids.page("Tasks")}`);
    expect(optionTexts(select)).toEqual([
      t("pageList.source.children"),
      t("pageList.source.tag", { name: "idea" }),
      t("pageList.source.page", { title: "Tasks" }),
      t("pageList.source.pickPage"),
      t("pageList.source.space"),
    ]);
  });

  it("writes the space source at once", async () => {
    const { user, editor } = await openHub(() => ({ kind: "children" }));
    const menu = await openSettings(user);
    await user.selectOptions(chooserOf(menu), t("pageList.source.space"));
    await waitFor(() => {
      expect(sourceOf(editor)).toEqual({ kind: "space" });
    });
    await waitFor(() => {
      expect(rowTitles()).toHaveLength(8);
    });
    expect(menu).toBeInTheDocument();
  });

  it("writes a tag and the children again as before", async () => {
    const { user, editor } = await openHub(() => ({ kind: "space" }));
    const menu = await openSettings(user);
    await user.selectOptions(chooserOf(menu), t("pageList.source.tag", { name: "idea" }));
    await waitFor(() => {
      expect(sourceOf(editor)).toEqual({ kind: "tag", optionId: TAG.idea });
    });
    await user.selectOptions(chooserOf(menu), t("pageList.source.children"));
    await waitFor(() => {
      expect(sourceOf(editor)).toEqual({ kind: "children" });
    });
  });

  it("opens a page picker for another page that lists the live pages without the host, templates and what is inside them", async () => {
    const { user, editor } = await openHub(() => ({ kind: "children" }));
    const menu = await openSettings(user);
    await user.selectOptions(chooserOf(menu), t("pageList.source.pickPage"));
    const picker = await screen.findByRole("dialog", { name: t("pageList.source.pickerLabel") });
    expect(pickerTitles(picker)).toEqual(["Alpha", "Beta", "Tasks", "Task A", "Task B", "Archive", "Old stuff", "Empty"]);
    // Nothing is written until a page is picked, and the select still shows the stored source.
    expect(sourceOf(editor)).toEqual({ kind: "children" });
    expect(chooserOf(menu)).toHaveValue("children");
  });

  it("narrows the picker by the title, picks a page with Enter and keeps the settings open", async () => {
    const { user, editor, ids } = await openHub(() => ({ kind: "children" }));
    const menu = await openSettings(user);
    await user.selectOptions(chooserOf(menu), t("pageList.source.pickPage"));
    const picker = await screen.findByRole("dialog", { name: t("pageList.source.pickerLabel") });
    const search = within(picker).getByRole("combobox", { name: t("pageList.source.pickerSearch") });
    expect(search).toHaveFocus();
    await user.type(search, "TASK");
    expect(pickerTitles(picker)).toEqual(["Tasks", "Task A", "Task B"]);
    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => {
      expect(sourceOf(editor)).toEqual({ kind: "page", pageId: ids.page("Task A") });
    });
    expect(screen.queryByRole("dialog", { name: t("pageList.source.pickerLabel") })).toBeNull();
    // The press in the picker did not close the settings: the chooser names the page, and the rows follow.
    expect(screen.getByRole("dialog", { name: t("pageList.viewSettings") })).toBe(menu);
    expect(chooserOf(menu)).toHaveValue(`page:${ids.page("Task A")}`);
    expect(optionTexts(chooserOf(menu))).toContain(t("pageList.source.page", { title: "Task A" }));
  });

  it("says so when no page matches, and Escape closes the picker alone", async () => {
    const { user } = await openHub(() => ({ kind: "children" }));
    const menu = await openSettings(user);
    await user.selectOptions(chooserOf(menu), t("pageList.source.pickPage"));
    const picker = await screen.findByRole("dialog", { name: t("pageList.source.pickerLabel") });
    await user.type(within(picker).getByRole("combobox", { name: t("pageList.source.pickerSearch") }), "zzzz");
    expect(within(picker).queryAllByRole("option")).toEqual([]);
    expect(within(picker).getByText(t("pageList.source.pickerNone"))).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("pageList.source.pickerLabel") })).toBeNull();
    });
    expect(screen.getByRole("dialog", { name: t("pageList.viewSettings") })).toBe(menu);
  });

  it("offers the picker again from a linked view, and the page that is picked replaces the source page", async () => {
    const { user, editor, ids } = await openHub((known) => ({ kind: "page", pageId: known.page("Tasks") }));
    const menu = await openSettings(user);
    await user.selectOptions(chooserOf(menu), t("pageList.source.pickPage"));
    const picker = await screen.findByRole("dialog", { name: t("pageList.source.pickerLabel") });
    await user.click(within(picker).getByRole("option", { name: /^Archive$/u }));
    await waitFor(() => {
      expect(sourceOf(editor)).toEqual({ kind: "page", pageId: ids.page("Archive") });
    });
    await waitFor(() => {
      expect(rowTitles()).toEqual(["Old stuff"]);
    });
    expect(headingText()).toBe(t("pageList.source.page", { title: "Archive" }));
  });
});

describe("a dashboard over a linked source", () => {
  const embedded = (): { [key: string]: JsonValue } => ({
    widgets: [{ id: "a0000001", kind: "view", title: "Linked", w: 4, h: "l", viewId: "v2" }],
  });
  const more = { id: "v2", name: "Linked", layout: "table", properties: [] as string[], sort: SORT };

  /** A dashboard block with an embedded table view; the stored block gets the second view. */
  async function openDashboard(source: (ids: Ids) => JsonValue) {
    const build = (content: JsonValue) =>
      new MemoryTransport({ now: ticking(), workspaceName: "Research", newId: counter(), spaces: [space(content)] });
    const dashboard = (known: Ids): JsonValue => {
      const stored = { id: "v1", name: "", layout: "dashboard", properties: [] as string[], sort: SORT, ...embedded() };
      return {
        type: "doc",
        content: [
          { type: "paragraph", attrs: { id: pid(1) } },
          {
            type: "pageList",
            attrs: { id: pid(2), source: source(known), layout: "dashboard", properties: [], sort: SORT, views: [stored, more] },
          },
        ],
      };
    };
    const known = await idsOf(build({ type: "doc", content: [] }));
    const memory = build(dashboard(known));
    const app = renderWith(recordingCore(memory).transport);
    await findMainNavigation();
    await openPage(app.user, "Hub");
    await screen.findAllByRole("group", { name: t("pageList.label") });
    return app;
  }

  it("draws an embedded view's rows from the sub-pages of the source page", async () => {
    await openDashboard((ids) => ({ kind: "page", pageId: ids.page("Tasks") }));
    await waitFor(() => {
      expect(rowTitles(screen.getByRole("group", { name: "Linked" }))).toEqual(["Task A", "Task B"]);
    });
  });

  it("draws an embedded view's rows from every page of the space", async () => {
    await openDashboard(() => ({ kind: "space" }));
    await waitFor(() => {
      expect(rowTitles(screen.getByRole("group", { name: "Linked" }))).toHaveLength(8);
    });
  });
});
