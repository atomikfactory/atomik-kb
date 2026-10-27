// The list layout of the list view block (ADR-0016, amendment of 2026-10-07; ADR-0054 for the layout registry): the
// pages of the view as rows, each with the page's icon and title (a button that opens the page) and the properties
// the view shows, read-only, then a "New page" button. A layout takes the props every layout takes
// (`page-list-layout-types.ts`): it asks to open a page through `onOpen` (the gesture decides between the peek, the
// page and another tab) and makes pages through `onCreatePage`; it writes nothing itself. The menu next to "New page"
// (`NewPageMenu`, ADR-0056) lists the source's templates. In an editable editor each row starts with the checkbox of the selection
// (`PageRowCheckbox`, ADR-0058), and at most 200 rows are drawn until "Show more" (`page-list-row-limit.ts`).

import { useContext } from "react";

import { PlusIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { openGestures } from "../page-open";
import { NewPageMenu } from "./NewPageMenu";
import { PageListEmptyContext } from "./page-list-empty";
import type { PageListLayoutProps } from "./page-list-layout-types";
import { valueOfPage } from "./page-list-model";
import { limitRows, ROW_LIMIT_STEP, useRowLimit } from "./page-list-row-limit";
import { PageListValue } from "./PageListValue";
import { PageRowCheckbox, selectLabel } from "./PageRowCheckbox";

/** The list: the layout of a view whose `layout` is `list` (and of one whose layout this version does not know). */
export function PageListRows({
  rows,
  view,
  schema,
  columns,
  today,
  onOpen,
  onUpdateView,
  onCreatePage,
  creating,
  selection,
}: PageListLayoutProps) {
  const emptyText = useContext(PageListEmptyContext);
  // At most 200 rows are drawn, "Show 200 more" adds 200 (ADR-0058); the selection, the counts and the CSV use every row.
  const limit = useRowLimit(view.id);
  const drawn = limitRows(rows, limit.limitOf());
  return (
    <>
      {rows.length > 0 ? (
        <ul className="page-list-rows" aria-label={t("pageList.rows")}>
          {drawn.map((page) => (
            <li key={page.id} className="page-list-row" data-selected={selection?.selected.has(page.id) === true ? "" : undefined}>
              {selection !== undefined && (
                <PageRowCheckbox
                  label={selectLabel(page.title)}
                  checked={selection.selected.has(page.id)}
                  onToggle={(range) => {
                    selection.onToggle(page.id, { range });
                  }}
                />
              )}
              <button
                type="button"
                className="page-list-link"
                {...openGestures<HTMLButtonElement>((disposition) => {
                  onOpen(page, disposition);
                })}
              >
                <PageIcon icon={page.icon} className="page-list-icon" />
                <span className="page-list-name">{page.title === "" ? t("page.untitled") : page.title}</span>
              </button>
              {columns.map((def) => {
                const value = valueOfPage(page, def.id);
                return value === undefined ? (
                  <span key={def.id} className="page-list-cell" data-empty="" aria-hidden="true" />
                ) : (
                  <span key={def.id} className="page-list-cell" role="group" aria-label={def.name} data-kind={def.kind}>
                    <PageListValue def={def} value={value} today={today} schema={schema} />
                  </span>
                );
              })}
            </li>
          ))}
        </ul>
      ) : (
        emptyText !== null && <p className="page-list-empty">{emptyText}</p>
      )}
      {drawn.length < rows.length && (
        <div className="page-list-more">
          <span className="page-list-more-count">{t("pageList.rows.shown", { shown: drawn.length, total: rows.length })}</span>
          <button
            type="button"
            className="page-list-more-button"
            onClick={() => {
              limit.showMore();
            }}
          >
            {t("pageList.rows.showMore", { count: Math.min(ROW_LIMIT_STEP, rows.length - drawn.length) })}
          </button>
        </div>
      )}
      {onCreatePage !== null && (
        <div className="database-new-group is-list">
          <button
            type="button"
            className="page-list-new"
            disabled={creating}
            onClick={() => {
              onCreatePage();
            }}
          >
            <PlusIcon />
            <span>{t("pageList.newPage")}</span>
          </button>
          <NewPageMenu view={view} onCreatePage={onCreatePage} onUpdateView={onUpdateView} disabled={creating} />
        </div>
      )}
    </>
  );
}
