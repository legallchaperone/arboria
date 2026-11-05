// 专注（第 2 期）: an idea's header, its runs, notes and todos; the notebook itself is
// still marimo, opened in its own window.

import type { Board, IdeaView } from "../types";
import { ago, clip, su, STATUS_TEXT, statusTone } from "../lib/format";
import { Glyph } from "../components/Glyph";

export function Idea(props: {
  board: Board;
  idea: IdeaView;
  now: number;
  busy: string | null;
  onNotebook: () => void;
  onCard: (run: string) => void;
}) {
  const { board: b, idea: i } = props;
  const runs = b.runs.filter((r) => r.idea === i.name).reverse();
  const notes = b.notes.filter((n) => n.idea === i.name);
  const todos = b.todos.filter((t) => t.idea === i.name && t.status !== "done");
  const seed = i.stage === "seed";
  return (
    <main className="app-main">
      <div className="app-column">
        <div className="app-head-block">
          <h1 className="title">{i.name}</h1>
          <div className="small ab-muted">
            {seed ? `种子 · ${i.gist ?? ""}` : `假设：${i.hypothesis ?? ""}`}
            {i.metric && ` · 指标 ${i.metric}`} · {i.runs} 个 run · 已用 {su(i.su_used)} SU
            {i.parent && ` · 从 ${i.parent}${i.from_run ? ` 的 ${i.from_run}` : ""} 分出`}
            {i.stage === "merged" && " · 已收"}
            {i.stage === "dropped" && " · 已放弃"}
          </div>
        </div>
        <div className="app-row">
          <button type="button" className="ab-btn body" onClick={props.onNotebook} disabled={props.busy !== null || seed}>
            {props.busy ?? "打开 notebook"}
          </button>
          <span className="small ab-muted">{seed ? "种子的 notebook 在种子园里：marimo edit seeds/" + i.name + ".py" : "notebook 这一期还是 marimo，在单独的窗口里打开。"}</span>
        </div>

        <section className="app-section" aria-labelledby="runs-h">
          <h2 className="heading" id="runs-h">run</h2>
          {runs.length === 0 && <p className="small ab-muted">还没有 run：在 notebook 里 lab.run(...)，或 arb submit。</p>}
          <div className="ab-list">
            {runs.map((r) => {
              const s = statusTone(r.status);
              const waiting = r.status === "awaiting_approval";
              return (
                <div key={r.id} className="ab-item app-run" onClick={waiting ? () => props.onCard(r.id) : undefined} role={waiting ? "button" : undefined}>
                  <span className="mark">
                    <Glyph name={s.glyph} tone={s.tone} />
                  </span>
                  <span className="strong ab-clip">
                    {r.id} <span className="ab-muted body">{STATUS_TEXT[r.status] ?? r.status}</span>
                  </span>
                  <span className="when small">{ago(r.finished_at ?? r.updated_at, props.now)}</span>
                  <span className="meta small">
                    {clip(r.change_reason, 60)}
                    {r.gpus != null && ` · ${r.gpus} GPU × ${r.walltime ?? "?"}`}
                    {r.machine && ` · ${r.machine}`}
                    {r.billed_su > 0 ? ` · ${su(r.billed_su)} SU` : r.est_su != null ? ` · ≈ ${su(r.est_su)} SU` : ""}
                    {r.summary && ` · ${clip(r.summary, 80)}`}
                    {waiting && " · 点开审核单签字"}
                  </span>
                </div>
              );
            })}
          </div>
        </section>

        {todos.length > 0 && (
          <section className="app-section" aria-labelledby="todo-h">
            <h2 className="heading" id="todo-h">待办（agent 在做）</h2>
            <div className="ab-list">
              {todos.map((t) => (
                <div key={t.id} className="ab-line body">
                  <span className="name code">{t.id}</span>
                  <span className="meta">{clip(t.text, 80)}</span>
                  <span className="small ab-muted">{t.status}</span>
                </div>
              ))}
            </div>
          </section>
        )}

        <section className="app-section" aria-labelledby="notes-h">
          <h2 className="heading" id="notes-h">笔记</h2>
          {notes.length === 0 && <p className="small ab-muted">还没有笔记（arb note 在你自己的终端里写）。</p>}
          {notes.map((n) => (
            <div key={n.id} className="app-note">
              <div className="small ab-muted">
                {ago(n.created_at, props.now)}
                {n.run && ` · ${n.run}`}
                {n.author === "agent" && " · agent"}
              </div>
              <p className="app-pre">{n.text}</p>
            </div>
          ))}
        </section>
      </div>
    </main>
  );
}
