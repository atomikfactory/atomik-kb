// The popover that sets or removes the icon of a page or a space (ADR-0029, section 1). Three
// tabs: Emoji (type or paste one, search the emoji, a few suggestions), Icons (the bundled set,
// searched by name, in the default colour or one of the nine of the palette) and Upload (an
// image from a file, a drop or a paste, scaled down in the webview and stored as an attachment of
// the page's space). A space has no Upload tab (`allowUpload`), a callout has the Emoji tab alone
// (`iconSet={false}`: its icon is an emoji, stored in the document). The tab that matches the
// current icon opens first, and the one choice that is the current icon is marked in its tab.

import { toCoreError } from "@atomik-kb/core-client";
import type { SpaceId } from "@atomik-kb/core-client";
import { useContext, useEffect, useId, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";

import { searchEmoji } from "../editor/emoji-data";
import type { EmojiChoice } from "../editor/emoji-data";
import { isFieldError } from "../errors";
import { t } from "../i18n";
import { parseIcon } from "../icons/icon-grammar";
import type { ParsedIcon } from "../icons/icon-grammar";
import { downscaleToPng } from "../icons/resize-image";
import type { ImageResizer } from "../icons/resize-image";
import { StoreContext } from "../store-context";
import { FloatingPanel } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { IconSetTab } from "./IconSetTab";
import { IconUploadTab } from "./IconUploadTab";

/** A few icons to start from; any emoji can be typed or pasted instead. */
const SUGGESTIONS = ["📄", "📝", "📚", "📁", "💡", "✅", "⭐", "🏠", "💼", "🎯", "🔬", "🎨", "🧠", "📌", "🌱", "🔒"];

/** The search lists at most this many emoji: three rows of the grid. */
const MAX_SEARCH_RESULTS = 24;

export type IconTab = "emoji" | "icons" | "upload";

export interface IconPickerProps {
  anchor: Anchor;
  /** The icon the page or space has now, if any. */
  current: string | undefined;
  /** Saves the icon (`null` removes it); rejects with a `CoreError` when the core refuses. */
  onSelect: (icon: string | null) => Promise<void>;
  onClose: () => void;
  anchorElement?: Element | null;
  /** Where focus goes when the picker closes; see `FloatingPanel`. */
  returnFocus?: Element | null;
  /**
   * Offers the Upload tab (a page's icon; a space's cannot be an image). It needs `spaceId`, the
   * space of the page, and a shell that can store attachments.
   */
  allowUpload?: boolean;
  spaceId?: SpaceId;
  /** Offers the Icons tab; `false` for the callout, whose icon is an emoji. Default `true`. */
  iconSet?: boolean;
  /** Makes the picture to upload from the file the user chose; the default scales it in a canvas (tests give their own). */
  resizeImage?: ImageResizer;
}

function describeError(failure: unknown): string {
  const error = toCoreError(failure);
  return isFieldError(error, "icon")
    ? t("form.fieldError.icon", { message: error.message })
    : t("icon.error", { message: error.message });
}

/** The tab that shows the current icon, when the picker offers it. */
function startTab(current: ParsedIcon | null, tabs: readonly IconTab[]): IconTab {
  const wanted: IconTab = current?.kind === "set" ? "icons" : current?.kind === "image" ? "upload" : "emoji";
  return tabs.includes(wanted) ? wanted : "emoji";
}

const TAB_LABELS = {
  emoji: "icon.tabs.emoji",
  icons: "icon.tabs.icons",
  upload: "icon.tabs.upload",
} as const;

interface EmojiTabProps {
  current: ParsedIcon | null;
  busy: boolean;
  error: string | null;
  errorId: string;
  clearError: () => void;
  apply: (icon: string | null) => Promise<void>;
}

/** Type or paste an emoji, search them by name, or take one of the suggestions. */
function EmojiTab({ current, busy, error, errorId, clearError, apply }: EmojiTabProps) {
  const currentEmoji = current?.kind === "emoji" ? current.text : null;
  const [value, setValue] = useState(currentEmoji ?? "");
  const [search, setSearch] = useState("");
  const [answer, setAnswer] = useState<{ query: string; choices: readonly EmojiChoice[] } | null>(null);
  const inputId = useId();

  // The emoji the search finds (the data loads with the first search: a chunk of its own).
  const query = search.trim();
  useEffect(() => {
    if (query === "") {
      return;
    }
    let active = true;
    void searchEmoji(query, MAX_SEARCH_RESULTS).then(
      (choices) => {
        if (active) {
          setAnswer({ query, choices });
        }
      },
      () => {
        if (active) {
          setAnswer({ query, choices: [] });
        }
      },
    );
    return () => {
      active = false;
    };
  }, [query]);
  // Nothing until the answer to what is typed now has come (never the answer to an older text).
  const found = query !== "" && answer?.query === query ? answer.choices : null;
  // Under the field: the suggestions while nothing is typed, then what the search found.
  const choices: { emoji: string; name: string | undefined }[] =
    query === ""
      ? SUGGESTIONS.map((icon) => ({ emoji: icon, name: undefined }))
      : (found ?? []).map((choice) => ({ emoji: choice.emoji, name: choice.name }));

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (!busy && value.trim() !== "") {
      void apply(value);
    }
  }

  return (
    <>
      <form className="icon-form" noValidate onSubmit={submit}>
        <label htmlFor={inputId}>{t("icon.label")}</label>
        <div className="row">
          <input
            id={inputId}
            className="input"
            type="text"
            value={value}
            placeholder={t("icon.placeholder")}
            autoComplete="off"
            data-autofocus=""
            aria-invalid={error !== null}
            aria-describedby={error !== null ? errorId : undefined}
            onChange={(event) => {
              setValue(event.target.value);
              clearError();
            }}
          />
          <button type="submit" className="button" disabled={busy || value.trim() === ""}>
            {t("icon.apply")}
          </button>
        </div>
      </form>
      <div className="icon-search">
        <input
          type="search"
          className="input"
          value={search}
          placeholder={t("emoji.search.placeholder")}
          aria-label={t("emoji.search")}
          autoComplete="off"
          onChange={(event) => {
            setSearch(event.target.value);
          }}
          onKeyDown={(event) => {
            // Enter takes the first match.
            const first = found?.[0];
            if (event.key === "Enter" && !busy) {
              event.preventDefault();
              if (first !== undefined) {
                void apply(first.emoji);
              }
            }
          }}
        />
      </div>
      {found !== null && found.length === 0 && (
        <p className="icon-no-match" role="status">
          {t("emoji.noMatch")}
        </p>
      )}
      {choices.length > 0 && (
        <div className="icon-choices" role="group" aria-label={query === "" ? t("icon.suggestions") : t("emoji.results")}>
          {choices.map(({ emoji, name }) => (
            <button
              key={emoji}
              type="button"
              className={emoji === currentEmoji ? "icon-choice is-current" : "icon-choice"}
              disabled={busy}
              aria-label={name}
              aria-pressed={emoji === currentEmoji ? true : undefined}
              title={name}
              onClick={() => {
                void apply(emoji);
              }}
            >
              {emoji}
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** A popover to set or remove the icon of a page or space. */
export function IconPicker({
  anchor,
  current,
  onSelect,
  onClose,
  anchorElement,
  returnFocus,
  allowUpload = false,
  spaceId,
  iconSet = true,
  resizeImage = downscaleToPng,
}: IconPickerProps) {
  const store = useContext(StoreContext);
  const parsed = parseIcon(current);
  // Uploading needs the page's space and a shell that stores attachments.
  const uploadSpace =
    allowUpload && spaceId !== undefined && store !== null && store.getState().attachmentCapabilities.upload
      ? spaceId
      : null;
  const tabs: IconTab[] = ["emoji", ...(iconSet ? (["icons"] as const) : []), ...(uploadSpace !== null ? (["upload"] as const) : [])];
  const [tab, setTab] = useState<IconTab>(() => startTab(parsed, tabs));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const errorId = useId();
  const tabsId = useId();
  const tabButtons = useRef(new Map<IconTab, HTMLButtonElement>());

  async function apply(icon: string | null): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      await onSelect(icon);
      onClose();
    } catch (failure) {
      setError(describeError(failure));
      setBusy(false);
    }
  }

  function moveTab(event: KeyboardEvent<HTMLDivElement>): void {
    const at = tabs.indexOf(tab);
    let next: IconTab | undefined;
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
      label={t("icon.title")}
      className="icon-picker"
      anchor={anchor}
      onClose={onClose}
      anchorElement={anchorElement}
      returnFocus={returnFocus}
      initialFocus={(panel) => panel.querySelector<HTMLElement>("[data-autofocus]")}
    >
      {tabs.length > 1 && (
        <div className="icon-tabs" role="tablist" aria-label={t("icon.tabs.label")} onKeyDown={moveTab}>
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
        {tab === "emoji" && (
          <EmojiTab
            current={parsed}
            busy={busy}
            error={error}
            errorId={errorId}
            clearError={() => {
              setError(null);
            }}
            apply={apply}
          />
        )}
        {tab === "icons" && <IconSetTab current={parsed} busy={busy} apply={apply} />}
        {tab === "upload" && uploadSpace !== null && (
          <IconUploadTab
            spaceId={uploadSpace}
            current={parsed}
            busy={busy}
            setBusy={setBusy}
            setError={setError}
            resize={resizeImage}
            apply={apply}
          />
        )}
      </div>
      {current !== undefined && (
        <button
          type="button"
          className="menu-item"
          disabled={busy}
          onClick={() => {
            void apply(null);
          }}
        >
          {t("icon.remove")}
        </button>
      )}
    </FloatingPanel>
  );
}
