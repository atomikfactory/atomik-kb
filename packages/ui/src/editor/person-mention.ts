// The person mention (ADR-0049, M13 slice 2): an inline atom `personMention { accountId, label }`, a
// reference to a member by account id so it survives renames. `label` is the member's display name when the
// mention was inserted (1 to 100 characters) and is a fallback only: the chip shows the member's current name
// from the space's membership profile, or "Former member (label)" when the account left. The node has no block
// id (unlike `dateMention`): nothing refers to a mention by id, the derived `person_mentions` table is keyed by
// the enclosing block and the account. It reads as `@label` as text, in the clipboard's plain text, in the text
// index (the label) and in the Markdown and HTML exports. `relative` is `me` on a mention that is filled in with whoever makes a
// page from the template that holds it, or clicks the template button (ADR-0056); `accountId` and `label` stay those of the
// person who wrote it, and it is not text. In HTML it is `<span data-type="personMention" data-account-id data-label data-relative>`.
//
// The live chip is `PersonMentionView` (the editor's `nodeView` option); the `@` menu lists the space's members
// (`mention-menu.ts`) and inserts the node with `insertPersonMention`. The attributes and the node name are what the
// Rust schema in `kb-model` and the portable JSON are keyed by.

import { mergeAttributes, Node } from "@tiptap/core";
import type { Editor, NodeViewRenderer } from "@tiptap/core";

import { takesInlineAtom } from "./date-mention";
import { personRelativeOf } from "./template-button";
import type { PersonRelative } from "./template-button";

/** The node's name in the schema. */
export const PERSON_MENTION_NODE = "personMention";

/** The longest label a mention keeps, in characters (the member's name at insertion). */
export const PERSON_LABEL_MAX_CHARS = 100;

export interface PersonMentionOptions {
  /** How the editor shows a mention (the live chip); `null` renders `renderHTML`. */
  nodeView: NodeViewRenderer | null;
  /** The space of the page the editor shows: its members are who a mention can name (`null`: none known, the label shows). */
  spaceId: string | null;
}

/** The text of a mention: `@label`, or `@` alone while the label is empty. */
function mentionText(label: unknown): string {
  return `@${typeof label === "string" ? label : ""}`;
}

/** A name as a label: at most {@link PERSON_LABEL_MAX_CHARS} characters. */
export function personLabel(name: string): string {
  return Array.from(name.trim()).slice(0, PERSON_LABEL_MAX_CHARS).join("");
}

export interface InsertPersonMention {
  /** The member's account id. */
  accountId: string;
  /** The member's name now (kept as the fallback label). */
  name: string;
  /** Fills the mention in with whoever makes a page from the template that holds it (ADR-0056); none by default. */
  relative?: PersonRelative | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    personMention: {
      /** Inserts a mention of a member (then a space) at the selection, in place of it. Refuses a place that takes no inline atoms. */
      insertPersonMention: (attributes: InsertPersonMention) => ReturnType;
    };
  }
}

/** Whether a mention can go where the selection is (a textblock that takes inline atoms). */
export function takesPersonMention(editor: Editor): boolean {
  return takesInlineAtom(editor, PERSON_MENTION_NODE);
}

/** The inline atom of block schema v1 that stands for a member of the space. */
export const PersonMention = Node.create<PersonMentionOptions>({
  name: PERSON_MENTION_NODE,
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  draggable: false,

  addOptions() {
    return { nodeView: null, spaceId: null };
  },

  addAttributes() {
    return {
      accountId: {
        isRequired: true,
        parseHTML: (element) => element.getAttribute("data-account-id"),
        renderHTML: (attributes) => ({ "data-account-id": String(attributes.accountId) }),
      },
      label: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-label") ?? "",
        renderHTML: (attributes) => ({ "data-label": typeof attributes.label === "string" ? attributes.label : "" }),
      },
      // Filled in when a page is made from a template, or a template button is clicked (ADR-0056): `me`, or no placeholder.
      relative: {
        default: null,
        parseHTML: (element) => personRelativeOf(element.getAttribute("data-relative")),
        renderHTML: (attributes) => {
          const relative = personRelativeOf(attributes.relative);
          return relative === null ? {} : { "data-relative": relative };
        },
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: `span[data-type="${PERSON_MENTION_NODE}"]`,
        // A chip without an account is not a mention: its text stays text.
        getAttrs: (element) => (element.getAttribute("data-account-id") === null ? false : null),
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    return [
      "span",
      mergeAttributes({ "data-type": PERSON_MENTION_NODE, class: "person-mention" }, HTMLAttributes),
      mentionText(node.attrs.label),
    ];
  },

  renderText({ node }) {
    return mentionText(node.attrs.label);
  },

  addNodeView() {
    return this.options.nodeView;
  },

  addCommands() {
    return {
      insertPersonMention:
        ({ accountId, name, relative = null }) =>
        ({ commands, editor }) => {
          const label = personLabel(name);
          if (label === "" || !takesPersonMention(editor)) {
            return false;
          }
          return commands.insertContent([
            { type: this.name, attrs: { accountId, label, relative } },
            { type: "text", text: " " },
          ]);
        },
    };
  },
});
