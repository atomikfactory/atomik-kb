// Typed page properties (ADR-0026) as the client sees them: the constants and the pure rules of
// `kb-types`' `properties` module and `kb-model`'s `properties/value.rs`, mirrored exactly, so
// that a form can check a value as it is typed and `MemoryTransport` (`./memory-properties`)
// answers with the core's errors. Dependency-free apart from the generated types, the date and
// time checks of date mentions (`./page-doc`) and `invalid` (`./validate`).
//
// A space owns a schema of properties (`PropertyDef`), a page owns at most one value per
// property (`PropertyValue`). Tags is a built-in multi-select with a fixed id that every space
// has. Kinds are fixed when a property is created.

import type { AttachmentId } from "./generated/AttachmentId";
import type { ChecklistItem } from "./generated/ChecklistItem";
import type { ChecklistItemId } from "./generated/ChecklistItemId";
import type { DateValue } from "./generated/DateValue";
import type { FormulaResult } from "./generated/FormulaResult";
import type { OptionColor } from "./generated/OptionColor";
import type { OptionId } from "./generated/OptionId";
import type { PageId } from "./generated/PageId";
import type { PageProperties } from "./generated/PageProperties";
import type { PlaceValue } from "./generated/PlaceValue";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyFilter } from "./generated/PropertyFilter";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyOption } from "./generated/PropertyOption";
import type { PropertySchema } from "./generated/PropertySchema";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RollupFunction } from "./generated/RollupFunction";
import type { RollupResult } from "./generated/RollupResult";
import type { RollupTarget } from "./generated/RollupTarget";
import type { RollupValue } from "./generated/RollupValue";
import type { StatusGroup } from "./generated/StatusGroup";
import { formatFormulaValue } from "./formula";
import { canonicalId, isIsoDate, isIsoTime } from "./page-doc";
import { invalid } from "./validate";

export type {
  ChecklistItem,
  ChecklistItemId,
  DateValue,
  OptionColor,
  OptionId,
  PageProperties,
  PlaceValue,
  PropertyConfig,
  PropertyDef,
  PropertyFilter,
  PropertyId,
  PropertyKind,
  PropertyOption,
  PropertySchema,
  PropertyValue,
  RollupFunction,
  RollupResult,
  RollupTarget,
  RollupValue,
  StatusGroup,
};

/** The fixed id of the built-in Tags property (a multi-select every space has, listed first). */
export const TAGS_PROPERTY_ID: PropertyId = "0000000a-0000-4000-8000-000000000001";
/** The name Tags starts with (it can be renamed). */
export const TAGS_NAME = "Tags";

/** Most properties a space's schema may hold, Tags included. */
export const MAX_PROPERTIES = 100;
/** Most options one select, multi-select or status property may hold. */
export const MAX_OPTIONS = 500;
/** Longest property or option name, in characters (after trimming). */
export const MAX_NAME_CHARS = 100;
/** Longest `text` value, in characters. */
export const MAX_TEXT_CHARS = 10_000;
/** Longest `url` value, in characters. */
export const MAX_URL_CHARS = 2_048;
/** Longest `email` value, in characters. */
export const MAX_EMAIL_CHARS = 254;
/** Longest `phone` value, in characters. */
export const MAX_PHONE_CHARS = 64;
/** Most options one `multi_select` value may list. */
export const MAX_SELECTED_OPTIONS = 100;
/** Longest filter text of a `TextContains` filter, in characters. */
export const MAX_FILTER_CHARS = 200;
/** Most attachments one `files` value may list. */
export const MAX_FILES = 50;
/** Most items one `checklist` value may hold. */
export const MAX_CHECKLIST_ITEMS = 200;
/** Longest text of one checklist item, in characters. */
export const MAX_CHECKLIST_TEXT_CHARS = 500;
/** Longest name (label or address) of a `place` value, in characters. */
export const MAX_PLACE_NAME_CHARS = 500;
/** Most pages one `relation` value may list. */
export const MAX_RELATED_PAGES = 100;
/** Most people one `person` value may list (`kb_types::properties::MAX_PEOPLE`, M13 slice 2). */
export const MAX_PEOPLE = 20;

/** Every kind, in the order the UI lists them (`PropertyKind::ALL`). */
export const PROPERTY_KINDS: readonly PropertyKind[] = [
  "text",
  "number",
  "select",
  "multi_select",
  "date",
  "checkbox",
  "url",
  "status",
  "email",
  "phone",
  "files",
  "checklist",
  "place",
  "person",
  "relation",
  "rollup",
  "created_time",
  "last_edited_time",
  "created_by",
  "last_edited_by",
  "formula",
];

/**
 * Whether a property of `kind` is computed (from the page itself: its creation or last edit, the
 * time or the device; or, for a rollup, from its related pages): it can be added to the schema, a
 * page stores no value for it and `pagePropertySet` refuses it (`PropertyKind::is_computed`).
 */
export function isComputedPropertyKind(kind: PropertyKind): boolean {
  return (
    kind === "rollup" ||
    kind === "created_time" ||
    kind === "last_edited_time" ||
    kind === "created_by" ||
    kind === "last_edited_by" ||
    kind === "formula"
  );
}

/** The nine colours of an option, in the order the pickers list them (`OptionColor::ALL`). */
export const OPTION_COLORS: readonly OptionColor[] = [
  "gray",
  "brown",
  "orange",
  "yellow",
  "green",
  "blue",
  "purple",
  "pink",
  "red",
];

/** The three groups of a `status` option, in order. */
export const STATUS_GROUPS: readonly StatusGroup[] = ["todo", "in_progress", "done"];

/** One of the options a new `status` property starts with. */
export interface DefaultStatusOption {
  name: string;
  group: StatusGroup;
  color: OptionColor;
}

/** The three options a new `status` property starts with, in order (`DEFAULT_STATUS_OPTIONS`). */
export const DEFAULT_STATUS_OPTIONS: readonly DefaultStatusOption[] = [
  { name: "Not started", group: "todo", color: "gray" },
  { name: "In progress", group: "in_progress", color: "blue" },
  { name: "Done", group: "done", color: "green" },
];

/** Whether a property of `kind` keeps a list of options (`select`, `multi_select`, `status`). */
export function propertyKindHasOptions(kind: PropertyKind): boolean {
  return kind === "select" || kind === "multi_select" || kind === "status";
}

/** The message of a write to a computed property (the field of the error is `propertyId`). */
export const COMPUTED_PROPERTY_MESSAGE = "this property is computed from the page and cannot be set";

/** The kind of property a value belongs to (`PropertyValue::kind`). */
export function propertyValueKind(value: PropertyValue): PropertyKind {
  return value.kind;
}

// ----- small text helpers (characters are code points, as Rust counts them) ------------------

/** Length in code points (what Rust's `chars().count()` is), not UTF-16 units. */
export function codePointLength(text: string): number {
  let length = 0;
  for (let at = 0; at < text.length; at += 1) {
    const unit = text.charCodeAt(at);
    // A high surrogate followed by a low one is one character.
    if (unit >= 0xd800 && unit <= 0xdbff && at + 1 < text.length) {
      const next = text.charCodeAt(at + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        at += 1;
      }
    }
    length += 1;
  }
  return length;
}

/** `str::trim().is_empty()`: nothing but Unicode white space. */
function isBlank(text: string): boolean {
  return /^\p{White_Space}*$/u.test(text);
}

// ----- urls ---------------------------------------------------------------------------------

/**
 * The schemes of a `url` value. Narrower than "any absolute URL" on purpose: a value is shown
 * as a link, and `javascript:` or `file:` links do not belong in notes.
 */
const URL_SCHEMES: readonly string[] = ["http", "https", "mailto", "tel"];

/**
 * Whether `text` is an absolute URL of an accepted scheme: `http(s)://host...`, or `mailto:` or
 * `tel:` followed by something; no whitespace or control character anywhere, at most
 * {@link MAX_URL_CHARS} characters. (`kb-model`'s `valid_url`.) The empty text is not a URL;
 * as a property value it means "no value".
 */
export function isValidUrl(text: string): boolean {
  if (codePointLength(text) > MAX_URL_CHARS || /[\p{White_Space}\p{Cc}]/u.test(text)) {
    return false;
  }
  const colon = text.indexOf(":");
  if (colon < 0) {
    return false;
  }
  const scheme = text.slice(0, colon);
  const rest = text.slice(colon + 1);
  if (!/^[A-Za-z][A-Za-z0-9+.-]*$/u.test(scheme)) {
    return false;
  }
  const lower = scheme.toLowerCase();
  if (!URL_SCHEMES.includes(lower)) {
    return false;
  }
  if (lower === "http" || lower === "https") {
    if (!rest.startsWith("//")) {
      return false;
    }
    // The host part: up to the first `/`, `?` or `#`; it must not be empty.
    return (rest.slice(2).split(/[/?#]/u, 1)[0] ?? "") !== "";
  }
  return rest !== "";
}

// ----- values -------------------------------------------------------------------------------

/** What a value that is not even shaped like its kind is called (the core's serde refuses these before any rule). */
const MALFORMED = "the value is malformed";

function tooLong(max: number): string {
  return `must be at most ${String(max)} characters`;
}

/** A non-blank single line of at most `max` characters, kept as typed (`single_line`). */
function singleLineProblem(text: unknown, max: number): string | undefined {
  if (typeof text !== "string") {
    return MALFORMED;
  }
  if (isBlank(text)) {
    return "must not be empty";
  }
  if (codePointLength(text) > max) {
    return tooLong(max);
  }
  if (/\p{Cc}/u.test(text)) {
    return "must not contain control characters";
  }
  return undefined;
}

/**
 * The message of the first rule `value` breaks, or `undefined` when it follows its own rules
 * (`kb-model`'s `value::check`, kind-level: whether an option id belongs to a property is
 * the caller's check). The empty text, URL and multi-select list break no rule: they mean
 * "no value" (see {@link normalisePropertyValue}). The messages are the core's, for the field
 * `value`.
 */
export function propertyValueProblem(value: PropertyValue): string | undefined {
  switch (value.kind) {
    case "text": {
      if (typeof value.value !== "string") {
        return MALFORMED;
      }
      if (codePointLength(value.value) > MAX_TEXT_CHARS) {
        return tooLong(MAX_TEXT_CHARS);
      }
      // Free text may span lines: no control characters but tab, line feed and carriage return.
      if (/(?![\t\n\r])\p{Cc}/u.test(value.value)) {
        return "must not contain control characters other than tabs and line breaks";
      }
      return undefined;
    }
    case "number": {
      if (typeof value.value !== "number") {
        return MALFORMED;
      }
      return Number.isFinite(value.value) ? undefined : "must be a finite number";
    }
    case "checkbox":
      return typeof value.value === "boolean" ? undefined : MALFORMED;
    case "select":
    case "status":
      return typeof value.value === "string" ? undefined : MALFORMED;
    case "multi_select": {
      const ids: unknown = value.value;
      if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string")) {
        return MALFORMED;
      }
      if (ids.length > MAX_SELECTED_OPTIONS) {
        return `must list at most ${String(MAX_SELECTED_OPTIONS)} options`;
      }
      return new Set(ids).size === ids.length ? undefined : "option ids must be distinct";
    }
    case "date": {
      const day: unknown = value.value;
      if (typeof day !== "object" || day === null || typeof (day as { date?: unknown }).date !== "string") {
        return MALFORMED;
      }
      const { date, time } = day as { date: string; time?: unknown };
      if (!isIsoDate(date)) {
        return "the date must be a real day as YYYY-MM-DD";
      }
      if (time !== undefined && (typeof time !== "string" || !isIsoTime(time))) {
        return "the time must be HH:mm (24 hours)";
      }
      return undefined;
    }
    case "url": {
      if (typeof value.value !== "string") {
        return MALFORMED;
      }
      if (value.value === "") {
        return undefined;
      }
      if (codePointLength(value.value) > MAX_URL_CHARS) {
        return tooLong(MAX_URL_CHARS);
      }
      return isValidUrl(value.value) ? undefined : "must be an absolute http, https, mailto or tel URL without spaces";
    }
    case "email":
      return singleLineProblem(value.value, MAX_EMAIL_CHARS);
    case "phone":
      return singleLineProblem(value.value, MAX_PHONE_CHARS);
    case "files": {
      const ids: unknown = value.value;
      if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string" && canonicalId(id) !== undefined)) {
        return MALFORMED;
      }
      if (ids.length > MAX_FILES) {
        return `must list at most ${String(MAX_FILES)} files`;
      }
      // Ids are the same id in any notation (the core parses them before it compares).
      return new Set(ids.map((id: string) => canonicalId(id))).size === ids.length
        ? undefined
        : "attachment ids must be distinct";
    }
    case "checklist": {
      const items: unknown = value.value;
      if (!Array.isArray(items) || !items.every(isChecklistItemShape)) {
        return MALFORMED;
      }
      if (items.length > MAX_CHECKLIST_ITEMS) {
        return `must hold at most ${String(MAX_CHECKLIST_ITEMS)} items`;
      }
      if (new Set(items.map((item) => canonicalId(item.id))).size !== items.length) {
        return "item ids must be distinct";
      }
      for (const item of items) {
        if (codePointLength(item.text) > MAX_CHECKLIST_TEXT_CHARS) {
          return `an item must be at most ${String(MAX_CHECKLIST_TEXT_CHARS)} characters`;
        }
        if (/\p{Cc}/u.test(item.text)) {
          return "an item must not contain control characters";
        }
      }
      return undefined;
    }
    case "place":
      return placeProblem(value.value);
    case "relation": {
      const pages: unknown = value.value;
      if (!Array.isArray(pages) || !pages.every((id) => typeof id === "string" && canonicalId(id) !== undefined)) {
        return MALFORMED;
      }
      if (pages.length > MAX_RELATED_PAGES) {
        return `must list at most ${String(MAX_RELATED_PAGES)} pages`;
      }
      // Ids are the same id in any notation (the core parses them before it compares).
      return new Set(pages.map((id: string) => canonicalId(id))).size === pages.length
        ? undefined
        : "page ids must be distinct";
    }
    case "person": {
      // The structural rules of a Person value (ADR-0049); that every account is a member of the space when the value is
      // written is the core's rule (`person_value_check`), kept by the mirror's `pageSet`.
      const people: unknown = value.value;
      if (!Array.isArray(people) || !people.every((id) => typeof id === "string" && canonicalId(id) !== undefined)) {
        return MALFORMED;
      }
      if (people.length > MAX_PEOPLE) {
        return `must list at most ${String(MAX_PEOPLE)} people`;
      }
      return new Set(people.map((id: string) => canonicalId(id))).size === people.length
        ? undefined
        : "people must be distinct";
    }
    case "rollup":
    case "formula":
    case "created_time":
    case "last_edited_time":
    case "created_by":
    case "last_edited_by":
      return "is computed from the page and cannot be set";
    default:
      return MALFORMED;
  }
}

/**
 * The rules of a `place` (`check_place`): a non-blank single-line name of at most
 * {@link MAX_PLACE_NAME_CHARS} characters; the coordinates both or neither, finite, latitude in
 * [-90, 90], longitude in [-180, 180].
 */
function placeProblem(place: unknown): string | undefined {
  if (typeof place !== "object" || place === null) {
    return MALFORMED;
  }
  const { name, lat, lon } = place as { name?: unknown; lat?: unknown; lon?: unknown };
  const optionalNumber = (n: unknown): boolean => n === undefined || typeof n === "number";
  if (typeof name !== "string" || !optionalNumber(lat) || !optionalNumber(lon)) {
    return MALFORMED;
  }
  const named = singleLineProblem(name, MAX_PLACE_NAME_CHARS);
  if (named !== undefined) {
    return named;
  }
  if (lat === undefined && lon === undefined) {
    return undefined;
  }
  if (typeof lat !== "number" || typeof lon !== "number") {
    return "the latitude and the longitude go together";
  }
  if (!Number.isFinite(lat) || lat < -90 || lat > 90) {
    return "the latitude must be between -90 and 90";
  }
  if (!Number.isFinite(lon) || lon < -180 || lon > 180) {
    return "the longitude must be between -180 and 180";
  }
  return undefined;
}

/** Whether `item` has the shape of a checklist item: a UUID text id, a text and a flag. */
function isChecklistItemShape(item: unknown): item is ChecklistItem {
  if (typeof item !== "object" || item === null) {
    return false;
  }
  const { id, text, done } = item as { id?: unknown; text?: unknown; done?: unknown };
  return typeof id === "string" && canonicalId(id) !== undefined && typeof text === "string" && typeof done === "boolean";
}

/** Whether `value` stands for "no value": an empty text, an empty URL or an empty list (options, files, items). */
export function isEmptyPropertyValue(value: PropertyValue): boolean {
  switch (value.kind) {
    case "text":
    case "url":
      return value.value === "";
    case "multi_select":
    case "files":
    case "relation":
    case "person":
    case "checklist":
      return value.value.length === 0;
    default:
      return false;
  }
}

/**
 * Whether `value` is acceptable for a property of `kind` as far as its own rules go: it is of
 * that kind and breaks none of them (`kb-model`'s `value::check`); the empty text, URL and list
 * are acceptable too, they clear the value. It does not know the property's options: an option
 * id is checked by the core against the property.
 */
export function isValidPropertyValue(kind: PropertyKind, value: PropertyValue): boolean {
  return value.kind === kind && propertyValueProblem(value) === undefined;
}

/**
 * Checks a value against its own rules and normalises it: `null` for the values that mean "no
 * value" (an empty text, URL or list), the value itself otherwise (text is kept as typed; the ids
 * of files and checklist items are written in the canonical form, as the core stores them).
 * Throws `InvalidInput` for the field `value` with the core's message when a rule is broken
 * (`kb-model`'s `normalise_value`).
 */
export function normalisePropertyValue(value: PropertyValue): PropertyValue | null {
  const problem = propertyValueProblem(value);
  if (problem !== undefined) {
    throw invalid("value", problem);
  }
  if (isEmptyPropertyValue(value)) {
    return null;
  }
  switch (value.kind) {
    case "files":
      return { kind: "files", value: value.value.map((id) => canonicalId(id) ?? id) };
    case "relation":
      return { kind: "relation", value: value.value.map((id) => canonicalId(id) ?? id) };
    case "person":
      return { kind: "person", value: value.value.map((id) => canonicalId(id) ?? id) };
    case "checklist":
      return {
        kind: "checklist",
        value: value.value.map((item) => ({ id: canonicalId(item.id) ?? item.id, text: item.text, done: item.done })),
      };
    case "place": {
      const place: PlaceValue = { name: value.value.name };
      if (value.value.lat !== undefined && value.value.lon !== undefined) {
        place.lat = value.value.lat;
        place.lon = value.value.lon;
      }
      return { kind: "place", value: place };
    }
    default:
      return value;
  }
}

/** The option ids a value refers to, in order (none for the kinds without a list). */
export function propertyValueOptions(value: PropertyValue): readonly OptionId[] {
  switch (value.kind) {
    case "select":
    case "status":
      return [value.value];
    case "multi_select":
      return value.value;
    default:
      return [];
  }
}

/** The attachments a value attaches, in order (none for the kinds other than `files`). */
export function propertyValueFiles(value: PropertyValue): readonly AttachmentId[] {
  return value.kind === "files" ? value.value : [];
}

/** The pages a value lists, in order (none for the kinds other than `relation`). */
export function propertyValuePages(value: PropertyValue): readonly PageId[] {
  return value.kind === "relation" ? value.value : [];
}

/** How many items of a checklist are done, and how many it has. */
export function checklistProgress(items: readonly ChecklistItem[]): { done: number; total: number } {
  return { done: items.filter((item) => item.done).length, total: items.length };
}

/** A copy of a value that shares nothing with the original. */
export function copyPropertyValue(value: PropertyValue): PropertyValue {
  switch (value.kind) {
    case "multi_select":
      return { kind: "multi_select", value: [...value.value] };
    case "files":
      return { kind: "files", value: [...value.value] };
    case "relation":
      return { kind: "relation", value: [...value.value] };
    case "person":
      return { kind: "person", value: [...value.value] };
    case "rollup":
      return { kind: "rollup", value: { function: value.value.function, result: copyRollupResult(value.value.result) } };
    case "formula":
      return { kind: "formula", value: copyFormulaResult(value.value) };
    case "checklist":
      return { kind: "checklist", value: value.value.map((item) => ({ ...item })) };
    case "place": {
      const copy: PlaceValue = { name: value.value.name };
      if (value.value.lat !== undefined) {
        copy.lat = value.value.lat;
      }
      if (value.value.lon !== undefined) {
        copy.lon = value.value.lon;
      }
      return { kind: "place", value: copy };
    }
    case "date": {
      const copy: DateValue = { date: value.value.date };
      if (value.value.time !== undefined) {
        copy.time = value.value.time;
      }
      return { kind: "date", value: copy };
    }
    default:
      return { kind: value.kind, value: value.value } as PropertyValue;
  }
}

/** A copy of a formula's result that shares nothing with the original. */
export function copyFormulaResult(result: FormulaResult): FormulaResult {
  switch (result.type) {
    case "date": {
      const copy: DateValue = { date: result.data.date };
      if (result.data.time !== undefined) {
        copy.time = result.data.time;
      }
      return { type: "date", data: copy };
    }
    case "list":
      return { type: "list", data: result.data.map(copyFormulaResult) };
    default:
      return { type: result.type, data: result.data } as FormulaResult;
  }
}

/** Whether two formula results are the same: same type and same content (a list item by item). */
export function formulaResultsEqual(a: FormulaResult, b: FormulaResult): boolean {
  if (a.type !== b.type) {
    return false;
  }
  switch (a.type) {
    case "date": {
      const other = (b as typeof a).data;
      return a.data.date === other.date && a.data.time === other.time;
    }
    case "list": {
      const other = (b as typeof a).data;
      return (
        a.data.length === other.length &&
        a.data.every((item, at) => {
          const peer = other[at];
          return peer !== undefined && formulaResultsEqual(item, peer);
        })
      );
    }
    default:
      return a.data === (b as typeof a).data;
  }
}

/** A copy of a rollup result that shares nothing with the original. */
export function copyRollupResult(result: RollupResult): RollupResult {
  switch (result.type) {
    case "number":
      return { type: "number", data: result.data };
    case "date": {
      const copy: DateValue = { date: result.data.date };
      if (result.data.time !== undefined) {
        copy.time = result.data.time;
      }
      return { type: "date", data: copy };
    }
    case "values":
      return { type: "values", data: result.data.map(copyPropertyValue) };
    case "progress":
      return { type: "progress", data: { done: result.data.done, total: result.data.total } };
    default:
      return result;
  }
}

function rollupResultsEqual(a: RollupResult, b: RollupResult): boolean {
  if (a.type !== b.type) {
    return false;
  }
  switch (a.type) {
    case "number":
      return a.data === (b as typeof a).data;
    case "date": {
      const other = (b as typeof a).data;
      return a.data.date === other.date && a.data.time === other.time;
    }
    case "values": {
      const other = (b as typeof a).data;
      return a.data.length === other.length && a.data.every((value, at) => {
        const peer = other[at];
        return peer !== undefined && propertyValuesEqual(value, peer);
      });
    }
    case "progress": {
      const other = (b as typeof a).data;
      return a.data.done === other.done && a.data.total === other.total;
    }
    default:
      return false;
  }
}

/**
 * Whether two values are the same: same kind and same content (a multi-select compares its
 * list in order, a date its day and its time, a date without a time only equals one without).
 */
export function propertyValuesEqual(a: PropertyValue, b: PropertyValue): boolean {
  if (a.kind !== b.kind) {
    return false;
  }
  switch (a.kind) {
    case "multi_select":
    case "files":
    case "person":
    case "relation": {
      const other = (b as typeof a).value;
      return a.value.length === other.length && a.value.every((id, at) => id === other[at]);
    }
    case "rollup": {
      const other = (b as typeof a).value;
      return a.value.function === other.function && rollupResultsEqual(a.value.result, other.result);
    }
    case "formula":
      return formulaResultsEqual(a.value, (b as typeof a).value);
    case "checklist": {
      const other = (b as typeof a).value;
      return (
        a.value.length === other.length &&
        a.value.every((item, at) => {
          const peer = other[at];
          return peer !== undefined && item.id === peer.id && item.text === peer.text && item.done === peer.done;
        })
      );
    }
    case "date": {
      const other = (b as typeof a).value;
      return a.value.date === other.date && a.value.time === other.time;
    }
    case "place": {
      const other = (b as typeof a).value;
      return a.value.name === other.name && a.value.lat === other.lat && a.value.lon === other.lon;
    }
    default:
      return a.value === (b as typeof a).value;
  }
}

/**
 * The searchable text of a value, given the name of an option id, the file name of an
 * attachment id and the title of a page id: option names (a multi-select joins them with a comma
 * and a space, in list order) for the kinds with a list, the string for `text`, `url`, `email` and
 * `phone`, the file names of a `files` value, the titles of the pages of a `relation` (blank ones
 * left out), the item texts of a `checklist` (blank ones left out; joined the same way), the name of a `place`,
 * the names of the people of a `person` value (an account that has no name here is left out), nothing
 * (`undefined`) for the others and for blank text (`kb-model`'s `search_text`; the text index holds one row per
 * property that has some). A formula's result is the text `format()` makes of it (ADR-0055: a number as JavaScript
 * writes it, a page by its title, a person by name, a list joined with `, `); the text index holds none, since it
 * reads stored values only.
 */
export function propertyValueText(
  value: PropertyValue,
  optionName: (id: OptionId) => string | undefined,
  fileName: (id: AttachmentId) => string | undefined = () => undefined,
  pageTitle: (id: PageId) => string | undefined = () => undefined,
  personName: (id: string) => string | undefined = () => undefined,
): string | undefined {
  let text: string;
  switch (value.kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
      text = value.value;
      break;
    case "select":
    case "status": {
      const name = optionName(value.value);
      if (name === undefined) {
        return undefined;
      }
      text = name;
      break;
    }
    case "multi_select":
      text = value.value
        .map((id) => optionName(id))
        .filter((name): name is string => name !== undefined)
        .join(", ");
      break;
    case "files":
      text = value.value
        .map((id) => fileName(id))
        .filter((name): name is string => name !== undefined)
        .join(", ");
      break;
    case "place":
      text = value.value.name;
      break;
    case "relation":
      text = value.value
        .map((id) => pageTitle(id))
        .filter((title): title is string => title !== undefined && !isBlank(title))
        .join(", ");
      break;
    case "checklist":
      text = value.value
        .map((item) => item.text)
        .filter((item) => !isBlank(item))
        .join(", ");
      break;
    case "person":
      text = value.value
        .map((id) => personName(id))
        .filter((name): name is string => name !== undefined && !isBlank(name))
        .join(", ");
      break;
    case "formula":
      text = formatFormulaValue(value.value, {
        title: pageTitle,
        person: (id) => {
          const name = personName(id);
          return name === undefined ? undefined : { name, email: "" };
        },
      });
      break;
    default:
      return undefined;
  }
  return isBlank(text) ? undefined : text;
}

// ----- seeding (`MemoryTransportOptions.spaces`) ------------------------------------------------

/** An option to create when seeding a select, multi-select or status property, or Tags. */
export interface SeedOption {
  /** The option id; default: the next `newId` of the transport. */
  id?: OptionId;
  name: string;
  /** Default `"gray"`. */
  color?: OptionColor;
  /** `status` options only; default `"todo"`. */
  group?: StatusGroup;
}

/** A property to create when seeding a space (`SeedSpace.properties`). */
export interface SeedProperty {
  /** The property id; default: the next `newId` of the transport. */
  id?: PropertyId;
  name: string;
  kind: PropertyKind;
  /**
   * The property's options in order (select, multi-select and status only). A `status` property
   * without `options` starts with the three default options ({@link DEFAULT_STATUS_OPTIONS});
   * explicit `options` (even none) replace them.
   */
  options?: readonly SeedOption[];
  /**
   * A `relation`: `twoWay` makes the paired property too (named `backName`, default "<name> (back)"),
   * created right after this one. Default: one-way.
   */
  twoWay?: boolean;
  backName?: string;
  /**
   * A `rollup`: the NAME of the relation property it follows, the NAME of the target property (or
   * `"title"` for the titles of the related pages) and the function. The relation must be seeded
   * before the rollup.
   */
  rollup?: { relation: string; target: string; function: RollupFunction };
  /**
   * A `formula`: the expression in display form, the properties it reads by NAME (`prop("Priority") * 10`, the title as
   * `prop("title")`). It may read the properties seeded before it, and is checked like
   * `PropertyCreate` checks one (a seed that does not check throws).
   */
  formula?: string;
}

/** An item of a seeded checklist (`SeedValue`). */
export interface SeedChecklistItem {
  /** The item id; default: the next `newId` of the transport. */
  id?: ChecklistItemId;
  text: string;
  /** Default `false`. */
  done?: boolean;
}

/**
 * The value of a property on a seeded page (`SeedPage.properties`, keyed by the property's name),
 * by the kind of the property: a string for `text`, `url`, `email` and `phone`; a number; a
 * boolean for `checkbox`; `{ date, time? }` for `date` (a plain `YYYY-MM-DD` string is a date
 * without a time); the NAME of an option (the first one that has it) for `select` and `status`;
 * the names of options for `multi_select`; the ids of attachments (as seeded, see
 * `MemoryTransportOptions.attachments`) for `files`; the items for `checklist`; `{ name, lat?, lon? }`
 * for `place`; the TITLES of pages (the first page of the space with each) for `relation`. The
 * computed kinds, rollups included, take no seeded value.
 */
export type SeedValue =
  | string
  | number
  | boolean
  | readonly string[]
  | readonly SeedChecklistItem[]
  | { date: string; time?: string }
  | { name: string; lat?: number; lon?: number };
