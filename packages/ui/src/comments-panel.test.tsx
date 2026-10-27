// The comments panel over the in-memory core (Milestone 13, slice 2; ADR-0048): the threads of a page with their comments, a new
// comment on the page, replies, edit, delete (final, after a confirmation) and resolve, "show resolved", what each role may do, the
// authors who left, the `@` people picker of the composer, and that the panel follows the core's events.

import type { MemberRole } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { THREADS, commentWith, commentsToggle, openPanel, openWorkPage, renderComments, threadCards, threadWith, toggleLabel } from "./test/comments";
import { idsOf } from "./test/page";

/** A button of a scope by its name. */
const button = (scope: HTMLElement, name: string): HTMLElement => within(scope).getByRole("button", { name });

describe("the panel", () => {
  it("opens from the page header, which counts the open threads, and lists the threads of the page", async () => {
    const { user } = await renderComments({ threads: THREADS });
    await openWorkPage(user, "Plan");
    // The resolved thread is not an open one.
    await waitFor(() => {
      expect(commentsToggle()).toHaveAccessibleName(toggleLabel(1));
    });
    expect(screen.queryByRole("complementary", { name: t("comments.panel") })).not.toBeInTheDocument();

    const panel = await openPanel(user);
    expect(await within(panel).findByText("Can we ship this on Friday?")).toBeInTheDocument();
    expect(threadCards(panel)).toHaveLength(1);
    const card = threadWith(panel, "Friday");
    // The words the thread is about, its comments in order with their authors, a mention as a chip.
    expect(within(card).getByRole("button", { name: "Ship it" })).toBeInTheDocument();
    expect(Array.from(card.querySelectorAll(".comment-author")).map((author) => author.textContent)).toEqual(["Carol", "Bob", "Maya"]);
    expect(card.querySelector(".person-mention")).toHaveTextContent("@Maya");
    expect(card.querySelector(".person-mention")).toHaveClass("is-me");
    expect(commentsToggle()).toHaveAttribute("aria-pressed", "true");

    // The other page has its own thread.
    await user.click(button(panel, t("comments.close")));
    expect(screen.queryByRole("complementary", { name: t("comments.panel") })).not.toBeInTheDocument();
  });

  it("says so when the page has no comment, and lists the resolved threads when asked", async () => {
    const { user } = await renderComments({ threads: THREADS.filter((thread) => thread.page === "Plan" && thread.resolved !== undefined) });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    expect(await within(panel).findByText(t("comments.empty.resolved"))).toBeInTheDocument();
    expect(threadCards(panel)).toHaveLength(0);
    await user.click(within(panel).getByRole("checkbox", { name: t("comments.showResolved") }));
    const card = threadWith(panel, "needs an owner");
    expect(card).toHaveClass("is-resolved");
    expect(within(card).getByText(t("comments.resolvedBy", { name: "Bob" }))).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: t("comments.reopen") })).toBeInTheDocument();
  });

  it("starts empty on a page without comments and follows the core's events: a thread another person starts shows at once", async () => {
    const { user, memory } = await renderComments();
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    expect(await within(panel).findByText(t("comments.empty"))).toBeInTheDocument();
    act(() => {
      memory.receiveComments({ type: "post", pageId: ids.page("Plan"), author: "Bob", body: "Fresh from Bob" });
    });
    expect(await within(panel).findByText("Fresh from Bob")).toBeInTheDocument();
    await waitFor(() => {
      expect(commentsToggle()).toHaveAccessibleName(toggleLabel(1));
    });
    // A comment on another page does not touch this one.
    act(() => {
      memory.receiveComments({ type: "post", pageId: ids.page("Notes"), author: "Bob", body: "Elsewhere" });
    });
    expect(within(panel).queryByText("Elsewhere")).not.toBeInTheDocument();
  });
});

describe("writing", () => {
  it("starts a comment on the page from the page menu or the panel, and shows the thread under the author's name", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("comments.menu.page") }));
    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    const field = await within(panel).findByRole("textbox", { name: t("comments.new.field") });
    expect(field).toHaveFocus();
    // Empty text cannot be sent.
    expect(within(panel).getByRole("button", { name: t("comments.new.send") })).toBeDisabled();
    await user.type(field, "A note for everyone");
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));

    const card = await waitFor(() => threadWith(panel, "A note for everyone"));
    expect(within(card).getByText("Maya")).toBeInTheDocument();
    // The draft is gone, "New comment on the page" is back, and it is a page-level thread (no anchor).
    expect(within(panel).queryByRole("textbox", { name: t("comments.new.field") })).not.toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: t("comments.newOnPage") })).toBeInTheDocument();
    const [thread] = await client.commentList(ids.page("Plan"));
    expect(thread?.anchor).toBeUndefined();
    expect(commentsToggle()).toHaveAccessibleName(toggleLabel(1));
  });

  it("sends with Enter, breaks the line with Shift+Enter and cancels with Escape", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await user.click(await within(panel).findByRole("button", { name: t("comments.newOnPage") }));
    const field = within(panel).getByRole("textbox", { name: t("comments.new.field") });
    await user.type(field, "Line one{Shift>}{Enter}{/Shift}Line two{Enter}");
    await waitFor(async () => {
      expect((await client.commentList(ids.page("Plan")))[0]?.comments[0]?.body).toEqual([{ type: "text", text: "Line one\nLine two" }]);
    });
    await user.click(within(panel).getByRole("button", { name: t("comments.newOnPage") }));
    await user.keyboard("{Escape}");
    expect(within(panel).queryByRole("textbox", { name: t("comments.new.field") })).not.toBeInTheDocument();
  });

  it("replies in a thread", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    await user.click(within(card).getByRole("button", { name: t("comments.reply") }));
    await user.type(within(card).getByRole("textbox", { name: t("comments.reply.label") }), "Thanks all");
    await user.click(within(card).getByRole("button", { name: t("comments.reply.send") }));
    await waitFor(() => {
      expect(threadWith(panel, "Friday")).toHaveTextContent("Thanks all");
    });
    const thread = (await client.commentList(ids.page("Plan"))).find((entry) => entry.anchor !== undefined);
    expect(thread?.comments).toHaveLength(4);
    expect(thread?.comments[3]?.isMine).toBe(true);
  });

  it("edits the comment of this account, with its mention kept", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    // Bob's comment has a mention of Maya: it is not this account's to edit.
    expect(within(commentWith(card, "Friday works")).queryByRole("button", { name: t("comments.edit") })).not.toBeInTheDocument();
    const mine = commentWith(card, "Confirmed: Friday.");
    await user.click(within(mine).getByRole("button", { name: t("comments.edit") }));
    const field = within(mine).getByRole("textbox", { name: t("comments.edit.label") });
    expect(field).toHaveValue("Confirmed: Friday.");
    await user.clear(field);
    await user.type(field, "Confirmed: Friday at ten");
    await user.click(within(mine).getByRole("button", { name: t("comments.save") }));
    await waitFor(() => {
      expect(commentWith(threadWith(panel, "Friday"), "Friday at ten")).toBeInTheDocument();
    });
    expect(within(commentWith(threadWith(panel, "Friday"), "Friday at ten")).getByText(t("comments.edited"))).toBeInTheDocument();
    const thread = (await client.commentList(ids.page("Plan"))).find((entry) => entry.anchor !== undefined);
    expect(thread?.comments[2]?.body).toEqual([{ type: "text", text: "Confirmed: Friday at ten" }]);
    expect(thread?.comments[2]?.editedAt).toBeDefined();
  });

  it("puts back the mention of a comment when it is edited, and sends it as a mention", async () => {
    const { user, client, memory } = await renderComments({
      threads: [{ page: "Plan", comments: [{ body: ["Over to ", { mention: "Bob" }, ", please"] }] }],
    });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const mine = await waitFor(() => commentWith(threadWith(panel, "Over to"), "please"));
    await user.click(within(mine).getByRole("button", { name: t("comments.edit") }));
    const field = within(mine).getByRole("textbox", { name: t("comments.edit.label") });
    expect(field).toHaveValue("Over to @Bob, please");
    await user.type(field, " thanks");
    await user.click(within(mine).getByRole("button", { name: t("comments.save") }));
    await waitFor(async () => {
      const body = (await client.commentList(ids.page("Plan")))[0]?.comments[0]?.body;
      expect(body).toEqual([
        { type: "text", text: "Over to " },
        { type: "mention", accountId: (await client.peopleMentionable((await client.spaceList()).find((space) => space.name === "Work")?.id ?? "")).find((person) => person.name === "Bob")?.accountId },
        { type: "text", text: ", please thanks" },
      ]);
    });
  });

  it("deletes a comment after asking, for good: a deleted root stays as a note while a reply follows", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    const root = commentWith(card, "Can we ship this on Friday?");
    await user.click(within(root).getByRole("button", { name: t("comments.delete") }));
    // Asked first; "Cancel" leaves it.
    const confirm = within(root).getByRole("group", { name: t("comments.delete.confirmLabel") });
    await user.click(within(confirm).getByRole("button", { name: t("comments.cancel") }));
    expect(root).toHaveTextContent("Can we ship this on Friday?");
    await user.click(within(root).getByRole("button", { name: t("comments.delete") }));
    await user.click(within(within(root).getByRole("group", { name: t("comments.delete.confirmLabel") })).getByRole("button", { name: t("comments.delete.yes") }));
    await waitFor(() => {
      expect(within(threadWith(panel, "Friday")).getByText(t("comments.deleted"))).toBeInTheDocument();
    });
    expect(within(threadWith(panel, "Friday")).queryByText("Can we ship this on Friday?")).not.toBeInTheDocument();
    const thread = (await client.commentList(ids.page("Plan"))).find((entry) => entry.anchor !== undefined);
    expect(thread?.comments[0]).toMatchObject({ deleted: true, body: [] });
    // Nothing left to edit or delete on a deleted comment.
    const placeholder = threadWith(panel, "Friday").querySelector("li.comment.is-deleted") as HTMLElement;
    expect(within(placeholder).queryByRole("button")).not.toBeInTheDocument();
  });

  it("resolves a thread, which leaves the list until \"show resolved\", and reopens it", async () => {
    const { user } = await renderComments({ threads: THREADS });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    await user.click(within(card).getByRole("button", { name: t("comments.resolve") }));
    await waitFor(() => {
      expect(threadCards(panel)).toHaveLength(0);
    });
    expect(commentsToggle()).toHaveAccessibleName(toggleLabel(0));
    await user.click(within(panel).getByRole("checkbox", { name: t("comments.showResolved") }));
    const resolved = threadWith(panel, "Friday");
    expect(within(resolved).getByText(t("comments.resolvedBy", { name: "Maya" }))).toBeInTheDocument();
    await user.click(within(resolved).getByRole("button", { name: t("comments.reopen") }));
    await waitFor(() => {
      expect(threadWith(panel, "Friday")).not.toHaveClass("is-resolved");
    });
    expect(commentsToggle()).toHaveAccessibleName(toggleLabel(1));
  });

  it("says why the core refused a comment and keeps the words", async () => {
    const { user, memory } = await renderComments();
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await user.click(await within(panel).findByRole("button", { name: t("comments.newOnPage") }));
    const field = within(panel).getByRole("textbox", { name: t("comments.new.field") });
    await user.type(field, "Will not go");
    memory.failNext({ code: "Forbidden", message: "this needs the commenter role in the space" });
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent(
      t("comments.error.save", { message: "this needs the commenter role in the space" }),
    );
    expect(field).toHaveValue("Will not go");
  });
});

describe("the words of a refusal, from the shape the core gives", () => {
  it("says that only the author edits, when the core refuses an edit with {reason: author}", async () => {
    const { user, memory } = await renderComments({ threads: THREADS });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    // Only this account's own comment has an Edit button.
    const own = (await within(panel).findAllByRole("button", { name: t("comments.edit") }))[0] as HTMLElement;
    const row = own.closest("li") as HTMLElement;
    await user.click(own);
    await user.type(within(row).getByRole("textbox", { name: t("comments.edit.label") }), "!");
    memory.failNext({ code: "Forbidden", message: "only the author of a comment may change it", details: { reason: "author" } });
    await user.click(within(row).getByRole("button", { name: t("comments.save") }));
    expect(await within(row).findByRole("alert")).toHaveTextContent(t("comments.error.save", { message: t("comments.refused.author") }));
  });

  it("says that deleting asks for the author or an Owner, when the core asks for the Owner role", async () => {
    const { user, memory } = await renderComments({ threads: THREADS });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const root = commentWith(await waitFor(() => threadWith(panel, "Friday")), "Can we ship this on Friday?");
    await user.click(within(root).getByRole("button", { name: t("comments.delete") }));
    memory.failNext({ code: "Forbidden", message: "this needs the owner role in the space", details: { reason: "role", required: "owner" } });
    await user.click(within(within(root).getByRole("group", { name: t("comments.delete.confirmLabel") })).getByRole("button", { name: t("comments.delete.yes") }));
    expect(await within(root).findByRole("alert")).toHaveTextContent(t("comments.error.action", { message: t("comments.refused.delete") }));
  });

  it("says that resolving asks for the thread's author or an Editor, when the core asks for the Editor role", async () => {
    const { user, memory } = await renderComments({ threads: THREADS });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    memory.failNext({ code: "Forbidden", message: "this needs the editor role in the space", details: { reason: "role", required: "editor" } });
    await user.click(within(card).getByRole("button", { name: t("comments.resolve") }));
    expect(await within(card).findByRole("alert")).toHaveTextContent(t("comments.error.action", { message: t("comments.refused.resolve") }));
  });

  it("names the role of any other refusal by role, and a read-only copy by its own words", async () => {
    const { user, memory } = await renderComments();
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await user.click(await within(panel).findByRole("button", { name: t("comments.newOnPage") }));
    const field = within(panel).getByRole("textbox", { name: t("comments.new.field") });
    await user.type(field, "Not allowed");
    memory.failNext({ code: "Forbidden", message: "this needs the commenter role in the space", details: { reason: "role", required: "commenter" } });
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent(
      t("comments.error.save", { message: t("comments.refused.role", { role: t("sharing.role.commenter") }) }),
    );
    memory.failNext({ code: "Forbidden", message: "this space is a read-only copy", details: { reason: "archived" } });
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));
    await waitFor(() => {
      expect(within(panel).getByRole("alert")).toHaveTextContent(t("comments.error.save", { message: t("comments.readOnly.archive") }));
    });
  });
});

describe("the @ people picker of a comment", () => {
  it("lists the members that match, picks one with Enter, and sends the name as a mention", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await user.click(await within(panel).findByRole("button", { name: t("comments.newOnPage") }));
    const field = within(panel).getByRole("textbox", { name: t("comments.new.field") });
    await user.type(field, "Hello @Bo");
    const list = await within(panel).findByRole("listbox", { name: t("comments.mention.label") });
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["BBob"]);
    await user.keyboard("{Enter}");
    expect(field).toHaveValue("Hello @Bob ");
    expect(within(panel).queryByRole("listbox", { name: t("comments.mention.label") })).not.toBeInTheDocument();
    await user.type(field, "can you look?");
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));

    const bob = (await client.peopleMentionable((await client.spaceList()).find((space) => space.name === "Work")?.id ?? "")).find((person) => person.name === "Bob");
    await waitFor(async () => {
      expect((await client.commentList(ids.page("Plan")))[0]?.comments[0]?.body).toEqual([
        { type: "text", text: "Hello " },
        { type: "mention", accountId: bob?.accountId },
        { type: "text", text: " can you look?" },
      ]);
    });
    // The chip of the thread is the member's current name.
    const card = threadWith(panel, "can you look?");
    expect(card.querySelector(".person-mention")).toHaveTextContent("@Bob");
  });

  it("offers everyone for a bare @, picks by click, closes with Escape, and a name that was edited stays plain text", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await user.click(await within(panel).findByRole("button", { name: t("comments.newOnPage") }));
    const field = within(panel).getByRole("textbox", { name: t("comments.new.field") });
    await user.type(field, "@");
    const list = await within(panel).findByRole("listbox", { name: t("comments.mention.label") });
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["MMayayou", "BBob", "CCarol", "DDan"]);
    await user.keyboard("{Escape}");
    expect(within(panel).queryByRole("listbox", { name: t("comments.mention.label") })).not.toBeInTheDocument();
    // Escape closed the picker only: the composer is still there.
    expect(field).toBeInTheDocument();

    await user.type(field, "{Backspace}@Ca");
    await user.click(within(await within(panel).findByRole("listbox", { name: t("comments.mention.label") })).getByRole("option", { name: /Carol/u }));
    expect(field).toHaveValue("@Carol ");
    // Edit the name after it was picked: it is not a mention any more.
    await user.type(field, "{Backspace}{Backspace}x then");
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));
    await waitFor(async () => {
      expect((await client.commentList(ids.page("Plan")))[0]?.comments[0]?.body).toEqual([{ type: "text", text: "@Carox then" }]);
    });
  });

  it("is not used in an address: a word before @ keeps it a plain character", async () => {
    const { user } = await renderComments();
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await user.click(await within(panel).findByRole("button", { name: t("comments.newOnPage") }));
    await user.type(within(panel).getByRole("textbox", { name: t("comments.new.field") }), "write to ben@exa");
    expect(within(panel).queryByRole("listbox", { name: t("comments.mention.label") })).not.toBeInTheDocument();
  });
});

describe("what each role may do", () => {
  const MATRIX: Record<MemberRole, { write: boolean; deleteOthers: boolean; resolveOthers: boolean }> = {
    owner: { write: true, deleteOthers: true, resolveOthers: true },
    editor: { write: true, deleteOthers: false, resolveOthers: true },
    commenter: { write: true, deleteOthers: false, resolveOthers: false },
    viewer: { write: false, deleteOthers: false, resolveOthers: false },
  };

  for (const [role, may] of Object.entries(MATRIX) as [MemberRole, (typeof MATRIX)[MemberRole]][]) {
    it(`as a ${role}: ${may.write ? "writes comments" : "reads them only"}, edits only its own, deletes ${may.deleteOthers ? "any" : "its own"}, resolves ${may.resolveOthers ? "any thread" : "its own threads"}`, async () => {
      const { user } = await renderComments({ role, threads: THREADS });
      await openWorkPage(user, "Plan");
      const panel = await openPanel(user);
      const card = await waitFor(() => threadWith(panel, "Friday"));
      const others = commentWith(card, "Can we ship this on Friday?");
      const mine = commentWith(card, "Confirmed: Friday.");
      // Never the comment of another: edit.
      expect(within(others).queryByRole("button", { name: t("comments.edit") })).not.toBeInTheDocument();
      expect(!!within(others).queryByRole("button", { name: t("comments.delete") })).toBe(may.deleteOthers);
      // This account's own comment (written when it was an Editor, say): the buttons follow the role's right to comment.
      expect(!!within(mine).queryByRole("button", { name: t("comments.edit") })).toBe(may.write);
      expect(!!within(mine).queryByRole("button", { name: t("comments.delete") })).toBe(may.write);
      // The thread was started by Carol: resolving it is for an Editor and up.
      expect(!!within(card).queryByRole("button", { name: t("comments.resolve") })).toBe(may.resolveOthers);
      expect(!!within(card).queryByRole("button", { name: t("comments.reply") })).toBe(may.write);
      expect(!!within(panel).queryByRole("button", { name: t("comments.newOnPage") })).toBe(may.write);
      expect(!!within(panel).queryByText(t("comments.readOnly.viewer"))).toBe(!may.write);
      // The page menu offers a comment on the page to whoever may.
      await user.click(screen.getByRole("button", { name: t("page.actions") }));
      const menu = await screen.findByRole("menu", { name: t("page.actions") });
      expect(!!within(menu).queryByRole("menuitem", { name: t("comments.menu.page") })).toBe(may.write);
    });
  }

  it("lets a Commenter resolve a thread that it started", async () => {
    const { user } = await renderComments({
      role: "commenter",
      threads: [{ page: "Plan", comments: [{ body: "Mine from the start" }, { author: "Bob", body: "A reply" }] }],
    });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Mine from the start"));
    await user.click(within(card).getByRole("button", { name: t("comments.resolve") }));
    await waitFor(() => {
      expect(threadCards(panel)).toHaveLength(0);
    });
  });

  it("reads the comments of a read-only copy and writes none", async () => {
    const { user } = await renderComments({ role: "editor", archive: "Closed", threads: THREADS });
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    expect(within(panel).getByText(t("comments.readOnly.archive"))).toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: t("comments.reply") })).not.toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: t("comments.resolve") })).not.toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: t("comments.delete") })).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: t("comments.newOnPage") })).not.toBeInTheDocument();
  });

  it("holds the panel's buttons back when the role changes under it", async () => {
    const { user, memory, client } = await renderComments({ role: "editor", threads: THREADS });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    expect(within(card).getByRole("button", { name: t("comments.resolve") })).toBeInTheDocument();
    const me = (await client.spaceMembers(ids.space("Work"))).members.find((member) => member.isMe);
    act(() => {
      memory.receiveSharing({ type: "roleSet", spaceId: ids.space("Work"), accountId: me?.accountId ?? "", role: "viewer" });
    });
    await waitFor(() => {
      expect(within(threadWith(panel, "Friday")).queryByRole("button", { name: t("comments.resolve") })).not.toBeInTheDocument();
    });
    expect(within(panel).getByText(t("comments.readOnly.viewer"))).toBeInTheDocument();
  });
});

describe("people who left", () => {
  it("shows an author who was removed as a former member, and keeps what they wrote", async () => {
    const { user, memory, client } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    const card = await waitFor(() => threadWith(panel, "Friday"));
    expect(within(commentWith(card, "Friday works")).getByText("Bob")).toBeInTheDocument();
    const bob = (await client.spaceMembers(ids.space("Work"))).members.find((member) => member.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "removed", spaceId: ids.space("Work"), accountId: bob?.accountId ?? "" });
    });
    await waitFor(() => {
      const row = commentWith(threadWith(panel, "Friday"), "Friday works");
      // The core keeps the last name Bob had, and the author reads "Former member (Bob)".
      expect(within(row).getByText(t("comments.formerMember.label", { label: "Bob" }))).toBeInTheDocument();
    });
    const row = commentWith(threadWith(panel, "Friday"), "Friday works");
    expect(within(row).queryByText("Bob")).not.toBeInTheDocument();
    expect(row).toHaveTextContent("Friday works");
  });
});

describe("where it shows", () => {
  it("has no panel for the Home of a space (a new tab), and the panel is back with the page", async () => {
    const { user } = await renderComments({ threads: THREADS });
    await openWorkPage(user, "Plan");
    await openPanel(user);
    expect(screen.getByRole("complementary", { name: t("comments.panel") })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("tabs.new") }));
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("comments.panel") })).not.toBeInTheDocument();
    });
  });
});
