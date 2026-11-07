// The chrome page: title bar, home (待你处理) and the ⌘P idea switcher. It renders what
// arbd returns (through Swift) and asks Swift to act. Ledger text only ever goes in via
// textContent. Wording follows the design system: "agent", never a vendor name.
(function () {
  const $ = (id) => document.getElementById(id);
  const post = (m) => window.webkit.messageHandlers.app.postMessage(m);
  const S = { project: null, inbox: null, current: null, panel: false, palette: false, sel: 0, hits: [],
              page: null, key: null };
  // arbd's read-only methods, through Swift (which allows only those)
  let rid = 0;
  const waiting = new Map();
  const call = (method, params = {}) => new Promise((ok, fail) => {
    rid += 1; waiting.set(rid, { ok, fail });
    post({ cmd: "call", rid, method, params });
  });

  const GLYPHS = {
    ok: '<path d="M3.5 8.5l3 3 6-7"/>',
    bad: '<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>',
    run: '<path class="solid" d="M5 3.2v9.6l7.6-4.8z"/>',
    wait: '<circle cx="8" cy="8" r="4.75"/>',
    none: '<path d="M4.5 8h7"/>',
    warn: '<path d="M3.6 12A5.6 5.6 0 0 1 2.9 9.2V8a5.1 5.1 0 0 1 10.2 0v1.6"/><path d="M6.1 13.6c-.6-1.4-.9-3.1-.9-5.1a2.8 2.8 0 0 1 5.6 0c0 1.9.3 3.4.9 4.7"/><path d="M8 8.4c0 2.1.3 3.8.9 5.2"/>',
  };
  function glyph(kind) {  // our own fixed markup, never ledger text
    const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    s.setAttribute("class", "ab-glyph"); s.setAttribute("viewBox", "0 0 16 16"); s.setAttribute("aria-hidden", "true");
    s.innerHTML = GLYPHS[kind] || GLYPHS.none;
    return s;
  }
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }
  function ago(ts) {
    if (!ts) return "";
    const s = (Date.now() - Date.parse(ts)) / 1000;
    if (!(s >= 0)) return "";
    if (s < 60) return "刚刚";
    if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
    if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
    return `${Math.floor(s / 86400)} 天前`;
  }

  // ---------------------------------------------------------------- title bar
  const PAGE = { run: (p) => p.id, runs: () => "run", compute: () => "算力" };
  function renderBar() {
    const p = S.project;
    $("crumb-project").textContent = p ? p.name : "Arboria";
    const idea = S.page && S.page.idea !== undefined ? S.page.idea : S.current;
    $("crumb-sep").hidden = !idea;
    $("crumb-idea").textContent = idea || "";
    $("crumb-sep2").hidden = !S.page;
    $("crumb-page").textContent = S.page ? PAGE[S.page.kind](S.page) : "";
    $("runs").hidden = !S.current;
    $("runs").classList.toggle("is-on", !!S.page && S.page.kind === "runs");
    $("compute").classList.toggle("is-on", !!S.page && S.page.kind === "compute");
    const n = S.inbox ? S.inbox.needs_you : 0;
    $("needs").hidden = !n || !S.current;
    $("needs-n").textContent = n;
    $("agent").classList.toggle("is-on", S.panel);
    $("agent").hidden = !S.current;
    document.documentElement.classList.toggle("is-bar", !!S.current);  // the notebook shows through
    $("home").hidden = !!S.current || !!S.page;
    $("page").hidden = !S.page;
  }

  // ---------------------------------------------------------------- pages (pages.js)
  const P = window.Pages({ el, glyph, ago, call, post, open: openPage });
  async function openPage(page) {
    if (page.kind === "run" && page.idea === undefined) page.idea = ideaOfRun(page.id);
    const was = !!S.page;
    S.page = page;
    if (!was) post({ cmd: "page", open: true });
    renderBar();
    const box = $("page-body");
    $("page").scrollTop = 0;
    box.replaceChildren(el("p", "body ab-muted", "…"));
    try {
      if (page.kind === "run") await P.run(box, page.id, S);
      else if (page.kind === "runs") await P.runs(box, page.idea);
      else await P.compute(box);
      if (page.kind === "run" && page.idea === null && S.page === page) { page.idea = undefined; renderBar(); }
    } catch (e) { box.replaceChildren(el("p", "body ab-bad", String(e))); }
  }
  function closePage(tell = true) {
    if (!S.page) return;
    S.page = null;
    if (tell) post({ cmd: "page", open: false });
    renderBar();
    if (!S.current) renderHome();
  }
  function ideaOfRun(id) {
    const it = ((S.inbox && S.inbox.items) || []).find((i) => i.id === id);
    return it && it.idea ? it.idea : S.current || null;
  }

  // ---------------------------------------------------------------- home
  const STATUS = { succeeded: "跑完了", failed: "失败", cancelled: "已取消" };
  function renderHome() {
    const box = $("inbox");
    box.replaceChildren();
    const items = (S.inbox && S.inbox.items) || [];
    if (S.inboxError) box.append(el("div", "empty small", S.inboxError));
    else if (!items.length) box.append(el("div", "empty", "没有要你处理的事"));
    for (const it of items) {
      const row = el("div", "ab-item");
      const tone = it.kind === "card" ? "warn" : it.tone === "ok" ? "ok" : it.tone === "bad" ? "bad" : "none";
      const mark = el("span", "mark " + ({ ok: "ab-ok", bad: "ab-bad", warn: "ab-warn" }[tone] || "ab-muted"));
      mark.append(glyph(it.kind === "card" ? "warn" : tone === "none" ? "wait" : tone));
      row.append(mark);
      const head = el("span", "ab-clip");
      head.append(el("span", "strong", it.id), el("span", "ab-muted", " · "));
      const what = it.kind === "card" ? "等你签字" : it.kind === "lesson" ? "经验待确认" : STATUS[it.status] || it.status;
      head.append(el("span", "", what + (it.idea ? ` · ${it.idea}` : "")));
      row.append(head, el("span", "when small", ago(it.when)));
      if (it.text) row.append(el("span", "meta ab-clip", it.text));
      if (it.command) {  // acting is the human's: in a terminal, or (a card) Touch ID on the run page
        const c = el("span", "cmd small", it.kind === "card" ? "点开用 Touch ID 签字，或在终端里：" : "在终端里：");
        c.append(el("code", "", it.command));
        row.append(c);
      }
      if (/^r\d+$/.test(it.id || "")) {
        row.classList.add("is-link");
        row.addEventListener("click", (e) => { if (!e.target.closest(".cmd")) openPage({ kind: "run", id: it.id, idea: it.idea }); });
      }
      box.append(row);
    }

    const ideas = $("ideas");
    ideas.replaceChildren();
    const list = (S.project && S.project.ideas) || [];
    const open = list.filter((i) => i.worktree && i.has_notebook);
    if (!open.length) ideas.append(el("div", "empty", "还没有进行中的 idea。在终端里：arb idea new NAME"));
    for (const i of open) ideas.append(ideaLine(i, () => post({ cmd: "select", idea: i.name })));

    const todos = (S.inbox && S.inbox.todos) || [];
    $("todos-section").hidden = !todos.length;
    const tbox = $("todos");
    tbox.replaceChildren();
    for (const t of todos) {
      const row = el("div", "ab-line");
      const m = el("span", "ab-muted"); m.append(glyph("wait"));
      row.append(m, el("span", "id", t.id), el("span", "sep", "·"), el("span", "meta", (t.idea ? t.idea + " · " : "") + t.text),
                 el("span", "when small", ago(t.when)));
      tbox.append(row);
    }
  }

  function ideaLine(i, onClick) {
    const row = el("div", "ab-line");
    row.setAttribute("role", "option");
    row.append(el("span", "name", i.name));
    const runs = i.run_count ? ` · ${i.run_count} 个 run` : "";
    row.append(el("span", "meta", (i.hypothesis || "") + runs));
    row.addEventListener("click", onClick);
    return row;
  }

  // ---------------------------------------------------------------- ⌘P
  function openPalette() {
    if (!S.project) return;
    S.palette = true; S.sel = 0;
    $("overlay").hidden = false;
    $("q").value = "";
    post({ cmd: "overlay", open: true });
    renderBar(); renderPalette();
    setTimeout(() => $("q").focus(), 0);
  }
  function closePalette() {
    if (!S.palette) return;
    S.palette = false;
    $("overlay").hidden = true;
    post({ cmd: "overlay", open: false });
    renderBar();
  }
  function renderPalette() {
    const q = $("q").value.trim().toLowerCase();
    const list = (S.project.ideas || []).filter((i) => i.worktree && i.has_notebook);
    S.hits = list.filter((i) => !q || i.name.toLowerCase().includes(q) || (i.hypothesis || "").toLowerCase().includes(q));
    S.sel = Math.max(0, Math.min(S.sel, S.hits.length - 1));
    const box = $("plist");
    box.replaceChildren();
    box.append(el("div", "ab-palette-group small", "idea"));
    S.hits.forEach((i, k) => {
      const row = ideaLine(i, () => choose(i.name));
      row.setAttribute("aria-selected", k === S.sel ? "true" : "false");
      if (i.name === S.current) row.append(el("span", "when small", "当前"));
      box.append(row);
    });
    const home = el("div", "ab-line");
    home.setAttribute("role", "option");
    home.append(el("span", "name", "待你处理"), el("span", "meta", "回到首页"), el("span", "when small", "⌘0"));
    home.addEventListener("click", () => { closePalette(); closePage(false); post({ cmd: "home" }); });
    const comp = el("div", "ab-line");
    comp.setAttribute("role", "option");
    comp.append(el("span", "name", "算力"), el("span", "meta", "作业和 SU 预算"));
    comp.addEventListener("click", () => { closePalette(); openPage({ kind: "compute", idea: S.current }); });
    box.append(el("div", "ab-palette-group small", "页面"), home, comp);
    if (S.current) {
      const rr = el("div", "ab-line");
      rr.setAttribute("role", "option");
      rr.append(el("span", "name", "run"), el("span", "meta", `${S.current} 的 run`));
      rr.addEventListener("click", () => { closePalette(); openPage({ kind: "runs", idea: S.current }); });
      box.append(rr);
    }
  }
  function choose(name) { closePalette(); closePage(); post({ cmd: "select", idea: name }); }

  $("q").addEventListener("input", () => { S.sel = 0; renderPalette(); });
  document.addEventListener("keydown", (e) => {
    if (!S.palette) {
      if (e.key === "Escape" && S.page) { e.preventDefault(); closePage(); }
      return;
    }
    if (e.key === "Escape") { e.preventDefault(); closePalette(); post({ cmd: "focusNotebook" }); }
    else if (e.key === "ArrowDown") { e.preventDefault(); S.sel = Math.min(S.sel + 1, S.hits.length - 1); renderPalette(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); S.sel = Math.max(S.sel - 1, 0); renderPalette(); }
    else if (e.key === "Enter") { e.preventDefault(); if (S.hits[S.sel]) choose(S.hits[S.sel].name); }
  });
  $("overlay").addEventListener("mousedown", (e) => { if (e.target === $("overlay")) closePalette(); });
  $("crumb").addEventListener("click", openPalette);
  $("needs").addEventListener("click", () => { closePage(false); post({ cmd: "home" }); });
  $("runs").addEventListener("click", () => (S.page && S.page.kind === "runs" ? closePage() : openPage({ kind: "runs", idea: S.current })));
  $("compute").addEventListener("click", () => (S.page && S.page.kind === "compute" ? closePage() : openPage({ kind: "compute", idea: S.current })));
  $("agent").addEventListener("click", () => post({ cmd: "panel" }));
  // dragging the window by the title bar's empty parts (the page sits under the title bar)
  $("bar").addEventListener("mousedown", (e) => { if (e.target.closest("[data-drag]") && !e.target.closest("button")) post({ cmd: "drag" }); });
  $("bar").addEventListener("dblclick", (e) => { if (!e.target.closest("button")) post({ cmd: "zoom" }); });

  window.arb = {
    render(state) {
      if ("project" in state) S.project = state.project;
      if ("inbox" in state) { S.inbox = state.inbox; S.inboxError = null; }
      if ("inboxError" in state) S.inboxError = state.inboxError;
      if ("current" in state) S.current = state.current;
      if ("panel" in state) S.panel = state.panel;
      if ("key" in state) S.key = state.key;
      if (state.error) { S.inboxError = state.error; }
      renderBar();
      if (!S.current) renderHome();
      if (S.palette) renderPalette();
    },
    palette() { S.palette ? closePalette() : openPalette(); },
    reply(id, obj) {
      const w = waiting.get(id); if (!w) return;
      waiting.delete(id);
      "error" in obj ? w.fail(obj.error) : w.ok(obj.result);
    },
    closePage() { closePage(false); },
    open(page) { openPage(page); },
    approved(run, obj) {  // Swift: the signature went through arbd (or did not)
      const note = document.querySelector(".ab-slip .ab-note"), btn = $("sign");
      if (obj.result) {
        toast(`已签字：${run} 已提交（${obj.result.status}）`);
        if (S.page && S.page.kind === "run" && S.page.id === run) openPage(S.page);
      } else {
        if (note) { note.textContent = obj.error; note.className = "ab-note small " + (obj.cancelled ? "ab-muted" : "ab-bad"); }
        if (btn) btn.disabled = false;
      }
    },
    active(on) { $("bar").classList.toggle("is-inactive", !on); },
  };
  function toast(text) {
    const t = el("div", "ab-toast body"); t.append(el("span", "txt", text));
    $("toasts").append(t);
    setTimeout(() => t.remove(), 6000);
  }
  renderBar();
  post({ cmd: "ready" });
})();
