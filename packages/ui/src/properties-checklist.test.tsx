// The editor of a `checklist` property (ADR-0026, amendment of 2026-10-07): the items with their
// checkboxes and texts, "N of M done" and its bar, Enter and Backspace, moving an item with Alt+Up
// and Alt+Down, what is written and when (the whole list in one write, the text of an item after
// 300 ms or when the field is left), dropping the items nobody typed into, the limit, adding the
// property to a page, a page in the trash and following changes made elsewhere.

import { CoreClient, MAX_CHECKLIST_ITEMS, MAX_CHECKLIST_TEXT_CHARS, MemoryTransport } from "@atomik-kb/core-client";
import type { ChecklistItem, SeedSpace } from "@atomik-kb/core-client";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PropertiesPanel } from "./components/PropertiesPanel";
import { t } from "./i18n";
import { createAppStore as createStore } from "./store";
import { StoreContext } from "./store-context";
import { idsOf } from "./test/page";
import { findMainNavigation, recordingTransport, renderWith } from "./test/render";
import { openProperties, propertyRow, renderProperties, rowNames, valueOf } from "./test/properties";

afterEach(() => {
  vi.restoreAllMocks();
});

const CHECKLIST = "Launch checklist";

const SPACE: SeedSpace = {
  name: "Personal",
  properties: [
    { name: CHECKLIST, kind: "checklist" },
    { name: "Notes", kind: "text" },
  ],
  pages: [
    {
      title: "Roadmap",
      properties: {
        [CHECKLIST]: [{ text: "Draft", done: true }, { text: "Review" }, { text: "Ship" }],
      },
    },
    { title: "Plain" },
    { title: "Single", properties: { [CHECKLIST]: [{ text: "Only", done: false }] } },
  ],
};

/** The items of the checklist of a page as the core holds them. */
async function itemsOf(memory: MemoryTransport, title: string): Promise<ChecklistItem[]> {
  const value = await valueOf(memory, title, CHECKLIST);
  return value?.kind === "checklist" ? value.value : [];
}

const texts = (items: readonly ChecklistItem[]): string[] => items.map((item) => item.text);

async function renderChecklists() {
  return renderProperties([SPACE]);
}

/** The text field of the n-th item (1-based) of a row. */
function field(row: HTMLElement, number: number): HTMLInputElement {
  return within(row).getByRole("textbox", { name: t("properties.checklist.item", { number }) });
}

function checkbox(row: HTMLElement, number: number): HTMLInputElement {
  return within(row).getByRole("checkbox", { name: t("properties.checklist.done", { number }) });
}

function fieldTexts(row: HTMLElement): string[] {
  return within(row)
    .queryAllByRole("textbox")
    .map((input) => (input as HTMLInputElement).value);
}

async function roadmapRow(user: Parameters<typeof openProperties>[0]): Promise<HTMLElement> {
  const panel = await openProperties(user, "Roadmap");
  return propertyRow(panel, CHECKLIST);
}

const pause = (ms: number): Promise<void> =>
  act(
    () =>
      new Promise<void>((resolve) => {
        setTimeout(resolve, ms);
      }),
  );

describe("the list", () => {
  it("shows the items with their checkboxes and texts, and how far the list is", async () => {
    const { user } = await renderChecklists();
    const panel = await openProperties(user, "Roadmap");
    expect(rowNames(panel)).toEqual(["Tags", CHECKLIST]);
    const row = propertyRow(panel, CHECKLIST);
    expect(fieldTexts(row)).toEqual(["Draft", "Review", "Ship"]);
    expect([1, 2, 3].map((number) => checkbox(row, number).checked)).toEqual([true, false, false]);
    expect(within(row).getByText(t("properties.checklist.progress", { done: 1, total: 3 }))).toBeInTheDocument();
    const bar = within(row).getByRole("progressbar", { name: t("properties.checklist.progressLabel", { name: CHECKLIST }) });
    expect(bar).toHaveAttribute("max", "3");
    expect(bar).toHaveAttribute("value", "1");
    expect(within(row).getByRole("button", { name: t("properties.checklist.add") })).toBeEnabled();
    expect(field(row, 1)).toHaveAttribute("maxlength", String(MAX_CHECKLIST_TEXT_CHARS));
  });

  it("follows what the core holds when another writer changes the list", async () => {
    const { user, memory } = await renderChecklists();
    const row = await roadmapRow(user);
    const client = new CoreClient(memory);
    const { page, space } = await idsOf(memory);
    const property = (await client.propertySchemaGet(space("Personal"))).find((def) => def.name === CHECKLIST)?.id ?? "";
    const items = await itemsOf(memory, "Roadmap");
    await act(async () => {
      await client.pagePropertySet(page("Roadmap"), property, {
        kind: "checklist",
        value: items.map((item) => ({ ...item, done: true })),
      });
    });
    await waitFor(() => {
      expect(within(row).getByText(t("properties.checklist.progress", { done: 3, total: 3 }))).toBeInTheDocument();
    });
    expect(checkbox(row, 2).checked).toBe(true);
  });
});

describe("what is written", () => {
  async function recorded() {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [SPACE] });
    const recording = recordingTransport(memory);
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const sets = () => recording.requests.filter((request) => request.type === "PagePropertySet");
    return { memory, user, sets };
  }

  it("a checkbox writes the whole list at once, with the ids and texts it had", async () => {
    const { memory, user, sets } = await recorded();
    const row = await roadmapRow(user);
    const before = await itemsOf(memory, "Roadmap");
    await user.click(checkbox(row, 2));
    await waitFor(async () => {
      expect((await itemsOf(memory, "Roadmap")).map((item) => item.done)).toEqual([true, true, false]);
    });
    expect(sets()).toHaveLength(1);
    expect(await itemsOf(memory, "Roadmap")).toEqual(before.map((item, at) => (at === 1 ? { ...item, done: true } : item)));
    expect(within(row).getByText(t("properties.checklist.progress", { done: 2, total: 3 }))).toBeInTheDocument();
    expect(within(row).getByRole("progressbar")).toHaveAttribute("value", "2");
    await user.click(checkbox(row, 1));
    await waitFor(async () => {
      expect((await itemsOf(memory, "Roadmap")).map((item) => item.done)).toEqual([false, true, false]);
    });
    expect(sets()).toHaveLength(2);
  });

  it("typing writes once, 300 ms after the last key, and the whole list with it", async () => {
    const { memory, user, sets } = await recorded();
    const row = await roadmapRow(user);
    await user.type(field(row, 2), " the plan");
    expect(sets()).toHaveLength(0);
    expect(field(row, 2)).toHaveValue("Review the plan");
    await waitFor(() => {
      expect(sets()).toHaveLength(1);
    });
    expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Draft", "Review the plan", "Ship"]);
    await pause(400);
    expect(sets()).toHaveLength(1);
  });

  it("leaving the field writes at once, without waiting", async () => {
    const { memory, user, sets } = await recorded();
    const row = await roadmapRow(user);
    await user.type(field(row, 3), "!");
    await user.tab();
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Draft", "Review", "Ship!"]);
    });
    await pause(400);
    expect(sets()).toHaveLength(1);
  });

  it("Escape leaves the field and writes what was typed", async () => {
    const { memory, user } = await recorded();
    const row = await roadmapRow(user);
    await user.type(field(row, 1), "ed{Escape}");
    expect(field(row, 1)).not.toHaveFocus();
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Drafted", "Review", "Ship"]);
    });
  });

  it("Enter adds the next item below, focused, and writes the list; the typing after it is another write", async () => {
    const { memory, user, sets } = await recorded();
    const row = await roadmapRow(user);
    const before = await itemsOf(memory, "Roadmap");
    await user.type(field(row, 1), "{Enter}");
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual(["Draft", "", "Review", "Ship"]);
    });
    expect(field(row, 2)).toHaveFocus();
    await waitFor(() => {
      expect(sets()).toHaveLength(1);
    });
    expect((await itemsOf(memory, "Roadmap")).map((item) => item.text)).toEqual(["Draft", "", "Review", "Ship"]);
    await user.type(field(row, 2), "Polish");
    await user.tab();
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Draft", "Polish", "Review", "Ship"]);
    });
    const items = await itemsOf(memory, "Roadmap");
    expect(items.filter((item) => before.some((old) => old.id === item.id))).toEqual(before);
    expect(new Set(items.map((item) => item.id)).size).toBe(4);
    expect(within(row).getByText(t("properties.checklist.progress", { done: 1, total: 4 }))).toBeInTheDocument();
  });

  it("Backspace in an empty item removes it and moves to the one above", async () => {
    const { memory, user } = await recorded();
    const row = await roadmapRow(user);
    const before = await itemsOf(memory, "Roadmap");
    await user.type(field(row, 2), "{Enter}");
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual(["Draft", "Review", "", "Ship"]);
    });
    await user.keyboard("{Backspace}");
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual(["Draft", "Review", "Ship"]);
    });
    expect(field(row, 2)).toHaveFocus();
    await waitFor(async () => {
      expect(await itemsOf(memory, "Roadmap")).toEqual(before);
    });
    // Backspace in an item that has text edits the text.
    await user.keyboard("{Backspace}");
    expect(field(row, 2)).toHaveValue("Revie");
    expect(fieldTexts(row)).toHaveLength(3);
  });

  it("Alt+Up and Alt+Down move the item and keep focus on it", async () => {
    const { memory, user } = await recorded();
    const row = await roadmapRow(user);
    await user.click(field(row, 3));
    await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Draft", "Ship", "Review"]);
    });
    expect(fieldTexts(row)).toEqual(["Draft", "Ship", "Review"]);
    expect(field(row, 2)).toHaveFocus();
    await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Ship", "Draft", "Review"]);
    });
    expect(field(row, 1)).toHaveFocus();
    await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
    expect(fieldTexts(row)).toEqual(["Ship", "Draft", "Review"]);
    await user.keyboard("{Alt>}{ArrowDown}{/Alt}");
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Draft", "Ship", "Review"]);
    });
    // The done flags travel with their items.
    expect((await itemsOf(memory, "Roadmap")).map((item) => item.done)).toEqual([true, false, false]);
  });

  it("the remove button of an item removes that item", async () => {
    const { memory, user } = await recorded();
    const row = await roadmapRow(user);
    await user.click(within(row).getByRole("button", { name: t("properties.checklist.remove", { number: 1 }) }));
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Roadmap"))).toEqual(["Review", "Ship"]);
    });
    expect(within(row).getByText(t("properties.checklist.progress", { done: 0, total: 2 }))).toBeInTheDocument();
  });

  it("Add item appends an empty item, and leaving the list drops it again", async () => {
    const { memory, user } = await recorded();
    const row = await roadmapRow(user);
    const before = await itemsOf(memory, "Roadmap");
    await user.click(within(row).getByRole("button", { name: t("properties.checklist.add") }));
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual(["Draft", "Review", "Ship", ""]);
    });
    expect(field(row, 4)).toHaveFocus();
    await waitFor(async () => {
      expect(await itemsOf(memory, "Roadmap")).toHaveLength(4);
    });
    // Nobody typed into it: the list is written again without it when focus leaves the list.
    await user.click(document.body);
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual(["Draft", "Review", "Ship"]);
    });
    await waitFor(async () => {
      expect(await itemsOf(memory, "Roadmap")).toEqual(before);
    });
  });

  it("moving between the items of the list does not drop the empty one", async () => {
    const { memory, user } = await recorded();
    const row = await roadmapRow(user);
    await user.type(field(row, 3), "{Enter}");
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual(["Draft", "Review", "Ship", ""]);
    });
    await user.click(field(row, 1));
    await pause(50);
    expect(fieldTexts(row)).toEqual(["Draft", "Review", "Ship", ""]);
    await waitFor(async () => {
      expect(await itemsOf(memory, "Roadmap")).toHaveLength(4);
    });
  });

  it("removing the last item clears the value, and the row with it", async () => {
    const { memory, user } = await recorded();
    const panel = await openProperties(user, "Single");
    const row = propertyRow(panel, CHECKLIST);
    await user.type(field(row, 1), "{Backspace}{Backspace}{Backspace}{Backspace}");
    expect(field(row, 1)).toHaveValue("");
    await user.keyboard("{Backspace}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Single", CHECKLIST)).toBeUndefined();
    });
    await waitFor(() => {
      expect(rowNames(panel)).toEqual(["Tags"]);
    });
  });

  it("writes a refusal of the core under the list", async () => {
    const { user } = await renderChecklists();
    const row = await roadmapRow(user);
    const dispatch = vi.spyOn(MemoryTransport.prototype, "dispatch");
    dispatch.mockImplementation(async (request) => {
      if (request.type === "PagePropertySet") {
        throw new Error("the disk is full");
      }
      return await Promise.reject(new Error("unexpected"));
    });
    await user.click(checkbox(row, 2));
    expect(await within(row).findByRole("alert")).toHaveTextContent("the disk is full");
  });
});

describe("adding the property to a page", () => {
  it("starts with an empty item to type into, and the typing builds the list", async () => {
    const { user, memory } = await renderChecklists();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: new RegExp(`^${CHECKLIST}`, "u") }));
    const row = await waitFor(() => propertyRow(panel, CHECKLIST));
    expect(field(row, 1)).toHaveValue("");
    await waitFor(() => {
      expect(field(row, 1)).toHaveFocus();
    });
    expect(rowNames(panel)).toEqual(["Tags", CHECKLIST]);
    // Nothing is written for an item nobody typed into.
    expect(await valueOf(memory, "Plain", CHECKLIST)).toBeUndefined();
    await user.keyboard("First{Enter}Second");
    await user.tab();
    await waitFor(async () => {
      expect(texts(await itemsOf(memory, "Plain"))).toEqual(["First", "Second"]);
    });
    expect(rowNames(panel)).toEqual(["Tags", CHECKLIST]);
    expect(within(row).getByText(t("properties.checklist.progress", { done: 0, total: 2 }))).toBeInTheDocument();
  });

  it("keeps an empty row until something is typed, and drops the item when focus leaves it", async () => {
    const { user, memory } = await renderChecklists();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: new RegExp(`^${CHECKLIST}`, "u") }));
    const row = await waitFor(() => propertyRow(panel, CHECKLIST));
    await waitFor(() => {
      expect(field(row, 1)).toHaveFocus();
    });
    await user.click(document.body);
    await waitFor(() => {
      expect(fieldTexts(row)).toEqual([]);
    });
    expect(within(row).getByRole("button", { name: t("properties.checklist.add") })).toBeInTheDocument();
    expect(await valueOf(memory, "Plain", CHECKLIST)).toBeUndefined();
  });
});

describe("the limits", () => {
  it("disables Add item at 200 items and says why", async () => {
    const { user, memory } = await renderChecklists();
    const client = new CoreClient(memory);
    const { page, space } = await idsOf(memory);
    const property = (await client.propertySchemaGet(space("Personal"))).find((def) => def.name === CHECKLIST)?.id ?? "";
    const items: ChecklistItem[] = Array.from({ length: MAX_CHECKLIST_ITEMS }, (_, n) => ({
      id: `d0000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
      text: `Item ${String(n)}`,
      done: false,
    }));
    await client.pagePropertySet(page("Plain"), property, { kind: "checklist", value: items });
    const panel = await openProperties(user, "Plain");
    const row = propertyRow(panel, CHECKLIST);
    const add = within(row).getByRole("button", { name: t("properties.checklist.add") });
    expect(add).toBeDisabled();
    expect(add).toHaveAttribute("title", t("properties.checklist.limit", { max: MAX_CHECKLIST_ITEMS }));
    // Enter in the last item adds nothing either.
    await user.type(field(row, MAX_CHECKLIST_ITEMS), "{Enter}");
    expect(fieldTexts(row)).toHaveLength(MAX_CHECKLIST_ITEMS);
  });
});

describe("a page in the trash", () => {
  it("shows the items with their state and no way to change them", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [SPACE] });
    const client = new CoreClient(memory);
    const { page } = await idsOf(memory);
    await client.pageTrash(page("Roadmap"));
    const info = await client.pageGet(page("Roadmap"));
    const store = createStore(client, { corePollMs: 5 });
    const stop = store.getState().start();
    render(
      <StoreContext value={store}>
        <PropertiesPanel page={info} />
      </StoreContext>,
    );
    const panel = await screen.findByRole("region", { name: t("properties.title") });
    const row = propertyRow(panel, CHECKLIST);
    expect(within(row).queryAllByRole("textbox")).toHaveLength(0);
    const boxes = within(row).getAllByRole("checkbox");
    expect(boxes).toHaveLength(3);
    expect(boxes.every((box) => (box as HTMLInputElement).disabled)).toBe(true);
    expect((boxes[0] as HTMLInputElement).checked).toBe(true);
    expect(within(row).getByText("Review")).toBeInTheDocument();
    expect(within(row).getByText(t("properties.checklist.progress", { done: 1, total: 3 }))).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: t("properties.checklist.add") })).toBeNull();
    stop();
  });
});
