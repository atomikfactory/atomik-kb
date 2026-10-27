// The editor of the page in the front tab, for the comments panel (ADR-0048), which lives beside the page rather than in it: it
// asks the editor which anchored threads still have their text and brings that text into view. `PageEditor` registers its editor
// while mounted; there is at most one (only the front tab is mounted, ADR-0034). Opening it read-only is fine: a Commenter's
// editor is registered too. The editor of a peek (ADR-0054) never registers: the second editor on screen is not the page the
// panel lists the threads of, and by not registering it can neither take the slot from the page in front nor empty it when it goes.

import type { Editor } from "@tiptap/core";

import type { PageId } from "@atomik-kb/core-client";

import { Observable } from "./ai/observable";

/** The page whose editor is mounted. */
export interface CommentsHost {
  pageId: PageId;
  editor: Editor;
}

/** The mounted editor, `null` while none is (the page is loading, or no page is open). */
export const commentsHost = new Observable<CommentsHost | null>(null);

/** Registers the editor of a page. */
export function registerCommentsHost(host: CommentsHost): void {
  commentsHost.set(host);
}

/** Forgets `editor` (when it goes), unless another editor was registered since. */
export function unregisterCommentsHost(editor: Editor): void {
  if (commentsHost.get()?.editor === editor) {
    commentsHost.set(null);
  }
}
