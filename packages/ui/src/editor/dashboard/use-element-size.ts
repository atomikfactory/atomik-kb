// The size of an element in pixels, kept up to date (a `ResizeObserver`), for the chart widgets: the charts draw SVG at the
// size of their body. Before the first measure, and where there is no layout (a test, a browser without the observer), the
// size is `fallback`, so a chart is drawn at a plausible size instead of none.

import { useLayoutEffect, useRef, useState } from "react";
import type { RefObject } from "react";

/** A size in pixels. */
export interface Size {
  readonly width: number;
  readonly height: number;
}

/** A ref for the element to measure and its size (`fallback` until it has one). */
export function useElementSize<T extends HTMLElement>(fallback: Size): [RefObject<T | null>, Size] {
  const ref = useRef<T>(null);
  const [size, setSize] = useState<Size>(fallback);

  useLayoutEffect(() => {
    const element = ref.current;
    if (element === null) {
      return;
    }
    const measured = (width: number, height: number): void => {
      if (width > 0 && height > 0) {
        const next = { width: Math.round(width), height: Math.round(height) };
        setSize((current) => (current.width === next.width && current.height === next.height ? current : next));
      }
    };
    const rect = element.getBoundingClientRect();
    measured(rect.width, rect.height);
    if (typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1];
      if (entry !== undefined) {
        measured(entry.contentRect.width, entry.contentRect.height);
      }
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, []);

  return [ref, size];
}
