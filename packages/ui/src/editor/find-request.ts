// The request for the find bar of the open page: "Find in page" of the page menu and of the desktop shell's menu bar
// (ADR-0040) send it, the find bar (`FindBar.tsx`) of the page's editor answers it. A module of its own so that the
// store can send it without importing the editor.

/** The event `requestFindInPage` sends to the document. */
export const FIND_IN_PAGE_EVENT = "atomik-kb:find-in-page";

/** Asks the find bar of the open page, if there is one, to open. */
export function requestFindInPage(): void {
  document.dispatchEvent(new Event(FIND_IN_PAGE_EVENT));
}
