// "Page info" (ADR-0040): a popover under the page menu with when the page was created and last edited and
// by which device (the page's own metadata: `PageInfo.createdBy` and `editedBy`, which pages made before the
// amendment of 2026-10-08 do not have), the words, characters and top-level blocks of the page as the editor
// holds it right now (`editor/page-stats.ts` says how they are counted), and where the page is: its space
// and the pages above it. The counts follow the text while the popover is open.

import type { PageInfo } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { useEditorState } from "@tiptap/react";

import { pageStats } from "../editor/page-stats";
import { activeLocale, t } from "../i18n";
import { deviceLabel, formatInstant } from "../property-format";
import { useAppStore } from "../store-context";
import { usePageTrail } from "./Breadcrumbs";
import { FloatingPanel } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";

/** Between the names of the space and of the pages above the page. */
const LOCATION_SEPARATOR = " › ";

export interface PageInfoPanelProps {
  page: PageInfo;
  /** The editor of the open page; `null` while it is still opening (the counts are then not known). */
  editor: Editor | null;
  anchor: Anchor;
  /** The control that opened the popover (the page menu's button): pressing it is not pressing outside, and focus returns to it. */
  anchorElement: Element;
  onClose: () => void;
}

/** When and by whom: the time as the locale writes it, and the device's name on a line of its own. */
function When({ at, device }: { at: number; device: string }) {
  return (
    <>
      <time dateTime={new Date(at).toISOString()}>{formatInstant(at)}</time>
      <span className="page-info-device muted">{t("pageInfo.by", { device })}</span>
    </>
  );
}

export function PageInfoPanel({ page, editor, anchor, anchorElement, onClose }: PageInfoPanelProps) {
  const trail = usePageTrail(page.id);
  const spaces = useAppStore((state) => state.spaces);
  const devices = useAppStore((state) => state.devices);
  const device = useAppStore((state) => state.device);
  const stats = useEditorState({
    editor,
    selector: ({ editor: current }) => (current === null || current.isDestroyed ? null : pageStats(current.state.doc)),
  });

  const own = device === null ? null : { deviceId: device.deviceId, name: device.name };
  const deviceName = (id: string | undefined): string =>
    id === undefined ? t("properties.computed.unknown") : deviceLabel(id, devices, own);
  const space = trail?.space ?? spaces.find((candidate) => candidate.id === page.spaceId)?.name ?? "";
  const location = [space, ...(trail?.ancestors ?? []).map((ancestor) => (ancestor.title === "" ? t("page.untitled") : ancestor.title))]
    .filter((name) => name !== "")
    .join(LOCATION_SEPARATOR);
  const number = new Intl.NumberFormat(activeLocale());
  const count = (value: number | undefined) =>
    value === undefined ? (
      <dd title={t("pageInfo.counts.unavailable")}>–</dd>
    ) : (
      <dd>{number.format(value)}</dd>
    );

  return (
    <FloatingPanel
      anchor={anchor}
      label={t("pageInfo.title")}
      onClose={onClose}
      anchorElement={anchorElement}
      returnFocus={anchorElement}
      className="page-info"
    >
      <h2 className="subtitle page-info-title">{t("pageInfo.title")}</h2>
      <dl className="page-info-list">
        <dt>{t("pageInfo.created")}</dt>
        <dd>
          <When at={page.createdAt} device={deviceName(page.createdBy)} />
        </dd>
        <dt>{t("pageInfo.edited")}</dt>
        <dd>
          <When at={page.updatedAt} device={deviceName(page.editedBy)} />
        </dd>
        <dt title={t("pageInfo.words.hint")}>{t("pageInfo.words")}</dt>
        {count(stats?.words)}
        <dt title={t("pageInfo.characters.hint")}>{t("pageInfo.characters")}</dt>
        {count(stats?.characters)}
        <dt title={t("pageInfo.blocks.hint")}>{t("pageInfo.blocks")}</dt>
        {count(stats?.blocks)}
        <dt>{t("pageInfo.location")}</dt>
        <dd>{location === "" ? "–" : location}</dd>
      </dl>
    </FloatingPanel>
  );
}
