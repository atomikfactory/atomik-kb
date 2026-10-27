import { toCoreError } from "@atomik-kb/core-client";
import type { OptionId, PropertyDef, PropertyOption, SpaceId, StatusGroup } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { GROUP_LABELS, groupedOptions, hasOptionNamed, matchingOptions, nextOptionColor } from "../property-format";
import { useAppStore } from "../store-context";
import { FloatingPanel } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { OptionEditForm } from "./OptionEditForm";
import { OptionPill } from "./OptionPill";
import { PencilIcon } from "./property-icons";

export interface OptionPickerProps {
  anchor: Anchor;
  /** The control that opened the picker: pressing it again is not "outside". */
  anchorElement: Element;
  spaceId: SpaceId;
  /** The property, as the store has it now (the picker follows its options as they change). */
  def: PropertyDef;
  /** The options the page has selected (none or one for a select or a status). */
  selected: readonly OptionId[];
  /** A multi-select: a pick toggles the option and the picker stays open. Else a pick sets it and closes. */
  multiple: boolean;
  /** Saves a new selection; rejects with a `CoreError`. */
  onChange: (selected: OptionId[]) => Promise<void>;
  /** Asks to delete an option. The owner confirms (a dialog opened from here would close the picker). */
  onDeleteOption: (option: PropertyOption) => void;
  onClose: () => void;
}

/** One line of the list, in the order the keyboard walks them. */
type Item = { kind: "option"; option: PropertyOption } | { kind: "create"; name: string };

/**
 * The popover that picks the option or options of a select, multi-select, status or Tags value:
 * the options as coloured pills (a status under its three headings), a field that filters them as
 * you type, "Create ..." for a name nobody has (it takes the next colour of the palette and is
 * picked at once) and a pencil on each option that edits its name, colour and group. Up and Down
 * move through the list, Enter picks, Escape closes.
 */
export function OptionPicker({
  anchor,
  anchorElement,
  spaceId,
  def,
  selected,
  multiple,
  onChange,
  onDeleteOption,
  onClose,
}: OptionPickerProps) {
  const addPropertyOption = useAppStore((state) => state.addPropertyOption);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [editing, setEditing] = useState<OptionId | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const listId = useId();
  const inputId = useId();

  const matches = matchingOptions(def.options, query);
  const sections: { group: StatusGroup | null; items: Item[] }[] =
    def.kind === "status"
      ? groupedOptions(matches).map((entry) => ({
          group: entry.group,
          items: entry.options.map((option): Item => ({ kind: "option", option })),
        }))
      : [{ group: null, items: matches.map((option): Item => ({ kind: "option", option })) }];
  const typed = query.trim();
  const createItem: Item | undefined =
    typed !== "" && !hasOptionNamed(def.options, typed) ? { kind: "create", name: typed } : undefined;
  const items: Item[] = sections.flatMap((section) => section.items);
  if (createItem !== undefined) {
    items.push(createItem);
  }
  const activeIndex = Math.min(active, items.length - 1);
  const idOf = (index: number): string => `${listId}-${String(index)}`;

  function describe(failure: unknown): string {
    const problem = toCoreError(failure);
    return isFieldError(problem, "name")
      ? t("form.fieldError", { message: problem.message })
      : t("properties.error.generic", { message: problem.message });
  }

  async function pick(optionId: OptionId): Promise<void> {
    if (multiple) {
      await onChange(selected.includes(optionId) ? selected.filter((id) => id !== optionId) : [...selected, optionId]);
    } else {
      await onChange([optionId]);
      onClose();
    }
  }

  async function activate(item: Item): Promise<void> {
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (item.kind === "option") {
        await pick(item.option.id);
      } else {
        const created = await addPropertyOption(
          spaceId,
          def.id,
          item.name,
          nextOptionColor(def.options),
          def.kind === "status" ? "todo" : undefined,
        );
        setQuery("");
        setActive(0);
        await pick(created.id);
      }
    } catch (failure) {
      setError(describe(failure));
    }
    setBusy(false);
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "ArrowDown" && items.length > 0) {
      event.preventDefault();
      setActive((activeIndex + 1) % items.length);
    } else if (event.key === "ArrowUp" && items.length > 0) {
      event.preventDefault();
      setActive((activeIndex - 1 + items.length) % items.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = items[activeIndex];
      if (item !== undefined) {
        void activate(item);
      }
    }
  }

  const editedOption = editing === null ? undefined : def.options.find((option) => option.id === editing);

  function renderItem(item: Item): ReactNode {
    const index = items.indexOf(item);
    if (item.kind === "create") {
      return (
        <div
          key="create"
          id={idOf(index)}
          role="option"
          aria-selected={false}
          className={index === activeIndex ? "option-row option-create is-active" : "option-row option-create"}
          onMouseEnter={() => {
            setActive(index);
          }}
          onClick={() => {
            void activate(item);
          }}
        >
          {t("properties.option.create", { name: item.name })}
        </div>
      );
    }
    const { option } = item;
    const isSelected = selected.includes(option.id);
    return (
      <div key={option.id} className={index === activeIndex ? "option-row is-active" : "option-row"}>
        <div
          id={idOf(index)}
          role="option"
          aria-selected={isSelected}
          className="option-row-pick"
          onMouseEnter={() => {
            setActive(index);
          }}
          onClick={() => {
            void activate(item);
          }}
        >
          <OptionPill name={option.name} color={option.color} />
          {isSelected && (
            <span className="option-row-check" aria-hidden="true">
              ✓
            </span>
          )}
        </div>
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
      </div>
    );
  }

  return (
    <FloatingPanel
      label={def.name}
      className="option-picker"
      anchor={anchor}
      anchorElement={anchorElement}
      onClose={onClose}
    >
      {editedOption !== undefined ? (
        <OptionEditForm
          spaceId={spaceId}
          def={def}
          option={editedOption}
          onDone={() => {
            setEditing(null);
          }}
          onDelete={() => {
            onClose();
            onDeleteOption(editedOption);
          }}
        />
      ) : (
        <>
          <input
            id={inputId}
            className="input option-picker-input"
            type="text"
            role="combobox"
            aria-label={t("properties.option.filter")}
            aria-expanded="true"
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={activeIndex >= 0 ? idOf(activeIndex) : undefined}
            placeholder={t("properties.option.filterPlaceholder")}
            autoComplete="off"
            spellCheck={false}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
              setError(null);
            }}
            onKeyDown={onKeyDown}
          />
          <div id={listId} role="listbox" aria-label={def.name} aria-multiselectable={multiple} className="option-picker-list">
            {sections.map((section) =>
              section.group === null ? (
                section.items.map(renderItem)
              ) : (
                <div key={section.group} role="group" aria-label={t(GROUP_LABELS[section.group])}>
                  <p className="option-group-title" aria-hidden="true">
                    {t(GROUP_LABELS[section.group])}
                  </p>
                  {section.items.map(renderItem)}
                </div>
              ),
            )}
            {createItem !== undefined && renderItem(createItem)}
          </div>
          {items.length === 0 && (
            <p className="option-picker-empty muted" role="status">
              {t("properties.option.none")}
            </p>
          )}
          {error !== null && (
            <p className="error-text option-picker-error" role="alert">
              {error}
            </p>
          )}
          {!multiple && selected.length > 0 && (
            <button
              type="button"
              className="menu-item"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                setError(null);
                onChange([]).then(
                  () => {
                    onClose();
                  },
                  (failure: unknown) => {
                    setError(describe(failure));
                    setBusy(false);
                  },
                );
              }}
            >
              {t("properties.option.clear")}
            </button>
          )}
        </>
      )}
    </FloatingPanel>
  );
}
