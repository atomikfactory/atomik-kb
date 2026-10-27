// The "Pages shown" chooser of a database block (ADR-0016, ADR-0058; `docs/architecture/database-power-ups.md` section 2.3): a
// select of where the block's pages come from, in the view settings popover (`PageListToolbar.tsx`) and, for a linked view whose
// page is gone, under the "no longer available" message (`PageListView.tsx`). Its entries, in order: the sub-pages of this page,
// one entry per Tags option, the source page of a linked view (when that is the source), "Sub-pages of another page…" and every
// page of the space. The last two are the linked sources: "another page" opens a small searchable list of the space's pages
// (`PageSourcePicker`), the rest write the source at once. The source is the block's, shared by all its views.

import { TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { PageId, PropertyDef, PropertyOption, TreeView } from "@atomik-kb/core-client";
import { useEffect, useEffectEvent, useId, useMemo, useState } from "react";
import type { KeyboardEvent } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { findPage } from "../tree";
import { choiceSource, sourceChoice } from "./page-list-model";
import type { PageListSource } from "./page-list-model";
import { sourceChoices } from "./page-list-sources";
import { PageListField } from "./PageListField";

/** The select value of "Sub-pages of another page…": it opens the picker and is never a source. */
const PICK_PAGE = "pick-page";

interface PickerProps {
  anchor: Anchor;
  anchorElement: Element;
  /** The page that holds the block: it cannot be its own source page. */
  hostId: PageId | null;
  onPick: (pageId: PageId) => void;
  onClose: () => void;
}

/** The popover that searches the pages of the space by title; Enter or a click picks one. */
export function PageSourcePicker({ anchor, anchorElement, hostId, onPick, onClose }: PickerProps) {
  const tree = useAppStore((state) => state.tree);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const untitled = t("page.untitled");
  const choices = useMemo(() => sourceChoices(tree, hostId, query, untitled), [tree, hostId, query, untitled]);
  const activeIndex = Math.min(active, choices.length - 1);

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "ArrowDown" && choices.length > 0) {
      event.preventDefault();
      setActive((activeIndex + 1) % choices.length);
    } else if (event.key === "ArrowUp" && choices.length > 0) {
      event.preventDefault();
      setActive((activeIndex - 1 + choices.length) % choices.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const choice = choices[activeIndex];
      if (choice !== undefined) {
        onPick(choice.page.id);
      }
    }
  }

  return (
    <FloatingPanel
      label={t("pageList.source.pickerLabel")}
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
        aria-label={t("pageList.source.pickerSearch")}
        placeholder={t("pageList.source.pickerPlaceholder")}
        autoComplete="off"
        spellCheck={false}
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          setActive(0);
        }}
        onKeyDown={onKeyDown}
      />
      <ul id={listId} role="listbox" aria-label={t("pageList.source.pickerResults")} className="relation-picker-list">
        {choices.map(({ page, parent }, index) => (
          <li
            key={page.id}
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
              onPick(page.id);
            }}
          >
            <PageIcon icon={page.icon} className="relation-chip-icon" />
            <span>{page.title === "" ? untitled : page.title}</span>
            {parent !== null && <span className="muted page-source-parent">{parent.title === "" ? untitled : parent.title}</span>}
          </li>
        ))}
      </ul>
      {choices.length === 0 && <p className="muted relation-picker-none">{t("pageList.source.pickerNone")}</p>}
    </FloatingPanel>
  );
}

/** What the entry of a linked source says: its page's title ("…" while the tree is not loaded), or that the page is gone. */
function linkedEntry(tree: TreeView | null, pageId: PageId): string {
  if (tree === null) {
    return t("pageList.source.page", { title: "…" });
  }
  const page = findPage(tree.roots, pageId);
  if (page === null) {
    return t("pageList.source.pageGone");
  }
  return t("pageList.source.page", { title: page.title === "" ? t("page.untitled") : page.title });
}

export interface PageListSourceChooserProps {
  /** `null` for a source this version does not know (the stored one shows as a disabled placeholder). */
  source: PageListSource | null;
  /** The page that holds the block, left out of the pages the picker offers. */
  hostId: PageId | null;
  /** The space's property schema (its Tags options are the tag entries). */
  defs: readonly PropertyDef[];
  /** Writes the block's source. */
  onChange: (source: PageListSource) => void;
  /**
   * Told when the picker opens and closes. A popover that holds the chooser (the view settings) uses it to stay open while
   * a press lands in the picker, which is outside it as far as the popover can tell.
   */
  onPickerOpen?: (open: boolean) => void;
}

/** The labelled select of the sources, and the picker of "Sub-pages of another page…". */
export function PageListSourceChooser({ source, hostId, defs, onChange, onPickerOpen }: PageListSourceChooserProps) {
  const tree = useAppStore((state) => state.tree);
  const [picker, setPicker] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const open = picker !== null;
  const notifyPickerOpen = useEffectEvent((isOpen: boolean) => {
    onPickerOpen?.(isOpen);
  });
  useEffect(() => {
    if (!open) {
      return;
    }
    notifyPickerOpen(true);
    return () => {
      notifyPickerOpen(false);
    };
  }, [open]);

  const tagsDef = defs.find((def) => def.id === TAGS_PROPERTY_ID);
  const tags: readonly PropertyOption[] = tagsDef?.options ?? [];
  const goneTag =
    source?.kind === "tag" && tagsDef !== undefined && !tags.some((option) => option.id === source.optionId) ? source : null;
  // The page of a linked source, when there is one: its entry names it, or says it is gone.
  const linked = source?.kind === "page" ? source : null;
  const linkedLabel = linked === null ? "" : linkedEntry(tree, linked.pageId);

  return (
    <>
      <PageListField label={t("pageList.source.label")}>
        <select
          className="page-list-select"
          value={source === null ? "" : sourceChoice(source)}
          onChange={(event) => {
            if (event.target.value === PICK_PAGE) {
              // The select keeps showing the stored source: only a page picked in the list changes it.
              setPicker({ anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget });
              return;
            }
            const next = choiceSource(event.target.value);
            if (next !== null) {
              onChange(next);
            }
          }}
        >
          {source === null && (
            <option value="" disabled>
              {t("pageList.unsupported")}
            </option>
          )}
          <option value="children">{t("pageList.source.children")}</option>
          {tags.map((option) => (
            <option key={option.id} value={`tag:${option.id}`}>
              {t("pageList.source.tag", { name: option.name })}
            </option>
          ))}
          {goneTag !== null && <option value={sourceChoice(goneTag)}>{t("pageList.source.tagGone")}</option>}
          {linked !== null && <option value={sourceChoice(linked)}>{linkedLabel}</option>}
          <option value={PICK_PAGE}>{t("pageList.source.pickPage")}</option>
          <option value="space">{t("pageList.source.space")}</option>
        </select>
      </PageListField>
      {picker !== null && (
        <PageSourcePicker
          anchor={picker.anchor}
          anchorElement={picker.trigger}
          hostId={hostId}
          onPick={(pageId) => {
            setPicker(null);
            onChange({ kind: "page", pageId });
          }}
          onClose={() => {
            setPicker(null);
          }}
        />
      )}
    </>
  );
}
