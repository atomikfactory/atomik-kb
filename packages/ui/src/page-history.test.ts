// The list of versions as the page history dialog groups and labels it (ADR-0057): days, the time or range of a session, the undated
// ones, the version selected at open and the one the keys move to.

import type { PageVersion } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { activeLocale, t } from "./i18n";
import { dayLabel, groupVersions, initialSelection, instantLabel, stepSelection, timeLabel } from "./page-history";

/** A local date and time (the grouping is by the local day, whatever the zone the tests run in). */
const at = (day: number, hour: number, minute = 0, second = 0): number => new Date(2026, 9, day, hour, minute, second).getTime();

const time = (ms: number): string => new Intl.DateTimeFormat(activeLocale(), { timeStyle: "short" }).format(new Date(ms));

const NOW = at(3, 15);

function dated(id: string, endedAt: number, extra: Partial<PageVersion> = {}): PageVersion {
  return { id, kind: "edit", startedAt: endedAt, endedAt, ...extra };
}

describe("the day headings", () => {
  it("say Today and Yesterday, then the date in the UI language", () => {
    expect(dayLabel(at(3, 0, 5), NOW)).toBe(t("history.list.today"));
    expect(dayLabel(at(3, 23, 59), NOW)).toBe(t("history.list.today"));
    expect(dayLabel(at(2, 23, 59), NOW)).toBe(t("history.list.yesterday"));
    expect(dayLabel(at(2, 0, 0), NOW)).toBe(t("history.list.yesterday"));
    expect(dayLabel(at(1, 12), NOW)).toBe(new Intl.DateTimeFormat(activeLocale(), { dateStyle: "long" }).format(new Date(at(1, 12))));
  });

  it("find Yesterday across the end of a month", () => {
    const first = new Date(2026, 10, 1, 8).getTime();
    expect(dayLabel(new Date(2026, 9, 31, 22).getTime(), first)).toBe(t("history.list.yesterday"));
  });
});

describe("grouping the versions", () => {
  it("groups the dated versions by the day they ended, in the core's order, and the undated ones after them", () => {
    const versions: PageVersion[] = [
      dated("a", at(3, 9, 30), { current: true }),
      dated("b", at(3, 9)),
      dated("c", at(2, 14)),
      dated("d", at(1, 10)),
      { id: "e", kind: "edit" },
      { id: "f", kind: "edit" },
    ];
    const groups = groupVersions(versions, NOW);
    expect(groups.days.map((day) => [day.label, day.versions.map((version) => version.id)])).toEqual([
      [t("history.list.today"), ["a", "b"]],
      [t("history.list.yesterday"), ["c"]],
      [dayLabel(at(1, 10), NOW), ["d"]],
    ]);
    expect(groups.undated.map((version) => version.id)).toEqual(["e", "f"]);
    expect(groups.ordered.map((version) => version.id)).toEqual(["a", "b", "c", "d", "e", "f"]);
  });

  it("makes no group for nothing", () => {
    expect(groupVersions([], NOW)).toEqual({ days: [], undated: [], ordered: [] });
  });

  it("keeps a day together when an older page of versions is appended", () => {
    const first = [dated("a", at(3, 9)), dated("b", at(3, 8))];
    const more = [...first, dated("c", at(3, 7)), dated("d", at(2, 7))];
    expect(groupVersions(first, NOW).days).toHaveLength(1);
    expect(groupVersions(more, NOW).days.map((day) => day.versions.length)).toEqual([3, 1]);
  });
});

describe("the time of a version", () => {
  it("is the time it ended, or the range of a session that lasted more than a minute", () => {
    expect(timeLabel(dated("a", at(3, 9, 30)))).toBe(time(at(3, 9, 30)));
    expect(timeLabel({ id: "a", kind: "edit", startedAt: at(3, 9, 30), endedAt: at(3, 9, 30, 59) })).toBe(time(at(3, 9, 30, 59)));
    expect(timeLabel({ id: "a", kind: "edit", startedAt: at(3, 9, 2), endedAt: at(3, 9, 20) })).toBe(
      t("history.list.range", { start: time(at(3, 9, 2)), end: time(at(3, 9, 20)) }),
    );
  });

  it("gives the date of the start of a session that crossed midnight", () => {
    const label = timeLabel({ id: "a", kind: "edit", startedAt: at(2, 23, 50), endedAt: at(3, 0, 10) });
    expect(label).toContain(time(at(3, 0, 10)));
    expect(label).toContain(String(new Date(at(2, 23, 50)).getDate()));
    expect(label).not.toBe(time(at(3, 0, 10)));
  });

  it("is empty for an undated version", () => {
    expect(timeLabel({ id: "a", kind: "edit" })).toBe("");
    expect(instantLabel({ id: "a", kind: "edit" })).toBe("");
    expect(instantLabel(dated("a", at(3, 9)))).toContain(time(at(3, 9)));
  });
});

describe("the selection", () => {
  const versions: PageVersion[] = [dated("a", at(3, 9), { current: true }), dated("b", at(2, 9)), { id: "c", kind: "edit" }];

  it("opens on the newest version that is not the page as it is now, else on the current one, else on nothing", () => {
    expect(initialSelection(versions)).toBe("b");
    expect(initialSelection([dated("a", at(3, 9), { current: true })])).toBe("a");
    expect(initialSelection([])).toBeNull();
  });

  it("moves one place, to the ends and not past them", () => {
    expect(stepSelection(versions, "a", 1)).toBe("b");
    expect(stepSelection(versions, "c", 1)).toBe("c");
    expect(stepSelection(versions, "a", -1)).toBe("a");
    expect(stepSelection(versions, "c", -1)).toBe("b");
    expect(stepSelection(versions, "b", "first")).toBe("a");
    expect(stepSelection(versions, "b", "last")).toBe("c");
    expect(stepSelection(versions, null, 1)).toBe("a");
    expect(stepSelection([], "a", 1)).toBeNull();
  });
});
