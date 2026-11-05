// The window: title bar, one of three screens (收件箱 / 全局 / idea), the approval card,
// the palettes and the terminal panel. Data comes from the core's read-only views and is
// re-read when the Rust side says a ledger changed.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, message, onChange } from "./api";
import type { Board, HelperStatus, Launcher, Platform, ProjectSummary } from "./types";
import { TitleBar, type Crumb } from "./components/TitleBar";
import { ApprovalSheet } from "./components/ApprovalSheet";
import { Palette, type Choice } from "./components/Palette";
import { TerminalPanel, type Dock, type TabSpec } from "./components/Terminal";
import { Inbox } from "./screens/Inbox";
import { Overview } from "./screens/Overview";
import { Idea } from "./screens/Idea";
import { Machines } from "./screens/Machines";
import { AddProject } from "./components/AddProject";
import type { Entry } from "./lib/inbox";

type View =
  | { k: "inbox" }
  | { k: "overview"; root: string }
  | { k: "idea"; root: string; idea: string }
  | { k: "machines" };

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

/** The clock (the screenshots pin it to the fixture's time). */
export const clock = { now: () => Date.now() };

/** `ssh host` out of the core's NeedLogin message. */
export function loginCommand(msg: string): string | null {
  const m = /`(ssh [^`]+)`/.exec(msg);
  return m ? m[1] : null;
}

export function App() {
  const [platform, setPlatform] = useState<Platform | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [boards, setBoards] = useState<Record<string, Board>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [helper, setHelper] = useState<HelperStatus | null>(null);
  const [view, setView] = useState<View>({ k: "inbox" });
  const [sheet, setSheet] = useState<{ root: string; run: string } | null>(null);
  const [palette, setPalette] = useState<"commands" | "ideas" | null>(null);
  const [termOpen, setTermOpen] = useState(false);
  const [dock, setDock] = useState<Dock>(() => remembered<Dock>("arboria.term.dock", "bottom"));
  const [termSize, setTermSize] = useState<Record<Dock, number>>(() =>
    remembered("arboria.term.size", { bottom: 320, left: 560, right: 560 }),
  );
  const [tabs, setTabs] = useState<TabSpec[]>([]);
  const [activeTab, setActiveTab] = useState<number | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [launchers, setLaunchers] = useState<Launcher[]>([{ id: "shell", label: "新终端", installed: true, group: null }]);
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [now, setNow] = useState(clock.now());
  const nextTab = useRef(1);

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

  const firstProject = projects.find((p) => p.kind === "project" && !p.error) ?? projects[0];
  const currentRoot = view.k === "inbox" || view.k === "machines" ? firstProject?.root : view.root;
  const waiting = projects.reduce((n, p) => n + p.inbox.length, 0);

  const openTab = useCallback(
    (program: string | null, type?: string) => {
      const cwd = currentRoot ?? "/";
      const key = nextTab.current++;
      const id = program ?? "shell";
      const l = launchers.find((x) => x.id === id);
      const title = id === "shell" ? `终端 ${key}` : id.startsWith("ssh:") ? id.slice(4) : l?.label ?? id;
      program = id;
      setTabs((t) => [...t, { key, title, cwd, program: id, type }]);
      setActiveTab(key);
      setTermOpen(true);
    },
    [currentRoot, launchers],
  );

  const addProject = async (path?: string) => {
    try {
      const entry = path ? await api.addProject(path) : await api.pickProject();
      if (!entry) return; // cancelled
      setAdding(false);
      setToast(`已添加 ${entry.path}`);
      await loadAll();
    } catch (e) {
      setToast(message(e));
    }
  };

  const openEntry = (e: Entry) => {
    if (e.kind === "approve" && e.run) setSheet({ root: e.root, run: e.run });
    else if (e.idea) setView({ k: "idea", root: e.root, idea: e.idea });
    else if (e.command) setToast(`在你自己的终端运行：${e.command}`);
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
  const recent = useMemo(
    () =>
      [...ideas]
        .filter((x) => x.idea.stage === "open")
        .sort((a, b) => (a.idea.last_active < b.idea.last_active ? 1 : -1))
        .slice(0, 3)
        .map((x) => ({ root: x.root, idea: x.idea.name })),
    [ideas],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette("commands");
      } else if (mod && e.key.toLowerCase() === "p") {
        e.preventDefault();
        setPalette("ideas");
      } else if (mod && e.key.toLowerCase() === "t") {
        e.preventDefault();
        openTab(null);
      } else if (mod && e.key === "`") {
        e.preventDefault();
        if (!termOpen && tabs.length === 0) openTab(null);
        else setTermOpen((o) => !o);
      } else if (e.key === "Escape" && !sheet && !palette && view.k !== "inbox") {
        setView({ k: "inbox" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const commands: Choice[] = [
    { key: "inbox", name: "收件箱", desc: "待你处理的事", run: () => setView({ k: "inbox" }) },
    ...(firstProject ? [{ key: "overview", name: "全局", desc: "idea 树、花费、机器", run: () => setView({ k: "overview", root: currentRoot ?? firstProject.root }) }] : []),
    ...launchers
      .filter((l) => l.installed)
      .map((l) => ({
        key: "term-" + l.id,
        name: l.label,
        desc: l.id === "shell" ? "在项目主 worktree 里（⌘T）" : l.group === "ssh" ? "ssh 到这台机器（Duo 过期时在这里登录）" : "在终端标签里",
        run: () => openTab(l.id),
      })),
    { key: "add", name: "添加项目…", desc: "本机的仓库，或者从仓库链接克隆", run: () => setAdding(true) },
    { key: "machines", name: "机器配置", desc: "编辑 clusters.toml（集群、账户、分区、路径）", run: () => setView({ k: "machines" }) },
    { key: "reload", name: "重新读取", desc: "所有项目的账本", run: () => void loadAll() },
    { key: "helper", name: "安装后台小助手", desc: "登录时启动，盯作业、发通知", run: () => void installHelper() },
  ];
  const ideaChoices: Choice[] = [...ideas]
    .sort((a, b) => (a.idea.last_active < b.idea.last_active ? 1 : -1))
    .map((x) => ({
    key: x.root + x.idea.name,
    name: x.idea.name,
    desc: x.idea.hypothesis ?? x.idea.gist ?? "",
    kind: x.idea.stage === "seed" ? "种子" : x.project,
    run: () => setView({ k: "idea", root: x.root, idea: x.idea.name }),
  }));

  async function installHelper() {
    try {
      const path = await api.helperInstall();
      setToast(`小助手已安装：${path}`);
      setHelper(await api.helperStatus());
    } catch (e) {
      setToast(message(e));
    }
  }

  const board = view.k === "overview" || view.k === "idea" ? boards[view.root] ?? null : null;
  const crumbs: Crumb[] = [];
  if (view.k === "inbox") crumbs.push({ label: "待你处理" });
  else if (view.k === "machines") crumbs.push({ label: "机器配置" });
  else {
    const name = board?.project.name ?? view.root;
    crumbs.push({ label: name, onClick: () => setView({ k: "overview", root: view.root }) });
    if (view.k === "idea") crumbs.push({ label: view.idea });
    else crumbs.push({ label: "全局" });
  }
  const idea = view.k === "idea" ? board?.ideas.find((i) => i.name === view.idea) : undefined;

  return (
    <div className={"app" + (platform?.os === "macos" ? " is-mac" : "")}>
      <TitleBar
        crumbs={crumbs}
        waiting={waiting}
        terminalOpen={termOpen}
        onInbox={() => setView({ k: "inbox" })}
        onOverview={() => firstProject && setView({ k: "overview", root: currentRoot ?? firstProject.root })}
        onSwitch={() => setPalette("ideas")}
        onCommands={() => setPalette("commands")}
        onTerminal={() => (tabs.length === 0 ? openTab(null) : setTermOpen((o) => !o))}
      />
      <div className={`app-work is-${dock}`}>
      <div className="app-body">
        {view.k === "inbox" && (
          <Inbox
            projects={projects}
            now={now}
            onOpen={openEntry}
            recent={recent}
            onIdea={(root, idea) => setView({ k: "idea", root, idea })}
            onAddProject={(p) => (p ? void addProject(p) : setAdding(true))}
          />
        )}
        {view.k === "overview" && (
          <Overview
            board={board}
            error={errors[view.root] ?? null}
            projects={projects}
            helper={helper}
            now={now}
            onIdea={(i) => setView({ k: "idea", root: view.root, idea: i })}
            onProject={(root) => setView({ k: "overview", root })}
            onInstallHelper={() => void installHelper()}
            onLogin={(m) => openTab(null, loginCommand(helper?.needs_login[m] ?? "") ?? `ssh ${m}`)}
            onMachines={() => setView({ k: "machines" })}
            onEditConfig={(path) => api.openInEditor(path).catch((e) => setToast(message(e)))}
          />
        )}
        {view.k === "idea" && board && idea && (
          <Idea
            board={board}
            idea={idea}
            now={now}
            busy={busy}
            onCard={(run) => setSheet({ root: view.root, run })}
            onNotebook={() => {
              setBusy("正在启动 marimo…");
              api
                .openNotebook(view.root, idea.name)
                .catch((e) => setToast(message(e)))
                .finally(() => setBusy(null));
            }}
          />
        )}
        {view.k === "machines" && <Machines onToast={setToast} onTerminal={(cmd) => openTab(null, cmd)} />}
        {view.k === "idea" && (!board || !idea) && (
          <main className="app-main">
            <p className="small ab-muted">{errors[view.root] ?? `读取 ${view.idea}…`}</p>
          </main>
        )}
      </div>
      <TerminalPanel
        tabs={tabs}
        active={activeTab}
        visible={termOpen}
        dock={dock}
        size={termSize[dock]}
        onSelect={setActiveTab}
        onNew={(p) => openTab(p)}
        launchers={launchers}
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
      {sheet && platform && (
        <ApprovalSheet
          root={sheet.root}
          run={sheet.run}
          platform={platform}
          now={clock.now}
          onClose={() => setSheet(null)}
          onRunInTerminal={(cmd) => {
            setSheet(null);
            openTab(null, cmd);
          }}
        />
      )}
      {adding && (
        <AddProject
          onClose={() => setAdding(false)}
          onFolder={() => void addProject()}
          onCloned={(c) => {
            setAdding(false);
            setToast(`已克隆并初始化 ${c.path}：在它的 arboria.toml 里填 default_machine 等设置`);
            void loadAll().then(() => setView({ k: "overview", root: c.path }));
          }}
        />
      )}
      {palette && (
        <Palette
          placeholder={palette === "ideas" ? "换 idea…" : "命令…"}
          choices={palette === "ideas" ? ideaChoices : commands}
          onClose={() => setPalette(null)}
        />
      )}
      {toast && (
        <div className="app-toasts">
          <div className="ab-toast small" role="status">
            <span>{toast}</span>
            <button type="button" className="app-link ab-muted" onClick={() => setToast(null)} aria-label="关闭">
              ×
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
