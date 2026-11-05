// An idea: 概览 (what it tests, its runs — select one: the inspector —, what the agent is
// doing, your notes) and Notebook (its marimo notebook embedded, until phase 3 brings the
// app's own: a sandboxed 127.0.0.1 iframe).

import { useEffect, useState } from "react";
import { api, message } from "../api";
import type { Board, IdeaView as Idea } from "../types";
import { ago, clip, su, STATUS_TEXT, statusTone } from "../lib/format";
import { Glyph } from "../components/Glyph";

/** The notebook tab: start (or reuse) the idea's marimo server and embed it. */
function Notebook(props: { root: string; idea: string }) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    setUrl(null);
    setError(null);
    api.openNotebook(props.root, props.idea).then(setUrl, (e) => setError(message(e)));
  }, [props.root, props.idea, n]);
  if (error)
    return (
      <div className="empty">
        <span className="big">notebook 打不开</span>
        <span className="err-box">{error}</span>
        <button type="button" className="ab-btn" onClick={() => setN((x) => x + 1)}>
          再试一次
        </button>
      </div>
    );
  if (!url) return <div className="empty">正在启动 marimo…</div>;
  return (
    <iframe
      className="nb-frame"
      title={`${props.idea} notebook`}
      src={url}
      sandbox="allow-scripts allow-same-origin allow-forms allow-downloads allow-modals allow-popups"
    />
  );
}

export function IdeaPage(props: {
  board: Board;
  idea: Idea;
  now: number;
  selectedRun: string | null;
  tab: "overview" | "notebook";
  onTab: (t: "overview" | "notebook") => void;
  onRun: (run: string) => void;
}) {
  const seed = props.idea.stage === "seed";
  return (
    <div className="idea-wrap">
      {!seed && (
        <div className="tabs" role="tablist">
          {(["overview", "notebook"] as const).map((t) => (
            <button key={t} type="button" role="tab" aria-selected={props.tab === t} className={"tab" + (props.tab === t ? " is-on" : "")} onClick={() => props.onTab(t)}>
              {t === "overview" ? "概览" : "Notebook"}
            </button>
          ))}
        </div>
      )}
      {props.tab === "notebook" && !seed ? (
        <Notebook root={props.board.project.root} idea={props.idea.name} />
      ) : (
        <div className="idea-scroll">
          <IdeaView {...props} />
        </div>
      )}
    </div>
  );
}

export function IdeaView(props: {
  board: Board;
  idea: Idea;
  now: number;
  selectedRun: string | null;
  onRun: (run: string) => void;
  onTab: (t: "overview" | "notebook") => void;
}) {
  const { board: b, idea: i } = props;
  const runs = b.runs.filter((r) => r.idea === i.name).reverse();
  const todos = b.todos.filter((t) => t.idea === i.name && t.status !== "done");
  const notes = b.notes.filter((n) => n.idea === i.name);
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="title">{i.name}</h1>
        <div className="sub">{i.stage === "seed" ? i.gist : i.hypothesis}</div>
        <div className="chips">
          {i.metric && (
            <span className="chip">
              指标 <b className="mono">{i.metric}</b>
            </span>
          )}
          <span className="chip">
            <b>{i.runs}</b> 个 run · <b>{su(i.su_used)}</b> SU
          </span>
          {i.parent && (
            <span className="chip">
              从 {i.parent}
              {i.from_run ? ` 的 ${i.from_run}` : ""} 分出
            </span>
          )}
          {i.stage !== "seed" && (
            <button type="button" className="link small" onClick={() => props.onTab("notebook")}>
              打开 notebook →
            </button>
          )}
        </div>
      </div>

      <section className="section">
        <div className="section-head">
          <h2 className="heading">run</h2>
        </div>
        {runs.length === 0 ? (
          <p className="small muted">还没有 run：在 notebook 里 lab.run(...)，或者让 Claude 用 arb submit。</p>
        ) : (
          <div className="rows" role="listbox" aria-label="run">
            {runs.map((r) => {
              const s = statusTone(r.status);
              return (
                <div
                  key={r.id}
                  role="option"
                  aria-selected={r.id === props.selectedRun}
                  className={"row" + (r.id === props.selectedRun ? " is-sel" : "")}
                  onClick={() => props.onRun(r.id)}
                >
                  <Glyph name={s.glyph} tone={s.tone} />
                  <span className="main">
                    <span className="t">
                      <span className="id">{r.id}</span>
                      {clip(r.change_reason, 60)}
                    </span>
                    {r.summary && <span className="m">{clip(r.summary, 90)}</span>}
                  </span>
                  <span className="r">
                    <span>{STATUS_TEXT[r.status] ?? r.status}</span>
                    <span className="mono">{r.billed_su > 0 ? `${su(r.billed_su)} SU` : r.est_su != null ? `≈${su(r.est_su)}` : ""}</span>
                    <span>{ago(r.finished_at ?? r.updated_at, props.now)}</span>
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </section>

      {todos.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h2 className="heading">agent 在做</h2>
          </div>
          <div className="rows">
            {todos.map((t) => (
              <div key={t.id} className="row">
                <Glyph name="circle" tone="muted" />
                <span className="main">
                  <span className="t">
                    <span className="id">{t.id}</span>
                    {clip(t.text, 80)}
                  </span>
                </span>
                <span className="r">{t.status}</span>
              </div>
            ))}
          </div>
        </section>
      )}

      <section className="section">
        <div className="section-head">
          <h2 className="heading">笔记</h2>
        </div>
        {notes.length === 0 && <p className="small muted">还没有笔记（arb note 在你自己的终端里写）。</p>}
        {notes.map((n) => (
          <div key={n.id} className="box">
            <div className="small muted">
              {ago(n.created_at, props.now)}
              {n.run && ` · ${n.run}`}
              {n.author === "agent" && " · agent"}
            </div>
            <p className="pre">{n.text}</p>
          </div>
        ))}
      </section>
    </div>
  );
}

export function SeedsView(props: { board: Board | null; now: number }) {
  const seeds = props.board?.ideas.filter((i) => i.stage === "seed") ?? [];
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="title">种子园</h1>
        <div className="sub">还没有仓库的想法。在仓库外 arb idea new 记一个，想清楚了在目标仓库里 arb idea plant。</div>
      </div>
      {seeds.length === 0 ? (
        <p className="small muted">没有种子。</p>
      ) : (
        <div className="rows">
          {seeds.map((s) => (
            <div key={s.name} className="row">
              <Glyph name="sparkle" tone="muted" />
              <span className="main">
                <span className="t">{s.name}</span>
                <span className="m">{s.gist ?? ""}</span>
              </span>
              <span className="r">{ago(s.last_active, props.now)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
