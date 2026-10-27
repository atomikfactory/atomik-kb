// Settings -> Appearance -> Accent colour (ADR-0040 item 8): nine round swatches, one per palette name, as a radio
// group. The swatch of the chosen accent is the one tab stop; the arrow keys move to the next or the previous
// swatch (and wrap) and choose it, as the keys of a native radio group do. The choice is the device setting
// `ui.accent`; the default (blue) is the setting's absence, which choosing nothing leaves alone.

import { toCoreError } from "@atomik-kb/core-client";
import { useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { ACCENT_KEY, selectAccent } from "../accent";
import type { Accent } from "../accent";
import { PALETTE_COLORS, PALETTE_LABELS, isPaletteColor } from "../editor/palette";
import { t } from "../i18n";
import { useAppStore } from "../store-context";

/** The step an arrow key takes along the row (the row wraps), or 0 for any other key. */
function stepOf(key: string): number {
  switch (key) {
    case "ArrowRight":
    case "ArrowDown":
      return 1;
    case "ArrowLeft":
    case "ArrowUp":
      return -1;
    default:
      return 0;
  }
}

export function AccentChoice() {
  const accent = useAppStore(selectAccent);
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);
  const legendId = useId();
  const hintId = useId();
  const swatches = useRef(new Map<Accent, HTMLButtonElement>());

  function choose(next: Accent): void {
    if (next === accent) {
      return;
    }
    setError(null);
    setSetting(ACCENT_KEY, next).catch((failure: unknown) => {
      setError(t("settings.accent.error", { message: toCoreError(failure).message }));
    });
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const step = stepOf(event.key);
    const from = event.target instanceof HTMLElement ? event.target.dataset.accentSwatch : undefined;
    if (step === 0 || !isPaletteColor(from) || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    event.preventDefault();
    const count = PALETTE_COLORS.length;
    const next = PALETTE_COLORS[(PALETTE_COLORS.indexOf(from) + step + count) % count];
    if (next !== undefined) {
      swatches.current.get(next)?.focus();
      choose(next);
    }
  }

  return (
    <fieldset className="fieldset accent-choice">
      <legend id={legendId}>{t("settings.accent.legend")}</legend>
      <p className="muted" id={hintId}>
        {t("settings.accent.hint")}
      </p>
      <div
        className="accent-swatches"
        role="radiogroup"
        aria-labelledby={legendId}
        aria-describedby={hintId}
        onKeyDown={onKeyDown}
      >
        {PALETTE_COLORS.map((name) => {
          const label = t(PALETTE_LABELS[name]);
          const checked = name === accent;
          return (
            <button
              key={name}
              type="button"
              role="radio"
              className="accent-swatch"
              data-accent-swatch={name}
              aria-checked={checked}
              aria-label={label}
              title={label}
              tabIndex={checked ? 0 : -1}
              ref={(element) => {
                if (element === null) {
                  swatches.current.delete(name);
                } else {
                  swatches.current.set(name, element);
                }
              }}
              onClick={() => {
                choose(name);
              }}
            />
          );
        })}
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </fieldset>
  );
}
