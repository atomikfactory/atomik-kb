// The arithmetic of the chart drawings (ADR-0055, `docs/architecture/dashboards.md` section 4.4): nice axis ticks, the
// text a label is cut to, the number written on an axis, and the paths of a bar's rounded end and of a donut's slice.
// Pure: no React, no DOM; `charts/` draw what these answer.

/** The ticks of an axis: the values, and the domain they cover (the first and the last tick). */
export interface Ticks {
  readonly ticks: readonly number[];
  readonly min: number;
  readonly max: number;
}

/** The step of 1, 2, 5 or 10 times a power of ten that is at least `raw` (or the nearest above it). */
function niceStep(raw: number): number {
  const exponent = Math.floor(Math.log10(raw));
  const base = 10 ** exponent;
  const fraction = raw / base;
  const nice = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  return nice * base;
}

/** A number without the noise of floating point (`0.30000000000000004` is `0.3`). */
function clean(value: number): number {
  return Number(value.toPrecision(12));
}

/**
 * Ticks that cover `[min, max]` with about `target` intervals, on round numbers (1, 2 or 5 times a power of ten). The
 * domain is widened to the first and the last tick; a domain of one value (or none) gets a unit of room. `integers` keeps
 * the step at 1 or more, for an axis of counts (no tick at 0.5 pages).
 */
export function niceTicks(min: number, max: number, target = 5, integers = false): Ticks {
  if (!Number.isFinite(min) || !Number.isFinite(max)) {
    return { ticks: [0, 1], min: 0, max: 1 };
  }
  let low = Math.min(min, max);
  let high = Math.max(min, max);
  if (low === high) {
    if (low === 0) {
      high = 1;
    } else if (low > 0) {
      low = 0;
    } else {
      high = 0;
    }
  }
  const step = Math.max(niceStep((high - low) / Math.max(1, target)), integers ? 1 : 0);
  const first = clean(Math.floor(low / step) * step);
  const last = clean(Math.ceil(high / step) * step);
  const ticks: number[] = [];
  // The loop is bounded: `target` intervals of a nice step cover the domain with at most two more.
  for (let at = 0; at <= Math.max(1, target) + 2; at += 1) {
    const tick = clean(first + at * step);
    if (tick > last + step / 1e6) {
      break;
    }
    ticks.push(tick);
  }
  const top = ticks.at(-1) ?? last;
  return { ticks, min: first, max: top };
}

/** `text` cut to `maxChars` characters with an ellipsis (as it is when it fits). */
export function truncateLabel(text: string, maxChars: number): string {
  const chars = Array.from(text);
  if (chars.length <= maxChars) {
    return text;
  }
  if (maxChars <= 1) {
    return "…";
  }
  return `${chars.slice(0, maxChars - 1).join("").trimEnd()}…`;
}

/** The width in pixels the text takes at the chart's small font: a good estimate, which is all a layout needs. */
export function labelWidth(text: string, charWidth = 6.2): number {
  return Array.from(text).length * charWidth;
}

/** A number on an axis: the locale's, compact from a thousand up (`0`, `500`, `1K`, `2.5M`). */
export function formatTick(value: number, locale: string): string {
  if (Math.abs(value) >= 1000) {
    return new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(value);
  }
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(value);
}

/** The side of a bar that is its end (the value's side of the baseline). */
export type BarTip = "up" | "down" | "left" | "right";

/**
 * The outline of a bar whose end (`tip`) has rounded corners and whose other end, on the baseline, is square. The
 * rectangle is `x, y, w, h`; the radius is held to what the bar can bear.
 */
export function roundedTipPath(x: number, y: number, w: number, h: number, tip: BarTip, radius: number): string {
  const r = Math.max(0, Math.min(radius, w / 2, h));
  const x1 = x + w;
  const y1 = y + h;
  switch (tip) {
    case "up":
      return `M${f(x)} ${f(y1)}V${f(y + r)}Q${f(x)} ${f(y)} ${f(x + r)} ${f(y)}H${f(x1 - r)}Q${f(x1)} ${f(y)} ${f(x1)} ${f(y + r)}V${f(y1)}Z`;
    case "down":
      return `M${f(x)} ${f(y)}V${f(y1 - r)}Q${f(x)} ${f(y1)} ${f(x + r)} ${f(y1)}H${f(x1 - r)}Q${f(x1)} ${f(y1)} ${f(x1)} ${f(y1 - r)}V${f(y)}Z`;
    case "right":
      return roundedTipPathSideways(x, y, w, h, "right", Math.max(0, Math.min(radius, h / 2, w)));
    case "left":
      return roundedTipPathSideways(x, y, w, h, "left", Math.max(0, Math.min(radius, h / 2, w)));
  }
}

function roundedTipPathSideways(x: number, y: number, w: number, h: number, tip: "left" | "right", r: number): string {
  const x1 = x + w;
  const y1 = y + h;
  if (tip === "right") {
    return `M${f(x)} ${f(y)}H${f(x1 - r)}Q${f(x1)} ${f(y)} ${f(x1)} ${f(y + r)}V${f(y1 - r)}Q${f(x1)} ${f(y1)} ${f(x1 - r)} ${f(y1)}H${f(x)}Z`;
  }
  return `M${f(x1)} ${f(y)}H${f(x + r)}Q${f(x)} ${f(y)} ${f(x)} ${f(y + r)}V${f(y1 - r)}Q${f(x)} ${f(y1)} ${f(x + r)} ${f(y1)}H${f(x1)}Z`;
}

/** A coordinate with at most two decimals, without a trailing zero. */
function f(value: number): string {
  return String(Math.round(value * 100) / 100);
}

/** The point at `angle` (radians, 0 at the top, clockwise) on the circle of centre `cx, cy` and radius `radius`. */
export function polar(cx: number, cy: number, radius: number, angle: number): { x: number; y: number } {
  return { x: cx + radius * Math.sin(angle), y: cy - radius * Math.cos(angle) };
}

/**
 * The outline of a donut slice between the radii `inner` and `outer` from `start` to `end` (radians, 0 at the top,
 * clockwise). A slice that is the whole ring is drawn as two half rings (an arc cannot start and end on one point).
 */
export function slicePath(cx: number, cy: number, inner: number, outer: number, start: number, end: number): string {
  const sweep = end - start;
  if (sweep >= Math.PI * 2 - 1e-6) {
    const top = polar(cx, cy, outer, 0);
    const bottom = polar(cx, cy, outer, Math.PI);
    const innerTop = polar(cx, cy, inner, 0);
    const innerBottom = polar(cx, cy, inner, Math.PI);
    return (
      `M${f(top.x)} ${f(top.y)}A${f(outer)} ${f(outer)} 0 1 1 ${f(bottom.x)} ${f(bottom.y)}A${f(outer)} ${f(outer)} 0 1 1 ${f(top.x)} ${f(top.y)}` +
      `M${f(innerTop.x)} ${f(innerTop.y)}A${f(inner)} ${f(inner)} 0 1 0 ${f(innerBottom.x)} ${f(innerBottom.y)}A${f(inner)} ${f(inner)} 0 1 0 ${f(innerTop.x)} ${f(innerTop.y)}Z`
    );
  }
  const large = sweep > Math.PI ? 1 : 0;
  const a = polar(cx, cy, outer, start);
  const b = polar(cx, cy, outer, end);
  const c = polar(cx, cy, inner, end);
  const d = polar(cx, cy, inner, start);
  return (
    `M${f(a.x)} ${f(a.y)}A${f(outer)} ${f(outer)} 0 ${String(large)} 1 ${f(b.x)} ${f(b.y)}` +
    `L${f(c.x)} ${f(c.y)}A${f(inner)} ${f(inner)} 0 ${String(large)} 0 ${f(d.x)} ${f(d.y)}Z`
  );
}
