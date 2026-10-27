// Input validation shared by the in-memory transport's handlers: a TypeScript twin of
// `kb-core`'s `validate.rs`. Every failure is an `InvalidInput` error whose
// `details.field` names the offending field, with the core's exact messages.

import { CoreError } from "./errors";

const utf8 = new TextEncoder();

/** An `InvalidInput` error for one field, as `ApiError::invalid(field, message)`. */
export function invalid(field: string, message: string): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field } });
}

/** `kb-core`'s `bounded_text`: at most `maxChars` characters (not UTF-16 units), no control characters. */
function boundedText(text: string, maxChars: number, field: string): string {
  if (Array.from(text).length > maxChars) {
    throw invalid(field, `must be at most ${String(maxChars)} characters`);
  }
  if (/\p{Cc}/u.test(text)) {
    throw invalid(field, "must not contain control characters");
  }
  return text;
}

/** `kb-core`'s `display_name`: trimmed, 1..=max characters, no control characters. */
export function displayName(raw: string, maxChars: number, field: string): string {
  const name = raw.trim();
  if (name === "") {
    throw invalid(field, "must not be empty");
  }
  return boundedText(name, maxChars, field);
}

/** `kb-core`'s `page_title`: trimmed, 0 to 500 characters, no control characters (so no newlines). */
export function pageTitle(raw: string): string {
  return boundedText(raw.trim(), 500, "title");
}

/** The prefix of an icon from the bundled set: `icon:<name>` or `icon:<name>:<color>`. */
const ICON_SET_PREFIX = "icon:";
/** The prefix of an uploaded image icon: `attachment:<attachment id>` (pages only). */
export const ICON_ATTACHMENT_PREFIX = "attachment:";
/** The longest name of an icon of the set, in characters. */
const ICON_NAME_MAX = 48;
/** The nine colours of an icon of the set (absent: the text colour). */
export const ICON_COLORS = [
  "gray",
  "brown",
  "orange",
  "yellow",
  "green",
  "blue",
  "purple",
  "pink",
  "red",
] as const;
/** A lowercase hyphenated UUID (8-4-4-4-12 hex digits). */
const CANONICAL_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** `kb-core`'s `icon_of_the_set`: what follows `icon:` is `<name>` or `<name>:<color>`. */
function iconOfTheSet(rest: string): void {
  const colon = rest.indexOf(":");
  const name = colon < 0 ? rest : rest.slice(0, colon);
  const color = colon < 0 ? undefined : rest.slice(colon + 1);
  if (name === "") {
    throw invalid(
      "icon",
      "an icon of the set is icon:<name> or icon:<name>:<color>; the name is missing",
    );
  }
  if (!/^[a-z0-9-]+$/.test(name)) {
    throw invalid("icon", "the icon name may only contain a-z, 0-9 and '-'");
  }
  if (name.length > ICON_NAME_MAX) {
    throw invalid("icon", `the icon name must be at most ${String(ICON_NAME_MAX)} characters`);
  }
  if (name.startsWith("-") || name.endsWith("-")) {
    throw invalid("icon", "the icon name must not start or end with '-'");
  }
  if (color !== undefined && !(ICON_COLORS as readonly string[]).includes(color)) {
    throw invalid(
      "icon",
      `the icon colour must be one of: ${ICON_COLORS.join(", ")}; omit it for the text colour`,
    );
  }
}

/** `kb-core`'s `icon_attachment_id`: the id after `attachment:` is a lowercase hyphenated UUID. */
function iconAttachmentIdOf(text: string, field = "icon"): string {
  if (!CANONICAL_UUID.test(text)) {
    throw invalid(field, "the attachment id must be a lowercase hyphenated UUID");
  }
  return text;
}

/**
 * `kb-core`'s `icon` (ADR-0029): `null` or `undefined` means "no icon" (clears one); otherwise
 * the trimmed text is one of
 *
 * - an emoji or short text: 1 to 32 UTF-8 bytes, no control characters, not starting with
 *   `icon:` or `attachment:`;
 * - an icon of the bundled set: `icon:<name>` or `icon:<name>:<color>`, the name 1 to 48
 *   characters of `[a-z0-9-]` not starting or ending with `-`, the colour one of the nine of the
 *   palette;
 * - an uploaded image: `attachment:<attachment id>`, the id a lowercase hyphenated UUID.
 *
 * The grammar only: whether an attachment exists, is of the page's space and is an image is
 * checked where the page is set, and a space refuses the third form (`spaceIcon`).
 */
export function icon(raw: string | null | undefined): string | undefined {
  if (raw === null || raw === undefined) {
    return undefined;
  }
  const text = raw.trim();
  if (text === "") {
    throw invalid("icon", "must not be empty; omit it to clear");
  }
  if (text.startsWith(ICON_SET_PREFIX)) {
    iconOfTheSet(text.slice(ICON_SET_PREFIX.length));
  } else if (text.startsWith(ICON_ATTACHMENT_PREFIX)) {
    iconAttachmentIdOf(text.slice(ICON_ATTACHMENT_PREFIX.length));
  } else {
    if (utf8.encode(text).length > 32) {
      throw invalid("icon", "must be at most 32 bytes");
    }
    if (/\p{Cc}/u.test(text)) {
      throw invalid("icon", "must not contain control characters");
    }
  }
  return text;
}

/** `kb-core`'s `space_icon`: the same grammar, without the uploaded image. */
export function spaceIcon(raw: string | null | undefined): string | undefined {
  const text = icon(raw);
  if (text?.startsWith(ICON_ATTACHMENT_PREFIX) === true) {
    throw invalid("icon", "a space icon cannot be an image");
  }
  return text;
}

/**
 * `kb-core`'s `icon_attachment`: the attachment id an icon names (`attachment:<id>`), `undefined`
 * for every other form. Reads what `icon` accepted; never throws.
 */
export function iconAttachment(text: string | undefined): string | undefined {
  if (text === undefined || !text.startsWith(ICON_ATTACHMENT_PREFIX)) {
    return undefined;
  }
  const id = text.slice(ICON_ATTACHMENT_PREFIX.length);
  return CANONICAL_UUID.test(id) ? id : undefined;
}

/** The prefix of a cover that is a palette colour: `color:<palette name>` (ADR-0040). */
const COVER_COLOR_PREFIX = "color:";
/** The prefix of a cover that is a bundled gradient: `gradient:<name>`; the UI defines the list. */
const COVER_GRADIENT_PREFIX = "gradient:";
/** The longest name of a gradient, in characters. */
const COVER_GRADIENT_NAME_MAX = 32;
/** The largest vertical position of a cover, in percent. */
export const COVER_Y_MAX = 100;
/** The default vertical position of a cover, in percent: stored as no value. */
export const COVER_Y_DEFAULT = 50;

/**
 * `kb-core`'s `cover` (ADR-0040): `null` or `undefined` means "no cover" (clears it); otherwise the
 * trimmed text is one of
 *
 * - an uploaded image: `attachment:<attachment id>`, the id a lowercase hyphenated UUID;
 * - a colour of the palette: `color:<name>`, the name one of the nine of the icon colours;
 * - a bundled gradient: `gradient:<name>`, the name 1 to 32 characters of `[a-z0-9-]` (the core
 *   checks the shape only, the UI defines the list).
 *
 * The grammar only: whether an attachment exists, is of the page's space and is an image is checked
 * where the page is set.
 */
export function cover(raw: string | null | undefined): string | undefined {
  if (raw === null || raw === undefined) {
    return undefined;
  }
  const text = raw.trim();
  if (text === "") {
    throw invalid("cover", "must not be empty; omit it to clear");
  }
  if (text.startsWith(ICON_ATTACHMENT_PREFIX)) {
    iconAttachmentIdOf(text.slice(ICON_ATTACHMENT_PREFIX.length), "cover");
  } else if (text.startsWith(COVER_COLOR_PREFIX)) {
    if (!(ICON_COLORS as readonly string[]).includes(text.slice(COVER_COLOR_PREFIX.length))) {
      throw invalid("cover", `the cover colour must be one of: ${ICON_COLORS.join(", ")}`);
    }
  } else if (text.startsWith(COVER_GRADIENT_PREFIX)) {
    const name = text.slice(COVER_GRADIENT_PREFIX.length);
    if (name === "") {
      throw invalid("cover", "a gradient cover is gradient:<name>; the name is missing");
    }
    if (!/^[a-z0-9-]+$/.test(name)) {
      throw invalid("cover", "the gradient name may only contain a-z, 0-9 and '-'");
    }
    if (name.length > COVER_GRADIENT_NAME_MAX) {
      throw invalid("cover", `the gradient name must be at most ${String(COVER_GRADIENT_NAME_MAX)} characters`);
    }
  } else {
    throw invalid("cover", "a cover is attachment:<id>, color:<name> or gradient:<name>");
  }
  return text;
}

/**
 * `kb-core`'s `cover_attachment`: the attachment id a cover names (`attachment:<id>`), `undefined`
 * for every other form. Reads what `cover` accepted; never throws.
 */
export function coverAttachment(text: string | undefined): string | undefined {
  return iconAttachment(text);
}

/** `kb-core`'s `cover_y`: `undefined` (the default) or an integer 0 to 100 percent. */
export function coverY(raw: number | null | undefined): number | undefined {
  if (raw === null || raw === undefined) {
    return undefined;
  }
  if (!Number.isInteger(raw) || raw < 0 || raw > COVER_Y_MAX) {
    throw invalid("coverY", `the cover position must be between 0 and ${String(COVER_Y_MAX)}`);
  }
  return raw;
}

/** The names of a page font on the wire (`PageFont`, ADR-0040): `default` clears the choice. */
const PAGE_FONTS = ["default", "serif", "mono"] as const;

/**
 * The look `PageSetAppearance` asks for (ADR-0040), as the structure applies it: `font` is `undefined`
 * (left as it is), `"serif"`, `"mono"` or `"default"` (clears it); `smallText` and `locked` are
 * `undefined` (left as they are) or a boolean (`false` clears). A field of another type or a font
 * with another name is refused like the core's request parser refuses it, on that field.
 */
export interface PageAppearanceChange {
  font: "default" | "serif" | "mono" | undefined;
  smallText: boolean | undefined;
  locked: boolean | undefined;
}

/** `kb-core`'s request parsing of `PageSetAppearance`: absent (or `null`, as an absent key) means "leave it". */
export function appearance(raw: {
  font?: unknown;
  smallText?: unknown;
  locked?: unknown;
}): PageAppearanceChange {
  const flag = (value: unknown, field: string): boolean | undefined => {
    if (value === undefined || value === null) {
      return undefined;
    }
    if (typeof value !== "boolean") {
      throw invalid(field, "must be true or false");
    }
    return value;
  };
  let font: PageAppearanceChange["font"];
  if (raw.font !== undefined && raw.font !== null) {
    if (typeof raw.font !== "string" || !(PAGE_FONTS as readonly string[]).includes(raw.font)) {
      throw invalid("font", `the font must be one of: ${PAGE_FONTS.join(", ")}`);
    }
    font = raw.font as PageAppearanceChange["font"];
  }
  return { font, smallText: flag(raw.smallText, "smallText"), locked: flag(raw.locked, "locked") };
}

/**
 * Positions are `u32` in the contract, so the real core refuses anything that is not a
 * non-negative integer before a handler runs; the memory transport reports it as
 * `InvalidInput` on `index`. Values past the sibling count are fine: they clamp.
 */
export function position(raw: number): number {
  if (!Number.isInteger(raw) || raw < 0) {
    throw invalid("index", "must be a non-negative integer");
  }
  return raw;
}

/** Shortest accepted app-lock passphrase, in UTF-8 bytes (`kb_keys::PASSPHRASE_MIN_BYTES`). */
export const PASSPHRASE_MIN_BYTES = 8;
/** Longest accepted app-lock passphrase, in UTF-8 bytes (`kb_keys::PASSPHRASE_MAX_BYTES`). */
export const PASSPHRASE_MAX_BYTES = 256;
/** Longest accepted kit text after trimming, in UTF-8 bytes (24 words need at most 215). */
export const MAX_KIT_TEXT_BYTES = 1024;

/** Whether a passphrase has the accepted length: 8 to 256 UTF-8 bytes, as typed. */
export function passphraseLengthOk(raw: string): boolean {
  const bytes = utf8.encode(raw).length;
  return bytes >= PASSPHRASE_MIN_BYTES && bytes <= PASSPHRASE_MAX_BYTES;
}

/**
 * An app-lock passphrase as the core accepts it for `AppLockEnable` (`kb_keys::check_passphrase`
 * through `map_keys_error`): 8 to 256 UTF-8 bytes, taken exactly as typed (never trimmed:
 * spaces are part of the secret), else `InvalidInput` for the field `passphrase`. (The core
 * names the field `new` in `AppLockChange`.) The message never contains the passphrase.
 */
export function passphrase(raw: string): string {
  if (!passphraseLengthOk(raw)) {
    throw invalid(
      "passphrase",
      `passphrase must be ${String(PASSPHRASE_MIN_BYTES)} to ${String(PASSPHRASE_MAX_BYTES)} bytes`,
    );
  }
  return raw;
}

/**
 * A recovery kit as typed (24 words or the code form), checked before it is sent: trimmed,
 * not empty, at most 1024 UTF-8 bytes. A client-side check for forms: the core itself only
 * trims, and reports an empty kit as a code of 0 characters. Errors name the field `kit` and
 * never quote the text, which is a secret.
 */
export function kitText(raw: string): string {
  const text = raw.trim();
  if (text === "") {
    throw invalid("kit", "must not be empty");
  }
  if (utf8.encode(text).length > MAX_KIT_TEXT_BYTES) {
    throw invalid("kit", `must be at most ${String(MAX_KIT_TEXT_BYTES)} bytes`);
  }
  return text;
}

/** What the core answers for a server address it refuses (`InvalidInput` for `serverUrl`). */
const SERVER_URL_MESSAGE = "the server address must be http(s)://host[:port]";

/** Longest accepted server address after trimming, in characters (the core's cap). */
export const MAX_SERVER_URL_CHARS = 2048;

/**
 * A sync server address as `SyncConfigure` takes it (Milestone 6), checked like the core: trimmed,
 * at most 2048 characters, `http://` or `https://` followed by a host, an optional port and an
 * optional path; no `@` (user info), `?` (query) or `#` (fragment) anywhere, no whitespace or
 * control characters. Anything else is `InvalidInput` for the field `serverUrl` with the core's
 * message. Returns the trimmed address, which is what the client sends.
 */
export function serverUrl(raw: string): string {
  const text = raw.trim();
  const scheme = /^https?:\/\//iu.exec(text);
  if (scheme === null || Array.from(text).length > MAX_SERVER_URL_CHARS || /[\s\p{Cc}?#@]/u.test(text)) {
    throw invalid("serverUrl", SERVER_URL_MESSAGE);
  }
  // A host must follow the scheme as typed: `URL` would skip extra slashes (`http:///host`).
  if (/^[/\\]?$/u.test(text.charAt(scheme[0].length))) {
    throw invalid("serverUrl", SERVER_URL_MESSAGE);
  }
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    throw invalid("serverUrl", SERVER_URL_MESSAGE);
  }
  if (
    (url.protocol !== "http:" && url.protocol !== "https:") ||
    url.hostname === "" ||
    url.username !== "" ||
    url.password !== "" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw invalid("serverUrl", SERVER_URL_MESSAGE);
  }
  return text;
}

/** Longest accepted invite token, in characters (all ASCII, so also bytes). */
export const MAX_INVITE_TOKEN_CHARS = 128;

/**
 * An operator invite token as `SyncConfigure` takes it: trimmed, then 1 to 128 printable ASCII
 * characters (space to `~`), else `InvalidInput` for the field `inviteToken`. The message never
 * quotes the token. Returns the trimmed token, which is what the client sends.
 */
export function inviteToken(raw: string): string {
  const token = raw.trim();
  if (token === "" || token.length > MAX_INVITE_TOKEN_CHARS || !/^[ -~]+$/u.test(token)) {
    throw invalid(
      "inviteToken",
      `the invite token must be 1 to ${String(MAX_INVITE_TOKEN_CHARS)} printable ASCII characters`,
    );
  }
  return token;
}

/** Characters of a pairing code (`kb_types::devices::PAIRING_CODE_LEN`). */
export const PAIRING_CODE_LEN = 12;
/** Crockford's base32 alphabet: digits and letters without I, L, O and U. */
const CROCKFORD_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/** Crockford's aliases: letters people type for the digits they look like. */
const CROCKFORD_ALIASES: Readonly<Record<string, string>> = { O: "0", I: "1", L: "1" };

/**
 * A pairing code as typed on the sponsor (Milestone 7, ADR-0022): spaces and dashes removed,
 * then 12 Crockford base32 characters, any case, `O` read as `0` and `I`/`L` as `1`. Anything
 * else is `InvalidInput` for the field `code` ("the code is 12 letters and digits"). Returns the
 * 12 canonical characters (upper case, aliases resolved), which is what the client sends.
 */
export function pairingCode(raw: string): string {
  let code = "";
  for (const char of raw.replace(/[\s-]/gu, "")) {
    // Per character and ASCII only: `toUpperCase` on the whole text would turn "ß" into "SS".
    const upper = /^[a-z]$/u.test(char) ? char.toUpperCase() : char;
    const canonical = CROCKFORD_ALIASES[upper] ?? upper;
    if (canonical.length !== 1 || !CROCKFORD_ALPHABET.includes(canonical)) {
      throw invalid("code", "the code is 12 letters and digits");
    }
    code += canonical;
  }
  if (code.length !== PAIRING_CODE_LEN) {
    throw invalid("code", "the code is 12 letters and digits");
  }
  return code;
}

/**
 * A pairing code as the UI shows it: the canonical characters in three groups of four
 * (`"ABCD EFGH JKMN"`). Throws like {@link pairingCode} for anything that is not a code.
 */
export function formatPairingCode(raw: string): string {
  const code = pairingCode(raw);
  return [code.slice(0, 4), code.slice(4, 8), code.slice(8)].join(" ");
}
