// The approval card (ADR 0004): the whole card as `arb card` prints it, signable only
// after it has been on screen for 2 s, then Touch ID (the Rust side asks for it). Without
// Touch ID (not a Mac, or the app runs inside Claude Code) it shows the command to run in
// your own terminal instead.

import { useEffect, useState } from "react";
import { api, message } from "../api";
import type { Approved, CardView, Platform } from "../types";
import { Glyph } from "./Glyph";

type Phase =
  | { k: "loading" }
  | { k: "ready"; card: CardView; at: number }
  | { k: "signing"; card: CardView }
  | { k: "done"; card: CardView; result: Approved }
  | { k: "error"; card: CardView | null; error: string };

export function ApprovalSheet(props: {
  root: string;
  run: string;
  platform: Platform;
  now: () => number;
  onClose: () => void;
  onRunInTerminal: (command: string) => void;
}) {
  const [phase, setPhase] = useState<Phase>({ k: "loading" });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let live = true;
    api
      .cardShown(props.root, props.run)
      .then((card) => live && setPhase({ k: "ready", card, at: props.now() }))
      .catch((e) => live && setPhase({ k: "error", card: null, error: message(e) }));
    return () => {
      live = false;
    };
  }, [props.root, props.run]);
  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 200);
    return () => clearInterval(t);
  }, []);
  const card = phase.k === "loading" ? null : phase.card;
  const canDevice = props.platform.touch_id && !props.platform.in_claude_code;
  const left =
    phase.k === "ready" ? Math.max(0, props.platform.min_shown_ms - (props.now() - phase.at)) : 0;
  const usable = phase.k === "ready" && card?.matches === true && !!card.card_hash;
  const signable = usable && canDevice && left === 0;
  void tick;

  const sign = async () => {
    if (phase.k !== "ready" || !signable || !phase.card.card_hash) return;
    const c = phase.card;
    setPhase({ k: "signing", card: c });
    try {
      const result = await api.approve(props.root, props.run, c.card_hash!);
      setPhase({ k: "done", card: c, result });
    } catch (e) {
      setPhase({ k: "error", card: c, error: message(e) });
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void sign();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const command = `arb approve ${props.run}`;
  return (
    <div className="app-modal" role="presentation" onClick={props.onClose}>
      <section
        className="ab-slip ab-on-fill app-slip"
        role="dialog"
        aria-modal="true"
        aria-labelledby="slip-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="ab-slip-head">
          <span className="strong" id="slip-title">
            审核单 {props.run}
          </span>
          <span className="ab-spacer" />
          {phase.k === "done" ? (
            <span className="ab-pill small" data-tone="ok">
              <Glyph name="check" />
              已签字
            </span>
          ) : (
            <span className="ab-pill small" data-tone="warn">
              <Glyph name="touch" />
              待签字
            </span>
          )}
        </div>
        {phase.k === "loading" && <p className="small ab-muted">重新计算审核单…</p>}
        {card && (
          <>
            <dl className="ab-slip-kv">
              <dt>资源</dt>
              <dd>{card.resources.slice(0, 3).join(" · ")}</dd>
              <dt>花费</dt>
              <dd>{card.estimate.join(" · ")}</dd>
              <dt>预算</dt>
              <dd>{card.budget.join(" · ") || "没有设预算"}</dd>
              <dt>哈希</dt>
              <dd className="code">card {(card.card_hash ?? "").slice(0, 8)}</dd>
            </dl>
            <pre className="code app-card-text" aria-label="审核单全文">
              {card.text ?? card.error}
            </pre>
          </>
        )}
        {card && card.matches === false && (
          <p className="small ab-bad">审核单已变化（重新算出的哈希和提交时不同）：请重新 arb submit。</p>
        )}
        {phase.k === "done" && (
          <p className="ab-slip-signed small">
            <Glyph name="check" />
            已批准并提交：{phase.result.status}
            {phase.result.jobs.length ? ` · 作业 ${phase.result.jobs.join(", ")}` : ""}
          </p>
        )}
        {phase.k === "error" && <p className="small ab-bad">{phase.error}</p>}
        <div className="ab-slip-foot">
          {canDevice ? (
            <span className="ab-note small">
              {left > 0 ? `读完审核单再签字（${Math.ceil(left / 1000)} 秒）` : "签字用触控 ID"}
            </span>
          ) : (
            <span className="ab-note small">
              {props.platform.in_claude_code
                ? "app 是从 Claude Code 里启动的，不能在这里签字："
                : `这里不能用触控 ID（${props.platform.touch_id_why ?? "不是 Mac"}）：`}
              在你自己的终端运行 <code className="code">{command}</code>
            </span>
          )}
          <button className="ab-btn body" type="button" onClick={props.onClose}>
            {phase.k === "done" ? "关闭" : "稍后"}
            <span className="ab-hint small">esc</span>
          </button>
          {canDevice ? (
            phase.k !== "done" && (
              <button
                className="ab-btn ab-btn-primary body"
                type="button"
                disabled={!signable}
                onClick={() => void sign()}
              >
                <Glyph name="touch" />
                {phase.k === "signing" ? "等待触控 ID…" : "签字"}
                <span className="ab-hint small">⌘⏎</span>
              </button>
            )
          ) : (
            <button
              className="ab-btn ab-btn-primary body"
              type="button"
              disabled={!usable}
              onClick={() => props.onRunInTerminal(command)}
            >
              <Glyph name="terminal" />
              在终端里签字
            </button>
          )}
        </div>
      </section>
    </div>
  );
}
