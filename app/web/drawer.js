// The agent drawer: draws an ACP session (through Swift, Acp.swift) with the user's own
// claude. Everything the agent sends goes in via textContent; the only markup is ours.
// The page relays the person's input; Swift decides what that may cause (no bypass mode,
// permission answers only for pending requests).
(function () {
  const $ = (id) => document.getElementById(id);
  const post = (m) => window.webkit.messageHandlers.drawer.postMessage(m);
  const log = $("log"), q = $("q");
  const S = { ready: false, busy: false, agent: null, thought: null, tools: new Map(), plan: null };
  const MODE_NAMES = { default: "每次问", acceptEdits: "自动改文件", plan: "只做计划", auto: "agent 判断" };
  const STATUS = { pending: "等待", in_progress: "进行中", completed: "完成", failed: "失败" };
  const TONE = { pending: "", in_progress: "run", completed: "ok", failed: "bad" };

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  const atBottom = () => log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  function add(node) {
    const stick = atBottom();
    log.querySelector(".empty")?.remove();
    log.append(node);
    if (stick) log.scrollTop = log.scrollHeight;
    return node;
  }
  function keepDown(fn) { const stick = atBottom(); fn(); if (stick) log.scrollTop = log.scrollHeight; }

  // Plain text with ``` fences as <pre> and `x` as <code>; built from text nodes only.
  function rich(box, text) {
    box.replaceChildren();
    const parts = text.split(/```[^\n]*\n?/);
    parts.forEach((part, i) => {
      if (i % 2 === 1) { box.append(el("pre", "", part.replace(/\n$/, ""))); return; }
      part.split(/(`[^`\n]+`)/).forEach((bit) => {
        if (/^`[^`\n]+`$/.test(bit)) box.append(el("code", "", bit.slice(1, -1)));
        else if (bit) box.append(document.createTextNode(bit));
      });
    });
  }

  function message(who, cls) {
    const m = el("div", "msg " + cls);
    m.append(el("span", "who small", who));
    const t = el("div", "text");
    t._raw = "";
    m.append(t);
    add(m);
    return t;
  }

  function chunkText(content) {
    if (!content) return "";
    if (content.type === "text") return content.text || "";
    if (content.type === "resource_link") return `[${content.name || content.uri}]`;
    return "";
  }

  function agentChunk(text) {
    S.thought = null;
    if (!S.agent) S.agent = message("agent", "agent");
    S.agent._raw += text;
    keepDown(() => rich(S.agent, S.agent._raw));
  }
  function thoughtChunk(text) {
    if (!S.thought) {
      const d = el("details", "thought small");
      d.append(el("summary", "", "思考"));
      S.thought = el("div", "text"); S.thought._raw = "";
      d.append(S.thought);
      add(d);
    }
    S.thought._raw += text;
    S.thought.textContent = S.thought._raw;
  }

  // paths inside the worktree are shown relative to it
  function short(p) { p = p || ""; return S.cwd && p.startsWith(S.cwd + "/") ? p.slice(S.cwd.length + 1) : p; }

  function diffLines(item) {
    const pre = el("pre");
    const old = (item.oldText || "").split("\n"), neu = (item.newText || "").split("\n");
    if (item.oldText) old.forEach((l) => pre.append(el("span", "del", "- " + l + "\n")));
    neu.forEach((l) => pre.append(el("span", "add", "+ " + l + "\n")));
    return pre;
  }

  function tool(u) {
    let t = S.tools.get(u.toolCallId);
    if (!t) {
      S.agent = null; S.thought = null;
      const box = el("div", "tool small");
      const line = el("div", "line");
      const status = el("span", "ab-status");
      const title = el("span", "title");
      const kind = el("span", "kind");
      line.append(status, title, kind);
      const body = el("div");
      box.append(line, body);
      t = { box, status, title, kind, body, data: {} };
      S.tools.set(u.toolCallId, t);
      add(box);
    }
    Object.assign(t.data, Object.fromEntries(Object.entries(u).filter(([, v]) => v != null)));
    const d = t.data;
    keepDown(() => {
      t.title.textContent = short(d.title || d.toolCallId || "工具");
      t.kind.textContent = d.kind || "";
      t.status.textContent = STATUS[d.status] || d.status || "";
      t.status.dataset.tone = TONE[d.status] || "";
      t.body.replaceChildren();
      for (const item of d.content || []) {
        if (item.type === "diff") {
          const det = el("details");
          const n = (item.newText || "").split("\n").length, o = item.oldText ? item.oldText.split("\n").length : 0;
          det.append(el("summary", "", `${short(item.path)}  +${n} −${o}`), diffLines(item));
          t.body.append(det);
        } else if (item.type === "content") {
          const text = chunkText(item.content).replace(/^```[^\n]*\n/, "").replace(/\n?```\s*$/, "");
          if (!text) continue;
          const det = el("details");
          det.append(el("summary", "", text.split("\n")[0].slice(0, 80) || "输出"), el("pre", "", text.slice(0, 20000)));
          t.body.append(det);
        }
      }
    });
  }

  function plan(entries) {
    if (!S.plan) { S.plan = add(el("div", "plan small")); }
    S.plan.replaceChildren(el("span", "ab-muted", "计划"));
    for (const e of entries || []) {
      S.plan.append(el("div", e.status === "completed" ? "done" : "", (e.status === "in_progress" ? "▸ " : "· ") + (e.content || "")));
    }
  }

  function permission(m) {
    S.agent = null; S.thought = null;
    const card = el("div", "ask-card small");
    const tc = m.toolCall || {};
    card.append(el("span", "ab-warn", "agent 想要："), el("span", "what body", tc.title || tc.kind || "使用一个工具"));
    const raw = tc.rawInput && (tc.rawInput.command || tc.rawInput.file_path || tc.rawInput.path);
    if (raw && String(raw) !== tc.title) card.append(el("code", "small", short(String(raw)).slice(0, 400)));
    const buttons = el("div", "buttons");
    const answer = (option, label) => {
      post({ kind: "permission", id: m.id, option });
      card.classList.add("is-answered");
      buttons.replaceChildren(el("span", "", label));
    };
    for (const o of m.options || []) {
      const allow = (o.kind || "").startsWith("allow");
      const b = el("button", "ab-btn body" + (o.kind === "allow_once" ? " ab-btn-primary" : ""), o.name || o.optionId);
      b.addEventListener("click", () => answer(o.optionId, (allow ? "已允许：" : "已拒绝：") + (o.name || o.optionId)));
      buttons.append(b);
    }
    card.append(buttons);
    card._cancel = () => answer(null, "已取消");
    add(card);
    S.pendingCards = (S.pendingCards || []).concat([card]);
  }

  function note(text, bad) { S.agent = null; S.thought = null; return add(el("div", "note small" + (bad ? " bad" : ""), text)); }

  function setBusy(b) {
    S.busy = b;
    $("send").hidden = b; $("stop").hidden = !b;
    q.disabled = !S.ready;
    $("send").disabled = !S.ready || b;
  }

  function usage(u) {
    if (u && u.used != null && u.size) {
      const k = (n) => (n >= 1000 ? Math.round(n / 1000) + "k" : String(n));
      $("status").textContent = `上下文 ${k(u.used)} / ${k(u.size)}`;
    }
  }

  function restartButton() {
    const b = el("button", "ab-btn body", "重新启动 agent");
    b.addEventListener("click", () => { b.remove(); post({ kind: "restart" }); });
    add(b);
  }

  window.drawer = {
    on(m) {
      switch (m.type) {
        case "idea": $("idea").textContent = m.idea; S.cwd = m.cwd || ""; break;
        case "status": $("status").textContent = m.text; break;
        case "session": {
          S.ready = true;
          const sel = $("mode");
          sel.replaceChildren();
          for (const mode of m.modes || []) {
            const o = el("option", "", MODE_NAMES[mode.id] || mode.name || mode.id);
            o.value = mode.id; sel.append(o);
          }
          sel.value = m.current; sel.disabled = !(m.modes || []).length;
          $("status").textContent = "";
          setBusy(false);
          q.focus();
          break;
        }
        case "update": {
          const u = m.update;
          const k = u.sessionUpdate;
          if (k === "agent_message_chunk") agentChunk(chunkText(u.content));
          else if (k === "agent_thought_chunk") thoughtChunk(chunkText(u.content));
          else if (k === "tool_call" || k === "tool_call_update") tool(u);
          else if (k === "plan") plan(u.entries);
          else if (k === "usage_update") usage(u);
          else if (k === "current_mode_update") $("mode").value = u.currentModeId;
          break;
        }
        case "permission": permission(m); break;
        case "done":
          setBusy(false);
          S.agent = null; S.thought = null; S.plan = null;
          if (m.stopReason === "cancelled") note("已停止");
          else if (m.stopReason === "max_tokens" || m.stopReason === "max_turn_requests") note("agent 到了上限，停下了");
          else if (m.stopReason === "refusal") note("agent 拒绝了这个请求", true);
          break;
        case "failed": setBusy(false); note(m.text, true); if (!S.ready) restartButton(); break;
        case "exited":
          S.ready = false; setBusy(false);
          (S.pendingCards || []).forEach((c) => c.classList.contains("is-answered") || c.classList.add("is-answered"));
          note("agent 已退出", true); restartButton();
          break;
      }
    },
    ask(text) { q.value = text; send(); },
    text() { return log.innerText; },
  };

  function send() {
    const text = q.value.trim();
    if (!text || !S.ready || S.busy) return;
    S.agent = null; S.thought = null; S.plan = null;
    rich(message("你", "you"), text);
    q.value = "";
    setBusy(true);
    post({ kind: "prompt", text });
  }
  q.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
  });
  $("send").addEventListener("click", send);
  $("stop").addEventListener("click", () => {
    (S.pendingCards || []).forEach((c) => c.classList.contains("is-answered") || c._cancel());
    post({ kind: "cancel" });
  });
  $("mode").addEventListener("change", (e) => post({ kind: "mode", id: e.target.value }));
  $("to-terminal").addEventListener("click", () => post({ kind: "tab", tab: "terminal" }));
  log.append(el("div", "empty", "agent 在这个 idea 的 worktree 里工作，用的是你自己的 claude。\n要提交 run，它只能写审核单，签字还是你。"));
  log.querySelector(".empty").style.whiteSpace = "pre-line";
  setBusy(false);
  post({ kind: "ready" });
})();
