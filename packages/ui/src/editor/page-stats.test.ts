// What "Page info" counts (ADR-0040): words, characters and top-level blocks of the editor's document.

import type { JsonValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { pid } from "../test/editor";
import { countCharacters, countWords, pageStats } from "./page-stats";
import { createEditorSchema } from "./schema";

const schema = createEditorSchema();

let counter = 100;
const id = (): { id: string } => ({ id: pid((counter += 1)) });

const text = (value: string): JsonValue => ({ type: "text", text: value });
const bold = (value: string): JsonValue => ({ type: "text", marks: [{ type: "bold" }], text: value });

function paragraph(...content: JsonValue[]): JsonValue {
  return { type: "paragraph", attrs: id(), ...(content.length === 0 ? {} : { content }) };
}

/** The statistics of a document made of `blocks`. */
function statsOf(...blocks: JsonValue[]) {
  return pageStats(schema.nodeFromJSON({ type: "doc", content: blocks }));
}

describe("countWords", () => {
  it("splits on any whitespace and ignores the ends", () => {
    expect(countWords("")).toBe(0);
    expect(countWords("   \n\t ")).toBe(0);
    expect(countWords("one")).toBe(1);
    expect(countWords("  one  two\tthree\nfour five ")).toBe(5);
    expect(countWords("well-known e.g. don't")).toBe(3);
  });
});

describe("countCharacters", () => {
  it("counts code points, spaces included and line breaks not", () => {
    expect(countCharacters("")).toBe(0);
    expect(countCharacters("a b")).toBe(3);
    expect(countCharacters("a\nb\r\nc d")).toBe(4);
    expect(countCharacters("é🎯")).toBe(2);
  });
});

describe("pageStats", () => {
  it("counts an empty page as one empty block: it is what the editor holds", () => {
    expect(statsOf(paragraph())).toEqual({ words: 0, characters: 0, blocks: 1 });
  });

  it("counts the words and characters of the text, marks and all, and the top-level blocks", () => {
    expect(
      statsOf(
        { type: "heading", attrs: { ...id(), level: 1 }, content: [text("Plan")] },
        paragraph(text("Ship "), bold("soon"), text(", then rest.")),
        paragraph(),
      ),
    ).toEqual({ words: 5, characters: "Plan".length + "Ship soon, then rest.".length, blocks: 3 });
  });

  it("never joins the end of a block to the start of the next one", () => {
    expect(statsOf(paragraph(text("end")), paragraph(text("start")))).toMatchObject({ words: 2, characters: 8 });
  });

  it("separates words at a line break and at an inline chip, and counts neither as a character", () => {
    const chip: JsonValue = { type: "pageRef", attrs: { ...id(), pageId: pid(7) } };
    expect(statsOf(paragraph(text("one"), { type: "hardBreak" }, text("two")))).toMatchObject({ words: 2, characters: 6 });
    expect(statsOf(paragraph(text("see"), chip, text("now")))).toMatchObject({ words: 2, characters: 6 });
  });

  it("counts what is nested (lists, toggles, callouts, table cells) in its words but once in the blocks", () => {
    const list: JsonValue = {
      type: "bulletList",
      attrs: id(),
      content: [
        { type: "listItem", attrs: id(), content: [paragraph(text("first item")), { type: "bulletList", attrs: id(), content: [{ type: "listItem", attrs: id(), content: [paragraph(text("nested"))] }] }] },
        { type: "listItem", attrs: id(), content: [paragraph(text("second item"))] },
      ],
    };
    const toggle: JsonValue = {
      type: "details",
      attrs: id(),
      content: [
        { type: "detailsSummary", attrs: id(), content: [text("Summary here")] },
        { type: "detailsContent", attrs: id(), content: [paragraph(text("hidden words"))] },
      ],
    };
    const callout: JsonValue = { type: "callout", attrs: { ...id(), icon: "💡", color: "default" }, content: [paragraph(text("note this"))] };
    const table: JsonValue = {
      type: "table",
      attrs: id(),
      content: [
        {
          type: "tableRow",
          attrs: id(),
          content: [
            { type: "tableHeader", attrs: id(), content: [paragraph(text("Head"))] },
            { type: "tableCell", attrs: id(), content: [paragraph(text("Cell one"))] },
          ],
        },
      ],
    };
    expect(statsOf(list, toggle, callout, table)).toEqual({
      words: 2 + 1 + 2 + 2 + 2 + 2 + 1 + 2,
      characters: "first item".length + "nested".length + "second item".length + "Summary here".length + "hidden words".length + "note this".length + "Head".length + "Cell one".length,
      blocks: 4,
    });
  });

  it("counts code as text, its newlines not as characters", () => {
    const code: JsonValue = { type: "codeBlock", attrs: { ...id(), language: null }, content: [text("let a = 1;\nlet b = 2;")] };
    expect(statsOf(code)).toEqual({ words: 8, characters: "let a = 1;".length + "let b = 2;".length, blocks: 1 });
  });

  it("counts a block with no text (a rule, a table of contents) as a block", () => {
    expect(statsOf({ type: "horizontalRule", attrs: id() }, paragraph(text("x")))).toEqual({ words: 1, characters: 1, blocks: 2 });
  });
});
