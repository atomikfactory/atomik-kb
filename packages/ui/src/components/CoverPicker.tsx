// The popover that sets the cover of a page (ADR-0040). Three tabs: Gallery (the nine palette colours and
// the bundled gradients as swatches), Upload (a picture from a file, a drop or a paste, stored as an
// attachment of the page's space and scaled down only past 2560 pixels) and Link (the link of a picture,
// downloaded once by the shell with the rules of "Image from URL"; absent where there is no shell that can).
// Upload and Link need the page's space and a shell that stores attachments. The tab that matches the
// current cover opens first, and the one choice that is the current cover is marked. Removing a cover is
// the banner's own button, not the picker's.

import { toCoreError } from "@atomik-kb/core-client";
import type { SpaceId } from "@atomik-kb/core-client";
import { useContext, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { parseCover } from "../icons/cover-grammar";
import type { ParsedCover } from "../icons/cover-grammar";
import { downscaleCover } from "../icons/resize-image";
import type { ImageResizer } from "../icons/resize-image";
import { StoreContext } from "../store-context";
import { CoverGalleryTab } from "./CoverGalleryTab";
import { CoverLinkTab } from "./CoverLinkTab";
import { CoverUploadTab } from "./CoverUploadTab";
import { FloatingPanel } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";

export type CoverTab = "gallery" | "upload" | "link";

export interface CoverPickerProps {
  anchor: Anchor;
  /** The cover the page has now, if any. */
  current: string | undefined;
  /** The space of the page: where an uploaded or downloaded picture is stored. */
  spaceId: SpaceId;
  /** Saves the cover; rejects with a `CoreError` when the core refuses. */
  onSelect: (cover: string) => Promise<void>;
  onClose: () => void;
  anchorElement?: Element | null;
  /** Where focus goes when the picker closes; see `FloatingPanel`. */
  returnFocus?: Element | null;
  /** Makes the picture to upload from the file the user chose; the default redraws a very large one (tests give their own). */
  resizeImage?: ImageResizer;
}

function describeError(failure: unknown): string {
  const error = toCoreError(failure);
  return isFieldError(error, "cover")
    ? t("form.fieldError.cover", { message: error.message })
    : t("cover.error", { message: error.message });
}

/** The tab that shows the current cover, when the picker offers it. */
function startTab(current: ParsedCover | null, tabs: readonly CoverTab[]): CoverTab {
  return current?.kind === "image" && tabs.includes("upload") ? "upload" : "gallery";
}

const TAB_LABELS = {
  gallery: "cover.tabs.gallery",
  upload: "cover.tabs.upload",
  link: "cover.tabs.link",
} as const;

/** A popover to choose the cover of a page. */
export function CoverPicker({
  anchor,
  current,
  spaceId,
  onSelect,
  onClose,
  anchorElement,
  returnFocus,
  resizeImage = downscaleCover,
}: CoverPickerProps) {
  const store = useContext(StoreContext);
  const parsed = parseCover(current);
  const capabilities = store?.getState().attachmentCapabilities;
  const tabs: CoverTab[] = [
    "gallery",
    ...(capabilities?.upload === true ? (["upload"] as const) : []),
    ...(capabilities?.importUrl === true ? (["link"] as const) : []),
  ];
  const [tab, setTab] = useState<CoverTab>(() => startTab(parsed, tabs));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const errorId = useId();
  const tabsId = useId();
  const tabButtons = useRef(new Map<CoverTab, HTMLButtonElement>());

  async function apply(cover: string): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      await onSelect(cover);
      onClose();
    } catch (failure) {
      setError(describeError(failure));
      setBusy(false);
    }
  }

  function moveTab(event: KeyboardEvent<HTMLDivElement>): void {
    const at = tabs.indexOf(tab);
    let next: CoverTab | undefined;
    switch (event.key) {
      case "ArrowRight":
        next = tabs[(at + 1) % tabs.length];
        break;
      case "ArrowLeft":
        next = tabs[(at - 1 + tabs.length) % tabs.length];
        break;
      case "Home":
        next = tabs[0];
        break;
      case "End":
        next = tabs[tabs.length - 1];
        break;
      default:
        return;
    }
    event.preventDefault();
    if (next !== undefined) {
      setTab(next);
      tabButtons.current.get(next)?.focus();
    }
  }

  const panelId = `${tabsId}-panel`;
  return (
    <FloatingPanel
      label={t("cover.title")}
      className="icon-picker cover-picker"
      anchor={anchor}
      onClose={onClose}
      anchorElement={anchorElement}
      returnFocus={returnFocus}
      initialFocus={(panel) => panel.querySelector<HTMLElement>("[data-autofocus]") ?? panel.querySelector<HTMLElement>(".cover-swatch[tabindex='0']")}
    >
      {tabs.length > 1 && (
        <div className="icon-tabs" role="tablist" aria-label={t("cover.tabs.label")} onKeyDown={moveTab}>
          {tabs.map((id) => (
            <button
              key={id}
              ref={(button) => {
                if (button === null) {
                  tabButtons.current.delete(id);
                } else {
                  tabButtons.current.set(id, button);
                }
              }}
              type="button"
              role="tab"
              id={`${tabsId}-${id}`}
              className="icon-tab"
              aria-selected={tab === id}
              aria-controls={panelId}
              tabIndex={tab === id ? 0 : -1}
              onClick={() => {
                setTab(id);
              }}
            >
              {t(TAB_LABELS[id])}
            </button>
          ))}
        </div>
      )}
      {error !== null && (
        <p className="error-text icon-error" id={errorId} role="alert">
          {error}
        </p>
      )}
      <div
        id={panelId}
        className="icon-tab-panel"
        role={tabs.length > 1 ? "tabpanel" : undefined}
        aria-labelledby={tabs.length > 1 ? `${tabsId}-${tab}` : undefined}
      >
        {tab === "gallery" && <CoverGalleryTab current={parsed} busy={busy} apply={apply} />}
        {tab === "upload" && (
          <CoverUploadTab spaceId={spaceId} busy={busy} setBusy={setBusy} setError={setError} resize={resizeImage} apply={apply} />
        )}
        {tab === "link" && <CoverLinkTab spaceId={spaceId} busy={busy} setBusy={setBusy} setError={setError} apply={apply} />}
      </div>
    </FloatingPanel>
  );
}
