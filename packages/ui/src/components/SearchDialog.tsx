import { TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { AiSource, SearchHit, TextRange } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { createPortal } from "react-dom";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { dispositionFromKey, openGestures } from "../page-open";
import type { OpenGestureHandlers } from "../page-open";
import { useRecentPages } from "../recent-pages";
import type { RecentPage } from "../recent-pages";
import { selectAiEnabled } from "../store-ai";
import { useAppStore } from "../store-context";
import type { SearchEdited, SearchSpaceFilter } from "../store-content";
import type { OpenDisposition } from "../store-tabs";
import { focusableIn } from "./FloatingPanel";
import { OptionPill } from "./OptionPill";
import { PageIcon } from "./PageIcon";
import { CloseIcon, TagGlyph } from "./property-icons";

/** The search waits this long after the last keystroke. */
export const SEARCH_DEBOUNCE_MS = 150;

/** Recent pages the dialog lists while nothing is typed (ADR-0029). */
export const SEARCH_RECENT_LIMIT = 10;

/** The "Related" group (ADR-0037) waits this long after the last keystroke. */
export const RELATED_DEBOUNCE_MS = 300;

/** The longest passage shown under a related page's title, in characters. */
const RELATED_SNIPPET_CHARS = 140;

/** A passage as one line of at most `RELATED_SNIPPET_CHARS` characters. */
function relatedSnippet(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  const chars = Array.from(line);
  return chars.length <= RELATED_SNIPPET_CHARS ? line : `${chars.slice(0, RELATED_SNIPPET_CHARS - 1).join("")}…`;
}

/** `text` with the `highlights` (UTF-16 ranges) wrapped in `<mark>`. */
export function highlighted(text: string, highlights: readonly TextRange[]): ReactNode[] {
  const parts: ReactNode[] = [];
  let at = 0;
  const ranges = [...highlights]
    .filter((range) => range.start >= 0 && range.end <= text.length && range.start < range.end)
    .sort((a, b) => a.start - b.start);
  for (const range of ranges) {
    if (range.start < at) {
      continue; // overlapping ranges: the first one wins
    }
    if (range.start > at) {
      parts.push(text.slice(at, range.start));
    }
    parts.push(<mark key={`${String(range.start)}-${String(range.end)}`}>{text.slice(range.start, range.end)}</mark>);
    at = range.end;
  }
  if (at < text.length) {
    parts.push(text.slice(at));
  }
  return parts;
}

/** The lists the filter chips open under the row of chips (one at a time). */
type FilterList = "tag" | "space" | "edited";

/** The choices of the Space chip, the default first. */
const SPACE_CHOICES: readonly { value: SearchSpaceFilter; label: MessageKey }[] = [
  { value: "all", label: "search.filter.space.all" },
  { value: "current", label: "search.filter.space.current" },
];

/** The choices of the Edited chip, the default first. */
const EDITED_CHOICES: readonly { value: SearchEdited; label: MessageKey }[] = [
  { value: "any", label: "search.filter.edited.any" },
  { value: "today", label: "search.filter.edited.today" },
  { value: "week", label: "search.filter.edited.week" },
  { value: "month", label: "search.filter.edited.month" },
];

/** The gestures of a result: as any link to a page, and a press on it keeps focus in the field (the dialog may stay open). */
function resultGestures(open: (disposition: OpenDisposition) => void): OpenGestureHandlers<HTMLDivElement> {
  const gestures = openGestures<HTMLDivElement>(open);
  return {
    ...gestures,
    onMouseDown: (event) => {
      gestures.onMouseDown(event);
      event.preventDefault();
    },
  };
}

interface HitGroup {
  pageId: string;
  first: SearchHit;
  hits: SearchHit[];
}

/** Hits grouped by page, pages in the order of their best hit. */
function groupByPage(hits: readonly SearchHit[]): HitGroup[] {
  const groups = new Map<string, HitGroup>();
  for (const hit of hits) {
    const group = groups.get(hit.pageId);
    if (group === undefined) {
      groups.set(hit.pageId, { pageId: hit.pageId, first: hit, hits: [hit] });
    } else {
      group.hits.push(hit);
    }
  }
  return [...groups.values()];
}

/**
 * Search over every space (Ctrl+K or Cmd+K, or the sidebar's button): results grouped by
 * page with the matches highlighted; Up and Down move, Enter or a click opens the page
 * (switching space when needed) and scrolls to the block; Escape closes. Ctrl/Cmd + Enter or
 * Ctrl/Cmd + Shift + click opens it in a new tab in front, Ctrl/Cmd + click or a middle click in a
 * background tab (ADR-0034). The dialog closes when the page opens in the active tab or in front;
 * a background open leaves it as it is (query, results, active item), so several hits can be
 * opened in a row.
 *
 * With local AI on (ADR-0037) and an index that has chunks, a query of three characters or more also asks for
 * related pages, which show under the matches as a "Related" group: one row per page, with a passage, for pages the
 * words themselves did not find. A failure there is silent.
 *
 * A row of filter chips sits under the field. The "Tag" chip lists the tags of the current
 * space (ADR-0026); picking one limits the results to the pages that have it, and with nothing
 * typed lists those pages. A click on a tag of the properties panel opens the dialog with the
 * chip already set. The "Space" chip (every space, or the open one), the "Edited" chip (any time,
 * today, the past 7 or 30 days) and the "Titles only" toggle (ADR-0040) are asked of the core with the
 * query; the Space chip stands down while a tag is chosen, since a tag belongs to one space. Each chip
 * is a button: Space and Edited open a short list of choices, like the Tag chip, and Escape closes it.
 */
export function SearchDialog() {
  const search = useAppStore((state) => state.search);
  const runSearch = useAppStore((state) => state.runSearch);
  const closeSearch = useAppStore((state) => state.closeSearch);
  const setSearchTag = useAppStore((state) => state.setSearchTag);
  const setSearchFilters = useAppStore((state) => state.setSearchFilters);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const openComments = useAppStore((state) => state.openComments);
  const currentSpaceId = useAppStore((state) => state.currentSpaceId);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  const relatedEnabled = useAppStore(selectAiEnabled);
  const related = useAppStore((state) => state.related);
  const runRelatedSearch = useAppStore((state) => state.runRelatedSearch);
  const clearRelated = useAppStore((state) => state.clearRelated);
  const tagSpaceId = search.tag?.spaceId ?? currentSpaceId;
  const tagOptions = useAppStore(
    (state) =>
      (tagSpaceId === null ? undefined : state.propertySchemas[tagSpaceId])?.find((def) => def.id === TAGS_PROPERTY_ID)
        ?.options,
  );
  const [query, setQuery] = useState(search.query);
  const [active, setActive] = useState(0);
  const [openList, setOpenList] = useState<FilterList | null>(null);
  const { tag, space, edited, titlesOnly } = search;
  const inputRef = useRef<HTMLInputElement>(null);
  const listId = useId();
  const optionId = useId();
  // Read while rendering, before the input takes focus.
  const [opener] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));

  useEffect(() => {
    inputRef.current?.focus();
    return () => {
      if (opener?.isConnected === true) {
        opener.focus();
      }
    };
  }, [opener]);

  // The tags the chip lists come with the schema of the space.
  useEffect(() => {
    if (tagSpaceId !== null) {
      loadPropertySchema(tagSpaceId);
    }
  }, [tagSpaceId, loadPropertySchema]);

  // The same search again when a filter changes (the filters are part of what is asked).
  useEffect(() => {
    const timer = setTimeout(() => {
      void runSearch(query);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [query, tag, space, edited, titlesOnly, runSearch]);

  // Related pages (ADR-0037) follow the query a moment after the last keystroke, and go with the dialog.
  useEffect(() => {
    if (!relatedEnabled) {
      return;
    }
    const timer = setTimeout(() => {
      void runRelatedSearch(query);
    }, RELATED_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [query, relatedEnabled, runRelatedSearch]);
  useEffect(
    () => () => {
      clearRelated();
    },
    [clearRelated],
  );

  // With nothing typed and no tag chosen, the dialog lists the pages opened last instead of results.
  const recents = useRecentPages(SEARCH_RECENT_LIMIT);
  const showRecents = query.trim() === "" && tag === null && recents.length > 0;
  const groups = search.query === query ? groupByPage(search.hits) : groupByPage([]);
  const options = groups.flatMap((group) => group.hits);
  // Related pages for this very query, without the pages the words found; they come after the matches.
  const shownPages = new Set(groups.map((group) => group.pageId));
  const relatedRows: readonly AiSource[] =
    relatedEnabled && tag === null && !showRecents && related.query === query.trim()
      ? related.items.filter((source) => !shownPages.has(source.pageId))
      : [];
  const optionCount = showRecents ? recents.length : options.length + relatedRows.length;
  const activeIndex = Math.min(active, optionCount - 1);
  const idOf = (index: number): string => `${optionId}-${String(index)}`;

  /** Leaves the dialog, unless the page opens in a background tab: then it stays as it is. */
  function leave(disposition: OpenDisposition): void {
    if (disposition !== "background") {
      closeSearch();
    }
  }

  function open(hit: SearchHit, disposition: OpenDisposition): void {
    leave(disposition);
    navigateTo({ pageId: hit.pageId, spaceId: hit.spaceId, blockId: hit.blockId, disposition });
    // A hit in a comment (ADR-0048) opens the page with the comments panel on the thread that holds it.
    if (hit.commentId !== undefined && disposition !== "background") {
      openComments({ commentId: hit.commentId });
    }
  }

  function openRelated(source: AiSource, disposition: OpenDisposition): void {
    leave(disposition);
    navigateTo({ pageId: source.pageId, spaceId: source.spaceId, blockId: source.blockId, disposition });
  }

  function openRecent(recent: RecentPage, disposition: OpenDisposition): void {
    leave(disposition);
    navigateTo({ pageId: recent.pageId, spaceId: recent.spaceId, disposition });
  }

  function pickTag(optionId: string): void {
    if (tagSpaceId !== null) {
      setSearchTag({ spaceId: tagSpaceId, propertyId: TAGS_PROPERTY_ID, optionId });
    }
    setOpenList(null);
    setActive(0);
    inputRef.current?.focus();
  }

  /** Opens or closes the list of a chip; one list at a time. */
  function toggleList(list: FilterList): void {
    setOpenList((current) => (current === list ? null : list));
  }

  function pickSpace(value: SearchSpaceFilter): void {
    setSearchFilters({ space: value });
    setOpenList(null);
    setActive(0);
    inputRef.current?.focus();
  }

  function pickEdited(value: SearchEdited): void {
    setSearchFilters({ edited: value });
    setOpenList(null);
    setActive(0);
    inputRef.current?.focus();
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    // Up, Down and Enter belong to the results while the field has focus, not to a chip.
    const inField = event.target === inputRef.current;
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        if (openList !== null) {
          setOpenList(null);
          inputRef.current?.focus();
        } else {
          closeSearch();
        }
        return;
      case "ArrowDown":
        if (!inField) {
          return;
        }
        if (optionCount > 0) {
          setActive((activeIndex + 1) % optionCount);
        }
        break;
      case "ArrowUp":
        if (!inField) {
          return;
        }
        if (optionCount > 0) {
          setActive((activeIndex - 1 + optionCount) % optionCount);
        }
        break;
      case "Enter": {
        if (!inField) {
          return;
        }
        const recent = showRecents ? recents[activeIndex] : undefined;
        const hit = showRecents ? undefined : options[activeIndex];
        const relatedHit = showRecents || activeIndex < options.length ? undefined : relatedRows[activeIndex - options.length];
        if (recent !== undefined) {
          openRecent(recent, dispositionFromKey(event));
        } else if (hit !== undefined) {
          open(hit, dispositionFromKey(event));
        } else if (relatedHit !== undefined) {
          openRelated(relatedHit, dispositionFromKey(event));
        }
        break;
      }
      case "Tab": {
        // Focus stays inside the dialog: the field, the filter chips and the tags they list.
        const stops = focusableIn(event.currentTarget);
        const first = stops[0];
        const last = stops[stops.length - 1];
        if (first !== undefined && last !== undefined) {
          if (event.shiftKey && document.activeElement === first) {
            last.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            first.focus();
          } else {
            return; // the browser moves focus to the next stop
          }
        }
        break;
      }
      default:
        return;
    }
    event.preventDefault();
  }

  const tagName = tag === null ? undefined : tagOptions?.find((option) => option.id === tag.optionId)?.name;

  let status: string | null = null;
  if (query.trim() === "" && tag === null) {
    status = t("search.hint");
  } else if (search.query !== query || search.status === "loading") {
    status = t("search.loading");
  } else if (search.status === "error") {
    status = t("search.error");
  } else if (options.length === 0 && relatedRows.length === 0) {
    status =
      query.trim() === ""
        ? t("search.noTagged", { tag: tagName ?? "" })
        : t("search.none", { query: query.trim() });
  }

  // Where each group's hits start among `options`.
  const offsets: number[] = [];
  let total = 0;
  for (const group of groups) {
    offsets.push(total);
    total += group.hits.length;
  }

  return createPortal(
    <div
      className="dialog-backdrop search-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          closeSearch();
        }
      }}
    >
      <div role="dialog" aria-modal="true" aria-label={t("search.label")} className="search-dialog" onKeyDown={onKeyDown}>
        <input
          ref={inputRef}
          className="input search-input"
          type="search"
          role="combobox"
          aria-label={t("search.label")}
          aria-expanded={optionCount > 0}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={activeIndex >= 0 ? idOf(activeIndex) : undefined}
          placeholder={t("search.placeholder")}
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActive(0);
          }}
        />
        <div className="search-filters" role="group" aria-label={t("search.filters")}>
          {tagSpaceId !== null &&
            (tag === null ? (
              <button
                type="button"
                className="search-chip"
                aria-haspopup="true"
                aria-expanded={openList === "tag"}
                onClick={() => {
                  toggleList("tag");
                }}
              >
                <TagGlyph />
                <span>{t("search.filter.tag")}</span>
              </button>
            ) : (
              <span className="search-chip is-active">
                <TagGlyph />
                <span>{t("search.filter.tagIs")}</span>
                {tagName !== undefined && (
                  <OptionPill name={tagName} color={tagOptions?.find((option) => option.id === tag.optionId)?.color ?? "gray"} />
                )}
                <button
                  type="button"
                  className="search-chip-remove"
                  aria-label={t("search.filter.remove", { name: tagName ?? t("search.filter.tag") })}
                  title={t("search.filter.remove", { name: tagName ?? t("search.filter.tag") })}
                  onClick={() => {
                    setSearchTag(null);
                    setActive(0);
                    inputRef.current?.focus();
                  }}
                >
                  <CloseIcon />
                </button>
              </span>
            ))}
          {tag === null && currentSpaceId !== null && (
            <button
              type="button"
              className={space === "all" ? "search-chip" : "search-chip is-active"}
              aria-haspopup="true"
              aria-expanded={openList === "space"}
              onClick={() => {
                toggleList("space");
              }}
            >
              <span>{t("search.filter.space")}:</span> <span>{t(SPACE_CHOICES.find((choice) => choice.value === space)?.label ?? "search.filter.space.all")}</span>
            </button>
          )}
          <button
            type="button"
            className={edited === "any" ? "search-chip" : "search-chip is-active"}
            aria-haspopup="true"
            aria-expanded={openList === "edited"}
            onClick={() => {
              toggleList("edited");
            }}
          >
            <span>{t("search.filter.edited")}:</span> <span>{t(EDITED_CHOICES.find((choice) => choice.value === edited)?.label ?? "search.filter.edited.any")}</span>
          </button>
          <button
            type="button"
            className={titlesOnly ? "search-chip is-active" : "search-chip"}
            aria-pressed={titlesOnly}
            onClick={() => {
              setSearchFilters({ titlesOnly: !titlesOnly });
              setActive(0);
            }}
          >
            <span>{t("search.filter.titlesOnly")}</span>
          </button>
        </div>
        {openList === "tag" && (
          <div className="search-tag-list" role="group" aria-label={t("search.filter.tagList")}>
            {tagOptions === undefined || tagOptions.length === 0 ? (
              <p className="muted">{t("search.filter.noTags")}</p>
            ) : (
              tagOptions.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className="search-tag"
                  onClick={() => {
                    pickTag(option.id);
                  }}
                >
                  <OptionPill name={option.name} color={option.color} />
                </button>
              ))
            )}
          </div>
        )}
        {openList === "space" && (
          <div className="search-tag-list" role="group" aria-label={t("search.filter.spaceList")}>
            {SPACE_CHOICES.map((choice) => (
              <button
                key={choice.value}
                type="button"
                className={choice.value === space ? "search-chip is-active" : "search-chip"}
                aria-pressed={choice.value === space}
                onClick={() => {
                  pickSpace(choice.value);
                }}
              >
                {t(choice.label)}
              </button>
            ))}
          </div>
        )}
        {openList === "edited" && (
          <div className="search-tag-list" role="group" aria-label={t("search.filter.editedList")}>
            {EDITED_CHOICES.map((choice) => (
              <button
                key={choice.value}
                type="button"
                className={choice.value === edited ? "search-chip is-active" : "search-chip"}
                aria-pressed={choice.value === edited}
                onClick={() => {
                  pickEdited(choice.value);
                }}
              >
                {t(choice.label)}
              </button>
            ))}
          </div>
        )}
        <div id={listId} role="listbox" aria-label={t("search.results")} className="search-results">
          {showRecents && (
            <div role="group" aria-label={t("recent.title")} className="search-group search-recent">
              <div className="search-group-title" aria-hidden="true">
                <span>{t("recent.title")}</span>
              </div>
              {recents.map((recent, index) => {
                const title = recent.title === "" ? t("page.untitled") : recent.title;
                const elsewhere = recent.spaceId !== currentSpaceId && recent.spaceName !== "";
                return (
                  <div
                    key={recent.pageId}
                    id={idOf(index)}
                    role="option"
                    aria-selected={index === activeIndex}
                    aria-label={elsewhere ? t("recent.inSpace", { title, space: recent.spaceName }) : title}
                    className={index === activeIndex ? "search-hit search-recent-hit is-active" : "search-hit search-recent-hit"}
                    onMouseEnter={() => {
                      setActive(index);
                    }}
                    {...resultGestures((disposition) => {
                      openRecent(recent, disposition);
                    })}
                  >
                    <PageIcon icon={recent.icon} className="search-recent-icon" />
                    <span className={recent.title === "" ? "search-hit-title is-untitled" : "search-hit-title"}>
                      {title}
                    </span>
                    {elsewhere && <span className="search-recent-space">{recent.spaceName}</span>}
                  </div>
                );
              })}
            </div>
          )}
          {groups.map((group, groupIndex) => {
            const title = group.first.title === "" ? t("page.untitled") : group.first.title;
            return (
              <div key={group.pageId} role="group" aria-label={title} className="search-group">
                <div className="search-group-title" aria-hidden="true">
                  <PageIcon icon={group.first.icon} className="search-group-icon" />
                  <span>{title}</span>
                </div>
                {group.hits.map((hit, hitIndex) => {
                  const at = (offsets[groupIndex] ?? 0) + hitIndex;
                  return (
                    <div
                      key={`${hit.commentId ?? hit.blockId ?? "title"}-${String(at)}`}
                      id={idOf(at)}
                      role="option"
                      aria-selected={at === activeIndex}
                      className={at === activeIndex ? "search-hit is-active" : "search-hit"}
                      onMouseEnter={() => {
                        setActive(at);
                      }}
                      {...resultGestures((disposition) => {
                        open(hit, disposition);
                      })}
                    >
                      <span className={hit.blockId === undefined && hit.commentId === undefined ? "search-hit-title" : "search-hit-snippet"}>
                        {hit.commentId !== undefined && <span className="search-hit-label muted">{t("search.inComment")} </span>}
                        {highlighted(hit.snippet, hit.highlights)}
                      </span>
                    </div>
                  );
                })}
              </div>
            );
          })}
          {relatedRows.length > 0 && (
            <div role="group" aria-label={t("search.related")} className="search-group search-related">
              <div className="search-group-title" aria-hidden="true">
                <span className="search-related-mark">✦</span>
                <span>{t("search.related")}</span>
              </div>
              {relatedRows.map((source, index) => {
                const at = options.length + index;
                const title = source.title === "" ? t("page.untitled") : source.title;
                return (
                  <div
                    key={source.pageId}
                    id={idOf(at)}
                    role="option"
                    aria-selected={at === activeIndex}
                    aria-label={t("search.related.row", { title })}
                    className={at === activeIndex ? "search-hit search-related-hit is-active" : "search-hit search-related-hit"}
                    onMouseEnter={() => {
                      setActive(at);
                    }}
                    {...resultGestures((disposition) => {
                      openRelated(source, disposition);
                    })}
                  >
                    <span className="search-related-mark" aria-hidden="true">
                      ✦
                    </span>
                    <span className="search-hit-title">{title}</span>
                    <span className="search-hit-snippet search-related-snippet">{relatedSnippet(source.text)}</span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        {status !== null && (
          <p className="search-status muted" role="status">
            {status}
          </p>
        )}
      </div>
    </div>,
    document.body,
  );
}
