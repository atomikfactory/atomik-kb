// What a list or a table says when it has no rows ("No pages yet", "No pages with this tag", "No pages match the filters"),
// handed from the node view to the layout that draws the rows. `PageListLayoutProps` (ADR-0054) carries the rows, not
// why there are none, and the text belongs where the layout puts it (in place of the list, under the table's grid and
// its "New page" row), so the node view provides it through this context: `null` while there are rows, while a tag
// source is still being asked for its pages, and for the layouts that say nothing (the board draws its empty columns).
//
// Owner: agent B.

import { createContext } from "react";

export const PageListEmptyContext = createContext<string | null>(null);
