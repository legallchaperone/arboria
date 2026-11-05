// TipTap node `arbBlock`: a command block. Its node view embeds CodeMirror 6 (the
// ProseMirror "embedded CodeMirror" pattern) and renders the block's outputs below it
// with React. The node holds only the id and the code; results live in the store.

import { Node as TNode, type Editor } from "@tiptap/core";
import { Plugin, PluginKey, Selection, TextSelection } from "@tiptap/pm/state";
import type { Node as PMNode } from "@tiptap/pm/model";
import type { EditorView as PMView, NodeView } from "@tiptap/pm/view";
import { undo, redo } from "@tiptap/pm/history";
import { EditorView as CMView, keymap, drawSelection, placeholder, type ViewUpdate } from "@codemirror/view";
import { EditorState as CMState, Prec } from "@codemirror/state";
import { autocompletion, closeBrackets, closeBracketsKeymap, completionStatus } from "@codemirror/autocomplete";
import { defaultKeymap, insertNewline, indentWithTab } from "@codemirror/commands";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { cmTheme, commandSource, highlighter, optionExtras, type CompletionData } from "./cm";
import { BlockOutputs } from "./Outputs";
import { BLOCK, newBlockId } from "./markdown";
import type { NbStore } from "./store";

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    arbBlock: {
      /** A command block below the cursor (replacing an empty paragraph). */
      insertArbBlock: (text?: string) => ReturnType;
    };
  }
}

export interface ArbBlockOptions {
  store: NbStore | null;
  /** A getter: TipTap's configure() deep-copies plain option objects. */
  data: () => CompletionData;
}

function blockIds(doc: PMNode): Set<string> {
  const ids = new Set<string>();
  doc.descendants((n) => {
    if (n.type.name === BLOCK && n.attrs.id) ids.add(String(n.attrs.id));
  });
  return ids;
}

class ArbBlockView implements NodeView {
  dom: HTMLElement;
  private cm: CMView;
  private updating = false;
  private root: Root;
  private out: HTMLElement;

  constructor(
    private node: PMNode,
    private view: PMView,
    private getPos: () => number | undefined,
    private opts: ArbBlockOptions,
  ) {
    this.dom = document.createElement("div");
    this.dom.className = "arb-block";
    const cell = document.createElement("div");
    cell.className = "arb-cell";
    this.out = document.createElement("div");
    this.out.className = "arb-out";
    this.out.contentEditable = "false";

    this.cm = new CMView({
      state: CMState.create({
        doc: node.textContent,
        extensions: [
          Prec.high(keymap.of(this.keys())),
          keymap.of([...closeBracketsKeymap, ...defaultKeymap, indentWithTab]),
          drawSelection(),
          closeBrackets(),
          highlighter,
          autocompletion({
            override: [commandSource(opts.data)],
            icons: false,
            addToOptions: optionExtras,
            optionClass: (c) => "cm-arb-opt" + ((c as { arbDesc?: string }).arbDesc ? " has-desc" : ""),
            tooltipClass: () => "cm-arb-tip",
          }),
          placeholder("/命令 或 lab.…，Shift ↩ 运行"),
          cmTheme,
          CMView.updateListener.of((u) => this.forward(u)),
        ],
      }),
    });

    const run = document.createElement("button");
    run.className = "arb-run";
    run.type = "button";
    run.title = "运行（Shift ↩）";
    run.setAttribute("aria-label", "运行这一块");
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", "M5 3.6v8.8a.5.5 0 0 0 .76.43l7-4.4a.5.5 0 0 0 0-.86l-7-4.4A.5.5 0 0 0 5 3.6Z");
    path.setAttribute("fill", "currentColor");
    svg.append(path);
    run.append(svg);
    run.addEventListener("mousedown", (e) => e.preventDefault());
    run.addEventListener("click", () => this.run());

    cell.append(this.cm.dom, run);
    this.dom.append(cell, this.out);
    this.dom.dataset.block = this.id;
    this.root = createRoot(this.out);
    this.render();
  }

  get id(): string {
    return String(this.node.attrs.id ?? "");
  }

  private render() {
    if (!this.opts.store) return;
    this.root.render(createElement(BlockOutputs, { store: this.opts.store, id: this.id, code: this.node.textContent }));
  }

  private run() {
    this.opts.store?.actions.run(this.id);
  }

  // ---- ProseMirror ↔ CodeMirror

  private forward(update: ViewUpdate) {
    if (this.updating || !this.cm.hasFocus) return;
    const pos = this.getPos();
    if (pos == null) return;
    let offset = pos + 1;
    const { main } = update.state.selection;
    const selFrom = offset + main.from;
    const selTo = offset + main.to;
    const pmSel = this.view.state.selection;
    if (update.docChanged || pmSel.from !== selFrom || pmSel.to !== selTo) {
      const tr = this.view.state.tr;
      update.changes.iterChanges((fromA, toA, fromB, toB, text) => {
        if (text.length) tr.replaceWith(offset + fromA, offset + toA, this.view.state.schema.text(text.toString()));
        else tr.delete(offset + fromA, offset + toA);
        offset += toB - fromB - (toA - fromA);
      });
      tr.setSelection(TextSelection.create(tr.doc, selFrom, selTo));
      this.view.dispatch(tr);
    }
  }

  setSelection(anchor: number, head: number) {
    this.cm.focus();
    this.updating = true;
    this.cm.dispatch({ selection: { anchor, head } });
    this.updating = false;
  }

  update(node: PMNode) {
    if (node.type !== this.node.type) return false;
    const idChanged = node.attrs.id !== this.node.attrs.id;
    const codeChanged = node.textContent !== this.node.textContent;
    this.node = node;
    if (idChanged) this.dom.dataset.block = this.id;
    if (idChanged || codeChanged) this.render();
    if (this.updating) return true;
    const next = node.textContent;
    const cur = this.cm.state.doc.toString();
    if (next !== cur) {
      let start = 0;
      let curEnd = cur.length;
      let nextEnd = next.length;
      while (start < curEnd && cur.charCodeAt(start) === next.charCodeAt(start)) ++start;
      while (curEnd > start && nextEnd > start && cur.charCodeAt(curEnd - 1) === next.charCodeAt(nextEnd - 1)) {
        curEnd--;
        nextEnd--;
      }
      this.updating = true;
      this.cm.dispatch({ changes: { from: start, to: curEnd, insert: next.slice(start, nextEnd) } });
      this.updating = false;
    }
    return true;
  }

  selectNode() {
    this.dom.classList.add("is-selected");
    this.cm.focus();
  }
  deselectNode() {
    this.dom.classList.remove("is-selected");
  }
  stopEvent() {
    return true;
  }
  ignoreMutation() {
    return true;
  }
  destroy() {
    this.cm.destroy();
    const root = this.root;
    // not while React renders (a node view can go away during a React update)
    setTimeout(() => root.unmount(), 0);
  }

  // ---- keys

  private keys() {
    const view = this.view;
    return [
      { key: "ArrowUp", run: () => this.escape("line", -1) },
      { key: "ArrowLeft", run: () => this.escape("char", -1) },
      { key: "ArrowDown", run: () => this.escape("line", 1) },
      { key: "ArrowRight", run: () => this.escape("char", 1) },
      { key: "Shift-Enter", run: () => (this.run(), true) },
      { key: "Mod-Enter", run: () => (this.run(), true) },
      { key: "Enter", run: () => this.enter() },
      { key: "Alt-Enter", run: insertNewline },
      { key: "Backspace", run: () => this.backspaceEmpty() },
      { key: "Mod-z", run: () => undo(view.state, view.dispatch) },
      { key: "Shift-Mod-z", run: () => redo(view.state, view.dispatch) },
      { key: "Mod-y", run: () => redo(view.state, view.dispatch) },
    ];
  }

  private escape(unit: "line" | "char", dir: -1 | 1): boolean {
    const { state } = this.cm;
    let range: { from: number; to: number } = state.selection.main;
    if (!state.selection.main.empty) return false;
    if (completionStatus(state) === "active") return false;
    if (unit === "line") range = state.doc.lineAt(state.selection.main.head);
    if (dir < 0 ? range.from > 0 : range.to < state.doc.length) return false;
    const pos = this.getPos();
    if (pos == null) return false;
    const target = pos + (dir < 0 ? 0 : this.node.nodeSize);
    const sel = Selection.near(this.view.state.doc.resolve(target), dir);
    this.view.dispatch(this.view.state.tr.setSelection(sel).scrollIntoView());
    this.view.focus();
    return true;
  }

  /** Enter: a new line inside Python (after `:`, an open bracket, or in a multi-line
   * block); otherwise leave the block for the text below. */
  private enter(): boolean {
    const { state } = this.cm;
    if (completionStatus(state) === "active") return false;
    const head = state.selection.main.head;
    const line = state.doc.lineAt(head);
    const before = state.sliceDoc(0, head);
    const open = (before.match(/[([{]/g) ?? []).length > (before.match(/[)\]}]/g) ?? []).length;
    const isSlash = /^\s*\//.test(line.text);
    if (open || /[:,\\]\s*$/.test(line.text.slice(0, head - line.from)) || (state.doc.lines > 1 && !isSlash) || head < line.to) {
      return insertNewline(this.cm);
    }
    return this.exitBelow();
  }

  private exitBelow(): boolean {
    const pos = this.getPos();
    if (pos == null) return false;
    const after = pos + this.node.nodeSize;
    const { state } = this.view;
    const next = state.doc.nodeAt(after);
    let tr = state.tr;
    if (!(next && next.type.name === "paragraph" && next.content.size === 0)) {
      tr = tr.insert(after, state.schema.nodes.paragraph.create());
    }
    tr.setSelection(TextSelection.create(tr.doc, after + 1)).scrollIntoView();
    this.view.dispatch(tr);
    this.view.focus();
    return true;
  }

  private backspaceEmpty(): boolean {
    if (this.cm.state.doc.length > 0) return false;
    const pos = this.getPos();
    if (pos == null) return false;
    const { state } = this.view;
    const tr = state.tr.replaceWith(pos, pos + this.node.nodeSize, state.schema.nodes.paragraph.create());
    tr.setSelection(TextSelection.create(tr.doc, pos + 1));
    this.view.dispatch(tr);
    this.view.focus();
    return true;
  }
}

export const ArbBlock = TNode.create<ArbBlockOptions>({
  name: BLOCK,
  group: "block",
  content: "text*",
  marks: "",
  code: true,
  defining: true,
  isolating: true,

  addOptions() {
    return { store: null, data: () => ({ commands: [], runs: [] }) };
  },

  addAttributes() {
    return { id: { default: null, keepOnSplit: false } };
  },

  parseHTML() {
    return [{ tag: "pre[data-arb]", preserveWhitespace: "full" }];
  },
  renderHTML() {
    return ["pre", { "data-arb": "" }, 0];
  },

  addNodeView() {
    return ({ node, view, getPos }) => new ArbBlockView(node, view, getPos, this.options);
  },

  addCommands() {
    return {
      insertArbBlock:
        (text = "") =>
        ({ tr, dispatch, state }) => {
          const { $from } = state.selection;
          const node = this.type.create({ id: newBlockId(blockIds(state.doc)) }, text ? state.schema.text(text) : null);
          if (dispatch) {
            const para = $from.parent;
            const empty = para.type.name === "paragraph" && para.content.size === 0;
            const at = empty ? $from.before() : $from.after();
            if (empty) tr.replaceWith(at, at + para.nodeSize, node);
            else tr.insert(at, node);
            tr.setSelection(TextSelection.create(tr.doc, at + 1 + text.length));
          }
          return true;
        },
    };
  },

  addKeyboardShortcuts() {
    const arrow =
      (dir: "left" | "right" | "up" | "down") =>
      ({ editor }: { editor: Editor }) => {
        const { state, view } = editor;
        if (!(state.selection.empty && view.endOfTextblock(dir))) return false;
        const side = dir === "left" || dir === "up" ? -1 : 1;
        const $head = state.selection.$head;
        const next = Selection.near(state.doc.resolve(side > 0 ? $head.after() : $head.before()), side);
        if (next.$head && next.$head.parent.type.name === BLOCK) {
          view.dispatch(state.tr.setSelection(next));
          return true;
        }
        return false;
      };
    return { ArrowLeft: arrow("left"), ArrowRight: arrow("right"), ArrowUp: arrow("up"), ArrowDown: arrow("down") };
  },

  addProseMirrorPlugins() {
    // every block has an id, unique in the notebook (pasting a block copies its id)
    return [
      new Plugin({
        key: new PluginKey("arbBlockIds"),
        appendTransaction: (trs, _old, state) => {
          if (!trs.some((t) => t.docChanged)) return null;
          const seen = new Set<string>();
          const fix: { pos: number; node: PMNode }[] = [];
          state.doc.descendants((n, pos) => {
            if (n.type.name !== BLOCK) return;
            const id = n.attrs.id ? String(n.attrs.id) : "";
            if (!id || seen.has(id)) fix.push({ pos, node: n });
            else seen.add(id);
          });
          if (!fix.length) return null;
          const tr = state.tr;
          const taken = blockIds(state.doc);
          for (const f of fix) {
            const id = newBlockId(taken);
            taken.add(id);
            tr.setNodeMarkup(f.pos, null, { ...f.node.attrs, id });
          }
          tr.setMeta("addToHistory", false);
          return tr;
        },
      }),
    ];
  },
});
