// What "Page info" counts in the page the editor holds (ADR-0040): the pure part, over a ProseMirror
// document. The numbers follow the live document, not what was saved.
//
// - Words: the text of every text block (a paragraph, a heading, a list item, a table cell, a callout, a
//   code block, a toggle's summary...), each block counted on its own so that the end of one and the start
//   of the next are never one word, split on whitespace. A line break and an inline chip (a page reference,
//   a date, a formula) separate words and are not counted as one. The title is not part of the document.
// - Characters: the same text in code points (an emoji is one), spaces included, line breaks not.
// - Blocks: the blocks at the top of the page. A list, a table, a toggle or a set of columns counts once,
//   whatever it holds, and an empty paragraph counts like any other: it is what the editor holds.

import type { Node as PmNode } from "@tiptap/pm/model";

/** The counts "Page info" shows. */
export interface PageStats {
  words: number;
  characters: number;
  blocks: number;
}

/** The number of words in `text`: runs of anything but whitespace. */
export function countWords(text: string): number {
  const trimmed = text.trim();
  return trimmed === "" ? 0 : trimmed.split(/\s+/u).length;
}

/** The number of characters in `text` (code points), line breaks left out. */
export function countCharacters(text: string): number {
  let count = 0;
  for (const character of text) {
    if (character !== "\n" && character !== "\r" && character !== " " && character !== " ") {
      count += 1;
    }
  }
  return count;
}

/** The counts of the page `doc`. */
export function pageStats(doc: PmNode): PageStats {
  let words = 0;
  let characters = 0;
  doc.descendants((node) => {
    if (!node.isTextblock) {
      return true;
    }
    // Inside a text block there is only inline content: its text, with a space where a leaf stands.
    words += countWords(node.textBetween(0, node.content.size, " ", " "));
    characters += countCharacters(node.textBetween(0, node.content.size, "", ""));
    return false;
  });
  return { words, characters, blocks: doc.childCount };
}
