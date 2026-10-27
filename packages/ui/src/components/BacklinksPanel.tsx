import type { Backlink, PageId } from "@atomik-kb/core-client";

import { t, tp } from "../i18n";
import { openGestures } from "../page-open";
import { useAppStore } from "../store-context";
import { PageIcon } from "./PageIcon";

interface BacklinkGroup {
  pageId: PageId;
  first: Backlink;
  items: Backlink[];
}

/** Backlinks grouped by referencing page, in the order the pages first appear. */
function groupByPage(items: readonly Backlink[]): BacklinkGroup[] {
  const groups = new Map<PageId, BacklinkGroup>();
  for (const item of items) {
    const group = groups.get(item.pageId);
    if (group === undefined) {
      groups.set(item.pageId, { pageId: item.pageId, first: item, items: [item] });
    } else {
      group.items.push(item);
    }
  }
  return [...groups.values()];
}

/**
 * "Linked from N pages" under the open page: each referencing page with the blocks that
 * reference this one. A click opens the page (switching space when needed) and scrolls
 * to the block; Ctrl/Cmd + click or a middle click opens it in a background tab (ADR-0034). Hidden
 * while nothing links here.
 */
export function BacklinksPanel({ pageId }: { pageId: PageId }) {
  const backlinks = useAppStore((state) => state.backlinks);
  const navigateTo = useAppStore((state) => state.navigateTo);
  if (backlinks?.pageId !== pageId || backlinks.items.length === 0) {
    return null;
  }
  const groups = groupByPage(backlinks.items);

  return (
    <section className="backlinks" aria-labelledby="backlinks-title">
      <h2 className="backlinks-title" id="backlinks-title">
        {tp("backlinks.title", groups.length)}
      </h2>
      <ul className="backlinks-list">
        {groups.map(({ pageId: sourceId, first, items }) => {
          const title = first.title === "" ? t("page.untitled") : first.title;
          return (
            <li key={sourceId} className="backlink">
              <button
                type="button"
                className="backlink-page"
                {...openGestures<HTMLButtonElement>((disposition) => {
                  navigateTo({ pageId: sourceId, spaceId: first.spaceId, disposition });
                })}
              >
                <PageIcon icon={first.icon} className="backlink-icon" />
                <span className="backlink-title">{title}</span>
              </button>
              <ul className="backlink-blocks">
                {items.map((item) => (
                  <li key={item.blockId}>
                    <button
                      type="button"
                      className="backlink-block"
                      aria-label={t("backlinks.block", { title, text: item.text })}
                      {...openGestures<HTMLButtonElement>((disposition) => {
                        navigateTo({ pageId: sourceId, spaceId: item.spaceId, blockId: item.blockId, disposition });
                      })}
                    >
                      {item.text === "" ? t("backlinks.emptyBlock") : item.text}
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
