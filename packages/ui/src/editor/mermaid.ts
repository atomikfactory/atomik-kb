// Offline Mermaid rendering for `mermaid` code blocks (code-block.ts).
//
// Mermaid is a large library, so it is loaded with a dynamic `import()` the first time a
// diagram is drawn: the bundler makes it (and every diagram type) lazily loaded chunks of
// the app, never part of the main bundle, and nothing is fetched at runtime (the desktop
// CSP allows only local resources). It runs with `securityLevel: "strict"`: labels are
// encoded, the SVG is sanitised, clicks and links in diagrams are off, and a diagram's own
// `%%{init}%%` directive cannot lower the level. This module draws; it never touches the
// document, so nothing about a diagram is stored beyond the block's text and language.

import type Mermaid from "mermaid";

/** The two looks a diagram has: Mermaid's `default` and `dark` themes. */
export type DiagramTheme = "light" | "dark";

/** Draws `source` as SVG markup in `theme`, or rejects with the reason it cannot. */
export type DiagramRenderer = (source: string, theme: DiagramTheme) => Promise<string>;

type MermaidApi = typeof Mermaid;

/** The app's text font (`--font` in the stylesheet), so that diagrams read like the page. */
const DIAGRAM_FONT = "system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif";

/** The Mermaid theme for a resolved app theme. */
function mermaidTheme(theme: DiagramTheme): "dark" | "default" {
  return theme === "dark" ? "dark" : "default";
}

const DARK_QUERY = "(prefers-color-scheme: dark)";

function darkMedia(): MediaQueryList | null {
  return typeof window.matchMedia === "function" ? window.matchMedia(DARK_QUERY) : null;
}

/**
 * The theme the page shows now: `data-theme` on `<html>` (`light` or `dark`; the app sets
 * `system` too), else the system's `prefers-color-scheme`.
 */
export function currentDiagramTheme(): DiagramTheme {
  const chosen = document.documentElement.dataset.theme;
  if (chosen === "dark" || chosen === "light") {
    return chosen;
  }
  return darkMedia()?.matches === true ? "dark" : "light";
}

const themeListeners = new Set<() => void>();
let stopWatching: (() => void) | null = null;

/** Starts watching `data-theme` and the system's colour scheme; returns what stops it. */
function watchTheme(): () => void {
  const notify = (): void => {
    for (const listener of [...themeListeners]) {
      listener();
    }
  };
  const observer = new MutationObserver(notify);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  const media = darkMedia();
  media?.addEventListener("change", notify);
  return () => {
    observer.disconnect();
    media?.removeEventListener("change", notify);
  };
}

/**
 * Calls `listener` when the theme the page shows may have changed (the app's theme setting
 * or the system's colour scheme); returns what removes it. One shared watcher runs while
 * there are listeners.
 */
export function onDiagramThemeChange(listener: () => void): () => void {
  themeListeners.add(listener);
  stopWatching ??= watchTheme();
  return () => {
    themeListeners.delete(listener);
    if (themeListeners.size === 0) {
      stopWatching?.();
      stopWatching = null;
    }
  };
}

let override: DiagramRenderer | null = null;

/** A test seam: draws diagrams with `renderer` (`null` brings Mermaid back), so tests never load it. */
export function setDiagramRenderer(renderer: DiagramRenderer | null): void {
  override = renderer;
}

let loading: Promise<MermaidApi> | null = null;

/** Loads Mermaid once; a failed load (a missing chunk) is tried again by the next diagram. */
function loadMermaid(): Promise<MermaidApi> {
  loading ??= import("mermaid").then(
    (module) => module.default,
    (error: unknown) => {
      loading = null;
      throw error;
    },
  );
  return loading;
}

/** The theme Mermaid was last initialised with (its configuration is global). */
let initialisedTheme: DiagramTheme | null = null;
let diagrams = 0;

async function drawWithMermaid(source: string, theme: DiagramTheme): Promise<string> {
  const mermaid = await loadMermaid();
  if (initialisedTheme !== theme) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      suppressErrorRendering: true,
      theme: mermaidTheme(theme),
      fontFamily: DIAGRAM_FONT,
    });
    initialisedTheme = theme;
  }
  diagrams += 1;
  const id = `atomik-diagram-${diagrams}`;
  try {
    const { svg } = await mermaid.render(id, source);
    return svg;
  } finally {
    // Mermaid measures the diagram in a temporary element of `<body>` (and, in sandbox mode,
    // an iframe); a failed draw can leave it behind.
    for (const leftover of [`d${id}`, `i${id}`, id]) {
      document.getElementById(leftover)?.remove();
    }
  }
}

/** Draws one diagram at a time: Mermaid's configuration (the theme) is global to its draws. */
let queue: Promise<unknown> = Promise.resolve();

/**
 * Draws `source` as SVG markup in `theme`. The markup is what Mermaid's strict security
 * level produced (sanitised); it is meant for `innerHTML` and nothing else. Rejects with
 * Mermaid's own error for a diagram it cannot parse.
 */
export function renderDiagram(source: string, theme: DiagramTheme): Promise<string> {
  const draw = queue.then(() => (override === null ? drawWithMermaid(source, theme) : override(source, theme)));
  queue = draw.catch(() => undefined);
  return draw;
}
