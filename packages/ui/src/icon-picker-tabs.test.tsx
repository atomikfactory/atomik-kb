// The icon picker's tabs (ADR-0029, section 1): Emoji, Icons (the bundled set, a search, the nine
// colours) and Upload (pages only: a picture is scaled down and stored as an attachment of the
// page's space), the arrow keys between tabs and between icons, and what the picker does with the
// other kinds of icon a page can hold. The Emoji tab's own behaviour is in icon-picker.test.tsx.

import { CoreClient, CoreError, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { AttachmentInfo } from "@atomik-kb/core-client";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { IconPicker } from "./components/IconPicker";
import type { IconPickerProps } from "./components/IconPicker";
import { ICON_COLUMNS } from "./components/IconSetTab";
import { PALETTE_COLORS, PALETTE_LABELS } from "./editor/palette";
import { t } from "./i18n";
import { fitWithin, pngName } from "./icons/resize-image";
import type { ImageResizer } from "./icons/resize-image";
import { createAppStore } from "./store";
import type { AppStore } from "./store";
import { StoreContext } from "./store-context";
import { pngFile, textFile } from "./test/attachments";
import { NOW, recordingTransport } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";

/** A started store over a memory core with one space and one attachment (an image). */
async function storeWithSpace(options: { attachments?: boolean } = {}) {
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Notes" }] }],
    attachments: [{ attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() }],
  });
  const transport = options.attachments === false ? recordingTransport(memory, { attachments: false }).transport : memory;
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  const client = new CoreClient(memory);
  const [space] = await client.spaceList();
  return { memory, client, store, stop, spaceId: space?.id ?? "" };
}

function open(props: Partial<IconPickerProps> & { store?: AppStore | null } = {}) {
  const { store = null, ...rest } = props;
  const onSelect = vi.fn<(icon: string | null) => Promise<void>>(() => Promise.resolve());
  const onClose = vi.fn();
  const user = userEvent.setup();
  render(
    <StoreContext.Provider value={store}>
      <IconPicker anchor={{ x: 10, y: 10 }} current={undefined} onSelect={onSelect} onClose={onClose} {...rest} />
    </StoreContext.Provider>,
  );
  const picker = screen.getByRole("dialog", { name: t("icon.title") });
  return { user, onSelect, onClose, picker };
}

/** The labels of the tabs, in order. */
function tabNames(picker: HTMLElement): (string | null)[] {
  return within(picker)
    .getAllByRole("tab")
    .map((tab) => tab.textContent);
}

function selectedTab(picker: HTMLElement): string | null {
  return within(picker).getByRole("tab", { selected: true }).textContent;
}

describe("the Emoji tab with other kinds of icon", () => {
  it("marks the emoji that is the current icon, and keeps it in the free field", () => {
    const { picker } = open({ current: "📚" });
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveValue("📚");
    const group = within(picker).getByRole("group", { name: t("icon.suggestions") });
    const pressed = within(group)
      .getAllByRole("button")
      .filter((button) => button.getAttribute("aria-pressed") === "true");
    expect(pressed.map((button) => button.textContent)).toEqual(["📚"]);
  });

  it("never shows the raw string of an icon of the set or of an uploaded picture in the free field", async () => {
    for (const current of ["icon:book-open:blue", `attachment:${CAT}`]) {
      const { user, picker } = open({ current });
      // The Emoji tab is the one left to open by hand.
      await user.click(within(picker).getByRole("tab", { name: t("icon.tabs.emoji") }));
      expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveValue("");
      cleanup();
    }
  });
});

describe("the tabs", () => {
  it("offers Emoji and Icons, with the Emoji tab first and free typing focused", () => {
    const { picker } = open();
    expect(within(picker).getByRole("tablist", { name: t("icon.tabs.label") })).toBeInTheDocument();
    expect(tabNames(picker)).toEqual([t("icon.tabs.emoji"), t("icon.tabs.icons")]);
    expect(selectedTab(picker)).toBe(t("icon.tabs.emoji"));
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveFocus();
    expect(within(picker).getByRole("tabpanel", { name: t("icon.tabs.emoji") })).toBeInTheDocument();
  });

  it("adds Upload for a page, in a shell that stores attachments", async () => {
    const { store, stop, spaceId } = await storeWithSpace();
    const { picker } = open({ store, allowUpload: true, spaceId });
    expect(tabNames(picker)).toEqual([t("icon.tabs.emoji"), t("icon.tabs.icons"), t("icon.tabs.upload")]);
    stop();
  });

  it("has no Upload tab for a space, nor without a space to store the picture in, nor without attachments", async () => {
    const { store, stop, spaceId } = await storeWithSpace();
    const twoTabs = [t("icon.tabs.emoji"), t("icon.tabs.icons")];

    expect(tabNames(open({ store, allowUpload: false, spaceId }).picker)).toEqual(twoTabs);
    cleanup();
    expect(tabNames(open({ store, allowUpload: true }).picker)).toEqual(twoTabs);
    cleanup();
    expect(tabNames(open({ allowUpload: true, spaceId }).picker)).toEqual(twoTabs); // no store
    cleanup();

    const bare = await storeWithSpace({ attachments: false });
    expect(bare.store.getState().attachmentCapabilities.upload).toBe(false);
    expect(tabNames(open({ store: bare.store, allowUpload: true, spaceId: bare.spaceId }).picker)).toEqual(twoTabs);
    bare.stop();
    stop();
  });

  it("has no tabs at all for a callout, whose icon is an emoji", () => {
    const { picker } = open({ iconSet: false, allowUpload: false });
    expect(within(picker).queryByRole("tablist")).not.toBeInTheDocument();
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveFocus();
    expect(within(picker).getByRole("group", { name: t("icon.suggestions") })).toBeInTheDocument();
  });

  it("opens on the tab that matches the current icon", async () => {
    const { store, stop, spaceId } = await storeWithSpace();
    const cases: [string | undefined, string][] = [
      [undefined, t("icon.tabs.emoji")],
      ["🌱", t("icon.tabs.emoji")],
      ["icon:star", t("icon.tabs.icons")],
      ["icon:star:red", t("icon.tabs.icons")],
      [`attachment:${CAT}`, t("icon.tabs.upload")],
      // A newer set's name still opens the Icons tab; a malformed value, the Emoji tab.
      ["icon:no-such-icon", t("icon.tabs.icons")],
      ["icon:", t("icon.tabs.emoji")],
    ];
    for (const [current, expected] of cases) {
      const { picker } = open({ store, allowUpload: true, spaceId, current });
      expect(selectedTab(picker), String(current)).toBe(expected);
      cleanup();
    }
    // Without the Upload tab a picture opens the first one.
    const { picker } = open({ current: `attachment:${CAT}` });
    expect(selectedTab(picker)).toBe(t("icon.tabs.emoji"));
    stop();
  });

  it("moves between the tabs with the arrow keys, wrapping, Home and End, one tab stop at a time", async () => {
    const { store, stop, spaceId } = await storeWithSpace();
    const { user, picker } = open({ store, allowUpload: true, spaceId });
    const emoji = within(picker).getByRole("tab", { name: t("icon.tabs.emoji") });
    const icons = within(picker).getByRole("tab", { name: t("icon.tabs.icons") });
    const upload = within(picker).getByRole("tab", { name: t("icon.tabs.upload") });
    expect([emoji.tabIndex, icons.tabIndex, upload.tabIndex]).toEqual([0, -1, -1]);

    await user.click(emoji);
    await user.keyboard("{ArrowRight}");
    expect(icons).toHaveFocus();
    expect(icons).toHaveAttribute("aria-selected", "true");
    expect(within(picker).getByRole("tabpanel", { name: t("icon.tabs.icons") })).toBeInTheDocument();
    expect([emoji.tabIndex, icons.tabIndex, upload.tabIndex]).toEqual([-1, 0, -1]);

    await user.keyboard("{ArrowRight}");
    expect(upload).toHaveFocus();
    expect(within(picker).getByRole("tabpanel", { name: t("icon.tabs.upload") })).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    expect(emoji).toHaveFocus(); // wraps
    await user.keyboard("{ArrowLeft}");
    expect(upload).toHaveFocus();
    await user.keyboard("{Home}");
    expect(emoji).toHaveFocus();
    expect(emoji).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{End}");
    expect(upload).toHaveFocus();
    stop();
  });

  it("keeps Remove under every tab", async () => {
    const { user, onSelect, picker } = open({ current: "icon:star" });
    expect(selectedTab(picker)).toBe(t("icon.tabs.icons"));
    await user.click(within(picker).getByRole("tab", { name: t("icon.tabs.emoji") }));
    expect(within(picker).getByRole("button", { name: t("icon.remove") })).toBeEnabled();
    await user.click(within(picker).getByRole("tab", { name: t("icon.tabs.icons") }));
    await user.click(within(picker).getByRole("button", { name: t("icon.remove") }));
    expect(onSelect).toHaveBeenCalledWith(null);
  });
});

// ----- the Icons tab -----------------------------------------------------------------------------

function iconButton(picker: HTMLElement, name: string): HTMLElement {
  return within(picker).getByRole("button", { name });
}

function colorGroup(picker: HTMLElement): HTMLElement {
  return within(picker).getByRole("group", { name: t("icon.set.color") });
}

function gridButtons(picker: HTMLElement): HTMLElement[] {
  return within(picker.querySelector(".icon-set-body") as HTMLElement).getAllByRole("button");
}

async function openIcons(props: Partial<IconPickerProps> = {}) {
  const view = open(props);
  if (props.current === undefined || !props.current.startsWith("icon:")) {
    await view.user.click(within(view.picker).getByRole("tab", { name: t("icon.tabs.icons") }));
  }
  return view;
}

describe("the Icons tab", () => {
  it("lists the icons by category, with a search and the colours, in one tab stop", async () => {
    const { picker } = await openIcons();
    expect(within(picker).getByRole("searchbox", { name: t("icon.set.search") })).toHaveAttribute(
      "placeholder",
      t("icon.set.search.placeholder"),
    );
    for (const category of ["documents", "work", "people", "places", "nature", "food", "objects", "symbols", "arrows", "tech"] as const) {
      const group = within(picker).getByRole("group", { name: t(`icon.set.category.${category}`) });
      expect(within(group).getAllByRole("button").length, category).toBeGreaterThanOrEqual(10);
    }
    const buttons = gridButtons(picker);
    expect(buttons.length).toBeGreaterThanOrEqual(190);
    expect(buttons.filter((button) => button.tabIndex === 0)).toHaveLength(1);
    expect(buttons[0]).toHaveAttribute("title", "file text");
  });

  it("opens with the search field focused when the current icon is one of the set", async () => {
    const { picker } = await openIcons({ current: "icon:star:red" });
    expect(within(picker).getByRole("searchbox", { name: t("icon.set.search") })).toHaveFocus();
  });

  it("stores icon:<name> for the default colour", async () => {
    const { user, onSelect, onClose, picker } = await openIcons();
    const [defaultColor] = within(colorGroup(picker)).getAllByRole("button");
    expect(defaultColor).toHaveAccessibleName(t("icon.set.color.default"));
    expect(defaultColor).toHaveAttribute("aria-pressed", "true");
    await user.click(iconButton(picker, "book open"));
    expect(onSelect).toHaveBeenCalledWith("icon:book-open");
    await waitFor(() => {
      expect(onClose).toHaveBeenCalled();
    });
  });

  it("stores icon:<name>:<color> for a colour chosen first", async () => {
    const { user, onSelect, picker } = await openIcons();
    const colors = colorGroup(picker);
    expect(within(colors).getAllByRole("button")).toHaveLength(10);
    for (const color of PALETTE_COLORS) {
      expect(within(colors).getByRole("button", { name: t(PALETTE_LABELS[color]) })).toHaveAttribute("aria-pressed", "false");
    }
    await user.click(within(colors).getByRole("button", { name: t("callout.color.blue") }));
    expect(within(colors).getByRole("button", { name: t("callout.color.blue") })).toHaveAttribute("aria-pressed", "true");
    expect(within(colors).getByRole("button", { name: t("icon.set.color.default") })).toHaveAttribute("aria-pressed", "false");
    // The grid shows the icons in that colour.
    expect(iconButton(picker, "star").querySelector("svg")).toHaveClass("page-icon-color-blue");

    await user.click(iconButton(picker, "star"));
    expect(onSelect).toHaveBeenLastCalledWith("icon:star:blue");
  });

  it("writes each colour of the palette, and icon:<name> again after the default colour is chosen back", async () => {
    for (const color of PALETTE_COLORS) {
      const { picker, onSelect } = open({ current: "icon:house" }); // opens on the Icons tab
      fireEvent.click(within(colorGroup(picker)).getByRole("button", { name: t(PALETTE_LABELS[color]) }));
      fireEvent.click(iconButton(picker, "house"));
      expect(onSelect, color).toHaveBeenCalledWith(`icon:house:${color}`);
      cleanup();
    }
    const { user, onSelect, picker } = await openIcons();
    await user.click(within(colorGroup(picker)).getByRole("button", { name: t("callout.color.red") }));
    await user.click(within(colorGroup(picker)).getByRole("button", { name: t("icon.set.color.default") }));
    await user.click(iconButton(picker, "house"));
    expect(onSelect).toHaveBeenCalledWith("icon:house");
  });

  it("searches names and keywords, shows only the matches, and takes the first one with Enter", async () => {
    const { user, onSelect, picker } = await openIcons();
    const field = within(picker).getByRole("searchbox", { name: t("icon.set.search") });
    await user.type(field, "doctor");
    const results = within(picker).getByRole("group", { name: t("icon.set.results") });
    expect(within(results).getAllByRole("button").map((button) => button.getAttribute("title"))).toContain("stethoscope");
    expect(within(picker).queryByRole("group", { name: t("icon.set.category.documents") })).not.toBeInTheDocument();

    await user.clear(field);
    await user.type(field, "book");
    const [first] = within(within(picker).getByRole("group", { name: t("icon.set.results") })).getAllByRole("button");
    expect(first).toHaveAttribute("title", "book");
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("icon:book");
  });

  it("says so when nothing matches, does nothing on Enter, and brings the categories back when cleared", async () => {
    const { user, onSelect, picker } = await openIcons();
    const field = within(picker).getByRole("searchbox", { name: t("icon.set.search") });
    await user.type(field, "zzzq{Enter}");
    expect(within(picker).getByText(t("icon.set.noMatch"))).toBeInTheDocument();
    expect(within(picker).queryByRole("group", { name: t("icon.set.results") })).not.toBeInTheDocument();
    expect(onSelect).not.toHaveBeenCalled();
    await user.clear(field);
    expect(within(picker).queryByText(t("icon.set.noMatch"))).not.toBeInTheDocument();
    expect(within(picker).getByRole("group", { name: t("icon.set.category.documents") })).toBeInTheDocument();
  });

  it("marks the current icon and its colour, and keeps that colour for the next choice", async () => {
    const { user, onSelect, picker } = await openIcons({ current: "icon:book-open:green" });
    expect(iconButton(picker, "book open")).toHaveAttribute("aria-pressed", "true");
    expect(iconButton(picker, "star")).toHaveAttribute("aria-pressed", "false");
    expect(within(colorGroup(picker)).getByRole("button", { name: t("callout.color.green") })).toHaveAttribute("aria-pressed", "true");
    // The grid's one tab stop is the current icon.
    expect(iconButton(picker, "book open").tabIndex).toBe(0);
    expect(iconButton(picker, "file text").tabIndex).toBe(-1);

    await user.click(iconButton(picker, "star"));
    expect(onSelect).toHaveBeenCalledWith("icon:star:green");
  });

  it("marks no icon for a current icon of a newer set, and still shows its colour", async () => {
    const { picker } = await openIcons({ current: "icon:no-such-icon:blue" });
    for (const button of gridButtons(picker)) {
      expect(button).toHaveAttribute("aria-pressed", "false");
    }
    expect(within(colorGroup(picker)).getByRole("button", { name: t("callout.color.blue") })).toHaveAttribute("aria-pressed", "true");
  });

  it("moves between icons with the arrow keys: one tab stop, rows of eight", async () => {
    const { user, picker } = await openIcons({ current: "icon:file-text" });
    const buttons = gridButtons(picker);
    expect(ICON_COLUMNS).toBe(8);
    buttons[0]?.focus();
    await user.keyboard("{ArrowRight}");
    expect(buttons[1]).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(buttons[1 + ICON_COLUMNS]).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(buttons[ICON_COLUMNS]).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(buttons[0]).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(buttons[0]).toHaveFocus(); // nothing above the first row
    await user.keyboard("{End}");
    expect(buttons[buttons.length - 1]).toHaveFocus();
    await user.keyboard("{Home}");
    expect(buttons[0]).toHaveFocus();
    // The tab stop followed the focus.
    expect(buttons.filter((button) => button.tabIndex === 0)).toEqual([buttons[0]]);
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(buttons.filter((button) => button.tabIndex === 0)).toEqual([buttons[2]]);
  });

  it("shows what the core refused for the icon, and stays open", async () => {
    const onSelect = vi.fn<(icon: string | null) => Promise<void>>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "is not an icon", details: { field: "icon" } })),
    );
    const { user, onClose, picker } = await openIcons({ onSelect });
    await user.click(iconButton(picker, "star"));
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("form.fieldError.icon", { message: "is not an icon" }));
    expect(onClose).not.toHaveBeenCalled();
    expect(iconButton(picker, "star")).toBeEnabled();
  });
});

// ----- the Upload tab ----------------------------------------------------------------------------

/** The picture a stand-in resizer makes: the chosen bytes as `icon.png`. */
function standInResizer() {
  return vi.fn<ImageResizer>((file) => Promise.resolve(new File([file], "icon.png", { type: "image/png" })));
}

async function openUpload(extra: Partial<IconPickerProps> = {}) {
  const ctx = await storeWithSpace();
  const resizeImage = standInResizer();
  const view = open({ store: ctx.store, allowUpload: true, spaceId: ctx.spaceId, resizeImage, ...extra });
  if (extra.current === undefined || !extra.current.startsWith("attachment:")) {
    await view.user.click(within(view.picker).getByRole("tab", { name: t("icon.tabs.upload") }));
  }
  const fileInput = view.picker.querySelector<HTMLInputElement>('input[type="file"]');
  if (fileInput === null) {
    throw new Error("no file input");
  }
  return { ...ctx, ...view, resizeImage, fileInput };
}

function choose(input: HTMLInputElement, file: File): void {
  fireEvent.change(input, { target: { files: [file] } });
}

describe("the Upload tab", () => {
  it("offers a button, a drop zone and the 256 pixel hint", async () => {
    const { picker, fileInput, stop } = await openUpload();
    expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toBeEnabled();
    expect(within(picker).getByText(t("icon.upload.drop"))).toBeInTheDocument();
    expect(within(picker).getByText(t("icon.upload.hint"))).toBeInTheDocument();
    expect(fileInput).toHaveAttribute("accept", "image/*");
    expect(fileInput).not.toBeVisible();
    stop();
  });

  it("opens the file picker from the button", async () => {
    const { user, picker, fileInput, stop } = await openUpload();
    const click = vi.spyOn(fileInput, "click").mockImplementation(() => undefined);
    await user.click(within(picker).getByRole("button", { name: t("icon.upload.choose") }));
    expect(click).toHaveBeenCalledTimes(1);
    stop();
  });

  it("scales the chosen picture, stores it as an attachment of the page's space, and sets attachment:<id>", async () => {
    const { client, spaceId, onSelect, onClose, resizeImage, fileInput, stop } = await openUpload();
    const photo = new File([demoImagePng()], "photo.jpeg", { type: "image/jpeg" });
    choose(fileInput, photo);

    await waitFor(() => {
      expect(onSelect).toHaveBeenCalledTimes(1);
    });
    expect(resizeImage).toHaveBeenCalledWith(photo);
    const value = onSelect.mock.calls[0]?.[0] ?? "";
    expect(value).toMatch(/^attachment:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    const list: AttachmentInfo[] = await client.attachmentList(spaceId);
    const uploaded = list.find((info) => `attachment:${info.attachmentId}` === value);
    expect(uploaded).toMatchObject({ name: "icon.png", mime: "image/png", spaceId });
    await waitFor(() => {
      expect(onClose).toHaveBeenCalled();
    });
    stop();
  });

  it("takes a picture dropped on the zone", async () => {
    const { picker, onSelect, resizeImage, stop } = await openUpload();
    const zone = picker.querySelector(".icon-upload") as HTMLElement;
    fireEvent.dragOver(zone, { dataTransfer: { files: [] } });
    expect(zone).toHaveClass("is-dragging");
    const file = pngFile("dropped.png");
    fireEvent.drop(zone, { dataTransfer: { files: [file] } });
    expect(zone).not.toHaveClass("is-dragging");
    await waitFor(() => {
      expect(onSelect).toHaveBeenCalledTimes(1);
    });
    expect(resizeImage).toHaveBeenCalledWith(file);
    expect(onSelect.mock.calls[0]?.[0]).toMatch(/^attachment:/);
    stop();
  });

  it("takes a picture pasted on the zone, and leaves a paste of text alone", async () => {
    const { picker, onSelect, resizeImage, stop } = await openUpload();
    const zone = picker.querySelector(".icon-upload") as HTMLElement;
    const text = fireEvent.paste(zone, { clipboardData: { files: [], getData: () => "hello" } });
    expect(text).toBe(true); // not cancelled
    expect(resizeImage).not.toHaveBeenCalled();

    const file = pngFile("screenshot.png");
    expect(fireEvent.paste(zone, { clipboardData: { files: [file] } })).toBe(false);
    await waitFor(() => {
      expect(onSelect).toHaveBeenCalledTimes(1);
    });
    expect(resizeImage).toHaveBeenCalledWith(file);
    stop();
  });

  it("refuses a file that is not an image, before reading or storing anything", async () => {
    const { client, spaceId, picker, onSelect, resizeImage, fileInput, stop } = await openUpload();
    const before = (await client.attachmentList(spaceId)).length;
    choose(fileInput, textFile());
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("icon.upload.error.notImage"));
    expect(resizeImage).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled();
    expect(await client.attachmentList(spaceId)).toHaveLength(before);
    expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toBeEnabled();
    stop();
  });

  it("says so when the picture cannot be read, and stores nothing", async () => {
    const { client, spaceId, picker, onSelect, resizeImage, fileInput, stop } = await openUpload();
    resizeImage.mockRejectedValueOnce(new Error("the image cannot be decoded"));
    const before = (await client.attachmentList(spaceId)).length;
    choose(fileInput, pngFile());
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("icon.upload.error.decode"));
    expect(onSelect).not.toHaveBeenCalled();
    expect(await client.attachmentList(spaceId)).toHaveLength(before);
    expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toBeEnabled();
    stop();
  });

  it("shows how far the upload is, and the core's message when it fails", async () => {
    const { store, picker, onSelect, fileInput, stop } = await openUpload();
    let fail: (error: unknown) => void = () => undefined;
    store.setState({
      uploadAttachment: (_spaceId, _file, onProgress) => {
        onProgress?.(5, 10);
        return new Promise<AttachmentInfo>((_resolve, reject) => {
          fail = reject;
        });
      },
    });
    choose(fileInput, pngFile());
    const bar = await within(picker).findByRole("progressbar", { name: t("icon.upload.progress") });
    expect(bar).toHaveAttribute("max", "10");
    expect(bar).toHaveAttribute("value", "5");
    expect(within(picker).getByText(t("icon.upload.uploading"))).toBeInTheDocument();
    expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toBeDisabled();

    fail(new CoreError({ code: "Storage", message: "disk full" }));
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("icon.upload.error", { message: "disk full" }));
    expect(within(picker).queryByRole("progressbar")).not.toBeInTheDocument();
    expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toBeEnabled();
    expect(onSelect).not.toHaveBeenCalled();
    stop();
  });

  it("shows what the core refuses for the icon after the upload, and stays open", async () => {
    const onSelect = vi.fn<(icon: string | null) => Promise<void>>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "must be an image", details: { field: "icon" } })),
    );
    const { picker, onClose, fileInput, stop } = await openUpload({ onSelect });
    choose(fileInput, pngFile());
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("form.fieldError.icon", { message: "must be an image" }));
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toBeEnabled();
    });
    stop();
  });

  it("opens on the Upload tab for a current picture, shows it, and focuses the button", async () => {
    const { picker, stop } = await openUpload({ current: `attachment:${CAT}` });
    expect(selectedTab(picker)).toBe(t("icon.tabs.upload"));
    expect(within(picker).getByText(t("icon.upload.current"))).toBeInTheDocument();
    expect(picker.querySelector(".icon-upload-preview")).not.toBeNull();
    expect(within(picker).getByRole("button", { name: t("icon.upload.choose") })).toHaveFocus();
    expect(within(picker).getByRole("button", { name: t("icon.remove") })).toBeEnabled();
    stop();
  });

  it("has no Upload tab, and no file input, for a space picker, even in a shell with attachments", async () => {
    const { store, stop, spaceId } = await storeWithSpace();
    const { picker } = open({ store, allowUpload: false, spaceId });
    expect(within(picker).queryByRole("tab", { name: t("icon.tabs.upload") })).not.toBeInTheDocument();
    expect(picker.querySelector('input[type="file"]')).toBeNull();
    stop();
  });
});

describe("the picture an icon is made of", () => {
  it("shrinks a picture to fit 256 x 256, keeps its proportions, and never enlarges it", () => {
    expect(fitWithin(1024, 512)).toEqual({ width: 256, height: 128 });
    expect(fitWithin(300, 600)).toEqual({ width: 128, height: 256 });
    expect(fitWithin(256, 256)).toEqual({ width: 256, height: 256 });
    expect(fitWithin(64, 32)).toEqual({ width: 64, height: 32 });
    expect(fitWithin(4000, 1)).toEqual({ width: 256, height: 1 });
    expect(fitWithin(512, 512, 64)).toEqual({ width: 64, height: 64 });
    // A picture with no size of its own (an SVG without dimensions) fills the square.
    expect(fitWithin(0, 0)).toEqual({ width: 256, height: 256 });
    expect(fitWithin(Number.NaN, 10)).toEqual({ width: 256, height: 256 });
  });

  it("names the upload after the file, as a PNG", () => {
    expect(pngName("photo.jpeg")).toBe("photo.png");
    expect(pngName("my.cat.GIF")).toBe("my.cat.png");
    expect(pngName("logo")).toBe("logo.png");
    expect(pngName(".hidden")).toBe("icon.png");
    expect(pngName("")).toBe("icon.png");
  });
});
