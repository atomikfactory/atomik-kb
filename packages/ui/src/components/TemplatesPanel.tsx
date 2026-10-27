// The Templates panel (ADR-0056, `docs/architecture/templates.md` section 6): an entry at the bottom of the sidebar, next to the
// trash, that opens a dialog listing the templates of the space grouped by where they live (the top level is "Page templates",
// the others sit under the title of their parent, such as a database's host page) and the built-in starters. A template can be
// used, edited (opened), duplicated and deleted (to the trash); a starter can be used or added to the space's templates; "New
// template" makes a top-level template page and opens it. A role that cannot edit the space only sees its templates and opens
// one (the starters, which are only there to make pages, are left out). Templates are hidden from the sidebar's tree, so this panel is where a top-level template is found.

import { toCoreError } from "@atomik-kb/core-client";
import type { PageInfo } from "@atomik-kb/core-client";
import { useId, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { t } from "../i18n";
import { useCurrentAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { starterEntries } from "../template-choices";
import type { StarterEntry } from "../template-choices";
import { templateGroups } from "../template-tree";
import type { TemplateGroup } from "../template-tree";
import { Modal } from "./Modal";
import { PageIcon } from "./PageIcon";
import { TemplateIcon } from "./template-icons";

function titleOf(page: PageInfo): string {
  return page.title === "" ? t("page.untitled") : page.title;
}

interface ItemProps {
  title: string;
  icon: string | undefined;
  /** A second line (a starter's description). */
  description?: string;
  untitled?: boolean;
  busy: boolean;
  children: (describedBy: string) => ReactNode;
}

/** One line of the panel: the icon, the title and the buttons that act on it. */
function Item({ title, icon, description, untitled = false, busy, children }: ItemProps) {
  const titleId = useId();
  return (
    <li className="template-item" aria-busy={busy}>
      <div className="template-item-info">
        <PageIcon icon={icon} className="tree-icon" />
        <span className="template-item-text">
          <span className={untitled ? "template-item-title is-untitled" : "template-item-title"} id={titleId}>
            {title}
          </span>
          {description !== undefined && <span className="template-item-description muted">{description}</span>}
        </span>
      </div>
      <div className="template-item-actions">{children(titleId)}</div>
    </li>
  );
}

function groupTitle(group: TemplateGroup): string {
  return group.parentId === null ? t("template.panel.groupTop") : group.title === "" ? t("page.untitled") : group.title;
}

/** The dialog itself. */
function TemplatesDialog({ onClose }: { onClose: () => void }) {
  const titleId = useId();
  const tree = useAppStore((state) => state.tree);
  const spaceId = useAppStore((state) => state.currentSpaceId);
  const canEdit = useCurrentAccess().canEdit;
  const navigateTo = useAppStore((state) => state.navigateTo);
  const createPage = useAppStore((state) => state.createPage);
  const setPageTemplate = useAppStore((state) => state.setPageTemplate);
  const focusPageTitle = useAppStore((state) => state.focusPageTitle);
  const createFromTemplate = useAppStore((state) => state.createFromTemplate);
  const createFromStarter = useAppStore((state) => state.createFromStarter);
  const addStarterAsTemplate = useAppStore((state) => state.addStarterAsTemplate);
  const duplicatePage = useAppStore((state) => state.duplicatePage);
  const trashPage = useAppStore((state) => state.trashPage);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const groups = useMemo(() => (spaceId === null ? [] : templateGroups(tree, spaceId)), [tree, spaceId]);
  const starters = starterEntries();

  /** Runs one action of the panel; a refusal shows in the panel, and `closes` ends the panel once the action worked. */
  async function act(key: string, action: () => Promise<unknown>, closes: boolean): Promise<void> {
    setError(null);
    setBusy(key);
    try {
      await action();
      if (closes) {
        onClose();
        return;
      }
    } catch (failure) {
      setError(t("template.error.panel", { message: toCoreError(failure).message }));
    }
    setBusy(null);
  }

  function edit(page: PageInfo): void {
    navigateTo({ pageId: page.id, spaceId: page.spaceId });
    onClose();
  }

  function newTemplate(): void {
    void act(
      "new",
      async () => {
        const page = await createPage({ title: t("template.newTitle"), select: false });
        await setPageTemplate(page.id, true);
        focusPageTitle(page.id);
        navigateTo({ pageId: page.id, spaceId: page.spaceId });
      },
      true,
    );
  }

  function makeFrom(page: PageInfo): void {
    void act(
      page.id,
      () => createFromTemplate(page.id, page.parentId === undefined ? {} : { parentId: page.parentId }),
      true,
    );
  }

  const noTemplates = groups.length === 0;

  return (
    <Modal labelledBy={titleId} onDismiss={onClose} className="templates-dialog">
      <div className="templates-header">
        <h2 className="subtitle" id={titleId}>
          {t("template.panel.title")}
        </h2>
        {canEdit && (
          <button type="button" className="button button-small" disabled={busy !== null} onClick={newTemplate}>
            {t("template.panel.new")}
          </button>
        )}
      </div>
      <p className="muted templates-help">{t("template.panel.help")}</p>
      {error !== null && (
        <p className="error-text templates-error" role="alert">
          {error}
        </p>
      )}
      <div className="templates-body">
        {noTemplates && <p className="muted templates-empty">{t("template.panel.empty")}</p>}
        {groups.map((group) => (
          <section key={group.parentId ?? "top"} className="templates-group" aria-label={groupTitle(group)}>
            <h3 className="templates-group-title">{groupTitle(group)}</h3>
            <ul className="template-list">
              {group.templates.map((page) => (
                <Item
                  key={page.id}
                  title={titleOf(page)}
                  icon={page.icon}
                  untitled={page.title === ""}
                  busy={busy === page.id}
                >
                  {(describedBy) => (
                    <>
                      {canEdit && (
                        <button
                          type="button"
                          className="link-button"
                          aria-describedby={describedBy}
                          disabled={busy !== null}
                          onClick={() => {
                            makeFrom(page);
                          }}
                        >
                          {t("template.use")}
                        </button>
                      )}
                      <button
                        type="button"
                        className="link-button"
                        aria-describedby={describedBy}
                        disabled={busy !== null}
                        onClick={() => {
                          edit(page);
                        }}
                      >
                        {canEdit ? t("template.edit") : t("template.open")}
                      </button>
                      {canEdit && (
                        <>
                          <button
                            type="button"
                            className="link-button"
                            aria-describedby={describedBy}
                            disabled={busy !== null}
                            onClick={() => {
                              void act(page.id, () => duplicatePage(page.id, { open: false }), false);
                            }}
                          >
                            {t("template.duplicate")}
                          </button>
                          <button
                            type="button"
                            className="link-button link-danger"
                            aria-describedby={describedBy}
                            disabled={busy !== null}
                            onClick={() => {
                              void act(page.id, () => trashPage(page.id), false);
                            }}
                          >
                            {t("template.delete")}
                          </button>
                        </>
                      )}
                    </>
                  )}
                </Item>
              ))}
            </ul>
          </section>
        ))}
        {canEdit && starters.length > 0 && (
          <section className="templates-group" aria-label={t("template.panel.starters")}>
            <h3 className="templates-group-title">{t("template.panel.starters")}</h3>
            <ul className="template-list">
              {starters.map((starter: StarterEntry) => (
                <Item
                  key={starter.id}
                  title={starter.title}
                  icon={starter.icon}
                  description={starter.description}
                  busy={busy === starter.id}
                >
                  {(describedBy) => (
                    <>
                      <button
                        type="button"
                        className="link-button"
                        aria-describedby={describedBy}
                        disabled={busy !== null}
                        onClick={() => {
                          void act(starter.id, () => createFromStarter(starter.id), true);
                        }}
                      >
                        {t("template.use")}
                      </button>
                      <button
                        type="button"
                        className="link-button"
                        aria-describedby={describedBy}
                        disabled={busy !== null}
                        onClick={() => {
                          void act(starter.id, () => addStarterAsTemplate(starter.id), false);
                        }}
                      >
                        {t("template.addToMine")}
                      </button>
                    </>
                  )}
                </Item>
              ))}
            </ul>
          </section>
        )}
      </div>
      <div className="dialog-actions">
        <button type="button" className="button" onClick={onClose}>
          {t("dialog.close")}
        </button>
      </div>
    </Modal>
  );
}

/**
 * The "Templates" entry at the bottom of the sidebar, above the trash, and the panel it opens. Nothing while the tree of the
 * space is not read.
 */
export function TemplatesPanel() {
  const hasSpace = useAppStore((state) => state.tree !== null);
  const [open, setOpen] = useState(false);
  if (!hasSpace) {
    return null;
  }
  return (
    <div className="templates-entry">
      <button
        type="button"
        className="templates-toggle"
        aria-haspopup="dialog"
        onClick={() => {
          setOpen(true);
        }}
      >
        <TemplateIcon />
        <span className="templates-toggle-label">{t("template.panel.open")}</span>
      </button>
      {open && (
        <TemplatesDialog
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </div>
  );
}
