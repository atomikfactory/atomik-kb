// What a dashboard asks of the block that holds it, besides the props every layout takes (ADR-0055,
// `docs/architecture/dashboards.md` section 3). `PageListView` provides it around the layout it draws.
//
// - **The registry's components.** An embedded view widget draws another view with the component the registry
//   (`page-list-layouts.tsx`) names for its layout. The dashboard is itself an entry of that registry, so it cannot import
//   it (the two would import each other): the block hands the lookup down instead.
// - **Opening a page from another view.** A click in an embedded view follows *that* view's `openIn` (the side peek, the
//   center peek, the page), not the dashboard's, and `openRow` needs the page that holds the block, which only the block knows.

import type { PageInfo } from "@atomik-kb/core-client";
import { createContext } from "react";

import type { OpenDisposition } from "../../store-tabs";
import type { PageListLayoutDef } from "../page-list-layouts";
import type { PageListLayout } from "../page-list-model";
import type { PageListViewSpec } from "../page-list-views";

export interface DashboardHost {
  /** The registry's entry of a layout (the embedded view draws its `Component`). */
  readonly layoutDef: (layout: PageListLayout) => Pick<PageListLayoutDef, "Component">;
  /** Opens a page as `view` says (its `openIn`) for the gesture `disposition`. */
  readonly openInView: (view: PageListViewSpec, page: PageInfo, disposition: OpenDisposition) => void;
}

/** `null` outside a database block: an embedded view cannot be drawn there. */
export const DashboardHostContext = createContext<DashboardHost | null>(null);
