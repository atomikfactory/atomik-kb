// The grammar of a page cover (ADR-0040), read the way the core writes it:
//
//   `attachment:<attachment id>`   an uploaded picture (stored like an icon's: an ordinary attachment of the space)
//   `color:<palette name>`         one of the nine colours of the palette (callouts and text colours)
//   `gradient:<name>`              a bundled gradient; the names below are defined in CSS (`app.css`)
//
// The core checks the grammar when it is written (and the shape `[a-z0-9-]{1,32}` of a gradient name, not the
// list: a newer app may bundle more). This reader is lenient about what it finds in a document: a gradient it
// does not know is a neutral banner, never the raw string; a value of no form at all is no cover.

import { isPaletteColor } from "../editor/palette";
import type { PaletteColor } from "../editor/palette";
import type { MessageKey } from "../i18n";
import { IMAGE_PREFIX, isAttachmentId } from "./icon-grammar";

/** A cover as the grammar says it, before the colour, the gradient or the attachment is looked up. */
export type ParsedCover =
  | { kind: "image"; attachmentId: string }
  | { kind: "color"; color: PaletteColor }
  | { kind: "gradient"; name: string };

export const COLOR_PREFIX = "color:";
export const GRADIENT_PREFIX = "gradient:";

/** The bundled gradients, in the order the gallery lists them: their names, defined in `app.css`. */
export const COVER_GRADIENTS = ["sunset", "dawn", "ocean", "forest", "lavender", "peach", "mint", "slate", "candy", "aurora"] as const;

export type CoverGradient = (typeof COVER_GRADIENTS)[number];

/** The message that names each bundled gradient (the accessible name of its swatch). */
export const GRADIENT_LABELS: Readonly<Record<CoverGradient, MessageKey>> = {
  sunset: "cover.gradient.sunset",
  dawn: "cover.gradient.dawn",
  ocean: "cover.gradient.ocean",
  forest: "cover.gradient.forest",
  lavender: "cover.gradient.lavender",
  peach: "cover.gradient.peach",
  mint: "cover.gradient.mint",
  slate: "cover.gradient.slate",
  candy: "cover.gradient.candy",
  aurora: "cover.gradient.aurora",
};

/** Whether `name` is a gradient this version bundles. */
export function isBundledGradient(name: string): name is CoverGradient {
  return (COVER_GRADIENTS as readonly string[]).includes(name);
}

/** `[a-z0-9-]`, 1 to 32 characters: the shape the core accepts for a gradient name. */
const GRADIENT_NAME = /^[a-z0-9-]{1,32}$/;

/**
 * What a cover string says. `null` for no cover (absent or blank) and for a value that does not follow the
 * grammar: a colour outside the palette, a malformed gradient name or attachment id, any other text.
 */
export function parseCover(cover: string | null | undefined): ParsedCover | null {
  const text = cover?.trim() ?? "";
  if (text.startsWith(IMAGE_PREFIX)) {
    const attachmentId = text.slice(IMAGE_PREFIX.length);
    return isAttachmentId(attachmentId) ? { kind: "image", attachmentId } : null;
  }
  if (text.startsWith(COLOR_PREFIX)) {
    const color = text.slice(COLOR_PREFIX.length);
    return isPaletteColor(color) ? { kind: "color", color } : null;
  }
  if (text.startsWith(GRADIENT_PREFIX)) {
    const name = text.slice(GRADIENT_PREFIX.length);
    return GRADIENT_NAME.test(name) ? { kind: "gradient", name } : null;
  }
  return null;
}

/** The string that stores a palette colour: `color:<name>`. */
export function colorCoverValue(color: PaletteColor): string {
  return `${COLOR_PREFIX}${color}`;
}

/** The string that stores a bundled gradient: `gradient:<name>`. */
export function gradientCoverValue(name: CoverGradient): string {
  return `${GRADIENT_PREFIX}${name}`;
}

/** The string that stores an uploaded picture: `attachment:<attachment id>`. */
export function imageCoverValue(attachmentId: string): string {
  return `${IMAGE_PREFIX}${attachmentId}`;
}

/** The default vertical position of a cover's picture, in percent (the core stores it as none). */
export const COVER_Y_DEFAULT = 50;

/** The position a page reports (`coverY`, absent for the default) as a number from 0 to 100. */
export function coverPosition(coverY: number | undefined): number {
  return coverY === undefined || !Number.isFinite(coverY) ? COVER_Y_DEFAULT : Math.min(100, Math.max(0, Math.round(coverY)));
}
