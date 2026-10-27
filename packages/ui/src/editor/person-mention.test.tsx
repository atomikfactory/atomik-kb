// The person mention (ADR-0049, Milestone 13 slice 2) in the editor over the in-memory core: the `@` menu lists the members of the
// space (this account alone in a personal space) between the days and the pages, choosing one inserts the chip that names the account
// and keeps the name as a label, the chip shows the member's current name or "Former member (label)", it reads as `@label` as text and
// in Markdown, a read-only editor shows it, and a mention of this account that another person adds is a notification.

import { CoreClient } from "@atomik-kb/core-client";
import type { MemoryTransport } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroList, LoroMap } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { openWorkPage, renderComments } from "../test/comments";
import { findEditor, idsOf } from "../test/page";
import { clickPage, openSpace, toasts } from "../test/sharing";

/** The cursor at the end of the page's one paragraph, then `typed`. */
function typeAtEnd(editor: Editor, typed: string): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, editor.state.doc.content.size - 1)));
    editor.commands.insertContent(typed);
  });
}

/** The mentions of the editor's document: the account and the label of each. */
function mentions(editor: Editor): { accountId: string; label: string }[] {
  const found: { accountId: string; label: string }[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === "personMention") {
      found.push({ accountId: String(node.attrs.accountId), label: String(node.attrs.label) });
    }
    return true;
  });
  return found;
}

/** Adds a mention to the first paragraph of a document, as another writer's change would. */
function addMention(doc: LoroDoc, accountId: string, label: string): void {
  const children = doc.getMap("doc").get("children") as LoroList;
  const paragraph = children.get(0) as LoroMap;
  const inline = paragraph.get("children") as LoroList;
  const node = inline.insertContainer(inline.length, new LoroMap());
  node.set("nodeName", "personMention");
  const attributes = node.setContainer("attributes", new LoroMap());
  attributes.set("accountId", accountId);
  attributes.set("label", label);
  node.setContainer("children", new LoroList());
}

async function accountOf(memory: MemoryTransport, name: string): Promise<string> {
  const client = new CoreClient(memory);
  const ids = await idsOf(memory);
  const person = (await client.peopleMentionable(ids.space("Work"))).find((candidate) => candidate.name === name);
  return person?.accountId ?? "";
}

const menuOf = (): Promise<HTMLElement> => screen.findByRole("listbox", { name: t("mention.label") });

describe("the @ menu in a shared space", () => {
  it("lists the members after the days and before the pages, under a People header, this account marked", async () => {
    const { user } = await renderComments();
    const editor = await openWorkPage(user, "Notes");
    typeAtEnd(editor, " @");
    const menu = await menuOf();
    await waitFor(() => {
      expect(within(menu).getAllByRole("group").map((group) => group.querySelector(".suggestion-section")?.textContent)).toEqual([
        t("mention.section.dates"),
        t("mention.section.people"),
        t("mention.section.pages"),
      ]);
    });
    const people = within(menu).getAllByRole("group")[1] as HTMLElement;
    expect(within(people).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual([
      "Maya",
      "Bob",
      "Carol",
      "Dan",
    ]);
    expect(within(people).getAllByRole("option")[0]).toHaveTextContent(t("mention.person.you"));
  });

  it("finds a member by what is typed, and Enter inserts a chip with the account and the name as its label", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    const editor = await openWorkPage(user, "Notes");
    typeAtEnd(editor, " @bo");
    const menu = await menuOf();
    await waitFor(() => {
      expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual(["Bob"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    const bob = await accountOf(memory, "Bob");
    expect(mentions(editor)).toEqual([{ accountId: bob, label: "Bob" }]);
    // The chip shows the name; as text it is `@label` and a space after it.
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".person-mention")).toHaveTextContent("@Bob");
    });
    expect(editor.getText()).toBe("Call the dentist @Bob ");
    // Saved with the page: the account id and the label are in the document, no block id of its own.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    await waitFor(async () => {
      expect(JSON.stringify((await client.pageContentGet(ids.page("Notes"))).content)).toContain(`"accountId":"${bob}"`);
    });
    expect(await client.pageExportMarkdown(ids.page("Notes"))).toBe("Call the dentist @Bob \n");
    // A mention of this account has its own colour.
    typeAtEnd(editor, "@ma");
    await menuOf();
    await waitFor(() => {
      expect(screen.getByRole("option", { name: /Maya/u })).toBeInTheDocument();
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll(".person-mention")).toHaveLength(2);
    });
    expect(editor.view.dom.querySelectorAll(".person-mention")[1]).toHaveClass("is-me");
  });

  it("lists this account alone in a personal space, as \"You\"", async () => {
    const { user } = await renderComments({ role: "owner", threads: [] });
    // The Personal space of the seeds is not shared: its page "Diary".
    await openSpace(user, "Personal");
    await clickPage(user, "Diary");
    const editor = await findEditor();
    typeAtEnd(editor, " @");
    const menu = await menuOf();
    const groups = within(menu).getAllByRole("group");
    expect(groups.map((group) => group.querySelector(".suggestion-section")?.textContent)).toContain(t("mention.section.people"));
    const people = groups.find((group) => group.querySelector(".suggestion-section")?.textContent === t("mention.section.people"));
    expect(within(people as HTMLElement).getAllByRole("option")).toHaveLength(1);
    expect(people).toHaveTextContent("Maya");
  });
});

describe("the chip", () => {
  it("shows the current name of the member, so a rename shows, and \"Former member (label)\" once they left", async () => {
    const { user, memory } = await renderComments();
    const ids = await idsOf(memory);
    const bob = await accountOf(memory, "Bob");
    const editor = await openWorkPage(user, "Notes");
    act(() => {
      memory.changePage(ids.page("Notes"), (doc) => {
        addMention(doc, bob, "Bob");
      });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".person-mention")).toHaveTextContent("@Bob");
    });
    act(() => {
      memory.receiveSharing({ type: "renamed", spaceId: ids.space("Work"), accountId: bob, name: "Robert" });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".person-mention")).toHaveTextContent("@Robert");
    });
    // The label stays what it was when the mention was made.
    expect(mentions(editor)).toEqual([{ accountId: bob, label: "Bob" }]);
    act(() => {
      memory.receiveSharing({ type: "removed", spaceId: ids.space("Work"), accountId: bob });
    });
    // Gone from the space, the name is the last one the space knew (Robert), not the label the mention was made with (Bob).
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".person-mention")).toHaveTextContent(`@${t("comments.formerMember.label", { label: "Robert" })}`);
    });
    expect(editor.view.dom.querySelector(".person-mention")).toHaveClass("is-former");
    expect(editor.view.dom.querySelector(".person-mention")).toHaveAttribute("title", t("comments.formerMember.title"));
  });

  it("is shown in a read-only editor, and a mention of an account nobody knows reads as a former member", async () => {
    const { user, memory } = await renderComments({ role: "viewer" });
    const ids = await idsOf(memory);
    const carol = await accountOf(memory, "Carol");
    const editor = await openWorkPage(user, "Notes");
    expect(editor.isEditable).toBe(false);
    act(() => {
      memory.changePage(ids.page("Notes"), (doc) => {
        addMention(doc, carol, "Carol");
        addMention(doc, "00000000-0000-4000-8000-0000000000ee", "Zed");
      });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll(".person-mention")).toHaveLength(2);
    });
    const [first, second] = Array.from(editor.view.dom.querySelectorAll(".person-mention"));
    expect(first).toHaveTextContent("@Carol");
    expect(second).toHaveTextContent(`@${t("comments.formerMember.label", { label: "Zed" })}`);
  });
});

describe("a mention of this account that another person adds", () => {
  it("is a notification of the page, and one this account adds itself is none", async () => {
    const { user, memory, client } = await renderComments();
    const ids = await idsOf(memory);
    const maya = await accountOf(memory, "Maya");
    const editor = await openWorkPage(user, "Notes");
    // A remote change without an actor notifies nobody; this account's own insertion neither.
    act(() => {
      memory.changePage(ids.page("Notes"), (doc) => {
        addMention(doc, maya, "Maya");
      });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll(".person-mention")).toHaveLength(1);
    });
    expect(await client.notificationList()).toEqual([]);
    // Another person's change that adds one more mention of this account is told.
    act(() => {
      memory.changePage(
        ids.page("Plan"),
        (doc) => {
          addMention(doc, maya, "Maya");
        },
        "Remote",
        "Bob",
      );
    });
    expect(await within(toasts()).findByText("Bob mentioned you on Plan")).toBeInTheDocument();
    const [row] = await client.notificationList();
    expect(row).toMatchObject({ kind: "PageMention", pageId: ids.page("Plan"), actorName: "Bob" });
  });
});
