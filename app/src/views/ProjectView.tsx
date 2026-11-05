// A project: its ideas (a table), the timeline of ideas and runs, spend, machines.

import type { Board } from "../types";
import { ago, su } from "../lib/format";
import { Glyph } from "../components/Glyph";
import { Timeline } from "./Timeline";
import { MenuButton } from "../components/Menu";
import { newIdea, projectMenu, useActions } from "./actions";

export function ProjectView(props: {
  board: Board | null;
  error: string | null;
  now: number;
  onIdea: (idea: string) => void;
  onSettings: () => void;
}) {
  const b = props.board;
  const a = useActions();
  if (!b) return <div className="empty">{props.error ?? "读取中…"}</div>;
  const ideas = b.ideas.filter((i) => i.stage !== "seed");
  const open = ideas.filter((i) => i.stage === "open");
  const closed = ideas.filter((i) => i.stage !== "open");
  const waiting = b.inbox.filter((i) => i.kind === "approve").length;
  return (
    <div className="page is-wide">
      <div className="page-head">
        <div className="app-row">
          <h1 className="title">{b.project.name}</h1>
          <span className="head-actions">
            <button type="button" className="ab-btn ab-btn-primary body" onClick={() => a.ask(newIdea(a, b))}>
              <Glyph name="plus" /> 新建 idea
            </button>
            <MenuButton label={<Glyph name="more" />} ariaLabel="项目的操作" items={projectMenu(a, { root: b.project.root, name: b.project.name }, b, { newIdea: false })} />
          </span>
        </div>
        <div className="chips">
          <span className="chip">
            <b>{open.length}</b> 个进行中
          </span>
          <span className="chip">
            <b>{b.runs.length}</b> 个 run
          </span>
          <span className="chip">
            已用 <b>{su(b.spend.used)}</b>
            {b.spend.total > 0 ? ` / ${su(b.spend.total)}` : ""} SU
          </span>
          {waiting > 0 && (
            <span className="chip">
              <Glyph name="touch" tone="warn" /> <b>{waiting}</b> 等你签字
            </span>
          )}
        </div>
        {b.project.config_error && <div className="err-box">{b.project.config_error}</div>}
      </div>

      <section className="section">
        <div className="section-head">
          <h2 className="heading">idea</h2>
        </div>
        {ideas.length === 0 ? (
          <p className="small muted">
            还没有 idea。点「新建 idea」，或者在 Claude Code 里让它用 arb idea new 建一个。
          </p>
        ) : (
          <div className="rows">
            {[...open, ...closed].map((i) => {
              const runs = b.runs.filter((r) => r.idea === i.name);
              const wait = runs.some((r) => r.status === "awaiting_approval");
              const running = runs.some((r) => ["running", "queued", "approved"].includes(r.status));
              return (
                <div key={i.name} className="row" role="button" tabIndex={0} onClick={() => props.onIdea(i.name)} onKeyDown={(e) => e.key === "Enter" && props.onIdea(i.name)}>
                  <span className={"dot " + (i.stage !== "open" ? "" : wait ? "warn" : running ? "run" : "ok")} />
                  <span className="main">
                    <span className={"t" + (i.stage !== "open" ? " muted" : "")}>{i.name}</span>
                    <span className="m">{i.hypothesis ?? ""}</span>
                  </span>
                  <span className="r">
                    {i.stage === "merged" ? "已收" : i.stage === "dropped" ? "已放弃" : `${runs.length} 个 run`}
                    <span className="mono">{su(i.su_used)} SU</span>
                    <span>{ago(i.last_active, props.now)}</span>
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </section>

      {ideas.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h2 className="heading">时间线</h2>
          </div>
          <Timeline board={b} onIdea={props.onIdea} />
        </section>
      )}

      {b.spend.total > 0 && (
        <section className="section">
          <div className="section-head">
            <h2 className="heading">花费</h2>
            <span className="small muted">
              {su(b.spend.used)} / {su(b.spend.total)} SU · 剩 {su(b.spend.total - b.spend.used)}
            </span>
          </div>
          <div className="app-spend" role="img" aria-label={`预算 ${su(b.spend.total)} SU 中已用 ${su(b.spend.used)}`}>
            {b.spend.by_idea.map((s, i) => (
              <div key={s.idea} className={`app-spend-part s${(i % 6) + 1}`} style={{ width: `${(100 * s.su) / b.spend.total}%` }} />
            ))}
          </div>
          <div className="app-legend small">
            {b.spend.by_idea.map((s, i) => (
              <span key={s.idea}>
                <i className={`app-swatch s${(i % 6) + 1}`} />
                {s.idea} <span className="mono">{su(s.su)}</span>
              </span>
            ))}
          </div>
        </section>
      )}

      {b.probes.length > 0 && (
        <section className="section">
          <div className="section-head">
            <h2 className="heading">机器</h2>
          </div>
          <div className="rows">
            {b.probes.map((p) => (
              <div key={p.machine + p.probe} className="row">
                <Glyph name="server" />
                <span className="main">
                  <span className="t">
                    {p.machine} · {p.probe.replace(/^(builtin|project|global):/, "")}
                  </span>
                  <span className="m">
                    {p.result.kind === "number"
                      ? `${p.result.label} ${p.result.value}${p.result.of != null ? ` / ${p.result.of}` : ""}${p.result.unit ? ` ${p.result.unit}` : ""}`
                      : p.result.kind === "text"
                        ? p.result.text
                        : p.result.rows.map((r) => r.join(" ")).join(" · ")}
                  </span>
                </span>
                <span className="r">{ago(p.at, props.now)}</span>
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
