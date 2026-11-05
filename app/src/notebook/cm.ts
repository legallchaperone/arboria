// CodeMirror pieces for command blocks: a small highlighter for `/cmd`, `lab.x(...)`, run
// ids and literals, completion from the project's command library and runs, the theme.

import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { RangeSetBuilder } from "@codemirror/state";
import type { Completion, CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import type { CommandEntry } from "../types";

// ---------------------------------------------------------------- highlighter

const TOKEN = new RegExp(
  [
    String.raw`(?<comment>#.*$)`,
    String.raw`(?<slash>^\s*\/[A-Za-z_]\w*)`,
    String.raw`(?<lab>\blab\.)(?<labname>[A-Za-z_]\w*)?`,
    String.raw`(?<str>"(?:[^"\\]|\\.)*"?|'(?:[^'\\]|\\.)*'?)`,
    String.raw`(?<run>\br\d{3,}\b)`,
    String.raw`(?<metric>\b(?:val|train|eval|test)\/[\w./-]+)`,
    String.raw`(?<kw>\b(?:def|return|for|in|if|else|elif|import|from|as|with|while|lambda|None|True|False|and|or|not)\b)`,
    String.raw`(?<key>\b[A-Za-z_]\w*(?==(?!=)))`,
    String.raw`(?<num>\b\d+(?:\.\d+)?(?:e-?\d+)?\b)`,
  ].join("|"),
  "gm",
);
const KINDS = ["comment", "slash", "lab", "labname", "str", "run", "metric", "kw", "key", "num"] as const;
const marks = Object.fromEntries(KINDS.map((k) => [k, Decoration.mark({ class: "tk-" + k })]));

function highlight(view: EditorView): DecorationSet {
  const b = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    let pos = from;
    while (pos <= to) {
      const line = view.state.doc.lineAt(pos);
      TOKEN.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = TOKEN.exec(line.text))) {
        if (m[0] === "") {
          TOKEN.lastIndex++;
          continue;
        }
        const g = m.groups!;
        const at = line.from + m.index;
        if (g.lab) {
          b.add(at, at + 4, marks.lab);
          if (g.labname) b.add(at + 4, at + 4 + g.labname.length, marks.labname);
          continue;
        }
        if (g.slash) {
          const lead = m[0].length - m[0].trimStart().length;
          b.add(at + lead, at + m[0].length, marks.slash);
          continue;
        }
        const kind = KINDS.find((k) => g[k] !== undefined);
        if (kind) b.add(at, at + m[0].length, marks[kind]);
      }
      pos = line.to + 1;
    }
  }
  return b.finish();
}

export const highlighter = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = highlight(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) this.decorations = highlight(u.view);
    }
  },
  { decorations: (v) => v.decorations },
);

// ---------------------------------------------------------------- completion

/** What completion offers; filled by the notebook page (and refreshed). */
export interface CompletionData {
  commands: CommandEntry[];
  runs: { id: string; status: string; change: string }[];
}

type Opt = Completion & { arbDesc?: string; arbKind?: string };

const slashSig = (sig: string) => sig.replace(/^[^(]*\(|\)$/g, "").replace(/\*+|=None|"/g, "").replace(/,\s*/g, " ");

function commandOption(c: CommandEntry, slash: boolean): Opt {
  return {
    label: slash ? "/" + c.name : c.name,
    detail: c.signature ? (slash ? slashSig(c.signature) : c.signature.replace(/^[^(]*/, "")) : undefined,
    arbDesc: c.description,
    arbKind: c.kind,
    type: "function",
    apply: slash
      ? "/" + c.name + " "
      : (view, _c, from, to) => {
          const next = view.state.sliceDoc(to, to + 1);
          const ins = next === "(" ? c.name : c.name + "()";
          view.dispatch({ changes: { from, to, insert: ins }, selection: { anchor: from + c.name.length + 1 } });
        },
  };
}

export function commandSource(get: () => CompletionData) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const data = get();
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = line.text.slice(0, ctx.pos - line.from);
    if (/^\s*#/.test(before)) return null;
    let m: RegExpExecArray | null;
    if ((m = /^\s*\/(\w*)$/.exec(before))) {
      return {
        from: ctx.pos - m[1].length - 1,
        options: data.commands.map((c, i) => ({ ...commandOption(c, true), boost: -i })),
        validFor: /^\/\w*$/,
      };
    }
    if ((m = /\blab\.(\w*)$/.exec(before))) {
      return {
        from: ctx.pos - m[1].length,
        options: data.commands.map((c, i) => ({ ...commandOption(c, false), boost: -i })),
        validFor: /^\w*$/,
      };
    }
    const inCommand = /^\s*\//.test(line.text) || /\blab\.\w+\(/.test(before);
    if (!inCommand) return null;
    const word = ctx.matchBefore(/r\d*$/);
    if (!word || (word.from === word.to && !ctx.explicit)) return null;
    return {
      from: word.from,
      options: data.runs.map((r) => ({ label: r.id, detail: r.change, arbDesc: r.status, arbKind: "run", type: "variable" }) as Opt),
      validFor: /^r\d*$/,
    };
  };
}

/** The description line and kind tag of a completion option. */
export const optionExtras = [
  {
    position: 80,
    render(c: Completion) {
      const d = (c as Opt).arbDesc;
      if (!d) return null;
      const el = document.createElement("div");
      el.className = "cm-arb-desc";
      el.textContent = d;
      return el;
    },
  },
  {
    position: 90,
    render(c: Completion) {
      const k = (c as Opt).arbKind;
      if (!k) return null;
      const el = document.createElement("span");
      el.className = "cm-arb-kind";
      el.dataset.kind = k;
      el.textContent = k;
      return el;
    },
  },
];

// ---------------------------------------------------------------- theme

export const cmTheme = EditorView.theme({
  "&": { backgroundColor: "transparent", color: "var(--ink)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.65", overflowX: "auto" },
  ".cm-content": { padding: "9px 0", caretColor: "var(--accent)", fontFeatureSettings: '"calt" 1, "cv01" 1' },
  ".cm-line": { padding: "0 44px 0 14px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "1.5px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--accent-fill-hover) !important",
  },
  ".cm-placeholder": { color: "var(--ink-faint)", fontFamily: "var(--font-mono)" },
});
