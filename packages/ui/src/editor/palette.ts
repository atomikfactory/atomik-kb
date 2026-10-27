import type { MessageKey } from "../i18n";

/**
 * The named colours of the editor, in the order the pickers list them: the background of a
 * callout (ADR-0016, amendment of 2026-10-06) and the text colour and highlight marks
 * (amendment of 2026-10-07) share them. A stored `color` is one of these names; the colours
 * themselves are CSS variables in `app.css`, different in the light and dark themes (a name
 * stands for the same hue in both, not for one fixed value).
 */
export const PALETTE_COLORS = ["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"] as const;

export type PaletteColor = (typeof PALETTE_COLORS)[number];

/** Whether `value` is one of the palette's names (a stored colour can be anything). */
export function isPaletteColor(value: unknown): value is PaletteColor {
  return typeof value === "string" && (PALETTE_COLORS as readonly string[]).includes(value);
}

/** The message that names each colour (the accessible name of its swatch). */
export const PALETTE_LABELS: Readonly<Record<PaletteColor, MessageKey>> = {
  gray: "callout.color.gray",
  brown: "callout.color.brown",
  orange: "callout.color.orange",
  yellow: "callout.color.yellow",
  green: "callout.color.green",
  blue: "callout.color.blue",
  purple: "callout.color.purple",
  pink: "callout.color.pink",
  red: "callout.color.red",
};
