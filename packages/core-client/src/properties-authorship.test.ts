// The pure rules of the `place` kind and the computed kinds (`./properties`, ADR-0026 amendment of
// 2026-10-08): the same cases as the Rust tests of `kb-model` (`tests/authorship.rs`).

import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import type { PropertyValue } from "./generated/PropertyValue";
import {
  PROPERTY_KINDS,
  copyPropertyValue,
  isComputedPropertyKind,
  isEmptyPropertyValue,
  isValidPropertyValue,
  normalisePropertyValue,
  propertyValueProblem,
  propertyValueText,
  propertyValuesEqual,
} from "./properties";

const place = (name: string, lat?: number, lon?: number): PropertyValue => ({
  kind: "place",
  value: { name, ...(lat === undefined ? {} : { lat }), ...(lon === undefined ? {} : { lon }) },
});

describe("place and the computed kinds", () => {
  it("know which kinds are computed", () => {
    expect(PROPERTY_KINDS.filter((kind) => isComputedPropertyKind(kind))).toEqual([
      "rollup",
      "created_time",
      "last_edited_time",
      "created_by",
      "last_edited_by",
      "formula",
    ]);
  });

  it("a place follows the rules of the core, with its messages", () => {
    expect(propertyValueProblem(place("Home"))).toBeUndefined();
    expect(propertyValueProblem(place("Pole", -90, 180))).toBeUndefined();
    expect(propertyValueProblem(place("é".repeat(500)))).toBeUndefined();
    expect(propertyValueProblem(place(""))).toBe("must not be empty");
    expect(propertyValueProblem(place("x".repeat(501)))).toBe("must be at most 500 characters");
    expect(propertyValueProblem(place("a\tb"))).toBe("must not contain control characters");
    expect(propertyValueProblem({ kind: "place", value: { name: "x", lat: 1 } })).toBe(
      "the latitude and the longitude go together",
    );
    expect(propertyValueProblem(place("x", 91, 0))).toBe("the latitude must be between -90 and 90");
    expect(propertyValueProblem(place("x", 0, -181))).toBe("the longitude must be between -180 and 180");
    expect(propertyValueProblem({ kind: "place", value: "x" } as unknown as PropertyValue)).toBe("the value is malformed");
    expect(isEmptyPropertyValue(place("x"))).toBe(false);
    expect(isValidPropertyValue("place", place("x"))).toBe(true);
    expect(propertyValueText(place("Café"), () => undefined)).toBe("Café");
    expect(propertyValuesEqual(place("a", 1, 2), place("a", 1, 2))).toBe(true);
    expect(propertyValuesEqual(place("a", 1, 2), place("a"))).toBe(false);
    expect(copyPropertyValue(place("a", 1, 2))).toEqual(place("a", 1, 2));
    expect(normalisePropertyValue({ kind: "place", value: { name: "a" } })).toEqual(place("a"));
  });

  it("a computed value is never acceptable and has no text", () => {
    for (const value of [
      { kind: "created_time", value: 5 },
      { kind: "last_edited_time", value: 5 },
      { kind: "created_by", value: "00000000-0000-4000-8000-000000000001" },
      { kind: "last_edited_by", value: "00000000-0000-4000-8000-000000000001" },
    ] as const) {
      expect(propertyValueProblem(value)).toBe("is computed from the page and cannot be set");
      expect(isValidPropertyValue(value.kind, value)).toBe(false);
      expect(() => normalisePropertyValue(value)).toThrow(CoreError);
      expect(propertyValueText(value, () => undefined)).toBeUndefined();
    }
  });
});
