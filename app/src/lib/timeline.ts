// The overview's timeline (全局 A): one lane per idea, runs placed by when they were
// submitted, a vertical line where an idea forks from a run of its parent.

import type { Board, IdeaView, RunView } from "../types";
import { day, parseUtc } from "./format";

export interface Lane {
  idea: IdeaView;
  /** The run count and stage line under the name. */
  sub: string;
  muted: boolean;
  /** Where its bar starts and ends (0–1). */
  x0: number;
  x1: number;
}

export interface Mark {
  run: RunView;
  lane: number;
  x: number;
}

export interface Fork {
  x: number;
  from: number;
  to: number;
  muted: boolean;
}

export interface Layout {
  lanes: Lane[];
  marks: Mark[];
  forks: Fork[];
  ticks: { x: number; label: string }[];
  /** e.g. "10-20 到 11-04". */
  span: string;
}

const STAGE: Record<string, string> = { open: "进行中", merged: "已收", dropped: "已放弃" };
const ORDER: Record<string, number> = { open: 0, merged: 1, dropped: 2 };

export function layout(b: Board, nTicks = 6): Layout {
  const now = parseUtc(b.project.loaded_at);
  const ideas = b.ideas
    .filter((i) => i.stage !== "seed")
    .sort(
      (a, c) =>
        (ORDER[a.stage] ?? 3) - (ORDER[c.stage] ?? 3) ||
        (a.last_active < c.last_active ? 1 : a.last_active > c.last_active ? -1 : 0),
    );
  const stamps = [
    ...ideas.map((i) => parseUtc(i.created_at)),
    ...b.runs.map((r) => parseUtc(r.created_at)),
  ].filter((t) => !Number.isNaN(t));
  const t0 = stamps.length ? Math.min(...stamps) : now - 86_400_000;
  const t1 = Math.max(now, ...stamps, t0 + 3_600_000);
  const x = (ts: string | null) => {
    const t = parseUtc(ts);
    return Number.isNaN(t) ? 1 : Math.min(1, Math.max(0, (t - t0) / (t1 - t0)));
  };
  const laneOf = new Map(ideas.map((i, k) => [i.name, k]));
  const runX = new Map(b.runs.map((r) => [r.id, x(r.created_at)]));
  const lanes: Lane[] = ideas.map((i) => {
    const n = b.runs.filter((r) => r.idea === i.name).length;
    const start = i.from_run && runX.has(i.from_run) ? runX.get(i.from_run)! : x(i.created_at);
    return {
      idea: i,
      sub: `${STAGE[i.stage] ?? i.stage} · ${n} 个 run`,
      muted: i.stage !== "open",
      x0: start,
      x1: i.closed_at ? x(i.closed_at) : 1,
    };
  });
  const marks: Mark[] = b.runs
    .filter((r) => laneOf.has(r.idea))
    .map((r) => ({ run: r, lane: laneOf.get(r.idea)!, x: x(r.created_at) }));
  const forks: Fork[] = [];
  for (const [k, l] of lanes.entries()) {
    const parent = l.idea.parent ? laneOf.get(l.idea.parent) : undefined;
    if (parent === undefined) continue;
    forks.push({ x: l.x0, from: parent, to: k, muted: l.muted });
  }
  const ticks = Array.from({ length: nTicks }, (_, k) => {
    const frac = nTicks === 1 ? 0 : k / (nTicks - 1);
    return { x: frac, label: day(new Date(t0 + frac * (t1 - t0)).toISOString(), now) };
  });
  return {
    lanes,
    marks,
    forks,
    ticks,
    span: `${day(new Date(t0).toISOString(), now)} 到 ${day(new Date(t1).toISOString(), now)}`,
  };
}
