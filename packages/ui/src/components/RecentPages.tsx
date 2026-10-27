import { useId } from "react";

import { formatAgo, formatTimestamp } from "../format";
import { t } from "../i18n";
import { openGestures } from "../page-open";
import { useRecentPages } from "../recent-pages";
import { useAppStore } from "../store-context";
import { PageIcon } from "./PageIcon";

/** Pages the Home screen lists. */
export const HOME_RECENT_LIMIT = 12;

/**
 * "Recently visited" on the Home screen (ADR-0029): the last pages opened on this device with
 * their icon, title, space and how long ago; a click opens the page (Ctrl/Cmd + click or a middle click in a
 * background tab, ADR-0034). Nothing while there are none.
 */
export function RecentPages() {
  const recents = useRecentPages(HOME_RECENT_LIMIT);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const headingId = useId();

  if (recents.length === 0) {
    return null;
  }

  return (
    <section className="recent" aria-labelledby={headingId}>
      <h2 className="subtitle" id={headingId}>
        {t("recent.visited")}
      </h2>
      <ul className="recent-list" aria-label={t("recent.visitedList")}>
        {recents.map((recent) => {
          const untitled = recent.title === "";
          const stamp = formatTimestamp(recent.openedAt);
          return (
            <li key={recent.pageId}>
              <button
                type="button"
                className="recent-item"
                {...openGestures<HTMLButtonElement>((disposition) => {
                  navigateTo({ pageId: recent.pageId, spaceId: recent.spaceId, disposition });
                })}
              >
                <PageIcon icon={recent.icon} className="recent-icon" />
                <span className={untitled ? "recent-title is-untitled" : "recent-title"}>
                  {untitled ? t("page.untitled") : recent.title}
                </span>
                <span className="recent-space">{recent.spaceName}</span>
                {stamp !== null && (
                  <time className="recent-time" dateTime={stamp.iso}>
                    {formatAgo(recent.openedAt)}
                  </time>
                )}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
