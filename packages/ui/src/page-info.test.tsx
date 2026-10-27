// "Page info" in the page menu (ADR-0040): when the page was created and last edited and by which device, the
// words, characters and top-level blocks of the page as the editor holds it, and where the page is.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageInfo, SpaceInfo } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PageInfoPanel } from "./components/PageInfoPanel";
import { t } from "./i18n";
import { formatInstant } from "./property-format";
import { createAppStore } from "./store";
import { StoreContext } from "./store-context";
import { paragraphs } from "./test/editor";
import { openPage, renderPages } from "./test/page";
import { NOW } from "./test/render";
import type { User } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

/** The value (`dd`) that goes with the label (`dt`) in the dialog. */
function valueOf(dialog: HTMLElement, label: string): HTMLElement {
  const term = within(dialog).getByText(label, { selector: "dt" });
  const value = term.nextElementSibling;
  if (!(value instanceof HTMLElement)) {
    throw new Error(`no value for ${label}`);
  }
  return value;
}

async function openPageInfo(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  await user.click(await screen.findByRole("menuitem", { name: t("page.info") }));
  return screen.findByRole("dialog", { name: t("pageInfo.title") });
}

const SPACES = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: paragraphs("Ship it", "soon") },
      { title: "Projects", children: [{ title: "atomik-kb", children: [{ title: "Roadmap", content: paragraphs("One two three") }] }] },
    ],
  },
];

describe("Page info in the page menu", () => {
  it("shows when the page was made and last edited, by which device, and what it holds", async () => {
    const { user } = await renderPages(SPACES, { deviceName: "Studio PC" });
    await openPage(user, "Notes");
    const dialog = await openPageInfo(user);

    for (const label of [t("pageInfo.created"), t("pageInfo.edited")]) {
      expect(valueOf(dialog, label)).toHaveTextContent(formatInstant(NOW));
      expect(valueOf(dialog, label)).toHaveTextContent(t("pageInfo.by", { device: "Studio PC" }));
    }
    // "Ship it" and "soon": three words, eleven characters, two blocks.
    expect(valueOf(dialog, t("pageInfo.words"))).toHaveTextContent(/^3$/);
    expect(valueOf(dialog, t("pageInfo.characters"))).toHaveTextContent(/^11$/);
    expect(valueOf(dialog, t("pageInfo.blocks"))).toHaveTextContent(/^2$/);
    // A top-level page is in its space.
    expect(valueOf(dialog, t("pageInfo.location"))).toHaveTextContent(/^Personal$/);
    // The way the counts are made is said where they are.
    expect(within(dialog).getByText(t("pageInfo.blocks"), { selector: "dt" })).toHaveAttribute("title", t("pageInfo.blocks.hint"));
    expect(within(dialog).getByText(t("pageInfo.words"), { selector: "dt" })).toHaveAttribute("title", t("pageInfo.words.hint"));
    expect(within(dialog).getByText(t("pageInfo.characters"), { selector: "dt" })).toHaveAttribute("title", t("pageInfo.characters.hint"));
  });

  it("gives the location of a nested page as its space and the pages above it", async () => {
    const { user } = await renderPages(SPACES);
    await openPage(user, "Roadmap");
    const dialog = await openPageInfo(user);
    expect(valueOf(dialog, t("pageInfo.location"))).toHaveTextContent("Personal › Projects › atomik-kb");
    expect(valueOf(dialog, t("pageInfo.words"))).toHaveTextContent(/^3$/);
  });

  it("follows the text while it is open", async () => {
    const { user } = await renderPages(SPACES);
    const editor = await openPage(user, "Notes");
    const dialog = await openPageInfo(user);
    expect(valueOf(dialog, t("pageInfo.words"))).toHaveTextContent(/^3$/);
    act(() => {
      insert(editor, "and then some more words");
    });
    await waitFor(() => {
      expect(valueOf(dialog, t("pageInfo.words"))).toHaveTextContent(/^8$/);
    });
    expect(valueOf(dialog, t("pageInfo.blocks"))).toHaveTextContent(/^3$/);
    expect(valueOf(dialog, t("pageInfo.characters"))).toHaveTextContent(
      String("Ship it".length + "soon".length + "and then some more words".length),
    );
  });

  it("closes on Escape and gives focus back to the page menu's button", async () => {
    const { user } = await renderPages(SPACES);
    await openPage(user, "Notes");
    const opener = screen.getByRole("button", { name: t("page.actions") });
    const dialog = await openPageInfo(user);
    await user.keyboard("{Escape}");
    expect(dialog).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
  });
});

/** Appends a paragraph with `value` to the editor's document. */
function insert(editor: Editor, value: string): void {
  editor.commands.insertContentAt(editor.state.doc.content.size, { type: "paragraph", content: [{ type: "text", text: value }] });
}

// ----- the panel on its own: what the page does not say ---------------------------------------------

const DEVICE = "d0000000-0000-4000-8000-000000000001";
const OTHER = "d0000000-0000-4000-8000-000000000002";
const SPACE = "a0000000-0000-4000-8000-000000000001";

function pageInfo(extra: Partial<PageInfo> = {}): PageInfo {
  return { id: "b0000000-0000-4000-8000-000000000001", spaceId: SPACE, title: "Old page", createdAt: NOW, updatedAt: NOW + 60_000, ...extra };
}

/** A panel over a store that knows one space and one device, with no editor. */
function renderPanel(page: PageInfo, devices: { deviceId: string; name: string; revoked?: boolean }[] = []) {
  const store = createAppStore(new CoreClient(new MemoryTransport({ now: () => NOW })));
  const space: SpaceInfo = { id: SPACE, name: "Personal", createdAt: NOW, pageCount: 1 };
  store.setState({
    spaces: [space],
    tree: { spaceId: SPACE, roots: [{ page, children: [] }], trashed: [], undo: { canUndo: false, canRedo: false } },
    devices: devices.map((device, index) => ({
      deviceId: device.deviceId,
      name: device.name,
      addedAt: NOW,
      addedViaKit: false,
      isThisDevice: index === 0,
      signingKey: "00",
      ...(device.revoked === true ? { revokedAt: NOW } : {}),
    })),
  });
  const onClose = vi.fn();
  const opener = document.createElement("button");
  document.body.append(opener);
  render(
    <StoreContext.Provider value={store}>
      <PageInfoPanel page={page} editor={null} anchor={{ x: 10, y: 10 }} anchorElement={opener} onClose={onClose} />
    </StoreContext.Provider>,
  );
  return { dialog: screen.getByRole("dialog", { name: t("pageInfo.title") }), onClose };
}

describe("the Page info panel", () => {
  it("says Unknown for a device the page did not record, whatever its times", () => {
    const { dialog } = renderPanel(pageInfo());
    expect(valueOf(dialog, t("pageInfo.created"))).toHaveTextContent(formatInstant(NOW));
    expect(valueOf(dialog, t("pageInfo.created"))).toHaveTextContent(t("pageInfo.by", { device: t("properties.computed.unknown") }));
    expect(valueOf(dialog, t("pageInfo.edited"))).toHaveTextContent(formatInstant(NOW + 60_000));
    expect(valueOf(dialog, t("pageInfo.edited"))).toHaveTextContent(t("pageInfo.by", { device: t("properties.computed.unknown") }));
  });

  it("names the devices from the device log, and a device it does not list or that was removed as unknown", () => {
    const { dialog } = renderPanel(pageInfo({ createdBy: DEVICE, editedBy: OTHER }), [
      { deviceId: DEVICE, name: "Kitchen tablet" },
      { deviceId: OTHER, name: "Old laptop", revoked: true },
    ]);
    expect(valueOf(dialog, t("pageInfo.created"))).toHaveTextContent(t("pageInfo.by", { device: "Kitchen tablet" }));
    expect(valueOf(dialog, t("pageInfo.edited"))).toHaveTextContent(t("pageInfo.by", { device: t("properties.computed.unknownDevice") }));
  });

  it("shows no counts without an editor (the page is still opening), and says why", () => {
    const { dialog } = renderPanel(pageInfo());
    for (const label of [t("pageInfo.words"), t("pageInfo.characters"), t("pageInfo.blocks")]) {
      expect(valueOf(dialog, label)).toHaveTextContent("–");
      expect(valueOf(dialog, label)).toHaveAttribute("title", t("pageInfo.counts.unavailable"));
    }
  });

  it("closes on Escape", async () => {
    const { onClose } = renderPanel(pageInfo());
    await userEvent.setup().keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});
