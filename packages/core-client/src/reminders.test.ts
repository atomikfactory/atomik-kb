// Reminders on date mentions (ADR-0024): the arithmetic of fire times on the local wall clock
// (under several machine time zones: a mention is floating, only "now" depends on the zone), the
// derived date-mention rows of a page document, the window of a tick, and the in-memory core's
// `RemindersDue` and scheduler tick.

import { LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import { afterEach, describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { ReminderDue } from "./generated/ReminderDue";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import {
  MAX_REMINDER_MINUTES,
  configurePageDoc,
  dateMentionText,
  extractBlocks,
  extractDateMentions,
  newPageDoc,
  portableToLoro,
  reminderFireAtLocal,
  reminderMinutesOf,
} from "./page-doc";
import {
  CATCH_UP_SECONDS,
  FIRES_PER_TICK,
  LAST_CHECKED_SETTING,
  MAX_REMINDERS_DUE,
  localNowSeconds,
  planTick,
  windowStart,
} from "./reminders";

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86_400;

/** 2026-10-07 14:30 on the wall clock, in seconds since 1970-01-01T00:00:00. */
const OCT_7_1430 = Date.UTC(2026, 9, 7, 14, 30) / 1000;

/** The wall clock reading `seconds` (a local time) as an instant on this machine: what `now` returns. */
function atLocal(seconds: number): number {
  const wall = new Date(seconds * 1000);
  return new Date(
    wall.getUTCFullYear(),
    wall.getUTCMonth(),
    wall.getUTCDate(),
    wall.getUTCHours(),
    wall.getUTCMinutes(),
    wall.getUTCSeconds(),
  ).getTime();
}

/** Runs `run` with the machine's time zone set to `zone` (this package has no Node.js types, hence the cast). */
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

const ZONES: [string, number][] = [
  ["UTC", 0],
  ["Pacific/Kiritimati", -840], // UTC+14
  ["Pacific/Pago_Pago", 660], // UTC-11
  ["America/Los_Angeles", 420], // UTC-7 in October
  ["Asia/Kolkata", -330], // UTC+5:30
];

afterEach(() => {
  const env = (globalThis as unknown as { process: { env: Record<string, string | undefined> } }).process.env;
  delete env.TZ;
});

describe("the fire time of a reminder", () => {
  it("is the moment minus the reminder, on the wall clock", () => {
    expect(reminderFireAtLocal("2026-10-07", "14:30", 0)).toBe(OCT_7_1430);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 5)).toBe(OCT_7_1430 - 5 * MINUTE);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 15)).toBe(OCT_7_1430 - 15 * MINUTE);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 30)).toBe(OCT_7_1430 - 30 * MINUTE);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 60)).toBe(OCT_7_1430 - HOUR);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 120)).toBe(OCT_7_1430 - 2 * HOUR);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 1440)).toBe(OCT_7_1430 - DAY);
    expect(reminderFireAtLocal("2026-10-07", "14:30", 2880)).toBe(OCT_7_1430 - 2 * DAY);
  });

  it("counts a day without a time from 09:00", () => {
    const nine = OCT_7_1430 - (5 * 60 + 30) * MINUTE;
    expect(reminderFireAtLocal("2026-10-07", null, 0)).toBe(nine);
    expect(reminderFireAtLocal("2026-10-07", null, 60)).toBe(nine - HOUR);
    // A malformed time is as good as none, as everywhere else.
    expect(reminderFireAtLocal("2026-10-07", "25:00", 0)).toBe(nine);
    expect(reminderFireAtLocal("2026-10-07", "", 0)).toBe(nine);
  });

  it("crosses midnights, months, leap days and the edges of the calendar", () => {
    expect(reminderFireAtLocal("1970-01-01", "00:00", 0)).toBe(0);
    expect(reminderFireAtLocal("1970-01-01", "00:00", 1)).toBe(-60);
    expect(reminderFireAtLocal("2026-01-01", "00:00", 1)).toBe(reminderFireAtLocal("2025-12-31", "23:59", 0));
    expect(reminderFireAtLocal("2024-02-29", "23:59", 0)).toBe(reminderFireAtLocal("2024-03-01", "00:00", 1));
    expect(reminderFireAtLocal("2026-03-01", "00:00", 1440)).toBe(reminderFireAtLocal("2026-02-28", "00:00", 0));
    expect(reminderFireAtLocal("0001-01-01", "00:00", 0)).toBe(-62_135_596_800);
    expect(reminderFireAtLocal("9999-12-31", "23:59", 0)).toBe(253_402_300_740);
    // Years below 100 are years, not 19xx.
    expect(reminderFireAtLocal("0099-01-01", "00:00", 0)).toBe(-59_042_995_200);
  });

  it("is the same in every machine time zone: a mention is floating", () => {
    for (const [zone, offset] of ZONES) {
      withZone(zone, () => {
        expect(new Date(2026, 6, 1).getTimezoneOffset(), zone).toBe(offset); // the zone is in force
        expect(reminderFireAtLocal("2026-10-07", "14:30", 15), zone).toBe(OCT_7_1430 - 15 * MINUTE);
        expect(reminderFireAtLocal("2026-10-07", null, 0), zone).toBe(OCT_7_1430 - (5 * 60 + 30) * MINUTE);
        // Days when the clocks change in some zones: the arithmetic is on the calendar.
        expect(reminderFireAtLocal("2026-03-29", "02:30", 0), zone).toBe(Date.UTC(2026, 2, 29, 2, 30) / 1000);
        expect(reminderFireAtLocal("2026-10-25", "02:30", 0), zone).toBe(Date.UTC(2026, 9, 25, 2, 30) / 1000);
        expect(reminderFireAtLocal("2026-11-01", "01:30", 90), zone).toBe(Date.UTC(2026, 10, 1, 0, 0) / 1000);
      });
    }
  });

  it("has none for a date that is not a real day", () => {
    expect(reminderFireAtLocal("2026-02-30", null, 0)).toBeNull();
    expect(reminderFireAtLocal("", null, 0)).toBeNull();
    expect(reminderFireAtLocal("2026-10-7", "14:30", 0)).toBeNull();
  });

  it("agrees with the date-mention text rules: only a valid time counts", () => {
    expect(dateMentionText("2026-10-07", "14:30")).toBe("2026-10-07 14:30");
    expect(reminderFireAtLocal("2026-10-07", "14:30", 0)).toBe(OCT_7_1430);
  });
});

describe("the reminder attribute", () => {
  it("is a non-negative integer number of minutes", () => {
    expect(reminderMinutesOf(0)).toBe(0);
    expect(reminderMinutesOf(15)).toBe(15);
    expect(reminderMinutesOf(2880)).toBe(2880);
    expect(reminderMinutesOf(MAX_REMINDER_MINUTES)).toBe(MAX_REMINDER_MINUTES);
    expect(reminderMinutesOf(MAX_REMINDER_MINUTES + 1)).toBeNull();
    expect(reminderMinutesOf(-1)).toBeNull();
    expect(reminderMinutesOf(1.5)).toBeNull();
    expect(reminderMinutesOf("15")).toBeNull();
    expect(reminderMinutesOf(true)).toBeNull();
    expect(reminderMinutesOf(null)).toBeNull();
    expect(reminderMinutesOf(undefined)).toBeNull();
    expect(reminderMinutesOf(Number.NaN)).toBeNull();
    expect(reminderMinutesOf(Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("the device's wall clock", () => {
  it("is UTC plus the offset of the machine's zone at that moment", () => {
    const instant = new Date(Date.UTC(2026, 9, 7, 12, 0));
    const utcSeconds = instant.getTime() / 1000;
    const offsets: Record<string, number> = {
      UTC: 0,
      "Pacific/Kiritimati": 14 * HOUR,
      "Pacific/Pago_Pago": -11 * HOUR,
      "America/Los_Angeles": -7 * HOUR,
      "Asia/Kolkata": 5 * HOUR + 30 * MINUTE,
    };
    for (const [zone] of ZONES) {
      withZone(zone, () => {
        expect(localNowSeconds(instant), zone).toBe(utcSeconds + (offsets[zone] ?? Number.NaN));
      });
    }
  });

  it("reads the wall clock of a moment built from local fields in every zone", () => {
    for (const [zone] of ZONES) {
      withZone(zone, () => {
        expect(localNowSeconds(new Date(2026, 9, 7, 14, 30)), zone).toBe(OCT_7_1430);
        expect(localNowSeconds(new Date(2026, 0, 1, 0, 0, 0)), zone).toBe(Date.UTC(2026, 0, 1) / 1000);
      });
    }
  });

  it("goes back when the clocks fall and jumps ahead when they spring forward", () => {
    withZone("Europe/Paris", () => {
      // 2026-10-25: at 01:00 UTC the clocks fall from 03:00 CEST back to 02:00 CET.
      const before = localNowSeconds(new Date(Date.UTC(2026, 9, 25, 0, 59, 59)));
      const after = localNowSeconds(new Date(Date.UTC(2026, 9, 25, 1, 0, 0)));
      expect(before).toBe(Date.UTC(2026, 9, 25, 2, 59, 59) / 1000);
      expect(after).toBe(Date.UTC(2026, 9, 25, 2, 0, 0) / 1000);
      // 2026-03-29: at 01:00 UTC they spring forward from 02:00 CET to 03:00 CEST.
      const early = localNowSeconds(new Date(Date.UTC(2026, 2, 29, 0, 59, 59)));
      const late = localNowSeconds(new Date(Date.UTC(2026, 2, 29, 1, 0, 0)));
      expect(early).toBe(Date.UTC(2026, 2, 29, 1, 59, 59) / 1000);
      expect(late).toBe(Date.UTC(2026, 2, 29, 3, 0, 0) / 1000);
    });
  });
});

// ----- the date mentions of a document ----------------------------------------------------

const B1 = "bbbbbbbb-0000-4000-8000-000000000001";
const B2 = "bbbbbbbb-0000-4000-8000-000000000002";
const M1 = "cccccccc-0000-4000-8000-000000000001";
const M2 = "cccccccc-0000-4000-8000-000000000002";
const M3 = "cccccccc-0000-4000-8000-000000000003";
const M4 = "cccccccc-0000-4000-8000-000000000004";

function isList(value: unknown): value is LoroList {
  return value instanceof LoroList;
}

function isMap(value: unknown): value is LoroMap {
  return value instanceof LoroMap;
}

function docOf(json: JsonValue): LoroDoc {
  const doc = newPageDoc();
  portableToLoro(doc, json);
  return doc;
}

function mention(id: string, attrs: Record<string, JsonValue>): JsonValue {
  return { type: "dateMention", attrs: { id, ...attrs } };
}

describe("the date mentions of a page document", () => {
  it("lists them with their block, time, reminder and fire time, in document order", () => {
    const doc = docOf({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: B1 },
          content: [
            { type: "text", text: "a " },
            mention(M1, { date: "2026-10-07", time: "14:30", reminder: 15 }),
            { type: "text", text: " b " },
            mention(M2, { date: "2026-10-08" }),
          ],
        },
        {
          type: "bulletList",
          attrs: { id: "bbbbbbbb-0000-4000-8000-0000000000a0" },
          content: [
            {
              type: "listItem",
              attrs: { id: "bbbbbbbb-0000-4000-8000-0000000000a1" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: B2 },
                  content: [mention(M3, { date: "2026-10-08", reminder: 60 })],
                },
              ],
            },
          ],
        },
      ],
    });
    expect(extractDateMentions(doc)).toEqual([
      {
        blockId: B1,
        nodeId: M1,
        date: "2026-10-07",
        time: "14:30",
        reminderMinutes: 15,
        fireAtLocal: OCT_7_1430 - 15 * MINUTE,
      },
      { blockId: B1, nodeId: M2, date: "2026-10-08", time: null, reminderMinutes: null, fireAtLocal: null },
      {
        blockId: B2,
        nodeId: M3,
        date: "2026-10-08",
        time: null,
        reminderMinutes: 60,
        // 2026-10-08 09:00 minus an hour: 17 hours 30 minutes after 2026-10-07 14:30.
        fireAtLocal: OCT_7_1430 + (17 * 60 + 30) * MINUTE,
      },
    ]);
  });

  it("reads a damaged reminder as none and leaves out mentions without a date or an id", () => {
    const doc = docOf({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: B1 },
          content: [
            mention(M1, { date: "2026-10-07", reminder: -5 }),
            mention(M2, { date: "2026-10-07", reminder: 1.5 }),
            mention(M3, { date: "2026-10-07", time: "25:61", reminder: 5 }),
            mention(M4, { date: "2026-02-30", reminder: 5 }),
            { type: "dateMention", attrs: { date: "2026-10-07", reminder: 5 } },
          ],
        },
      ],
    });
    const found = extractDateMentions(doc);
    expect(found.map((m) => [m.nodeId, m.reminderMinutes, m.time])).toEqual([
      [M1, null, null],
      [M2, null, null],
      [M3, 5, null], // a malformed time counts as none: the moment is 09:00
    ]);
    expect(found[2]?.fireAtLocal).toBe(OCT_7_1430 - (5 * 60 + 30) * MINUTE - 5 * MINUTE);
  });

  it("does not skip a mention outside any block with an id", () => {
    const doc = docOf({ type: "doc", content: [mention(M1, { date: "2026-10-07", reminder: 5 })] });
    expect(extractDateMentions(doc)).toEqual([]);
  });

  it("does not change the text: the reminder is not text", () => {
    const with_ = docOf({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: B1 },
          content: [{ type: "text", text: "Call " }, mention(M1, { date: "2026-10-07", time: "14:30", reminder: 15 })],
        },
      ],
    });
    const without = docOf({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: B1 },
          content: [{ type: "text", text: "Call " }, mention(M1, { date: "2026-10-07", time: "14:30" })],
        },
      ],
    });
    expect(extractBlocks(with_)).toEqual(extractBlocks(without));
    expect(extractBlocks(with_).find((b) => b.id === B1)?.inlines).toEqual([
      { type: "text", text: "Call 2026-10-07 14:30" },
    ]);
  });
});

// ----- the window of a tick ----------------------------------------------------------------

function due(fireAt: number, node = "n"): ReminderDue {
  return {
    pageId: "00000000-0000-4000-8000-000000000001",
    spaceId: "00000000-0000-4000-8000-000000000002",
    blockId: B1,
    nodeId: node,
    pageTitle: "Page",
    text: "text",
    date: "2026-10-07",
    reminderMinutes: 0,
    fireAt,
  };
}

/** A source over `list`: the window open below and closed above, oldest first, limited. */
function sourceOf(list: ReminderDue[]) {
  return (after: number, until: number, limit: number): ReminderDue[] =>
    list
      .filter((item) => item.fireAt > after && item.fireAt <= until)
      .sort((a, b) => a.fireAt - b.fireAt)
      .slice(0, limit);
}

describe("the window of a tick", () => {
  it("starts at the mark and reaches back seven days at most", () => {
    expect(CATCH_UP_SECONDS).toBe(7 * DAY);
    expect(windowStart(null, 10 * DAY)).toBe(3 * DAY);
    expect(windowStart(5 * DAY, 10 * DAY)).toBe(5 * DAY);
    expect(windowStart(1 * DAY, 10 * DAY)).toBe(3 * DAY);
    expect(windowStart(20 * DAY, 10 * DAY)).toBe(20 * DAY);
  });

  it("fires what is after the mark and not after now, and moves the mark to now", () => {
    const list = [due(100, "a"), due(200, "b"), due(300, "c")];
    expect(planTick(sourceOf(list), 100, 250)).toEqual({ fire: [due(200, "b")], mark: 250 });
    expect(planTick(sourceOf(list), 250, 250)).toEqual({ fire: [], mark: 250 });
    expect(planTick(sourceOf(list), null, 300).fire.map((d) => d.nodeId)).toEqual(["a", "b", "c"]);
  });

  it("never moves the mark back, so a clock that falls back fires nothing twice", () => {
    expect(planTick(sourceOf([due(100, "a")]), 500, 400)).toEqual({ fire: [], mark: 500 });
  });

  it("fires a long backlog in batches, the moment of the last of a batch included", () => {
    const list = Array.from({ length: 40 }, (_, i) => due(1000 + i, `n${String(i).padStart(2, "0")}`));
    list.push(due(1000 + FIRES_PER_TICK - 1, "tie1"), due(1000 + FIRES_PER_TICK - 1, "tie2"));
    const first = planTick(sourceOf(list), 0, 5000);
    expect(first.fire).toHaveLength(FIRES_PER_TICK + 2);
    expect(first.mark).toBe(1000 + FIRES_PER_TICK - 1);
    const second = planTick(sourceOf(list), first.mark, 5000);
    expect(second.fire).toHaveLength(40 - FIRES_PER_TICK);
    expect(second.mark).toBe(5000);
    expect(MAX_REMINDERS_DUE).toBeGreaterThanOrEqual(FIRES_PER_TICK);
  });
});

// ----- the in-memory core --------------------------------------------------------------------

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

/** A page of one paragraph: `text`, then the mentions each followed by a space. */
function page(title: string, paragraph: string, text: string, mentions: JsonValue[]): { title: string; content: JsonValue } {
  return {
    title,
    content: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: paragraph },
          content: [{ type: "text", text }, ...mentions.flatMap((item) => [item, { type: "text", text: " " }])],
        },
      ],
    },
  };
}

/** Seeds: device 1, spaces and pages in creation order (Personal 2: Plan 3, Other 4; Work 5: Standup 6). */
function setup(options: MemoryTransportOptions = {}, clock = { now: atLocal(OCT_7_1430 - 2 * HOUR) }) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    spaces: [
      {
        name: "Personal",
        pages: [
          page("Plan", B1, "Call the dentist ", [
            mention(M1, { date: "2026-10-07", time: "14:30", reminder: 15 }),
            mention(M2, { date: "2026-10-07", time: "14:30", reminder: 0 }),
            mention(M3, { date: "2026-10-08" }),
          ]),
          { title: "Other" },
        ],
      },
      {
        name: "Work",
        pages: [page("Standup", B2, "Daily ", [mention(M4, { date: "2026-10-07", time: "15:00", reminder: 60 })])],
      },
    ],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events, clock };
}

const dueEvents = (events: Event[]): ReminderDue[] =>
  events.flatMap((event) => (event.type === "ReminderDue" ? [event.data] : []));

describe("RemindersDue of the in-memory core", () => {
  it("lists the reminders that fell due before `until`, with the page and the block text", async () => {
    const { client } = setup();
    // The first fire time is 14:00 (an hour before the 15:00 of the Work space).
    expect(await client.remindersDue(OCT_7_1430 - 31 * MINUTE)).toEqual([]);
    expect(await client.remindersDue(OCT_7_1430 - 30 * MINUTE)).toEqual([
      {
        pageId: id(6),
        spaceId: id(5),
        blockId: B2,
        nodeId: M4,
        pageTitle: "Standup",
        text: "Daily 2026-10-07 15:00 ",
        date: "2026-10-07",
        time: "15:00",
        reminderMinutes: 60,
        fireAt: OCT_7_1430 - 30 * MINUTE,
      },
    ]);
    // Oldest first, across spaces; a mention without a reminder is never due.
    const all = await client.remindersDue(OCT_7_1430);
    expect(all.map((r) => [r.nodeId, r.pageTitle, r.reminderMinutes])).toEqual([
      [M4, "Standup", 60],
      [M1, "Plan", 15],
      [M2, "Plan", 0],
    ]);
    expect(all[1]?.text).toBe("Call the dentist 2026-10-07 14:30 2026-10-07 14:30 2026-10-08 ");
    expect(all[1]?.spaceId).toBe(id(2));
  });

  it("does not move anything: asking again gives the same answer and no event", async () => {
    const { client, events } = setup();
    const again = await client.remindersDue(OCT_7_1430);
    expect(await client.remindersDue(OCT_7_1430)).toEqual(again);
    expect(events).toEqual([]);
    expect(await client.settingsGet(LAST_CHECKED_SETTING)).toBeNull();
  });

  it("starts after the mark, which counts as seven days back at most", async () => {
    const { client } = setup({ settings: { [LAST_CHECKED_SETTING]: OCT_7_1430 - 30 * MINUTE } });
    expect((await client.remindersDue(OCT_7_1430)).map((r) => r.nodeId)).toEqual([M1, M2]);

    const old = setup({ settings: { [LAST_CHECKED_SETTING]: 0 } });
    // Seven days after 14:30: the window reaches back to 14:30 (open below), so the reminders of
    // 14:00 and 14:15 are out of it, the one at 14:30 is in a second sooner and out at the moment.
    expect((await old.client.remindersDue(OCT_7_1430 + 7 * DAY - 1)).map((r) => r.nodeId)).toEqual([M2]);
    expect((await old.client.remindersDue(OCT_7_1430 + 7 * DAY)).map((r) => r.nodeId)).toEqual([]);
  });

  it("leaves out trashed pages and brings them back with a restore", async () => {
    const { client, transport } = setup();
    expect((await client.remindersDue(OCT_7_1430)).map((r) => r.nodeId)).toEqual([M4, M1, M2]);
    await client.pageTrash(id(6));
    expect((await client.remindersDue(OCT_7_1430)).map((r) => r.nodeId)).toEqual([M1, M2]);
    await client.pageRestore(id(6));
    expect((await client.remindersDue(OCT_7_1430)).map((r) => r.nodeId)).toEqual([M4, M1, M2]);
    expect(transport).toBeDefined();
  });

  it("follows the document on every save: add, change and remove a mention", async () => {
    const { client, transport } = setup();
    const other = id(4);
    // Add a mention to the empty page "Other" through an editor replica.
    const doc = new LoroDoc();
    configurePageDoc(doc);
    doc.import(await transport.docs.open(other));
    let acked = doc.oplogVersion();
    const blocks = doc.getMap("doc").get("children");
    if (!isList(blocks)) {
      throw new Error("no blocks");
    }
    const block = blocks.get(0);
    if (!isMap(block)) {
      throw new Error("no first block");
    }
    const children = block.get("children");
    if (!isList(children)) {
      throw new Error("no children");
    }
    children.insertContainer(0, new LoroText()).insert(0, "Pay ");
    const node = children.insertContainer(1, new LoroMap());
    node.set("nodeName", "dateMention");
    const attributes = node.setContainer("attributes", new LoroMap());
    attributes.set("id", M3.replace("0003", "0009"));
    attributes.set("date", "2026-10-07");
    attributes.set("time", "14:30");
    attributes.set("reminder", 5);
    node.setContainer("children", new LoroList());
    doc.commit();
    await transport.docs.import(other, doc.export({ mode: "update", from: acked }));
    acked = doc.oplogVersion();
    let found = (await client.remindersDue(OCT_7_1430)).filter((r) => r.pageId === other);
    expect(found.map((r) => [r.reminderMinutes, r.fireAt])).toEqual([[5, OCT_7_1430 - 5 * MINUTE]]);
    expect(found[0]?.text).toBe("Pay 2026-10-07 14:30");

    // Change the reminder.
    attributes.set("reminder", 60);
    doc.commit();
    await transport.docs.import(other, doc.export({ mode: "update", from: acked }));
    acked = doc.oplogVersion();
    found = (await client.remindersDue(OCT_7_1430)).filter((r) => r.pageId === other);
    expect(found.map((r) => [r.reminderMinutes, r.fireAt])).toEqual([[60, OCT_7_1430 - HOUR]]);

    // Remove the reminder (the attribute), then the mention.
    attributes.delete("reminder");
    doc.commit();
    await transport.docs.import(other, doc.export({ mode: "update", from: acked }));
    acked = doc.oplogVersion();
    expect((await client.remindersDue(OCT_7_1430)).filter((r) => r.pageId === other)).toEqual([]);
    children.delete(1, 1);
    doc.commit();
    await transport.docs.import(other, doc.export({ mode: "update", from: acked }));
    expect((await client.remindersDue(OCT_7_1430)).filter((r) => r.pageId === other)).toEqual([]);
  });

  it("is a request the contract names", async () => {
    const { transport } = setup();
    const response = await transport.dispatch({ type: "RemindersDue", data: { until: 0 } });
    expect(response).toEqual({ type: "Reminders", data: [] });
  });
});

describe("the scheduler tick of the in-memory core", () => {
  it("fires each reminder once, oldest first, and advances the mark", async () => {
    const { client, events, clock } = setup();
    expect(client.hasRemindersTick()).toBe(true);

    // Before anything is due: nothing fires, and the mark is now.
    expect(await client.remindersTick()).toBe(0);
    expect(dueEvents(events)).toEqual([]);
    expect(await client.settingsGet(LAST_CHECKED_SETTING)).toBe(OCT_7_1430 - 2 * HOUR);

    // 14:16: the one of the Work space (an hour before 15:00 is 14:00) and the 15 minutes before
    // one (14:15) are due, the one at the time (14:30) is not.
    clock.now = atLocal(OCT_7_1430 - 14 * MINUTE);
    expect(await client.remindersTick()).toBe(2);
    expect(dueEvents(events).map((r) => r.nodeId)).toEqual([M4, M1]);
    expect(events.at(-1)?.type).toBe("ReminderDue");

    // Ticking again, now or a minute later, fires nothing more.
    expect(await client.remindersTick()).toBe(0);
    clock.now += MINUTE * 1000;
    expect(await client.remindersTick()).toBe(0);

    // 14:31: the one at the time.
    clock.now = atLocal(OCT_7_1430 + MINUTE);
    expect(await client.remindersTick()).toBe(1);
    expect(dueEvents(events).map((r) => r.nodeId)).toEqual([M4, M1, M2]);
    expect(await client.settingsGet(LAST_CHECKED_SETTING)).toBe(OCT_7_1430 + MINUTE);
  });

  it("emits events that carry what a toast shows", async () => {
    const { client, events, clock } = setup();
    await client.remindersTick();
    clock.now = atLocal(OCT_7_1430);
    await client.remindersTick();
    expect(dueEvents(events)[1]).toEqual({
      pageId: id(3),
      spaceId: id(2),
      blockId: B1,
      nodeId: M1,
      pageTitle: "Plan",
      text: "Call the dentist 2026-10-07 14:30 2026-10-07 14:30 2026-10-08 ",
      date: "2026-10-07",
      time: "14:30",
      reminderMinutes: 15,
      fireAt: OCT_7_1430 - 15 * MINUTE,
    });
  });

  it("looks back seven days at the first tick, and no further", async () => {
    const { client, events } = setup(
      {},
      {
        now: atLocal(Date.UTC(2026, 9, 7 + 7, 14, 30) / 1000),
      },
    );
    expect(await client.remindersTick()).toBe(0);
    expect(dueEvents(events)).toEqual([]);
    const recent = setup({}, { now: atLocal(Date.UTC(2026, 9, 7 + 6, 14, 30) / 1000) });
    expect(await recent.client.remindersTick()).toBe(3);
  });

  it("does nothing while the core is not ready, and catches up after the unlock", async () => {
    const { client, transport, events, clock } = setup();
    await client.remindersTick();
    transport.setCoreState("locked");
    clock.now = atLocal(OCT_7_1430 + HOUR);
    expect(await client.remindersTick()).toBe(0);
    expect(dueEvents(events)).toEqual([]);
    transport.setCoreState("ready");
    expect(await client.remindersTick()).toBe(3);
  });

  it("does not fire the reminders of trashed pages", async () => {
    const { client, events, clock } = setup();
    await client.remindersTick();
    await client.pageTrash(id(6));
    clock.now = atLocal(OCT_7_1430 + HOUR);
    expect(await client.remindersTick()).toBe(2);
    expect(dueEvents(events).map((r) => r.nodeId)).toEqual([M1, M2]);
  });

  it("fires a long backlog one batch per tick, every reminder once", async () => {
    const mentions = Array.from({ length: 40 }, (_, i) =>
      mention(`cccccccc-0000-4000-8000-${String(100 + i).padStart(12, "0")}`, {
        date: "2026-10-07",
        time: "14:30",
        reminder: 40 - i,
      }),
    );
    const { client, events } = setup(
      {
        spaces: [{ name: "Personal", pages: [page("Many", B1, "x ", mentions)] }],
      },
      { now: atLocal(OCT_7_1430 + HOUR) },
    );
    expect(await client.remindersTick()).toBe(FIRES_PER_TICK);
    expect(await client.remindersTick()).toBe(40 - FIRES_PER_TICK);
    expect(await client.remindersTick()).toBe(0);
    const nodes = dueEvents(events).map((r) => r.nodeId);
    expect(new Set(nodes).size).toBe(40);
    const times = dueEvents(events).map((r) => r.fireAt);
    expect(times).toEqual([...times].sort((a, b) => a - b));
  });

  it("is not offered by a transport without a scheduler of its own", async () => {
    const bare = new CoreClient({
      dispatch: () => Promise.reject(new Error("unused")),
      subscribe: () => () => undefined,
      docs: {
        open: () => Promise.reject(new Error("unused")),
        import: () => Promise.reject(new Error("unused")),
        export: () => Promise.reject(new Error("unused")),
        close: () => Promise.reject(new Error("unused")),
      },
    });
    expect(bare.hasRemindersTick()).toBe(false);
    const error = await bare.remindersTick().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("Internal");
  });
});
