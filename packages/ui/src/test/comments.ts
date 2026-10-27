// Helpers for the tests of comments, person mentions, the Person property and the inbox (Milestone 13, slice 2): the app over the
// in-memory core with the Work space shared for a role and threads seeded on its pages, the panel, and a selection of words.

import type { MemoryNotificationSeed, MemoryThreadSeed } from "@atomik-kb/core-client";
import { act, screen, within } from "@testing-library/react";
import type { Editor } from "@tiptap/core";

import { t, tp } from "../i18n";
import { findEditor } from "./page";
import type { User } from "./render";
import { clickPage, openSpace, renderSharing } from "./sharing";
import type { SharingRender } from "./sharing";

/**
 * Threads on the pages of `SHARING_SPACES` (Work): an anchored one on the words "Ship it" of Plan with three comments (Carol, Bob,
 * who mentions Maya, and this account), a resolved one on Plan as a whole, and one on Notes. Maya is this account.
 */
export const THREADS: readonly MemoryThreadSeed[] = [
  {
    page: "Plan",
    quote: "Ship it",
    comments: [
      { author: "Carol", body: "Can we ship this on Friday?", minutesAgo: 180 },
      { author: "Bob", body: ["Friday works. ", { mention: "Maya" }, " can you confirm?"], minutesAgo: 120 },
      { body: "Confirmed: Friday.", minutesAgo: 90 },
    ],
  },
  {
    page: "Plan",
    resolved: "Bob",
    comments: [
      { author: "Carol", body: "The page needs an owner.", minutesAgo: 1_500 },
      { author: "Bob", body: "Added Maya.", minutesAgo: 1_400 },
    ],
  },
  { page: "Notes", comments: [{ author: "Bob", body: "Before Friday please.", minutesAgo: 50 }] },
];

export interface CommentsRender extends Omit<SharingRender, "sharing"> {
  /** The threads of the shared Work space (none by default). */
  threads?: readonly MemoryThreadSeed[];
  /** The rows of the inbox of this device. */
  notifications?: readonly MemoryNotificationSeed[];
}

/** The app with the Work space shared for `role`, with the threads and inbox rows given. */
export async function renderComments(options: CommentsRender = {}) {
  const role = options.role === undefined || options.role === "personal" ? "owner" : options.role;
  const { threads, notifications, ...rest } = options;
  return renderSharing({
    ...rest,
    role,
    sharing: {
      profileName: "Maya",
      stepMs: 30,
      spaces: [
        {
          space: "Work",
          role,
          ...(options.archive === undefined ? {} : { archive: options.archive }),
          ...(threads === undefined ? {} : { comments: threads }),
          ...(notifications === undefined ? {} : { notifications }),
        },
      ],
    },
  });
}

/** Opens a page of the Work space and answers its editor. */
export async function openWorkPage(user: User, title: string): Promise<Editor> {
  await openSpace(user, "Work");
  await clickPage(user, title);
  return findEditor();
}

/** The button of the page header that opens and closes the comments panel. */
export function commentsToggle(): HTMLElement {
  return screen.getByRole("button", { name: new RegExp(`^${t("comments.toggle")}`, "u") });
}

/** Opens the panel from the header (when it is closed) and answers it. */
export async function openPanel(user: User): Promise<HTMLElement> {
  if (screen.queryByRole("complementary", { name: t("comments.panel") }) === null) {
    await user.click(commentsToggle());
  }
  return screen.findByRole("complementary", { name: t("comments.panel") });
}

/** The panel, when it is open. */
export function panelOf(): HTMLElement {
  return screen.getByRole("complementary", { name: t("comments.panel") });
}

/** The thread cards of the panel, in order. */
export function threadCards(panel: HTMLElement): HTMLElement[] {
  const list = within(panel).queryByRole("list", { name: t("comments.threads") });
  return list === null ? [] : Array.from(list.children).filter((child): child is HTMLElement => child instanceof HTMLElement);
}

/** The card of the thread that holds `text`. */
export function threadWith(panel: HTMLElement, text: string): HTMLElement {
  const found = threadCards(panel).find((card) => card.textContent.includes(text));
  if (found === undefined) {
    throw new Error(`no thread with ${text}`);
  }
  return found;
}

/** The comment of a thread that holds `text`. */
export function commentWith(card: HTMLElement, text: string): HTMLElement {
  const found = Array.from(card.querySelectorAll<HTMLElement>("li.comment")).find((entry) => entry.textContent.includes(text));
  if (found === undefined) {
    throw new Error(`no comment with ${text}`);
  }
  return found;
}

/** Selects the words `from` to `to` (editor positions) as a person does, so that the selection toolbar shows. */
export function selectWords(editor: Editor, from: number, to: number): void {
  editor.view.dom.tabIndex = 0; // jsdom only focuses a contenteditable that has one
  act(() => {
    editor.chain().focus().setTextSelection({ from, to }).run();
  });
}

/** "Comments (n open threads)": the label of the header button. */
export function toggleLabel(open: number): string {
  return open === 0 ? t("comments.toggle") : tp("comments.toggleCount", open);
}

/** The highlighted words of the page: the text of each decoration of a comment thread. */
export function highlighted(editor: Editor): string[] {
  return Array.from(editor.view.dom.querySelectorAll(".comment-highlight")).map((element) => element.textContent);
}
