// The formula editor (ADR-0055, `docs/architecture/formulas.md` section 9): completion (a list under the field; Up and Down,
// Tab or Enter accepts, Esc closes the list before the dialog), the error shown and underlined, the result type, the live
// preview on a page, Save writing the stored form (ids, not names) and Cancel writing nothing, and the host that builds the
// editor from the store (the first page of the space, the schema, the property created only when saved). The engine is the
// real one: the corpus of `crates/kb-model/tests/formula-cases/` keeps it right, these tests keep the editor right.

import { CoreClient, CoreError, FORMULA_FUNCTIONS, MemoryTransport } from "@atomik-kb/core-client";
import type { PageId, PageInfo, PropertyDef, SeedPage, SpaceId } from "@atomik-kb/core-client";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { describe, expect, it, onTestFinished, vi } from "vitest";

import { t } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { FormulaEditor, FormulaEditorHost } from "./FormulaEditor";
import type { FormulaEditorProps } from "./FormulaEditor";
import { buildFormulaHost, formulaSchemaOf } from "./formula-editor-model";

// ----- the editor over fixtures --------------------------------------------------------------------------------------------

const def = (id: string, name: string, kind: PropertyDef["kind"], extra: Partial<PropertyDef> = {}): PropertyDef => ({
  id,
  name,
  kind,
  options: [],
  createdAt: 0,
  ...extra,
});

const DEFS: readonly PropertyDef[] = [
  def("p-price", "Price", "number"),
  def("p-qty", "Qty", "number"),
  def("p-note", "Note", "text"),
];

const ORDER: PageInfo = {
  id: "page-order",
  spaceId: "s",
  title: "Order",
  createdAt: 1,
  updatedAt: 1,
  properties: [
    ["p-price", { kind: "number", value: 3 }],
    ["p-qty", { kind: "number", value: 4 }],
  ],
};
const PLAIN: PageInfo = { id: "page-plain", spaceId: "s", title: "Plain", createdAt: 1, updatedAt: 1 };

function mountEditor(overrides: Partial<FormulaEditorProps> = {}) {
  const user = userEvent.setup();
  const schema = overrides.schema ?? formulaSchemaOf(DEFS);
  const pages = new Map([ORDER, PLAIN].map((page) => [page.id, page] as const));
  const onSave = vi.fn<(stored: string) => Promise<void>>(() => Promise.resolve());
  const onCancel = vi.fn();
  const props: FormulaEditorProps = {
    name: "Total",
    schema,
    initial: "",
    preview: { host: buildFormulaHost(schema, { pages, people: [] }), page: ORDER.id, title: ORDER.title },
    titleLabel: "Title",
    onSave,
    onCancel,
    ...overrides,
  };
  const view = render(<FormulaEditor {...props} />);
  return { user, onSave, onCancel, ...view };
}

const field = (): HTMLTextAreaElement => screen.getByRole("combobox", { name: t("formula.editor.code") });
const saveButton = (): HTMLElement => screen.getByRole("button", { name: t("formula.editor.save") });
const status = (): HTMLElement => screen.getByRole("status");
const options = (): string[] => screen.queryAllByRole("option").map((option) => within(option).getByText(/./u, { selector: ".formula-completion-label" }).textContent ?? "");

describe("the dialog", () => {
  it("is named by the property, starts empty for a new formula, and cannot be saved yet", () => {
    mountEditor();
    expect(screen.getByRole("dialog", { name: t("formula.editor.title", { name: "Total" }) })).toBeInTheDocument();
    expect(field()).toHaveValue("");
    expect(field()).toHaveFocus();
    expect(saveButton()).toBeDisabled();
    expect(within(status()).getByText(t("formula.editor.empty"))).toBeInTheDocument();
    // An empty formula is not an error to underline.
    expect(field()).toHaveAttribute("aria-invalid", "false");
  });

  it("shows an existing formula with the names of its properties, not their ids", () => {
    mountEditor({ initial: 'prop("p-price") * prop("p-qty")' });
    expect(field()).toHaveValue('prop("Price") * prop("Qty")');
    expect(saveButton()).toBeEnabled();
  });

  it("shows the title column by the label the UI gives it", () => {
    mountEditor({ initial: 'length(prop("title"))', titleLabel: "Title" });
    expect(field()).toHaveValue('length(prop("Title"))');
  });

  it("draws the same text in the mirror under the field", () => {
    const { container } = mountEditor({ initial: 'prop("p-price") * 2' });
    const mirror = container.ownerDocument.querySelector(".formula-mirror");
    expect(mirror).toHaveAttribute("aria-hidden", "true");
    expect(mirror?.textContent?.replace("​", "")).toBe('prop("Price") * 2');
  });

  it("closes with Cancel and with Escape, saving nothing", async () => {
    const { user, onSave, onCancel } = mountEditor();
    await user.click(screen.getByRole("button", { name: t("dialog.cancel") }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onSave).not.toHaveBeenCalled();
  });
});

describe("completion", () => {
  it("proposes a property written as prop(\"Name\") while a word is typed", async () => {
    const { user } = mountEditor();
    await user.type(field(), "Pri");
    const list = screen.getByRole("listbox", { name: t("formula.completion.label") });
    expect(within(list).getByRole("option", { name: /prop\("Price"\)/u })).toBeInTheDocument();
    expect(field()).toHaveAttribute("aria-expanded", "true");
    expect(field()).toHaveAttribute("aria-controls", list.id);
  });

  it("accepts the first proposal with Enter, without a new line, and closes the list", async () => {
    const { user } = mountEditor();
    await user.type(field(), "Pri{Enter}");
    expect(field()).toHaveValue('prop("Price")');
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(field()).toHaveAttribute("aria-expanded", "false");
  });

  it("accepts with Tab and keeps the focus in the field", async () => {
    const { user } = mountEditor();
    await user.type(field(), "Pri{Tab}");
    expect(field()).toHaveValue('prop("Price")');
    expect(field()).toHaveFocus();
  });

  it("chooses with Up and Down, wrapping around, and says which one with aria-activedescendant", async () => {
    const { user } = mountEditor();
    // The functions that start with P, then the properties: `Price` is the last proposal.
    await user.type(field(), "P");
    const all = screen.getAllByRole("option");
    expect(all.length).toBeGreaterThan(2);
    expect(all[0]).toHaveAttribute("aria-selected", "true");
    expect(field()).toHaveAttribute("aria-activedescendant", all[0]?.id);
    await user.keyboard("{ArrowDown}");
    const second = screen.getAllByRole("option")[1];
    expect(second).toHaveAttribute("aria-selected", "true");
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "false");
    expect(field()).toHaveAttribute("aria-activedescendant", second?.id);
    // Up from the second goes to the first, and Up again wraps to the last.
    await user.keyboard("{ArrowUp}{ArrowUp}");
    const last = screen.getAllByRole("option").at(-1);
    expect(last).toHaveAttribute("aria-selected", "true");
    expect(field()).toHaveAttribute("aria-activedescendant", last?.id);
    await user.keyboard("{Enter}");
    expect(field()).toHaveValue('prop("Price")');
  });

  it("accepts a proposal that was clicked", async () => {
    const { user } = mountEditor();
    await user.type(field(), "Not");
    await user.click(screen.getByRole("option", { name: /prop\("Note"\)/u }));
    expect(field()).toHaveValue('prop("Note")');
    expect(field()).toHaveFocus();
  });

  it("proposes the names inside prop(\"…\") and writes the name after the quote that is open", async () => {
    const { user } = mountEditor();
    await user.type(field(), 'prop("');
    expect(options()).toEqual(['prop("Title")', 'prop("Price")', 'prop("Qty")', 'prop("Note")']);
    await user.type(field(), "qt");
    expect(options()).toEqual(['prop("Qty")']);
    await user.keyboard("{Enter}");
    expect(field()).toHaveValue('prop("Qty"');
  });

  it("closes the list with Escape first, and only the next Escape closes the dialog", async () => {
    const { user, onCancel } = mountEditor();
    await user.type(field(), "Pri");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("opens the list again when the word changes after Escape", async () => {
    const { user } = mountEditor();
    await user.type(field(), "Pri{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    await user.type(field(), "c");
    expect(screen.getByRole("option", { name: /prop\("Price"\)/u })).toBeInTheDocument();
  });

  it("leaves Enter alone when there is no list, so that it makes a new line", async () => {
    const { user } = mountEditor();
    await user.type(field(), "1 +{Enter}2");
    expect(field()).toHaveValue("1 +\n2");
  });

  it("proposes the functions of the language", async () => {
    expect(FORMULA_FUNCTIONS.length).toBeGreaterThan(0);
    const { user } = mountEditor();
    await user.type(field(), "rou");
    expect(options()).toContain("round");
    await user.keyboard("{Enter}");
    expect(field().value.startsWith("round(")).toBe(true);
  });
});

describe("the error", () => {
  it("says what is wrong, underlines it in the mirror, and refuses Save", async () => {
    const { user, container } = mountEditor();
    await user.type(field(), "1 +");
    expect(await within(status()).findByText(/./u, { selector: ".formula-error" })).toBeInTheDocument();
    expect(field()).toHaveAttribute("aria-invalid", "true");
    expect(saveButton()).toBeDisabled();
    expect(container.ownerDocument.querySelector(".formula-token-error-range")).not.toBeNull();
  });

  it("names a property that does not exist", async () => {
    const { user, container } = mountEditor();
    await user.type(field(), 'prop("Nope") + 1');
    expect(await within(status()).findByText(t("formula.error.unknown_property", { name: "Nope" }))).toBeInTheDocument();
    // The underline is on the name, in the text as it is written.
    const underlined = [...container.ownerDocument.querySelectorAll(".formula-token-error-range")].map((node) => node.textContent).join("");
    expect(underlined).toContain("Nope");
    expect(saveButton()).toBeDisabled();
  });

  it("names a function that does not exist", async () => {
    const { user } = mountEditor();
    await user.type(field(), "nosuch(1)");
    expect(await within(status()).findByText(t("formula.error.unknown_function", { name: "nosuch" }))).toBeInTheDocument();
  });

  it("is a type error that tells the two types", async () => {
    const { user } = mountEditor();
    await user.type(field(), 'prop("Price") + prop("Price") * "a"');
    expect(await within(status()).findByText(t("formula.error.type", { expected: t("formula.type.number"), found: t("formula.type.text") }))).toBeInTheDocument();
  });

  it("goes away when the formula is fixed, and Save works again", async () => {
    const { user, container } = mountEditor();
    await user.type(field(), "1 +");
    expect(saveButton()).toBeDisabled();
    await user.type(field(), " 2");
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    expect(field()).toHaveAttribute("aria-invalid", "false");
    expect(container.ownerDocument.querySelector(".formula-token-error-range")).toBeNull();
    expect(within(status()).queryByText(/./u, { selector: ".formula-error" })).not.toBeInTheDocument();
  });

  it("is shown for a stored formula that no longer checks (a property it used is gone)", () => {
    mountEditor({ initial: 'prop("p-deleted") * 2' });
    expect(within(status()).getByText(/./u, { selector: ".formula-error" })).toBeInTheDocument();
    expect(saveButton()).toBeDisabled();
  });

  it("is a cycle when a formula reads itself", async () => {
    const schema = formulaSchemaOf([...DEFS, def("p-self", "Self", "formula", { config: { kind: "formula", expression: "1" } })]);
    const { user } = mountEditor({ schema, selfId: "p-self", name: "Self" });
    await user.type(field(), 'prop("Self")');
    expect(await within(status()).findByText(t("formula.error.cycle", { name: "Self" }))).toBeInTheDocument();
  });
});

describe("the result", () => {
  it("says its type", async () => {
    const { user } = mountEditor();
    await user.type(field(), 'prop("Price") * 2');
    expect(await within(status()).findByText(t("formula.result", { type: t("formula.type.number") }))).toBeInTheDocument();
    await user.clear(field());
    await user.type(field(), '"a" + "b"');
    expect(await within(status()).findByText(t("formula.result", { type: t("formula.type.text") }))).toBeInTheDocument();
  });

  it("is previewed on the page, live, as the formula is typed", async () => {
    const { user } = mountEditor();
    const preview = screen.getByRole("region", { name: t("formula.preview.label") });
    expect(within(preview).getByText(t("formula.preview.on", { title: "Order" }))).toBeInTheDocument();
    // Nothing to compute yet, and nothing said about it.
    expect(within(preview).queryByText(t("formula.preview.waiting"))).not.toBeInTheDocument();
    await user.type(field(), 'prop("Price") * prop("Qty")');
    expect(await within(preview).findByText("12")).toBeInTheDocument();
    await user.type(field(), " + 1");
    expect(await within(preview).findByText("13")).toBeInTheDocument();
  });

  it("says there is no result to show while the formula has an error", async () => {
    const { user } = mountEditor();
    const preview = screen.getByRole("region", { name: t("formula.preview.label") });
    await user.type(field(), "1 +");
    expect(await within(preview).findByText(t("formula.preview.waiting"))).toBeInTheDocument();
    await user.type(field(), " 2");
    expect(await within(preview).findByText("3")).toBeInTheDocument();
    expect(within(preview).queryByText(t("formula.preview.waiting"))).not.toBeInTheDocument();
  });

  it("is previewed as text, as a box for a boolean and as a date", async () => {
    const { user } = mountEditor();
    const preview = screen.getByRole("region", { name: t("formula.preview.label") });
    await user.type(field(), '"Total: " + format(prop("Price") + prop("Qty"))');
    expect(await within(preview).findByText("Total: 7")).toBeInTheDocument();
    await user.clear(field());
    await user.type(field(), 'prop("Price") > 2');
    expect(await within(preview).findByRole("checkbox", { name: t("pageList.checked") })).toBeChecked();
    await user.clear(field());
    await user.type(field(), 'prop("Price") > 5');
    expect(await within(preview).findByRole("checkbox", { name: t("pageList.unchecked") })).not.toBeChecked();
  });

  it("is \"Empty\" for a page where the formula has no value", async () => {
    const schema = formulaSchemaOf(DEFS);
    const pages = new Map([ORDER, PLAIN].map((page) => [page.id, page] as const));
    const { user } = mountEditor({ preview: { host: buildFormulaHost(schema, { pages, people: [] }), page: PLAIN.id, title: PLAIN.title } });
    await user.type(field(), 'prop("Price") * 2');
    const preview = screen.getByRole("region", { name: t("formula.preview.label") });
    expect(await within(preview).findByText(t("properties.empty"))).toBeInTheDocument();
  });

  it("says so when there is no page to preview on", () => {
    mountEditor({ preview: null });
    expect(screen.getByText(t("formula.preview.none"))).toBeInTheDocument();
  });
});

describe("saving", () => {
  it("writes the stored form: property ids in place of names, the rest of the text as it was typed", async () => {
    const { user, onSave } = mountEditor();
    await user.type(field(), 'prop("Price")  *  /* tax */ prop("Qty")');
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    await user.click(saveButton());
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledWith('prop("p-price")  *  /* tax */ prop("p-qty")');
  });

  it("writes the title as its own id, by the label the UI gives it", async () => {
    const { user, onSave } = mountEditor({ titleLabel: "Title" });
    await user.type(field(), 'length(prop("Title"))');
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    await user.click(saveButton());
    expect(onSave).toHaveBeenCalledWith('length(prop("title"))');
  });

  it("saves with Ctrl+Enter too", async () => {
    const { user, onSave } = mountEditor();
    await user.type(field(), "1 + 2");
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(onSave).toHaveBeenCalledWith("1 + 2");
  });

  it("does not save a formula with an error, whatever the key", async () => {
    const { user, onSave } = mountEditor();
    await user.type(field(), "1 +");
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(onSave).not.toHaveBeenCalled();
  });

  it("shows a refusal of the core and stays open for another try", async () => {
    const { user, onSave } = mountEditor();
    onSave.mockRejectedValueOnce(new CoreError({ code: "InvalidInput", message: "a property with that name exists" }));
    await user.type(field(), "1 + 2");
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    await user.click(saveButton());
    expect(await screen.findByRole("alert")).toHaveTextContent(t("formula.error.save", { message: "a property with that name exists" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(saveButton()).toBeEnabled();
    await user.click(saveButton());
    expect(onSave).toHaveBeenCalledTimes(2);
  });
});

describe("the warning about names", () => {
  it("is shown when the formula reads a name that two properties have", async () => {
    const twin = [...DEFS, def("p-price-2", "Price", "number")];
    const schema = formulaSchemaOf(twin);
    const { user } = mountEditor({ schema });
    await user.type(field(), 'prop("Qty")');
    expect(screen.queryByText(t("formula.warning.ambiguous", { name: "Price" }))).not.toBeInTheDocument();
    await user.clear(field());
    await user.type(field(), 'prop("Price") * 2');
    expect(screen.getByText(t("formula.warning.ambiguous", { name: "Price" }))).toBeInTheDocument();
  });
});

describe("the reference", () => {
  it("lists the functions by category with their signature, description and example", () => {
    mountEditor();
    const reference = screen.getByRole("region", { name: t("formula.reference.title") });
    expect(within(reference).getAllByRole("group").length).toBeGreaterThan(1);
    const round = FORMULA_FUNCTIONS.find((fn) => fn.name === "round");
    expect(round).toBeDefined();
    if (round !== undefined) {
      const item = within(reference).getByRole("button", { name: new RegExp(`^round`, "u") });
      expect(item).toHaveTextContent(round.example);
      expect(item).toHaveTextContent(t("formula.fn.round"));
      expect(item).toHaveTextContent(round.signatures[0] ?? "");
    }
  });

  it("describes every function of the language, in the person's words", () => {
    for (const fn of FORMULA_FUNCTIONS) {
      expect(t(`formula.fn.${fn.name}` as Parameters<typeof t>[0]), fn.name).not.toBe(`formula.fn.${fn.name}`);
    }
  });

  it("filters by what is typed", async () => {
    const { user } = mountEditor();
    const reference = screen.getByRole("region", { name: t("formula.reference.title") });
    await user.type(within(reference).getByRole("searchbox", { name: t("formula.reference.filter") }), "padstart");
    const buttons = within(reference).getAllByRole("button");
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toHaveTextContent("padStart");
    await user.type(within(reference).getByRole("searchbox"), "zzz");
    expect(within(reference).getByText(t("formula.reference.none"))).toBeInTheDocument();
  });

  it("writes a function at the caret, over the selection, with the caret inside the brackets", async () => {
    const { user } = mountEditor();
    await user.type(field(), "1 + X");
    field().setSelectionRange(4, 5);
    const reference = screen.getByRole("region", { name: t("formula.reference.title") });
    await user.click(within(reference).getByRole("button", { name: new RegExp(`^round`, "u") }));
    expect(field()).toHaveValue("1 + round()");
    expect(field()).toHaveFocus();
    expect(field().selectionStart).toBe("1 + round(".length);
  });
});

// ----- the host: the schema, the pages and the property from the store ---------------------------------------------------------

const SPACE_PAGES: SeedPage[] = [
  { title: "Order", properties: { Price: 3, Qty: 4 } },
  { title: "Other", properties: { Price: 10 } },
];

/** A store over a space with two number properties and two pages, started, and the ids it holds. */
async function mountStore() {
  const memory = new MemoryTransport({
    now: () => 0,
    workspaceName: "W",
    spaces: [
      {
        name: "S",
        properties: [
          { name: "Price", kind: "number" },
          { name: "Qty", kind: "number" },
        ],
        pages: SPACE_PAGES,
      },
    ],
  });
  const client = new CoreClient(memory);
  const store = createAppStore(client);
  onTestFinished(store.getState().start());
  await waitFor(() => {
    expect(store.getState().tree).not.toBeNull();
  });
  const spaceId: SpaceId = store.getState().tree?.spaceId ?? "";
  const pageIds = new Map<string, PageId>((store.getState().tree?.roots ?? []).map((node) => [node.page.title, node.page.id] as const));
  return { memory, client, store, spaceId, pageIds };
}

function mountHost(store: ReturnType<typeof createAppStore>, props: ComponentProps<typeof FormulaEditorHost>) {
  const user = userEvent.setup();
  const view = render(
    <StoreContext value={store}>
      <FormulaEditorHost {...props} />
    </StoreContext>,
  );
  return { user, ...view };
}

describe("the host", () => {
  it("previews on the first page of the space when no page is in front, and follows a page given", async () => {
    const { store, spaceId } = await mountStore();
    const { user } = mountHost(store, { spaceId, name: "Total", onSaved: vi.fn(), onClose: vi.fn() });
    const preview = await screen.findByRole("region", { name: t("formula.preview.label") });
    expect(within(preview).getByText(t("formula.preview.on", { title: "Order" }))).toBeInTheDocument();
    await user.type(field(), 'prop("Price") * 2');
    expect(await within(preview).findByText("6")).toBeInTheDocument();
  });

  it("previews on the pages it is given first (the rows of a table)", async () => {
    const { store, spaceId } = await mountStore();
    const other = (store.getState().tree?.roots ?? []).find((node) => node.page.title === "Other")?.page;
    expect(other).toBeDefined();
    const { user } = mountHost(store, { spaceId, name: "Total", previewPages: other === undefined ? [] : [other], onSaved: vi.fn(), onClose: vi.fn() });
    const preview = await screen.findByRole("region", { name: t("formula.preview.label") });
    expect(within(preview).getByText(t("formula.preview.on", { title: "Other" }))).toBeInTheDocument();
    await user.type(field(), 'prop("Price") * 2');
    expect(await within(preview).findByText("20")).toBeInTheDocument();
  });

  it("creates the property only when saved, with the stored expression", async () => {
    const { store, spaceId, client } = await mountStore();
    const onSaved = vi.fn();
    const { user } = mountHost(store, { spaceId, name: "Total", onSaved, onClose: vi.fn() });
    await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Total" }) });
    // Nothing exists while the editor is open.
    expect((await client.propertySchemaGet(spaceId)).map((entry) => entry.name)).not.toContain("Total");
    await user.type(field(), 'prop("Price") * prop("Qty")');
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    await user.click(saveButton());
    await waitFor(() => {
      expect(onSaved).toHaveBeenCalledTimes(1);
    });
    const schema = await client.propertySchemaGet(spaceId);
    const created = schema.find((entry) => entry.name === "Total");
    const price = schema.find((entry) => entry.name === "Price");
    const qty = schema.find((entry) => entry.name === "Qty");
    expect(created?.kind).toBe("formula");
    expect(created?.config).toMatchObject({ kind: "formula", expression: `prop("${price?.id ?? ""}") * prop("${qty?.id ?? ""}")` });
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ name: "Total", kind: "formula" }));
  });

  it("creates nothing when cancelled", async () => {
    const { store, spaceId, client } = await mountStore();
    const onClose = vi.fn();
    const { user } = mountHost(store, { spaceId, name: "Total", onSaved: vi.fn(), onClose });
    await screen.findByRole("dialog");
    await user.type(field(), "1 + 2");
    await user.click(screen.getByRole("button", { name: t("dialog.cancel") }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect((await client.propertySchemaGet(spaceId)).map((entry) => entry.name)).not.toContain("Total");
  });

  it("changes the expression of an existing formula, shown with names, and keeps the others as they are", async () => {
    const { store, spaceId, client } = await mountStore();
    const schema = await client.propertySchemaGet(spaceId);
    const price = schema.find((entry) => entry.name === "Price");
    // Through the store, which then holds the schema (and the new property in it).
    const made = await store.getState().createProperty(spaceId, "Double", "formula", { kind: "formula", expression: `prop("${price?.id ?? ""}") * 2` });
    const current = store.getState().propertySchemas[spaceId]?.find((entry) => entry.id === made.id);
    expect(current).toBeDefined();
    const onSaved = vi.fn();
    const { user } = mountHost(store, { spaceId, def: current, onSaved, onClose: vi.fn() });
    await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Double" }) });
    expect(field()).toHaveValue('prop("Price") * 2');
    await user.clear(field());
    await user.type(field(), 'prop("Price") * 3');
    await waitFor(() => {
      expect(saveButton()).toBeEnabled();
    });
    await user.click(saveButton());
    await waitFor(() => {
      expect(onSaved).toHaveBeenCalledTimes(1);
    });
    const after = (await client.propertySchemaGet(spaceId)).find((entry) => entry.id === made.id);
    expect(after?.name).toBe("Double");
    expect(after?.config).toMatchObject({ kind: "formula", expression: `prop("${price?.id ?? ""}") * 3` });
  });
});
