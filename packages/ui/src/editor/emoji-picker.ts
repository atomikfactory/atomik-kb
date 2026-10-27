// `:` opens the emoji picker: emoji by shortcode as you type (`:smi` finds `:smile:`, `:+1`
// the thumbs up), in the shared suggestion menu. Enter or Tab puts the emoji in the text in
// place of the `:query`; it is plain text, no node, so the Loro document holds one character
// (or a short sequence) and nothing else. Typing the closing colon of a shortcode that names an
// emoji exactly (`:tada:`) does the same at once, and Backspace right after it brings the
// shortcode back. The `/emoji` command of the `/` menu opens the same picker (`openEmojiPicker`).
//
// The picker opens after two typed characters, so that a colon in prose (`Note: x`, `at :30`)
// shows no menu, and only after whitespace or at the start of a block, never in code (a code
// block, the math block, inline code). The emoji data is a lazily loaded chunk (emoji-data.ts).

import { Extension, InputRule } from "@tiptap/core";
import type { Editor, Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import type { EditorState } from "@tiptap/pm/state";
import { Suggestion } from "@tiptap/suggestion";

import { emojiForCode, loadEmoji, searchEmoji } from "./emoji-data";
import type { EmojiChoice } from "./emoji-data";
import { pagePickerKey } from "./page-picker";
import type { SuggestionMenuStore } from "./suggestion-menu";

/** The picker lists at most this many emoji. */
export const MAX_EMOJI_RESULTS = 8;

/** Typed characters after the colon before the picker opens by itself. */
export const MIN_EMOJI_QUERY = 2;

export interface EmojiPickerOptions {
  /** Where the picker shows; `null` disables it (and the closing-colon shortcut). */
  store: SuggestionMenuStore<EmojiChoice> | null;
}

export interface EmojiPickerStorage {
  /**
   * The picker was opened by `/emoji`: it shows from the bare colon on (the default list),
   * wherever the colon stands. Set by `openEmojiPicker`, cleared when the picker closes.
   */
  opened: boolean;
}

declare module "@tiptap/core" {
  interface Storage {
    emojiPicker: EmojiPickerStorage;
  }
}

interface PickerState {
  active: boolean;
}

export const emojiPickerKey = new PluginKey<PickerState>("emojiPicker");

/** The text `leafText` gives an inline node that is not text, for what stands before a colon. */
function leafText(node: { type: { name: string } }): string {
  return node.type.name === "hardBreak" ? "\n" : "￼";
}

/** Whether the colon at `from` starts a word: first in its block, after a line break, or after whitespace. */
function startsWord(state: EditorState, from: number): boolean {
  if (state.doc.resolve(from).parentOffset === 0) {
    return true;
  }
  return /\s/u.test(state.doc.textBetween(from - 1, from, "", leafText));
}

/**
 * Whether the `[[` page picker is open: its query (a page title) may hold ` :ab` and is no
 * shortcode. Read from the editor's state, the one before the transaction being applied: the
 * picker's own state is not up to date yet when this plugin runs (Tiptap runs the plugins of
 * the extensions in reverse order), and it opened with an earlier keystroke anyway.
 */
function pagePickerOpen(editor: Editor): boolean {
  const picker = pagePickerKey.getState(editor.state) as { active?: boolean } | undefined;
  return picker?.active === true;
}

/** Whether `range` is code: in a code block (the math block is one), or inline code. */
function inCode(state: EditorState, range: Range): boolean {
  if (state.doc.resolve(range.from).parent.type.spec.code === true) {
    return true;
  }
  const code = state.schema.marks.code;
  return code !== undefined && state.doc.rangeHasMark(range.from, range.to, code);
}

/**
 * Opens the picker where the typed `/query` was: deletes it and types the trigger, and the
 * picker shows its default list at once (it needs no typed characters then). Absent in editors
 * built without the picker: the colon is then plain text.
 */
export function openEmojiPicker(editor: Editor, range: Range): void {
  const storage = (editor.storage as unknown as { emojiPicker?: EmojiPickerStorage }).emojiPicker;
  if (storage !== undefined) {
    storage.opened = true;
  }
  editor.chain().focus().deleteRange(range).insertContent(":").run();
  // The picker stays shut where it cannot open (in code): do not leave the next colon open.
  if (storage !== undefined && emojiPickerKey.getState(editor.state)?.active !== true) {
    storage.opened = false;
  }
}

/** A shortcode between colons at the end of the text before the caret, after whitespace or at the start. */
const CLOSED_SHORTCODE = /(?:^|\s):([^\s:]+):$/u;

/**
 * Finds `:code:` at the end of `text` (the text before the caret with the typed colon) when
 * `code` names an emoji exactly. Needs the emoji data loaded: the picker loads it as soon as
 * the first character after the colon is typed.
 */
function findClosedShortcode(text: string) {
  const found = CLOSED_SHORTCODE.exec(text);
  const code = found?.[1];
  if (found === null || code === undefined) {
    return null;
  }
  const emoji = emojiForCode(code);
  if (emoji === null) {
    return null;
  }
  const start = found.index + found[0].length - code.length - 2;
  return { index: start, text: text.slice(start), data: { emoji } };
}

export const EmojiPicker = Extension.create<EmojiPickerOptions, EmojiPickerStorage>({
  name: "emojiPicker",

  addOptions() {
    return { store: null };
  },

  addStorage() {
    return { opened: false };
  },

  addInputRules() {
    if (this.options.store === null) {
      return [];
    }
    return [
      new InputRule({
        find: findClosedShortcode,
        handler: ({ state, range, match }) => {
          const emoji: unknown = match.data?.emoji;
          if (typeof emoji !== "string") {
            return null;
          }
          // The range holds `:code` and the typed colon.
          state.tr.insertText(emoji, range.from, range.to);
          return undefined;
        },
      }),
    ];
  },

  addProseMirrorPlugins() {
    const { store } = this.options;
    const { storage } = this;
    if (store === null) {
      return [];
    }
    return [
      Suggestion<EmojiChoice, EmojiChoice>({
        editor: this.editor,
        pluginKey: emojiPickerKey,
        char: ":",
        allowSpaces: false,
        allowedPrefixes: null, // `allow` below is stricter: whitespace or the start of a block
        items: async ({ query }) => {
          try {
            return await searchEmoji(query, MAX_EMOJI_RESULTS);
          } catch {
            return []; // the data could not be loaded: no menu, the text stays as typed
          }
        },
        command: ({ editor, range, props }) => {
          // Text, so it takes the marks of the words around it, and one Loro insert.
          editor.chain().focus().insertContentAt(range, { type: "text", text: props.emoji }).run();
        },
        allow: ({ editor, state, range }) =>
          !pagePickerOpen(editor) && !inCode(state, range) && (storage.opened || startsWord(state, range.from)),
        shouldShow: ({ query }) => {
          if (query !== "") {
            void loadEmoji().catch(() => undefined); // ready by the time the closing colon is typed
          }
          return storage.opened || query.length >= MIN_EMOJI_QUERY;
        },
        render: () => {
          const renderer = store.renderer();
          return {
            ...renderer,
            // The menu is hidden while nothing matches (a colon in prose): its keys are the editor's then.
            onKeyDown: (props) => {
              const listed = (store.getSnapshot()?.items.length ?? 0) > 0;
              return listed && (renderer.onKeyDown?.(props) ?? false);
            },
            onExit: (props) => {
              storage.opened = false; // closed: the next colon waits for its characters again
              renderer.onExit?.(props);
            },
          };
        },
      }),
    ];
  },
});
