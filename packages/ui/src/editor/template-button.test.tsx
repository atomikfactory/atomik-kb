// The template button (ADR-0056, `docs/architecture/templates.md` section 9) in the editor: the block in the schema (content,
// attributes, HTML), the `relative` attribute of the date and person mentions, what a click inserts (a copy with new block ids
// below or above, relative mentions filled in, the caret at the end, one undo step), the view (default label, the fold-out,
// the settings popover, a read-only editor) and `/button` in the slash menu.

import { CoreClient, loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { TextSelection } from "@tiptap/pm/state";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { pid, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { clearWhiteboardStash, whiteboardCopyState } from "./whiteboard-copy";
import { loroSyncPluginKey } from "./loro-binding";
import { createEditorSchema, createSchemaExtensions, ID_BEARING_NODES } from "./schema";
import { clickTemplateButton, fillRelative, insertTemplateCopy, meOf } from "./template-button-copy";
import { labelOf, positionOf, TEMPLATE_BUTTON_LABEL_MAX_CHARS } from "./template-button";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  clearWhiteboardStash();
});

const ME = { accountId: "0000aaaa-0000-4000-8000-00000000aaaa", name: "Ada Lovelace" };
const MOMENT = { date: "2026-10-18", time: "14:05" };

function text(words: string): JsonValue {
  return { type: "text", text: words };
}

/** `Tasks`, then a button (`pid(1)`) holding one paragraph with a date to fill in, a person to fill in and a reminder. */
function tasksPage(buttonAttrs: Record<string, JsonValue> = {}, content?: JsonValue[]): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [text("Tasks")] },
      {
        type: "templateButton",
        attrs: { id: pid(1), label: "Add task", position: "below", ...buttonAttrs },
        content: content ?? [
          {
            type: "paragraph",
            attrs: { id: pid(2) },
            content: [
              text("Task due "),
              { type: "dateMention", attrs: { id: pid(3), date: "2026-01-01", reminder: 30, relative: "today" } },
              text(" for "),
              { type: "personMention", attrs: { accountId: pid(90), label: "Someone", relative: "me" } },
            ],
          },
          { type: "paragraph", attrs: { id: pid(4) }, content: [text("Second line")] },
        ],
      },
    ],
  };
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/** A schema-only editor (no views, no binding) holding `content`. */
function mount(content: JSONContent): Editor {
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({ element, extensions: createSchemaExtensions(), content });
  mounted.push({ editor, element });
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
});

/** The `relative` of each mention node, as the document holds it. */
function relativesOf(editor: Editor, name: string): unknown[] {
  return mentionsIn(editor.state.doc, name).map((node): unknown => node.attrs.relative);
}

/** The ids of every node of the editor's document that has one. */
function idsIn(editor: Editor): string[] {
  const found: string[] = [];
  editor.state.doc.descendants((node) => {
    const id: unknown = node.attrs.id;
    if (typeof id === "string") {
      found.push(id);
    }
    return true;
  });
  return found;
}

/** The text of a node as the page reads it: mentions as their text, the blocks of a container one bar apart. */
function plain(node: PmNode): string {
  if (node.isText) {
    return node.text ?? "";
  }
  if (node.type.name === "dateMention") {
    return String(node.attrs.date) + (typeof node.attrs.time === "string" ? ` ${node.attrs.time}` : "");
  }
  if (node.type.name === "personMention") {
    return `@${String(node.attrs.label)}`;
  }
  const parts: string[] = [];
  node.forEach((child) => {
    parts.push(plain(child));
  });
  return parts.join(node.isTextblock ? "" : "|");
}

/** `type:text` of each top-level block. */
function outline(editor: Editor): string[] {
  const rows: string[] = [];
  editor.state.doc.forEach((node) => {
    rows.push(`${node.type.name}:${plain(node)}`);
  });
  return rows;
}

function mentionsIn(node: PmNode, name: string): PmNode[] {
  const found: PmNode[] = [];
  node.descendants((child) => {
    if (child.type.name === name) {
      found.push(child);
    }
    return true;
  });
  return found;
}

describe("the schema", () => {
  it("has the node: a block that holds blocks, with an id, a label and a position", () => {
    const schema = createEditorSchema();
    const node = schema.nodes.templateButton;
    expect(node?.spec.group).toBe("block");
    expect(node?.spec.content).toBe("block+");
    expect(node?.isAtom).toBe(false);
    expect(Object.keys(node?.spec.attrs ?? {})).toEqual(["id", "label", "position"]);
    expect(node?.createAndFill()?.attrs).toMatchObject({ label: "", position: "below" });
    expect(node?.createAndFill()?.childCount).toBe(1);
    expect(ID_BEARING_NODES).toContain("templateButton");
  });

  it("matches the regenerated fixture, the mentions' new attribute included", () => {
    const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
    const described = describeSchema(createEditorSchema());
    expect(described.nodes.templateButton).toEqual(fixture.nodes.templateButton);
    expect(fixture.nodes.templateButton?.attrs).toEqual(["id", "label", "position"]);
    expect(described.nodes.dateMention).toEqual(fixture.nodes.dateMention);
    expect(fixture.nodes.dateMention?.attrs).toEqual(["id", "date", "time", "reminder", "reminderFor", "relative"]);
    expect(described.nodes.personMention).toEqual(fixture.nodes.personMention);
    expect(fixture.nodes.personMention?.attrs).toEqual(["accountId", "label", "relative"]);
    expect(described.idBearing).toEqual(fixture.idBearing);
  });

  it("reads a position and a label from whatever the document holds", () => {
    expect(positionOf("above")).toBe("above");
    for (const other of ["below", "", "middle", null, undefined, 3]) {
      expect(positionOf(other), String(other)).toBe("below");
    }
    expect(labelOf("Add task")).toBe("Add task");
    expect(labelOf("x".repeat(100))).toHaveLength(TEMPLATE_BUTTON_LABEL_MAX_CHARS);
    for (const other of [null, undefined, 3, {}]) {
      expect(labelOf(other)).toBe("");
    }
  });

  it("renders its attributes as HTML and reads them back (a paste)", () => {
    const editor = mount(tasksPage({ label: "Add task", position: "above" }) as JSONContent);
    const html = editor.getHTML();
    expect(html).toContain('data-type="templateButton"');
    expect(html).toContain('data-label="Add task"');
    expect(html).toContain('data-position="above"');
    editor.commands.setContent(
      '<div data-type="templateButton" data-label="Hi" data-position="sideways"><div class="template-button-content"><p>a</p></div></div>',
    );
    const button = editor.state.doc.child(0);
    expect(button.type.name).toBe("templateButton");
    expect(button.attrs).toMatchObject({ label: "Hi", position: "below" });
    expect(button.textContent).toBe("a");
  });

  it("gives the mentions a `relative` attribute that is `null` by default and is no text", () => {
    const editor = mount({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "dateMention", attrs: { date: "2026-10-18", relative: "now" } },
            { type: "dateMention", attrs: { date: "2026-10-18" } },
            { type: "personMention", attrs: { accountId: pid(1), label: "Ada", relative: "me" } },
            { type: "personMention", attrs: { accountId: pid(1), label: "Ada" } },
          ],
        },
      ],
    });
    expect(relativesOf(editor, "dateMention")).toEqual(["now", null]);
    expect(relativesOf(editor, "personMention")).toEqual(["me", null]);
    expect(editor.getHTML()).toContain('data-relative="now"');
    expect(editor.getHTML()).toContain('data-relative="me"');
    editor.commands.setContent(
      '<p><span data-type="dateMention" data-date="2026-10-18" data-relative="yesterday"></span><span data-type="personMention" data-account-id="a" data-label="A" data-relative="you"></span></p>',
    );
    expect(relativesOf(editor, "dateMention")).toEqual([null]);
    expect(relativesOf(editor, "personMention")).toEqual([null]);
  });
});

describe("what a click inserts", () => {
  /** The position of the button in a document of `Tasks` and a button. */
  const BUTTON_POS = 7;

  it("is a copy of the content right below the button, with new ids, as one transaction", () => {
    const editor = mount(tasksPage() as JSONContent);
    const before = idsIn(editor);
    expect(editor.state.doc.child(1).type.name).toBe("templateButton");
    const pos = editor.state.doc.child(0).nodeSize;
    expect(pos).toBe(BUTTON_POS);

    const tr = insertTemplateCopy(editor.state, pos, { moment: MOMENT, me: ME });
    expect(tr).not.toBeNull();
    if (tr === null) {
      return;
    }
    let dispatched = 0;
    editor.on("transaction", () => {
      dispatched += 1;
    });
    editor.view.dispatch(tr);
    expect(dispatched).toBe(1);

    expect(outline(editor)).toEqual([
      "paragraph:Tasks",
      "templateButton:Task due 2026-01-01 for @Someone|Second line",
      "paragraph:Task due 2026-10-18 for @Ada Lovelace",
      "paragraph:Second line",
    ]);
    // The button and what it holds are as they were; the copy has ids of its own, all different.
    expect(before.every((id) => idsIn(editor).includes(id))).toBe(true);
    const copyIds = idsIn(editor).filter((id) => !before.includes(id));
    expect(copyIds.length).toBeGreaterThanOrEqual(3); // two paragraphs and the date mention
    expect(new Set(idsIn(editor)).size).toBe(idsIn(editor).length);
    const button = editor.state.doc.child(1);
    expect(button.attrs).toMatchObject({ id: pid(1), label: "Add task" });
    expect(mentionsIn(button, "dateMention")[0]?.attrs).toMatchObject({ date: "2026-01-01", relative: "today", reminder: 30 });
  });

  it("fills in the relative mentions of the copy and nothing else", () => {
    const editor = mount(tasksPage() as JSONContent);
    const tr = insertTemplateCopy(editor.state, 7, { moment: MOMENT, me: ME });
    if (tr === null) {
      throw new Error("no change");
    }
    editor.view.dispatch(tr);

    const copy = editor.state.doc.child(2);
    const date = mentionsIn(copy, "dateMention")[0];
    expect(date?.attrs).toMatchObject({ date: MOMENT.date, time: null, relative: null, reminder: 30 });
    const person = mentionsIn(copy, "personMention")[0];
    expect(person?.attrs).toMatchObject({ accountId: ME.accountId, label: ME.name, relative: null });
    expect(typeof date?.attrs.id).toBe("string");
    expect(date?.attrs.id).not.toBe(pid(3));
  });

  it("fills `now` with the day and the time, and leaves a `me` it cannot resolve alone", () => {
    const editor = mount(
      tasksPage({}, [
        {
          type: "paragraph",
          attrs: { id: pid(2) },
          content: [
            { type: "dateMention", attrs: { id: pid(3), date: "2026-01-01", time: "08:00", relative: "now" } },
            { type: "dateMention", attrs: { id: pid(5), date: "2026-02-02", time: "10:30" } },
            { type: "personMention", attrs: { accountId: pid(90), label: "Someone", relative: "me" } },
          ],
        },
      ]) as JSONContent,
    );
    const button = editor.state.doc.child(1);
    const filled = fillRelative(button.child(0), MOMENT, undefined);
    const dates = mentionsIn(filled, "dateMention");
    expect(dates[0]?.attrs).toMatchObject({ date: MOMENT.date, time: MOMENT.time, relative: null });
    expect(dates[1]?.attrs).toMatchObject({ date: "2026-02-02", time: "10:30", relative: null }); // a fixed date stays
    // Nobody to be: the mention is not turned into someone's.
    expect(mentionsIn(filled, "personMention")[0]?.attrs).toMatchObject({ accountId: pid(90), relative: "me" });
    // Nothing to fill in: the same node.
    const plain = editor.state.doc.child(0);
    expect(fillRelative(plain, MOMENT, ME)).toBe(plain);
  });

  it("goes above the button when its position says so, the caret at the end of the copy", () => {
    const editor = mount(tasksPage({ position: "above" }) as JSONContent);
    const tr = insertTemplateCopy(editor.state, 7, { moment: MOMENT, me: ME });
    if (tr === null) {
      throw new Error("no change");
    }
    editor.view.dispatch(tr);
    expect(outline(editor).map((row) => row.split(":")[0])).toEqual(["paragraph", "paragraph", "paragraph", "templateButton"]);
    expect(outline(editor)[2]).toBe("paragraph:Second line");
    const { $from } = editor.state.selection;
    expect(editor.state.selection.empty).toBe(true);
    expect($from.parent.textContent).toBe("Second line");
    expect($from.parentOffset).toBe("Second line".length);
    expect($from.parent).toBe(editor.state.doc.child(2));
  });

  it("puts the caret at the end of the last block of the copy, in a list too", () => {
    const editor = mount(
      tasksPage({}, [
        { type: "paragraph", attrs: { id: pid(2) }, content: [text("Intro")] },
        {
          type: "bulletList",
          attrs: { id: pid(5) },
          content: [
            { type: "listItem", attrs: { id: pid(6) }, content: [{ type: "paragraph", attrs: { id: pid(7) }, content: [text("one")] }] },
            { type: "listItem", attrs: { id: pid(8) }, content: [{ type: "paragraph", attrs: { id: pid(9) }, content: [text("two")] }] },
          ],
        },
      ]) as JSONContent,
    );
    const tr = insertTemplateCopy(editor.state, 7, { moment: MOMENT, me: ME });
    if (tr === null) {
      throw new Error("no change");
    }
    editor.view.dispatch(tr);
    const { $from } = editor.state.selection;
    expect($from.parent.textContent).toBe("two");
    expect($from.parentOffset).toBe(3);
    expect($from.node(1)).toBe(editor.state.doc.child(3));
    // The copy's lists and items have ids of their own too.
    expect(new Set(idsIn(editor)).size).toBe(idsIn(editor).length);
  });

  it("queues the drawing of a whiteboard in the content for the copy, which gets an id of its own", () => {
    const editor = mount(
      tasksPage({}, [
        { type: "paragraph", attrs: { id: pid(2) }, content: [text("Sketch")] },
        { type: "whiteboard", attrs: { id: pid(5) } },
      ]) as JSONContent,
    );
    const before = whiteboardCopyState().pending;
    const tr = insertTemplateCopy(editor.state, 7, { moment: MOMENT, me: ME });
    if (tr === null) {
      throw new Error("no change");
    }
    editor.view.dispatch(tr);
    expect(whiteboardCopyState().pending).toBe(before + 1);
    const boards = mentionsIn(editor.state.doc, "whiteboard");
    expect(boards).toHaveLength(2);
    expect(boards[0]?.attrs.id).toBe(pid(5));
    expect(boards[1]?.attrs.id).not.toBe(pid(5));
  });

  it("does nothing for a position that is no button", () => {
    const editor = mount(tasksPage() as JSONContent);
    expect(insertTemplateCopy(editor.state, 0, { moment: MOMENT, me: ME })).toBeNull();
    expect(clickTemplateButton(editor, 0, ME)).toBe(false);
    expect(outline(editor)).toHaveLength(2);
  });

  it("reads this account among the people of a space", () => {
    expect(meOf(undefined)).toBeUndefined();
    expect(meOf([{ accountId: "a", name: "A", verified: false, isMe: false }])).toBeUndefined();
    expect(
      meOf([
        { accountId: "a", name: "A", verified: false, isMe: false },
        { accountId: "b", name: "B", verified: false, isMe: true },
      ]),
    ).toEqual({ accountId: "b", name: "B" });
  });
});

// ----- the view, in the app ---------------------------------------------------------------------

function docOf(editor: Editor): LoroDoc {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

async function openTasks(content: JsonValue = tasksPage()) {
  const app = await renderPages([{ name: "Personal", pages: [{ title: "Tasks", content }] }]);
  const editor = await openPage(app.user, "Tasks");
  await waitFor(() => {
    expect(editor.view.dom.querySelector(".template-button-trigger")).not.toBeNull();
  });
  return { ...app, editor };
}

const trigger = (name: string): HTMLElement => screen.getByRole("button", { name });
const fold = (): HTMLElement => screen.getByRole("button", { name: t("templateButton.content") });
const settingsButton = (): HTMLElement => screen.getByRole("button", { name: t("templateButton.settings") });
const frame = (editor: Editor): HTMLElement => {
  const element = editor.view.dom.querySelector(".template-button-frame");
  if (!(element instanceof HTMLElement)) {
    throw new Error("no frame");
  }
  return element;
};

/** The account the in-memory core calls this one, as the people of the space list it. */
async function meIn(memory: MemoryTransport): Promise<{ accountId: string; name: string }> {
  const { space } = await idsOf(memory);
  const people = await new CoreClient(memory).peopleMentionable(space("Personal"));
  const mine = people.find((person) => person.isMe);
  if (mine === undefined) {
    throw new Error("nobody is this account");
  }
  return { accountId: mine.accountId, name: mine.name };
}

describe("the button", () => {
  it("shows the label, or `New item` when there is none, and says where the copy goes", async () => {
    const { editor } = await openTasks();
    expect(trigger("Add task")).toBeEnabled();
    expect(trigger("Add task")).toHaveAttribute("title", t("templateButton.insertsBelow"));
    expect(editor.view.dom.querySelector("[data-type='templateButton']")).toHaveAttribute("data-label", "Add task");

    editor.view.dispatch(editor.state.tr.setNodeAttribute(7, "label", ""));
    editor.view.dispatch(editor.state.tr.setNodeAttribute(7, "position", "above"));
    await waitFor(() => {
      expect(trigger(t("templateButton.defaultLabel"))).toHaveAttribute("title", t("templateButton.insertsAbove"));
    });
  });

  it("inserts the copy below when clicked, in the document and in the core, with the people filled in", async () => {
    vi.useFakeTimers({ toFake: ["Date"], now: new Date(2026, 9, 18, 14, 5, 30) });
    const { user, editor, memory } = await openTasks();
    const me = await meIn(memory);

    await user.click(trigger("Add task"));
    expect(outline(editor)[2]).toBe(`paragraph:Task due 2026-10-18 for @${me.name}`);
    expect(outline(editor)[3]).toBe("paragraph:Second line");
    expect(outline(editor)).toHaveLength(4);

    const copy = editor.state.doc.child(2);
    expect(mentionsIn(copy, "dateMention")[0]?.attrs).toMatchObject({ date: "2026-10-18", relative: null, reminder: 30 });
    expect(mentionsIn(copy, "personMention")[0]?.attrs).toMatchObject({ accountId: me.accountId, label: me.name, relative: null });
    expect(new Set(idsIn(editor)).size).toBe(idsIn(editor).length);
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(3));

    // The page in the core holds the copy as plain blocks after the button, and the button as it was.
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      const content = (await contentOf(memory, page("Tasks"))) as { content: { type: string; attrs: { id: string }; content?: unknown[] }[] };
      expect(content.content.map((block) => block.type)).toEqual(["paragraph", "templateButton", "paragraph", "paragraph"]);
      expect(content.content[1]?.attrs.id).toBe(pid(1));
      expect(content.content[2]?.attrs.id).not.toBe(pid(2));
    });

    // A second click makes a second copy, again with ids that are new.
    await user.click(trigger("Add task"));
    expect(outline(editor)).toHaveLength(6);
    expect(new Set(idsIn(editor)).size).toBe(idsIn(editor).length);
  });

  it("is one undo step, and a redo brings the copy back", async () => {
    const { user, editor } = await openTasks();
    await tick(600);
    await user.click(trigger("Add task"));
    expect(outline(editor)).toHaveLength(4);

    expect(editor.commands.keyboardShortcut("Mod-z")).toBe(true);
    await waitFor(() => {
      expect(outline(editor)).toEqual(["paragraph:Tasks", "templateButton:Task due 2026-01-01 for @Someone|Second line"]);
    });
    expect(editor.commands.keyboardShortcut("Mod-Shift-z")).toBe(true);
    await waitFor(() => {
      expect(outline(editor)).toHaveLength(4);
    });
  });

  it("inserts above when its position says so", async () => {
    const { user, editor } = await openTasks(tasksPage({ position: "above" }));
    await user.click(trigger("Add task"));
    expect(outline(editor).map((row) => row.split(":")[0])).toEqual(["paragraph", "paragraph", "paragraph", "templateButton"]);
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));
  });

  it("is disabled in a read-only editor, which shows no fold-out and no settings", async () => {
    const { user, editor } = await openTasks();
    expect(fold()).toBeInTheDocument();
    expect(settingsButton()).toBeInTheDocument();

    editor.setEditable(false);
    await waitFor(() => {
      expect(trigger("Add task")).toBeDisabled();
    });
    expect(screen.queryByRole("button", { name: t("templateButton.content") })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: t("templateButton.settings") })).not.toBeInTheDocument();
    expect(frame(editor)).not.toBeVisible();
    await user.click(trigger("Add task"));
    expect(outline(editor)).toHaveLength(2);
  });
});

describe("the fold-out", () => {
  it("is closed over content that was written, and a click opens it and closes it", async () => {
    const { user, editor } = await openTasks();
    expect(fold()).toHaveAttribute("aria-expanded", "false");
    expect(frame(editor)).not.toBeVisible();
    expect(frame(editor)).toHaveAttribute("hidden");

    await user.click(fold());
    expect(fold()).toHaveAttribute("aria-expanded", "true");
    expect(frame(editor)).toBeVisible();
    expect(within(frame(editor)).getByText("Second line")).toBeInTheDocument();

    await user.click(fold());
    expect(fold()).toHaveAttribute("aria-expanded", "false");
    expect(frame(editor)).not.toBeVisible();
  });

  it("is open while the button is new, that is while it holds one empty paragraph", async () => {
    const empty: JsonValue[] = [{ type: "paragraph", attrs: { id: pid(2) } }];
    const { editor } = await openTasks(tasksPage({ label: "" }, empty));
    expect(fold()).toHaveAttribute("aria-expanded", "true");
    expect(frame(editor)).toBeVisible();
    expect(trigger(t("templateButton.defaultLabel"))).toBeInTheDocument();
  });

  it("moves a caret that was inside the content to the block after the button when it closes", async () => {
    const { user, editor } = await openTasks();
    await user.click(fold());
    // Inside the first paragraph of the content.
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 7 + 2)));
    expect(editor.state.selection.$from.parent.textContent).toContain("Task due");
    await user.click(fold());
    // The button is the last block: the caret went to the end of the block before it.
    expect(editor.state.selection.$from.parent.textContent).toBe("Tasks");
  });
});

describe("the settings", () => {
  it("opens a popover with the label and the place of the copy, and writes the label on Enter", async () => {
    const { user, editor } = await openTasks();
    const doc = docOf(editor);
    await user.click(settingsButton());
    const panel = await screen.findByRole("dialog", { name: t("templateButton.settings") });
    const field = within(panel).getByRole("textbox", { name: t("templateButton.label") });
    expect(field).toHaveValue("Add task");
    expect(within(panel).getByRole("radio", { name: t("templateButton.insertBelow") })).toBeChecked();
    expect(within(panel).getByRole("radio", { name: t("templateButton.insertAbove") })).not.toBeChecked();

    await user.clear(field);
    await user.type(field, "New chore{Enter}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("templateButton.settings") })).not.toBeInTheDocument();
    });
    expect(editor.state.doc.child(1).attrs).toMatchObject({ id: pid(1), label: "New chore", position: "below" });
    expect(trigger("New chore")).toBeInTheDocument();
    const stored = loroToPortable(doc) as { content: { attrs: Record<string, unknown> }[] };
    expect(stored.content[1]?.attrs).toMatchObject({ id: pid(1), label: "New chore" });
  });

  it("keeps the label typed when the popover is closed with Escape, and cuts it at 80 characters", async () => {
    const { user, editor } = await openTasks();
    await user.click(settingsButton());
    const field = await screen.findByRole("textbox", { name: t("templateButton.label") });
    expect(field).toHaveAttribute("maxlength", String(TEMPLATE_BUTTON_LABEL_MAX_CHARS));
    await user.clear(field);
    await user.type(field, "  Spaced  ");
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(editor.state.doc.child(1).attrs.label).toBe("Spaced");
    });
  });

  it("sets where the copy goes: one attribute, and the next click follows it", async () => {
    const { user, editor } = await openTasks();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(settingsButton());
    const panel = await screen.findByRole("dialog", { name: t("templateButton.settings") });
    await user.click(within(panel).getByRole("radio", { name: t("templateButton.insertAbove") }));
    expect(editor.state.doc.child(1).attrs.position).toBe("above");
    const changes = doc.exportJsonUpdates(before).changes.flatMap((change) => change.ops.map((op) => op.content));
    expect(changes).toEqual([expect.objectContaining({ type: "insert", key: "position", value: "above" })]);

    fireEvent.pointerDown(document.body);
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("templateButton.settings") })).not.toBeInTheDocument();
    });
    await user.click(trigger("Add task"));
    expect(outline(editor).map((row) => row.split(":")[0])).toEqual(["paragraph", "paragraph", "paragraph", "templateButton"]);
  });
});

describe("/button", () => {
  async function typeSlash(typed: string) {
    const app = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: { type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] } }] },
    ]);
    const editor = await openPage(app.user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
    editor.commands.insertContent(typed);
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    return { ...app, editor, menu };
  }

  it("is listed as `Template button` and found by `/button`", async () => {
    const { menu } = await typeSlash("/button");
    await waitFor(() => {
      expect(within(menu).getAllByRole("option")[0]?.querySelector(".suggestion-title")?.textContent).toBe(t("slash.templateButton"));
    });
    expect(within(menu).getAllByRole("option")[0]).toHaveTextContent(t("slash.templateButton.hint"));
  });

  it("inserts a button with one empty paragraph in it, its content open and the caret inside", async () => {
    const { editor } = await typeSlash("/button");
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    await waitFor(() => {
      expect(editor.state.doc.child(0).type.name).toBe("templateButton");
    });
    const button = editor.state.doc.child(0);
    expect(button.attrs).toMatchObject({ label: "", position: "below" });
    expect(button.childCount).toBe(1);
    expect(button.child(0).type.name).toBe("paragraph");
    expect(button.child(0).content.size).toBe(0);
    expect(typeof button.attrs.id).toBe("string");
    // The empty paragraph that held `/button` was replaced, not kept before the button.
    expect(editor.state.doc.child(0).type.name).toBe("templateButton");
    expect(editor.state.selection.$from.parent).toBe(button.child(0));

    await waitFor(() => {
      expect(trigger(t("templateButton.defaultLabel"))).toBeInTheDocument();
    });
    expect(fold()).toHaveAttribute("aria-expanded", "true");
    expect(frame(editor)).toBeVisible();
  });

  it("is not available inside a button's content (it holds no button)", async () => {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Tasks", content: tasksPage() }] }]);
    const editor = await openPage(app.user, "Tasks");
    // The caret at the end of the "Second line" paragraph inside the button, then `/button`.
    const end = editor.state.doc.content.size - 2;
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, end)));
    editor.commands.insertContent(" /button");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    const option = within(menu).getAllByRole("option")[0];
    expect(option).toHaveTextContent(t("slash.templateButton.nested"));
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentionsIn(editor.state.doc, "templateButton")).toHaveLength(1);
  });
});
