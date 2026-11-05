// The window's title bar (ab-titlebar): where you are, what waits for you, the shortcuts.
// Pressing anywhere on it but a button drags the window (double-click: zoom); on macOS
// the traffic lights sit over its left padding.

import type { MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

function onBar(e: MouseEvent) {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button, input, a")) return;
  const w = getCurrentWindow();
  if (e.detail === 2) void w.toggleMaximize().catch(() => undefined);
  else void w.startDragging().catch(() => undefined);
}

export interface Crumb {
  label: string;
  onClick?: () => void;
}

export function TitleBar(props: {
  crumbs: Crumb[];
  waiting: number;
  terminalOpen: boolean;
  onInbox: () => void;
  onOverview: () => void;
  onSwitch: () => void;
  onCommands: () => void;
  onTerminal: () => void;
}) {
  const last = props.crumbs.length - 1;
  return (
    <div className="ab-titlebar app-titlebar" role="toolbar" aria-label="窗口" onMouseDown={onBar}>
      <span className="ab-crumb body">
        {props.crumbs.map((c, i) => (
          <span key={i} className={i === last ? "strong" : "up"}>
            {c.onClick && i !== last ? (
              <button type="button" className="app-link" onClick={c.onClick}>
                {c.label}
              </button>
            ) : (
              c.label
            )}
            {i !== last && <span className="up"> › </span>}
          </span>
        ))}
      </span>
      <span className="ab-spacer" />
      <button type="button" className="tool small app-tool" onClick={props.onInbox}>
        <span className="lbl">待你处理 </span>
        <span className="n">{props.waiting}</span>
      </button>
      <button type="button" className="tool small app-tool" onClick={props.onOverview}>
        <span className="lbl">全局</span>
      </button>
      <button type="button" className="tool small app-tool" onClick={props.onSwitch}>
        ⌘P<span className="lbl"> 换 idea</span>
      </button>
      <button type="button" className="tool small app-tool" onClick={props.onCommands}>
        ⌘K<span className="lbl"> 命令</span>
      </button>
      <button
        type="button"
        className={"tool small app-tool" + (props.terminalOpen ? " is-on" : "")}
        onClick={props.onTerminal}
      >
        ⌘`<span className="lbl"> 终端</span>
      </button>
    </div>
  );
}
