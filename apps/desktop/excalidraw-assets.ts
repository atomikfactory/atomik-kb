// Excalidraw's assets in the build (the whiteboard block, ADR-0033), as a Vite plugin. Kept out of
// vite.config.ts so that a test can check it against the library that is installed.

import { createReadStream, existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import type { Plugin } from "vite";

/** Where the app finds Excalidraw's canvas fonts at run time (`EXCALIDRAW_ASSET_PATH`, packages/ui/src/editor/excalidraw-engine.ts). */
const EXCALIDRAW_BASE = "excalidraw";
/** The font family left out of the build: 13 MB of CJK subsets, loaded only for text in those scripts (the system's font draws it instead). */
const EXCALIDRAW_SKIPPED_FONTS = new Set(["Xiaolai"]);

/**
 * The library loads its Mermaid-to-drawing converter with `import("@excalidraw/mermaid-to-excalidraw")`,
 * which brings a second copy of Mermaid (the app has its own, for diagram blocks) and every diagram type:
 * megabytes the whiteboard never uses, because its menu does not offer that command (WhiteboardEditor.tsx).
 * In the build the specifier resolves to this stub, which only fails if something calls it.
 */
const MERMAID_SPECIFIER = "@excalidraw/mermaid-to-excalidraw";
const MERMAID_STUB = "\0atomik-excalidraw-no-mermaid";

/** Every file under `dir`, as paths relative to it. */
function filesUnder(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      found.push(...filesUnder(full).map((file) => join(entry, file)));
    } else {
      found.push(entry);
    }
  }
  return found;
}

/** The directory of the library's production build (`dist/prod`). */
export function excalidrawDist(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  // The library is a dependency of packages/ui (pnpm keeps it out of apps/desktop's own node_modules).
  return dirname(createRequire(resolve(here, "../../packages/ui/package.json")).resolve("@excalidraw/excalidraw"));
}

/**
 * The library's built-in address for a font it cannot find locally, a template literal with a nested one:
 * `https://esm.sh/${pkg ? `${name}@${version}` : "..."}/dist/prod/`.
 */
const FALLBACK = /`https:\/\/esm\.sh\/\$\{[^`]*`[^`]*`[^`]*\}\/dist\/prod\/`/u;

/**
 * `code` of a library file with that address replaced by the app's own font path; `null` for a file
 * that has no address of that host. Throws when the host appears in any other shape (the library changed:
 * the plugin must be updated rather than let the app ship a fetch to another host).
 */
export function rewriteFontFallback(code: string): string | null {
  if (!code.includes("https://esm.sh/")) {
    return null;
  }
  const fallback = FALLBACK.exec(code);
  if (fallback === null || code.split("https://esm.sh/").length !== 2) {
    throw new Error("Excalidraw's font fallback address was not found in the expected form: update excalidraw-assets.ts");
  }
  return code.replace(fallback[0], `(window.location.origin + "/${EXCALIDRAW_BASE}/")`);
}

/**
 * Excalidraw (the whiteboard block, ADR-0033) draws its canvas text in fonts it loads from
 * `window.EXCALIDRAW_ASSET_PATH`, by default a CDN. The app sets the path to its own origin
 * (`/excalidraw/`) and this plugin makes the files be there:
 *
 * - the build copies the library's fonts to `excalidraw/fonts/` (the CJK family `Xiaolai` aside);
 * - the dev server serves them from the package;
 * - the library's built-in fallback address, a CDN used when a font file is missing, is rewritten to the
 *   same local path, so that even a missing font (a CJK subset) asks nobody else. The build fails if the
 *   library no longer has the shape this expects, rather than ship a fetch to another host.
 */
export function excalidrawAssets(): Plugin {
  const fonts = join(excalidrawDist(), "fonts");
  const wanted = (file: string): boolean => !EXCALIDRAW_SKIPPED_FONTS.has(file.split(sep)[0] ?? "");
  return {
    name: "atomik-excalidraw-assets",
    // Before Vite's own resolver, so that the stub below is found first.
    enforce: "pre",
    resolveId(source) {
      return source === MERMAID_SPECIFIER ? MERMAID_STUB : null;
    },
    load(id) {
      return id === MERMAID_STUB
        ? "export const parseMermaidToExcalidraw = () => Promise.reject(new Error(\"Mermaid import is not part of this build\"));"
        : null;
    },
    configureServer(server) {
      server.middlewares.use(`/${EXCALIDRAW_BASE}/fonts/`, (request, response, next) => {
        const file = resolve(fonts, decodeURIComponent(((request as { url?: string }).url ?? "").split("?")[0] ?? "").replace(/^\/+/u, ""));
        if (!file.startsWith(fonts + sep) || !existsSync(file) || !statSync(file).isFile()) {
          next();
          return;
        }
        // Connect's response is a Node.js ServerResponse, whose types this package does not have.
        (response as { setHeader: (name: string, value: string) => void }).setHeader("Content-Type", "font/woff2");
        createReadStream(file).pipe(response);
      });
    },
    generateBundle() {
      if (!existsSync(fonts)) {
        this.error(`Excalidraw's fonts are not at ${fonts}`);
      }
      for (const file of filesUnder(fonts).filter(wanted)) {
        this.emitFile({
          type: "asset",
          fileName: `${EXCALIDRAW_BASE}/fonts/${relative(fonts, join(fonts, file)).split(sep).join("/")}`,
          source: readFileSync(join(fonts, file)),
        });
      }
    },
    transform(code, id) {
      if (!/[\\/]@excalidraw[\\/]excalidraw[\\/]dist[\\/](?:prod|dev)[\\/][^\\/]+\.js$/u.test(id.split("?")[0] ?? id)) {
        return null;
      }
      return rewriteFontFallback(code);
    },
  };
}
