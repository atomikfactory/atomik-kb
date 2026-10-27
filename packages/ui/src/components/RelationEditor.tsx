// The editor of a `relation` property (ADR-0027): the pages it links to as chips (the page's icon and
// live title; a click opens the page, Ctrl/Cmd + click or a middle click in a background tab, ADR-0034;
// a trashed page is dimmed, a page that cannot be read shows as
// "Unknown page"), a remove button on each, and "Add a page", which opens a small picker that
// searches the titles of the pages of the same space. A change is one `pagePropertySet` with the
// whole list. A two-way relation needs nothing more here: the core writes the paired property of
// the other page in the same change, and the tree refresh shows it.

import { MAX_RELATED_PAGES } from "@atomik-kb/core-client";
import type { PageId, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { useReferencedPage } from "../editor/node-views";
import type { ReferencedPage } from "../editor/node-views";
import { t } from "../i18n";
import { openGestures } from "../page-open";
import { useAppStore } from "../store-context";
import type { PageChoice } from "../store-content";
import { FloatingPanel, anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { PlusIcon } from "./icons";
import { PageIcon } from "./PageIcon";
import { ErrorLine, useCommit } from "./property-editor-parts";
import type { PropertyEditorProps } from "./property-editor-parts";
import { CloseIcon } from "./property-icons";

/** The pages of a relation value, in order. */
function pagesOf(value: PropertyEditorProps["value"]): readonly PageId[] {
  return value?.kind === "relation" ? value.value : [];
}

/** What a related page is called: its title ("Untitled" when blank), "Unknown page" when it cannot be read, "…" while it is read. */
export function relatedPageTitle(page: ReferencedPage): string {
  if (page.status === "missing") {
    return t("properties.relation.unknown");
  }
  if (page.status === "loading") {
    return "…";
  }
  return page.page.title === "" ? t("page.untitled") : page.page.title;
}

interface ChipProps {
  pageId: PageId;
  propertyName: string;
  readOnly: boolean;
  onRemove: () => void;
}

/** One linked page. */
function RelationChip({ pageId, propertyName, readOnly, onRemove }: ChipProps) {
  const page = useReferencedPage(pageId);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const found = page.status === "found" ? page.page : null;
  const title = relatedPageTitle(page);
  const trashed = found?.trashedAt !== undefined;
  return (
    <span
      className={`relation-chip${trashed ? " is-trashed" : ""}${page.status === "missing" ? " is-deleted" : ""}`}
      title={trashed ? t("properties.relation.trashed") : undefined}
    >
      <button
        type="button"
        className="relation-chip-open"
        disabled={found === null}
        aria-label={t("properties.relation.open", { title })}
        {...openGestures<HTMLButtonElement>((disposition) => {
          if (found !== null) {
            navigateTo({ pageId: found.id, spaceId: found.spaceId, disposition });
          }
        })}
      >
        <PageIcon icon={found?.icon} className="relation-chip-icon" />
        <span className="relation-chip-title">{title}</span>
      </button>
      {!readOnly && (
        <button
          type="button"
          className="option-chip-remove"
          aria-label={t("properties.relation.remove", { title, name: propertyName })}
          title={t("properties.relation.remove", { title, name: propertyName })}
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
  /** Pages that are linked already (not offered again). */
  taken: readonly PageId[];
  onPick: (pageId: PageId) => void;
  onClose: () => void;
}

/** The popover that searches the pages of the space by title. */
export function PagePickerPanel({ anchor, anchorElement, spaceId, name, taken, onPick, onClose }: PickerProps) {
  const findPages = useAppStore((state) => state.findPages);
  const [query, setQuery] = useState("");
  const [choices, setChoices] = useState<readonly PageChoice[] | null>(null);
  const [active, setActive] = useState(0);
  const ticket = useRef(0);
  const listId = useId();

  useEffect(() => {
    ticket.current += 1;
    const mine = ticket.current;
    findPages(query).then(
      (found) => {
        if (mine === ticket.current) {
          setChoices(found.filter((choice) => choice.spaceId === spaceId && !taken.includes(choice.pageId)));
          setActive(0);
        }
      },
      () => {
        if (mine === ticket.current) {
          setChoices([]);
        }
      },
    );
    // `taken` changes only when a page is picked, which closes the picker.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, findPages, spaceId]);

  const shown = choices ?? [];
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
        onPick(choice.pageId);
      }
    }
  }

  return (
    <FloatingPanel
      label={t("properties.relation.addTo", { name })}
      className="relation-picker"
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
        aria-label={t("properties.relation.search")}
        placeholder={t("properties.relation.searchPlaceholder")}
        autoComplete="off"
        spellCheck={false}
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
        }}
        onKeyDown={onKeyDown}
      />
      <ul id={listId} role="listbox" aria-label={t("properties.relation.results")} className="relation-picker-list">
        {shown.map((choice, index) => (
          <li
            key={choice.pageId}
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
              onPick(choice.pageId);
            }}
          >
            <PageIcon icon={choice.icon} className="relation-chip-icon" />
            <span>{choice.title === "" ? t("page.untitled") : choice.title}</span>
          </li>
        ))}
      </ul>
      {choices !== null && shown.length === 0 && <p className="muted relation-picker-none">{t("properties.relation.none")}</p>}
    </FloatingPanel>
  );
}

export function RelationEditor({ def, spaceId, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const { run, error } = useCommit(commit);
  const pages = pagesOf(value);
  const [picker, setPicker] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const full = pages.length >= MAX_RELATED_PAGES;

  useEffect(() => {
    if (autoFocus) {
      trigger.current?.focus();
    }
  }, [autoFocus]);

  function save(next: readonly PageId[]): Promise<boolean> {
    return run(next.length === 0 ? null : { kind: "relation", value: [...next] });
  }

  return (
    <div className="property-editor property-chips" role="group" aria-label={t("properties.relation.label", { name: def.name })}>
      {pages.map((pageId) => (
        <RelationChip
          key={pageId}
          pageId={pageId}
          propertyName={def.name}
          readOnly={readOnly}
          onRemove={() => {
            void save(pages.filter((id) => id !== pageId));
          }}
        />
      ))}
      {!readOnly && (
        <button
          ref={trigger}
          type="button"
          className={pages.length === 0 ? "property-chip is-empty" : "property-chip property-chip-add"}
          aria-haspopup="dialog"
          aria-expanded={picker !== null}
          aria-label={pages.length === 0 ? undefined : t("properties.relation.addTo", { name: def.name })}
          title={full ? t("properties.relation.limit", { max: MAX_RELATED_PAGES }) : pages.length === 0 ? undefined : t("properties.relation.addTo", { name: def.name })}
          disabled={full}
          onClick={(event) => {
            setPicker((current) => (current === null ? { anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget } : null));
          }}
        >
          {pages.length === 0 ? t("properties.relation.add") : <PlusIcon />}
        </button>
      )}
      {picker !== null && (
        <PagePickerPanel
          anchor={picker.anchor}
          anchorElement={picker.trigger}
          spaceId={spaceId}
          name={def.name}
          taken={pages}
          onPick={(pageId) => {
            setPicker(null);
            void save([...pages, pageId]);
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
