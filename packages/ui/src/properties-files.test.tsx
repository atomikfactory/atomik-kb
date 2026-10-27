// The editor of a `files` property (ADR-0026, amendment of 2026-10-07): the chips of the attachments
// (thumbnails of images from their bytes, a glyph and the name of other files), Open, Save as and
// Remove, "Add file" through the shell's picker, files dropped on the row, a shell without a
// picker, files that are gone, the limit, a page in the trash and adding the property to a page.

import { CoreClient, MAX_ATTACHMENT_BYTES, MAX_FILES, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentTransport, MemoryTransportOptions, SeedSpace } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PropertiesPanel } from "./components/PropertiesPanel";
import { formatBytes } from "./format";
import { t } from "./i18n";
import { createAppStore as createStore } from "./store";
import { StoreContext } from "./store-context";
import { dropFiles, stubObjectUrls, textFile } from "./test/attachments";
import { findMainNavigation, recordingTransport, renderWith } from "./test/render";
import { idsOf } from "./test/page";
import { openProperties, propertyRow, renderProperties, rowNames, valueOf } from "./test/properties";

afterEach(() => {
  vi.restoreAllMocks();
});

const IMAGE = "a0000000-0000-4000-8000-000000000001";
const NOTES = "a0000000-0000-4000-8000-000000000002";
const SPARE = "a0000000-0000-4000-8000-000000000003";
const LIST_TEXT = "Toothbrush\nCharger\n";

const ATTACHMENTS: MemoryTransportOptions["attachments"] = [
  { attachmentId: IMAGE, name: "atomik.png", mime: "image/png", bytes: demoImagePng() },
  { attachmentId: NOTES, name: "packing-list.txt", mime: "text/plain", bytes: new TextEncoder().encode(LIST_TEXT) },
  { attachmentId: SPARE, name: "spare.pdf", mime: "application/pdf", bytes: new TextEncoder().encode("%PDF") },
];

const SPACE: SeedSpace = {
  name: "Personal",
  properties: [
    { name: "Files", kind: "files" },
    { name: "Notes", kind: "text" },
  ],
  pages: [
    { title: "Welcome", properties: { Files: [IMAGE, NOTES] } },
    { title: "Plain" },
    { title: "Other", properties: { Notes: "no files here" } },
  ],
};

/** The attachment ids of the `Files` value of a page, in order. */
async function filesOf(memory: MemoryTransport, title: string): Promise<string[]> {
  const value = await valueOf(memory, title, "Files");
  return value?.kind === "files" ? value.value : [];
}

/** The names of the attachments in a list of ids, from the core. */
async function namesOf(memory: MemoryTransport, ids: readonly string[]): Promise<string[]> {
  const client = new CoreClient(memory);
  return Promise.all(ids.map(async (id) => (await client.attachmentGet(id)).name));
}

async function renderFiles(options: Parameters<typeof renderProperties>[1] = {}) {
  return renderProperties([SPACE], { attachments: ATTACHMENTS, ...options });
}

/** The chips of the row, by their file name. */
function chips(row: HTMLElement): HTMLElement[] {
  return within(row).queryAllByRole("listitem");
}

async function chipOf(row: HTMLElement, name: string): Promise<HTMLElement> {
  return await waitFor(() => {
    const found = chips(row).find((chip) => within(chip).queryByText(name) !== null);
    if (found === undefined) {
      throw new Error(`no chip for ${name}`);
    }
    return found;
  });
}

describe("the chips", () => {
  it("show an image as a thumbnail from its bytes and another file with a glyph, its name and its size", async () => {
    const urls = stubObjectUrls();
    const { user } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    expect(rowNames(panel)).toEqual(["Tags", "Files"]);
    const row = propertyRow(panel, "Files");

    const image = await chipOf(row, "atomik.png");
    const thumbnail = await within(image).findByRole("img", { name: t("properties.files.thumbnail", { name: "atomik.png" }) });
    expect(thumbnail).toHaveAttribute("src", "blob:test/1");
    expect(urls.create).toHaveBeenCalledTimes(1);

    const notes = await chipOf(row, "packing-list.txt");
    expect(within(notes).queryByRole("img")).toBeNull();
    expect(within(notes).getByText(formatBytes(LIST_TEXT.length))).toBeInTheDocument();
    expect(notes.querySelector(".file-chip-glyph")).toHaveTextContent("📄");
    expect(chips(row).map((chip) => chip.querySelector(".file-chip-name")?.textContent)).toEqual([
      "atomik.png",
      "packing-list.txt",
    ]);
    // Each chip has Open, Save as and Remove.
    for (const [chip, name] of [
      [image, "atomik.png"],
      [notes, "packing-list.txt"],
    ] as const) {
      expect(within(chip).getByRole("button", { name: t("properties.files.open", { name }) })).toBeInTheDocument();
      expect(within(chip).getByRole("button", { name: t("properties.files.saveAs", { name }) })).toBeInTheDocument();
      expect(within(chip).getByRole("button", { name: t("properties.files.remove", { name }) })).toBeInTheDocument();
    }
    expect(within(row).getByRole("button", { name: t("properties.files.add") })).toBeEnabled();
  });

  it("let go of the thumbnail's object URL when the row goes", async () => {
    const urls = stubObjectUrls();
    const { user } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    await within(propertyRow(panel, "Files")).findByRole("img");
    await openProperties(user, "Plain");
    await waitFor(() => {
      expect(urls.revoke).toHaveBeenCalledWith("blob:test/1");
    });
  });

  it("save a file with Save as and say so", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const chip = await chipOf(propertyRow(panel, "Files"), "packing-list.txt");
    await user.click(within(chip).getByRole("button", { name: t("properties.files.saveAs", { name: "packing-list.txt" }) }));
    expect(await within(chip).findByText(t("attachment.saved"))).toBeInTheDocument();
    expect(memory.files.has("/downloads/packing-list.txt")).toBe(true);
    // A cancelled dialog says nothing.
    memory.setCancelDialogs(true);
    await user.click(within(chip).getByRole("button", { name: t("properties.files.saveAs", { name: "packing-list.txt" }) }));
    await waitFor(() => {
      expect(within(chip).queryByText(t("attachment.saved"))).not.toBeInTheDocument();
    });
    expect(within(chip).queryByRole("alert")).toBeNull();
  });

  it("open a file through the shell", async () => {
    stubObjectUrls();
    const memory = new MemoryTransport({
      workspaceName: "Research",
      spaces: [SPACE],
      attachments: ATTACHMENTS,
    });
    const recording = recordingTransport(memory);
    const open = vi.spyOn(recording.transport.attachments as AttachmentTransport, "open");
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    const panel = await openProperties(user, "Welcome");
    const chip = await chipOf(propertyRow(panel, "Files"), "packing-list.txt");
    await user.click(within(chip).getByRole("button", { name: t("properties.files.open", { name: "packing-list.txt" }) }));
    await waitFor(() => {
      expect(open).toHaveBeenCalledWith(NOTES);
    });
  });

  it("show a file that was deleted as such, with Remove only", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "packing-list.txt");
    await act(async () => {
      await memory.dispatch({ type: "AttachmentDelete", data: { attachmentId: NOTES } });
    });
    const gone = await chipOf(row, t("attachment.deleted.file"));
    expect(within(gone).getByText(t("attachment.deleted.detail"))).toBeInTheDocument();
    expect(within(gone).queryByRole("button", { name: /^Open/ })).toBeNull();
    expect(within(gone).queryByRole("button", { name: /Save/ })).toBeNull();
    await user.click(within(gone).getByRole("button", { name: /^Remove/ }));
    await waitFor(async () => {
      expect(await filesOf(memory, "Welcome")).toEqual([IMAGE]);
    });
  });
});

describe("removing and adding", () => {
  it("Remove takes one file out with one write of the whole list, and the last one clears the value", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    const image = await chipOf(row, "atomik.png");
    await user.click(within(image).getByRole("button", { name: t("properties.files.remove", { name: "atomik.png" }) }));
    await waitFor(async () => {
      expect(await filesOf(memory, "Welcome")).toEqual([NOTES]);
    });
    expect(await within(row).findAllByRole("listitem")).toHaveLength(1);
    const notes = await chipOf(row, "packing-list.txt");
    await user.click(within(notes).getByRole("button", { name: t("properties.files.remove", { name: "packing-list.txt" }) }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Welcome", "Files")).toBeUndefined();
    });
    await waitFor(() => {
      expect(rowNames(panel)).toEqual(["Tags"]);
    });
    // The files stay in the space (they are collected after the retention).
    expect(await namesOf(memory, [IMAGE, NOTES])).toEqual(["atomik.png", "packing-list.txt"]);
  });

  it("Add file picks a file through the shell, uploads it and appends it to the value", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    memory.files.set("/docs/report.pdf", new TextEncoder().encode("%PDF-report"));
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "atomik.png");
    await user.click(within(row).getByRole("button", { name: t("properties.files.add") }));
    await chipOf(row, "report.pdf");
    const ids = await filesOf(memory, "Welcome");
    expect(ids).toHaveLength(3);
    expect(ids.slice(0, 2)).toEqual([IMAGE, NOTES]);
    expect(await namesOf(memory, ids)).toEqual(["atomik.png", "packing-list.txt", "report.pdf"]);
  });

  it("leaves everything as it was when the dialog is cancelled or the file is refused", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "atomik.png");

    memory.setCancelDialogs(true);
    await user.click(within(row).getByRole("button", { name: t("properties.files.add") }));
    await waitFor(() => {
      expect(within(row).getByRole("button", { name: t("properties.files.add") })).toBeEnabled();
    });
    expect(await filesOf(memory, "Welcome")).toEqual([IMAGE, NOTES]);
    expect(within(row).queryByRole("alert")).toBeNull();

    // A file the core refuses (above the limit; only its length is made up) shows why under the row.
    memory.setCancelDialogs(false);
    const huge = new Uint8Array(1);
    Object.defineProperty(huge, "length", { value: MAX_ATTACHMENT_BYTES + 1 });
    memory.files.set("/docs/huge.iso", huge);
    await user.click(within(row).getByRole("button", { name: t("properties.files.add") }));
    expect(await within(row).findByRole("alert")).toHaveTextContent(
      t("attachment.error.import", { message: "the file is larger than an attachment may be" }),
    );
    expect(await filesOf(memory, "Welcome")).toEqual([IMAGE, NOTES]);
    expect(within(row).getByRole("button", { name: t("properties.files.add") })).toBeEnabled();
  });

  it("does not list a file twice", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "atomik.png");
    // The same attachment made known again (as a second window would): nothing is appended twice.
    await new CoreClient(memory).pagePropertySet(
      (await idsOf(memory)).page("Welcome"),
      (await new CoreClient(memory).propertySchemaGet((await idsOf(memory)).space("Personal"))).find((def) => def.name === "Files")?.id ?? "",
      { kind: "files", value: [IMAGE, NOTES, SPARE] },
    );
    await chipOf(row, "spare.pdf");
    expect(await filesOf(memory, "Welcome")).toEqual([IMAGE, NOTES, SPARE]);
  });
});

describe("dropping files on the row", () => {
  it("uploads them one after the other and appends them in order", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "atomik.png");
    const target = row.querySelector(".property-files");
    expect(target).not.toBeNull();
    expect(dropFiles(target as Element, [textFile("first.txt", "1"), textFile("second.txt", "2")])).toBe(true);
    await chipOf(row, "second.txt");
    const ids = await filesOf(memory, "Welcome");
    expect(await namesOf(memory, ids)).toEqual(["atomik.png", "packing-list.txt", "first.txt", "second.txt"]);
  });

  it("shows what a refused file says and still takes the others", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "atomik.png");
    const target = row.querySelector(".property-files") as Element;
    dropFiles(target, [textFile(`${"y".repeat(300)}.txt`), textFile("fine.txt")]);
    await chipOf(row, "fine.txt");
    expect(await within(row).findByRole("alert")).toHaveTextContent(/cannot be added/);
    expect(await namesOf(memory, await filesOf(memory, "Welcome"))).toEqual(["atomik.png", "packing-list.txt", "fine.txt"]);
  });
});

describe("a shell without a file picker", () => {
  async function panelWith(capabilities: { importPath: boolean; upload: boolean }) {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [SPACE], attachments: ATTACHMENTS });
    const client = new CoreClient(memory);
    const { page } = await idsOf(memory);
    const info = await client.pageGet(page("Welcome"));
    const store = createStore(client, { corePollMs: 5 });
    const stop = store.getState().start();
    store.setState({
      attachmentCapabilities: { ...store.getState().attachmentCapabilities, ...capabilities },
    });
    render(
      <StoreContext value={store}>
        <PropertiesPanel page={info} />
      </StoreContext>,
    );
    const panel = await screen.findByRole("region", { name: t("properties.title") });
    return { panel, stop, store };
  }

  it("disables Add file and says to drop the file instead when files can still be dropped", async () => {
    const { panel, stop } = await panelWith({ importPath: false, upload: true });
    const add = await within(panel).findByRole("button", { name: t("properties.files.add") });
    expect(add).toBeDisabled();
    expect(within(panel).getByText(t("slash.attachment.dropInstead"))).toBeInTheDocument();
    stop();
  });

  it("says that files cannot be added when nothing can", async () => {
    const { panel, stop } = await panelWith({ importPath: false, upload: false });
    const add = await within(panel).findByRole("button", { name: t("properties.files.add") });
    expect(add).toBeDisabled();
    expect(within(panel).getByText(t("slash.attachment.unsupported"))).toBeInTheDocument();
    stop();
  });
});

describe("the limit", () => {
  it("disables Add file at fifty files and says why", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const client = new CoreClient(memory);
    const { page, space } = await idsOf(memory);
    const property = (await client.propertySchemaGet(space("Personal"))).find((def) => def.name === "Files")?.id ?? "";
    const ids = Array.from({ length: MAX_FILES }, (_, n) => `b0000000-0000-4000-8000-${String(n).padStart(12, "0")}`);
    await client.pagePropertySet(page("Welcome"), property, { kind: "files", value: ids });
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await waitFor(() => {
      expect(within(row).getAllByRole("listitem")).toHaveLength(MAX_FILES);
    });
    expect(within(row).getByRole("button", { name: t("properties.files.add") })).toBeDisabled();
    expect(within(row).getByText(t("properties.files.limit", { max: MAX_FILES }))).toBeInTheDocument();
  });
});

describe("adding the property to a page", () => {
  it("offers the Add file button, focused, and keeps the row until a file is added", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    memory.files.set("/docs/new.pdf", new TextEncoder().encode("%PDF-new"));
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    const menu = await screen.findByRole("menu", { name: t("properties.add") });
    expect(within(menu).getByRole("menuitem", { name: /^Files/ })).toHaveTextContent(t("properties.kind.files"));
    await user.click(within(menu).getByRole("menuitem", { name: /^Files/ }));
    const row = await waitFor(() => propertyRow(panel, "Files"));
    const add = within(row).getByRole("button", { name: t("properties.files.add") });
    await waitFor(() => {
      expect(add).toHaveFocus();
    });
    expect(rowNames(panel)).toEqual(["Tags", "Files"]);
    await user.click(add);
    await chipOf(row, "new.pdf");
    expect(await namesOf(memory, await filesOf(memory, "Plain"))).toEqual(["new.pdf"]);
  });
});

describe("a page in the trash", () => {
  it("shows the files and offers neither adding nor removing, but still Open and Save as", async () => {
    stubObjectUrls();
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [SPACE], attachments: ATTACHMENTS });
    const client = new CoreClient(memory);
    const { page } = await idsOf(memory);
    await client.pageTrash(page("Welcome"));
    const info = await client.pageGet(page("Welcome"));
    const store = createStore(client, { corePollMs: 5 });
    const stop = store.getState().start();
    render(
      <StoreContext value={store}>
        <PropertiesPanel page={info} />
      </StoreContext>,
    );
    const panel = await screen.findByRole("region", { name: t("properties.title") });
    const row = propertyRow(panel, "Files");
    const notes = await chipOf(row, "packing-list.txt");
    expect(within(row).queryByRole("button", { name: t("properties.files.add") })).toBeNull();
    expect(within(notes).queryByRole("button", { name: /^Remove/ })).toBeNull();
    expect(within(notes).getByRole("button", { name: /^Save/ })).toBeInTheDocument();
    expect(await within(row).findByRole("img")).toBeInTheDocument();
    stop();
  });
});

describe("following changes made elsewhere", () => {
  it("shows a file another writer attached", async () => {
    stubObjectUrls();
    const { user, memory } = await renderFiles();
    const panel = await openProperties(user, "Welcome");
    const row = propertyRow(panel, "Files");
    await chipOf(row, "atomik.png");
    const client = new CoreClient(memory);
    const { page, space } = await idsOf(memory);
    const property = (await client.propertySchemaGet(space("Personal"))).find((def) => def.name === "Files")?.id ?? "";
    await act(async () => {
      await client.pagePropertySet(page("Welcome"), property, { kind: "files", value: [NOTES, SPARE] });
    });
    await chipOf(row, "spare.pdf");
    expect(chips(row).map((chip) => chip.querySelector(".file-chip-name")?.textContent)).toEqual([
      "packing-list.txt",
      "spare.pdf",
    ]);
    fireEvent.click(within(row).getByRole("button", { name: t("properties.files.add") }), {});
  });
});

describe("the other properties are left alone", () => {
  it("a page with no files has no Files row, and a text row stays a text field", async () => {
    stubObjectUrls();
    const { user } = await renderFiles();
    const panel = await openProperties(user, "Other");
    expect(rowNames(panel)).toEqual(["Tags", "Notes"]);
    expect(within(propertyRow(panel, "Notes")).getByRole("textbox", { name: "Notes" })).toHaveValue("no files here");
  });
});
