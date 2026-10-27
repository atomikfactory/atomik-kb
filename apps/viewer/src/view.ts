// What the viewer shows and how each answer of the transport changes it (M13 slice 4): plain data and one pure function,
// so that the states are tested without a browser. `Viewer.tsx` draws them.

import type { ReadOnlyPage } from "@atomik-kb/ui";

import type { OpenedPublication } from "./kb-wasm";
import { VIEWER_SPACE_ID } from "./ViewerTransport";
import type { LinkLoad } from "./ViewerTransport";

/** What the viewer shows. */
export type View =
  | { kind: "loading" }
  | { kind: "page"; page: ReadOnlyPage; revision: number; offline: boolean }
  /** The link is gone (revoked, expired, never existed), its key does not open, or the first request failed. */
  | { kind: "gone" | "invalid" | "failed" | "nokey" };

/** The part of a publication the page view needs besides its document. */
function pageOf(publication: OpenedPublication): ReadOnlyPage {
  return {
    pageId: publication.pageId,
    spaceId: VIEWER_SPACE_ID,
    title: publication.title,
    ...(publication.icon === undefined ? {} : { icon: publication.icon }),
    ...(publication.cover === undefined ? {} : { cover: publication.cover, coverY: publication.coverY }),
  };
}

/** What `load` (the first one, or a poll) does to the view. */
export function nextView(current: View, load: LinkLoad): View {
  switch (load.status) {
    case "ready":
      // The view is made again for every publication that is handed over (a newer one, or the same one once more).
      return { kind: "page", page: pageOf(load.publication), revision: current.kind === "page" ? current.revision + 1 : 1, offline: false };
    case "unchanged":
      return current.kind === "page" && current.offline ? { ...current, offline: false } : current;
    case "gone":
      return { kind: "gone" };
    case "invalid":
      return { kind: "invalid" };
    case "unavailable":
      if (current.kind === "page") {
        return current.offline ? current : { ...current, offline: true };
      }
      // A link that was gone stays gone while the network is down; before anything was shown it is a failure to load.
      return current.kind === "gone" ? current : { kind: "failed" };
  }
}
