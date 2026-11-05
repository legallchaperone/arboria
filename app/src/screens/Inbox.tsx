// 收件箱 A: the home screen when no idea is open. One column, grouped by kind, newest
// first; ↑↓ to move, ⏎ to open.

import { useEffect, useMemo, useState } from "react";
import type { ProjectSummary } from "../types";
import { ago } from "../lib/format";
import { entries, group, type Entry } from "../lib/inbox";
import { Glyph } from "../components/Glyph";

export function Inbox(props: {
  projects: ProjectSummary[];
  now: number;
  onOpen: (e: Entry) => void;
  /** No path: the system's “choose a folder” dialog. */
  onAddProject: (path?: string) => void;
  recent: { root: string; idea: string }[];
  onIdea: (root: string, idea: string) => void;
}) {
  const all = useMemo(() => entries(props.projects), [props.projects]);
  const groups = useMemo(() => group(all), [all]);
  const flat = groups.flatMap((g) => g.items);
  const [sel, setSel] = useState(0);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement) return;
      if (e.key === "ArrowDown") setSel((n) => Math.min(flat.length - 1, n + 1));
      else if (e.key === "ArrowUp") setSel((n) => Math.max(0, n - 1));
      else if (e.key === "Enter" && !e.metaKey && flat[sel]) props.onOpen(flat[sel]);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const broken = props.projects.filter((p) => p.error);
  const [path, setPath] = useState("");
  return (
    <main className="app-main">
      <div className="app-column">
        <div className="app-head">
          <h1 className="title">待你处理</h1>
          <span className="small ab-muted">{flat.length ? `${flat.length} 件 · 最新的在上` : "没有要你处理的事"}</span>
        </div>
        <div className="ab-inbox app-inbox" role="listbox" aria-label="待你处理">
          {groups.map((g) => (
            <div key={g.kind} role="group" aria-label={g.label} className="app-group">
              <div className="small ab-muted app-group-label">{g.label}</div>
              {g.items.map((it) => {
                const i = flat.indexOf(it);
                return (
                  <div
                    key={it.key}
                    className="ab-item"
                    role="option"
                    aria-selected={i === sel}
                    onMouseEnter={() => setSel(i)}
                    onClick={() => props.onOpen(it)}
                  >
                    <span className="mark">
                      <Glyph name={g.glyph} tone={g.tone} />
                    </span>
                    <span className="strong ab-clip">{it.title}</span>
                    <span className="when small">{ago(it.when, props.now)}</span>
                    <span className="act small">⏎ 打开</span>
                    <span className="meta small">{it.meta}</span>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
        {broken.map((p) => (
          <p key={p.root} className="small ab-bad app-pad">
            {p.name}：{p.error}
          </p>
        ))}
        {props.recent.length > 0 && (
          <p className="small ab-muted app-pad">
            {flat.length ? "" : "没有别的事了。"}最近的 idea：
            {props.recent.map((r, k) => (
              <span key={r.root + r.idea}>
                {k > 0 && " · "}
                <button type="button" className="app-link ab-accent" onClick={() => props.onIdea(r.root, r.idea)}>
                  {r.idea}
                </button>
              </span>
            ))}
            （⌘P）
          </p>
        )}
        {props.projects.filter((p) => p.kind === "project").length === 0 && (
          <form
            className="app-add small"
            onSubmit={(e) => {
              e.preventDefault();
              props.onAddProject(path.trim() || undefined);
            }}
          >
            <span className="ab-muted">还没有项目。在仓库里运行 arb init（会自动添加），或者选一个 arb init 过的仓库：</span>
            <button className="ab-btn ab-btn-primary small" type="button" onClick={() => props.onAddProject()}>
              选择文件夹…
            </button>
            <span className="ab-muted">或粘贴路径</span>
            <input className="app-input code" placeholder="/Users/you/lm" value={path} onChange={(e) => setPath(e.target.value)} />
            <button className="ab-btn small" type="submit" disabled={!path.trim()}>
              添加
            </button>
          </form>
        )}
      </div>
    </main>
  );
}
