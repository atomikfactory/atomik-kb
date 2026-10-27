# Vendored: loro-prosemirror

- Upstream: <https://github.com/loro-dev/loro-prosemirror>
- Version: 0.4.4, commit `0e036a9` (merge of PR #83, release 0.4.4, 2026-08-22)
- License: MIT, Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024 LORO. The full text
  is in `LICENSE-loro-prosemirror`; every file carries a header pointing to it.
- Why vendored: ADR-0011 and ADR-0014 (we own the binding until upstream is stable;
  fixes are offered upstream).

## Files

| Here | Upstream |
|---|---|
| `lib.ts` | `src/lib.ts` |
| `sync-plugin.ts` | `src/sync-plugin.ts` |
| `sync-plugin-key.ts` | `src/sync-plugin-key.ts` |
| `undo-plugin.ts` | `src/undo-plugin.ts` |
| `undo-plugin-key.ts` | `src/undo-plugin-key.ts` |
| `text-style.ts` | `src/text-style.ts` |
| `cursor/common.ts` | `src/cursor/common.ts` (the conversions, and the cursor plugin again, see patches c and c2) |
| `cursor/ephemeral.ts` | `src/cursor/ephemeral.ts` (back, see patch c2) |
| `index.ts` | `src/index.ts` |

Not vendored: `src/cursor/awareness.ts` (Loro's deprecated `Awareness`, patch c; the store is
`cursor/ephemeral.ts`), the upstream tests (ours are `../binding.test.ts`, `../binding-fuzz.test.ts`, which runs with
the cursor plugin in both views, and `../cursor-plugin.test.ts`).

Line endings are LF (the upstream checkout had CRLF). Review with
`diff -u --strip-trailing-cr <upstream>/src/<file> <file>`; every change in the code is
marked with a comment `atomik-kb patch (<letter>)`.

## Patches

**(a) One ProseMirror.** Imports come from `@tiptap/pm/model`, `@tiptap/pm/state` and
`@tiptap/pm/view` instead of `prosemirror-*`, so the binding and Tiptap share one
ProseMirror instance. Imports used only as types became `import type` (lint).

**(b) No timestamps.** The `update-state` commit no longer passes
`timestamp: Date.now()` (privacy, ADR-0011: no timestamps in documents).

**(c) No collaboration plugins.** `cursor/awareness.ts`, `cursor/ephemeral.ts` and the
presence part of `cursor/common.ts` (`createCursorPlugin`, `createDecorations`, the
presence store types) were not vendored, and `index.ts` did not export them. The
selection <-> Loro cursor conversions in `cursor/common.ts` stay (the sync and undo
plugins use them). `lib0` is kept as a dependency (`simpleDiff`, `equalityDeep`).
**Partly reverted by (c2)**: everything of this patch stays except `cursor/ephemeral.ts` and the
presence part of `cursor/common.ts`, which are back; `cursor/awareness.ts` is still not vendored.

**(c2) The cursor plugin is back (M13 slice 3, ADR-0050).** What came back, in `cursor/`:
- `ephemeral.ts`: `CursorEphemeralStore` (a Loro `EphemeralStore` of cursors: `setLocal`, `getLocal`, `getAll`,
  `subscribeBy`; the update of a presence frame is its encoding) and `LoroEphemeralCursorPlugin(store, options)`, which
  puts the store behind the plugin (`ephemeralStoreAdapter`).
- `common.ts` (below the conversions of patch c): the `PresenceStore`, `CursorState` and `CursorPluginOptions` types,
  `createCursorPlugin` (the plugin: decorations from the other devices' states, this view's selection as Loro cursors
  while the editor has focus), `createDecorations`, `defaultCursorBuilder` (the caret), `defaultSelectionBuilder` (the
  range) and `loroCursorPluginKey`. `index.ts` exports them again.

  **Provenance, to check before the next upstream update.** The upstream source of those two files was not at hand when
  this was written (the package is vendored, not installed, so it is not in `node_modules`, and nothing was downloaded):
  they are written to upstream's names, exports and structure at 0.4.4 (`0e036a9`), not copied from it. Run
  `diff -u --strip-trailing-cr <upstream>/src/cursor/ephemeral.ts cursor/ephemeral.ts` (and the same for the presence
  part of `common.ts`) when the checkout is available, take upstream's version wherever the differences below do not
  need ours, and fix this paragraph.

  What differs from upstream's design, each for a reason of presence between members of a shared space (every change
  carries a comment `atomik-kb patch (c2)`):
  - **Names and colours are not the frame's.** Upstream draws a device with the `user` its state carries. Here the host
    fills `user` from membership (the member's profile name, the colour of their join order, `PresenceInfo`), a remote
    device's state is keyed by the device the core verified (never by a key the sender picked, so a frame cannot
    overwrite another device's cursor), and `setRemote` keeps the two cursors only: anything else in the update is
    dropped, and so is a cursor that is empty or over 256 bytes. A device's own update carries no `user` either.
  - **A frame is decoded on a store of its own** with a week's timeout: Loro compares the sender's timestamp with the
    receiver's clock, so a clock that is minutes off would hide the state (`decodeCursorUpdate`; garbage, an empty
    update and one over `PRESENCE_UPDATE_MAX_BYTES` decode to nothing).
  - **The timeout is exact.** A remote state is removed 30 seconds after its latest frame by a timer of its own (Loro's
    cleanup only runs at half the timeout), and `subscribeBy` says who changed the store (`local`, `import`,
    `timeout`); the plugin redraws for the last two. `encodeLocal` sets this device's state again before encoding, so
    a caret that does not move still makes frames that say it is there.
  - **A cursor this replica cannot place is not drawn** (`isResolvable`: its container is gone or its position is not
    here yet). Upstream resolved it anyway, which logs an error and puts the caret at the start of the page.
  - **Decorations are recomputed** for a remote change of the document, the first load and a change of the other devices'
    cursors, and mapped through the transaction for a local edit (as upstream), by a `{ refresh: true }` meta.
  - **Order.** The plugin reads the sync plugin's state in its own `apply`, so it must come after it in the list: the
    Tiptap extension (`../presence-cursors.ts`) has priority 90, below `loroBinding`'s 100 (Tiptap lists higher
    priorities first), and `presence-ui.test.tsx` asserts the order.
  - **Not an edit.** A remote cursor never writes to the Loro document; the fuzz run draws the cursors of each replica
    in the other (`relayCursors`) and checks that every decoration lies inside the document and, where a selection
    converts to a cursor and back to itself, that the caret is at the other replica's head.

**(d) Upstream PRs #84 and #85.**
- #84: `updateLoroMapAttributes` deletes a key only when the Loro map has it. Deleting
  an absent key still records a delete op, so every update of a node with a `null`
  attribute (a code block's `language`, a page reference's `blockId`) emitted one.
- #85: a remote change restores the selection in the same transaction as the change
  (upstream dispatched a second transaction from a `setTimeout`, so a keystroke in
  between landed at a stale position). The selection is taken as Loro cursors at every
  view update, while ProseMirror and Loro agree, and resolved after the import (upstream
  built the cursors from the old view against the already changed document, so text
  inserted before the cursor moved it). `safeSelection` is `safeSetSelection` without
  the dispatch.

**(e) The init race (upstream issue #77) cannot happen.**
- `init` treats a Loro document without a root node as a programming error: it logs,
  makes the editor read-only and writes nothing (upstream wrote the editor's default
  document into it, racing with content that arrives later). Our documents always have
  a root: the core creates root + an empty paragraph, and the editor mounts only after
  the snapshot was imported (`../PageEditor.tsx`).
- Nothing is written to Loro before `init` replaced the editor's initial content with
  the Loro document (the `doc-changed` handler requires `docSubscription`).
- Read-only state: `LoroSyncPluginProps.readOnly` and `LoroSyncPluginState.readOnlyReason`
  (`empty-document`, `unknown-content`, `requested`). While it is set, the `editable`
  prop is false and nothing is written; remote changes still update the view. The page lock
  (ADR-0040, `../page-lock.ts`) sets `requested` after the view exists, with an `update-state`
  transaction (upstream's own meta), and clears only that reason when the page is unlocked.
- `findUnknownNames(schema, root)`: node and mark names the schema does not know. The
  binding cannot represent them (it would drop them and the next write would delete
  them), so `init` and every remote change check for them and turn read-only first
  (ADR-0016: an editor that meets content outside its schema fails closed).

**(f) Mark expansion table.** `getLoroTextStyle` is exported (tests assert the derived
table). The derivation is upstream's (`inclusive !== false` -> `after`, else `none`);
our schema sets `inclusive: false` on `code` and `link`, which gives the shape v1 table.

**(g) `null` mark attributes are not stored.** `nodeMarksToAttributes` drops `null`
values, as node attributes already did, so a link is stored as `{ href }` (shape v1,
ADR-0016). Without it, a link written by the core (`{ href }`) was rewritten with
`target: null, rel: null, ...` on the first edit of its paragraph.

**(r) Content that merges left outside the schema.**
- `createNodeFromLoroObj` no longer drops a node whose content the schema rejects
  (concurrent edits can leave, for example, a list item starting with a callout, or an
  empty list): it is completed where ProseMirror can (`createAndFill`) or created as it
  is. Upstream dropped it from the view, and the next write deleted it from the
  document. Unknown node names still fail (and patch e makes the editor read-only).
- The binding's own transactions (initial load, remote changes, undo, redo) are not
  written back to Loro. They show what Loro holds; writing them back made every replica
  write its own completion of merged content (duplicate fillers). The document node they
  produce is registered in the mapping instead (`registerDocNode`), which the write-back
  used to do as a side effect.

**(t) Strict TypeScript.** `!` where an index is known to be in range
(`noUncheckedIndexedAccess`), typed casts instead of `as any`, an explicit
`return undefined` (`noImplicitReturns`). ESLint: `src/editor/loro-binding` turns off
the `no-unsafe-*` assignment/argument rules, `no-unnecessary-type-assertion` and
`prefer-const`, and allows unused `_` parameters (`eslint.config.js`), to keep the diff
to upstream small.

**(u) Views stop listening.** The sync plugin's view `destroy` unsubscribes from the
document (upstream only cleared the init timeout, so a destroyed view stayed
subscribed).

**(v) Undo flag.** `undo`/`redo` clear `isUndoing` as soon as `undo()`/`redo()` return
(loro-crdt delivers the undo's events before that), not in a `setTimeout`: a local edit
dispatched before the timeout was never written to Loro.

**(w) No error log for expected misses.** `absolutePositionToCursor` no longer logs
"Cannot find the loroNode": the undo plugin asks for every transaction, including the
ones appended between a change and its write to Loro (UniqueID's ids after Enter or a
paste), when the new nodes are not in the mapping yet.

**(x) Selections the Loro cursors cannot carry.** A remote change (and the selection
restore after undo/redo, `syncCursorsToPmSelection`) uses the Loro cursors only while
their text container exists. It is gone when another replica deleted the block, or
wrapped, lifted or moved it (the binding writes those as a delete and an insert);
resolving such a cursor walked containers this view never mapped (upstream logged the
children list and put the caret somewhere else). Then, and for selections outside text
(a selected horizontal rule), `fallbackSelection` takes over: a text position stays in
the textblock with the same `id` attribute, at the same offset, when the new document has
one; otherwise the selection is mapped across the range where the old and new documents
differ (`changedRange`). Upstream mapped these selections through the whole-document
replace, which moved them to the end of the document.

**(y) Events outside the editor's tree.** The sync plugin listened to every container of the
document and rebuilt the whole ProseMirror document for each remote batch. A batch whose events
all belong to another root container than `doc` (the whiteboard elements of ADR-0033, in the
root map `whiteboards`) is now ignored, so a drawing that streams in from another device does
not redraw the page or move the caret.

## Known limitations (unchanged from upstream)

- Marks on inline nodes other than text (a page reference inside bold text) are not
  stored: shape v1 has marks on text only. The editor removes them right away
  (`TextOnlyMarks` in `../extensions.ts`).
- A block moved by drag and drop is deleted and inserted (ADR-0011, "moved blocks win").
- Remote changes replace the whole document in ProseMirror (unchanged nodes are reused,
  so the DOM is not redrawn).

## Updating

Diff the new upstream release against commit `0e036a9`, apply it here, keep every
`atomik-kb patch` (or drop the ones upstream fixed; for (c2) see its provenance note), update this file, and run
`pnpm --filter @atomik-kb/ui test` (the binding tests, the cursor plugin tests and the fuzz suite, which runs with the
cursor plugin; once with `KB_FUZZ_ITERATIONS=10000`).
