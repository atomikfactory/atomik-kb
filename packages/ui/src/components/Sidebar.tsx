import { toCoreError } from "@atomik-kb/core-client";
import { useCallback, useEffect, useState } from "react";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useCurrentAccess } from "../sharing-hooks";
import { MODIFIER, ignoresShortcuts } from "../shortcuts";
import type { Route } from "../store";
import { useAppStore, useStoreApi } from "../store-context";
import { selectCurrentCanEdit } from "../store-sharing";
import { ActionContext } from "./actions";
import type { RunAction } from "./actions";
import { Favorites } from "./Favorites";
import { LockIcon, RedoIcon, SearchIcon, SidebarIcon, UndoIcon } from "./icons";
import { PageTree } from "./PageTree";
import { SpaceSwitcher } from "./SpaceSwitcher";
import { NotificationsBell } from "./NotificationsBell";
import { SyncIndicator } from "./SyncIndicator";
import { TemplatesPanel } from "./TemplatesPanel";
import { TrashPanel } from "./TrashPanel";

const NAV_ITEMS: readonly { route: Route; label: MessageKey }[] = [
  { route: "home", label: "nav.home" },
  { route: "settings", label: "nav.settings" },
  { route: "diagnostics", label: "nav.diagnostics" },
];

/**
 * The left column: workspace (with the sync indicator) and undo/redo, the space switcher,
 * the page tree, the Templates panel (ADR-0056), the trash, the navigation to Settings and Diagnostics and, with the app
 * lock on, "Lock now". It also owns the alert that shows why the core refused something (a
 * failed move, a name that is too long).
 *
 * Ctrl+Z and Ctrl+Shift+Z (Cmd on macOS) undo and redo structural changes in the current
 * space, unless focus is in a text field.
 *
 * `hidden` takes it off the screen (ADR-0029) without unmounting it, so the tree keeps its
 * state; its header has the button that hides it (`MainLayout` has the one that shows it).
 */
export function Sidebar({ hidden = false }: { hidden?: boolean }) {
  const store = useStoreApi();
  const workspaceName = useAppStore((state) => state.workspace?.name);
  const route = useAppStore((state) => state.route);
  const navigate = useAppStore((state) => state.navigate);
  // Undo and redo are structure edits: a role that cannot edit the space has none (Milestone 13).
  const canEdit = useCurrentAccess().canEdit;
  const canUndo = useAppStore((state) => state.tree?.undo.canUndo ?? false) && canEdit;
  const canRedo = useAppStore((state) => state.tree?.undo.canRedo ?? false) && canEdit;
  const undo = useAppStore((state) => state.undo);
  const redo = useAppStore((state) => state.redo);
  const openSearch = useAppStore((state) => state.openSearch);
  const canLock = useAppStore((state) => state.capabilities.appLock && state.security?.appLock.enabled === true);
  const lock = useAppStore((state) => state.lock);
  const setSidebarHidden = useAppStore((state) => state.setSidebarHidden);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback<RunAction>(async (action, errorKey) => {
    setError(null);
    try {
      await action;
    } catch (failure) {
      setError(t(errorKey, { message: toCoreError(failure).message }));
    }
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (event.defaultPrevented || !(event.ctrlKey || event.metaKey) || event.altKey) {
        return;
      }
      if (event.key.toLowerCase() !== "z" || ignoresShortcuts(event.target)) {
        return;
      }
      const state = store.getState();
      const available = event.shiftKey ? state.tree?.undo.canRedo : state.tree?.undo.canUndo;
      if (available !== true || !selectCurrentCanEdit(state)) {
        return;
      }
      event.preventDefault();
      if (event.shiftKey) {
        void run(state.redo(), "tree.error.redo");
      } else {
        void run(state.undo(), "tree.error.undo");
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [store, run]);

  return (
    <ActionContext value={run}>
      <nav className="sidebar" aria-label={t("nav.label")} hidden={hidden}>
        <div className="sidebar-header">
          <div className="sidebar-title">
            <p className="brand">{t("app.name")}</p>
            {workspaceName !== undefined && (
              <div className="sidebar-workspace-row">
                <p className="sidebar-workspace">{workspaceName}</p>
                <SyncIndicator />
              </div>
            )}
          </div>
          <div className="sidebar-history">
            <NotificationsBell />
            <button
              type="button"
              className="icon-button"
              aria-label={t("sidebar.undo")}
              title={t("sidebar.undoTitle", { shortcut: `${MODIFIER}+Z` })}
              disabled={!canUndo}
              onClick={() => {
                void run(undo(), "tree.error.undo");
              }}
            >
              <UndoIcon />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label={t("sidebar.redo")}
              title={t("sidebar.redoTitle", { shortcut: `${MODIFIER}+Shift+Z` })}
              disabled={!canRedo}
              onClick={() => {
                void run(redo(), "tree.error.redo");
              }}
            >
              <RedoIcon />
            </button>
            <button
              type="button"
              className="icon-button sidebar-hide"
              aria-label={t("sidebar.hide")}
              title={t("sidebar.hideTitle", { shortcut: `${MODIFIER}+\\` })}
              onClick={() => {
                setSidebarHidden(true);
              }}
            >
              <SidebarIcon />
            </button>
          </div>
        </div>

        <button
          type="button"
          className="sidebar-search"
          aria-label={t("search.open")}
          title={t("search.openTitle", { shortcut: `${MODIFIER}+K` })}
          onClick={openSearch}
        >
          <SearchIcon />
          <span className="sidebar-search-label">{t("search.open")}</span>
          <kbd className="sidebar-kbd" aria-hidden="true">
            {MODIFIER}+K
          </kbd>
        </button>
        <SpaceSwitcher />
        <div className="sidebar-scroll">
          <Favorites />
          <PageTree />
        </div>
        {error !== null && (
          <div className="sidebar-error" role="alert">
            <span>{error}</span>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setError(null);
              }}
            >
              {t("sidebar.dismissError")}
            </button>
          </div>
        )}
        <TemplatesPanel />
        <TrashPanel />

        <ul className="nav-list">
          {NAV_ITEMS.map((item) => (
            <li key={item.route}>
              <button
                type="button"
                className="nav-item"
                aria-current={route === item.route ? "page" : undefined}
                onClick={() => {
                  navigate(item.route);
                }}
              >
                {t(item.label)}
              </button>
            </li>
          ))}
        </ul>
        {canLock && (
          <div className="sidebar-footer">
            <button
              type="button"
              className="nav-item sidebar-lock"
              onClick={() => {
                void run(lock(), "security.lock.error.lock");
              }}
            >
              <LockIcon />
              <span>{t("sidebar.lockNow")}</span>
            </button>
          </div>
        )}
      </nav>
    </ActionContext>
  );
}
