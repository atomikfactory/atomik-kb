// The gallery layout of a database view (ADR-0054, `docs/architecture/database-views.md` section 6): a grid of
// cards, each with a preview (the page's cover, else the first image in the page, else the page's icon large and centred
// on a neutral ground: a plain card, whose preview area is the same shape so the cards stay even), the icon, the title (a
// button that opens the page; the whole card opens it too) and the shown properties, and a "New page" card at the end;
// with the toolbar's settings section (the card size).
//
// The rows arrive filtered and sorted (the gallery has no order of its own). The card asks to open a page through
// `onOpen` with the disposition of the gesture, read-only editors included, and a page is made through `onCreatePage`.
// A card reads a picture's bytes only when it is near the viewport (`NearViewport`), through the icon-image cache,
// which shares one `blob:` URL per attachment and releases it when the last card showing it unmounts. A cover that
// cannot be shown (not an image, too big, unreadable) is no preview: the page's first image is tried next.
//
// Owner: agent D. The pure rules are `page-gallery-model.ts`, the first-image lookup is `page-gallery-images.ts`.

import { useCallback, useContext, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { CSSProperties, KeyboardEvent, MouseEvent, ReactNode } from "react";
import { useStore } from "zustand";
import type { AttachmentId, PageId, PageInfo, PropertyDef } from "@atomik-kb/core-client";

import { PlusIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { coverPosition, isBundledGradient } from "../icons/cover-grammar";
import { iconImagesOf, MAX_COVER_BYTES, useIconImage } from "../icons/icon-images";
import { dispositionFromKey, openGestures } from "../page-open";
import type { AppStore } from "../store";
import { StoreContext } from "../store-context";
import type { OpenDisposition } from "../store-tabs";
import { NearViewport, useNearViewport, usePageFirstImages } from "./page-gallery-images";
import {
  cardSizeOf,
  GALLERY_CARD_WIDTHS,
  needsFirstImage,
  pictureStatus,
  previewCandidates,
  readCardSize,
} from "./page-gallery-model";
import type { GalleryPreview } from "./page-gallery-model";
import { NewPageMenu } from "./NewPageMenu";
import { PageListEmptyContext } from "./page-list-empty";
import type { PageListLayoutProps, PageListLayoutSettingsProps } from "./page-list-layout-types";
import { CARD_SIZES, DEFAULT_CARD_SIZE } from "./page-list-views";
import { valueOfPage } from "./page-list-model";
import { PageListField } from "./PageListField";
import { PageListValue } from "./PageListValue";

/** An uploaded picture: a placeholder while it is read, the picture once it is, the next candidate when it cannot be. */
function Picture({
  store,
  attachmentId,
  position,
  kind,
  next,
}: {
  store: AppStore;
  attachmentId: string;
  /** The vertical position of the picture in percent (a cover's `coverY`; 50 for a first image). */
  position: number;
  kind: "cover" | "image";
  /** What the card tries when this picture cannot be shown. */
  next: ReactNode;
}) {
  const url = useIconImage(store, attachmentId, MAX_COVER_BYTES);
  const lookup = useStore(store, (state) => state.attachmentLookup[attachmentId]);
  const canRead = useStore(store, (state) => state.attachmentCapabilities.read);
  // A picture the webview cannot decode is a failed one too.
  const [broken, setBroken] = useState<string | null>(null);
  const status = pictureStatus({
    url,
    broken,
    canRead,
    lookup,
    readFailed: iconImagesOf(store).snapshot(attachmentId).status === "failed",
  });
  if (status === "failed") {
    return next;
  }
  return (
    <div className="page-gallery-preview" data-preview={kind} data-state={status}>
      {status === "ready" && url !== null && (
        <img
          className="page-gallery-image"
          src={url}
          alt=""
          draggable={false}
          style={{ objectPosition: `50% ${String(position)}%` }}
          onError={() => {
            setBroken(url);
          }}
        />
      )}
    </div>
  );
}

/**
 * The first of `candidates` that can be drawn. A colour or a bundled gradient always can; a picture can when the card is
 * near the viewport (`near`: it reads nothing before) and the attachment turns out to be a usable image.
 */
function Preview({
  store,
  candidates,
  near,
  icon,
}: {
  store: AppStore | null;
  candidates: readonly GalleryPreview[];
  near: boolean;
  /** The page's icon (`undefined`: none, the default glyph of a page), shown when no candidate can be drawn. */
  icon: string | undefined;
}) {
  const [first, ...rest] = candidates;
  if (first === undefined || first.kind === "none") {
    // A plain card: the icon, large, in the picture's place. (While the first-image lookup is on its way a page that has
    // one shows this for a moment; the area keeps its shape, so nothing moves when the picture comes.)
    return (
      <div className="page-gallery-preview" data-preview="icon" data-state="ready">
        <PageIcon icon={icon} className="page-gallery-preview-icon" />
      </div>
    );
  }
  const next = <Preview store={store} candidates={rest} near={near} icon={icon} />;
  if (first.kind === "cover" && first.cover.kind !== "image") {
    const named =
      first.cover.kind === "color"
        ? { "data-cover-color": first.cover.color }
        : isBundledGradient(first.cover.name)
          ? { "data-cover-gradient": first.cover.name }
          : null;
    return named === null ? (
      next
    ) : (
      <div className="page-gallery-preview" data-preview="cover" data-state="ready" {...named} />
    );
  }
  const attachmentId = first.kind === "image" ? first.attachmentId : first.cover.kind === "image" ? first.cover.attachmentId : null;
  if (attachmentId === null || store === null) {
    return next;
  }
  if (!near) {
    // Far from the viewport: the picture's place, and no read.
    return <div className="page-gallery-preview" data-preview={first.kind} data-state="far" />;
  }
  return (
    <Picture
      store={store}
      attachmentId={attachmentId}
      position={first.kind === "cover" ? first.y : coverPosition(undefined)}
      kind={first.kind}
      next={next}
    />
  );
}

/** Whether the page is being printed (`print.ts`): paper does not scroll, so every picture is read. */
function usePrinting(store: AppStore | null): boolean {
  const subscribe = useCallback((listener: () => void) => (store === null ? () => undefined : store.subscribe(listener)), [store]);
  return useSyncExternalStore(subscribe, () => store?.getState().printing ?? false);
}

/** True for a mouse event that comes from the title button, which handles its own gestures. */
function fromTitle(event: MouseEvent<HTMLElement>): boolean {
  return event.target instanceof Element && event.target.closest(".page-gallery-title") !== null;
}

interface CardProps {
  page: PageInfo;
  firstImages: ReadonlyMap<PageId, AttachmentId>;
  columns: readonly PropertyDef[];
  schema: readonly PropertyDef[];
  today: string;
  store: AppStore | null;
  viewport: NearViewport;
  /** Every picture is read, near or not (print mode). */
  printing: boolean;
  onOpen: (page: PageInfo, disposition: OpenDisposition) => void;
}

function GalleryCard({ page, firstImages, columns, schema, today, store, viewport, printing, onOpen }: CardProps) {
  const { ref, near } = useNearViewport(viewport);
  const candidates = previewCandidates(page, firstImages);
  const title = page.title === "" ? t("page.untitled") : page.title;
  const open = (disposition: OpenDisposition): void => {
    onOpen(page, disposition);
  };
  const button = openGestures<HTMLButtonElement>(open);
  const card = openGestures<HTMLLIElement>(open);
  return (
    <li
      ref={ref}
      className="page-gallery-card"
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
      <Preview store={store} candidates={candidates} near={near || printing} icon={page.icon} />
      <div className="page-gallery-body">
        <button
          type="button"
          className="page-gallery-title"
          aria-label={t("pageGallery.open", { title })}
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
          <span className="page-gallery-name">{title}</span>
        </button>
        {columns.map((def) => {
          const value = valueOfPage(page, def.id);
          return value === undefined ? null : (
            <div key={def.id} className="page-gallery-prop" role="group" aria-label={def.name} data-kind={def.kind}>
              <PageListValue def={def} value={value} today={today} schema={schema} />
            </div>
          );
        })}
      </div>
    </li>
  );
}

/** The gallery. */
export function PageGalleryView({
  rows,
  view,
  schema,
  columns,
  spaceId,
  today,
  onOpen,
  onUpdateView,
  onCreatePage,
  creating,
}: PageListLayoutProps) {
  // `editable` is not read: a read-only editor draws the same cards and opens pages; only `onCreatePage` (the "New page" card) is `null`.
  const store = useContext(StoreContext);
  const printing = usePrinting(store);
  // What the node view says when there are no rows (it knows why: no pages, a tag nobody has, the filters); `null` while rows are on their way.
  const emptyText = useContext(PageListEmptyContext);
  const size = cardSizeOf(view);
  // The pages whose cover is not a colour or a gradient may need their first image.
  const ids = useMemo(() => rows.filter(needsFirstImage).map((page) => page.id), [rows]);
  const firstImages = usePageFirstImages(spaceId, ids);
  const [viewport] = useState(() => new NearViewport());
  useEffect(
    () => () => {
      viewport.dispose();
    },
    [viewport],
  );

  const showGrid = rows.length > 0 || onCreatePage !== null;
  return (
    <div className="page-gallery" role="group" aria-label={t("pageGallery.label")} data-size={size}>
      {rows.length === 0 && emptyText !== null && <p className="page-list-empty">{emptyText}</p>}
      {showGrid && (
        <ul
          className="page-gallery-grid"
          aria-label={t("pageGallery.grid")}
          style={{ "--gallery-card-width": `${String(GALLERY_CARD_WIDTHS[size])}px` } as CSSProperties}
        >
          {rows.map((page) => (
            <GalleryCard
              key={page.id}
              page={page}
              firstImages={firstImages}
              columns={columns}
              schema={schema}
              today={today}
              store={store}
              viewport={viewport}
              printing={printing}
              onOpen={onOpen}
            />
          ))}
          {onCreatePage !== null && (
            <li className="page-gallery-card page-gallery-new">
              <button
                type="button"
                className="page-gallery-new-button"
                disabled={creating}
                onClick={() => {
                  onCreatePage();
                }}
              >
                <PlusIcon />
                <span>{t("pageGallery.newCard")}</span>
              </button>
              <NewPageMenu view={view} onCreatePage={onCreatePage} onUpdateView={onUpdateView} disabled={creating} />
            </li>
          )}
        </ul>
      )}
    </div>
  );
}

/** The gallery's section of the view settings menu: the card size, with its visible label. */
export function PageGallerySettings({ view, onUpdateView }: PageListLayoutSettingsProps) {
  return (
    <PageListField label={t("pageGallery.size")}>
      <select
        className="page-list-select"
        value={cardSizeOf(view)}
        onChange={(event) => {
          const size = readCardSize(event.target.value);
          // The default is the absence of the setting.
          onUpdateView({ cardSize: size === DEFAULT_CARD_SIZE ? null : size });
        }}
      >
        {CARD_SIZES.map((size) => (
          <option key={size} value={size}>
            {t(`pageGallery.size.${size}`)}
          </option>
        ))}
      </select>
    </PageListField>
  );
}
