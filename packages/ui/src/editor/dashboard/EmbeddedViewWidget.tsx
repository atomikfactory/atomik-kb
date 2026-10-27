// The embedded view widget of a dashboard (ADR-0055, `docs/architecture/dashboards.md` section 3): another view of the same
// block drawn inside the dashboard by the component its layout names in the registry, with its own pipeline (`useViewRows`):
// the source's pages, the dashboard's filter, the view's own filter, the view's sort. It behaves as the view does in its own tab:
// a change of its settings (a table's column width, a calendar's month) patches *that* view (`onUpdateOtherView`), a page
// opens as *that* view's `openIn` says (`DashboardHost.openInView`), and it may make pages like the view does: "New" makes a page from
// *that* view's default template (ADR-0056) and the menu next to it sets that view's default. A view that was deleted,
// or that is a dashboard (a dashboard never shows one), is drawn as "This view was deleted" with a way to remove the widget.

import { useContext, useMemo } from "react";

import { t } from "../../i18n";
import { EMPTY_FILTER_GROUP } from "../page-list-filter-tree";
import { resolveDefaultTemplate, useDatabaseTemplates } from "../database-templates";
import { PageListEmptyContext } from "../page-list-empty";
import type { PageListLayoutProps } from "../page-list-layout-types";
import { DEFAULT_PAGE_LIST_SORT } from "../page-list-model";
import type { PageListViewSpec } from "../page-list-views";
import { useViewRows } from "../use-view-rows";
import { DashboardHostContext } from "./dashboard-host";

/** The pipeline of a widget that has no view to show: it runs and finds nothing to draw. */
const NO_VIEW: Pick<PageListViewSpec, "filter" | "sorts"> = { filter: EMPTY_FILTER_GROUP, sorts: [DEFAULT_PAGE_LIST_SORT] };

export interface EmbeddedViewWidgetProps {
  /** The view shown; `undefined` when it is gone (or is a dashboard). */
  readonly target: PageListViewSpec | undefined;
  /** The dashboard's own props: the source's pages, the schema, the editor's state and the handlers of the block. */
  readonly dashboard: PageListLayoutProps;
  /** Removes the widget (offered for a view that is gone, in an editable editor). */
  readonly onRemove: () => void;
}

/** The view a widget shows, or the message that it is gone. */
export function EmbeddedViewWidget({ target, dashboard, onRemove }: EmbeddedViewWidgetProps) {
  const host = useContext(DashboardHostContext);
  const database = useDatabaseTemplates();
  const { schema, spaceId, editable, view: dashboardView } = dashboard;
  // The pages before any filter; without them (props of an older caller) the dashboard's rows are all there is, and its filter was applied to them.
  const source = dashboard.sourcePages ?? dashboard.rows;
  const outerFilter = dashboard.sourcePages === undefined ? undefined : dashboardView.filter;
  const { rows } = useViewRows(source, target ?? NO_VIEW, schema, spaceId === "" ? null : spaceId, outerFilter);
  const createPage = dashboard.onCreatePage;
  const properties = target?.properties;
  const columns = useMemo(
    () => (properties ?? []).flatMap((id) => schema.find((def) => def.id === id) ?? []),
    [properties, schema],
  );

  if (target === undefined) {
    return (
      <div className="page-dashboard-message">
        <p>{t("dashboard.view.deleted")}</p>
        {editable && (
          <button type="button" className="page-list-button" onClick={onRemove}>
            {t("dashboard.widget.remove")}
          </button>
        )}
      </div>
    );
  }
  if (host === null) {
    return (
      <div className="page-dashboard-message">
        <p>{t("dashboard.widget.unknown")}</p>
      </div>
    );
  }
  const { Component } = host.layoutDef(target.layout);
  const emptyText = rows.length > 0 ? null : source.length > 0 ? t("pageList.empty.filtered") : t("pageList.empty.children");
  return (
    <div className="page-dashboard-embedded" data-layout={target.layout}>
      <PageListEmptyContext value={emptyText}>
        <Component
          rows={rows}
          view={target}
          schema={schema}
          columns={columns}
          spaceId={spaceId}
          editable={editable}
          today={dashboard.today}
          onOpen={(page, disposition) => {
            host.openInView(target, page, disposition);
          }}
          onUpdateView={(patch) => {
            dashboard.onUpdateOtherView?.(target.id, patch);
          }}
          onCreatePage={
            createPage === null
              ? null
              : (init) => {
                  // The block resolves an absent template against the active view (the dashboard); this widget names its own view's.
                  const templateId = init?.templateId === undefined ? resolveDefaultTemplate(target, database?.templates ?? []) : init.templateId;
                  createPage({ ...init, templateId });
                }
          }
          creating={dashboard.creating}
        />
      </PageListEmptyContext>
    </div>
  );
}
