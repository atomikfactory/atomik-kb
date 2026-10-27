// The arithmetic of the chart drawings (`chart-scale.ts`): nice ticks, cut labels, the number on an axis, the outline of a bar's
// rounded end and of a donut's slice. Pure.

import { describe, expect, it } from "vitest";

import { formatTick, labelWidth, niceTicks, polar, roundedTipPath, slicePath, truncateLabel } from "./chart-scale";

describe("niceTicks", () => {
  it("covers the domain on round numbers", () => {
    expect(niceTicks(0, 10, 5)).toEqual({ ticks: [0, 2, 4, 6, 8, 10], min: 0, max: 10 });
    expect(niceTicks(0, 7, 5)).toEqual({ ticks: [0, 2, 4, 6, 8], min: 0, max: 8 });
    expect(niceTicks(0, 1, 4)).toEqual({ ticks: [0, 0.5, 1], min: 0, max: 1 });
  });

  it("takes steps of 1, 2 or 5 times a power of ten", () => {
    for (const [max, step] of [
      [3, 1],
      [9, 2],
      [23, 5],
      [95, 20],
      [480, 100],
      [1200, 500],
      [0.9, 0.2],
    ] as const) {
      const { ticks } = niceTicks(0, max, 5);
      expect(ticks[1]).toBe(step);
      expect(ticks.at(-1)).toBeGreaterThanOrEqual(max);
    }
  });

  it("keeps the step at a whole number for an axis of counts", () => {
    expect(niceTicks(0, 1, 4).ticks).toEqual([0, 0.5, 1]);
    expect(niceTicks(0, 1, 4, true).ticks).toEqual([0, 1]);
    expect(niceTicks(0, 3, 5, true).ticks).toEqual([0, 1, 2, 3]);
    // A step above 1 is as it was.
    expect(niceTicks(0, 95, 5, true).ticks[1]).toBe(20);
  });

  it("includes the minimum below 0", () => {
    const { ticks, min, max } = niceTicks(-4, 6, 5);
    expect(min).toBe(-4);
    expect(max).toBe(6);
    expect(ticks).toEqual([-4, -2, 0, 2, 4, 6]);
  });

  it("gives a domain of one value room", () => {
    expect(niceTicks(0, 0)).toMatchObject({ min: 0, max: 1 });
    expect(niceTicks(5, 5)).toMatchObject({ min: 0 });
    expect(niceTicks(5, 5).max).toBeGreaterThanOrEqual(5);
    expect(niceTicks(-5, -5).max).toBe(0);
    expect(niceTicks(-5, -5).min).toBeLessThanOrEqual(-5);
  });

  it("does not leave floating point noise in a tick", () => {
    expect(niceTicks(0, 0.3, 3).ticks).toEqual([0, 0.1, 0.2, 0.3]);
  });

  it("survives what is not a number", () => {
    expect(niceTicks(Number.NaN, 4)).toEqual({ ticks: [0, 1], min: 0, max: 1 });
    expect(niceTicks(0, Number.POSITIVE_INFINITY)).toEqual({ ticks: [0, 1], min: 0, max: 1 });
  });

  it("has a bounded number of ticks", () => {
    expect(niceTicks(0, 1e9, 5).ticks.length).toBeLessThanOrEqual(8);
    expect(niceTicks(0, 3, 0).ticks.length).toBeLessThanOrEqual(4);
  });
});

describe("labels and numbers", () => {
  it("cuts a label with an ellipsis", () => {
    expect(truncateLabel("Short", 10)).toBe("Short");
    expect(truncateLabel("A long label", 6)).toBe("A lon…");
    expect(truncateLabel("A long label", 1)).toBe("…");
    expect(truncateLabel("A long label", 0)).toBe("…");
    expect(truncateLabel("ab cd", 3)).toBe("ab…");
  });

  it("counts characters, not code units", () => {
    expect(truncateLabel("😀😀😀😀", 3)).toBe("😀😀…");
    expect(labelWidth("😀😀", 10)).toBe(20);
  });

  it("writes a number on an axis compact from a thousand", () => {
    expect(formatTick(0, "en-US")).toBe("0");
    expect(formatTick(2.5, "en-US")).toBe("2.5");
    expect(formatTick(999, "en-US")).toBe("999");
    expect(formatTick(1000, "en-US")).toBe("1K");
    expect(formatTick(2500, "en-US")).toBe("2.5K");
    expect(formatTick(-3_000_000, "en-US")).toBe("-3M");
  });
});

describe("paths", () => {
  it("rounds the end of a bar and leaves the baseline square", () => {
    const up = roundedTipPath(10, 20, 8, 30, "up", 4);
    expect(up.startsWith("M10 50V24Q10 20 14 20")).toBe(true);
    expect(up.endsWith("V50Z")).toBe(true);
    const down = roundedTipPath(10, 20, 8, 30, "down", 4);
    expect(down.startsWith("M10 20V46Q10 50 14 50")).toBe(true);
    const right = roundedTipPath(10, 20, 30, 8, "right", 4);
    expect(right.startsWith("M10 20H36Q40 20 40 24")).toBe(true);
    const left = roundedTipPath(10, 20, 30, 8, "left", 4);
    expect(left.startsWith("M40 20H14Q10 20 10 24")).toBe(true);
  });

  it("holds the radius to what the bar can bear", () => {
    // A bar one pixel long cannot have a corner of four.
    expect(roundedTipPath(0, 0, 8, 1, "up", 4)).toBe("M0 1V1Q0 0 1 0H7Q8 0 8 1V1Z");
    expect(roundedTipPath(0, 0, 2, 20, "up", 4)).toContain("Q0 0 1 0");
  });

  it("finds a point on a circle from the top, clockwise", () => {
    const top = polar(10, 10, 5, 0);
    expect(top.x).toBeCloseTo(10);
    expect(top.y).toBeCloseTo(5);
    const right = polar(10, 10, 5, Math.PI / 2);
    expect(right.x).toBeCloseTo(15);
    expect(right.y).toBeCloseTo(10);
  });

  it("draws a slice between two radii, with the large arc flag for more than half a turn", () => {
    const small = slicePath(50, 50, 20, 40, 0, Math.PI / 2);
    expect(small.startsWith("M50 10A40 40 0 0 1 90 50L70 50A20 20 0 0 0 50 30Z")).toBe(true);
    const large = slicePath(50, 50, 20, 40, 0, Math.PI * 1.5);
    expect(large).toContain("A40 40 0 1 1");
    expect(large).toContain("A20 20 0 1 0");
  });

  it("draws a whole ring as two half rings", () => {
    const ring = slicePath(50, 50, 20, 40, 0, Math.PI * 2);
    expect(ring.match(/A40 40/gu)).toHaveLength(2);
    expect(ring.match(/A20 20/gu)).toHaveLength(2);
    expect(ring.endsWith("Z")).toBe(true);
  });
});
