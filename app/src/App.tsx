// The window (Linear-like): the sidebar (收件箱, projects and their ideas, 种子园, 设置), a
// header, the page, the inspector on the right (a run, the card to sign), the terminal
// (bottom / left / right). Data comes from the core's read-only views and is re-read when
// the Rust side says a ledger changed.

import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, message, onChange } from "./api";
import type { Action, Board, HelperStatus, Launcher, Platform, ProjectSummary } from "./types";
import { Palette, type Choice } from "./components/Palette";
import { TerminalPanel, type Dock, type TabSpec } from "./components/Terminal";
import { AddProject } from "./components/AddProject";
import { Dialog, type DialogSpec } from "./components/Dialog";
import { ActionsContext, newIdea, newSeed, type Actions } from "./views/actions";
import { Glyph } from "./components/Glyph";
import type { Entry } from "./lib/inbox";
import { Sidebar } from "./views/Sidebar";
import { Inspector } from "./views/Inspector";
import { InboxView } from "./views/InboxView";
import { ProjectView } from "./views/ProjectView";
import { IdeaPage, SeedsView } from "./views/IdeaView";
import { SettingsView } from "./views/SettingsView";
import type { Inspect, Route } from "./views/route";

/** The clock (the screenshots pin it to the fixture's time). */
export const clock = { now: () => Date.now() };

/** `ssh host` out of the core's NeedLogin message. */
export function loginCommand(msg: string): string | null {
  const m = /`(ssh [^`]+)`/.exec(msg);
  return m ? m[1] : null;
}

/** Per-viewer conveniences (the terminal's place and size); never state that matters. */
function remembered<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}
function remember(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* private mode */
  }
}

/** Pressing on a header (not on a control) drags the window; double-click zooms. */
function drag(e: MouseEvent) {
  const interactive =
    "button, input, a, select, textarea, label, [role=menu], [role=menuitem], [role=option], [role=button], [role=tab], [role=dialog], [role=separator]";
  if (e.button !== 0 || (e.target as HTMLElement).closest(interactive)) return;
  const w = getCurrentWindow();
  if (e.detail === 2) void w.toggleMaximize().catch(() => undefined);
  else void w.startDragging().catch(() => undefined);
}

export function App() {
  const [platform, setPlatform] = useState<Platform | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [boards, setBoards] = useState<Record<string, Board>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [helper, setHelper] = useState<HelperStatus | null>(null);
  const [route, setRoute] = useState<Route>({ k: "inbox" });
  const [inspect, setInspect] = useState<Inspect | null>(null);
  const [inboxSel, setInboxSel] = useState<string | null>(null);
  const [palette, setPalette] = useState<"commands" | "ideas" | null>(null);
  const [termOpen, setTermOpen] = useState(false);
  const [tabs, setTabs] = useState<TabSpec[]>([]);
  const [activeTab, setActiveTab] = useState<number | null>(null);
  const [dock, setDock] = useState<Dock>(() => remembered<Dock>("arboria.term.dock", "bottom"));
  const [termSize, setTermSize] = useState<Record<Dock, number>>(() => remembered("arboria.term.size", { bottom: 300, left: 520, right: 520 }));
  const [launchers, setLaunchers] = useState<Launcher[]>([{ id: "shell", label: "新终端", installed: true, group: null }]);
  const [adding, setAdding] = useState(false);
  const [dialog, setDialog] = useState<DialogSpec | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [ideaTab, setIdeaTab] = useState<"overview" | "notebook">("overview");
  const [now, setNow] = useState(clock.now());
  const nextTab = useRef(1);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 5000);
    return () => clearTimeout(t);
  }, [toast]);

  const loadBoard = useCallback(async (root: string) => {
    try {
      const b = await api.board(root);
      setBoards((m) => ({ ...m, [root]: b }));
      setErrors((m) => {
        const { [root]: _, ...rest } = m;
        return rest;
      });
    } catch (e) {
      setErrors((m) => ({ ...m, [root]: message(e) }));
    }
  }, []);
  const loadAll = useCallback(async () => {
    try {
      const ps = await api.projects();
      setProjects(ps);
      await Promise.all(ps.filter((p) => !p.error).map((p) => loadBoard(p.root)));
    } catch (e) {
      setToast(message(e));
    }
    api.helperStatus().then(setHelper, () => undefined);
  }, [loadBoard]);

  useEffect(() => {
    api.platform().then(setPlatform, (e) => setToast(message(e)));
    api.terminalLaunchers().then(setLaunchers, () => undefined);
    void loadAll();
    const un = onChange((root) => {
      if (root) {
        void loadBoard(root);
        api.projects().then(setProjects, () => undefined);
      } else void loadAll();
    });
    const t = setInterval(() => {
      setNow(clock.now());
      api.helperStatus().then(setHelper, () => undefined);
    }, 30_000);
    return () => {
      clearInterval(t);
      void un.then((fs) => fs.forEach((f) => f()));
    };
  }, [loadAll, loadBoard]);

  const go = (r: Route) => {
    if (r.k !== "idea" || route.k !== "idea" || r.idea !== route.idea || r.root !== route.root) setIdeaTab("overview");
    setRoute(r);
    setInspect(null);
  };
  const firstProject = projects.find((p) => p.kind === "project" && !p.error);
  const currentRoot = route.k === "project" || route.k === "idea" ? route.root : firstProject?.root;
  const waiting = projects.reduce((n, p) => n + p.inbox.length, 0);
  const home = projects.find((p) => p.kind === "home");

  const openTab = useCallback(
    (program: string | null, type?: string, at?: string) => {
      const cwd = at ?? currentRoot ?? home?.root ?? "/";
      const key = nextTab.current++;
      const id = program ?? "shell";
      const l = launchers.find((x) => x.id === id);
      const title = id === "shell" ? `终端 ${key}` : id.startsWith("ssh:") ? id.slice(4) : l?.label ?? id;
      setTabs((t) => [...t, { key, title, cwd, program: id, type }]);
      setActiveTab(key);
      setTermOpen(true);
    },
    [currentRoot, launchers, home],
  );

  const addProject = async (path?: string) => {
    try {
      const entry = path ? await api.addProject(path) : await api.pickProject();
      if (!entry) return;
      setAdding(false);
      setToast(`已添加 ${entry.path}`);
      await loadAll();
      go({ k: "project", root: entry.path });
    } catch (e) {
      setToast(message(e));
    }
  };

  const actions: Actions = {
    ask: setDialog,
    act: async (root: string, action: Action, done?: string) => {
      const out = await api.act(root, action);
      setToast(done ?? (out.output || "好了"));
      await loadBoard(root);
      if (home && root !== home.root && action.kind === "seed_plant") await loadBoard(home.root);
      if (action.kind === "seed_new" && !home) await loadAll();
      else api.projects().then(setProjects, () => undefined);
      if (action.kind === "idea_new") go({ k: "idea", root, idea: action.name });
      if (action.kind === "seed_plant") go({ k: "idea", root, idea: action.as_name ?? action.name });
      if (action.kind === "idea_close" && route.k === "idea" && route.idea === action.name) go({ k: "project", root });
    },
    terminalAt: (cwd: string) => openTab(null, undefined, cwd),
    reveal: (root: string, path?: string) => void api.reveal(root, path).catch((e) => setToast(message(e))),
    removeProject: async (root: string) => {
      await api.removeProject(root);
      setBoards(({ [root]: _, ...rest }) => rest);
      if ((route.k === "project" || route.k === "idea") && route.root === root) go({ k: "inbox" });
      setToast("已从列表移除（文件没动）");
      await loadAll();
    },
    projectSettings: (root: string) => go({ k: "settings", tab: "project", root }),
    projects,
  };

  const inspectEntry = (e: Entry) => {
    setInboxSel(e.key);
    if (e.run) setInspect({ k: "run", root: e.root, run: e.run });
    else if (e.task) setInspect({ k: "todo", root: e.root, task: e.task });
    else if (e.command) setInspect({ k: "command", title: e.title, text: e.meta, command: e.command });
  };

  const ideas = useMemo(
    () =>
      Object.values(boards).flatMap((b) =>
        b.ideas
          .filter((i) => i.stage === "open" || i.stage === "seed")
          .map((i) => ({ root: b.project.root, project: b.project.name, idea: i })),
      ),
    [boards],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const k = e.key.toLowerCase();
      if (mod && k === "k") {
        e.preventDefault();
        setPalette("commands");
      } else if (mod && k === "p") {
        e.preventDefault();
        setPalette("ideas");
      } else if (mod && k === "t") {
        e.preventDefault();
        openTab(null);
      } else if (mod && e.key === "`") {
        e.preventDefault();
        if (tabs.length === 0) openTab(null);
        else setTermOpen((o) => !o);
      } else if (mod && e.key === ",") {
        e.preventDefault();
        go({ k: "settings", tab: "general" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const commands: Choice[] = [
    { key: "inbox", name: "收件箱", run: () => go({ k: "inbox" }) },
    ...projects
      .filter((p) => p.kind === "project")
      .map((p) => ({ key: "p" + p.root, name: p.name, desc: "项目概览", kind: "项目", run: () => go({ k: "project", root: p.root }) })),
    ...launchers
      .filter((l) => l.installed)
      .map((l) => ({
        key: "term-" + l.id,
        name: l.label,
        desc: l.id === "shell" ? "⌘T" : l.group === "ssh" ? "ssh 到这台机器" : "在终端标签里",
        kind: "终端",
        run: () => openTab(l.id),
      })),
    ...(currentRoot && boards[currentRoot]
      ? [{ key: "new-idea", name: "新建 idea…", desc: boards[currentRoot].project.name, run: () => setDialog(newIdea(actions, boards[currentRoot])) }]
      : []),
    ...(platform ? [{ key: "new-seed", name: "新的种子…", desc: "种子园", run: () => setDialog(newSeed(actions, home?.root ?? platform.home)) }] : []),
    { key: "add", name: "添加项目…", desc: "本机的仓库，或者从仓库链接克隆", run: () => setAdding(true) },
    { key: "settings", name: "设置", desc: "⌘,", run: () => go({ k: "settings", tab: "general" }) },
    { key: "machines", name: "机器", desc: "集群、账户、分区、路径", kind: "设置", run: () => go({ k: "settings", tab: "machines" }) },
    { key: "reload", name: "重新读取", desc: "所有项目的账本", run: () => void loadAll() },
  ];
  const ideaChoices: Choice[] = [...ideas]
    .sort((a, b) => (a.idea.last_active < b.idea.last_active ? 1 : -1))
    .map((x) => ({
      key: x.root + x.idea.name,
      name: x.idea.name,
      desc: x.idea.hypothesis ?? x.idea.gist ?? "",
      kind: x.idea.stage === "seed" ? "种子" : x.project,
      run: () => (x.idea.stage === "seed" ? go({ k: "seeds" }) : go({ k: "idea", root: x.root, idea: x.idea.name })),
    }));

  // header crumbs
  const board = route.k === "project" || route.k === "idea" ? boards[route.root] ?? null : null;
  const idea = route.k === "idea" ? board?.ideas.find((i) => i.name === route.idea) : undefined;
  const crumbs: { label: string; to?: Route }[] =
    route.k === "inbox"
      ? [{ label: "收件箱" }]
      : route.k === "seeds"
        ? [{ label: "种子园" }]
        : route.k === "settings"
          ? [{ label: "设置" }]
          : route.k === "project"
            ? [{ label: board?.project.name ?? "项目" }]
            : [{ label: board?.project.name ?? "项目", to: { k: "project", root: route.root } }, { label: route.idea }];

  let page: ReactNode = null;
  if (route.k === "inbox")
    page = (
      <InboxView
        projects={projects}
        now={now}
        selected={inboxSel}
        onSelect={inspectEntry}
        onOpen={(e) => e.idea && go({ k: "idea", root: e.root, idea: e.idea })}
        onAdd={() => setAdding(true)}
      />
    );
  else if (route.k === "project")
    page = (
      <ProjectView
        board={board}
        error={errors[route.root] ?? null}
        now={now}
        onIdea={(i) => go({ k: "idea", root: route.root, idea: i })}
        onSettings={() => go({ k: "settings", tab: "project", root: route.root })}
      />
    );
  else if (route.k === "idea")
    page =
      board && idea ? (
        <IdeaPage
          board={board}
          idea={idea}
          now={now}
          tab={ideaTab}
          onTab={setIdeaTab}
          selectedRun={inspect?.k === "run" ? inspect.run : null}
          onRun={(run) => setInspect({ k: "run", root: route.root, run })}
        />
      ) : (
        <div className="empty">{errors[route.root] ?? "读取中…"}</div>
      );
  else if (route.k === "seeds") page = <SeedsView board={home ? boards[home.root] ?? null : null} homeRoot={platform?.home ?? null} now={now} />;
  else
    page = (
      <SettingsView
        tab={route.tab}
        root={route.root}
        projects={projects}
        now={now}
        go={(tab) => setRoute({ k: "settings", tab, root: route.root })}
        toast={setToast}
        onTerminal={(cmd) => openTab(null, cmd)}
      />
    );

  return (
    <ActionsContext.Provider value={actions}>
    <div className={"shell" + (platform?.os === "macos" ? " is-mac" : "")}>
      <div onMouseDown={drag} style={{ display: "contents" }}>
        <Sidebar
          route={route}
          projects={projects}
          boards={boards}
          waiting={waiting}
          helper={helper}
          go={go}
          onAdd={() => setAdding(true)}
          onLogin={(m) => openTab(`ssh:${m}`, undefined)}
        />
      </div>
      <div className="work">
        <header className="head" onMouseDown={drag}>
          <span className="crumbs">
            {crumbs.map((c, i) => (
              <span key={i} style={{ display: "contents" }}>
                {i > 0 && <span className="sep">/</span>}
                {c.to ? (
                  <button type="button" onClick={() => go(c.to!)}>
                    {c.label}
                  </button>
                ) : (
                  <span className="here">{c.label}</span>
                )}
              </span>
            ))}
          </span>
          <button type="button" className="icon-btn" aria-label="搜索和命令（⌘K）" title="搜索和命令 ⌘K" onClick={() => setPalette("commands")}>
            <Glyph name="search" />
          </button>
          <button
            type="button"
            className={"icon-btn" + (termOpen ? " is-on" : "")}
            aria-label="终端（⌘`）"
            title="终端 ⌘`"
            onClick={() => (tabs.length === 0 ? openTab(null) : setTermOpen((o) => !o))}
          >
            <Glyph name="terminal" />
          </button>
        </header>
        <div className={`app-work is-${dock}`}>
          <div className="body-split app-body">
            <main className={"content" + (route.k === "idea" ? " is-fill" : "")}>{page}</main>
            {inspect && (
              <Inspector
                what={inspect}
                boards={boards}
                platform={platform}
                now={clock.now}
                onClose={() => setInspect(null)}
                onTerminal={(cmd) => openTab(null, cmd)}
                onIdea={(root, i) => go({ k: "idea", root, idea: i })}
                onSigned={() => currentRoot && void loadBoard(currentRoot)}
              />
            )}
          </div>
          <TerminalPanel
            tabs={tabs}
            active={activeTab}
            visible={termOpen}
            dock={dock}
            size={termSize[dock]}
            launchers={launchers}
            onSelect={setActiveTab}
            onNew={(p) => openTab(p)}
            onClose={(k) => {
              const rest = tabs.filter((x) => x.key !== k);
              setTabs(rest);
              if (activeTab === k) setActiveTab(rest.at(-1)?.key ?? null);
              if (rest.length === 0) setTermOpen(false);
            }}
            onDock={(d) => {
              setDock(d);
              remember("arboria.term.dock", d);
            }}
            onSize={(px) =>
              setTermSize((m) => {
                const next = { ...m, [dock]: px };
                remember("arboria.term.size", next);
                return next;
              })
            }
            onHide={() => setTermOpen(false)}
          />
        </div>
      </div>
      {adding && (
        <AddProject
          onClose={() => setAdding(false)}
          onFolder={() => void addProject()}
          onCloned={(c) => {
            setAdding(false);
            setToast(`已克隆并初始化 ${c.path}`);
            void loadAll().then(() => go({ k: "settings", tab: "project", root: c.path }));
          }}
        />
      )}
      {dialog && <Dialog spec={dialog} onClose={() => setDialog(null)} />}
      {palette && (
        <Palette placeholder={palette === "ideas" ? "换 idea…" : "搜索或输入命令…"} choices={palette === "ideas" ? ideaChoices : commands} onClose={() => setPalette(null)} />
      )}
      {toast && (
        <div className="app-toasts">
          <div className="ab-toast small" role="status">
            <span>{toast}</span>
            <button type="button" className="link muted" onClick={() => setToast(null)} aria-label="关闭">
              ×
            </button>
          </div>
        </div>
      )}
    </div>
    </ActionsContext.Provider>
  );
}
