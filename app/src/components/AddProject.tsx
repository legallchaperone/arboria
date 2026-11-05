// 添加项目: a repo already on this Mac (the folder dialog), or a repo link that the app
// clones, runs `arb init` in and lists.

import { useState } from "react";
import { api, message } from "../api";
import type { Cloned } from "../types";
import { Glyph } from "./Glyph";

export function AddProject(props: {
  onClose: () => void;
  onFolder: () => void;
  onCloned: (c: Cloned) => void;
}) {
  const [url, setUrl] = useState("");
  const [parent, setParent] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const clone = async () => {
    if (!url.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      props.onCloned(await api.cloneProject(url.trim(), parent));
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="app-modal" role="presentation" onClick={() => !busy && props.onClose()}>
      <section
        className="ab-sheet is-alone app-add-sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby="add-h"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && !busy && props.onClose()}
      >
        <h2 className="heading" id="add-h">添加项目</h2>
        <button type="button" className="app-choice" onClick={props.onFolder} disabled={busy}>
          <Glyph name="folder" />
          <span>
            <span className="strong">本机的仓库</span>
            <span className="small ab-muted">选一个文件夹；还没 arb init 的，顺便初始化</span>
          </span>
        </button>
        <form
          className="app-choice is-form"
          onSubmit={(e) => {
            e.preventDefault();
            void clone();
          }}
        >
          <Glyph name="link" />
          <span className="app-choice-body">
            <span className="strong">从仓库链接</span>
            <span className="small ab-muted">克隆下来、arb init，再加进来。用你自己的 git 凭据（ssh key、钥匙串）。</span>
            <input
              autoFocus
              className="app-input code app-field"
              placeholder="git@github.com:you/lm.git 或 https://github.com/you/lm"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              disabled={busy}
            />
            <span className="app-row small">
              <span className="ab-muted">放在</span>
              <span className="code">{parent ?? "~/code（没有就放在主目录）"}</span>
              <button type="button" className="app-link ab-accent" disabled={busy} onClick={() => api.pickFolder().then((p) => p && setParent(p), (e) => setError(message(e)))}>
                换一个…
              </button>
            </span>
            {error && <span className="small ab-bad app-pre">{error}</span>}
            <span className="app-row">
              <button type="submit" className="ab-btn ab-btn-primary body" disabled={!url.trim() || busy}>
                {busy ? "正在克隆…" : "克隆并初始化"}
              </button>
              <button type="button" className="ab-btn body" disabled={busy} onClick={props.onClose}>
                取消<span className="ab-hint small">esc</span>
              </button>
            </span>
          </span>
        </form>
      </section>
    </div>
  );
}
