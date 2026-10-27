// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.

import type { ContainerID, LoroDoc, Subscription } from "loro-crdt";
// atomik-kb patch (a): ProseMirror from @tiptap/pm, the instance Tiptap uses.
import { PluginKey } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import type { LoroDocType, LoroNodeMapping } from "./lib";

export const loroSyncPluginKey = new PluginKey<LoroSyncPluginState>(
  "loro-sync",
);

/**
 * atomik-kb patch (e): why the binding stopped writing to the Loro document.
 * `empty-document`: the Loro document had no root node when the view was
 * initialised (the editor never creates documents, upstream issue #77);
 * `unknown-content`: the document holds node or mark names outside the schema;
 * `requested`: the editor was created read-only.
 */
export interface LoroSyncReadOnly {
  reason: "empty-document" | "unknown-content" | "requested";
  /** Node names outside the schema (for `unknown-content`). */
  nodes?: string[];
  /** Mark names outside the schema (for `unknown-content`). */
  marks?: string[];
}

export interface LoroSyncPluginProps {
  doc: LoroDocType;
  mapping?: LoroNodeMapping;
  containerId?: ContainerID;
  /**
   * atomik-kb patch (e): start read-only. The binding then never writes to the
   * Loro document; remote changes still update the view.
   */
  readOnly?: boolean;
}

export interface LoroSyncPluginState extends LoroSyncPluginProps {
  changedBy: "local" | "import" | "checkout";
  mapping: LoroNodeMapping;
  snapshot?: LoroDoc | null;
  view?: EditorView;
  containerId?: ContainerID;
  docSubscription?: Subscription | null;
  /** atomik-kb patch (e): set once the binding refuses to write; never cleared. */
  readOnlyReason?: LoroSyncReadOnly | null;
}
