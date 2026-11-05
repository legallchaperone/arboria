// 机器配置: clusters.toml in the app. The text is checked as `arb` reads it on every
// change (each [machine] table: ✓ or why arb refuses it); ⌘S saves only a valid file
// (the old one becomes clusters.toml.bak). Or open it in your own editor.

import { useEffect, useRef, useState } from "react";
import { api, message, onClustersChange } from "../api";
import type { ClustersCheck } from "../types";
import { Glyph } from "../components/Glyph";

export function Machines(props: { onToast: (m: string) => void; onTerminal: (type: string) => void }) {
  const [saved, setSaved] = useState<ClustersCheck | null>(null);
  const [text, setText] = useState("");
  const [check, setCheck] = useState<ClustersCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const dirty = saved !== null && text !== saved.text;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  const load = () =>
    api.clustersRead().then(
      (c) => {
        setSaved(c);
        setText(c.text);
        setCheck(c);
      },
      (e) => props.onToast(message(e)),
    );
  useEffect(() => {
    void load();
    const un = onClustersChange(() => {
      if (!dirtyRef.current) void load(); // changed elsewhere: follow it unless you are editing
    });
    return () => void un.then((f) => f());
  }, []);
  useEffect(() => {
    if (saved === null) return;
    const t = setTimeout(() => api.clustersCheck(text).then(setCheck, () => undefined), 250);
    return () => clearTimeout(t);
  }, [text]);

  const ok = !!check && !check.syntax_error && check.profiles.every((p) => !p.error);
  const save = async () => {
    if (!dirty && saved?.exists) return;
    setBusy(true);
    try {
      const c = await api.clustersSave(text);
      setSaved(c);
      setCheck(c);
      props.onToast(`已保存 ${c.path}`);
    } catch (e) {
      props.onToast(message(e));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="form">
        <div className="page-head">
          <div className="strong">clusters.toml 原文</div>
          <span className="small muted">
            <span className="mono">{saved?.path ?? "…"}</span> · 一般用「机器」里的表单就够了；这里给要直接改文件的时候。
            {saved && !saved.exists && " 现在还没有这个文件：下面是示例，<尖括号> 里的值要换成你的，保存后才生效。"}
          </span>
        </div>
        <div className="app-editor">
          <textarea
            className="code app-textarea"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label="clusters.toml"
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
          <aside className="app-editor-side">
            <div className="small ab-muted">检查（和 arb 读它时一样）</div>
            {check?.syntax_error && (
              <div className="small ab-bad">
                <Glyph name="cross" tone="bad" /> 不是合法的 TOML：{check.syntax_error}
              </div>
            )}
            {check?.profiles.map((p) => (
              <div key={p.name} className="app-check">
                <div>
                  <Glyph name={p.error ? "cross" : "check"} tone={p.error ? "bad" : "ok"} />{" "}
                  <span className="strong">{p.name}</span>
                  <span className="small ab-muted"> {p.kind ?? ""}{p.host ? ` · ${p.host}` : ""}</span>
                </div>
                {p.error && <div className="small ab-bad app-pre">{p.error.replace(/^clusters\.toml: \[[^\]]*\]:? ?/, "")}</div>}
              </div>
            ))}
            {check && !check.syntax_error && check.profiles.length === 0 && <div className="small ab-muted">还没有机器。</div>}
            <div className="app-row app-gap">
              <button type="button" className="ab-btn ab-btn-primary body" disabled={!ok || busy || (!dirty && !!saved?.exists)} onClick={() => void save()}>
                保存<span className="ab-hint small">⌘S</span>
              </button>
              <button type="button" className="ab-btn body" disabled={!dirty} onClick={() => saved && setText(saved.text)}>
                撤销修改
              </button>
            </div>
            <div className="app-row">
              <button type="button" className="ab-btn small" onClick={() => saved && api.openInEditor(saved.path).catch((e) => props.onToast(message(e)))}>
                用编辑器打开
              </button>
              <button type="button" className="ab-btn small" onClick={() => props.onTerminal("arb machine init ")}>
                在终端里 arb machine init…
              </button>
            </div>
            <p className="small ab-muted app-pad0">
              新集群最省事的办法：在终端里运行 <code className="code">arb machine init 名字 --host 登录节点</code>，它登录上去测好分区、计费和路径，加 <code className="code">--write</code> 直接写进这个文件。
            </p>
          </aside>
        </div>
    </div>
  );
}
