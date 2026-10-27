// The photo gallery block (ADR-0030, ADR-0016 amendment of 2026-10-09): the pure rules (reading
// the attributes, moving, captions, Markdown), the schema in HTML, `/gallery`, the grid, the
// toolbar (columns, the several-file chooser), captions, removing, moving by button and by drag,
// adding pictures by the chooser, by paste and by drop, the full-size viewer that steps through
// the gallery, read-only editors, and that every picture counts as referenced. Whole-app tests
// over the in-memory core, like image-layout.test.tsx.

import { CoreClient, MAX_ATTACHMENT_BYTES, demoImagePng } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport, MemoryTransportOptions } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { getSchema } from "@tiptap/core";
import { DOMParser as PmDOMParser, DOMSerializer } from "@tiptap/pm/model";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatBytes } from "../format";
import { t } from "../i18n";
import { MIB, dropFiles, pasteFiles, pngFile, sizedFile, stubObjectUrls, textFile } from "../test/attachments";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { galleryItemsOf, galleryMarkdown, moveItem, readGalleryColumns, readGalleryItems, withCaption } from "./gallery-nodes";
import { GALLERY_DRAG_TYPE, galleryStopsEvent } from "./GalleryView";
import { createSchemaExtensions } from "./schema";

afterEach(() => {
  vi.restoreAllMocks();
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const DOG = "a0000000-0000-4000-8000-0000000000d0";
const GHOST = "a0000000-0000-4000-8000-0000000000fe";

const SEED: MemoryTransportOptions["attachments"] = [
  { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() },
  { attachmentId: DOG, name: "dog.png", mime: "image/png", bytes: demoImagePng() },
];

type Item = { attachmentId: string; caption?: string };

/** A page of a paragraph and a gallery (block `pid(1)`) with these items. */
function pageWith(items: Item[], columns = 3): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Trip:" }] },
      { type: "gallery", attrs: { id: pid(1), columns, items } },
    ],
  };
}

const space = (content: JsonValue) => [{ name: "Personal", pages: [{ title: "Notes", content }] }];

/** Opens the page and waits until `count` pictures are drawn. */
async function openGallery(items: Item[], { columns = 3, count = items.length }: { columns?: number; count?: number } = {}) {
  const urls = stubObjectUrls();
  const app = await renderPages(space(pageWith(items, columns)), { attachments: SEED });
  const editor = await openPage(app.user, "Notes");
  await waitFor(() => {
    expect(editor.view.dom.querySelectorAll("img")).toHaveLength(count);
  });
  return { ...app, editor, urls };
}

function galleryElement(editor: Editor): HTMLElement {
  const element = editor.view.dom.querySelector(".gallery-block");
  if (!(element instanceof HTMLElement)) {
    throw new Error("no gallery");
  }
  return element;
}

/** The gallery of the page (blocks dropped or pasted on the page itself land before it). */
function galleryNode(editor: Editor) {
  let found = null as ReturnType<Editor["state"]["doc"]["child"]> | null;
  editor.state.doc.forEach((node) => {
    if (node.type.name === "gallery") {
      found = node;
    }
  });
  if (found === null) {
    throw new Error("no gallery node");
  }
  return found;
}

/** Where the gallery starts in the document. */
function galleryPos(editor: Editor): number {
  let at = 0;
  editor.state.doc.forEach((node, offset) => {
    if (node.type.name === "gallery") {
      at = offset;
    }
  });
  return at;
}

function attrsOf(editor: Editor): Record<string, unknown> {
  return galleryNode(editor).attrs;
}

function itemsOf(editor: Editor): Item[] {
  return galleryItemsOf(galleryNode(editor));
}

/** The gallery as the core holds it. */
async function stored(memory: MemoryTransport): Promise<{ items: Item[]; columns: unknown }> {
  const { page } = await idsOf(memory);
  const content = (await contentOf(memory, page("Notes"))) as { content: { type: string; attrs?: Record<string, unknown> }[] };
  const gallery = content.content.find((node) => node.type === "gallery");
  return { items: (gallery?.attrs?.items ?? []) as Item[], columns: gallery?.attrs?.columns };
}

/** The tiles of the grid, in order. */
function tiles(element: HTMLElement): HTMLElement[] {
  return Array.from(element.querySelectorAll<HTMLElement>(".gallery-item"));
}

/** Chooses files in the hidden file input of the gallery. */
function choose(element: HTMLElement, files: File[]): void {
  const input = within(element).getByTestId("gallery-file-input");
  Object.defineProperty(input, "files", { configurable: true, value: files });
  fireEvent.change(input);
}

describe("the rules", () => {
  it("read the items leniently: maps with a valid attachment, captions only if they are text", () => {
    expect(
      readGalleryItems([
        { attachmentId: CAT, caption: "One" },
        { attachmentId: DOG },
        { attachmentId: CAT.toUpperCase().replaceAll("-", ""), caption: "" },
        { attachmentId: "nope", caption: "Lost" },
        { caption: "no picture" },
        { attachmentId: DOG, caption: 7 },
        "x",
        null,
        [],
      ]),
    ).toEqual([
      { attachmentId: CAT, caption: "One" },
      { attachmentId: DOG },
      { attachmentId: CAT },
      { attachmentId: DOG },
    ]);
    expect(readGalleryItems("items")).toEqual([]);
    expect(readGalleryItems(undefined)).toEqual([]);
  });

  it("read the columns as 2, 3 or 4, else 3", () => {
    expect([2, 3, 4].map(readGalleryColumns)).toEqual([2, 3, 4]);
    expect(["2", "4"].map(readGalleryColumns)).toEqual([2, 4]);
    for (const value of [1, 5, 2.5, "x", null, undefined, true]) {
      expect(readGalleryColumns(value), String(value)).toBe(3);
    }
  });

  it("move an item, set or clear a caption", () => {
    const items = [{ attachmentId: CAT }, { attachmentId: DOG }, { attachmentId: GHOST }];
    expect(moveItem(items, 0, 2).map((item) => item.attachmentId)).toEqual([DOG, GHOST, CAT]);
    expect(moveItem(items, 2, 0).map((item) => item.attachmentId)).toEqual([GHOST, CAT, DOG]);
    expect(moveItem(items, 1, 1)).toEqual(items);
    expect(moveItem(items, 0, 3)).toEqual(items);
    expect(moveItem(items, 0, -1)).toEqual(items);
    expect(withCaption({ attachmentId: CAT }, "  A\ncat  ")).toEqual({ attachmentId: CAT, caption: "A cat" });
    expect(withCaption({ attachmentId: CAT, caption: "Old" }, "   ")).toEqual({ attachmentId: CAT });
  });
});

describe("the node", () => {
  const schema = getSchema(createSchemaExtensions());

  it("is a leaf block with items and columns, and keeps them through HTML", () => {
    const node = schema.nodes.gallery?.create({
      id: pid(1),
      columns: 4,
      items: [{ attachmentId: CAT, caption: 'Say "hi"' }, { attachmentId: DOG }],
    });
    expect(node?.isLeaf).toBe(true);
    expect(node?.type.isAtom).toBe(true);
    expect(schema.nodes.gallery?.spec.group).toBe("block");

    // Parsed back with the lenient rules: a gallery pasted from HTML keeps its usable pictures.
    const source = document.createElement("div");
    source.innerHTML = `<div data-type="gallery" data-columns="2" data-items='[{"attachmentId":"${CAT}","caption":"Hi"},{"attachmentId":"bad"}]'>Hi</div>`;
    const parsed = PmDOMParser.fromSchema(schema).parse(source).firstChild;
    expect(parsed?.type.name).toBe("gallery");
    expect(parsed?.attrs.columns).toBe(2);
    expect(parsed?.attrs.items).toEqual([{ attachmentId: CAT, caption: "Hi" }]);
    expect(Object.keys(schema.nodes.gallery?.spec.attrs ?? {})).toEqual(["id", "items", "columns"]);
    const html = node === undefined ? null : DOMSerializer.fromSchema(schema).serializeNode(node);
    expect(html).toBeInstanceOf(HTMLElement);
    expect((html as HTMLElement).getAttribute("data-type")).toBe("gallery");
    expect((html as HTMLElement).getAttribute("data-columns")).toBe("4");
    expect((html as HTMLElement).textContent).toBe('Say "hi"');
  });

  it("is plain text as the Markdown the core writes: one image line per item", () => {
    const node = schema.nodes.gallery?.create({
      columns: 2,
      items: [{ attachmentId: CAT, caption: 'She said "hi" at C:\\temp' }, { attachmentId: DOG }],
    });
    expect(node === undefined ? "" : galleryMarkdown(node)).toBe(
      `![](attachment://${CAT} "She said \\"hi\\" at C:\\\\temp")\n\n![](attachment://${DOG})`,
    );
  });
});

describe("/gallery", () => {
  it("inserts an empty gallery with an id and the defaults, and a paragraph below it", async () => {
    stubObjectUrls();
    const { user, memory } = await renderPages(space({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: pid(0) } }],
    }));
    const editor = await openPage(user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
    editor.commands.insertContent("/gallery");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    // The Gallery view entry (ADR-0054) starts with the word, so it ranks first; the photo gallery
    // is found among the results all the same.
    await waitFor(() => {
      const titles = within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent);
      expect(titles).toContain("Photo gallery");
    });
    await user.click(within(menu).getByText("Photo gallery"));

    const gallery = editor.state.doc.child(0);
    expect(gallery.type.name).toBe("gallery");
    expect(gallery.attrs.items).toEqual([]);
    expect(gallery.attrs.columns).toBe(3);
    expect(typeof gallery.attrs.id).toBe("string");
    expect(editor.state.doc.lastChild?.type.name).toBe("paragraph");
    expect(within(galleryElement(editor)).getByText(t("gallery.empty"))).toBeInTheDocument();
    await waitFor(async () => {
      expect(await stored(memory)).toEqual({ items: [], columns: 3 });
    });
  });

  it("is found by photos, pictures and album", async () => {
    const { filterSlashCommands } = await import("./slash-commands");
    for (const word of ["gallery", "photo", "photos", "pictures", "album"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("gallery");
    }
    // "Gallery view" (ADR-0054) is first for the start of the word; the photo gallery follows it.
    expect(filterSlashCommands("gall").map((command) => command.id)).toContain("gallery");
  });
});

describe("the grid", () => {
  it("shows every picture from its bytes, in order, with the columns of the block", async () => {
    const { editor } = await openGallery([{ attachmentId: CAT, caption: "Sunrise" }, { attachmentId: DOG }, { attachmentId: CAT }], { columns: 2 });
    const element = galleryElement(editor);
    expect(element).toHaveAttribute("data-columns", "2");
    expect(element.querySelector<HTMLElement>(".gallery-grid")?.style.gridTemplateColumns).toBe("repeat(2, minmax(0, 1fr))");
    const cells = tiles(element);
    expect(cells).toHaveLength(3);
    for (const img of Array.from(element.querySelectorAll("img"))) {
      expect(img).toHaveAttribute("src", expect.stringMatching(/^blob:test\//));
    }
    expect(within(cells[0] as HTMLElement).getByRole("textbox", { name: t("gallery.caption.label", { n: 1 }) })).toHaveValue("Sunrise");
    expect(within(cells[1] as HTMLElement).getByRole("textbox", { name: t("gallery.caption.label", { n: 2 }) })).toHaveValue("");
  });

  it("shows a placeholder for a picture that was deleted and one too large to preview, and still the others", async () => {
    stubObjectUrls();
    const BIG = "a0000000-0000-4000-8000-0000000000b1";
    const bytes = new Uint8Array(33 * MIB);
    bytes.set(demoImagePng());
    const app = await renderPages(
      space(pageWith([{ attachmentId: CAT }, { attachmentId: GHOST, caption: "Gone" }, { attachmentId: BIG }, { attachmentId: DOG }])),
      { attachments: [...(SEED ?? []), { attachmentId: BIG, name: "big.png", mime: "image/png", bytes }] },
    );
    const editor = await openPage(app.user, "Notes");
    const element = galleryElement(editor);
    expect(await within(element).findByText(t("attachment.deleted.image"))).toBeInTheDocument();
    // Over the 32 MiB preview cap: the name and the size, not the bytes.
    expect(await within(element).findByText("big.png")).toBeInTheDocument();
    expect(within(element).getByText(t("attachment.preview.tooLarge", { size: formatBytes(bytes.length) }))).toBeInTheDocument();
    await waitFor(() => {
      expect(element.querySelectorAll("img")).toHaveLength(2);
    });
    // The caption of a picture that cannot be shown is page text all the same, and a click still opens the viewer.
    expect(within(tiles(element)[1] as HTMLElement).getByRole("textbox", { name: t("gallery.caption.label", { n: 2 }) })).toHaveValue("Gone");
    expect(within(tiles(element)[2] as HTMLElement).getByRole("button", { name: t("gallery.open", { n: 3 }) })).toBeEnabled();
  });
});

describe("editing", () => {
  it("sets the columns with one write, the other attribute untouched", async () => {
    const { editor, user, memory } = await openGallery([{ attachmentId: CAT }, { attachmentId: DOG }]);
    const element = galleryElement(editor);
    const group = within(element).getByRole("group", { name: t("gallery.columns.group") });
    expect(within(group).getByRole("button", { name: t("gallery.columns", { n: 3 }) })).toHaveAttribute("aria-pressed", "true");
    await user.click(within(group).getByRole("button", { name: t("gallery.columns", { n: 4 }) }));
    expect(attrsOf(editor).columns).toBe(4);
    expect(itemsOf(editor)).toHaveLength(2);
    await waitFor(() => {
      expect(galleryElement(editor).querySelector<HTMLElement>(".gallery-grid")?.style.gridTemplateColumns).toBe(
        "repeat(4, minmax(0, 1fr))",
      );
    });
    expect(within(galleryElement(editor)).getByRole("button", { name: t("gallery.columns", { n: 4 }) })).toHaveAttribute("aria-pressed", "true");
    await waitFor(async () => {
      expect((await stored(memory)).columns).toBe(4);
    });
  });

  it("edits a caption in place: Enter saves it, an empty one removes it, Escape undoes", async () => {
    const { editor, user, memory } = await openGallery([{ attachmentId: CAT }, { attachmentId: DOG, caption: "Old" }]);
    const first = within(galleryElement(editor)).getByRole("textbox", { name: t("gallery.caption.label", { n: 1 }) });
    await user.click(first);
    await user.type(first, "Sunrise on the pier{Enter}");
    expect(itemsOf(editor)[0]).toEqual({ attachmentId: CAT, caption: "Sunrise on the pier" });

    const second = within(galleryElement(editor)).getByRole("textbox", { name: t("gallery.caption.label", { n: 2 }) });
    await user.clear(second);
    await user.type(second, "Lost{Escape}");
    expect(itemsOf(editor)[1]).toEqual({ attachmentId: DOG, caption: "Old" });
    await user.clear(second);
    await user.keyboard("{Enter}");
    expect(itemsOf(editor)[1]).toEqual({ attachmentId: DOG });
    await waitFor(async () => {
      expect((await stored(memory)).items).toEqual([{ attachmentId: CAT, caption: "Sunrise on the pier" }, { attachmentId: DOG }]);
    });
  });

  it("removes a picture, and moves one earlier or later with buttons", async () => {
    const { editor, user, memory } = await openGallery([
      { attachmentId: CAT, caption: "a" },
      { attachmentId: DOG, caption: "b" },
      { attachmentId: CAT, caption: "c" },
    ]);
    const captions = () => itemsOf(editor).map((item) => item.caption);
    const element = () => galleryElement(editor);

    await user.click(within(element()).getByRole("button", { name: t("gallery.moveLater", { n: 1 }) }));
    expect(captions()).toEqual(["b", "a", "c"]);
    await user.click(within(element()).getByRole("button", { name: t("gallery.moveEarlier", { n: 3 }) }));
    expect(captions()).toEqual(["b", "c", "a"]);
    // The first cannot go earlier, the last cannot go later.
    expect(within(element()).getByRole("button", { name: t("gallery.moveEarlier", { n: 1 }) })).toBeDisabled();
    expect(within(element()).getByRole("button", { name: t("gallery.moveLater", { n: 3 }) })).toBeDisabled();

    await user.click(within(element()).getByRole("button", { name: t("gallery.remove", { n: 2 }) }));
    expect(captions()).toEqual(["b", "a"]);
    await waitFor(async () => {
      expect((await stored(memory)).items.map((item) => item.caption)).toEqual(["b", "a"]);
    });
  });

  it("reorders by dragging a picture onto another", async () => {
    const { editor } = await openGallery([
      { attachmentId: CAT, caption: "a" },
      { attachmentId: DOG, caption: "b" },
      { attachmentId: CAT, caption: "c" },
    ]);
    const cells = tiles(galleryElement(editor));
    // jsdom has no layout: the pointer is over the gallery, where the drop cursor stays off.
    const at = galleryPos(editor);
    vi.spyOn(editor.view, "posAtCoords").mockReturnValue({ pos: at, inside: at });
    const dataTransfer = { setData: vi.fn(), types: [GALLERY_DRAG_TYPE], effectAllowed: "all", dropEffect: "none" };
    const grip = (cells[0] as HTMLElement).querySelector(".gallery-thumb-wrap") as HTMLElement;
    expect(grip).toHaveAttribute("draggable", "true");

    fireEvent.dragStart(grip, { dataTransfer });
    expect(dataTransfer.setData).toHaveBeenCalledWith(GALLERY_DRAG_TYPE, "0");
    fireEvent.dragOver(cells[2] as HTMLElement, { dataTransfer });
    expect(tiles(galleryElement(editor))[2]).toHaveClass("is-drop-target");
    fireEvent.drop(cells[2] as HTMLElement, { dataTransfer });
    expect(itemsOf(editor).map((item) => item.caption)).toEqual(["b", "c", "a"]);
    await waitFor(() => {
      expect(tiles(galleryElement(editor))[2]).not.toHaveClass("is-drop-target");
    });
  });

  it("leaves the editor out of a gallery's own drags and controls, and takes the drops of files", async () => {
    const { editor } = await openGallery([{ attachmentId: CAT }]);
    const cell = tiles(galleryElement(editor))[0] as HTMLElement;
    const grip = cell.querySelector(".gallery-thumb-wrap") as HTMLElement;
    const own = (type: string, target: Element, types: string[]) => {
      const event = new Event(type, { bubbles: true });
      Object.defineProperty(event, "dataTransfer", { value: { types } });
      Object.defineProperty(event, "target", { value: target });
      return galleryStopsEvent(event);
    };
    expect(own("dragstart", grip, [])).toBe(true);
    expect(own("dragover", cell, [GALLERY_DRAG_TYPE])).toBe(true);
    expect(own("drop", cell, [GALLERY_DRAG_TYPE])).toBe(true);
    // Files from outside are the editor's drop (they become pictures of this gallery, see below).
    expect(own("dragover", cell, ["Files"])).toBe(false);
    expect(own("drop", cell, ["Files"])).toBe(false);
    const field = cell.querySelector("input") as HTMLElement;
    expect(galleryStopsEvent(Object.defineProperty(new Event("keydown", { bubbles: true }), "target", { value: field }))).toBe(true);
    // A click on the block's own area selects the block.
    expect(galleryStopsEvent(Object.defineProperty(new Event("mousedown", { bubbles: true }), "target", { value: galleryElement(editor) }))).toBe(false);
  });
});

/** The attachment has one reference and no unreferenced-since time (retention never collects it). */
async function expectReferenced(client: CoreClient, attachmentId: string): Promise<void> {
  const info = await client.attachmentGet(attachmentId);
  expect(info.references).toBe(1);
  expect(info.unreferencedSince ?? null).toBeNull();
}

describe("adding pictures", () => {
  it("takes several files from the chooser, in order, each appended as soon as it is stored", async () => {
    const { editor, memory } = await openGallery([{ attachmentId: CAT }]);
    expect(within(galleryElement(editor)).getByRole("button", { name: t("gallery.addLabel") })).toBeEnabled();
    const input = within(galleryElement(editor)).getByTestId("gallery-file-input");
    expect(input).toHaveAttribute("multiple");
    expect(input).toHaveAttribute("accept", "image/*");

    choose(galleryElement(editor), [pngFile("one.png"), pngFile("two.png"), pngFile("three.png")]);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(4);
    });
    const client = new CoreClient(memory);
    const names = await Promise.all(itemsOf(editor).slice(1).map(async (item) => (await client.attachmentGet(item.attachmentId)).name));
    expect(names).toEqual(["one.png", "two.png", "three.png"]);
    // Nothing is left under way, and nothing but the gallery changed on the page.
    expect(galleryElement(editor).querySelector(".gallery-transfers")).toBeNull();
    expect(editor.state.doc.childCount).toBe(2);
    await waitFor(async () => {
      expect((await stored(memory)).items).toHaveLength(4);
    });
  });

  it("refuses a file that is not an image, with a line that can be dismissed, and takes the rest", async () => {
    const { editor, user } = await openGallery([]);
    choose(galleryElement(editor), [textFile("notes.txt"), pngFile("ok.png")]);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(1);
    });
    const alert = await within(galleryElement(editor)).findByRole("alert");
    expect(alert).toHaveTextContent(t("gallery.error.notImage", { name: "notes.txt" }));
    await user.click(within(galleryElement(editor)).getByRole("button", { name: t("attachment.dismiss") }));
    expect(within(galleryElement(editor)).queryByRole("alert")).toBeNull();
  });

  it("shows why an upload the core refuses did not happen, and goes on with the next", async () => {
    const { editor } = await openGallery([]);
    const huge = sizedFile("huge.png", 1, "image/png");
    Object.defineProperty(huge, "size", { value: MAX_ATTACHMENT_BYTES + 1 });
    choose(galleryElement(editor), [huge, pngFile("fine.png")]);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(1);
    });
    expect(await within(galleryElement(editor)).findByRole("alert")).toHaveTextContent(
      t("attachment.error.size", { name: "huge.png", limit: formatBytes(MAX_ATTACHMENT_BYTES) }),
    );
  });

  it("takes pictures dropped on the gallery into it, not as image blocks of the page", async () => {
    const { editor } = await openGallery([{ attachmentId: CAT }]);
    expect(dropFiles(galleryElement(editor), [pngFile("dropped.png"), textFile("notes.txt")])).toBe(true);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(2);
    });
    expect(editor.state.doc.childCount).toBe(2);
    expect(await within(galleryElement(editor)).findByRole("alert")).toHaveTextContent("notes.txt");
    // A drop elsewhere on the page still makes image blocks.
    dropFiles(editor.view.dom.querySelector("p") as HTMLElement, [pngFile("page.png")]);
    await waitFor(() => {
      expect(editor.state.doc.childCount).toBe(3);
    });
    expect(itemsOf(editor)).toHaveLength(2);
  });

  it("takes pictures pasted while the gallery is selected, else pastes them as image blocks", async () => {
    const { editor } = await openGallery([{ attachmentId: CAT }]);
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, galleryPos(editor))));
    });
    pasteFiles(editor.view.dom, [pngFile("pasted.png")]);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(2);
    });
    expect(editor.state.doc.childCount).toBe(2);

    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 2)));
    });
    pasteFiles(editor.view.dom, [pngFile("loose.png")]);
    await waitFor(() => {
      expect(editor.state.doc.childCount).toBe(3);
    });
    expect(itemsOf(editor)).toHaveLength(2);
  });

  it("holds at most 50 pictures: the chooser button is disabled when full, and the extra files are refused", async () => {
    const full = Array.from({ length: 49 }, () => ({ attachmentId: CAT }));
    const { editor } = await openGallery(full, { count: 49 });
    choose(galleryElement(editor), [pngFile("a.png"), pngFile("b.png")]);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(50);
    });
    expect(await within(galleryElement(editor)).findByRole("alert")).toHaveTextContent(t("gallery.error.full", { name: "b.png", max: 50 }));
    expect(within(galleryElement(editor)).getByRole("button", { name: t("gallery.addLabel") })).toBeDisabled();
  });

  it("keeps every picture a gallery lists referenced, whatever its position or caption", async () => {
    const { editor, memory } = await openGallery([{ attachmentId: CAT, caption: "a" }, { attachmentId: CAT }, { attachmentId: DOG }]);
    const client = new CoreClient(memory);
    // One block, listing a picture twice: one reference.
    await expectReferenced(client, CAT);
    await expectReferenced(client, DOG);
    choose(galleryElement(editor), [pngFile("new.png")]);
    await waitFor(() => {
      expect(itemsOf(editor)).toHaveLength(4);
    });
    const added = itemsOf(editor)[3];
    await waitFor(async () => {
      await expectReferenced(client, added?.attachmentId ?? "");
    });
  });
});

describe("the full-size viewer", () => {
  it("opens on the clicked picture and steps through this gallery's pictures", async () => {
    const { editor, user } = await openGallery([
      { attachmentId: CAT, caption: "Sunrise" },
      { attachmentId: DOG },
      { attachmentId: CAT, caption: "Evening" },
    ]);
    // A picture of the page outside the gallery is not part of the walk.
    await user.click(within(galleryElement(editor)).getByRole("button", { name: t("gallery.open", { n: 2 }) }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveAccessibleName("dog.png");
    expect(within(dialog).getByText(t("attachment.image.viewer.position", { index: 2, total: 3 }))).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("Evening");
    });
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("Sunrise");
    });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(within(galleryElement(editor)).getByRole("button", { name: t("gallery.open", { n: 2 }) })).toHaveFocus();
  });
});

describe("a gallery in a read-only editor", () => {
  async function openReadOnly() {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = pageWith([{ attachmentId: CAT, caption: "Sunrise" }, { attachmentId: DOG }], 2) as { content: JsonValue[] };
    const withUnknownBlock: JsonValue = { type: "doc", content: [...content.content, { type: "kanban", attrs: { id: pid(9) } }] };
    stubObjectUrls();
    const app = await renderPages(space(withUnknownBlock), { attachments: SEED });
    const editor = await openPage(app.user, "Notes");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll("img")).toHaveLength(2);
    });
    return { ...app, editor };
  }

  it("shows the pictures and captions as text, with no toolbar, fields or buttons to change them", async () => {
    const { editor } = await openReadOnly();
    const element = galleryElement(editor);
    expect(element.querySelector(".gallery-caption")).toHaveTextContent("Sunrise");
    expect(within(element).queryByRole("textbox")).toBeNull();
    expect(within(element).queryByRole("toolbar")).toBeNull();
    expect(within(element).queryByRole("button", { name: t("gallery.addLabel") })).toBeNull();
    expect(within(element).queryByRole("button", { name: t("gallery.remove", { n: 1 }) })).toBeNull();
    expect(element.querySelector(".gallery-thumb-wrap")).toHaveAttribute("draggable", "false");
    // Dropped files are refused: the page stays as it is.
    dropFiles(element, [pngFile()]);
    await Promise.resolve();
    expect(itemsOf(editor)).toHaveLength(2);
  });

  it("opens the viewer and steps through the pictures", async () => {
    const { editor, user } = await openReadOnly();
    await user.click(within(galleryElement(editor)).getByRole("button", { name: t("gallery.open", { n: 1 }) }));
    expect(await screen.findByRole("dialog")).toHaveAccessibleName("Sunrise");
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("dog.png");
    });
  });
});

describe("the text of a gallery", () => {
  it("makes its captions searchable and exports one image line per picture", async () => {
    const { memory } = await openGallery([{ attachmentId: CAT, caption: "Lighthouse at dawn" }, { attachmentId: DOG }]);
    const client = new CoreClient(memory);
    await waitFor(async () => {
      const hits = await client.search("Lighthouse");
      expect(hits.length).toBeGreaterThan(0);
    });
    const { page } = await idsOf(memory);
    const markdown = await client.pageExportMarkdown(page("Notes"));
    expect(markdown).toBe(`Trip:\n\n![](attachment://${CAT} "Lighthouse at dawn")\n\n![](attachment://${DOG})\n`);
  });
});
