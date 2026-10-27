import { STATUS_GROUPS, TAGS_PROPERTY_ID, propertyKindHasOptions, toCoreError } from "@atomik-kb/core-client";
import type { OptionId, PropertyDef, PropertyId, PropertyOption, SpaceId, StatusGroup } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";

import { isFieldError } from "../errors";
import { t, tp } from "../i18n";
import { GROUP_LABELS, KIND_LABELS, groupedOptions, nextOptionColor } from "../property-format";
import { useAppStore } from "../store-context";
import { FormulaSettings } from "./FormulaSettings";
import { ChevronDownIcon, ChevronIcon, PlusIcon, TrashIcon } from "./icons";
import { InlineEdit } from "./InlineEdit";
import { Modal } from "./Modal";
import { NewPropertyForm } from "./NewPropertyForm";
import { OptionEditForm } from "./OptionEditForm";
import { OptionPill } from "./OptionPill";
import { DeleteOptionConfirm, DeletePropertyConfirm, useCount } from "./PropertyConfirms";
import { RelationSummary, RollupSettings } from "./RollupSettings";
import { ArrowDownIcon, ArrowUpIcon, KindGlyph, PencilIcon, TagGlyph } from "./property-icons";

export interface PropertiesDialogProps {
  spaceId: SpaceId;
  /** The property whose details (usage and options) are open when the dialog appears. */
  initialPropertyId?: PropertyId;
  /** Where focus goes when the dialog closes (the control that opened it). */
  returnFocus?: Element | null;
  onClose: () => void;
}

type Question = { kind: "property"; def: PropertyDef } | { kind: "option"; def: PropertyDef; option: PropertyOption };

/** How many pages use a property, asked of the core when its details open. */
function Usage({ spaceId, def }: { spaceId: SpaceId; def: PropertyDef }) {
  const countPagesWithProperty = useAppStore((state) => state.countPagesWithProperty);
  const count = useCount(() => countPagesWithProperty(spaceId, def.id));
  if (count === null) {
    return <p className="muted manage-usage">{t("properties.manage.usageLoading")}</p>;
  }
  const text = count === 0 ? t("properties.manage.usage.none") : tp("properties.manage.usage", count);
  return <p className="muted manage-usage">{text}</p>;
}

interface OptionsManagerProps {
  spaceId: SpaceId;
  def: PropertyDef;
  onDeleteOption: (option: PropertyOption) => void;
}

/** The options of one property: edit, delete, and a field to add one (a status chooses the group). */
function OptionsManager({ spaceId, def, onDeleteOption }: OptionsManagerProps) {
  const addPropertyOption = useAppStore((state) => state.addPropertyOption);
  const [editing, setEditing] = useState<OptionId | null>(null);
  const [name, setName] = useState("");
  const [group, setGroup] = useState<StatusGroup>("todo");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameId = useId();
  const groupId = useId();
  const errorId = useId();

  async function add(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (name.trim() === "") {
      setError(t("properties.error.optionEmpty"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await addPropertyOption(spaceId, def.id, name, nextOptionColor(def.options), def.kind === "status" ? group : undefined);
      setName("");
    } catch (failure) {
      const problem = toCoreError(failure);
      setError(
        isFieldError(problem, "name")
          ? t("form.fieldError", { message: problem.message })
          : t("properties.error.generic", { message: problem.message }),
      );
    }
    setBusy(false);
  }

  function renderOption(option: PropertyOption): ReactNode {
    return (
      <li key={option.id} className="manage-option">
        {editing === option.id ? (
          <OptionEditForm
            spaceId={spaceId}
            def={def}
            option={option}
            onDone={() => {
              setEditing(null);
            }}
            onDelete={() => {
              setEditing(null);
              onDeleteOption(option);
            }}
          />
        ) : (
          <>
            <OptionPill name={option.name} color={option.color} />
            <button
              type="button"
              className="icon-button"
              aria-label={t("properties.option.edit", { name: option.name })}
              title={t("properties.option.edit", { name: option.name })}
              onClick={() => {
                setEditing(option.id);
              }}
            >
              <PencilIcon />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label={t("properties.option.deleteLabel", { name: option.name })}
              title={t("properties.option.deleteLabel", { name: option.name })}
              onClick={() => {
                onDeleteOption(option);
              }}
            >
              <TrashIcon />
            </button>
          </>
        )}
      </li>
    );
  }

  return (
    <div className="manage-options">
      <h3 className="manage-options-title">{t(def.id === TAGS_PROPERTY_ID ? "properties.manage.tags" : "properties.manage.options")}</h3>
      {def.options.length === 0 ? (
        <p className="muted">{t("properties.option.none")}</p>
      ) : def.kind === "status" ? (
        groupedOptions(def.options).map((entry) => (
          <div key={entry.group} role="group" aria-label={t(GROUP_LABELS[entry.group])}>
            <p className="option-group-title" aria-hidden="true">
              {t(GROUP_LABELS[entry.group])}
            </p>
            <ul className="manage-option-list">{entry.options.map(renderOption)}</ul>
          </div>
        ))
      ) : (
        <ul className="manage-option-list">{def.options.map(renderOption)}</ul>
      )}
      <form
        className="manage-option-add"
        noValidate
        aria-label={t("properties.manage.addOption", { name: def.name })}
        onSubmit={(event) => {
          void add(event);
        }}
      >
        <label className="visually-hidden" htmlFor={nameId}>
          {t("properties.manage.newOption")}
        </label>
        <input
          id={nameId}
          className="input"
          type="text"
          value={name}
          placeholder={t("properties.manage.newOptionPlaceholder")}
          autoComplete="off"
          aria-invalid={error !== null}
          aria-describedby={error !== null ? errorId : undefined}
          onChange={(event) => {
            setName(event.target.value);
            setError(null);
          }}
        />
        {def.kind === "status" && (
          <>
            <label className="visually-hidden" htmlFor={groupId}>
              {t("properties.option.group")}
            </label>
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
        <button type="submit" className="button" disabled={busy}>
          {t("properties.manage.addOptionButton")}
        </button>
      </form>
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

interface ManageRowProps {
  spaceId: SpaceId;
  def: PropertyDef;
  index: number;
  count: number;
  expanded: boolean;
  onToggle: () => void;
  onQuestion: (question: Question) => void;
  onError: (message: string) => void;
}

/** One property of the schema: rename, reorder, delete, and (opened) its usage and options. */
function ManageRow({ spaceId, def, index, count, expanded, onToggle, onQuestion, onError }: ManageRowProps) {
  const renameProperty = useAppStore((state) => state.renameProperty);
  const moveProperty = useAppStore((state) => state.moveProperty);
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const [renaming, setRenaming] = useState(false);
  const detailsId = useId();
  const isTags = def.id === TAGS_PROPERTY_ID;

  function move(before: PropertyId | undefined): void {
    moveProperty(spaceId, def.id, before).catch((failure: unknown) => {
      onError(t("properties.error.move", { message: toCoreError(failure).message }));
    });
  }

  return (
    <li className="manage-row">
      <div className="manage-row-head">
        <button
          type="button"
          className="icon-button manage-toggle"
          aria-expanded={expanded}
          aria-controls={expanded ? detailsId : undefined}
          aria-label={t("properties.manage.details", { name: def.name })}
          title={t("properties.manage.details", { name: def.name })}
          onClick={onToggle}
        >
          {expanded ? <ChevronDownIcon /> : <ChevronIcon />}
        </button>
        <span className="property-glyph">{isTags ? <TagGlyph /> : <KindGlyph kind={def.kind} />}</span>
        {renaming ? (
          <InlineEdit
            className="property-rename"
            value={def.name}
            label={t("properties.rename.label")}
            onCommit={async (name) => {
              try {
                await renameProperty(spaceId, def.id, name);
              } catch (failure) {
                onError(t("properties.error.rename", { message: toCoreError(failure).message }));
              }
            }}
            onDone={() => {
              setRenaming(false);
            }}
          />
        ) : (
          <button
            type="button"
            className="link-button manage-name"
            aria-label={t("properties.manage.rename", { name: def.name })}
            title={t("properties.manage.rename", { name: def.name })}
            onClick={() => {
              setRenaming(true);
            }}
          >
            {def.name}
          </button>
        )}
        <span className="muted manage-kind">{t(KIND_LABELS[def.kind])}</span>
        <div className="manage-actions">
          <button
            type="button"
            className="icon-button"
            aria-label={t("properties.manage.up", { name: def.name })}
            title={t("properties.manage.up", { name: def.name })}
            disabled={index <= 1}
            onClick={() => {
              move(schema?.[index - 1]?.id);
            }}
          >
            <ArrowUpIcon />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label={t("properties.manage.down", { name: def.name })}
            title={t("properties.manage.down", { name: def.name })}
            disabled={isTags || index >= count - 1}
            onClick={() => {
              move(schema?.[index + 2]?.id);
            }}
          >
            <ArrowDownIcon />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label={t("properties.manage.delete", { name: def.name })}
            title={isTags ? t("properties.menu.deleteTags") : t("properties.manage.delete", { name: def.name })}
            disabled={isTags}
            onClick={() => {
              onQuestion({ kind: "property", def });
            }}
          >
            <TrashIcon />
          </button>
        </div>
      </div>
      {expanded && (
        <div className="manage-details" id={detailsId}>
          <Usage spaceId={spaceId} def={def} />
          {def.kind === "relation" && <RelationSummary spaceId={spaceId} def={def} />}
          {def.kind === "rollup" && <RollupSettings spaceId={spaceId} def={def} />}
          {def.kind === "formula" && <FormulaSettings spaceId={spaceId} def={def} />}
          {propertyKindHasOptions(def.kind) && (
            <OptionsManager
              spaceId={spaceId}
              def={def}
              onDeleteOption={(option) => {
                onQuestion({ kind: "option", def, option });
              }}
            />
          )}
        </div>
      )}
    </li>
  );
}

/**
 * The schema of a space (the space menu's "Properties..."): every property with its kind, renamed
 * in place, moved up and down, deleted after a question, and opened for the number of pages that use
 * it and the options of a select, multi-select or status (rename, colour, group, delete, add).
 * Tags is listed first; it can be renamed and given options, not moved or deleted.
 */
export function PropertiesDialog({ spaceId, initialPropertyId, returnFocus, onClose }: PropertiesDialogProps) {
  const titleId = useId();
  const space = useAppStore((state) => state.spaces.find((entry) => entry.id === spaceId));
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  const [expandedId, setExpandedId] = useState<PropertyId | null>(initialPropertyId ?? null);
  const [creating, setCreating] = useState(false);
  const [question, setQuestion] = useState<Question | null>(null);
  const [error, setError] = useState<string | null>(null);
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);

  useEffect(() => {
    loadPropertySchema(spaceId);
  }, [loadPropertySchema, spaceId]);

  return (
    <>
      <Modal labelledBy={titleId} onDismiss={onClose} className="properties-dialog" returnFocus={returnRef}>
        <h2 className="subtitle" id={titleId}>
          {t("properties.dialog.title")}
        </h2>
        <p className="muted">{t("properties.dialog.intro", { space: space?.name ?? "" })}</p>
        {schema === undefined ? (
          <p className="muted" role="status">
            {t("properties.dialog.loading")}
          </p>
        ) : (
          <ul className="manage-list" aria-label={t("properties.dialog.list")}>
            {schema.map((def, index) => (
              <ManageRow
                key={def.id}
                spaceId={spaceId}
                def={def}
                index={index}
                count={schema.length}
                expanded={expandedId === def.id}
                onToggle={() => {
                  setExpandedId((current) => (current === def.id ? null : def.id));
                }}
                onQuestion={(next) => {
                  setError(null);
                  setQuestion(next);
                }}
                onError={setError}
              />
            ))}
          </ul>
        )}
        {error !== null && (
          <p className="error-text" role="alert">
            {error}
          </p>
        )}
        {creating ? (
          <NewPropertyForm
            spaceId={spaceId}
            onCancel={() => {
              setCreating(false);
            }}
            onCreated={(property) => {
              setCreating(false);
              setExpandedId(property.id);
            }}
          />
        ) : (
          <button
            type="button"
            className="button manage-new"
            onClick={() => {
              setCreating(true);
            }}
          >
            <PlusIcon />
            <span>{t("properties.new.button")}</span>
          </button>
        )}
        <div className="dialog-actions">
          <button type="button" className="button" onClick={onClose}>
            {t("dialog.close")}
          </button>
        </div>
      </Modal>
      {question?.kind === "property" && (
        <DeletePropertyConfirm
          spaceId={spaceId}
          def={question.def}
          onError={setError}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
      {question?.kind === "option" && (
        <DeleteOptionConfirm
          spaceId={spaceId}
          def={question.def}
          option={question.option}
          onError={setError}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
    </>
  );
}
