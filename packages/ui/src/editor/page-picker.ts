// `[[` opens the page picker: titles across spaces as you type; Enter inserts a page
// reference (`pageRef`, the live chip). The "Link to page" command of the `/` menu opens the
// same picker in card mode (`openPagePicker`): Enter then inserts a `pageLink` block, the card.

import { Extension } from "@tiptap/core";
import type { Editor, Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import { Suggestion } from "@tiptap/suggestion";

import type { PageChoice } from "../store-content";
import { insertPageLinkCard } from "./insert-page-link";
import type { SuggestionMenuStore } from "./suggestion-menu";

export interface PagePickerOptions {
  /** Where the picker shows; `null` disables it. */
  store: SuggestionMenuStore<PageChoice> | null;
  /** The pages to offer for what was typed after `[[`. */
  findPages: (query: string) => Promise<PageChoice[]>;
}

export const pagePickerKey = new PluginKey("pagePicker");

export interface PagePickerStorage {
  /**
   * The picker that is open (or about to open) inserts a card, not a chip. Set by
   * `openPagePicker`, read and cleared when a page is chosen, cleared when the picker closes.
   */
  asBlock: boolean;
}

declare module "@tiptap/core" {
  interface Storage {
    pagePicker: PagePickerStorage;
  }
}

/**
 * Opens the page picker where the typed `/query` was, in card mode: choosing a page inserts
 * a `pageLink` block (the card) instead of the inline chip `[[` gives. The picker is the
 * `[[` one: this deletes the range and types the trigger.
 */
export function openPagePicker(editor: Editor, range: Range, asBlock: boolean): void {
  // Absent in editors built without the picker: the `[[` is then plain text.
  const storage = (editor.storage as unknown as { pagePicker?: PagePickerStorage }).pagePicker;
  if (storage !== undefined) {
    storage.asBlock = asBlock;
  }
  editor.chain().focus().deleteRange(range).insertContent("[[").run();
}

/** Waits this long after the last keystroke before searching. */
const SEARCH_DEBOUNCE_MS = 150;

export const PagePicker = Extension.create<PagePickerOptions, PagePickerStorage>({
  name: "pagePicker",

  addOptions() {
    return { store: null, findPages: () => Promise.resolve([]) };
  },

  addStorage() {
    return { asBlock: false };
  },

  addProseMirrorPlugins() {
    const { store, findPages } = this.options;
    const { storage } = this;
    if (store === null) {
      return [];
    }
    return [
      Suggestion<PageChoice, PageChoice>({
        editor: this.editor,
        pluginKey: pagePickerKey,
        char: "[[",
        allowSpaces: true,
        allowedPrefixes: null,
        debounce: SEARCH_DEBOUNCE_MS,
        items: async ({ query }) => {
          try {
            // A closing `]]` typed by habit is not part of the title.
            return await findPages(query.replace(/\]+$/, ""));
          } catch {
            return [];
          }
        },
        command: ({ editor, range, props }) => {
          const asBlock = storage.asBlock;
          storage.asBlock = false;
          if (asBlock) {
            editor.chain().focus().deleteRange(range).run();
            insertPageLinkCard(editor, { pageId: props.pageId });
            return;
          }
          editor
            .chain()
            .focus()
            .insertContentAt(range, [
              { type: "pageRef", attrs: { pageId: props.pageId } },
              { type: "text", text: " " },
            ])
            .run();
        },
        allow: ({ state, range }) => {
          if (storage.asBlock) {
            return true; // a card goes wherever a block does: `insertPageLinkCard` finds the place
          }
          const $from = state.doc.resolve(range.from);
          const pageRef = state.schema.nodes.pageRef;
          return pageRef !== undefined && $from.parent.canReplaceWith($from.index(), $from.index(), pageRef);
        },
        render: () => {
          const renderer = store.renderer();
          return {
            ...renderer,
            onExit: (props) => {
              storage.asBlock = false; // closed without a choice: the next `[[` is a chip again
              renderer.onExit?.(props);
            },
          };
        },
      }),
    ];
  },
});
