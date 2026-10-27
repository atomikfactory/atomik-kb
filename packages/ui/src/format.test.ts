import { describe, expect, it } from "vitest";

import { formatRelativeTime } from "./format";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/** Local noon, so that "yesterday" does not depend on the time zone the tests run in. */
const NOON = new Date(2026, 9, 3, 12, 0, 0).getTime();

describe("formatRelativeTime", () => {
  it("says just now under a minute, and for a time slightly ahead of the clock", () => {
    expect(formatRelativeTime(NOON, NOON)).toBe("just now");
    expect(formatRelativeTime(NOON - 59_999, NOON)).toBe("just now");
    expect(formatRelativeTime(NOON + 5_000, NOON)).toBe("just now");
  });

  it("counts minutes under an hour and hours under a day", () => {
    expect(formatRelativeTime(NOON - MINUTE, NOON)).toBe("1 min ago");
    expect(formatRelativeTime(NOON - 2 * MINUTE - 30_000, NOON)).toBe("2 min ago");
    expect(formatRelativeTime(NOON - 59 * MINUTE, NOON)).toBe("59 min ago");
    expect(formatRelativeTime(NOON - HOUR, NOON)).toBe("1 h ago");
    expect(formatRelativeTime(NOON - 3 * HOUR - 20 * MINUTE, NOON)).toBe("3 h ago");
    expect(formatRelativeTime(NOON - 23 * HOUR, NOON)).toBe("23 h ago");
  });

  it("says yesterday for the calendar day before, then gives the date", () => {
    expect(formatRelativeTime(NOON - 24 * HOUR, NOON)).toBe("yesterday");
    // Yesterday just after midnight, 36 hours earlier: still yesterday.
    expect(formatRelativeTime(new Date(2026, 9, 2, 0, 30).getTime(), NOON)).toBe("yesterday");
    const older = new Date(2026, 9, 1, 23, 30);
    expect(formatRelativeTime(older.getTime(), NOON)).toBe(
      new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(older),
    );
  });

  it("accepts the bigint ts-rs gives i64 and refuses what is not a date", () => {
    expect(formatRelativeTime(BigInt(NOON - 2 * MINUTE), NOON)).toBe("2 min ago");
    expect(formatRelativeTime(Number.NaN, NOON)).toBeNull();
  });

  it("uses the current time by default", () => {
    expect(formatRelativeTime(Date.now())).toBe("just now");
  });
});
