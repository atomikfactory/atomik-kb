// "Image from URL" (ADR-0028): the `/` entry (present only where the shell can download a link),
// its dialog (the link is checked as it is typed, Enter inserts, Escape cancels), the placeholder
// that says the picture is downloading, the image block that replaces it, and the error card a
// failed download leaves. The store's side of it comes first.

import { CoreClient, CoreError, MAX_PREVIEW_BYTES, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentTransport, RemoteImage, Transport } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatBytes } from "../format";
import { t } from "../i18n";
import { createAppStore } from "../store";
import { stubObjectUrls } from "../test/attachments";
import { paragraphs } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { NOW, findMainNavigation, recordingTransport, renderWith } from "../test/render";
import { describeUrlError, importImageFromUrl } from "./attachments";
import { ImageUrlDialogStore, ImageUrlForm, linkProblem } from "./ImageUrlDialog";
import { SLASH_COMMANDS } from "./slash-commands";

afterEach(() => {
  vi.restoreAllMocks();
});

const DEMO_URL = "https://example.com/photos/demo.png";

/** The canned answers of the in-memory shell: a picture, and a page that is not one. */
function remoteImages(): Map<string, RemoteImage> {
  return new Map<string, RemoteImage>([
    [DEMO_URL, { bytes: demoImagePng(), contentType: "image/png" }],
    ["https://example.com/page.html", { bytes: new TextEncoder().encode("<html>not a picture</html>".repeat(4)), contentType: "text/html" }],
  ]);
}

function newMemory(): MemoryTransport {
  return new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }],
    remoteImages: remoteImages(),
  });
}

/** The same transport without `importUrl`: a shell that has file attachments but cannot fetch a link. */
function withoutImportUrl(memory: MemoryTransport): Transport {
  const files = memory.attachments;
  const attachments: AttachmentTransport = {
    write: (attachmentId, bytes) => files.write(attachmentId, bytes),
    read: (attachmentId, range) => files.read(attachmentId, range),
    importPath: (spaceId, path) => files.importPath(spaceId, path),
    saveAs: (attachmentId, path) => files.saveAs(attachmentId, path),
    open: (attachmentId) => files.open(attachmentId),
  };
  return {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    attachments,
  };
}

function placeCursor(editor: Editor, pos: number): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
  });
}

/** Position just inside the `index`-th top-level block. */
function startOf(editor: Editor, index: number): number {
  let pos = 1;
  for (let i = 0; i < index; i += 1) {
    pos += editor.state.doc.child(i).nodeSize;
  }
  return pos;
}

function kinds(editor: Editor): string[] {
  const names: string[] = [];
  editor.state.doc.forEach((node) => {
    names.push(node.type.name);
  });
  return names;
}

/** The labels of the entries the `/` menu lists for what is typed. */
async function slashLabels(editor: Editor, typed: string): Promise<string[]> {
  act(() => {
    editor.commands.insertContent(typed);
  });
  const menu = await screen.findByRole("listbox", { name: t("slash.label") });
  return within(menu)
    .getAllByRole("option")
    .map((option) => option.querySelector(".suggestion-title")?.textContent ?? "");
}

async function chooseImageFromUrl(editor: Editor): Promise<HTMLElement> {
  act(() => {
    editor.commands.insertContent("/url");
  });
  const menu = await screen.findByRole("listbox", { name: t("slash.label") });
  fireEvent.click(within(menu).getByText(t("slash.imageUrl")));
  return screen.findByRole("dialog", { name: t("attachment.url.title") });
}

/**
 * Puts a link in the dialog's field and presses Enter. Pasted, not typed: typing a 40-character link is 40 rounds of the event loop
 * (and of React), which a loaded machine stretches into seconds, and these tests are about what Enter does with the whole link.
 */
async function submitLink(user: ReturnType<typeof userEvent.setup>, dialog: HTMLElement, link: string): Promise<void> {
  await user.click(within(dialog).getByLabelText(t("attachment.url.label")));
  await user.paste(link);
  await user.keyboard("{Enter}");
}

// ----- the store ---------------------------------------------------------------------------------

describe("the store", () => {
  async function storeOver(transport: Transport) {
    const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
    const stop = store.getState().start();
    await vi.waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    const [space] = await new CoreClient(transport).spaceList();
    return { store, stop, spaceId: space?.id ?? "" };
  }

  it("reports importUrl only where the transport can download a link, apart from the file picker", async () => {
    const memory = newMemory();
    const full = await storeOver(memory);
    expect(full.store.getState().attachmentCapabilities).toMatchObject({ importPath: true, importUrl: true });
    full.stop();

    const noUrl = await storeOver(withoutImportUrl(newMemory()));
    expect(noUrl.store.getState().attachmentCapabilities).toMatchObject({ importPath: true, importUrl: false });
    noUrl.stop();

    const { transport } = recordingTransport(newMemory(), { attachments: false });
    const none = await storeOver(transport);
    expect(none.store.getState().attachmentCapabilities).toMatchObject({ importPath: false, importUrl: false });
    none.stop();
  });

  it("importAttachmentFromUrl stores the picture, remembers it, and passes the shell's failure on", async () => {
    const memory = newMemory();
    const { store, stop, spaceId } = await storeOver(memory);

    const info = await store.getState().importAttachmentFromUrl(spaceId, DEMO_URL);

    expect(info).toMatchObject({ name: "demo.png", mime: "image/png", spaceId });
    expect(store.getState().attachmentLookup[info.attachmentId]).toEqual({ status: "found", info });

    await expect(store.getState().importAttachmentFromUrl(spaceId, "https://example.com/page.html")).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "url", reason: "notImage" },
    });
    stop();
  });
});

// ----- the dialog ---------------------------------------------------------------------------------

describe("linkProblem", () => {
  it("accepts https links the shell would take, and says what is wrong with the others", () => {
    expect(linkProblem("https://example.com/a.png")).toBeNull();
    expect(linkProblem("  https://example.com/a.png \n")).toBeNull();
    const cases: [string, string][] = [
      ["", "attachment.url.error.empty"],
      ["   ", "attachment.url.error.empty"],
      ["http://example.com/a.png", "attachment.url.error.scheme"],
      ["file:///C:/a.png", "attachment.url.error.scheme"],
      ["javascript:alert(1)", "attachment.url.error.scheme"],
      ["data:image/png;base64,AAAA", "attachment.url.error.scheme"],
      ["https://user:secret@example.com/a.png", "attachment.url.error.credentials"],
      ["example.com/a.png", "attachment.url.error.invalid"],
      ["https://", "attachment.url.error.invalid"],
      ["not a link", "attachment.url.error.invalid"],
      [`https://example.com/${"a".repeat(2100)}`, "attachment.url.error.tooLong"],
    ];
    for (const [text, key] of cases) {
      expect(linkProblem(text), JSON.stringify(text.slice(0, 40))).toBe(key);
    }
  });
});

describe("the dialog's form", () => {
  function open() {
    const onSubmit = vi.fn<(url: string) => void>();
    const onCancel = vi.fn<() => void>();
    const user = userEvent.setup();
    render(<ImageUrlForm onSubmit={onSubmit} onCancel={onCancel} />);
    const dialog = screen.getByRole("dialog", { name: t("attachment.url.title") });
    const field = within(dialog).getByLabelText(t("attachment.url.label"));
    return { user, dialog, field, onSubmit, onCancel };
  }

  it("is a modal dialog with the field focused and no complaint yet", () => {
    const { dialog, field } = open();
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveAccessibleDescription(t("attachment.url.hint"));
    expect(field).toHaveFocus();
    expect(field).toHaveAttribute("placeholder", t("attachment.url.placeholder"));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("inserts the trimmed link on Enter and on the Insert button", async () => {
    const { user, field, onSubmit, onCancel } = open();
    await user.type(field, "  https://example.com/photos/demo.png  {Enter}");
    expect(onSubmit).toHaveBeenCalledExactlyOnceWith("https://example.com/photos/demo.png");

    await user.click(screen.getByRole("button", { name: t("attachment.url.insert") }));
    expect(onSubmit).toHaveBeenCalledTimes(2);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("says at once that a link must be https, and does not insert it", async () => {
    const { user, field, onSubmit } = open();
    await user.type(field, "http://example.com/a.png");
    expect(await screen.findByRole("alert")).toHaveTextContent(t("attachment.url.error.scheme"));
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(field).toHaveAccessibleDescription(t("attachment.url.error.scheme"));

    await user.keyboard("{Enter}");
    expect(onSubmit).not.toHaveBeenCalled();
    expect(field).toHaveFocus();

    // Corrected: the complaint goes, and the link goes through.
    await user.clear(field);
    await user.type(field, "https://example.com/a.png");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(onSubmit).toHaveBeenCalledExactlyOnceWith("https://example.com/a.png");
  });

  it("waits for Enter before it complains about a link that is only unfinished, and an empty field is refused too", async () => {
    const { user, field, onSubmit } = open();
    await user.type(field, "https:");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument(); // on its way to a link
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(t("attachment.url.error.invalid"));

    await user.clear(field);
    await user.keyboard("{Enter}");
    expect(screen.getByRole("alert")).toHaveTextContent(t("attachment.url.error.empty"));
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("gives up on Escape and on Cancel", async () => {
    const { user, field, onCancel } = open();
    await user.type(field, "https://example.com/a.png");
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: t("dialog.cancel") }));
    expect(onCancel).toHaveBeenCalledTimes(2);
  });
});

describe("ImageUrlDialogStore", () => {
  it("resolves an ask with the link or with null, and gives up the first of two asks", async () => {
    const store = new ImageUrlDialogStore();
    const listener = vi.fn();
    store.subscribe(listener);
    expect(store.isOpen()).toBe(false);

    const first = store.ask();
    expect(store.isOpen()).toBe(true);
    const second = store.ask(); // a second ask while it is open gives up the first
    await expect(first).resolves.toBeNull();
    expect(store.isOpen()).toBe(true);
    store.submit("https://example.com/a.png");
    await expect(second).resolves.toBe("https://example.com/a.png");
    expect(store.isOpen()).toBe(false);

    const third = store.ask();
    store.cancel();
    await expect(third).resolves.toBeNull();
    store.cancel(); // nothing is waiting: nothing happens
    expect(store.isOpen()).toBe(false);
    expect(listener).toHaveBeenCalled();
  });
});

// ----- the menu entry -----------------------------------------------------------------------------

describe("the Image from URL entry", () => {
  it("sits in the media section, right after Image, with its words and a glyph", () => {
    const ids = SLASH_COMMANDS.map((command) => command.id);
    expect(ids.indexOf("imageUrl")).toBe(ids.indexOf("image") + 1);
    const command = SLASH_COMMANDS.find((candidate) => candidate.id === "imageUrl");
    expect(command).toMatchObject({
      section: "media",
      label: "slash.imageUrl",
      hint: "slash.imageUrl.hint",
      keywords: ["url", "link", "web", "embed", "picture", "photo"],
    });
    expect(typeof command?.icon).toBe("function");
    expect(t("slash.imageUrl")).toBe("Image from URL");
    expect(t("slash.imageUrl.hint")).toBe("Download a picture from a link");
  });

  it("is in the menu where the shell can download a link", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }], {
      remoteImages: remoteImages(),
    });
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    const labels = await slashLabels(editor, "/image");
    expect(labels.slice(0, 2)).toEqual(["Image", "Image from URL"]);
  });

  it("is not in the menu when the shell has no importUrl, nor when it has no attachments at all", async () => {
    for (const make of [
      (memory: MemoryTransport): Transport => withoutImportUrl(memory),
      (memory: MemoryTransport): Transport => recordingTransport(memory, { attachments: false }).transport,
    ]) {
      const memory = newMemory();
      const { user, unmount } = renderWith(make(memory));
      await findMainNavigation();
      const editor = await openPage(user, "Notes");
      placeCursor(editor, startOf(editor, 1));
      const labels = await slashLabels(editor, "/image");
      expect(labels).not.toContain("Image from URL");
      expect(labels).toContain("Image");
      unmount();
    }
  });
});

// ----- the flow -----------------------------------------------------------------------------------

describe("inserting an image from a link", () => {
  async function pageWithEditor() {
    const urls = stubObjectUrls();
    const memory = newMemory();
    const recording = recordingTransport(memory);
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));
    return { user, editor, memory, recording, urls };
  }

  it("asks for the link, shows the card downloading, then the image block where the command was", async () => {
    const { user, editor, memory, recording } = await pageWithEditor();
    const dialog = await chooseImageFromUrl(editor);
    expect(editor.state.doc.child(1).textContent).toBe(""); // the typed command is gone
    // The card stands in the page but is out of sight while the link is being asked for.
    expect(within(document.body).queryByRole("progressbar")).toBeNull();

    const release = recording.hold();
    await submitLink(user, dialog, DEMO_URL);

    // The dialog is gone, the card says the picture is downloading.
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("attachment.url.title") })).not.toBeInTheDocument();
    });
    const bar = await screen.findByRole("progressbar", { name: t("attachment.url.downloading") });
    expect(bar.closest(".attachment-upload")).toHaveTextContent(t("attachment.url.downloading"));
    expect(kinds(editor)).toEqual(["paragraph", "paragraph"]);

    release();
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["paragraph", "image"]); // it took the place of the empty paragraph
    });
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(editor.view.dom.querySelector("img")).not.toBeNull();
    });
    expect(editor.view.dom.querySelector("img")).toHaveAttribute("src", "blob:test/1");

    // An ordinary attachment of the space, named after the link; the page holds it, not the link.
    const { space, page } = await idsOf(memory);
    const [attachment] = await new CoreClient(memory).attachmentList(space("Personal"));
    expect(attachment).toMatchObject({ name: "demo.png", mime: "image/png" });
    await waitFor(async () => {
      const content = await contentOf(memory, page("Notes"));
      expect(content).toMatchObject({
        content: [{ type: "paragraph" }, { type: "image", attrs: { attachmentId: attachment?.attachmentId } }],
      });
      expect(JSON.stringify(content)).not.toContain("example.com");
    });
  });

  it("puts the block where the command was typed, in the middle of a page too", async () => {
    stubObjectUrls();
    const { user } = await renderPages(
      [{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("First", "", "Last") }] }],
      { remoteImages: remoteImages() },
    );
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));
    const dialog = await chooseImageFromUrl(editor);
    await submitLink(user, dialog, DEMO_URL);
    await waitFor(() => {
      expect(kinds(editor)).toEqual(["paragraph", "image", "paragraph"]);
    });
    expect([0, 2].map((index) => editor.state.doc.child(index).textContent)).toEqual(["First", "Last"]);
  });

  it("takes the card away and inserts nothing when the dialog is cancelled, and the page is editable again", async () => {
    const { editor, memory } = await pageWithEditor();
    await chooseImageFromUrl(editor);
    fireEvent.keyDown(screen.getByRole("dialog", { name: t("attachment.url.title") }), { key: "Escape" });

    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("attachment.url.title") })).not.toBeInTheDocument();
    });
    expect(editor.view.dom.querySelector(".attachment-upload")).toBeNull();
    expect(kinds(editor)).toEqual(["paragraph", "paragraph"]);
    await waitFor(() => {
      expect(editor.view.dom).toHaveFocus();
    });
    const { space } = await idsOf(memory);
    expect(await new CoreClient(memory).attachmentList(space("Personal"))).toEqual([]);
  });

  it("makes no request for a link the dialog refuses", async () => {
    const { user, editor, memory } = await pageWithEditor();
    const dialog = await chooseImageFromUrl(editor);
    await submitLink(user, dialog, "http://example.com/photos/demo.png");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("attachment.url.error.scheme"));
    expect(screen.getByRole("dialog", { name: t("attachment.url.title") })).toBeInTheDocument();
    const { space } = await idsOf(memory);
    expect(await new CoreClient(memory).attachmentList(space("Personal"))).toEqual([]);
    // The card holds its place but is still out of sight: nothing was downloaded.
    expect(editor.view.dom.querySelector<HTMLElement>(".attachment-upload")?.style.display).toBe("none");
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("leaves an error card that says why, until it is dismissed, when the link is not a picture", async () => {
    const { user, editor, memory } = await pageWithEditor();
    const dialog = await chooseImageFromUrl(editor);
    await submitLink(user, dialog, "https://example.com/page.html");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("attachment.url.error.notImage"));
    expect(alert.textContent).not.toContain("example.com"); // nothing repeats the link
    expect(kinds(editor)).toEqual(["paragraph", "paragraph"]);
    const { space } = await idsOf(memory);
    expect(await new CoreClient(memory).attachmentList(space("Personal"))).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: t("attachment.dismiss") }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(editor.view.dom.querySelector(".attachment-upload")).toBeNull();
  });

  it("says what the shell reported when the download fails: status, network, timeout, redirect, size", async () => {
    const { user, editor, memory } = await pageWithEditor();
    const failures: [Record<string, unknown>, string][] = [
      [{ field: "url", reason: "status", status: 404 }, t("attachment.url.error.status", { status: 404 })],
      [{ field: "url", reason: "network" }, t("attachment.url.error.network")],
      [{ field: "url", reason: "timeout" }, t("attachment.url.error.timeout")],
      [{ field: "url", reason: "redirect" }, t("attachment.url.error.redirect")],
      [{ field: "size", limit: MAX_PREVIEW_BYTES }, t("attachment.url.error.size", { limit: formatBytes(MAX_PREVIEW_BYTES) })],
    ];
    for (const [details, message] of failures) {
      memory.failNextAttachments({ code: "InvalidInput", message: "the server said no", details: details as never });
      const dialog = await chooseImageFromUrl(editor);
      await submitLink(user, dialog, DEMO_URL);
      expect(await screen.findByRole("alert")).toHaveTextContent(message);
      fireEvent.click(screen.getByRole("button", { name: t("attachment.dismiss") }));
      expect(editor.view.dom.querySelector(".attachment-upload")).toBeNull();
      // The next try starts from an empty paragraph again.
      placeCursor(editor, startOf(editor, editor.state.doc.childCount - 1));
    }
    expect(kinds(editor).every((kind) => kind === "paragraph")).toBe(true);
  });

  it("does nothing, and leaves the typed command, when the editor cannot download a link", () => {
    // `importImageFromUrl` is reached through the menu only where the entry is listed; called anyway
    // it must not touch the page.
    const editor = { schema: { nodes: {} }, state: { doc: {} }, storage: {} } as unknown as Editor;
    expect(() => {
      importImageFromUrl(editor, { from: 1, to: 2 });
    }).not.toThrow();
  });
});

describe("describeUrlError", () => {
  it("words every failure the shell can report, without the link", () => {
    const refused = (details: Record<string, unknown>, message = "refused") =>
      new CoreError({ code: "InvalidInput", message, details: details as never });
    expect(describeUrlError(refused({ field: "url", reason: "status", status: 503 }))).toBe(
      t("attachment.url.error.status", { status: 503 }),
    );
    expect(describeUrlError(refused({ field: "url", reason: "status" }))).toBe(t("attachment.url.error.status", { status: "?" }));
    expect(describeUrlError(refused({ field: "url", reason: "notImage" }))).toBe(t("attachment.url.error.notImage"));
    // The shell's own refusal of a link (not https, credentials, too long): its message.
    expect(describeUrlError(refused({ field: "url" }, "the link has no host"))).toBe(
      t("attachment.url.error.rejected", { message: "the link has no host" }),
    );
    expect(describeUrlError(refused({ field: "size" }))).toBe(
      t("attachment.url.error.size", { limit: formatBytes(MAX_PREVIEW_BYTES) }),
    );
    expect(describeUrlError(new CoreError({ code: "Locked", message: "the workspace is locked" }))).toBe(
      t("attachment.url.error.generic", { message: "the workspace is locked" }),
    );
    expect(describeUrlError(new Error("boom"))).toContain("boom");
  });
});
