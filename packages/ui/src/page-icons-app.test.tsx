// Page and space icons in the whole app (ADR-0029, section 1): an icon of the set or an uploaded
// picture chosen in the picker is stored by the core and shown by every place that shows the icon
// of a page (the tree and the title here; the other places share the component).

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { AttachmentTransport } from "@atomik-kb/core-client";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { pngFile, stubObjectUrls } from "./test/attachments";
import { idsOf } from "./test/page";
import { NOW, findMainNavigation, recordingTransport, renderWith } from "./test/render";
import type { User } from "./test/render";
import { renderSidebar, row } from "./test/sidebar";

afterEach(() => {
  vi.restoreAllMocks();
});

/** The icon the core holds for a page, by title. */
async function iconOf(memory: MemoryTransport, title: string): Promise<string | undefined> {
  const ids = await idsOf(memory);
  return (await new CoreClient(memory).pageGet(ids.page(title))).icon;
}

async function openPicker(user: User, tab: "icons" | "upload"): Promise<HTMLElement> {
  await user.click(await screen.findByRole("button", { name: /^(Add|Change) icon$/ }));
  const picker = await screen.findByRole("dialog", { name: t("icon.title") });
  await user.click(within(picker).getByRole("tab", { name: t(tab === "icons" ? "icon.tabs.icons" : "icon.tabs.upload") }));
  return picker;
}

describe("an icon of the set", () => {
  it("is chosen in the picker, stored, shown in the tree and the title, recoloured and removed", async () => {
    const { user, memory } = await renderSidebar();
    await user.click(row("Journal"));
    let picker = await openPicker(user, "icons");
    await user.type(within(picker).getByRole("searchbox", { name: t("icon.set.search") }), "rocket");
    await user.click(within(picker).getByRole("button", { name: "rocket" }));

    await waitFor(() => {
      expect(row("Journal").querySelector(".page-icon svg.lucide-rocket")).not.toBeNull();
    });
    expect(await iconOf(memory, "Journal")).toBe("icon:rocket");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    const title = screen.getByRole("heading", { level: 1 });
    expect(title.querySelector(".page-title-icon svg.lucide-rocket")).not.toBeNull();
    expect(title).not.toHaveTextContent("icon:");

    // The picker opens on the Icons tab with the icon marked; a colour and the same icon again.
    await user.click(screen.getByRole("button", { name: t("page.changeIcon") }));
    picker = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(picker).getByRole("tab", { selected: true })).toHaveTextContent(t("icon.tabs.icons"));
    await user.click(within(picker).getByRole("button", { name: t("callout.color.purple") }));
    await user.click(within(picker).getByRole("button", { name: "rocket" }));
    await waitFor(() => {
      expect(title.querySelector("svg.lucide-rocket")).toHaveClass("page-icon-color-purple");
    });
    expect(await iconOf(memory, "Journal")).toBe("icon:rocket:purple");
    expect(row("Journal").querySelector("svg.lucide-rocket")).toHaveClass("page-icon-color-purple");

    await user.click(screen.getByRole("button", { name: t("page.changeIcon") }));
    await user.click(await screen.findByRole("button", { name: t("icon.remove") }));
    await waitFor(() => {
      expect(row("Journal").querySelector("svg.lucide-rocket")).toBeNull();
    });
    expect(await iconOf(memory, "Journal")).toBeUndefined();
    expect(row("Journal").querySelector(".page-icon")).toHaveAttribute("data-icon-kind", "default");
  });

  it("shows the default glyph, not the text, for a name this version does not have", async () => {
    const { memory } = await renderSidebar({
      spaces: [{ name: "Personal", pages: [{ title: "Future", icon: "icon:from-a-newer-set:blue" }, { title: "Plain" }] }],
    });
    expect(await iconOf(memory, "Future")).toBe("icon:from-a-newer-set:blue");
    const future = row("Future");
    expect(future.querySelector(".page-icon")).toHaveAttribute("data-icon-kind", "default");
    expect(future).not.toHaveTextContent("icon:");
    expect(future.querySelector(".page-icon svg")).not.toBeNull();
    expect(future.querySelector(".page-icon")?.innerHTML).toBe(row("Plain").querySelector(".page-icon")?.innerHTML);
  });

  it("is chosen for a space too, which has no Upload tab", async () => {
    const { user, memory } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    await user.click(await screen.findByRole("menuitem", { name: t("spaces.changeIcon") }));
    const picker = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(picker).queryByRole("tab", { name: t("icon.tabs.upload") })).not.toBeInTheDocument();
    await user.click(within(picker).getByRole("tab", { name: t("icon.tabs.icons") }));
    await user.click(within(picker).getByRole("button", { name: "briefcase" }));

    const switcher = screen.getByRole("button", { description: t("spaces.switchTitle") });
    await waitFor(() => {
      expect(switcher.querySelector("svg.lucide-briefcase")).not.toBeNull();
    });
    const [personal] = await new CoreClient(memory).spaceList();
    expect(personal?.icon).toBe("icon:briefcase");
  });
});

describe("an uploaded picture", () => {
  it("is scaled, stored as an attachment of the page's space, and shown from one blob URL by the tree and the title", async () => {
    const urls = stubObjectUrls();
    // Without a canvas the picker uploads the chosen picture as it is.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => null);
    const memory = new MemoryTransport({
      now: () => NOW,
      workspaceName: "Research",
      spaces: [{ name: "Personal", pages: [{ title: "Journal" }, { title: "Other" }] }],
    });
    const recording = recordingTransport(memory);
    const read = vi.spyOn(recording.transport.attachments as AttachmentTransport, "read");
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    await user.click(row("Journal"));

    const picker = await openPicker(user, "upload");
    const input = picker.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input).not.toBeNull();
    fireEvent.change(input as HTMLInputElement, { target: { files: [pngFile("me.png")] } });

    await waitFor(() => {
      expect(row("Journal").querySelector("img.page-icon-image")).toHaveAttribute("src", "blob:test/1");
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    const icon = await iconOf(memory, "Journal");
    expect(icon).toMatch(/^attachment:[0-9a-f-]{36}$/);
    const info = await new CoreClient(memory).attachmentGet(icon?.slice("attachment:".length) ?? "");
    expect(info).toMatchObject({ name: "me.png", mime: "image/png" });
    expect(info.spaceId).toBe((await new CoreClient(memory).pageGet((await idsOf(memory)).page("Journal"))).spaceId);

    const title = screen.getByRole("heading", { level: 1 });
    expect(title.querySelector("img.page-icon-image")).toHaveAttribute("src", "blob:test/1");
    expect(row("Other").querySelector("img")).toBeNull();
    // One read and one URL for the tree and the title together.
    expect(read).toHaveBeenCalledTimes(1);
    expect(urls.create).toHaveBeenCalledTimes(1);

    // The picker now opens on the Upload tab with the current picture shown.
    await user.click(screen.getByRole("button", { name: t("page.changeIcon") }));
    const again = await screen.findByRole("dialog", { name: t("icon.title") });
    expect(within(again).getByRole("tab", { selected: true })).toHaveTextContent(t("icon.tabs.upload"));
    expect(within(again).getByText(t("icon.upload.current"))).toBeInTheDocument();
    // Another icon replaces it; the picture is let go.
    await user.click(within(again).getByRole("tab", { name: t("icon.tabs.emoji") }));
    await user.click(within(again).getByRole("button", { name: "💡" }));
    await waitFor(() => {
      expect(urls.revoke).toHaveBeenCalledWith("blob:test/1");
    });
    expect(await iconOf(memory, "Journal")).toBe("💡");
    expect(row("Journal").querySelector("img")).toBeNull();
  });

  it("shows the default glyph for a picture that does not exist (a purged attachment)", async () => {
    stubObjectUrls();
    const gone = "a0000000-0000-4000-8000-0000000000ff";
    const { memory } = await renderSidebar({
      spaces: [{ name: "Personal", pages: [{ title: "Orphan", icon: `attachment:${gone}` }] }],
    });
    expect((await iconOf(memory, "Orphan"))?.startsWith("attachment:")).toBe(true);
    await waitFor(() => {
      expect(row("Orphan")).toBeInTheDocument();
    });
    expect(row("Orphan").querySelector(".page-icon")).toHaveAttribute("data-icon-kind", "default");
    expect(row("Orphan")).not.toHaveTextContent("attachment:");
  });
});
