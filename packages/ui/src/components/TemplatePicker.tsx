// The compact template picker (ADR-0056, `docs/architecture/templates.md` section 6): a small menu of what a page can be made
// from, the templates of the space (the ones that live under a page say where) and the built-in starters, opened by "New
// page from template…" next to the sidebar's New page, by "New sub-page from template…" of a row's menu and by `/template`.
// It only chooses; whoever opened it makes the page.

import type { PageId } from "@atomik-kb/core-client";
import { useMemo } from "react";

import { t } from "../i18n";
import { starterEntries } from "../template-choices";
import type { TemplateChoice } from "../template-choices";
import { templateGroups } from "../template-tree";
import { useAppStore } from "../store-context";
import { FloatingPanel } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { PageIcon } from "./PageIcon";

export interface TemplatePickerProps {
  anchor: Anchor;
  /** The element that opened the picker: pressing it again closes it instead of reopening it. */
  anchorElement?: Element | null;
  /** Where focus goes when the picker closes. */
  returnFocus?: Element | null;
  /** The choice the person made (the picker then asks to be closed). */
  onPick: (choice: TemplateChoice) => void;
  onClose: () => void;
}

/** One template of the list: its page and where it lives (empty for the top level). */
interface Listed {
  id: PageId;
  icon: string | undefined;
  title: string;
  where: string;
}

export function TemplatePicker({ anchor, anchorElement, returnFocus, onPick, onClose }: TemplatePickerProps) {
  const tree = useAppStore((state) => state.tree);
  const spaceId = useAppStore((state) => state.currentSpaceId);
  const listed = useMemo(
    () =>
      spaceId === null
        ? []
        : templateGroups(tree, spaceId).flatMap((group): Listed[] =>
            group.templates.map((page) => ({
              id: page.id,
              icon: page.icon,
              title: page.title === "" ? t("page.untitled") : page.title,
              where: group.title,
            })),
          ),
    [tree, spaceId],
  );
  const starters = starterEntries();

  function choose(choice: TemplateChoice): void {
    // The choice first: a host that waits for the answer takes the close for a dismissal.
    onPick(choice);
    onClose();
  }

  return (
    <FloatingPanel
      role="menu"
      label={t("template.picker.label")}
      className="template-picker"
      anchor={anchor}
      anchorElement={anchorElement}
      returnFocus={returnFocus}
      onClose={onClose}
    >
      {listed.length > 0 && (
        <div role="group" aria-label={t("template.picker.templates")}>
          <p className="template-picker-heading" aria-hidden="true">
            {t("template.picker.templates")}
          </p>
          {listed.map((template) => (
            <button
              key={template.id}
              type="button"
              role="menuitem"
              className="menu-item template-picker-item"
              onClick={() => {
                choose({ kind: "template", templateId: template.id });
              }}
            >
              <PageIcon icon={template.icon} className="tree-icon" />
              <span className="template-picker-text">
                <span className="template-picker-title">{template.title}</span>
                {template.where !== "" && <span className="template-picker-hint muted">{template.where}</span>}
              </span>
            </button>
          ))}
        </div>
      )}
      {starters.length > 0 && (
        <div role="group" aria-label={t("template.picker.starters")}>
          <p className="template-picker-heading" aria-hidden="true">
            {t("template.picker.starters")}
          </p>
          {starters.map((starter) => (
            <button
              key={starter.id}
              type="button"
              role="menuitem"
              className="menu-item template-picker-item"
              onClick={() => {
                choose({ kind: "starter", starterId: starter.id });
              }}
            >
              <PageIcon icon={starter.icon} className="tree-icon" />
              <span className="template-picker-text">
                <span className="template-picker-title">{starter.title}</span>
                <span className="template-picker-hint muted">{starter.description}</span>
              </span>
            </button>
          ))}
        </div>
      )}
      {listed.length === 0 && starters.length === 0 && (
        <p className="muted template-picker-empty" role="status">
          {t("template.picker.empty")}
        </p>
      )}
    </FloatingPanel>
  );
}
