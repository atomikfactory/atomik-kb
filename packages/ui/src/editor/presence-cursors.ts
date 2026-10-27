// The other members' cursors in the editor (Milestone 13, slice 3; ADR-0050): the vendored cursor plugin on the page's presence
// store (`page-presence.ts`), as a Tiptap extension. It draws a caret with the member's name and the range they selected, in
// their colour, and writes this editor's selection to the store. Without a presence (a personal space's page on a transport
// that has none, the schema-only editors of the tests) it adds nothing.

import { Extension } from "@tiptap/core";

import { LoroEphemeralCursorPlugin } from "./loro-binding";
import type { PagePresence } from "./page-presence";

export interface PresenceCursorsOptions {
  /** The page's presence; `null` adds nothing. */
  presence: PagePresence | null;
}

export const PresenceCursors = Extension.create<PresenceCursorsOptions>({
  name: "presenceCursors",
  // Tiptap lists the plugins of the extensions by priority, the highest first, and the extensions of one priority in the reverse of
  // the order they are given in. The cursor plugin reads the state of the sync plugin, which a state field only sees once the sync
  // plugin has applied the transaction: it must come after `loroBinding` (priority 100) in the list, so its priority is lower.
  priority: 90,

  addOptions() {
    return { presence: null };
  },

  addProseMirrorPlugins() {
    const { presence } = this.options;
    return presence === null ? [] : [LoroEphemeralCursorPlugin(presence.store)];
  },
});
