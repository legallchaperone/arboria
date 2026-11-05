import { describe, expect, it } from "vitest";
import fixture from "../fixtures/board.json";
import type { Board } from "../types";
import { layout } from "./timeline";

const board = fixture.boards[0] as unknown as Board;

describe("timeline", () => {
  const l = layout(board);
  it("puts open ideas first, most recent first, seeds left out", () => {
    expect(l.lanes.map((x) => x.idea.name)).toEqual(["warmup-long", "label-smooth", "data-dedup", "no-dropout"]);
    expect(l.lanes.map((x) => x.muted)).toEqual([false, false, true, true]);
    expect(l.lanes[0].sub).toBe("进行中 · 4 个 run");
    expect(layout(fixture.boards[1] as unknown as Board).lanes).toEqual([]);
  });
  it("places runs on their lane in time order and forks at the parent run", () => {
    const r014 = l.marks.find((m) => m.run.id === "r014")!;
    const r001 = l.marks.find((m) => m.run.id === "r001")!;
    expect(r001.x).toBeLessThan(0.01);
    expect(r014.lane).toBe(0);
    expect(r014.x).toBeGreaterThan(0.95);
    const r005 = l.marks.find((m) => m.run.id === "r005")!;
    const fork = l.forks.find((f) => f.to === 0)!;
    expect(fork.from).toBe(2);
    expect(fork.x).toBeCloseTo(r005.x);
    expect(l.lanes[2].x1).toBeLessThan(1); // merged: the bar ends when it closed
  });
  it("labels the axis with days", () => {
    expect(l.ticks[0].label).toBe("10-20");
    expect(l.ticks.at(-1)!.label).toBe("11-04");
    expect(l.span).toBe("10-20 到 11-04");
  });
});
