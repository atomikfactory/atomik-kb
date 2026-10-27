// The limits of formulas, mirrored from `kb_types::properties` (ADR-0055).

/** Longest formula expression, in characters (code points). */
export const MAX_FORMULA_CHARS = 4_000;
/** Deepest nesting of a formula (parentheses, calls, lists). */
export const MAX_FORMULA_DEPTH = 64;
/** Most items of a list inside a formula or in its result. */
export const MAX_FORMULA_LIST_ITEMS = 10_000;
/** Most evaluation steps of one formula on one page, nested formulas included. */
export const MAX_FORMULA_STEPS = 100_000;
/** The id a stored expression uses for the page title: `prop("title")`. */
export const FORMULA_TITLE_ID = "title";
/** Longest text a formula works with, in characters (`MAX_TEXT_CHARS`): a longer one is empty. */
export const MAX_FORMULA_TEXT_CHARS = 10_000;
