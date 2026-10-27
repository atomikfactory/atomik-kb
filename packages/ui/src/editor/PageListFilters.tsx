// The filter editor of a view: a popover with the view's filter tree (ADR-0054, `page-list-filter-tree.ts`): a top
// "Match all filters / any filter" control, one row per filter (a property, a condition and the value the
// condition needs), "Add a filter", and "Add filter group", a box with its own all/any control, its filter rows,
// "Add a filter to group n" and "Remove group". One level only: a group cannot hold a group. At most 50 filters in
// all and 10 groups; the buttons that would go past them are disabled and say so. Every change writes the whole
// view's `filter` in one transaction (`onChange`); the fields that take typed text write when they are left or on
// Enter, not on every key, so that a filter is one change of the document. A filter this version cannot show (its
// property is gone, or its condition is not one it knows) is listed as it is, with the one control that can be used
// on it: remove.
//
// The value of a day condition (is, before, after, on or before, on or after) is a select (Exact date, Today,
// Yesterday or Tomorrow) and, for an exact date, the date field; the value of "is within" is a select of the
// periods (this, last and next week and month, the past or the next days) and, for the days, their number (ADR-0058).

import type { PropertyDef } from "@atomik-kb/core-client";
import { createContext, useContext, useEffect, useRef, useState } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { PagePickerPanel, relatedPageTitle } from "../components/RelationEditor";
import { CloseIcon } from "../components/property-icons";
import { PlusIcon } from "../components/icons";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { usePeople, usePersonNames } from "../comments-hooks";
import { formulaShapeOf } from "../formula-format";
import { deviceLabel, parseNumber } from "../property-format";
import { useAppStore } from "../store-context";
import { useReferencedPage } from "./node-views";
import {
  addFilter,
  addGroup,
  canAddFilter,
  canAddGroup,
  EMPTY_FILTER_GROUP,
  isFilterGroup,
  MAX_FILTER_GROUPS,
  MAX_FILTERS,
  removeAt,
  replaceFilter,
  setGroupOp,
} from "./page-list-filter-tree";
import type { FilterGroup, FilterGroupOp, FilterPath } from "./page-list-filter-tree";
import {
  DEFAULT_RELATIVE_DAYS,
  filterForOp,
  filterOps,
  isFilterOp,
  MAX_RELATIVE_DAYS,
  newFilter,
  opTakesValue,
  periodCountsDays,
  readDateRange,
  readRelativeDay,
  readRelativePeriod,
  RELATIVE_DAYS,
  RELATIVE_PERIODS,
} from "./page-list-filters";
import type { FilterOp, PageListFilter, RelativeDay, RelativePeriodName } from "./page-list-filters";

const OP_LABELS: Readonly<Record<FilterOp, MessageKey>> = {
  isSet: "pageList.op.isSet",
  isEmpty: "pageList.op.isEmpty",
  equals: "pageList.op.equals",
  contains: "pageList.op.contains",
  hasOption: "pageList.op.hasOption",
  before: "pageList.op.before",
  after: "pageList.op.after",
  between: "pageList.op.between",
  checked: "pageList.op.checked",
  unchecked: "pageList.op.unchecked",
  complete: "pageList.op.complete",
  incomplete: "pageList.op.incomplete",
  relatesTo: "pageList.op.relatesTo",
  isMe: "pageList.op.isMe",
  hasPerson: "pageList.op.hasPerson",
  notEquals: "formula.op.notEquals",
  greater: "formula.op.greater",
  less: "formula.op.less",
  atLeast: "formula.op.atLeast",
  atMost: "formula.op.atMost",
  notContains: "formula.op.notContains",
  startsWith: "pageList.filter.op.startsWith",
  endsWith: "pageList.filter.op.endsWith",
  lacksOption: "pageList.filter.op.lacksOption",
  notRelatesTo: "pageList.filter.op.notRelatesTo",
  notMe: "pageList.filter.op.notMe",
  lacksPerson: "pageList.filter.op.lacksPerson",
  onOrBefore: "pageList.filter.op.onOrBefore",
  onOrAfter: "pageList.filter.op.onOrAfter",
  within: "pageList.filter.op.within",
};

const DAY_LABELS: Readonly<Record<RelativeDay, MessageKey>> = {
  today: "pageList.filter.day.today",
  yesterday: "pageList.filter.day.yesterday",
  tomorrow: "pageList.filter.day.tomorrow",
};

const PERIOD_LABELS: Readonly<Record<RelativePeriodName, MessageKey>> = {
  thisWeek: "pageList.filter.period.thisWeek",
  lastWeek: "pageList.filter.period.lastWeek",
  nextWeek: "pageList.filter.period.nextWeek",
  thisMonth: "pageList.filter.period.thisMonth",
  lastMonth: "pageList.filter.period.lastMonth",
  nextMonth: "pageList.filter.period.nextMonth",
  pastDays: "pageList.filter.period.pastDays",
  nextDays: "pageList.filter.period.nextDays",
};

/** A text field that writes what it holds when it is left or on Enter (and only when that changed it). */
function DraftField({
  value,
  label,
  inputMode,
  onCommit,
}: {
  value: string;
  label: string;
  inputMode?: "decimal" | "numeric";
  onCommit: (text: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  const [seen, setSeen] = useState(value);
  // What the document holds changed (an edit from elsewhere): the field follows it.
  if (seen !== value) {
    setSeen(value);
    setDraft(value);
  }
  function commit(): void {
    if (draft !== value) {
      onCommit(draft);
    }
  }
  return (
    <input
      type="text"
      className="page-filter-input"
      aria-label={label}
      inputMode={inputMode}
      autoComplete="off"
      value={draft}
      onChange={(event) => {
        setDraft(event.target.value);
      }}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter" && !event.nativeEvent.isComposing) {
          event.preventDefault();
          commit();
        }
      }}
    />
  );
}

/**
 * Tells the popover that a picker (itself a floating panel) is open over it: a press inside the
 * picker is outside the popover as far as the popover can tell, and must not close it.
 */
const PickerOpen = createContext<(open: boolean) => void>(() => undefined);

/** The device a filter on "created by" / "last edited by" is for: this device and the ones the log lists. */
function DeviceField({ stored, onPick }: { stored: unknown; onPick: (id: string | undefined) => void }) {
  const devices = useAppStore((state) => state.devices);
  const device = useAppStore((state) => state.device);
  const own = device === null ? null : { deviceId: device.deviceId, name: device.name };
  const choices = [
    ...(own === null ? [] : [{ id: own.deviceId, name: own.name }]),
    ...devices.filter((entry) => entry.revokedAt === undefined && entry.deviceId !== own?.deviceId).map((entry) => ({ id: entry.deviceId, name: entry.name })),
  ];
  const known = typeof stored === "string" && choices.some((choice) => choice.id === stored);
  return (
    <select
      className="page-list-select"
      aria-label={t("pageList.filter.device")}
      value={known ? stored : ""}
      onChange={(event) => {
        onPick(event.target.value === "" ? undefined : event.target.value);
      }}
    >
      <option value="">{t("pageList.filter.chooseDevice")}</option>
      {choices.map((choice) => (
        <option key={choice.id} value={choice.id}>
          {deviceLabel(choice.id, devices, own)}
        </option>
      ))}
      {typeof stored === "string" && stored !== "" && !known && <option value={stored}>{deviceLabel(stored, devices, own)}</option>}
    </select>
  );
}

/** The person a "includes" filter is for: the members of the space, and the one the filter holds when they left. */
function PersonField({ stored, onPick }: { stored: unknown; onPick: (id: string | undefined) => void }) {
  const spaceId = useAppStore((state) => state.tree?.spaceId ?? null);
  const people = usePeople(spaceId);
  const chosen = typeof stored === "string" && stored !== "" ? stored : "";
  const known = chosen !== "" && (people ?? []).some((person) => person.accountId === chosen);
  const formerName = usePersonNames(spaceId, chosen === "" ? [] : [chosen])(chosen);
  return (
    <select
      className="page-list-select"
      aria-label={t("pageList.filter.person")}
      value={chosen}
      onChange={(event) => {
        onPick(event.target.value === "" ? undefined : event.target.value);
      }}
    >
      <option value="">{t("pageList.filter.choosePerson")}</option>
      {(people ?? []).map((person) => (
        <option key={person.accountId} value={person.accountId}>
          {person.name}
        </option>
      ))}
      {chosen !== "" && !known && <option value={chosen}>{formerName === "" ? t("comments.formerMember") : formerName}</option>}
    </select>
  );
}

/** The page a "links to" filter is for: a button with its title that opens the page picker. */
function PageField({ stored, onPick }: { stored: unknown; onPick: (id: string) => void }) {
  const spaceId = useAppStore((state) => state.tree?.spaceId ?? "");
  const [picker, setPicker] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const setPickerOpen = useContext(PickerOpen);
  const open = picker !== null;
  useEffect(() => {
    if (!open) {
      return;
    }
    setPickerOpen(true);
    return () => {
      setPickerOpen(false);
    };
  }, [open, setPickerOpen]);
  const chosen = typeof stored === "string" && stored !== "" ? stored : null;
  return (
    <>
      <button
        type="button"
        className="page-list-select"
        aria-haspopup="dialog"
        aria-expanded={picker !== null}
        aria-label={t("pageList.filter.page")}
        onClick={(event) => {
          setPicker((current) => (current === null ? { anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget } : null));
        }}
      >
        {chosen === null ? t("pageList.filter.choosePage") : <ChosenPageTitle id={chosen} />}
      </button>
      {picker !== null && (
        <PagePickerPanel
          anchor={picker.anchor}
          anchorElement={picker.trigger}
          spaceId={spaceId}
          name={t("pageList.filter.page")}
          taken={chosen === null ? [] : [chosen]}
          onPick={(id) => {
            setPicker(null);
            onPick(id);
          }}
          onClose={() => {
            setPicker(null);
          }}
        />
      )}
    </>
  );
}

function ChosenPageTitle({ id }: { id: string }) {
  return <>{relatedPageTitle(useReferencedPage(id))}</>;
}

/**
 * The value of a day condition: "Exact date" with the date field, or a relative day (Today, Yesterday, Tomorrow),
 * which each reader resolves on their own calendar. `set` gets the stored value (`undefined`: none yet).
 */
function DayValueField({ stored, set }: { stored: unknown; set: (value: unknown) => void }) {
  const relative = readRelativeDay(stored);
  return (
    <>
      <select
        className="page-list-select"
        aria-label={t("pageList.filter.dayMode")}
        value={relative ?? "exact"}
        onChange={(event) => {
          const next = RELATIVE_DAYS.find((day) => day === event.target.value);
          set(next === undefined ? undefined : { relative: next });
        }}
      >
        <option value="exact">{t("pageList.filter.day.exact")}</option>
        {RELATIVE_DAYS.map((day) => (
          <option key={day} value={day}>
            {t(DAY_LABELS[day])}
          </option>
        ))}
      </select>
      {relative === null && (
        <input
          type="date"
          className="page-filter-input"
          aria-label={t("pageList.filter.date")}
          value={typeof stored === "string" ? stored : ""}
          onChange={(event) => {
            set(event.target.value === "" ? undefined : event.target.value);
          }}
        />
      )}
    </>
  );
}

/** The value of "is within": a relative period and, for the past or the next days, how many (1 to 365). */
function PeriodValueField({ stored, set }: { stored: unknown; set: (value: unknown) => void }) {
  const period = readRelativePeriod(stored);
  return (
    <>
      <select
        className="page-list-select"
        aria-label={t("pageList.filter.period")}
        value={period?.relative ?? ""}
        onChange={(event) => {
          const next = RELATIVE_PERIODS.find((name) => name === event.target.value);
          if (next === undefined) {
            set(undefined);
          } else {
            set(periodCountsDays(next) ? { relative: next, n: period?.n ?? DEFAULT_RELATIVE_DAYS } : { relative: next });
          }
        }}
      >
        {period === null && <option value="">{t("pageList.filter.choosePeriod")}</option>}
        {RELATIVE_PERIODS.map((name) => (
          <option key={name} value={name}>
            {t(PERIOD_LABELS[name])}
          </option>
        ))}
      </select>
      {period !== null && periodCountsDays(period.relative) && (
        <DraftField
          value={period.n === undefined ? "" : String(period.n)}
          label={t("pageList.filter.days")}
          inputMode="numeric"
          onCommit={(text) => {
            const parsed = parseNumber(text);
            const n =
              parsed.kind === "number" && Number.isInteger(parsed.value) && parsed.value >= 1
                ? Math.min(parsed.value, MAX_RELATIVE_DAYS)
                : undefined;
            set(n === undefined ? { relative: period.relative } : { relative: period.relative, n });
          }}
        />
      )}
    </>
  );
}

interface RowProps {
  /** The accessible name of the row ("Filter 2", "Filter 1 of group 2"). */
  label: string;
  filter: PageListFilter;
  defs: readonly PropertyDef[];
  onChange: (filter: PageListFilter) => void;
  onRemove: () => void;
}

/** The control for the value of a filter, `null` when its condition stands alone. */
function ValueField({ filter, def, onChange }: { filter: PageListFilter; def: PropertyDef; onChange: (filter: PageListFilter) => void }) {
  const set = (value: unknown): void => {
    onChange(value === undefined ? { propertyId: filter.propertyId, op: filter.op } : { propertyId: filter.propertyId, op: filter.op, value });
  };
  const stored = filter.value;
  // The result of a formula decides what its conditions take: a number, a text or a day.
  const formulaShape = def.kind === "formula" ? formulaShapeOf(def) : null;
  switch (filter.op) {
    case "hasOption":
    case "lacksOption":
      return (
        <select
          className="page-list-select"
          aria-label={t("pageList.filter.option")}
          value={typeof stored === "string" && def.options.some((option) => option.id === stored) ? stored : ""}
          onChange={(event) => {
            set(event.target.value === "" ? undefined : event.target.value);
          }}
        >
          <option value="">{t("pageList.filter.chooseOption")}</option>
          {def.options.map((option) => (
            <option key={option.id} value={option.id}>
              {option.name}
            </option>
          ))}
        </select>
      );
    case "contains":
    case "notContains":
    case "startsWith":
    case "endsWith":
      return <DraftField value={typeof stored === "string" ? stored : ""} label={t("pageList.filter.text")} onCommit={(text) => { set(text.trim() === "" ? undefined : text); }} />;
    case "greater":
    case "less":
    case "atLeast":
    case "atMost":
      return (
        <DraftField
          value={typeof stored === "number" ? String(stored) : ""}
          label={t("pageList.filter.number")}
          inputMode="decimal"
          onCommit={(text) => {
            const parsed = parseNumber(text);
            set(parsed.kind === "number" ? parsed.value : undefined);
          }}
        />
      );
    case "notEquals":
    case "equals":
      if (formulaShape === "date" || def.kind === "date" || def.kind === "created_time" || def.kind === "last_edited_time") {
        return <DayValueField stored={stored} set={set} />;
      }
      if (filter.op === "equals" && (def.kind === "created_by" || def.kind === "last_edited_by")) {
        return <DeviceField stored={stored} onPick={set} />;
      }
      if (def.kind === "number" || def.kind === "rollup" || formulaShape === "number") {
        return (
          <DraftField
            value={typeof stored === "number" ? String(stored) : ""}
            label={t("pageList.filter.number")}
            inputMode="decimal"
            onCommit={(text) => {
              const parsed = parseNumber(text);
              set(parsed.kind === "number" ? parsed.value : undefined);
            }}
          />
        );
      }
      return <DraftField value={typeof stored === "string" ? stored : ""} label={t("pageList.filter.text")} onCommit={(text) => { set(text === "" ? undefined : text); }} />;
    case "relatesTo":
    case "notRelatesTo":
      return <PageField stored={stored} onPick={set} />;
    case "hasPerson":
    case "lacksPerson":
      return <PersonField stored={stored} onPick={set} />;
    case "before":
    case "after":
    case "onOrBefore":
    case "onOrAfter":
      return <DayValueField stored={stored} set={set} />;
    case "within":
      return <PeriodValueField stored={stored} set={set} />;
    case "between": {
      const range = readDateRange(stored);
      const change = (end: "from" | "to", day: string): void => {
        const next = { ...range };
        if (day === "") {
          delete next[end];
        } else {
          next[end] = day;
        }
        set(next.from === undefined && next.to === undefined ? undefined : next);
      };
      return (
        <>
          <input
            type="date"
            className="page-filter-input"
            aria-label={t("pageList.filter.from")}
            value={range.from ?? ""}
            onChange={(event) => {
              change("from", event.target.value);
            }}
          />
          <input
            type="date"
            className="page-filter-input"
            aria-label={t("pageList.filter.to")}
            value={range.to ?? ""}
            onChange={(event) => {
              change("to", event.target.value);
            }}
          />
        </>
      );
    }
    default:
      return null;
  }
}

function FilterRow({ label, filter, defs, onChange, onRemove }: RowProps) {
  const def = defs.find((candidate) => candidate.id === filter.propertyId);
  const known = def !== undefined && isFilterOp(def.kind, filter.op, def);
  return (
    <div className="page-filter-row" role="group" aria-label={label}>
      {def === undefined || !known ? (
        <span className="page-filter-unknown">{t("pageList.filter.unknown")}</span>
      ) : (
        <>
          <select
            className="page-list-select"
            aria-label={t("pageList.filter.property")}
            value={def.id}
            onChange={(event) => {
              const next = defs.find((candidate) => candidate.id === event.target.value);
              if (next !== undefined) {
                onChange(newFilter(next));
              }
            }}
          >
            {defs.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </select>
          <select
            className="page-list-select"
            aria-label={t("pageList.filter.op")}
            value={filter.op}
            onChange={(event) => {
              // Another condition starts without a value (the old one may mean something else).
              onChange(filterForOp(def.id, event.target.value));
            }}
          >
            {filterOps(def.kind, def).map((op) => (
              <option key={op} value={op}>
                {t(OP_LABELS[op])}
              </option>
            ))}
          </select>
          {opTakesValue(filter.op) && <ValueField filter={filter} def={def} onChange={onChange} />}
        </>
      )}
      <button type="button" className="icon-button" aria-label={t("pageList.filter.remove")} title={t("pageList.filter.remove")} onClick={onRemove}>
        <CloseIcon />
      </button>
    </div>
  );
}

/** The "match all / any" select of a group; `label` names it for assistive technology. */
function MatchSelect({ op, label, onChange }: { op: FilterGroupOp; label: string; onChange: (op: FilterGroupOp) => void }) {
  return (
    <div className="page-filter-match">
      <span aria-hidden="true">{t("pageList.filter.match")}</span>
      <select
        className="page-list-select"
        aria-label={label}
        value={op}
        onChange={(event) => {
          onChange(event.target.value === "or" ? "or" : "and");
        }}
      >
        <option value="and">{t("pageList.filter.match.all")}</option>
        <option value="or">{t("pageList.filter.match.any")}</option>
      </select>
    </div>
  );
}

export interface PageListFiltersProps {
  anchor: Anchor;
  anchorElement: Element;
  defs: readonly PropertyDef[];
  /** The view's filter tree. */
  group: FilterGroup;
  /** Gets the whole tree after each change (the default tree, `EMPTY_FILTER_GROUP`, when the last item was removed). */
  onChange: (group: FilterGroup) => void;
  onClose: () => void;
}

/** The filters popover. It stays open while filters are changed. */
export function PageListFilters({ anchor, anchorElement, defs, group, onChange, onClose }: PageListFiltersProps) {
  const [first] = defs;
  const picking = useRef(false);
  const filterFits = canAddFilter(group);
  const groupFits = canAddGroup(group);

  /** Reports a change; a tree left with nothing in it is the default one (an OR of nothing says nothing). */
  function change(next: FilterGroup): void {
    if (next === group) {
      return;
    }
    onChange(next.items.length === 0 ? EMPTY_FILTER_GROUP : next);
  }

  // The numbers in the names: the top-level filters count among themselves, the groups among themselves.
  const numbers: number[] = [];
  let filtersSeen = 0;
  let groupsSeen = 0;
  for (const item of group.items) {
    if (isFilterGroup(item)) {
      groupsSeen += 1;
      numbers.push(groupsSeen);
    } else {
      filtersSeen += 1;
      numbers.push(filtersSeen);
    }
  }
  // The top control says nothing about fewer than two items; it stays for a tree that is already "any".
  const showMatch = group.items.length >= 2 || group.op === "or";

  return (
    <FloatingPanel
      anchor={anchor}
      anchorElement={anchorElement}
      label={t("pageList.filter.label")}
      className="page-filters"
      onClose={() => {
        if (!picking.current) {
          onClose();
        }
      }}
    >
      <PickerOpen
        value={(open) => {
          picking.current = open;
        }}
      >
        {showMatch && (
          <MatchSelect
            op={group.op}
            label={t("pageList.filter.match")}
            onChange={(op) => {
              change(setGroupOp(group, op));
            }}
          />
        )}
        {group.items.length === 0 && <p className="page-list-props-empty">{t("pageList.filter.none")}</p>}
        {group.items.map((item, index) => {
          const n = numbers[index] ?? index + 1;
          if (!isFilterGroup(item)) {
            return (
              <FilterRow
                // A filter has no id of its own: its place in the list is its identity.
                key={index}
                label={t("pageList.filter.row", { n })}
                filter={item}
                defs={defs}
                onChange={(next) => {
                  change(replaceFilter(group, [index], next));
                }}
                onRemove={() => {
                  change(removeAt(group, [index]));
                }}
              />
            );
          }
          return (
            <div
              // A group has no id of its own either.
              key={index}
              className="page-filter-group"
              role="group"
              aria-label={t("pageList.filter.group", { n })}
            >
              <div className="page-filter-group-head">
                <MatchSelect
                  op={item.op}
                  label={t("pageList.filter.groupMatch", { n })}
                  onChange={(op) => {
                    change(setGroupOp(group, op, index));
                  }}
                />
                <button
                  type="button"
                  className="icon-button"
                  aria-label={t("pageList.filter.removeGroup")}
                  title={t("pageList.filter.removeGroup")}
                  onClick={() => {
                    const path: FilterPath = [index];
                    change(removeAt(group, path));
                  }}
                >
                  <CloseIcon />
                </button>
              </div>
              {item.items.length === 0 && <p className="page-list-props-empty">{t("pageList.filter.groupEmpty")}</p>}
              {item.items.map((entry, at) =>
                isFilterGroup(entry) ? null : (
                  <FilterRow
                    key={at}
                    label={t("pageList.filter.rowInGroup", { n: at + 1, group: n })}
                    filter={entry}
                    defs={defs}
                    onChange={(next) => {
                      change(replaceFilter(group, [index, at], next));
                    }}
                    onRemove={() => {
                      change(removeAt(group, [index, at]));
                    }}
                  />
                ),
              )}
              <button
                type="button"
                className="page-filter-add"
                disabled={first === undefined || !filterFits}
                onClick={() => {
                  if (first !== undefined) {
                    change(addFilter(group, newFilter(first), index));
                  }
                }}
              >
                <PlusIcon />
                <span>{t("pageList.filter.addToGroup", { n })}</span>
              </button>
            </div>
          );
        })}
        <div className="page-filter-actions">
          <button
            type="button"
            className="page-filter-add"
            disabled={first === undefined || !filterFits}
            onClick={() => {
              if (first !== undefined) {
                change(addFilter(group, newFilter(first)));
              }
            }}
          >
            <PlusIcon />
            <span>{t("pageList.filter.add")}</span>
          </button>
          <button
            type="button"
            className="page-filter-add"
            disabled={!groupFits}
            onClick={() => {
              change(addGroup(group));
            }}
          >
            <PlusIcon />
            <span>{t("pageList.filter.addGroup")}</span>
          </button>
        </div>
        {!filterFits && (
          <p className="page-list-props-empty" role="status">
            {t("pageList.filter.limit", { max: MAX_FILTERS })}
          </p>
        )}
        {!groupFits && (
          <p className="page-list-props-empty" role="status">
            {t("pageList.filter.groupsLimit", { max: MAX_FILTER_GROUPS })}
          </p>
        )}
      </PickerOpen>
    </FloatingPanel>
  );
}
