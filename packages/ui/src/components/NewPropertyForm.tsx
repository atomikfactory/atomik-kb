import { PROPERTY_KINDS, toCoreError } from "@atomik-kb/core-client";
import type { PropertyConfig, PropertyDef, PropertyKind, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { FormEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { KIND_HINTS, KIND_LABELS } from "../property-format";
import { useAppStore } from "../store-context";
import { FormulaEditorHost } from "./FormulaEditor";
import { Modal } from "./Modal";
import { KindGlyph } from "./property-icons";
import { RollupConfigFields, defaultRollupDraft, rollupConfigOfDraft } from "./RollupConfigFields";
import type { RollupDraft } from "./RollupConfigFields";

export interface NewPropertyFormProps {
  spaceId: SpaceId;
  /** The property was created (and the store knows it). */
  onCreated: (property: PropertyDef) => void;
  onCancel: () => void;
}

/**
 * The form of a new property: a name and one of the kinds, each with a line on what it holds. The
 * kind cannot be changed afterwards, which the form says. A relation asks whether it is two-way
 * (it then also adds a property to the related pages that lists the pages linking to them); a rollup
 * asks which relation, property and function it uses; a formula opens the formula editor once it has a name, and the property
 * is created when the formula is saved (nothing is created when the editor is cancelled).
 */
export function NewPropertyForm({ spaceId, onCreated, onCancel }: NewPropertyFormProps) {
  const createProperty = useAppStore((state) => state.createProperty);
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<PropertyKind>("text");
  const [twoWay, setTwoWay] = useState(true);
  const [rollup, setRollup] = useState<RollupDraft | null>(null);
  const [writing, setWriting] = useState(false);
  const twoWayId = useId();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameId = useId();
  const errorId = useId();
  const kindsId = useId();

  useEffect(() => {
    loadPropertySchema(spaceId);
  }, [loadPropertySchema, spaceId]);

  // The rollup draft starts from the first relation of the space as soon as the schema is known.
  const draft = rollup ?? (schema === undefined ? null : defaultRollupDraft(schema));
  const rollupBlocked = kind === "rollup" && draft === null;

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (name.trim() === "") {
      setError(t("properties.error.nameEmpty"));
      return;
    }
    if (kind === "formula") {
      setError(null);
      setWriting(true);
      return;
    }
    let config: PropertyConfig | undefined;
    if (kind === "relation") {
      config = { kind: "relation", twoWay };
    } else if (kind === "rollup") {
      if (draft === null) {
        return;
      }
      config = rollupConfigOfDraft(draft);
    }
    setBusy(true);
    setError(null);
    try {
      onCreated(await createProperty(spaceId, name, kind, config));
    } catch (failure) {
      const problem = toCoreError(failure);
      setError(
        isFieldError(problem, "name")
          ? t("form.fieldError", { message: problem.message })
          : t("properties.error.create", { message: problem.message }),
      );
      setBusy(false);
    }
  }

  return (
    <form
      className="new-property"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      <label htmlFor={nameId}>{t("properties.new.name")}</label>
      <input
        id={nameId}
        className="input"
        type="text"
        value={name}
        placeholder={t("properties.new.namePlaceholder")}
        autoComplete="off"
        autoFocus
        aria-invalid={error !== null}
        aria-describedby={error !== null ? errorId : undefined}
        onChange={(event) => {
          setName(event.target.value);
          setError(null);
        }}
      />
      <fieldset className="fieldset new-property-kinds">
        <legend>{t("properties.new.kind")}</legend>
        <p className="muted" id={kindsId}>
          {t("properties.new.kindHint")}
        </p>
        {PROPERTY_KINDS.map((candidate) => (
          <label key={candidate} className="new-property-kind">
            <input
              type="radio"
              name={kindsId}
              value={candidate}
              checked={kind === candidate}
              aria-label={t(KIND_LABELS[candidate])}
              onChange={() => {
                setKind(candidate);
              }}
            />
            <span className="new-property-kind-glyph">
              <KindGlyph kind={candidate} />
            </span>
            <span className="new-property-kind-text">
              <span className="new-property-kind-name">{t(KIND_LABELS[candidate])}</span>
              <span className="muted">{t(KIND_HINTS[candidate])}</span>
            </span>
          </label>
        ))}
      </fieldset>
      {kind === "relation" && (
        <div className="new-property-config">
          <label className="checkbox-label" htmlFor={twoWayId}>
            <input
              id={twoWayId}
              type="checkbox"
              checked={twoWay}
              onChange={(event) => {
                setTwoWay(event.target.checked);
              }}
            />
            <span>{t("properties.new.twoWay")}</span>
          </label>
          <p className="muted">{t("properties.new.twoWayHint")}</p>
        </div>
      )}
      {kind === "formula" && <p className="muted new-property-config">{t("formula.new.hint")}</p>}
      {kind === "rollup" && (
        <div className="new-property-config">
          {draft === null ? (
            <p className="muted">{t("properties.rollup.noRelation")}</p>
          ) : (
            <RollupConfigFields schema={schema ?? []} draft={draft} onChange={setRollup} />
          )}
        </div>
      )}
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
      <div className="dialog-actions">
        <button type="button" className="button" disabled={busy} onClick={onCancel}>
          {t("dialog.cancel")}
        </button>
        <button type="submit" className="button button-primary" disabled={busy || rollupBlocked}>
          {kind === "formula" ? t("formula.new.next") : busy ? t("properties.new.creating") : t("properties.new.create")}
        </button>
      </div>
      {writing && (
        <FormulaEditorHost
          spaceId={spaceId}
          name={name}
          onSaved={(property) => {
            setWriting(false);
            onCreated(property);
          }}
          onClose={() => {
            setWriting(false);
          }}
        />
      )}
    </form>
  );
}

/** The new property form in a dialog of its own (the properties panel's "New property..."). */
export function NewPropertyDialog({
  spaceId,
  onCreated,
  onClose,
}: {
  spaceId: SpaceId;
  onCreated: (property: PropertyDef) => void;
  onClose: () => void;
}) {
  const titleId = useId();
  return (
    <Modal labelledBy={titleId} onDismiss={onClose} className="new-property-dialog">
      <h2 className="subtitle" id={titleId}>
        {t("properties.new.title")}
      </h2>
      <NewPropertyForm
        spaceId={spaceId}
        onCancel={onClose}
        onCreated={(property) => {
          onCreated(property);
          onClose();
        }}
      />
    </Modal>
  );
}
