// Page covers in the app (ADR-0040): "Add cover" on the header of a page without one, the picker (Gallery,
// Upload, Link; keyboard accessible), the banner of a page that has one (a colour, a gradient or a picture
// of the page's space, placed by its position), "Change cover", "Reposition" (drag or arrow keys, one write on
// "Save position"), "Remove", a banner that cannot be pressed (read-only, print), and the grammar and the picture
// of an uploaded cover.

import { CoreClient, CoreError, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { RemoteImage, Request, SeedSpace, Transport } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PageCover } from "./components/PageCover";
import { COVER_GRADIENTS, coverPosition, isBundledGradient, parseCover } from "./icons/cover-grammar";
import { MAX_COVER_SIDE, coverName, downscaleCover } from "./icons/resize-image";
import { t } from "./i18n";
import { createAppStore } from "./store";
import { StoreContext } from "./store-context";
import { pngFile, stubObjectUrls, textFile } from "./test/attachments";
import { idsOf } from "./test/page";
import { NOW, findMainNavigation, recordingTransport, renderWith } from "./test/render";
import { renderInterop } from "./test/interop";
import { row } from "./test/sidebar";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete document.documentElement.dataset.theme;
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const DEMO_URL = "https://example.com/photos/banner.png";

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Journal" },
      { title: "Sunny", cover: "gradient:sunset" },
      { title: "Photo", cover: `attachment:${CAT}`, coverY: 30 },
      { title: "Future", cover: "gradient:from-a-newer-version" },
      { title: "Plain" },
    ],
  },
];

function newMemory(): MemoryTransport {
  return new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: SPACES,
    attachments: [{ attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() }],
    remoteImages: new Map<string, RemoteImage>([
      [DEMO_URL, { bytes: demoImagePng(), contentType: "image/png" }],
      ["https://example.com/page.html", { bytes: new TextEncoder().encode("<html>no picture</html>".repeat(4)), contentType: "text/html" }],
    ]),
  });
}

/** The app over the memory core, with the requests it sends recorded. */
async function open(options: { transport?: (memory: MemoryTransport) => Transport } = {}) {
  const memory = newMemory();
  const recording = recordingTransport(memory);
  const { user } = renderWith(options.transport?.(memory) ?? recording.transport);
  await findMainNavigation();
  return { memory, user, requests: recording.requests };
}

async function coverOf(memory: MemoryTransport, title: string) {
  const ids = await idsOf(memory);
  const page = await new CoreClient(memory).pageGet(ids.page(title));
  return { cover: page.cover, coverY: page.coverY };
}

const banner = (): HTMLElement => screen.getByRole("region", { name: t("cover.banner") });
const noBanner = (): void => {
  expect(screen.queryByRole("region", { name: t("cover.banner") })).not.toBeInTheDocument();
};

const setCovers = (requests: readonly Request[]) => requests.filter((request) => request.type === "PageSetCover");

describe("the grammar of a cover", () => {
  it("reads the three forms and nothing else", () => {
    expect(parseCover(`attachment:${CAT}`)).toEqual({ kind: "image", attachmentId: CAT });
    expect(parseCover(" color:blue ")).toEqual({ kind: "color", color: "blue" });
    expect(parseCover("gradient:sunset")).toEqual({ kind: "gradient", name: "sunset" });
    // A gradient of a newer version is well formed; its name is not in this version's list.
    expect(parseCover("gradient:from-a-newer-version")).toEqual({ kind: "gradient", name: "from-a-newer-version" });
    expect(isBundledGradient("from-a-newer-version")).toBe(false);
    expect(COVER_GRADIENTS.every(isBundledGradient)).toBe(true);
    for (const bad of [undefined, null, "", "  ", "🌱", "icon:book", "color:teal", "color:", "gradient:", "gradient:Sun", "gradient:a_b", "attachment:nope", `gradient:${"x".repeat(33)}`, "https://example.com/a.png"]) {
      expect(parseCover(bad), String(bad)).toBeNull();
    }
  });

  it("reads the position as a whole number from 0 to 100, 50 when there is none", () => {
    expect(coverPosition(undefined)).toBe(50);
    expect(coverPosition(0)).toBe(0);
    expect(coverPosition(33.4)).toBe(33);
    expect(coverPosition(250)).toBe(100);
    expect(coverPosition(-5)).toBe(0);
    expect(coverPosition(Number.NaN)).toBe(50);
  });
});

describe("Add cover and the picker", () => {
  it("is on the header of a page without a cover, opens the gallery, and a colour becomes the banner", async () => {
    const { user, memory } = await open();
    await user.click(row("Journal"));
    expect(screen.queryByRole("region", { name: t("cover.banner") })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    expect(within(picker).getAllByRole("tab").map((tab) => tab.textContent)).toEqual([t("cover.tabs.gallery"), t("cover.tabs.upload"), t("cover.tabs.link")]);
    expect(within(picker).getByRole("tab", { selected: true })).toHaveTextContent(t("cover.tabs.gallery"));
    // The nine colours and the bundled gradients, each a button with its name.
    for (const name of ["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"] as const) {
      expect(within(picker).getByRole("button", { name: t(`callout.color.${name}`) })).toBeInTheDocument();
    }
    for (const name of COVER_GRADIENTS) {
      expect(within(picker).getByRole("button", { name: t(`cover.gradient.${name}`) })).toBeInTheDocument();
    }

    await user.click(within(picker).getByRole("button", { name: t("callout.color.blue") }));
    await waitFor(() => {
      expect(banner()).toHaveAttribute("data-cover-color", "blue");
    });
    expect(screen.queryByRole("dialog", { name: t("cover.title") })).not.toBeInTheDocument();
    expect(await coverOf(memory, "Journal")).toEqual({ cover: "color:blue", coverY: undefined });
    // The header no longer offers "Add cover"; the banner has the buttons.
    expect(screen.queryByRole("button", { name: t("page.addCover") })).not.toBeInTheDocument();
    for (const name of ["cover.change", "cover.remove"] as const) {
      expect(within(banner()).getByRole("button", { name: t(name) })).toBeInTheDocument();
    }
    // A colour or a gradient has no picture to reposition.
    expect(within(banner()).queryByRole("button", { name: t("cover.reposition") })).not.toBeInTheDocument();
  });

  it("changes the cover from the banner, marks the current one, and removes it", async () => {
    const { user, memory } = await open();
    await user.click(row("Sunny"));
    expect(banner()).toHaveAttribute("data-cover-gradient", "sunset");

    await user.click(within(banner()).getByRole("button", { name: t("cover.change") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    expect(within(picker).getByRole("button", { name: t("cover.gradient.sunset") })).toHaveAttribute("aria-pressed", "true");
    expect(within(picker).getByRole("button", { name: t("cover.gradient.ocean") })).toHaveAttribute("aria-pressed", "false");
    await user.click(within(picker).getByRole("button", { name: t("cover.gradient.ocean") }));
    await waitFor(() => {
      expect(banner()).toHaveAttribute("data-cover-gradient", "ocean");
    });
    expect(await coverOf(memory, "Sunny")).toEqual({ cover: "gradient:ocean", coverY: undefined });

    await user.click(within(banner()).getByRole("button", { name: t("cover.remove") }));
    await waitFor(noBanner);
    expect(await coverOf(memory, "Sunny")).toEqual({ cover: undefined, coverY: undefined });
    expect(screen.getByRole("button", { name: t("page.addCover") })).toBeInTheDocument();
  });

  it("is reached and used with the keyboard", async () => {
    const { user, memory } = await open();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });

    // The arrow keys move between the tabs.
    const gallery = within(picker).getByRole("tab", { name: t("cover.tabs.gallery") });
    gallery.focus();
    await user.keyboard("{ArrowRight}");
    expect(within(picker).getByRole("tab", { selected: true })).toHaveTextContent(t("cover.tabs.upload"));
    await user.keyboard("{End}");
    expect(within(picker).getByRole("tab", { selected: true })).toHaveTextContent(t("cover.tabs.link"));
    await user.keyboard("{Home}");
    expect(within(picker).getByRole("tab", { selected: true })).toHaveTextContent(t("cover.tabs.gallery"));

    // The swatches are one tab stop, and the arrow keys move between them (Down by a row of five).
    const swatches = within(picker).getAllByRole("button").filter((button) => button.classList.contains("cover-swatch"));
    expect(swatches.map((swatch) => swatch.tabIndex)).toEqual(swatches.map((_, at) => (at === 0 ? 0 : -1)));
    swatches[0]?.focus();
    await user.keyboard("{ArrowRight}");
    expect(swatches[1]).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(swatches[6]).toHaveFocus();
    await user.keyboard("{ArrowLeft}{ArrowUp}");
    expect(swatches[0]).toHaveFocus();
    await user.keyboard("{End}");
    expect(swatches[swatches.length - 1]).toHaveFocus();

    // Enter takes the swatch that has focus (the last one: Aurora).
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(banner()).toHaveAttribute("data-cover-gradient", "aurora");
    });
    expect(await coverOf(memory, "Journal")).toMatchObject({ cover: "gradient:aurora" });

    // Escape closes the picker and leaves the cover as it is; focus returns to the button that opened it.
    await user.click(within(banner()).getByRole("button", { name: t("cover.change") }));
    await screen.findByRole("dialog", { name: t("cover.title") });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("cover.title") })).not.toBeInTheDocument();
    expect(await coverOf(memory, "Journal")).toMatchObject({ cover: "gradient:aurora" });
  });

  it("starts the Tab stop of the gallery at the current cover", async () => {
    const { user } = await open();
    await user.click(row("Sunny"));
    await user.click(within(banner()).getByRole("button", { name: t("cover.change") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    const stops = within(picker).getAllByRole("button").filter((button) => button.classList.contains("cover-swatch") && button.tabIndex === 0);
    expect(stops.map((stop) => stop.getAttribute("aria-label"))).toEqual([t("cover.gradient.sunset")]);
  });
});

describe("the Upload tab", () => {
  it("stores a picture as an attachment of the page's space and makes it the cover", async () => {
    const urls = stubObjectUrls();
    // Without a canvas the picture is uploaded as it is.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => null);
    const { user, memory } = await open();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    await user.click(within(picker).getByRole("tab", { name: t("cover.tabs.upload") }));
    expect(within(picker).getByText(t("cover.upload.hint"))).toBeInTheDocument();
    const input = picker.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input).not.toBeNull();
    expect(input).toHaveAttribute("accept", "image/*");
    fireEvent.change(input as HTMLInputElement, { target: { files: [pngFile("sunrise.png")] } });

    await waitFor(() => {
      expect(banner().querySelector("img.page-cover-image")).not.toBeNull();
    });
    const { cover, coverY } = await coverOf(memory, "Journal");
    expect(cover).toMatch(/^attachment:/u);
    expect(coverY).toBeUndefined();
    const client = new CoreClient(memory);
    const info = await client.attachmentGet((cover ?? "").slice("attachment:".length));
    expect(info).toMatchObject({ name: "sunrise.png", mime: "image/png", references: 1 });
    const ids = await idsOf(memory);
    expect(info.spaceId).toBe(ids.space("Personal"));
    // Shown from one blob URL, placed at the default position.
    expect(urls.create).toHaveBeenCalled();
    expect(banner().querySelector("img.page-cover-image")).toHaveStyle({ objectPosition: "50% 50%" });
  });

  it("refuses what is not an image and leaves the cover as it was", async () => {
    const { user, memory } = await open();
    await user.click(row("Sunny"));
    await user.click(within(banner()).getByRole("button", { name: t("cover.change") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    await user.click(within(picker).getByRole("tab", { name: t("cover.tabs.upload") }));
    const input = picker.querySelector<HTMLInputElement>('input[type="file"]');
    fireEvent.change(input as HTMLInputElement, { target: { files: [textFile("notes.txt")] } });
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("icon.upload.error.notImage"));
    expect(await coverOf(memory, "Sunny")).toMatchObject({ cover: "gradient:sunset" });
  });

  it("is not offered by a shell that cannot store attachments", async () => {
    const memory = newMemory();
    const { user } = renderWith(recordingTransport(memory, { attachments: false }).transport);
    await findMainNavigation();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    // The gallery alone: no tabs at all.
    expect(within(picker).queryByRole("tablist")).not.toBeInTheDocument();
    expect(within(picker).getByRole("button", { name: t("callout.color.red") })).toBeInTheDocument();
  });
});

describe("the Link tab", () => {
  it("downloads the picture once through the shell and makes it the cover", async () => {
    const urls = stubObjectUrls();
    const { user, memory } = await open();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    await user.click(within(picker).getByRole("tab", { name: t("cover.tabs.link") }));
    const field = within(picker).getByRole("textbox", { name: t("attachment.url.label") });
    await user.type(field, DEMO_URL);
    await user.click(within(picker).getByRole("button", { name: t("cover.link.use") }));

    await waitFor(() => {
      expect(banner().querySelector("img.page-cover-image")).not.toBeNull();
    });
    const { cover } = await coverOf(memory, "Journal");
    expect(cover).toMatch(/^attachment:/u);
    const client = new CoreClient(memory);
    const info = await client.attachmentGet((cover ?? "").slice("attachment:".length));
    expect(info).toMatchObject({ name: "banner.png", mime: "image/png", references: 1 });
    // The page holds the attachment, never the link.
    expect(cover).not.toContain("https");
    expect(urls.create).toHaveBeenCalled();
  });

  it("checks the link as it is typed and says why a download failed", async () => {
    const { user, memory } = await open();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    await user.click(within(picker).getByRole("tab", { name: t("cover.tabs.link") }));
    const field = within(picker).getByRole("textbox", { name: t("attachment.url.label") });
    expect(within(picker).getByRole("button", { name: t("cover.link.use") })).toBeDisabled();

    await user.type(field, "http://example.com/a.png");
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("attachment.url.error.scheme"));
    await user.clear(field);
    await user.type(field, "https://example.com/page.html");
    await user.click(within(picker).getByRole("button", { name: t("cover.link.use") }));
    expect(await within(picker).findByRole("alert")).toHaveTextContent(t("attachment.url.error.notImage"));
    expect(await coverOf(memory, "Journal")).toEqual({ cover: undefined, coverY: undefined });
    expect(screen.getByRole("dialog", { name: t("cover.title") })).toBeInTheDocument();
  });

  it("is absent where the shell cannot download a link", async () => {
    const memory = newMemory();
    const files = memory.attachments;
    const withoutImportUrl: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      attachments: {
        write: (attachmentId, bytes) => files.write(attachmentId, bytes),
        read: (attachmentId, range) => files.read(attachmentId, range),
        importPath: (spaceId, path) => files.importPath(spaceId, path),
        saveAs: (attachmentId, path) => files.saveAs(attachmentId, path),
        open: (attachmentId) => files.open(attachmentId),
      },
    };
    const { user } = renderWith(withoutImportUrl);
    await findMainNavigation();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("page.addCover") }));
    const picker = await screen.findByRole("dialog", { name: t("cover.title") });
    expect(within(picker).getAllByRole("tab").map((tab) => tab.textContent)).toEqual([t("cover.tabs.gallery"), t("cover.tabs.upload")]);
  });
});

describe("the banner", () => {
  it("draws a picture of the page's space at the stored position", async () => {
    stubObjectUrls();
    const { user } = await open();
    await user.click(row("Photo"));
    await waitFor(() => {
      expect(banner().querySelector("img.page-cover-image")).not.toBeNull();
    });
    const image = banner().querySelector("img.page-cover-image");
    expect(image).toHaveAttribute("src", expect.stringMatching(/^blob:/u));
    expect(image).toHaveStyle({ objectPosition: "50% 30%" });
    expect(banner()).toHaveAttribute("data-cover-kind", "image");
    // A picture can be repositioned.
    expect(within(banner()).getByRole("button", { name: t("cover.reposition") })).toBeInTheDocument();
  });

  it("is a neutral banner, never the raw text, for a gradient this version does not bundle", async () => {
    const { user } = await open();
    await user.click(row("Future"));
    expect(banner()).toHaveAttribute("data-cover-kind", "gradient");
    expect(banner()).not.toHaveAttribute("data-cover-gradient");
    expect(banner()).not.toHaveTextContent("from-a-newer-version");
  });

  it("is above the header, and there is none on a page without a cover", async () => {
    const { user } = await open();
    await user.click(row("Plain"));
    noBanner();
    await user.click(row("Sunny"));
    const section = banner().closest("section.page");
    expect(section).not.toBeNull();
    expect(section?.firstElementChild).toBe(banner());
    expect(section?.querySelector("header.page-header")).not.toBeNull();
  });
});

describe("Reposition", () => {
  async function openPhoto() {
    const urls = stubObjectUrls();
    const context = await open();
    await context.user.click(row("Photo"));
    await waitFor(() => {
      expect(banner().querySelector("img.page-cover-image")).not.toBeNull();
    });
    await context.user.click(within(banner()).getByRole("button", { name: t("cover.reposition") }));
    return { ...context, urls, slider: within(banner()).getByRole("slider", { name: t("cover.position") }) };
  }

  /** A pointer event with a position (jsdom has no PointerEvent; React listens to the event's name). */
  function pointer(target: Element, type: "pointerdown" | "pointermove" | "pointerup", clientY: number): void {
    fireEvent(target, new MouseEvent(type, { bubbles: true, cancelable: true, clientY, button: 0 }));
  }

  it("shows the hint, a slider at the stored position and Save position and Cancel instead of the buttons", async () => {
    const { slider } = await openPhoto();
    expect(slider).toHaveAttribute("aria-valuemin", "0");
    expect(slider).toHaveAttribute("aria-valuemax", "100");
    expect(slider).toHaveAttribute("aria-valuenow", "30");
    expect(slider).toHaveFocus();
    expect(slider).toHaveAccessibleDescription(t("cover.positionHint"));
    const actions = within(banner());
    expect(actions.getByRole("button", { name: t("cover.savePosition") })).toBeInTheDocument();
    expect(actions.getByRole("button", { name: t("cover.cancelPosition") })).toBeInTheDocument();
    for (const name of ["cover.change", "cover.reposition", "cover.remove"] as const) {
      expect(actions.queryByRole("button", { name: t(name) })).not.toBeInTheDocument();
    }
  });

  it("moves the picture with the arrow keys and saves once", async () => {
    const { user, memory, requests, slider } = await openPhoto();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(slider).toHaveAttribute("aria-valuenow", "40");
    expect(banner().querySelector("img.page-cover-image")).toHaveStyle({ objectPosition: "50% 40%" });
    await user.keyboard("{PageUp}");
    expect(slider).toHaveAttribute("aria-valuenow", "20");
    await user.keyboard("{Home}");
    expect(slider).toHaveAttribute("aria-valuenow", "0");
    await user.keyboard("{ArrowUp}");
    expect(slider).toHaveAttribute("aria-valuenow", "0");
    await user.keyboard("{End}{PageDown}");
    expect(slider).toHaveAttribute("aria-valuenow", "100");
    await user.keyboard("{PageUp}{ArrowUp}");
    expect(slider).toHaveAttribute("aria-valuenow", "75");
    // Nothing was written while moving.
    expect(setCovers(requests)).toHaveLength(0);

    await user.click(within(banner()).getByRole("button", { name: t("cover.savePosition") }));
    await waitFor(() => {
      expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    });
    expect(setCovers(requests)).toHaveLength(1);
    expect(setCovers(requests)[0]).toMatchObject({ data: { coverY: 75 } });
    expect(setCovers(requests)[0]?.type === "PageSetCover" && setCovers(requests)[0]).not.toHaveProperty("data.cover");
    expect(await coverOf(memory, "Photo")).toMatchObject({ cover: `attachment:${CAT}`, coverY: 75 });
    // The buttons are back.
    expect(within(banner()).getByRole("button", { name: t("cover.reposition") })).toBeInTheDocument();
  });

  it("moves the picture by dragging it up and down", async () => {
    const { user, requests, slider } = await openPhoto();
    // Dragging down shows more of the top of the picture: the position goes down; 200 px sweep the whole range
    // when the layout cannot be measured.
    pointer(slider, "pointerdown", 100);
    pointer(slider, "pointermove", 120);
    expect(slider).toHaveAttribute("aria-valuenow", "20");
    pointer(slider, "pointermove", 60);
    expect(slider).toHaveAttribute("aria-valuenow", "50");
    pointer(slider, "pointermove", -500);
    expect(slider).toHaveAttribute("aria-valuenow", "100");
    pointer(slider, "pointermove", 900);
    expect(slider).toHaveAttribute("aria-valuenow", "0");
    pointer(slider, "pointerup", 900);
    // Moving the pointer after the release does nothing.
    pointer(slider, "pointermove", 100);
    expect(slider).toHaveAttribute("aria-valuenow", "0");
    expect(setCovers(requests)).toHaveLength(0);
    await user.click(within(banner()).getByRole("button", { name: t("cover.savePosition") }));
    await waitFor(() => {
      expect(setCovers(requests)).toHaveLength(1);
    });
    expect(setCovers(requests)[0]).toMatchObject({ data: { coverY: 0 } });
  });

  it("writes nothing when cancelled, on Escape, or when the position did not change", async () => {
    const { user, memory, requests } = await openPhoto();
    await user.keyboard("{ArrowDown}");
    await user.click(within(banner()).getByRole("button", { name: t("cover.cancelPosition") }));
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    expect(banner().querySelector("img.page-cover-image")).toHaveStyle({ objectPosition: "50% 30%" });

    await user.click(within(banner()).getByRole("button", { name: t("cover.reposition") }));
    await user.keyboard("{ArrowDown}{Escape}");
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();

    await user.click(within(banner()).getByRole("button", { name: t("cover.reposition") }));
    await user.keyboard("{ArrowDown}{ArrowUp}");
    await user.click(within(banner()).getByRole("button", { name: t("cover.savePosition") }));
    expect(screen.queryByRole("slider")).not.toBeInTheDocument();
    expect(setCovers(requests)).toHaveLength(0);
    expect(await coverOf(memory, "Photo")).toMatchObject({ coverY: 30 });
  });

  it("says when the core refuses and keeps the position to try again", async () => {
    const memory = newMemory();
    const recording = recordingTransport(memory);
    const refusing: Transport = {
      ...recording.transport,
      attachments: recording.transport.attachments,
      dispatch: (request) =>
        request.type === "PageSetCover"
          ? Promise.reject(new CoreError({ code: "InvalidInput", message: "is not valid", details: { field: "coverY" } }))
          : recording.transport.dispatch(request),
    };
    stubObjectUrls();
    const { user } = renderWith(refusing);
    await findMainNavigation();
    await user.click(row("Photo"));
    await waitFor(() => {
      expect(banner().querySelector("img.page-cover-image")).not.toBeNull();
    });
    await user.click(within(banner()).getByRole("button", { name: t("cover.reposition") }));
    await user.keyboard("{ArrowDown}");
    await user.click(within(banner()).getByRole("button", { name: t("cover.savePosition") }));
    expect(await screen.findByText(t("cover.error", { message: "is not valid" }))).toBeInTheDocument();
    expect(screen.getByRole("slider")).toHaveAttribute("aria-valuenow", "35");
  });
});

describe("a banner that cannot be pressed", () => {
  function renderBanner(readOnly: boolean, cover = "color:green") {
    const memory = newMemory();
    const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
    render(
      <StoreContext.Provider value={store}>
        <PageCover cover={cover} coverY={undefined} readOnly={readOnly} onChange={vi.fn()} onSavePosition={vi.fn()} onRemove={vi.fn()} />
      </StoreContext.Provider>,
    );
  }

  it("shows the banner with nothing to press when read-only", () => {
    renderBanner(true);
    expect(banner()).toHaveAttribute("data-cover-color", "green");
    expect(within(banner()).queryAllByRole("button")).toHaveLength(0);
  });

  it("has the buttons when it can be changed", () => {
    renderBanner(false);
    expect(within(banner()).getAllByRole("button").map((button) => button.textContent)).toEqual([t("cover.change"), t("cover.remove")]);
  });

  it("is shown on paper, without its buttons or the header's Add cover", async () => {
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    document.documentElement.dataset.theme = "dark";
    const { user, store } = await renderInterop({ spaces: SPACES });
    await user.click(row("Sunny"));
    expect(within(banner()).getByRole("button", { name: t("cover.change") })).toBeInTheDocument();
    act(() => {
      store.getState().startPrint();
    });
    await waitFor(() => {
      expect(store.getState().printing).toBe(true);
    });
    // The banner stays (print mode puts the light palette on the page, which its colours follow); its buttons go.
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(banner()).toHaveAttribute("data-cover-gradient", "sunset");
    expect(within(banner()).queryAllByRole("button")).toHaveLength(0);
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    await waitFor(() => {
      expect(within(banner()).getByRole("button", { name: t("cover.change") })).toBeInTheDocument();
    });
  });
});

describe("the picture of an uploaded cover", () => {
  it("names a redrawn file after the original", () => {
    expect(coverName("holiday.photo.HEIC", true)).toBe("holiday.photo.jpg");
    expect(coverName("banner.webp", false)).toBe("banner.png");
    expect(coverName("", false)).toBe("cover.png");
    expect(MAX_COVER_SIDE).toBe(2560);
  });

  /** Stands in for the webview's image decoder and canvas (jsdom has neither). */
  function stubDrawing(width: number, height: number) {
    class FakeImage {
      naturalWidth = width;
      naturalHeight = height;
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;
      set src(_value: string) {
        queueMicrotask(() => {
          this.onload?.();
        });
      }
    }
    vi.stubGlobal("Image", FakeImage);
    stubObjectUrls();
    const draw = vi.fn();
    const context = { drawImage: draw } as unknown as CanvasRenderingContext2D;
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => context);
    const encoded: { type: string | undefined; quality: number | undefined; size: { width: number; height: number } }[] = [];
    vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(function (this: HTMLCanvasElement, callback, type, quality) {
      encoded.push({ type, quality, size: { width: this.width, height: this.height } });
      callback(new Blob(["x"], { type }));
    });
    return { draw, encoded };
  }

  it("keeps a picture that fits as it is, so a cover is not scaled down to an icon's size", async () => {
    const drawing = stubDrawing(2560, 1000);
    const file = pngFile("wide.png");
    expect(await downscaleCover(file)).toBe(file);
    expect(drawing.draw).not.toHaveBeenCalled();
    const small = stubDrawing(300, 200);
    expect(await downscaleCover(file)).toBe(file);
    expect(small.draw).not.toHaveBeenCalled();
  });

  it("redraws a picture larger than 2560 pixels, a JPEG as a JPEG and the rest as a PNG", async () => {
    const drawing = stubDrawing(5120, 2560);
    const jpeg = await downscaleCover(new File(["x"], "phone.jpeg", { type: "image/jpeg" }));
    expect(jpeg).toMatchObject({ name: "phone.jpg", type: "image/jpeg" });
    expect(drawing.draw).toHaveBeenCalledWith(expect.anything(), 0, 0, 2560, 1280);
    expect(drawing.encoded[0]).toMatchObject({ type: "image/jpeg", quality: 0.9, size: { width: 2560, height: 1280 } });
    const png = await downscaleCover(new File(["x"], "scan.tiff", { type: "image/png" }));
    expect(png).toMatchObject({ name: "scan.png", type: "image/png" });
    expect(drawing.encoded[1]).toMatchObject({ type: "image/png", size: { width: 2560, height: 1280 } });
    // A tall picture is limited on its height too.
    const tall = stubDrawing(1000, 5000);
    await downscaleCover(new File(["x"], "tall.png", { type: "image/png" }));
    expect(tall.draw).toHaveBeenCalledWith(expect.anything(), 0, 0, 512, 2560);
  });

  it("keeps a picture whose size is not known (a vector picture) and a file the webview cannot draw", async () => {
    const vector = stubDrawing(0, 0);
    const svg = new File(["<svg/>"], "logo.svg", { type: "image/svg+xml" });
    expect(await downscaleCover(svg)).toBe(svg);
    expect(vector.draw).not.toHaveBeenCalled();
    vi.restoreAllMocks();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(() => null);
    const file = pngFile();
    expect(await downscaleCover(file)).toBe(file);
  });
});

describe("where the banner sits", () => {
  const CSS = readFileSync("src/styles/app.css", "utf8");

  it("does not climb over a banner above the page: with one, the top padding it climbs into is taken away", () => {
    // The cover's negative top margin is the page's top padding (`--page-pad-top`), which is the main area's padding above the
    // page's section. A banner (the recovery kit, a key rotation, a sync warning, a devices notice) stands in that place.
    expect(CSS).toMatch(/\.page-cover\s*\{[^}]*margin:\s*calc\(-1 \* var\(--page-pad-top, 0px\)\)/u);
    expect(CSS).toMatch(
      /\.content-body > :is\(\.home-banners:not\(:empty\), \.devices-notice\) ~ section\.page\s*\{\s*--page-pad-top:\s*0px;\s*\}/u,
    );
  });
});
