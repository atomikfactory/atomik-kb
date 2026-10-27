// The calendar arithmetic and the words of date mentions (`dates.ts`, `today.ts`): floating local
// dates (`YYYY-MM-DD`, no time zone), leap years, months of different lengths, the first day of
// the week of a locale, the month grid, and what a mention says ("Today", "Tomorrow", the medium
// date, the time).

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reminderFireAtLocal } from "@atomik-kb/core-client";

import { loadLanguage, resetLanguage, t } from "../i18n";
import {
  REMINDER_CHOICES,
  addDays,
  addMonths,
  daysBetween,
  daysInMonth,
  firstDayOfWeek,
  formatDateMention,
  fullDate,
  isIsoDate,
  isIsoTime,
  isoOf,
  mediumDate,
  monthGrid,
  monthTitle,
  partsOf,
  reminderFireTime,
  reminderLabel,
  todayIso,
  weekdayNames,
  weekdayOf,
} from "./dates";
import { useToday } from "./today";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  resetLanguage();
});

/** Runs `run` with the machine's time zone set to `zone` (the UI package has no Node.js types, hence the cast). */
function withZone<T>(zone: string, run: () => T): T {
  const env = (globalThis as unknown as { process: { env: Record<string, string | undefined> } }).process.env;
  const previous = env.TZ;
  env.TZ = zone;
  try {
    return run();
  } finally {
    if (previous === undefined) {
      delete env.TZ;
    } else {
      env.TZ = previous;
    }
  }
}

describe("the parts of a date", () => {
  it("reads a real day and writes it back, padded", () => {
    expect(partsOf("2026-10-07")).toEqual({ year: 2026, month: 10, day: 7 });
    expect(isoOf({ year: 2026, month: 10, day: 7 })).toBe("2026-10-07");
    expect(isoOf({ year: 5, month: 1, day: 2 })).toBe("0005-01-02");
    expect(partsOf("2026-02-30")).toBeNull();
    expect(partsOf("tomorrow")).toBeNull();
  });

  it("is the same check as the core's: real days only, 0001 to 9999, a leap day in a leap year", () => {
    expect(isIsoDate("2024-02-29")).toBe(true);
    expect(isIsoDate("2026-02-29")).toBe(false);
    expect(isIsoDate("1900-02-29")).toBe(false);
    expect(isIsoDate("2000-02-29")).toBe(true);
    expect(isIsoTime("23:59")).toBe(true);
    expect(isIsoTime("24:00")).toBe(false);
  });

  it("knows the length of a month, leap Februaries included", () => {
    expect(daysInMonth(2026, 1)).toBe(31);
    expect(daysInMonth(2026, 4)).toBe(30);
    expect(daysInMonth(2026, 2)).toBe(28);
    expect(daysInMonth(2024, 2)).toBe(29);
    expect(daysInMonth(1900, 2)).toBe(28);
    expect(daysInMonth(2000, 2)).toBe(29);
    expect(daysInMonth(2026, 12)).toBe(31);
  });
});

describe("calendar arithmetic", () => {
  it("adds days across months, years and leap days", () => {
    expect(addDays("2026-10-07", 1)).toBe("2026-10-08");
    expect(addDays("2026-10-31", 1)).toBe("2026-11-01");
    expect(addDays("2026-12-31", 1)).toBe("2027-01-01");
    expect(addDays("2026-01-01", -1)).toBe("2025-12-31");
    expect(addDays("2024-02-28", 1)).toBe("2024-02-29");
    expect(addDays("2026-02-28", 1)).toBe("2026-03-01");
    expect(addDays("2026-10-07", 0)).toBe("2026-10-07");
    expect(addDays("2026-10-07", 365)).toBe("2027-10-07");
    expect(addDays("not a date", 3)).toBe("not a date");
  });

  it("is not moved by daylight saving: a day is a day wherever the clock changes", () => {
    // The clocks change on these days in many zones; the arithmetic is on the calendar.
    expect(addDays("2026-03-28", 1)).toBe("2026-03-29");
    expect(addDays("2026-03-29", 1)).toBe("2026-03-30");
    expect(addDays("2026-10-24", 2)).toBe("2026-10-26");
    expect(addDays("2026-10-25", 1)).toBe("2026-10-26");
    expect(daysBetween("2026-03-28", "2026-03-30")).toBe(2);
    expect(daysBetween("2026-10-24", "2026-10-26")).toBe(2);
  });

  it("adds months and keeps the last day of a shorter month", () => {
    expect(addMonths("2026-10-07", 1)).toBe("2026-11-07");
    expect(addMonths("2026-10-07", -1)).toBe("2026-09-07");
    expect(addMonths("2026-01-31", 1)).toBe("2026-02-28");
    expect(addMonths("2024-01-31", 1)).toBe("2024-02-29");
    expect(addMonths("2026-03-31", -1)).toBe("2026-02-28");
    expect(addMonths("2026-12-15", 1)).toBe("2027-01-15");
    expect(addMonths("2026-01-15", -1)).toBe("2025-12-15");
    expect(addMonths("2026-10-07", 12)).toBe("2027-10-07");
    expect(addMonths("2026-10-07", -12)).toBe("2025-10-07");
  });

  it("stays inside the years a date may have", () => {
    expect(addMonths("0001-01-15", -1)).toBe("0001-01-15");
    expect(addMonths("9999-12-15", 1)).toBe("9999-12-15");
    expect(isIsoDate(addDays("9999-12-31", 1))).toBe(false); // the caller checks, as the picker does
  });

  it("tells the day of the week and the distance between days", () => {
    expect(weekdayOf("2026-10-07")).toBe(3); // a Wednesday
    expect(weekdayOf("2023-01-01")).toBe(0);
    expect(daysBetween("2026-10-07", "2026-10-08")).toBe(1);
    expect(daysBetween("2026-10-07", "2026-10-06")).toBe(-1);
    expect(daysBetween("2026-10-07", "2026-10-07")).toBe(0);
    expect(daysBetween("2025-10-07", "2026-10-07")).toBe(365);
  });

  it("takes today from the local clock, not from UTC", () => {
    // Late in the evening or early in the morning the UTC day is another one in most zones.
    expect(todayIso(new Date(2026, 9, 7, 23, 59, 59))).toBe("2026-10-07");
    expect(todayIso(new Date(2026, 9, 7, 0, 0, 0))).toBe("2026-10-07");
    expect(todayIso(new Date(2026, 0, 1, 0, 30))).toBe("2026-01-01");
  });
});

describe("the first day of the week", () => {
  it("comes from the locale", () => {
    expect(firstDayOfWeek("en-US")).toBe(0); // Sunday
    expect(firstDayOfWeek("de-DE")).toBe(1); // Monday
    expect(firstDayOfWeek("fr-FR")).toBe(1);
    expect(firstDayOfWeek("ar-EG")).toBe(6); // Saturday
  });

  it("is Monday for a locale it cannot read, and follows the app's locale by default", async () => {
    expect(firstDayOfWeek("not a locale!!")).toBe(1);
    // The locale is the app's (ADR-0040 item 13): the system's language when the app has it, the system's region
    // formats when it does not, so a German system keeps its Monday although the words are English.
    await loadLanguage("system", () => Promise.resolve({}), ["en-US"]);
    expect(firstDayOfWeek()).toBe(0);
    await loadLanguage("system", () => Promise.resolve({}), ["de-DE"]);
    expect(firstDayOfWeek()).toBe(1);
    await loadLanguage("system", () => Promise.resolve({}), ["fr-FR"]);
    expect(firstDayOfWeek()).toBe(1);
    resetLanguage();
  });

  it("names the days in the order a week is shown", () => {
    expect(weekdayNames(1, "en-US").map((name) => name.long)).toEqual([
      "Monday",
      "Tuesday",
      "Wednesday",
      "Thursday",
      "Friday",
      "Saturday",
      "Sunday",
    ]);
    expect(weekdayNames(0, "en-US").map((name) => name.short)).toEqual(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]);
    expect(weekdayNames(6, "en-US")[0]?.long).toBe("Saturday");
  });
});

describe("the month grid", () => {
  it("is six weeks of seven days that start on the first day of the week", () => {
    for (const first of [0, 1, 6]) {
      for (const month of ["2026-02-15", "2026-10-07", "2024-02-29", "2026-11-30", "2026-03-01"]) {
        const grid = monthGrid(month, first);
        expect(grid, `${month} from ${String(first)}`).toHaveLength(6);
        for (const week of grid) {
          expect(week).toHaveLength(7);
          expect(weekdayOf(week[0] ?? ""), month).toBe(first);
          expect(week.every((day) => isIsoDate(day))).toBe(true);
        }
        const days = grid.flat();
        // Consecutive days, and every day of the month is there once.
        expect(days.every((day, index) => index === 0 || day === addDays(days[index - 1] ?? "", 1))).toBe(true);
        const inMonth = days.filter((day) => day.slice(0, 7) === month.slice(0, 7));
        expect(inMonth).toHaveLength(daysInMonth(Number(month.slice(0, 4)), Number(month.slice(5, 7))));
      }
    }
  });

  it("shows October 2026 from Sunday and from Monday", () => {
    // 2026-10-01 is a Thursday.
    expect(monthGrid("2026-10-07", 0)[0]).toEqual([
      "2026-09-27",
      "2026-09-28",
      "2026-09-29",
      "2026-09-30",
      "2026-10-01",
      "2026-10-02",
      "2026-10-03",
    ]);
    expect(monthGrid("2026-10-07", 1)[0]).toEqual([
      "2026-09-28",
      "2026-09-29",
      "2026-09-30",
      "2026-10-01",
      "2026-10-02",
      "2026-10-03",
      "2026-10-04",
    ]);
    expect(monthGrid("2026-10-07", 1)[5]?.at(-1)).toBe("2026-11-08");
    expect(monthGrid("garbage", 1)).toEqual([]);
  });
});

describe("what a mention says", () => {
  const TODAY = "2026-10-07";
  const medium = (iso: string): string => mediumDate(iso, "en-US");

  it("says Today, Tomorrow and Yesterday for those days, and the medium date of the locale for the others", () => {
    expect(formatDateMention("2026-10-07", null, TODAY, "en-US")).toBe(t("date.today"));
    expect(formatDateMention("2026-10-08", null, TODAY, "en-US")).toBe(t("date.tomorrow"));
    expect(formatDateMention("2026-10-06", null, TODAY, "en-US")).toBe(t("date.yesterday"));
    expect(formatDateMention("2026-10-09", null, TODAY, "en-US")).toBe("Oct 9, 2026");
    expect(formatDateMention("2026-10-05", null, TODAY, "en-US")).toBe("Oct 5, 2026");
    expect(formatDateMention("2026-12-25", null, TODAY, "en-US")).toBe("Dec 25, 2026");
    expect(medium("2026-12-25")).toBe("Dec 25, 2026");
    // The same text in another locale, from the same date.
    expect(formatDateMention("2026-12-25", null, TODAY, "de-DE")).toBe("25.12.2026");
    expect(formatDateMention("2026-12-25", null, TODAY, "fr-FR")).toBe("25 déc. 2026");
  });

  it("puts the time after the day, in the locale's short time", () => {
    expect(formatDateMention("2026-10-07", "14:30", TODAY, "en-US")).toBe(
      t("date.relativeTime", { day: t("date.today"), time: "2:30 PM" }),
    );
    expect(formatDateMention("2026-10-08", "09:05", TODAY, "en-US")).toBe(
      t("date.relativeTime", { day: t("date.tomorrow"), time: "9:05 AM" }),
    );
    expect(formatDateMention("2026-12-25", "14:30", TODAY, "en-US")).toBe("Dec 25, 2026, 2:30 PM");
    expect(formatDateMention("2026-12-25", "00:00", TODAY, "en-US")).toBe("Dec 25, 2026, 12:00 AM");
    expect(formatDateMention("2026-12-25", "14:30", TODAY, "de-DE")).toBe("25.12.2026, 14:30");
  });

  it("leaves out a time that is not one, and shows a date that is not one as it is stored", () => {
    expect(formatDateMention("2026-12-25", "25:00", TODAY, "en-US")).toBe("Dec 25, 2026");
    expect(formatDateMention("2026-12-25", "", TODAY, "en-US")).toBe("Dec 25, 2026");
    expect(formatDateMention("2026-02-30", null, TODAY, "en-US")).toBe("2026-02-30");
    expect(formatDateMention("", null, TODAY, "en-US")).toBe("");
  });

  it("does not shift with the zone: a floating date is the same day and the same time everywhere", () => {
    // The text is built from the parts, not from an instant: the zone of the machine does not matter.
    const zones: [string, number][] = [
      ["Pacific/Kiritimati", -840], // UTC+14
      ["Pacific/Pago_Pago", 660], // UTC-11
      ["America/Los_Angeles", 420], // UTC-7 in July
      ["UTC", 0],
    ];
    for (const [zone, offset] of zones) {
      withZone(zone, () => {
        expect(new Date(2026, 6, 1).getTimezoneOffset(), zone).toBe(offset); // the zone is in force
        expect(formatDateMention("2026-01-01", "00:30", TODAY, "en-US"), zone).toBe("Jan 1, 2026, 12:30 AM");
        expect(formatDateMention("2026-12-31", "23:59", TODAY, "en-US"), zone).toBe("Dec 31, 2026, 11:59 PM");
        expect(formatDateMention("2026-12-25", null, TODAY, "en-US"), zone).toBe("Dec 25, 2026");
        expect(addDays("2026-10-07", 1), zone).toBe("2026-10-08");
        expect(weekdayOf("2026-10-07"), zone).toBe(3);
        expect(monthTitle("2026-10-07", "en-US"), zone).toBe("October 2026");
        // Only "today" follows the clock of the machine: this instant is another day in each zone.
        expect(todayIso(new Date(Date.UTC(2026, 9, 7, 23, 30)))).toBe(
          { "Pacific/Kiritimati": "2026-10-08", "Pacific/Pago_Pago": "2026-10-07", "America/Los_Angeles": "2026-10-07", UTC: "2026-10-07" }[zone],
        );
        expect(todayIso(new Date(Date.UTC(2026, 9, 7, 1, 0)))).toBe(
          { "Pacific/Kiritimati": "2026-10-07", "Pacific/Pago_Pago": "2026-10-06", "America/Los_Angeles": "2026-10-06", UTC: "2026-10-07" }[zone],
        );
      });
    }
  });

  it("names a month and a day in full for the picker", () => {
    expect(monthTitle("2026-10-07", "en-US")).toBe("October 2026");
    expect(fullDate("2026-10-07", "en-US")).toBe("Wednesday, October 7, 2026");
    expect(monthTitle("garbage")).toBe("");
    expect(fullDate("garbage")).toBe("garbage");
  });
});

describe("today", () => {
  it("is the day of the local clock, and changes at midnight without anything being asked", () => {
    vi.useFakeTimers({ now: new Date(2026, 9, 7, 23, 59, 30) });
    const { result } = renderHook(() => useToday());
    expect(result.current).toBe("2026-10-07");

    act(() => {
      vi.advanceTimersByTime(29_000);
    });
    expect(result.current).toBe("2026-10-07");
    act(() => {
      vi.advanceTimersByTime(2_000); // past midnight: the timer of the day change fires
    });
    expect(result.current).toBe("2026-10-08");

    // The next midnight is scheduled too.
    act(() => {
      vi.advanceTimersByTime(24 * 3_600_000);
    });
    expect(result.current).toBe("2026-10-09");
  });

  it("looks at the clock again when the window gets the focus back, or becomes visible", () => {
    vi.useFakeTimers({ now: new Date(2026, 9, 7, 12, 0, 0), toFake: ["Date", "setTimeout", "clearTimeout"] });
    const { result } = renderHook(() => useToday());
    expect(result.current).toBe("2026-10-07");

    // The machine slept through midnight: no timer fired, the focus event tells.
    vi.setSystemTime(new Date(2026, 9, 9, 8, 0, 0));
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(result.current).toBe("2026-10-09");

    vi.setSystemTime(new Date(2026, 9, 10, 8, 0, 0));
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(result.current).toBe("2026-10-10");
  });

  it("keeps one timer for every listener, and none while nobody listens", () => {
    vi.useFakeTimers({ now: new Date(2026, 9, 7, 12, 0, 0) });
    expect(vi.getTimerCount()).toBe(0);
    const first = renderHook(() => useToday());
    const second = renderHook(() => useToday());
    expect(vi.getTimerCount()).toBe(1);
    first.unmount();
    expect(vi.getTimerCount()).toBe(1);
    second.unmount();
    expect(vi.getTimerCount()).toBe(0);
    // With nobody listening it still answers, from the clock.
    vi.setSystemTime(new Date(2026, 9, 8, 12, 0, 0));
    const third = renderHook(() => useToday());
    expect(third.result.current).toBe("2026-10-08");
    third.unmount();
  });
});

describe("reminders", () => {
  it("offers None and the eight presets the design lists, in order", () => {
    expect(REMINDER_CHOICES).toEqual([null, 0, 5, 15, 30, 60, 120, 1440, 2880]);
  });

  it("names a preset for a mention with a time, and for a whole day by its 9:00", () => {
    expect(REMINDER_CHOICES.map((minutes) => reminderLabel(minutes, "14:30"))).toEqual([
      "None",
      "At time",
      "5 minutes before",
      "15 minutes before",
      "30 minutes before",
      "1 hour before",
      "2 hours before",
      "1 day before",
      "2 days before",
    ]);
    expect(REMINDER_CHOICES.map((minutes) => reminderLabel(minutes, null))).toEqual([
      "None",
      "At 9:00",
      "5 minutes before 9:00",
      "15 minutes before 9:00",
      "30 minutes before 9:00",
      "1 hour before 9:00",
      "2 hours before 9:00",
      "1 day before, at 9:00",
      "2 days before, at 9:00",
    ]);
    // A time that is not a time is no time.
    expect(reminderLabel(0, "25:00")).toBe("At 9:00");
  });

  it("names any other number of minutes by it", () => {
    expect(reminderLabel(90, "14:30")).toBe("90 minutes before");
    expect(reminderLabel(90, null)).toBe("90 minutes before 9:00");
    expect(reminderLabel(1, "08:00")).toBe("1 minutes before");
    expect(reminderLabel(7, "08:00")).toBe("7 minutes before");
  });

  it("fires at the moment minus the reminder on this machine's clock, the same wall clock time in every zone", () => {
    const zones = ["UTC", "Pacific/Kiritimati", "Pacific/Pago_Pago", "America/Los_Angeles", "Asia/Kolkata", "Europe/Paris"];
    for (const zone of zones) {
      withZone(zone, () => {
        const fire = reminderFireTime("2026-10-07", "14:30", 15);
        expect(fire, zone).not.toBeNull();
        // The fields on the local clock are the wall clock time: 14:15 on the 7th.
        expect(
          [fire?.getFullYear(), fire?.getMonth(), fire?.getDate(), fire?.getHours(), fire?.getMinutes(), fire?.getSeconds()],
          zone,
        ).toEqual([2026, 9, 7, 14, 15, 0]);

        const dayBefore = reminderFireTime("2026-10-07", null, 1440);
        expect([dayBefore?.getDate(), dayBefore?.getHours(), dayBefore?.getMinutes()], zone).toEqual([6, 9, 0]);

        const crossing = reminderFireTime("2026-03-01", "00:10", 30);
        expect(
          [crossing?.getMonth(), crossing?.getDate(), crossing?.getHours(), crossing?.getMinutes()],
          zone,
        ).toEqual([1, 28, 23, 40]);
      });
    }
  });

  it("is an instant that is later in the day in the east than the same wall clock time in the west", () => {
    const at = (zone: string) => withZone(zone, () => reminderFireTime("2026-10-07", "14:30", 0)?.getTime() ?? Number.NaN);
    // 14:30 in UTC+14 happens 25 hours before 14:30 in UTC-11.
    expect(at("Pacific/Pago_Pago") - at("Pacific/Kiritimati")).toBe(25 * 3_600_000);
    expect(at("UTC") - at("Asia/Kolkata")).toBe(5.5 * 3_600_000);
  });

  it("has none for a date that is not a real day", () => {
    expect(reminderFireTime("2026-02-30", null, 0)).toBeNull();
    expect(reminderFireTime("", "14:30", 0)).toBeNull();
  });

  it("is the number the core stores, read as local fields", () => {
    // 2026-10-07 14:30 minus 15 minutes, on the wall clock, in seconds.
    const seconds = reminderFireAtLocal("2026-10-07", "14:30", 15);
    expect(seconds).toBe(Date.UTC(2026, 9, 7, 14, 15) / 1000);
    const fire = reminderFireTime("2026-10-07", "14:30", 15);
    expect(fire === null ? null : Date.UTC(fire.getFullYear(), fire.getMonth(), fire.getDate(), fire.getHours(), fire.getMinutes()) / 1000).toBe(seconds);
  });
});
