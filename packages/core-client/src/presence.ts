// Presence in shared spaces as the client sees it (Milestone 13, slice 3; ADR-0050, `docs/security/sharing.md`
// section 9): the limits of `kb_types::sharing` the editor and the avatars keep to, and nothing else. The frames are
// sealed and signed by the core and relayed by the server without being read, so the page only ever holds the
// accepted ones (`PresenceUpdate`, `./transport`) and the list of who is where (`CoreClient.presenceList`).

/** Seconds without a frame after which a device's presence is gone (`PRESENCE_TIMEOUT_SECONDS`). */
export const PRESENCE_TIMEOUT_SECONDS = 30;
/** The most presence updates the UI sends per second (`PRESENCE_UPDATES_PER_SECOND`); one update is one frame. */
export const PRESENCE_UPDATES_PER_SECOND = 10;
/** The most bytes of an ephemeral-store update a frame carries (`PRESENCE_UPDATE_MAX_BYTES`); more is `InvalidInput`. */
export const PRESENCE_UPDATE_MAX_BYTES = 3_072;
