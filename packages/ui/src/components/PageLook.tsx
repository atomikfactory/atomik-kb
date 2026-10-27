// The controls of the page's appearance (ADR-0040), the lower part of the page menu: three "Ag" buttons for
// the font (the default one, a serif one and the monospace one, each drawn in its own face; one is pressed),
// and the switches "Small text" and "Lock page". They write through the store and do not close the menu, so
// the effect shows on the page behind it; what the page reports (`PageInfo`) says what is pressed.

import type { PageFont, PageInfo } from "@atomik-kb/core-client";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";

/** The three fonts, in the order of the buttons. */
export const PAGE_FONTS: readonly { font: PageFont; name: MessageKey }[] = [
  { font: "default", name: "page.look.default" },
  { font: "serif", name: "page.look.serif" },
  { font: "mono", name: "page.look.mono" },
];

export interface PageLookProps {
  page: PageInfo;
  /** Asks for a change of the look; the page follows when it is written. */
  onChange: (change: { font?: PageFont; smallText?: boolean; locked?: boolean }) => void;
}

interface SwitchProps {
  label: string;
  checked: boolean;
  onToggle: () => void;
}

/** A row with a name on the left and a switch on the right. */
function Switch({ label, checked, onToggle }: SwitchProps) {
  return (
    <button type="button" role="switch" aria-checked={checked} className="menu-item menu-switch" data-menu-nav onClick={onToggle}>
      <span>{label}</span>
      <span className="menu-switch-track" aria-hidden="true">
        <span className="menu-switch-thumb" />
      </span>
    </button>
  );
}

export function PageLook({ page, onChange }: PageLookProps) {
  const current: PageFont = page.font ?? "default";
  return (
    <div className="page-look">
      <div role="group" aria-label={t("page.look.font")} className="page-look-fonts">
        {PAGE_FONTS.map(({ font, name }) => (
          <button
            key={font}
            type="button"
            className="page-look-font"
            data-font={font}
            data-menu-nav
            aria-pressed={current === font}
            aria-label={t("page.look.fontNamed", { name: t(name) })}
            title={t("page.look.fontNamed", { name: t(name) })}
            onClick={() => {
              if (current !== font) {
                onChange({ font });
              }
            }}
          >
            <span className="page-look-glyph" aria-hidden="true">
              {t("page.look.glyph")}
            </span>
            <span className="page-look-name" aria-hidden="true">
              {t(name)}
            </span>
          </button>
        ))}
      </div>
      <Switch
        label={t("page.look.smallText")}
        checked={page.smallText === true}
        onToggle={() => {
          onChange({ smallText: page.smallText !== true });
        }}
      />
      <Switch
        label={t("page.look.lock")}
        checked={page.locked === true}
        onToggle={() => {
          onChange({ locked: page.locked !== true });
        }}
      />
    </div>
  );
}

/** The class names a page's section carries for its look (`app.css` draws them): font, small text, lock. */
export function pageLookClasses(page: Pick<PageInfo, "font" | "smallText" | "locked">): string {
  return [
    "page",
    page.font === "serif" || page.font === "mono" ? `page-font-${page.font}` : "",
    page.smallText === true ? "page-small-text" : "",
    page.locked === true ? "page-locked" : "",
  ]
    .filter((name) => name !== "")
    .join(" ");
}
