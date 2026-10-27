// The Icons tab of the icon picker: the bundled set (icons/icon-set.ts) in its categories, a
// search over names and keywords, and a colour row (the default colour, which is the text
// colour, and the nine of the palette). Choosing an icon stores `icon:<name>` or
// `icon:<name>:<color>` with the colour chosen then. The grid is one tab stop: Tab reaches the
// icon last used (the current one first), the arrow keys move between icons.

import { useMemo, useState } from "react";
import type { KeyboardEvent } from "react";

import { PALETTE_COLORS, PALETTE_LABELS } from "../editor/palette";
import type { PaletteColor } from "../editor/palette";
import { t } from "../i18n";
import type { ParsedIcon } from "../icons/icon-grammar";
import { setIconValue } from "../icons/icon-grammar";
import { ICON_CATEGORIES, ICON_SET, iconLabel, searchIcons } from "../icons/icon-set";
import type { IconCategory, SetIcon } from "../icons/icon-set";
import { PageIcon } from "./PageIcon";

/** The columns of the icon grid (`.icon-set-choices` in app.css): the Up and Down keys move by this many. */
export const ICON_COLUMNS = 8;

export interface IconSetTabProps {
  current: ParsedIcon | null;
  busy: boolean;
  /** Stores the icon (`icon:<name>[:<color>]`) and closes the picker. */
  apply: (icon: string | null) => Promise<void>;
}

/** The message that names a category of the set. */
function categoryLabel(category: IconCategory): string {
  return t(`icon.set.category.${category}`);
}

export function IconSetTab({ current, busy, apply }: IconSetTabProps) {
  const currentName = current?.kind === "set" ? current.name : null;
  const [color, setColor] = useState<PaletteColor | null>(current?.kind === "set" ? (current.color ?? null) : null);
  const [search, setSearch] = useState("");
  const query = search.trim();
  const results = useMemo(() => searchIcons(query), [query]);

  // What the grid lists, flat: the categories while nothing is typed, then the matches.
  const groups: { id: string; label: string; icons: readonly SetIcon[] }[] =
    query === ""
      ? ICON_CATEGORIES.map((category) => ({ id: category, label: categoryLabel(category), icons: ICON_SET[category] }))
      : results.length === 0
        ? []
        : [{ id: "results", label: t("icon.set.results"), icons: results }];
  const flat = groups.flatMap((group) => group.icons);

  // The icon the Tab key reaches: the current one, then the last one that had focus.
  const [focused, setFocused] = useState<{ query: string; name: string } | null>(null);
  const tabStop =
    focused !== null && focused.query === query && flat.some((icon) => icon.name === focused.name)
      ? focused.name
      : flat.find((icon) => icon.name === currentName)?.name ?? flat[0]?.name;

  function pick(name: string): void {
    if (!busy) {
      void apply(setIconValue(name, color));
    }
  }

  function moveFocus(event: KeyboardEvent<HTMLDivElement>): void {
    const target = event.target;
    if (!(target instanceof HTMLElement) || !target.classList.contains("icon-choice")) {
      return;
    }
    const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(".icon-choice"));
    const at = buttons.indexOf(target);
    let to: number;
    switch (event.key) {
      case "ArrowRight":
        to = at + 1;
        break;
      case "ArrowLeft":
        to = at - 1;
        break;
      case "ArrowDown":
        to = at + ICON_COLUMNS;
        break;
      case "ArrowUp":
        to = at - ICON_COLUMNS;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = buttons.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    buttons[to]?.focus();
  }

  return (
    <>
      <div className="icon-search">
        <input
          type="search"
          className="input"
          value={search}
          placeholder={t("icon.set.search.placeholder")}
          aria-label={t("icon.set.search")}
          autoComplete="off"
          data-autofocus=""
          onChange={(event) => {
            setSearch(event.target.value);
          }}
          onKeyDown={(event) => {
            // Enter takes the first match.
            const first = results[0];
            if (event.key === "Enter") {
              event.preventDefault();
              if (query !== "" && first !== undefined) {
                pick(first.name);
              }
            }
          }}
        />
      </div>
      <div className="icon-colors" role="group" aria-label={t("icon.set.color")}>
        <button
          type="button"
          className="icon-color"
          aria-label={t("icon.set.color.default")}
          title={t("icon.set.color.default")}
          aria-pressed={color === null}
          onClick={() => {
            setColor(null);
          }}
        >
          <span className="icon-color-dot" aria-hidden="true" />
        </button>
        {PALETTE_COLORS.map((name) => (
          <button
            key={name}
            type="button"
            className="icon-color"
            aria-label={t(PALETTE_LABELS[name])}
            title={t(PALETTE_LABELS[name])}
            aria-pressed={color === name}
            onClick={() => {
              setColor(name);
            }}
          >
            <span className="icon-color-dot" data-color={name} aria-hidden="true" />
          </button>
        ))}
      </div>
      {query !== "" && results.length === 0 && (
        <p className="icon-no-match" role="status">
          {t("icon.set.noMatch")}
        </p>
      )}
      <div className="icon-set-body" onKeyDown={moveFocus}>
        {groups.map((group) => (
          <section key={group.id} className="icon-set-group">
            {query === "" && (
              <h3 className="icon-category" aria-hidden="true">
                {group.label}
              </h3>
            )}
            <div className="icon-choices icon-set-choices" role="group" aria-label={group.label}>
              {group.icons.map((entry) => {
                const isCurrent = entry.name === currentName;
                return (
                  <button
                    key={entry.name}
                    type="button"
                    className={isCurrent ? "icon-choice is-current" : "icon-choice"}
                    disabled={busy}
                    aria-label={iconLabel(entry.name)}
                    aria-pressed={isCurrent}
                    title={iconLabel(entry.name)}
                    tabIndex={entry.name === tabStop ? 0 : -1}
                    onFocus={() => {
                      setFocused({ query, name: entry.name });
                    }}
                    onClick={() => {
                      pick(entry.name);
                    }}
                  >
                    <PageIcon icon={setIconValue(entry.name, color)} />
                  </button>
                );
              })}
            </div>
          </section>
        ))}
      </div>
    </>
  );
}
