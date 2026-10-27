// The page history dialog (ADR-0057, `docs/architecture/page-history.md` section 7) over the in-memory core: opened from the page menu and
// from the peek, the versions by day with who made them and the marks, the preview of the selected version (`VersionPreview` is drawn
// by `history/`, so a stand-in shows what the dialog gave it), the keys, restoring with its toast and Undo, what a Viewer, a locked page
// and a refusal do. The answers the twin cannot make (the edits of a member, several pages of versions) are canned.

import { CoreClient, CoreError } from "@atomik-kb/core-client";
import type { JsonValue, PageHistory, PageVersion, Request, Response, SeedSpace } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { LoroList, LoroMap, LoroText } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { activeLocale, t } from "../i18n";
import { paragraphs, pid } from "../test/editor";
import { interopTransport, renderInterop } from "../test/interop";
import { contentOf, idsOf, openPage } from "../test/page";
import type { User } from "../test/render";
import { row } from "../test/sidebar";
import { clickPage, openSpace, renderSharing } from "../test/sharing";

// The preview is drawn by `history/VersionPreview.tsx`: a stand-in that shows the text of the version it is given, whether it was given
// the page now and the title of the version.
vi.mock("../history/VersionPreview", async () => {
  const { createElement } = await import("react");
  interface Node {
    text?: string;
    content?: Node[];
  }
  return {
    VersionPreview: (props: { version: Node; current: unknown; title?: string }) =>
      createElement(
        "div",
        {
          "data-testid": "version-preview",
          "data-title": props.title ?? "",
          "data-current": props.current === null ? "loading" : "loaded",
        },
        (props.version.content ?? []).map((block) => (block.content ?? []).map((run) => run.text ?? "").join("")).join(" | "),
      ),
  };
});

/** A local date and time in October 2026. */
const at = (day: number, hour: number, minute = 0): number => new Date(2026, 9, day, hour, minute).getTime();

const time = (ms: number): string => new Intl.DateTimeFormat(activeLocale(), { timeStyle: "short" }).format(new Date(ms));
const date = (ms: number): string => new Intl.DateTimeFormat(activeLocale(), { dateStyle: "long" }).format(new Date(ms));
const instant = (ms: number): string => new Intl.DateTimeFormat(activeLocale(), { dateStyle: "medium", timeStyle: "short" }).format(new Date(ms));

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: paragraphs("Start") },
      { title: "Other", content: paragraphs("Elsewhere") },
    ],
  },
];

/** Appends a paragraph to a page document, as a writer of the core does. */
function appendParagraph(doc: LoroDoc, text: string, index: number): void {
  const children = doc.getMap("doc").get("children") as LoroList;
  const paragraph = children.insertContainer(children.length, new LoroMap());
  paragraph.set("nodeName", "paragraph");
  paragraph.setContainer("attributes", new LoroMap()).set("id", pid(100 + index));
  const inline = paragraph.setContainer("children", new LoroList());
  inline.insertContainer(0, new LoroText()).insert(0, text);
}

/** The texts of the paragraphs of portable content. */
function textsOf(content: JsonValue): string[] {
  const blocks = (content as { content?: { content?: { text?: string }[] }[] }).content ?? [];
  return blocks.map((block) => (block.content ?? []).map((run) => run.text ?? "").join(""));
}

type Intercept = (request: Request) => Response | Promise<Response> | undefined;

/**
 * The app over a personal space whose page "Notes" has a history: it started as "Start" (undated: written before any journal), then
 * "One" on October 1 at 10:00 and "Two" on October 2 at 14:00 (both titled "Notes"), the page was renamed to "Notes (final)", then
 * "Three" at 9:00 and "Four" at 9:30 on October 3 (today: the dialog's clock says 15:00). "Four" is the page now.
 */
async function setup(intercept?: Intercept) {
  let clock = at(1, 10);
  const app = await renderInterop({ spaces: SPACES, deviceName: "Studio PC", now: () => clock }, { intercept });
  const client = new CoreClient(app.memory);
  const pageId = (await idsOf(app.memory)).page("Notes");
  const edit = (when: number, text: string, index: number): void => {
    clock = when;
    app.memory.changePage(
      pageId,
      (doc) => {
        appendParagraph(doc, text, index);
      },
      "Core",
    );
  };
  edit(at(1, 10), "One", 1);
  edit(at(2, 14), "Two", 2);
  clock = at(2, 14, 5);
  await client.pageRename(pageId, "Notes (final)");
  edit(at(3, 9), "Three", 3);
  edit(at(3, 9, 30), "Four", 4);
  clock = at(3, 15);
  await waitFor(() => {
    expect(row("Notes (final)")).toBeInTheDocument();
  });
  await openPage(app.user, "Notes (final)");
  return { ...app, client, pageId };
}

beforeEach(() => {
  // "Today" is October 3 for the dialog; only the date is faked, so timers and promises run as they do.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(at(3, 15));
});

afterEach(() => {
  vi.useRealTimers();
});

/** Opens the page menu and chooses "Page history". */
async function chooseHistory(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  await user.click(await screen.findByRole("menuitem", { name: t("history.dialog.menu") }));
  return screen.findByRole("dialog", { name: t("history.dialog.title") });
}

/** Opens the dialog from the page menu and waits for the list. */
async function openHistory(user: User): Promise<HTMLElement> {
  const dialog = await chooseHistory(user);
  await within(dialog).findByRole("listbox", { name: t("history.list.label") });
  return dialog;
}

const options = (dialog: HTMLElement): HTMLElement[] => within(dialog).getAllByRole("option");
const optionAt = (dialog: HTMLElement, index: number): HTMLElement => {
  const found = options(dialog)[index];
  if (found === undefined) {
    throw new Error(`no version at ${String(index)}`);
  }
  return found;
};
const selectedOption = (dialog: HTMLElement): HTMLElement => {
  const found = options(dialog).find((option) => option.getAttribute("aria-selected") === "true");
  if (found === undefined) {
    throw new Error("no version is selected");
  }
  return found;
};
const preview = (dialog: HTMLElement): HTMLElement => within(dialog).getByTestId("version-preview");
const toasts = (): HTMLElement => screen.getByRole("log", { name: t("toast.label") });
const restoreButton = (dialog: HTMLElement): HTMLElement | null => within(dialog).queryByRole("button", { name: t("history.action.restore") });
const confirmButton = (dialog: HTMLElement): HTMLElement | null => within(dialog).queryByRole("button", { name: t("history.action.confirmYes") });

/** Waits until the preview shows `text`. */
async function expectPreview(dialog: HTMLElement, text: string | RegExp): Promise<void> {
  await waitFor(() => {
    expect(preview(dialog)).toHaveTextContent(text);
  });
}

const FOUR = "Start | One | Two | Three | Four";
const THREE = "Start | One | Two | Three";
const TWO = "Start | One | Two";

async function closed(): Promise<void> {
  await waitFor(() => {
    expect(screen.queryByRole("dialog", { name: t("history.dialog.title") })).not.toBeInTheDocument();
  });
}

describe("opening the history", () => {
  it("is an entry of the page menu, which opens the dialog on the page's versions, and closes back to the menu's button", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    expect(within(dialog).getByRole("heading", { name: t("history.dialog.title") })).toBeInTheDocument();
    expect(dialog).toHaveTextContent("Notes (final)");
    await user.click(within(dialog).getByRole("button", { name: t("history.dialog.close") }));
    await closed();
    expect(screen.getByRole("button", { name: t("page.actions") })).toHaveFocus();
  });

  it("closes with Escape", async () => {
    const { user } = await setup();
    await openHistory(user);
    await user.keyboard("{Escape}");
    await closed();
  });

  it("is a button of the peek, over the page in front, and Escape leaves the peek open", async () => {
    const { user, store, memory } = await setup();
    const ids = await idsOf(memory);
    const space = (await new CoreClient(memory).spaceList())[0]?.id ?? "";
    // The page in front is another one; "Notes (final)" opens in the side peek.
    await user.click(row("Other"));
    act(() => {
      store.getState().openPeek({ pageId: ids.page("Notes (final)"), spaceId: space, mode: "side" });
    });
    const peek = await waitFor(() => {
      const found = document.querySelector<HTMLElement>("aside[data-peek]");
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
    await user.click(within(peek).getByRole("button", { name: t("history.dialog.menu") }));
    const dialog = await screen.findByRole("dialog", { name: t("history.dialog.title") });
    await within(dialog).findByRole("listbox", { name: t("history.list.label") });
    await expectPreview(dialog, THREE);
    await user.keyboard("{Escape}");
    await closed();
    expect(document.querySelector("aside[data-peek]")).not.toBeNull();
  });
});

describe("the list", () => {
  it("groups the versions by day, newest first, with the time, who made it, the marks and the titles that differ", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    const groups = within(dialog).getAllByRole("group");
    expect(groups.map((group) => group.querySelector(".history-group-title")?.textContent)).toEqual([
      t("history.list.today"),
      t("history.list.yesterday"),
      date(at(1, 10)),
      t("history.list.undated"),
    ]);
    const rows = groups.map((group) => within(group).getAllByRole("option").map((option) => option.textContent));
    // Today: the page as it is now (marked), and the session before it. The title is the page's own, so it is not repeated.
    expect(rows[0]).toEqual([`${time(at(3, 9, 30))}${t("history.list.current")}Studio PC`, `${time(at(3, 9))}Studio PC`]);
    // Before the rename the page was called "Notes".
    expect(rows[1]).toEqual([`${time(at(2, 14))}Studio PC${t("history.list.title", { title: "Notes" })}`]);
    expect(rows[2]).toEqual([`${time(at(1, 10))}Studio PC${t("history.list.title", { title: "Notes" })}`]);
    // What the journal never saw is listed by position.
    expect(rows[3]).toEqual([t("history.list.earlierVersion", { n: 1 })]);
    expect(within(dialog).queryByRole("button", { name: t("history.list.older") })).not.toBeInTheDocument();
  });

  it("opens on the newest version that is not the page now, with the list ready for the keys", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    expect(selectedOption(dialog)).toHaveTextContent(time(at(3, 9)));
    expect(within(dialog).getByRole("listbox")).toHaveFocus();
    await expectPreview(dialog, THREE);
    // The page now is given to the preview to compare with.
    await waitFor(() => {
      expect(preview(dialog)).toHaveAttribute("data-current", "loaded");
    });
  });

  it("lists a page that never changed as the one version it is, which has nothing to restore", async () => {
    const { user } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Other");
    const dialog = await openHistory(user);
    expect(options(dialog)).toHaveLength(1);
    expect(selectedOption(dialog)).toHaveTextContent(t("history.list.earlierVersion", { n: 1 }));
    expect(selectedOption(dialog)).toHaveTextContent(t("history.list.current"));
    expect(restoreButton(dialog)).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("history.action.current"))).toBeInTheDocument();
  });

  it("says so when the core lists no version", async () => {
    const { user } = await renderInterop(
      { spaces: SPACES },
      { intercept: (request) => (request.type === "PageHistoryList" ? { type: "PageHistory", data: { pageId: request.data.pageId, versions: [], more: false } } : undefined) },
    );
    await openPage(user, "Other");
    const dialog = await chooseHistory(user);
    expect(await within(dialog).findByText(t("history.dialog.empty"))).toBeInTheDocument();
    expect(within(dialog).queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("says when the list cannot be read, and reads it again on request", async () => {
    let failures = 1;
    const { user } = await renderInterop(
      { spaces: SPACES },
      {
        intercept: (request) => {
          if (request.type === "PageHistoryList" && failures > 0) {
            failures -= 1;
            throw new CoreError({ code: "Internal", message: "the journal is unreadable" });
          }
          return undefined;
        },
      },
    );
    await openPage(user, "Other");
    const dialog = await chooseHistory(user);
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(t("history.dialog.error", { message: "the journal is unreadable" }));
    await user.click(within(alert).getByRole("button", { name: t("history.dialog.retry") }));
    expect(await within(dialog).findByRole("listbox", { name: t("history.list.label") })).toBeInTheDocument();
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("the preview", () => {
  it("shows the version that is selected, with the title it had", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    await user.click(optionAt(dialog, 2)); // October 2, 14:00
    expect(selectedOption(dialog)).toHaveTextContent(time(at(2, 14)));
    await expectPreview(dialog, TWO);
    expect(preview(dialog)).toHaveAttribute("data-title", "Notes");
    // The heading names the version.
    expect(within(dialog).getByRole("heading", { level: 3 })).toHaveTextContent(instant(at(2, 14)));
    // An undated version has no title, and is named by its position.
    await user.click(optionAt(dialog, 4));
    await expectPreview(dialog, /^Start$/u);
    expect(preview(dialog)).toHaveAttribute("data-title", "");
    expect(within(dialog).getByRole("heading", { level: 3 })).toHaveTextContent(t("history.list.earlierVersion", { n: 1 }));
    // The page now is a version like the others.
    await user.click(optionAt(dialog, 0));
    await expectPreview(dialog, FOUR);
  });

  it("stays mounted when another version is selected, so that its own state (Show changes) is kept", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    await expectPreview(dialog, THREE);
    const drawn = preview(dialog);
    await user.click(optionAt(dialog, 2));
    await expectPreview(dialog, TWO);
    expect(preview(dialog)).toBe(drawn);
  });

  it("reads a version once: coming back to it asks nothing", async () => {
    const { user, requests } = await setup();
    const dialog = await openHistory(user);
    await expectPreview(dialog, THREE);
    await user.click(optionAt(dialog, 2));
    await expectPreview(dialog, TWO);
    await user.click(optionAt(dialog, 1));
    await expectPreview(dialog, THREE);
    expect(requests.filter((request) => request.type === "PageHistoryGet")).toHaveLength(2);
  });

  it("says when a version cannot be opened, and tries again", async () => {
    let failures = 1;
    const { user } = await setup((request) => {
      if (request.type === "PageHistoryGet" && failures > 0) {
        failures -= 1;
        throw new CoreError({ code: "InvalidInput", message: "names changes the page does not have", details: { field: "version" } });
      }
      return undefined;
    });
    const dialog = await openHistory(user);
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(t("history.dialog.previewError", { message: "names changes the page does not have" }));
    await user.click(within(alert).getByRole("button", { name: t("history.dialog.retry") }));
    await expectPreview(dialog, THREE);
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("the keys", () => {
  it("move through the list with the arrows, Home and End, and ask before Enter restores", async () => {
    const { user, pageId, memory } = await setup();
    const dialog = await openHistory(user);
    const listbox = within(dialog).getByRole("listbox");
    expect(listbox).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(selectedOption(dialog)).toHaveTextContent(time(at(2, 14)));
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(selectedOption(dialog)).toHaveTextContent(t("history.list.current"));
    await user.keyboard("{ArrowUp}");
    expect(selectedOption(dialog)).toHaveTextContent(t("history.list.current"));
    await user.keyboard("{End}");
    expect(selectedOption(dialog)).toHaveTextContent(t("history.list.earlierVersion", { n: 1 }));
    await user.keyboard("{Home}");
    // The page now has nothing to restore: Enter does nothing there. The preview follows the selection.
    await user.keyboard("{Enter}");
    expect(confirmButton(dialog)).not.toBeInTheDocument();
    await expectPreview(dialog, FOUR);

    // Enter on an older version asks, and Escape takes the question back without closing the dialog.
    await user.keyboard("{ArrowDown}{Enter}");
    expect(within(dialog).getByRole("group", { name: t("history.action.confirm", { version: instant(at(3, 9)) }) })).toBeInTheDocument();
    expect(confirmButton(dialog)).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(confirmButton(dialog)).not.toBeInTheDocument();
    expect(dialog).toBeInTheDocument();
    expect(listbox).toHaveFocus();
    expect(textsOf(await contentOf(memory, pageId))).toEqual(["Start", "One", "Two", "Three", "Four"]);

    // Enter, then Enter again on the question, restores.
    await user.keyboard("{Enter}");
    await user.keyboard("{Enter}");
    await closed();
    expect(textsOf(await contentOf(memory, pageId))).toEqual(["Start", "One", "Two", "Three"]);
  });

  it("keep a click on a version from leaving the keys behind", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    await user.click(optionAt(dialog, 3));
    expect(within(dialog).getByRole("listbox")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(selectedOption(dialog)).toHaveTextContent(time(at(2, 14)));
  });
});

describe("restoring", () => {
  it("makes the page what the version was, closes the dialog and says so with an Undo that goes back", async () => {
    const { user, client, pageId, memory } = await setup();
    const dialog = await openHistory(user);
    await user.click(optionAt(dialog, 2)); // October 2, 14:00: "Start, One, Two", titled "Notes"
    await expectPreview(dialog, TWO);
    await user.click(restoreButton(dialog) as HTMLElement);
    await closed();
    expect(textsOf(await contentOf(memory, pageId))).toEqual(["Start", "One", "Two"]);
    // The title came back with the content.
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Notes$/u);
    });
    expect(within(toasts()).getByText(t("history.toast.restored"))).toBeInTheDocument();
    // The restore is a version of its own, and the one before it is still listed.
    const history = await client.pageHistoryList(pageId);
    expect(history.versions[0]).toMatchObject({ kind: "restore", current: true });

    await user.click(within(toasts()).getByRole("button", { name: t("history.toast.undo") }));
    await waitFor(() => {
      expect(within(toasts()).getByText(t("history.toast.undone"))).toBeInTheDocument();
    });
    expect(textsOf(await contentOf(memory, pageId))).toEqual(["Start", "One", "Two", "Three", "Four"]);
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Notes (final)");
    });
    expect(within(toasts()).queryByRole("button", { name: t("history.toast.undo") })).not.toBeInTheDocument();
  });

  it("goes back to the version that is current when the restore is made, not the one the list showed", async () => {
    const { user, pageId, memory } = await setup();
    const dialog = await openHistory(user);
    await user.click(optionAt(dialog, 2));
    await expectPreview(dialog, TWO);
    // Another device's edit arrives while the dialog is open.
    act(() => {
      memory.changePage(pageId, (doc) => {
        appendParagraph(doc, "Five", 5);
      });
    });
    await user.click(restoreButton(dialog) as HTMLElement);
    await closed();
    await user.click(within(toasts()).getByRole("button", { name: t("history.toast.undo") }));
    await waitFor(() => {
      expect(within(toasts()).getByText(t("history.toast.undone"))).toBeInTheDocument();
    });
    expect(textsOf(await contentOf(memory, pageId))).toEqual(["Start", "One", "Two", "Three", "Four", "Five"]);
  });

  it("marks the restore in the list, with the version before it still there", async () => {
    const { user, client, pageId } = await setup();
    const versions = (await client.pageHistoryList(pageId)).versions;
    await client.pageHistoryRestore(pageId, versions[2]?.id ?? "");
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/^Notes$/u);
    });
    const dialog = await openHistory(user);
    expect(optionAt(dialog, 0)).toHaveTextContent(t("history.list.restored"));
    expect(optionAt(dialog, 0)).toHaveTextContent(t("history.list.current"));
    // Opens on the page as it was before the restore, which was called "Notes (final)".
    expect(selectedOption(dialog)).toHaveTextContent(time(at(3, 9, 30)));
    expect(selectedOption(dialog)).toHaveTextContent(t("history.list.title", { title: "Notes (final)" }));
    await expectPreview(dialog, FOUR);
  });

  it("shows the core's reason in the dialog when it refuses, and keeps the dialog open", async () => {
    const { user, pageId, memory } = await setup((request) => {
      if (request.type === "PageHistoryRestore") {
        throw new CoreError({ code: "InvalidInput", message: "the page is in the trash", details: { field: "pageId" } });
      }
      return undefined;
    });
    const dialog = await openHistory(user);
    await user.click(restoreButton(dialog) as HTMLElement);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("history.action.error", { message: "the page is in the trash" }));
    expect(restoreButton(dialog)).toBeEnabled();
    expect(within(toasts()).queryByText(t("history.toast.restored"))).not.toBeInTheDocument();
    expect(textsOf(await contentOf(memory, pageId))).toEqual(["Start", "One", "Two", "Three", "Four"]);
    // Selecting another version clears it.
    await user.click(optionAt(dialog, 2));
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("says when Undo is refused", async () => {
    let restores = 0;
    const { user } = await setup((request) => {
      if (request.type === "PageHistoryRestore") {
        restores += 1;
        if (restores > 1) {
          throw new CoreError({ code: "InvalidInput", message: "the page is in the trash", details: { field: "pageId" } });
        }
      }
      return undefined;
    });
    const dialog = await openHistory(user);
    await user.click(restoreButton(dialog) as HTMLElement);
    await closed();
    await user.click(within(toasts()).getByRole("button", { name: t("history.toast.undo") }));
    expect(await within(toasts()).findByText(t("history.toast.undoError", { message: "the page is in the trash" }))).toBeInTheDocument();
  });

  it("does not offer the page as it is now", async () => {
    const { user } = await setup();
    const dialog = await openHistory(user);
    await user.click(optionAt(dialog, 0));
    expect(restoreButton(dialog)).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("history.action.current"))).toBeInTheDocument();
  });

  it("is not offered for a locked page, which can be looked at", async () => {
    const { user, client, pageId } = await setup();
    await client.pageSetAppearance(pageId, { locked: true });
    await screen.findByText(t("page.locked"));
    const dialog = await openHistory(user);
    expect(options(dialog)).toHaveLength(5);
    expect(restoreButton(dialog)).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("history.action.locked"))).toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(confirmButton(dialog)).not.toBeInTheDocument();
  });
});

describe("a Viewer", () => {
  it("sees the list and the preview and cannot restore", async () => {
    const { user } = await renderSharing({ role: "viewer" });
    await openSpace(user, "Work");
    await clickPage(user, "Notes");
    const dialog = await openHistory(user);
    await expectPreview(dialog, /Call the dentist/u);
    expect(restoreButton(dialog)).not.toBeInTheDocument();
    expect(within(dialog).getByText(t("history.action.viewOnly"))).toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(confirmButton(dialog)).not.toBeInTheDocument();
  });
});

describe("who made a version", () => {
  it("names this device, a member by the name the space gives, a device of this account by its label, and a former member as such", async () => {
    const authors: PageVersion[] = [];
    const { user, client } = await renderSharing({
      role: "owner",
      wrap: (memory) =>
        interopTransport(memory, {
          intercept: (request) => {
            if (request.type === "PageHistoryList") {
              return { type: "PageHistory", data: { pageId: request.data.pageId, versions: authors, more: false } };
            }
            if (request.type === "PageHistoryGet") {
              return { type: "PageVersionContent", data: { pageId: request.data.pageId, version: request.data.version, content: paragraphs("Canned") } };
            }
            return undefined;
          },
        }).transport,
    });
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const members = (await client.spaceMembers(work?.id ?? "")).members;
    const bob = members.find((member) => member.name === "Bob")?.accountId ?? "";
    const maya = members.find((member) => member.name === "Maya")?.accountId ?? "";
    const version = (n: number, extra: Partial<PageVersion>): PageVersion => ({ id: `1@${String(n)}`, kind: "edit", startedAt: at(3, 10 - n), endedAt: at(3, 10 - n), ...extra });
    authors.push(
      version(1, { current: true, author: { thisDevice: true, accountId: maya } }),
      version(2, { author: { accountId: bob } }),
      version(3, { author: { accountId: maya, deviceId: "another-device" } }),
      version(4, { author: { accountId: "someone-who-left" } }),
      version(5, {}),
    );
    await openSpace(user, "Work");
    await clickPage(user, "Notes");
    const dialog = await openHistory(user);
    const device = await client.deviceInfo();
    await waitFor(() => {
      const rows = options(dialog).map((option) => option.textContent);
      expect(rows[0]).toBe(`${time(at(3, 9))}${t("history.list.current")}${device.name}`);
      expect(rows[1]).toBe(`${time(at(3, 8))}Bob`);
      expect(rows[2]).toBe(`${time(at(3, 7))}${t("properties.computed.unknownDevice")}`);
      expect(rows[3]).toBe(`${time(at(3, 6))}${t("comments.formerMember")}`);
      expect(rows[4]).toBe(time(at(3, 5)));
    });
  });
});

describe("older versions", () => {
  const version = (n: number, hour: number, extra: Partial<PageVersion> = {}): PageVersion => ({
    id: `7@${String(n)}`,
    kind: "edit",
    startedAt: at(3, hour),
    endedAt: at(3, hour),
    ...extra,
  });

  /** The list in pages of two, the second asked `before` the last version of the first. `fail` makes that second ask fail once. */
  function pagesOf(state: { fail: boolean }, asked: (string | undefined)[]): Intercept {
    return (request) => {
      if (request.type === "PageHistoryList") {
        asked.push(request.data.before);
        const pageId = request.data.pageId;
        if (request.data.before === undefined) {
          const first: PageHistory = { pageId, versions: [version(4, 12, { current: true }), version(3, 11)], more: true };
          return { type: "PageHistory", data: first };
        }
        if (state.fail) {
          state.fail = false;
          throw new CoreError({ code: "Internal", message: "the journal is unreadable" });
        }
        const second: PageHistory = { pageId, versions: [version(2, 10), version(1, 9)], more: false };
        return { type: "PageHistory", data: second };
      }
      if (request.type === "PageHistoryGet") {
        return { type: "PageVersionContent", data: { pageId: request.data.pageId, version: request.data.version, content: paragraphs(request.data.version) } };
      }
      return undefined;
    };
  }

  it("loads the next page of versions when asked, keeping the day together and the selection", async () => {
    const asked: (string | undefined)[] = [];
    const { user } = await renderInterop({ spaces: SPACES }, { intercept: pagesOf({ fail: false }, asked) });
    await openPage(user, "Other");
    const dialog = await openHistory(user);
    expect(options(dialog)).toHaveLength(2);
    expect(selectedOption(dialog)).toHaveTextContent(time(at(3, 11)));
    await expectPreview(dialog, "7@3");
    await user.click(within(dialog).getByRole("button", { name: t("history.list.older") }));
    await waitFor(() => {
      expect(options(dialog)).toHaveLength(4);
    });
    expect(within(dialog).getAllByRole("group")).toHaveLength(1);
    expect(options(dialog).map((option) => option.textContent)).toEqual([
      `${time(at(3, 12))}${t("history.list.current")}`,
      time(at(3, 11)),
      time(at(3, 10)),
      time(at(3, 9)),
    ]);
    expect(selectedOption(dialog)).toHaveTextContent(time(at(3, 11)));
    expect(within(dialog).queryByRole("button", { name: t("history.list.older") })).not.toBeInTheDocument();
    // The second ask resumed after the last version of the first answer.
    expect(asked.filter((before) => before !== undefined)).toEqual(["7@3"]);
    // The older versions can be opened as well.
    await user.click(optionAt(dialog, 3));
    await expectPreview(dialog, "7@1");
  });

  it("says when the older versions cannot be read, and keeps what is listed", async () => {
    const { user } = await renderInterop({ spaces: SPACES }, { intercept: pagesOf({ fail: true }, []) });
    await openPage(user, "Other");
    const dialog = await openHistory(user);
    await user.click(within(dialog).getByRole("button", { name: t("history.list.older") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("history.list.olderError", { message: "the journal is unreadable" }));
    expect(options(dialog)).toHaveLength(2);
    await user.click(within(dialog).getByRole("button", { name: t("history.list.older") }));
    await waitFor(() => {
      expect(options(dialog)).toHaveLength(4);
    });
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });
});
