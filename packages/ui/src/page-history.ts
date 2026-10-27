// What the page history dialog shows of the list of versions (ADR-0057, `docs/architecture/page-history.md` section 7): the dated
// versions grouped by the day they ended ("Today", "Yesterday", then the date in the UI language), the time of a version or the
// range of its session, the undated ones after them with their position, and the version selected when the dialog opens. Pure:
// `PageHistoryDialog` draws what these functions answer.

import type { PageVersion } from "@atomik-kb/core-client";

import { activeLocale, t } from "./i18n";

/** A session longer than this (milliseconds) is shown as a range of times. */
export const RANGE_MS = 60_000;

/** The dated versions of one day, in the order the core listed them (newest first). */
export interface DayGroup {
  /** `YYYY-M-D` of the local day; distinct per group. */
  key: string;
  /** "Today", "Yesterday" or the date. */
  label: string;
  versions: readonly PageVersion[];
}

/** The versions as the dialog lists them. */
export interface VersionGroups {
  /** The dated versions by day, newest day first. */
  days: readonly DayGroup[];
  /** The undated ones, newest first. */
  undated: readonly PageVersion[];
  /** Every version in the order it is drawn (the days, then the undated): what Up and Down walk through. */
  ordered: readonly PageVersion[];
}

function dayKey(ms: number): string {
  const day = new Date(ms);
  return `${day.getFullYear()}-${day.getMonth()}-${day.getDate()}`;
}

const formatters = new Map<string, Intl.DateTimeFormat>();

/** An `Intl.DateTimeFormat` of the active locale, kept (the list formats one time per row). */
function formatter(options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const locale = activeLocale();
  const key = `${locale}|${JSON.stringify(options)}`;
  let found = formatters.get(key);
  if (found === undefined) {
    found = new Intl.DateTimeFormat(locale, options);
    formatters.set(key, found);
  }
  return found;
}

/** The heading of a day: "Today" and "Yesterday" for those, else the date in the UI language. */
export function dayLabel(ms: number, nowMs: number): string {
  const key = dayKey(ms);
  if (key === dayKey(nowMs)) {
    return t("history.list.today");
  }
  const now = new Date(nowMs);
  if (key === dayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).getTime())) {
    return t("history.list.yesterday");
  }
  return formatter({ dateStyle: "long" }).format(new Date(ms));
}

/** Whether a version has a time (the journal knew it); the others are the document's older history. */
export function isDated(version: PageVersion): boolean {
  return version.endedAt !== undefined;
}

/** Groups the versions the core listed (dated first, then undated, each newest first) for the dialog. */
export function groupVersions(versions: readonly PageVersion[], nowMs: number): VersionGroups {
  const days: { key: string; label: string; versions: PageVersion[] }[] = [];
  const undated: PageVersion[] = [];
  for (const version of versions) {
    if (version.endedAt === undefined) {
      undated.push(version);
      continue;
    }
    const key = dayKey(version.endedAt);
    const last = days[days.length - 1];
    if (last?.key === key) {
      last.versions.push(version);
    } else {
      days.push({ key, label: dayLabel(version.endedAt, nowMs), versions: [version] });
    }
  }
  return { days, undated, ordered: [...days.flatMap((day) => day.versions), ...undated] };
}

/** The time of a version ("14:20"), or the range of its session ("14:02 – 14:20") when it spans more than a minute. */
export function timeLabel(version: PageVersion): string {
  if (version.endedAt === undefined) {
    return "";
  }
  const time = formatter({ timeStyle: "short" });
  const end = time.format(new Date(version.endedAt));
  if (version.startedAt === undefined || version.endedAt - version.startedAt <= RANGE_MS) {
    return end;
  }
  const start =
    dayKey(version.startedAt) === dayKey(version.endedAt)
      ? time.format(new Date(version.startedAt))
      : formatter({ dateStyle: "short", timeStyle: "short" }).format(new Date(version.startedAt));
  return t("history.list.range", { start, end });
}

/** The date and time a version ended, in full ("Oct 3, 2026, 2:20 PM"); empty for an undated one. */
export function instantLabel(version: PageVersion): string {
  return version.endedAt === undefined ? "" : formatter({ dateStyle: "medium", timeStyle: "short" }).format(new Date(version.endedAt));
}

/**
 * The version selected when the dialog opens: the newest one that is not the page as it is now, else the first (the current
 * one, when it is the only one); `null` for no version at all.
 */
export function initialSelection(versions: readonly PageVersion[]): string | null {
  return (versions.find((version) => version.current !== true) ?? versions[0])?.id ?? null;
}

/** The version `delta` places from the one with id `from` in `ordered`, kept inside the list; the first when `from` is not there. */
export function stepSelection(ordered: readonly PageVersion[], from: string | null, delta: -1 | 1 | "first" | "last"): string | null {
  if (ordered.length === 0) {
    return null;
  }
  if (delta === "first") {
    return ordered[0]?.id ?? null;
  }
  if (delta === "last") {
    return ordered[ordered.length - 1]?.id ?? null;
  }
  const at = ordered.findIndex((version) => version.id === from);
  if (at < 0) {
    return ordered[0]?.id ?? null;
  }
  return ordered[Math.min(ordered.length - 1, Math.max(0, at + delta))]?.id ?? null;
}
