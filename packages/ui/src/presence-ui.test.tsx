// Presence on screen (Milestone 13, slice 3; ADR-0050) over the in-memory core and its simulated people: the avatars of who is on the
// open page (its header) and who is in a space (the space switcher) with initials, the colour of their join order and the name as a
// tooltip, and never you; the cursors of a member who edits the page drawn in the editor with their name; what this device sends
// (nothing in a personal space; a Viewer's presence too) and the texts in French and Spanish.

import type { MemoryPresencePeer, MemoryTransport, PresenceInfo, PresenceState, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { loroCursorPluginKey, loroSyncPluginKey } from "./editor/loro-binding";
import { loadLanguage, resetLanguage, t, tp } from "./i18n";
import type { Dictionary } from "./i18n";
import { PRESENCE_PALETTE } from "./presence";
import { findEditor, idsOf } from "./test/page";
import { clickPage, openSpace, renderSharing } from "./test/sharing";

const rgb = (hex: string): string => {
  const value = Number.parseInt(hex.slice(1), 16);
  return `rgb(${String((value >> 16) & 255)}, ${String((value >> 8) & 255)}, ${String(value & 255)})`;
};

beforeEach(() => {
  // The window is the one in use: presence goes out.
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
});

afterEach(() => {
  resetLanguage();
  vi.restoreAllMocks();
});

/** Bob (join order 1) edits Plan, Carol (2) reads it, Dan (3) reads Notes. This account, Maya, is the creator (0). */
const PEOPLE: readonly MemoryPresencePeer[] = [
  { space: "Work", member: "Bob", route: [{ page: "Plan", does: "edit" }] },
  { space: "Work", member: "Carol", route: [{ page: "Plan" }] },
  { space: "Work", member: "Dan", route: [{ page: "Notes" }] },
];

const withPeople = (peers: readonly MemoryPresencePeer[] = PEOPLE) => ({ presence: { stepMs: 0, peers } });

/** What a transport that records this device's presence frames sends on. */
interface Sent {
  spaceId: string;
  pageId: string | undefined;
  state: PresenceState;
  update: Uint8Array;
}

/** The in-memory core behind a transport that passes everything on, except what `overrides` replaces. */
function wrapped(memory: MemoryTransport, overrides: Partial<Transport> = {}): Transport {
  return {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (listener) => memory.subscribe(listener),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    attachments: memory.attachments,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
    backupExport: (path) => memory.backupExport(path),
    backupImport: (path, kit) => memory.backupImport(path, kit),
    presence: memory.presence,
    ...overrides,
  };
}

/** A transport that records this device's presence frames, and sends them on. */
function recordingPresence(memory: MemoryTransport, sent: Sent[]): Transport {
  return wrapped(memory, {
    presence: {
      send: (spaceId, pageId, state, update) => {
        sent.push({ spaceId, pageId, state, update });
        return memory.presence.send(spaceId, pageId, state, update);
      },
      subscribe: (listener) => memory.presence.subscribe(listener),
    },
  });
}

const pageAvatars = (): HTMLElement => screen.getByRole("list", { name: t("presence.page.label") });
const namesIn = (group: HTMLElement): string[] => within(group).getAllByRole("listitem").map((item) => item.getAttribute("title") ?? "");

describe("the avatars of the page header", () => {
  it("show who is on the open page: initials, the colour of their join order and their name as a tooltip", async () => {
    const { user } = await renderSharing({ memory: withPeople() });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    const group = await waitFor(pageAvatars);
    // Bob and Carol are on Plan, in join order; Dan is on Notes.
    expect(namesIn(group)).toEqual(["Bob", "Carol"]);
    const [bob, carol] = within(group).getAllByRole("listitem");
    expect(bob).toHaveTextContent("B");
    expect(carol).toHaveTextContent("C");
    expect(bob).toHaveStyle({ backgroundColor: rgb(PRESENCE_PALETTE[1] ?? "") });
    expect(carol).toHaveStyle({ backgroundColor: rgb(PRESENCE_PALETTE[2] ?? "") });
    // The name is there for a screen reader too, and the initials are not read.
    expect(within(bob as HTMLElement).getByText("Bob")).toHaveClass("visually-hidden");
    expect(within(bob as HTMLElement).getByText("B")).toHaveAttribute("aria-hidden", "true");
  });

  it("never show you, whatever the core lists: this account's own other device is left out", async () => {
    const { user } = await renderSharing({
      memory: withPeople(),
      wrap: (inner) =>
        wrapped(inner, {
          dispatch: async (request) => {
            const response = await inner.dispatch(request);
            if (request.type === "PresenceList" && response.type === "Presence") {
              const members = await inner.dispatch({ type: "SpaceMembers", data: { spaceId: request.data.spaceId } });
              const me = members.type === "Members" ? members.data.members.find((member) => member.isMe)?.accountId : undefined;
              const first = response.data[0];
              const mine: PresenceInfo = {
                accountId: me ?? "",
                deviceId: "99999999-9999-4999-8999-999999999999",
                name: "Maya",
                color: 0,
                state: "Editing",
                updatedAt: 1,
                ...(first?.pageId === undefined ? {} : { pageId: first.pageId }),
              };
              return { type: "Presence", data: [mine, ...response.data] };
            }
            return response;
          },
        }),
    });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    expect(namesIn(await waitFor(pageAvatars))).toEqual(["Bob", "Carol"]);
    const button = screen.getByRole("button", { description: t("spaces.switchTitle") });
    expect(namesIn(within(button).getByRole("list", { name: t("presence.space.label") }))).toEqual(["Bob", "Carol", "Dan"]);
    expect(screen.queryByTitle("Maya")).not.toBeInTheDocument();
  });

  it("follow the page: a member who arrives is shown, one who goes is not", async () => {
    const { user, memory, client } = await renderSharing({ memory: withPeople([]) });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    expect(screen.queryByRole("list", { name: t("presence.page.label") })).not.toBeInTheDocument();
    const ids = await idsOf(memory);
    const work = ids.space("Work");
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: work, member: "Dan", pageId: ids.page("Plan") });
    });
    expect(namesIn(await waitFor(pageAvatars))).toEqual(["Dan"]);
    // Dan moves to another page: not on this one.
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: work, member: "Dan", pageId: ids.page("Notes") });
    });
    await waitFor(() => {
      expect(screen.queryByRole("list", { name: t("presence.page.label") })).not.toBeInTheDocument();
    });
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: ids.page("Plan") });
      memory.receivePresence({ type: "frame", spaceId: work, member: "Carol", pageId: ids.page("Plan") });
    });
    await waitFor(() => {
      expect(namesIn(pageAvatars())).toEqual(["Bob", "Carol"]);
    });
    // Bob's member leaves the space: his avatar goes with him.
    const bob = (await client.peopleMentionable(work)).find((person) => person.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "memberLeft", spaceId: work, accountId: bob?.accountId ?? "" });
    });
    await waitFor(() => {
      expect(namesIn(pageAvatars())).toEqual(["Carol"]);
    });
  });

  it("count the ones that do not fit: the fifth is a +N with the names in its tooltip", async () => {
    const { user, memory } = await renderSharing({
      memory: withPeople([]),
      sharing: {
        spaces: [
          {
            space: "Work",
            members: ["Bob", "Carol", "Dan", "Erin", "Frank", "Gus"].map((name) => ({ name, role: "editor" as const })),
          },
        ],
      },
    });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    const ids = await idsOf(memory);
    act(() => {
      for (const member of ["Bob", "Carol", "Dan", "Erin", "Frank", "Gus"]) {
        memory.receivePresence({ type: "frame", spaceId: ids.space("Work"), member, pageId: ids.page("Plan") });
      }
    });
    const group = await waitFor(pageAvatars);
    const items = within(group).getAllByRole("listitem");
    expect(items).toHaveLength(4);
    expect(namesIn(group).slice(0, 3)).toEqual(["Bob", "Carol", "Dan"]);
    expect(items[3]).toHaveTextContent("+3");
    expect(items[3]).toHaveAttribute("title", "Erin, Frank, Gus");
    expect(within(items[3] as HTMLElement).getByText(`${tp("presence.more", 3)}: Erin, Frank, Gus`)).toHaveClass("visually-hidden");
  });

  it("are not in a personal space", async () => {
    const { user } = await renderSharing({ role: "personal", memory: withPeople([]) });
    await openSpace(user, "Personal");
    await clickPage(user, "Diary");
    expect(screen.queryByRole("list", { name: t("presence.page.label") })).not.toBeInTheDocument();
  });
});

describe("the avatars of the space switcher", () => {
  it("show who is in the space, and dim those who are idle, in the button and in the list", async () => {
    const { user, memory } = await renderSharing({ memory: withPeople() });
    await openSpace(user, "Work");
    const button = screen.getByRole("button", { description: t("spaces.switchTitle") });
    const group = await waitFor(() => within(button).getByRole("list", { name: t("presence.space.label") }));
    expect(namesIn(group)).toEqual(["Bob", "Carol", "Dan"]);
    const ids = await idsOf(memory);
    // Carol goes idle: she is in the space, in no page.
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: ids.space("Work"), member: "Carol", state: "Idle" });
    });
    await waitFor(() => {
      expect(namesIn(within(button).getByRole("list", { name: t("presence.space.label") }))).toEqual(["Bob", t("presence.person.idle", { name: "Carol" }), "Dan"]);
    });
    const carol = within(button).getAllByRole("listitem")[1];
    expect(carol).toHaveClass("is-idle");
    // The list of spaces shows them for the space that is shared, and nothing for the others.
    await user.click(button);
    const list = await screen.findByRole("list", { name: t("spaces.listLabel") });
    expect(within(within(list).getByRole("button", { name: /^Work/u })).getByRole("list", { name: t("presence.space.label") })).toBeInTheDocument();
    expect(within(within(list).getByRole("button", { name: /^Personal/u })).queryByRole("list", { name: t("presence.space.label") })).not.toBeInTheDocument();
  });

  it("show nothing for a space where nobody else is", async () => {
    const { user } = await renderSharing({ memory: withPeople([]) });
    await openSpace(user, "Work");
    expect(screen.queryByRole("list", { name: t("presence.space.label") })).not.toBeInTheDocument();
  });
});

describe("the cursors in the editor", () => {
  it("draws the caret of a member who edits the page with their name and colour, and follows it as it moves", async () => {
    const { user, memory } = await renderSharing({ memory: withPeople() });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    const editor = await findEditor();
    // The cursor plugin reads the sync plugin's state, so it must come after it in the editor's plugins.
    const order = (key: unknown): number => editor.state.plugins.findIndex((plugin) => plugin.spec.key === key);
    expect(order(loroSyncPluginKey)).toBeGreaterThanOrEqual(0);
    expect(order(loroCursorPluginKey)).toBeGreaterThan(order(loroSyncPluginKey));
    // Bob's device sends a frame as the page is open (his script takes a step): the caret at the end of "Ship".
    act(() => {
      memory.presenceStep();
    });
    const label = await waitFor(() => {
      const found = editor.view.dom.querySelector<HTMLElement>(".loro-cursor-label");
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
    expect(label).toHaveTextContent("Bob");
    expect(label).toHaveStyle({ backgroundColor: rgb(PRESENCE_PALETTE[1] ?? "") });
    // Only Bob edits the page: Carol reads it and has no cursor.
    expect(editor.view.dom.querySelectorAll(".loro-cursor")).toHaveLength(1);
    expect(editor.view.dom.querySelector(".loro-cursor-selection")).toBeNull();
    // The next step: the word "it" selected.
    act(() => {
      memory.presenceStep();
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".loro-cursor-selection")?.textContent).toBe("it");
    });
    expect(editor.view.dom.querySelectorAll(".loro-cursor")).toHaveLength(1);
    // None of it is in the document.
    expect(editor.state.doc.textContent).toBe("Ship it");
  });

  it("a frame for another page draws nothing here, and the cursor goes when its member leaves the page", async () => {
    const { user, memory } = await renderSharing({ memory: withPeople([]) });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    const editor = await findEditor();
    const ids = await idsOf(memory);
    const work = ids.space("Work");
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: ids.page("Notes"), select: { quote: "dentist" } });
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(editor.view.dom.querySelector(".loro-cursor")).toBeNull();
    // On this page, with a cursor in it.
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: ids.page("Plan"), select: { quote: "Ship it" } });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".loro-cursor-selection")?.textContent).toBe("Ship it");
    });
    // He moves to Notes: his cursor is gone from this page at once.
    act(() => {
      memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: ids.page("Notes"), select: { quote: "dentist" } });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".loro-cursor")).toBeNull();
    });
  });
});

describe("what this device sends", () => {
  it("sends nothing in a personal space, whatever the editor does", async () => {
    const sent: Sent[] = [];
    const { user, memory } = await renderSharing({ role: "personal", memory: withPeople([]), wrap: (inner) => recordingPresence(inner, sent) });
    await openSpace(user, "Personal");
    await clickPage(user, "Diary");
    const editor = await findEditor();
    editor.commands.focus("end");
    editor.commands.setTextSelection({ from: 2, to: 5 });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 250));
    });
    expect(sent).toEqual([]);
    expect(memory.sentPresence()).toEqual([]);
  });

  it("sends the page and what the device does in a shared space: viewing a page, editing with a selection", async () => {
    const sent: Sent[] = [];
    const { user, memory } = await renderSharing({ memory: withPeople([]), wrap: (inner) => recordingPresence(inner, sent) });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    const ids = await idsOf(memory);
    const editor = await findEditor();
    await waitFor(() => {
      expect(sent.at(-1)).toMatchObject({ spaceId: ids.space("Work"), pageId: ids.page("Plan"), state: "Viewing" });
    });
    editor.commands.focus("end");
    editor.commands.setTextSelection({ from: 2, to: 5 });
    await waitFor(() => {
      expect(sent.at(-1)).toMatchObject({ pageId: ids.page("Plan"), state: "Editing" });
    });
    expect(sent.at(-1)?.update.length).toBeGreaterThan(0);
    expect(memory.sentPresence().at(-1)).toMatchObject({ pageId: ids.page("Plan"), state: "Editing" });
  });

  it("a Viewer sends presence too, and views the page", async () => {
    const sent: Sent[] = [];
    const { user, memory } = await renderSharing({ role: "viewer", memory: withPeople([]), wrap: (inner) => recordingPresence(inner, sent) });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    const ids = await idsOf(memory);
    await findEditor();
    await waitFor(() => {
      expect(sent.at(-1)).toMatchObject({ spaceId: ids.space("Work"), pageId: ids.page("Plan"), state: "Viewing" });
    });
  });

  it("sends nothing while the window is not the one in use, and one Idle frame as it is left", async () => {
    const sent: Sent[] = [];
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    const { user } = await renderSharing({ memory: withPeople([]), wrap: (inner) => recordingPresence(inner, sent) });
    await openSpace(user, "Work");
    await clickPage(user, "Plan");
    await findEditor();
    expect(sent).toEqual([]);
    focus.mockReturnValue(true);
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => {
      expect(sent.at(-1)?.state).toBe("Viewing");
    });
    focus.mockReturnValue(false);
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    await waitFor(() => {
      expect(sent.at(-1)).toMatchObject({ pageId: undefined, state: "Idle" });
    });
  });
});

describe("in French and Spanish", () => {
  for (const language of ["fr", "es"] as const) {
    it(`says it in ${language}: the groups, an idle member and the ones that do not fit`, async () => {
      const dictionary = JSON.parse(readFileSync(`src/i18n/${language}.json`, "utf8")) as Dictionary;
      await loadLanguage(language, () => Promise.resolve(dictionary));
      for (const key of ["presence.page.label", "presence.space.label", "presence.person.idle", "presence.more.one", "presence.more.other"]) {
        expect(dictionary[key], key).toBeDefined();
      }
      // A placeholder is filled, never left in the text.
      expect(t("presence.person.idle", { name: "Bob" })).toMatch(/^Bob \(/u);
      expect(t("presence.person.idle", { name: "Bob" })).not.toContain("{");
      expect(tp("presence.more", 1)).toContain("1");
      expect(tp("presence.more", 3)).toContain("3");
      expect(tp("presence.more", 3)).not.toBe(tp("presence.more", 1));
      expect(t("presence.page.label")).not.toBe("People on this page");
      expect(t("presence.space.label")).not.toBe("People in this space");
    });
  }
});
