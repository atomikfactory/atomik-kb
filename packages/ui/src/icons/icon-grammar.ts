// The grammar of a page or space icon (ADR-0029, section 1), read the way the core writes it:
//
//   `🌱`                          an emoji or a short text
//   `icon:<name>[:<color>]`       an icon of the bundled set, in one of the nine palette colours
//   `attachment:<attachment id>`  an uploaded image (pages only)
//
// The core refuses what does not follow the grammar when it is written; this reader is lenient
// about what it finds in a document (a newer app may know more names and colours): it answers
// `null` for a value it cannot make sense of, and the caller shows the default glyph, never the
// raw string.

import { isPaletteColor } from "../editor/palette";
import type { PaletteColor } from "../editor/palette";

/** An icon as the grammar says it, before the set or the attachment is looked up. */
export type ParsedIcon =
  | { kind: "emoji"; text: string }
  | { kind: "set"; name: string; color?: PaletteColor }
  | { kind: "image"; attachmentId: string };

export const SET_PREFIX = "icon:";
export const IMAGE_PREFIX = "attachment:";

/** `[a-z0-9-]`, 1 to 48 characters, not starting or ending with `-`. */
const NAME = /^(?:[a-z0-9]|[a-z0-9][a-z0-9-]{0,46}[a-z0-9])$/;

/** A lowercase hyphenated UUID, as the core writes an attachment id. */
const ATTACHMENT_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** Whether `name` is a well-formed icon name (a name of the set, or one a newer set may have). */
export function isIconName(name: string): boolean {
  return NAME.test(name);
}

/** Whether `id` has the shape of an attachment id. */
export function isAttachmentId(id: string): boolean {
  return ATTACHMENT_ID.test(id);
}

/**
 * What an icon string says: an emoji (any other text), a name of the icon set with its colour,
 * or an uploaded image. `null` for no icon (absent or blank) and for a value of the second and
 * third form that does not follow the grammar: a malformed name, a colour outside the palette, a
 * malformed attachment id.
 */
export function parseIcon(icon: string | null | undefined): ParsedIcon | null {
  const text = icon?.trim() ?? "";
  if (text === "") {
    return null;
  }
  if (text.startsWith(SET_PREFIX)) {
    const [name = "", color, ...rest] = text.slice(SET_PREFIX.length).split(":");
    if (rest.length > 0 || !isIconName(name)) {
      return null;
    }
    if (color === undefined) {
      return { kind: "set", name };
    }
    return isPaletteColor(color) ? { kind: "set", name, color } : null;
  }
  if (text.startsWith(IMAGE_PREFIX)) {
    const attachmentId = text.slice(IMAGE_PREFIX.length);
    return isAttachmentId(attachmentId) ? { kind: "image", attachmentId } : null;
  }
  return { kind: "emoji", text };
}

/** The string that stores an icon of the set: `icon:<name>` or `icon:<name>:<color>`. */
export function setIconValue(name: string, color?: PaletteColor | null): string {
  return color === undefined || color === null ? `${SET_PREFIX}${name}` : `${SET_PREFIX}${name}:${color}`;
}

/** The string that stores an uploaded image: `attachment:<attachment id>`. */
export function imageIconValue(attachmentId: string): string {
  return `${IMAGE_PREFIX}${attachmentId}`;
}
