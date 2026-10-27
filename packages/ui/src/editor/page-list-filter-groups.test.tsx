// The filter-group editor of a view (ADR-0054, `docs/architecture/database-views.md` section 3): the top "Match all / any" control,
// filter rows, "Add filter group" (a box with its own all/any control, its rows, "Add a filter to group n" and "Remove group"),
// one level only, the limits (50 filters, 10 groups), and what the tree does to the rows through a block: AND, OR, groups, an
// item that cannot be applied being skipped, and what is written (a flat AND list stays in `filters` for a block without
// `views`, anything else writes `views` and the mirror).

import { CoreClient, MemoryTransport, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { JsonValue, PropertyDef, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { LoroDoc } from "loro-crdt";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { pid } from "../test/editor";
import { openPage, renderPages } from "../test/page";
import { loroSyncPluginKey } from "./loro-binding";
import { MAX_FILTER_GROUPS, MAX_FILTERS, normaliseFilterGroup } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import { parseViews } from "./page-list-views";
import { PageListFilters } from "./PageListFilters";

afterEach(() => {
  vi.restoreAllMocks();
});

// ----- the editor on its own ---------------------------------------------------------------

const def = (id: string, name: string, kind: PropertyDef["kind"], options: PropertyDef["options"] = []): PropertyDef => ({
  id,
  name,
  kind,
  options,
  createdAt: 0,
});
const DEFS: PropertyDef[] = [
  def("p-priority", "Priority", "number"),
  def("p-done", "Done", "checkbox"),
  def("p-stage", "Stage", "select", [
    { id: "o-draft", name: "Draft", color: "gray" },
    { id: "o-review", name: "Review", color: "orange" },
  ]),
];
const isSet = { propertyId: "p-priority", op: "isSet" };
const checked = { propertyId: "p-done", op: "checked" };

/** The panel over a group the test holds in state (as the toolbar does), reporting each change. */
function Panel({ initial, onChange }: { initial: FilterGroup; onChange: (group: FilterGroup) => void }) {
  const [group, setGroup] = useState(initial);
  const anchor = document.body.appendChild(document.createElement("button"));
  return (
    <PageListFilters
      anchor={{ x: 0, y: 0 }}
      anchorElement={anchor}
      defs={DEFS}
      group={group}
      onChange={(next) => {
        setGroup(next);
        onChange(next);
      }}
      onClose={() => undefined}
    />
  );
}

function openPanel(initial: FilterGroup = { op: "and", items: [] }) {
  const memory = new MemoryTransport({ now: () => 0, workspaceName: "W", spaces: [{ name: "S", pages: [] }] });
  const store = createAppStore(new CoreClient(memory));
  const stop = store.getState().start();
  const onChange = vi.fn<(group: FilterGroup) => void>();
  const user = userEvent.setup();
  render(
    <StoreContext value={store}>
      <Panel initial={initial} onChange={onChange} />
    </StoreContext>,
  );
  return { user, onChange, stop, last: (): FilterGroup | undefined => onChange.mock.lastCall?.[0] };
}

const panel = (): HTMLElement => screen.getByRole("dialog", { name: t("pageList.filter.label") });
const addFilter = (): HTMLElement => within(panel()).getByRole("button", { name: t("pageList.filter.add") });
const addGroup = (): HTMLElement => within(panel()).getByRole("button", { name: t("pageList.filter.addGroup") });
const groupBox = (n: number): HTMLElement => within(panel()).getByRole("group", { name: t("pageList.filter.group", { n }) });

describe("the filter editor", () => {
  it("starts empty: no match control, the empty text, and the two add buttons", () => {
    const { stop } = openPanel();
    expect(within(panel()).getByText(t("pageList.filter.none"))).toBeInTheDocument();
    expect(within(panel()).queryByRole("combobox", { name: t("pageList.filter.match") })).not.toBeInTheDocument();
    expect(addFilter()).toBeEnabled();
    expect(addGroup()).toBeEnabled();
    stop();
  });

  it("shows the top match control from two items on, all by default, and writes the operator", async () => {
    const { user, last, stop } = openPanel({ op: "and", items: [isSet] });
    expect(within(panel()).queryByRole("combobox", { name: t("pageList.filter.match") })).not.toBeInTheDocument();
    await user.click(addFilter());
    const match = within(panel()).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.filter.match") });
    expect(match).toHaveValue("and");
    expect(Array.from(match.options).map((option) => option.textContent)).toEqual([t("pageList.filter.match.all"), t("pageList.filter.match.any")]);
    await user.selectOptions(match, t("pageList.filter.match.any"));
    expect(last()).toMatchObject({ op: "or" });
    expect(last()?.items).toHaveLength(2);
    expect(match).toHaveValue("or");
    await user.selectOptions(match, t("pageList.filter.match.all"));
    expect(last()?.op).toBe("and");
    stop();
  });

  it("keeps the top match control while the tree is \"any\", even with one item, so that it can be changed back", () => {
    const { stop } = openPanel({ op: "or", items: [isSet] });
    expect(within(panel()).getByRole("combobox", { name: t("pageList.filter.match") })).toHaveValue("or");
    stop();
  });

  it("numbers the filter rows among the top-level filters", () => {
    const { stop } = openPanel({ op: "and", items: [isSet, { op: "or", items: [checked] }, checked] });
    expect(within(panel()).getByRole("group", { name: t("pageList.filter.row", { n: 1 }) })).toBeInTheDocument();
    expect(within(panel()).getByRole("group", { name: t("pageList.filter.row", { n: 2 }) })).toBeInTheDocument();
    expect(within(panel()).queryByRole("group", { name: t("pageList.filter.row", { n: 3 }) })).not.toBeInTheDocument();
    stop();
  });

  it("adds a filter group: a box with the opposite operator, its own match control, an empty text and its own add button", async () => {
    const { user, last, stop } = openPanel({ op: "and", items: [isSet] });
    await user.click(addGroup());
    expect(last()).toEqual({ op: "and", items: [isSet, { op: "or", items: [] }] });
    const box = groupBox(1);
    expect(within(box).getByRole("combobox", { name: t("pageList.filter.groupMatch", { n: 1 }) })).toHaveValue("or");
    expect(within(box).getByText(t("pageList.filter.groupEmpty"))).toBeInTheDocument();
    expect(within(box).getByRole("button", { name: t("pageList.filter.removeGroup") })).toBeInTheDocument();

    await user.click(within(box).getByRole("button", { name: t("pageList.filter.addToGroup", { n: 1 }) }));
    expect(last()?.items[1]).toEqual({ op: "or", items: [{ propertyId: "p-priority", op: "equals" }] });
    const row = within(groupBox(1)).getByRole("group", { name: t("pageList.filter.rowInGroup", { n: 1, group: 1 }) });
    expect(within(groupBox(1)).queryByText(t("pageList.filter.groupEmpty"))).not.toBeInTheDocument();
    expect(within(row).getByRole("combobox", { name: t("pageList.filter.property") })).toBeInTheDocument();
    stop();
  });

  it("changes the operator of a group alone, and edits and removes the rows of a group", async () => {
    const { user, last, stop } = openPanel({ op: "and", items: [{ op: "or", items: [isSet, checked] }] });
    const box = groupBox(1);
    await user.selectOptions(within(box).getByRole("combobox", { name: t("pageList.filter.groupMatch", { n: 1 }) }), t("pageList.filter.match.all"));
    expect(last()).toEqual({ op: "and", items: [{ op: "and", items: [isSet, checked] }] });
    const row = within(box).getByRole("group", { name: t("pageList.filter.rowInGroup", { n: 2, group: 1 }) });
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Priority");
    expect(last()?.items[0]).toEqual({ op: "and", items: [isSet, { propertyId: "p-priority", op: "equals" }] });
    await user.click(within(row).getByRole("button", { name: t("pageList.filter.remove") }));
    expect(last()?.items[0]).toEqual({ op: "and", items: [isSet] });
    // Removing the last filter of a group leaves the empty group, which has its own remove button.
    await user.click(within(groupBox(1)).getByRole("button", { name: t("pageList.filter.remove") }));
    expect(last()).toEqual({ op: "and", items: [{ op: "and", items: [] }] });
    expect(within(groupBox(1)).getByText(t("pageList.filter.groupEmpty"))).toBeInTheDocument();
    stop();
  });

  it("removes a group with its filters", async () => {
    const { user, last, stop } = openPanel({ op: "and", items: [isSet, { op: "or", items: [checked] }] });
    await user.click(within(groupBox(1)).getByRole("button", { name: t("pageList.filter.removeGroup") }));
    expect(last()).toEqual({ op: "and", items: [isSet] });
    expect(within(panel()).queryByRole("group", { name: t("pageList.filter.group", { n: 1 }) })).not.toBeInTheDocument();
    stop();
  });

  it("goes back to the default tree when the last item is removed, whatever the operator", async () => {
    const { user, last, stop } = openPanel({ op: "or", items: [{ op: "and", items: [] }] });
    await user.click(within(groupBox(1)).getByRole("button", { name: t("pageList.filter.removeGroup") }));
    expect(last()).toEqual({ op: "and", items: [] });
    stop();
  });

  it("goes one level only: no \"Add filter group\" inside a group, and the group buttons are those of the group", () => {
    const { stop } = openPanel({ op: "and", items: [{ op: "or", items: [isSet] }] });
    expect(within(groupBox(1)).queryByRole("button", { name: t("pageList.filter.addGroup") })).not.toBeInTheDocument();
    expect(within(panel()).getAllByRole("button", { name: t("pageList.filter.addGroup") })).toHaveLength(1);
    expect(within(groupBox(1)).queryByRole("group", { name: t("pageList.filter.group", { n: 1 }) })).not.toBeInTheDocument();
    stop();
  });

  it("stops at 50 filters in all, counting the groups' ones, and says so", async () => {
    const filters = Array.from({ length: MAX_FILTERS - 3 }, () => isSet);
    const { user, last, stop } = openPanel({ op: "and", items: [...filters, { op: "or", items: [checked, checked] }] });
    expect(within(panel()).queryByText(t("pageList.filter.limit", { max: MAX_FILTERS }))).not.toBeInTheDocument();
    expect(addFilter()).toBeEnabled();
    await user.click(addFilter());
    expect(addFilter()).toBeDisabled();
    expect(within(groupBox(1)).getByRole("button", { name: t("pageList.filter.addToGroup", { n: 1 }) })).toBeDisabled();
    expect(within(panel()).getByText(t("pageList.filter.limit", { max: MAX_FILTERS }))).toBeInTheDocument();
    expect(last()?.items).toHaveLength(MAX_FILTERS - 3 + 1 + 1);
    // A group can still be added (it holds no filter): the limit is of filters.
    expect(addGroup()).toBeEnabled();
    stop();
  });

  it("stops at 10 groups and says so", async () => {
    const groups = Array.from({ length: MAX_FILTER_GROUPS - 1 }, (): FilterGroup => ({ op: "or", items: [] }));
    const { user, last, stop } = openPanel({ op: "and", items: groups });
    expect(addGroup()).toBeEnabled();
    await user.click(addGroup());
    expect(last()?.items).toHaveLength(MAX_FILTER_GROUPS);
    expect(addGroup()).toBeDisabled();
    expect(within(panel()).getByText(t("pageList.filter.groupsLimit", { max: MAX_FILTER_GROUPS }))).toBeInTheDocument();
    expect(addFilter()).toBeEnabled();
    stop();
  });
});

// ----- through a block -----------------------------------------------------------------------

const PROP = {
  priority: "de000000-0000-4000-8000-00000000c004",
  summary: "de000000-0000-4000-8000-00000000c005",
};

const eq = (value: number): JsonValue => ({ propertyId: PROP.priority, op: "equals", value });
const half: JsonValue = { propertyId: PROP.priority, op: "equals" }; // no value yet: it cannot be applied

function viewWith(filter: JsonValue): JsonValue {
  return { id: "k1", name: "", layout: "list", properties: [], sort: { by: "title", dir: "asc" }, filter };
}

function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      { type: "pageList", attrs: { id: pid(2), source: { kind: "children" }, layout: "list", properties: [], sort: { by: "title", dir: "asc" }, ...list } },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

function space(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
    properties: [
      { id: PROP.priority, name: "Priority", kind: "number" },
      { id: PROP.summary, name: "Summary", kind: "text" },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Alpha", properties: { Priority: 3, Summary: "A note" } },
          { title: "Beta", properties: { Priority: 10 } },
          { title: "Gamma" },
          { title: "Delta", properties: { Priority: 2 } },
        ],
      },
    ],
  };
}

async function openHub(content: JsonValue) {
  const app = await renderPages([space(content)]);
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  return { ...app, editor };
}

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

function titles(): string[] {
  return Array.from(block().querySelectorAll(".page-list-row .page-list-name")).map((name) => name.textContent ?? "");
}

function attrsOf(editor: Editor): Record<string, unknown> {
  let found: Record<string, unknown> = {};
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageList") {
      found = node.attrs;
    }
    return true;
  });
  return found;
}

function docOf(editor: Editor): LoroDoc {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

function toolbar(): HTMLElement {
  return within(block()).getByRole("toolbar", { name: t("pageList.toolbar") });
}

describe("the filter tree on the rows of a block", () => {
  it("combines the filters of the top group with OR", async () => {
    await openHub(hubContent({ views: [viewWith({ op: "or", items: [eq(3), eq(10)] })] }));
    expect(titles()).toEqual(["Alpha", "Beta"]);
    expect(within(toolbar()).getByRole("button", { name: /^Filter/u })).toHaveTextContent("Filter 2");
  });

  it("combines a group with the filters around it: A and (B or C)", async () => {
    await openHub(
      hubContent({ views: [viewWith({ op: "and", items: [{ propertyId: PROP.priority, op: "isSet" }, { op: "or", items: [eq(2), eq(3)] }] })] }),
    );
    expect(titles()).toEqual(["Alpha", "Delta"]);
    // The count is of all the filters, the group's included.
    expect(within(toolbar()).getByRole("button", { name: /^Filter/u })).toHaveTextContent("Filter 3");
  });

  it("skips a filter that cannot be applied inside an OR instead of letting every page through", async () => {
    await openHub(hubContent({ views: [viewWith({ op: "or", items: [eq(3), half] })] }));
    expect(titles()).toEqual(["Alpha"]);
  });

  it("skips an empty group and a group of half-made filters", async () => {
    await openHub(hubContent({ views: [viewWith({ op: "and", items: [eq(3), { op: "or", items: [] }, { op: "or", items: [half] }] })] }));
    expect(titles()).toEqual(["Alpha"]);
  });

  it("hides nothing for a tree with nothing it can apply", async () => {
    await openHub(hubContent({ views: [viewWith({ op: "or", items: [half, { op: "and", items: [half] }] })] }));
    expect(titles()).toEqual(["Alpha", "Beta", "Delta", "Gamma"]);
  });
});

describe("what the editor writes", () => {
  async function openFilters(content: JsonValue) {
    const app = await openHub(content);
    await app.user.click(within(toolbar()).getByRole("button", { name: /^Filter/u }));
    const dialog = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    return { ...app, dialog };
  }

  it("keeps a flat AND list in the legacy `filters` of a block without views, one change", async () => {
    const { user, editor, dialog } = await openFilters(hubContent());
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(within(dialog).getByRole("button", { name: t("pageList.filter.add") }));
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: TAGS_PROPERTY_ID, op: "hasOption" }]);
    });
    expect(attrsOf(editor).views).toBeNull();
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    await user.click(within(dialog).getByRole("button", { name: t("pageList.filter.add") }));
    await waitFor(() => {
      expect(attrsOf(editor).filters).toHaveLength(2);
    });
    expect(attrsOf(editor).views).toBeNull(); // still expressible: AND, flat
  });

  it("upgrades the block when a group is added: `views` with the tree, and the mirror carries the plain filters", async () => {
    const { user, editor, dialog } = await openFilters(hubContent({ filters: [{ propertyId: PROP.priority, op: "isSet" }] }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(within(dialog).getByRole("button", { name: t("pageList.filter.addGroup") }));
    await waitFor(() => {
      expect(Array.isArray(attrsOf(editor).views)).toBe(true);
    });
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    const [view] = parseViews(attrsOf(editor));
    expect(view?.filter).toEqual({ op: "and", items: [{ propertyId: PROP.priority, op: "isSet" }, { op: "or", items: [] }] });
    expect(attrsOf(editor).filters).toEqual([{ propertyId: PROP.priority, op: "isSet" }]);
    // A filter in the group (the first property of the space, Tags, as a new row starts): one more change of the view.
    await user.click(within(dialog).getByRole("button", { name: t("pageList.filter.addToGroup", { n: 1 }) }));
    await waitFor(() => {
      expect(normaliseFilterGroup(parseViews(attrsOf(editor))[0]?.filter).items[1]).toEqual({ op: "or", items: [{ propertyId: TAGS_PROPERTY_ID, op: "hasOption" }] });
    });
    expect(attrsOf(editor).filters).toEqual([{ propertyId: PROP.priority, op: "isSet" }]);
  });

  it("writes `views` when the top operator becomes \"any\", and an older build is then told no filters (it shows every page)", async () => {
    const { user, editor, dialog } = await openFilters(hubContent({ filters: [eq(3), eq(10)] }));
    expect(titles()).toEqual([]); // 3 and 10 at once: no page
    await user.selectOptions(within(dialog).getByRole("combobox", { name: t("pageList.filter.match") }), t("pageList.filter.match.any"));
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "Beta"]);
    });
    const [view] = parseViews(attrsOf(editor));
    expect(view?.filter.op).toBe("or");
    expect(view?.filter.items).toHaveLength(2);
    expect(attrsOf(editor).filters).toBeNull();
  });

  it("goes through the view once the block has views: a filter typed in a group is written when the field is left", async () => {
    const { user, editor, dialog } = await openFilters(hubContent({ views: [viewWith({ op: "and", items: [{ op: "or", items: [half] }] })] }));
    const doc = docOf(editor);
    const row = within(dialog).getByRole("group", { name: t("pageList.filter.rowInGroup", { n: 1, group: 1 }) });
    const number = within(row).getByRole("textbox", { name: t("pageList.filter.number") });
    const before = doc.oplogVersion();
    await user.type(number, "3");
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(0); // typing writes nothing
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha"]);
    });
    expect(parseViews(attrsOf(editor))[0]?.filter).toEqual({ op: "and", items: [{ op: "or", items: [eq(3)] }] });
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
  });

  it("drops the tree back to the legacy attributes' emptiness when everything is removed from a block with views", async () => {
    const { user, editor, dialog } = await openFilters(hubContent({ views: [viewWith({ op: "or", items: [eq(3)] })] }));
    await user.click(within(dialog).getByRole("button", { name: t("pageList.filter.remove") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "Beta", "Delta", "Gamma"]);
    });
    const [view] = parseViews(attrsOf(editor));
    expect(view?.filter).toEqual({ op: "and", items: [] });
    expect(JSON.stringify(attrsOf(editor).views)).not.toContain('"filter"'); // the default is not stored
  });
});
