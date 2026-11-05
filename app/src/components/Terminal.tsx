// The embedded terminal (ADR 0004): xterm.js over a pseudo-terminal on the Rust side
// (portable-pty). Tabs: the login shell, or Claude Code, in the project's main worktree.

import { useEffect, useRef, useState } from "react";
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

function Pane({ tab, active, onExit }: { tab: TabSpec; active: boolean; onExit: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
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
    const ro = new ResizeObserver(() => {
      fit.fit();
      if (id !== null) void api.ptyResize(id, term.cols, term.rows);
    });
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
  onSelect: (key: number) => void;
  onClose: (key: number) => void;
  onNew: (program: string | null) => void;
}) {
  return (
    <section className="app-term" aria-label="终端">
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
      </div>
      {props.tabs.map((t) => (
        <Pane key={t.key} tab={t} active={t.key === props.active} onExit={() => undefined} />
      ))}
      {props.tabs.length === 0 && <p className="small ab-muted app-pad">没有打开的终端。</p>}
    </section>
  );
}
