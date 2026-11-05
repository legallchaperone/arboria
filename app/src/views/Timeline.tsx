// The timeline of a project (全局 A): one lane per idea, runs by when they were
// submitted, a vertical line where an idea forks from a run of its parent.

import type { Board } from "../types";
import { statusTone } from "../lib/format";
import { layout } from "../lib/timeline";
import { Glyph } from "../components/Glyph";

const LANE = 48;

export function Timeline({ board, onIdea }: { board: Board; onIdea: (idea: string) => void }) {
  const l = layout(board);
  if (l.lanes.length === 0) return <p className="small ab-muted">还没有 idea：arb idea new NAME。</p>;
  const h = l.lanes.length * LANE;
  const pct = (x: number) => `${(x * 100).toFixed(3)}%`;
  return (
    <div className="app-scroll-x">
      <div className="app-tl" role="img" aria-label={`idea 时间线，${l.span}`}>
        <div className="app-tl-names">
          {l.lanes.map((ln) => (
            <button key={ln.idea.name} type="button" className="app-tl-name app-link" style={{ height: LANE }} onClick={() => onIdea(ln.idea.name)}>
              <span className={ln.muted ? "ab-muted" : undefined}>{ln.idea.name}</span>
              <span className="small ab-muted">{ln.sub}</span>
            </button>
          ))}
        </div>
        <div className="app-tl-plot" style={{ height: h + 24 }}>
          {l.ticks.map((t) => (
            <div key={t.x} className="app-tl-grid" style={{ left: pct(t.x), height: h }} />
          ))}
          <div className="app-tl-axis" style={{ top: h }} />
          {l.ticks.map((t) => (
            <span key={"l" + t.x} className="caption ab-muted app-tl-tick" style={{ left: pct(t.x), top: h + 6 }}>
              {t.label}
            </span>
          ))}
          {l.lanes.map((ln, k) => (
            <div
              key={"b" + ln.idea.name}
              className={"app-tl-bar" + (ln.muted ? " is-muted" : "")}
              style={{ top: k * LANE + 11, left: pct(ln.x0), width: pct(Math.max(0.002, ln.x1 - ln.x0)) }}
            />
          ))}
          {l.forks.map((f, i) => (
            <div
              key={"f" + i}
              className={"app-tl-fork" + (f.muted ? " is-muted" : "")}
              style={{
                left: `calc(${pct(f.x)} - 1px)`,
                top: Math.min(f.from, f.to) * LANE + 12,
                height: Math.abs(f.to - f.from) * LANE,
              }}
            />
          ))}
          {l.marks.map((m) => {
            const s = statusTone(m.run.status);
            return (
              <span key={m.run.id} className="app-tl-mark" style={{ top: m.lane * LANE + 4, left: `calc(${pct(m.x)} - 8px)` }} title={`${m.run.id} · ${m.run.change_reason}`}>
                <Glyph name={s.glyph} tone={l.lanes[m.lane].muted && s.tone === "ink" ? "muted" : s.tone} />
                <span className="small ab-muted app-tl-id">{m.run.id}</span>
              </span>
            );
          })}
        </div>
      </div>
    </div>
  );
}

