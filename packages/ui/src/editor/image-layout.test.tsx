// The layout of an image block (ADR-0028): its width in percent of the column with a resize
// handle on each edge, its alignment, its caption, the toolbar and menu that change them, and
// the full-size viewer. Whole-app tests over the in-memory core, like attachments.test.tsx.

import { demoImagePng } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransportOptions } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import type { Transaction } from "@tiptap/pm/state";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { stubObjectUrls } from "../test/attachments";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";

afterEach(() => {
  vi.restoreAllMocks();
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const DOG = "a0000000-0000-4000-8000-0000000000d0";

const SEED: MemoryTransportOptions["attachments"] = [
  { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() },
  { attachmentId: DOG, name: "dog.png", mime: "image/png", bytes: demoImagePng() },
];

type Attrs = Record<string, JsonValue>;

/** A page of a paragraph and the given images (`pid(1)`, `pid(2)`, ...), with these attributes. */
function pageOf(...images: { attachmentId?: string; attrs?: Attrs }[]): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Look:" }] },
      ...images.map((image, index) => ({
        type: "image",
        attrs: { id: pid(index + 1), attachmentId: image.attachmentId ?? CAT, alt: "", ...image.attrs },
      })),
    ],
  };
}

const space = (content: JsonValue) => [{ name: "Personal", pages: [{ title: "Notes", content }] }];

/** Opens the page and waits until all its `count` images are drawn. */
async function openImages(content: JsonValue, count = 1) {
  const urls = stubObjectUrls();
  const app = await renderPages(space(content), { attachments: SEED });
  const editor = await openPage(app.user, "Notes");
  await waitFor(() => {
    expect(editor.view.dom.querySelectorAll("img")).toHaveLength(count);
  });
  return { ...app, editor, urls };
}

/** The node view of the block with id `pid(index)`. */
function block(editor: Editor, index: number): HTMLElement {
  const element = editor.view.dom.querySelector(`[data-id='${pid(index)}']`);
  if (!(element instanceof HTMLElement)) {
    throw new Error(`no block ${String(index)}`);
  }
  return element;
}

/** The attributes of the image block with id `pid(index)`. */
function attrsOf(editor: Editor, index: number): Record<string, unknown> {
  const node = editor.state.doc.child(index);
  return node.attrs;
}

function rect(width: number): DOMRect {
  return { x: 0, y: 0, width, height: 0, top: 0, right: width, bottom: 0, left: 0, toJSON: () => ({}) };
}

/** jsdom has no layout: gives the block's column (and, if given, the picture's frame) a width. */
function layOut(element: HTMLElement, column: number, frame?: number): void {
  const wrapper = element.querySelector(".image-block");
  const picture = element.querySelector(".image-frame");
  if (wrapper === null || picture === null) {
    throw new Error("not an image block");
  }
  vi.spyOn(wrapper, "getBoundingClientRect").mockReturnValue(rect(column));
  vi.spyOn(picture, "getBoundingClientRect").mockReturnValue(rect(frame ?? column));
}

/** The number of transactions that changed the document, from now on. */
function countChanges(editor: Editor): () => number {
  let changes = 0;
  editor.on("transaction", ({ transaction }: { transaction: Transaction }) => {
    if (transaction.docChanged) {
      changes += 1;
    }
  });
  return () => changes;
}

function handles(element: HTMLElement): { left: HTMLElement; right: HTMLElement } {
  const [left, right] = within(element).getAllByRole("slider", { name: t("attachment.image.width") });
  if (left === undefined || right === undefined) {
    throw new Error("no handles");
  }
  return { left, right };
}

function frameOf(element: HTMLElement): HTMLElement {
  const frame = element.querySelector(".image-frame");
  if (!(frame instanceof HTMLElement)) {
    throw new Error("no frame");
  }
  return frame;
}

/** The attributes of the `index`-th image as the core holds them (keys that are not stored are absent). */
async function storedAttrs(memory: Parameters<typeof idsOf>[0], index: number): Promise<Record<string, unknown>> {
  const { page } = await idsOf(memory);
  const content = (await contentOf(memory, page("Notes"))) as { content: { attrs?: Record<string, unknown> }[] };
  return content.content[index]?.attrs ?? {};
}

function pointer(type: "pointerDown" | "pointerMove" | "pointerUp" | "pointerCancel", target: Element, clientX: number): void {
  fireEvent[type](target, { clientX, pointerId: 7, button: 0, pointerType: "mouse" });
}

describe("the width of an image", () => {
  it("is a percentage of the column, else the old pixel width clamped to it, else the natural size", async () => {
    const { editor } = await openImages(
      pageOf(
        { attrs: { widthPercent: 40 } },
        { attrs: { width: 320 } },
        { attrs: { width: 320, widthPercent: 25 } },
        {},
      ),
      4,
    );
    expect(frameOf(block(editor, 1)).style.width).toBe("40%");
    expect(frameOf(block(editor, 2)).style.width).toBe("320px");
    // The percentage wins when both are there.
    expect(frameOf(block(editor, 3)).style.width).toBe("25%");
    expect(frameOf(block(editor, 4)).style.width).toBe("");
    for (let index = 1; index <= 4; index += 1) {
      expect(block(editor, index).querySelector(".image-layout")).toHaveAttribute("data-align", "center");
    }
  });

  it("is dragged with a handle: the width shows live, one update is written on release", async () => {
    const { editor, memory } = await openImages(pageOf({ attrs: { widthPercent: 50 } }));
    const picture = block(editor, 1);
    layOut(picture, 1000, 500);
    const { right } = handles(picture);
    const changes = countChanges(editor);
    const seen = vi.fn();
    for (const type of ["pointerdown", "mousedown", "dragstart"]) {
      editor.view.dom.addEventListener(type, seen);
    }

    pointer("pointerDown", right, 500);
    pointer("pointerMove", right, 550);
    // A centred picture grows on both sides: 50 px of pointer is 5 %, twice.
    expect(frameOf(picture).style.width).toBe("60%");
    expect(frameOf(picture)).toHaveClass("is-resizing");
    pointer("pointerMove", right, 600);
    pointer("pointerMove", right, 700);
    expect(frameOf(picture).style.width).toBe("90%");
    // Nothing is written while the pointer moves.
    expect(changes()).toBe(0);
    expect(attrsOf(editor, 1).widthPercent).toBe(50);

    pointer("pointerUp", right, 700);
    expect(changes()).toBe(1);
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: 90, width: null });
    // The view draws the new node a moment later (a microtask).
    await waitFor(() => {
      expect(frameOf(picture).style.width).toBe("90%");
    });
    expect(frameOf(picture)).not.toHaveClass("is-resizing");
    expect(handles(picture).right).toHaveAttribute("aria-valuenow", "90");
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).toMatchObject({ widthPercent: 90 });
    });
    expect(await storedAttrs(memory, 1)).not.toHaveProperty("width");
    // The handle's events never reached the editor: no block drag, no text selection.
    expect(seen).not.toHaveBeenCalled();
  });

  it("is dragged from the left handle the other way, and stays between 10 and 100 percent", async () => {
    const { editor } = await openImages(pageOf({ attrs: { widthPercent: 50, align: "right" } }));
    const picture = block(editor, 1);
    layOut(picture, 1000, 500);
    const { left } = handles(picture);

    // A right-aligned picture does not grow on both sides: 1 px of pointer is 1 px of width.
    pointer("pointerDown", left, 500);
    pointer("pointerMove", left, 400);
    expect(frameOf(picture).style.width).toBe("60%");
    pointer("pointerMove", left, -5000);
    expect(frameOf(picture).style.width).toBe("100%");
    pointer("pointerMove", left, 5000);
    expect(frameOf(picture).style.width).toBe("10%");
    pointer("pointerUp", left, 5000);
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: 10, width: null, align: "right" });
  });

  it("starts a drag from the natural size, measured, and replaces a pixel width", async () => {
    const { editor } = await openImages(pageOf({ attrs: { width: 320 } }, {}), 2);
    const [legacy, natural] = [block(editor, 1), block(editor, 2)];
    layOut(legacy, 800, 320); // 40 % of the column
    layOut(natural, 800, 200); // 25 %
    const changes = countChanges(editor);

    pointer("pointerDown", handles(legacy).right, 0);
    pointer("pointerMove", handles(legacy).right, 40); // 5 % of 800, twice
    pointer("pointerUp", handles(legacy).right, 40);
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: 50, width: null });

    pointer("pointerDown", handles(natural).right, 0);
    pointer("pointerMove", handles(natural).right, -40);
    pointer("pointerUp", handles(natural).right, -40);
    expect(attrsOf(editor, 2)).toMatchObject({ widthPercent: 15, width: null });
    expect(changes()).toBe(2);
  });

  it("writes nothing for a click on a handle, a cancelled drag or a drag back to where it began", async () => {
    const { editor } = await openImages(pageOf({ attrs: { widthPercent: 50 } }));
    const picture = block(editor, 1);
    layOut(picture, 1000, 500);
    const { right } = handles(picture);
    const changes = countChanges(editor);

    pointer("pointerDown", right, 500);
    pointer("pointerUp", right, 500);
    pointer("pointerDown", right, 500);
    pointer("pointerMove", right, 600);
    expect(frameOf(picture).style.width).toBe("70%");
    pointer("pointerCancel", right, 600);
    expect(frameOf(picture).style.width).toBe("50%");
    pointer("pointerDown", right, 500);
    pointer("pointerMove", right, 600);
    pointer("pointerMove", right, 500);
    pointer("pointerUp", right, 500);
    // Escape also gives the drag up.
    pointer("pointerDown", right, 500);
    pointer("pointerMove", right, 600);
    fireEvent.keyDown(right, { key: "Escape" });
    expect(frameOf(picture).style.width).toBe("50%");
    pointer("pointerUp", right, 600);

    expect(changes()).toBe(0);
    expect(attrsOf(editor, 1).widthPercent).toBe(50);
  });

  it("does not let a handle start the block's drag or a text selection", async () => {
    const { editor } = await openImages(pageOf({ attrs: { widthPercent: 50 } }));
    const { left, right } = handles(block(editor, 1));
    for (const handle of [left, right]) {
      expect(fireEvent.mouseDown(handle)).toBe(false); // default prevented: no selection, no native drag
      expect(fireEvent.dragStart(handle)).toBe(false);
    }
    expect(right).toHaveFocus();
  });

  it("is changed from the keyboard: arrows by 5 (Shift 1), Home and End, one update per key", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { widthPercent: 50, width: 400 } }));
    const picture = block(editor, 1);
    const { right } = handles(picture);
    expect(right).toHaveAttribute("aria-valuemin", "10");
    expect(right).toHaveAttribute("aria-valuemax", "100");
    expect(right).toHaveAttribute("aria-valuenow", "50");
    const changes = countChanges(editor);

    right.focus();
    await user.keyboard("{ArrowRight}");
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: 55, width: null });
    expect(changes()).toBe(1);
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(attrsOf(editor, 1).widthPercent).toBe(65);
    expect(changes()).toBe(3);
    await user.keyboard("{Shift>}{ArrowLeft}{/Shift}");
    expect(attrsOf(editor, 1).widthPercent).toBe(64);
    await user.keyboard("{ArrowLeft}");
    expect(attrsOf(editor, 1).widthPercent).toBe(59);
    expect(handles(picture).right).toHaveAttribute("aria-valuenow", "59");
    await user.keyboard("{Home}");
    expect(attrsOf(editor, 1).widthPercent).toBe(10);
    await user.keyboard("{ArrowLeft}"); // already the narrowest: nothing to write
    expect(changes()).toBe(6);
    await user.keyboard("{End}");
    expect(attrsOf(editor, 1).widthPercent).toBe(100);
    await user.keyboard("{ArrowRight}");
    expect(attrsOf(editor, 1).widthPercent).toBe(100);
    expect(changes()).toBe(7);
  });

  it("goes back to the original size on a double click on a handle", async () => {
    const { editor, memory } = await openImages(pageOf({ attrs: { widthPercent: 60, width: 640 } }));
    const picture = block(editor, 1);
    const changes = countChanges(editor);

    fireEvent.doubleClick(handles(picture).left);
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: null, width: null });
    await waitFor(() => {
      expect(frameOf(picture).style.width).toBe("");
    });
    expect(changes()).toBe(1);
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).not.toHaveProperty("widthPercent");
    });
    expect(await storedAttrs(memory, 1)).not.toHaveProperty("width");

    // Nothing to reset the second time.
    fireEvent.doubleClick(handles(picture).right);
    expect(changes()).toBe(1);
  });
});

describe("the alignment of an image", () => {
  it("is set by the toolbar; centered is no key at all", async () => {
    const { editor, memory, user } = await openImages(pageOf({ attrs: { widthPercent: 50 } }));
    const picture = block(editor, 1);
    const layout = picture.querySelector(".image-layout");
    const align = (name: "alignLeft" | "alignCenter" | "alignRight") =>
      within(picture).getByRole("button", { name: t(`attachment.image.${name}`) });
    expect(align("alignCenter")).toHaveAttribute("aria-pressed", "true");
    expect(align("alignLeft")).toHaveAttribute("aria-pressed", "false");
    const changes = countChanges(editor);

    await user.click(align("alignRight"));
    expect(attrsOf(editor, 1).align).toBe("right");
    expect(layout).toHaveAttribute("data-align", "right");
    expect(align("alignRight")).toHaveAttribute("aria-pressed", "true");
    expect(align("alignCenter")).toHaveAttribute("aria-pressed", "false");
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).toMatchObject({ align: "right" });
    });

    await user.click(align("alignLeft"));
    expect(attrsOf(editor, 1).align).toBe("left");
    expect(layout).toHaveAttribute("data-align", "left");

    await user.click(align("alignCenter"));
    expect(attrsOf(editor, 1).align).toBeNull();
    expect(layout).toHaveAttribute("data-align", "center");
    expect(align("alignCenter")).toHaveAttribute("aria-pressed", "true");
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).not.toHaveProperty("align");
    });
    expect(changes()).toBe(3);

    // Pressing the one that is already on writes nothing.
    await user.click(align("alignCenter"));
    expect(changes()).toBe(3);
    // The width was not touched.
    expect(attrsOf(editor, 1).widthPercent).toBe(50);
  });
});

describe("the caption of an image", () => {
  const captionInput = (picture: HTMLElement) => within(picture).getByRole("textbox", { name: t("attachment.image.captionLabel") });
  const captionButton = (picture: HTMLElement) => within(picture).getByRole("button", { name: t("attachment.image.caption") });

  it("is written under the picture from the toolbar, and committed with Enter", async () => {
    const { editor, memory, user } = await openImages(pageOf({ attrs: { alt: "A cat" } }));
    const picture = block(editor, 1);
    expect(picture.querySelector(".image-caption")).toBeNull();

    await user.click(captionButton(picture));
    const input = captionInput(picture);
    expect(input).toHaveFocus();
    expect(input).toHaveAttribute("placeholder", t("attachment.image.captionPlaceholder"));
    expect(input).toHaveAttribute("maxlength", "1000");
    expect(input).toHaveValue("");
    // Distinct from the alt text editor.
    expect(within(picture).queryByRole("textbox", { name: t("attachment.alt.label") })).toBeNull();
    const changes = countChanges(editor);

    await user.type(input, "The cat on the sofa");
    expect(changes()).toBe(0);
    await user.keyboard("{Enter}");
    expect(attrsOf(editor, 1)).toMatchObject({ caption: "The cat on the sofa", alt: "A cat" });
    expect(changes()).toBe(1);
    expect(picture.querySelector(".image-caption")).toHaveTextContent("The cat on the sofa");
    expect(picture.querySelector("input.image-caption-edit")).toBeNull();
    expect(captionButton(picture)).toHaveFocus();
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).toMatchObject({ caption: "The cat on the sofa", alt: "A cat" });
    });
  });

  it("follows the alignment of the picture", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { caption: "Cat", align: "left" } }));
    const picture = block(editor, 1);
    expect(picture.querySelector(".image-caption")).toHaveAttribute("data-align", "left");
    await user.click(within(picture).getByRole("button", { name: t("attachment.image.alignRight") }));
    expect(picture.querySelector(".image-caption")).toHaveAttribute("data-align", "right");
    await user.click(captionButton(picture));
    expect(captionInput(picture)).toHaveAttribute("data-align", "right");
  });

  it("is edited by a click on it, cancelled with Escape and committed when the field loses focus", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { caption: "The cat" } }));
    const picture = block(editor, 1);
    const changes = countChanges(editor);

    await user.click(picture.querySelector(".image-caption") as HTMLElement);
    expect(captionInput(picture)).toHaveValue("The cat");
    await user.type(captionInput(picture), " and the dog{Escape}");
    expect(picture.querySelector("input.image-caption-edit")).toBeNull();
    expect(attrsOf(editor, 1).caption).toBe("The cat");
    expect(changes()).toBe(0);

    await user.click(picture.querySelector(".image-caption") as HTMLElement);
    await user.type(captionInput(picture), " and the dog");
    await user.tab();
    expect(attrsOf(editor, 1).caption).toBe("The cat and the dog");
    expect(changes()).toBe(1);

    // An edit that changes nothing writes nothing.
    await user.click(picture.querySelector(".image-caption") as HTMLElement);
    await user.keyboard("{Enter}");
    expect(changes()).toBe(1);
  });

  it("is removed (the key too) when it becomes empty", async () => {
    const { editor, memory, user } = await openImages(pageOf({ attrs: { caption: "The cat" } }));
    const picture = block(editor, 1);
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).toMatchObject({ caption: "The cat" });
    });

    await user.click(captionButton(picture));
    await user.clear(captionInput(picture));
    await user.keyboard("{Enter}");
    expect(attrsOf(editor, 1).caption).toBeNull();
    expect(picture.querySelector(".image-caption")).toBeNull();
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).not.toHaveProperty("caption");
    });

    // Only spaces is empty too.
    await user.click(captionButton(picture));
    await user.type(captionInput(picture), "   {Enter}");
    expect(attrsOf(editor, 1).caption).toBeNull();
  });

  it("is trimmed when committed (line breaks become spaces: see normalizeCaption, a text field keeps none)", async () => {
    const { editor, user } = await openImages(pageOf({}));
    const picture = block(editor, 1);
    await user.click(captionButton(picture));
    await user.type(captionInput(picture), "  one two  {Enter}");
    expect(attrsOf(editor, 1).caption).toBe("one two");
  });
});

describe("the menu of an image", () => {
  it("makes it full width, or gives it back its original size", async () => {
    const { editor, memory, user } = await openImages(pageOf({ attrs: { widthPercent: 30, width: 400, align: "left" } }));
    const picture = block(editor, 1);
    const openMenu = async (): Promise<HTMLElement> => {
      await user.click(within(picture).getByRole("button", { name: t("attachment.actions") }));
      return screen.findByRole("menu", { name: t("attachment.actions") });
    };

    const menu = await openMenu();
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("attachment.image.viewFull"),
      t("attachment.image.fullWidth"),
      t("attachment.image.originalSize"),
      t("attachment.saveAs"),
      t("attachment.open"),
      t("attachment.alt.edit"),
      t("attachment.copyMarkdown"),
      t("attachment.remove"),
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: t("attachment.image.fullWidth") }));
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: 100, width: null, align: "left" });
    expect(frameOf(picture).style.width).toBe("100%");
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).toMatchObject({ widthPercent: 100 });
    });

    await user.click(within(await openMenu()).getByRole("menuitem", { name: t("attachment.image.originalSize") }));
    expect(attrsOf(editor, 1)).toMatchObject({ widthPercent: null, width: null, align: "left" });
    expect(frameOf(picture).style.width).toBe("");
    await waitFor(async () => {
      expect(await storedAttrs(memory, 1)).not.toHaveProperty("widthPercent");
    });
  });

  it("copies the caption into the Markdown it offers", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { alt: "A cat", caption: 'The "cat"' } }));
    const picture = block(editor, 1);
    await user.click(within(picture).getByRole("button", { name: t("attachment.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("attachment.copyMarkdown") }));
    expect(await navigator.clipboard.readText()).toBe(`![A cat](attachment://${CAT} "The \\"cat\\"")`);
  });
});

describe("the full-size viewer", () => {
  const viewer = () => screen.findByRole("dialog");

  it("opens on a double click on the picture and closes with Escape, giving focus back to the page", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { alt: "A cat", caption: "The cat" } }));
    const img = block(editor, 1).querySelector("img") as HTMLImageElement;
    expect(screen.queryByRole("dialog")).toBeNull();

    await user.dblClick(img);
    const dialog = await viewer();
    expect(dialog).toHaveAttribute("aria-modal", "true");
    // Named by the caption (else the alt text, else the file name).
    expect(dialog).toHaveAccessibleName("The cat");
    const big = await within(dialog).findByRole("img");
    expect(big).toHaveAttribute("src", expect.stringMatching(/^blob:test\//));
    expect(within(dialog).queryByText(t("attachment.image.viewer.position", { index: 1, total: 1 }))).toBeNull();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(editor.view.dom).toHaveFocus();
  });

  it("is named by the alt text, else by the file name", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { alt: "A cat" } }, { attachmentId: DOG }), 2);
    await user.dblClick(block(editor, 1).querySelector("img") as HTMLImageElement);
    expect(await viewer()).toHaveAccessibleName("A cat");
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("dog.png");
    });
  });

  it("opens from the toolbar and the menu, and gives focus back to the button that opened it", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { alt: "A cat" } }));
    const picture = block(editor, 1);

    const fullSize = within(picture).getByRole("button", { name: t("attachment.image.viewFull") });
    await user.click(fullSize);
    await viewer();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(fullSize).toHaveFocus();

    const more = within(picture).getByRole("button", { name: t("attachment.actions") });
    await user.click(more);
    await user.click(await screen.findByRole("menuitem", { name: t("attachment.image.viewFull") }));
    await viewer();
    await user.click(screen.getByRole("button", { name: t("attachment.image.viewer.close") }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(more).toHaveFocus();
  });

  it("steps between the page's images with the arrow keys, in document order", async () => {
    const { user, editor, urls } = await openImages(
      pageOf({ attrs: { alt: "A cat", caption: "The cat" } }, { attachmentId: DOG, attrs: { alt: "A dog" } }),
      2,
    );
    // The viewer opens on the image that was double clicked, the second one here.
    await user.dblClick(block(editor, 2).querySelector("img") as HTMLImageElement);
    let dialog = await viewer();
    expect(dialog).toHaveAccessibleName("A dog");
    expect(within(dialog).getByText(t("attachment.image.viewer.position", { index: 2, total: 2 }))).toBeInTheDocument();
    const dog = (await within(dialog).findByRole("img")).getAttribute("src");

    await user.keyboard("{ArrowLeft}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("The cat");
    });
    dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(t("attachment.image.viewer.position", { index: 1, total: 2 }))).toBeInTheDocument();
    await waitFor(() => {
      expect(within(dialog).getByRole("img").getAttribute("src")).not.toBe(dog);
    });
    // The picture left behind lets its blob URL go.
    expect(urls.revoke).toHaveBeenCalledWith(dog);

    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("A dog");
    });
    // Past the last one it comes round to the first.
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("The cat");
    });
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: t("attachment.image.viewer.next") }));
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("A dog");
    });
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: t("attachment.image.viewer.previous") }));
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toHaveAccessibleName("The cat");
    });

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    // Closing lets the picture's own URL go too.
    expect(urls.revoke.mock.calls.length).toBeGreaterThan(1);
  });

  it("toggles the actual size with a click on the picture, and closes with a click on the backdrop", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { alt: "A cat" } }));
    await user.dblClick(block(editor, 1).querySelector("img") as HTMLImageElement);
    const dialog = await viewer();
    const big = await within(dialog).findByRole("img");
    const stage = dialog.querySelector(".image-viewer-stage") as HTMLElement;
    expect(big).not.toHaveClass("is-actual");

    await user.click(big);
    expect(big).toHaveClass("is-actual");
    expect(stage).toHaveClass("is-scrollable");
    expect(within(dialog).getByRole("button", { name: t("attachment.image.viewer.actualSize") })).toHaveAttribute("aria-pressed", "true");
    await user.click(big);
    expect(big).not.toHaveClass("is-actual");

    // A click on the picture does not close it; one beside it does.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await user.click(stage);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes with the backdrop too", async () => {
    const { editor, user } = await openImages(pageOf({ attrs: { alt: "A cat" } }));
    await user.dblClick(block(editor, 1).querySelector("img") as HTMLImageElement);
    const dialog = await viewer();
    await user.click(dialog.parentElement as HTMLElement);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("an image in a read-only editor", () => {
  async function openReadOnly() {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = pageOf({ attrs: { alt: "A cat", caption: "The cat", widthPercent: 50, align: "right" } }) as { content: JsonValue[] };
    const withUnknownBlock: JsonValue = {
      type: "doc",
      content: [...content.content, { type: "kanban", attrs: { id: pid(9) } }],
    };
    stubObjectUrls();
    const app = await renderPages(space(withUnknownBlock), { attachments: SEED });
    const editor = await openPage(app.user, "Notes");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    await waitFor(() => {
      expect(editor.view.dom.querySelector("img")).not.toBeNull();
    });
    return { ...app, editor };
  }

  it("shows its layout and caption, with a toolbar of View full size and the menu only", async () => {
    const { editor, user } = await openReadOnly();
    const picture = block(editor, 1);
    expect(frameOf(picture).style.width).toBe("50%");
    expect(picture.querySelector(".image-layout")).toHaveAttribute("data-align", "right");
    expect(picture.querySelector(".image-caption")).toHaveTextContent("The cat");
    expect(picture.querySelector(".image-caption")).not.toHaveClass("is-editable");

    expect(within(picture).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual([
      t("attachment.image.viewFull"),
      t("attachment.actions"),
    ]);
    expect(within(picture).queryByRole("slider")).toBeNull();

    // The caption cannot be edited, and nothing but the viewer and the copies is in the menu.
    await user.click(picture.querySelector(".image-caption") as HTMLElement);
    expect(within(picture).queryByRole("textbox")).toBeNull();
    await user.click(within(picture).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu", { name: t("attachment.actions") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("attachment.image.viewFull"),
      t("attachment.saveAs"),
      t("attachment.open"),
      t("attachment.copyMarkdown"),
    ]);
  });

  it("opens the viewer with a single click on the picture", async () => {
    const { editor, user } = await openReadOnly();
    await user.click(block(editor, 1).querySelector("img") as HTMLImageElement);
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveAccessibleName("The cat");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("an image that cannot be shown", () => {
  it("keeps the whole row, with the menu button only: no handles, no toolbar", async () => {
    stubObjectUrls();
    const { user } = await renderPages(space(pageOf({ attrs: { widthPercent: 50, caption: "Gone" } })));
    const editor = await openPage(user, "Notes");
    const picture = block(editor, 1);
    expect(await within(picture).findByText(t("attachment.deleted.image"))).toBeInTheDocument();
    expect(picture.querySelector(".image-layout")).toHaveClass("is-card");
    expect(picture.querySelector(".image-frame")).toBeNull();
    expect(within(picture).queryByRole("slider")).toBeNull();
    expect(within(picture).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual([t("attachment.actions")]);
    // The caption is page text: it stays.
    expect(picture.querySelector(".image-caption")).toHaveTextContent("Gone");
  });
});
