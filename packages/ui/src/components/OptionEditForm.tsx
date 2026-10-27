import { OPTION_COLORS, STATUS_GROUPS, toCoreError } from "@atomik-kb/core-client";
import type { OptionColor, PropertyDef, PropertyOption, PropertyOptionPatch, SpaceId, StatusGroup } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { COLOR_LABELS, GROUP_LABELS, groupOf, optionColorOf } from "../property-format";
import { useAppStore } from "../store-context";

export interface OptionEditFormProps {
  spaceId: SpaceId;
  def: PropertyDef;
  option: PropertyOption;
  /** The form is over (saved, or cancelled). */
  onDone: () => void;
  /** Asks to delete the option; the owner confirms, because it also leaves every page that has it. */
  onDelete: () => void;
}

/**
 * Edits one option: its name, its colour (the nine of the palette) and, for a status, its group.
 * Enter saves, Escape cancels. The properties dialog shows it in a row, the option picker in place
 * of its list.
 */
export function OptionEditForm({ spaceId, def, option, onDone, onDelete }: OptionEditFormProps) {
  const updatePropertyOption = useAppStore((state) => state.updatePropertyOption);
  const [name, setName] = useState(option.name);
  const [color, setColor] = useState<OptionColor>(optionColorOf(option.color));
  const [group, setGroup] = useState<StatusGroup>(groupOf(option));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameId = useId();
  const groupId = useId();
  const errorId = useId();

  async function save(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const patch: PropertyOptionPatch = {};
    if (name.trim() !== option.name) {
      patch.name = name;
    }
    if (color !== optionColorOf(option.color)) {
      patch.color = color;
    }
    if (def.kind === "status" && group !== groupOf(option)) {
      patch.group = group;
    }
    if (Object.keys(patch).length === 0) {
      onDone();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await updatePropertyOption(spaceId, def.id, option.id, patch);
      onDone();
    } catch (failure) {
      const problem = toCoreError(failure);
      setError(
        isFieldError(problem, "name")
          ? t("form.fieldError", { message: problem.message })
          : t("properties.error.generic", { message: problem.message }),
      );
      setBusy(false);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLFormElement>): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onDone();
    }
  }

  return (
    <form
      className="option-edit"
      noValidate
      aria-label={t("properties.option.editTitle", { name: option.name })}
      onSubmit={(event) => {
        void save(event);
      }}
      onKeyDown={onKeyDown}
    >
      <label htmlFor={nameId}>{t("properties.option.name")}</label>
      <input
        id={nameId}
        className="input"
        type="text"
        value={name}
        autoComplete="off"
        autoFocus
        aria-invalid={error !== null}
        aria-describedby={error !== null ? errorId : undefined}
        onFocus={(event) => {
          event.currentTarget.select();
        }}
        onChange={(event) => {
          setName(event.target.value);
          setError(null);
        }}
      />
      <div className="option-edit-colors" role="group" aria-label={t("properties.option.color")}>
        {OPTION_COLORS.map((candidate) => (
          <button
            key={candidate}
            type="button"
            className="option-swatch-button"
            aria-label={t(COLOR_LABELS[candidate])}
            title={t(COLOR_LABELS[candidate])}
            aria-pressed={color === candidate}
            onClick={() => {
              setColor(candidate);
            }}
          >
            <span className="option-swatch" data-color={candidate} aria-hidden="true" />
          </button>
        ))}
      </div>
      {def.kind === "status" && (
        <>
          <label htmlFor={groupId}>{t("properties.option.group")}</label>
          <select
            id={groupId}
            className="input select"
            value={group}
            onChange={(event) => {
              setGroup(event.target.value as StatusGroup);
            }}
          >
            {STATUS_GROUPS.map((candidate) => (
              <option key={candidate} value={candidate}>
                {t(GROUP_LABELS[candidate])}
              </option>
            ))}
          </select>
        </>
      )}
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
      <div className="option-edit-actions">
        <button type="submit" className="button button-primary" disabled={busy}>
          {t("properties.option.save")}
        </button>
        <button type="button" className="button" disabled={busy} onClick={onDone}>
          {t("dialog.cancel")}
        </button>
        <button type="button" className="link-button link-danger" disabled={busy} onClick={onDelete}>
          {t("properties.option.delete")}
        </button>
      </div>
    </form>
  );
}
