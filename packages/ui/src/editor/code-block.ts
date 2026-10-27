// The code block: block schema v1's `codeBlock` (content `text*`, attributes `id` and
// `language`) with offline syntax highlighting and a language picker.
//
// Highlighting is a ProseMirror decoration plugin: it only styles what the document
// already holds and never writes to it, so nothing is stored or synced. The grammars are
// lowlight's "common" set, bundled with the app (no remote assets, the desktop CSP allows
// only local resources). The picker is a native `<select>` in the block's node view;
// choosing a language changes the `language` attribute and nothing else
// (`tr.setNodeAttribute`), so the block `id` is never dropped.
//
// A block whose language is `mermaid` keeps its source editable and also shows the drawn
// diagram below it: a preview in the node view, outside the editable text, drawn offline
// by `./mermaid` after the text pauses. Like the highlighting it is view state only;
// nothing but the block's `language` and text is stored.

import type { NodeViewRendererProps } from "@tiptap/core";
import { CodeBlockLowlight } from "@tiptap/extension-code-block-lowlight";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { NodeView, ViewMutationRecord } from "@tiptap/pm/view";
import { common, createLowlight } from "lowlight";

import { activeLocale, t } from "../i18n";
import { currentDiagramTheme, onDiagramThemeChange, renderDiagram } from "./mermaid";

const grammars = createLowlight(common);

/** The attribute holding a block's id (`BLOCK_ID_ATTRIBUTE` in `schema.ts`, which imports this file). */
const ID_ATTRIBUTE = "id";

/**
 * What the extension is configured with: the common grammars, but no auto-detection
 * (`highlightAuto` finds nothing). Tiptap's own plugin guesses a language for a block
 * without one, which would colour "Plain text" as some random language, and runs every
 * grammar over the text to do it. A block shows the language it names, or none.
 */
const highlighter = {
  highlight: (language: string, code: string) => grammars.highlight(language, code),
  highlightAuto: (): ReturnType<typeof grammars.highlightAuto> => ({ type: "root", children: [] }),
  listLanguages: () => grammars.listLanguages(),
  registered: (language: string) => grammars.registered(language),
};

/** Longer code is shown plain: highlighting runs again on every edit of the block. */
const MAX_HIGHLIGHTED_LENGTH = 50_000;

/** The part of a highlight tree (`hast`) that the highlighting reads. */
interface HighlightNode {
  type: string;
  value?: string;
  properties?: { className?: unknown };
  children?: readonly HighlightNode[];
}

/** A styled stretch of a code block's text, as offsets into that text. */
interface HighlightSpan {
  from: number;
  to: number;
  className: string;
}

/**
 * Adds the styled stretches of `nodes` (a highlight result's children) to `spans`, the
 * text starting at offset `start`; returns the offset after it.
 */
function collectSpans(
  nodes: readonly HighlightNode[],
  inherited: readonly string[],
  start: number,
  spans: HighlightSpan[],
): number {
  let at = start;
  for (const node of nodes) {
    const own = node.properties?.className;
    const classes = Array.isArray(own) ? [...inherited, ...(own as unknown[]).map(String)] : inherited;
    if (node.children !== undefined) {
      at = collectSpans(node.children, classes, at, spans);
    } else {
      const length = node.value?.length ?? 0;
      if (length > 0 && classes.length > 0) {
        spans.push({ from: at, to: at + length, className: classes.join(" ") });
      }
      at += length;
    }
  }
  return at;
}

/** The styled stretches of a block's text in `language`; none for plain text or an unknown language. */
function highlightSpans(language: unknown, text: string): HighlightSpan[] {
  if (typeof language !== "string" || language === "" || text.length > MAX_HIGHLIGHTED_LENGTH) {
    return [];
  }
  if (!grammars.registered(language)) {
    return [];
  }
  try {
    const spans: HighlightSpan[] = [];
    collectSpans(grammars.highlight(language, text).children, [], 0, spans);
    return spans;
  } catch {
    return []; // a grammar that fails on odd input leaves the block plain
  }
}

/** The spans of a block, by node: nodes are immutable, so the same node always has the same spans. */
const spansOfNode = new WeakMap<PmNode, HighlightSpan[]>();

function spansOf(node: PmNode): HighlightSpan[] {
  let spans = spansOfNode.get(node);
  if (spans === undefined) {
    spans = highlightSpans(node.attrs.language, node.textContent);
    spansOfNode.set(node, spans);
  }
  return spans;
}

function highlightDecorations(doc: PmNode, name: string): DecorationSet {
  const decorations: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (node.type.name !== name) {
      return node.isBlock; // inline nodes (and text) hold no code block
    }
    // A code block holds text only, so offsets in its text are offsets in the document.
    for (const { from, to, className } of spansOf(node)) {
      decorations.push(Decoration.inline(pos + 1 + from, pos + 1 + to, { class: className }));
    }
    return false;
  });
  return DecorationSet.create(doc, decorations);
}

const highlightKey = new PluginKey<DecorationSet>("codeBlockHighlight");

/**
 * The highlighting: inline decorations over the text of every code block, from the block's
 * own `language` and text, so a pick in the picker, a remote change and an undo all show at
 * once (Tiptap's plugin only looks again while the caret is in a code block, and would
 * leave the old colours after a pick). View state only: nothing reaches the document.
 * Blocks whose node did not change keep their spans.
 */
function highlightPlugin(name: string): Plugin<DecorationSet> {
  return new Plugin<DecorationSet>({
    key: highlightKey,
    state: {
      init: (_config, { doc }) => highlightDecorations(doc, name),
      apply: (tr, decorations) => (tr.docChanged ? highlightDecorations(tr.doc, name) : decorations),
    },
    props: {
      decorations: (state) => highlightKey.getState(state),
    },
  });
}

/** How the common languages are shown in the picker; any other is shown by its grammar name. */
const LANGUAGE_LABELS: Readonly<Record<string, string>> = {
  arduino: "Arduino",
  bash: "Bash",
  c: "C",
  cpp: "C++",
  csharp: "C#",
  css: "CSS",
  diff: "Diff",
  go: "Go",
  graphql: "GraphQL",
  ini: "INI / TOML",
  java: "Java",
  javascript: "JavaScript",
  json: "JSON",
  kotlin: "Kotlin",
  less: "Less",
  lua: "Lua",
  makefile: "Makefile",
  markdown: "Markdown",
  objectivec: "Objective-C",
  perl: "Perl",
  php: "PHP",
  "php-template": "PHP template",
  python: "Python",
  "python-repl": "Python REPL",
  r: "R",
  ruby: "Ruby",
  rust: "Rust",
  scss: "SCSS",
  shell: "Shell session",
  sql: "SQL",
  swift: "Swift",
  typescript: "TypeScript",
  vbnet: "Visual Basic .NET",
  wasm: "WebAssembly",
  xml: "HTML / XML",
  yaml: "YAML",
};

/**
 * Names that Markdown fences and the "```lang " rule commonly carry, mapped to the picker
 * entry that shows them (the highlighting itself resolves aliases on its own). `""` is
 * "Plain text". Display only: the stored `language` is never rewritten.
 */
const LANGUAGE_ALIASES: Readonly<Record<string, string>> = {
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  ts: "typescript",
  tsx: "typescript",
  py: "python",
  rb: "ruby",
  rs: "rust",
  golang: "go",
  sh: "bash",
  zsh: "bash",
  html: "xml",
  svg: "xml",
  xhtml: "xml",
  yml: "yaml",
  toml: "ini",
  "c++": "cpp",
  cc: "cpp",
  hpp: "cpp",
  h: "c",
  cs: "csharp",
  "c#": "csharp",
  kt: "kotlin",
  md: "markdown",
  objc: "objectivec",
  pl: "perl",
  gql: "graphql",
  make: "makefile",
  console: "shell",
  patch: "diff",
  mermaid: "mermaid", // the picker entry for "Mermaid" and any other casing
  text: "",
  txt: "",
  plain: "",
  plaintext: "",
};

interface PickerChoice {
  value: string;
  label: string;
}

/**
 * The language of diagram blocks. It is no highlighting grammar (the text stays plain);
 * blocks in it show the drawn diagram below their source.
 */
export const MERMAID_LANGUAGE = "mermaid";

/**
 * Every language of the common grammars but `plaintext` (that is "Plain text"), and
 * Mermaid, by label.
 */
function pickerChoices(): PickerChoice[] {
  const choices = grammars
    .listLanguages()
    .filter((name) => name !== "plaintext")
    .map((name) => ({ value: name, label: LANGUAGE_LABELS[name] ?? name }));
  choices.push({ value: MERMAID_LANGUAGE, label: "Mermaid" });
  return choices.sort((a, b) => a.label.localeCompare(b.label, activeLocale(), { sensitivity: "base" }));
}

/** The picker value that shows `language`: `""` for plain text, else a choice or the raw name. */
export function pickerValueFor(language: unknown): string {
  if (typeof language !== "string" || language === "") {
    return "";
  }
  const key = language.toLowerCase();
  return Object.hasOwn(LANGUAGE_ALIASES, key) ? (LANGUAGE_ALIASES[key] ?? "") : language;
}

/** How long the source must rest before its diagram is drawn again. */
const PREVIEW_DEBOUNCE_MS = 300;

/** A Mermaid message can run to many lines (a parse error quotes the source): the start tells enough. */
const MAX_ERROR_LENGTH = 400;

/** Mermaid's reason for a diagram it cannot draw, trimmed to a short message. */
function describeDiagramError(error: unknown): string {
  const message = (error instanceof Error ? error.message : String(error)).trim();
  return message.length > MAX_ERROR_LENGTH ? `${message.slice(0, MAX_ERROR_LENGTH).trimEnd()}...` : message;
}

/** The drawn diagram of a `mermaid` block. */
interface DiagramPreview {
  readonly element: HTMLElement;
  /** The source changed: draw it again once typing pauses. */
  changed: () => void;
  destroy: () => void;
}

/**
 * The preview below a diagram block's source: `div.code-block-preview`, not editable. It
 * draws `sourceOf()` at once, again `PREVIEW_DEBOUNCE_MS` after each `changed()` and when
 * the theme changes. The last good diagram stays while the next one is drawn; a source
 * that does not parse shows Mermaid's message in its place, an empty one a hint. The SVG
 * markup comes from `renderDiagram` only (sanitised by Mermaid's strict security level).
 */
function createDiagramPreview(sourceOf: () => string): DiagramPreview {
  const element = document.createElement("div");
  element.className = "code-block-preview";
  element.setAttribute("contenteditable", "false");
  element.setAttribute("role", "group");
  element.setAttribute("aria-label", t("codeBlock.diagramLabel"));

  let timer: ReturnType<typeof setTimeout> | undefined;
  /** Counts the draws: a result is shown only while it is the latest one's. */
  let latest = 0;
  let theme = currentDiagramTheme();

  const place = (state: string, ...content: Node[]): void => {
    element.dataset.state = state;
    delete element.dataset.busy;
    element.replaceChildren(...content);
  };
  const note = (state: "loading" | "empty", text: string): void => {
    const hint = document.createElement("p");
    hint.className = "code-block-preview-hint";
    hint.textContent = text;
    place(state, hint);
  };
  const failed = (reason: string): void => {
    const error = document.createElement("div");
    error.className = "code-block-preview-error";
    const title = document.createElement("p");
    title.textContent = t("codeBlock.diagramError");
    error.append(title);
    if (reason !== "") {
      const detail = document.createElement("div");
      detail.className = "code-block-preview-detail";
      detail.textContent = reason;
      error.append(detail);
    }
    place("error", error);
  };
  const drawn = (svg: string): void => {
    const diagram = document.createElement("div");
    diagram.className = "code-block-diagram";
    diagram.innerHTML = svg; // Mermaid's own output (strict security level), never the document's text
    place("ready", diagram);
  };

  const draw = (): void => {
    clearTimeout(timer);
    latest += 1;
    const mine = latest;
    const source = sourceOf();
    theme = currentDiagramTheme();
    if (source.trim() === "") {
      note("empty", t("codeBlock.diagramEmpty"));
      return;
    }
    if (element.firstElementChild === null) {
      note("loading", t("codeBlock.diagramLoading"));
    }
    // A diagram being drawn again (the theme changed) keeps the old one on show; print mode waits for the new one.
    element.dataset.busy = "";
    renderDiagram(source, theme).then(
      (svg) => {
        if (mine === latest) {
          drawn(svg);
        }
      },
      (error: unknown) => {
        if (mine === latest) {
          failed(describeDiagramError(error));
        }
      },
    );
  };

  const stopWatching = onDiagramThemeChange(() => {
    if (currentDiagramTheme() !== theme) {
      draw();
    }
  });
  draw();

  return {
    element,
    changed: () => {
      clearTimeout(timer);
      timer = setTimeout(draw, PREVIEW_DEBOUNCE_MS);
    },
    destroy: () => {
      clearTimeout(timer);
      latest += 1; // a draw still running shows nothing
      stopWatching();
      element.remove();
    },
  };
}

type CodeBlockViewProps = Pick<NodeViewRendererProps, "node" | "editor" | "getPos">;

/**
 * The block's view: `div.code-block` with the picker and `pre > code`, which holds the text
 * (and the highlighting spans). `code` keeps the `language-<x>` class the schema renders.
 * A `mermaid` block has the diagram preview after the `pre`.
 */
function createCodeBlockView({ node: initial, editor, getPos }: CodeBlockViewProps, classPrefix: string): NodeView {
  let node = initial;

  const dom = document.createElement("div");
  dom.className = "code-block";

  const select = document.createElement("select");
  select.className = "code-block-language";
  select.setAttribute("contenteditable", "false");
  select.title = t("codeBlock.language");
  select.setAttribute("aria-label", t("codeBlock.language"));
  const plain = document.createElement("option");
  plain.value = "";
  plain.textContent = t("codeBlock.plainText");
  select.append(plain);
  const known = new Set<string>([""]);
  for (const { value, label } of pickerChoices()) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    select.append(option);
    known.add(value);
  }
  // A language the picker does not list (an import, a newer version): shown as it is stored.
  let unlisted: HTMLOptionElement | null = null;

  const pre = document.createElement("pre");
  const code = document.createElement("code");
  pre.append(code);
  dom.append(select, pre);

  // The diagram of a `mermaid` block, after its source; drawn again when the text changes.
  let preview: DiagramPreview | null = null;
  let previewSource = "";
  const syncPreview = (): void => {
    if (pickerValueFor(node.attrs.language) !== MERMAID_LANGUAGE) {
      preview?.destroy();
      preview = null;
      return;
    }
    const source = node.textContent;
    if (preview === null) {
      preview = createDiagramPreview(() => node.textContent);
      pre.after(preview.element);
    } else if (source !== previewSource) {
      preview.changed();
    }
    previewSource = source;
  };

  const show = (): void => {
    const language: unknown = node.attrs.language;
    const id: unknown = node.attrs[ID_ATTRIBUTE];
    if (typeof id === "string") {
      dom.setAttribute("data-id", id);
    } else {
      dom.removeAttribute("data-id");
    }
    code.className = typeof language === "string" && language !== "" ? classPrefix + language : "";
    const value = pickerValueFor(language);
    if (known.has(value)) {
      unlisted?.remove();
      unlisted = null;
    } else {
      unlisted ??= document.createElement("option");
      unlisted.value = value;
      unlisted.textContent = value;
      select.append(unlisted);
    }
    select.value = value;
    select.disabled = !editor.isEditable;
    syncPreview();
  };
  show();

  select.addEventListener("change", () => {
    const pos = getPos();
    if (!editor.isEditable || pos === undefined) {
      show();
      return;
    }
    // The one attribute only: a full attrs object would drop the block's id.
    const language = select.value === "" ? null : select.value;
    editor.view.dispatch(editor.state.tr.setNodeAttribute(pos, "language", language));
  });

  return {
    dom,
    contentDOM: code,
    update: (updated: PmNode) => {
      if (updated.type !== node.type) {
        return false;
      }
      node = updated;
      show();
      return true;
    },
    // The picker works like any form control: ProseMirror leaves its events alone, so a
    // click never moves the caret into the code and the keys reach the control.
    stopEvent: (event: Event) => event.target instanceof Node && select.contains(event.target),
    // Only the text (`code`) is the document's: the picker and the diagram preview are
    // this view's own, so what changes in them (a drawn SVG, a new option) is no edit.
    ignoreMutation: (mutation: ViewMutationRecord) => !code.contains(mutation.target),
    destroy: () => {
      preview?.destroy();
      preview = null;
    },
  };
}

/**
 * `codeBlock` with highlighting and the picker. The options are StarterKit's code block
 * defaults (the "```lang " rule, exit on triple Enter and on the arrow keys); the lowlight
 * extension adds no attribute, so the stored shape stays `id` and `language`.
 */
export const CodeBlock = CodeBlockLowlight.extend({
  addProseMirrorPlugins() {
    // What the code block inherits (the paste handler for VS Code), except Tiptap's own
    // highlighting, which `highlightPlugin` replaces.
    const inherited = (this.parent?.() ?? []).filter((plugin) => plugin.spec.props?.decorations === undefined);
    return [...inherited, highlightPlugin(this.name)];
  },

  addNodeView() {
    const classPrefix = this.options.languageClassPrefix ?? "language-";
    return (props: NodeViewRendererProps) => createCodeBlockView(props, classPrefix);
  },
}).configure({ lowlight: highlighter, defaultLanguage: null });
