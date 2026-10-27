// Database templates in the UI (ADR-0056, `docs/architecture/templates.md` section 7): the rows of a view without templates, the
// templates of a source, the menu next to each layout's "New" (`NewPageMenu`), which template "New" makes a page from (the view's
// `defaultTemplate`, an explicit choice, an empty page), "Set as default" / "Remove as default", the actions of a template row (edit,
// duplicate, delete) and "New template".
//
// The pure rules and the menu are tested on their own (a fake context, no app). The rest runs in the whole app over the in-memory core,
// whose templates are seeded (`template: true`), with a thin wrapper (`recordingCore`) that records what the UI sent: these tests are about
// what the UI asks for; the core's own behaviour (the copy, the placeholders, the index) is tested with the core.

import { CoreClient, CoreError, MemoryTransport, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type {
  JsonValue,
  PageId,
  PageInfo,
  PageNode,
  Request,
  SeedSpace,
  SpaceId,
  Transport,
  TreeView,
} from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import { activeLocale, t } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { pid } from "../test/editor";
import { idsOf, openPage } from "../test/page";
import { findMainNavigation, NOW, renderWith } from "../test/render";
import { DashboardHostContext } from "./dashboard/dashboard-host";
import { PageDashboardView } from "./dashboard/PageDashboardView";
import { fullDate } from "./dates";
import { DatabaseTemplatesContext, hiddenTemplatePageIds, resolveDefaultTemplate, splitTemplates, tagValueFor } from "./database-templates";
import type { DatabaseTemplateActions } from "./database-templates";
import { loroSyncPluginKey } from "./loro-binding";
import { NewPageMenu } from "./NewPageMenu";
import type { PageListLayoutProps } from "./page-list-layout-types";
import { layoutDef } from "./page-list-layouts";
import { parseView, parseViews } from "./page-list-views";
import type { PageListViewPatch } from "./page-list-views";

afterEach(() => {
  vi.restoreAllMocks();
});

// ----- pages and trees for the pure rules ----------------------------------------------------------

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const id = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;

function page(n: number, title: string, extra: Partial<PageInfo> = {}): PageInfo {
  return { id: id(n), spaceId: SPACE, title, createdAt: n, updatedAt: n, ...extra };
}

function node(info: PageInfo, ...children: PageNode[]): PageNode {
  return { page: info, children };
}

function treeOf(...roots: PageNode[]): TreeView {
  return { spaceId: SPACE, roots, trashed: [], undo: { canUndo: false, canRedo: false } };
}

const titlesOf = (pages: readonly PageInfo[]): string[] => pages.map((entry) => entry.title);

describe("the rows of a source and its templates", () => {
  // Hub (1): Alpha (2), Weekly (3, a template) with Weekly notes (4), Beta (5), Brief (6, a template). Notes (7) is elsewhere.
  const weekly = page(3, "Weekly", { template: true });
  const brief = page(6, "Brief", { template: true });
  const alpha = page(2, "Alpha");
  const beta = page(5, "Beta");
  const notes = page(4, "Weekly notes");
  const hub = page(1, "Hub");
  const tree = treeOf(node(hub, node(alpha), node(weekly, node(notes)), node(beta), node(brief)), node(page(7, "Notes")));
  const children = [alpha, weekly, beta, brief];

  it("leaves templates out of the rows and lists them apart, in tree order", () => {
    const { rows, templates } = splitTemplates(tree, children, hub.id);
    expect(titlesOf(rows)).toEqual(["Alpha", "Beta"]);
    expect(titlesOf(templates)).toEqual(["Weekly", "Brief"]);
  });

  it("leaves out the pages inside a template too (a tag source can list them)", () => {
    const { rows, templates } = splitTemplates(tree, [alpha, weekly, notes, beta], hub.id);
    expect(titlesOf(rows)).toEqual(["Alpha", "Beta"]);
    expect(titlesOf(templates)).toEqual(["Weekly"]);
  });

  it("keeps the page that holds the block among the rows of a tag source, and never among the templates", () => {
    expect(titlesOf(splitTemplates(tree, [hub, alpha, weekly], hub.id).rows)).toEqual(["Hub", "Alpha"]);
    const flaggedHub = page(1, "Hub", { template: true });
    const inTemplate = treeOf(node(flaggedHub, node(alpha)));
    expect(titlesOf(splitTemplates(inTemplate, [flaggedHub, alpha], flaggedHub.id).templates)).toEqual([]);
  });

  it("has no rows to hide in an empty tree and hides nothing while the tree is not read", () => {
    expect(splitTemplates(null, children, hub.id).rows).toEqual(children);
    expect([...hiddenTemplatePageIds(null, hub.id)]).toEqual([]);
  });

  it("draws the pages of the template that holds the block as ordinary rows", () => {
    // A template of a project (Plan, flagged) holds a table of its own tasks: Plan's children are its rows.
    const plan = page(10, "Plan", { template: true });
    const task = page(11, "Task");
    const nested = page(12, "Task notes");
    const inside = treeOf(node(plan, node(task, node(nested))), node(weekly));
    const split = splitTemplates(inside, [task], plan.id);
    expect(titlesOf(split.rows)).toEqual(["Task"]);
    // ... while another block, outside Plan, still does not draw them.
    expect(titlesOf(splitTemplates(inside, [task], null).rows)).toEqual([]);
    expect([...hiddenTemplatePageIds(inside, plan.id)].sort()).toEqual([plan.id, weekly.id].sort());
  });

  it("uses the view's default template only while it is one of the source's templates", () => {
    const templates = [weekly, brief];
    expect(resolveDefaultTemplate({ defaultTemplate: brief.id }, templates)).toBe(brief.id);
    expect(resolveDefaultTemplate({}, templates)).toBeNull();
    expect(resolveDefaultTemplate({ defaultTemplate: id(99) }, templates)).toBeNull();
    expect(resolveDefaultTemplate({ defaultTemplate: brief.id }, [])).toBeNull();
  });

  it("gives a page of a tag source the template's own tags and the source's tag once", () => {
    const withTags = page(8, "Tagged", { template: true, properties: [[TAGS_PROPERTY_ID, { kind: "multi_select", value: ["a", "b"] }]] });
    expect(tagValueFor(withTags, "b")).toEqual({ kind: "multi_select", value: ["a", "b"] });
    expect(tagValueFor(withTags, "c")).toEqual({ kind: "multi_select", value: ["a", "b", "c"] });
    expect(tagValueFor(weekly, "c")).toEqual({ kind: "multi_select", value: ["c"] });
    expect(tagValueFor(undefined, "c")).toEqual({ kind: "multi_select", value: ["c"] });
  });
});

// ----- the menu on its own -------------------------------------------------------------------------

type FakeActions = { [K in keyof DatabaseTemplateActions]: Mock<DatabaseTemplateActions[K]> };

function fakeActions(): FakeActions {
  return {
    edit: vi.fn<DatabaseTemplateActions["edit"]>(),
    duplicate: vi.fn<DatabaseTemplateActions["duplicate"]>(() => Promise.resolve()),
    remove: vi.fn<DatabaseTemplateActions["remove"]>(() => Promise.resolve()),
    create: vi.fn<DatabaseTemplateActions["create"]>(() => Promise.resolve()),
  };
}

const WEEKLY = page(3, "Weekly", { template: true, icon: "📅" });
const BRIEF = page(6, "Brief", { template: true });

function renderMenu(
  options: {
    templates?: readonly PageInfo[];
    view?: { defaultTemplate?: PageId };
    init?: { values: readonly (readonly [string, { kind: "text"; value: string }])[] };
    disabled?: boolean;
    label?: string;
    context?: boolean;
  } = {},
) {
  const actions = fakeActions();
  const onCreatePage = vi.fn();
  const onUpdateView = vi.fn<(patch: PageListViewPatch) => void>();
  const onOuterClick = vi.fn();
  const user = userEvent.setup();
  const menu = (
    <div onClick={onOuterClick}>
      <NewPageMenu
        view={options.view ?? {}}
        onCreatePage={onCreatePage}
        onUpdateView={onUpdateView}
        init={options.init}
        disabled={options.disabled}
        label={options.label}
      />
    </div>
  );
  render(
    options.context === false ? (
      menu
    ) : (
      <DatabaseTemplatesContext value={{ templates: options.templates ?? [WEEKLY, BRIEF], actions }}>{menu}</DatabaseTemplatesContext>
    ),
  );
  return { user, actions, onCreatePage, onUpdateView, onOuterClick };
}

/** What a menu item says: a template row's title without its icon. */
function label(item: HTMLElement): string {
  return item.querySelector(".database-new-title")?.textContent ?? item.textContent ?? "";
}

const trigger = (name = t("databaseTemplate.menu")): HTMLElement => screen.getByRole("button", { name });
const openedMenu = (): Promise<HTMLElement> => screen.findByRole("menu", { name: t("databaseTemplate.panel") });

async function openRowOptions(user: ReturnType<typeof userEvent.setup>, title: string): Promise<HTMLElement> {
  const menu = await openedMenu();
  await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title }) }));
  return menu;
}

describe("the menu next to New", () => {
  it("draws nothing outside a database block", () => {
    renderMenu({ context: false });
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("is a button that opens a menu of the templates, Empty page and New template, and closes with Escape", async () => {
    const { user } = renderMenu();
    expect(trigger()).toHaveAttribute("aria-haspopup", "menu");
    expect(trigger()).toHaveAttribute("aria-expanded", "false");
    await user.click(trigger());
    const menu = await openedMenu();
    expect(trigger()).toHaveAttribute("aria-expanded", "true");
    expect(within(menu).getAllByRole("menuitem").map(label)).toEqual([
      "Weekly",
      "Brief",
      t("databaseTemplate.emptyPage"),
      t("databaseTemplate.newTemplate"),
    ]);
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
  });

  it("closes when the button is pressed again, and takes the name and the state the layout gives it", async () => {
    const { user } = renderMenu({ label: "New page options in Review", disabled: false });
    await user.click(trigger("New page options in Review"));
    await openedMenu();
    await user.click(trigger("New page options in Review"));
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
  });

  it("waits while a page is being made", () => {
    renderMenu({ disabled: true });
    expect(trigger()).toBeDisabled();
  });

  it("says there are no templates and still offers an empty page and a new template", async () => {
    const { user } = renderMenu({ templates: [] });
    await user.click(trigger());
    const menu = await openedMenu();
    expect(within(menu).getByText(t("databaseTemplate.none"))).toBeInTheDocument();
    expect(within(menu).getAllByRole("menuitem").map(label)).toEqual([
      t("databaseTemplate.emptyPage"),
      t("databaseTemplate.newTemplate"),
    ]);
  });

  it("marks the view's default template, and not a default that is no template of the source", async () => {
    const first = renderMenu({ view: { defaultTemplate: BRIEF.id } });
    await first.user.click(trigger());
    const menu = await openedMenu();
    expect(within(menu).getByRole("menuitem", { name: new RegExp(`^Brief${t("databaseTemplate.default")}$`, "u") })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Weekly" })).toBeInTheDocument();
    expect(within(menu).getAllByText(t("databaseTemplate.default"))).toHaveLength(1);
  });

  it("does not mark a default that is gone", async () => {
    const { user } = renderMenu({ view: { defaultTemplate: id(99) } });
    await user.click(trigger());
    const menu = await openedMenu();
    expect(within(menu).queryByText(t("databaseTemplate.default"))).toBeNull();
  });

  it("makes a page from the template that is chosen, with what the layout gave the page, and closes", async () => {
    const init = { values: [["stage", { kind: "text", value: "x" }]] as const };
    const { user, onCreatePage } = renderMenu({ init });
    await user.click(trigger());
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: "Brief" }));
    expect(onCreatePage).toHaveBeenCalledTimes(1);
    expect(onCreatePage).toHaveBeenCalledWith({ values: [["stage", { kind: "text", value: "x" }]], templateId: BRIEF.id });
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
  });

  it("makes an empty page whatever the default is", async () => {
    const { user, onCreatePage } = renderMenu({ view: { defaultTemplate: WEEKLY.id } });
    await user.click(trigger());
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: t("databaseTemplate.emptyPage") }));
    expect(onCreatePage).toHaveBeenCalledWith({ templateId: null });
  });

  it("opens a template's actions under its row: Edit, Set as default, Duplicate and Delete", async () => {
    const { user } = renderMenu();
    await user.click(trigger());
    const menu = await openRowOptions(user, "Weekly");
    const names = within(menu)
      .getAllByRole("menuitem")
      .map(label);
    expect(names).toEqual([
      "Weekly",
      t("databaseTemplate.edit"),
      t("databaseTemplate.setDefault"),
      t("databaseTemplate.duplicate"),
      t("databaseTemplate.delete"),
      "Brief",
      t("databaseTemplate.emptyPage"),
      t("databaseTemplate.newTemplate"),
    ]);
    // The other row is still folded; pressing the button again folds this one.
    expect(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Brief" }) })).toHaveAttribute("aria-expanded", "false");
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Weekly" }) }));
    expect(within(menu).queryByRole("menuitem", { name: t("databaseTemplate.edit") })).toBeNull();
  });

  it("sets the view's default template, and removes it for the template that is the default", async () => {
    const { user, onUpdateView } = renderMenu();
    await user.click(trigger());
    await openRowOptions(user, "Brief");
    await user.click(screen.getByRole("menuitem", { name: t("databaseTemplate.setDefault") }));
    expect(onUpdateView).toHaveBeenLastCalledWith({ defaultTemplate: BRIEF.id });
    // The menu stays open for it.
    expect(screen.getByRole("menu")).toBeInTheDocument();
  });

  it("offers Remove as default on the default template", async () => {
    const { user, onUpdateView } = renderMenu({ view: { defaultTemplate: WEEKLY.id } });
    await user.click(trigger());
    await openRowOptions(user, "Weekly");
    await user.click(screen.getByRole("menuitem", { name: t("databaseTemplate.removeDefault") }));
    expect(onUpdateView).toHaveBeenLastCalledWith({ defaultTemplate: null });
  });

  it("edits, duplicates and deletes a template through the block's actions, and closes", async () => {
    const { user, actions } = renderMenu();
    await user.click(trigger());
    await openRowOptions(user, "Weekly");
    await user.click(screen.getByRole("menuitem", { name: t("databaseTemplate.edit") }));
    expect(actions.edit).toHaveBeenCalledWith(WEEKLY);
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });

    await user.click(trigger());
    await openRowOptions(user, "Brief");
    await user.click(screen.getByRole("menuitem", { name: t("databaseTemplate.duplicate") }));
    expect(actions.duplicate).toHaveBeenCalledWith(BRIEF);

    await user.click(trigger());
    await openRowOptions(user, "Weekly");
    await user.click(screen.getByRole("menuitem", { name: t("databaseTemplate.delete") }));
    expect(actions.remove).toHaveBeenCalledWith(WEEKLY);
  });

  it("makes a new template through the block's action", async () => {
    const { user, actions, onCreatePage } = renderMenu();
    await user.click(trigger());
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: t("databaseTemplate.newTemplate") }));
    expect(actions.create).toHaveBeenCalledTimes(1);
    expect(onCreatePage).not.toHaveBeenCalled();
  });

  it("keeps a click in the menu from reaching what is around the button (a calendar day makes a page when clicked)", async () => {
    const { user, onOuterClick } = renderMenu();
    await user.click(trigger());
    onOuterClick.mockClear();
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: "Brief" }));
    expect(onOuterClick).not.toHaveBeenCalled();
  });

  it("takes the focus on its first template and moves between its items with the arrow keys", async () => {
    const { user } = renderMenu();
    await user.click(trigger());
    const menu = await openedMenu();
    const first = within(menu).getByRole("menuitem", { name: "Weekly" });
    await waitFor(() => {
      expect(first).toHaveFocus();
    });
    await user.keyboard("{ArrowDown}");
    expect(first).not.toHaveFocus();
    expect(menu).toContainElement(document.activeElement as HTMLElement);
    await user.keyboard("{ArrowUp}");
    expect(first).toHaveFocus();
  });
});

// ----- the whole app -------------------------------------------------------------------------------

const PROP = { stage: "de000000-0000-4000-8000-00000000f001", due: "de000000-0000-4000-8000-00000000f002" };
const TAG = { idea: "de000000-0000-4000-8000-00000000f101", project: "de000000-0000-4000-8000-00000000f102" };

const SORT = { by: "title", dir: "asc" };

function ticking(): () => number {
  let now = Date.UTC(2026, 9, 3, 12, 0, 0);
  return () => {
    now += 1000;
    return now;
  };
}

/** Page ids in creation order, the same in every build of the same seeds (so a block can name a template before the core exists). */
function counter(): () => string {
  let next = 0;
  return () => {
    next += 1;
    return pid(100_000 + next);
  };
}

interface Block {
  readonly layout?: string;
  /** The block's source (children of the page by default). */
  readonly source?: JsonValue;
  /** Extra keys of the first view of the block (`defaultTemplate`, `dateBy`, a dashboard's `widgets`). */
  readonly view?: { [key: string]: JsonValue };
  /** The views after the first, as stored. */
  readonly more?: JsonValue[];
}

function hubContent({ layout = "table", source = { kind: "children" }, view = {}, more = [] }: Block = {}): JsonValue {
  const stored = { id: "v1", name: "", layout, properties: [] as string[], sort: SORT, ...(layout === "board" ? { groupBy: PROP.stage } : {}), ...view };
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      {
        type: "pageList",
        attrs: { id: pid(2), source, layout, properties: [], sort: SORT, ...(layout === "board" ? { groupBy: PROP.stage } : {}), views: [stored, ...more] },
      },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

/**
 * Hub holds the block. Its children: Alpha (tagged idea), Beta, Weekly (a template, tagged idea and project, with a sub-page) and Brief (a template).
 * Notes is a top-level page.
 */
function space(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
    tagOptions: [
      { id: TAG.idea, name: "idea", color: "green" },
      { id: TAG.project, name: "project", color: "blue" },
    ],
    properties: [
      {
        id: PROP.stage,
        name: "Stage",
        kind: "select",
        options: [
          { name: "Review", color: "orange" },
          { name: "Draft", color: "gray" },
        ],
      },
      { id: PROP.due, name: "Due", kind: "date" },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Alpha", tags: ["idea"], properties: { Stage: "Draft", Due: { date: "2026-10-15" } } },
          { title: "Beta" },
          { title: "Weekly", template: true, icon: "📅", tags: ["idea", "project"], properties: { Stage: "Review" }, children: [{ title: "Weekly notes" }] },
          { title: "Brief", template: true, icon: "📝" },
        ],
      },
      { title: "Notes" },
    ],
  };
}

/** The data of a request type (nothing for one that has none). */
type Sent<T extends Request["type"]> = Extract<Request, { type: T }> extends { data: infer D } ? D : never;

interface RecordingCore {
  readonly transport: Transport;
  /** What the UI sent of one request type, in order. */
  readonly sent: <T extends Request["type"]>(type: T) => Sent<T>[];
  /** The core refuses the next requests of this type with `message` (a template that was trashed meanwhile). */
  readonly refuse: (type: Request["type"], message: string) => void;
}

/** The in-memory core as the app's transport, recording every request the UI sends and able to refuse one. */
function recordingCore(inner: MemoryTransport): RecordingCore {
  const requests: Request[] = [];
  const refusals = new Map<Request["type"], string>();
  const transport: Transport = {
    dispatch(request) {
      requests.push(request);
      const refusal = refusals.get(request.type);
      if (refusal !== undefined) {
        return Promise.reject(new CoreError({ code: "NotFound", message: refusal }));
      }
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
    refuse: (type, message) => {
      refusals.set(type, message);
    },
  };
}

/** The ids of the seeds are the same in every build with the same seeds and `counter`; the first build only tells them. */
async function openHub(block: (ids: Ids) => Block = () => ({})) {
  const build = (content: JsonValue) =>
    new MemoryTransport({ now: ticking(), workspaceName: "Research", newId: counter(), spaces: [space(content)] });
  const known = await idsOf(build(hubContent()));
  const memory = build(hubContent(block(known)));
  const core = recordingCore(memory);
  const app = renderWith(core.transport);
  await findMainNavigation();
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const ids = await idsOf(memory);
  const client = new CoreClient(memory);
  return { ...app, memory, core, editor, ids, client, spaceId: ids.space("Personal") };
}

type Ids = Awaited<ReturnType<typeof idsOf>>;

const blockElement = (): HTMLElement => screen.getByRole("group", { name: t("pageList.label") });

/** The attributes of the block as the editor holds them. */
function attrsOf(editor: Editor): Record<string, unknown> {
  let found: Record<string, unknown> = {};
  editor.state.doc.descendants((entry) => {
    if (entry.type.name === "pageList") {
      found = entry.attrs;
    }
    return true;
  });
  return found;
}

function docOf(editor: Editor) {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

async function openNewMenu(user: ReturnType<typeof userEvent.setup>, scope: HTMLElement = blockElement()): Promise<HTMLElement> {
  await user.click(within(scope).getByRole("button", { name: t("databaseTemplate.menu") }));
  return openedMenu();
}

const newPageButton = (scope: HTMLElement = blockElement()): HTMLElement => within(scope).getByRole("button", { name: t("pageList.newPage") });

describe("the rows of every layout", () => {
  it.each([
    ["list", () => within(blockElement()).getAllByRole("button", { name: /^(Alpha|Beta|Weekly|Brief)/u }).map((entry) => entry.textContent)],
    [
      "table",
      () =>
        Array.from(blockElement().querySelectorAll(".page-table-row[data-page] .page-table-title")).map((entry) => entry.textContent),
    ],
    [
      "gallery",
      () => Array.from(blockElement().querySelectorAll(".page-gallery-card .page-gallery-title")).map((entry) => entry.textContent),
    ],
  ])("hide the templates and the pages inside them in the %s", async (layout, titles) => {
    await openHub(() => ({ layout }));
    await waitFor(() => {
      expect(titles().length).toBeGreaterThan(0);
    });
    expect(titles().map((title) => title?.replace(/\s+/gu, ""))).toEqual(["Alpha", "Beta"]);
    expect(within(blockElement()).queryByText("Weekly")).toBeNull();
    expect(within(blockElement()).queryByText("Brief")).toBeNull();
  });

  it("hide the templates in the board", async () => {
    await openHub(() => ({ layout: "board" }));
    const columns = Array.from(blockElement().querySelectorAll(".page-board-card")).map((entry) => entry.textContent);
    expect(columns.some((text) => text?.includes("Alpha"))).toBe(true);
    expect(within(blockElement()).queryByText("Weekly")).toBeNull();
    expect(within(blockElement()).queryByText("Brief")).toBeNull();
  });

  it("hide the templates of a tag source, which the core lists among the tagged pages", async () => {
    await openHub(() => ({ source: { kind: "tag", optionId: TAG.idea } }));
    await waitFor(() => {
      expect(blockElement().querySelectorAll(".page-table-row[data-page]")).toHaveLength(1);
    });
    expect(within(blockElement()).getByText("Alpha")).toBeInTheDocument();
    expect(within(blockElement()).queryByText("Weekly")).toBeNull();
  });
});

describe("the menu next to New in a table", () => {
  it("lists the templates of the source (icon and title) and not its other pages", async () => {
    const { user } = await openHub();
    const menu = await openNewMenu(user);
    expect(within(menu).getAllByRole("menuitem").map(label)).toEqual([
      "Weekly",
      "Brief",
      t("databaseTemplate.emptyPage"),
      t("databaseTemplate.newTemplate"),
    ]);
    expect(label(within(menu).getByRole("menuitem", { name: /Weekly/u }))).toBe("Weekly");
    expect(menu.querySelector('[data-template] .page-icon')).toHaveTextContent("📅");
    expect(within(menu).queryByRole("menuitem", { name: "Alpha" })).toBeNull();
  });

  it("marks the default template of the view", async () => {
    const { user } = await openHub((ids) => ({ view: { defaultTemplate: ids.page("Brief") } }));
    const menu = await openNewMenu(user);
    const brief = within(menu).getByRole("menuitem", { name: new RegExp(`^Brief${t("databaseTemplate.default")}$`, "u") });
    expect(brief).toBeInTheDocument();
    expect(within(menu).getAllByText(t("databaseTemplate.default"))).toHaveLength(1);
  });

  it("is not offered in a read-only editor", async () => {
    const { editor } = await openHub();
    editor.setEditable(false);
    await waitFor(() => {
      expect(within(blockElement()).queryByRole("button", { name: t("pageList.newPage") })).toBeNull();
    });
    expect(within(blockElement()).queryByRole("button", { name: t("databaseTemplate.menu") })).toBeNull();
  });
});

describe("New", () => {
  it("makes an empty page when the view has no default template", async () => {
    const { user, core, ids, client, spaceId } = await openHub();
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([]);
    const hub = (await client.treeGet(spaceId)).roots.find((entry) => entry.page.id === ids.page("Hub"));
    expect(hub?.children.map((entry) => entry.page.title)).toEqual(["Alpha", "Beta", "Weekly", "Brief", ""]);
  });

  it("makes a page from the view's default template, under the page that holds the block", async () => {
    const { user, core, ids, client, spaceId } = await openHub((known) => ({ view: { defaultTemplate: known.page("Brief") } }));
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([{ templateId: ids.page("Brief"), parentId: ids.page("Hub") }]);
    expect(core.sent("PageCreate")).toEqual([]);
    // The core made the copy: the template's title, under the page, and the copy is a page, not a template.
    await waitFor(async () => {
      const hub = (await client.treeGet(spaceId)).roots.find((entry) => entry.page.id === ids.page("Hub"));
      expect(hub?.children.map((entry) => entry.page.title)).toEqual(["Alpha", "Beta", "Weekly", "Brief", "Brief"]);
    });
    const hub = (await client.treeGet(spaceId)).roots.find((entry) => entry.page.id === ids.page("Hub"));
    const made = hub?.children.at(-1)?.page;
    expect(made?.id).not.toBe(ids.page("Brief"));
    expect(made?.template).not.toBe(true);
    // ... and it is a row of the table, the template is not.
    await waitFor(() => {
      expect(Array.from(blockElement().querySelectorAll(".page-table-row[data-page] .page-table-title")).map((entry) => entry.textContent)).toEqual(["Alpha", "Beta", "Brief"]);
    });
  });

  it("opens the page it made like a row: in the side peek", async () => {
    const { user } = await openHub((known) => ({ view: { defaultTemplate: known.page("Brief") } }));
    await user.click(newPageButton());
    const peek = await screen.findByRole("complementary", { name: /Brief/u });
    expect(peek).toHaveAttribute("data-mode", "side");
  });

  it("makes an empty page when the default is not one of the source's templates (a page that is not a template, a page that is gone)", async () => {
    const first = await openHub((known) => ({ view: { defaultTemplate: known.page("Beta") } }));
    await first.user.click(newPageButton());
    await waitFor(() => {
      expect(first.core.sent("PageCreate")).toHaveLength(1);
    });
    expect(first.core.sent("PageCreateFromTemplate")).toEqual([]);
  });

  it("makes an empty page when the default points at a page that does not exist", async () => {
    const { user, core } = await openHub(() => ({ view: { defaultTemplate: pid(424_242) } }));
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([]);
  });

  it("makes a page from the template that is chosen in the menu, whatever the default", async () => {
    const { user, core, ids } = await openHub((known) => ({ view: { defaultTemplate: known.page("Brief") } }));
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: "Weekly" }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([{ templateId: ids.page("Weekly"), parentId: ids.page("Hub") }]);
  });

  it("makes an empty page from the menu's Empty page, even with a default template", async () => {
    const { user, core } = await openHub((known) => ({ view: { defaultTemplate: known.page("Brief") } }));
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.emptyPage") }));
    await waitFor(() => {
      expect(core.sent("PageCreate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([]);
  });

  it("gives the page of a tag source the tag, keeping the other tags of the template", async () => {
    const { user, core, ids, client, spaceId } = await openHub((known) => ({ source: { kind: "tag", optionId: TAG.idea }, view: { defaultTemplate: known.page("Weekly") } }));
    await waitFor(() => {
      expect(within(blockElement()).getByText("Alpha")).toBeInTheDocument();
    });
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    // The copy of the template (and its sub-page) has both tags, and the stage the template holds.
    const hubChildren = async () => (await client.treeGet(spaceId)).roots.find((entry) => entry.page.id === ids.page("Hub"))?.children ?? [];
    await waitFor(async () => {
      expect((await hubChildren()).map((entry) => entry.page.title)).toEqual(["Alpha", "Beta", "Weekly", "Brief", "Weekly"]);
    });
    const made = (await hubChildren()).at(-1);
    expect(made?.children.map((entry) => entry.page.title)).toEqual(["Weekly notes"]);
    expect(await client.pagePropertiesGet(made?.page.id ?? "")).toEqual(
      expect.arrayContaining([[TAGS_PROPERTY_ID, { kind: "multi_select", value: [TAG.idea, TAG.project] }]]),
    );
    expect(core.sent("PageCreateFromTemplate")).toEqual([
      {
        templateId: ids.page("Weekly"),
        parentId: ids.page("Hub"),
        values: [[TAGS_PROPERTY_ID, { kind: "multi_select", value: [TAG.idea, TAG.project] }]],
      },
    ]);
  });

  it("gives the page of a tag source the tag when it is made empty", async () => {
    const { user, core } = await openHub(() => ({ source: { kind: "tag", optionId: TAG.project } }));
    // The block asks the core for the tagged pages before it draws them; its New waits for nothing, but the page is the tag's.
    await waitFor(() => {
      expect(blockElement().querySelector(".page-table-grid, [role='grid']")).not.toBeNull();
    });
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PagePropertySet")).toHaveLength(1);
    });
    const [tag] = core.sent("PagePropertySet");
    expect(tag?.propertyId).toBe(TAGS_PROPERTY_ID);
    expect(tag?.value).toEqual({ kind: "multi_select", value: [TAG.project] });
    expect(core.sent("PageCreateFromTemplate")).toEqual([]);
  });

  it("makes the page of a board column from the default template with the column's value", async () => {
    const { user, core, ids, client, spaceId } = await openHub((known) => ({ layout: "board", view: { defaultTemplate: known.page("Brief") } }));
    const stage = (await client.propertySchemaGet(spaceId)).find((def) => def.id === PROP.stage);
    const review = stage?.options.find((option) => option.name === "Review");
    const column = screen.getByRole("group", { name: "Review" });
    await user.click(within(column).getByRole("button", { name: t("pageBoard.newIn", { name: "Review" }) }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([
      { templateId: ids.page("Brief"), parentId: ids.page("Hub"), values: [[PROP.stage, { kind: "select", value: review?.id }]] },
    ]);
    // The new page is in the column the button belongs to.
    await waitFor(() => {
      expect(within(column).getAllByText("Brief")).toHaveLength(1);
    });
  });

  it("offers the menu in a board column, which makes the page in that column from the template chosen", async () => {
    const { user, core, ids, client, spaceId } = await openHub(() => ({ layout: "board" }));
    const stage = (await client.propertySchemaGet(spaceId)).find((def) => def.id === PROP.stage);
    const draft = stage?.options.find((option) => option.name === "Draft");
    const column = screen.getByRole("group", { name: "Draft" });
    await user.click(within(column).getByRole("button", { name: t("databaseTemplate.menuIn", { name: "Draft" }) }));
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: "Weekly" }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([
      { templateId: ids.page("Weekly"), parentId: ids.page("Hub"), values: [[PROP.stage, { kind: "select", value: draft?.id }]] },
    ]);
  });

  it("makes the page of a calendar day from the default template, dated that day", async () => {
    const { user, core, ids } = await openHub((known) => ({ layout: "calendar", view: { dateBy: PROP.due, defaultTemplate: known.page("Brief") } }));
    const day = "2026-10-20";
    const cell = blockElement().querySelector<HTMLElement>(`[data-day="${day}"]`);
    expect(cell).not.toBeNull();
    await user.click(within(cell as HTMLElement).getByRole("button", { name: t("pageCalendar.newOn", { date: fullDate(day, activeLocale()) }) }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([
      { templateId: ids.page("Brief"), parentId: ids.page("Hub"), values: [[PROP.due, { kind: "date", value: { date: day } }]] },
    ]);
  });

  it("offers the menu on a calendar day, and the click that chooses does not also make a page for the day", async () => {
    const { user, core, ids } = await openHub(() => ({ layout: "calendar", view: { dateBy: PROP.due } }));
    const day = "2026-10-21";
    const cell = blockElement().querySelector<HTMLElement>(`[data-day="${day}"]`) as HTMLElement;
    await user.click(within(cell).getByRole("button", { name: t("databaseTemplate.menuOn", { date: fullDate(day, activeLocale()) }) }));
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: "Weekly" }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreate")).toEqual([]);
    expect(core.sent("PageCreateFromTemplate")).toEqual([
      { templateId: ids.page("Weekly"), parentId: ids.page("Hub"), values: [[PROP.due, { kind: "date", value: { date: day } }]] },
    ]);
  });

  it("makes the page of the gallery's card from the default template, and offers the menu on the card", async () => {
    const { user, core, ids } = await openHub((known) => ({ layout: "gallery", view: { defaultTemplate: known.page("Weekly") } }));
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")[0]?.templateId).toBe(ids.page("Weekly"));
    const menu = await openNewMenu(user);
    expect(within(menu).getByRole("menuitem", { name: new RegExp(`^Weekly${t("databaseTemplate.default")}$`, "u") })).toBeInTheDocument();
  });

  it("makes the page of the list's New from the default template", async () => {
    const { user, core, ids } = await openHub((known) => ({ layout: "list", view: { defaultTemplate: known.page("Brief") } }));
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")[0]?.templateId).toBe(ids.page("Brief"));
  });

  it("says why a page could not be made from a template, and the controls come back", async () => {
    const { user, core } = await openHub((known) => ({ view: { defaultTemplate: known.page("Brief") } }));
    // The template was trashed meanwhile (on another device): the core refuses, and the block says so.
    core.refuse("PageCreateFromTemplate", "The template is gone");
    await user.click(newPageButton());
    expect(await within(blockElement()).findByRole("alert")).toHaveTextContent(t("pageList.error", { message: "The template is gone" }));
    await waitFor(() => {
      expect(newPageButton()).toBeEnabled();
    });
    expect(core.sent("PageCreate")).toEqual([]);
  });
});

describe("Set as default and Remove as default", () => {
  it("writes the view's defaultTemplate with the other parts of the view as they were, and New uses it", async () => {
    const { user, editor, core, ids } = await openHub();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Weekly" }) }));
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.setDefault") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[0]?.defaultTemplate).toBe(ids.page("Weekly"));
    });
    // One write, `views` and the mirror of the first view; the layout and the sort are what they were.
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    expect(parseViews(attrsOf(editor))[0]).toMatchObject({ id: "v1", layout: "table", sort: SORT, defaultTemplate: ids.page("Weekly") });
    expect(attrsOf(editor).layout).toBe("table");

    // The menu shows the mark, and now says Remove as default for the row.
    expect(within(screen.getByRole("menu")).getByText(t("databaseTemplate.default"))).toBeInTheDocument();
    expect(within(screen.getByRole("menu")).getByRole("menuitem", { name: t("databaseTemplate.removeDefault") })).toBeInTheDocument();
    await user.keyboard("{Escape}");

    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")[0]?.templateId).toBe(ids.page("Weekly"));
  });

  it("removes the key, and New makes an empty page again", async () => {
    const { user, editor, core } = await openHub((known) => ({ view: { defaultTemplate: known.page("Brief") } }));
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Brief" }) }));
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.removeDefault") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[0]?.defaultTemplate).toBeUndefined();
    });
    const stored = (attrsOf(editor).views as { [key: string]: unknown }[])[0];
    expect(stored).not.toHaveProperty("defaultTemplate");
    await user.keyboard("{Escape}");
    await user.click(newPageButton());
    await waitFor(() => {
      expect(core.sent("PageCreate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")).toEqual([]);
  });

  it("changes the default of the view that is shown, not the other views of the block", async () => {
    const { user, editor, ids } = await openHub();
    // A second view, added through the tab strip's own control, becomes the one that is shown.
    await user.click(screen.getByRole("button", { name: t("pageList.views.add") }));
    const adding = await screen.findByRole("menu", { name: t("pageList.views.addMenu") });
    await user.click(within(adding).getAllByRole("menuitem")[0] as HTMLElement);
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))).toHaveLength(2);
    });
    const second = parseViews(attrsOf(editor))[1];
    const picker = await openNewMenu(user);
    await user.click(within(picker).getByRole("button", { name: t("databaseTemplate.options", { title: "Brief" }) }));
    await user.click(within(picker).getByRole("menuitem", { name: t("databaseTemplate.setDefault") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor)).find((view) => view.id === second?.id)?.defaultTemplate).toBe(ids.page("Brief"));
    });
    expect(parseViews(attrsOf(editor))[0]?.defaultTemplate).toBeUndefined();
  });
});

describe("the actions of a template row", () => {
  it("Edit opens the template in the side peek, whatever the view says about where pages open", async () => {
    const { user, ids } = await openHub(() => ({ view: { openIn: "page" } }));
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Weekly" }) }));
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.edit") }));
    const peek = await screen.findByRole("complementary", { name: /Weekly/u });
    expect(peek).toHaveAttribute("data-mode", "side");
    // The page that holds the block is still the page in front.
    expect(screen.getByRole("heading", { level: 1, name: "Hub" })).toBeInTheDocument();
    expect(ids.page("Weekly")).not.toBe("");
  });

  it("Duplicate copies the template and leaves the page in front where it is", async () => {
    const { user, core, ids } = await openHub();
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Brief" }) }));
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.duplicate") }));
    await waitFor(() => {
      expect(core.sent("PageDuplicate")).toEqual([{ pageId: ids.page("Brief") }]);
    });
    expect(screen.getByRole("heading", { level: 1, name: "Hub" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: /Brief/u })).toBeNull();
  });

  it("Delete moves the template to the trash", async () => {
    const { user, core, ids } = await openHub();
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Brief" }) }));
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.delete") }));
    await waitFor(() => {
      expect(core.sent("PageTrash")).toEqual([{ pageId: ids.page("Brief") }]);
    });
    await waitFor(() => {
      expect(within(blockElement()).queryByRole("button", { name: t("databaseTemplate.menu") })).toBeInTheDocument();
    });
  });
});

describe("New template", () => {
  it("makes a page under the page that holds the block, makes it a template and opens it in the side peek", async () => {
    const { user, core, ids, client, spaceId } = await openHub();
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.newTemplate") }));
    await waitFor(() => {
      expect(core.sent("PageSetTemplate")).toHaveLength(1);
    });
    const hub = (await client.treeGet(spaceId)).roots.find((entry) => entry.page.id === ids.page("Hub"));
    const created = hub?.children.at(-1)?.page;
    expect(created?.title).toBe(t("databaseTemplate.templateTitle"));
    expect(core.sent("PageSetTemplate")).toEqual([{ pageId: created?.id, template: true }]);
    const peek = await screen.findByRole("complementary", { name: new RegExp(t("databaseTemplate.templateTitle"), "u") });
    expect(peek).toHaveAttribute("data-mode", "side");
    // It is a template: not a row, and the next menu lists it.
    await waitFor(() => {
      expect(within(blockElement()).queryByText(t("databaseTemplate.templateTitle"))).toBeNull();
    });
  });

  it("gives the template the tag of a tag source before it is made a template", async () => {
    const { user, core } = await openHub(() => ({ source: { kind: "tag", optionId: TAG.idea } }));
    await waitFor(() => {
      expect(within(blockElement()).getByText("Alpha")).toBeInTheDocument();
    });
    const menu = await openNewMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.newTemplate") }));
    await waitFor(() => {
      expect(core.sent("PageSetTemplate")).toHaveLength(1);
    });
    const [tag] = core.sent("PagePropertySet");
    expect(tag?.propertyId).toBe(TAGS_PROPERTY_ID);
    expect(tag?.value).toEqual({ kind: "multi_select", value: [TAG.idea] });
    expect(tag?.pageId).toBe(core.sent("PageSetTemplate")[0]?.pageId);
  });
});

// ----- a dashboard's embedded view -----------------------------------------------------------------

describe("a dashboard in a block", () => {
  const embedded = (defaultTemplate?: PageId): Block => ({
    layout: "dashboard",
    view: { widgets: [{ id: "a0000001", kind: "view", title: "Tasks", w: 4, h: "l", viewId: "v2" }] },
    more: [{ id: "v2", name: "Tasks", layout: "table", properties: [], sort: SORT, ...(defaultTemplate === undefined ? {} : { defaultTemplate }) }],
  });
  const tasks = (): HTMLElement => screen.getByRole("group", { name: "Tasks" });

  it("draws the rows of an embedded view without the templates", async () => {
    await openHub(() => embedded());
    await waitFor(() => {
      expect(tasks().querySelectorAll(".page-table-row[data-page]")).toHaveLength(2);
    });
    expect(Array.from(tasks().querySelectorAll(".page-table-row[data-page] .page-table-title")).map((entry) => entry.textContent)).toEqual(["Alpha", "Beta"]);
  });

  it("makes the page of an embedded view's New from that view's default template", async () => {
    const { user, core, ids } = await openHub((known) => embedded(known.page("Weekly")));
    await user.click(within(tasks()).getByRole("button", { name: t("pageList.newPage") }));
    await waitFor(() => {
      expect(core.sent("PageCreateFromTemplate")).toHaveLength(1);
    });
    expect(core.sent("PageCreateFromTemplate")[0]?.templateId).toBe(ids.page("Weekly"));
  });

  it("writes the default template of the embedded view, not of the dashboard", async () => {
    const { user, editor, ids } = await openHub(() => embedded());
    await user.click(within(tasks()).getByRole("button", { name: t("databaseTemplate.menu") }));
    const menu = await openedMenu();
    await user.click(within(menu).getByRole("button", { name: t("databaseTemplate.options", { title: "Brief" }) }));
    await user.click(within(menu).getByRole("menuitem", { name: t("databaseTemplate.setDefault") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor)).find((view) => view.id === "v2")?.defaultTemplate).toBe(ids.page("Brief"));
    });
    expect(parseViews(attrsOf(editor))[0]?.defaultTemplate).toBeUndefined();
  });
});

describe("a dashboard's embedded view", () => {
  const SORTED = { by: "title", dir: "asc" };

  function views(tasksExtra: { [key: string]: unknown }, dashboardExtra: { [key: string]: unknown } = {}) {
    const dashboard = parseView({
      id: "dash",
      name: "",
      layout: "dashboard",
      properties: [],
      sort: SORTED,
      widgets: [{ id: "a0000001", kind: "view", title: "Tasks", w: 4, h: "l", viewId: "tasks" }],
      ...dashboardExtra,
    });
    const tasks = parseView({ id: "tasks", name: "All tasks", layout: "table", properties: [], sort: SORTED, ...tasksExtra });
    if (dashboard === null || tasks === null) {
      throw new Error("the views did not read");
    }
    return [dashboard, tasks] as const;
  }

  function setupDashboard(tasksExtra: { [key: string]: unknown }, dashboardExtra: { [key: string]: unknown } = {}) {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [{ name: "Personal", pages: [{ title: "Hub" }] }] });
    const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
    store.setState({ people: { [SPACE]: [] } });
    const [dashboard, tasks] = views(tasksExtra, dashboardExtra);
    const actions = fakeActions();
    const onCreatePage = vi.fn();
    const onUpdateOtherView = vi.fn();
    const pages = [page(1, "Alpha"), page(2, "Beta")];
    const props: PageListLayoutProps = {
      rows: pages,
      view: dashboard,
      schema: [],
      columns: [],
      spaceId: SPACE,
      editable: true,
      today: "2026-10-03",
      onOpen: vi.fn(),
      onUpdateView: vi.fn(),
      onCreatePage,
      creating: false,
      views: [dashboard, tasks],
      sourcePages: pages,
      onUpdateOtherView,
      onActivateView: vi.fn(),
    };
    const user = userEvent.setup();
    render(
      <StoreContext value={store}>
        <DashboardHostContext value={{ layoutDef, openInView: vi.fn() }}>
          <DatabaseTemplatesContext value={{ templates: [WEEKLY, BRIEF], actions }}>
            <PageDashboardView {...props} />
          </DatabaseTemplatesContext>
        </DashboardHostContext>
      </StoreContext>,
    );
    return { user, actions, onCreatePage, onUpdateOtherView };
  }

  const tasksFrame = (): HTMLElement => screen.getByRole("group", { name: "Tasks" });

  it("makes a page from its own view's default template", async () => {
    const { user, onCreatePage } = setupDashboard({ defaultTemplate: BRIEF.id });
    await user.click(within(tasksFrame()).getByRole("button", { name: t("pageList.newPage") }));
    expect(onCreatePage).toHaveBeenCalledTimes(1);
    expect(onCreatePage).toHaveBeenCalledWith({ templateId: BRIEF.id });
  });

  it("makes an empty page when its own view has no default, whatever the dashboard's own default is", async () => {
    const { user, onCreatePage } = setupDashboard({}, { defaultTemplate: WEEKLY.id });
    await user.click(within(tasksFrame()).getByRole("button", { name: t("pageList.newPage") }));
    expect(onCreatePage).toHaveBeenCalledWith({ templateId: null });
  });

  it("makes an empty page when its default is not one of the templates of the source", async () => {
    const { user, onCreatePage } = setupDashboard({ defaultTemplate: id(99) });
    await user.click(within(tasksFrame()).getByRole("button", { name: t("pageList.newPage") }));
    expect(onCreatePage).toHaveBeenCalledWith({ templateId: null });
  });

  it("offers the menu: a template, an empty page, and Set as default writes the embedded view's own key", async () => {
    const { user, onCreatePage, onUpdateOtherView } = setupDashboard({ defaultTemplate: BRIEF.id });
    await user.click(within(tasksFrame()).getByRole("button", { name: t("databaseTemplate.menu") }));
    const menu = await openedMenu();
    await user.click(within(menu).getByRole("menuitem", { name: "Weekly" }));
    expect(onCreatePage).toHaveBeenLastCalledWith({ templateId: WEEKLY.id });

    await user.click(within(tasksFrame()).getByRole("button", { name: t("databaseTemplate.menu") }));
    await user.click(within(await openedMenu()).getByRole("menuitem", { name: t("databaseTemplate.emptyPage") }));
    expect(onCreatePage).toHaveBeenLastCalledWith({ templateId: null });

    await user.click(within(tasksFrame()).getByRole("button", { name: t("databaseTemplate.menu") }));
    const again = await openedMenu();
    await user.click(within(again).getByRole("button", { name: t("databaseTemplate.options", { title: "Weekly" }) }));
    await user.click(within(again).getByRole("menuitem", { name: t("databaseTemplate.setDefault") }));
    expect(onUpdateOtherView).toHaveBeenLastCalledWith("tasks", { defaultTemplate: WEEKLY.id });
  });
});
