// The three choices that configure a rollup (ADR-0027): the relation it follows, the property of the
// related pages (or their titles) it summarises, and the function. Only the functions the target
// accepts are offered, so a configuration the core would refuse cannot be made here. Used by the
// new property form and by the settings of an existing rollup.

import { ROLLUP_TITLE_TARGET, isRollupTargetKind, rollupFunctionsFor } from "@atomik-kb/core-client";
import type { PropertyConfig, PropertyDef, PropertyKind, RollupFunction } from "@atomik-kb/core-client";
import { useId } from "react";

import { t } from "../i18n";
import { ROLLUP_FUNCTION_LABELS } from "../property-format";

/** What the form holds for a rollup. */
export interface RollupDraft {
  relationPropertyId: string;
  /** The property id, or `"title"`. */
  target: string;
  function: RollupFunction;
}

/** The relation properties of a schema (what a rollup can follow). */
export function relationsOf(schema: readonly PropertyDef[]): PropertyDef[] {
  return schema.filter((def) => def.kind === "relation");
}

/** The kind of the target: `undefined` for the titles (and for a property the schema lacks). */
function targetKind(schema: readonly PropertyDef[], target: string): PropertyKind | undefined {
  return target === ROLLUP_TITLE_TARGET ? undefined : schema.find((def) => def.id === target)?.kind;
}

/** A draft over the schema's first relation, summarising the titles by counting them; `null` without a relation. */
export function defaultRollupDraft(schema: readonly PropertyDef[]): RollupDraft | null {
  const relation = relationsOf(schema)[0];
  return relation === undefined
    ? null
    : { relationPropertyId: relation.id, target: ROLLUP_TITLE_TARGET, function: "count_all" };
}

/** The draft of an existing rollup. */
export function rollupDraftOf(def: PropertyDef): RollupDraft | null {
  return def.config?.kind === "rollup"
    ? { relationPropertyId: def.config.relationPropertyId, target: def.config.target, function: def.config.function }
    : null;
}

/** The configuration a draft stands for. */
export function rollupConfigOfDraft(draft: RollupDraft): PropertyConfig {
  return {
    kind: "rollup",
    relationPropertyId: draft.relationPropertyId,
    target: draft.target,
    function: draft.function,
  };
}

export interface RollupConfigFieldsProps {
  schema: readonly PropertyDef[];
  draft: RollupDraft;
  disabled?: boolean;
  onChange: (draft: RollupDraft) => void;
}

export function RollupConfigFields({ schema, draft, disabled, onChange }: RollupConfigFieldsProps) {
  const relationId = useId();
  const targetId = useId();
  const functionId = useId();
  const kind = targetKind(schema, draft.target);
  const functions = rollupFunctionsFor(kind);
  const targets = schema.filter((def) => isRollupTargetKind(def.kind));

  /** A new target keeps the function when it still fits, else takes the first that does. */
  function withTarget(target: string): RollupDraft {
    const accepted = rollupFunctionsFor(targetKind(schema, target));
    return { ...draft, target, function: accepted.includes(draft.function) ? draft.function : (accepted[0] ?? "count_all") };
  }

  return (
    <div className="rollup-fields">
      <label htmlFor={relationId}>{t("properties.rollup.relation")}</label>
      <select
        id={relationId}
        className="input select"
        value={draft.relationPropertyId}
        disabled={disabled}
        onChange={(event) => {
          onChange({ ...draft, relationPropertyId: event.target.value });
        }}
      >
        {relationsOf(schema).map((def) => (
          <option key={def.id} value={def.id}>
            {def.name}
          </option>
        ))}
        {schema.find((def) => def.id === draft.relationPropertyId) === undefined && (
          <option value={draft.relationPropertyId}>{draft.relationPropertyId}</option>
        )}
      </select>
      <label htmlFor={targetId}>{t("properties.rollup.target")}</label>
      <select
        id={targetId}
        className="input select"
        value={draft.target}
        disabled={disabled}
        onChange={(event) => {
          onChange(withTarget(event.target.value));
        }}
      >
        <option value={ROLLUP_TITLE_TARGET}>{t("properties.rollup.targetTitle")}</option>
        {draft.target !== ROLLUP_TITLE_TARGET && schema.find((def) => def.id === draft.target) === undefined && (
          <option value={draft.target}>{draft.target}</option>
        )}
        {targets.map((def) => (
          <option key={def.id} value={def.id}>
            {def.name}
          </option>
        ))}
      </select>
      <label htmlFor={functionId}>{t("properties.rollup.function")}</label>
      <select
        id={functionId}
        className="input select"
        value={draft.function}
        disabled={disabled}
        onChange={(event) => {
          onChange({ ...draft, function: event.target.value as RollupFunction });
        }}
      >
        {functions.map((fn) => (
          <option key={fn} value={fn}>
            {t(ROLLUP_FUNCTION_LABELS[fn])}
          </option>
        ))}
      </select>
    </div>
  );
}
