// 全局 A: the idea timeline, the spend bar, the seeds, each machine's probes, and the
// background helper.

import type { Board, HelperStatus, ProbeView, ProjectSummary } from "../types";
import { ago, su, statusTone } from "../lib/format";
import { layout } from "../lib/timeline";
import { Glyph } from "../components/Glyph";

const LANE = 48;

function Timeline({ board, onIdea }: { board: Board; onIdea: (idea: string) => void }) {
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

function ProbeCard({ p, now }: { p: ProbeView; now: number }) {
  const r = p.result;
  return (
    <div className="ab-panel app-probe">
      <div className="small ab-muted">
        {p.machine} · {p.probe.replace(/^builtin:/, "")} · {ago(p.at, now)}
      </div>
      {r.kind === "number" && (
        <div>
          <span className="ab-muted">{r.label} </span>
          <span className="strong">{r.value}</span>
          {r.of != null && <span className="ab-muted"> / {r.of}</span>}
          {r.unit && <span className="ab-muted"> {r.unit}</span>}
        </div>
      )}
      {r.kind === "text" && <pre className="small app-pre">{r.text}</pre>}
      {r.kind === "table" && (
        <table className="ab-table small">
          <thead>
            <tr>{r.columns.map((c) => <th key={c}>{c}</th>)}</tr>
          </thead>
          <tbody>
            {r.rows.map((row, i) => (
              <tr key={i}>{row.map((c, j) => <td key={j}>{c == null ? "" : String(c)}</td>)}</tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

export function Overview(props: {
  board: Board | null;
  error: string | null;
  projects: ProjectSummary[];
  helper: HelperStatus | null;
  now: number;
  onIdea: (idea: string) => void;
  onProject: (root: string) => void;
  onInstallHelper: () => void;
  onLogin: (machine: string) => void;
  onMachines: () => void;
  onEditConfig: (path: string) => void;
}) {
  const b = props.board;
  const homeSeeds = props.projects.find((p) => p.kind === "home")?.seeds ?? 0;
  const others = props.projects.filter((p) => p.kind === "project" && p.root !== b?.project.root);
  return (
    <main className="app-main">
      <div className="app-wide">
        {props.error && <p className="small ab-bad">{props.error}</p>}
        {b && (
          <>
            <div className="app-head-block">
              <div className="app-head">
                <h1 className="title">{b.project.name}</h1>
                <span className="small ab-muted">
                  {b.ideas.filter((i) => i.stage !== "seed").length} 个 idea · {b.runs.length} 个 run
                  {layout(b).lanes.length ? ` · ${layout(b).span}` : ""}
                </span>
              </div>
              <div className="app-stats">
                <span>
                  <span className="ab-muted">进行中的 idea</span> <span className="strong">{b.ideas.filter((i) => i.stage === "open").length}</span>
                </span>
                <span className="ab-muted" aria-hidden="true">·</span>
                <span>
                  <span className="ab-muted">已用</span> <span className="strong">{su(b.spend.used)}</span>
                  {b.spend.total > 0 && <span className="ab-muted"> / {su(b.spend.total)} SU</span>}
                </span>
                <span className="ab-muted" aria-hidden="true">·</span>
                <span>
                  <span className="ab-muted">等你签字</span> <span className="strong">{b.inbox.filter((i) => i.kind === "approve").length}</span>
                </span>
              </div>
              {b.project.config_error && <p className="small ab-bad">{b.project.config_error}</p>}
            </div>

            <section aria-labelledby="tl-h" className="app-section">
              <div className="app-head">
                <h2 className="heading" id="tl-h">idea 和 run</h2>
                <div className="small ab-muted app-legend">
                  <span><Glyph name="check" />完成</span>
                  <span><Glyph name="play" tone="accent" />运行中</span>
                  <span><Glyph name="touch" tone="warn" />待签字</span>
                  <span><Glyph name="cross" tone="bad" />失败</span>
                  <span><i className="app-legend-fork" />竖线：从父 run 分出</span>
                </div>
              </div>
              <Timeline board={b} onIdea={props.onIdea} />
            </section>

            <section aria-labelledby="sp-h" className="app-section">
              <div className="app-head">
                <h2 className="heading" id="sp-h">花费</h2>
                <span>
                  <span className="strong">{su(b.spend.used)} SU</span>
                  {b.spend.total > 0 && <span className="ab-muted"> / {su(b.spend.total)} SU · 预算剩 {su(b.spend.total - b.spend.used)} SU</span>}
                </span>
              </div>
              {b.spend.total > 0 && (
                <div className="app-spend" role="img" aria-label={`预算 ${su(b.spend.total)} SU 中已用 ${su(b.spend.used)}`}>
                  {b.spend.by_idea.map((s, i) => (
                    <div key={s.idea} className={`app-spend-part s${(i % 6) + 1}`} style={{ width: `${(100 * s.su) / b.spend.total}%` }} />
                  ))}
                </div>
              )}
              <div className="app-legend">
                {b.spend.by_idea.map((s, i) => (
                  <span key={s.idea}>
                    <i className={`app-swatch s${(i % 6) + 1}`} />
                    {s.idea} <span className="strong">{su(s.su)}</span>
                  </span>
                ))}
              </div>
              {homeSeeds > 0 && <p className="small ab-muted">种子 {homeSeeds} 个在思考中（种子园）</p>}
            </section>
          </>
        )}

        <section aria-labelledby="m-h" className="app-section">
          <div className="app-head">
            <h2 className="heading" id="m-h">机器</h2>
            <button type="button" className="app-link small ab-accent" onClick={props.onMachines}>
              编辑机器配置（clusters.toml）
            </button>
            {b && b.project.kind === "project" && (
              <button type="button" className="app-link small ab-accent" onClick={() => props.onEditConfig(`${b.project.root}/arboria.toml`)}>
                用编辑器打开这个项目的 arboria.toml
              </button>
            )}
          </div>
          {b && b.probes.length === 0 && <p className="small ab-muted">还没有 probe 结果（小助手每 30 分钟查一次余额）。</p>}
          <div className="app-cards">{b?.probes.map((p) => <ProbeCard key={p.machine + p.probe} p={p} now={props.now} />)}</div>
          <div className="ab-panel app-helper">
            <div className="small ab-muted">后台小助手</div>
            <p className="small ab-muted app-pad0">
              一个在后台常驻的小程序（arb helper run，登录 Mac 时自动启动）：app 关着也每 2 分钟查一次作业状态，
              run 跑完、失败、等你签字、集群要重新登录时弹系统通知。它从不批准任何东西。
            </p>
            {props.helper ? (
              <>
                <div>
                  {props.helper.installed ? "已安装" : "没有安装"} · {props.helper.running ? "运行中" : "没在运行"}
                  {props.helper.last_tick && <span className="ab-muted"> · 上一轮 {ago(props.helper.last_tick, props.now)}</span>}
                </div>
                {Object.entries(props.helper.needs_login).map(([m, msg]) => (
                  <div key={m} className="small">
                    <span className="ab-warn">{m} 需要登录</span> <span className="ab-muted">{msg}</span>{" "}
                    <button type="button" className="ab-btn small" onClick={() => props.onLogin(m)}>
                      在终端里登录
                    </button>
                  </div>
                ))}
                {props.helper.errors.map((e) => (
                  <div key={e} className="small ab-bad">{e}</div>
                ))}
                {!props.helper.installed && (
                  <button type="button" className="ab-btn small app-gap" onClick={props.onInstallHelper}>
                    安装小助手（登录时自动运行，盯作业、发通知）
                  </button>
                )}
              </>
            ) : (
              <div className="small ab-muted">读取中…</div>
            )}
          </div>
        </section>

        {others.length > 0 && (
          <section className="app-section">
            <h2 className="heading">别的项目</h2>
            <div className="app-legend">
              {others.map((p) => (
                <button key={p.root} type="button" className="app-link" onClick={() => props.onProject(p.root)}>
                  {p.name} <span className="ab-muted small">{p.error ? "读不了" : `${p.open_ideas} 个进行中 · ${p.inbox.length} 件待处理`}</span>
                </button>
              ))}
            </div>
          </section>
        )}
      </div>
    </main>
  );
}
