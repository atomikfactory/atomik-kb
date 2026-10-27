// The popover that sets a callout's background colour: one swatch per name of the palette
// (`CALLOUT_COLORS`) and "Default". It floats above the page like the icon picker, takes
// focus when it opens and gives it back when it closes (Escape, a press outside or a pick).

import { FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { t } from "../i18n";
import { CALLOUT_COLORS } from "./callout";
import type { CalloutColor } from "./callout";
import { PALETTE_LABELS } from "./palette";

export interface CalloutColorPickerProps {
  anchor: Anchor;
  /** The button that opened the picker: pressing it again is not "outside", so it can close the picker. */
  anchorElement: Element;
  /**
   * The colour the callout holds. A value outside the palette (a name from a newer version)
   * marks no swatch, and is left alone until another colour is picked.
   */
  current: unknown;
  /** Called with the picked colour; `null` is the default background. */
  onSelect: (color: CalloutColor | null) => void;
  onClose: () => void;
}

export function CalloutColorPicker({ anchor, anchorElement, current, onSelect, onClose }: CalloutColorPickerProps) {
  const isDefault = current === null || current === undefined || current === "";

  function pick(color: CalloutColor | null): void {
    onSelect(color);
    onClose();
  }

  return (
    <FloatingPanel
      label={t("callout.colorTitle")}
      className="callout-color-picker"
      anchor={anchor}
      onClose={onClose}
      anchorElement={anchorElement}
    >
      <div className="callout-swatches" role="group" aria-label={t("callout.colorTitle")}>
        {CALLOUT_COLORS.map((color) => (
          <button
            key={color}
            type="button"
            className="callout-swatch-button"
            aria-label={t(PALETTE_LABELS[color])}
            title={t(PALETTE_LABELS[color])}
            aria-pressed={current === color}
            onClick={() => {
              pick(color);
            }}
          >
            <span className="callout-swatch" data-color={color} aria-hidden="true" />
          </button>
        ))}
      </div>
      <button
        type="button"
        className="menu-item"
        aria-pressed={isDefault}
        onClick={() => {
          pick(null);
        }}
      >
        {t("callout.color.default")}
      </button>
    </FloatingPanel>
  );
}
