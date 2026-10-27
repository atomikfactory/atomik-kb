// The value editors of the properties panel, one per kind (ADR-0026). Each is always on screen as
// the control it is (no click-to-edit step) and commits what it holds through `commit` (the panel
// saves it with `pagePropertySet`): the text fields on Enter and when they lose focus, a checkbox
// and a pick of an option at once. Escape puts the field back as it was. A value that breaks a rule
// of its kind is not sent: the reason shows under the field. A refusal of the core shows there too.

import { TAGS_PROPERTY_ID, propertyValuesEqual } from "@atomik-kb/core-client";
import type { DateValue, OptionId, PropertyValue } from "@atomik-kb/core-client";
import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { formatDateMention } from "../editor/dates";
import { DatePicker } from "../editor/DatePicker";
import { useToday } from "../editor/today";
import { t } from "../i18n";
import { draftOf, formatNumber, mailtoLink, parseDraft } from "../property-format";
import { useAppStore } from "../store-context";
import { isExternalHref } from "../store-content";
import { anchorBelow } from "./FloatingPanel";
import type { Anchor, CloseReason } from "./FloatingPanel";
import { ChecklistEditor } from "./ChecklistEditor";
import { FilesEditor } from "./FilesEditor";
import { PlusIcon } from "./icons";
import { ComputedValue } from "./ComputedValue";
import { PersonEditor } from "./PersonEditor";
import { PlaceEditor } from "./PlaceEditor";
import { RelationEditor } from "./RelationEditor";
import { OptionPicker } from "./OptionPicker";
import { OptionPill } from "./OptionPill";
import { ErrorLine, useCommit } from "./property-editor-parts";
import type { PropertyEditorProps } from "./property-editor-parts";
import { CloseIcon, OpenIcon } from "./property-icons";

export type { PropertyEditorProps };

// ----- text, number, url, email, phone ----------------------------------------------------------

const PLACEHOLDERS = {
  text: "properties.placeholder.text",
  number: "properties.placeholder.number",
  url: "properties.placeholder.url",
  email: "properties.placeholder.email",
  phone: "properties.placeholder.phone",
} as const;

function InputEditor({ def, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const openExternal = useAppStore((state) => state.openExternal);
  const { run, error, clearError } = useCommit(commit);
  const kind = def.kind as keyof typeof PLACEHOLDERS;
  const stored = draftOf(kind, value);
  const [draft, setDraft] = useState(stored);
  // What the page holds changed (a save, or an edit from elsewhere): the field follows it.
  const [seen, setSeen] = useState(stored);
  const [focused, setFocused] = useState(false);
  // Enter was pressed or the field was left with a draft that cannot be saved: show why.
  const [attempted, setAttempted] = useState(false);
  const reverted = useRef(false);
  const field = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  if (seen !== stored) {
    setSeen(stored);
    setDraft(stored);
    setAttempted(false);
  }

  const parsed = parseDraft(kind, draft);
  const dirty = draft !== stored;
  const problem = dirty && !parsed.ok && (attempted || !focused) ? parsed.problem : null;
  const shown = kind === "number" && value?.kind === "number" && !focused && !dirty ? formatNumber(value.value) : draft;

  // A text field grows with what is typed (no layout in tests: a zero height is left alone).
  useEffect(() => {
    const element = field.current;
    if (element !== null && kind === "text" && element.scrollHeight > 0) {
      element.style.height = "auto";
      element.style.height = `${String(element.scrollHeight)}px`;
    }
  }, [kind, shown]);

  async function save(): Promise<void> {
    const result = parseDraft(kind, draft);
    if (!result.ok) {
      setAttempted(true);
      return;
    }
    const unchanged = result.value === null ? value === undefined : value !== undefined && propertyValuesEqual(result.value, value);
    if (unchanged) {
      setDraft(stored);
      return;
    }
    await run(result.value);
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      reverted.current = true;
      setDraft(stored);
      setAttempted(false);
      clearError();
      event.currentTarget.blur();
    } else if (event.key === "Enter" && !(kind === "text" && event.shiftKey)) {
      event.preventDefault();
      setAttempted(true);
      void save();
    }
  }

  const link = value?.kind === "url" && isExternalHref(value.value) ? value.value : value?.kind === "email" ? mailtoLink(value.value) : null;
  const linkButton =
    link === null ? null : (
      <button
        type="button"
        className="icon-button"
        aria-label={t("properties.open", { name: def.name })}
        title={t("properties.open", { name: def.name })}
        onClick={() => {
          void openExternal(link);
        }}
      >
        <OpenIcon />
      </button>
    );

  if (readOnly) {
    return (
      <div className="property-editor">
        <span className="properties-text">{value === undefined ? "" : kind === "number" && value.kind === "number" ? formatNumber(value.value) : stored}</span>
        {linkButton}
      </div>
    );
  }

  const shared = {
    value: shown,
    placeholder: t(PLACEHOLDERS[kind]),
    autoComplete: "off",
    autoFocus,
    "aria-label": def.name,
    "aria-invalid": problem !== null,
    onChange: (event: { target: { value: string } }) => {
      setDraft(event.target.value);
      setAttempted(false);
      clearError();
    },
    onFocus: () => {
      setFocused(true);
    },
    onBlur: () => {
      setFocused(false);
      if (reverted.current) {
        reverted.current = false;
        return;
      }
      void save();
    },
    onKeyDown,
  } as const;

  return (
    <div className="property-editor">
      {kind === "text" ? (
        <textarea ref={field} className="property-input property-textarea" rows={1} {...shared} />
      ) : (
        <input
          ref={field}
          className="property-input"
          type="text"
          inputMode={kind === "number" ? "decimal" : kind === "phone" ? "tel" : kind === "email" ? "email" : "text"}
          spellCheck={false}
          {...shared}
        />
      )}
      {linkButton}
      <ErrorLine message={problem ?? error} />
    </div>
  );
}

// ----- checkbox ------------------------------------------------------------------------------------

function CheckboxEditor({ def, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const { run, error } = useCommit(commit);
  const checked = value?.kind === "checkbox" && value.value;
  return (
    <div className="property-editor">
      <input
        type="checkbox"
        className="property-checkbox"
        checked={checked}
        disabled={readOnly}
        autoFocus={autoFocus}
        aria-label={def.name}
        onChange={(event) => {
          void run({ kind: "checkbox", value: event.target.checked });
        }}
      />
      <ErrorLine message={error} />
    </div>
  );
}

// ----- date ------------------------------------------------------------------------------------------

function DateEditor({ def, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const today = useToday();
  const { run, error } = useCommit(commit);
  const stored: DateValue | undefined = value?.kind === "date" ? value.value : undefined;
  const [picker, setPicker] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  // What the picker holds while it is open: applied when it closes, dropped on Escape.
  const [draft, setDraft] = useState<DateValue | undefined>(undefined);
  const draftRef = useRef<DateValue | undefined>(undefined);
  const trigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (autoFocus) {
      trigger.current?.focus();
    }
  }, [autoFocus]);

  const shown = draft ?? stored;
  const label =
    stored === undefined ? t("properties.date.pick") : formatDateMention(stored.date, stored.time ?? null, today);

  function close(reason?: CloseReason): void {
    const chosen = draftRef.current;
    draftRef.current = undefined;
    setDraft(undefined);
    setPicker(null);
    if (reason !== "escape" && chosen !== undefined && (stored?.date !== chosen.date || stored.time !== chosen.time)) {
      void run({ kind: "date", value: chosen });
    }
  }

  if (readOnly) {
    return (
      <div className="property-editor">
        <span className="properties-text">{stored === undefined ? "" : label}</span>
      </div>
    );
  }

  return (
    <div className="property-editor">
      <button
        ref={trigger}
        type="button"
        className={stored === undefined ? "property-chip is-empty" : "property-chip"}
        aria-haspopup="dialog"
        aria-expanded={picker !== null}
        aria-label={t("properties.date.change", { name: def.name, date: label })}
        onClick={(event) => {
          if (picker !== null) {
            close();
          } else {
            setPicker({ anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget });
          }
        }}
      >
        {label}
      </button>
      {stored !== undefined && (
        <button
          type="button"
          className="icon-button"
          aria-label={t("properties.date.clear", { name: def.name })}
          title={t("properties.date.clear", { name: def.name })}
          onClick={() => {
            void run(null);
          }}
        >
          <CloseIcon />
        </button>
      )}
      {picker !== null && (
        <DatePicker
          anchor={picker.anchor}
          anchorElement={picker.trigger}
          date={shown?.date ?? ""}
          time={shown?.time ?? null}
          onChange={(date, time) => {
            const next: DateValue = time === null ? { date } : { date, time };
            draftRef.current = next;
            setDraft(next);
          }}
          onClose={close}
        />
      )}
      <ErrorLine message={error} />
    </div>
  );
}

// ----- select, status, multi-select, Tags -----------------------------------------------------------

function selectedIds(value: PropertyValue | undefined): readonly OptionId[] {
  if (value?.kind === "select" || value?.kind === "status") {
    return [value.value];
  }
  return value?.kind === "multi_select" ? value.value : [];
}

function OptionsEditor({ def, spaceId, value, readOnly, autoFocus, commit, onDeleteOption }: PropertyEditorProps) {
  const openSearchWithTag = useAppStore((state) => state.openSearchWithTag);
  const { run, error } = useCommit(commit);
  const multiple = def.kind === "multi_select";
  const isTags = def.id === TAGS_PROPERTY_ID;
  const ids = selectedIds(value);
  const options = ids.flatMap((id) => def.options.filter((option) => option.id === id));
  const [picker, setPicker] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (autoFocus) {
      trigger.current?.focus();
    }
  }, [autoFocus]);

  /** The value a selection stands for. */
  function valueOf(next: readonly OptionId[]): PropertyValue | null {
    const first = next[0];
    if (first === undefined) {
      return null;
    }
    if (multiple) {
      return { kind: "multi_select", value: [...next] };
    }
    return def.kind === "status" ? { kind: "status", value: first } : { kind: "select", value: first };
  }

  function toggle(event: { currentTarget: Element }): void {
    if (picker !== null) {
      setPicker(null);
    } else {
      setPicker({ anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget });
    }
  }

  const popover =
    picker === null ? null : (
      <OptionPicker
        anchor={picker.anchor}
        anchorElement={picker.trigger}
        spaceId={spaceId}
        def={def}
        selected={ids}
        multiple={multiple}
        onChange={(next) => commit(valueOf(next))}
        onDeleteOption={onDeleteOption}
        onClose={() => {
          setPicker(null);
        }}
      />
    );

  if (!multiple) {
    const [option] = options;
    if (readOnly) {
      return (
        <div className="property-editor">{option !== undefined && <OptionPill name={option.name} color={option.color} />}</div>
      );
    }
    return (
      <div className="property-editor">
        <button
          ref={trigger}
          type="button"
          className="property-chip"
          aria-haspopup="listbox"
          aria-expanded={picker !== null}
          aria-label={t("properties.value.label", { name: def.name })}
          onClick={toggle}
        >
          {option === undefined ? (
            <span className="muted">{t("properties.empty")}</span>
          ) : (
            <OptionPill name={option.name} color={option.color} />
          )}
        </button>
        {popover}
      </div>
    );
  }

  return (
    <div className="property-editor property-chips">
      {options.map((option) => (
        <span key={option.id} className="option-pill option-chip" data-color={option.color}>
          {isTags ? (
            <button
              type="button"
              className="option-chip-label"
              aria-label={t("properties.tag.search", { name: option.name })}
              title={t("properties.tag.search", { name: option.name })}
              onClick={() => {
                openSearchWithTag({ spaceId, propertyId: def.id, optionId: option.id });
              }}
            >
              {option.name}
            </button>
          ) : (
            <span>{option.name}</span>
          )}
          {!readOnly && (
            <button
              type="button"
              className="option-chip-remove"
              aria-label={t("properties.option.remove", { name: option.name })}
              title={t("properties.option.remove", { name: option.name })}
              onClick={() => {
                void run(valueOf(ids.filter((id) => id !== option.id)));
              }}
            >
              <CloseIcon />
            </button>
          )}
        </span>
      ))}
      {!readOnly && (
        <button
          ref={trigger}
          type="button"
          className={options.length === 0 ? "property-chip is-empty" : "property-chip property-chip-add"}
          aria-haspopup="listbox"
          aria-expanded={picker !== null}
          aria-label={options.length === 0 ? undefined : t("properties.option.add", { name: def.name })}
          title={options.length === 0 ? undefined : t("properties.option.add", { name: def.name })}
          onClick={toggle}
        >
          {options.length === 0 ? isTags ? t("properties.tags.add") : t("properties.options.add") : <PlusIcon />}
        </button>
      )}
      {popover}
      <ErrorLine message={error} />
    </div>
  );
}

/** The editor for the kind of `def`. */
export function PropertyEditor(props: PropertyEditorProps) {
  switch (props.def.kind) {
    case "text":
    case "number":
    case "url":
    case "email":
    case "phone":
      return <InputEditor {...props} />;
    case "checkbox":
      return <CheckboxEditor {...props} />;
    case "date":
      return <DateEditor {...props} />;
    case "select":
    case "status":
    case "multi_select":
      return <OptionsEditor {...props} />;
    case "files":
      return <FilesEditor {...props} />;
    case "checklist":
      return <ChecklistEditor {...props} />;
    case "place":
      return <PlaceEditor {...props} />;
    case "person":
      return <PersonEditor {...props} />;
    case "relation":
      return <RelationEditor {...props} />;
    case "rollup":
    case "formula":
    case "created_time":
    case "last_edited_time":
    case "created_by":
    case "last_edited_by":
      return <ComputedValue {...props} />;
  }
}
