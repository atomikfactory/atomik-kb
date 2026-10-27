// The popover of the bubble menu's colour button: two rows of swatches, the text colours (an "A"
// in each colour of the palette) and the highlights (a filled swatch each), each row with its
// own Default. It floats above the page like the callout's colour picker, takes focus when it
// opens and gives it back when it closes (Escape, a press outside or a pick).

import { FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { t } from "../i18n";
import { PALETTE_COLORS, PALETTE_LABELS } from "./palette";
import type { PaletteColor } from "./palette";

export interface TextColorPickerProps {
  anchor: Anchor;
  /** The button that opened the picker: pressing it again is not "outside", so it can close the picker. */
  anchorElement: Element;
  /**
   * The text colour of the selection: a name (one outside the palette marks no swatch), `null`
   * for none, `undefined` when the selection is mixed (nothing is marked).
   */
  textColor: string | null | undefined;
  /** The highlight of the selection, as `textColor`. */
  highlight: string | null | undefined;
  /** Called with the picked text colour; `null` is the default (no mark). */
  onSelectTextColor: (color: PaletteColor | null) => void;
  /** Called with the picked highlight; `null` is none. */
  onSelectHighlight: (color: PaletteColor | null) => void;
  onClose: () => void;
}

interface RowProps {
  kind: "text" | "fill";
  current: string | null | undefined;
  onSelect: (color: PaletteColor | null) => void;
}

/** One row: the nine swatches and Default. */
function ColorRow({ kind, current, onSelect }: RowProps) {
  const title = kind === "text" ? t("format.textColor") : t("format.highlight");
  return (
    <div role="group" aria-label={title}>
      <p className="color-row-title" aria-hidden="true">
        {title}
      </p>
      <div className="color-swatches">
        {PALETTE_COLORS.map((color) => {
          const label = t(kind === "text" ? "format.textColorOption" : "format.highlightOption", {
            color: t(PALETTE_LABELS[color]),
          });
          return (
            <button
              key={color}
              type="button"
              className="color-swatch-button"
              aria-label={label}
              title={label}
              aria-pressed={current === color}
              onClick={() => {
                onSelect(color);
              }}
            >
              <span
                className={kind === "text" ? "color-swatch color-swatch-text" : "color-swatch color-swatch-fill"}
                data-color={color}
                aria-hidden="true"
              >
                {kind === "text" ? "A" : ""}
              </span>
            </button>
          );
        })}
      </div>
      <button
        type="button"
        className="menu-item"
        aria-pressed={current === null}
        onClick={() => {
          onSelect(null);
        }}
      >
        {t("format.colorDefault")}
      </button>
    </div>
  );
}

export function TextColorPicker({
  anchor,
  anchorElement,
  textColor,
  highlight,
  onSelectTextColor,
  onSelectHighlight,
  onClose,
}: TextColorPickerProps) {
  return (
    <FloatingPanel
      label={t("format.colorTitle")}
      className="text-color-picker"
      anchor={anchor}
      onClose={onClose}
      anchorElement={anchorElement}
    >
      <ColorRow
        kind="text"
        current={textColor}
        onSelect={(color) => {
          onSelectTextColor(color);
          onClose();
        }}
      />
      <ColorRow
        kind="fill"
        current={highlight}
        onSelect={(color) => {
          onSelectHighlight(color);
          onClose();
        }}
      />
    </FloatingPanel>
  );
}
