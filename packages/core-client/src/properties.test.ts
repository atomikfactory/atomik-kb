// The pure rules of typed page properties (`./properties`): the constants of `kb-types`, the URL
// and value checks of `kb-model`'s `properties/value.rs` (same cases as its unit tests), the
// normalisation, the derived search text and the small value helpers.

import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import type { ChecklistItem } from "./generated/ChecklistItem";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyValue } from "./generated/PropertyValue";
import {
  DEFAULT_STATUS_OPTIONS,
  MAX_CHECKLIST_ITEMS,
  MAX_CHECKLIST_TEXT_CHARS,
  MAX_EMAIL_CHARS,
  MAX_FILES,
  MAX_FILTER_CHARS,
  MAX_NAME_CHARS,
  MAX_OPTIONS,
  MAX_PHONE_CHARS,
  MAX_PLACE_NAME_CHARS,
  MAX_PROPERTIES,
  MAX_SELECTED_OPTIONS,
  MAX_TEXT_CHARS,
  MAX_URL_CHARS,
  OPTION_COLORS,
  PROPERTY_KINDS,
  STATUS_GROUPS,
  TAGS_NAME,
  TAGS_PROPERTY_ID,
  checklistProgress,
  codePointLength,
  copyPropertyValue,
  isEmptyPropertyValue,
  isValidPropertyValue,
  isValidUrl,
  normalisePropertyValue,
  propertyKindHasOptions,
  propertyValueFiles,
  propertyValueKind,
  propertyValueOptions,
  propertyValueProblem,
  propertyValueText,
  propertyValuesEqual,
} from "./properties";

const option = (n: number): string => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const file = (n: number): string => `a7000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const item = (n: number, itemText: string, done = false): ChecklistItem => ({
  id: `c1000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
  text: itemText,
  done,
});
const checklist = (...items: ChecklistItem[]): PropertyValue => ({ kind: "checklist", value: items });
const text = (value: string): PropertyValue => ({ kind: "text", value });
const day = (date: string, time?: string): PropertyValue => ({
  kind: "date",
  value: time === undefined ? { date } : { date, time },
});

describe("constants", () => {
  it("are the ones of kb-types", () => {
    expect(TAGS_PROPERTY_ID).toBe("0000000a-0000-4000-8000-000000000001");
    expect(TAGS_NAME).toBe("Tags");
    expect([MAX_PROPERTIES, MAX_OPTIONS, MAX_NAME_CHARS, MAX_TEXT_CHARS]).toEqual([100, 500, 100, 10_000]);
    expect([MAX_URL_CHARS, MAX_EMAIL_CHARS, MAX_PHONE_CHARS, MAX_SELECTED_OPTIONS, MAX_FILTER_CHARS]).toEqual([
      2048, 254, 64, 100, 200,
    ]);
    expect([MAX_FILES, MAX_CHECKLIST_ITEMS, MAX_CHECKLIST_TEXT_CHARS, MAX_PLACE_NAME_CHARS]).toEqual([50, 200, 500, 500]);
  });

  it("list the kinds, colours and groups in the order the UI shows them", () => {
    expect(PROPERTY_KINDS).toEqual([
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
    ]);
    expect(OPTION_COLORS).toEqual(["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"]);
    expect(STATUS_GROUPS).toEqual(["todo", "in_progress", "done"]);
    expect(DEFAULT_STATUS_OPTIONS).toEqual([
      { name: "Not started", group: "todo", color: "gray" },
      { name: "In progress", group: "in_progress", color: "blue" },
      { name: "Done", group: "done", color: "green" },
    ]);
  });

  it("know which kinds keep a list of options", () => {
    const withOptions = PROPERTY_KINDS.filter((kind) => propertyKindHasOptions(kind));
    expect(withOptions).toEqual(["select", "multi_select", "status"]);
  });
});

describe("codePointLength", () => {
  it("counts characters, not UTF-16 units", () => {
    expect(codePointLength("")).toBe(0);
    expect(codePointLength("abc")).toBe(3);
    expect(codePointLength("\u{1F600}\u{1F600}")).toBe(2);
    expect(codePointLength("a\u{1F600}b")).toBe(3);
    // A lone surrogate counts as one.
    expect(codePointLength("\ud83d")).toBe(1);
  });
});

describe("isValidUrl", () => {
  it("accepts absolute URLs of the four schemes without spaces", () => {
    for (const ok of [
      "https://example.com",
      "http://localhost:8080/a?b=c#d",
      "HTTPS://Example.com/Path",
      "mailto:ada@example.com",
      "tel:+15550100",
      "https://example.com/ünï",
      "http://a",
      "https://example.com?x",
      "https://example.com#frag",
    ]) {
      expect(isValidUrl(ok), ok).toBe(true);
    }
  });

  it("refuses everything else", () => {
    for (const bad of [
      "",
      "example.com",
      "//example.com",
      "https://",
      "https:///path",
      "https://?x",
      "https:example.com",
      "javascript:alert(1)",
      "file:///etc/passwd",
      "ftp://example.com",
      "data:text/html,x",
      "mailto:",
      "tel:",
      "https://exa mple.com",
      " https://example.com",
      "https://example.com ",
      "https://example.com/\n",
      "https://example.com/ ",
      "https://example.com/\u0085",
      "1http://example.com",
      "ht tp://example.com",
      "-http://example.com",
    ]) {
      expect(isValidUrl(bad), JSON.stringify(bad)).toBe(false);
    }
  });

  it("allows at most 2048 characters, counted as characters", () => {
    const base = "https://example.com/";
    expect(isValidUrl(base + "a".repeat(MAX_URL_CHARS - base.length))).toBe(true);
    expect(isValidUrl(base + "a".repeat(MAX_URL_CHARS - base.length + 1))).toBe(false);
    // 2000 characters that are 4000 UTF-16 units.
    expect(isValidUrl(base + "\u{1F600}".repeat(MAX_URL_CHARS - base.length))).toBe(true);
  });
});

/** Whether a value passes its kind's rules, and whether it then counts as a value at all. */
function check(value: PropertyValue): "refused" | "empty" | "value" {
  const problem = propertyValueProblem(value);
  if (problem !== undefined) {
    return "refused";
  }
  return isEmptyPropertyValue(value) ? "empty" : "value";
}

describe("values follow their rules and empties mean no value", () => {
  it("text", () => {
    expect(check(text("x"))).toBe("value");
    expect(check(text("line\nbreak\ttab\rreturn"))).toBe("value");
    expect(check(text(""))).toBe("empty");
    expect(check(text("a\u0000"))).toBe("refused");
    expect(check(text("a\u007f"))).toBe("refused");
    expect(check(text("a\u0085"))).toBe("refused");
    expect(check(text("x".repeat(MAX_TEXT_CHARS + 1)))).toBe("refused");
    expect(check(text("x".repeat(MAX_TEXT_CHARS)))).toBe("value");
    expect(check(text("\u{1F600}".repeat(MAX_TEXT_CHARS)))).toBe("value");
    expect(propertyValueProblem(text("x".repeat(MAX_TEXT_CHARS + 1)))).toBe("must be at most 10000 characters");
    expect(propertyValueProblem(text("a\u0007"))).toBe(
      "must not contain control characters other than tabs and line breaks",
    );
  });

  it("number", () => {
    expect(check({ kind: "number", value: 0 })).toBe("value");
    expect(check({ kind: "number", value: -1.5e300 })).toBe("value");
    expect(check({ kind: "number", value: Number.NaN })).toBe("refused");
    expect(check({ kind: "number", value: Number.POSITIVE_INFINITY })).toBe("refused");
    expect(check({ kind: "number", value: Number.NEGATIVE_INFINITY })).toBe("refused");
    expect(propertyValueProblem({ kind: "number", value: Number.NaN })).toBe("must be a finite number");
  });

  it("checkbox, select and status take anything of their shape", () => {
    expect(check({ kind: "checkbox", value: false })).toBe("value");
    expect(check({ kind: "select", value: option(1) })).toBe("value");
    expect(check({ kind: "status", value: option(1) })).toBe("value");
  });

  it("multi_select", () => {
    expect(check({ kind: "multi_select", value: [] })).toBe("empty");
    expect(check({ kind: "multi_select", value: [option(1), option(2)] })).toBe("value");
    expect(check({ kind: "multi_select", value: [option(1), option(1)] })).toBe("refused");
    expect(propertyValueProblem({ kind: "multi_select", value: [option(1), option(1)] })).toBe(
      "option ids must be distinct",
    );
    const many = Array.from({ length: MAX_SELECTED_OPTIONS + 1 }, (_, n) => option(n));
    expect(check({ kind: "multi_select", value: many })).toBe("refused");
    expect(propertyValueProblem({ kind: "multi_select", value: many })).toBe("must list at most 100 options");
    expect(check({ kind: "multi_select", value: many.slice(0, MAX_SELECTED_OPTIONS) })).toBe("value");
  });

  it("files", () => {
    expect(check({ kind: "files", value: [] })).toBe("empty");
    expect(check({ kind: "files", value: [file(1), file(2)] })).toBe("value");
    expect(check({ kind: "files", value: [file(1), file(2), file(1)] })).toBe("refused");
    expect(propertyValueProblem({ kind: "files", value: [file(1), file(1)] })).toBe("attachment ids must be distinct");
    // The same id in another notation is the same id (the core parses ids before it compares them).
    expect(check({ kind: "files", value: [file(1), file(1).toUpperCase()] })).toBe("refused");
    expect(check({ kind: "files", value: [file(1), file(1).replaceAll("-", "")] })).toBe("refused");
    expect(check({ kind: "files", value: [file(1).replaceAll("-", ""), file(2)] })).toBe("value");
    const many = Array.from({ length: MAX_FILES + 1 }, (_, n) => file(n));
    expect(check({ kind: "files", value: many })).toBe("refused");
    expect(propertyValueProblem({ kind: "files", value: many })).toBe("must list at most 50 files");
    expect(check({ kind: "files", value: many.slice(0, MAX_FILES) })).toBe("value");
    // Existence is not checked: any well-formed id will do.
    expect(check({ kind: "files", value: [file(999)] })).toBe("value");
  });

  it("checklist", () => {
    expect(check(checklist())).toBe("empty");
    expect(check(checklist(item(1, "a"), item(2, "b", true)))).toBe("value");
    expect(check(checklist(item(1, "")))).toBe("value");
    expect(check(checklist(item(1, "a"), item(1, "b")))).toBe("refused");
    expect(propertyValueProblem(checklist(item(1, "a"), item(1, "b")))).toBe("item ids must be distinct");
    expect(check(checklist(item(1, "x".repeat(MAX_CHECKLIST_TEXT_CHARS))))).toBe("value");
    expect(check(checklist(item(1, "x".repeat(MAX_CHECKLIST_TEXT_CHARS + 1))))).toBe("refused");
    expect(propertyValueProblem(checklist(item(1, "x".repeat(MAX_CHECKLIST_TEXT_CHARS + 1))))).toBe(
      "an item must be at most 500 characters",
    );
    // Characters, not UTF-16 units.
    expect(check(checklist(item(1, "\u{1F600}".repeat(MAX_CHECKLIST_TEXT_CHARS))))).toBe("value");
    for (const bad of ["line\nbreak", "tab\there", "nul\u0000", "bell\u0007", "del\u007f"]) {
      expect(propertyValueProblem(checklist(item(1, bad))), JSON.stringify(bad)).toBe(
        "an item must not contain control characters",
      );
    }
    const many = Array.from({ length: MAX_CHECKLIST_ITEMS + 1 }, (_, n) => item(n, String(n)));
    expect(check(checklist(...many))).toBe("refused");
    expect(propertyValueProblem(checklist(...many))).toBe("must hold at most 200 items");
    expect(check(checklist(...many.slice(0, MAX_CHECKLIST_ITEMS)))).toBe("value");
  });

  it("date", () => {
    expect(check(day("2026-10-07"))).toBe("value");
    expect(check(day("2026-10-07", "14:30"))).toBe("value");
    expect(check(day("2024-02-29", "00:00"))).toBe("value");
    expect(check(day("0001-01-01"))).toBe("value");
    expect(check(day("9999-12-31", "23:59"))).toBe("value");
    expect(check(day("2026-02-29"))).toBe("refused");
    expect(check(day("1900-02-29"))).toBe("refused");
    expect(check(day("2000-02-29"))).toBe("value");
    expect(check(day("2026-13-01"))).toBe("refused");
    expect(check(day("0000-10-07"))).toBe("refused");
    expect(check(day("26-10-07"))).toBe("refused");
    expect(check(day("2026-10-07T00:00"))).toBe("refused");
    expect(check(day("2026-10-07", "24:00"))).toBe("refused");
    expect(check(day("2026-10-07", "9:05"))).toBe("refused");
    expect(check(day("2026-10-07", "10:60"))).toBe("refused");
    expect(check(day("2026-10-07", ""))).toBe("refused");
    expect(propertyValueProblem(day("2026-02-30"))).toBe("the date must be a real day as YYYY-MM-DD");
    expect(propertyValueProblem(day("2026-10-07", "25:00"))).toBe("the time must be HH:mm (24 hours)");
  });

  it("url", () => {
    expect(check({ kind: "url", value: "" })).toBe("empty");
    expect(check({ kind: "url", value: "https://example.com" })).toBe("value");
    expect(check({ kind: "url", value: "example.com" })).toBe("refused");
    expect(propertyValueProblem({ kind: "url", value: "example.com" })).toBe(
      "must be an absolute http, https, mailto or tel URL without spaces",
    );
    const long = { kind: "url", value: `https://example.com/${"a".repeat(MAX_URL_CHARS)}` } as const;
    expect(propertyValueProblem(long)).toBe("must be at most 2048 characters");
  });

  it("email and phone are non-blank single lines kept as typed", () => {
    expect(check({ kind: "email", value: "ada@example.com" })).toBe("value");
    // The format is the UI's check.
    expect(check({ kind: "email", value: "not even an address" })).toBe("value");
    expect(check({ kind: "email", value: "" })).toBe("refused");
    expect(check({ kind: "email", value: "   " })).toBe("refused");
    expect(check({ kind: "email", value: " 　" })).toBe("refused");
    expect(check({ kind: "email", value: "a\nb" })).toBe("refused");
    expect(check({ kind: "email", value: "a".repeat(MAX_EMAIL_CHARS + 1) })).toBe("refused");
    expect(check({ kind: "email", value: "a".repeat(MAX_EMAIL_CHARS) })).toBe("value");
    expect(check({ kind: "phone", value: "+1 (555) 010-0100" })).toBe("value");
    expect(check({ kind: "phone", value: "" })).toBe("refused");
    expect(check({ kind: "phone", value: "1".repeat(MAX_PHONE_CHARS + 1) })).toBe("refused");
    expect(propertyValueProblem({ kind: "email", value: "" })).toBe("must not be empty");
    expect(propertyValueProblem({ kind: "email", value: "a\nb" })).toBe("must not contain control characters");
    expect(propertyValueProblem({ kind: "email", value: "a".repeat(255) })).toBe("must be at most 254 characters");
    expect(propertyValueProblem({ kind: "phone", value: "1".repeat(65) })).toBe("must be at most 64 characters");
  });

  it("a value that is not even shaped like its kind is malformed, never a crash", () => {
    const malformed = [
      { kind: "text", value: 1 },
      { kind: "number", value: "3" },
      { kind: "checkbox", value: "true" },
      { kind: "select", value: 5 },
      { kind: "multi_select", value: "a" },
      { kind: "multi_select", value: [1] },
      { kind: "date", value: "2026-10-07" },
      { kind: "date", value: null },
      { kind: "date", value: { date: "2026-10-07", time: 5 } },
      { kind: "url", value: null },
      { kind: "email", value: 1 },
      { kind: "files", value: "a" },
      { kind: "files", value: [1] },
      { kind: "files", value: ["not an id"] },
      { kind: "checklist", value: "a" },
      { kind: "checklist", value: ["a"] },
      { kind: "checklist", value: [null] },
      { kind: "checklist", value: [{ id: "nope", text: "a", done: false }] },
      { kind: "checklist", value: [{ id: item(1, "").id, text: 1, done: false }] },
      { kind: "checklist", value: [{ id: item(1, "").id, text: "a" }] },
      { kind: "unknown", value: "x" },
    ] as unknown as PropertyValue[];
    for (const value of malformed) {
      expect(propertyValueProblem(value), JSON.stringify(value)).toBeTypeOf("string");
    }
  });
});

describe("isValidPropertyValue", () => {
  it("wants the kind of the property, and a value that follows its rules; empties are fine", () => {
    expect(isValidPropertyValue("text", text("x"))).toBe(true);
    expect(isValidPropertyValue("text", text(""))).toBe(true);
    expect(isValidPropertyValue("text", { kind: "number", value: 1 })).toBe(false);
    expect(isValidPropertyValue("number", { kind: "number", value: Number.NaN })).toBe(false);
    expect(isValidPropertyValue("url", { kind: "url", value: "javascript:x" })).toBe(false);
    expect(isValidPropertyValue("multi_select", { kind: "multi_select", value: [] })).toBe(true);
    expect(isValidPropertyValue("files", { kind: "files", value: [file(1)] })).toBe(true);
    expect(isValidPropertyValue("files", { kind: "files", value: [] })).toBe(true);
    expect(isValidPropertyValue("checklist", checklist(item(1, "a")))).toBe(true);
    expect(isValidPropertyValue("checklist", { kind: "files", value: [file(1)] })).toBe(false);
    expect(isValidPropertyValue("files", checklist(item(1, "a")))).toBe(false);
    expect(isValidPropertyValue("email", { kind: "email", value: "" })).toBe(false);
    expect(isValidPropertyValue("date", day("2026-10-07", "14:30"))).toBe(true);
    const kinds: PropertyKind[] = [...PROPERTY_KINDS];
    expect(kinds.filter((kind) => isValidPropertyValue(kind, text("x")))).toEqual(["text"]);
  });
});

describe("normalisePropertyValue", () => {
  it("returns null for the empty text, URL and list, and the value itself otherwise", () => {
    expect(normalisePropertyValue(text(""))).toBeNull();
    expect(normalisePropertyValue({ kind: "url", value: "" })).toBeNull();
    expect(normalisePropertyValue({ kind: "multi_select", value: [] })).toBeNull();
    expect(normalisePropertyValue({ kind: "files", value: [] })).toBeNull();
    expect(normalisePropertyValue(checklist())).toBeNull();
    const kept = text("  as typed  ");
    expect(normalisePropertyValue(kept)).toBe(kept);
    expect(normalisePropertyValue({ kind: "checkbox", value: false })).toEqual({ kind: "checkbox", value: false });
  });

  it("writes the ids of files and checklist items in the canonical form, as the core stores them", () => {
    const loud = file(1).toUpperCase();
    expect(normalisePropertyValue({ kind: "files", value: [loud, file(2).replaceAll("-", "")] })).toEqual({
      kind: "files",
      value: [file(1), file(2)],
    });
    const spelled = { id: item(1, "").id.replaceAll("-", "").toUpperCase(), text: "a", done: true };
    expect(normalisePropertyValue({ kind: "checklist", value: [spelled] })).toEqual({
      kind: "checklist",
      value: [{ id: item(1, "").id, text: "a", done: true }],
    });
  });

  it("throws InvalidInput for the field value with the core's message", () => {
    const refused: [PropertyValue, string][] = [
      [{ kind: "files", value: [file(1), file(1)] }, "attachment ids must be distinct"],
      [checklist(item(1, "a"), item(1, "b")), "item ids must be distinct"],
      [checklist(item(1, "a\nb")), "an item must not contain control characters"],
      [{ kind: "number", value: Number.NaN }, "must be a finite number"],
      [{ kind: "email", value: " " }, "must not be empty"],
      [day("2026-02-30"), "the date must be a real day as YYYY-MM-DD"],
    ];
    for (const [value, message] of refused) {
      let error: unknown;
      try {
        normalisePropertyValue(value);
      } catch (caught) {
        error = caught;
      }
      expect(error).toBeInstanceOf(CoreError);
      expect((error as CoreError).toApiError()).toEqual({
        code: "InvalidInput",
        message,
        details: { field: "value" },
      });
    }
  });
});

describe("propertyValueOptions and propertyValueKind", () => {
  it("name the option ids of a value in order, and its kind", () => {
    expect(propertyValueOptions({ kind: "select", value: option(1) })).toEqual([option(1)]);
    expect(propertyValueOptions({ kind: "status", value: option(2) })).toEqual([option(2)]);
    expect(propertyValueOptions({ kind: "multi_select", value: [option(3), option(1)] })).toEqual([
      option(3),
      option(1),
    ]);
    expect(propertyValueOptions(text("x"))).toEqual([]);
    expect(propertyValueOptions({ kind: "files", value: [file(1)] })).toEqual([]);
    expect(propertyValueKind(day("2026-10-07"))).toBe("date");
    expect(propertyValueKind({ kind: "files", value: [] })).toBe("files");
    expect(propertyValueKind(checklist())).toBe("checklist");
  });

  it("name the attachments of a files value in order, and none for the other kinds", () => {
    expect(propertyValueFiles({ kind: "files", value: [file(3), file(1)] })).toEqual([file(3), file(1)]);
    expect(propertyValueFiles(text("x"))).toEqual([]);
    expect(propertyValueFiles(checklist(item(1, "a")))).toEqual([]);
  });
});

describe("checklistProgress", () => {
  it("counts the items done and the items", () => {
    expect(checklistProgress([])).toEqual({ done: 0, total: 0 });
    expect(checklistProgress([item(1, "a", true), item(2, "b"), item(3, "c", true)])).toEqual({ done: 2, total: 3 });
  });
});

describe("propertyValueText", () => {
  const names = new Map([
    [option(1), "urgent"],
    [option(2), "work"],
  ]);
  const name = (id: string): string | undefined => names.get(id);

  it("is option names and text values only", () => {
    expect(propertyValueText({ kind: "multi_select", value: [option(1), option(9), option(2)] }, name)).toBe(
      "urgent, work",
    );
    expect(propertyValueText({ kind: "select", value: option(2) }, name)).toBe("work");
    expect(propertyValueText({ kind: "status", value: option(9) }, name)).toBeUndefined();
    expect(propertyValueText(text("Hello"), name)).toBe("Hello");
    expect(propertyValueText({ kind: "url", value: "https://a.b" }, name)).toBe("https://a.b");
    expect(propertyValueText({ kind: "email", value: "a@b.c" }, name)).toBe("a@b.c");
    expect(propertyValueText({ kind: "phone", value: "+1 555" }, name)).toBe("+1 555");
    expect(propertyValueText({ kind: "number", value: 1 }, name)).toBeUndefined();
    expect(propertyValueText({ kind: "checkbox", value: true }, name)).toBeUndefined();
    expect(propertyValueText(day("2026-10-07"), name)).toBeUndefined();
  });

  it("is nothing for blank text", () => {
    expect(propertyValueText(text("   \n"), name)).toBeUndefined();
    expect(propertyValueText({ kind: "multi_select", value: [option(9)] }, name)).toBeUndefined();
  });

  it("is the file names of a files value and the item texts of a checklist", () => {
    const fileNames = new Map([
      [file(1), "plan.pdf"],
      [file(2), "atomik.png"],
    ]);
    const fileName = (id: string): string | undefined => fileNames.get(id);
    expect(propertyValueText({ kind: "files", value: [file(2), file(9), file(1)] }, name, fileName)).toBe(
      "atomik.png, plan.pdf",
    );
    expect(propertyValueText({ kind: "files", value: [file(9)] }, name, fileName)).toBeUndefined();
    expect(propertyValueText({ kind: "files", value: [file(1)] }, name)).toBeUndefined();
    expect(propertyValueText(checklist(item(1, "Pack"), item(2, "  "), item(3, "Call Ada", true)), name)).toBe(
      "Pack, Call Ada",
    );
    expect(propertyValueText(checklist(item(1, "")), name)).toBeUndefined();
  });
});

describe("propertyValuesEqual and copyPropertyValue", () => {
  it("compare kind and content (a list in order, a date by day and time)", () => {
    expect(propertyValuesEqual(text("a"), text("a"))).toBe(true);
    expect(propertyValuesEqual(text("a"), text("b"))).toBe(false);
    expect(propertyValuesEqual(text("a"), { kind: "email", value: "a" })).toBe(false);
    expect(
      propertyValuesEqual({ kind: "multi_select", value: [option(1), option(2)] }, { kind: "multi_select", value: [option(1), option(2)] }),
    ).toBe(true);
    expect(
      propertyValuesEqual({ kind: "multi_select", value: [option(1), option(2)] }, { kind: "multi_select", value: [option(2), option(1)] }),
    ).toBe(false);
    expect(propertyValuesEqual(day("2026-10-07"), day("2026-10-07"))).toBe(true);
    expect(propertyValuesEqual(day("2026-10-07"), day("2026-10-07", "10:00"))).toBe(false);
    expect(propertyValuesEqual(day("2026-10-07", "10:00"), day("2026-10-07", "10:00"))).toBe(true);
    expect(propertyValuesEqual({ kind: "checkbox", value: false }, { kind: "checkbox", value: false })).toBe(true);
    expect(propertyValuesEqual({ kind: "number", value: 1 }, { kind: "number", value: 2 })).toBe(false);
    const pair: PropertyValue = { kind: "files", value: [file(1), file(2)] };
    expect(propertyValuesEqual(pair, { kind: "files", value: [file(1), file(2)] })).toBe(true);
    expect(propertyValuesEqual(pair, { kind: "files", value: [file(2), file(1)] })).toBe(false);
    expect(propertyValuesEqual(pair, { kind: "files", value: [file(1)] })).toBe(false);
    expect(propertyValuesEqual(checklist(item(1, "a", true)), checklist(item(1, "a", true)))).toBe(true);
    expect(propertyValuesEqual(checklist(item(1, "a", true)), checklist(item(1, "a", false)))).toBe(false);
    expect(propertyValuesEqual(checklist(item(1, "a")), checklist(item(1, "b")))).toBe(false);
    expect(propertyValuesEqual(checklist(item(1, "a")), checklist(item(2, "a")))).toBe(false);
    expect(propertyValuesEqual(checklist(item(1, "a"), item(2, "b")), checklist(item(2, "b"), item(1, "a")))).toBe(false);
  });

  it("copy shares nothing with the original and leaves a missing time out", () => {
    const list: PropertyValue = { kind: "multi_select", value: [option(1)] };
    const listCopy = copyPropertyValue(list);
    expect(listCopy).toEqual(list);
    (listCopy as { value: string[] }).value.push(option(2));
    expect(list).toEqual({ kind: "multi_select", value: [option(1)] });

    const when = day("2026-10-07");
    const whenCopy = copyPropertyValue(when);
    expect(whenCopy).toStrictEqual(when);
    expect("time" in (whenCopy as { value: object }).value).toBe(false);
    expect(copyPropertyValue(day("2026-10-07", "10:00"))).toStrictEqual(day("2026-10-07", "10:00"));
    expect(copyPropertyValue(text("x"))).toStrictEqual(text("x"));

    const files: PropertyValue = { kind: "files", value: [file(1)] };
    const filesCopy = copyPropertyValue(files) as { value: string[] };
    filesCopy.value.push(file(2));
    expect(files).toEqual({ kind: "files", value: [file(1)] });
    const todo = checklist(item(1, "a"));
    const todoCopy = copyPropertyValue(todo) as { value: ChecklistItem[] };
    expect(todoCopy).toStrictEqual(todo);
    (todoCopy.value[0] as ChecklistItem).done = true;
    expect(todo).toEqual(checklist(item(1, "a")));
  });
});
