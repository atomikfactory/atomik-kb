// The settings of an existing rollup (ADR-0027), in the details of the schema dialog: the same three
// choices as the new property form, saved with `PropertyConfigSet`. A relation shows how it is
// linked instead (two-way with another property, or one-way): that cannot be changed.

import { toCoreError } from "@atomik-kb/core-client";
import type { PropertyDef, SpaceId } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";
import { pairedPropertyOf } from "../property-format";
import { useAppStore } from "../store-context";
import { RollupConfigFields, defaultRollupDraft, rollupConfigOfDraft, rollupDraftOf } from "./RollupConfigFields";
import type { RollupDraft } from "./RollupConfigFields";

export function RollupSettings({ spaceId, def }: { spaceId: SpaceId; def: PropertyDef }) {
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const setPropertyConfig = useAppStore((state) => state.setPropertyConfig);
  const stored = rollupDraftOf(def);
  const [edited, setEdited] = useState<RollupDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const draft = edited ?? stored ?? (schema === undefined ? null : defaultRollupDraft(schema));
  if (schema === undefined || draft === null) {
    return <p className="muted">{t("properties.rollup.noRelation")}</p>;
  }
  const dirty =
    edited !== null &&
    (stored === null ||
      edited.relationPropertyId !== stored.relationPropertyId ||
      edited.target !== stored.target ||
      edited.function !== stored.function);

  async function save(): Promise<void> {
    if (draft === null || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await setPropertyConfig(spaceId, def.id, rollupConfigOfDraft(draft));
      setEdited(null);
    } catch (failure) {
      setError(t("properties.error.config", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  return (
    <div className="manage-rollup">
      <h3 className="manage-options-title">{t("properties.manage.rollup")}</h3>
      {stored === null && <p className="error-text">{t("properties.rollup.broken")}</p>}
      <RollupConfigFields
        schema={schema}
        draft={draft}
        disabled={busy}
        onChange={(next) => {
          setEdited(next);
          setError(null);
        }}
      />
      <button
        type="button"
        className="button"
        disabled={busy || (!dirty && stored !== null)}
        onClick={() => {
          void save();
        }}
      >
        {busy ? t("properties.rollup.saving") : t("properties.rollup.save")}
      </button>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** How a relation is linked: with the property that shows it on the other pages, or one-way. */
export function RelationSummary({ spaceId, def }: { spaceId: SpaceId; def: PropertyDef }) {
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const paired = schema === undefined ? undefined : pairedPropertyOf(def, schema);
  return (
    <p className="muted manage-usage">
      {paired === undefined ? t("properties.manage.oneWay") : t("properties.manage.twoWay", { name: paired.name })}
    </p>
  );
}
