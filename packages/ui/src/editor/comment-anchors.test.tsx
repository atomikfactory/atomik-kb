// Comments anchored to the words of a page (Milestone 13, slice 2; ADR-0048), in the editor over the in-memory core: "Comment" in the
// selection toolbar (also in the read-only editor of a Commenter, who cannot write the page), the anchor it makes (a block id and
// Loro cursors), the highlights drawn as decorations, the way they follow edits of the text, a click on a highlight, and the anchor
// that cannot be resolved any more, which reads as a page-level thread that keeps its quote.

import type { JsonValue } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { THREADS, highlighted, openPanel, openWorkPage, renderComments, selectWords, threadCards, threadWith } from "../test/comments";
import { pid } from "../test/editor";
import { contentOf, idsOf } from "../test/page";

/** The first run of text of the page: the document of `SHARING_SPACES` has one paragraph. */
function firstText(doc: LoroDoc): LoroText {
  const children = doc.getMap("doc").get("children") as LoroList;
  const paragraph = children.get(0) as LoroMap;
  return (paragraph.get("children") as LoroList).get(0) as LoroText;
}

describe("Comment in the selection toolbar", () => {
  it("starts a comment on the selected words, anchored with the block and Loro cursors, and highlights them", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    const editor = await openWorkPage(user, "Plan");
    selectWords(editor, 1, 8); // "Ship it"
    const toolbar = await screen.findByRole("toolbar", { name: t("format.label") });
    // The formatting and the comment side by side, for an editor that can write.
    expect(within(toolbar).getByRole("button", { name: t("format.bold") })).toBeInTheDocument();
    await user.click(within(toolbar).getByRole("button", { name: t("comments.add") }));

    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    const draft = within(panel).getByRole("region", { name: t("comments.new.label") });
    expect(within(draft).getByText("Ship it")).toBeInTheDocument();
    // The click let the selection go, so the menu goes with it.
    await waitFor(() => {
      expect(screen.queryByRole("toolbar", { name: t("format.label") })).not.toBeInTheDocument();
    });
    await user.type(within(draft).getByRole("textbox", { name: t("comments.new.field") }), "Why Friday?");
    await user.click(within(draft).getByRole("button", { name: t("comments.new.send") }));

    const [thread] = await waitFor(async () => {
      const found = await client.commentList(ids.page("Plan"));
      expect(found).toHaveLength(1);
      return found;
    });
    expect(thread?.anchor).toMatchObject({ blockId: pid(0), quote: "Ship it" });
    expect(thread?.anchor?.start).toMatch(/^[A-Za-z0-9+/]+=*$/u);
    expect(thread?.anchor?.end).toMatch(/^[A-Za-z0-9+/]+=*$/u);
    expect(thread?.anchor?.start).not.toBe(thread?.anchor?.end);
    // The thread shows the quote as a button; the words are highlighted in the page.
    expect(within(threadWith(panel, "Why Friday?")).getByRole("button", { name: "Ship it" })).toBeInTheDocument();
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
  });

  it("lets a Commenter comment on words of a page it cannot edit: the menu has \"Comment\" alone and the page is not written", async () => {
    const { user, client, memory } = await renderComments({ role: "commenter" });
    const ids = await idsOf(memory);
    const before = await contentOf(memory, ids.page("Plan"));
    const editor = await openWorkPage(user, "Plan");
    expect(editor.isEditable).toBe(false);
    selectWords(editor, 1, 5); // "Ship"
    const toolbar = await screen.findByRole("toolbar", { name: t("format.label") });
    expect(within(toolbar).getAllByRole("button").map((control) => control.getAttribute("aria-label"))).toEqual([t("comments.add")]);
    await user.click(within(toolbar).getByRole("button", { name: t("comments.add") }));
    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    await user.type(await within(panel).findByRole("textbox", { name: t("comments.new.field") }), "Which ship?");
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));
    const [thread] = await waitFor(async () => {
      const found = await client.commentList(ids.page("Plan"));
      expect(found).toHaveLength(1);
      return found;
    });
    expect(thread?.anchor).toMatchObject({ blockId: pid(0), quote: "Ship" });
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship"]);
    });
    // Comments are not the page: the document is as it was, and the editor still takes no edit.
    expect(await contentOf(memory, ids.page("Plan"))).toEqual(before);
    expect(editor.isEditable).toBe(false);
  });

  it("offers a Viewer nothing on a selection, and a Commenter nothing on a selection of no words", async () => {
    const viewer = await renderComments({ role: "viewer", threads: THREADS });
    const editor = await openWorkPage(viewer.user, "Plan");
    selectWords(editor, 1, 8);
    expect(screen.queryByRole("toolbar", { name: t("format.label") })).not.toBeInTheDocument();
    // What it reads is highlighted all the same.
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    viewer.unmount();

    const commenter = await renderComments({ role: "commenter" });
    const reader = await openWorkPage(commenter.user, "Plan");
    selectWords(reader, 3, 3);
    expect(screen.queryByRole("toolbar", { name: t("format.label") })).not.toBeInTheDocument();
  });
});

describe("the highlights", () => {
  it("draw the words of an anchored thread from the page's load, whatever the panel does, and not the threads of the page as a whole", async () => {
    const { user } = await renderComments({ threads: THREADS });
    const editor = await openWorkPage(user, "Plan");
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    expect(editor.view.dom.querySelector(".comment-highlight")).toHaveAttribute("data-comment-thread");
    expect(editor.view.dom.querySelector(".comment-highlight")).not.toHaveClass("is-resolved");
  });

  it("show a resolved thread's words only while \"show resolved\" is on", async () => {
    const { user } = await renderComments({
      threads: [{ page: "Notes", quote: "dentist", resolved: true, comments: [{ author: "Bob", body: "Done already" }] }],
    });
    const editor = await openWorkPage(user, "Notes");
    const panel = await openPanel(user);
    await waitFor(() => {
      expect(within(panel).getByText(t("comments.empty.resolved"))).toBeInTheDocument();
    });
    expect(highlighted(editor)).toEqual([]);
    await user.click(within(panel).getByRole("checkbox", { name: t("comments.showResolved") }));
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["dentist"]);
    });
    expect(editor.view.dom.querySelector(".comment-highlight")).toHaveClass("is-resolved");
  });

  it("follow the words when the text before them changes, and the thread keeps its place", async () => {
    const { user, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    const editor = await openWorkPage(user, "Plan");
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    act(() => {
      memory.changePage(ids.page("Plan"), (doc) => {
        firstText(doc).insert(0, "So: ");
      });
    });
    await waitFor(() => {
      expect(editor.getText()).toBe("So: Ship it");
    });
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    // A local edit of the words before them too: typed into the editor, mapped by the transaction.
    act(() => {
      editor.chain().focus().insertContentAt(1, "Hey ").run();
    });
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    expect(editor.getText()).toBe("Hey So: Ship it");
  });

  it("open the thread of the words that are clicked, which takes the focus in the panel and is drawn stronger", async () => {
    const { user } = await renderComments({ threads: THREADS });
    const editor = await openWorkPage(user, "Plan");
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    expect(screen.queryByRole("complementary", { name: t("comments.panel") })).not.toBeInTheDocument();
    fireEvent.click(editor.view.dom.querySelector(".comment-highlight") as Element);
    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    await waitFor(() => {
      expect(threadWith(panel, "Friday")).toHaveClass("is-focused");
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".comment-highlight")).toHaveClass("is-active");
    });
    // Clicking the quote of a thread brings its words into focus too.
    expect(within(threadWith(panel, "Friday")).getByRole("button", { name: "Ship it" })).toBeInTheDocument();
  });
});

describe("an anchor that cannot be resolved", () => {
  it("is a page-level thread that keeps its quote, with no highlight: cursors that are not of this document", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    const editor = await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await client.commentPost({
      pageId: ids.page("Plan"),
      anchor: { blockId: pid(0), start: "AQIDBAUGBwg=", end: "AQIDBAUGBwg=", quote: "Ship it" },
      body: [{ type: "text", text: "About words that moved" }],
    });
    const card = await waitFor(() => threadWith(panel, "About words that moved"));
    expect(card).toHaveClass("is-orphan");
    expect(card.querySelector("blockquote.comment-quote")).toHaveTextContent("Ship it");
    expect(within(card).getByText(t("comments.orphan"))).toBeInTheDocument();
    // The quote is no button: there are no words to go to.
    expect(within(card).queryByRole("button", { name: "Ship it" })).not.toBeInTheDocument();
    expect(highlighted(editor)).toEqual([]);
    // It still takes replies like any thread.
    expect(within(card).getByRole("button", { name: t("comments.reply") })).toBeInTheDocument();
  });

  it("is that too when it has no cursors and no block, and a block anchor without cursors highlights the block's text", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    const editor = await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await client.commentPost({ pageId: ids.page("Plan"), anchor: { quote: "Gone words" }, body: [{ type: "text", text: "Quote only" }] });
    await client.commentPost({ pageId: ids.page("Plan"), anchor: { blockId: pid(0), quote: "Ship it" }, body: [{ type: "text", text: "A whole block" }] });
    await client.commentPost({ pageId: ids.page("Plan"), anchor: { blockId: pid(9), quote: "Removed block" }, body: [{ type: "text", text: "A block that is gone" }] });
    await waitFor(() => {
      expect(threadCards(panel)).toHaveLength(3);
    });
    expect(threadWith(panel, "Quote only")).toHaveClass("is-orphan");
    expect(threadWith(panel, "A block that is gone")).toHaveClass("is-orphan");
    expect(threadWith(panel, "A whole block")).not.toHaveClass("is-orphan");
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
  });

  it("is the case of words that were deleted: the thread becomes a page-level one with its quote", async () => {
    const { user, memory } = await renderComments({ threads: THREADS });
    const ids = await idsOf(memory);
    const editor = await openWorkPage(user, "Plan");
    const panel = await openPanel(user);
    await waitFor(() => {
      expect(highlighted(editor)).toEqual(["Ship it"]);
    });
    expect(threadWith(panel, "Friday")).not.toHaveClass("is-orphan");
    act(() => {
      memory.changePage(ids.page("Plan"), (doc) => {
        firstText(doc).delete(0, 7);
      });
    });
    await waitFor(() => {
      expect(editor.getText()).toBe("");
    });
    await waitFor(() => {
      expect(threadWith(panel, "Friday")).toHaveClass("is-orphan");
    });
    expect(highlighted(editor)).toEqual([]);
    const card = threadWith(panel, "Friday");
    expect(card.querySelector("blockquote.comment-quote")).toHaveTextContent("Ship it");
    expect(within(card).getByText(t("comments.orphan"))).toBeInTheDocument();
  });
});

describe("the page's content is not touched by comments", () => {
  it("keeps the document as it was through a thread, a reply and a resolve of an Owner", async () => {
    const { user, client, memory } = await renderComments();
    const ids = await idsOf(memory);
    const before: JsonValue = await contentOf(memory, ids.page("Plan"));
    const editor = await openWorkPage(user, "Plan");
    selectWords(editor, 1, 8);
    const toolbar = await screen.findByRole("toolbar", { name: t("format.label") });
    await user.click(within(toolbar).getByRole("button", { name: t("comments.add") }));
    const panel = await screen.findByRole("complementary", { name: t("comments.panel") });
    await user.type(await within(panel).findByRole("textbox", { name: t("comments.new.field") }), "Look");
    await user.click(within(panel).getByRole("button", { name: t("comments.new.send") }));
    const card = await waitFor(() => threadWith(panel, "Look"));
    await user.click(within(card).getByRole("button", { name: t("comments.resolve") }));
    await waitFor(async () => {
      expect((await client.commentList(ids.page("Plan")))[0]?.resolved).toBe(true);
    });
    expect(await contentOf(memory, ids.page("Plan"))).toEqual(before);
  });
});
