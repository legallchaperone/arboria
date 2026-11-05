// The inbox across projects (收件箱 A: one column, grouped by kind, newest first).

import type { InboxItem, InboxKind, ProjectSummary } from "../types";
import type { GlyphName, Tone } from "./format";

export const GROUPS: { kind: InboxKind; label: string; glyph: GlyphName; tone: Tone }[] = [
  { kind: "approve", label: "要签字", glyph: "touch", tone: "warn" },
  { kind: "failed", label: "失败", glyph: "cross", tone: "bad" },
  { kind: "finished", label: "跑完了", glyph: "check", tone: "ok" },
  { kind: "agent_done", label: "agent 写好了", glyph: "check", tone: "ok" },
  { kind: "lesson", label: "待确认经验", glyph: "circle", tone: "muted" },
  { kind: "probe_reenable", label: "probe 待重新启用", glyph: "circle", tone: "warn" },
];

export interface Entry extends InboxItem {
  root: string;
  project: string;
  /** A stable id for selection. */
  key: string;
}

export interface Group {
  kind: InboxKind;
  label: string;
  glyph: GlyphName;
  tone: Tone;
  items: Entry[];
}

export function entries(projects: ProjectSummary[]): Entry[] {
  const many = projects.filter((p) => !p.error && p.inbox.length > 0).length > 1;
  return projects.flatMap((p) =>
    p.inbox.map((i) => ({
      ...i,
      root: p.root,
      project: p.name,
      meta: many ? `${p.name} · ${i.meta}` : i.meta,
      key: `${p.root}|${i.kind}|${i.run ?? i.task ?? i.lesson ?? i.probe ?? i.title}`,
    })),
  );
}

/** Groups in [`GROUPS`] order, each newest first; empty groups left out. */
export function group(items: Entry[]): Group[] {
  return GROUPS.map((g) => ({
    ...g,
    items: items.filter((i) => i.kind === g.kind).sort((a, b) => (a.when < b.when ? 1 : a.when > b.when ? -1 : 0)),
  })).filter((g) => g.items.length > 0);
}

/** Things only the human can act on (todos wait for the agent, not for you). */
export function needsYou(items: InboxItem[]): number {
  return items.length;
}
