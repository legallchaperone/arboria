// The app's own notebook (ADR 0005): text and command blocks in one editor, run in the
// notebook's kernel, outputs drawn under each block. The file is saved as you type
// (canonical markdown, ids kept); results go to the outputs cache, never into the file.
// A block whose command was handed to the agent re-runs once the agent has delivered.

import { useEffect, useRef, useState, useSyncExternalStore, type JSX } from "react";
import { Editor } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { Placeholder } from "@tiptap/extensions";
import { api, message, onAgent, onNotebook } from "../api";
import type { Board, KernelInfo, NbView } from "../types";
import { ArbBlock } from "./arbBlock";
import { SlashMenu } from "./slash";
import { BLOCK, docToText, partsToDoc, type Node } from "./markdown";
import { NbStore, staleBlocks, waitingTask } from "./store";
import type { CompletionData } from "./cm";
import { STATUS_TEXT, clip } from "../lib/format";

const SAVE_DELAY = 600;

/** Blocks in document order, with their code. */
function blocksOf(editor: Editor): { id: string; code: string }[] {
  const out: { id: string; code: string }[] = [];
  editor.state.doc.forEach((n) => {
    if (n.type.name === BLOCK && n.attrs.id) out.push({ id: String(n.attrs.id), code: n.textContent });
  });
  return out;
}

function KernelBar(props: {
  store: NbStore;
  kernel: KernelInfo | null;
  onRunAll: () => void;
  onInterrupt: () => void;
  onRestart: () => void;
  saving: string;
}) {
  const running = useSyncExternalStore(
    (f) => props.store.subscribeAll(f),
    () => props.store.ids().filter((id) => props.store.get(id).running || props.store.get(id).queued).length,
  );
  const k = props.kernel;
  let state: JSX.Element;
  if (!k) state = <span className="nb-muted">kernel 没在跑（运行一块时启动）</span>;
  else if (k.lab === false)
    state = (
      <span className="nb-err" title={k.lab_error ?? ""}>
        Python {k.python} · lab 导入失败：{clip(k.lab_error ?? "", 80)}
      </span>
    );
  else
    state = (
      <span className="nb-muted">
        Python {k.python} · pid {k.pid}
        {running ? ` · ${running} 块在跑` : ""}
      </span>
    );
  return (
    <div className="nb-bar">
      {state}
      <span className="nb-bar-actions">
        <span className="nb-muted">{props.saving}</span>
        <button type="button" className="ab-btn" onClick={props.onRunAll} title="从上到下运行每一块">
          全部运行
        </button>
        <button type="button" className="ab-btn" onClick={props.onInterrupt} disabled={!running} title="中断正在跑的块（KeyboardInterrupt）">
          中断
        </button>
        <button type="button" className="ab-btn" onClick={props.onRestart} disabled={!k} title="停掉 kernel；下次运行时重新启动，变量都清空">
          重启 kernel
        </button>
      </span>
    </div>
  );
}

/** The editor for a notebook that exists as `.md`. */
function NotebookEditor(props: {
  root: string;
  idea: string;
  view: NbView;
  board: Board;
  onRun: (run: string) => void;
  onTask: (task: string) => void;
}) {
  const { root, idea } = props;
  const host = useRef<HTMLDivElement>(null);
  const [store] = useState(() => new NbStore());
  const [kernel, setKernel] = useState<KernelInfo | null>(props.view.kernel);
  const [saving, setSaving] = useState("");
  const editorRef = useRef<Editor | null>(null);
  const data = useRef<CompletionData>({ commands: [], runs: [] }).current;
  const alive = useRef(props.view.kernel != null);
  alive.current = kernel != null;

  // completion: the board's runs, the command library (re-read when the agent finished)
  data.runs = props.board.runs
    .filter((r) => r.idea === idea || props.board.project.kind === "home")
    .map((r) => ({ id: r.id, status: STATUS_TEXT[r.status] ?? r.status, change: clip(r.change_reason, 40) }));

  useEffect(() => {
    let live = true;
    const loadCommands = () =>
      api.nbCommands(root).then(
        (c) => {
          if (live) data.commands = c;
        },
        () => {},
      );
    void loadCommands();
    const refreshTasks = () =>
      api.agentRows(root).then(
        (rows) => {
          if (!live) return;
          store.setTasks(rows);
          rerunDelivered();
        },
        () => {},
      );
    void refreshTasks();
    const un = onAgent(
      (r) => {
        if (r !== root) return;
        void refreshTasks();
        void loadCommands();
      },
      () => {},
    );
    return () => {
      live = false;
      void un.then((fs) => fs.forEach((f) => f()));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root]);

  // ---- saving

  const lastBody = useRef<string>("");
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveChain = useRef<Promise<void>>(Promise.resolve());

  const save = (): Promise<void> => {
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = null;
    const ed = editorRef.current;
    if (!ed) return saveChain.current;
    const body = docToText(ed.getJSON() as Node);
    if (body === lastBody.current) return saveChain.current;
    lastBody.current = body;
    setSaving("保存中…");
    saveChain.current = saveChain.current.then(() =>
      api.nbSave(root, idea, body).then(
        () => setSaving("已保存"),
        (e) => setSaving(`没保存上：${message(e)}`),
      ),
    );
    return saveChain.current;
  };

  // ---- running

  const runChain = useRef<Promise<void>>(Promise.resolve());

  const markStale = () => {
    const ed = editorRef.current;
    if (!ed) return;
    const order = blocksOf(ed).map((b) => b.id);
    const stale = staleBlocks(order, (id) => store.get(id).cached);
    for (const id of order) if (store.get(id).stale !== stale.has(id)) store.set(id, { stale: stale.has(id) });
  };

  const run = (id: string): Promise<void> => {
    store.set(id, { queued: true, error: null });
    const p = runChain.current.then(async () => {
      const ed = editorRef.current;
      const block = ed && blocksOf(ed).find((b) => b.id === id);
      if (!block) {
        store.set(id, { queued: false });
        return;
      }
      await save();
      store.set(id, { queued: false, running: true, live: [] });
      try {
        const cached = await api.nbRun(root, idea, id, block.code);
        store.set(id, { cached, running: false, live: null, stale: false });
      } catch (e) {
        store.set(id, { running: false, live: null, error: message(e) });
      }
      markStale();
    });
    runChain.current = p.catch(() => {});
    return p;
  };

  const runAll = () => {
    const ed = editorRef.current;
    if (ed) for (const b of blocksOf(ed)) void run(b.id);
  };

  // a block waiting for the agent re-runs once its todo is done (once per todo)
  const rerun = useRef(new Set<string>());
  const rerunDelivered = () => {
    if (!alive.current) return;
    for (const id of store.ids()) {
      const s = store.get(id);
      const task = waitingTask(s.cached);
      if (!task || s.running || s.queued) continue;
      const row = store.tasks.get(task);
      const key = `${id}:${task}`;
      if (row?.status === "done" && !rerun.current.has(key)) {
        rerun.current.add(key);
        void run(id);
      }
    }
  };

  store.actions = { run: (id) => void run(id), openRun: props.onRun, openTask: props.onTask };

  // ---- the editor

  useEffect(() => {
    for (const [id, cached] of Object.entries(props.view.outputs)) store.set(id, { cached });
    const content = partsToDoc(props.view.doc.parts);
    lastBody.current = docToText(content);
    const editor = new Editor({
      element: host.current!,
      extensions: [
        StarterKit.configure({
          link: { openOnClick: false, autolink: false, linkOnPaste: false },
          underline: false,
          strike: false,
          undoRedo: { newGroupDelay: 600 },
          heading: { levels: [1, 2, 3, 4, 5, 6] },
        }),
        Placeholder.configure({
          placeholder: ({ node }) => (node.type.name === "heading" ? "标题" : "写点什么，或输入 / 插入命令块"),
          showOnlyCurrent: true,
        }),
        ArbBlock.configure({ store, data: () => data }),
        SlashMenu,
      ],
      content,
      editorProps: { attributes: { class: "nb-doc", spellcheck: "false", "aria-label": `${idea} 的 notebook` } },
      onUpdate: () => {
        if (saveTimer.current) clearTimeout(saveTimer.current);
        saveTimer.current = setTimeout(() => void save(), SAVE_DELAY);
        setSaving("");
      },
    });
    editorRef.current = editor;
    markStale();
    const un = onNotebook(
      (e) => {
        if (e.root !== root || e.idea !== idea) return;
        if (e.event === "stream") store.push(e.block, { kind: "stream", stderr: e.name === "stderr", text: e.text });
        else store.push(e.block, e.data);
      },
      (r, i, info) => {
        if (r === root && i === idea) setKernel(info);
      },
    );
    return () => {
      void save();
      editor.destroy();
      editorRef.current = null;
      void un.then((fs) => fs.forEach((f) => f()));
    };
    // the page re-mounts per notebook (key)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="nb-page">
      <KernelBar
        store={store}
        kernel={kernel}
        saving={saving}
        onRunAll={runAll}
        onInterrupt={() => void api.nbInterrupt(root, idea).catch(() => {})}
        onRestart={() =>
          void api.nbRestart(root, idea).then(
            () => setKernel(null),
            () => {},
          )
        }
      />
      <div className="nb-col">
        <div ref={host} />
        <div className="nb-hints">
          <span>
            <kbd>/</kbd> 插入命令块
          </span>
          <span>
            <kbd>⇧</kbd>
            <kbd>↩</kbd> 运行
          </span>
          <span>
            <kbd>⌥</kbd>
            <kbd>↩</kbd> 块里换行
          </span>
          <span>缺的命令会交给 agent，写好后自动重跑</span>
        </div>
      </div>
    </div>
  );
}

/** The Notebook tab: the app's notebook, or — for an old marimo one — migrating it. */
export function NotebookPage(props: {
  root: string;
  idea: string;
  board: Board;
  onRun: (run: string) => void;
  onTask: (task: string) => void;
  /** The embedded marimo notebook (until it is migrated). */
  marimo: () => JSX.Element;
}) {
  const [view, setView] = useState<NbView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  const [useMarimo, setUseMarimo] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setView(null);
    setError(null);
    api.nbLoad(props.root, props.idea).then(
      (v) => live && setView(v),
      (e) => live && setError(message(e)),
    );
    return () => {
      live = false;
    };
  }, [props.root, props.idea, n]);
  if (error)
    return (
      <div className="empty">
        <span className="big">notebook 打不开</span>
        <span className="err-box">{error}</span>
        <button type="button" className="ab-btn" onClick={() => setN((x) => x + 1)}>
          再试一次
        </button>
      </div>
    );
  if (!view) return <div className="empty">正在打开 notebook…</div>;
  if (view.exists)
    return <NotebookEditor key={`${props.root}\u0000${props.idea}`} {...props} view={view} />;
  if (view.notebook.marimo && useMarimo) return props.marimo();
  if (view.notebook.marimo)
    return (
      <div className="empty nb-migrate">
        <span className="big">这还是 marimo notebook</span>
        <span>
          迁移把 <code>mo.md(…)</code> 的 cell 变成文字，其他 cell 变成命令块（只读代码，不执行）。原来的 <code>.py</code> 留着。
        </span>
        {busy ? <span className="nb-muted">{busy}</span> : null}
        <span className="app-row">
          <button
            type="button"
            className="ab-btn ab-btn-primary"
            disabled={busy != null}
            onClick={() => {
              setBusy("正在迁移…");
              api.act(props.root, { kind: "migrate", name: props.idea }).then(
                () => {
                  setBusy(null);
                  setN((x) => x + 1);
                },
                (e) => setBusy(`迁移没成功：${message(e)}`),
              );
            }}
          >
            迁移到新 notebook
          </button>
          <button type="button" className="ab-btn" onClick={() => setUseMarimo(true)}>
            先用 marimo 打开
          </button>
        </span>
      </div>
    );
  return (
    <div className="empty">
      <span className="big">没有 notebook 文件</span>
      <code>{view.notebook.path}</code>
    </div>
  );
}
