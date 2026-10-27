// The inbox of notifications (Milestone 13, slice 2; ADR-0048 and ADR-0049) over the in-memory core, which raises them from the changes
// of the other people only: the bell and its count, the inbox with who did what on which page (never the words of a comment), a row
// that opens what it is about at the anchor and is marked read, "mark all as read", and the toast of a notification that arrives.

import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t, tp } from "./i18n";
import { THREADS, openPanel, openWorkPage, renderComments, threadWith } from "./test/comments";
import { idsOf } from "./test/page";
import { openSpace, toasts } from "./test/sharing";

const ROWS = [
  { kind: "CommentMention", actor: "Bob", page: "Plan", threadQuote: "Ship it", minutesAgo: 120 },
  { kind: "PersonAssigned", actor: "Carol", page: "Notes", read: true, minutesAgo: 1_300 },
] as const;

function bell(): HTMLElement {
  return screen.getByRole("button", { name: new RegExp(`^${t("inbox.open")}`, "u") });
}

async function openInbox(user: Awaited<ReturnType<typeof renderComments>>["user"]): Promise<HTMLElement> {
  await user.click(bell());
  return screen.findByRole("dialog", { name: t("inbox.title") });
}

describe("the bell", () => {
  it("has no badge while there is nothing unread, and counts what is unread", async () => {
    const quiet = await renderComments({ threads: THREADS });
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(t("inbox.open"));
    });
    expect(bell().querySelector(".notifications-badge")).toBeNull();
    quiet.unmount();

    await renderComments({ threads: THREADS, notifications: ROWS });
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(tp("inbox.openUnread", 1));
    });
    expect(bell().querySelector(".notifications-badge")).toHaveTextContent("1");
  });

  it("lists who did what on which page, newest first, with the unread ones marked, and never the words of a comment", async () => {
    const { user } = await renderComments({ threads: THREADS, notifications: ROWS });
    const inbox = await openInbox(user);
    const list = await within(inbox).findByRole("list", { name: t("inbox.list") });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((row) => row.querySelector(".inbox-text")?.textContent)).toEqual([
      "Bob mentioned you in a comment on Plan",
      "Carol assigned you on Notes",
    ]);
    expect(rows[0]).toHaveClass("is-unread");
    expect(rows[1]).not.toHaveClass("is-unread");
    expect(inbox).not.toHaveTextContent("Can we ship");
    // An unread row has a button of its own; a read one has none.
    expect(within(rows[0] as HTMLElement).getByRole("button", { name: t("inbox.markRead.label", { text: "Bob mentioned you in a comment on Plan" }) })).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).queryByRole("button", { name: t("inbox.markRead") })).not.toBeInTheDocument();
  });

  it("opens the page at the comment of a row and marks it read", async () => {
    const { user, memory } = await renderComments({ threads: THREADS, notifications: ROWS });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    const inbox = await openInbox(user);
    await user.click(within(inbox).getByRole("button", { name: /^Bob mentioned you in a comment on Plan/u }));

    // The page, the comments panel and the thread of the comment in focus; the inbox is closed.
    expect(await screen.findByRole("heading", { level: 1, name: "Plan" })).toBeInTheDocument();
    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    await waitFor(() => {
      expect(threadWith(panel, "Friday")).toHaveClass("is-focused");
    });
    expect(screen.queryByRole("dialog", { name: t("inbox.title") })).not.toBeInTheDocument();
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(t("inbox.open"));
    });
    expect(ids.page("Plan")).not.toBe("");
  });

  it("marks one row read, then all of them", async () => {
    const { user } = await renderComments({
      threads: THREADS,
      notifications: [...ROWS, { kind: "PageMention", actor: "Dan", page: "Plan", minutesAgo: 10 }],
    });
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(tp("inbox.openUnread", 2));
    });
    const inbox = await openInbox(user);
    await user.click(await within(inbox).findByRole("button", { name: t("inbox.markRead.label", { text: "Dan mentioned you on Plan" }) }));
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(tp("inbox.openUnread", 1));
    });
    expect(within(inbox).getByRole("button", { name: t("inbox.markAll") })).toBeEnabled();
    await user.click(within(inbox).getByRole("button", { name: t("inbox.markAll") }));
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(t("inbox.open"));
    });
    expect(within(inbox).getByRole("button", { name: t("inbox.markAll") })).toBeDisabled();
    expect(within(inbox).getAllByRole("listitem").every((row) => !row.classList.contains("is-unread"))).toBe(true);
  });

  it("says so when there is nothing, and closes with Escape", async () => {
    const { user } = await renderComments();
    const inbox = await openInbox(user);
    expect(within(inbox).getByText(t("inbox.empty"))).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("inbox.title") })).not.toBeInTheDocument();
  });
});

describe("a notification that arrives", () => {
  it("is a toast that says who did what and where, with an Open button that goes to the thread, and a row of the inbox", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    const thread = (await client.commentList(ids.page("Plan"))).find((entry) => entry.anchor !== undefined);
    act(() => {
      memory.receiveComments({ type: "post", pageId: ids.page("Plan"), author: "Bob", threadId: thread?.threadId, body: "A secret reply" });
    });
    const toast = await within(toasts()).findByText("Bob replied to your thread on Plan");
    // Never the words of the comment.
    expect(toasts()).not.toHaveTextContent("secret");
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(tp("inbox.openUnread", 1));
    });

    await user.click(within(toast.closest(".toast") as HTMLElement).getByRole("button", { name: t("comments.notify.open") }));
    expect(await screen.findByRole("heading", { level: 1, name: "Plan" })).toBeInTheDocument();
    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    await waitFor(() => {
      expect(within(threadWith(panel, "Friday")).getByText("A secret reply")).toBeInTheDocument();
    });
    await waitFor(() => {
      expect(threadWith(panel, "Friday")).toHaveClass("is-focused");
    });
    // Opened from the toast, the row is read.
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(t("inbox.open"));
    });
  });

  it("says a mention in a comment as a mention, and raises nothing for a comment of this account", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    // This account's own comment, with a mention of itself, notifies nobody.
    await client.commentPost({ pageId: ids.page("Plan"), body: [{ type: "text", text: "A note to myself" }] });
    await waitFor(() => {
      expect(threadWith(panel, "A note to myself")).toBeInTheDocument();
    });
    expect(within(toasts()).queryAllByText(/./u)).toHaveLength(0);
    expect(bell()).toHaveAccessibleName(t("inbox.open"));

    act(() => {
      memory.receiveComments({ type: "post", pageId: ids.page("Notes"), author: "Carol", body: [{ mention: "Maya" }, " have a look"] });
    });
    expect(await within(toasts()).findByText("Carol mentioned you in a comment on Notes")).toBeInTheDocument();
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(tp("inbox.openUnread", 1));
    });
  });

  it("is one mention, not a reply too, when a reply in a thread of this account mentions it", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    const thread = (await client.commentList(ids.page("Plan"))).find((entry) => entry.anchor !== undefined);
    act(() => {
      memory.receiveComments({
        type: "post",
        pageId: ids.page("Plan"),
        author: "Bob",
        threadId: thread?.threadId,
        body: [{ mention: "Maya" }, " can you look?"],
      });
    });
    expect(await within(toasts()).findByText("Bob mentioned you in a comment on Plan")).toBeInTheDocument();
    expect(within(toasts()).queryByText("Bob replied to your thread on Plan")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(bell()).toHaveAccessibleName(tp("inbox.openUnread", 1));
    });
    const inbox = await openInbox(user);
    const rows = within(await within(inbox).findByRole("list", { name: t("inbox.list") })).getAllByRole("listitem");
    expect(rows.map((row) => row.querySelector(".inbox-text")?.textContent)).toEqual(["Bob mentioned you in a comment on Plan"]);
  });

  it("names an actor who has left the space \"Former member (name)\", in the toast and in the inbox", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS, notifications: ROWS });
    const ids = await idsOf(memory);
    const bob = (await client.spaceMembers(ids.space("Work"))).members.find((member) => member.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "removed", spaceId: ids.space("Work"), accountId: bob?.accountId ?? "" });
    });
    const former = t("comments.formerMember.label", { label: "Bob" });
    const inbox = await openInbox(user);
    expect(await within(inbox).findByText(`${former} mentioned you in a comment on Plan`)).toBeInTheDocument();
    // The name the space gives the people who stayed is as it was.
    expect(within(inbox).getByText("Carol assigned you on Notes")).toBeInTheDocument();
  });

  it("is told with the title of the page, \"Untitled\" for a page without one", async () => {
    const { user, client, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    await client.pageRename(ids.page("Notes"), "");
    act(() => {
      memory.receiveComments({ type: "post", pageId: ids.page("Notes"), author: "Carol", body: [{ mention: "Maya" }] });
    });
    expect(await within(toasts()).findByText(`Carol mentioned you in a comment on ${t("page.untitled")}`)).toBeInTheDocument();
  });
});
