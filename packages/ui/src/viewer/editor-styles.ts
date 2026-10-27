// The editor's base styles for a page served under a policy with no `'unsafe-inline'` (M13 slice 4, ADR-0051): the web
// viewer's. Tiptap writes a `<style data-tiptap-style>` element with the ProseMirror rules every time it needs them, with the
// rules as the element's text, which such a policy refuses (and so is an empty one: the element gets no sheet, and the
// console reports a violation). `installEditorStyles` stops that and puts the same rules in a constructable stylesheet, which
// is CSSOM and allowed. Call it once, before the first editor is made (the app does not: it runs under no such policy).
//
// The rules are Tiptap's (`@tiptap/core`'s `style`, the ProseMirror base rules, MIT); they change with ProseMirror, rarely.

import { Editor } from "@tiptap/core";

const RULES: readonly string[] = [
  ".ProseMirror { position: relative; }",
  `.ProseMirror {
    word-wrap: break-word;
    white-space: pre-wrap;
    white-space: break-spaces;
    -webkit-font-variant-ligatures: none;
    font-variant-ligatures: none;
    font-feature-settings: "liga" 0;
  }`,
  '.ProseMirror [contenteditable="false"] { white-space: normal; }',
  '.ProseMirror [contenteditable="false"] [contenteditable="true"] { white-space: pre-wrap; }',
  ".ProseMirror pre { white-space: pre-wrap; }",
  `img.ProseMirror-separator {
    display: inline !important;
    border: none !important;
    margin: 0 !important;
    width: 0 !important;
    height: 0 !important;
  }`,
  ".ProseMirror-gapcursor { display: none; pointer-events: none; position: absolute; margin: 0; }",
  `.ProseMirror-gapcursor:after {
    content: "";
    display: block;
    position: absolute;
    top: -2px;
    width: 20px;
    border-top: 1px solid black;
    animation: ProseMirror-cursor-blink 1.1s steps(2, start) infinite;
  }`,
  "@keyframes ProseMirror-cursor-blink { to { visibility: hidden; } }",
  ".ProseMirror-hideselection *::selection { background: transparent; }",
  ".ProseMirror-hideselection *::-moz-selection { background: transparent; }",
  ".ProseMirror-hideselection * { caret-color: transparent; }",
  ".ProseMirror-focused .ProseMirror-gapcursor { display: block; }",
];

/** Whether the styles were installed (the function is idempotent). */
let installed = false;

/**
 * Has `@tiptap/core`'s editors write no `<style>` element (`Editor.prototype.injectCSS` does nothing) and adds the rules it
 * would have written to the document as a constructable stylesheet. Without `adoptedStyleSheets` (an old browser) the page
 * lacks those rules and nothing else.
 */
export function installEditorStyles(): void {
  if (installed) {
    return;
  }
  installed = true;
  // `injectCSS` is the editor's own (private) step that writes the element; the property is replaced, not assigned.
  Object.defineProperty(Editor.prototype, "injectCSS", { value: () => undefined, configurable: true, writable: true });
  if (typeof CSSStyleSheet === "undefined" || !("adoptedStyleSheets" in document)) {
    return;
  }
  const sheet = new CSSStyleSheet();
  for (const rule of RULES) {
    try {
      sheet.insertRule(rule, sheet.cssRules.length);
    } catch {
      // A rule this browser does not know (a prefixed selector): the others still apply.
    }
  }
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
}
