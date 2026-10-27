// The deep-link part of the app store (ADR-0040): a link of the operating system (`atomik-kb://page/<pageId>[#<blockId>]`)
// that the desktop shell accepted and handed to the UI. The shell has already refused everything else (it reads
// the link strictly and never logs it); what is left to do here is to open the page.
//
// - **When.** A link opens at once while the app is ready. While it is still opening (the first start of the app, by
//   a link) or locked, the last link is held and opens when the workspace is: the lock screen shows nothing of it, and
//   a locked app opens no page. A link that comes before the workspace exists (the first-run screen) or while the app
//   failed to start has nothing to open and is dropped.
// - **How.** Like a plain click on a page link (ADR-0034): in the active tab, switching space when the page is in
//   another, and scrolling to the block when the link names one. A page that is not there, or is in the trash, is
//   said in a toast ("Unknown page"); the link and its ids are never shown, logged or kept.
// - **Invitation links (Milestone 13).** `atomik-kb://invite?server=<url>#<secret>` is another kind of link the shell may hand
//   over (`receiveInvite`): it holds and releases exactly like a page link (an app that is opening or locked keeps the last one,
//   a first-run screen or a failed start drops it), and when the app is ready `openInviteLink` (`store-sharing.ts`) opens it:
//   the core fetches the invitation and the preview dialog shows (`InviteSessionHost`). The link carries a secret: it is never
//   shown, logged or kept after the session starts, and a failure says only that the invitation could not be opened.
// - **Where it listens.** From `start()` to its undoing, through `CoreClient.onShellEvent` (`store-shell.ts` routes a `deepLink` message here): a transport with no shell
//   (the in-memory one, the browser build) has none, so nothing here ever runs there.

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, DeepLink, SpaceId } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { t } from "./i18n";
import type { AppState } from "./store";

export interface DeepLinkSlice {
  /** What the transport's listener calls with each link: opens it, holds it until the workspace is open, or drops it. */
  receive: (link: DeepLink) => void;
  /** The same for an invitation link (`atomik-kb://invite?...`): the preview of the invitation opens when the app is ready. */
  receiveInvite: (link: string) => void;
  /** The workspace is open (the app became ready): opens the link that waited, if any. */
  ready: () => void;
  /** There is no workspace to open a page in (the first-run screen, a failed start): forgets a link that waited. */
  drop: () => void;
}

export function createDeepLinkSlice(client: CoreClient, get: StoreApi<AppState>["getState"]): DeepLinkSlice {
  // The last link that could not open yet. Not part of the store's state: nothing shows it.
  let held: DeepLink | null = null;
  // The same for an invitation link; it holds a secret, so it lives here and nowhere else.
  let heldInvite: string | null = null;

  /** Keeps the link for the next time the app is ready, unless there is nothing to wait for. */
  function hold(link: DeepLink): void {
    const { phase } = get();
    held = phase === "loading" || phase === "locked" ? link : null;
  }

  async function open(link: DeepLink): Promise<void> {
    let spaceId: SpaceId;
    try {
      const page = await client.pageGet(link.pageId);
      if (page.trashedAt !== undefined) {
        get().showToast(t("deepLink.unknown"), "info");
        return;
      }
      spaceId = page.spaceId;
    } catch (error) {
      const { code } = toCoreError(error);
      if (code === "NotFound") {
        get().showToast(t("deepLink.unknown"), "info");
      } else if (code === "Locked") {
        // Locked between the check and the call: it opens after the next unlock.
        held = link;
      } else {
        // The code only: the link and its ids stay out of the log.
        console.error("a deep link could not be opened", code);
        get().showToast(t("deepLink.error"), "info");
      }
      return;
    }
    if (get().phase !== "ready") {
      hold(link); // the app locked while the page was being read
      return;
    }
    get().navigateTo({ pageId: link.pageId, spaceId, blockId: link.blockId, disposition: "current" });
  }

  /** Opens an invitation link: the preview shows once the core has the invitation; the link itself is not kept. */
  async function openInvite(link: string): Promise<void> {
    try {
      await get().openInviteLink(link);
    } catch (error) {
      const { code } = toCoreError(error);
      if (code === "Locked") {
        // Locked between the check and the call: it opens after the next unlock.
        heldInvite = link;
      } else {
        // The code only: the link holds a secret and its message can name it.
        console.error("an invitation link could not be opened", code);
        get().showToast(t("sharing.preview.openError"), "error");
      }
    }
  }

  return {
    receiveInvite: (link) => {
      if (get().phase === "ready") {
        heldInvite = null; // a newer link replaces one that waited
        void openInvite(link);
      } else {
        const { phase } = get();
        heldInvite = phase === "loading" || phase === "locked" ? link : null;
      }
    },

    receive: (link) => {
      if (get().phase === "ready") {
        held = null; // a newer link replaces one that waited
        void open(link);
      } else {
        hold(link);
      }
    },

    ready: () => {
      const link = held;
      held = null;
      if (link !== null) {
        void open(link);
      }
      const invite = heldInvite;
      heldInvite = null;
      if (invite !== null) {
        void openInvite(invite);
      }
    },

    drop: () => {
      held = null;
      heldInvite = null;
    },
  };
}
