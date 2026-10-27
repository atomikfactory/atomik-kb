// View-only links as the client sees them (Milestone 13, slice 4; ADR-0051, `docs/security/sharing.md` section 10): the
// limits and the policy of `kb_types::sharing` the share menu and the web viewer keep to, and nothing else. The link's
// key lives in the URL fragment and the publication behind it is sealed under it, so the page only ever holds the URL
// (`ViewLinkInfo.url`) and the list of links (`CoreClient.viewLinkList`); the viewer (`apps/viewer`) is another app.

/** How long after a change to its page a member's device waits before it refreshes the page's links (`LINK_REFRESH_DEBOUNCE_MS`). */
export const LINK_REFRESH_DEBOUNCE_MS = 60_000;
/** Seconds between the viewer's polls of its publication (`LINK_POLL_SECONDS`). */
export const LINK_POLL_SECONDS = 60;
/** The longest expiry a view link may be given, in days (`LINK_EXPIRY_MAX_DAYS`); `viewLinkCreate` leaves it out for none. */
export const LINK_EXPIRY_MAX_DAYS = 365;
/** The path of the viewer on a server (`VIEWER_PATH`); a link's URL is the server, this path, `#` and the key. */
export const VIEWER_PATH = "/view";
/**
 * The `Content-Security-Policy` of `/view` (`VIEWER_CSP`, ADR-0051): the viewer's own scripts and WebAssembly, requests to its own
 * origin only, images from its own origin and from the blobs it decrypts. `apps/viewer/index.html` carries the same text as a
 * `<meta http-equiv>` policy, and its test compares the two.
 */
export const VIEWER_CSP =
  "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:";
