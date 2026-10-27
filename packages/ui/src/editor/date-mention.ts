// The date mention (ADR-0016, amendment of 2026-10-07): an inline atom `dateMention { id, date,
// time? }`, the block schema's chip for "when". `date` is a floating local date, `YYYY-MM-DD`: a
// calendar day with no time zone, so it reads the same on every device and never shifts when
// someone travels; `time` is `HH:mm` (24 hours), also floating, and not stored when unset;
// `reminder` is the number of minutes before the moment (`date` at `time`, or at 09:00) that the
// app reminds the person (ADR-0024), not stored when unset and not text; `relative` is `today` or `now` on a mention that
// is filled in when a page is made from a template or a template button is clicked (ADR-0056; `date` and `time` stay
// the valid values of the moment it was written), not stored when unset and not text. In HTML it is `<span
// data-type="dateMention" data-date data-time data-reminder data-relative>`; as text, and in the text index, the
// backlinks and the Markdown export, it is its ISO text (`2026-10-07`, `2026-10-07 14:30`).

import { dateMentionText, reminderMinutesOf } from "@atomik-kb/core-client";
import type { SpaceId } from "@atomik-kb/core-client";
import { mergeAttributes, Node } from "@tiptap/core";
import type { Editor, NodeViewRenderer } from "@tiptap/core";
import { NodeSelection } from "@tiptap/pm/state";

import { isIsoDate, isIsoTime } from "./dates";
import { dateRelativeOf } from "./template-button";
import type { DateRelative } from "./template-button";

export const DATE_MENTION_NODE = "dateMention";

export interface DateMentionOptions {
  /** How the editor shows a mention (the live chip); `null` renders `renderHTML`. */
  nodeView: NodeViewRenderer | null;
  /**
   * The space of the page: in a shared one the date picker's reminder says who is reminded (`reminderFor`: the members of this space);
   * `null` (a page of no space, a test) or a personal space offers no such choice.
   */
  spaceId: SpaceId | null;
}

/**
 * Which mention has its date picker open, if any (by block id). The chip of a mention draws its own
 * picker, so a command that wants one opened (`/date`, the `@` menu's "Pick a date...") says which
 * mention's it is, and the chip that holds that id opens it.
 */
export class DatePickerRequests {
  #openFor: string | null = null;
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The block id of the mention whose picker is open. */
  readonly getSnapshot = (): string | null => this.#openFor;

  open(id: string): void {
    this.#set(id);
  }

  /** Closes the picker of `id`, or whichever is open when no id is given. */
  close(id?: string): void {
    if (id === undefined || this.#openFor === id) {
      this.#set(null);
    }
  }

  #set(id: string | null): void {
    if (this.#openFor === id) {
      return;
    }
    this.#openFor = id;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/**
 * Whether the selection sits where an inline atom named `name` (a date mention, a page reference)
 * may go: in a textblock whose content takes it (not in a code block, a math block or a toggle's
 * summary, which hold text only).
 */
export function takesInlineAtom(editor: Editor, name: string): boolean {
  const type = editor.schema.nodes[name];
  const { $from, $to } = editor.state.selection;
  return type !== undefined && $from.parent.canReplaceWith($from.index(), $to.indexAfter(), type);
}

export interface DateMentionStorage {
  picker: DatePickerRequests;
}

export interface InsertDateMention {
  /** The day, `YYYY-MM-DD`. */
  date: string;
  /** The time of day, `HH:mm`; none by default. */
  time?: string | null;
  /** Opens the mention's date picker once it is in the page. */
  openPicker?: boolean;
  /** Fills the mention in when a page is made from the template that holds it (ADR-0056); none by default. */
  relative?: DateRelative | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    dateMention: {
      /**
       * Inserts a mention of `date` (a block id of its own, then a space) at the selection, in place
       * of it. Refuses a date that is not a real day and a place that takes no inline atoms.
       */
      insertDateMention: (attributes: InsertDateMention) => ReturnType;
    };
  }

  interface Storage {
    dateMention: DateMentionStorage;
  }
}

/** The inline atom of block schema v1 that stands for a day (and a time of day). */
export const DateMention = Node.create<DateMentionOptions, DateMentionStorage>({
  name: DATE_MENTION_NODE,
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  draggable: false,

  addOptions() {
    return { nodeView: null, spaceId: null };
  },

  addStorage() {
    return { picker: new DatePickerRequests() };
  },

  addAttributes() {
    return {
      date: {
        isRequired: true,
        parseHTML: (element) => element.getAttribute("data-date"),
        renderHTML: (attributes) => ({ "data-date": String(attributes.date) }),
      },
      time: {
        default: null,
        parseHTML: (element) => {
          const time = element.getAttribute("data-time");
          return time !== null && isIsoTime(time) ? time : null;
        },
        renderHTML: (attributes) => (typeof attributes.time === "string" ? { "data-time": attributes.time } : {}),
      },
      reminder: {
        default: null,
        // A reminder that is not a non-negative integer of minutes is no reminder, as in the core.
        parseHTML: (element) => {
          const text = element.getAttribute("data-reminder");
          return text !== null && /^\d+$/u.test(text) ? reminderMinutesOf(Number(text)) : null;
        },
        renderHTML: (attributes) => {
          const minutes = reminderMinutesOf(attributes.reminder);
          return minutes === null ? {} : { "data-reminder": String(minutes) };
        },
      },
      // The account whose devices fire the reminder in a shared space (ADR-0047, M13 slice 2): an account id,
      // not stored when unset. The date picker of a shared space writes it ("Remind: Space Owners / Me / a member");
      // without it the reminder is the Owners' in a shared space and this device's in a personal one.
      reminderFor: {
        default: null,
        parseHTML: (element) => element.getAttribute("data-reminder-for"),
        renderHTML: (attributes) =>
          typeof attributes.reminderFor === "string" ? { "data-reminder-for": attributes.reminderFor } : {},
      },
      // Filled in when a page is made from a template, or a template button is clicked (ADR-0056): `today` or `now`;
      // anything else is no placeholder. Not stored when unset.
      relative: {
        default: null,
        parseHTML: (element) => dateRelativeOf(element.getAttribute("data-relative")),
        renderHTML: (attributes) => {
          const relative = dateRelativeOf(attributes.relative);
          return relative === null ? {} : { "data-relative": relative };
        },
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: `span[data-type="${DATE_MENTION_NODE}"]`,
        // A chip without a real day is not a mention (the date is required): its text stays text.
        getAttrs: (element) => (isIsoDate(element.getAttribute("data-date") ?? "") ? null : false),
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    const text = dateMentionText(node.attrs.date, node.attrs.time) ?? "";
    return ["span", mergeAttributes({ "data-type": DATE_MENTION_NODE, class: "date-mention" }, HTMLAttributes), text];
  },

  renderText({ node }) {
    return dateMentionText(node.attrs.date, node.attrs.time) ?? "";
  },

  addNodeView() {
    return this.options.nodeView;
  },

  addCommands() {
    return {
      insertDateMention:
        ({ date, time = null, openPicker = false, relative = null }) =>
        ({ commands, editor }) => {
          if (!isIsoDate(date) || (time !== null && !isIsoTime(time)) || !takesInlineAtom(editor, this.name)) {
            return false;
          }
          const id = crypto.randomUUID();
          const inserted = commands.insertContent([
            { type: this.name, attrs: { id, date, time, relative } },
            { type: "text", text: " " },
          ]);
          if (inserted && openPicker) {
            this.storage.picker.open(id);
          }
          return inserted;
        },
    };
  },

  addKeyboardShortcuts() {
    return {
      // A mention selected with the arrow keys opens its picker, as a click does.
      Enter: ({ editor }) => {
        const { selection } = editor.state;
        if (!editor.isEditable || !(selection instanceof NodeSelection) || selection.node.type.name !== this.name) {
          return false;
        }
        const id: unknown = selection.node.attrs.id;
        if (typeof id !== "string") {
          return false;
        }
        this.storage.picker.open(id);
        return true;
      },
    };
  },
});
