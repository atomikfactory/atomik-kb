// The accent colour (ADR-0040 item 8): the nine palette names, what each one sets for the light and the dark
// theme, and that `app.css` says the same as the table. The contrast is computed from the table (WCAG 2.x
// relative luminance), so a colour that reads badly fails here and not in somebody's eyes.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { ACCENT_COLORS, ACCENT_KEY, DEFAULT_ACCENT, accentAttribute, parseAccent } from "./accent";
import type { Accent, AccentShades, AccentTheme } from "./accent";
import { PALETTE_COLORS } from "./editor/palette";

const THEMES: readonly AccentTheme[] = ["light", "dark"];

/** WCAG's normal-text threshold (AA). */
const AA = 4.5;

function channel(value: number): number {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

/** The relative luminance of `#rrggbb`. */
function luminance(hex: string): number {
  expect(hex).toMatch(/^#[0-9a-f]{6}$/u);
  const [r, g, b] = [1, 3, 5].map((at) => channel(Number.parseInt(hex.slice(at, at + 2), 16)));
  return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
}

/** The contrast ratio of two `#rrggbb` colours, 1 to 21. */
function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

const CSS = readFileSync("src/styles/app.css", "utf8");

interface Rule {
  selector: string;
  declarations: Readonly<Record<string, string>>;
}

/** Every `selector { declarations }` of the stylesheet, comments gone and white space collapsed (a rule inside `@media` too). */
const RULES: readonly Rule[] = [...CSS.replace(/\/\*[\s\S]*?\*\//gu, "").matchAll(/([^{}]+)\{([^{}]*)\}/gu)].map((match) => ({
  selector: (match[1] ?? "").replace(/\s+/gu, " ").trim(),
  declarations: Object.fromEntries(
    (match[2] ?? "")
      .split(";")
      .map((declaration) => declaration.split(/:(.*)/su))
      .filter((pair) => (pair[0] ?? "").trim() !== "")
      .map((pair) => [(pair[0] ?? "").trim(), (pair[1] ?? "").trim()]),
  ),
}));

function ruleFor(selector: string): Rule {
  const rule = RULES.find((candidate) => candidate.selector === selector);
  expect(rule, `no rule for ${selector}`).toBeDefined();
  return rule ?? { selector, declarations: {} };
}

/** The selectors of the base theme rules, whose first occurrences hold the defaults (blue). */
const BASE_SELECTOR: Readonly<Record<AccentTheme, string>> = {
  light: ":root",
  dark: ':root[data-theme="dark"]',
};

/**
 * The selector of the rule that gives `accent` its colours in `theme`: the page's root (`data-accent`) and a swatch of
 * the settings screen (`data-accent-swatch`, which shows the colour the theme has for it). The dark rules exist twice:
 * for the system's dark scheme (inside `@media`) and for an explicit `data-theme="dark"`.
 */
function accentSelectors(accent: Accent): Readonly<Record<AccentTheme | "system-dark", string>> {
  const pair = (scope: string, joined: string): string =>
    `${scope}[data-accent="${accent}"], ${joined} [data-accent-swatch="${accent}"]`;
  return {
    light: pair(":root", ":root"),
    "system-dark": pair(':root:not([data-theme="light"])', ':root:not([data-theme="light"])'),
    dark: pair(':root[data-theme="dark"]', ':root[data-theme="dark"]'),
  };
}

function shadesOf(rule: Rule): AccentShades {
  const { declarations } = rule;
  return {
    accent: declarations["--accent"] ?? "",
    hover: declarations["--accent-hover"] ?? "",
    text: declarations["--accent-text"] ?? "",
    selected: declarations["--selected"] ?? "",
    drop: declarations["--drop-bg"] ?? "",
  };
}

describe("the accent setting", () => {
  it("lives in the device settings under ui.accent", () => {
    expect(ACCENT_KEY).toBe("ui.accent");
  });

  it("accepts the nine palette names and treats everything else as the default blue", () => {
    expect(DEFAULT_ACCENT).toBe("blue");
    for (const name of PALETTE_COLORS) {
      expect(parseAccent(name)).toBe(name);
    }
    for (const value of [null, undefined, "", "Blue", "GREEN", "teal", "default", 1, true, ["red"], { accent: "red" }]) {
      expect(parseAccent(value), JSON.stringify(value)).toBe("blue");
    }
  });

  it("leaves the root without the attribute for the default and names the others", () => {
    expect(accentAttribute("blue")).toBeUndefined();
    for (const name of PALETTE_COLORS.filter((color) => color !== "blue")) {
      expect(accentAttribute(name)).toBe(name);
    }
  });
});

describe("the accent table", () => {
  it("has the nine palette names, in the palette's order, each with a light and a dark set", () => {
    expect(Object.keys(ACCENT_COLORS)).toEqual([...PALETTE_COLORS]);
    for (const name of PALETTE_COLORS) {
      expect(Object.keys(ACCENT_COLORS[name]).sort(), name).toEqual(["dark", "light"]);
    }
  });

  it("gives every name its own colour in each theme", () => {
    for (const theme of THEMES) {
      const accents = PALETTE_COLORS.map((name) => ACCENT_COLORS[name][theme].accent);
      expect(new Set(accents).size, theme).toBe(PALETTE_COLORS.length);
    }
  });

  it("keeps the blue the app always had, light and dark", () => {
    expect(ACCENT_COLORS.blue.light).toEqual({
      accent: "#3b5bdb",
      hover: "#3050c4",
      text: "#ffffff",
      selected: "#e6ebfc",
      drop: "#eef1fd",
    });
    expect(ACCENT_COLORS.blue.dark).toEqual({
      accent: "#7c97ff",
      hover: "#93a9ff",
      text: "#0b1020",
      selected: "#26304f",
      drop: "#222a44",
    });
  });
});

describe("the contrast of the accents", () => {
  const surfaces = (theme: AccentTheme): { bg: string; surface: string } => {
    const { declarations } = ruleFor(BASE_SELECTOR[theme]);
    return { bg: declarations["--bg"] ?? "", surface: declarations["--surface"] ?? "" };
  };

  for (const name of PALETTE_COLORS) {
    for (const theme of THEMES) {
      it(`${name} in the ${theme} theme: text on the accent, and the accent as text, reach AA (4.5:1)`, () => {
        const shades = ACCENT_COLORS[name][theme];
        const { bg, surface } = surfaces(theme);
        // A primary button, at rest and under the pointer.
        expect(contrast(shades.text, shades.accent), "text on the accent").toBeGreaterThanOrEqual(AA);
        expect(contrast(shades.text, shades.hover), "text on the hover").toBeGreaterThanOrEqual(AA);
        // Links, the active tab's label, the focus ring and the rest that draw with the accent over the page,
        // the panels, a selected row and a drop target.
        for (const colour of [shades.accent, shades.hover]) {
          for (const [what, ground] of [
            ["the page", bg],
            ["a panel", surface],
            ["a selected row", shades.selected],
            ["a drop target", shades.drop],
          ] as const) {
            expect(contrast(colour, ground), `${colour} on ${what}`).toBeGreaterThanOrEqual(AA);
          }
        }
      });
    }
  }
});

describe("app.css says what the table says", () => {
  it("has the default (blue) in the three theme rules, as before", () => {
    for (const [theme, selector] of Object.entries(BASE_SELECTOR) as [AccentTheme, string][]) {
      expect(shadesOf(ruleFor(selector)), theme).toEqual(ACCENT_COLORS.blue[theme]);
    }
    expect(shadesOf(ruleFor(':root:not([data-theme="light"])'))).toEqual(ACCENT_COLORS.blue.dark);
    // The focus ring is the accent.
    expect(ruleFor(":root").declarations["--focus"]).toBe(ACCENT_COLORS.blue.light.accent);
    expect(ruleFor(':root[data-theme="dark"]').declarations["--focus"]).toBe(ACCENT_COLORS.blue.dark.accent);
  });

  for (const name of PALETTE_COLORS) {
    it(`gives ${name} its light, system-dark and dark colours, for the root and for its swatch`, () => {
      const selectors = accentSelectors(name);
      expect(shadesOf(ruleFor(selectors.light))).toEqual(ACCENT_COLORS[name].light);
      expect(shadesOf(ruleFor(selectors["system-dark"]))).toEqual(ACCENT_COLORS[name].dark);
      expect(shadesOf(ruleFor(selectors.dark))).toEqual(ACCENT_COLORS[name].dark);
      for (const selector of Object.values(selectors)) {
        expect(ruleFor(selector).declarations["--focus"], selector).toBe("var(--accent)");
      }
    });
  }

  it("puts the rules for the system's dark scheme inside one media query and the others outside it", () => {
    // The section starts at its comment; the media block is the first rule at column 0 that opens with `@media`.
    const start = CSS.indexOf("/* Accent colours (ADR-0040)");
    expect(start).toBeGreaterThan(0);
    const media = /@media \(prefers-color-scheme: dark\) \{[\s\S]*?\n\}\n/u.exec(CSS.slice(start));
    expect(media).not.toBeNull();
    const inside = (media?.[0] ?? "").replace(/\s+/gu, " ");
    for (const name of PALETTE_COLORS) {
      const selectors = accentSelectors(name);
      expect(inside).toContain(selectors["system-dark"]);
      expect(inside).not.toContain(selectors.light);
      expect(inside).not.toContain(selectors.dark);
    }
  });

  it("draws the text selection of a PDF with the accent and not a fixed blue", () => {
    expect(CSS).not.toContain("rgba(59, 91, 219");
    expect(ruleFor(".pdf-text-layer ::selection").declarations["background"]).toBe(
      "color-mix(in srgb, var(--accent) 35%, transparent)",
    );
  });
});

describe("the settings panel", () => {
  it("leaves room between its fieldsets (theme, accent, language, recent pages)", () => {
    expect(CSS).toMatch(/\.panel > \.fieldset \+ \.fieldset\s*\{\s*margin-top:\s*18px;\s*\}/u);
  });
});
