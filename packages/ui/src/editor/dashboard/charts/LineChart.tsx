// The line chart of a chart widget (ADR-0055, `docs/architecture/dashboards.md` section 4.4): the points joined in order by a 2 px
// line with a light area under it, the value axis from 0 (or the minimum below it), the labels thinned to what fits. A group
// without a value is a gap (the line breaks); a point alone between gaps is drawn as a dot. The dot of the point under the
// mouse or the keys has a ring in the surface colour, and a hairline marks its place.

import { activeLocale } from "../../../i18n";
import { formatTick, labelWidth, niceTicks, truncateLabel } from "../chart-scale";
import { ChartTooltip, useChartInteraction } from "./chart-parts";
import type { ChartViewProps } from "./chart-parts";

const TOP = 10;
const BOTTOM = 26;
const RIGHT = 12;
/** Room before the first point and after the last, so that their dots are not cut by the edge. */
const PAD = 8;

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}

interface Point {
  readonly index: number;
  readonly x: number;
  readonly y: number;
}

/** The points of each run of groups that have a value, in order. */
function runsOf(points: readonly (Point | null)[]): Point[][] {
  const runs: Point[][] = [];
  let current: Point[] = [];
  for (const point of points) {
    if (point === null) {
      if (current.length > 0) {
        runs.push(current);
      }
      current = [];
    } else {
      current.push(point);
    }
  }
  if (current.length > 0) {
    runs.push(current);
  }
  return runs;
}

export function LineChart({ groups, width, height, label, describe, onSelect }: ChartViewProps) {
  const { active, tooltipId, plotProps, markProps } = useChartInteraction(groups.length, onSelect);
  const locale = activeLocale();
  const count = groups.length;
  const present = groups.flatMap((group) => (group.value === null ? [] : [group.value]));
  const plotHeight = Math.max(10, height - TOP - BOTTOM);
  const ticks = niceTicks(
    Math.min(0, ...present),
    Math.max(0, ...present),
    clamp(Math.floor(plotHeight / 40), 2, 6),
    present.every((value) => Number.isInteger(value)),
  );
  const tickLabels = ticks.ticks.map((tick) => formatTick(tick, locale));
  const left = Math.ceil(Math.max(0, ...tickLabels.map((text) => labelWidth(text)))) + 12;
  const plotWidth = Math.max(10, width - left - RIGHT);
  const span = ticks.max - ticks.min || 1;
  const y = (value: number): number => TOP + plotHeight * (1 - (value - ticks.min) / span);
  const baseline = y(0);
  const step = count > 1 ? (plotWidth - 2 * PAD) / (count - 1) : plotWidth;
  const x = (index: number): number => (count > 1 ? left + PAD + step * index : left + plotWidth / 2);

  const points = groups.map((group, index): Point | null => (group.value === null ? null : { index, x: x(index), y: y(group.value) }));
  const runs = runsOf(points);
  const needed = Math.max(0, ...groups.map((group) => Math.min(group.label.length, 12))) * 6.2 + 12;
  const stride = step >= needed ? 1 : Math.ceil(needed / Math.max(step, 1));
  const hitWidth = count > 1 ? step : plotWidth;
  const here = active === null ? null : (points[active] ?? null);
  const activeGroup = active === null ? undefined : groups[active];

  return (
    <div
      {...plotProps}
      className="chart-plot"
      role="img"
      aria-label={label}
      aria-describedby={active === null ? undefined : tooltipId}
      data-chart="line"
      data-has-active={active === null ? undefined : "true"}
      style={{ width, height }}
    >
      <svg className="chart-svg" width={width} height={height} viewBox={`0 0 ${String(width)} ${String(height)}`} aria-hidden="true" focusable="false">
        {ticks.ticks.map((tick) => {
          const at = y(tick);
          return (
            <g key={tick} className="chart-axis">
              <line className={tick === 0 ? "chart-baseline" : "chart-grid"} x1={left} x2={left + plotWidth} y1={at} y2={at} />
              <text className="chart-tick" x={left - 6} y={at} dy="0.32em" textAnchor="end">
                {formatTick(tick, locale)}
              </text>
            </g>
          );
        })}
        {groups.map((group, index) =>
          index % stride === 0 ? (
            <text key={group.key} className="chart-category" x={x(index)} y={TOP + plotHeight + 17} textAnchor="middle">
              {truncateLabel(group.label, 12)}
              {truncateLabel(group.label, 12) !== group.label && <title>{group.fullLabel}</title>}
            </text>
          ) : null,
        )}
        {here !== null && <line className="chart-guide" x1={here.x} x2={here.x} y1={TOP} y2={TOP + plotHeight} />}
        {runs.map((run) => {
          const first = run[0];
          const last = run[run.length - 1];
          if (first === undefined || last === undefined) {
            return null;
          }
          const line = run.map((point, at) => `${at === 0 ? "M" : "L"}${String(Math.round(point.x * 100) / 100)} ${String(Math.round(point.y * 100) / 100)}`).join("");
          const area = `${line}L${String(last.x)} ${String(baseline)}L${String(first.x)} ${String(baseline)}Z`;
          return (
            <g key={first.index} data-series="1">
              {run.length > 1 && <path className="chart-area" d={area} />}
              {run.length > 1 && <path className="chart-line" d={line} />}
              {run.length === 1 && <circle className="chart-dot" cx={first.x} cy={first.y} r={4} />}
            </g>
          );
        })}
        {here !== null && (
          <g data-series="1">
            <circle className="chart-dot" cx={here.x} cy={here.y} r={4} />
          </g>
        )}
        {groups.map((group, index) => (
          <g key={group.key} className="chart-point" {...markProps(index)} data-key={group.key} data-value={group.value ?? ""}>
            <rect className="chart-hit" x={x(index) - hitWidth / 2} y={TOP} width={hitWidth} height={plotHeight} />
          </g>
        ))}
      </svg>
      {activeGroup !== undefined && active !== null && (
        <ChartTooltip
          id={tooltipId}
          x={x(active)}
          y={here === null ? baseline : here.y}
          width={width}
          description={describe(activeGroup)}
        />
      )}
    </div>
  );
}
