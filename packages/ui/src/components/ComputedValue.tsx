// The value of a computed property: when the page was created or last edited and the device that did
// it (ADR-0026, amendment of 2026-10-08), or the result of a rollup over the related pages
// (ADR-0027). Read only: the app fills it in, and the core refuses a write. A device is shown by the
// name the device log gives it; "Unknown device" stands for one the log does not list or that was
// revoked, and "Unknown" for a page that did not record it. A rollup shows its number (a percent with
// its sign), its day, its progress or, for "show original", the values of the related pages; one
// without a result is empty, and one whose setup broke says so. A formula (ADR-0055) shows its result the way a view's
// cell does (`FormulaValue`); one without a result is empty, and one that no longer checks says so.

import { useEffect } from "react";

import { formatDateMention } from "../editor/dates";
import { FormulaValue } from "../editor/PageListValue";
import { useToday } from "../editor/today";
import { formulaResultOf, formulaTypeOf } from "../formula-format";
import { t } from "../i18n";
import { deviceLabel, formatInstant, rollupConfigOf, rollupTexts } from "../property-format";
import { useAppStore } from "../store-context";
import type { PropertyEditorProps } from "./property-editor-parts";

/** A rollup: its result as text, or why there is none. */
function RollupResult({ def, spaceId, value }: PropertyEditorProps) {
  const today = useToday();
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  useEffect(() => {
    loadPropertySchema(spaceId);
  }, [loadPropertySchema, spaceId]);
  const config = rollupConfigOf(def);
  const target = config === undefined ? undefined : schema?.find((candidate) => candidate.id === config.target);
  if (value?.kind !== "rollup") {
    // No result: an average of nothing, or a setup that is not valid (a property it used is gone).
    const broken =
      config === undefined ||
      (schema !== undefined &&
        (schema.find((candidate) => candidate.id === config.relationPropertyId) === undefined ||
          (config.target !== "title" && target === undefined)));
    return (
      <div className="property-editor">
        <span className="properties-text muted" title={t("properties.computed.title")}>
          {broken ? t("properties.rollup.broken") : t("properties.empty")}
        </span>
      </div>
    );
  }
  const texts = rollupTexts(value.value.function, value.value.result, target, (date, time) =>
    formatDateMention(date, time, today),
  );
  const list = value.value.result.type === "values";
  return (
    <div className={list ? "property-editor property-chips" : "property-editor"}>
      {list ? (
        texts.map((text, at) => (
          <span key={`${String(at)}:${text}`} className="rollup-item">
            {text}
          </span>
        ))
      ) : (
        <span
          className="properties-text"
          title={t("properties.computed.title")}
          aria-label={t("properties.rollup.label", { name: def.name, result: texts.join(", ") })}
        >
          {texts.join(", ")}
        </span>
      )}
    </div>
  );
}

/** A formula: its result, or nothing (an empty result), or why there is none (it does not check). */
function FormulaResultRow({ def, value }: PropertyEditorProps) {
  const today = useToday();
  const result = formulaResultOf(value);
  if (result === undefined) {
    return (
      <div className="property-editor">
        <span className="properties-text muted" title={t("properties.computed.title")}>
          {formulaTypeOf(def) === undefined ? t("formula.broken") : t("properties.empty")}
        </span>
      </div>
    );
  }
  return (
    <div className="property-editor formula-value" title={t("properties.computed.title")}>
      <FormulaValue result={result} today={today} variant="panel" />
    </div>
  );
}

export function ComputedValue(props: PropertyEditorProps) {
  const { def, value } = props;
  const devices = useAppStore((state) => state.devices);
  const device = useAppStore((state) => state.device);
  if (def.kind === "rollup") {
    return <RollupResult {...props} />;
  }
  if (def.kind === "formula") {
    return <FormulaResultRow {...props} />;
  }
  const own = device === null ? null : { deviceId: device.deviceId, name: device.name };
  let text: string | null = null;
  let instant: string | undefined;
  switch (value?.kind) {
    case "created_time":
    case "last_edited_time":
      text = formatInstant(value.value);
      instant = new Date(value.value).toISOString();
      break;
    case "created_by":
    case "last_edited_by":
      text = deviceLabel(value.value, devices, own);
      break;
    default:
      break;
  }
  return (
    <div className="property-editor">
      {text === null ? (
        <span className="properties-text muted" title={t("properties.computed.title")}>
          {t("properties.computed.unknown")}
        </span>
      ) : (
        <span className="properties-text" title={instant ?? t("properties.computed.title")} aria-label={`${def.name}: ${text}`}>
          {text}
        </span>
      )}
    </div>
  );
}
