// The experiment pages (P3): one run (curves against its base, what changed, jobs, cost,
// and, while it waits, the card to sign), an idea's runs, and compute (jobs per machine over
// time, the SU budget). Data comes from arbd's read-only methods through Swift; ledger text
// only ever goes in via textContent. Signing is Swift's: the page only asks for it.
window.Pages = function (U) {
  const { el, glyph, ago, call, post } = U;
  const SVG = "http://www.w3.org/2000/svg";
  const svg = (tag, attrs, text) => {
    const e = document.createElementNS(SVG, tag);
    for (const [k, v] of Object.entries(attrs || {})) e.setAttribute(k, v);
    if (text != null) e.textContent = text;
    return e;
  };
  const TONE = { succeeded: "ok", completed: "ok", failed: "bad", cancelled: "bad", timeout: "bad",
                 rejected: "bad", queued: "run", running: "run", pending: "run", approved: "run",
                 submitted: "run", awaiting_approval: "warn" };
  const WORD = { succeeded: "跑完了", failed: "失败", cancelled: "已取消", queued: "排队中",
                 running: "在跑", awaiting_approval: "等你签字", approved: "已批准", rejected: "已拒绝",
                 draft: "草稿", completed: "完成", pending: "排队中", timeout: "超时" };
  const GL = { ok: "ok", bad: "bad", run: "run", warn: "warn" };
  function status(s) {
    const tone = TONE[s] || "none";
    const e = el("span", "ab-status body"); e.dataset.tone = tone;
    e.append(glyph(GL[tone] || "wait"), el("span", "", WORD[s] || s || "—"));
    return e;
  }
  const num = (v, d = 2) => {
    if (v == null || v === "") return "—";
    const s = (+v).toFixed(d);
    return s.includes(".") ? s.replace(/0+$/, "").replace(/\.$/, "") : s;
  };
  const fmt = (v) => {
    if (!isFinite(v)) return "";
    const a = Math.abs(v);
    return a !== 0 && (a < 1e-3 || a >= 1e5) ? v.toExponential(1) : (+v.toPrecision(3)).toString();
  };
  const dur = (s) => (s == null ? "—" : s < 3600 ? `${Math.round(s / 60)} 分钟` : `${(s / 3600).toFixed(1)} 小时`);
  function section(title, cls = "heading") {
    const s = el("section");
    if (title) s.append(el("h2", cls, title));
    return s;
  }
  function table(head, rows, numCols = []) {
    const t = el("table", "ab-table body");
    const hr = el("tr");
    head.forEach((h, i) => { const th = el("th", numCols.includes(i) ? "num" : "", h); hr.append(th); });
    t.append(hr);
    for (const r of rows) {
      const tr = el("tr");
      r.forEach((c, i) => tr.append(c instanceof Node ? (() => { const td = el("td"); td.append(c); return td; })()
                                                         : el("td", numCols.includes(i) ? "num" : "", c)));
      t.append(tr);
    }
    return t;
  }
  function link(text, onClick) {
    const a = el("button", "ab-link body", text);
    a.addEventListener("click", onClick);
    return a;
  }

  // ------------------------------------------------------------ line chart
  // series: [{name, pts: [[x, y]], k: 1..6, step?}], our own SVG with the design system's classes
  function chart(series, { height = 180, xfmt = fmt } = {}) {
    const W = 640, H = height, L = 56, R = 12, T = 8, B = 22;
    const all = series.flatMap((s) => s.pts);
    const box = el("div", "ab-chart");
    if (!all.length) return box;
    let [x0, x1] = [Math.min(...all.map((p) => p[0])), Math.max(...all.map((p) => p[0]))];
    let [y0, y1] = [Math.min(...all.map((p) => p[1])), Math.max(...all.map((p) => p[1]))];
    if (x0 === x1) x1 = x0 + 1;
    if (y0 === y1) { y0 -= 0.5; y1 += 0.5; }
    const X = (x) => L + ((x - x0) / (x1 - x0)) * (W - L - R);
    const Y = (y) => T + (1 - (y - y0) / (y1 - y0)) * (H - T - B);
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
    for (const f of [0, 0.5, 1]) {
      const y = y0 + f * (y1 - y0);
      s.append(svg("line", { class: f === 0 ? "base" : "grid", x1: L, x2: W - R, y1: Y(y), y2: Y(y) }));
      s.append(svg("text", { class: "tick small", x: L - 8, y: Y(y) + 4, "text-anchor": "end" }, fmt(y)));
    }
    s.append(svg("text", { class: "tick small", x: L, y: H - 4 }, xfmt(x0)));
    s.append(svg("text", { class: "tick small", x: W - R, y: H - 4, "text-anchor": "end" }, xfmt(x1)));
    for (const ser of [...series].reverse()) {  // the first series on top
      if (!ser.pts.length) continue;
      let d = "";
      ser.pts.forEach((p, i) => {
        if (i && ser.step) d += `H${X(p[0]).toFixed(1)}`;
        d += `${i ? "L" : "M"}${X(p[0]).toFixed(1)},${Y(p[1]).toFixed(1)}`;
      });
      s.append(svg("path", { class: `s s${ser.k}`, d }));
      const last = ser.pts[ser.pts.length - 1];
      s.append(svg("circle", { class: `d${ser.k} ring`, cx: X(last[0]), cy: Y(last[1]), r: 3.5 }));
    }
    box.append(s);
    return box;
  }
  function legend(series) {
    const g = el("div", "ab-legend small");
    for (const s of series) {
      const sp = el("span"); const i = el("i", `k${s.k}`);
      sp.append(i, el("span", "", s.name + (s.pts.length ? ` ${fmt(s.pts[s.pts.length - 1][1])}` : "")));
      g.append(sp);
    }
    return g;
  }

  // ------------------------------------------------------------ one run
  async function run(box, id, S) {
    const p = await call("run", { run: id });
    const r = p.run;
    box.replaceChildren();

    const head = section(null);
    const h = el("div", "ab-row page-head");
    h.append(el("h1", "title", r.id), status(r.status));
    head.append(h);
    const meta = el("p", "meta body ab-muted");
    meta.append(el("span", "", r.idea));
    if (p.base) { meta.append(el("span", "", " · 基于 ")); meta.append(link(p.base, () => U.open({ kind: "run", id: p.base }))); }
    const where = [r.machine, r.partition].filter(Boolean).join(" / ");
    if (where) meta.append(el("span", "", ` · ${where}`));
    if (r.gpus) meta.append(el("span", "", ` · ${r.gpus} GPU × ${r.walltime || "—"}`));
    meta.append(el("span", "", ` · ${ago(r.created_at)}`));
    head.append(meta);
    if (r.change_reason) head.append(el("p", "body", r.change_reason));
    if (r.summary) head.append(el("p", "body ab-muted", r.summary));
    box.append(head);

    if (r.status === "awaiting_approval") box.append(await cardSection(r, S));

    // curves: the idea's metric first, then what else the run logged (a few)
    const keys = Object.keys(p.metrics || {});
    const order = [r.idea_metric, ...keys.filter((k) => k !== r.idea_metric)].filter((k) => k && keys.includes(k)).slice(0, 4);
    const cur = section("曲线");
    if (!order.length) {
      cur.append(el("p", "body ab-muted", r.status === "succeeded" || r.status === "failed"
        ? "这个 run 没有 metrics.jsonl（训练代码每步写一行 JSON 到 $ARBORIA_OUT/metrics.jsonl）"
        : "跑完、arb poll 拉回 out/ 之后，这里画 metrics.jsonl 的曲线"));
    }
    for (const k of order) {
      const series = [{ name: r.id, pts: p.metrics[k] || [], k: 1 }];
      if (p.base && (p.base_metrics || {})[k]) series.push({ name: p.base, pts: p.base_metrics[k], k: 2 });
      const fig = el("figure", "fig");
      const cap = el("figcaption", "ab-row");
      cap.append(el("span", "strong body", k), el("span", "ab-spacer"), legend(series));
      fig.append(cap, chart(series, { xfmt: (x) => `step ${fmt(x)}` }));
      cur.append(fig);
    }
    box.append(cur);

    if (p.diff) {
      const d = section(`和 ${p.base} 比`);
      if (!p.diff.rows.length) d.append(el("p", "body ab-muted", "配置和代码都一样"));
      else {
        let last = null;
        d.append(table(["", "", p.base, r.id], p.diff.rows.map((row) => {
          const g = row[0] === last ? "" : row[0]; last = row[0];
          return [g, row[1], row[2], row[3]];
        })));
      }
      if (p.diff.same_tree) d.append(el("p", "small ab-muted", "快照不同，但代码树一样"));
      if (p.diff.stat) d.append(el("pre", "code stat", p.diff.stat));
      box.append(d);
    }

    const j = section("作业与花费");
    if (p.jobs.length) {
      j.append(table(["段", "作业", "状态", "用时", "SU"], p.jobs.map((x) => [
        `s${x.segment}`, x.job_id || "—", status(x.state), dur(x.elapsed_s), num(x.billed_su)]), [3, 4]));
    }
    j.append(el("p", "body ab-muted", `预估 ${num(r.est_cost)} SU · 实际 ${num(r.actual_cost)} SU`));
    box.append(j);
  }

  // the card, as `arb card` shows it, and the request to sign it (Swift fetches the card
  // again itself and the system's Touch ID prompt names run, estimate and hash)
  async function cardSection(r, S) {
    const sec = section("审核单");
    const slip = el("div", "ab-slip");
    let card;
    try { card = await call("card", { run: r.id }); } catch (e) { slip.append(el("p", "body ab-bad", String(e))); sec.append(slip); return sec; }
    slip.append(el("pre", "code card", card.text));
    const foot = el("div", "ab-slip-foot");
    const note = el("span", "ab-note small", "");
    const key = S.key;
    const btn = el("button", "ab-btn ab-btn-primary body", "用 Touch ID 签字");
    btn.id = "sign";
    if (!card.matches) { note.textContent = "审核单和提交时的不一致：重新 arb submit"; note.className = "ab-note small ab-bad"; btn.disabled = true; }
    else if (!key) { note.textContent = "这台 Mac 上没有签字密钥"; btn.disabled = true; }
    else if (!key.trusted) {
      note.replaceChildren(el("span", "", `先在你自己的终端里信任这台 Mac 的签字密钥 ${key.fingerprint}：`), el("code", "", "arb key trust"));
      btn.disabled = true;
    } else note.textContent = `签的是：${r.id} · ≈ ${num(card.est_su, 0)} SU · ${card.card_hash.slice(0, 8)}`;
    btn.addEventListener("click", () => {
      btn.disabled = true; note.textContent = "等 Touch ID…"; note.className = "ab-note small";
      post({ cmd: "approve", run: r.id });
    });
    foot.append(note, btn);
    slip.append(foot);
    sec.append(slip);
    sec.append(el("p", "small ab-muted", `也可以在终端里：arb approve ${r.id}`));
    return sec;
  }

  // ------------------------------------------------------------ an idea's runs
  async function runs(box, idea) {
    const p = await call("runs", { idea });
    box.replaceChildren();
    const s = section(`${idea} 的 run`, "title");
    const list = el("div", "ab-list");
    if (!p.runs.length) list.append(el("div", "empty body", "还没有 run。在 notebook 里 lab.run(...)，或 arb submit"));
    for (const r of p.runs) {
      const row = el("div", "ab-line");
      row.setAttribute("role", "option");
      row.append(status(r.status), el("span", "name strong", r.id),
                 el("span", "meta ab-clip", (r.parent_run ? `← ${r.parent_run} · ` : "") + (r.change_reason || "")),
                 el("span", "when small", ago(r.created_at)));
      row.addEventListener("click", () => U.open({ kind: "run", id: r.id }));
      list.append(row);
    }
    s.append(list);
    box.append(s);
  }

  // ------------------------------------------------------------ compute
  async function compute(box) {
    const p = await call("compute");
    box.replaceChildren();
    const head = section("算力", "title");
    const used = p.used_su || 0, total = p.total_su;
    const line = el("p", "body");
    line.append(el("span", "strong", `${num(used, 1)} SU`), el("span", "ab-muted",
      total ? ` / 预算 ${num(total, 0)} SU（${Math.round((100 * used) / total)}%）` : " 已用（arboria.toml 里没设预算）"));
    if (p.awaiting_su) line.append(el("span", "ab-warn", ` · 等你签字的 ≈ ${num(p.awaiting_su, 0)} SU`));
    head.append(line);
    if (total) {
      const m = el("div", "meter"); const f = el("i");
      f.style.width = `${Math.min(100, (100 * used) / total)}%`;
      const a = el("i", "await");
      a.style.width = `${Math.max(0, Math.min(100 - (100 * used) / total, (100 * (p.awaiting_su || 0)) / total))}%`;
      m.append(f, a); head.append(m);
    }
    box.append(head);

    if (p.timeline.length) {
      const b = section("累计花费");
      const t0 = Date.parse(p.timeline[0][0]);
      const pts = [[0, 0], ...p.timeline.map(([t, su]) => [(Date.parse(t) - t0) / 3600e3, su])];
      const series = [{ name: "已用", pts, k: 1, step: true }];
      b.append(legend(series), chart(series, { height: 140, xfmt: (h) => (h < 48 ? `${fmt(h)} 小时` : `${fmt(h / 24)} 天`) }));
      box.append(b);
    }

    const g = section("作业");
    if (!p.jobs.length) g.append(el("p", "body ab-muted", "还没有作业"));
    else g.append(gantt(p.jobs));
    box.append(g);
  }

  // jobs per machine over time: one lane per job, grouped by machine
  function gantt(jobs) {
    const now = Date.now();
    const rows = jobs.map((j) => {
      const a = Date.parse(j.submitted_at), live = TONE[j.state] === "run";
      const b = live ? now : Math.max(a, Date.parse(j.updated_at) || a);
      return { ...j, a, b };
    }).filter((j) => isFinite(j.a)).sort((x, y) => (x.machine || "").localeCompare(y.machine || "") || x.a - y.a);
    const t0 = Math.min(...rows.map((r) => r.a)), t1 = Math.max(...rows.map((r) => r.b), t0 + 60e3);
    const W = 640, L = 120, R = 12, RH = 24, T = 4;
    let y = T, lastM = null;
    const lanes = [];
    for (const r of rows) {
      if (r.machine !== lastM) { lanes.push({ head: r.machine || "—", y }); y += RH; lastM = r.machine; }
      lanes.push({ r, y }); y += RH;
    }
    const H = y + 20;
    const X = (t) => L + ((t - t0) / (t1 - t0)) * (W - L - R);
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
    for (const ln of lanes) {
      if (ln.head) { s.append(svg("text", { class: "lane-head small", x: 0, y: ln.y + 16 }, ln.head)); continue; }
      const r = ln.r, tone = TONE[r.state] || "none";
      const lbl = svg("text", { class: "lane small", x: 8, y: ln.y + 16 }, `${r.run} s${r.segment}`);
      lbl.addEventListener("click", () => U.open({ kind: "run", id: r.run }));
      s.append(lbl);
      const bar = svg("rect", { class: `bar t-${tone}`, x: X(r.a), y: ln.y + 6, width: Math.max(3, X(r.b) - X(r.a)), height: RH - 12, rx: 2 });
      bar.append(svg("title", {}, `${r.run} s${r.segment} · ${WORD[r.state] || r.state} · ${dur(r.elapsed_s)} · ${num(r.billed_su)} SU`));
      s.append(bar);
    }
    const day = (t) => new Date(t).toLocaleString("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
    s.append(svg("line", { class: "base", x1: L, x2: W - R, y1: H - 18, y2: H - 18 }));
    s.append(svg("text", { class: "tick small", x: L, y: H - 2 }, day(t0)));
    s.append(svg("text", { class: "tick small", x: W - R, y: H - 2, "text-anchor": "end" }, day(t1)));
    const box = el("div", "ab-chart gantt");
    box.append(s);
    return box;
  }

  // ------------------------------------------------------------ roadmap
  // the human's milestones: a time strip, then each one's budget, ideas and checklist.
  // Changing them is the human's, in a terminal; the page only shows the commands.
  async function roadmap(box) {
    const p = await call("roadmap", { all: true });
    box.replaceChildren();
    const head = section("路线图", "title");
    const open = p.milestones.filter((m) => m.status === "open");
    if (!p.milestones.length) {
      head.append(el("p", "body ab-muted", "还没有里程碑。在你自己的终端里："));
      head.append(el("pre", "code cmdline", "arb milestone new NAME --title \"…\" --deadline YYYY-MM-DD --su 500 --idea IDEA --item \"…\""));
      box.append(head);
      return;
    }
    if (open.some((m) => m.deadline)) head.append(strip(open));
    box.append(head);
    for (const m of p.milestones) box.append(milestone(m));
  }

  const DAY = 86400e3;
  const today = () => { const d = new Date(); d.setHours(0, 0, 0, 0); return d.getTime(); };
  function strip(ms) {  // today → the last deadline, one diamond per milestone
    const t0 = today();
    const dated = ms.filter((m) => m.deadline).map((m) => ({ m, t: Date.parse(m.deadline + "T00:00:00") }));
    const lo = Math.min(t0, ...dated.map((d) => d.t)), hi = Math.max(t0 + 7 * DAY, ...dated.map((d) => d.t));
    const W = 640, L = 8, R = 8, H = 30 + 18 * dated.length;
    const X = (t) => L + ((t - lo) / (hi - lo)) * (W - L - R);
    const s = svg("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
    s.append(svg("line", { class: "base", x1: L, x2: W - R, y1: 12, y2: 12 }));
    s.append(svg("line", { class: "today", x1: X(t0), x2: X(t0), y1: 4, y2: H - 4 }));
    s.append(svg("text", { class: "tick small", x: X(t0) + 4, y: 9 }, "今天"));
    dated.sort((a, b) => a.t - b.t).forEach(({ m, t }, k) => {
      const x = X(t), y = 30 + 18 * k;
      const tone = m.flags.includes("late") || m.flags.includes("over") ? "bad" : m.flags.length ? "warn" : "ok";
      s.append(svg("path", { class: `ms t-${tone}`, d: `M${x},6 l6,6 l-6,6 l-6,-6z` }));
      s.append(svg("line", { class: "grid", x1: x, x2: x, y1: 18, y2: y - 10 }));
      s.append(svg("text", { class: "lane small", x: x < W * 0.7 ? x + 6 : x - 6, y, "text-anchor": x < W * 0.7 ? "start" : "end" },
                   `${m.name} · ${m.deadline.slice(5)}`));
    });
    const b = el("div", "ab-chart strip"); b.append(s);
    return b;
  }

  function milestone(m) {
    const sec = el("section", "milestone" + (m.status === "open" ? "" : " is-closed"));
    const h = el("div", "ab-row page-head");
    h.append(el("h2", "heading", m.name), el("span", "body", m.title), el("span", "ab-spacer"));
    if (m.status !== "open") h.append(el("span", "small ab-muted", m.status === "done" ? "已完成" : "已放弃"));
    else if (m.days_left != null) {
      const late = m.days_left < 0;
      const w = el("span", "small " + (late ? "ab-bad" : m.flags.includes("soon") ? "ab-warn" : "ab-muted"),
                   late ? `已过截止 ${-m.days_left} 天` : m.days_left === 0 ? "今天截止" : `还有 ${m.days_left} 天 · ${m.deadline}`);
      h.append(w);
    }
    sec.append(h);

    const su = el("p", "body");
    if (m.su_budget == null) su.append(el("span", "", `已用 ${num(m.spent_su, 1)} SU`), el("span", "ab-muted", "（未设算力预算）"));
    else {
      su.append(el("span", "strong", `${num(m.spent_su, 1)} SU`), el("span", "ab-muted", ` / ${num(m.su_budget, 0)} SU`));
      su.append(el("span", m.left_su < 0 ? "ab-bad" : "ab-muted", m.left_su < 0 ? ` · 超出 ${num(-m.left_su, 1)}` : ` · 剩余 ${num(m.left_su, 1)}`));
    }
    if (m.awaiting_su) su.append(el("span", m.flags.includes("tight") ? "ab-warn" : "ab-muted", ` · 等你签字的 ≈ ${num(m.awaiting_su, 0)} SU`));
    sec.append(su);
    if (m.su_budget) {
      const bar = el("div", "meter"); const f = el("i"), a = el("i", "await");
      const pct = (v) => Math.max(0, Math.min(100, (100 * v) / m.su_budget));
      f.style.width = `${pct(m.spent_su)}%`; a.style.width = `${Math.min(100 - pct(m.spent_su), pct(m.awaiting_su))}%`;
      if (m.left_su < 0) f.classList.add("over");
      bar.append(f, a); sec.append(bar);
    }

    if (m.idea_progress.length) {
      const list = el("div", "ab-list ideas");
      for (const i of m.idea_progress) {
        const row = el("div", "ab-line");
        row.append(el("span", "name", i.name), el("span", "meta", `${i.status} · ${i.runs} 个 run · ${num(i.su, 1)} SU`));
        if (i.runs) { row.setAttribute("role", "option"); row.addEventListener("click", () => U.open({ kind: "runs", idea: i.name })); }
        list.append(row);
      }
      sec.append(list);
    }
    if (m.items.length) {
      const list = el("div", "checklist");
      m.items.forEach((it, k) => {
        const row = el("div", "item body" + (it.done ? " is-done" : ""));
        const mark = el("span", it.done ? "ab-ok" : "ab-muted"); mark.append(glyph(it.done ? "ok" : "wait"));
        row.append(mark, el("span", "", `${k + 1}. ${it.text}`));
        list.append(row);
      });
      sec.append(list);
      sec.append(el("p", "small ab-muted", `打勾在终端里：arb milestone check ${m.name} 编号`));
    }
    return sec;
  }

  return { run, runs, compute, roadmap };
};
