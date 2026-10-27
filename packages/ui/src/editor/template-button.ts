// The template button (ADR-0056, `docs/architecture/templates.md` section 9): a block `templateButton { id, label,
// position }` whose content is blocks (`block+`, like a callout). The page shows a button, "+ label", and a fold-out
// "Edit content" with the blocks it holds (`TemplateButtonView.tsx`); a click inserts a fresh copy of those blocks right
// below the button (or above it, `position`) as one change of the page, so one undo step takes it away.
//
// The click is `template-button-copy.ts`. The copy is made like a duplicated block: block ids removed so that `UniqueID`
// gives new ones, a drawing's elements following its new id (`whiteboard-copy.ts`). Then the values that were left to be
// filled in are: a date mention with `relative: "today"` takes the local day, one with `"now"` the day and the time (and
// each loses `relative`), a person mention with `relative: "me"` takes the account and the name of whoever clicks (the
// people of the space), and also loses it. The caret goes to the end of the inserted content.
//
// The content is template material, not the page's own: the core leaves it out of the page's text, its links (backlinks),
// its date mentions (reminders) and its person mentions (`kb-model`'s readers and their TypeScript twins). The Markdown
// and HTML exports write nothing for it; the Markdown file flavour (ADR-0036) writes it between two annotations
// (`{"node":"templateButton",...}` and `{"end":"templateButton"}`) so that an import gives the button back.

import type { PersonInfo, SpaceId } from "@atomik-kb/core-client";
import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";
import type { EditorState } from "@tiptap/pm/state";

/** The node's name in the schema. */
export const TEMPLATE_BUTTON_NODE = "templateButton";

/** The longest label a button keeps, in characters. */
export const TEMPLATE_BUTTON_LABEL_MAX_CHARS = 80;

/** Where a click inserts the copy. */
export type TemplateButtonPosition = "below" | "above";

/** What a date mention can be left to be filled in with (the `relative` attribute). */
export type DateRelative = "today" | "now";

/** What a person mention can be left to be filled in with (the `relative` attribute). */
export type PersonRelative = "me";

/** A date mention's `relative`, read from whatever the document holds: the two names, else `null`. */
export function dateRelativeOf(value: unknown): DateRelative | null {
  return value === "today" || value === "now" ? value : null;
}

/** A person mention's `relative`, read from whatever the document holds: `me`, else `null`. */
export function personRelativeOf(value: unknown): PersonRelative | null {
  return value === "me" ? value : null;
}

/** Where a button inserts, read from its `position` (anything but `above` is below). */
export function positionOf(value: unknown): TemplateButtonPosition {
  return value === "above" ? "above" : "below";
}

/** A button's label as stored: a string of at most {@link TEMPLATE_BUTTON_LABEL_MAX_CHARS} characters, `""` otherwise. */
export function labelOf(value: unknown): string {
  return typeof value === "string" ? Array.from(value).slice(0, TEMPLATE_BUTTON_LABEL_MAX_CHARS).join("") : "";
}

export interface TemplateButtonOptions {
  /** How the editor shows a button (its view); `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
  /** The space of the page: its people are who "me" can be (`null`: none known, a relative person mention stays as it is). */
  spaceId: SpaceId | null;
  /** The people of the space, read once when a click needs this account; the one with `isMe` is who clicks. */
  findPeople: () => Promise<readonly PersonInfo[]>;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    templateButton: {
      /**
       * Inserts a template button holding one empty paragraph where the selection is (in place of an empty paragraph, after
       * a selected block, or splitting a paragraph), the caret in that paragraph. Refuses a place inside a template button.
       */
      insertTemplateButton: () => ReturnType;
    };
  }
}

/** Whether the selection sits inside a template button (a button holds no button: the commands refuse). */
export function insideTemplateButton(state: EditorState): boolean {
  const { $from } = state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    if ($from.node(depth).type.name === TEMPLATE_BUTTON_NODE) {
      return true;
    }
  }
  return false;
}

/**
 * A button: a block that holds blocks (block schema v1, `templateButton` with `content: block+` and the attributes `label` and
 * `position`). In HTML it is `<div data-type="templateButton" data-label data-position>` with the button and a content element.
 */
export const TemplateButton = Node.create<TemplateButtonOptions>({
  name: TEMPLATE_BUTTON_NODE,
  group: "block",
  content: "block+",
  defining: true,
  // Backspace at the start of the content and the like stay inside the button: the content is not unwrapped by accident.
  isolating: true,

  addOptions() {
    return { nodeView: null, spaceId: null, findPeople: () => Promise.resolve([]) };
  },

  addAttributes() {
    return {
      label: {
        default: "",
        parseHTML: (element) => labelOf(element.getAttribute("data-label")),
        renderHTML: (attributes) => ({ "data-label": labelOf(attributes.label) }),
      },
      position: {
        default: "below",
        parseHTML: (element) => positionOf(element.getAttribute("data-position")),
        renderHTML: (attributes) => ({ "data-position": positionOf(attributes.position) }),
      },
    };
  },

  parseHTML() {
    return [{ tag: `div[data-type="${TEMPLATE_BUTTON_NODE}"]`, contentElement: "div.template-button-content" }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      "div",
      mergeAttributes({ "data-type": TEMPLATE_BUTTON_NODE, class: "template-button" }, HTMLAttributes),
      ["div", { class: "template-button-content" }, 0],
    ];
  },

  addNodeView() {
    return this.options.nodeView;
  },

  addCommands() {
    return {
      insertTemplateButton:
        () =>
        ({ chain, state }) =>
          !insideTemplateButton(state) &&
          chain()
            .insertContent({ type: this.name, attrs: { label: "", position: "below" }, content: [{ type: "paragraph" }] })
            .run(),
    };
  },
});
