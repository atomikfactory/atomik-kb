// The bar chart of a chart widget (ADR-0055, `docs/architecture/dashboards.md` section 4.4): vertical by default, horizontal
// with `horizontal`. Bars are at most 28 px thick, rounded at the value's end and square on the baseline; the value axis has
// nice ticks from 0 (or the minimum below it) and hairline grid lines; category labels are cut with an ellipsis and carry the
// full text in a tooltip, and are thinned when the bars are too close for all of them. A group without a value is drawn as 0.

import { activeLocale } from "../../../i18n";
import { CHART_PALETTE_SIZE } from "../chart-model";
import type { ChartGroup } from "../chart-model";
import { formatTick, labelWidth, niceTicks, roundedTipPath, truncateLabel } from "../chart-scale";
import { ChartTooltip, colorAttributes, useChartInteraction } from "./chart-parts";
import type { ChartViewProps } from "./chart-parts";

const MAX_BAR = 28;
const CORNER = 4;
const LABEL_HEIGHT = 14;

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}

/** Which category labels are drawn: every `stride`-th, so that neighbours do not touch. */
function strideFor(band: number, needed: number): number {
  return band >= needed ? 1 : Math.ceil(needed / Math.max(band, 1));
}

/**
 * One bar's rectangle: `thickness` thick around `centre`, from the baseline (`start`) to the value's end (`end`), both along the
 * value axis in pixels. A value that is not 0 is at least one pixel long. `forward` says the bar grows towards larger
 * coordinates (a positive value of a horizontal bar, a negative one of a vertical bar).
 */
function barRect(
  vertical: boolean,
  centre: number,
  thickness: number,
  start: number,
  end: number,
  forward: boolean,
): { x: number; y: number; w: number; h: number } {
  const length = Math.max(1, Math.abs(end - start));
  const from = forward ? start : start - length;
  return vertical ? { x: centre - thickness / 2, y: from, w: thickness, h: length } : { x: from, y: centre - thickness / 2, w: length, h: thickness };
}

export interface BarChartProps extends ChartViewProps {
  readonly horizontal: boolean;
}

export function BarChart({ groups, width, height, dated, label, describe, onSelect, horizontal }: BarChartProps) {
  const { active, tooltipId, plotProps, markProps } = useChartInteraction(groups.length, onSelect);
  const locale = activeLocale();
  const count = Math.max(1, groups.length);
  const values = groups.map((group) => group.value ?? 0);
  const low = Math.min(0, ...values);
  const high = Math.max(0, ...values);
  const counts = values.every((value) => Number.isInteger(value));
  // Bars tell themselves apart by their labels: a date source (a time axis) and more groups than the palette has hues take one colour.
  const single = dated || groups.length > CHART_PALETTE_SIZE;

  // Vertical: the value axis is on the left (its width is its widest tick label), the labels under the bars.
  // Horizontal: the labels are on the left (as wide as the longest, within 40 % of the chart), the value axis under the bars.
  const top = horizontal ? 6 : 8;
  const bottom = 24;
  const right = horizontal ? 14 : 8;
  const plotHeight = Math.max(10, height - top - bottom);
  const verticalTicks = niceTicks(low, high, clamp(Math.floor(plotHeight / 40), 2, 6), counts);
  const verticalLabels = verticalTicks.ticks.map((tick) => formatTick(tick, locale));
  const left = horizontal
    ? clamp(Math.max(0, ...groups.map((group) => labelWidth(group.label))) + 14, 56, Math.max(56, width * 0.4))
    : Math.ceil(Math.max(0, ...verticalLabels.map((text) => labelWidth(text)))) + 12;
  const plotWidth = Math.max(10, width - left - right);
  const horizontalTicks = niceTicks(low, high, clamp(Math.floor(plotWidth / 70), 2, 5), counts);
  const ticks = horizontal ? horizontalTicks : verticalTicks;
  const span = ticks.max - ticks.min || 1;
  const along = (value: number): number =>
    horizontal ? left + (plotWidth * (value - ticks.min)) / span : top + plotHeight * (1 - (value - ticks.min) / span);
  const baseline = along(0);
  const band = (horizontal ? plotHeight : plotWidth) / count;
  const thickness = clamp(band * 0.7, 2, MAX_BAR);
  const stride = horizontal ? strideFor(band, LABEL_HEIGHT) : strideFor(band, 44);
  const maxChars = horizontal ? Math.floor((left - 14) / 6.2) : Math.max(3, Math.floor((band * stride - 6) / 6.2));

  const marks = groups.map((group: ChartGroup, index) => {
    const centre = horizontal ? top + band * (index + 0.5) : left + band * (index + 0.5);
    const value = group.value ?? 0;
    const reach = along(value);
    const drawn = value !== 0 && group.value !== null;
    const rect = drawn ? barRect(!horizontal, centre, thickness, baseline, reach, horizontal === (value >= 0)) : null;
    const tip = horizontal ? (value >= 0 ? "right" : "left") : value >= 0 ? "up" : "down";
    return { group, index, centre, reach, rect, tip } as const;
  });

  const anchor = active === null ? null : marks[active];
  const tooltipAt = anchor === null || anchor === undefined ? null : horizontal ? { x: anchor.reach, y: anchor.centre - thickness / 2 } : { x: anchor.centre, y: anchor.reach };

  return (
    <div
      {...plotProps}
      className="chart-plot"
      role="img"
      aria-label={label}
      aria-describedby={active === null ? undefined : tooltipId}
      data-chart="bar"
      data-orientation={horizontal ? "horizontal" : "vertical"}
      data-has-active={active === null ? undefined : "true"}
      style={{ width, height }}
    >
      <svg className="chart-svg" width={width} height={height} viewBox={`0 0 ${String(width)} ${String(height)}`} aria-hidden="true" focusable="false">
        {ticks.ticks.map((tick) => {
          const at = along(tick);
          return (
            <g key={tick} className="chart-axis">
              {horizontal ? (
                <line className={tick === 0 ? "chart-baseline" : "chart-grid"} x1={at} x2={at} y1={top} y2={top + plotHeight} />
              ) : (
                <line className={tick === 0 ? "chart-baseline" : "chart-grid"} x1={left} x2={left + plotWidth} y1={at} y2={at} />
              )}
              {horizontal ? (
                <text className="chart-tick" x={at} y={top + plotHeight + 15} textAnchor="middle">
                  {formatTick(tick, locale)}
                </text>
              ) : (
                <text className="chart-tick" x={left - 6} y={at} dy="0.32em" textAnchor="end">
                  {formatTick(tick, locale)}
                </text>
              )}
            </g>
          );
        })}
        {marks.map(({ group, index, centre, rect, tip }) => (
          <g key={group.key} className="chart-bar" {...markProps(index)} data-key={group.key} data-value={group.value ?? ""}>
            {horizontal ? (
              <rect className="chart-hit" x={left} y={centre - band / 2} width={plotWidth} height={band} />
            ) : (
              <rect className="chart-hit" x={centre - band / 2} y={top} width={band} height={plotHeight} />
            )}
            {rect !== null && (
              <path
                className="chart-mark"
                d={roundedTipPath(rect.x, rect.y, rect.w, rect.h, tip, CORNER)}
                {...colorAttributes(group, single)}
              />
            )}
            {index % stride === 0 && (
              <text
                className="chart-category"
                x={horizontal ? left - 8 : centre}
                y={horizontal ? centre : top + plotHeight + 16}
                dy={horizontal ? "0.32em" : undefined}
                textAnchor={horizontal ? "end" : "middle"}
              >
                {truncateLabel(group.label, maxChars)}
                {truncateLabel(group.label, maxChars) !== group.label && <title>{group.fullLabel}</title>}
              </text>
            )}
          </g>
        ))}
      </svg>
      {anchor !== null && anchor !== undefined && tooltipAt !== null && (
        <ChartTooltip id={tooltipId} x={tooltipAt.x} y={tooltipAt.y} width={width} description={describe(anchor.group)} />
      )}
    </div>
  );
}
