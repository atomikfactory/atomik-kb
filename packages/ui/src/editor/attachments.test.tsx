// Attachment blocks in the page editor (Milestone 5): uploads by drop and paste with their
// placeholders, the `/` commands that pick a file through the shell, the image and file
// views (preview, too large, deleted, unsupported) and their menus.

import { CoreClient, MAX_ATTACHMENT_BYTES, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentTransport, JsonValue, MemoryTransportOptions, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatBytes } from "../format";
import { t } from "../i18n";
import { MIB, dropFiles, pasteFiles, pngFile, sizedFile, stubObjectUrls, textFile, upload } from "../test/attachments";
import { paragraphs, pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { NOW, findMainNavigation, recordingTransport, renderWith } from "../test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** The attachment of the seeded pages below. */
const CAT = "a0000000-0000-4000-8000-0000000000ca";

const CAT_SEED: MemoryTransportOptions["attachments"] = [
  { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() },
];

/** A page with a paragraph and an image of `CAT`. */
function catPage(alt = ""): SeedSpace {
  const content: JsonValue = {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Look:" }] },
      { type: "image", attrs: { id: pid(1), attachmentId: CAT, alt } },
    ],
  };
  return { name: "Personal", pages: [{ title: "Notes", content }] };
}

/** Position just inside the `index`-th top-level block. */
function startOf(editor: Editor, index: number): number {
  let pos = 1;
  for (let i = 0; i < index; i += 1) {
    pos += editor.state.doc.child(i).nodeSize;
  }
  return pos;
}

function placeCursor(editor: Editor, pos: number): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
  });
}

/** The node names of the top-level blocks. */
function kinds(editor: Editor): string[] {
  const names: string[] = [];
  editor.state.doc.forEach((node) => {
    names.push(node.type.name);
  });
  return names;
}

/** The attachment ids of the top-level attachment blocks, in order. */
function attachmentIds(editor: Editor): string[] {
  const ids: string[] = [];
  editor.state.doc.forEach((node) => {
    if (typeof node.attrs.attachmentId === "string") {
      ids.push(node.attrs.attachmentId);
    }
  });
  return ids;
}

async function namesOf(memory: MemoryTransport, ids: readonly string[]): Promise<string[]> {
  const client = new CoreClient(memory);
  return Promise.all(ids.map(async (id) => (await client.attachmentGet(id)).name));
}

/** The DOM of the block with id `blockId`. */
function blockElement(editor: Editor, blockId: string): HTMLElement {
  const element = editor.view.dom.querySelector(`[data-id='${blockId}']`);
  if (!(element instanceof HTMLElement)) {
    throw new Error(`no block ${blockId}`);
  }
  return element;
}

async function findImage(editor: Editor): Promise<HTMLImageElement> {
  return waitFor(() => {
    const img = editor.view.dom.querySelector("img");
    expect(img).not.toBeNull();
    return img as HTMLImageElement;
  });
}

describe("drop and paste", () => {
  it("uploads a dropped image and shows the block from the attachment's bytes", async () => {
    const urls = stubObjectUrls();
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));

    expect(dropFiles(editor.view.dom, [pngFile("cat.png")])).toBe(true);

    // The empty paragraph the drop landed in gives way to the image.
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["paragraph", "image"]);
    });
    const image = editor.state.doc.child(1);
    expect(image.attrs.id).toMatch(UUID);
    const [attachmentId = ""] = attachmentIds(editor);
    expect(await findImage(editor)).toHaveAttribute("src", "blob:test/1");
    expect(urls.create.mock.calls[0]?.[0].type).toBe("image/png");
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();

    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [{ type: "paragraph" }, { type: "image", attrs: { id: image.attrs.id as string, attachmentId } }],
      });
    });
    await waitFor(async () => {
      expect(await new CoreClient(memory).attachmentGet(attachmentId)).toMatchObject({ name: "cat.png", references: 1 });
    });
  });

  it("shows an upload's progress on a placeholder, and inserts the block only once the attachment exists", async () => {
    const memory = new MemoryTransport({
      now: () => NOW,
      workspaceName: "Research",
      spaces: [{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }],
    });
    const recording = recordingTransport(memory);
    const files = recording.transport.attachments as AttachmentTransport;
    const write = files.write.bind(files);
    let release: () => void = () => undefined;
    const secondPiece = new Promise<void>((resolve) => {
      release = resolve;
    });
    let writes = 0;
    files.write = async (attachmentId, bytes) => {
      writes += 1;
      if (writes === 2) {
        await secondPiece;
      }
      return write(attachmentId, bytes);
    };
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);

    dropFiles(editor.view.dom, [sizedFile("big.bin", 5 * MIB)]);

    expect(await screen.findByRole("progressbar", { name: t("attachment.uploading", { name: "big.bin" }) })).toBeInTheDocument();
    expect(await screen.findByText("80%")).toBeInTheDocument(); // the first 4 MiB are in
    expect(kinds(editor)).toEqual(["paragraph"]);

    act(release);
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["file"]);
    });
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".file-card")).toHaveTextContent(`big.bin${formatBytes(5 * MIB)}`);
    });
  });

  it("drops several files as several blocks, in their order", async () => {
    stubObjectUrls();
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 6); // the end of "Intro"

    dropFiles(editor.view.dom, [pngFile("one.png"), textFile("notes.txt"), pngFile("two.png")]);

    await waitFor(() => {
      expect(kinds(editor)).toEqual(["paragraph", "image", "file", "image"]);
    });
    expect(await namesOf(memory, attachmentIds(editor))).toEqual(["one.png", "notes.txt", "two.png"]);
    expect(editor.state.doc.child(0).textContent).toBe("Intro");
  });

  it("pastes image data as an image, and text that comes with a picture of itself as text", async () => {
    stubObjectUrls();
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);

    pasteFiles(editor.view.dom, [pngFile("image.png")]);
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["image"]);
    });
    expect(await namesOf(memory, attachmentIds(editor))).toEqual(["image.png"]);

    act(() => {
      editor.chain().insertContentAt(editor.state.doc.content.size, { type: "paragraph" }).focus("end").run();
    });
    pasteFiles(editor.view.dom, [pngFile("cells.png")], "A1 B1");
    await waitFor(() => {
      expect(editor.state.doc.lastChild?.textContent).toBe("A1 B1");
    });
    expect(kinds(editor)).toEqual(["image", "paragraph"]);
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("refuses files dropped outside the editor, so that the webview never opens them in place of the app", async () => {
    await renderPages([{ name: "Personal", pages: [{ title: "Notes" }] }]);
    const sidebar = screen.getByRole("navigation", { name: t("nav.label") });
    const transfer = { types: ["Files"], files: [pngFile()], dropEffect: "copy", getData: () => "" };
    const event = (type: string): Event => {
      const created = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperty(created, "dataTransfer", { value: transfer });
      return created;
    };
    const over = event("dragover");
    sidebar.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    expect(transfer.dropEffect).toBe("none");
    const drop = event("drop");
    sidebar.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(true);
  });

  it("shows a refused upload on its placeholder until it is dismissed", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    const huge = sizedFile("huge.iso", 1);
    Object.defineProperty(huge, "size", { value: MAX_ATTACHMENT_BYTES + 1 });
    const longName = textFile(`${"x".repeat(300)}.txt`);

    dropFiles(editor.view.dom, [huge, longName]);

    await waitFor(() => {
      expect(screen.getAllByRole("alert")).toHaveLength(2);
    });
    const [tooLarge, badName] = screen.getAllByRole("alert");
    expect(tooLarge).toHaveTextContent(
      t("attachment.error.size", { name: "huge.iso", limit: formatBytes(MAX_ATTACHMENT_BYTES) }),
    );
    expect(badName).toHaveTextContent(/^x{300}\.txt cannot be added: .*255 bytes\.$/);
    expect(kinds(editor)).toEqual(["paragraph"]);

    for (const button of screen.getAllByRole("button", { name: t("attachment.dismiss") })) {
      fireEvent.click(button);
    }
    expect(screen.queryAllByRole("alert")).toHaveLength(0);
    const { space } = await idsOf(memory);
    expect(await new CoreClient(memory).attachmentList(space("Personal"))).toEqual([]);
  });
});

describe("the Image and File commands", () => {
  async function choose(editor: Editor, label: string, typed: string): Promise<void> {
    act(() => {
      editor.commands.insertContent(typed);
    });
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.click(within(menu).getByText(label));
  }

  it("pick a file through the shell's dialog and put its block where the command was", async () => {
    stubObjectUrls();
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    memory.files.set("/pictures/cat.png", demoImagePng());
    const editor = await openPage(user, "Notes");

    placeCursor(editor, startOf(editor, 1));
    await choose(editor, "Image", "/ima");
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["paragraph", "image"]);
    });
    expect(await findImage(editor)).toHaveAttribute("src", "blob:test/1");

    // "File" makes a file card, even of a picture.
    act(() => {
      editor.chain().insertContentAt(editor.state.doc.content.size, { type: "paragraph" }).focus("end").run();
    });
    await choose(editor, "File", "/file");
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["paragraph", "image", "file"]);
    });
    expect(await namesOf(memory, attachmentIds(editor))).toEqual(["cat.png", "cat.png"]);

    // A cancelled dialog leaves nothing behind.
    memory.setCancelDialogs(true);
    act(() => {
      editor.chain().insertContentAt(editor.state.doc.content.size, { type: "paragraph" }).focus("end").run();
    });
    await choose(editor, "Image", "/image");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".attachment-upload")).toBeNull();
    });
    expect(kinds(editor)).toEqual(["paragraph", "image", "file", "paragraph"]);
  });
});

describe("the image view", () => {
  it("shows a deleted attachment as such and lets its object URL go", async () => {
    const urls = stubObjectUrls();
    const { user, memory } = await renderPages([catPage("A cat")], { attachments: CAT_SEED });
    const editor = await openPage(user, "Notes");
    const img = await findImage(editor);
    expect(img).toHaveAttribute("src", "blob:test/1");
    expect(img).toHaveAttribute("alt", "A cat");

    await act(async () => {
      await memory.dispatch({ type: "AttachmentDelete", data: { attachmentId: CAT } });
    });
    expect(await within(blockElement(editor, pid(1))).findByText(t("attachment.deleted.image"))).toBeInTheDocument();
    expect(editor.view.dom.querySelector("img")).toBeNull();
    expect(urls.revoke).toHaveBeenCalledWith("blob:test/1");
  });

  it("shows a card with Save as and Open for an image too large to preview, without reading it", async () => {
    const urls = stubObjectUrls();
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [catPage()], attachments: CAT_SEED });
    const recording = recordingTransport(memory);
    const dispatch = recording.transport.dispatch.bind(recording.transport);
    // The core reports the image as 40 MiB (a real one would cost the test that much memory).
    recording.transport.dispatch = async (request) => {
      const response = await dispatch(request);
      return response.type === "Attachment" ? { ...response, data: { ...response.data, size: 40 * MIB } } : response;
    };
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const editor = await openPage(user, "Notes");

    const block = blockElement(editor, pid(1));
    expect(
      await within(block).findByText(t("attachment.preview.tooLarge", { size: formatBytes(40 * MIB) })),
    ).toBeInTheDocument();
    expect(urls.create).not.toHaveBeenCalled();
    await user.click(within(block).getByRole("button", { name: t("attachment.saveAs") }));
    expect(await within(block).findByText(t("attachment.saved"))).toBeInTheDocument();
    expect(memory.files.has("/downloads/cat.png")).toBe(true);
    expect(within(block).getByRole("button", { name: t("attachment.open") })).toBeInTheDocument();
  });

  it("has a menu to edit the alt text, copy the block as Markdown, open the file and remove the block", async () => {
    stubObjectUrls();
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [catPage()], attachments: CAT_SEED });
    const recording = recordingTransport(memory);
    const open = vi.spyOn(recording.transport.attachments as AttachmentTransport, "open");
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const editor = await openPage(user, "Notes");
    await findImage(editor);
    const block = blockElement(editor, pid(1));
    const openMenu = async (): Promise<HTMLElement> => {
      await user.click(within(block).getByRole("button", { name: t("attachment.actions") }));
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
    await user.click(within(menu).getByRole("menuitem", { name: t("attachment.alt.edit") }));
    await user.type(within(block).getByRole("textbox", { name: t("attachment.alt.label") }), "A cat{Enter}");
    expect(editor.state.doc.child(1).attrs).toMatchObject({ id: pid(1), attachmentId: CAT, alt: "A cat" });
    expect(await findImage(editor)).toHaveAttribute("alt", "A cat");
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Notes")))).toContain('"alt":"A cat"');
    });

    await user.click(within(await openMenu()).getByRole("menuitem", { name: t("attachment.copyMarkdown") }));
    expect(await within(block).findByText(t("attachment.markdown.copied"))).toBeInTheDocument();
    expect(await navigator.clipboard.readText()).toBe(`![A cat](attachment://${CAT})`);

    await user.click(within(await openMenu()).getByRole("menuitem", { name: t("attachment.open") }));
    await waitFor(() => {
      expect(open).toHaveBeenCalledWith(CAT);
    });

    await user.click(within(await openMenu()).getByRole("menuitem", { name: t("attachment.remove") }));
    expect(kinds(editor)).toEqual(["paragraph"]);
  });

  it("turns into a file card and back through the block menu, keeping its ids", async () => {
    stubObjectUrls();
    const { user } = await renderPages([catPage()], { attachments: CAT_SEED });
    const editor = await openPage(user, "Notes");
    await findImage(editor);

    const turnInto = async (label: string): Promise<HTMLElement> => {
      fireEvent.mouseMove(blockElement(editor, pid(1)));
      fireEvent.click(await screen.findByRole("button", { name: t("block.actions") }));
      const menu = await screen.findByRole("menu", { name: t("block.actions") });
      await user.click(within(menu).getByRole("menuitem", { name: t("block.turnInto") }));
      const kindsMenu = await screen.findByRole("menu", { name: t("block.turnInto") });
      const items = within(kindsMenu).getAllByRole("menuitem");
      await user.click(within(kindsMenu).getByRole("menuitem", { name: label }));
      return items[0] as HTMLElement;
    };

    await turnInto("File");
    expect(editor.state.doc.child(1).type.name).toBe("file");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1), attachmentId: CAT, view: null });
    await waitFor(() => {
      expect(blockElement(editor, pid(1)).querySelector(".file-card")).toHaveTextContent("cat.png");
    });

    await turnInto("Image");
    expect(editor.state.doc.child(1).attrs).toMatchObject({ id: pid(1), attachmentId: CAT });
    expect(editor.state.doc.child(1).type.name).toBe("image");
  });
});

describe("the file view", () => {
  it("shows the file's type, name and size, saves it and copies it as Markdown", async () => {
    const bytes = new TextEncoder().encode("hello, world");
    const content: JsonValue = {
      type: "doc",
      content: [{ type: "file", attrs: { id: pid(0), attachmentId: CAT } }],
    };
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }], {
      attachments: [{ attachmentId: CAT, name: "notes.txt", mime: "text/plain", bytes }],
    });
    const editor = await openPage(user, "Notes");
    const block = blockElement(editor, pid(0));
    await waitFor(() => {
      expect(block.querySelector(".file-card")).toHaveTextContent(`📄notes.txt${formatBytes(bytes.length)}`);
    });

    await user.click(within(block).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu", { name: t("attachment.actions") });
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.alt.edit") })).toBeNull();
    await user.click(within(menu).getByRole("menuitem", { name: t("attachment.saveAs") }));
    expect(await within(block).findByText(t("attachment.saved"))).toBeInTheDocument();
    expect(Array.from(memory.files.get("/downloads/notes.txt") ?? [])).toEqual(Array.from(bytes));

    await user.click(within(block).getByRole("button", { name: t("attachment.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("attachment.copyMarkdown") }));
    await waitFor(async () => {
      expect(await navigator.clipboard.readText()).toBe(`[notes.txt](attachment://${CAT})`);
    });
  });

  it("shows a block whose attachment does not exist as deleted", async () => {
    const content: JsonValue = { type: "doc", content: [{ type: "file", attrs: { id: pid(0), attachmentId: CAT } }] };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(user, "Notes");
    expect(await within(blockElement(editor, pid(0))).findByText(t("attachment.deleted.file"))).toBeInTheDocument();
  });

  it("exports with the page's Markdown", async () => {
    const { user, memory } = await renderPages([catPage("A cat")], { attachments: CAT_SEED });
    await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    expect(await new CoreClient(memory).pageExportMarkdown(page("Notes"))).toContain(`![A cat](attachment://${CAT})`);
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.copyMarkdown") }));
    await waitFor(async () => {
      expect(await navigator.clipboard.readText()).toContain(`![A cat](attachment://${CAT})`);
    });
  });
});

describe("without attachment support in the transport", () => {
  it("disables the commands with the reason, refuses drops and still shows what it knows", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [catPage()], attachments: CAT_SEED });
    const urls = stubObjectUrls();
    const { user } = renderWith(recordingTransport(memory, { attachments: false }).transport);
    await findMainNavigation();
    const editor = await openPage(user, "Notes");
    const block = blockElement(editor, pid(1));

    expect(
      await within(block).findByText(t("attachment.preview.unavailable", { size: formatBytes(demoImagePng().length) })),
    ).toBeInTheDocument();
    expect(urls.create).not.toHaveBeenCalled();
    await user.click(within(block).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu", { name: t("attachment.actions") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("attachment.image.fullWidth"),
      t("attachment.image.originalSize"),
      t("attachment.alt.edit"),
      t("attachment.copyMarkdown"),
      t("attachment.remove"),
    ]);
    fireEvent.keyDown(menu, { key: "Escape" });

    placeCursor(editor, 6);
    act(() => {
      editor.commands.insertContent(" /ima");
    });
    const slash = await screen.findByRole("listbox", { name: t("slash.label") });
    const option = within(slash).getByRole("option");
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(option).toHaveTextContent(t("slash.attachment.unsupported"));
    fireEvent.click(option);
    expect(editor.state.doc.child(0).textContent).toBe("Look: /ima");

    dropFiles(editor.view.dom, [pngFile()]);
    expect(await screen.findByRole("alert")).toHaveTextContent(t("attachment.unsupported"));
    expect(kinds(editor)).toEqual(["paragraph", "image"]);
  });
});

describe("uploads in a space of their own", () => {
  it("go to the space of the open page", async () => {
    stubObjectUrls();
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Home" }] },
      { name: "Work", pages: [{ title: "Notes", content: paragraphs("") }] },
    ]);
    const { space } = await idsOf(memory);
    await upload(memory, space("Personal"), textFile("elsewhere.txt"));
    await user.click(screen.getByRole("button", { description: t("spaces.switchTitle") }));
    const spaces = await screen.findByRole("dialog", { name: t("spaces.listLabel") });
    await user.click(within(spaces).getByRole("button", { name: "Work" }));
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    dropFiles(editor.view.dom, [textFile("work.txt")]);
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["file"]);
    });
    const list = await new CoreClient(memory).attachmentList(space("Work"));
    expect(list.map((info) => info.name)).toEqual(["work.txt"]);
  });
});
