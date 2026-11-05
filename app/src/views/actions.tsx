// The everyday actions behind the app's buttons and "⋯" menus, in one place: each opens a
// dialog (a form or a confirmation), then runs one `arb` command through the Rust side
// (`act`, src-tauri/src/actions.rs). Approving is not here: that is the Sign box.

import { createContext, useContext } from "react";
import type { Action, Board, IdeaView, ProjectSummary } from "../types";
import type { DialogSpec } from "../components/Dialog";
import type { MenuItem } from "../components/Menu";

export interface Actions {
  /** Open a dialog. */
  ask: (spec: DialogSpec) => void;
  /** Run an action in the project (or seed home) at root; throws arb's message. */
  act: (root: string, action: Action, done?: string) => Promise<void>;
  /** A terminal tab in this folder. */
  terminalAt: (cwd: string) => void;
  reveal: (root: string, path?: string) => void;
  /** Take a project off the list (files stay). */
  removeProject: (root: string) => Promise<void>;
  projectSettings: (root: string) => void;
  projects: ProjectSummary[];
}

export const ActionsContext = createContext<Actions | null>(null);

export function useActions(): Actions {
  const a = useContext(ActionsContext);
  if (!a) throw new Error("ActionsContext missing");
  return a;
}

const SLUG = "小写字母、数字和 -，例如 lr-warmup";

export function newIdea(a: Actions, b: Board, parent?: string): DialogSpec {
  const open = b.ideas.filter((i) => i.stage === "open");
  return {
    title: `新建 idea · ${b.project.name}`,
    body: "会建分支 idea/名字 和它的 worktree，以及这个 idea 的 notebook。",
    fields: [
      { key: "name", label: "名字", placeholder: "lr-warmup", required: true, hint: SLUG, mono: true, max: 64 },
      { key: "hypothesis", label: "假设", placeholder: "预期会怎样、为什么", required: true, multiline: true, max: 300 },
      { key: "metric", label: "指标", placeholder: "怎么判断成没成，例如 val/loss 在 10k 步时更低", required: true, max: 200 },
      ...(open.length > 0
        ? [
            {
              key: "parent",
              label: "从哪个 idea 分出",
              initial: parent ?? "",
              options: [{ value: "", label: "不从别的 idea 分出（主分支）" }, ...open.map((i) => ({ value: i.name, label: i.name }))],
            },
          ]
        : []),
    ],
    confirm: "新建",
    run: (v) =>
      a.act(
        b.project.root,
        { kind: "idea_new", name: v.name.trim(), hypothesis: v.hypothesis, metric: v.metric, parent: v.parent || null },
        `已新建 ${v.name.trim()}`,
      ),
  };
}

export function writeNote(a: Actions, root: string, idea: string, run: string | null): DialogSpec {
  return {
    title: run ? `给 ${run} 写笔记` : `给 ${idea} 写笔记`,
    fields: [{ key: "text", label: "笔记", required: true, multiline: true, max: 1000, placeholder: "看到了什么、下一步打算" }],
    confirm: "保存",
    run: (v) => a.act(root, { kind: "note", idea, text: v.text, run }, "笔记已保存"),
  };
}

export function closeIdea(a: Actions, root: string, i: IdeaView, merge: boolean): DialogSpec {
  return merge
    ? {
        title: `合并 ${i.name}`,
        body: `把 idea/${i.name} 合并进主分支，然后删掉它的 worktree 和分支。账本、run 和笔记都留着。`,
        confirm: "合并",
        run: () => a.act(root, { kind: "idea_close", name: i.name, merge: true }, `已合并 ${i.name}`),
      }
    : {
        title: `放弃 ${i.name}`,
        body: `给 worktree 存一个最终快照（打 tag，可以找回），然后删掉 worktree 和分支 idea/${i.name}。账本、run 和笔记都留着。`,
        confirm: "放弃",
        danger: true,
        run: () => a.act(root, { kind: "idea_close", name: i.name, merge: false }, `已放弃 ${i.name}`),
      };
}

export function rejectRun(a: Actions, root: string, run: string): DialogSpec {
  return {
    title: `拒绝 ${run}`,
    body: "不提交到集群。原因会记成这个 run 的笔记。",
    fields: [{ key: "reason", label: "原因", multiline: true, max: 1000, placeholder: "例如：lr 太大，先跑短的" }],
    confirm: "拒绝",
    danger: true,
    run: (v) => a.act(root, { kind: "reject", run, reason: v.reason || null }, `已拒绝 ${run}`),
  };
}

export function cancelRun(a: Actions, root: string, run: string, machine: string | null): DialogSpec {
  return {
    title: `取消 ${run}`,
    body: `在${machine ? ` ${machine} 上` : "集群上"} scancel 它所有的作业（要能 ssh 上去）。已经用掉的 SU 照算。`,
    confirm: "取消作业",
    danger: true,
    run: () => a.act(root, { kind: "cancel", run }, `已取消 ${run}`),
  };
}

export function removeProject(a: Actions, p: { root: string; name: string }): DialogSpec {
  return {
    title: `从列表移除 ${p.name}`,
    body: `只是不在 app 里显示了：${p.root} 里的文件、账本和 worktree 都不动。以后可以再「添加项目」加回来。`,
    confirm: "移除",
    danger: true,
    run: () => a.removeProject(p.root),
  };
}

export function newSeed(a: Actions, home: string): DialogSpec {
  return {
    title: "新的种子",
    body: "还没有仓库的想法：记在种子园里，带一个 notebook。想清楚了再种进项目。",
    fields: [
      { key: "name", label: "名字", required: true, hint: SLUG, mono: true, max: 64 },
      { key: "gist", label: "一句话", required: true, max: 300, placeholder: "这个想法是什么" },
      { key: "hypothesis", label: "假设", multiline: true, max: 300 },
      { key: "metric", label: "指标", max: 200 },
    ],
    confirm: "记下",
    run: (v) =>
      a.act(home, { kind: "seed_new", name: v.name.trim(), gist: v.gist, hypothesis: v.hypothesis || null, metric: v.metric || null }, `已记下 ${v.name.trim()}`),
  };
}

export function plantSeed(a: Actions, s: IdeaView): DialogSpec {
  const seed = s.name;
  const projects = a.projects.filter((p) => p.kind === "project" && !p.error);
  return {
    title: `把 ${seed} 种进项目`,
    body: projects.length
      ? "在那个项目里新建一个 idea（分支、worktree、notebook），notebook 开头链回这个种子。"
      : "还没有项目：先添加一个。",
    fields: projects.length
      ? [
          { key: "root", label: "项目", options: projects.map((p) => ({ value: p.root, label: p.name })) },
          { key: "as", label: "idea 的名字", placeholder: seed, hint: "默认和种子同名", mono: true, max: 64 },
          { key: "hypothesis", label: "假设", initial: s.hypothesis ?? "", required: true, multiline: true, max: 300 },
          { key: "metric", label: "指标", initial: s.metric ?? "", required: true, max: 200, placeholder: "例如 val/loss 在 10k 步时更低" },
        ]
      : [],
    confirm: "种下",
    run: async (v) => {
      if (!v.root) throw "先添加一个项目";
      await a.act(v.root, { kind: "seed_plant", name: seed, as_name: v.as || null, hypothesis: v.hypothesis || null, metric: v.metric || null }, `已种进 ${projects.find((p) => p.root === v.root)?.name}`);
    },
  };
}

export function projectMenu(a: Actions, p: { root: string; name: string }, b: Board | undefined, extra?: { newIdea?: boolean }): MenuItem[] {
  return [
    ...(extra?.newIdea !== false && b
      ? [{ key: "new", label: "新建 idea…", glyph: "plus" as const, run: () => a.ask(newIdea(a, b)) }]
      : []),
    { key: "term", label: "在这里开终端", glyph: "terminal", run: () => a.terminalAt(p.root) },
    { key: "reveal", label: "在 Finder 中显示", glyph: "folder", run: () => a.reveal(p.root) },
    { key: "settings", label: "项目设置", glyph: "panel", run: () => a.projectSettings(p.root) },
    { key: "remove", label: "从列表移除…", glyph: "trash", divider: true, run: () => a.ask(removeProject(a, p)) },
  ];
}

export function ideaMenu(a: Actions, root: string, b: Board, i: IdeaView): MenuItem[] {
  const open = i.stage === "open";
  const wt = i.worktree;
  return [
    ...(open ? [{ key: "note", label: "写笔记…", glyph: "pen" as const, run: () => a.ask(writeNote(a, root, i.name, null)) }] : []),
    ...(open ? [{ key: "fork", label: "从这里分出新 idea…", glyph: "plus" as const, run: () => a.ask(newIdea(a, b, i.name)) }] : []),
    ...(wt
      ? [
          { key: "term", label: "在 worktree 里开终端", glyph: "terminal" as const, divider: open, run: () => a.terminalAt(wt) },
          { key: "reveal", label: "在 Finder 中显示", glyph: "folder" as const, run: () => a.reveal(root, wt) },
        ]
      : []),
    ...(open
      ? [
          { key: "merge", label: "合并进主分支…", glyph: "merge" as const, divider: true, run: () => a.ask(closeIdea(a, root, i, true)) },
          { key: "drop", label: "放弃这个 idea…", glyph: "trash" as const, run: () => a.ask(closeIdea(a, root, i, false)) },
        ]
      : []),
  ];
}
