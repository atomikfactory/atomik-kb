// The Person property (ADR-0049, Milestone 13 slice 2): its editor in the properties panel (chips of the people, a picker over the
// members of the space, "Former member" for an account that left), what each role may do, the notification of being added by another
// person, and the filters of a view (`is me`, which is each viewer's own, and `includes` a person).

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageInfo, PropertyDef, PropertyValue, SeedSpace } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PageListFilters } from "./editor/PageListFilters";
import { applyFilters, filterOps, isFilterOp, matchFilter, newFilter, opTakesValue } from "./editor/page-list-filters";
import { t } from "./i18n";
import { createAppStore } from "./store";
import { StoreContext } from "./store-context";
import { renderComments } from "./test/comments";
import { idsOf } from "./test/page";
import { openProperties, propertyRow, valueOf } from "./test/properties";
import type { User } from "./test/render";
import { SHARING_SPACES, openSpace, toasts } from "./test/sharing";

/** The sharing spaces with a Person property "Owner" on Work: Plan lists Bob, Notes nobody. */
const SPACES: readonly SeedSpace[] = SHARING_SPACES.map((space) =>
  space.name === "Work"
    ? {
        ...space,
        properties: [{ name: "Owner", kind: "person" as const }],
        pages: (space.pages ?? []).map((page) => (page.title === "Plan" ? { ...page, properties: { Owner: ["Bob"] } } : page)),
      }
    : space,
);

const chips = (row: HTMLElement): string[] => Array.from(row.querySelectorAll(".person-chip-name")).map((node) => node.textContent ?? "");

/** Adds the row of a property that a page has no value for (the page's "Add a property" menu), and answers the row. */
async function addOwnerRow(user: User, panel: HTMLElement): Promise<HTMLElement> {
  await user.click(await within(panel).findByRole("button", { name: t("properties.add") }));
  const menu = await screen.findByRole("menu", { name: t("properties.add") });
  await user.click(within(menu).getByRole("menuitem", { name: /^Owner/u }));
  return within(panel).findByRole("listitem", { name: "Owner" });
}

async function openOwner(role: "owner" | "editor" | "commenter" | "viewer" = "owner", page = "Plan") {
  const app = await renderComments({ role, memory: { spaces: SPACES } });
  await openSpace(app.user, "Work");
  const panel = await openProperties(app.user, page);
  // The rows come once the schema and the values are read; a page without a value offers the property in its menu.
  const row =
    page === "Plan" ? await within(panel).findByRole("listitem", { name: "Owner" }) : await addOwnerRow(app.user, panel);
  return { ...app, panel, row };
}

describe("the editor", () => {
  it("shows the people of the value as chips with the names of the members", async () => {
    const { row } = await openOwner();
    expect(within(row).getByRole("group", { name: t("properties.person.label", { name: "Owner" }) })).toBeInTheDocument();
    expect(chips(row)).toEqual(["Bob"]);
    expect(row.querySelector(".person-chip")).not.toHaveClass("is-former");
  });

  it("adds a person from the picker, which lists the members that are not on it yet, and removes one", async () => {
    const { user, memory, row } = await openOwner();
    await user.click(within(row).getByRole("button", { name: t("properties.person.addTo", { name: "Owner" }) }));
    const list = await screen.findByRole("listbox", { name: t("properties.person.results") });
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["MMayayou", "CCarol", "DDan"]);
    // Narrowed by what is typed, picked with a click.
    await user.type(screen.getByRole("combobox", { name: t("properties.person.search") }), "ca");
    await waitFor(() => {
      expect(within(screen.getByRole("listbox", { name: t("properties.person.results") })).getAllByRole("option")).toHaveLength(1);
    });
    await user.click(screen.getByRole("option", { name: /Carol/u }));
    await waitFor(() => {
      expect(chips(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner"))).toEqual(["Bob", "Carol"]);
    });
    const value = await valueOf(memory, "Plan", "Owner", "Work");
    const people = (await new CoreClient(memory).peopleMentionable((await idsOf(memory)).space("Work"))).filter((person) =>
      ["Bob", "Carol"].includes(person.name),
    );
    expect(value).toEqual({ kind: "person", value: people.map((person) => person.accountId) });

    await user.click(within(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner")).getByRole("button", { name: t("properties.person.remove", { person: "Bob", name: "Owner" }) }));
    await waitFor(() => {
      expect(chips(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner"))).toEqual(["Carol"]);
    });
  });

  it("picks with the keyboard, and clears the property when the last person goes", async () => {
    const { user, memory } = await openOwner("owner", "Notes");
    const row = () => propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner");
    // A page without a value: one button that says what it does.
    await user.click(within(row()).getByRole("button", { name: t("properties.person.add") }));
    await screen.findByRole("listbox", { name: t("properties.person.results") });
    await user.keyboard("{ArrowDown}{Enter}"); // Maya is first, Bob second
    await waitFor(() => {
      expect(chips(row())).toEqual(["Bob"]);
    });
    await user.click(within(row()).getByRole("button", { name: t("properties.person.remove", { person: "Bob", name: "Owner" }) }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Owner", "Work")).toBeUndefined();
    });
    // A property without a value on the page is offered again in its menu, not shown as a row.
    await waitFor(() => {
      expect(within(screen.getByRole("region", { name: t("properties.title") })).queryByRole("listitem", { name: "Owner" })).not.toBeInTheDocument();
    });
  });

  it("shows an account the space no longer lists as \"Former member (name)\" with the last name, and keeps it in the value", async () => {
    const { memory, client } = await openOwner();
    const ids = await idsOf(memory);
    const bob = (await client.peopleMentionable(ids.space("Work"))).find((person) => person.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "removed", spaceId: ids.space("Work"), accountId: bob?.accountId ?? "" });
    });
    await waitFor(() => {
      expect(chips(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner"))).toEqual([
        t("comments.formerMember.label", { label: "Bob" }),
      ]);
    });
    expect(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner").querySelector(".person-chip")).toHaveClass("is-former");
    expect(await valueOf(memory, "Plan", "Owner", "Work")).toEqual({ kind: "person", value: [bob?.accountId] });
  });

  it("says why the core refused a person", async () => {
    const { user, memory, row } = await openOwner();
    await user.click(within(row).getByRole("button", { name: t("properties.person.addTo", { name: "Owner" }) }));
    await screen.findByRole("listbox", { name: t("properties.person.results") });
    memory.failNext({ code: "InvalidInput", message: "a person is not a member of the space", details: { field: "value" } });
    await user.click(screen.getByRole("option", { name: /Dan/u }));
    expect(await within(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner")).findByRole("alert")).toHaveTextContent(
      "a person is not a member of the space",
    );
  });

  it("is read only for a Viewer and a Commenter: chips, no way to add or remove", async () => {
    for (const role of ["viewer", "commenter"] as const) {
      const { row, unmount } = await openOwner(role);
      expect(chips(row)).toEqual(["Bob"]);
      expect(within(row).queryByRole("button")).not.toBeInTheDocument();
      unmount();
    }
  });

  it("is a personal space's own: this account is the only person to pick", async () => {
    const { user } = await renderComments({
      role: "owner",
      memory: {
        spaces: [
          { name: "Personal", properties: [{ name: "Owner", kind: "person" as const }], pages: [{ title: "Diary" }] },
          { name: "Work", pages: [{ title: "Plan" }] },
        ],
      },
    });
    const panel = await openProperties(user, "Diary");
    const owner = await addOwnerRow(user, panel);
    await user.click(within(owner).getByRole("button", { name: t("properties.person.add") }));
    const list = await screen.findByRole("listbox", { name: t("properties.person.results") });
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["MMayayou"]);
  });
});

describe("being added by another person", () => {
  it("is a notification of the page, and the row of the page shows the people at once", async () => {
    const { memory, client } = await openOwner();
    const ids = await idsOf(memory);
    const def = (await client.propertySchemaGet(ids.space("Work"))).find((candidate) => candidate.name === "Owner");
    act(() => {
      memory.receiveComments({ type: "personSet", pageId: ids.page("Plan"), author: "Bob", propertyId: def?.id ?? "", people: ["Bob", "Maya"] });
    });
    expect(await within(toasts()).findByText("Bob assigned you on Plan")).toBeInTheDocument();
    await waitFor(() => {
      expect(chips(propertyRow(screen.getByRole("region", { name: t("properties.title") }), "Owner"))).toEqual(["Bob", "Maya"]);
    });
    const [row] = await client.notificationList();
    expect(row).toMatchObject({ kind: "PersonAssigned", actorName: "Bob", pageTitle: "Plan" });
    // Bob's own assignment of Bob is nobody's news, and neither is one this account makes.
    act(() => {
      memory.receiveComments({ type: "personSet", pageId: ids.page("Notes"), author: "Bob", propertyId: def?.id ?? "", people: ["Bob"] });
    });
    expect(await client.notificationList()).toHaveLength(1);
  });
});

// ----- the filters of a view ---------------------------------------------------------------------------------------

const ME = "00000000-0000-4000-8000-a0000000000f";
const BOB = "00000000-0000-4000-8000-a00000000001";
const OWNER: PropertyDef = { id: "p-owner", name: "Owner", kind: "person", options: [], createdAt: 0 };
const STATUS: PropertyDef = { id: "p-status", name: "Status", kind: "text", options: [], createdAt: 0 };

function page(id: string, value?: PropertyValue): PageInfo {
  return {
    id,
    spaceId: "s",
    title: id,
    createdAt: 0,
    updatedAt: 0,
    ...(value === undefined ? {} : { properties: [[OWNER.id, value] as [string, PropertyValue]] }),
  };
}

describe("the filters of a view", () => {
  const pages = [page("mine", { kind: "person", value: [ME, BOB] }), page("bobs", { kind: "person", value: [BOB] }), page("nobodys")];

  it("offer is me, is not me, includes, does not include, is set and is empty for a Person property, and only those", () => {
    expect(filterOps("person")).toEqual(["isMe", "notMe", "hasPerson", "lacksPerson", "isSet", "isEmpty"]);
    expect(isFilterOp("person", "isMe")).toBe(true);
    expect(isFilterOp("text", "isMe")).toBe(false);
    expect(isFilterOp("person", "contains")).toBe(false);
    // "Is me" and "is not me" stand alone; "includes" and "does not include" need a person.
    expect(opTakesValue("isMe")).toBe(false);
    expect(opTakesValue("notMe")).toBe(false);
    expect(opTakesValue("hasPerson")).toBe(true);
    expect(opTakesValue("lacksPerson")).toBe(true);
    expect(newFilter(OWNER)).toEqual({ propertyId: OWNER.id, op: "isMe" });
  });

  it("keep the pages that list me, with \"me\" being the viewer's own account", () => {
    const filters = [{ propertyId: OWNER.id, op: "isMe" }];
    expect(applyFilters(pages, filters, [OWNER, STATUS], ME).map((entry) => entry.id)).toEqual(["mine"]);
    // Another viewer of the same saved view: the same filter, their own pages.
    expect(applyFilters(pages, filters, [OWNER, STATUS], BOB).map((entry) => entry.id)).toEqual(["mine", "bobs"]);
    // Who "me" is is not known yet: the filter cannot be applied and hides nothing, until it is.
    expect(matchFilter(pages[0] as PageInfo, filters[0] as { propertyId: string; op: string }, [OWNER], undefined)).toBeNull();
    expect(applyFilters(pages, filters, [OWNER], undefined)).toHaveLength(3);
  });

  it("keep the pages that list a person, and a filter without one is ignored", () => {
    expect(applyFilters(pages, [{ propertyId: OWNER.id, op: "hasPerson", value: BOB }], [OWNER], ME).map((entry) => entry.id)).toEqual([
      "mine",
      "bobs",
    ]);
    expect(applyFilters(pages, [{ propertyId: OWNER.id, op: "hasPerson", value: ME }], [OWNER], ME).map((entry) => entry.id)).toEqual(["mine"]);
    expect(applyFilters(pages, [{ propertyId: OWNER.id, op: "hasPerson" }], [OWNER], ME)).toHaveLength(3);
    // An account that left is still found: a value keeps it.
    expect(applyFilters(pages, [{ propertyId: OWNER.id, op: "hasPerson", value: "gone" }], [OWNER], ME)).toEqual([]);
  });

  it("are chosen in the filter popover: the condition lists the four conditions on people, and Includes asks for a person", async () => {
    const onChange = vi.fn();
    const memory = new MemoryTransport({ now: () => 0, workspaceName: "W", spaces: [{ name: "S", pages: [] }] });
    const store = createAppStore(new CoreClient(memory));
    const stop = store.getState().start();
    const anchor = document.body.appendChild(document.createElement("button"));
    render(
      <StoreContext value={store}>
        <PageListFilters
          anchor={{ x: 0, y: 0 }}
          anchorElement={anchor}
          defs={[OWNER]}
          group={{ op: "and", items: [{ propertyId: OWNER.id, op: "hasPerson" }] }}
          onChange={onChange}
          onClose={() => undefined}
        />
      </StoreContext>,
    );
    const row = await screen.findByRole("group", { name: t("pageList.filter.row", { n: 1 }) });
    const condition = within(row).getByRole("combobox", { name: t("pageList.filter.op") });
    expect(within(condition).getAllByRole("option").map((option) => option.textContent)).toEqual([
      t("pageList.op.isMe"),
      t("pageList.filter.op.notMe"),
      t("pageList.op.hasPerson"),
      t("pageList.filter.op.lacksPerson"),
      t("pageList.op.isSet"),
      t("pageList.op.isEmpty"),
    ]);
    expect(within(row).getByRole("combobox", { name: t("pageList.filter.person") })).toBeInTheDocument();
    fireEvent.change(condition, { target: { value: "isMe" } });
    expect(onChange).toHaveBeenCalledWith({ op: "and", items: [{ propertyId: OWNER.id, op: "isMe" }] });
    stop();
  });
});
