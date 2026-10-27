// The Gallery tab of the cover picker (ADR-0040): the nine colours of the palette and the bundled
// gradients as swatches. A swatch is a button; choosing one stores `color:<name>` or `gradient:<name>`.
// The swatches are one tab stop: Tab reaches the current cover (else the first swatch), the arrow keys
// move between swatches (Up and Down by a row).

import { useId, useState } from "react";
import type { KeyboardEvent } from "react";

import { PALETTE_COLORS, PALETTE_LABELS } from "../editor/palette";
import { t } from "../i18n";
import { COVER_GRADIENTS, GRADIENT_LABELS, colorCoverValue, gradientCoverValue } from "../icons/cover-grammar";
import type { ParsedCover } from "../icons/cover-grammar";

/** The swatches of a row (`.cover-swatches` in app.css): the Up and Down keys move by this many. */
export const COVER_COLUMNS = 5;

export interface CoverGalleryTabProps {
  current: ParsedCover | null;
  busy: boolean;
  /** Stores the cover (`color:<name>` or `gradient:<name>`) and closes the picker. */
  apply: (cover: string) => Promise<void>;
}

export function CoverGalleryTab({ current, busy, apply }: CoverGalleryTabProps) {
  const ids = useId();
  const currentValue =
    current?.kind === "color" ? colorCoverValue(current.color) : current?.kind === "gradient" ? `gradient:${current.name}` : null;
  const swatches = [
    ...PALETTE_COLORS.map((name) => ({ value: colorCoverValue(name), label: t(PALETTE_LABELS[name]), attribute: "data-cover-color", name })),
    ...COVER_GRADIENTS.map((name) => ({
      value: gradientCoverValue(name),
      label: t(GRADIENT_LABELS[name]),
      attribute: "data-cover-gradient",
      name,
    })),
  ];
  // The swatch the Tab key reaches: the last one that had focus, else the current cover, else the first.
  const [focused, setFocused] = useState<string | null>(null);
  const tabStop = swatches.find((swatch) => swatch.value === (focused ?? currentValue))?.value ?? swatches[0]?.value;

  function moveFocus(event: KeyboardEvent<HTMLDivElement>): void {
    const target = event.target;
    if (!(target instanceof HTMLElement) || !target.classList.contains("cover-swatch")) {
      return;
    }
    const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(".cover-swatch"));
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
        to = at + COVER_COLUMNS;
        break;
      case "ArrowUp":
        to = at - COVER_COLUMNS;
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

  const group = (id: "colors" | "gradients", list: typeof swatches) => (
    <section className="cover-group">
      <h3 className="icon-category" id={`${ids}-${id}`}>
        {t(id === "colors" ? "cover.gallery.colors" : "cover.gallery.gradients")}
      </h3>
      <div className="cover-swatches" role="group" aria-labelledby={`${ids}-${id}`}>
        {list.map((swatch) => {
          const isCurrent = swatch.value === currentValue;
          return (
            <button
              key={swatch.value}
              type="button"
              className={isCurrent ? "cover-swatch is-current" : "cover-swatch"}
              {...{ [swatch.attribute]: swatch.name }}
              disabled={busy}
              aria-label={swatch.label}
              aria-pressed={isCurrent}
              title={swatch.label}
              tabIndex={swatch.value === tabStop ? 0 : -1}
              onFocus={() => {
                setFocused(swatch.value);
              }}
              onClick={() => {
                if (!busy) {
                  void apply(swatch.value);
                }
              }}
            />
          );
        })}
      </div>
    </section>
  );

  return (
    <div className="cover-gallery" onKeyDown={moveFocus}>
      {group("colors", swatches.slice(0, PALETTE_COLORS.length))}
      {group("gradients", swatches.slice(PALETTE_COLORS.length))}
    </div>
  );
}
