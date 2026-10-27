// Date mentions (ADR-0016, amendment of 2026-10-07): the `dateMention` node in the schema (shape,
// HTML in and out, text), the chip in the page editor (the words of the day, Today and Tomorrow,
// redrawn when the day changes, an atom that Backspace removes, nothing to click in a read-only
// editor), the date picker (a day, a typed date, a time, Clear time, the keyboard, Escape),
// `/date`, the reminder (ADR-0024: the attribute, the bell, the select of the picker), and what
// the Loro binding does with all of it (one update per insert or change, two
// editors converge, a portable round trip).

import { loroToPortable, portableToLoro } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { loroDocFrom, paragraphs, pid, sync, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { DATE_MENTION_NODE } from "./date-mention";
import { firstDayOfWeek, formatDateMention, fullDate, mediumDate, monthTitle, weekdayNames } from "./dates";
import { LoroBinding } from "./extensions";
import { createNodeFromLoroObj, loroSyncPluginKey, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode } from "./loro-binding";
import { createEditorSchema, createSchemaExtensions, ID_BEARING_NODES } from "./schema";

const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;

/** The day of every test that looks at the words of a chip: a Wednesday, at noon (local time). */
const NOW = new Date(2026, 9, 7, 12, 0, 0);
const TODAY = "2026-10-07";

// ----- helpers ------------------------------------------------------------------------------

function mention(n: number, date: JsonValue, time?: JsonValue): JsonValue {
  return { type: "dateMention", attrs: time === undefined ? { id: pid(n), date } : { id: pid(n), date, time } };
}

function text(value: string): JsonValue {
  return { type: "text", text: value };
}

/** One paragraph (id `pid(0)`) of the given inline content, then an empty one (`pid(1)`). */
function page(...content: JsonValue[]): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content },
      { type: "paragraph", attrs: { id: pid(1) } },
    ],
  };
}

/** Every op `doc` recorded since `since`, grouped by change. */
function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

function docOf(editor: Editor): LoroDoc {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

/** The mentions of the editor's document, as `date` or `date time`, in order. */
function mentions(editor: Editor): string[] {
  const found: string[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === DATE_MENTION_NODE) {
      found.push(typeof node.attrs.time === "string" ? `${String(node.attrs.date)} ${node.attrs.time}` : String(node.attrs.date));
    }
    return true;
  });
  return found;
}

/** Where the `n`-th mention starts. */
function positionOf(editor: Editor, n = 0): number {
  let seen = -1;
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name === DATE_MENTION_NODE) {
      seen += 1;
      if (seen === n) {
        found = pos;
      }
    }
    return found === -1;
  });
  if (found === -1) {
    throw new Error("no such mention");
  }
  return found;
}

/** The texts of the chips of the open page, in order (the glyph has none). */
function chipTexts(editor: Editor): string[] {
  return Array.from(editor.view.dom.querySelectorAll(".date-mention")).map((chip) => chip.textContent ?? "");
}

function chip(editor: Editor, n = 0): HTMLElement {
  const found = editor.view.dom.querySelectorAll<HTMLElement>(".date-mention")[n];
  if (found === undefined) {
    throw new Error("no such chip");
  }
  return found;
}

async function findPicker(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: t("date.pickerTitle") });
}

function pickerIsOpen(): boolean {
  return screen.queryByRole("dialog", { name: t("date.pickerTitle") }) !== null;
}

/** The button of a day in the open picker. */
function dayButton(iso: string): HTMLElement {
  return within(screen.getByRole("dialog", { name: t("date.pickerTitle") })).getByRole("button", { name: fullDate(iso) });
}

function placeCursor(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** The editor's initial content, read from the document like the page editor does. */
function contentOfLoro(doc: LoroDoc): JSONContent {
  const root = doc.getMap(ROOT_DOC_KEY);
  return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/** A Tiptap editor over the schema extensions, and the Loro binding when given a document. */
function mount(content: JSONContent, doc: LoroDoc | null = null): Editor {
  const extensions = createSchemaExtensions();
  if (doc !== null) {
    extensions.push(LoroBinding.configure({ doc, readOnly: false }));
  }
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({ element, extensions, content });
  mounted.push({ editor, element });
  return editor;
}

async function mountBound(doc: LoroDoc): Promise<Editor> {
  const editor = mount(contentOfLoro(doc), doc);
  await tick();
  await tick();
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "languages");
});

/** Today is `TODAY` for the test that calls this (only the clock is faked: timers stay real). */
function fakeToday(): void {
  vi.useFakeTimers({ now: NOW, toFake: ["Date"] });
}

async function openNotes(content: JsonValue) {
  const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
  const editor = await openPage(app.user, "Notes");
  await waitFor(() => {
    expect(editor.view.dom.querySelector(".date-mention")).not.toBeNull();
  });
  return { ...app, editor };
}

// ----- the node -----------------------------------------------------------------------------

describe("the node in the schema", () => {
  it("is an inline atom with id, date and time, and the schema matches the regenerated fixture", () => {
    const schema = createEditorSchema();
    const node = schema.nodes.dateMention;
    expect(node).toBeDefined();
    expect(node?.isInline).toBe(true);
    expect(node?.isAtom).toBe(true);
    expect(node?.spec.group).toBe("inline");
    expect(node?.spec.selectable).toBe(true);
    expect(node?.isLeaf).toBe(true);
    expect(Object.keys(node?.spec.attrs ?? {})).toEqual(["id", "date", "time", "reminder", "reminderFor", "relative"]);
    expect(node?.spec.attrs?.reminderFor?.default).toBeNull();
    expect(node?.spec.attrs?.relative?.default).toBeNull();
    expect(node?.spec.attrs?.time?.default).toBeNull();
    expect(node?.spec.attrs?.reminder?.default).toBeNull();
    expect(ID_BEARING_NODES).toContain("dateMention");

    expect(fixture.nodes.dateMention).toEqual({
      content: "",
      group: "inline",
      inline: true,
      atom: true,
      attrs: ["id", "date", "time", "reminder", "reminderFor", "relative"],
    });
    expect(describeSchema(schema).nodes.dateMention).toEqual(fixture.nodes.dateMention);
    expect(fixture.idBearing).toContain("dateMention");
    expect([...ID_BEARING_NODES].sort()).toEqual([...fixture.idBearing].sort());
  });

  it("can sit wherever inline content can, and nowhere else", () => {
    const schema = createEditorSchema();
    const type = schema.nodes.dateMention;
    if (type === undefined) {
      throw new Error("no dateMention in the schema");
    }
    for (const name of ["paragraph", "heading"]) {
      expect(schema.nodes[name]?.contentMatch.matchType(type), name).not.toBeNull();
    }
    // A code block, a math block and a toggle's summary hold text only.
    for (const name of ["codeBlock", "mathBlock", "detailsSummary"]) {
      expect(schema.nodes[name]?.contentMatch.matchType(type), name).toBeNull();
    }
    // It is no block: a column, a callout or the page cannot hold one directly.
    expect(schema.nodes.doc?.contentMatch.matchType(type)).toBeNull();
    expect(schema.nodes.callout?.contentMatch.matchType(type)).toBeNull();
  });

  it("reads as its ISO text as plain text, and as a chip in HTML", () => {
    const editor = mount(page(text("On "), mention(2, "2026-10-07"), text(" and "), mention(3, "2026-10-08", "14:30")) as JSONContent);
    expect(editor.getText()).toBe("On 2026-10-07 and 2026-10-08 14:30\n\n");
    const html = editor.getHTML();
    expect(html).toContain('data-type="dateMention"');
    expect(html).toContain('data-date="2026-10-07"');
    expect(html).toContain('data-date="2026-10-08" data-time="14:30"');
    expect(html).toContain(">2026-10-08 14:30</span>");
    // The date alone has no time attribute.
    expect(html.match(/data-time/gu)).toHaveLength(1);
  });

  it("is read from HTML (a paste or a drop): a real date makes a mention, anything else stays text", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setContent(
      '<p>a <span data-type="dateMention" data-date="2026-10-07">x</span> b ' +
        '<span data-type="dateMention" data-date="2026-10-08" data-time="09:05">y</span> c ' +
        '<span data-type="dateMention" data-date="2026-02-30">z</span> d ' +
        '<span data-type="dateMention">w</span> e ' +
        '<span data-type="dateMention" data-date="2026-10-09" data-time="25:00">v</span></p>',
    );
    expect(mentions(editor)).toEqual(["2026-10-07", "2026-10-08 09:05", "2026-10-09"]);
    // The two that were not dates are their text.
    expect(editor.getText()).toContain("z d w e");
    // A time that is not a time is dropped.
    expect(editor.state.doc.firstChild?.childCount).toBeGreaterThan(5);
  });

  it("is inserted by a command that checks the day, gives it an id, and puts a space after it", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "Hi" }] }] });
    placeCursor(editor, 3);
    expect(editor.commands.insertDateMention({ date: "2026-02-30" })).toBe(false);
    expect(editor.commands.insertDateMention({ date: "2026-10-07", time: "24:00" })).toBe(false);
    expect(mentions(editor)).toEqual([]);

    expect(editor.commands.insertDateMention({ date: "2026-10-07", time: "14:30" })).toBe(true);
    expect(mentions(editor)).toEqual(["2026-10-07 14:30"]);
    const node = editor.state.doc.firstChild?.child(1);
    expect(node?.type.name).toBe("dateMention");
    expect(node?.attrs.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u);
    expect(editor.state.doc.firstChild?.child(2).text).toBe(" ");
    // The caret ends up after the space.
    expect(editor.state.selection.from).toBe(editor.state.doc.firstChild!.nodeSize - 1);
  });

  it("is refused where inline atoms cannot go (a code block)", () => {
    const editor = mount({ type: "doc", content: [{ type: "codeBlock", content: [{ type: "text", text: "x" }] }] });
    placeCursor(editor, 1);
    expect(editor.commands.insertDateMention({ date: "2026-10-07" })).toBe(false);
    expect(mentions(editor)).toEqual([]);
  });

});

// ----- the chip -----------------------------------------------------------------------------

describe("the chip", () => {
  it("says Today, Tomorrow and Yesterday, and the medium date of the locale otherwise, with the time when there is one", async () => {
    fakeToday();
    const { editor } = await openNotes(
      page(
        mention(2, "2026-10-07"),
        mention(3, "2026-10-08"),
        mention(4, "2026-10-06"),
        mention(5, "2026-12-25"),
        mention(6, "2026-10-07", "14:30"),
        mention(7, "2026-12-25", "09:05"),
      ),
    );
    expect(chipTexts(editor)).toEqual([
      t("date.today"),
      t("date.tomorrow"),
      t("date.yesterday"),
      mediumDate("2026-12-25"),
      formatDateMention("2026-10-07", "14:30", TODAY),
      formatDateMention("2026-12-25", "09:05", TODAY),
    ]);
    expect(chipTexts(editor).slice(0, 3)).toEqual(["Today", "Tomorrow", "Yesterday"]);
    // The time is part of it: a short time in the locale after the day.
    expect(chipTexts(editor)[4]).toContain(new Intl.DateTimeFormat(undefined, { timeStyle: "short", timeZone: "UTC" }).format(new Date(Date.UTC(2026, 9, 7, 14, 30))));
    expect(chipTexts(editor)[5]).toContain(mediumDate("2026-12-25"));
  });

  it("is a button with the date in its name, a calendar glyph and a machine-readable time element", async () => {
    fakeToday();
    const { editor } = await openNotes(page(text("On "), mention(2, "2026-10-08", "14:30")));
    const view = editor.view.dom.querySelector("[data-type='dateMention']");
    expect(view).not.toBeNull();
    expect(view).toHaveAttribute("data-date", "2026-10-08");
    expect(view).toHaveAttribute("data-time", "14:30");

    const button = screen.getByRole("button", { name: t("date.change", { date: formatDateMention("2026-10-08", "14:30", TODAY) }) });
    expect(button).toBe(chip(editor));
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button.querySelector(".date-mention-icon svg")).toHaveAttribute("aria-hidden", "true");
    expect(button.querySelector("time")).toHaveAttribute("datetime", "2026-10-08T14:30");
    expect(editor.getText()).toContain("On 2026-10-08 14:30");
  });

  it("draws a date that is not a real day as it is stored, struck through, and still opens the picker", async () => {
    fakeToday();
    const { editor, user } = await openNotes(page(mention(2, "2026-02-30")));
    expect(chipTexts(editor)).toEqual(["2026-02-30"]);
    expect(chip(editor)).toHaveClass("is-invalid");
    expect(chip(editor).querySelector("time")).not.toHaveAttribute("datetime");
    await user.click(chip(editor));
    // The picker starts on today, and picking a day repairs the mention.
    const picker = await findPicker();
    expect(within(picker).getByRole("grid", { name: monthTitle(TODAY) })).toBeInTheDocument();
    await user.click(dayButton("2026-10-14"));
    expect(mentions(editor)).toEqual(["2026-10-14"]);
    expect(chip(editor)).not.toHaveClass("is-invalid");
  });

  it("is redrawn when the day changes: the window getting the focus back is enough", async () => {
    fakeToday();
    const { editor } = await openNotes(page(mention(2, "2026-10-07"), mention(3, "2026-10-08")));
    expect(chipTexts(editor)).toEqual(["Today", "Tomorrow"]);

    vi.setSystemTime(new Date(2026, 9, 8, 9, 0, 0));
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => {
      expect(chipTexts(editor)).toEqual(["Yesterday", "Today"]);
    });

    vi.setSystemTime(new Date(2026, 9, 10, 9, 0, 0));
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await waitFor(() => {
      expect(chipTexts(editor)).toEqual([mediumDate("2026-10-07"), mediumDate("2026-10-08")]);
    });
  });

  it("is selected like any atom and Backspace or Delete removes it, in the page too", async () => {
    fakeToday();
    const { editor, memory } = await openNotes(page(text("a "), mention(2, "2026-10-07"), text(" b")));
    const { page: pageId } = await idsOf(memory);
    expect(mentions(editor)).toEqual(["2026-10-07"]);

    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".ProseMirror-selectednode")).not.toBeNull();
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Backspace")).toBe(true);
    });
    expect(mentions(editor)).toEqual([]);
    expect(editor.getText()).toBe("a  b\n\n");
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, pageId("Notes")))).not.toContain("dateMention");
    });

    // Delete, on a mention the caret is in front of.
    act(() => {
      editor.commands.insertContentAt(3, { type: "dateMention", attrs: { date: "2026-10-09" } });
    });
    expect(mentions(editor)).toEqual(["2026-10-09"]);
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Delete")).toBe(true);
    });
    expect(mentions(editor)).toEqual([]);
  });

  it("opens nothing in a read-only editor: no button, no picker on a click", async () => {
    fakeToday();
    const { editor, user } = await openNotes(page(mention(2, "2026-10-07")));
    act(() => {
      editor.setEditable(false);
    });
    await waitFor(() => {
      expect(chip(editor)).not.toHaveAttribute("role");
    });
    expect(chip(editor)).not.toHaveAttribute("aria-haspopup");
    expect(screen.queryByRole("button", { name: /Change date/u })).not.toBeInTheDocument();
    expect(chipTexts(editor)).toEqual(["Today"]); // still drawn, still the words of the day
    await user.click(chip(editor));
    expect(pickerIsOpen()).toBe(false);
    // And a request to open one (Enter on the selected mention) is not answered either.
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    act(() => {
      editor.commands.keyboardShortcut("Enter");
    });
    expect(pickerIsOpen()).toBe(false);
  });
});

// ----- the picker ---------------------------------------------------------------------------

describe("the date picker", () => {
  async function openPicker(content: JsonValue = page(mention(2, "2026-10-07"))) {
    fakeToday();
    const app = await openNotes(content);
    await app.user.click(chip(app.editor));
    const picker = await findPicker();
    return { ...app, picker };
  }

  it("opens on a click on the chip: the month of the date, the day marked, today marked, and the focus on the day", async () => {
    const { editor, picker } = await openPicker(page(mention(2, "2026-10-14")));
    expect(chip(editor)).toHaveAttribute("aria-expanded", "true");
    const grid = within(picker).getByRole("grid", { name: monthTitle("2026-10-14") });
    // Six weeks of seven days, under the seven names of the week.
    expect(within(grid).getAllByRole("row")).toHaveLength(7);
    expect(within(grid).getAllByRole("columnheader").map((header) => header.getAttribute("aria-label"))).toEqual(
      weekdayNames(firstDayOfWeek()).map((name) => name.long),
    );
    expect(within(grid).getAllByRole("gridcell")).toHaveLength(42);
    const selected = within(grid).getAllByRole("gridcell").filter((cell) => cell.getAttribute("aria-selected") === "true");
    expect(selected).toHaveLength(1);
    expect(within(selected[0] as HTMLElement).getByRole("button")).toHaveAccessibleName(fullDate("2026-10-14"));
    expect(dayButton(TODAY)).toHaveAttribute("aria-current", "date");
    expect(within(grid).getAllByRole("button").filter((button) => button.hasAttribute("aria-current"))).toHaveLength(1);
    await waitFor(() => {
      expect(dayButton("2026-10-14")).toHaveFocus();
    });
    // The field holds the date as text, and there is no time yet.
    expect(within(picker).getByLabelText(t("date.field"))).toHaveValue("2026-10-14");
    expect(within(picker).getByLabelText(t("date.time"))).toHaveValue("");
    expect(within(picker).getByRole("button", { name: t("date.clearTime") })).toBeDisabled();
  });

  it("changes the day with a click, closes, and writes one update with the new date", async () => {
    const { editor, user } = await openPicker();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(dayButton("2026-10-21"));

    expect(mentions(editor)).toEqual(["2026-10-21"]);
    expect(pickerIsOpen()).toBe(false);
    expect(chipTexts(editor)).toEqual([mediumDate("2026-10-21")]);
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(JSON.stringify(changesSince(doc, before))).toContain("2026-10-21");
    expect(editor.state.doc.firstChild?.child(0).attrs.id).toBeDefined();
    // The block id stays: a change of the day is a change of an attribute.
    const stored = loroToPortable(doc) as { content: { content: { type: string; attrs: { id: string; date: string } }[] }[] };
    const stored0 = stored.content[0]?.content[0];
    expect(stored0?.type).toBe("dateMention");
    expect(stored0?.attrs).toEqual({ id: pid(2), date: "2026-10-21" });
  });

  it("clicking the same day closes it without a change", async () => {
    const { editor, user } = await openPicker();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(dayButton("2026-10-07"));
    expect(pickerIsOpen()).toBe(false);
    expect(changesSince(doc, before)).toEqual([]);
    expect(mentions(editor)).toEqual(["2026-10-07"]);
  });

  it("goes to the previous and the next month, and a day of another month picks that day", async () => {
    const { editor, user, picker } = await openPicker();
    await user.click(within(picker).getByRole("button", { name: t("date.nextMonth") }));
    expect(within(picker).getByRole("grid", { name: monthTitle("2026-11-07") })).toBeInTheDocument();
    await user.click(within(picker).getByRole("button", { name: t("date.nextMonth") }));
    expect(within(picker).getByRole("grid", { name: monthTitle("2026-12-07") })).toBeInTheDocument();
    await user.click(within(picker).getByRole("button", { name: t("date.prevMonth") }));
    await user.click(within(picker).getByRole("button", { name: t("date.prevMonth") }));
    await user.click(within(picker).getByRole("button", { name: t("date.prevMonth") }));
    expect(within(picker).getByRole("grid", { name: monthTitle("2026-09-07") })).toBeInTheDocument();
    // Moving through months changes nothing in the page.
    expect(mentions(editor)).toEqual(["2026-10-07"]);
    // The days of the month before and after fill the first and the last week; they can be picked too.
    const outside = within(picker)
      .getAllByRole("button")
      .filter((button) => button.classList.contains("is-outside"));
    expect(outside.length).toBeGreaterThan(0);
    await user.click(dayButton("2026-10-01"));
    expect(mentions(editor)).toEqual(["2026-10-01"]);
  });

  it("takes a date typed in the field as soon as it is a real day, and shows what is not one", async () => {
    const { editor, user, picker } = await openPicker();
    const doc = docOf(editor);
    const field = within(picker).getByLabelText(t("date.field"));
    const before = doc.oplogVersion();

    await user.clear(field);
    await user.type(field, "2026-02-3");
    expect(field).toHaveAttribute("aria-invalid", "true");
    await user.type(field, "0"); // 2026-02-30: not a day
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(mentions(editor)).toEqual(["2026-10-07"]);
    expect(changesSince(doc, before)).toEqual([]); // nothing applied while it is not a day

    await user.clear(field);
    await user.type(field, "2026-12-25");
    expect(field).toHaveAttribute("aria-invalid", "false");
    expect(mentions(editor)).toEqual(["2026-12-25"]);
    expect(changesSince(doc, before)).toHaveLength(1); // one update for the whole date
    // The grid followed it, and the picker stays open for more.
    expect(within(picker).getByRole("grid", { name: monthTitle("2026-12-25") })).toBeInTheDocument();
    expect(dayButton("2026-12-25").closest("[role='gridcell']")).toHaveAttribute("aria-selected", "true");
    expect(pickerIsOpen()).toBe(true);

    // Enter closes it, and not before there is a real day in the field.
    await user.clear(field);
    await user.type(field, "2026-13-45{Enter}");
    expect(pickerIsOpen()).toBe(true);
    await user.clear(field);
    await user.type(field, "2027-01-02{Enter}");
    expect(mentions(editor)).toEqual(["2027-01-02"]);
    expect(pickerIsOpen()).toBe(false);
  });

  it("sets a time, shows it on the chip, and takes it away with Clear time (the key is gone from the document)", async () => {
    const { editor, user, picker } = await openPicker();
    const doc = docOf(editor);
    const time = within(picker).getByLabelText(t("date.time"));
    const clear = within(picker).getByRole("button", { name: t("date.clearTime") });
    expect(clear).toBeDisabled();

    const before = doc.oplogVersion();
    fireEvent.change(time, { target: { value: "14:30" } });
    expect(mentions(editor)).toEqual(["2026-10-07 14:30"]);
    await waitFor(() => {
      expect(chipTexts(editor)[0]).toContain(new Intl.DateTimeFormat(undefined, { timeStyle: "short", timeZone: "UTC" }).format(new Date(Date.UTC(2026, 9, 7, 14, 30))));
    });
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(JSON.stringify(changesSince(doc, before))).toContain("14:30");
    expect(pickerIsOpen()).toBe(true);
    expect(clear).toBeEnabled();
    expect(loroToPortable(doc)).toMatchObject({
      content: [{ content: [{ type: "dateMention", attrs: { id: pid(2), date: "2026-10-07", time: "14:30" } }] }, {}],
    });

    // Another time replaces it (one update), and a day picked later keeps it.
    const beforeSecond = doc.oplogVersion();
    fireEvent.change(time, { target: { value: "09:05" } });
    expect(mentions(editor)).toEqual(["2026-10-07 09:05"]);
    expect(changesSince(doc, beforeSecond)).toHaveLength(1);

    const beforeClear = doc.oplogVersion();
    await user.click(clear);
    expect(mentions(editor)).toEqual(["2026-10-07"]);
    expect(editor.state.doc.firstChild?.child(0).attrs.time).toBeNull();
    expect(changesSince(doc, beforeClear)).toHaveLength(1);
    expect(JSON.stringify(changesSince(doc, beforeClear))).toContain('"delete"');
    expect(JSON.stringify(loroToPortable(doc))).not.toContain('"time"');
    expect(time).toHaveValue("");
    expect(clear).toBeDisabled();
    expect(time).toHaveFocus();
  });

  it("keeps the time when another day is picked", async () => {
    const { editor, user, picker } = await openPicker(page(mention(2, "2026-10-07", "14:30")));
    expect(within(picker).getByLabelText(t("date.time"))).toHaveValue("14:30");
    await user.click(dayButton("2026-10-09"));
    expect(mentions(editor)).toEqual(["2026-10-09 14:30"]);
  });

  it("clears the time when the time field is emptied", async () => {
    const { editor, picker } = await openPicker(page(mention(2, "2026-10-07", "14:30")));
    fireEvent.change(within(picker).getByLabelText(t("date.time")), { target: { value: "" } });
    expect(mentions(editor)).toEqual(["2026-10-07"]);
  });

  it("has a Today button that picks today and closes", async () => {
    const { editor, user, picker } = await openPicker(page(mention(2, "2026-12-25")));
    await user.click(within(picker).getByRole("button", { name: t("date.today") }));
    expect(mentions(editor)).toEqual([TODAY]);
    expect(pickerIsOpen()).toBe(false);
    expect(chipTexts(editor)).toEqual([t("date.today")]);
  });

  it("moves with the arrow keys, by a week with Up and Down, by a month with PageUp and PageDown, and picks with Enter", async () => {
    const { editor, picker } = await openPicker(page(mention(2, "2026-10-14")));
    await waitFor(() => {
      expect(dayButton("2026-10-14")).toHaveFocus();
    });
    const press = async (key: string, expected: string, options: { shiftKey?: boolean } = {}) => {
      fireEvent.keyDown(document.activeElement ?? picker, { key, ...options });
      await waitFor(() => {
        expect(dayButton(expected)).toHaveFocus();
      });
      // Exactly one day is in the tab order: the one the keyboard is on.
      const tabbable = within(picker)
        .getAllByRole("button")
        .filter((button) => button.classList.contains("date-picker-day") && button.tabIndex === 0);
      expect(tabbable).toHaveLength(1);
    };
    await press("ArrowRight", "2026-10-15");
    await press("ArrowLeft", "2026-10-14");
    await press("ArrowDown", "2026-10-21");
    await press("ArrowUp", "2026-10-14");
    await press("ArrowDown", "2026-10-21");
    await press("ArrowDown", "2026-10-28");
    await press("ArrowDown", "2026-11-04"); // into the next month, which the grid shows
    expect(within(picker).getByRole("grid", { name: monthTitle("2026-11-04") })).toBeInTheDocument();
    await press("PageUp", "2026-10-04");
    await press("PageDown", "2026-11-04");
    await press("PageDown", "2026-12-04");
    await press("PageUp", "2025-12-04", { shiftKey: true }); // a year back
    await press("PageDown", "2026-12-04", { shiftKey: true });
    await press("Home", addToWeekStart("2026-12-04", 0));
    await press("End", addToWeekStart("2026-12-04", 6));

    // Nothing was written by moving.
    expect(mentions(editor)).toEqual(["2026-10-14"]);
    // Enter picks the day the keyboard is on and closes.
    fireEvent.keyDown(document.activeElement ?? picker, { key: "Enter" });
    expect(mentions(editor)).toEqual([addToWeekStart("2026-12-04", 6)]);
    expect(pickerIsOpen()).toBe(false);
  });

  it("picks with Space too", async () => {
    const { editor, picker } = await openPicker();
    await waitFor(() => {
      expect(dayButton(TODAY)).toHaveFocus();
    });
    fireEvent.keyDown(document.activeElement ?? picker, { key: "ArrowRight" });
    await waitFor(() => {
      expect(dayButton("2026-10-08")).toHaveFocus();
    });
    fireEvent.keyDown(document.activeElement ?? picker, { key: " " });
    expect(mentions(editor)).toEqual(["2026-10-08"]);
    expect(pickerIsOpen()).toBe(false);
  });

  it("stops at the first and the last day a date can be", async () => {
    const { editor, picker } = await openPicker(page(mention(2, "9999-12-31")));
    await waitFor(() => {
      expect(dayButton("9999-12-31")).toHaveFocus();
    });
    fireEvent.keyDown(document.activeElement ?? picker, { key: "ArrowRight" });
    fireEvent.keyDown(document.activeElement ?? picker, { key: "ArrowDown" });
    expect(dayButton("9999-12-31")).toHaveFocus();
    expect(mentions(editor)).toEqual(["9999-12-31"]);
  });

  it("closes on Escape without a change, and gives the focus back to the page", async () => {
    const { editor, user } = await openPicker();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.keyboard("{ArrowRight}");
    await user.keyboard("{Escape}");
    expect(pickerIsOpen()).toBe(false);
    expect(mentions(editor)).toEqual(["2026-10-07"]);
    expect(changesSince(doc, before)).toEqual([]);
    expect(chip(editor)).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => {
      expect(editor.view.dom).toHaveFocus();
    });
  });

  it("closes on a press outside, and on a second click on the chip", async () => {
    const { editor, user } = await openPicker();
    await user.click(document.body);
    expect(pickerIsOpen()).toBe(false);
    expect(mentions(editor)).toEqual(["2026-10-07"]);

    await user.click(chip(editor));
    expect(await findPicker()).toBeInTheDocument();
    await user.click(chip(editor));
    await waitFor(() => {
      expect(pickerIsOpen()).toBe(false);
    });
  });


  it("opens with Enter on the selected mention", async () => {
    fakeToday();
    const { editor } = await openNotes(page(text("a "), mention(2, "2026-10-07"), text(" b")));
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Enter")).toBe(true);
    });
    expect(await findPicker()).toBeInTheDocument();
    expect(mentions(editor)).toEqual(["2026-10-07"]);
  });

  it("shows the week from the first day of the locale", async () => {
    // A German system: the words stay English (the app has no German), the week starts on Monday.
    Object.defineProperty(navigator, "languages", { configurable: true, value: ["de-DE"] });
    const { picker } = await openPicker();
    const headers = within(picker).getAllByRole("columnheader");
    expect(headers[0]).toHaveAttribute("aria-label", weekdayNames(1)[0]?.long);
    expect(headers.map((header) => header.getAttribute("aria-label"))).toEqual(weekdayNames(1).map((name) => name.long));
    // The first week of October 2026 starts on a Monday: the 28th of September.
    const firstRow = within(picker).getAllByRole("row")[1] as HTMLElement;
    expect(within(firstRow).getAllByRole("button")[0]).toHaveAccessibleName(fullDate("2026-09-28"));
  });
});

/** The day in the week of `iso` that is `offset` days after the first day of the week (of the locale). */
function addToWeekStart(iso: string, offset: number): string {
  const parts = iso.split("-").map(Number);
  const date = new Date(Date.UTC(parts[0] ?? 0, (parts[1] ?? 1) - 1, parts[2] ?? 1));
  const lead = (date.getUTCDay() - firstDayOfWeek() + 7) % 7;
  date.setUTCDate(date.getUTCDate() - lead + offset);
  return date.toISOString().slice(0, 10);
}

// ----- /date --------------------------------------------------------------------------------

describe("/date", () => {
  it("inserts a mention of today and opens its picker, where a day replaces it", async () => {
    fakeToday();
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    const editor = await openPage(user, "Notes");
    const { page: pageId } = await idsOf(memory);
    placeCursor(editor, editor.state.doc.child(0).nodeSize + 1); // in the empty second paragraph
    editor.commands.insertContent("/date");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    expect(within(menu).getAllByRole("option")[0]).toHaveTextContent(t("slash.date"));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // The command text is gone, the mention is there (and a space after it), and the picker is open on it.
    expect(mentions(editor)).toEqual([TODAY]);
    expect(editor.getText()).toBe("Intro\n\n2026-10-07 ");
    const picker = await findPicker();
    expect(dayButton(TODAY).closest("[role='gridcell']")).toHaveAttribute("aria-selected", "true");
    expect(changesSince(doc, before).length).toBeGreaterThan(0);
    expect(chipTexts(editor)).toEqual([t("date.today")]);

    await act(async () => {
      fireEvent.click(within(picker).getByRole("button", { name: fullDate("2026-10-12") }));
      await tick();
    });
    expect(mentions(editor)).toEqual(["2026-10-12"]);
    expect(pickerIsOpen()).toBe(false);
    await waitFor(async () => {
      expect(await contentOf(memory, pageId("Notes"))).toMatchObject({
        content: [
          {},
          {
            type: "paragraph",
            content: [
              { type: "dateMention", attrs: { date: "2026-10-12", id: expect.stringMatching(/^[0-9a-f-]{36}$/u) as unknown } },
              { type: "text", text: " " },
            ],
          },
        ],
      });
    });
  });

  it("finds /date by its words, and keeps today's mention when the picker is closed with Escape", async () => {
    fakeToday();
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("/calendar");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    // "calendar" is one of Date's words; the Calendar view entry's label starts with it, so that
    // entry ranks first (ADR-0054) and Date is found further down.
    const dateOption = (): HTMLElement | undefined =>
      within(menu)
        .getAllByRole("option")
        .find((option) => option.querySelector(".suggestion-title")?.textContent === t("slash.date"));
    await waitFor(() => {
      expect(dateOption()).toBeDefined();
    });
    await user.click(dateOption() as HTMLElement);
    await findPicker();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => {
      expect(pickerIsOpen()).toBe(false);
    });
    expect(mentions(editor)).toEqual([TODAY]);
  });

  it("is in the inline section with a glyph and a hint, and unavailable where inline atoms cannot go", async () => {
    const { user } = await renderPages([
      {
        name: "Personal",
        pages: [
          {
            title: "Notes",
            content: {
              type: "doc",
              content: [
                {
                  type: "details",
                  attrs: { id: pid(0), open: true },
                  content: [
                    { type: "detailsSummary", attrs: { id: pid(1) }, content: [text("Sum")] },
                    { type: "detailsContent", attrs: { id: pid(2) }, content: [{ type: "paragraph", attrs: { id: pid(3) } }] },
                  ],
                },
              ],
            },
          },
        ],
      },
    ]);
    const editor = await openPage(user, "Notes");
    // Inside the summary (text only) the command shows why it cannot run, and does nothing.
    placeCursor(editor, 1 + 1 + 3);
    editor.commands.insertContent(" /date");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    const option = within(menu).getAllByRole("option")[0] as HTMLElement;
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(within(option).getByText(t("slash.inline.notHere"))).toBeInTheDocument();
    fireEvent.click(option);
    expect(mentions(editor)).toEqual([]);
    expect(editor.getText()).toContain("/date");
  });
});

// ----- the Loro binding ---------------------------------------------------------------------

describe("the mention in a bound document", () => {
  it("is written as one update per insert and one per change, as a node with its attributes", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const editor = await mountBound(doc);
    placeCursor(editor, 6);

    const before = doc.oplogVersion();
    expect(editor.commands.insertDateMention({ date: "2026-10-07", time: "14:30" })).toBe(true);
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1); // the mention and its space are one transaction, one update
    expect(JSON.stringify(changes)).toContain("dateMention");
    const stored = loroToPortable(doc) as { content: { content: { type: string; attrs?: Record<string, unknown>; text?: string }[] }[] };
    const inline = stored.content[0]?.content ?? [];
    expect(inline.map((node) => node.type)).toEqual(["text", "dateMention", "text"]);
    expect(inline[0]?.text).toBe("Hello");
    expect(inline[1]?.attrs).toMatchObject({ date: "2026-10-07", time: "14:30" });
    expect(typeof inline[1]?.attrs?.id).toBe("string");
    expect(inline[2]?.text).toBe(" ");

    // A change of the day: one update, the id kept.
    const id = editor.state.doc.firstChild?.child(1).attrs.id as string;
    const beforeChange = doc.oplogVersion();
    editor.view.dispatch(
      editor.state.tr.setNodeMarkup(positionOf(editor), undefined, { id, date: "2026-10-08", time: null }),
    );
    expect(changesSince(doc, beforeChange)).toHaveLength(1);
    const changed = loroToPortable(doc) as typeof stored;
    expect(changed.content[0]?.content[1]?.attrs).toEqual({ id, date: "2026-10-08" });
  });

  it("is undone and redone with the editor's undo, an insert and a change each in a step of their own", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const editor = await mountBound(doc);
    placeCursor(editor, 6);
    editor.commands.insertDateMention({ date: "2026-10-07" });
    await tick(600); // past the time within which changes undo together
    const attrs = editor.state.doc.firstChild?.child(1).attrs ?? {};
    editor.view.dispatch(editor.state.tr.setNodeMarkup(positionOf(editor), undefined, { ...attrs, date: "2026-10-09", time: "08:15" }));
    expect(mentions(editor)).toEqual(["2026-10-09 08:15"]);

    expect(editor.commands.keyboardShortcut("Mod-z")).toBe(true);
    await tick();
    expect(mentions(editor)).toEqual(["2026-10-07"]);
    expect(editor.state.doc.firstChild?.child(1).attrs.id).toBe(attrs.id);
    expect(editor.commands.keyboardShortcut("Mod-z")).toBe(true);
    await tick();
    expect(mentions(editor)).toEqual([]);
    expect(editor.getText()).toBe("Hello");
    expect(editor.commands.keyboardShortcut("Mod-Shift-z")).toBe(true);
    await tick();
    expect(mentions(editor)).toEqual(["2026-10-07"]);
    expect(JSON.stringify(loroToPortable(doc))).toContain("2026-10-07");
  });

  it("is read back from the document: a page that holds mentions shows them and writes nothing on load", async () => {
    const doc = loroDocFrom(page(text("On "), mention(2, "2026-10-07"), text(" and "), mention(3, "2026-10-08", "14:30")));
    const before = doc.oplogVersion();
    const editor = await mountBound(doc);
    expect(mentions(editor)).toEqual(["2026-10-07", "2026-10-08 14:30"]);
    expect(changesSince(doc, before)).toEqual([]);
    expect(editor.getText()).toContain("On 2026-10-07 and 2026-10-08 14:30");
  });

  it("converges between two editors: an insert and a change on one show on the other, concurrent changes agree", async () => {
    const docA = loroDocFrom(paragraphs("Hello"));
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    const editorA = await mountBound(docA);
    const editorB = await mountBound(docB);

    placeCursor(editorA, 6);
    editorA.commands.insertDateMention({ date: "2026-10-07" });
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(mentions(editorA)).toEqual(["2026-10-07"]);
    expect(mentions(editorB)).toEqual(["2026-10-07"]);
    expect(editorB.getText()).toBe("Hello2026-10-07 ");
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(editorB.state.doc.firstChild?.child(1).attrs.id).toBe(editorA.state.doc.firstChild?.child(1).attrs.id);

    // A moves it to another day while B gives it a time: both changes are in the end result of both.
    const attrs = (editor: Editor): Record<string, unknown> => editor.state.doc.firstChild?.child(1).attrs ?? {};
    editorA.view.dispatch(editorA.state.tr.setNodeMarkup(positionOf(editorA), undefined, { ...attrs(editorA), date: "2026-10-09" }));
    editorB.view.dispatch(editorB.state.tr.setNodeMarkup(positionOf(editorB), undefined, { ...attrs(editorB), time: "08:15" }));
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(mentions(editorA)).toEqual(mentions(editorB));
    expect(mentions(editorA)).toHaveLength(1);
    expect(mentions(editorA)[0]).toMatch(/^2026-10-(07|09)( 08:15)?$/u);

    // One removes it while the other types next to it.
    editorA.view.dispatch(editorA.state.tr.delete(positionOf(editorA), positionOf(editorA) + 1));
    placeCursor(editorB, 3);
    editorB.view.dispatch(editorB.state.tr.insertText("!"));
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(mentions(editorA)).toEqual(mentions(editorB));
  });

  it("round-trips through the portable JSON: the Loro document builds the same editor content, and back", async () => {
    const json = page(
      text("a "),
      mention(2, "2026-10-07"),
      text(" "),
      mention(3, "2026-10-08", "14:30"),
      { type: "hardBreak" },
      mention(4, "2027-02-28"),
    );
    const doc = new LoroDoc();
    portableToLoro(doc, json);
    expect(loroToPortable(doc)).toEqual(json);

    const editor = await mountBound(doc);
    expect(mentions(editor)).toEqual(["2026-10-07", "2026-10-08 14:30", "2027-02-28"]);
    expect(loroToPortable(doc)).toEqual(json);
    // And an edit next to them leaves them as they are.
    placeCursor(editor, 3);
    editor.view.dispatch(editor.state.tr.insertText("X"));
    expect(mentions(editor)).toEqual(["2026-10-07", "2026-10-08 14:30", "2027-02-28"]);
    expect(loroToPortable(doc)).toEqual(
      page(text("a X"), mention(2, "2026-10-07"), text(" "), mention(3, "2026-10-08", "14:30"), { type: "hardBreak" }, mention(4, "2027-02-28")),
    );
  });

  it("keeps a mention with a date from a newer version as it is, until it is changed", async () => {
    // A date that is not a real day is stored as it is: reading never rewrites it.
    const json = page(mention(2, "2026-02-30", "99:99"));
    const doc = loroDocFrom(json);
    const before = doc.oplogVersion();
    const editor = await mountBound(doc);
    expect(editor.state.doc.firstChild?.child(0).attrs).toMatchObject({ date: "2026-02-30", time: "99:99" });
    expect(changesSince(doc, before)).toEqual([]);
    expect(loroToPortable(doc)).toEqual(json);
  });
});


// ----- reminders (ADR-0016 amendment of 2026-10-07, ADR-0024) -------------------------------------

/** A mention with a reminder (minutes before its moment). */
function reminded(n: number, date: string, time: string | null, reminder: number): JsonValue {
  return {
    type: "dateMention",
    attrs: time === null ? { id: pid(n), date, reminder } : { id: pid(n), date, time, reminder },
  };
}

/** The `reminder` of the editor's `n`-th mention. */
function reminderOf(editor: Editor, n = 0): unknown {
  let seen = -1;
  let found: unknown;
  editor.state.doc.descendants((node) => {
    if (node.type.name === DATE_MENTION_NODE) {
      seen += 1;
      if (seen === n) {
        found = node.attrs.reminder;
      }
    }
    return true;
  });
  return found;
}

describe("the reminder attribute", () => {
  it("is rendered as data-reminder when set, read back from HTML only as a non-negative integer, and is not text", () => {
    const editor = mount(
      page(text("On "), reminded(2, "2026-10-07", "14:30", 15), text(" and "), mention(3, "2026-10-08")) as JSONContent,
    );
    expect(editor.getText()).toBe("On 2026-10-07 14:30 and 2026-10-08\n\n");
    const html = editor.getHTML();
    expect(html).toContain('data-date="2026-10-07" data-time="14:30" data-reminder="15"');
    expect(html.match(/data-reminder/gu)).toHaveLength(1);

    const pasted = mount({ type: "doc", content: [{ type: "paragraph" }] });
    pasted.commands.setContent(
      '<p><span data-type="dateMention" data-date="2026-10-07" data-reminder="0">a</span> ' +
        '<span data-type="dateMention" data-date="2026-10-08" data-reminder="2880">b</span> ' +
        '<span data-type="dateMention" data-date="2026-10-09" data-reminder="-5">c</span> ' +
        '<span data-type="dateMention" data-date="2026-10-10" data-reminder="1.5">d</span> ' +
        '<span data-type="dateMention" data-date="2026-10-11" data-reminder="soon">e</span> ' +
        '<span data-type="dateMention" data-date="2026-10-12">f</span></p>',
    );
    expect([0, 1, 2, 3, 4, 5].map((n) => reminderOf(pasted, n))).toEqual([0, 2880, null, null, null, null]);
  });

  it("is carried through a copy of the HTML (a mention pasted from this app keeps its reminder)", () => {
    const source = mount(page(reminded(2, "2026-10-07", "14:30", 60)) as JSONContent);
    const copy = mount({ type: "doc", content: [{ type: "paragraph" }] });
    copy.commands.setContent(source.getHTML());
    expect(mentions(copy)).toEqual(["2026-10-07 14:30"]);
    expect(reminderOf(copy)).toBe(60);
  });
});

describe("the bell of the chip", () => {
  it("shows a bell and says the reminder in the name of the chip when there is one", async () => {
    fakeToday();
    const { editor } = await openNotes(
      page(mention(2, "2026-10-07", "14:30"), text(" "), reminded(3, "2026-10-08", "14:30", 15)),
    );
    expect(chip(editor, 0).querySelector(".date-mention-bell")).toBeNull();
    expect(chip(editor, 0)).toHaveAccessibleName(
      t("date.change", { date: formatDateMention("2026-10-07", "14:30", TODAY) }),
    );

    const bell = chip(editor, 1).querySelector<HTMLElement>(".date-mention-bell");
    expect(bell).not.toBeNull();
    expect(chip(editor, 1)).toHaveAccessibleName(
      t("date.changeWithReminder", {
        date: formatDateMention("2026-10-08", "14:30", TODAY),
        reminder: t("date.reminder.m15"),
      }),
    );
    expect(bell?.getAttribute("aria-hidden")).toBe("true");
    expect(bell?.getAttribute("title")).toBe(t("date.reminderSet", { reminder: t("date.reminder.m15") }));
    // The text of the chip is the date: the bell adds none.
    expect(chip(editor, 1).querySelector("time")?.textContent).toBe(formatDateMention("2026-10-08", "14:30", TODAY));
  });

  it("names a reminder of a day without a time by its 9:00, and one that is not a preset by its minutes", async () => {
    fakeToday();
    const { editor } = await openNotes(
      page(reminded(2, "2026-10-08", null, 0), text(" "), reminded(3, "2026-10-08", null, 1440), text(" "), reminded(4, "2026-10-08", "14:30", 90)),
    );
    expect(chip(editor, 0)).toHaveAccessibleName(expect.stringContaining(t("date.reminder.atNine")));
    expect(chip(editor, 1)).toHaveAccessibleName(expect.stringContaining(t("date.reminder.m1440Nine")));
    expect(chip(editor, 2)).toHaveAccessibleName(
      expect.stringContaining(t("date.reminder.custom", { count: 90 })),
    );
  });

  it("is a labelled image in a read-only editor, where the chip is no button", async () => {
    fakeToday();
    const app = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: page(reminded(2, "2026-10-08", "14:30", 60)) }] },
    ]);
    const editor = await openPage(app.user, "Notes");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".date-mention")).not.toBeNull();
    });
    editor.setEditable(false);
    await waitFor(() => {
      expect(chip(editor).getAttribute("role")).toBeNull();
    });
    const bell = chip(editor).querySelector<HTMLElement>(".date-mention-bell");
    expect(bell?.getAttribute("role")).toBe("img");
    expect(bell).toHaveAccessibleName(t("date.reminderSet", { reminder: t("date.reminder.m60") }));
  });
});

describe("the reminder field of the date picker", () => {
  async function openPicker(content: JsonValue) {
    fakeToday();
    const app = await openNotes(content);
    await app.user.click(chip(app.editor));
    const picker = await findPicker();
    const select = within(picker).getByLabelText(t("date.reminder"));
    return { ...app, picker, select: select as HTMLSelectElement };
  }

  const PRESETS = [null, 0, 5, 15, 30, 60, 120, 1440, 2880];

  it("lists None and the eight presets for a mention with a time, with the labels of the design", async () => {
    const { select } = await openPicker(page(mention(2, "2026-10-09", "14:30")));
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual([
      "None",
      "At time",
      "5 minutes before",
      "15 minutes before",
      "30 minutes before",
      "1 hour before",
      "2 hours before",
      "1 day before",
      "2 days before",
    ]);
    expect(Array.from(select.options).map((option) => option.value)).toEqual(PRESETS.map((value) => (value === null ? "none" : String(value))));
    expect(select).toHaveValue("none");
  });

  it("says 9:00 in every label for a day without a time", async () => {
    const { select } = await openPicker(page(mention(2, "2026-10-09")));
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual([
      "None",
      "At 9:00",
      "5 minutes before 9:00",
      "15 minutes before 9:00",
      "30 minutes before 9:00",
      "1 hour before 9:00",
      "2 hours before 9:00",
      "1 day before, at 9:00",
      "2 days before, at 9:00",
    ]);
  });

  it("shows the reminder of the mention", async () => {
    const { select } = await openPicker(page(reminded(2, "2026-10-09", "14:30", 120)));
    expect(select).toHaveValue("120");
    expect(select.options).toHaveLength(PRESETS.length);
  });

  it("shows a reminder that is no preset as an option of its own, so that the picker never changes it", async () => {
    const { select } = await openPicker(page(reminded(3, "2026-10-09", "14:30", 90)));
    expect(select).toHaveValue("90");
    expect(Array.from(select.options).map((option) => option.textContent)).toContain("90 minutes before");
    expect(select.options).toHaveLength(PRESETS.length + 1);
  });

  it("writes the reminder alone, as one update of one attribute, and leaves the picker open", async () => {
    const { editor, picker, select } = await openPicker(page(mention(2, "2026-10-09", "14:30")));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(select, { target: { value: "15" } });

    expect(reminderOf(editor)).toBe(15);
    expect(mentions(editor)).toEqual(["2026-10-09 14:30"]);
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    const ops = JSON.stringify(changes);
    expect(ops).toContain('"reminder"');
    expect(ops).not.toContain('"date"');
    expect(ops).not.toContain('"time"');
    expect(pickerIsOpen()).toBe(true);
    expect(picker).toBeInTheDocument();
    expect(loroToPortable(doc)).toMatchObject({
      content: [
        { content: [{ type: "dateMention", attrs: { id: pid(2), date: "2026-10-09", time: "14:30", reminder: 15 } }] },
        {},
      ],
    });
  });

  it("changes the reminder in one update, and None takes the attribute out of the document", async () => {
    const { editor, select } = await openPicker(page(reminded(2, "2026-10-09", "14:30", 15)));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(select, { target: { value: "1440" } });
    expect(reminderOf(editor)).toBe(1440);
    expect(changesSince(doc, before)).toHaveLength(1);

    const beforeNone = doc.oplogVersion();
    fireEvent.change(select, { target: { value: "none" } });
    expect(reminderOf(editor)).toBeNull();
    expect(changesSince(doc, beforeNone)).toHaveLength(1);
    expect(JSON.stringify(changesSince(doc, beforeNone))).toContain('"delete"');
    expect(JSON.stringify(loroToPortable(doc))).not.toContain('"reminder"');
    await waitFor(() => {
      expect(select).toHaveValue("none");
    });
  });

  it("keeps the reminder when the day or the time changes", async () => {
    const { editor, user, picker } = await openPicker(page(reminded(2, "2026-10-09", "14:30", 30)));
    await user.click(dayButton("2026-10-14"));
    expect(mentions(editor)).toEqual(["2026-10-14 14:30"]);
    expect(reminderOf(editor)).toBe(30);

    await user.click(chip(editor));
    const reopened = await findPicker();
    fireEvent.change(within(reopened).getByLabelText(t("date.time")), { target: { value: "09:05" } });
    expect(mentions(editor)).toEqual(["2026-10-14 09:05"]);
    expect(reminderOf(editor)).toBe(30);
    expect(picker).toBeDefined();
  });

  it("puts the bell on the chip at once, and takes it away with None", async () => {
    const { editor, select } = await openPicker(page(mention(2, "2026-10-09", "14:30")));
    expect(chip(editor).querySelector(".date-mention-bell")).toBeNull();
    fireEvent.change(select, { target: { value: "0" } });
    await waitFor(() => {
      expect(chip(editor).querySelector(".date-mention-bell")).not.toBeNull();
    });
    fireEvent.change(select, { target: { value: "none" } });
    await waitFor(() => {
      expect(chip(editor).querySelector(".date-mention-bell")).toBeNull();
    });
  });

  it("says when the moment of the reminder has passed: it will not fire", async () => {
    // Today is 2026-10-07 12:00 (fakeToday): yesterday's reminder is behind the clock.
    const { picker } = await openPicker(page(reminded(2, "2026-10-06", "09:00", 15)));
    expect(within(picker).getByRole("status")).toHaveTextContent(t("date.reminderPast"));
  });

  it("says nothing of a reminder that is still to come", async () => {
    const { picker } = await openPicker(page(reminded(3, "2026-10-08", "09:00", 15)));
    expect(within(picker).queryByText(t("date.reminderPast"))).toBeNull();
  });

  it("says nothing when there is no reminder, whatever the day", async () => {
    const { picker } = await openPicker(page(mention(4, "2026-10-06", "09:00")));
    expect(within(picker).queryByText(t("date.reminderPast"))).toBeNull();
  });
});

describe("the reminder in a bound document", () => {
  it("survives the portable round trip", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const editor = await mountBound(doc);
    placeCursor(editor, 6);
    expect(editor.commands.insertDateMention({ date: "2026-10-07", time: "14:30" })).toBe(true);
    const id = editor.state.doc.firstChild?.child(1).attrs.id as string;
    editor.view.dispatch(
      editor.state.tr.setNodeMarkup(positionOf(editor), undefined, { id, date: "2026-10-07", time: "14:30", reminder: 15 }),
    );
    const stored = loroToPortable(doc) as { content: { content: { type: string; attrs?: Record<string, unknown> }[] }[] };
    expect(stored.content[0]?.content[1]?.attrs).toEqual({ id, date: "2026-10-07", time: "14:30", reminder: 15 });

    const rebuilt = new LoroDoc();
    portableToLoro(rebuilt, loroToPortable(doc));
    expect(loroToPortable(rebuilt)).toEqual(loroToPortable(doc));
    const second = await mountBound(rebuilt);
    expect(reminderOf(second)).toBe(15);
  });

  it("converges between two editors, a change of the reminder next to a change of the day", async () => {
    const docA = loroDocFrom(paragraphs("Hello"));
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    const editorA = await mountBound(docA);
    const editorB = await mountBound(docB);
    placeCursor(editorA, 6);
    editorA.commands.insertDateMention({ date: "2026-10-07", time: "14:30" });
    sync(docA, docB);
    await tick();
    expect(reminderOf(editorB)).toBeNull();

    const attrs = (editor: Editor): Record<string, unknown> => editor.state.doc.firstChild?.child(1).attrs ?? {};
    editorA.view.dispatch(editorA.state.tr.setNodeMarkup(positionOf(editorA), undefined, { ...attrs(editorA), reminder: 60 }));
    editorB.view.dispatch(editorB.state.tr.setNodeMarkup(positionOf(editorB), undefined, { ...attrs(editorB), date: "2026-10-09" }));
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    // Each wrote one attribute, so both changes are in the end result of both.
    expect(reminderOf(editorA)).toBe(60);
    expect(reminderOf(editorB)).toBe(60);
    expect(mentions(editorA)).toEqual(["2026-10-09 14:30"]);
    expect(mentions(editorB)).toEqual(["2026-10-09 14:30"]);
  });
});
