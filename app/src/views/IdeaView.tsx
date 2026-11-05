// An idea: 概览 (what it tests, its runs — select one: the inspector —, what the agent is
// doing, your notes) and Notebook (its marimo notebook embedded, until phase 3 brings the
// app's own: a sandboxed 127.0.0.1 iframe).

import { useEffect, useState } from "react";
import { api, message } from "../api";
import type { Board, IdeaView as Idea } from "../types";
import { ago, clip, su, STATUS_TEXT, statusTone } from "../lib/format";
import { Glyph } from "../components/Glyph";
import { MenuButton } from "../components/Menu";
import { ideaMenu, newSeed, seedMenu, useActions } from "./actions";

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
  const a = useActions();
  const root = props.board.project.root;
  const isSeed = props.board.project.kind === "home";
  const hasNotebook = isSeed ? props.idea.notebook != null : props.idea.stage === "open";
  return (
    <div className="idea-wrap">
      <div className="tabs" role="tablist">
        {(["overview", "notebook"] as const).map((t) => (
          <button
            key={t}
            type="button"
            role="tab"
            aria-selected={props.tab === t}
            disabled={t === "notebook" && !hasNotebook}
            title={t === "notebook" && !hasNotebook ? (isSeed ? "这个种子没有 notebook 文件" : "关掉的 idea 没有 worktree") : undefined}
            className={"tab" + (props.tab === t ? " is-on" : "")}
            onClick={() => props.onTab(t)}
          >
            {t === "overview" ? "概览" : "Notebook"}
          </button>
        ))}
        <span className="head-actions">
          <MenuButton
            label={<Glyph name="more" />}
            ariaLabel={`${props.idea.name} 的操作`}
            items={isSeed ? seedMenu(a, root, props.idea) : ideaMenu(a, root, props.board, props.idea)}
          />
        </span>
      </div>
      {props.tab === "notebook" && hasNotebook ? (
        <Notebook root={root} idea={props.idea.name} />
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
  const isSeed = b.project.kind === "home";
  const runs = b.runs.filter((r) => r.idea === i.name).reverse();
  const todos = b.todos.filter((t) => t.idea === i.name && t.status !== "done");
  const notes = b.notes.filter((n) => n.idea === i.name);
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="title">{i.name}</h1>
        <div className="sub">{isSeed ? i.gist : i.hypothesis}</div>
        {isSeed && i.hypothesis && <p className="pre">假设：{i.hypothesis}</p>}
        <div className="chips">
          {i.metric && (
            <span className="chip">
              指标 <b className="mono">{i.metric}</b>
            </span>
          )}
          {!isSeed && (
            <span className="chip">
              <b>{i.runs}</b> 个 run · <b>{su(i.su_used)}</b> SU
            </span>
          )}
          {i.parent && (
            <span className="chip">
              从 {i.parent}
              {i.from_run ? ` 的 ${i.from_run}` : ""} 分出
            </span>
          )}
          {i.stage !== "open" && (
            <span className="chip">{{ merged: "已合并", dropped: "已放弃", seed: "种子 · 思考中", planted: "种子 · 已种下" }[i.stage] ?? i.stage}</span>
          )}
          {(i.stage === "open" || (isSeed && i.notebook)) && (
            <button type="button" className="link small" onClick={() => props.onTab("notebook")}>
              打开 notebook →
            </button>
          )}
        </div>
      </div>

      {!isSeed && (
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
      )}

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

      {(!isSeed || notes.length > 0) && (
      <section className="section">
        <div className="section-head">
          <h2 className="heading">笔记</h2>
        </div>
        {i.stage === "open" && !isSeed && <NewNote root={b.project.root} idea={i.name} run={props.selectedRun} />}
        {notes.length === 0 && <p className="small muted">还没有笔记。</p>}
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
      )}
    </div>
  );
}

/** The box under an idea's notes: write one (attached to the selected run, if any). */
function NewNote(props: { root: string; idea: string; run: string | null }) {
  const a = useActions();
  const [text, setText] = useState("");
  const [attach, setAttach] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = attach ? props.run : null;
  const save = async () => {
    if (!text.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await a.act(props.root, { kind: "note", idea: props.idea, text, run }, "笔记已保存");
      setText("");
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="note-new">
      <textarea
        className="app-field body"
        placeholder="写一条笔记：看到了什么、下一步打算（⌘⏎ 保存）"
        maxLength={1000}
        value={text}
        disabled={busy}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void save();
          }
        }}
      />
      {error && <div className="err-box app-pre">{error}</div>}
      {text.trim() && (
        <div className="app-row small">
          {props.run && (
            <label className="app-row">
              <input type="checkbox" checked={attach} onChange={(e) => setAttach(e.target.checked)} /> 记在 {props.run} 上
            </label>
          )}
          <span className="ab-spacer" />
          <button type="button" className="ab-btn ab-btn-primary body" disabled={busy} onClick={() => void save()}>
            {busy ? "保存中…" : "保存笔记"}
          </button>
        </div>
      )}
    </div>
  );
}

export function SeedsView(props: { board: Board | null; homeRoot: string | null; now: number; onOpen: (seed: string) => void }) {
  const a = useActions();
  const all = props.board?.ideas ?? [];
  const seeds = all.filter((i) => i.stage === "seed");
  const dropped = all.filter((i) => i.stage === "dropped");
  const planted = all.filter((i) => i.stage === "planted");
  const home = props.board?.project.root ?? props.homeRoot ?? undefined;
  return (
    <div className="page">
      <div className="page-head">
        <div className="app-row">
          <h1 className="title">种子园</h1>
          <span className="head-actions">
            {home && (
              <button type="button" className="ab-btn ab-btn-primary body" onClick={() => a.ask(newSeed(a, home))}>
                <Glyph name="plus" /> 新的种子
              </button>
            )}
          </span>
        </div>
        <div className="sub">还没有仓库的想法。想清楚了，从「⋯」种进一个项目，变成 idea。</div>
      </div>
      {!props.board && <p className="small muted">还没有种子。记下第一个时，种子园建在 {props.homeRoot ?? "~/arboria"}。</p>}
      {props.board && seeds.length === 0 && <p className="small muted">没有种子。</p>}
      {home && seeds.length > 0 && (
        <div className="rows">
          {seeds.map((s) => (
            <div key={s.name} className="row has-menu" role="button" tabIndex={0} onClick={() => props.onOpen(s.name)} onKeyDown={(e) => e.key === "Enter" && props.onOpen(s.name)}>
              <Glyph name="sparkle" tone="muted" />
              <span className="main">
                <span className="t">{s.name}</span>
                <span className="m">{s.gist ?? ""}</span>
              </span>
              <span className="r">{ago(s.last_active, props.now)}</span>
              <span onClick={(e) => e.stopPropagation()}>
                <MenuButton label={<Glyph name="more" />} ariaLabel={`${s.name} 的操作`} items={seedMenu(a, home, s)} />
              </span>
            </div>
          ))}
        </div>
      )}
      {planted.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h2 className="heading">已种下</h2>
          </div>
          <div className="rows">
            {planted.map((s) => (
              <div key={s.name} className="row" role="button" tabIndex={0} onClick={() => props.onOpen(s.name)}>
                <Glyph name="check" tone="muted" />
                <span className="main">
                  <span className="t muted">{s.name}</span>
                  <span className="m">{s.gist ?? ""}</span>
                </span>
                <span className="r">{ago(s.last_active, props.now)}</span>
              </div>
            ))}
          </div>
        </section>
      )}
      {home && dropped.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h2 className="heading">已放弃</h2>
          </div>
          <div className="rows">
            {dropped.map((s) => (
              <div key={s.name} className="row" role="button" tabIndex={0} onClick={() => props.onOpen(s.name)}>
                <Glyph name="sparkle" tone="muted" />
                <span className="main">
                  <span className="t muted">{s.name}</span>
                  <span className="m">{s.gist ?? ""}</span>
                </span>
                <button type="button" className="ab-btn body" onClick={(e) => (e.stopPropagation(), void a.act(home, { kind: "seed_revive", name: s.name }, `已恢复 ${s.name}`).catch((e) => a.ask({ title: "没能恢复", body: message(e), confirm: "好", run: async () => undefined })))}>
                  恢复
                </button>
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
