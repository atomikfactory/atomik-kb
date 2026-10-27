// Find in page and replace. A ProseMirror plugin holds the search (the query, match case and
// whole word), the matches as document ranges and the active one; the find bar (`FindBar.tsx`)
// reads it and drives it through the commands below.
//
// What is searched is the text of every textblock: paragraphs, headings, list items, quotes, table
// cells, columns, toggles, callouts, and the source of code and math blocks. Inside one textblock
// the adjacent text nodes (a bold word in the middle of a sentence) are searched as one text, so a
// match can run across marks; an atom (a page chip, a date mention, a line break) ends the text, and
// no match runs across it. Matches never overlap, and a block boundary ends a match too.
//
// The matches are view state: inline decorations (`find-match`, and `find-match-active` on the
// active one) over the document, which the Loro binding never sees. A replacement is an ordinary
// transaction, so the binding writes it (one update, one undo step for "Replace all") and every
// change to the document, local or remote, finds the matches again (the cost is one pass over the
// text). Closing the bar clears the search.

import { Extension } from "@tiptap/core";
import type { Editor } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey, TextSelection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";

import { loroSyncPluginKey } from "./loro-binding";

/** The class of every match. */
export const FIND_MATCH_CLASS = "find-match";

/** The class of the active match, besides {@link FIND_MATCH_CLASS}. */
export const FIND_ACTIVE_CLASS = "find-match-active";

/** A stretch of the document: positions as ProseMirror counts them. */
export interface FindMatch {
  from: number;
  to: number;
}

/** What a search is told besides its words. */
export interface FindOptions {
  /** Upper and lower case are different letters. */
  matchCase: boolean;
  /** The match is a whole word: no letter, digit or underscore next to it (checked on the side of the query that is a word character). */
  wholeWord: boolean;
}

/** A change to the search; what is left out stays as it was. */
export interface FindSearch extends Partial<FindOptions> {
  query?: string;
}

export interface FindReplaceState extends FindOptions {
  /** What is searched for; empty for no search (the bar is closed, or nothing is typed). */
  query: string;
  /** Every match, in document order. */
  matches: readonly FindMatch[];
  /** The index of the active match in `matches`; `-1` when there is none. */
  active: number;
  /** Counts how often the active match was asked to be brought into view (not when the document changes). */
  reveal: number;
  decorations: DecorationSet;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    findReplace: {
      /** Starts or changes the search; the active match is the first one at or after where the last active one was (or the selection). */
      setFind: (search: FindSearch) => ReturnType;
      /** The next match, after the last one the first. `false` without matches. */
      findNext: () => ReturnType;
      /** The previous match, before the first one the last. `false` without matches. */
      findPrevious: () => ReturnType;
      /** Replaces the active match with `text` (marks of the match's start stay) and goes on to the next. `false` when there is none, or the editor is read-only. */
      replaceActive: (text: string) => ReturnType;
      /** Replaces every match with `text` in one transaction. `false` when there is none, or the editor is read-only. */
      replaceAll: (text: string) => ReturnType;
      /** Ends the search; with `select` the active match becomes the selection (the editor is then where the match was). */
      closeFind: (options?: { select?: boolean }) => ReturnType;
    };
  }
}

type FindAction =
  | { type: "set"; search: FindSearch }
  | { type: "step"; direction: 1 | -1 }
  /** The matches were replaced: the active one is the first from `resume` on (in the new document). */
  | { type: "replaced"; resume: number }
  | { type: "clear" };

export const findReplaceKey = new PluginKey<FindReplaceState>("findReplace");

/** The search of an editor state; `null` for an editor without the extension. */
export function findStateOf(state: EditorState): FindReplaceState | null {
  return findReplaceKey.getState(state) ?? null;
}

const WORD_CHARACTER = /[\p{L}\p{M}\p{N}_]/u;

function isWordCharacter(character: string | undefined): boolean {
  return character !== undefined && WORD_CHARACTER.test(character);
}

/** The code point that ends `text` before `index`, `undefined` at the start. */
function characterBefore(text: string, index: number): string | undefined {
  if (index <= 0) {
    return undefined;
  }
  const unit = text.charCodeAt(index - 1);
  if (index >= 2 && unit >= 0xdc00 && unit <= 0xdfff) {
    const lead = text.charCodeAt(index - 2);
    if (lead >= 0xd800 && lead <= 0xdbff) {
      return text.slice(index - 2, index);
    }
  }
  return text.slice(index - 1, index);
}

/** The code point that starts at `index`, `undefined` at the end. */
function characterAt(text: string, index: number): string | undefined {
  const point = text.codePointAt(index);
  return point === undefined ? undefined : String.fromCodePoint(point);
}

/**
 * The pattern for a query: the words as they are (no wildcards), global, and for the case
 * insensitive search with the Unicode flag, whose case folding keeps every match as long as
 * the text it matched (lower-casing the text could change its length and move the positions).
 */
function patternOf(query: string, matchCase: boolean): RegExp | null {
  try {
    return new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), matchCase ? "gu" : "giu");
  } catch {
    return null;
  }
}

/**
 * Every match of `query` in the text of the document, in order. The text of a textblock is
 * searched as one, across the marks and up to the next atom (see the top of this file).
 */
export function findMatches(doc: PmNode, query: string, options: FindOptions): FindMatch[] {
  const pattern = query === "" ? null : patternOf(query, options.matchCase);
  if (pattern === null) {
    return [];
  }
  const checkBefore = options.wholeWord && isWordCharacter(characterAt(query, 0));
  const checkAfter = options.wholeWord && isWordCharacter(characterBefore(query, query.length));
  const matches: FindMatch[] = [];

  /** The matches in `text`, which starts at the document position `start`. */
  const scan = (start: number, text: string): void => {
    pattern.lastIndex = 0;
    for (let found = pattern.exec(text); found !== null; found = pattern.exec(text)) {
      const index = found.index;
      const end = index + found[0].length;
      if ((checkBefore && isWordCharacter(characterBefore(text, index))) || (checkAfter && isWordCharacter(characterAt(text, end)))) {
        // Inside a longer word: look on from the next character.
        pattern.lastIndex = index + (characterAt(text, index)?.length ?? 1);
        continue;
      }
      matches.push({ from: start + index, to: start + end });
    }
  };

  doc.descendants((node, pos) => {
    if (!node.isTextblock) {
      return true;
    }
    let text = "";
    let start = 0;
    node.forEach((child, offset) => {
      if (child.isText) {
        if (text === "") {
          start = pos + 1 + offset;
        }
        text += child.text ?? "";
      } else if (text !== "") {
        scan(start, text);
        text = "";
      }
    });
    if (text !== "") {
      scan(start, text);
    }
    return false;
  });
  return matches;
}

/** The index of the first match that starts at or after `pos`; the first match when none does; `-1` without matches. */
function firstFrom(matches: readonly FindMatch[], pos: number): number {
  if (matches.length === 0) {
    return -1;
  }
  const index = matches.findIndex((match) => match.from >= pos);
  return index < 0 ? 0 : index;
}

function decorate(doc: PmNode, matches: readonly FindMatch[], active: number): DecorationSet {
  if (matches.length === 0) {
    return DecorationSet.empty;
  }
  return DecorationSet.create(
    doc,
    matches.map((match, index) =>
      Decoration.inline(match.from, match.to, {
        class: index === active ? `${FIND_MATCH_CLASS} ${FIND_ACTIVE_CLASS}` : FIND_MATCH_CLASS,
      }),
    ),
  );
}

const NO_SEARCH: FindReplaceState = {
  query: "",
  matchCase: false,
  wholeWord: false,
  matches: [],
  active: -1,
  reveal: 0,
  decorations: DecorationSet.empty,
};

/** The state of a search that finds nothing (no query), keeping the options. */
function withoutSearch(options: FindOptions, reveal: number): FindReplaceState {
  return { ...NO_SEARCH, matchCase: options.matchCase, wholeWord: options.wholeWord, reveal };
}

/** The active match after a change: where the one before was (mapped through the change), or where `action` or the selection says. */
function activeAfter(
  previous: FindReplaceState,
  tr: Transaction,
  state: EditorState,
  matches: readonly FindMatch[],
  action: FindAction | undefined,
): number {
  if (matches.length === 0) {
    return -1;
  }
  if (action?.type === "replaced") {
    return firstFrom(matches, action.resume);
  }
  const before = previous.matches[previous.active];
  if (before === undefined) {
    return firstFrom(matches, state.selection.from);
  }
  const mapped = tr.mapping.mapResult(before.from, 1);
  // A change that took the old place away (the binding shows a remote change as one replaced
  // stretch of the whole page) keeps the index.
  return mapped.deleted ? Math.min(previous.active, matches.length - 1) : firstFrom(matches, mapped.pos);
}

function nextState(previous: FindReplaceState, tr: Transaction, state: EditorState, action: FindAction | undefined): FindReplaceState {
  if (action?.type === "clear") {
    return withoutSearch(previous, previous.reveal);
  }
  if (action?.type === "step") {
    const count = previous.matches.length;
    if (count === 0) {
      return previous;
    }
    const from = previous.active < 0 ? (action.direction > 0 ? -1 : 0) : previous.active;
    const active = (from + action.direction + count) % count;
    return { ...previous, active, reveal: previous.reveal + 1, decorations: decorate(state.doc, previous.matches, active) };
  }
  const changed = action?.type === "set";
  if (!changed && (previous.query === "" || !tr.docChanged)) {
    return previous;
  }
  const search = {
    query: changed ? (action.search.query ?? previous.query) : previous.query,
    matchCase: changed ? (action.search.matchCase ?? previous.matchCase) : previous.matchCase,
    wholeWord: changed ? (action.search.wholeWord ?? previous.wholeWord) : previous.wholeWord,
  };
  if (search.query === "") {
    return withoutSearch(search, previous.reveal);
  }
  const matches = findMatches(state.doc, search.query, search);
  const active = activeAfter(previous, tr, state, matches, action);
  return {
    ...search,
    matches,
    active,
    reveal: changed ? previous.reveal + 1 : previous.reveal,
    decorations: decorate(state.doc, matches, active),
  };
}

/** True when the editor accepts changes: editable, and the Loro binding writes them. */
function canWrite(editor: Editor, state: EditorState): boolean {
  return editor.isEditable && (loroSyncPluginKey.getState(state)?.readOnlyReason ?? null) === null;
}

/**
 * Replaces the match with `text`. The text takes the marks of the match's first character (those
 * the block allows: a code block has none).
 */
function replaceMatch(tr: Transaction, match: FindMatch, text: string): void {
  if (text === "") {
    tr.delete(match.from, match.to);
    return;
  }
  const $from = tr.doc.resolve(match.from);
  const marks = ($from.nodeAfter?.marks ?? []).filter((mark) => $from.parent.type.allowsMarkType(mark.type));
  tr.replaceWith(match.from, match.to, tr.doc.type.schema.text(text, marks));
}

/** Find in page and replace: the search, its matches drawn over the text, and the commands. */
export const FindReplace = Extension.create({
  name: "findReplace",

  addCommands() {
    return {
      setFind:
        (search) =>
        ({ tr, dispatch }) => {
          if (dispatch) {
            tr.setMeta(findReplaceKey, { type: "set", search } satisfies FindAction);
          }
          return true;
        },

      findNext:
        () =>
        ({ tr, state, dispatch }) => {
          if ((findStateOf(state)?.matches.length ?? 0) === 0) {
            return false;
          }
          if (dispatch) {
            tr.setMeta(findReplaceKey, { type: "step", direction: 1 } satisfies FindAction);
          }
          return true;
        },

      findPrevious:
        () =>
        ({ tr, state, dispatch }) => {
          if ((findStateOf(state)?.matches.length ?? 0) === 0) {
            return false;
          }
          if (dispatch) {
            tr.setMeta(findReplaceKey, { type: "step", direction: -1 } satisfies FindAction);
          }
          return true;
        },

      replaceActive:
        (text) =>
        ({ editor, tr, state, dispatch }) => {
          const find = findStateOf(state);
          const match = find?.matches[find.active];
          if (match === undefined || !canWrite(editor, state)) {
            return false;
          }
          if (dispatch) {
            replaceMatch(tr, match, text);
            tr.setMeta(findReplaceKey, { type: "replaced", resume: match.from + text.length } satisfies FindAction);
          }
          return true;
        },

      replaceAll:
        (text) =>
        ({ editor, tr, state, dispatch }) => {
          const matches = findStateOf(state)?.matches ?? [];
          if (matches.length === 0 || !canWrite(editor, state)) {
            return false;
          }
          if (dispatch) {
            // From the last match to the first, so that the positions of the ones before stay right.
            for (let index = matches.length - 1; index >= 0; index -= 1) {
              const match = matches[index];
              if (match !== undefined) {
                replaceMatch(tr, match, text);
              }
            }
            tr.setMeta(findReplaceKey, { type: "replaced", resume: 0 } satisfies FindAction);
          }
          return true;
        },

      closeFind:
        (options) =>
        ({ tr, state, dispatch }) => {
          if (dispatch) {
            const find = findStateOf(state);
            const match = find?.matches[find.active];
            if (options?.select === true && match !== undefined) {
              tr.setSelection(TextSelection.create(tr.doc, match.from, match.to)).scrollIntoView();
            }
            tr.setMeta(findReplaceKey, { type: "clear" } satisfies FindAction);
          }
          return true;
        },
    };
  },

  addProseMirrorPlugins() {
    return [
      new Plugin<FindReplaceState>({
        key: findReplaceKey,
        state: {
          init: () => NO_SEARCH,
          apply: (tr, previous, _old, state) => nextState(previous, tr, state, tr.getMeta(findReplaceKey) as FindAction | undefined),
        },
        props: {
          decorations: (state) => findStateOf(state)?.decorations ?? null,
        },
        view: () => ({
          // Brings the active match into view when it changed because the search or the active match
          // was asked to (not when the document changed under it: typing must not scroll away).
          update: (view, previous) => {
            const now = findStateOf(view.state);
            const before = findStateOf(previous);
            if (now === null || before === null || now.reveal === before.reveal || now.active < 0) {
              return;
            }
            view.dom.querySelector(`.${FIND_ACTIVE_CLASS}`)?.scrollIntoView({ block: "center", inline: "nearest" });
          },
        }),
      }),
    ];
  },
});
