// A property value on a row of the list view, read-only: an option as its coloured pill, a date as
// the property panel writes it, a checkbox as a box, a number as the locale writes it, text cut to
// one short line, a place by its name, the times and devices of ADR-0026, the pages of a relation as
// chips, the people of a Person property by their names ("Former member" for one who left, ADR-0049), a rollup's result
// (ADR-0027) and a formula's result (ADR-0055: `FormulaValue`, also the properties panel's and the editor's preview), and a
// value of a kind this version does not know as its text.

import type { DeviceId, FormulaResult, PageId, PropertyDef, PropertyValue } from "@atomik-kb/core-client";

import { initialOf, isFormerMember } from "../comments";
import { usePeople, usePersonNames } from "../comments-hooks";
import { OptionPill } from "../components/OptionPill";
import { PageIcon } from "../components/PageIcon";
import { relatedPageTitle } from "../components/RelationEditor";
import { t } from "../i18n";
import { formulaItems } from "../formula-format";
import { deviceLabel, formatInstant, formatNumber, rollupConfigOf, rollupTexts } from "../property-format";
import { useAppStore } from "../store-context";
import { formatDateMention } from "./dates";
import { useReferencedPage } from "./node-views";
import { genericText } from "./page-list-model";

/** Longest text a cell shows, in characters; the full text is its tooltip. */
export const MAX_CELL_CHARS = 80;

/** `text` on one line (runs of white space become one space), cut to `MAX_CELL_CHARS` with an ellipsis. */
export function oneLine(text: string): { shown: string; cut: boolean } {
  const flat = text.replace(/\s+/gu, " ").trim();
  const chars = Array.from(flat);
  return chars.length <= MAX_CELL_CHARS
    ? { shown: flat, cut: false }
    : { shown: `${chars.slice(0, MAX_CELL_CHARS - 1).join("")}…`, cut: true };
}

function TextValue({ text }: { text: string }) {
  const { shown, cut } = oneLine(text);
  if (shown === "") {
    return null;
  }
  return (
    <span className="page-list-text" title={cut ? text : undefined}>
      {shown}
    </span>
  );
}

/** A device by the name the device log gives it ("Unknown device" for one it does not list). */
function DeviceName({ id }: { id: DeviceId }) {
  const devices = useAppStore((state) => state.devices);
  const device = useAppStore((state) => state.device);
  const own = device === null ? null : { deviceId: device.deviceId, name: device.name };
  return <TextValue text={deviceLabel(id, devices, own)} />;
}

/** One page a relation links to: its icon and live title ("Unknown page" when it cannot be read). */
function RelatedPage({ id }: { id: PageId }) {
  const page = useReferencedPage(id);
  const found = page.status === "found" ? page.page : null;
  return (
    <span
      className={`relation-chip is-static${found?.trashedAt !== undefined ? " is-trashed" : ""}${page.status === "missing" ? " is-deleted" : ""}`}
    >
      <span className="relation-chip-open">
        <PageIcon icon={found?.icon} className="relation-chip-icon" />
        <span className="relation-chip-title">{relatedPageTitle(page)}</span>
      </span>
    </span>
  );
}

/** One person of a Person value: a round mark and the member's current name, "Former member" for one the space does not list. */
function PersonName({ id }: { id: string }) {
  const spaceId = useAppStore((state) => state.tree?.spaceId ?? null);
  const people = usePeople(spaceId);
  const name = usePersonNames(spaceId, [id])(id);
  return (
    <span className={`person-chip is-static${isFormerMember(people, id) ? " is-former" : ""}`}>
      <span className="person-mark" aria-hidden="true">
        {initialOf(name === "" ? "?" : name)}
      </span>
      <span className="person-chip-name">{name}</span>
    </span>
  );
}

/** The result of a rollup: a number, a day or a progress as text, "show original" as one item per value. */
function RollupResultValue({
  def,
  value,
  today,
  schema,
}: {
  def: PropertyDef;
  value: Extract<PropertyValue, { kind: "rollup" }>;
  today: string;
  schema: readonly PropertyDef[];
}) {
  const config = rollupConfigOf(def);
  const target = config === undefined ? undefined : schema.find((candidate) => candidate.id === config.target);
  const texts = rollupTexts(value.value.function, value.value.result, target, (date, time) => formatDateMention(date, time, today));
  if (value.value.result.type !== "values") {
    return <TextValue text={texts.join(", ")} />;
  }
  return texts.length === 0 ? null : (
    <span className="page-list-pills">
      {texts.map((text, at) => (
        <span key={`${String(at)}:${text}`} className="rollup-item">
          {text}
        </span>
      ))}
    </span>
  );
}

/** Where a formula's result is drawn: in a cell of a view, in the properties panel (full text, a box to look at), or the editor's preview. */
export type FormulaVariant = "cell" | "panel";

/** One scalar of a formula's result as text (a list's item, or the whole result). */
function formulaScalarText(result: FormulaResult, today: string): string {
  switch (result.type) {
    case "number":
      return formatNumber(result.data);
    case "text":
      return result.data;
    case "date":
      return formatDateMention(result.data.date, result.data.time ?? null, today);
    case "boolean":
      return t(result.data ? "properties.rollup.yes" : "properties.rollup.no");
    default:
      return "";
  }
}

/** A text of a formula's result: one line cut short in a cell, in full in the panel. */
function FormulaText({ text, variant }: { text: string; variant: FormulaVariant }) {
  return variant === "cell" ? <TextValue text={text} /> : <span className="properties-text">{text}</span>;
}

/**
 * The result of a formula, read only (ADR-0055): a number as the locale writes it, a text, a day as a date property
 * shows it, a boolean as a box (checked or not), a page as a chip with its live title, a person as a chip with the name,
 * and a list as one chip per item.
 */
export function FormulaValue({ result, today, variant = "cell" }: { result: FormulaResult; today: string; variant?: FormulaVariant }) {
  switch (result.type) {
    case "boolean":
      return variant === "cell" ? (
        <span
          className="page-list-check"
          role="img"
          aria-label={result.data ? t("pageList.checked") : t("pageList.unchecked")}
          data-checked={result.data ? "true" : "false"}
        >
          {result.data ? "✓" : ""}
        </span>
      ) : (
        <input
          type="checkbox"
          className="property-checkbox"
          checked={result.data}
          readOnly
          disabled
          aria-label={result.data ? t("pageList.checked") : t("pageList.unchecked")}
        />
      );
    case "page":
      return <RelatedPage id={result.data} />;
    case "person":
      return <PersonName id={result.data} />;
    case "list": {
      if (result.data.length === 0) {
        return null;
      }
      return (
        <span className={variant === "cell" ? "page-list-pills" : "formula-chips"}>
          {formulaItems(result).map((item, at) => {
            if (item.type === "page") {
              return <RelatedPage key={`${String(at)}:${item.data}`} id={item.data} />;
            }
            if (item.type === "person") {
              return <PersonName key={`${String(at)}:${item.data}`} id={item.data} />;
            }
            const text = formulaScalarText(item, today);
            return (
              <span key={`${String(at)}:${text}`} className="rollup-item">
                {text}
              </span>
            );
          })}
        </span>
      );
    }
    default:
      return <FormulaText text={formulaScalarText(result, today)} variant={variant} />;
  }
}

const NO_SCHEMA: readonly PropertyDef[] = [];

export interface PageListValueProps {
  def: PropertyDef;
  value: PropertyValue;
  /** The local day, for "Today" and "Tomorrow" in dates. */
  today: string;
  /** The space's properties: a rollup reads the property it summarises from them. */
  schema?: readonly PropertyDef[];
}

/** The value of one property of one page. */
export function PageListValue({ def, value, today, schema = NO_SCHEMA }: PageListValueProps) {
  switch (value.kind) {
    case "select":
    case "status": {
      const option = def.options.find((candidate) => candidate.id === value.value);
      return option === undefined ? null : <OptionPill name={option.name} color={option.color} />;
    }
    case "multi_select": {
      const options = value.value.flatMap((id) => def.options.find((candidate) => candidate.id === id) ?? []);
      return options.length === 0 ? null : (
        <span className="page-list-pills">
          {options.map((option) => (
            <OptionPill key={option.id} name={option.name} color={option.color} />
          ))}
        </span>
      );
    }
    case "date":
      return <TextValue text={formatDateMention(value.value.date, value.value.time ?? null, today)} />;
    case "checkbox":
      return (
        <span
          className="page-list-check"
          role="img"
          aria-label={value.value ? t("pageList.checked") : t("pageList.unchecked")}
          data-checked={value.value ? "true" : "false"}
        >
          {value.value ? "✓" : ""}
        </span>
      );
    case "number":
      return <TextValue text={formatNumber(value.value)} />;
    case "text":
    case "url":
    case "email":
    case "phone":
      return <TextValue text={value.value} />;
    case "place": {
      const { name, lat, lon } = value.value;
      const where = lat !== undefined && lon !== undefined ? `${String(lat)}, ${String(lon)}` : undefined;
      return (
        <span className="page-list-text" title={where}>
          {oneLine(name).shown}
        </span>
      );
    }
    case "created_time":
    case "last_edited_time":
      return (
        <span className="page-list-text" title={new Date(value.value).toISOString()}>
          {formatInstant(value.value)}
        </span>
      );
    case "created_by":
    case "last_edited_by":
      return <DeviceName id={value.value} />;
    case "relation":
      return value.value.length === 0 ? null : (
        <span className="page-list-pills">
          {value.value.map((id) => (
            <RelatedPage key={id} id={id} />
          ))}
        </span>
      );
    case "person":
      return value.value.length === 0 ? null : (
        <span className="page-list-pills">
          {value.value.map((id) => (
            <PersonName key={id} id={id} />
          ))}
        </span>
      );
    case "rollup":
      return <RollupResultValue def={def} value={value} today={today} schema={schema} />;
    case "formula":
      return <FormulaValue result={value.value} today={today} />;
    default:
      return <TextValue text={genericText(value)} />;
  }
}
