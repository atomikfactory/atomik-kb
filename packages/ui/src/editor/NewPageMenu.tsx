// The menu next to a layout's "New" control (ADR-0056, `docs/architecture/templates.md` section 7): a chevron button that
// opens the list of the templates of the block's source and the other ways to make a page. "New" itself is the layout's own
// control and makes a page from the view's default template (or an empty one); this menu is the extra button.
//
// - **A template** (icon, title, a "Default" mark on the view's default): its title makes a page from it, with the `init` the
//   layout gave (a board column's value, a calendar day's date). Its "..." button opens the row's actions: **Edit** (the side
//   peek), **Set as default** / **Remove as default** (writes the view's `defaultTemplate` through `onUpdateView`), **Duplicate**
//   and **Delete** (the trash).
// - **Empty page** makes a page with no template, whatever the default.
// - **New template** makes a template page under the host page and opens it in the side peek.
//
// The templates and what the actions do come from `DatabaseTemplatesContext` (provided by the block); outside one the menu
// draws nothing. The menu is a portal, so a click in it must not reach the layout around the button (a calendar day makes a
// page when it is clicked): the body of the panel stops it.

import type { PageId, PageInfo } from "@atomik-kb/core-client";
import { useState } from "react";
import type { MouseEvent } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { ChevronDownIcon, MoreIcon, PlusIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { resolveDefaultTemplate, useDatabaseTemplates } from "./database-templates";
import type { NewPageInit } from "./page-list-layout-types";
import type { PageListViewPatch, PageListViewSpec } from "./page-list-views";

export interface NewPageMenuProps {
  /** The view the layout draws: its `defaultTemplate` gets the "Default" mark. */
  readonly view: Pick<PageListViewSpec, "defaultTemplate">;
  /** The layout's `onCreatePage`: the menu always names the template (`templateId`), an empty page being `null`. */
  readonly onCreatePage: (init?: NewPageInit) => void;
  /** Changes the view (sets or removes `defaultTemplate`). */
  readonly onUpdateView: (patch: PageListViewPatch) => void;
  /** What the new page starts with, as for the layout's own "New" (a board column's value, a calendar day's date). */
  readonly init?: NewPageInit;
  /** A page is being made: the button waits. */
  readonly disabled?: boolean;
  /** The accessible name of the button (default: "New page options"). */
  readonly label?: string;
  /** The tab index of the button (a roving one in a calendar day). */
  readonly tabIndex?: number;
}

interface Opened {
  readonly anchor: Anchor;
  readonly trigger: Element;
}

const titleOf = (page: PageInfo): string => (page.title === "" ? t("page.untitled") : page.title);

/** A click inside the menu is the menu's own. */
function keep(event: MouseEvent): void {
  event.stopPropagation();
}

/** The menu next to "New". */
export function NewPageMenu({ view, onCreatePage, onUpdateView, init, disabled = false, label, tabIndex }: NewPageMenuProps) {
  const database = useDatabaseTemplates();
  const [opened, setOpened] = useState<Opened | null>(null);
  // The template whose actions are shown under its row.
  const [expanded, setExpanded] = useState<PageId | null>(null);
  if (database === null) {
    return null;
  }
  const { templates, actions } = database;
  const defaultId = resolveDefaultTemplate(view, templates);
  const name = label ?? t("databaseTemplate.menu");

  function close(): void {
    setOpened(null);
    setExpanded(null);
  }

  function create(templateId: PageId | null): void {
    close();
    onCreatePage({ ...init, templateId });
  }

  return (
    <>
      <button
        type="button"
        className="database-new-trigger"
        aria-haspopup="menu"
        aria-expanded={opened !== null}
        aria-label={name}
        title={name}
        tabIndex={tabIndex}
        disabled={disabled}
        onClick={(event) => {
          const trigger = event.currentTarget;
          if (opened === null) {
            setOpened({ anchor: anchorBelow(trigger), trigger });
          } else {
            close();
          }
        }}
      >
        <ChevronDownIcon />
      </button>
      {opened !== null && (
        <FloatingPanel
          anchor={opened.anchor}
          anchorElement={opened.trigger}
          role="menu"
          label={t("databaseTemplate.panel")}
          className="database-new-menu"
          onClose={close}
        >
          <div className="database-new-body" onClick={keep}>
            <p className="database-new-heading" aria-hidden="true">
              {t("databaseTemplate.heading")}
            </p>
            {templates.length === 0 && <p className="database-new-none">{t("databaseTemplate.none")}</p>}
            {templates.map((template) => {
              const isDefault = template.id === defaultId;
              const open = expanded === template.id;
              return (
                <div key={template.id} className="database-new-template" role="none" data-template={template.id}>
                  <div className="database-new-template-line" role="none">
                    <button
                      type="button"
                      role="menuitem"
                      className="menu-item database-new-use"
                      title={t("databaseTemplate.use", { title: titleOf(template) })}
                      onClick={() => {
                        create(template.id);
                      }}
                    >
                      <PageIcon icon={template.icon} className="database-new-icon" />
                      <span className="database-new-title">{titleOf(template)}</span>
                      {isDefault && <span className="database-new-default">{t("databaseTemplate.default")}</span>}
                    </button>
                    <button
                      type="button"
                      data-menu-nav=""
                      className="database-new-more"
                      aria-expanded={open}
                      aria-label={t("databaseTemplate.options", { title: titleOf(template) })}
                      title={t("databaseTemplate.options", { title: titleOf(template) })}
                      onClick={() => {
                        setExpanded(open ? null : template.id);
                      }}
                    >
                      <MoreIcon />
                    </button>
                  </div>
                  {open && (
                    <div className="database-new-actions" role="none">
                      <button
                        type="button"
                        role="menuitem"
                        className="menu-item"
                        onClick={() => {
                          close();
                          actions.edit(template);
                        }}
                      >
                        {t("databaseTemplate.edit")}
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        className="menu-item"
                        onClick={() => {
                          onUpdateView({ defaultTemplate: isDefault ? null : template.id });
                        }}
                      >
                        {isDefault ? t("databaseTemplate.removeDefault") : t("databaseTemplate.setDefault")}
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        className="menu-item"
                        onClick={() => {
                          close();
                          void actions.duplicate(template);
                        }}
                      >
                        {t("databaseTemplate.duplicate")}
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        className="menu-item menu-item-danger"
                        onClick={() => {
                          close();
                          void actions.remove(template);
                        }}
                      >
                        {t("databaseTemplate.delete")}
                      </button>
                    </div>
                  )}
                </div>
              );
            })}
            <hr className="database-new-separator" />
            <button
              type="button"
              role="menuitem"
              className="menu-item"
              onClick={() => {
                create(null);
              }}
            >
              {t("databaseTemplate.emptyPage")}
            </button>
            <button
              type="button"
              role="menuitem"
              className="menu-item database-new-create"
              onClick={() => {
                close();
                void actions.create();
              }}
            >
              <PlusIcon />
              <span>{t("databaseTemplate.newTemplate")}</span>
            </button>
          </div>
        </FloatingPanel>
      )}
    </>
  );
}
