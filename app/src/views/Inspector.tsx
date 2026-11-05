// The right panel: one run (status, resources, jobs, summary) and, when it waits for a
// signature, the card: the summary first, the whole card folded, 签字 after 2 s with
// Touch ID (the Rust side checks the 2 s and asks for it); elsewhere the command to run in
// your own terminal. A todo, or a command to run, for the other inbox items.

import { useEffect, useState, type ReactNode } from "react";
import { api, message } from "../api";
import type { Approved, Board, CardView, Platform } from "../types";
import { ago, su, STATUS_TEXT, statusTone } from "../lib/format";
import { Glyph } from "../components/Glyph";
import { cancelRun, rejectRun, useActions, writeNote } from "./actions";
import type { Inspect } from "./route";

const TONE: Record<string, string> = { ok: "ok", ink: "ok", accent: "run", warn: "warn", bad: "bad", muted: "" };

function Sign(props: { root: string; run: string; platform: Platform; now: () => number; onTerminal: (cmd: string) => void; onDone: () => void }) {
  const [card, setCard] = useState<CardView | null>(null);
  const [at, setAt] = useState(0);
  const [phase, setPhase] = useState<"ready" | "signing" | "done">("ready");
  const [result, setResult] = useState<Approved | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [, tick] = useState(0);
  useEffect(() => {
    setCard(null);
    setError(null);
    setPhase("ready");
    api.cardShown(props.root, props.run).then(
      (c) => {
        setCard(c);
        setAt(props.now());
      },
      (e) => setError(message(e)),
    );
    const t = setInterval(() => tick((n) => n + 1), 250);
    return () => clearInterval(t);
  }, [props.root, props.run]);
  const device = props.platform.touch_id && !props.platform.in_claude_code;
  const left = card ? Math.max(0, props.platform.min_shown_ms - (props.now() - at)) : 1;
  const usable = card?.matches === true && !!card.card_hash;
  const sign = async () => {
    if (!card?.card_hash || !usable || left > 0 || phase !== "ready") return;
    setPhase("signing");
    setError(null);
    try {
      setResult(await api.approve(props.root, props.run, card.card_hash));
      setPhase("done");
      props.onDone();
    } catch (e) {
      setError(message(e));
      setPhase("ready");
    }
  };
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && device) void sign();
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });
  const command = `arb approve ${props.run}`;
  if (phase === "done" && result)
    return (
      <div className="box">
        <div className="ab-ok strong">
          <Glyph name="check" /> 已签字并提交
        </div>
        <div className="small muted">
          {STATUS_TEXT[result.status] ?? result.status}
          {result.jobs.length > 0 && <> · 作业 <span className="mono">{result.jobs.join(", ")}</span></>}
        </div>
      </div>
    );
  return (
    <div className="box is-sign">
      <div className="strong">等你签字</div>
      {!card && !error && <div className="small muted">重新计算审核单…</div>}
      {card && (
        <>
          <dl className="kv small">
            <dt>资源</dt>
            <dd>{card.resources[0] ?? ""}</dd>
            <dt>时长</dt>
            <dd>{card.resources.find((x) => x.startsWith("时长"))?.replace("时长 ", "") ?? ""}</dd>
            <dt>命令</dt>
            <dd className="mono">{card.resources.find((x) => x.startsWith("命令"))?.replace("命令 ", "") ?? ""}</dd>
            <dt>花费</dt>
            <dd>{card.est_su != null ? `≈ ${su(card.est_su)} SU（上限）` : card.estimate.join(" · ")}</dd>
            <dt>预算</dt>
            <dd>{card.budget[0] ?? "没有设预算"}</dd>
          </dl>
          <details className="full">
            <summary>审核单全文（arb card {props.run}）</summary>
            <pre>{card.text ?? card.error}</pre>
          </details>
          {card.matches === false && <div className="err-box">审核单已变化（重新算出的哈希和提交时不同）：请重新 arb submit。</div>}
        </>
      )}
      {error && <div className="err-box">{error}</div>}
      <div className="sign-foot">
        {device ? (
          <>
            <span className="note">{left > 0 ? `读完再签字 · ${Math.ceil(left / 1000)} 秒` : "用触控 ID 签字"}</span>
            <button type="button" className="ab-btn ab-btn-primary" disabled={!usable || left > 0 || phase !== "ready"} onClick={() => void sign()}>
              <Glyph name="touch" /> {phase === "signing" ? "等待触控 ID…" : "签字"} <span className="ab-hint small">⌘⏎</span>
            </button>
          </>
        ) : (
          <>
            <span className="note">
              {props.platform.in_claude_code ? "app 从 Claude Code 里启动，不能在这里签字" : `这里不能用触控 ID（${props.platform.touch_id_why ?? "不是 Mac"}）`}
            </span>
            <button type="button" className="ab-btn" disabled={!usable} onClick={() => props.onTerminal(command)}>
              <Glyph name="terminal" /> 在终端里签字
            </button>
          </>
        )}
      </div>
    </div>
  );
}

export function Inspector(props: {
  what: Inspect;
  boards: Record<string, Board>;
  platform: Platform | null;
  now: () => number;
  onClose: () => void;
  onTerminal: (cmd: string) => void;
  onIdea: (root: string, idea: string) => void;
  onSigned: () => void;
}) {
  const w = props.what;
  const a = useActions();
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });
  let title = "";
  let body: ReactNode = null;
  if (w.k === "run") {
    const b = props.boards[w.root];
    const r = b?.runs.find((x) => x.id === w.run);
    title = w.run;
    if (!r) body = <p className="small muted">读取中…</p>;
    else {
      const s = statusTone(r.status);
      body = (
        <>
          <div className="chips">
            <span className="pill" data-tone={TONE[s.tone]}>
              <Glyph name={s.glyph} /> {STATUS_TEXT[r.status] ?? r.status}
            </span>
            <button type="button" className="link small" onClick={() => props.onIdea(w.root, r.idea)}>
              {r.idea}
            </button>
          </div>
          <div>{r.change_reason}</div>
          {r.status === "awaiting_approval" && props.platform && (
            <Sign root={w.root} run={r.id} platform={props.platform} now={props.now} onTerminal={props.onTerminal} onDone={props.onSigned} />
          )}
          <div className="app-row">
            {r.status === "awaiting_approval" && (
              <button type="button" className="ab-btn body" onClick={() => a.ask(rejectRun(a, w.root, r.id))}>
                <Glyph name="cross" /> 拒绝
              </button>
            )}
            {["approved", "queued", "running"].includes(r.status) && (
              <button type="button" className="ab-btn body" onClick={() => a.ask(cancelRun(a, w.root, r.id, r.machine))}>
                <Glyph name="cross" /> 取消作业
              </button>
            )}
            {b?.ideas.find((i) => i.name === r.idea)?.stage === "open" && (
              <button type="button" className="ab-btn body" onClick={() => a.ask(writeNote(a, w.root, r.idea, r.id))}>
                <Glyph name="pen" /> 写笔记
              </button>
            )}
          </div>
          <dl className="kv">
            <dt>机器</dt>
            <dd>
              {r.machine ?? "–"}
              {r.partition && ` · ${r.partition}`}
            </dd>
            <dt>资源</dt>
            <dd>
              {r.gpus ?? "?"} GPU × {r.walltime ?? "?"}
            </dd>
            <dt>花费</dt>
            <dd>{r.billed_su > 0 ? `${su(r.billed_su)} SU` : r.est_su != null ? `≈ ${su(r.est_su)} SU` : "–"}</dd>
            <dt>提交</dt>
            <dd>{ago(r.created_at, props.now())}</dd>
            {r.finished_at && (
              <>
                <dt>结束</dt>
                <dd>{ago(r.finished_at, props.now())}</dd>
              </>
            )}
            {r.parent_run && (
              <>
                <dt>基于</dt>
                <dd className="mono">{r.parent_run}</dd>
              </>
            )}
          </dl>
          {r.summary && (
            <div className="section">
              <div className="small muted">总结</div>
              <p className="pre">{r.summary}</p>
            </div>
          )}
          {r.jobs.length > 0 && (
            <div className="section">
              <div className="small muted">作业</div>
              <div className="jobs">
                {r.jobs.map((j) => (
                  <div key={j.segment} className="j">
                    <span>s{j.segment}</span>
                    <span>{j.job_id ?? "–"}</span>
                    <span className="muted">{j.state}</span>
                    {j.billed_su != null && <span className="muted">{su(j.billed_su)} SU</span>}
                  </div>
                ))}
              </div>
            </div>
          )}
          {r.wandb_url && (
            <a className="link small" href={r.wandb_url} target="_blank" rel="noreferrer">
              W&B
            </a>
          )}
        </>
      );
    }
  } else if (w.k === "todo") {
    const t = props.boards[w.root]?.todos.find((x) => x.id === w.task);
    title = w.task;
    body = t ? (
      <>
        <div className="chips">
          <span className="pill" data-tone={t.status === "done" ? "ok" : "run"}>{t.status === "done" ? "做完了" : t.status}</span>
          <button type="button" className="link small" onClick={() => props.onIdea(w.root, t.idea)}>
            {t.idea}
          </button>
        </div>
        <p className="pre">{t.text}</p>
        {t.result && (
          <div className="section">
            <div className="small muted">agent 的结果</div>
            <p className="pre">{t.result}</p>
          </div>
        )}
        {t.status !== "done" && t.status !== "dropped" && (
          <div className="app-row">
            <button
              type="button"
              className="ab-btn body"
              onClick={() =>
                a.ask({
                  title: `放弃待办 ${t.id}`,
                  body: "agent 不再做它。",
                  confirm: "放弃",
                  danger: true,
                  run: () => a.act(w.root, { kind: "todo_drop", task: t.id }, `已放弃 ${t.id}`),
                })
              }
            >
              <Glyph name="trash" /> 放弃这个待办
            </button>
          </div>
        )}
      </>
    ) : (
      <p className="small muted">读取中…</p>
    );
  } else {
    title = w.title;
    body = (
      <>
        <p className="pre">{w.text}</p>
        <div className="box">
          <div className="small muted">这一步只在你自己的终端里做</div>
          <code className="code">{w.command}</code>
          <div>
            <button type="button" className="ab-btn" onClick={() => props.onTerminal(w.command)}>
              <Glyph name="terminal" /> 在终端里填好
            </button>
          </div>
        </div>
      </>
    );
  }
  return (
    <aside className="inspector" aria-label="详情">
      <div className="insp-head">
        <span className="ttl">{title}</span>
        <span style={{ flex: 1 }} />
        <button type="button" className="icon-btn" aria-label="关闭详情（esc）" title="关闭（esc）" onClick={props.onClose}>
          <Glyph name="cross" />
        </button>
      </div>
      <div className="insp-body">{body}</div>
    </aside>
  );
}
