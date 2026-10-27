// The drawing engine of the whiteboard block (ADR-0033): Excalidraw (`@excalidraw/excalidraw`, MIT,
// pinned), loaded with a dynamic `import()` the first time a whiteboard is shown, so it is never part
// of the main bundle (as pdf.js in pdf-engine.ts, Mermaid in mermaid.ts and KaTeX in math.ts).
//
// Nothing is fetched from the network:
// - Excalidraw loads the fonts of the canvas from `window.EXCALIDRAW_ASSET_PATH` (default: a CDN).
//   It is set to a path of the app's own origin before the library loads; `apps/desktop/vite.config.ts`
//   (`excalidrawAssets`) copies the library's fonts to `excalidraw/fonts/` in the build, serves them in
//   development and points the library's built-in CDN fallback at the same path.
// - the stylesheet is imported lazily with the library (the bundler makes it a lazily loaded chunk and
//   emits its font as a local file); the CSP needs nothing new (`font-src 'self'`, `script-src 'self'`).
// - the options the views pass turn off what would reach another host (the library, sharing, embeds,
//   AI, images): see `WhiteboardEditor.tsx`.
//
// The views talk to the small `ExcalidrawEngine` interface below, not to the library's whole export, so
// tests replace this module and the engine stays swappable. The library's published type declarations
// refer to packages of its own monorepo that are not installed (`@excalidraw/utils`, `@excalidraw/math`),
// so the functions that use them are typed here by hand and wrapped once.

import type { ComponentType } from "react";

import type * as ExcalidrawModule from "@excalidraw/excalidraw";
import type { ExcalidrawProps } from "@excalidraw/excalidraw/types";

/** Where the library finds its fonts: a directory of the app's own origin. */
export const EXCALIDRAW_ASSET_PATH = "/excalidraw/";

/** The options of {@link ExcalidrawEngine.exportToSvg} the picture uses. */
export interface SvgExport {
  elements: readonly unknown[];
  appState: { exportBackground: boolean; exportWithDarkMode: boolean; viewBackgroundColor: string };
  exportPadding: number;
}

/** The part of the library the whiteboard views use. */
export interface ExcalidrawEngine {
  /** The editor component. */
  Excalidraw: ComponentType<ExcalidrawProps>;
  /** The menu of the editor, so that a menu of our own replaces the default one (links, sign-in, ...). */
  MainMenu: typeof ExcalidrawModule.MainMenu;
  /** The drawing as an SVG element (fitted to its content, fonts not inlined: nothing is fetched). */
  exportToSvg: (options: SvgExport) => Promise<SVGSVGElement>;
  /** Elements as the library keeps them (fields an older writer left out are filled in), `local` being the scene's. */
  restoreElements: (elements: readonly unknown[]) => readonly unknown[];
  /** The scene update that does not enter the editor's undo history (a change that came from the document). */
  neverCapture: ExcalidrawModuleCapture;
}

/** What `updateScene` takes as `captureUpdate`. */
export type ExcalidrawModuleCapture = typeof ExcalidrawModule.CaptureUpdateAction.NEVER;

declare global {
  interface Window {
    EXCALIDRAW_ASSET_PATH?: string | string[];
  }
}

let override: Promise<ExcalidrawEngine> | null = null;
let loading: Promise<ExcalidrawEngine> | null = null;

/** A test seam: the views get `engine` (`null` brings the library back), so tests never load it. */
export function setExcalidrawEngine(engine: ExcalidrawEngine | null): void {
  override = engine === null ? null : Promise.resolve(engine);
}

/* The declarations of the library lose the types of the functions it re-exports from its own monorepo
   (`exportToSvg`) and of the elements, so calls into it are unchecked: this is the one place they are made. */
/* eslint-disable @typescript-eslint/no-unsafe-call */
async function adapt(): Promise<ExcalidrawEngine> {
  window.EXCALIDRAW_ASSET_PATH = EXCALIDRAW_ASSET_PATH;
  const [library] = await Promise.all([import("@excalidraw/excalidraw"), import("@excalidraw/excalidraw/index.css")]);
  return {
    Excalidraw: library.Excalidraw,
    MainMenu: library.MainMenu,
    exportToSvg: async (options) => {
      const svg: unknown = await library.exportToSvg({
        elements: options.elements,
        appState: options.appState,
        files: null,
        exportPadding: options.exportPadding,
        skipInliningFonts: true,
      });
      if (!(svg instanceof SVGSVGElement)) {
        throw new Error("the drawing did not export to SVG");
      }
      return svg;
    },
    restoreElements: (elements) => library.restoreElements(elements as never, null),
    neverCapture: library.CaptureUpdateAction.NEVER,
  };
}
/* eslint-enable @typescript-eslint/no-unsafe-call */

/** Loads the library once; a failed load (a missing chunk) is tried again by the next view. */
export function loadExcalidraw(): Promise<ExcalidrawEngine> {
  if (override !== null) {
    return override;
  }
  loading ??= adapt().catch((error: unknown) => {
    loading = null;
    throw error;
  });
  return loading;
}
