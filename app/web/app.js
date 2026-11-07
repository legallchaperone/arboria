// The sidebar: renders what arbd returns (via Swift) and asks Swift to select ideas.
// Ledger text is only ever set with textContent (never innerHTML).
(function () {
  const $ = (id) => document.getElementById(id);
  const post = (m) => window.webkit.messageHandlers.app.postMessage(m);
  let current = null;

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function depthOf(idea, byName) {
    let d = 0, p = idea.parent;
    while (p && byName[p] && d < 3) { d++; p = byName[p].parent; }
    return d;
  }

  function renderTree(ideas) {
    const tree = $("tree");
    tree.replaceChildren();
    const byName = Object.fromEntries(ideas.map((i) => [i.name, i]));
    const groups = [["进行中", ideas.filter((i) => i.status === "open")],
                    ["已结束", ideas.filter((i) => i.status !== "open")]];
    for (const [label, list] of groups) {
      if (!list.length) continue;
      tree.append(el("div", "group", label));
      const ul = el("ul");
      for (const i of list) {
        const li = el("li", (i.worktree ? "open" : "closed") + " depth" + depthOf(i, byName));
        if (i.name === current) li.classList.add("current");
        li.append(el("div", "name", i.name));
        const runs = i.run_count ? ` · ${i.run_count} 个 run` : "";
        li.append(el("div", "meta", (i.hypothesis || "") + runs));
        li.title = i.worktree ? `${i.hypothesis || ""}\n指标：${i.metric || ""}` : `${i.status}，没有 worktree`;
        if (i.worktree && i.has_notebook) li.addEventListener("click", () => post({ cmd: "select", idea: i.name }));
        ul.append(li);
      }
      tree.append(ul);
    }
    if (!ideas.length) tree.append(el("div", "sub", "还没有 idea。在终端里：arb idea new NAME"));
  }

  window.arb = {
    render(state) {
      if ("current" in state) current = state.current;
      if (state.error) { $("status").textContent = state.error; return; }
      if (state.status != null) $("status").textContent = state.status;
      if (state.project) {
        $("status").textContent = "";
        $("title").textContent = state.project.name;
        $("sub").textContent = state.project.project_id ? `项目 ${state.project.project_id}` : state.project.root;
        renderTree(state.project.ideas);
      }
    },
  };
  post({ cmd: "ready" });
})();
