// Attachment blocks while their content syncs (Milestone 8, ADR-0023): the card of a file on
// its way (how far it got), waiting for the device that added it, or not downloaded on a
// device that downloads on demand ("Download"); none of them reads the bytes, and an image
// shows itself once its attachment is complete. Also: a block whose manifest arrives after
// it stops showing "deleted".

import { CoreError, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentInfo, AttachmentTransport, JsonValue, MemoryTransportOptions, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatBytes } from "../format";
import { t } from "../i18n";
import { MIB, stubObjectUrls } from "../test/attachments";
import { pid } from "../test/editor";
import { idsOf, openPage } from "../test/page";
import { NOW, findMainNavigation, recordingTransport, renderWith } from "../test/render";
import type { User } from "../test/render";
import { withTransfers } from "../test/transfers";
import { downloadedBytes } from "./AttachmentViews";

afterEach(() => {
  vi.restoreAllMocks();
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const IMAGE_BYTES = demoImagePng();

const CAT_SEED: MemoryTransportOptions["attachments"] = [
  { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: IMAGE_BYTES },
];

/** A page with an image block and a file block of `CAT`. */
function bothBlocks(): SeedSpace {
  const content: JsonValue = {
    type: "doc",
    content: [
      { type: "image", attrs: { id: pid(0), attachmentId: CAT, alt: "" } },
      { type: "file", attrs: { id: pid(1), attachmentId: CAT } },
    ],
  };
  return { name: "Personal", pages: [{ title: "Notes", content }] };
}

/** The app over a memory core with `CAT`, whose availability the test sets before the page opens. */
async function renderTransfers(prepare: (transfers: ReturnType<typeof withTransfers>) => void) {
  const urls = stubObjectUrls();
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [bothBlocks()], attachments: CAT_SEED });
  const recording = recordingTransport(memory);
  const transfers = withTransfers(recording.transport);
  prepare(transfers);
  const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
  const { user } = renderWith(recording.transport);
  await findMainNavigation();
  const editor = await openPage(user, "Notes");
  const { space } = await idsOf(memory);
  return { user, memory, recording, transfers, read, urls, editor, spaceId: space("Personal") };
}

function blockElement(editor: Editor, blockId: string): HTMLElement {
  const element = editor.view.dom.querySelector(`[data-id='${blockId}']`);
  if (!(element instanceof HTMLElement)) {
    throw new Error(`no block ${blockId}`);
  }
  return element;
}

const imageBlock = (editor: Editor): HTMLElement => blockElement(editor, pid(0));
const fileBlock = (editor: Editor): HTMLElement => blockElement(editor, pid(1));

function progressOf(block: HTMLElement): HTMLProgressElement {
  return within(block).getByRole<HTMLProgressElement>("progressbar", {
    name: t("attachment.transfer.progress", { name: "cat.png" }),
  });
}

async function findImage(editor: Editor): Promise<HTMLImageElement> {
  return waitFor(() => {
    const img = editor.view.dom.querySelector("img");
    expect(img).not.toBeNull();
    return img as HTMLImageElement;
  });
}

async function menuLabels(user: User, block: HTMLElement): Promise<string[]> {
  await user.click(within(block).getByRole("button", { name: t("attachment.actions") }));
  const menu = await screen.findByRole("menu", { name: t("attachment.actions") });
  const labels = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.textContent);
  await user.keyboard("{Escape}");
  return labels;
}

describe("an attachment on its way", () => {
  it("shows how far the download got, without reading the bytes, and the image once complete", async () => {
    const { transfers, read, urls, editor, spaceId } = await renderTransfers((transfers) => {
      transfers.set(CAT, { size: 9 * MIB, chunkCount: 3, localChunks: 1, availability: "Downloading" });
    });
    const downloading = (done: number) =>
      t("attachment.transfer.downloading", { done: formatBytes(done), total: formatBytes(9 * MIB) });

    for (const block of [imageBlock(editor), fileBlock(editor)]) {
      expect(await within(block).findByText(downloading(4 * MIB))).toBeInTheDocument();
      expect(within(block).getByText("cat.png")).toBeInTheDocument();
      const bar = progressOf(block);
      expect([bar.value, bar.max]).toEqual([1, 3]);
    }

    // A chunk arrives: the core says so with `AttachmentsChanged`, and the cards follow.
    transfers.set(CAT, { size: 9 * MIB, chunkCount: 3, localChunks: 2, availability: "Downloading" });
    transfers.changed(spaceId);
    expect(await within(imageBlock(editor)).findByText(downloading(8 * MIB))).toBeInTheDocument();
    expect(progressOf(fileBlock(editor)).value).toBe(2);
    expect(read).not.toHaveBeenCalled();
    expect(urls.create).not.toHaveBeenCalled();

    // The last one: the image reads its bytes now, once.
    transfers.set(CAT, null);
    transfers.changed(spaceId);
    expect(await findImage(editor)).toHaveAttribute("src", "blob:test/1");
    expect(read).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(fileBlock(editor).querySelector(".file-card")).toHaveTextContent(`cat.png${formatBytes(IMAGE_BYTES.length)}`);
    });
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("offers Save as and Open only once the whole file is here", async () => {
    const { user, transfers, editor, spaceId } = await renderTransfers((transfers) => {
      transfers.set(CAT, { localChunks: 0, availability: "Downloading" });
    });
    const block = fileBlock(editor);
    await waitFor(() => {
      expect(progressOf(block).value).toBe(0);
    });
    expect(await menuLabels(user, block)).toEqual([t("attachment.copyMarkdown"), t("attachment.remove")]);

    transfers.set(CAT, null);
    transfers.changed(spaceId);
    await waitFor(() => {
      expect(block.querySelector(".file-card")).not.toBeNull();
    });
    expect(await menuLabels(user, block)).toEqual([
      t("attachment.saveAs"),
      t("attachment.open"),
      t("attachment.copyMarkdown"),
      t("attachment.remove"),
    ]);
  });

  it("says the file waits for the device that added it, then shows it", async () => {
    const { transfers, read, editor, spaceId } = await renderTransfers((transfers) => {
      transfers.set(CAT, { localChunks: 0, availability: "Waiting" });
    });
    for (const block of [imageBlock(editor), fileBlock(editor)]) {
      expect(await within(block).findByText(t("attachment.transfer.waiting"))).toBeInTheDocument();
      expect(within(block).getByText("cat.png")).toBeInTheDocument();
      expect(within(block).queryByRole("progressbar")).not.toBeInTheDocument();
      expect(within(block).queryByRole("button", { name: t("attachment.transfer.download") })).not.toBeInTheDocument();
    }
    expect(imageBlock(editor).querySelector(".attachment-card")).toHaveClass("is-waiting");
    expect(read).not.toHaveBeenCalled();

    transfers.set(CAT, null);
    transfers.changed(spaceId);
    expect(await findImage(editor)).toHaveAttribute("src", "blob:test/1");
    expect(read).toHaveBeenCalledTimes(1);
  });
});

describe("an attachment this device has not downloaded (on demand)", () => {
  it("shows its size and a Download button that asks the core for it", async () => {
    const { user, recording, transfers, read, editor, spaceId } = await renderTransfers((transfers) => {
      transfers.set(CAT, { localChunks: 0, availability: "NotDownloaded" });
    });
    const detail = t("attachment.transfer.notDownloaded", { size: formatBytes(IMAGE_BYTES.length) });
    for (const block of [imageBlock(editor), fileBlock(editor)]) {
      expect(await within(block).findByText(detail)).toBeInTheDocument();
      expect(within(block).queryByRole("progressbar")).not.toBeInTheDocument();
    }
    expect(read).not.toHaveBeenCalled();

    await user.click(within(fileBlock(editor)).getByRole("button", { name: t("attachment.transfer.download") }));
    expect(recording.requests.filter((request) => request.type === "AttachmentDownload").map((request) => request.data)).toEqual([
      { attachmentId: CAT },
    ]);
    // The answer says it is on its way: both blocks of the attachment show it.
    await waitFor(() => {
      expect(progressOf(imageBlock(editor)).value).toBe(0);
    });
    expect(progressOf(fileBlock(editor)).value).toBe(0);
    expect(screen.queryByRole("button", { name: t("attachment.transfer.download") })).not.toBeInTheDocument();

    transfers.set(CAT, null);
    transfers.changed(spaceId);
    expect(await findImage(editor)).toHaveAttribute("src", "blob:test/1");
  });

  it("says why the download could not be asked for, and lets the user try again", async () => {
    const { user, recording, editor } = await renderTransfers((transfers) => {
      transfers.set(CAT, { localChunks: 0, availability: "NotDownloaded" });
    });
    const dispatch = recording.transport.dispatch.bind(recording.transport);
    recording.transport.dispatch = (request) =>
      request.type === "AttachmentDownload"
        ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
        : dispatch(request);

    const block = imageBlock(editor);
    const button = await within(block).findByRole("button", { name: t("attachment.transfer.download") });
    await user.click(button);
    expect(await within(block).findByRole("alert")).toHaveTextContent(t("attachment.transfer.error", { message: "disk I/O error" }));
    expect(button).toBeEnabled();
    expect(within(block).getByText(t("attachment.transfer.notDownloaded", { size: formatBytes(IMAGE_BYTES.length) }))).toBeInTheDocument();
  });
});

describe("a block whose attachment arrives after it", () => {
  it("shows the file once the core knows it, whatever space the event names", async () => {
    const { transfers, editor } = await renderTransfers((transfers) => {
      transfers.set(CAT, "absent");
    });
    expect(await within(fileBlock(editor)).findByText(t("attachment.deleted.file"))).toBeInTheDocument();
    expect(within(imageBlock(editor)).getByText(t("attachment.deleted.image"))).toBeInTheDocument();

    // The structure document with the manifest arrives (the event names its space; a missing
    // lookup does not know its space, so any space will do).
    transfers.set(CAT, { localChunks: 0, availability: "Downloading" });
    transfers.changed("b0000000-0000-4000-8000-000000000001");
    await waitFor(() => {
      expect(progressOf(fileBlock(editor)).value).toBe(0);
    });
    expect(within(imageBlock(editor)).queryByText(t("attachment.deleted.image"))).not.toBeInTheDocument();
  });
});

describe("over the memory core's attachment sync", () => {
  /** The app over a memory core where `CAT` was made on another device (its chunks on the server). */
  async function renderRemote(options: { policy: "eager" | "on_demand"; bytes?: Uint8Array; waiting?: boolean }) {
    const urls = stubObjectUrls();
    const memory = new MemoryTransport({
      now: () => NOW,
      workspaceName: "Research",
      spaces: [bothBlocks()],
      attachmentSync: {
        downloadPolicy: options.policy,
        remoteAttachments: [
          { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: options.bytes ?? IMAGE_BYTES, waiting: options.waiting },
        ],
      },
    });
    const recording = recordingTransport(memory);
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const editor = await openPage(user, "Notes");
    return { user, memory, recording, read, urls, editor };
  }

  it("on demand: Download fetches the file, and the image shows once its chunks are here", async () => {
    const { user, memory, read, editor } = await renderRemote({ policy: "on_demand" });
    const detail = t("attachment.transfer.notDownloaded", { size: formatBytes(IMAGE_BYTES.length) });
    expect(await within(imageBlock(editor)).findByText(detail)).toBeInTheDocument();
    expect(within(fileBlock(editor)).getByText(detail)).toBeInTheDocument();

    // Nothing comes by itself under this policy.
    act(() => {
      expect(memory.advanceTransfers(3)).toBe(0);
    });
    await user.click(within(imageBlock(editor)).getByRole("button", { name: t("attachment.transfer.download") }));
    await waitFor(() => {
      expect(progressOf(imageBlock(editor)).value).toBe(0);
    });
    expect(read).not.toHaveBeenCalled();

    act(() => {
      expect(memory.advanceTransfers()).toBe(1);
    });
    expect(await findImage(editor)).toHaveAttribute("src", "blob:test/1");
    expect(read).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(fileBlock(editor).querySelector(".file-card")).not.toBeNull();
    });
  });

  it("eagerly: a large file shows its progress chunk by chunk", async () => {
    const size = 9 * MIB;
    const { memory, read, editor } = await renderRemote({ policy: "eager", bytes: new Uint8Array(size) });
    const block = fileBlock(editor);
    const downloading = (done: number) => t("attachment.transfer.downloading", { done: formatBytes(done), total: formatBytes(size) });
    expect(await within(block).findByText(downloading(0))).toBeInTheDocument();
    expect([progressOf(block).value, progressOf(block).max]).toEqual([0, 3]);

    act(() => {
      memory.advanceTransfers();
    });
    expect(await within(block).findByText(downloading(4 * MIB))).toBeInTheDocument();
    act(() => {
      memory.advanceTransfers();
    });
    expect(await within(block).findByText(downloading(8 * MIB))).toBeInTheDocument();
    expect(progressOf(block).value).toBe(2);

    act(() => {
      memory.advanceTransfers();
    });
    await waitFor(() => {
      expect(block.querySelector(".file-card")).toHaveTextContent(`cat.png${formatBytes(size)}`);
    });
    // 9 MiB is under the preview limit: the image block reads it now, once.
    expect(read).toHaveBeenCalledTimes(1);
  });

  it("waits while the other device has not uploaded the file", async () => {
    const { memory, read, editor } = await renderRemote({ policy: "eager", waiting: true });
    act(() => {
      memory.advanceTransfers();
    });
    expect(await within(imageBlock(editor)).findByText(t("attachment.transfer.waiting"))).toBeInTheDocument();
    expect(within(fileBlock(editor)).getByText(t("attachment.transfer.waiting"))).toBeInTheDocument();
    expect(read).not.toHaveBeenCalled();
  });
});

describe("downloadedBytes", () => {
  it("counts whole chunks, at most the size", () => {
    const info: AttachmentInfo = {
      attachmentId: CAT,
      spaceId: "b0000000-0000-4000-8000-000000000001",
      name: "big.bin",
      mime: "application/octet-stream",
      size: 9 * MIB,
      chunkCount: 3,
      createdAt: NOW,
      references: 1,
      localChunks: 0,
      availability: "Downloading",
    };
    expect([0, 1, 2, 3].map((localChunks) => downloadedBytes({ ...info, localChunks }))).toEqual([0, 4 * MIB, 8 * MIB, 9 * MIB]);
  });
});
