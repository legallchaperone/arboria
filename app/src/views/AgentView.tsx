// The agent panel (ADR 0005): every todo the background `claude -p` works on — kind,
// owner, status, model, attempt, turns, tokens, cost, time — and, per todo, its log and
// the buttons: hand to the agent, stop, retry, drop. All through `arb` (actions.rs).
// Settings (agent.toml): auto, model, turns, daily limit, timeout.

import { useEffect, useState } from "react";
import { api, message, onAgent } from "../api";
import type { AgentLogLine, AgentRow, AgentSettings, ProjectSummary } from "../types";
import { ago, clip } from "../lib/format";
import { Glyph } from "../components/Glyph";
import { useActions } from "./actions";
import { clock } from "../App";

export const KIND_TEXT: Record<string, string> = {
  plain: "待办",
  command_new: "写命令",
  command_revise: "改命令",
  inspect: "读代码",
  probe_new: "写探针",
  probe_revise: "改探针",
};

export const AGENT_STATUS: Record<AgentRow["status"], { text: string; tone: string }> = {
  open: { text: "等你交给 agent", tone: "" },
  queued: { text: "排队", tone: "run" },
  running: { text: "在做", tone: "run" },
  check_failed: { text: "没通过检查", tone: "warn" },
  needs_human: { text: "要你看", tone: "bad" },
  done: { text: "做完了", tone: "ok" },
  dropped: { text: "已放弃", tone: "" },
};

const k = (n: number) => (n >= 10_000 ? `${Math.round(n / 1000)}k` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));

function took(r: AgentRow): string {
  if (!r.started_at) return "";
  const end = r.finished_at && r.status !== "running" ? Date.parse(r.finished_at) : clock.now();
  const s = Math.max(0, Math.round((end - Date.parse(r.started_at)) / 1000));
  return s < 60 ? `${s} 秒` : `${Math.floor(s / 60)} 分 ${s % 60} 秒`;
}

/** The rows of one project, kept fresh while the panel is open. */
export function useAgentRows(root: string | null): { rows: AgentRow[] | null; error: string | null; reload: () => void } {
  const [rows, setRows] = useState<AgentRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!root) return;
    let live = true;
    const load = () =>
      api.agentRows(root).then(
        (r) => {
          if (!live) return;
          setRows(r);
          setError(null);
        },
        (e) => live && setError(message(e)),
      );
    void load();
    const un = onAgent(
      (r) => r === root && void load(),
      () => {},
    );
    const t = setInterval(load, 15_000);
    return () => {
      live = false;
      clearInterval(t);
      void un.then((fs) => fs.forEach((f) => f()));
    };
  }, [root, n]);
  return { rows, error, reload: () => setN((x) => x + 1) };
}

/** The buttons a todo's state allows. */
function TaskButtons(props: { root: string; row: AgentRow; onDone: () => void }) {
  const a = useActions();
  const r = props.row;
  const act = (action: Parameters<typeof a.act>[1], done: string) => void a.act(props.root, action, done).then(props.onDone, () => {});
  return (
    <div className="app-row">
      {r.status === "open" && (
        <button type="button" className="ab-btn ab-btn-primary" onClick={() => act({ kind: "agent_queue", task: r.id }, `已交给 agent：${r.id}`)}>
          <Glyph name="sparkle" /> 交给 agent
        </button>
      )}
      {(r.status === "needs_human" || r.status === "check_failed") && (
        <button type="button" className="ab-btn" onClick={() => act({ kind: "agent_queue", task: r.id }, `${r.id} 重新排队`)}>
          重试
        </button>
      )}
      {r.status === "running" && (
        <button type="button" className="ab-btn" onClick={() => act({ kind: "agent_stop", task: r.id }, `已停下 ${r.id}`)}>
          停止
        </button>
      )}
      {r.status !== "done" && r.status !== "dropped" && r.status !== "running" && (
        <button
          type="button"
          className="ab-btn"
          onClick={() =>
            a.ask({
              title: `放弃待办 ${r.id}`,
              body: "agent 不再做它。",
              confirm: "放弃",
              danger: true,
              run: () => a.act(props.root, { kind: "todo_drop", task: r.id }, `已放弃 ${r.id}`).then(props.onDone),
            })
          }
        >
          <Glyph name="trash" /> 放弃
        </button>
      )}
    </div>
  );
}

function Usage(props: { row: AgentRow }) {
  const r = props.row;
  const u = r.usage;
  return (
    <dl className="kv nb-kv">
      <dt>状态</dt>
      <dd>
        <span className="pill" data-tone={AGENT_STATUS[r.status].tone}>
          {AGENT_STATUS[r.status].text}
        </span>
      </dd>
      <dt>种类</dt>
      <dd>{KIND_TEXT[r.kind] ?? r.kind}</dd>
      <dt>属于</dt>
      <dd className="mono">{r.idea ?? r.seed ?? "?"}</dd>
      {r.model && (
        <>
          <dt>模型</dt>
          <dd className="mono">{r.model}</dd>
        </>
      )}
      {r.attempts > 0 && (
        <>
          <dt>第几次</dt>
          <dd>{r.attempts}</dd>
        </>
      )}
      {u && (
        <>
          <dt>轮数</dt>
          <dd>{u.turns}</dd>
          <dt>token</dt>
          <dd className="mono">
            入 {k(u.input_tokens)} · 出 {k(u.output_tokens)} · 缓存读 {k(u.cache_read_tokens)}
          </dd>
          <dt>花费</dt>
          <dd className="mono">${u.cost_usd.toFixed(3)}（订阅额度）</dd>
        </>
      )}
      {r.started_at && (
        <>
          <dt>用时</dt>
          <dd>{took(r)}</dd>
        </>
      )}
    </dl>
  );
}

function Log(props: { root: string; row: AgentRow }) {
  const [lines, setLines] = useState<AgentLogLine[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { root, row } = props;
  useEffect(() => {
    let live = true;
    let pending: ReturnType<typeof setTimeout> | null = null;
    const load = () =>
      api.agentLog(root, row.id).then(
        (l) => live && setLines(l),
        (e) => live && setError(message(e)),
      );
    void load();
    // while it runs, re-read the log as lines come (at most twice a second)
    const un = onAgent(
      (r) => r === root && void load(),
      (r) => {
        if (r !== root || pending) return;
        pending = setTimeout(() => {
          pending = null;
          void load();
        }, 500);
      },
    );
    return () => {
      live = false;
      if (pending) clearTimeout(pending);
      void un.then((fs) => fs.forEach((f) => f()));
    };
  }, [root, row.id, row.status, row.attempts]);
  if (error) return <p className="small nb-err">{error}</p>;
  if (!lines) return <p className="small muted">读取日志…</p>;
  if (!lines.length) return <p className="small muted">还没有日志（agent 还没开始做）</p>;
  return (
    <div className="nb-log">
      {lines.map((l, i) => (
        <div key={i} className={"nb-log-line is-" + l.kind}>
          <span className="nb-log-kind">{{ text: "说", tool: "工具", result: "结果", done: "结束", error: "错误" }[l.kind] ?? l.kind}</span>
          <span className="nb-log-text">{l.text}</span>
        </div>
      ))}
    </div>
  );
}

/** One todo in the inspector: what it asks, where the agent is, its log, the buttons. */
export function AgentTask(props: { root: string; task: string; onIdea: (idea: string) => void }) {
  const { rows, error, reload } = useAgentRows(props.root);
  const r = rows?.find((x) => x.id === props.task);
  if (error) return <p className="small nb-err">{error}</p>;
  if (!rows) return <p className="small muted">读取中…</p>;
  if (!r) return <p className="small muted">找不到 {props.task}</p>;
  return (
    <>
      <p className="pre">{r.text}</p>
      {(r.idea ?? r.seed) && (
        <button type="button" className="link small" onClick={() => props.onIdea((r.idea ?? r.seed)!)}>
          打开 {r.idea ?? r.seed}
        </button>
      )}
      <Usage row={r} />
      {r.result && (
        <div className="section">
          <div className="small muted">agent 的结果</div>
          <p className="pre">{r.result}</p>
        </div>
      )}
      <TaskButtons root={props.root} row={r} onDone={reload} />
      {r.attempts > 0 && (
        <div className="section">
          <div className="small muted">最近一次的日志</div>
          <Log root={props.root} row={r} />
        </div>
      )}
    </>
  );
}

function Settings() {
  const [s, setS] = useState<AgentSettings | null>(null);
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    api.agentSettings().then(setS, (e) => setNote(message(e)));
  }, []);
  if (!s) return note ? <p className="small nb-err">{note}</p> : null;
  const num = (key: "max_turns" | "daily_limit" | "timeout_s", label: string, hint: string) => (
    <label className="nb-field">
      <span>{label}</span>
      <input
        className="app-input nb-input mono"
        inputMode="numeric"
        value={String(s[key])}
        onChange={(e) => setS({ ...s, [key]: Number(e.target.value.replace(/\D/g, "")) || 0 })}
      />
      <span className="small muted">{hint}</span>
    </label>
  );
  return (
    <div className="box">
      <label className="nb-check">
        <input type="checkbox" checked={s.auto} onChange={(e) => setS({ ...s, auto: e.target.checked })} />
        <span>缺命令和读代码的待办一出现就交给 agent</span>
      </label>
      <div className="nb-fields">
        <label className="nb-field">
          <span>模型</span>
          <input className="app-input nb-input mono" value={s.model} onChange={(e) => setS({ ...s, model: e.target.value.trim() })} />
          <span className="small muted">sonnet、opus、haiku 或完整的模型名</span>
        </label>
        {num("max_turns", "每次最多几轮", "claude -p --max-turns")}
        {num("daily_limit", "每天最多几次", "每个项目分开算")}
        {num("timeout_s", "每次最多几秒", "超时就停下，记成没通过")}
      </div>
      <div className="app-row">
        <button
          type="button"
          className="ab-btn"
          onClick={() =>
            api.agentSaveSettings(s).then(
              (saved) => {
                setS(saved);
                setNote("已保存");
              },
              (e) => setNote(message(e)),
            )
          }
        >
          保存设置
        </button>
        {note && <span className="small muted">{note}</span>}
      </div>
      <p className="small muted">
        agent 是你自己登录的 <code>claude</code>（<code>claude -p</code>，用订阅额度），在 idea 的 worktree 里跑。它不能签字、提交作业、改 notebook。
      </p>
    </div>
  );
}

function ProjectRows(props: { p: ProjectSummary; now: number; onTask: (root: string, task: string) => void }) {
  const { rows, error } = useAgentRows(props.p.root);
  const shown = (rows ?? []).filter((r) => r.kind !== "plain" || r.status !== "open" || r.attempts > 0);
  return (
    <div className="section">
      <div className="section-head">
        <h2 className="heading">{props.p.kind === "home" ? "种子园" : props.p.name}</h2>
        {error && <span className="small nb-err">{error}</span>}
      </div>
      {rows && !shown.length ? <p className="small muted">没有交给 agent 的待办</p> : null}
      {shown.length > 0 && (
        <div className="nb-table-box">
          <table className="nb-table nb-agent-table">
            <thead>
              <tr>
                <th>待办</th>
                <th>种类</th>
                <th>属于</th>
                <th>状态</th>
                <th>模型</th>
                <th className="num">次</th>
                <th className="num">轮</th>
                <th className="num">token 入 / 出 / 缓存</th>
                <th className="num">花费</th>
                <th>时间</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((r) => (
                <tr key={r.id} className="nb-agent-row" onClick={() => props.onTask(props.p.root, r.id)} title={r.text}>
                  <td>
                    <span className="mono">{r.id}</span> <span className="muted">{clip(r.text, 32)}</span>
                  </td>
                  <td>{KIND_TEXT[r.kind] ?? r.kind}</td>
                  <td className="mono">{r.idea ?? r.seed ?? ""}</td>
                  <td>
                    <span className="pill" data-tone={AGENT_STATUS[r.status].tone}>
                      {AGENT_STATUS[r.status].text}
                    </span>
                  </td>
                  <td className="mono">{r.model ?? ""}</td>
                  <td className="num">{r.attempts || ""}</td>
                  <td className="num">{r.usage?.turns ?? ""}</td>
                  <td className="num">{r.usage ? `${k(r.usage.input_tokens)} / ${k(r.usage.output_tokens)} / ${k(r.usage.cache_read_tokens)}` : ""}</td>
                  <td className="num">{r.usage ? `$${r.usage.cost_usd.toFixed(2)}` : ""}</td>
                  <td className="muted">{r.status === "running" ? took(r) : ago(r.finished_at ?? r.created_at, props.now)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** The page: every project's agent todos, and the settings. */
export function AgentView(props: { projects: ProjectSummary[]; now: number; onTask: (root: string, task: string) => void }) {
  const [showSettings, setShowSettings] = useState(false);
  return (
    <div className="page is-wide">
      <div className="page-head">
        <h1 className="title">Agent</h1>
        <div className="sub">缺的命令、读代码的请求交给后台的 claude -p，一次一个；它交的东西经 arb 校验。</div>
      </div>
      <div className="app-row">
        <button type="button" className="ab-btn" onClick={() => setShowSettings(!showSettings)}>
          {showSettings ? "收起设置" : "设置"}
        </button>
      </div>
      {showSettings && <Settings />}
      {props.projects
        .filter((p) => !p.error)
        .map((p) => (
          <ProjectRows key={p.root} p={p} now={props.now} onTask={props.onTask} />
        ))}
    </div>
  );
}
