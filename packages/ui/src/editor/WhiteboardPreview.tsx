// The inline picture of a whiteboard (ADR-0033): the drawing as SVG, fitted to the block's height,
// drawn by Excalidraw's own export. It is static (the full-screen editor is where it is edited), so a
// page with several whiteboards costs a few SVG trees and not several editors. The library is loaded
// only when there is something to draw; an empty drawing shows its hint and loads nothing.
//
// The SVG is built with DOM calls by the library (text through `textContent`), never from a string of
// markup, and the elements are drawn without their links and images: a click on the picture opens the
// editor, never an address, and the drawing references no attachment.

import { useEffect, useRef, useState } from "react";

import { t } from "../i18n";
import { loadExcalidraw } from "./excalidraw-engine";
import { currentDiagramTheme, onDiagramThemeChange } from "./mermaid";
import type { DiagramTheme } from "./mermaid";
import type { WhiteboardValue } from "@atomik-kb/core-client";

/** Space around the drawing in the picture, in drawing units. */
const PADDING = 16;

/** The element types the drawing does not show (images are off in v1, embeds fetch pages). */
const HIDDEN_TYPES: ReadonlySet<string> = new Set(["image", "embeddable", "iframe"]);

/** The elements to draw: not deleted, not hidden, and no link. */
export function drawableElements(elements: readonly WhiteboardValue[]): WhiteboardValue[] {
  return elements
    .filter((element) => element.isDeleted !== true && !HIDDEN_TYPES.has(typeof element.type === "string" ? element.type : ""))
    .map((element) => (element.link === null || element.link === undefined ? element : { ...element, link: null }));
}

/** The theme the page shows now, following the app's setting and the system's. */
export function useAppTheme(): DiagramTheme {
  const [theme, setTheme] = useState(currentDiagramTheme);
  useEffect(
    () =>
      onDiagramThemeChange(() => {
        setTheme(currentDiagramTheme());
      }),
    [],
  );
  return theme;
}

export interface WhiteboardPreviewProps {
  elements: readonly WhiteboardValue[];
  height: number;
}

/** The drawing as a picture; the hint when there is nothing to draw. */
export function WhiteboardPreview({ elements, height }: WhiteboardPreviewProps) {
  const theme = useAppTheme();
  const host = useRef<HTMLDivElement>(null);
  const drawable = drawableElements(elements);
  const empty = drawable.length === 0;
  const [failure, setFailure] = useState<string | null>(null);
  // The theme the picture on show was drawn in: it is drawn again when the theme changes (print mode waits for that).
  const [drawnTheme, setDrawnTheme] = useState<DiagramTheme | null>(null);
  const drawn = drawnTheme !== null;

  useEffect(() => {
    const target = host.current;
    if (target === null || empty) {
      return;
    }
    let active = true;
    loadExcalidraw()
      .then(async (engine) => {
        // Restoring repairs elements an older or foreign writer left incomplete; it never touches the stored ones.
        const svg = await engine.exportToSvg({
          elements: engine.restoreElements(drawable),
          appState: { exportBackground: false, exportWithDarkMode: theme === "dark", viewBackgroundColor: "transparent" },
          exportPadding: PADDING,
        });
        if (!active) {
          return;
        }
        svg.setAttribute("width", "100%");
        svg.setAttribute("height", "100%");
        svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
        svg.removeAttribute("style");
        svg.setAttribute("focusable", "false");
        target.replaceChildren(svg);
        setFailure(null);
        setDrawnTheme(theme);
      })
      .catch((error: unknown) => {
        if (active) {
          setFailure(error instanceof Error ? error.message : String(error));
        }
      });
    return () => {
      active = false;
    };
    // `drawable` is rebuilt every render; the elements themselves are the dependency.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [elements, empty, theme]);

  // An emptied drawing drops its picture (and a locked app drops the view with it).
  useEffect(() => {
    if (empty) {
      host.current?.replaceChildren();
    }
  }, [empty]);

  return (
    <div
      className="whiteboard-stage"
      style={{ height }}
      data-state={empty ? "empty" : failure !== null ? "failed" : drawn ? "drawn" : "loading"}
      data-busy={!empty && failure === null && drawn && drawnTheme !== theme ? "" : undefined}
    >
      <div ref={host} className="whiteboard-svg" aria-hidden={empty ? "true" : undefined} />
      {empty && <p className="whiteboard-hint">{t("whiteboard.empty")}</p>}
      {!empty && failure !== null && (
        <p className="whiteboard-hint" role="alert">
          {t("whiteboard.error", { message: failure })}
        </p>
      )}
      {!empty && failure === null && !drawn && (
        <p className="whiteboard-hint" role="status">
          {t("whiteboard.loading")}
        </p>
      )}
    </div>
  );
}
