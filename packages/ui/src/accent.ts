// The accent colour (ADR-0040 item 8): the device setting `ui.accent` holds one of the nine palette names (the ones the
// callouts and the text colours use); absent, or anything else, is the blue the app has always had. The app puts the
// name on <html> as `data-accent` (nothing for blue), and `app.css` has a rule per name that sets the accent's
// variables for the light and the dark theme. The table below holds the same values: a test reads `app.css` and
// checks that it says the same, and computes the contrast of each pair (the text on the accent, and the accent as
// text on the page, a panel, a selected row and a drop target reach WCAG AA, 4.5:1).
//
// The light accents are dark enough to carry white text and to read as text on white; the dark ones are light and
// carry near-black text. A name stands for the same hue in both themes, not for one value.

import type { JsonValue } from "@atomik-kb/core-client";

import { isPaletteColor } from "./editor/palette";
import type { PaletteColor } from "./editor/palette";
import type { AppState } from "./store";

/** Setting that stores the accent (`ui.*` keys belong to the UI; see the core's key grammar). Never synced. */
export const ACCENT_KEY = "ui.accent";

export type Accent = PaletteColor;

/** The accent of a device that never chose one: today's blue. Its absence from the settings is the default, it is never written for it. */
export const DEFAULT_ACCENT: Accent = "blue";

export type AccentTheme = "light" | "dark";

/** What an accent sets in one theme (`#rrggbb`). The focus ring is the accent itself. */
export interface AccentShades {
  /** `--accent`: primary buttons, links, active marks. */
  accent: string;
  /** `--accent-hover`: a primary button under the pointer, a hovered link. */
  hover: string;
  /** `--accent-text`: the text on the accent. */
  text: string;
  /** `--selected`: the background of a selected row and of the text selection. */
  selected: string;
  /** `--drop-bg`: the background of a drop target. */
  drop: string;
}

/** The colours of every accent, light and dark, in the palette's order. Blue is what `app.css` has for no accent at all. */
export const ACCENT_COLORS: Readonly<Record<Accent, Readonly<Record<AccentTheme, AccentShades>>>> = {
  gray: {
    light: { accent: "#606b7b", hover: "#535c6a", text: "#ffffff", selected: "#ecedef", drop: "#f4f5f6" },
    dark: { accent: "#9ea7b3", hover: "#b2b9c2", text: "#0c1118", selected: "#303339", drop: "#272a2f" },
  },
  brown: {
    light: { accent: "#925c42", hover: "#7d4f38", text: "#ffffff", selected: "#f2ebe8", drop: "#f7f4f2" },
    dark: { accent: "#c99d87", hover: "#d4b1a0", text: "#18100c", selected: "#383130", drop: "#2d2929" },
  },
  orange: {
    light: { accent: "#b54505", hover: "#983a04", text: "#ffffff", selected: "#f6e9e1", drop: "#faf2ee" },
    dark: { accent: "#f5945c", hover: "#f7aa7e", text: "#18100c", selected: "#412f27", drop: "#342823" },
  },
  yellow: {
    light: { accent: "#8a6300", hover: "#6b4d00", text: "#ffffff", selected: "#f1ece0", drop: "#f7f4ed" },
    dark: { accent: "#f5c95c", hover: "#f7d47e", text: "#18150c", selected: "#413a27", drop: "#342f23" },
  },
  green: {
    light: { accent: "#1c7842", hover: "#165f35", text: "#ffffff", selected: "#e4efe8", drop: "#eff6f2" },
    dark: { accent: "#73de9f", hover: "#8fe5b3", text: "#0c1811", selected: "#273e35", drop: "#21322d" },
  },
  blue: {
    light: { accent: "#3b5bdb", hover: "#3050c4", text: "#ffffff", selected: "#e6ebfc", drop: "#eef1fd" },
    dark: { accent: "#7c97ff", hover: "#93a9ff", text: "#0b1020", selected: "#26304f", drop: "#222a44" },
  },
  purple: {
    light: { accent: "#814acf", hover: "#7134c5", text: "#ffffff", selected: "#f0e9f9", drop: "#f6f2fc" },
    dark: { accent: "#ad88df", hover: "#c0a4e7", text: "#110c18", selected: "#332d41", drop: "#292636" },
  },
  pink: {
    light: { accent: "#c12573", hover: "#a72063", text: "#ffffff", selected: "#f8e5ee", drop: "#fbf0f5" },
    dark: { accent: "#e472ab", hover: "#ea90bd", text: "#180c12", selected: "#3e2837", drop: "#31232e" },
  },
  red: {
    light: { accent: "#c8202e", hover: "#ad1c28", text: "#ffffff", selected: "#f8e4e6", drop: "#fbeff0" },
    dark: { accent: "#e9757e", hover: "#ee949b", text: "#180c0d", selected: "#3f292e", drop: "#322328" },
  },
};

/** Reads a stored accent; anything unexpected (unset, wrong type, a name from a newer version) means the default blue. */
export function parseAccent(value: JsonValue | null | undefined): Accent {
  return isPaletteColor(value) ? value : DEFAULT_ACCENT;
}

export const selectAccent = (state: AppState): Accent => parseAccent(state.settings[ACCENT_KEY]);

/** The value of `data-accent` on <html> for an accent: none for the default, whose colours are the stylesheet's own. */
export function accentAttribute(accent: Accent): Accent | undefined {
  return accent === DEFAULT_ACCENT ? undefined : accent;
}

/** Puts the accent on `root` (`<html>`), or takes the attribute off for the default. */
export function applyAccent(root: HTMLElement, accent: Accent): void {
  const attribute = accentAttribute(accent);
  if (attribute === undefined) {
    delete root.dataset.accent;
  } else {
    root.dataset.accent = attribute;
  }
}
