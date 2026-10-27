import { useEffect, useLayoutEffect, useRef } from "react";
import type { CSSProperties } from "react";

import { useAutoLock } from "../auto-lock";
import { AboutDialog } from "../components/AboutDialog";
import { AiChatPanel } from "../components/AiChatPanel";
import { CommentsPanel } from "../components/CommentsPanel";
import { DevicesNotice } from "../components/DevicesNotice";
import { InviteSessionHost } from "../components/InviteSessionHost";
import { PeekHost } from "../components/PeekHost";
import { SidebarIcon } from "../components/icons";
import { ReminderToasts } from "../components/ReminderToasts";
import { SearchDialog } from "../components/SearchDialog";
import { Sidebar } from "../components/Sidebar";
import { SidebarResizeHandle } from "../components/SidebarResizeHandle";
import { TAB_PANEL_ID, TabStrip, tabElementId } from "../components/TabStrip";
import { Toasts } from "../components/Toasts";
import { t } from "../i18n";
import { useMenuBarKeys } from "../menu-bar";
import { MODIFIER, ignoresNonTypingShortcuts, inFormField, insideOverlay, tabCommand } from "../shortcuts";
import type { TabCommand } from "../shortcuts";
import { selectAiAvailable, selectAiEnabled } from "../store-ai";
import { useAppStore, useStoreApi } from "../store-context";
import { selectSidebarHidden, selectSidebarWidth } from "../store-layout";
import { selectPeekWidth } from "../store-peek";
import { ScrollContainerContext } from "../tab-scroll";
import { DiagnosticsScreen } from "./DiagnosticsScreen";
import { HomeScreen } from "./HomeScreen";
import { SettingsScreen } from "./SettingsScreen";

/** The buttons of a mouse that go back (3) and forward (4). */
const MOUSE_BACK = 3;
const MOUSE_FORWARD = 4;

/**
 * The app once a workspace exists: the sidebar on the left, on the right the tab strip
 * (ADR-0034) above the current screen. Ctrl+K (Cmd+K on macOS) opens the search from anywhere,
 * the editor included. With the app lock on, it locks itself after the chosen time without
 * activity.
 *
 * The sidebar's width and visibility are device settings (ADR-0029): the width is the CSS
 * variable `--sidebar-width` on the layout (the resize handle changes it live), Ctrl+\
 * (Cmd+\) and the buttons hide and show the sidebar, and a button at the top left of the
 * content shows it again while it is hidden.
 *
 * The tab keys work anywhere but inside a dialog or a menu, in form fields and in the editor
 * too: Ctrl/Cmd+T opens a tab, Ctrl/Cmd+W (and Ctrl+F4) closes the active one, Ctrl/Cmd+Shift+T
 * reopens the last closed one, Ctrl+Tab and Ctrl+PageDown go to the next tab, Ctrl+Shift+Tab
 * and Ctrl+PageUp to the previous one, Ctrl/Cmd+1 to 8 to that tab and Ctrl/Cmd+9 to the last,
 * and Ctrl+Shift+PageUp and PageDown move the tab. Alt+Left and Alt+Right, and Ctrl+[ and Ctrl+]
 * (Cmd+[ and Cmd+] on macOS) go back and forward in the tab, except in a form field; so do the back and forward
 * buttons of a mouse, wherever the pointer is (the webview must not go back in its own history).
 * The screen scrolls in its own body under the strip, which keeps still; `HomeScreen` remembers
 * where each entry of the tabs was scrolled to.
 *
 * With local AI on (ADR-0037) Ctrl/Cmd+J and the button at the end of the tab strip open the chat panel, a
 * column to the right of the screen (`AiChatPanel`); with AI off neither exists. The comments panel (ADR-0048,
 * `CommentsPanel`) is a column of its own beside the page while it is open and a page is. The side peek of a
 * database row (ADR-0054, `PeekHost`) is one more column, next to the page and before the comments and the chat: 480 px by
 * default and resizable (the device setting `ui.peek.width`; the CSS variable `--peek-width` on this area while its handle
 * is dragged). Its center mode is a dialog (`Modal`) and takes no column. It shows with the main screen only.
 *
 * In the desktop app a lone Alt press shows the native menu bar (ADR-0040, `useMenuBarKeys`), and the menu's About
 * opens `AboutDialog` here.
 */
export function MainLayout() {
  const store = useStoreApi();
  const route = useAppStore((state) => state.route);
  const searchOpen = useAppStore((state) => state.search.open);
  const activeTabId = useAppStore((state) => state.activeTabId);
  const width = useAppStore(selectSidebarWidth);
  const hidden = useAppStore(selectSidebarHidden);
  const setSidebarHidden = useAppStore((state) => state.setSidebarHidden);
  const aiEnabled = useAppStore(selectAiEnabled);
  const aiAvailable = useAppStore(selectAiAvailable);
  const chatOpen = useAppStore((state) => state.chatOpen);
  const toggleChat = useAppStore((state) => state.toggleChat);
  const commentsOpen = useAppStore((state) => state.commentsOpen && state.pageComments !== null);
  const peekWidth = useAppStore(selectPeekWidth);
  // The side peek is a column of the main area while it is open; the center peek is a dialog and takes none.
  const peekColumn = useAppStore((state) => state.route === "home" && state.peek?.mode === "side");
  const layoutRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const showButtonRef = useRef<HTMLButtonElement>(null);
  const wasHidden = useRef(hidden);
  const aboutOpen = useAppStore((state) => state.aboutOpen);
  const closeAbout = useAppStore((state) => state.closeAbout);
  useAutoLock();
  useMenuBarKeys();

  useEffect(() => {
    /** Runs a tab command; one that changes what is in front shows the main screen (not Settings). */
    function runTabCommand(command: TabCommand): void {
      const state = store.getState();
      const active = state.tabs.findIndex((tab) => tab.id === state.activeTabId);
      let fronts = true;
      switch (command.kind) {
        case "new":
          state.newTab();
          break;
        case "close": {
          const tab = state.tabs[active];
          if (tab !== undefined) {
            state.closeTab(tab.id);
          }
          break;
        }
        case "reopen":
          state.reopenClosedTab();
          break;
        case "adjacent":
          state.activateAdjacentTab(command.delta);
          break;
        case "at":
          state.activateTabAt(command.position);
          break;
        case "move": {
          const tab = state.tabs[active];
          if (tab !== undefined) {
            state.moveTab(tab.id, Math.min(Math.max(active + command.delta, 0), state.tabs.length - 1));
          }
          fronts = false;
          break;
        }
        case "history":
          if (command.delta < 0) {
            state.goBack();
          } else {
            state.goForward();
          }
          break;
      }
      if (fronts && store.getState().route !== "home") {
        store.getState().navigate("home");
      }
    }

    function onKeyDown(event: KeyboardEvent): void {
      if (event.defaultPrevented) {
        return;
      }
      const command = tabCommand(event);
      if (command !== null) {
        // A dialog or menu owns its keys; Back and Forward are not for a form field either.
        if (insideOverlay(event.target) || (command.kind === "history" && inFormField(event.target))) {
          return;
        }
        event.preventDefault();
        runTabCommand(command);
        return;
      }
      if (!(event.ctrlKey || event.metaKey) || event.shiftKey) {
        return;
      }
      // The key that types "\" (with AltGr on many layouts, which reports Alt too), or the
      // physical backslash key whatever the layout prints on it (`*` on AZERTY, `#` on QWERTZ).
      if (event.key === "\\" || (event.code === "Backslash" && !event.altKey)) {
        // Not while typing in a field or in a dialog or menu; the page editor is fine.
        if (!ignoresNonTypingShortcuts(event.target)) {
          event.preventDefault();
          store.getState().toggleSidebar();
        }
        return;
      }
      if (!event.altKey && event.key.toLowerCase() === "j") {
        // The chat panel (ADR-0037): only with AI on, and not under a dialog or menu.
        if (!insideOverlay(event.target) && selectAiEnabled(store.getState())) {
          event.preventDefault();
          store.getState().toggleChat();
        }
        return;
      }
      if (event.altKey || event.key.toLowerCase() !== "k") {
        return;
      }
      event.preventDefault();
      store.getState().openSearch();
    }

    // The mouse's back and forward buttons go back and forward in the tab. The webview would go back
    // in its own history on them, so the press is cancelled everywhere; the history of the tab moves
    // on the release (once: the click that follows it is only cancelled), but not under a dialog.
    let released = false;
    function onMouseButton(event: MouseEvent): void {
      if (event.button !== MOUSE_BACK && event.button !== MOUSE_FORWARD) {
        return;
      }
      event.preventDefault();
      if (event.type === "mousedown") {
        released = false;
        return;
      }
      const alreadyHandled = event.type === "auxclick" && released;
      released = event.type === "mouseup";
      if (alreadyHandled || insideOverlay(event.target)) {
        return;
      }
      const state = store.getState();
      if (event.button === MOUSE_BACK) {
        state.goBack();
      } else {
        state.goForward();
      }
      if (store.getState().route !== "home") {
        store.getState().navigate("home");
      }
    }

    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("mousedown", onMouseButton);
    document.addEventListener("mouseup", onMouseButton);
    document.addEventListener("auxclick", onMouseButton);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("mousedown", onMouseButton);
      document.removeEventListener("mouseup", onMouseButton);
      document.removeEventListener("auxclick", onMouseButton);
    };
  }, [store]);

  // Focus does not get lost with the button that was pressed: hiding moves it to the "Show"
  // button, showing moves it to the "Hide" button. Hiding or showing from the keyboard
  // shortcut while focus is elsewhere leaves focus where it is.
  useLayoutEffect(() => {
    if (wasHidden.current === hidden) {
      return;
    }
    wasHidden.current = hidden;
    const layout = layoutRef.current;
    if (layout === null) {
      return;
    }
    const focus = document.activeElement;
    const lost = focus === null || focus === document.body;
    if (hidden) {
      if (lost || layout.querySelector(".sidebar")?.contains(focus) === true) {
        showButtonRef.current?.focus();
      }
    } else if (lost) {
      layout.querySelector<HTMLElement>(".sidebar-hide")?.focus();
    }
  }, [hidden]);

  // The body is the panel of the active tab while a page screen shows (Settings is not one).
  const panel = route === "home" && activeTabId !== null;
  // The chat panel is a column beside the screen while it is open (a grid; the tab strip spans both columns).
  const chatVisible = aiAvailable && chatOpen;
  const commentsVisible = route === "home" && commentsOpen;
  const contentClass = [
    "content",
    aiEnabled ? "has-ai-toggle" : "",
    chatVisible ? "has-chat" : "",
    commentsVisible ? "has-comments" : "",
    peekColumn ? "has-peek" : "",
  ]
    .filter((name) => name !== "")
    .join(" ");

  return (
    <div
      ref={layoutRef}
      className={hidden ? "layout is-sidebar-hidden" : "layout"}
      style={{ "--sidebar-width": `${String(width)}px` } as CSSProperties}
    >
      <Sidebar hidden={hidden} />
      {!hidden && <SidebarResizeHandle />}
      {hidden && (
        <button
          ref={showButtonRef}
          type="button"
          className="icon-button sidebar-show"
          aria-label={t("sidebar.show")}
          title={t("sidebar.showTitle", { shortcut: `${MODIFIER}+\\` })}
          onClick={() => {
            setSidebarHidden(false);
          }}
        >
          <SidebarIcon />
        </button>
      )}
      <main className={contentClass} style={peekColumn ? ({ "--peek-width": `${String(peekWidth)}px` } as CSSProperties) : undefined}>
        <TabStrip />
        {aiEnabled && (
          <button
            type="button"
            className="icon-button ai-toggle"
            aria-pressed={chatOpen}
            aria-label={t("aiChat.toggle")}
            title={t("aiChat.toggleTitle", { shortcut: `${MODIFIER}+J` })}
            onClick={toggleChat}
          >
            <span aria-hidden="true">✦</span>
          </button>
        )}
        <div
          ref={bodyRef}
          className="content-body"
          id={panel ? TAB_PANEL_ID : undefined}
          role={panel ? "tabpanel" : undefined}
          aria-labelledby={panel ? tabElementId(activeTabId) : undefined}
        >
          <ScrollContainerContext.Provider value={bodyRef}>
            <DevicesNotice />
            {route === "home" && <HomeScreen />}
            {route === "settings" && <SettingsScreen />}
            {route === "diagnostics" && <DiagnosticsScreen />}
          </ScrollContainerContext.Provider>
        </div>
        {route === "home" && <PeekHost />}
        {commentsVisible && <CommentsPanel />}
        {chatVisible && <AiChatPanel />}
      </main>
      {searchOpen && <SearchDialog />}
      {aboutOpen && <AboutDialog onClose={closeAbout} />}
      <InviteSessionHost />
      <ReminderToasts />
      <Toasts />
    </div>
  );
}
