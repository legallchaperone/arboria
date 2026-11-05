// The left sidebar: 收件箱, each project with its open ideas, 种子园; 设置 and 添加项目 at
// the bottom. The helper is not shown unless a machine needs a login.

import type { Board, HelperStatus, ProjectSummary } from "../types";
import { Glyph } from "../components/Glyph";
import type { Route } from "./route";

function ideaDot(b: Board, idea: string): string {
  const runs = b.runs.filter((r) => r.idea === idea);
  if (runs.some((r) => r.status === "awaiting_approval")) return "warn";
  if (runs.some((r) => ["running", "queued", "approved"].includes(r.status))) return "run";
  return "";
}

export function Sidebar(props: {
  route: Route;
  projects: ProjectSummary[];
  boards: Record<string, Board>;
  waiting: number;
  helper: HelperStatus | null;
  go: (r: Route) => void;
  onAdd: () => void;
  onLogin: (machine: string) => void;
}) {
  const r = props.route;
  const projects = props.projects.filter((p) => p.kind === "project");
  const home = props.projects.find((p) => p.kind === "home");
  const login = Object.entries(props.helper?.needs_login ?? {});
  return (
    <nav className="side" aria-label="导航">
      <div className="side-top" data-drag />
      <div className="side-scroll">
        <button type="button" className={"nav" + (r.k === "inbox" ? " is-on" : "")} onClick={() => props.go({ k: "inbox" })}>
          <Glyph name="bell" />
          <span className="lbl">收件箱</span>
          {props.waiting > 0 && <span className="count is-hot">{props.waiting}</span>}
        </button>
        <div className="side-group">
          <span>项目</span>
          <button type="button" className="icon-btn" style={{ width: 20, height: 20 }} aria-label="添加项目" title="添加项目" onClick={props.onAdd}>
            <Glyph name="plus" />
          </button>
        </div>
        {projects.length === 0 && (
          <button type="button" className="nav muted" onClick={props.onAdd}>
            <span className="lbl">添加第一个项目…</span>
          </button>
        )}
        {projects.map((p) => {
          const b = props.boards[p.root];
          const open = b?.ideas.filter((i) => i.stage === "open") ?? [];
          const on = (r.k === "project" || r.k === "idea") && r.root === p.root;
          return (
            <div key={p.root}>
              <button
                type="button"
                className={"nav" + (r.k === "project" && r.root === p.root ? " is-on" : "")}
                onClick={() => props.go({ k: "project", root: p.root })}
                title={p.root}
              >
                <Glyph name="folder" />
                <span className="lbl">{p.name}</span>
                {p.error ? <span className="dot bad" title={p.error} /> : p.inbox.length > 0 && <span className="count">{p.inbox.length}</span>}
              </button>
              {(on || open.length <= 6) &&
                open.map((i) => (
                  <button
                    key={i.name}
                    type="button"
                    className={"nav is-child" + (r.k === "idea" && r.root === p.root && r.idea === i.name ? " is-on" : "")}
                    onClick={() => props.go({ k: "idea", root: p.root, idea: i.name })}
                  >
                    <span className="lbl">{i.name}</span>
                    {b && ideaDot(b, i.name) && <span className={"dot " + ideaDot(b, i.name)} />}
                  </button>
                ))}
            </div>
          );
        })}
        {home && (
          <>
            <div className="side-group">
              <span>想法</span>
            </div>
            <button type="button" className={"nav" + (r.k === "seeds" ? " is-on" : "")} onClick={() => props.go({ k: "seeds" })}>
              <Glyph name="sparkle" />
              <span className="lbl">种子园</span>
              {home.seeds > 0 && <span className="count">{home.seeds}</span>}
            </button>
          </>
        )}
      </div>
      <div className="side-foot">
        {login.map(([m]) => (
          <button key={m} type="button" className="side-alert" onClick={() => props.onLogin(m)} title="在终端里登录">
            <Glyph name="server" />
            <span>{m} 需要重新登录 · 点这里在终端里登录</span>
          </button>
        ))}
        <button type="button" className={"nav" + (r.k === "settings" ? " is-on" : "")} onClick={() => props.go({ k: "settings", tab: "general" })}>
          <Glyph name="panel" />
          <span className="lbl">设置</span>
          <span className="count faint">⌘,</span>
        </button>
      </div>
    </nav>
  );
}
