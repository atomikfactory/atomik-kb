// Reminders in the app store (ADR-0024): the toast a `ReminderDue` event makes (the page title and
// the block's text), the "Open" button that goes to the page and the block, the 15 seconds, several
// toasts, nothing kept while locked, and the timer that ticks a core with no shell of its own.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Transport } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import type { AppState } from "./store";
import { MAX_REMINDER_TOASTS, REMINDER_TICK_MS, REMINDER_TOAST_MS } from "./store-reminders";

const PASSPHRASE = "correct horse battery";

/** The wall clock of the tests: 2026-10-07 14:00, as local fields. */
function at(hour: number, minute: number): number {
  return new Date(2026, 9, 7, hour, minute, 0).getTime();
}

const PARAGRAPH = "bbbbbbbb-0000-4000-8000-000000000001";
const WORK_PARAGRAPH = "bbbbbbbb-0000-4000-8000-000000000002";

function mention(n: number, time: string, reminder: number) {
  return {
    type: "dateMention",
    attrs: { id: `cccccccc-0000-4000-8000-${String(n).padStart(12, "0")}`, date: "2026-10-07", time, reminder },
  };
}

function page(title: string, paragraph: string, text: string, mentions: ReturnType<typeof mention>[]) {
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

interface Options extends MemoryTransportOptions {
  /** Hide the transport's scheduler, as the desktop transport does (its shell ticks the core). */
  noScheduler?: boolean;
}

/**
 * The memory core with two spaces (Personal: Plan, Work: Standup) and a started store over it.
 * The clock is `clock.now`, a wall clock of local fields.
 */
function setup({ noScheduler = false, ...options }: Options = {}, start = at(13, 0)) {
  const clock = { now: start };
  const memory = new MemoryTransport({
    now: () => clock.now,
    newId: (() => {
      let n = 0;
      return () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`;
    })(),
    workspaceName: "Home",
    spaces: [
      {
        name: "Personal",
        pages: [page("Plan", PARAGRAPH, "Call the dentist ", [mention(1, "14:30", 15), mention(2, "14:30", 0)])],
      },
      { name: "Work", pages: [page("Standup", WORK_PARAGRAPH, "Daily ", [mention(3, "15:00", 60)])] },
    ],
    ...options,
  });
  const transport: Transport = noScheduler
    ? {
        dispatch: (request) => memory.dispatch(request),
        subscribe: (onEvent) => memory.subscribe(onEvent),
        coreState: () => memory.coreState(),
        docs: memory.docs,
        lock: () => memory.lock(),
        unlock: (passphrase) => memory.unlock(passphrase),
      }
    : memory;
  const client = new CoreClient(transport);
  const store = createAppStore(client, { corePollMs: 5 });
  const stop = store.getState().start();
  return { memory, store, stop, clock, client };
}

type Ctx = ReturnType<typeof setup>;

const settled = (ctx: Ctx, check: (state: AppState) => boolean) =>
  vi.waitFor(
    () => {
      expect(check(ctx.store.getState())).toBe(true);
    },
    { timeout: 4000 },
  );

/** Moves fake time until the app is ready (the store starts with promises and a poll timer). */
async function readyUnderFakeTimers(ctx: Ctx): Promise<void> {
  for (let i = 0; i < 100 && ctx.store.getState().phase !== "ready"; i += 1) {
    await vi.advanceTimersByTimeAsync(10);
  }
  expect(ctx.store.getState().phase).toBe("ready");
}

const toasts = (ctx: Ctx) => ctx.store.getState().reminderToasts;

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("a reminder toast", () => {
  it("appears for a ReminderDue event, with the title of the page and the text of the block", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "ready");
    expect(toasts(ctx)).toEqual([]);

    ctx.clock.now = at(14, 16); // 14:15 (15 minutes before 14:30) and 14:00 (an hour before 15:00)
    expect(await ctx.memory.remindersTick()).toBe(2);
    expect(toasts(ctx).map((toast) => toast.reminder.pageTitle)).toEqual(["Standup", "Plan"]);
    expect(toasts(ctx)[1]?.reminder).toMatchObject({
      pageTitle: "Plan",
      text: "Call the dentist 2026-10-07 14:30 2026-10-07 14:30 ",
      reminderMinutes: 15,
    });
    ctx.stop();
  });

  it("is made at launch for what fell due while the app was closed", async () => {
    const ctx = setup({}, at(14, 40));
    await settled(ctx, (s) => s.phase === "ready" && s.reminderToasts.length === 3);
    expect(toasts(ctx).map((toast) => toast.reminder.reminderMinutes)).toEqual([60, 15, 0]);
    ctx.stop();
  });

  it("goes to the page and the block with Open, in another space too, and is gone", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "ready" && s.currentSpaceId !== null);
    const personal = ctx.store.getState().currentSpaceId;
    ctx.clock.now = at(14, 16);
    await ctx.memory.remindersTick();
    const standup = toasts(ctx).find((toast) => toast.reminder.pageTitle === "Standup");
    if (standup === undefined) {
      throw new Error("no toast for Standup");
    }
    expect(standup.reminder.spaceId).not.toBe(personal);

    ctx.store.getState().openReminderToast(standup.id);
    const state = ctx.store.getState();
    expect(state.selectedPageId).toBe(standup.reminder.pageId);
    expect(state.currentSpaceId).toBe(standup.reminder.spaceId);
    expect(state.route).toBe("home");
    expect(state.scrollTarget).toMatchObject({ pageId: standup.reminder.pageId, blockId: WORK_PARAGRAPH });
    expect(toasts(ctx).map((toast) => toast.id)).not.toContain(standup.id);
    expect(toasts(ctx)).toHaveLength(1);
    ctx.stop();
  });

  it("is dismissed by its button, and Open on a toast that is gone does nothing", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "ready");
    ctx.clock.now = at(14, 16);
    await ctx.memory.remindersTick();
    const [first, second] = toasts(ctx);
    if (first === undefined || second === undefined) {
      throw new Error("two toasts expected");
    }
    ctx.store.getState().dismissReminderToast(first.id);
    expect(toasts(ctx).map((toast) => toast.id)).toEqual([second.id]);
    const before = ctx.store.getState().selectedPageId;
    ctx.store.getState().openReminderToast(first.id);
    expect(ctx.store.getState().selectedPageId).toBe(before);
    ctx.stop();
  });

  it("goes by itself after 15 seconds, each toast on its own clock", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "ready");
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });

    ctx.clock.now = at(14, 5);
    await ctx.memory.remindersTick(); // the Work one, 14:00
    expect(toasts(ctx)).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(10_000);
    ctx.clock.now = at(14, 16);
    await ctx.memory.remindersTick(); // the Plan one, 14:15
    expect(toasts(ctx)).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(REMINDER_TOAST_MS - 10_000 - 1);
    expect(toasts(ctx)).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(toasts(ctx).map((toast) => toast.reminder.pageTitle)).toEqual(["Plan"]);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(toasts(ctx)).toEqual([]);
    ctx.stop();
  });

  it("stacks, shows at most five and drops the oldest, and a replayed reminder is one toast", async () => {
    const mentions = Array.from({ length: 8 }, (_, i) => mention(10 + i, "14:30", 8 - i));
    const ctx = setup({
      spaces: [{ name: "Personal", pages: [page("Many", PARAGRAPH, "x ", mentions)] }],
    });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.clock.now = at(14, 31);
    expect(await ctx.memory.remindersTick()).toBe(8);
    expect(toasts(ctx)).toHaveLength(MAX_REMINDER_TOASTS);
    // The oldest three went: what is left fired at 14:25, ..., 14:29 (reminders of 5 to 1 minutes).
    expect(toasts(ctx).map((toast) => toast.reminder.reminderMinutes)).toEqual([5, 4, 3, 2, 1]);
    ctx.stop();
  });

  it("is kept for no longer than the app is unlocked: locking clears the toasts", async () => {
    const ctx = setup({ security: { kitCreatedAt: at(13, 0), appLockPassphrase: PASSPHRASE } });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.clock.now = at(14, 16);
    await ctx.memory.remindersTick();
    expect(toasts(ctx)).toHaveLength(2);

    await ctx.store.getState().lock();
    expect(ctx.store.getState().phase).toBe("locked");
    expect(toasts(ctx)).toEqual([]);
    ctx.stop();
  });
});

describe("the scheduler of a core with no shell", () => {
  it("ticks every fifteen seconds while the store runs, and not after it stopped", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
    const startedAt = Date.now();
    const ctx = setup();
    const tick = vi.spyOn(ctx.memory, "remindersTick");
    await readyUnderFakeTimers(ctx);
    const afterStart = tick.mock.calls.length;
    expect(afterStart).toBe(1); // the first tick, as soon as the app is ready

    // The interval was set when the store started: its first tick is REMINDER_TICK_MS after that.
    ctx.clock.now = at(14, 16);
    await vi.advanceTimersByTimeAsync(REMINDER_TICK_MS - 1 - (Date.now() - startedAt));
    expect(tick.mock.calls.length).toBe(afterStart);
    expect(toasts(ctx)).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(tick.mock.calls.length).toBe(afterStart + 1);
    expect(toasts(ctx)).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(REMINDER_TICK_MS * 3);
    expect(tick.mock.calls.length).toBe(afterStart + 4);
    ctx.stop();
    await vi.advanceTimersByTimeAsync(REMINDER_TICK_MS * 3);
    expect(tick.mock.calls.length).toBe(afterStart + 4);
  });

  it("does not tick while the app is locked", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
    const ctx = setup({ security: { kitCreatedAt: at(13, 0), appLockPassphrase: PASSPHRASE } });
    const tick = vi.spyOn(ctx.memory, "remindersTick");
    await readyUnderFakeTimers(ctx);
    const afterStart = tick.mock.calls.length;
    await ctx.store.getState().lock();
    await vi.advanceTimersByTimeAsync(REMINDER_TICK_MS * 4);
    expect(tick.mock.calls.length).toBe(afterStart);
    ctx.stop();
  });

  it("is not started for a transport whose shell ticks its own core", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "Date"] });
    const ctx = setup({ noScheduler: true });
    const tick = vi.spyOn(ctx.memory, "remindersTick");
    await readyUnderFakeTimers(ctx);
    ctx.clock.now = at(14, 16);
    await vi.advanceTimersByTimeAsync(REMINDER_TICK_MS * 4);
    expect(tick).not.toHaveBeenCalled();
    expect(toasts(ctx)).toEqual([]);
    ctx.stop();
  });
});
