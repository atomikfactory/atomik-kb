// The value controls of the filter editor for the operators of ADR-0058 (`docs/architecture/database-power-ups.md` section
// 3.4): a label for every operator, the select of a day condition ("Exact date", "Today", "Yesterday", "Tomorrow") with the
// date field for an exact date, the select of "is within" with the number of days for the past and the next days, and what
// each choice writes. The panel is mounted over a filter group the test holds in state, as the toolbar does.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PropertyDef } from "@atomik-kb/core-client";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import type { FilterGroup } from "./page-list-filter-tree";
import type { PageListFilter } from "./page-list-filters";
import { PageListFilters } from "./PageListFilters";

const def = (id: string, name: string, kind: PropertyDef["kind"], options: PropertyDef["options"] = []): PropertyDef => ({
  id,
  name,
  kind,
  options,
  createdAt: 0,
});
const DEFS: PropertyDef[] = [
  def("p-name", "Name", "text"),
  def("p-priority", "Priority", "number"),
  def("p-due", "Due", "date"),
  def("p-made", "Created", "created_time"),
  def("p-stage", "Stage", "select", [
    { id: "o-draft", name: "Draft", color: "gray" },
    { id: "o-review", name: "Review", color: "orange" },
  ]),
  def("p-tasks", "Tasks", "relation"),
  def("p-owner", "Owner", "person"),
];

function Panel({ initial, onChange }: { initial: FilterGroup; onChange: (group: FilterGroup) => void }) {
  const [group, setGroup] = useState(initial);
  const [anchor] = useState(() => document.body.appendChild(document.createElement("button")));
  return (
    <PageListFilters
      anchor={{ x: 0, y: 0 }}
      anchorElement={anchor}
      defs={DEFS}
      group={group}
      onChange={(next) => {
        setGroup(next);
        onChange(next);
      }}
      onClose={() => undefined}
    />
  );
}

/** Opens the editor over one filter; `last()` is the filter the editor wrote last. */
function open(filter: PageListFilter) {
  const memory = new MemoryTransport({ now: () => 0, workspaceName: "W", spaces: [{ name: "S", pages: [] }] });
  const store = createAppStore(new CoreClient(memory));
  const stop = store.getState().start();
  const onChange = vi.fn<(group: FilterGroup) => void>();
  const user = userEvent.setup();
  render(
    <StoreContext value={store}>
      <Panel initial={{ op: "and", items: [filter] }} onChange={onChange} />
    </StoreContext>,
  );
  const last = (): PageListFilter | undefined => {
    const written = onChange.mock.lastCall?.[0].items[0];
    return written === undefined || "items" in written ? undefined : written;
  };
  return { user, onChange, stop, last };
}

const row = (): HTMLElement => within(screen.getByRole("dialog", { name: t("pageList.filter.label") })).getByRole("group", { name: t("pageList.filter.row", { n: 1 }) });
const select = (name: string): HTMLSelectElement => within(row()).getByRole<HTMLSelectElement>("combobox", { name });
const maybe = (role: "combobox" | "textbox", name: string): HTMLElement | null => within(row()).queryByRole(role, { name });
const optionsOf = (element: HTMLSelectElement): string[] => Array.from(element.options).map((option) => option.textContent ?? "");
const dateField = (): HTMLInputElement => within(row()).getByLabelText<HTMLInputElement>(t("pageList.filter.date"));

describe("the conditions the editor lists", () => {
  it("labels every condition of a text, in the order of the table", () => {
    const { stop } = open({ propertyId: "p-name", op: "contains", value: "x" });
    expect(optionsOf(select(t("pageList.filter.op")))).toEqual([
      t("pageList.op.contains"),
      t("formula.op.notContains"),
      t("pageList.op.equals"),
      t("formula.op.notEquals"),
      t("pageList.filter.op.startsWith"),
      t("pageList.filter.op.endsWith"),
      t("pageList.op.isSet"),
      t("pageList.op.isEmpty"),
    ]);
    stop();
  });

  it("labels every condition of a number, a select, a person and a date", async () => {
    const { user, stop } = open({ propertyId: "p-priority", op: "equals" });
    const property = (): HTMLSelectElement => select(t("pageList.filter.property"));
    const ops = (): string[] => optionsOf(select(t("pageList.filter.op")));
    expect(ops()).toEqual([
      t("pageList.op.equals"),
      t("formula.op.notEquals"),
      t("formula.op.greater"),
      t("formula.op.less"),
      t("formula.op.atLeast"),
      t("formula.op.atMost"),
      t("pageList.op.isSet"),
      t("pageList.op.isEmpty"),
    ]);
    await user.selectOptions(property(), "Stage");
    expect(ops()).toEqual([t("pageList.op.hasOption"), t("pageList.filter.op.lacksOption"), t("pageList.op.isSet"), t("pageList.op.isEmpty")]);
    await user.selectOptions(property(), "Tasks");
    expect(ops()).toEqual([t("pageList.op.relatesTo"), t("pageList.filter.op.notRelatesTo"), t("pageList.op.isSet"), t("pageList.op.isEmpty")]);
    await user.selectOptions(property(), "Owner");
    expect(ops()).toEqual([
      t("pageList.op.isMe"),
      t("pageList.filter.op.notMe"),
      t("pageList.op.hasPerson"),
      t("pageList.filter.op.lacksPerson"),
      t("pageList.op.isSet"),
      t("pageList.op.isEmpty"),
    ]);
    await user.selectOptions(property(), "Due");
    expect(ops()).toEqual([
      t("pageList.op.equals"),
      t("pageList.op.before"),
      t("pageList.op.after"),
      t("pageList.filter.op.onOrBefore"),
      t("pageList.filter.op.onOrAfter"),
      t("pageList.op.between"),
      t("pageList.filter.op.within"),
      t("pageList.op.isSet"),
      t("pageList.op.isEmpty"),
    ]);
    stop();
  });

  it("has a label of its own for every new condition", () => {
    const keys = [
      "pageList.filter.op.startsWith",
      "pageList.filter.op.endsWith",
      "pageList.filter.op.lacksOption",
      "pageList.filter.op.notRelatesTo",
      "pageList.filter.op.notMe",
      "pageList.filter.op.lacksPerson",
      "pageList.filter.op.onOrBefore",
      "pageList.filter.op.onOrAfter",
      "pageList.filter.op.within",
    ] as const;
    expect(new Set(keys.map((key) => t(key))).size).toBe(keys.length);
    for (const key of keys) {
      expect(t(key), key).not.toBe(key);
    }
  });
});

describe("the value of a day condition", () => {
  it.each(["equals", "before", "after", "onOrBefore", "onOrAfter"] as const)("is the select of days and the date field for \"%s\" on a date", (op) => {
    const { stop } = open({ propertyId: "p-due", op });
    expect(optionsOf(select(t("pageList.filter.dayMode")))).toEqual([
      t("pageList.filter.day.exact"),
      t("pageList.filter.day.today"),
      t("pageList.filter.day.yesterday"),
      t("pageList.filter.day.tomorrow"),
    ]);
    expect(select(t("pageList.filter.dayMode"))).toHaveValue("exact");
    expect(dateField()).toHaveValue("");
    stop();
  });

  it("is the same for the created time", () => {
    const { stop } = open({ propertyId: "p-made", op: "onOrAfter", value: { relative: "yesterday" } });
    expect(select(t("pageList.filter.dayMode"))).toHaveValue("yesterday");
    expect(within(row()).queryByLabelText(t("pageList.filter.date"))).not.toBeInTheDocument();
    stop();
  });

  it("shows an exact day in the date field, and writes the day the field gets", () => {
    const { last, stop } = open({ propertyId: "p-due", op: "before", value: "2026-10-14" });
    expect(select(t("pageList.filter.dayMode"))).toHaveValue("exact");
    expect(dateField()).toHaveValue("2026-10-14");
    fireEvent.change(dateField(), { target: { value: "2026-10-20" } });
    expect(last()).toEqual({ propertyId: "p-due", op: "before", value: "2026-10-20" });
    fireEvent.change(dateField(), { target: { value: "" } });
    expect(last()).toEqual({ propertyId: "p-due", op: "before" });
    stop();
  });

  it("writes a relative day for Today, Yesterday and Tomorrow, hides the date field, and comes back to an exact date with no day", async () => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "onOrBefore", value: "2026-10-14" });
    await user.selectOptions(select(t("pageList.filter.dayMode")), t("pageList.filter.day.today"));
    expect(last()).toEqual({ propertyId: "p-due", op: "onOrBefore", value: { relative: "today" } });
    expect(within(row()).queryByLabelText(t("pageList.filter.date"))).not.toBeInTheDocument();
    await user.selectOptions(select(t("pageList.filter.dayMode")), t("pageList.filter.day.yesterday"));
    expect(last()?.value).toEqual({ relative: "yesterday" });
    await user.selectOptions(select(t("pageList.filter.dayMode")), t("pageList.filter.day.tomorrow"));
    expect(last()?.value).toEqual({ relative: "tomorrow" });
    await user.selectOptions(select(t("pageList.filter.dayMode")), t("pageList.filter.day.exact"));
    expect(last()).toEqual({ propertyId: "p-due", op: "onOrBefore" });
    expect(dateField()).toHaveValue("");
    fireEvent.change(dateField(), { target: { value: "2026-11-02" } });
    expect(last()?.value).toBe("2026-11-02");
    stop();
  });

  it("shows a value it cannot read as an exact date with no day, and keeps it until a control writes", () => {
    const { onChange, stop } = open({ propertyId: "p-due", op: "after", value: { relative: "someday" } });
    expect(select(t("pageList.filter.dayMode"))).toHaveValue("exact");
    expect(dateField()).toHaveValue("");
    expect(onChange).not.toHaveBeenCalled();
    stop();
  });

  it("starts a chosen day condition without a value, whatever the old one held", async () => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "before", value: { relative: "today" } });
    await user.selectOptions(select(t("pageList.filter.op")), t("pageList.filter.op.onOrAfter"));
    expect(last()).toEqual({ propertyId: "p-due", op: "onOrAfter" });
    expect(select(t("pageList.filter.dayMode"))).toHaveValue("exact");
    stop();
  });
});

describe("the value of \"is within\"", () => {
  it("starts with this week when the condition is chosen, in the select of the periods", async () => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "before", value: "2026-10-14" });
    await user.selectOptions(select(t("pageList.filter.op")), t("pageList.filter.op.within"));
    expect(last()).toEqual({ propertyId: "p-due", op: "within", value: { relative: "thisWeek" } });
    const period = select(t("pageList.filter.period"));
    expect(period).toHaveValue("thisWeek");
    expect(optionsOf(period)).toEqual([
      t("pageList.filter.period.thisWeek"),
      t("pageList.filter.period.lastWeek"),
      t("pageList.filter.period.nextWeek"),
      t("pageList.filter.period.thisMonth"),
      t("pageList.filter.period.lastMonth"),
      t("pageList.filter.period.nextMonth"),
      t("pageList.filter.period.pastDays"),
      t("pageList.filter.period.nextDays"),
    ]);
    // A week has no number of days, and no date field either.
    expect(maybe("textbox", t("pageList.filter.days"))).toBeNull();
    expect(within(row()).queryByLabelText(t("pageList.filter.date"))).not.toBeInTheDocument();
    stop();
  });

  it.each([
    ["lastWeek", t("pageList.filter.period.lastWeek")],
    ["nextWeek", t("pageList.filter.period.nextWeek")],
    ["thisMonth", t("pageList.filter.period.thisMonth")],
    ["lastMonth", t("pageList.filter.period.lastMonth")],
    ["nextMonth", t("pageList.filter.period.nextMonth")],
  ])("writes %s with no number of days", async (relative, label) => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "within", value: { relative: "pastDays", n: 30 } });
    await user.selectOptions(select(t("pageList.filter.period")), label);
    expect(last()).toEqual({ propertyId: "p-due", op: "within", value: { relative } });
    expect(maybe("textbox", t("pageList.filter.days"))).toBeNull();
    stop();
  });

  it("asks how many days for the past and the next days, starting at 7 and keeping the number from one to the other", async () => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "within", value: { relative: "thisWeek" } });
    await user.selectOptions(select(t("pageList.filter.period")), t("pageList.filter.period.pastDays"));
    expect(last()?.value).toEqual({ relative: "pastDays", n: 7 });
    const days = (): HTMLInputElement => within(row()).getByRole<HTMLInputElement>("textbox", { name: t("pageList.filter.days") });
    expect(days()).toHaveValue("7");
    await user.clear(days());
    await user.type(days(), "30{Enter}");
    expect(last()?.value).toEqual({ relative: "pastDays", n: 30 });
    await user.selectOptions(select(t("pageList.filter.period")), t("pageList.filter.period.nextDays"));
    expect(last()?.value).toEqual({ relative: "nextDays", n: 30 });
    expect(days()).toHaveValue("30");
    stop();
  });

  it("writes the number of days when the field is left, cuts it to 365, and drops it when it is no whole number from 1", async () => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "within", value: { relative: "nextDays", n: 7 } });
    const days = (): HTMLInputElement => within(row()).getByRole<HTMLInputElement>("textbox", { name: t("pageList.filter.days") });
    await user.clear(days());
    await user.type(days(), "400");
    await user.tab();
    expect(last()?.value).toEqual({ relative: "nextDays", n: 365 });
    expect(days()).toHaveValue("365");
    await user.clear(days());
    await user.type(days(), "2.5{Enter}");
    expect(last()?.value).toEqual({ relative: "nextDays" });
    expect(days()).toHaveValue("");
    await user.type(days(), "0{Enter}");
    expect(last()?.value).toEqual({ relative: "nextDays" });
    await user.clear(days());
    await user.type(days(), "1{Enter}");
    expect(last()?.value).toEqual({ relative: "nextDays", n: 1 });
    stop();
  });

  it("shows a stored period, with its number of days", () => {
    const { onChange, stop } = open({ propertyId: "p-made", op: "within", value: { relative: "pastDays", n: 14 } });
    expect(select(t("pageList.filter.period"))).toHaveValue("pastDays");
    expect(within(row()).getByRole("textbox", { name: t("pageList.filter.days") })).toHaveValue("14");
    expect(onChange).not.toHaveBeenCalled();
    stop();
  });

  it("asks to choose a period for a value that is none, and a period without its days shows an empty field", async () => {
    const { user, last, stop } = open({ propertyId: "p-due", op: "within", value: "next week" });
    const period = select(t("pageList.filter.period"));
    expect(period).toHaveValue("");
    expect(optionsOf(period)[0]).toBe(t("pageList.filter.choosePeriod"));
    expect(maybe("textbox", t("pageList.filter.days"))).toBeNull();
    await user.selectOptions(period, t("pageList.filter.period.thisMonth"));
    expect(last()?.value).toEqual({ relative: "thisMonth" });
    // The placeholder is gone once a period is chosen.
    expect(optionsOf(select(t("pageList.filter.period")))).not.toContain(t("pageList.filter.choosePeriod"));
    stop();
  });

  it("shows a count of days that is not usable as an empty field", () => {
    const { stop } = open({ propertyId: "p-due", op: "within", value: { relative: "pastDays", n: 999 } });
    expect(select(t("pageList.filter.period"))).toHaveValue("pastDays");
    expect(within(row()).getByRole("textbox", { name: t("pageList.filter.days") })).toHaveValue("");
    stop();
  });
});

describe("the value of the other new conditions", () => {
  it("is a text for starts with and ends with, written when the field is left", async () => {
    const { user, last, stop } = open({ propertyId: "p-name", op: "startsWith" });
    await user.type(within(row()).getByRole("textbox", { name: t("pageList.filter.text") }), "Project");
    await user.tab();
    expect(last()).toEqual({ propertyId: "p-name", op: "startsWith", value: "Project" });
    await user.selectOptions(select(t("pageList.filter.op")), t("pageList.filter.op.endsWith"));
    expect(last()).toEqual({ propertyId: "p-name", op: "endsWith" });
    stop();
  });

  it("is an option for \"does not have\", a page for \"does not link to\" and a person for \"does not include\"", async () => {
    const { user, last, stop } = open({ propertyId: "p-stage", op: "lacksOption" });
    expect(optionsOf(select(t("pageList.filter.option")))).toEqual([t("pageList.filter.chooseOption"), "Draft", "Review"]);
    await user.selectOptions(select(t("pageList.filter.option")), "Review");
    expect(last()).toEqual({ propertyId: "p-stage", op: "lacksOption", value: "o-review" });
    await user.selectOptions(select(t("pageList.filter.property")), "Tasks");
    await user.selectOptions(select(t("pageList.filter.op")), t("pageList.filter.op.notRelatesTo"));
    expect(within(row()).getByRole("button", { name: t("pageList.filter.page") })).toBeInTheDocument();
    await user.selectOptions(select(t("pageList.filter.property")), "Owner");
    await user.selectOptions(select(t("pageList.filter.op")), t("pageList.filter.op.lacksPerson"));
    expect(select(t("pageList.filter.person"))).toHaveValue("");
    stop();
  });

  it("is nothing for \"is not me\", which stands alone", async () => {
    const { user, last, stop } = open({ propertyId: "p-owner", op: "isMe" });
    await user.selectOptions(select(t("pageList.filter.op")), t("pageList.filter.op.notMe"));
    expect(last()).toEqual({ propertyId: "p-owner", op: "notMe" });
    expect(within(row()).queryByRole("combobox", { name: t("pageList.filter.person") })).not.toBeInTheDocument();
    stop();
  });

  it("is a number for the comparisons of a number", async () => {
    const { user, last, stop } = open({ propertyId: "p-priority", op: "greater" });
    await user.type(within(row()).getByRole("textbox", { name: t("pageList.filter.number") }), "3");
    await user.tab();
    expect(last()).toEqual({ propertyId: "p-priority", op: "greater", value: 3 });
    stop();
  });
});
