// The window's title bar (ab-titlebar): where you are, what waits for you, the shortcuts.
// It is the drag region; on macOS the traffic lights sit over its left padding.

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
    <div className="ab-titlebar app-titlebar" role="toolbar" aria-label="窗口" data-tauri-drag-region>
      <span className="ab-crumb body" data-tauri-drag-region>
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
      <span className="ab-spacer" data-tauri-drag-region />
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
