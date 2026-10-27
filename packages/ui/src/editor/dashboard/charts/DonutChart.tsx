// The donut chart of a chart widget (ADR-0055, `docs/architecture/dashboards.md` section 4.4): a ring of slices in proportion to
// their values (the data keeps at most eight and one "Other", and leaves out what is not above 0), the total in the centre (the
// value of the slice under the mouse or the keys while there is one), and a legend at the side of the ring or, in a narrow widget,
// under it. A slice is separated from the next by a 2 px gap in the surface colour. The legend's entries answer the mouse as the
// slices do; the keyboard reaches the slices through the plot.

import { labelWidth, polar, slicePath, truncateLabel } from "../chart-scale";
import { ChartTooltip, colorAttributes, useChartInteraction } from "./chart-parts";
import type { ChartViewProps } from "./chart-parts";

/** Below this width the legend goes under the ring. */
const SIDE_LEGEND_MIN_WIDTH = 340;
const HOLE = 0.62;

export interface DonutChartProps extends ChartViewProps {
  /** What the centre says when no slice is active: the sum of the slices (a count or a sum), else the measure over all the pages. */
  readonly total: number | null;
  /** The measure's name, under the total. */
  readonly totalLabel: string;
  /** Whether a slice is a share of the whole (a count or a sum), which the legend and the tooltip then say in percent. */
  readonly shares: boolean;
  readonly locale: string;
}

/** A share in percent, as the locale writes it (`37%`, `0.4%`). */
function percent(share: number, locale: string): string {
  return new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: share < 0.1 ? 1 : 0 }).format(share);
}

export function DonutChart({ groups, width, height, label, formatValue, describe, onSelect, total, totalLabel, shares, locale }: DonutChartProps) {
  const { active, tooltipId, plotProps, markProps, pointerProps } = useChartInteraction(groups.length, onSelect);
  const side = width >= SIDE_LEGEND_MIN_WIDTH;
  const size = Math.max(60, Math.floor(side ? Math.min(height, width * 0.5) : Math.min(width, height * 0.56)));
  const centre = size / 2;
  const outer = Math.max(10, centre - 4);
  const inner = outer * HOLE;
  const sum = groups.reduce((acc, group) => acc + (group.value ?? 0), 0) || 1;

  const slices = groups.map((group, index) => {
    // Where the slice begins: the shares of the ones before it, as a turn of the ring (the last one ends where the ring does).
    const before = groups.slice(0, index).reduce((acc, earlier) => acc + (earlier.value ?? 0), 0) / sum;
    const after = index === groups.length - 1 ? 1 : before + (group.value ?? 0) / sum;
    return { group, index, start: before * Math.PI * 2, end: after * Math.PI * 2 };
  });

  const activeGroup = active === null ? undefined : groups[active];
  const middle = activeGroup === undefined ? total : activeGroup.value;
  const middleText = middle === null ? "—" : formatValue(middle);
  const fontSize = clampFont(inner, middleText);
  const caption = activeGroup === undefined ? totalLabel : truncateLabel(activeGroup.label, Math.max(4, Math.floor((inner * 1.6) / 6.2)));
  const slice = active === null ? undefined : slices[active];
  const tooltipAt = slice === undefined ? null : polar(centre, centre, outer, (slice.start + slice.end) / 2);

  return (
    <div className="chart-donut" data-legend={side ? "side" : "below"} style={{ width, height }}>
      <div
        {...plotProps}
        className="chart-plot"
        role="img"
        aria-label={label}
        aria-describedby={active === null ? undefined : tooltipId}
        data-chart="donut"
        data-has-active={active === null ? undefined : "true"}
        style={{ width: size, height: size }}
      >
        <svg className="chart-svg" width={size} height={size} viewBox={`0 0 ${String(size)} ${String(size)}`} aria-hidden="true" focusable="false">
          {slices.map(({ group, index, start, end }) => (
            <g key={group.key} className="chart-slice" {...markProps(index)} data-key={group.key} data-value={group.value ?? ""}>
              <path
                className="chart-mark"
                d={slicePath(centre, centre, inner, outer, start, end)}
                fillRule="evenodd"
                {...colorAttributes(group, false)}
              />
            </g>
          ))}
          <text className="chart-donut-value" x={centre} y={centre} dy="0.34em" textAnchor="middle" style={{ fontSize }}>
            {middleText}
          </text>
          {inner >= 34 && (
            <text className="chart-donut-caption" x={centre} y={centre + fontSize * 0.9} textAnchor="middle">
              {caption}
            </text>
          )}
        </svg>
        {slice !== undefined && tooltipAt !== null && (
          <ChartTooltip id={tooltipId} x={tooltipAt.x} y={tooltipAt.y} width={size} description={describe(slice.group)} />
        )}
      </div>
      <ul className="chart-legend">
        {groups.map((group, index) => (
          <li key={group.key} className="chart-legend-item" data-active={active === index ? "true" : undefined} {...pointerProps(index)}>
            <span className="chart-swatch" {...colorAttributes(group, false)} />
            <span className="chart-legend-label" title={group.fullLabel}>
              {group.label}
            </span>
            <span className="chart-legend-value">
              {group.value === null ? "—" : formatValue(group.value)}
              {shares && group.value !== null && <span className="chart-legend-share">{percent(group.value / sum, locale)}</span>}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** The font size of the centre's number: as large as the hole holds, up to 28 px. */
function clampFont(inner: number, text: string): number {
  const fits = (inner * 1.7) / Math.max(1, labelWidth(text, 0.6));
  return Math.round(Math.max(10, Math.min(28, inner * 0.55, fits)));
}
