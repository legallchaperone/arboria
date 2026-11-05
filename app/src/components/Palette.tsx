// ⌘K (commands) and ⌘P (ideas): one filtered list, ↑↓ to move, ⏎ to run, esc to close.

import { useEffect, useMemo, useState } from "react";

export interface Choice {
  key: string;
  name: string;
  desc?: string;
  kind?: string;
  run: () => void;
}

export function Palette(props: { placeholder: string; choices: Choice[]; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const shown = useMemo(() => {
    const s = q.trim().toLowerCase();
    return s
      ? props.choices.filter((c) => (c.name + " " + (c.desc ?? "")).toLowerCase().includes(s))
      : props.choices;
  }, [q, props.choices]);
  useEffect(() => setSel(0), [q]);
  const pick = (c: Choice | undefined) => {
    if (!c) return;
    props.onClose();
    c.run();
  };
  return (
    <div className="app-modal app-modal-top" role="presentation" onClick={props.onClose}>
      <div className="ab-palette" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
        <div className="ab-palette-input body">
          <input
            autoFocus
            className="app-input body"
            placeholder={props.placeholder}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setSel((n) => Math.min(shown.length - 1, n + 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setSel((n) => Math.max(0, n - 1));
              } else if (e.key === "Enter") {
                pick(shown[sel]);
              } else if (e.key === "Escape") {
                props.onClose();
              }
            }}
          />
        </div>
        <div className="ab-palette-list" role="listbox">
          {shown.map((c, i) => (
            <div
              key={c.key}
              role="option"
              aria-selected={i === sel}
              className="ab-line body"
              onMouseEnter={() => setSel(i)}
              onClick={() => pick(c)}
            >
              <span className="name">{c.name}</span>
              {c.desc && <span className="desc small">{c.desc}</span>}
              {c.kind && <span className="kind small">{c.kind}</span>}
            </div>
          ))}
          {shown.length === 0 && <div className="ab-palette-group small">没有匹配的</div>}
        </div>
        <div className="ab-palette-foot small">
          <span>↑↓ 选择</span>
          <span>⏎ 打开</span>
          <span>esc 关闭</span>
        </div>
      </div>
    </div>
  );
}
