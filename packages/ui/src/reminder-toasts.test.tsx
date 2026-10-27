// The in-app toasts of reminders (ADR-0024) in the whole app: what a toast says, "Open" taking the
// person to the page and the block, stacking, and the toast container being a live region that is
// no `status`.

import type { MemoryTransportOptions } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { findEditor, openPage, renderPages } from "./test/page";

const PLAN_BLOCK = "bbbbbbbb-0000-4000-8000-000000000001";
const NOTES_BLOCK = "bbbbbbbb-0000-4000-8000-000000000002";

/** A wall clock reading on 2026-10-07, as local fields (what the in-memory core reads as "now"). */
function at(hour: number, minute: number): number {
  return new Date(2026, 9, 7, hour, minute, 0).getTime();
}

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
        { type: "paragraph", attrs: { id: paragraph }, content: [{ type: "text", text }, ...mentions] },
        { type: "paragraph", attrs: { id: `${paragraph.slice(0, -2)}ff` }, content: [{ type: "text", text: "Another block" }] },
      ],
    },
  };
}

async function setup(options: MemoryTransportOptions = {}) {
  const clock = { now: at(13, 0) };
  const app = await renderPages(
    [
      {
        name: "Personal",
        pages: [
          page("Plan", PLAN_BLOCK, "Call the dentist ", [mention(1, "14:30", 15)]),
          page("Notes", NOTES_BLOCK, "Water the plants ", [mention(2, "14:40", 0)]),
        ],
      },
    ],
    { now: () => clock.now, ...options },
  );
  return { ...app, clock };
}

const log = () => screen.getByRole("log", { name: t("reminders.label") });

afterEach(() => {
  vi.useRealTimers();
});

describe("the reminder toasts", () => {
  it("are a live region that is there before anything is due, and no status", async () => {
    await setup();
    expect(log()).toBeInTheDocument();
    expect(within(log()).queryAllByRole("button")).toEqual([]);
    expect(screen.queryAllByRole("status")).toEqual([]);
  });

  it("shows 'Reminder: <page title> — <text>' with an Open and a Dismiss button when a reminder is due", async () => {
    const { memory, clock, user } = await setup();
    clock.now = at(14, 16);
    await act(async () => {
      await memory.remindersTick();
    });
    const toast = await within(log()).findByText(
      t("reminders.toast", { title: "Plan", text: "Call the dentist 2026-10-07 14:30" }),
    );
    expect(toast).toBeInTheDocument();
    expect(within(log()).getByRole("button", { name: t("reminders.open") })).toBeInTheDocument();

    await user.click(within(log()).getByRole("button", { name: t("reminders.dismiss") }));
    expect(within(log()).queryByText(/Reminder:/u)).toBeNull();
  });

  it("goes to the page and the block with Open, and the toast goes", async () => {
    const { memory, clock, user } = await setup();
    // Another page is open when the reminder comes.
    await openPage(user, "Notes");
    expect(screen.getByRole("textbox", { name: t("editor.label") })).toHaveTextContent("Water the plants");

    clock.now = at(14, 16);
    await act(async () => {
      await memory.remindersTick();
    });
    await user.click(await within(log()).findByRole("button", { name: t("reminders.open") }));

    const editor = await findEditor();
    await waitFor(() => {
      expect(editor.getText()).toContain("Call the dentist");
    });
    // The block of the mention is the one that is shown: it carries the highlight for a moment.
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".is-target")?.textContent).toContain("Call the dentist");
    });
    expect(within(log()).queryByRole("button", { name: t("reminders.open") })).toBeNull();
  });

  it("stack, newest last, each with its own Open", async () => {
    const { memory, clock } = await setup();
    clock.now = at(14, 45);
    await act(async () => {
      await memory.remindersTick();
    });
    await waitFor(() => {
      expect(within(log()).getAllByRole("button", { name: t("reminders.open") })).toHaveLength(2);
    });
    const text = log().textContent ?? "";
    expect(text.indexOf("Plan")).toBeGreaterThanOrEqual(0);
    expect(text.indexOf("Plan")).toBeLessThan(text.indexOf("Notes"));
  });

  it("name an untitled page 'Untitled' and leave out the dash of a block without text", async () => {
    const clock = { now: at(13, 0) };
    const app = await renderPages(
      [
        {
          name: "Personal",
          pages: [
            {
              title: "",
              content: {
                type: "doc",
                content: [{ type: "paragraph", attrs: { id: PLAN_BLOCK }, content: [mention(1, "14:30", 15)] }],
              },
            },
          ],
        },
      ],
      { now: () => clock.now },
    );
    clock.now = at(14, 20);
    await act(async () => {
      await app.memory.remindersTick();
    });
    // The block's text is the date alone: the dash is for a text that says more.
    await within(log()).findByText(t("reminders.toast", { title: t("page.untitled"), text: "2026-10-07 14:30" }));
  });
});
