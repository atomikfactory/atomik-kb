// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.

import type { Loro } from "loro-crdt";
// atomik-kb patch (a): ProseMirror from @tiptap/pm, the instance Tiptap uses.
import type { Schema } from "@tiptap/pm/model";

const LORO_TEXT_STYLE_CACHE = new WeakSet<Loro>();

// atomik-kb patch (f): exported, so tests can assert the derived table.
export function getLoroTextStyle(schema: Schema): {
  [mark: string]: { expand: "before" | "after" | "none" | "both" };
} {
  return Object.fromEntries(
    Object.entries(schema.marks).map(([markName, markType]) => [
      markName,
      // ProseMirror marks are inclusive unless the spec says otherwise, so
      // only an explicit `inclusive: false` should map to "none".
      { expand: markType.spec.inclusive !== false ? "after" : "none" },
    ]),
  );
}

export function configLoroTextStyle(doc: Loro, schema: Schema) {
  // Avoid reconfiguring the text style for the same Loro document.
  if (LORO_TEXT_STYLE_CACHE.has(doc)) {
    return;
  }
  LORO_TEXT_STYLE_CACHE.add(doc);

  doc.configTextStyle(getLoroTextStyle(schema));
}
