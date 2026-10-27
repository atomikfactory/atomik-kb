// The settings of an existing formula (ADR-0055), in the details of the schema dialog: the expression as it is written
// with property names, the type of its result (or that it no longer checks), and "Edit formula…", which opens the
// formula editor. The editing itself is `FormulaEditor.tsx`'s.

import { formulaToDisplay } from "@atomik-kb/core-client";
import type { PropertyDef, SpaceId } from "@atomik-kb/core-client";
import { useMemo, useState } from "react";

import { formulaExpressionOf, formulaTypeOf } from "../formula-format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { FormulaEditorHost } from "./FormulaEditor";
import { formulaSchemaOf, formulaTypeLabel } from "./formula-editor-model";

export function FormulaSettings({ spaceId, def }: { spaceId: SpaceId; def: PropertyDef }) {
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  // The editor is open while this holds the button that opened it (where focus returns).
  const [editing, setEditing] = useState<Element | null>(null);
  const stored = formulaExpressionOf(def);
  const display = useMemo(
    () => (schema === undefined || stored === undefined ? "" : formulaToDisplay(stored, formulaSchemaOf(schema), t("pageList.sort.title"))),
    [schema, stored],
  );
  const type = formulaTypeOf(def);
  return (
    <div className="manage-formula">
      <h3 className="manage-options-title">{t("formula.manage.title")}</h3>
      {display !== "" && (
        <pre className="formula-readout">
          <code>{display}</code>
        </pre>
      )}
      {type === undefined ? (
        <p className="error-text">{t("formula.broken")}</p>
      ) : (
        <p className="muted manage-usage">{t("formula.result", { type: formulaTypeLabel(type) })}</p>
      )}
      <button
        type="button"
        className="button"
        onClick={(event) => {
          setEditing(event.currentTarget);
        }}
      >
        {t("formula.menu.edit")}
      </button>
      {editing !== null && (
        <FormulaEditorHost
          spaceId={spaceId}
          def={def}
          returnFocus={editing}
          onSaved={() => {
            setEditing(null);
          }}
          onClose={() => {
            setEditing(null);
          }}
        />
      )}
    </div>
  );
}
