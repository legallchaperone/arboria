// 收件箱: what needs you across every project, grouped by kind, newest first. Selecting a
// row opens it in the inspector (↑↓ move, ⏎ opens the idea).

import { useEffect, useMemo } from "react";
import type { ProjectSummary } from "../types";
import { ago } from "../lib/format";
import { entries, group, type Entry } from "../lib/inbox";
import { Glyph } from "../components/Glyph";

export function InboxView(props: {
  projects: ProjectSummary[];
  now: number;
  selected: string | null;
  onSelect: (e: Entry) => void;
  onOpen: (e: Entry) => void;
  onAdd: () => void;
}) {
  const groups = useMemo(() => group(entries(props.projects)), [props.projects]);
  const flat = groups.flatMap((g) => g.items);
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      const at = flat.findIndex((x) => x.key === props.selected);
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        const n = flat[Math.min(flat.length - 1, at + 1)];
        if (n) props.onSelect(n);
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        const n = flat[Math.max(0, at - 1)];
        if (n) props.onSelect(n);
      } else if (e.key === "Enter" && !e.metaKey && at >= 0) props.onOpen(flat[at]);
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });
  const noProjects = props.projects.filter((p) => p.kind === "project").length === 0;
  if (noProjects)
    return (
      <div className="empty">
        <span className="big">还没有项目</span>
        <span>添加本机的仓库，或者从仓库链接克隆一个。在仓库里运行 arb init 也会自动加进来。</span>
        <button type="button" className="ab-btn ab-btn-primary" onClick={props.onAdd}>
          <Glyph name="plus" /> 添加项目
        </button>
      </div>
    );
  if (flat.length === 0)
    return (
      <div className="empty">
        <Glyph name="check" tone="ok" />
        <span className="big">没有要你处理的事</span>
        <span>run 等你签字、跑完或失败时会出现在这里。</span>
      </div>
    );
  return (
    <div className="page">
      <div className="page-head">
        <h1 className="title">收件箱</h1>
      </div>
      {groups.map((g) => (
        <div key={g.kind} role="group" aria-label={g.label}>
          <div className="group-label">
            {g.label} <span className="n">{g.items.length}</span>
          </div>
          <div className="rows" role="listbox">
            {g.items.map((it) => (
              <div
                key={it.key}
                role="option"
                aria-selected={it.key === props.selected}
                className={"row" + (it.key === props.selected ? " is-sel" : "")}
                onClick={() => props.onSelect(it)}
                onDoubleClick={() => props.onOpen(it)}
              >
                <Glyph name={g.glyph} tone={g.tone} />
                <span className="main">
                  <span className="t">{it.title}</span>
                  <span className="m">{it.meta}</span>
                </span>
                <span className="r">{ago(it.when, props.now)}</span>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
