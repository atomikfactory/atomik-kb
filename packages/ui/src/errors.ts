import type { CoreError } from "@atomik-kb/core-client";

/** A text value of the error's `details` object (`details.field`, `details.path`), if there is one. */
export function detailOf(error: CoreError, key: string): string | null {
  const { details } = error;
  if (typeof details !== "object" || details === null || Array.isArray(details)) {
    return null;
  }
  const value = details[key];
  return typeof value === "string" ? value : null;
}

/**
 * True when the core rejected one specific input: `InvalidInput` whose `details.field`
 * names it. The core's message then reads as a predicate ("must not be empty"), so a
 * form can show it as "<Field> <message>".
 */
export function isFieldError(error: CoreError, field: string): boolean {
  return error.code === "InvalidInput" && detailOf(error, "field") === field;
}
