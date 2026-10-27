// React views of the editor's own nodes: the page reference chip, the page link card and the callout.

import type { PageId, PageInfo, TreeView } from "@atomik-kb/core-client";
import { NodeViewContent, NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useEffect, useState } from "react";
import type { MouseEvent } from "react";

import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { IconPicker } from "../components/IconPicker";
import { ChevronIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { openGestures } from "../page-open";
import type { AppState } from "../store";
import { useAppStore } from "../store-context";
import { DEFAULT_CALLOUT_ICON, isCalloutColor } from "./callout";
import { CalloutColorPicker } from "./CalloutColorPicker";

/** Page metadata by id for every page of a tree, built once per tree. */
const treeIndexes = new WeakMap<TreeView, Map<PageId, PageInfo>>();

export function indexOf(tree: TreeView): Map<PageId, PageInfo> {
  let index = treeIndexes.get(tree);
  if (index === undefined) {
    const built = new Map<PageId, PageInfo>();
    const visit = (nodes: TreeView["roots"]): void => {
      for (const node of nodes) {
        built.set(node.page.id, node.page);
        visit(node.children);
      }
    };
    visit(tree.roots);
    treeIndexes.set(tree, built);
    index = built;
  }
  return index;
}

/** What is known about a referenced page: the live tree first, then the lookup cache. */
export type ReferencedPage = { status: "loading" } | { status: "found"; page: PageInfo } | { status: "missing" };

export function selectReferencedPage(state: AppState, pageId: PageId): ReferencedPage {
  const live = state.tree === null ? undefined : indexOf(state.tree).get(pageId);
  if (live !== undefined) {
    return { status: "found", page: live };
  }
  return state.pageLookup[pageId] ?? { status: "loading" };
}

/** The current title (or "Untitled") of a referenced page, `null` while unknown or gone. */
export function referencedTitle(state: AppState, pageId: PageId): string | null {
  const page = selectReferencedPage(state, pageId);
  if (page.status !== "found") {
    return null;
  }
  return page.page.title === "" ? t("page.untitled") : page.page.title;
}

/** A referenced page, kept fresh: from the current space's tree, else read once from the core. */
export function useReferencedPage(pageId: PageId): ReferencedPage {
  // Two stable selections (a new object per read would re-render forever).
  const live = useAppStore((state) => (state.tree === null ? undefined : indexOf(state.tree).get(pageId)));
  const cached = useAppStore((state) => state.pageLookup[pageId]);
  const lookupPage = useAppStore((state) => state.lookupPage);
  const needsLookup = live === undefined && cached === undefined;
  useEffect(() => {
    if (needsLookup) {
      lookupPage(pageId);
    }
  }, [needsLookup, lookupPage, pageId]);
  if (live !== undefined) {
    return { status: "found", page: live };
  }
  return cached ?? { status: "loading" };
}

/**
 * The `pageRef` chip: the page's icon and live title ("Deleted page" once it is gone,
 * struck through while it is in the trash). A click opens the page, switching space when
 * needed, and scrolls to the referenced block if there is one. Ctrl/Cmd + click and a middle
 * click open it in a background tab, Ctrl/Cmd + Shift + click in a tab in front (ADR-0034); the
 * press of those gestures stays out of the editor, so the chip is not selected and the caret
 * does not move.
 */
export function PageRefView({ node }: ReactNodeViewProps) {
  const pageId = String(node.attrs.pageId);
  const blockId: unknown = node.attrs.blockId;
  const page = useReferencedPage(pageId);
  const navigateTo = useAppStore((state) => state.navigateTo);

  let title = t("pageRef.loading");
  let className = "page-ref";
  let tooltip: string | undefined;
  if (page.status === "missing") {
    title = t("pageRef.deleted");
    className += " is-deleted";
  } else if (page.status === "found") {
    title = page.page.title === "" ? t("page.untitled") : page.page.title;
    if (page.page.trashedAt !== undefined) {
      className += " is-trashed";
      tooltip = t("pageRef.trashed");
    }
  }
  const found = page.status === "found" ? page.page : null;
  const gestures = openGestures<HTMLElement>(
    (disposition) => {
      if (found !== null) {
        navigateTo({
          pageId: found.id,
          spaceId: found.spaceId,
          blockId: typeof blockId === "string" ? blockId : undefined,
          disposition,
        });
      }
    },
    { fromEditor: true },
  );

  return (
    <NodeViewWrapper
      as="span"
      className={className}
      role="link"
      aria-disabled={found === null}
      title={tooltip}
      {...gestures}
      onClick={(event: MouseEvent<HTMLElement>) => {
        event.preventDefault();
        gestures.onClick(event);
      }}
    >
      <PageIcon icon={found?.icon} className="page-ref-icon" />
      <span className="page-ref-title">{title}</span>
    </NodeViewWrapper>
  );
}

/**
 * The `pageLink` card, the block twin of the chip: the page's icon, its live title ("Unknown
 * page" when it cannot be found, struck through while it is in the trash) and a chevron. A
 * click opens the page, switching space when needed, and scrolls to the linked block if the
 * card names one. Ctrl/Cmd + click and a middle click open the page in a background tab, Ctrl/Cmd +
 * Shift + click in a tab in front (ADR-0034), without selecting the card. A read-only editor
 * navigates too: opening a page writes nothing. The card is selected like any atom (arrow keys, or
 * the block handle) and Backspace or Delete removes it.
 */
export function PageLinkView({ node }: ReactNodeViewProps) {
  const pageId = String(node.attrs.pageId);
  const blockId: unknown = node.attrs.blockId;
  const page = useReferencedPage(pageId);
  const navigateTo = useAppStore((state) => state.navigateTo);

  let title = t("pageRef.loading");
  let className = "page-link";
  let tooltip: string | undefined;
  if (page.status === "missing") {
    title = t("pageLink.unknown");
    className += " is-unknown";
  } else if (page.status === "found") {
    title = page.page.title === "" ? t("page.untitled") : page.page.title;
    if (page.page.trashedAt !== undefined) {
      className += " is-trashed";
      tooltip = t("pageRef.trashed");
    }
  }
  const found = page.status === "found" ? page.page : null;
  const gestures = openGestures<HTMLElement>(
    (disposition) => {
      if (found !== null) {
        navigateTo({
          pageId: found.id,
          spaceId: found.spaceId,
          blockId: typeof blockId === "string" ? blockId : undefined,
          disposition,
        });
      }
    },
    { fromEditor: true },
  );

  return (
    <NodeViewWrapper
      className={className}
      role="link"
      aria-disabled={found === null}
      title={tooltip}
      {...gestures}
      onClick={(event: MouseEvent<HTMLElement>) => {
        event.preventDefault();
        gestures.onClick(event);
      }}
    >
      <PageIcon icon={found?.icon} className="page-link-icon" />
      <span className="page-link-title">{title}</span>
      <span className="page-link-chevron" aria-hidden="true">
        <ChevronIcon />
      </span>
    </NodeViewWrapper>
  );
}

/**
 * The callout: its icon (a button that opens the icon picker) next to its blocks, and, in
 * an editable editor, a colour button (shown while the pointer is over the callout or
 * focus is in it) that opens the colour picker. Both pickers change their one attribute
 * through `updateAttributes`, which keeps the others, the block `id` included.
 */
export function CalloutView({ node, updateAttributes, editor }: ReactNodeViewProps) {
  const icon = typeof node.attrs.icon === "string" && node.attrs.icon !== "" ? node.attrs.icon : DEFAULT_CALLOUT_ICON;
  const color: unknown = node.attrs.color;
  // The binding can turn the editor read-only after the view was drawn (content from a newer version).
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const [picker, setPicker] = useState<{ anchor: Anchor; opener: Element } | null>(null);
  const [palette, setPalette] = useState<{ anchor: Anchor; opener: Element } | null>(null);

  return (
    <NodeViewWrapper className="callout-body">
      <button
        type="button"
        className="callout-icon"
        contentEditable={false}
        aria-label={t("callout.icon")}
        title={t("callout.icon")}
        onClick={(event) => {
          if (editor.isEditable) {
            setPicker({ anchor: anchorBelow(event.currentTarget), opener: event.currentTarget });
          }
        }}
      >
        {icon}
      </button>
      {editable && (
        <button
          type="button"
          className="callout-color-button"
          contentEditable={false}
          aria-label={t("callout.color")}
          title={t("callout.color")}
          aria-haspopup="dialog"
          aria-expanded={palette !== null}
          onClick={(event) => {
            const opener = event.currentTarget;
            if (!editor.isEditable) {
              return;
            }
            setPalette((current) => (current === null ? { anchor: anchorBelow(opener), opener } : null));
          }}
        >
          <span className="callout-swatch" data-color={isCalloutColor(color) ? color : undefined} aria-hidden="true" />
        </button>
      )}
      <NodeViewContent className="callout-content" />
      {picker !== null && (
        <IconPicker
          anchor={picker.anchor}
          anchorElement={picker.opener}
          current={icon}
          iconSet={false}
          onSelect={(next) => {
            updateAttributes({ icon: next ?? DEFAULT_CALLOUT_ICON });
            return Promise.resolve();
          }}
          onClose={() => {
            setPicker(null);
          }}
        />
      )}
      {editable && palette !== null && (
        <CalloutColorPicker
          anchor={palette.anchor}
          anchorElement={palette.opener}
          current={color}
          onSelect={(next) => {
            updateAttributes({ color: next });
          }}
          onClose={() => {
            setPalette(null);
          }}
        />
      )}
    </NodeViewWrapper>
  );
}
