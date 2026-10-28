import "@testing-library/jest-dom/vitest";
import { cleanup, configure } from "@testing-library/react";
import { afterEach } from "vitest";

import { setEmojiData } from "../editor/emoji-data";
import { setMathRenderer } from "../editor/math";
import { setDiagramRenderer } from "../editor/mermaid";
import { STAND_IN_EMOJI } from "./emoji";

// Mermaid is a large library that lays diagrams out with a real browser: tests draw
// `mermaid` code blocks with this stand-in and never load it (code-block.test.tsx swaps in
// its own, and has the one test that imports Mermaid itself).
setDiagramRenderer(() => Promise.resolve('<svg xmlns="http://www.w3.org/2000/svg" data-stand-in-diagram=""></svg>'));

// Likewise KaTeX and its stylesheet: tests typeset math blocks with this stand-in and never load
// them (math-block.test.tsx swaps in its own, and has the one test that imports KaTeX itself).
setMathRenderer(() => '<span class="katex" data-stand-in-math=""></span>');

// And the emoji data file (about 390 kB, behind the `:` picker and the icon picker's search):
// tests search this small stand-in (emoji-data.test.ts has the one test that loads the file).
setEmojiData(STAND_IN_EMOJI);

// Generous: CI runners and busy machines are slow, and a test should only fail for a real reason.
configure({ asyncUtilTimeout: 20_000 });

// jsdom has no DragEvent. A MouseEvent that also carries `dataTransfer` lets drag tests give
// events a position (`clientY`) and a data store, which is all the UI reads from them.
if (typeof window.DragEvent === "undefined") {
  class DragEventPolyfill extends MouseEvent {
    readonly dataTransfer: DataTransfer | null;

    constructor(type: string, init: DragEventInit = {}) {
      super(type, init);
      this.dataTransfer = init.dataTransfer ?? null;
    }
  }
  Object.defineProperty(window, "DragEvent", { configurable: true, value: DragEventPolyfill });
}

// jsdom has no layout. ProseMirror measures ranges and coordinates (selection drawing,
// scrolling, menus placed at the cursor), so give those calls an empty answer.
function emptyRect(): DOMRect {
  return { x: 0, y: 0, width: 0, height: 0, top: 0, right: 0, bottom: 0, left: 0, toJSON: () => ({}) };
}

function emptyRectList(): DOMRectList {
  const list: DOMRect[] = [];
  return Object.assign(list, { item: () => null });
}

function shim(target: object, name: string, value: unknown): void {
  if (typeof (target as Record<string, unknown>)[name] !== "function") {
    Object.defineProperty(target, name, { configurable: true, writable: true, value });
  }
}

shim(Range.prototype, "getBoundingClientRect", emptyRect);
shim(Range.prototype, "getClientRects", emptyRectList);
shim(Element.prototype, "scrollIntoView", () => undefined);
shim(Document.prototype, "elementFromPoint", () => null);

// Vitest runs without globals, so Testing Library cannot register its own cleanup.
afterEach(() => {
  cleanup();
});
