// Page appearance in the app (ADR-0040): the font and the small text of a page (three "Ag" buttons and a switch in
// the page menu, classes on the page's section that reach this page only, kept by print) and the lock (a switch in
// the menu, "Locked" with an Unlock button in the header, a read-only title, icon, cover, properties and text,
// flipping live when another device changes it, and a guard of the UI that stops nothing else).

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransportOptions, Request, SeedSpace, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { LoroList, LoroMap } from "loro-crdt";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import { requestFindInPage } from "./editor/FindBar";
import { t } from "./i18n";
import { PAGE_FONTS, pageLookClasses } from "./components/PageLook";
import { AI_ON, AI_SETTING } from "./test/ai";
import { dropFiles, pasteFiles, pngFile, stubObjectUrls } from "./test/attachments";
import { paragraphs } from "./test/editor";
import { contentOf, findEditor, idsOf, openPage } from "./test/page";
import { NOW, findMainNavigation, recordingTransport, renderWith } from "./test/render";
import { row } from "./test/sidebar";

afterEach(() => {
  vi.restoreAllMocks();
});

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    properties: [{ name: "Summary", kind: "text" }],
    pages: [
      { title: "Notes", content: paragraphs("Hello"), properties: { Summary: "Draft" } },
      { title: "Plain", content: paragraphs("Elsewhere") },
      { title: "Serif", content: paragraphs("Georgia"), font: "serif", smallText: true },
      { title: "Fixed", content: paragraphs("Locked down"), locked: true, font: "mono", properties: { Summary: "Final" } },
    ],
  },
];

/** The app over the memory core, with the requests it sends recorded. */
async function open(
  options: MemoryTransportOptions & { wrap?: (transport: Transport) => Transport; spaces?: SeedSpace[]; plain?: boolean } = {},
) {
  const { wrap, spaces = SPACES, plain = false, ...rest } = options;
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces, ...rest });
  const recording = recordingTransport(memory);
  // The recording wrapper does not carry the AI of the memory core: a test that needs it renders the core itself.
  const transport = plain ? memory : recording.transport;
  const { user } = renderWith(wrap === undefined ? transport : wrap(transport));
  await findMainNavigation();
  return { memory, user, requests: recording.requests, client: new CoreClient(memory) };
}

const section = (): HTMLElement => {
  const found = document.querySelector<HTMLElement>("section.page");
  if (found === null) {
    throw new Error("no page is open");
  }
  return found;
};

const appearanceRequests = (requests: readonly Request[]) =>
  requests.flatMap((request) => (request.type === "PageSetAppearance" ? [request.data] : []));

async function openMenu(user: Awaited<ReturnType<typeof open>>["user"]): Promise<HTMLElement> {
  const button = screen.getByRole("button", { name: t("page.actions") });
  if (screen.queryByRole("menu", { name: t("page.actions") }) === null) {
    await user.click(button);
  }
  return screen.findByRole("menu", { name: t("page.actions") });
}

const fontButton = (menu: HTMLElement, name: "default" | "serif" | "mono") =>
  within(menu).getByRole("button", { name: t("page.look.fontNamed", { name: t(`page.look.${name}`) }) });
const smallTextSwitch = (menu: HTMLElement) => within(menu).getByRole("switch", { name: t("page.look.smallText") });
const lockSwitch = (menu: HTMLElement) => within(menu).getByRole("switch", { name: t("page.look.lock") });

const lockedStatus = () => screen.queryByText(t("page.locked"));
const unlockButton = () => screen.queryByRole("button", { name: t("page.unlock") });

async function pageOf(client: CoreClient, memory: MemoryTransport, title: string) {
  return client.pageGet((await idsOf(memory)).page(title));
}

describe("the font and the small text of a page", () => {
  it("has three font buttons and two switches in the page menu, and each sets only its own part", async () => {
    const { user, memory, client, requests } = await open();
    await openPage(user, "Notes");
    const menu = await openMenu(user);

    // The buttons: the default font is pressed; each carries the sample "Ag" and its name.
    const group = within(menu).getByRole("group", { name: t("page.look.font") });
    expect(within(group).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(
      PAGE_FONTS.map(({ name }) => t("page.look.fontNamed", { name: t(name) })),
    );
    expect(within(group).getAllByRole("button").map((button) => button.textContent)).toEqual(
      PAGE_FONTS.map(({ name }) => `${t("page.look.glyph")}${t(name)}`),
    );
    expect(fontButton(menu, "default")).toHaveAttribute("aria-pressed", "true");
    expect(fontButton(menu, "serif")).toHaveAttribute("aria-pressed", "false");
    expect(smallTextSwitch(menu)).toHaveAttribute("aria-checked", "false");
    expect(lockSwitch(menu)).toHaveAttribute("aria-checked", "false");
    expect(section()).toHaveClass("page");
    expect(section().className).toBe("page");

    await user.click(fontButton(menu, "serif"));
    await waitFor(() => {
      expect(section()).toHaveClass("page-font-serif");
    });
    expect(await pageOf(client, memory, "Notes")).toMatchObject({ font: "serif" });
    // Only the changed part is asked for, and the menu stays open with the new state.
    expect(appearanceRequests(requests)).toEqual([{ pageId: (await idsOf(memory)).page("Notes"), font: "serif" }]);
    expect(screen.getByRole("menu", { name: t("page.actions") })).toBeInTheDocument();
    expect(fontButton(menu, "serif")).toHaveAttribute("aria-pressed", "true");
    expect(fontButton(menu, "default")).toHaveAttribute("aria-pressed", "false");

    await user.click(fontButton(menu, "mono"));
    await waitFor(() => {
      expect(section()).toHaveClass("page-font-mono");
    });
    expect(section()).not.toHaveClass("page-font-serif");
    expect(fontButton(menu, "mono")).toHaveAttribute("aria-pressed", "true");

    await user.click(smallTextSwitch(menu));
    await waitFor(() => {
      expect(section()).toHaveClass("page-small-text");
    });
    expect(smallTextSwitch(menu)).toHaveAttribute("aria-checked", "true");
    expect(section()).toHaveClass("page-font-mono");
    expect(appearanceRequests(requests).at(-1)).toEqual({ pageId: (await idsOf(memory)).page("Notes"), smallText: true });
    expect(await pageOf(client, memory, "Notes")).toMatchObject({ font: "mono", smallText: true });

    // The default font and the switch off clear each part.
    await user.click(fontButton(menu, "default"));
    await waitFor(() => {
      expect(section()).not.toHaveClass("page-font-mono");
    });
    await user.click(smallTextSwitch(menu));
    await waitFor(() => {
      expect(section()).not.toHaveClass("page-small-text");
    });
    expect(appearanceRequests(requests).slice(-2)).toEqual([
      { pageId: (await idsOf(memory)).page("Notes"), font: "default" },
      { pageId: (await idsOf(memory)).page("Notes"), smallText: false },
    ]);
    const cleared = await pageOf(client, memory, "Notes");
    expect(cleared.font).toBeUndefined();
    expect(cleared.smallText).toBeUndefined();
    expect(section().className).toBe("page");
  });

  it("does not ask again for the font that is already pressed", async () => {
    const { user, requests } = await open();
    await openPage(user, "Notes");
    const menu = await openMenu(user);
    await user.click(fontButton(menu, "default"));
    expect(appearanceRequests(requests)).toEqual([]);
  });

  it("reaches this page's title, properties and text only: the classes are on the page's section and nowhere else", async () => {
    const { user, client, memory } = await open();
    await openPage(user, "Notes");
    await client.pageSetAppearance((await idsOf(memory)).page("Notes"), { font: "serif", smallText: true });
    await waitFor(() => {
      expect(section()).toHaveClass("page-font-serif", "page-small-text");
    });
    // The title, the properties and the editor are inside the section; the app around it is not.
    expect(section()).toContainElement(screen.getByRole("heading", { level: 1 }));
    expect(section()).toContainElement(screen.getByRole("textbox", { name: t("editor.label") }));
    expect(section()).toContainElement(screen.getByRole("region", { name: t("properties.title") }));
    const marked = [...document.querySelectorAll(".page-font-serif, .page-font-mono, .page-small-text, .page-locked")];
    expect(marked).toEqual([section()]);
    expect(screen.getByRole("navigation", { name: t("nav.label") })).not.toHaveClass("page-font-serif");
    expect(document.body).not.toHaveClass("page-font-serif");
    // The menu floats in the document body, outside the section: the app's own face.
    const menu = await openMenu(user);
    expect(section()).not.toContainElement(menu);
  });

  it("is each page's own: another page has its look and the app has none", async () => {
    const { user } = await open();
    await openPage(user, "Serif");
    expect(section()).toHaveClass("page-font-serif", "page-small-text");
    await openPage(user, "Plain");
    expect(section().className).toBe("page");
    await openPage(user, "Serif");
    expect(section()).toHaveClass("page-font-serif", "page-small-text");
    const menu = await openMenu(user);
    expect(fontButton(menu, "serif")).toHaveAttribute("aria-pressed", "true");
    expect(smallTextSwitch(menu)).toHaveAttribute("aria-checked", "true");
    expect(lockSwitch(menu)).toHaveAttribute("aria-checked", "false");
  });

  it("follows a change made on another device", async () => {
    const { user, client, memory } = await open();
    await openPage(user, "Notes");
    expect(section().className).toBe("page");
    await act(async () => {
      await client.pageSetAppearance((await idsOf(memory)).page("Notes"), { font: "mono" });
    });
    await waitFor(() => {
      expect(section()).toHaveClass("page-font-mono");
    });
    await act(async () => {
      await client.pageSetAppearance((await idsOf(memory)).page("Notes"), { font: "default", smallText: true });
    });
    await waitFor(() => {
      expect(section().className).toBe("page page-small-text");
    });
  });

  it("is reached with the keyboard: the arrow keys visit the buttons and the switches after the commands", async () => {
    const { user } = await open();
    await openPage(user, "Notes");
    await openMenu(user);
    await user.keyboard("{End}");
    expect(screen.getByRole("switch", { name: t("page.look.lock") })).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("switch", { name: t("page.look.smallText") })).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("button", { name: t("page.look.fontNamed", { name: t("page.look.mono") }) })).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(screen.getByRole("button", { name: t("page.look.fontNamed", { name: t("page.look.default") }) })).toHaveFocus();
    await user.keyboard("{Enter}");
    // Pressing the default font again is nothing; the menu stays.
    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => {
      expect(section()).toHaveClass("page-font-serif");
    });
    expect(screen.getByRole("menu", { name: t("page.actions") })).toBeInTheDocument();
    await user.keyboard("{Home}");
    expect(within(screen.getByRole("menu")).getAllByRole("menuitem")[0]).toHaveFocus();
    // Escape closes the menu and gives focus back to the button.
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu", { name: t("page.actions") })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("page.actions") })).toHaveFocus();
  });

  it("says why a change was refused and leaves the buttons as they were", async () => {
    const wrap = (inner: Transport): Transport => ({
      ...inner,
      dispatch: (request) =>
        request.type === "PageSetAppearance"
          ? Promise.reject(new CoreError({ code: "Storage", message: "the disk is full" }))
          : inner.dispatch(request),
    });
    const { user, client, memory } = await open({ wrap });
    await openPage(user, "Notes");
    const menu = await openMenu(user);
    await user.click(fontButton(menu, "serif"));
    expect(await screen.findByText(t("page.look.error", { message: "the disk is full" }))).toBeInTheDocument();
    expect(fontButton(menu, "serif")).toHaveAttribute("aria-pressed", "false");
    expect(section().className).toBe("page");
    expect((await pageOf(client, memory, "Notes")).font).toBeUndefined();
  });

  it("is a class list that reads only the two fonts", () => {
    expect(pageLookClasses({})).toBe("page");
    expect(pageLookClasses({ font: "serif" })).toBe("page page-font-serif");
    expect(pageLookClasses({ font: "mono", smallText: true, locked: true })).toBe("page page-font-mono page-small-text page-locked");
    expect(pageLookClasses({ font: "default" })).toBe("page");
    expect(pageLookClasses({ smallText: false, locked: false })).toBe("page");
  });
});

describe("a locked page", () => {
  it("is locked from the menu: Locked and Unlock in the header, and nothing on the page can be edited", async () => {
    const { user, requests, client, memory } = await open();
    const editor = await openPage(user, "Notes");
    expect(editor.isEditable).toBe(true);
    expect(lockedStatus()).not.toBeInTheDocument();
    const panel = screen.getByRole("region", { name: t("properties.title") });
    expect(within(panel).getByRole("textbox", { name: "Summary" })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: t("properties.add") })).toBeInTheDocument();
    expect(within(screen.getByRole("heading", { level: 1 })).getByRole("button", { name: "Notes" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("page.addIcon") })).toBeEnabled();
    expect(screen.getByRole("button", { name: t("page.addCover") })).toBeInTheDocument();

    const menu = await openMenu(user);
    await user.click(lockSwitch(menu));
    await waitFor(() => {
      expect(lockedStatus()).toBeInTheDocument();
    });
    expect(appearanceRequests(requests)).toEqual([{ pageId: (await idsOf(memory)).page("Notes"), locked: true }]);
    expect(section()).toHaveClass("page-locked");
    expect(lockSwitch(menu)).toHaveAttribute("aria-checked", "true");
    expect(unlockButton()).toBeEnabled();
    expect((await pageOf(client, memory, "Notes")).locked).toBe(true);

    // The text: the editor is not editable and its element says so.
    await waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(screen.getByRole("textbox", { name: t("editor.label") })).toHaveAttribute("contenteditable", "false");
    // The title is text, not a button to rename it.
    const heading = screen.getByRole("heading", { level: 1 });
    expect(within(heading).queryByRole("button")).not.toBeInTheDocument();
    expect(heading).toHaveTextContent("Notes");
    // The icon and the cover cannot be changed.
    expect(screen.getByRole("button", { name: t("page.addIcon") })).toBeDisabled();
    expect(screen.queryByRole("button", { name: t("page.addCover") })).not.toBeInTheDocument();
    // The properties are values, not fields, and no property can be added.
    expect(within(panel).queryByRole("textbox", { name: "Summary" })).not.toBeInTheDocument();
    expect(within(panel).getByText("Draft")).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: t("properties.add") })).not.toBeInTheDocument();
    // The commands of the menu are still there: the lock stops edits, not structure.
    for (const name of [t("page.find"), t("page.copyMarkdown"), t("page.copyLink"), t("page.duplicate"), t("page.info")]) {
      expect(within(menu).getByRole("menuitem", { name })).toBeEnabled();
    }

    // Unlock puts everything back.
    await user.click(unlockButton() as HTMLElement);
    await waitFor(() => {
      expect(lockedStatus()).not.toBeInTheDocument();
    });
    expect(appearanceRequests(requests).at(-1)).toEqual({ pageId: (await idsOf(memory)).page("Notes"), locked: false });
    await waitFor(() => {
      expect(editor.isEditable).toBe(true);
    });
    expect(screen.getByRole("textbox", { name: t("editor.label") })).toHaveAttribute("contenteditable", "true");
    expect(within(screen.getByRole("heading", { level: 1 })).getByRole("button", { name: "Notes" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("page.addIcon") })).toBeEnabled();
    expect(screen.getByRole("button", { name: t("page.addCover") })).toBeInTheDocument();
    expect(within(panel).getByRole("textbox", { name: "Summary" })).toBeInTheDocument();
    expect(section()).not.toHaveClass("page-locked");
    // The press on Unlock closed the menu (it was outside it); the switch of a new menu is off.
    expect(lockSwitch(await openMenu(user))).toHaveAttribute("aria-checked", "false");
  });

  it("opens read-only when it was locked before: no editable moment, the font it has, a title that is not a button", async () => {
    const { user } = await open();
    const editor = await openPage(user, "Fixed");
    expect(editor.isEditable).toBe(false);
    expect(screen.getByRole("textbox", { name: t("editor.label") })).toHaveAttribute("contenteditable", "false");
    expect(lockedStatus()).toBeInTheDocument();
    expect(section()).toHaveClass("page-locked", "page-font-mono");
    expect(within(screen.getByRole("heading", { level: 1 })).queryByRole("button")).not.toBeInTheDocument();
    // The warning for content this version cannot show is not drawn: the header says what it is.
    expect(screen.queryByText(t("editor.readOnly"))).not.toBeInTheDocument();
    const menu = await openMenu(user);
    expect(lockSwitch(menu)).toHaveAttribute("aria-checked", "true");
    // The next page is not locked.
    await user.keyboard("{Escape}");
    await openPage(user, "Plain");
    expect(lockedStatus()).not.toBeInTheDocument();
    expect((await findEditor()).isEditable).toBe(true);
  });

  it("writes nothing to the document, and the same edit goes through once the page is unlocked", async () => {
    const { user, memory, client } = await open();
    const editor = await openPage(user, "Fixed");
    const page = (await idsOf(memory)).page("Fixed");
    expect(editor.isEditable).toBe(false);
    // Even a command (which does not ask the view whether it is editable) reaches no document.
    editor.commands.insertContentAt(editor.state.doc.content.size, { type: "paragraph", content: [{ type: "text", text: "while locked" }] });
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(JSON.stringify(await contentOf(memory, page))).not.toContain("while locked");

    await act(async () => {
      await client.pageSetAppearance(page, { locked: false });
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(true);
    });
    editor.commands.insertContentAt(editor.state.doc.content.size, { type: "paragraph", content: [{ type: "text", text: "after unlock" }] });
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page))).toContain("after unlock");
    });
  });

  it("stays read-only after the unlock when it was read-only for another reason: a block this version cannot show", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "p0" }, content: [{ type: "text", text: "Kept as it is" }] },
        { type: "kanban", attrs: { id: "k0", columns: 3 } },
      ],
    };
    const { user, client, memory } = await open({ spaces: [{ name: "Personal", pages: [{ title: "Board", content, locked: true }] }] });
    const editor = await openPage(user, "Board");
    expect(editor.isEditable).toBe(false);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    await act(async () => {
      await client.pageSetAppearance((await idsOf(memory)).page("Board"), { locked: false });
    });
    await waitFor(() => {
      expect(lockedStatus()).not.toBeInTheDocument();
    });
    // The lock is gone; the page is still not editable, and still says why.
    expect(editor.isEditable).toBe(false);
    expect(screen.getByRole("textbox", { name: t("editor.label") })).toHaveAttribute("contenteditable", "false");
    expect(screen.getByText(t("editor.readOnly"))).toBeInTheDocument();
  });

  it("becomes read-only for the block another device added while it was locked, once it is unlocked", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user, client, memory } = await open();
    const editor = await openPage(user, "Notes");
    const page = (await idsOf(memory)).page("Notes");
    await act(async () => {
      await client.pageSetAppearance(page, { locked: true });
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    act(() => {
      memory.changePage(
        page,
        (doc) => {
          const children = doc.getMap("doc").get("children") as LoroList;
          const node = children.insertContainer(children.length, new LoroMap());
          node.set("nodeName", "kanban");
          node.setContainer("attributes", new LoroMap()).set("id", "k0");
          node.setContainer("children", new LoroList());
        },
        "Core",
      );
    });
    await act(async () => {
      await client.pageSetAppearance(page, { locked: false });
    });
    await waitFor(() => {
      expect(lockedStatus()).not.toBeInTheDocument();
    });
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
  });

  it("takes dropped and pasted files without uploading them, and uploads again once the page is unlocked", async () => {
    stubObjectUrls();
    const { user, memory, client } = await open();
    const editor = await openPage(user, "Fixed");
    const { page, space } = await idsOf(memory);
    const files = async () => (await client.attachmentList(space("Personal"))).map((info) => info.name);

    // The drop is taken (the webview must not open the file in place of the app) and nothing is stored or added.
    expect(dropFiles(editor.view.dom, [pngFile("locked.png")])).toBe(true);
    pasteFiles(editor.view.dom, [pngFile("pasted.png")]);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(await files()).toEqual([]);
    expect(editor.state.doc.childCount).toBe(1);

    await act(async () => {
      await client.pageSetAppearance(page("Fixed"), { locked: false });
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(true);
    });
    dropFiles(editor.view.dom, [pngFile("unlocked.png")]);
    await waitFor(async () => {
      expect(await files()).toEqual(["unlocked.png"]);
    });
  });

  it("flips live when another device locks and unlocks it, closing what was open to edit the page", async () => {
    const { user, client, memory, requests } = await open();
    const editor = await openPage(user, "Notes");
    const page = (await idsOf(memory)).page("Notes");
    // The rename field and the icon picker are open when the lock arrives.
    await user.click(within(screen.getByRole("heading", { level: 1 })).getByRole("button", { name: "Notes" }));
    expect(screen.getByRole("textbox", { name: t("tree.renameLabel") })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("page.addIcon") }));
    expect(await screen.findByRole("dialog", { name: t("icon.title") })).toBeInTheDocument();

    await act(async () => {
      await client.pageSetAppearance(page, { locked: true });
    });
    await waitFor(() => {
      expect(lockedStatus()).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: t("icon.title") })).not.toBeInTheDocument();
    expect(requests.filter((request) => request.type === "PageRename")).toEqual([]);
    expect(section()).toHaveClass("page-locked");

    await act(async () => {
      await client.pageSetAppearance(page, { locked: false });
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(true);
    });
    expect(lockedStatus()).not.toBeInTheDocument();
    // The rename field does not come back by itself.
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    expect(within(screen.getByRole("heading", { level: 1 })).getByRole("button", { name: "Notes" })).toBeInTheDocument();
  });

  it("closes the cover picker and the banner's buttons when the lock arrives", async () => {
    const { user, client, memory } = await open({
      spaces: [{ name: "Personal", pages: [{ title: "Covered", cover: "color:blue", content: paragraphs("x") }] }],
    });
    await openPage(user, "Covered");
    const page = (await idsOf(memory)).page("Covered");
    const banner = screen.getByRole("region", { name: t("cover.banner") });
    expect(within(banner).getByRole("button", { name: t("cover.change") })).toBeInTheDocument();
    await act(async () => {
      await client.pageSetAppearance(page, { locked: true });
    });
    await waitFor(() => {
      expect(lockedStatus()).toBeInTheDocument();
    });
    expect(within(screen.getByRole("region", { name: t("cover.banner") })).queryByRole("button")).not.toBeInTheDocument();
  });

  it("cannot replace in the find bar, and can again after unlocking", async () => {
    const { user, client, memory } = await open();
    await openPage(user, "Notes");
    const page = (await idsOf(memory)).page("Notes");
    act(() => {
      requestFindInPage();
    });
    expect(await screen.findByRole("search", { name: t("find.label") })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("find.toggleReplace") })).toBeInTheDocument();

    await act(async () => {
      await client.pageSetAppearance(page, { locked: true });
    });
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: t("find.toggleReplace") })).not.toBeInTheDocument();
    });
    // Finding still works on a locked page.
    expect(screen.getByRole("search", { name: t("find.label") })).toBeInTheDocument();

    await act(async () => {
      await client.pageSetAppearance(page, { locked: false });
    });
    expect(await screen.findByRole("button", { name: t("find.toggleReplace") })).toBeInTheDocument();
  });

  it("does not stop the structure: the sidebar, Duplicate (the copy is locked too) and moving the page work", async () => {
    const { user, client, memory, requests } = await open();
    await openPage(user, "Fixed");
    const menu = await openMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: t("page.duplicate") }));
    // The copy opens in the tab, locked like the original.
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Fixed (copy)");
    });
    expect(lockedStatus()).toBeInTheDocument();
    expect(requests.filter((request) => request.type === "PageDuplicate")).toHaveLength(1);
    const original = (await idsOf(memory)).page("Fixed");
    await client.pageRename(original, "Still renamable");
    await client.pageMove(original, (await idsOf(memory)).page("Plain"), 0);
    expect(await client.pageGet(original)).toMatchObject({ title: "Still renamable", locked: true });
    expect(row("Plain")).toBeInTheDocument();
  });

  it("makes \"Insert into page\" of the chat panel unavailable, with its reason, and available again after unlocking", async () => {
    const { user, client, memory } = await open({
      plain: true,
      ai: { mode: "fake" },
      settings: { [AI_SETTING]: AI_ON },
      spaces: [
        {
          name: "Personal",
          pages: [{ title: "Garden plans", content: paragraphs("Tomatoes need full sun and regular watering."), locked: true }],
        },
      ],
    });
    await openPage(user, "Garden plans");
    await user.click(screen.getByRole("button", { name: t("aiChat.toggle") }));
    const panel = await screen.findByRole("complementary", { name: t("aiChat.title") });
    await user.type(within(panel).getByRole("textbox", { name: t("aiChat.placeholder") }), "tomatoes{Enter}");
    const insert = await within(panel).findByRole("button", { name: t("aiChat.insert") });
    expect(insert).toBeDisabled();
    expect(insert).toHaveAttribute("title", t("aiChat.insertLocked"));

    await act(async () => {
      await client.pageSetAppearance((await idsOf(memory)).page("Garden plans"), { locked: false });
    });
    await waitFor(() => {
      expect(within(panel).getByRole("button", { name: t("aiChat.insert") })).toBeEnabled();
    });
  });
});

describe("print and the stylesheet", () => {
  it("keeps the font and the size of the page and leaves the lock indicator out", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { user } = await open();
    await openPage(user, "Serif");
    const menu = await openMenu(user);
    await user.click(within(menu).getByRole("menuitem", { name: t("interop.menu.print") }));
    await waitFor(() => {
      expect(print).toHaveBeenCalled();
    });
    // Print mode is on (the notice says so) and the page still carries its look.
    expect(screen.getByText(t("print.preparing"))).toBeInTheDocument();
    expect(section()).toHaveClass("page-font-serif", "page-small-text");
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
  });

  const CSS = readFileSync("src/styles/app.css", "utf8");

  it("draws the fonts from the system and scales the text through a variable, on the page's section only", () => {
    const serif = /--font-serif:\s*([^;]+);/u.exec(CSS)?.[1] ?? "";
    expect(serif).toMatch(/Georgia/u);
    expect(serif).toMatch(/Times New Roman/u);
    expect(serif.trim().endsWith("serif")).toBe(true);
    expect(CSS).not.toMatch(/@font-face/u);
    expect(CSS).toMatch(/\.page\.page-font-mono\s*\{\s*--page-font:\s*var\(--mono\);/u);
    expect(CSS).toMatch(/\.page\.page-font-serif\s*\{\s*--page-font:\s*var\(--font-serif\);/u);
    expect(CSS).toMatch(/\.page\.page-small-text\s*\{\s*--page-text-scale:\s*0\.875;/u);
    // Every rule that applies the font or the scale belongs to a page that has the class.
    const rules = [...CSS.matchAll(/([^{}]+)\{([^{}]*)\}/gu)].map((match) => ({ selector: (match[1] ?? "").trim(), body: match[2] ?? "" }));
    const users = rules.filter((rule) => /var\(--page-font\)|var\(--page-text-scale\)/u.test(rule.body));
    expect(users.length).toBeGreaterThan(3);
    for (const { selector } of users) {
      expect(selector, selector).toMatch(/^\.page\.page-(font-(serif|mono)|small-text)/u);
    }
  });

  it("prints the page as it looks: no print rule resets the font or the scale, and the lock indicator does not print", () => {
    const block = CSS.slice(CSS.lastIndexOf("@media print"));
    expect(block).not.toMatch(/page-font|page-small-text|--page-font|--page-text-scale/u);
    expect(block).toContain(".page-lock");
    expect(block).toMatch(/\.page-lock[\s\S]*display:\s*none\s*!important/u);
  });
});
