// What the three charts share (ADR-0055, `docs/architecture/dashboards.md` section 4.4): the props every drawing takes, the
// interaction of a chart (one tab stop, the arrow keys move between marks, Enter opens the mark's pages, a tooltip on hover and
// on focus) and the tooltip itself. A chart draws its own marks and says where each mark's tooltip points; the rest is here.

import { useCallback, useId, useRef, useState } from "react";
import type { HTMLAttributes, KeyboardEvent, MouseEvent, RefObject } from "react";

import type { Anchor } from "../../../components/FloatingPanel";
import type { ChartGroup } from "../chart-model";

/** What a tooltip says about a group: its name, then lines (the measure's value, the share, the pages). */
export interface ChartDescription {
  readonly title: string;
  readonly lines: readonly string[];
}

/** The props of every chart drawing (`BarChart`, `LineChart`, `DonutChart`). */
export interface ChartViewProps {
  readonly groups: readonly ChartGroup[];
  /** The size of the drawing in pixels. */
  readonly width: number;
  readonly height: number;
  /** The groups are dates: one series colour. */
  readonly dated: boolean;
  /** The accessible name of the drawing: the chart's name with its groups and values. */
  readonly label: string;
  /** A value as the table footer writes a number. */
  readonly formatValue: (value: number) => string;
  /** What the tooltip of a group says. */
  readonly describe: (group: ChartGroup) => ChartDescription;
  /** A click or Enter on the mark `index`: where its popover opens (viewport coordinates). */
  readonly onSelect: (index: number, anchor: Anchor) => void;
}

/**
 * The attributes that give a mark (or a legend swatch) its colour: an option's own, else (`single`) the one series colour of a
 * chart that has no use for colour to tell its marks apart, else the palette slot of the group; the neutral ones for "Other"
 * and for the pages without a value.
 */
export function colorAttributes(group: ChartGroup, single: boolean): Record<string, string> {
  if (group.kind === "other") {
    return { "data-series": "other" };
  }
  if (group.kind === "empty") {
    return { "data-series": "empty" };
  }
  if (group.color !== undefined) {
    return { "data-color": group.color };
  }
  return { "data-series": single ? "1" : String(group.paletteIndex + 1) };
}

/** The keyboard step of an arrow key: next (right, down) or previous (left, up). */
function stepOf(key: string): 1 | -1 | null {
  if (key === "ArrowRight" || key === "ArrowDown") {
    return 1;
  }
  return key === "ArrowLeft" || key === "ArrowUp" ? -1 : null;
}

/** The mouse handlers of a mark or a legend entry. */
export interface PointerHandlers {
  readonly onPointerEnter: () => void;
  readonly onPointerLeave: () => void;
  readonly onClick: (event: MouseEvent<Element>) => void;
}

export interface ChartInteraction {
  /** The mark the tooltip is on (hovered or reached with the keys), `null` for none. */
  readonly active: number | null;
  /** The id of the tooltip, for `aria-describedby`. */
  readonly tooltipId: string;
  /** The props of the element that is the chart's one tab stop. */
  readonly plotProps: HTMLAttributes<HTMLDivElement> & { ref: RefObject<HTMLDivElement | null> };
  /** The props of a mark: its index, its state and the mouse. */
  readonly markProps: (index: number) => PointerHandlers & { readonly "data-mark": number; readonly "data-active": "true" | undefined };
  /** The mouse part alone, for a legend entry. */
  readonly pointerProps: (index: number) => PointerHandlers;
}

/**
 * The interaction of a chart with `count` marks. The plot is one tab stop; the arrow keys (and Home and End) move the tooltip
 * between marks, Enter or Space opens the mark's pages (`onSelect`), Escape hides the tooltip. The mouse shows the tooltip of the
 * mark under it and a click opens its pages. A focus that comes from a press does not start at the first mark.
 */
export function useChartInteraction(count: number, onSelect: (index: number, anchor: Anchor) => void): ChartInteraction {
  const tooltipId = useId();
  const plot = useRef<HTMLDivElement>(null);
  const [raw, setRaw] = useState<number | null>(null);
  // The mark the keys last reached: where a focus that comes back (a popover closing) picks up.
  const last = useRef(0);
  const pressed = useRef(false);
  const active = raw !== null && raw < count ? raw : null;

  const activate = useCallback((index: number | null): void => {
    if (index !== null) {
      last.current = index;
    }
    setRaw(index);
  }, []);

  /** Where the popover of a mark opens for the keyboard: under the mark. */
  const anchorOfMark = (index: number): Anchor => {
    const mark = plot.current?.querySelector(`[data-mark="${String(index)}"]`);
    const rect = (mark ?? plot.current)?.getBoundingClientRect();
    return { x: rect?.left ?? 0, y: (rect?.bottom ?? 0) + 4 };
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (count === 0 || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    const step = stepOf(event.key);
    if (step !== null) {
      event.preventDefault();
      activate(active === null ? (step === 1 ? 0 : count - 1) : Math.min(count - 1, Math.max(0, active + step)));
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      activate(event.key === "Home" ? 0 : count - 1);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const index = active ?? last.current;
      activate(index);
      onSelect(index, anchorOfMark(index));
    } else if (event.key === "Escape" && active !== null) {
      setRaw(null);
    }
  };

  const plotProps: ChartInteraction["plotProps"] = {
    ref: plot,
    tabIndex: 0,
    onPointerDown: () => {
      pressed.current = true;
    },
    onPointerUp: () => {
      pressed.current = false;
    },
    onPointerCancel: () => {
      pressed.current = false;
    },
    onFocus: () => {
      if (pressed.current || count === 0) {
        return;
      }
      activate(Math.min(last.current, count - 1));
    },
    onBlur: (event) => {
      if (!(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget))) {
        setRaw(null);
      }
    },
    onKeyDown,
  };

  const pointerProps: ChartInteraction["pointerProps"] = (index) => ({
    onPointerEnter: () => {
      activate(index);
    },
    onPointerLeave: () => {
      setRaw((current) => (current === index ? null : current));
    },
    onClick: (event) => {
      activate(index);
      onSelect(index, { x: event.clientX, y: event.clientY });
    },
  });

  return {
    active,
    tooltipId,
    plotProps,
    pointerProps,
    markProps: (index) => ({
      ...pointerProps(index),
      "data-mark": index,
      "data-active": active === index ? "true" : undefined,
    }),
  };
}

/** A tooltip over the plot at `x, y` (pixels in the plot): above the point, kept inside the plot by which side it grows to. */
export function ChartTooltip({
  id,
  x,
  y,
  width,
  description,
}: {
  id: string;
  x: number;
  y: number;
  /** The plot's width. */
  width: number;
  description: ChartDescription;
}) {
  const align = x < width * 0.25 ? "start" : x > width * 0.75 ? "end" : "center";
  return (
    <div
      id={id}
      role="tooltip"
      className="chart-tooltip"
      data-align={align}
      data-below={y < 64 ? "true" : undefined}
      style={{ left: x, top: y }}
    >
      <strong className="chart-tooltip-title">{description.title}</strong>
      {description.lines.map((line) => (
        <span key={line} className="chart-tooltip-line">
          {line}
        </span>
      ))}
    </div>
  );
}
