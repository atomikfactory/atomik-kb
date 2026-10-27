// The chain from a space down to a page: the space's name, each ancestor page (its icon and
// title, a button that opens it, in a background tab with Ctrl/Cmd + click or a middle click) and the
// page itself, separated by chevrons. Read from the app's structure store, so it follows renames,
// icon changes and moves of any page of the chain. Used by the breadcrumb block of the editor, and, as
// `PagePath` without the page itself, by the page header (ADR-0040).

import type { PageId, PageInfo, SpaceInfo, TreeView } from "@atomik-kb/core-client";
import { useMemo } from "react";

import { t } from "../i18n";
import { openGestures } from "../page-open";
import { useAppStore } from "../store-context";
import { pathTo } from "../tree";
import { ChevronIcon } from "./icons";
import { PageIcon } from "./PageIcon";

/** Where a page sits: its space's name, the pages above it from the top down, and the page. */
export interface BreadcrumbTrail {
  space: string;
  ancestors: readonly PageInfo[];
  page: PageInfo;
}

/**
 * The trail of `pageId` in `tree`, or `null` while the tree is not loaded or does not hold the
 * page (another space, or the trash).
 */
export function breadcrumbTrail(
  tree: TreeView | null,
  spaces: readonly SpaceInfo[],
  pageId: PageId,
): BreadcrumbTrail | null {
  const path = tree === null ? null : pathTo(tree.roots, pageId);
  const page = path?.at(-1);
  if (tree === null || path === null || page === undefined) {
    return null;
  }
  return {
    space: spaces.find((space) => space.id === tree.spaceId)?.name ?? "",
    ancestors: path.slice(0, -1),
    page,
  };
}

function Crumb({ page }: { page: PageInfo }) {
  return (
    <>
      <PageIcon icon={page.icon} className="breadcrumb-icon" />
      <span className="breadcrumb-title">{page.title === "" ? t("page.untitled") : page.title}</span>
    </>
  );
}

function Separator() {
  return (
    <span className="breadcrumb-separator" aria-hidden="true">
      <ChevronIcon />
    </span>
  );
}

/** The trail of the page `pageId` in the tree of the current space, or `null` while the page is not in it. */
export function usePageTrail(pageId: PageId): BreadcrumbTrail | null {
  const tree = useAppStore((state) => state.tree);
  const spaces = useAppStore((state) => state.spaces);
  return useMemo(() => breadcrumbTrail(tree, spaces, pageId), [tree, spaces, pageId]);
}

interface TrailListProps {
  trail: BreadcrumbTrail;
  label: string;
  /** Draws the page itself as the last, current crumb (the breadcrumb block does; the page header has its title). */
  showPage: boolean;
  className?: string;
}

/** The chain: the space's name, each ancestor as a button that opens it, and the page when `showPage`. */
function TrailList({ trail, label, showPage, className }: TrailListProps) {
  const navigateTo = useAppStore((state) => state.navigateTo);
  const hasSpace = trail.space !== "";
  return (
    <nav className={className === undefined ? "breadcrumbs" : `breadcrumbs ${className}`} aria-label={label}>
      <ol className="breadcrumb-list">
        {hasSpace && (
          <li className="breadcrumb-item breadcrumb-space">
            <span className="breadcrumb-title">{trail.space}</span>
          </li>
        )}
        {trail.ancestors.map((ancestor, index) => (
          <li key={ancestor.id} className="breadcrumb-item">
            {(hasSpace || index > 0) && <Separator />}
            <button
              type="button"
              className="breadcrumb-link"
              {...openGestures<HTMLButtonElement>((disposition) => {
                navigateTo({ pageId: ancestor.id, spaceId: ancestor.spaceId, disposition });
              })}
            >
              <Crumb page={ancestor} />
            </button>
          </li>
        ))}
        {showPage && (
          <li className="breadcrumb-item breadcrumb-current" aria-current="page">
            {(hasSpace || trail.ancestors.length > 0) && <Separator />}
            <Crumb page={trail.page} />
          </li>
        )}
      </ol>
    </nav>
  );
}

/** The trail of the page `pageId` of the current space; draws nothing when it is not in the tree. */
export function Breadcrumbs({ pageId }: { pageId: PageId }) {
  const trail = usePageTrail(pageId);
  return trail === null ? null : <TrailList trail={trail} label={t("breadcrumb.label")} showPage />;
}

/**
 * The path in the page header (ADR-0040): the space, then the pages above the page, on one line above
 * its title (which stands for the page). Draws nothing for a top-level page, which has no ancestor, nor
 * while the page is not in the tree.
 */
export function PagePath({ pageId }: { pageId: PageId }) {
  const trail = usePageTrail(pageId);
  if (trail === null || trail.ancestors.length === 0) {
    return null;
  }
  return <TrailList trail={trail} label={t("pagePath.label")} showPage={false} className="page-path" />;
}
