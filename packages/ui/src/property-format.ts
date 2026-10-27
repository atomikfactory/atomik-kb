// Pure helpers of the properties UI (ADR-0026): the names of the kinds and groups, the order and
// colour of options, and the text <-> value conversions of the editors (numbers typed in any
// notation, the format checks of email and phone, which the core stores as typed).

import {
  MAX_URL_CHARS,
  OPTION_COLORS,
  STATUS_GROUPS,
  codePointLength,
  isValidUrl,
  propertyValueProblem,
} from "@atomik-kb/core-client";
import type {
  DeviceEntry,
  DeviceId,
  OptionColor,
  OptionId,
  PlaceValue,
  PropertyConfig,
  PropertyDef,
  PropertyKind,
  PropertyOption,
  PropertyValue,
  RollupFunction,
  RollupResult,
  StatusGroup,
} from "@atomik-kb/core-client";

import type { MessageKey } from "./i18n";
import { activeLocale, t } from "./i18n";

/** The name of each kind. */
export const KIND_LABELS: Readonly<Record<PropertyKind, MessageKey>> = {
  text: "properties.kind.text",
  number: "properties.kind.number",
  select: "properties.kind.select",
  multi_select: "properties.kind.multi_select",
  date: "properties.kind.date",
  checkbox: "properties.kind.checkbox",
  url: "properties.kind.url",
  status: "properties.kind.status",
  email: "properties.kind.email",
  phone: "properties.kind.phone",
  files: "properties.kind.files",
  checklist: "properties.kind.checklist",
  place: "properties.kind.place",
  person: "properties.kind.person",
  relation: "properties.kind.relation",
  rollup: "properties.kind.rollup",
  created_time: "properties.kind.created_time",
  last_edited_time: "properties.kind.last_edited_time",
  created_by: "properties.kind.created_by",
  last_edited_by: "properties.kind.last_edited_by",
  formula: "properties.kind.formula",
};

/** One line on what each kind holds (the kind picker of a new property). */
export const KIND_HINTS: Readonly<Record<PropertyKind, MessageKey>> = {
  text: "properties.kind.text.hint",
  number: "properties.kind.number.hint",
  select: "properties.kind.select.hint",
  multi_select: "properties.kind.multi_select.hint",
  date: "properties.kind.date.hint",
  checkbox: "properties.kind.checkbox.hint",
  url: "properties.kind.url.hint",
  status: "properties.kind.status.hint",
  email: "properties.kind.email.hint",
  phone: "properties.kind.phone.hint",
  files: "properties.kind.files.hint",
  checklist: "properties.kind.checklist.hint",
  place: "properties.kind.place.hint",
  person: "properties.kind.person.hint",
  relation: "properties.kind.relation.hint",
  rollup: "properties.kind.rollup.hint",
  created_time: "properties.kind.created_time.hint",
  last_edited_time: "properties.kind.last_edited_time.hint",
  created_by: "properties.kind.created_by.hint",
  last_edited_by: "properties.kind.last_edited_by.hint",
  formula: "properties.kind.formula.hint",
};

/** The heading of each group of a status property. */
export const GROUP_LABELS: Readonly<Record<StatusGroup, MessageKey>> = {
  todo: "properties.group.todo",
  in_progress: "properties.group.in_progress",
  done: "properties.group.done",
};

/** The message that names each colour of an option (the same names as the editor's palette). */
export const COLOR_LABELS: Readonly<Record<OptionColor, MessageKey>> = {
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

/** An option's colour as the stylesheet knows it: a name this version does not know reads as gray. */
export function optionColorOf(color: string): OptionColor {
  return (OPTION_COLORS as readonly string[]).includes(color) ? (color as OptionColor) : "gray";
}

/** The colour a new option gets: the one after the last option's in the palette (gray for the first). */
export function nextOptionColor(options: readonly PropertyOption[]): OptionColor {
  const last = options.at(-1);
  if (last === undefined) {
    return "gray";
  }
  const at = OPTION_COLORS.indexOf(optionColorOf(last.color));
  return OPTION_COLORS[(at + 1) % OPTION_COLORS.length] ?? "gray";
}

/** The group of a status option; one that has none (damaged data) counts as to-do. */
export function groupOf(option: PropertyOption): StatusGroup {
  return option.group ?? "todo";
}

/** The options of a status property under their groups, in the order of the groups; empty groups are left out. */
export function groupedOptions(options: readonly PropertyOption[]): { group: StatusGroup; options: PropertyOption[] }[] {
  return STATUS_GROUPS.map((group) => ({ group, options: options.filter((option) => groupOf(option) === group) })).filter(
    (entry) => entry.options.length > 0,
  );
}

/** Whether `name` is the name of an option, ignoring case and the space around it. */
export function hasOptionNamed(options: readonly PropertyOption[], name: string): boolean {
  const wanted = name.trim().toLowerCase();
  return options.some((option) => option.name.trim().toLowerCase() === wanted);
}

/** The options whose name contains `query` (ignoring case); all of them for an empty query. */
export function matchingOptions(options: readonly PropertyOption[], query: string): PropertyOption[] {
  const wanted = query.trim().toLowerCase();
  return wanted === "" ? [...options] : options.filter((option) => option.name.toLowerCase().includes(wanted));
}

/** The option with `id`, if the property lists it. */
export function optionById(def: PropertyDef, id: OptionId): PropertyOption | undefined {
  return def.options.find((option) => option.id === id);
}

// ----- numbers ------------------------------------------------------------------------------

/** The separator the browser's locale puts between thousands, `undefined` when it uses none. */
function localeGroupSeparator(): string | undefined {
  return new Intl.NumberFormat(activeLocale()).formatToParts(1234567.5).find((part) => part.type === "group")?.value;
}

/** What {@link parseNumber} makes of a text. */
export type ParsedNumber = { kind: "empty" } | { kind: "number"; value: number } | { kind: "invalid" };

/**
 * Reads a number the way a person writes it, in any notation: `1234.5`, `1,234.50`, `1.234,50`,
 * `1 234,5`, `1'234.5`, `-3`, `1e3`. A single `,` or `.` is a decimal separator, except when it
 * stands exactly like a thousands separator of the locale (`1,234` in English is 1234); a text
 * with both reads the last one as the decimal separator. Empty text is "no value".
 */
export function parseNumber(text: string): ParsedNumber {
  const cleaned = text.replace(/[\u2212\u2012\u2013]/gu, "-").replace(/[\s\u00a0\u202f']/gu, "");
  if (cleaned === "") {
    return { kind: "empty" };
  }
  if (/^[+-]?\d+(?:\.\d+)?e[+-]?\d+$/iu.test(cleaned)) {
    const value = Number(cleaned);
    return Number.isFinite(value) ? { kind: "number", value: value === 0 ? 0 : value } : { kind: "invalid" };
  }
  if (!/^[+-]?[\d.,]+$/u.test(cleaned) || !/\d/u.test(cleaned)) {
    return { kind: "invalid" };
  }
  const negative = cleaned.startsWith("-");
  const body = cleaned.replace(/^[+-]/u, "");
  const lastDot = body.lastIndexOf(".");
  const lastComma = body.lastIndexOf(",");
  let decimalAt = -1;
  if (lastDot >= 0 && lastComma >= 0) {
    decimalAt = Math.max(lastDot, lastComma);
    const decimal = body.charAt(decimalAt);
    const group = decimal === "." ? "," : ".";
    // One decimal separator, after every group separator.
    if (body.split(decimal).length !== 2 || body.lastIndexOf(group) > decimalAt) {
      return { kind: "invalid" };
    }
  } else if (lastDot >= 0 || lastComma >= 0) {
    const separator = lastDot >= 0 ? "." : ",";
    const position = body.lastIndexOf(separator);
    if (body.split(separator).length === 2) {
      const digitsAfter = body.length - position - 1;
      const looksLikeGroup = digitsAfter === 3 && position >= 1 && position <= 3 && !body.startsWith("0");
      decimalAt = looksLikeGroup && separator === localeGroupSeparator() ? -1 : position;
    }
  }
  const whole = (decimalAt < 0 ? body : body.slice(0, decimalAt)).replace(/[.,]/gu, "");
  const fraction = decimalAt < 0 ? "" : body.slice(decimalAt + 1);
  if (!/^\d*$/u.test(whole) || !/^\d*$/u.test(fraction) || (whole === "" && fraction === "")) {
    return { kind: "invalid" };
  }
  const value = Number(`${whole === "" ? "0" : whole}.${fraction === "" ? "0" : fraction}`);
  if (!Number.isFinite(value)) {
    return { kind: "invalid" };
  }
  const signed = negative ? -value : value;
  return { kind: "number", value: signed === 0 ? 0 : signed };
}

/** A number as the locale writes it (digits grouped, the locale's decimal separator, nothing rounded away). */
export function formatNumber(value: number): string {
  return new Intl.NumberFormat(activeLocale(), { maximumFractionDigits: 20 }).format(value);
}

// ----- email, phone, url ---------------------------------------------------------------------

/** Whether `text` looks like an email address: something, an `@`, a domain with a dot. */
export function isEmailAddress(text: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/u.test(text);
}

/** Whether `text` looks like a phone number: digits (at least three) with spaces and `+ ( ) - . / # *`. */
export function isPhoneNumber(text: string): boolean {
  return /^[\d\s+().\-/#*]+$/u.test(text) && (text.match(/\d/gu)?.length ?? 0) >= 3;
}

/** The `mailto:` link of an address (the address is escaped, so it cannot add headers to the link). */
export function mailtoLink(address: string): string {
  return `mailto:${encodeURIComponent(address).replace(/%40/gu, "@")}`;
}

// ----- the text of an editor ---------------------------------------------------------------------

/** The text an editor of a kind starts from, for a value (empty without one). */
export function draftOf(kind: PropertyKind, value: PropertyValue | undefined): string {
  if (value === undefined) {
    return "";
  }
  switch (value.kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
      return value.value;
    case "number":
      return String(value.value);
    default:
      return "";
  }
}

/** What an editor's text means: the value to save (`null` clears), or why it cannot be saved. */
export type Draft = { ok: true; value: PropertyValue | null } | { ok: false; problem: string };

function refuse(value: PropertyValue): Draft | undefined {
  const problem = propertyValueProblem(value);
  return problem === undefined ? undefined : { ok: false, problem: t("properties.problem.value", { message: problem }) };
}

/**
 * The value a text typed into an editor of `kind` stands for. Text is kept as typed; the single
 * line kinds are trimmed; an empty text clears. A problem is the message to show next to the field.
 */
export function parseDraft(kind: PropertyKind, draft: string): Draft {
  switch (kind) {
    case "text": {
      if (draft === "") {
        return { ok: true, value: null };
      }
      const value: PropertyValue = { kind: "text", value: draft };
      return refuse(value) ?? { ok: true, value };
    }
    case "number": {
      const parsed = parseNumber(draft);
      if (parsed.kind === "empty") {
        return { ok: true, value: null };
      }
      if (parsed.kind === "invalid") {
        return { ok: false, problem: t("properties.problem.number") };
      }
      const value: PropertyValue = { kind: "number", value: parsed.value };
      return refuse(value) ?? { ok: true, value };
    }
    case "url": {
      const text = draft.trim();
      if (text === "") {
        return { ok: true, value: null };
      }
      const value: PropertyValue = { kind: "url", value: text };
      if (isValidUrl(text)) {
        return { ok: true, value };
      }
      // The core words a link that is not one in its own way: a text that is too long keeps its message, the rest says what a link is.
      const tooLong = codePointLength(text) > MAX_URL_CHARS;
      return (tooLong ? refuse(value) : undefined) ?? { ok: false, problem: t("properties.problem.url") };
    }
    case "email": {
      const text = draft.trim();
      if (text === "") {
        return { ok: true, value: null };
      }
      const value: PropertyValue = { kind: "email", value: text };
      return refuse(value) ?? (isEmailAddress(text) ? { ok: true, value } : { ok: false, problem: t("properties.problem.email") });
    }
    case "phone": {
      const text = draft.trim();
      if (text === "") {
        return { ok: true, value: null };
      }
      const value: PropertyValue = { kind: "phone", value: text };
      return refuse(value) ?? (isPhoneNumber(text) ? { ok: true, value } : { ok: false, problem: t("properties.problem.phone") });
    }
    default:
      return { ok: false, problem: t("properties.problem.value", { message: "cannot be typed" }) };
  }
}

// ----- places ---------------------------------------------------------------------------------

/** The map the "Open in map" action opens (the shell's external-link opener takes http and https only). */
const MAP_ORIGIN = "https://www.openstreetmap.org";

/**
 * The link "Open in map" opens for a place: the position on OpenStreetMap when it has coordinates,
 * a search for its name otherwise. Nothing is fetched by the app: the link opens in the person's
 * browser, only when they ask for it.
 */
export function placeMapUrl(place: PlaceValue): string {
  if (place.lat !== undefined && place.lon !== undefined) {
    const lat = String(place.lat);
    const lon = String(place.lon);
    return `${MAP_ORIGIN}/?mlat=${lat}&mlon=${lon}#map=16/${lat}/${lon}`;
  }
  return `${MAP_ORIGIN}/search?query=${encodeURIComponent(place.name)}`;
}

/** The three texts of a place editor: the name and the two coordinates as typed. */
export interface PlaceDraft {
  name: string;
  lat: string;
  lon: string;
}

/** The texts an editor starts from, for a value (empty without one). */
export function placeDraftOf(value: PropertyValue | undefined): PlaceDraft {
  if (value?.kind !== "place") {
    return { name: "", lat: "", lon: "" };
  }
  return {
    name: value.value.name,
    lat: value.value.lat === undefined ? "" : String(value.value.lat),
    lon: value.value.lon === undefined ? "" : String(value.value.lon),
  };
}

/** What the texts of a place editor mean: the value to save (`null` clears), or why it cannot be saved. */
export function parsePlaceDraft(draft: PlaceDraft): Draft {
  const name = draft.name.trim();
  const lat = draft.lat.trim();
  const lon = draft.lon.trim();
  if (name === "" && lat === "" && lon === "") {
    return { ok: true, value: null };
  }
  if (name === "") {
    return { ok: false, problem: t("properties.place.problem.name") };
  }
  const place: PlaceValue = { name };
  if (lat !== "" || lon !== "") {
    if (lat === "" || lon === "") {
      return { ok: false, problem: t("properties.place.problem.pair") };
    }
    const parsedLat = parseNumber(lat);
    if (parsedLat.kind !== "number" || parsedLat.value < -90 || parsedLat.value > 90) {
      return { ok: false, problem: t("properties.place.problem.lat") };
    }
    const parsedLon = parseNumber(lon);
    if (parsedLon.kind !== "number" || parsedLon.value < -180 || parsedLon.value > 180) {
      return { ok: false, problem: t("properties.place.problem.lon") };
    }
    place.lat = parsedLat.value;
    place.lon = parsedLon.value;
  }
  const value: PropertyValue = { kind: "place", value: place };
  return refuse(value) ?? { ok: true, value };
}

// ----- computed values ---------------------------------------------------------------------------

/**
 * The name of a device for display: this device by its own name, another by the device log's name,
 * and "Unknown device" for one the log does not list or that was revoked.
 */
export function deviceLabel(
  id: DeviceId,
  devices: readonly DeviceEntry[],
  own: { deviceId: DeviceId; name: string } | null,
): string {
  if (own !== null && own.deviceId === id) {
    return own.name;
  }
  const entry = devices.find((device) => device.deviceId === id);
  return entry === undefined || entry.revokedAt !== undefined ? t("properties.computed.unknownDevice") : entry.name;
}

/** An instant (Unix ms) as the person's locale writes a date and a time. */
export function formatInstant(ms: number): string {
  return new Intl.DateTimeFormat(activeLocale(), { dateStyle: "medium", timeStyle: "short" }).format(new Date(ms));
}

// ----- relations and rollups (ADR-0027) -----------------------------------------------------------

/** The name of each rollup function, as the function picker lists it. */
export const ROLLUP_FUNCTION_LABELS: Readonly<Record<RollupFunction, MessageKey>> = {
  count_all: "properties.rollup.fn.count_all",
  count_values: "properties.rollup.fn.count_values",
  count_unique: "properties.rollup.fn.count_unique",
  count_empty: "properties.rollup.fn.count_empty",
  percent_empty: "properties.rollup.fn.percent_empty",
  percent_not_empty: "properties.rollup.fn.percent_not_empty",
  sum: "properties.rollup.fn.sum",
  average: "properties.rollup.fn.average",
  median: "properties.rollup.fn.median",
  min: "properties.rollup.fn.min",
  max: "properties.rollup.fn.max",
  range: "properties.rollup.fn.range",
  earliest: "properties.rollup.fn.earliest",
  latest: "properties.rollup.fn.latest",
  percent_checked: "properties.rollup.fn.percent_checked",
  count_checked: "properties.rollup.fn.count_checked",
  show_original: "properties.rollup.fn.show_original",
  checklist_progress: "properties.rollup.fn.checklist_progress",
};

/** The functions whose number is a percent (0 to 100). */
const PERCENT_FUNCTIONS: readonly RollupFunction[] = ["percent_empty", "percent_not_empty", "percent_checked"];

/** A number of a rollup as the locale writes it: at most two decimals, a percent with its sign. */
export function formatRollupNumber(value: number, fn: RollupFunction): string {
  const plain = new Intl.NumberFormat(activeLocale(), { maximumFractionDigits: 2 }).format(value);
  return PERCENT_FUNCTIONS.includes(fn) ? t("properties.rollup.percent", { value: plain }) : plain;
}

/**
 * One value of a rollup's list ("show original") as text: the name of an option, a number as the
 * locale writes it, a day through `formatDay`, "Yes" or "No" for a checkbox, the text of the
 * rest. `target` is the property the rollup summarises (`undefined` for the titles).
 */
export function rollupItemText(
  value: PropertyValue,
  target: PropertyDef | undefined,
  formatDay: (date: string, time: string | null) => string,
): string {
  const optionName = (id: OptionId): string => target?.options.find((option) => option.id === id)?.name ?? "";
  switch (value.kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
      return value.value;
    case "number":
      return formatNumber(value.value);
    case "checkbox":
      return t(value.value ? "properties.rollup.yes" : "properties.rollup.no");
    case "date":
      return formatDay(value.value.date, value.value.time ?? null);
    case "select":
    case "status":
      return optionName(value.value);
    case "multi_select":
      return value.value.map(optionName).filter((name) => name !== "").join(", ");
    case "place":
      return value.value.name;
    case "checklist":
      return t("properties.rollup.progress", {
        done: value.value.filter((item) => item.done).length,
        total: value.value.length,
      });
    default:
      return "";
  }
}

/**
 * What a rollup shows, as texts: one for a number, a day or a progress, one per value for a list
 * ("show original"; values that read as nothing are left out).
 */
export function rollupTexts(
  fn: RollupFunction,
  result: RollupResult,
  target: PropertyDef | undefined,
  formatDay: (date: string, time: string | null) => string,
): string[] {
  switch (result.type) {
    case "number":
      return [formatRollupNumber(result.data, fn)];
    case "date":
      return [formatDay(result.data.date, result.data.time ?? null)];
    case "progress":
      return [t("properties.rollup.progress", { done: result.data.done, total: result.data.total })];
    case "values":
      return result.data.map((value) => rollupItemText(value, target, formatDay)).filter((text) => text !== "");
  }
}

/** The configuration of a rollup, when `def` is one that has it. */
export function rollupConfigOf(def: PropertyDef): Extract<PropertyConfig, { kind: "rollup" }> | undefined {
  return def.config?.kind === "rollup" ? def.config : undefined;
}

/** The paired property of a two-way relation, when `def` is one. */
export function pairedPropertyOf(def: PropertyDef, schema: readonly PropertyDef[]): PropertyDef | undefined {
  if (def.config?.kind !== "relation" || !def.config.twoWay) {
    return undefined;
  }
  const pairedId = def.config.pairedPropertyId;
  return schema.find((candidate) => candidate.id === pairedId);
}
