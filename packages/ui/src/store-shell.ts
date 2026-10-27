// The desktop shell's part of the app store (ADR-0040): what the shell tells the page on its own account, the
// settings of the tray and the quick capture, and the words the page gives the shell.
//
// - **Messages.** `CoreClient.onShellEvent` delivers `deepLink`, `capture`, `lock`, `shortcut`, `menu` and `menuBar`
//   messages from the desktop shell (a transport with no shell never calls it). A link goes to `store-deep-links.ts`.
//   An `inviteLink` message (`{ link }`, an `atomik-kb://invite?...` link the shell accepted, Milestone 13) goes to the same place
//   and opens the invitation's preview when the workspace is open.
//   `shortcut` is the state of the quick-capture shortcut after the shell (re)registered it, kept for Settings.
//   `lock` is the tray's or the menu bar's "Lock": the UI's own lock action runs (it sends the editor's unsaved text
//   before the core closes), or a toast says that the app lock is off. `capture` is the tray's, the menu bar's and
//   the shortcut's new page. `menu` and `menuBar` are the native menu bar's (`store-menu-bar.ts`).
// - **Capture.** The global shortcut and the tray's "New page" make a page at the top level of the space in use
//   (the current space, which is the active tab's), open it in the active tab and ask for its title to be edited
//   (`focusPageTitle`, the same request "turn into a page" makes). While the app is still opening or locked the
//   request is held (one, however many came) and runs when the workspace is open, like a deep link: a locked app
//   creates nothing until the passphrase has been given. Without a workspace (first run) or after a failed start
//   there is nothing to make a page in and the request is dropped.
// - **Settings.** `app.tray.close_to_tray` (a boolean, default true) and `app.shortcut.capture` (the accelerator,
//   default `Ctrl+Alt+N`, `""` for none) are device settings the shell follows (`desktop_settings.rs` of the shell). The
//   parsers here read them the way the shell does.
// - **Words.** The tray menu, the menu bar, the shell's consent dialogs (an AI endpoint that is another computer, an
//   AI tool's access to spaces, the download of an import's images, the download of a dictation model, the sign-in to Google Drive and the page the browser lands on after it) and the file types of its file dialogs are
//   native: the UI sends its strings (`tray.*`, `menu.*`, `shellDialog.*`, `fileFilter.*`) with `shell_labels_set` as
//   soon as it starts with a language and again whenever the language changes (the store, `applyLanguage`), so that
//   the translation of the UI (ADR-0040 item 13) reaches them without the shell knowing a language. The consent
//   texts keep their `{placeholders}` (`{host}`, `{client}`, `{spaces}`, ...): the shell fills them, and refuses a
//   text that dropped one that a consent depends on.

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, JsonValue, ShellEvent, ShellLabels, ShortcutState } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { DEFAULT_CAPTURE_SHORTCUT } from "./accelerator";
import { t } from "./i18n";
import { createDeepLinkSlice } from "./store-deep-links";
import { createMenuBarSlice } from "./store-menu-bar";
import type { MenuBarState } from "./store-menu-bar";
import type { AppState } from "./store";

/** The link of an `inviteLink` message's data (`{ link }`); `null` for anything else. */
function inviteLinkOf(data: unknown): string | null {
  if (typeof data !== "object" || data === null || !("link" in data)) {
    return null;
  }
  const { link } = data;
  return typeof link === "string" && link !== "" ? link : null;
}

/** The device setting that decides what the window's close button does (the shell reads it too). */
export const CLOSE_TO_TRAY_KEY = "app.tray.close_to_tray";

/** The device setting that holds the quick-capture accelerator (the shell reads it too). */
export const CAPTURE_SHORTCUT_KEY = "app.shortcut.capture";

/** Whether closing the window keeps the app in the tray: yes unless the setting is `false`. */
export function parseCloseToTray(value: JsonValue | null | undefined): boolean {
  return value !== false;
}

/**
 * The accelerator the setting holds, as text: the default while it is unset, the empty text for "no shortcut" (and for
 * a value that is not text, which the shell reports as invalid), otherwise the stored text.
 */
export function parseCaptureShortcut(value: JsonValue | null | undefined): string {
  if (value === undefined || value === null) {
    return DEFAULT_CAPTURE_SHORTCUT;
  }
  return typeof value === "string" ? value.trim() : "";
}

/** The words of the shell's native dialogs and of the file types in its file dialogs, from the active language's strings. */
export function dialogLabels(): ShellLabels {
  return {
    dialogAllow: t("shellDialog.allow"),
    dialogCancel: t("shellDialog.cancel"),
    dialogYes: t("shellDialog.yes"),
    dialogNo: t("shellDialog.no"),
    dialogUntitled: t("shellDialog.untitled"),
    dialogAiTitle: t("shellDialog.aiTitle"),
    dialogAiMessage: t("shellDialog.aiMessage"),
    dialogMcpTitle: t("shellDialog.mcpTitle"),
    dialogMcpRead: t("shellDialog.mcpRead"),
    dialogMcpWrite: t("shellDialog.mcpWrite"),
    dialogMcpBoth: t("shellDialog.mcpBoth"),
    dialogListAnd: t("shellDialog.listAnd"),
    dialogListMore: t("shellDialog.listMore"),
    dialogImagesTitle: t("shellDialog.imagesTitle"),
    dialogImagesOne: t("shellDialog.imagesOne"),
    dialogImagesOther: t("shellDialog.imagesOther"),
    dialogHostsOne: t("shellDialog.hostsOne"),
    dialogHostsOther: t("shellDialog.hostsOther"),
    dialogNotifyThreadReply: t("shellDialog.notifyThreadReply"),
    dialogNotifyCommentMention: t("shellDialog.notifyCommentMention"),
    dialogNotifyPageMention: t("shellDialog.notifyPageMention"),
    dialogNotifyPersonAssigned: t("shellDialog.notifyPersonAssigned"),
    dialogDictationTitle: t("shellDialog.dictationTitle"),
    dialogDictationMessage: t("shellDialog.dictationMessage"),
    dialogDownload: t("shellDialog.download"),
    dialogDriveTitle: t("shellDialog.driveTitle"),
    dialogDriveMessage: t("shellDialog.driveMessage"),
    dialogDriveAccept: t("shellDialog.driveAccept"),
    dialogDriveCancel: t("shellDialog.driveCancel"),
    dialogDriveDone: t("shellDialog.driveDone"),
    fileFilterBackup: t("fileFilter.backup"),
    fileFilterZip: t("fileFilter.zip"),
    fileFilterText: t("fileFilter.text"),
    fileFilterImport: t("fileFilter.import"),
  };
}

export interface ShellState extends MenuBarState {
  /** A desktop shell is around the core: it has a tray, a shortcut and a window that can hide (not the web build). */
  desktopShell: boolean;
  /** What the shell last said about the quick-capture shortcut; `null` before it said anything. */
  shortcutState: ShortcutState | null;
}

export interface ShellSlice {
  initial: ShellState;
  /** What the transport's listener calls with each message of the shell. */
  receive: (event: ShellEvent) => void;
  /** The workspace is open (the app became ready): runs what waited. */
  ready: () => void;
  /** There is no workspace to act in (the first-run screen, a failed start): forgets what waited. */
  drop: () => void;
  /** Gives the shell the words of its native surfaces (the tray menu, the menu bar, the dialogs), from the UI's strings. */
  sendLabels: () => void;
}

export function createShellSlice(client: CoreClient, set: StoreApi<AppState>["setState"], get: StoreApi<AppState>["getState"]): ShellSlice {
  const deepLinks = createDeepLinkSlice(client, get);
  const menuBar = createMenuBarSlice(client, set, get);
  // A capture that could not run yet. Not part of the store's state: nothing shows it.
  let captureHeld = false;

  /** Keeps the capture for the next time the app is ready, unless there is nothing to wait for. */
  function hold(): void {
    const { phase } = get();
    captureHeld = phase === "loading" || phase === "locked";
  }

  async function capture(): Promise<void> {
    if (get().currentSpaceId === null) {
      return; // no space to make a page in
    }
    try {
      // At the top level of the space in use; the page opens in the active tab with its title ready to type.
      const page = await get().createPage({ parentId: null, select: false });
      get().focusPageTitle(page.id);
      get().navigateTo({ pageId: page.id, spaceId: page.spaceId, disposition: "current" });
    } catch (error) {
      // The code only: a message can name content.
      console.error("a quick capture could not be made", toCoreError(error).code);
      get().showToast(t("shell.capture.error"), "error");
    }
  }

  async function lockFromTray(): Promise<void> {
    const { capabilities, security } = get();
    if (!capabilities.appLock || security?.appLock.enabled !== true) {
      get().showToast(t("shell.lock.unavailable"), "info");
      return;
    }
    try {
      await get().lock();
    } catch (error) {
      console.error("the app could not be locked from the tray", toCoreError(error).code);
      get().showToast(t("shell.lock.error"), "error");
    }
  }

  return {
    initial: { desktopShell: client.hasDesktopShell(), shortcutState: null, ...menuBar.initial },

    receive: (event) => {
      // An invitation link (Milestone 13): read through a looser type so that this build handles the message whether or not
      // the `ShellEvent` type of the transport names it yet (the shell adds it with its `invite` link form).
      const message: { type: string; data?: unknown } = event;
      if (message.type === "inviteLink") {
        const link = inviteLinkOf(message.data);
        if (link !== null) {
          deepLinks.receiveInvite(link);
        }
        return;
      }
      switch (event.type) {
        case "deepLink":
          deepLinks.receive(event.data);
          break;
        case "capture":
          if (get().phase === "ready") {
            captureHeld = false; // one capture is as good as two
            void capture();
          } else {
            hold();
          }
          break;
        case "lock":
          // A locked app is locked, and an app that is opening or has no workspace has nothing to lock.
          if (get().phase === "ready") {
            void lockFromTray();
          }
          break;
        case "shortcut":
          set({ shortcutState: event.data.state });
          break;
        case "menu":
          menuBar.run(event.data.id);
          break;
        case "menuBar":
          menuBar.visibility(event.data.visible);
          break;
        // Messages of a newer shell are not this UI's business (the transport drops the unknown types).
      }
    },

    ready: () => {
      menuBar.reset();
      deepLinks.ready();
      if (captureHeld) {
        captureHeld = false;
        void capture();
      }
    },

    drop: () => {
      menuBar.reset();
      deepLinks.drop();
      captureHeld = false;
    },

    sendLabels: () => {
      if (!client.hasDesktopShell()) {
        return;
      }
      client
        .shellLabelsSet({
          open: t("tray.open"),
          newPage: t("tray.newPage"),
          lock: t("tray.lock"),
          quit: t("tray.quit"),
          ...menuBar.labels(),
          ...dialogLabels(),
        })
        .catch((error: unknown) => {
          // The shell keeps its English defaults. The code only: a label is the person's language.
          console.error("the shell's labels could not be set", toCoreError(error).code);
        });
    },
  };
}
