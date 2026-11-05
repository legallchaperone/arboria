// A block's outputs, by kind (ADR 0005). Everything is drawn from data as React elements
// or a uPlot canvas — never as HTML (`src/lib/rules.test.ts`).

import { useEffect, useRef, useState, useSyncExternalStore, type JSX } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { AgentRow, InspectResult, NbCached, NbOutput } from "../types";
import { parseBlocks, type Node } from "./markdown";
import { codeSha, type NbStore } from "./store";
import { ago } from "../lib/format";
import { clock } from "../App";

// ---------------------------------------------------------------- markdown (no HTML)

function inline(nodes: Node[] | undefined): JSX.Element[] {
  return (nodes ?? []).map((n, k) => {
    if (n.type === "hardBreak") return <br key={k} />;
    let el: JSX.Element = <>{n.text ?? ""}</>;
    for (const m of n.marks ?? []) {
      if (m.type === "code") el = <code>{el}</code>;
      else if (m.type === "bold") el = <strong>{el}</strong>;
      else if (m.type === "italic") el = <em>{el}</em>;
      // links are shown, not followed: the window never navigates
      else if (m.type === "link") el = <span className="nb-md-link" title={String(m.attrs?.href ?? "")}>{el}</span>;
    }
    return <span key={k}>{el}</span>;
  });
}

function mdTable(lines: string[]): JSX.Element | null {
  if (lines.length < 2 || !lines.every((l) => l.trim().startsWith("|"))) return null;
  if (!/^\s*\|?\s*:?-{2,}/.test(lines[1])) return null;
  const cells = (l: string) => l.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
  const head = cells(lines[0]);
  return (
    <div className="nb-table-box">
      <table className="nb-table">
        <thead>
          <tr>{head.map((c, i) => <th key={i}>{inline(parseInlineSafe(c))}</th>)}</tr>
        </thead>
        <tbody>
          {lines.slice(2).map((l, r) => (
            <tr key={r}>{cells(l).map((c, i) => <td key={i}>{inline(parseInlineSafe(c))}</td>)}</tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function parseInlineSafe(s: string): Node[] {
  const p = parseBlocks(s)[0];
  return p?.content ?? [];
}

function blocks(nodes: Node[]): JSX.Element[] {
  return nodes.map((n, k) => {
    switch (n.type) {
      case "heading":
        return <div key={k} className={"nb-md-h nb-md-h" + Math.min(3, Number(n.attrs?.level ?? 1))}>{inline(n.content)}</div>;
      case "bulletList":
        return <ul key={k}>{(n.content ?? []).map((li, i) => <li key={i}>{blocks(li.content ?? [])}</li>)}</ul>;
      case "orderedList":
        return (
          <ol key={k} start={Number(n.attrs?.start ?? 1)}>
            {(n.content ?? []).map((li, i) => <li key={i}>{blocks(li.content ?? [])}</li>)}
          </ol>
        );
      case "blockquote":
        return <blockquote key={k}>{blocks(n.content ?? [])}</blockquote>;
      case "codeBlock":
        return <pre key={k} className="nb-code">{(n.content ?? []).map((t) => t.text).join("")}</pre>;
      case "horizontalRule":
        return <hr key={k} />;
      default: {
        const text = (n.content ?? []).map((t) => (t.type === "hardBreak" ? "\n" : (t.text ?? ""))).join("");
        const table = mdTable(text.split("\n"));
        return table ? <div key={k}>{table}</div> : <p key={k}>{inline(n.content)}</p>;
      }
    }
  });
}

export function Markdown(props: { text: string }) {
  return <div className="nb-md">{blocks(parseBlocks(props.text))}</div>;
}

// ---------------------------------------------------------------- plot

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

function fmtNum(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  const a = Math.abs(v);
  if (a !== 0 && (a >= 1e5 || a < 1e-3)) return v.toExponential(2);
  return String(Math.round(v * 1000) / 1000);
}

function Plot(props: { out: Extract<NbOutput, { kind: "plot" }> }) {
  const host = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState<number | null>(null);
  const o = props.out;
  // one shared x axis: the union of every series' x (uPlot wants aligned data)
  const xs = [...new Set(o.series.flatMap((s) => s.x.filter((v): v is number => v != null)))].sort((a, b) => a - b);
  const ys = o.series.map((s) => {
    const m = new Map<number, number | null>();
    s.x.forEach((x, i) => x != null && m.set(x, s.y[i] ?? null));
    return xs.map((x) => (m.has(x) ? m.get(x)! : null));
  });
  const key = JSON.stringify([o.series.map((s) => s.name), xs.length, o.log, o.style]);
  useEffect(() => {
    const el = host.current;
    if (!el || !xs.length) return;
    const colors = [1, 2, 3, 4, 5, 6].map((i) => css(`--series-${i}`));
    const tick = css("--ink-faint");
    const grid = css("--line");
    const font = `11px ${css("--font-mono")}`;
    const bar = o.style === "bar";
    const axis = (extra: uPlot.Axis = {}): uPlot.Axis => ({
      stroke: tick,
      font,
      grid: { stroke: grid, width: 1 },
      ticks: { show: false },
      gap: 6,
      size: 40,
      ...extra,
    });
    const u = new uPlot(
      {
        width: Math.max(260, el.clientWidth || 600),
        height: bar ? 180 : 220,
        padding: [8, 8, 0, 0],
        legend: { show: false },
        cursor: { y: false, points: { size: 7, width: 2 }, drag: { x: false, y: false } },
        scales: {
          x: { time: false, ...(bar ? { range: (_u, mn, mx) => [mn - 0.6, mx + 0.6] as uPlot.Range.MinMax } : {}) },
          y: o.log ? { distr: 3 } : {},
        },
        axes: [
          axis({
            label: o.x,
            labelFont: font,
            labelSize: o.x ? 18 : 0,
            // bars sit on whole numbers: no ticks in between
            ...(bar ? { values: (_u: uPlot, v: number[]) => v.map((x) => (Number.isInteger(x) ? String(x) : "")) } : {}),
          }), axis({ size: 52, label: o.y, labelFont: font, labelSize: o.y ? 18 : 0, values: (_u, v) => v.map(fmtNum) })],
        series: [
          {},
          ...o.series.map((s, i) => ({
            label: s.name,
            stroke: colors[i % colors.length],
            width: bar ? 0 : 1.75,
            ...(bar ? { fill: colors[i % colors.length], paths: uPlot.paths.bars!({ size: [0.62, 26], radius: 0.18 }) } : {}),
            points: { show: false },
            spanGaps: false,
          })),
        ],
        hooks: { setCursor: [(u) => setAt(u.cursor.idx ?? null)] },
      },
      [xs, ...ys] as uPlot.AlignedData,
      el,
    );
    const ro = new ResizeObserver(() => {
      const w = el.clientWidth;
      if (w && Math.abs(w - u.width) > 1) u.setSize({ width: w, height: u.height });
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      u.destroy();
    };
    // the chart is rebuilt when the data's shape changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, JSON.stringify(ys)]);
  if (!xs.length) return <div className="nb-muted">（图里没有数据点）</div>;
  const k = at ?? xs.length - 1;
  return (
    <div className="nb-chart">
      <div className="nb-legend">
        {o.title ? <span className="nb-legend-title">{o.title}</span> : null}
        {o.series.map((s, i) => (
          <span key={i} className="nb-legend-item">
            <i className="nb-sw" style={{ background: `var(--series-${(i % 6) + 1})` }} />
            <span className="nb-legend-id">{s.name}</span>
            <b className="nb-legend-val">{fmtNum(ys[i][k] ?? (at == null ? lastValue(ys[i]) : null))}</b>
          </span>
        ))}
        <span className="nb-legend-at">{at == null ? "最后" : `${o.x ?? "x"} = ${fmtNum(xs[k])}`}</span>
      </div>
      <div className="nb-plot" ref={host} onMouseLeave={() => setAt(null)} />
      {o.note ? <div className="nb-prov">{o.note}</div> : null}
    </div>
  );
}

function lastValue(ys: (number | null)[]): number | null {
  for (let i = ys.length - 1; i >= 0; i--) if (ys[i] != null) return ys[i];
  return null;
}

// ---------------------------------------------------------------- the others

function Table(props: { out: Extract<NbOutput, { kind: "table" }> }) {
  const o = props.out;
  const numeric = o.columns.map((_, i) => o.rows.length > 0 && o.rows.every((r) => r[i] == null || typeof r[i] === "number"));
  const cell = (v: unknown) => (v == null ? "" : typeof v === "number" ? fmtNum(v) : typeof v === "boolean" ? String(v) : String(v));
  return (
    <div>
      {o.title ? <div className="nb-legend-title">{o.title}</div> : null}
      <div className="nb-table-box">
        <table className="nb-table">
          <thead>
            <tr>
              {o.columns.map((c, i) => (
                <th key={i} className={numeric[i] ? "num" : ""}>
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {o.rows.map((r, j) => (
              <tr key={j}>
                {r.map((v, i) => (
                  <td key={i} className={numeric[i] ? "num" : ""}>
                    {cell(v)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {o.total > o.rows.length ? <div className="nb-prov">显示前 {o.rows.length} 行，共 {o.total} 行</div> : null}
    </div>
  );
}

const TASK_STATE: Record<string, string> = {
  open: "等你交给 agent",
  queued: "排队等 agent",
  running: "agent 正在做",
  check_failed: "agent 交的没通过检查，再试一次",
  needs_human: "agent 没做成，要你看一下",
  done: "agent 做完了，正在重跑",
  dropped: "待办已放弃",
};

function Waiting(props: { task: string | null; text: string; store: NbStore }) {
  const row: AgentRow | undefined = props.task ? props.store.tasks.get(props.task) : undefined;
  const stuck = row && (row.status === "needs_human" || row.status === "dropped");
  return (
    <div className="nb-pending">
      <div className="nb-line">
        <span className={"nb-pending-mark" + (stuck ? " is-stuck" : "")} />
        <span>{props.text}</span>
      </div>
      {props.task ? (
        <div className="nb-pending-sub">
          {row ? TASK_STATE[row.status] ?? row.status : "等 agent"}
          {row?.attempts ? ` · 第 ${row.attempts} 次` : ""}
          <span className="nb-dot">·</span>
          <button type="button" className="app-link nb-link" onClick={() => props.store.actions.openTask(props.task!)}>
            在 agent 面板里看 {props.task}
          </button>
        </div>
      ) : null}
      {!stuck && props.task ? (
        <div className="nb-progress">
          <i />
        </div>
      ) : null}
    </div>
  );
}

function Inspect(props: { out: Extract<NbOutput, { kind: "inspect" }>; store: NbStore }) {
  const o = props.out;
  const r: InspectResult | null = o.result;
  if (!r) return <Waiting task={o.task} store={props.store} text={o.text || `请 agent 读代码：${o.question}`} />;
  return (
    <div className="nb-inspect">
      <div className="nb-inspect-title">{r.title}</div>
      {r.summary ? <p className="nb-inspect-sum">{r.summary}</p> : null}
      {r.spans.map((s) => (
        <div key={s.id} className="nb-span">
          <div className="nb-prov">
            <code>
              {s.path}:{s.start}–{s.end}
            </code>
            {s.note ? <span>{s.note}</span> : null}
          </div>
          <div className="nb-code-box">
            {s.lines.map((line, i) => {
              const n = s.start + i;
              const note = s.line_notes[String(n)];
              return (
                <div key={n}>
                  <div className={"nb-code-row" + (note ? " has-note" : "")}>
                    <span className="ln">{n}</span>
                    <span className="src">{line.replace(/\r?\n$/, "")}</span>
                  </div>
                  {note ? (
                    <div className="nb-note-row">
                      <span className="ln" />
                      <span className="note">{note}</span>
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>
      ))}
      {r.text ? <pre className="nb-code">{r.text}</pre> : null}
      {r.mermaid ? (
        <>
          <div className="nb-prov">架构图（Mermaid 源码）</div>
          <pre className="nb-code">{r.mermaid}</pre>
        </>
      ) : null}
      <div className="nb-prov">
        <span>agent 读的代码版本</span>
        <code>{r.code_ref.slice(0, 8)}</code>
        {o.text ? <span>{o.text}</span> : null}
      </div>
    </div>
  );
}

function ErrorOut(props: { out: Extract<NbOutput, { kind: "error" }> }) {
  const [open, setOpen] = useState(false);
  const o = props.out;
  return (
    <div className="nb-error">
      <div className="nb-line">
        <b>{o.ename}</b>
        {o.evalue ? `：${o.evalue}` : ""}
      </div>
      {o.traceback.length ? (
        <>
          <button type="button" className="app-link nb-link" onClick={() => setOpen(!open)}>
            {open ? "收起" : "展开"} traceback
          </button>
          {open ? <pre className="nb-code">{o.traceback.join("\n")}</pre> : null}
        </>
      ) : null}
    </div>
  );
}

export function Output(props: { out: NbOutput; store: NbStore }) {
  const o = props.out;
  switch (o.kind) {
    case "text":
      return <pre className="nb-text">{o.text + (o.truncated ? "\n…（截断了）" : "")}</pre>;
    case "stream":
      return <pre className={"nb-text" + (o.stderr ? " is-stderr" : "")}>{o.text.replace(/\n$/, "")}</pre>;
    case "markdown":
      return <Markdown text={o.text} />;
    case "table":
      return <Table out={o} />;
    case "plot":
      return <Plot out={o} />;
    case "image":
      return o.mime === "image/png" || o.mime === "image/jpeg" ? (
        <img className="nb-image" alt="" src={`data:${o.mime};base64,${o.data}`} />
      ) : null;
    case "error":
      return <ErrorOut out={o} />;
    case "pending":
      return <Waiting task={o.task} store={props.store} text={o.text || `命令 ${o.name ?? ""} 还不存在，已交给 agent`} />;
    case "run":
      return (
        <div className="nb-run">
          {o.text ? <Markdown text={o.text} /> : <div className="nb-line">审核单 {o.run}</div>}
          <div className="app-row">
            <button type="button" className="ab-btn" onClick={() => props.store.actions.openRun(o.run)}>
              看审核单 {o.run}
            </button>
            <span className="nb-muted">签字在审核单面板里（触控 ID）</span>
          </div>
        </div>
      );
    case "inspect":
      return <Inspect out={o} store={props.store} />;
    default:
      return <pre className="nb-text">{JSON.stringify(o)}</pre>;
  }
}

// ---------------------------------------------------------------- under a block

function useBlock(store: NbStore, id: string) {
  return useSyncExternalStore(
    (f) => store.subscribe(id, f),
    () => store.get(id),
  );
}

function useSha(code: string): string | null {
  const [sha, setSha] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const t = setTimeout(() => void codeSha(code).then((s) => live && setSha(s)), 150);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [code]);
  return sha;
}

function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} 分 ${Math.round((ms % 60_000) / 1000)} 秒`;
}

/** Everything under one command block: the live or cached outputs and a status line. */
export function BlockOutputs(props: { store: NbStore; id: string; code: string }) {
  const s = useBlock(props.store, props.id);
  const sha = useSha(props.code);
  const c: NbCached | null = s.cached;
  const outs = s.running || s.live ? (s.live ?? []) : (c?.outputs ?? []);
  const changed = !s.running && c && c.code_sha && sha && sha !== c.code_sha;
  const meta: JSX.Element[] = [];
  if (s.queued) meta.push(<span key="q">等前面的块跑完</span>);
  else if (s.running)
    meta.push(
      <span key="r" className="nb-running">
        <span className="nb-spinner" />
        运行中
      </span>,
    );
  else if (c) {
    meta.push(<span key="t">{c.ok ? "" : "出错 · "}{ago(c.ran_at, clock.now())}运行 · {duration(c.duration_ms)}</span>);
    if (changed) meta.push(<span key="c" className="nb-chip">代码改过</span>);
    if (s.stale) meta.push(<span key="s" className="nb-chip">过期：上面的块重跑过</span>);
  }
  if (s.error) meta.push(<span key="e" className="nb-err">{s.error}</span>);
  if (!outs.length && !meta.length) return null;
  return (
    <>
      {outs.map((o, i) => (
        <div key={i} className="nb-out-item">
          <Output out={o} store={props.store} />
        </div>
      ))}
      {meta.length ? <div className="nb-meta">{meta}</div> : null}
    </>
  );
}
