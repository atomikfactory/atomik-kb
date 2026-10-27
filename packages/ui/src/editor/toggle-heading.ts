// Toggle headings (ADR-0016, amendment of 2026-10-14; ADR-0040): a toggle whose summary is a
// heading. Block schema v1's `detailsSummary` gains one optional attribute, `level` (1, 2 or 3),
// which is `null` by default and then not stored: a summary without it is the plain toggle every
// page has today. The summary keeps its content (`text*`) and stays the summary of a toggle: it
// is drawn with the type of a heading of its level (`data-level` and the class `toggle-heading`,
// see `app.css`) and the toggle opens and closes as before.
//
// A level this version does not know (a number outside 1 to 3, or not a number, written by a newer
// version) is kept in the document as it is and shows a plain toggle, as an unknown callout colour
// shows the default background.

import { DetailsSummary } from "@tiptap/extension-details";

/** The levels of a toggle heading, the levels of headings (the schema allows these three). */
export const TOGGLE_HEADING_LEVELS = [1, 2, 3] as const;

export type ToggleHeadingLevel = (typeof TOGGLE_HEADING_LEVELS)[number];

/** The level a stored value means: 1, 2 or 3, and `null` for anything else (a plain toggle). */
export function toggleHeadingLevel(value: unknown): ToggleHeadingLevel | null {
  return value === 1 || value === 2 || value === 3 ? value : null;
}

/** The level of the toggle heading whose summary is `summary` (an editor node), `null` for a plain one. */
export function summaryLevel(summary: { type: { name: string }; attrs: Record<string, unknown> }): ToggleHeadingLevel | null {
  return summary.type.name === "detailsSummary" ? toggleHeadingLevel(summary.attrs.level) : null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    toggleHeading: {
      /**
       * Wraps the selected blocks in a toggle whose summary is a heading of `level`, the caret in
       * the (empty) summary: `setDetails` with the level set on the new summary.
       */
      setToggleHeading: (level: ToggleHeadingLevel) => ReturnType;
    };
  }
}

/**
 * The summary of a toggle: Tiptap's own (content `text*`, HTML `<summary>`) plus the optional
 * `level`. In HTML a toggle heading's summary is `<summary class="toggle-heading" data-level="2">`
 * and `data-level` is read back (pasted HTML), where anything but 1 to 3 is no level.
 */
export const ToggleSummary = DetailsSummary.extend({
  addAttributes() {
    return {
      level: {
        default: null,
        parseHTML: (element: HTMLElement) => {
          const raw = element.getAttribute("data-level");
          return raw === null ? null : toggleHeadingLevel(Number(raw));
        },
        renderHTML: (attributes: Record<string, unknown>) => {
          const level = toggleHeadingLevel(attributes.level);
          return level === null ? {} : { "data-level": String(level), class: "toggle-heading" };
        },
      },
    };
  },

  addCommands() {
    return {
      setToggleHeading:
        (level: ToggleHeadingLevel) =>
        ({ chain }) =>
          chain()
            .setDetails()
            .command(({ tr }) => {
              const { $from } = tr.selection;
              if ($from.parent.type.name !== this.name) {
                return false;
              }
              tr.setNodeAttribute($from.before(), "level", level);
              return true;
            })
            .run(),
    };
  },
});
