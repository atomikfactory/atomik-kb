// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.

export { LoroSyncPlugin } from "./sync-plugin";
export {
  loroSyncPluginKey,
  type LoroSyncPluginProps,
  type LoroSyncPluginState,
  // atomik-kb patch (e).
  type LoroSyncReadOnly,
} from "./sync-plugin-key";
export {
  createNodeFromLoroObj,
  updateLoroToPmState,
  // atomik-kb patch (e).
  findUnknownNames,
  ROOT_DOC_KEY,
  NODE_NAME_KEY,
  CHILDREN_KEY,
  ATTRIBUTES_KEY,
  type LoroNodeMapping,
  type LoroDocType,
  type LoroChildrenListType,
  type LoroNodeContainerType,
  type LoroNode,
  type LoroContainer,
  type LoroType,
} from "./lib";
// atomik-kb patch (c2), ADR-0050: the cursor plugin and its store are back (not
// upstream's `awareness.ts`: Loro's `Awareness` is deprecated for `EphemeralStore`).
export {
  createCursorPlugin,
  createDecorations,
  defaultCursorBuilder,
  defaultSelectionBuilder,
  loroCursorPluginKey,
  type CursorPluginOptions,
  type CursorState,
  type PresenceStore,
  type PresenceUser,
} from "./cursor/common";
export {
  CursorEphemeralStore,
  LoroEphemeralCursorPlugin,
  decodeCursorUpdate,
  type CursorWire,
  type EphemeralBy,
} from "./cursor/ephemeral";
export { LoroUndoPlugin, undo, redo, canUndo, canRedo } from "./undo-plugin";
export { loroUndoPluginKey, type LoroUndoPluginProps } from "./undo-plugin-key";
// atomik-kb patch (f).
export { configLoroTextStyle, getLoroTextStyle } from "./text-style";
