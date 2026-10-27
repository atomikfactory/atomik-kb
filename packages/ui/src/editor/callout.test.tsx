// The callout's colour (ADR-0016, amendment of 2026-10-06): the `color` attribute in the
// schema (parse, render, default, unknown names), the colour button and its swatch popover
// in the page editor (one attribute change per pick, the block id stays, nothing for a
// read-only editor) and what the choices do to a bound Loro document.

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { loroDocFrom, pid, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { CALLOUT_COLORS, DEFAULT_CALLOUT_ICON, isCalloutColor } from "./callout";
import { LoroBinding } from "./extensions";
import { createNodeFromLoroObj, loroSyncPluginKey, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode } from "./loro-binding";
import { createEditorSchema, createSchemaExtensions } from "./schema";

const NAMES = ["Gray", "Brown", "Orange", "Yellow", "Green", "Blue", "Purple", "Pink", "Red"];

/** `Before`, then a callout (`id` pid(1)) holding one paragraph `Note`. */
function calloutPage(attrs: Record<string, JsonValue> = {}): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
      {
        type: "callout",
        attrs: { id: pid(1), icon: DEFAULT_CALLOUT_ICON, ...attrs },
        content: [{ type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "Note" }] }],
      },
    ],
  };
}

/** Every op `doc` recorded since `since`, grouped by change. */
function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

/** The Loro document the page editor is bound to. */
function docOf(editor: Editor): LoroDoc {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

/** The callout's attributes, as the editor holds them. */
function calloutAttrs(editor: Editor): Record<string, unknown> {
  return editor.state.doc.child(1).attrs;
}

/** The callout's element in the editor (the attribute is empty, not absent, for the default colour). */
function calloutElement(editor: Editor): HTMLElement {
  const element = editor.view.dom.querySelector("[data-type='callout']");
  if (!(element instanceof HTMLElement)) {
    throw new Error("no callout");
  }
  return element;
}

function dataColor(editor: Editor): string {
  return calloutElement(editor).getAttribute("data-color") ?? "";
}

function colorButton(): HTMLElement {
  return screen.getByRole("button", { name: t("callout.color") });
}

async function findPicker(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: t("callout.colorTitle") });
}

function pickerIsOpen(): boolean {
  return screen.queryByRole("dialog", { name: t("callout.colorTitle") }) !== null;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the attribute", () => {
  const mounted: { editor: Editor; element: HTMLElement }[] = [];

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

  afterEach(() => {
    for (const { editor, element } of mounted.splice(0)) {
      editor.destroy();
      element.remove();
    }
  });

  /** The editor's initial content, read from the document like the page editor does. */
  function contentOfLoro(doc: LoroDoc): JSONContent {
    const root = doc.getMap(ROOT_DOC_KEY);
    return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
  }

  it("names the palette: nine colours, in the order of the picker", () => {
    expect([...CALLOUT_COLORS]).toEqual(["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"]);
    expect(CALLOUT_COLORS.every((color) => isCalloutColor(color))).toBe(true);
    for (const other of ["", "Blue", "chartreuse", null, undefined, 3]) {
      expect(isCalloutColor(other), String(other)).toBe(false);
    }
  });

  it("is `null` by default, after `id` and `icon`, and the schema matches the regenerated fixture", () => {
    const schema = createEditorSchema();
    const callout = schema.nodes.callout;
    expect(Object.keys(callout?.spec.attrs ?? {})).toEqual(["id", "icon", "color"]);
    expect(callout?.createAndFill()?.attrs).toMatchObject({ icon: DEFAULT_CALLOUT_ICON, color: null });

    const fixture = (blockSchemaFixture as unknown as BlockSchemaFixture).nodes.callout;
    expect(describeSchema(schema).nodes.callout).toEqual(fixture);
    expect(fixture?.attrs).toEqual(["id", "icon", "color"]);
  });

  it("renders `data-color` for a colour, and nothing for none", () => {
    const editor = mount(calloutPage({ color: "purple" }) as JSONContent);
    expect(calloutElement(editor)).toHaveAttribute("data-color", "purple");
    expect(calloutElement(editor)).toHaveAttribute("data-icon", DEFAULT_CALLOUT_ICON);

    const plain = mount(calloutPage() as JSONContent);
    expect(calloutElement(plain)).not.toHaveAttribute("data-color");
    expect(calloutAttrs(plain).color).toBeNull();
    // What is copied out of the editor carries the colour too.
    expect(editor.getHTML()).toContain('data-color="purple"');
    expect(plain.getHTML()).not.toContain("data-color");
  });

  it("parses `data-color` from HTML (a paste), and an empty one as none", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setContent(
      '<div data-type="callout" data-icon="📌" data-color="orange"><div class="callout-content"><p>a</p></div></div>' +
        '<div data-type="callout" data-icon="📌" data-color=""><div class="callout-content"><p>b</p></div></div>' +
        '<div data-type="callout"><div class="callout-content"><p>c</p></div></div>',
    );
    const colors: unknown[] = [];
    editor.state.doc.forEach((node) => {
      colors.push(node.attrs.color);
    });
    expect(colors).toEqual(["orange", null, null]);
  });

  it("keeps a colour this version does not know: no tint is a style rule's business, and no edit rewrites it", () => {
    const editor = mount(calloutPage({ color: "chartreuse" }) as JSONContent);
    expect(calloutAttrs(editor).color).toBe("chartreuse");
    expect(calloutElement(editor)).toHaveAttribute("data-color", "chartreuse");

    // An unrelated edit (typing in the callout, changing its icon) leaves it alone.
    editor.view.dispatch(editor.state.tr.insertText("!", 1 + editor.state.doc.child(0).nodeSize + 1 + "Note".length));
    expect(editor.state.doc.child(1).textContent).toBe("Note!");
    editor.view.dispatch(editor.state.tr.setNodeAttribute(editor.state.doc.child(0).nodeSize, "icon", "📌"));
    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: "📌", color: "chartreuse" });
  });

  it("writes nothing to a bound document when a callout with or without a colour is loaded", async () => {
    const variants: Record<string, JsonValue>[] = [{}, { color: "blue" }, { color: "chartreuse" }];
    for (const attrs of variants) {
      const doc = loroDocFrom(calloutPage(attrs));
      const before = doc.oplogVersion();
      const editor = mount(contentOfLoro(doc), doc);
      await tick();
      await tick();

      expect(editor.state.doc.child(1).attrs.color).toBe(attrs.color ?? null);
      expect(changesSince(doc, before), JSON.stringify(attrs)).toEqual([]);
    }
  });

  it("does not store the default: a callout without a colour holds no `color` key", () => {
    const doc = loroDocFrom(calloutPage());
    expect(loroToPortable(doc)).toEqual(calloutPage());
    const stored = JSON.stringify(loroToPortable(doc));
    expect(stored).not.toContain("color");
  });

  it("leaves a stored unknown colour in the document through an unrelated edit", async () => {
    const doc = loroDocFrom(calloutPage({ color: "chartreuse" }));
    const editor = mount(contentOfLoro(doc), doc);
    await tick();
    await tick();

    const before = doc.oplogVersion();
    editor.view.dispatch(editor.state.tr.insertText("!", 1 + editor.state.doc.child(0).nodeSize + 1 + "Note".length));
    const ops = changesSince(doc, before);
    expect(ops.length).toBeGreaterThan(0);
    expect(JSON.stringify(ops)).not.toContain('"color"');
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        {
          type: "callout",
          attrs: { id: pid(1), icon: DEFAULT_CALLOUT_ICON, color: "chartreuse" },
          content: [{ type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "Note!" }] }],
        },
      ],
    });
  });
});

describe("the colour button and the swatch popover", () => {
  async function openCalloutPage(attrs: Record<string, JsonValue> = {}) {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: calloutPage(attrs) }] }]);
    const editor = await openPage(app.user, "Notes");
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-type='callout'] .callout-icon")).not.toBeNull();
    });
    return { ...app, editor };
  }

  it("is a button next to the icon, named and with a popover, not drawn tinted when there is no colour", async () => {
    const { editor } = await openCalloutPage();
    const button = colorButton();

    expect(button).toHaveAccessibleName(t("callout.color"));
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button.previousElementSibling).toHaveClass("callout-icon");
    expect(button.closest("[data-type='callout']")).toBe(calloutElement(editor));
    expect(button.querySelector(".callout-swatch")).not.toHaveAttribute("data-color");
    expect(dataColor(editor)).toBe("");
    expect(pickerIsOpen()).toBe(false);
  });

  it("lists the nine colours by name and Default, and marks the one the callout has", async () => {
    const { user } = await openCalloutPage({ color: "green" });
    expect(colorButton().querySelector(".callout-swatch")).toHaveAttribute("data-color", "green");

    await user.click(colorButton());
    const picker = await findPicker();
    expect(colorButton()).toHaveAttribute("aria-expanded", "true");

    const swatches = within(picker).getAllByRole("button");
    expect(swatches.map((swatch) => swatch.getAttribute("aria-label") ?? swatch.textContent)).toEqual([
      ...NAMES,
      t("callout.color.default"),
    ]);
    for (const [index, key] of (
      [
        "callout.color.gray",
        "callout.color.brown",
        "callout.color.orange",
        "callout.color.yellow",
        "callout.color.green",
        "callout.color.blue",
        "callout.color.purple",
        "callout.color.pink",
        "callout.color.red",
      ] satisfies MessageKey[]
    ).entries()) {
      expect(swatches[index]).toHaveAccessibleName(t(key));
      expect(swatches[index]?.querySelector(".callout-swatch")).toHaveAttribute("data-color", CALLOUT_COLORS[index]);
    }
    const pressed = swatches.filter((swatch) => swatch.getAttribute("aria-pressed") === "true");
    expect(pressed.map((swatch) => swatch.getAttribute("aria-label"))).toEqual([t("callout.color.green")]);
  });

  it("sets `color` and nothing else: the block id and icon stay, one attribute is written", async () => {
    const { user, editor, memory } = await openCalloutPage();
    const doc = docOf(editor);

    const before = doc.oplogVersion();
    await user.click(colorButton());
    await user.click(within(await findPicker()).getByRole("button", { name: "Blue" }));

    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: DEFAULT_CALLOUT_ICON, color: "blue" });
    expect(dataColor(editor)).toBe("blue");
    expect(calloutElement(editor)).toHaveAttribute("data-icon", DEFAULT_CALLOUT_ICON);
    expect(editor.state.doc.child(1).textContent).toBe("Note");
    expect(changesSince(doc, before)).toEqual([[expect.objectContaining({ type: "insert", key: "color", value: "blue" })]]);
    expect(loroToPortable(doc)).toEqual(calloutPage({ color: "blue" }));
    expect(pickerIsOpen()).toBe(false); // closed by the pick

    // The next pick replaces it (still one write), and the picker shows the new current one.
    const beforeRed = doc.oplogVersion();
    await user.click(colorButton());
    const picker = await findPicker();
    expect(within(picker).getByRole("button", { name: "Blue" })).toHaveAttribute("aria-pressed", "true");
    await user.click(within(picker).getByRole("button", { name: "Red" }));
    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: DEFAULT_CALLOUT_ICON, color: "red" });
    expect(dataColor(editor)).toBe("red");
    expect(changesSince(doc, beforeRed)).toEqual([[expect.objectContaining({ type: "insert", key: "color", value: "red" })]]);

    // It reaches the core with the block ids the page had.
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toEqual(calloutPage({ color: "red" }));
    });
  });

  it("goes back to the default with Default: `null` in the editor, the key deleted, no `data-color`", async () => {
    const { user, editor } = await openCalloutPage({ color: "pink" });
    const doc = docOf(editor);
    expect(dataColor(editor)).toBe("pink");

    const before = doc.oplogVersion();
    await user.click(colorButton());
    const picker = await findPicker();
    expect(within(picker).getByRole("button", { name: t("callout.color.default") })).toHaveAttribute("aria-pressed", "false");
    await user.click(within(picker).getByRole("button", { name: t("callout.color.default") }));

    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: DEFAULT_CALLOUT_ICON, color: null });
    expect(dataColor(editor)).toBe(""); // overwritten: the old value must not stay on the element
    expect(colorButton().querySelector(".callout-swatch")).not.toHaveAttribute("data-color");
    expect(changesSince(doc, before)).toEqual([[expect.objectContaining({ type: "delete", key: "color" })]]);
    const stored = loroToPortable(doc);
    expect(stored).toEqual(calloutPage());
    expect(JSON.stringify(stored)).not.toContain("color");

    // Reopened, Default is the marked one; picking it again changes and writes nothing.
    await user.click(colorButton());
    const again = await findPicker();
    expect(within(again).getByRole("button", { name: t("callout.color.default") })).toHaveAttribute("aria-pressed", "true");
    const beforeAgain = doc.oplogVersion();
    await user.click(within(again).getByRole("button", { name: t("callout.color.default") }));
    expect(changesSince(doc, beforeAgain)).toEqual([]);
  });

  it("closes on Escape, on a press outside and after a pick, and gives focus back to the button", async () => {
    const { user, editor } = await openCalloutPage();

    await user.click(colorButton());
    await findPicker();
    await user.keyboard("{Escape}");
    expect(pickerIsOpen()).toBe(false);
    expect(colorButton()).toHaveFocus();

    await user.click(colorButton());
    await findPicker();
    fireEvent.pointerDown(document.body);
    await waitFor(() => {
      expect(pickerIsOpen()).toBe(false);
    });

    // Pressing the button again closes it (the button is not "outside").
    await user.click(colorButton());
    await findPicker();
    await user.click(colorButton());
    expect(pickerIsOpen()).toBe(false);

    await user.click(colorButton());
    await user.click(within(await findPicker()).getByRole("button", { name: "Yellow" }));
    expect(pickerIsOpen()).toBe(false);
    expect(colorButton()).toHaveAttribute("aria-expanded", "false");
    expect(calloutAttrs(editor).color).toBe("yellow");
    // None of it changed the text.
    expect([editor.state.doc.child(0).textContent, editor.state.doc.child(1).textContent]).toEqual(["Before", "Note"]);
  });

  it("can be used from the keyboard: Enter opens it with focus on the first colour, Tab reaches every choice", async () => {
    const { user, editor } = await openCalloutPage();

    colorButton().focus();
    await user.keyboard("{Enter}");
    const picker = await findPicker();
    const swatches = within(picker).getAllByRole("button");
    expect(swatches[0]).toHaveFocus();
    for (const swatch of swatches.slice(1)) {
      await user.tab();
      expect(swatch).toHaveFocus();
    }

    // Enter on a focused colour picks it.
    swatches[2]?.focus();
    await user.keyboard("{Enter}");
    expect(calloutAttrs(editor).color).toBe("orange");
    expect(pickerIsOpen()).toBe(false);
  });

  it("is not offered in a read-only editor, which still shows the colour", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = calloutPage({ color: "red" }) as { content: JsonValue[] };
    const withUnknownBlock: JsonValue = {
      type: "doc",
      content: [...content.content, { type: "kanban", attrs: { id: pid(9) } }],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Board", content: withUnknownBlock }] }]);
    const editor = await openPage(user, "Board");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);

    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-type='callout'] .callout-icon")).not.toBeNull();
    });
    expect(dataColor(editor)).toBe("red");
    expect(screen.queryByRole("button", { name: t("callout.color") })).not.toBeInTheDocument();
    expect(editor.view.dom.querySelector(".callout-color-button")).toBeNull();
    expect(pickerIsOpen()).toBe(false);
  });

  it("shows a colour it does not know as none, leaves it through an unrelated edit and replaces it on a pick", async () => {
    const { user, editor } = await openCalloutPage({ color: "chartreuse" });
    const doc = docOf(editor);
    expect(calloutAttrs(editor).color).toBe("chartreuse");
    // No swatch is drawn for it (the tint comes from the known names only).
    expect(colorButton().querySelector(".callout-swatch")).not.toHaveAttribute("data-color");

    const before = doc.oplogVersion();
    editor.view.dispatch(editor.state.tr.insertText("!", 1 + editor.state.doc.child(0).nodeSize + 1 + "Note".length));
    await user.click(colorButton());
    const picker = await findPicker();
    expect(within(picker).getAllByRole("button").some((button) => button.getAttribute("aria-pressed") === "true")).toBe(false);
    await user.keyboard("{Escape}");
    expect(JSON.stringify(changesSince(doc, before))).not.toContain('"color"');
    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: DEFAULT_CALLOUT_ICON, color: "chartreuse" });

    const beforePick = doc.oplogVersion();
    await user.click(colorButton());
    await user.click(within(await findPicker()).getByRole("button", { name: "Blue" }));
    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: DEFAULT_CALLOUT_ICON, color: "blue" });
    expect(changesSince(doc, beforePick)).toEqual([[expect.objectContaining({ type: "insert", key: "color", value: "blue" })]]);
  });

  it("changes the icon without touching the colour, and the colour without touching the icon", async () => {
    const { user, editor } = await openCalloutPage({ color: "brown" });

    await user.click(screen.getByRole("button", { name: t("callout.icon") }));
    await user.click(within(await screen.findByRole("dialog", { name: t("icon.title") })).getByRole("button", { name: "📌" }));
    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: "📌", color: "brown" });
    expect(dataColor(editor)).toBe("brown");

    await user.click(colorButton());
    await user.click(within(await findPicker()).getByRole("button", { name: "Gray" }));
    expect(calloutAttrs(editor)).toEqual({ id: pid(1), icon: "📌", color: "gray" });
  });
});
