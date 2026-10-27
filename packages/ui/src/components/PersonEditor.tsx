// The editor of a `person` property (ADR-0049): the people it lists as chips (the member's current name, "Former member" for an
// account the space no longer lists; a chip of this account is marked), a remove button on each, and "Add a person", which opens a
// small picker over the members of the space (`PeopleMentionable`; a personal space offers this account alone). A change is one
// `pagePropertySet` with the whole list; the core refuses a person who is not a member and more than 20 people.

import { MAX_PEOPLE } from "@atomik-kb/core-client";
import type { AccountId, PersonInfo, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { initialOf } from "../comments";
import { usePeople, usePersonNames } from "../comments-hooks";
import { personChoices } from "../editor/mention-menu";
import { t } from "../i18n";
import { FloatingPanel, anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { PlusIcon } from "./icons";
import { ErrorLine, useCommit } from "./property-editor-parts";
import type { PropertyEditorProps } from "./property-editor-parts";
import { CloseIcon } from "./property-icons";

/** The accounts of a person value, in order. */
function accountsOf(value: PropertyEditorProps["value"]): readonly AccountId[] {
  return value?.kind === "person" ? value.value : [];
}

/** One person of the value. */
export function PersonChip({
  accountId,
  spaceId,
  people,
  propertyName,
  readOnly,
  onRemove,
}: {
  accountId: AccountId;
  spaceId: SpaceId;
  people: readonly PersonInfo[] | undefined;
  propertyName: string;
  readOnly: boolean;
  onRemove: () => void;
}) {
  const known = people?.find((person) => person.accountId === accountId);
  const former = people !== undefined && known === undefined;
  const name = usePersonNames(spaceId, [accountId])(accountId);
  return (
    <span
      className={`person-chip${former ? " is-former" : ""}${known?.isMe === true ? " is-me" : ""}`}
      title={former ? t("comments.formerMember.title") : undefined}
    >
      <span className="person-mark" aria-hidden="true">
        {initialOf(name === "" ? "?" : name)}
      </span>
      <span className="person-chip-name">{name}</span>
      {!readOnly && (
        <button
          type="button"
          className="option-chip-remove"
          aria-label={t("properties.person.remove", { person: name, name: propertyName })}
          title={t("properties.person.remove", { person: name, name: propertyName })}
          onClick={onRemove}
        >
          <CloseIcon />
        </button>
      )}
    </span>
  );
}

interface PickerProps {
  anchor: Anchor;
  anchorElement: Element;
  spaceId: SpaceId;
  name: string;
  /** People that are listed already (not offered again). */
  taken: readonly AccountId[];
  onPick: (accountId: AccountId) => void;
  onClose: () => void;
}

/** The popover that lists the members of the space, narrowed by what is typed. */
export function PersonPickerPanel({ anchor, anchorElement, spaceId, name, taken, onPick, onClose }: PickerProps) {
  const people = usePeople(spaceId);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const shown = personChoices((people ?? []).filter((person) => !taken.includes(person.accountId)), query)
    .flatMap((choice) => (choice.kind === "person" ? [choice.person] : []));
  const activeIndex = Math.min(active, shown.length - 1);

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "ArrowDown" && shown.length > 0) {
      event.preventDefault();
      setActive((activeIndex + 1) % shown.length);
    } else if (event.key === "ArrowUp" && shown.length > 0) {
      event.preventDefault();
      setActive((activeIndex - 1 + shown.length) % shown.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const choice = shown[activeIndex];
      if (choice !== undefined) {
        onPick(choice.accountId);
      }
    }
  }

  return (
    <FloatingPanel
      label={t("properties.person.addTo", { name })}
      className="relation-picker person-picker"
      anchor={anchor}
      anchorElement={anchorElement}
      onClose={onClose}
      initialFocus={(panel) => panel.querySelector("input")}
    >
      <input
        className="input"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls={listId}
        aria-activedescendant={activeIndex >= 0 ? `${listId}-${String(activeIndex)}` : undefined}
        aria-label={t("properties.person.search")}
        placeholder={t("properties.person.searchPlaceholder")}
        autoComplete="off"
        spellCheck={false}
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
      />
      <ul id={listId} role="listbox" aria-label={t("properties.person.results")} className="relation-picker-list">
        {shown.map((person, index) => (
          <li
            key={person.accountId}
            id={`${listId}-${String(index)}`}
            role="option"
            aria-selected={index === activeIndex}
            className={index === activeIndex ? "menu-item is-active" : "menu-item"}
            onMouseEnter={() => {
              setActive(index);
            }}
            onMouseDown={(event) => {
              event.preventDefault();
            }}
            onClick={() => {
              onPick(person.accountId);
            }}
          >
            <span className="person-mark" aria-hidden="true">
              {initialOf(person.name)}
            </span>
            <span>{person.name}</span>
            {person.isMe && <span className="muted">{t("mention.person.you")}</span>}
          </li>
        ))}
      </ul>
      {people !== undefined && shown.length === 0 && <p className="muted relation-picker-none">{t("properties.person.none")}</p>}
    </FloatingPanel>
  );
}

export function PersonEditor({ def, spaceId, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const { run, error } = useCommit(commit);
  const people = usePeople(spaceId);
  const accounts = accountsOf(value);
  const [picker, setPicker] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const full = accounts.length >= MAX_PEOPLE;

  useEffect(() => {
    if (autoFocus) {
      trigger.current?.focus();
    }
  }, [autoFocus]);

  function save(next: readonly AccountId[]): Promise<boolean> {
    return run(next.length === 0 ? null : { kind: "person", value: [...next] });
  }

  return (
    <div className="property-editor property-chips" role="group" aria-label={t("properties.person.label", { name: def.name })}>
      {accounts.map((accountId) => (
        <PersonChip
          key={accountId}
          accountId={accountId}
          spaceId={spaceId}
          people={people}
          propertyName={def.name}
          readOnly={readOnly}
          onRemove={() => {
            void save(accounts.filter((id) => id !== accountId));
          }}
        />
      ))}
      {readOnly && accounts.length === 0 && <span className="properties-text muted">{t("properties.empty")}</span>}
      {!readOnly && (
        <button
          ref={trigger}
          type="button"
          className={accounts.length === 0 ? "property-chip is-empty" : "property-chip property-chip-add"}
          aria-haspopup="dialog"
          aria-expanded={picker !== null}
          aria-label={accounts.length === 0 ? undefined : t("properties.person.addTo", { name: def.name })}
          title={full ? t("properties.person.limit", { max: MAX_PEOPLE }) : accounts.length === 0 ? undefined : t("properties.person.addTo", { name: def.name })}
          disabled={full}
          onClick={(event) => {
            // The element now: the event's own is gone when the updater runs.
            const opener = event.currentTarget;
            setPicker((current) => (current === null ? { anchor: anchorBelow(opener), trigger: opener } : null));
          }}
        >
          {accounts.length === 0 ? t("properties.person.add") : <PlusIcon />}
        </button>
      )}
      {picker !== null && (
        <PersonPickerPanel
          anchor={picker.anchor}
          anchorElement={picker.trigger}
          spaceId={spaceId}
          name={def.name}
          taken={accounts}
          onPick={(accountId) => {
            setPicker(null);
            void save([...accounts, accountId]);
          }}
          onClose={() => {
            setPicker(null);
          }}
        />
      )}
      <ErrorLine message={error} />
    </div>
  );
}
