// The feed layout of a database view (ADR-0058, `docs/architecture/database-power-ups.md` section 6.1): one column of cards, each
// with the page's icon and title (a button that opens the page; the whole card opens it too), the properties the view shows (drawn
// as the gallery's cards draw them) and the **excerpt**, the first lines of the page's text (at most 280 characters from the core,
// three lines on screen, line breaks kept), and a "New page" card at the end.
//
// The rows arrive filtered and sorted (the feed has no order of its own; a new feed view sorts by created time, newest first,
// through its registry entry's `initialPatch`). The card asks to open a page through `onOpen` with the disposition of the
// gesture, read-only editors included, and a page is made through `onCreatePage`. The feed draws 25 cards at first and adds 25
// each time "Show more" is pressed or the last card comes near the viewport; the excerpts are asked of the store for the cards
// drawn, which asks the core once for each batch it does not know (`store-feed.ts`).
//
// Owner: agent F of the database power-ups batch. The pure rules are `page-feed-model.ts`, the asking and the scroll watch are
// `page-feed-hooks.ts`.

import { useCallback, useContext, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, Ref } from "react";
import type { PageInfo, PropertyDef } from "@atomik-kb/core-client";

import { PlusIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { dispositionFromKey, openGestures } from "../page-open";
import type { OpenDisposition } from "../store-tabs";
import { FEED_PAGE_SIZE, hasMoreCards, nextRequested, shownRows } from "./page-feed-model";
import { usePageExcerpts, useReachedEnd } from "./page-feed-hooks";
import { NewPageMenu } from "./NewPageMenu";
import { PageListEmptyContext } from "./page-list-empty";
import type { PageListLayoutProps } from "./page-list-layout-types";
import { valueOfPage } from "./page-list-model";
import { PageListValue } from "./PageListValue";

/** True for a mouse event that comes from the title button, which handles its own gestures. */
function fromTitle(event: MouseEvent<HTMLElement>): boolean {
  return event.target instanceof Element && event.target.closest(".page-feed-title") !== null;
}

interface CardProps {
  page: PageInfo;
  /** The first lines of the page's text; `undefined` while it is not known or the page has none. */
  excerpt: string | undefined;
  columns: readonly PropertyDef[];
  schema: readonly PropertyDef[];
  today: string;
  onOpen: (page: PageInfo, disposition: OpenDisposition) => void;
  /** Set on the last card only: the feed watches it to add the next page of cards. */
  cardRef: Ref<HTMLLIElement> | undefined;
}

function FeedCard({ page, excerpt, columns, schema, today, onOpen, cardRef }: CardProps) {
  const title = page.title === "" ? t("page.untitled") : page.title;
  const open = (disposition: OpenDisposition): void => {
    onOpen(page, disposition);
  };
  const button = openGestures<HTMLButtonElement>(open);
  const card = openGestures<HTMLLIElement>(open);
  const props = columns.flatMap((def) => {
    const value = valueOfPage(page, def.id);
    return value === undefined ? [] : [{ def, value }];
  });
  return (
    <li
      ref={cardRef}
      className="page-feed-card"
      data-page={page.id}
      onClick={(event) => {
        if (!fromTitle(event)) {
          card.onClick(event);
        }
      }}
      onAuxClick={(event) => {
        if (!fromTitle(event)) {
          card.onAuxClick(event);
        }
      }}
      onMouseDown={(event) => {
        if (!fromTitle(event)) {
          card.onMouseDown(event);
        }
      }}
    >
      <button
        type="button"
        className="page-feed-title"
        aria-label={t("pageList.feed.open", { title })}
        {...button}
        onKeyDown={(event: KeyboardEvent<HTMLButtonElement>) => {
          // Ctrl+Enter opens a tab in front (`dispositionFromKey`); the browser would send a click with Ctrl held, which means a background tab.
          if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
            event.preventDefault();
            open(dispositionFromKey(event));
          }
        }}
      >
        <PageIcon icon={page.icon} className="page-list-icon" />
        <span className="page-feed-name">{title}</span>
      </button>
      {props.length > 0 && (
        <div className="page-feed-props">
          {props.map(({ def, value }) => (
            <div key={def.id} className="page-feed-prop" role="group" aria-label={def.name} data-kind={def.kind}>
              <PageListValue def={def} value={value} today={today} schema={schema} />
            </div>
          ))}
        </div>
      )}
      {excerpt !== undefined && excerpt !== "" && <p className="page-feed-excerpt">{excerpt}</p>}
    </li>
  );
}

/** How many cards were asked for, for the view they were asked for (another view starts again at one page). */
interface Paging {
  readonly viewId: string;
  readonly requested: number;
}

/** The feed. */
export function PageFeedView({ rows, view, schema, columns, spaceId, today, onOpen, onUpdateView, onCreatePage, creating }: PageListLayoutProps) {
  // `editable` is not read: a read-only editor draws the same cards and opens pages; only `onCreatePage` (the "New page" card) is `null`.
  // What the node view says when there are no rows (it knows why: no pages, a tag nobody has, the filters); `null` while rows are on their way.
  const emptyText = useContext(PageListEmptyContext);
  const [paging, setPaging] = useState<Paging>({ viewId: view.id, requested: FEED_PAGE_SIZE });
  if (paging.viewId !== view.id) {
    // Another view is shown: it starts again at one page of cards (state adjusted while drawing, as React advises, not in an effect).
    setPaging({ viewId: view.id, requested: FEED_PAGE_SIZE });
  }
  const requested = paging.viewId === view.id ? paging.requested : FEED_PAGE_SIZE;
  const shown = useMemo(() => shownRows(rows, requested), [rows, requested]);
  const ids = useMemo(() => shown.map((page) => page.id), [shown]);
  const excerpts = usePageExcerpts(spaceId, ids);
  const more = hasMoreCards(requested, rows.length);
  const viewId = view.id;
  const total = rows.length;
  const showMore = useCallback(() => {
    setPaging((current) => ({
      viewId,
      requested: nextRequested(current.viewId === viewId ? current.requested : FEED_PAGE_SIZE, total),
    }));
  }, [viewId, total]);
  const lastCard = useRef<HTMLLIElement | null>(null);
  useReachedEnd(lastCard, more, showMore, shown.length);

  return (
    <div className="page-feed" role="group" aria-label={t("pageList.feed.label")}>
      {rows.length === 0 && emptyText !== null && <p className="page-list-empty">{emptyText}</p>}
      {shown.length > 0 && (
        <ul className="page-feed-list" aria-label={t("pageList.feed.list")}>
          {shown.map((page, index) => (
            <FeedCard
              key={page.id}
              page={page}
              excerpt={excerpts.get(page.id)}
              columns={columns}
              schema={schema}
              today={today}
              onOpen={onOpen}
              cardRef={index === shown.length - 1 ? lastCard : undefined}
            />
          ))}
        </ul>
      )}
      {more && (
        <button type="button" className="page-feed-more" onClick={showMore}>
          {t("pageList.feed.showMore")}
        </button>
      )}
      {onCreatePage !== null && (
        <div className="page-feed-card page-feed-new">
          <button
            type="button"
            className="page-feed-new-button"
            disabled={creating}
            onClick={() => {
              onCreatePage();
            }}
          >
            <PlusIcon />
            <span>{t("pageList.feed.newCard")}</span>
          </button>
          <NewPageMenu view={view} onCreatePage={onCreatePage} onUpdateView={onUpdateView} disabled={creating} />
        </div>
      )}
    </div>
  );
}
