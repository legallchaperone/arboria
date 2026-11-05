// The embedded terminal (ADR 0004): xterm.js over a pseudo-terminal on the Rust side
// (portable-pty). Tabs: the login shell, or Claude Code, in the project's main worktree.
// Hiding the panel keeps the tabs (and their processes) alive; it docks at the bottom,
// left or right, and its edge can be dragged.

import { useEffect, useRef, useState, type MouseEvent as RMouseEvent } from "react";
import { Terminal as Xterm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, message } from "../api";

export interface TabSpec {
  key: number;
  title: string;
  cwd: string;
  program: string | null;
  /** Typed into the shell once it is up (e.g. `arb approve r014`, not run: no newline). */
  type?: string;
}

function readVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export type Dock = "bottom" | "left" | "right";

function Pane({ tab, active, onExit }: { tab: TabSpec; active: boolean; onExit: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const refit = useRef<() => void>(() => undefined);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (active) requestAnimationFrame(() => refit.current());
  }, [active]);
  useEffect(() => {
    const term = new Xterm({
      fontFamily: readVar("--font-mono") || "monospace",
      fontSize: 13,
      lineHeight: 1.25,
      cursorBlink: true,
      theme: {
        background: readVar("--bg-subtle"),
        foreground: readVar("--ink"),
        cursor: readVar("--ink"),
        selectionBackground: readVar("--accent-fill"),
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(box.current!);
    fit.fit();
    let id: number | null = null;
    let closed = false;
    api
      .ptyOpen(
        tab.cwd,
        tab.program,
        term.cols,
        term.rows,
        (bytes) => term.write(bytes),
        () => {
          term.write("\r\n[进程已退出]\r\n");
          onExit();
        },
      )
      .then((n) => {
        if (closed) return void api.ptyClose(n);
        id = n;
        if (tab.type) setTimeout(() => id !== null && void api.ptyWrite(id, tab.type!), 400);
      })
      .catch((e) => setError(message(e)));
    const sub = term.onData((d) => id !== null && void api.ptyWrite(id, d));
    const resize = () => {
      if (!box.current || box.current.clientWidth === 0) return; // hidden: keep the size
      fit.fit();
      if (id !== null) void api.ptyResize(id, term.cols, term.rows);
    };
    refit.current = () => {
      resize();
      term.focus();
    };
    const ro = new ResizeObserver(resize);
    ro.observe(box.current!);
    return () => {
      closed = true;
      ro.disconnect();
      sub.dispose();
      if (id !== null) void api.ptyClose(id);
      term.dispose();
    };
  }, [tab.key]);
  return (
    <div className="app-term-pane" hidden={!active}>
      {error && <p className="small ab-bad">{error}</p>}
      <div ref={box} className="app-term-box" />
    </div>
  );
}

export function TerminalPanel(props: {
  tabs: TabSpec[];
  active: number | null;
  visible: boolean;
  dock: Dock;
  /** Height (bottom) or width (left / right) in px. */
  size: number;
  onSelect: (key: number) => void;
  onClose: (key: number) => void;
  onNew: (program: string | null) => void;
  onDock: (d: Dock) => void;
  onSize: (px: number) => void;
  onHide: () => void;
}) {
  const startDrag = (e: RMouseEvent) => {
    e.preventDefault();
    const start = props.dock === "bottom" ? e.clientY : e.clientX;
    const from = props.size;
    const move = (m: MouseEvent) => {
      const at = props.dock === "bottom" ? m.clientY : m.clientX;
      const delta = props.dock === "right" || props.dock === "bottom" ? start - at : at - start;
      const max = (props.dock === "bottom" ? window.innerHeight : window.innerWidth) - 160;
      props.onSize(Math.max(140, Math.min(max, from + delta)));
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      document.body.classList.remove("is-resizing");
    };
    document.body.classList.add("is-resizing");
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  const style = props.dock === "bottom" ? { height: props.size } : { width: props.size };
  return (
    <section className={`app-term is-${props.dock}`} aria-label="终端" hidden={!props.visible} style={style}>
      <div
        className={`app-term-grip is-${props.dock}`}
        role="separator"
        aria-orientation={props.dock === "bottom" ? "horizontal" : "vertical"}
        aria-label="拖动调整终端大小"
        onMouseDown={startDrag}
      />
      <div className="app-term-tabs small">
        {props.tabs.map((t) => (
          <span key={t.key} className={"app-term-tab" + (t.key === props.active ? " is-on" : "")}>
            <button type="button" className="app-link" onClick={() => props.onSelect(t.key)}>
              {t.title}
            </button>
            <button type="button" className="app-link ab-muted" aria-label={`关闭 ${t.title}`} onClick={() => props.onClose(t.key)}>
              ×
            </button>
          </span>
        ))}
        <span className="ab-spacer" />
        <button type="button" className="ab-btn small" onClick={() => props.onNew(null)}>
          新终端
        </button>
        <button type="button" className="ab-btn small" onClick={() => props.onNew("claude")}>
          Claude Code
        </button>
        <span className="app-seg" role="group" aria-label="终端位置">
          {(["left", "bottom", "right"] as Dock[]).map((d) => (
            <button
              key={d}
              type="button"
              className={"app-link small" + (props.dock === d ? " is-on" : "")}
              aria-pressed={props.dock === d}
              onClick={() => props.onDock(d)}
            >
              {d === "left" ? "左" : d === "bottom" ? "下" : "右"}
            </button>
          ))}
        </span>
        <button type="button" className="app-link ab-muted" aria-label="收起终端（⌘`）" onClick={props.onHide}>
          ⌄
        </button>
      </div>
      {props.tabs.map((t) => (
        <Pane key={t.key} tab={t} active={props.visible && t.key === props.active} onExit={() => undefined} />
      ))}
      {props.tabs.length === 0 && <p className="small ab-muted app-pad">没有打开的终端。</p>}
    </section>
  );
}
