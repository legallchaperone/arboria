// The notebook's text ↔ the editor's document (ADR 0005). The file is the human's: the
// restricted markdown subset (headings, paragraphs, lists, quotes, bold / italic, inline
// code, links; other code fences are kept as code) comes in as TipTap nodes and goes out
// in the same canonical shape `nbdoc::render` writes, so an untouched notebook saves to
// the same bytes. Anything outside the subset stays as literal text — never HTML.

import type { NbPart } from "../types";

/** TipTap's JSON (`JSONContent`), kept local so this file needs no editor import. */
export interface Node {
  type: string;
  attrs?: Record<string, unknown>;
  content?: Node[];
  marks?: Mark[];
  text?: string;
}
export interface Mark {
  type: string;
  attrs?: Record<string, unknown>;
}

// ---------------------------------------------------------------- inline

const PUNCT = /[!-/:-@[-`{-~]/;
const WORD = /[\p{L}\p{N}_]/u;

function textNode(text: string, marks: Mark[]): Node {
  return marks.length ? { type: "text", text, marks: marks.map((m) => ({ ...m })) } : { type: "text", text };
}

/** Inline markdown → text nodes (with `hardBreak` for line breaks). */
export function parseInline(src: string, marks: Mark[] = []): Node[] {
  const out: Node[] = [];
  let buf = "";
  const flush = () => {
    if (buf) out.push(textNode(buf, marks));
    buf = "";
  };
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "\\" && i + 1 < src.length && PUNCT.test(src[i + 1])) {
      buf += src[i + 1];
      i += 2;
      continue;
    }
    if (c === "\n") {
      flush();
      out.push({ type: "hardBreak" });
      i += 1;
      continue;
    }
    if (c === "`") {
      let n = 0;
      while (src[i + n] === "`") n++;
      const fence = "`".repeat(n);
      let j = src.indexOf(fence, i + n);
      while (j !== -1 && src[j + n] === "`") j = src.indexOf(fence, j + n + 1);
      if (j !== -1 && !src.slice(i + n, j).includes("\n")) {
        let code = src.slice(i + n, j);
        if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim()) code = code.slice(1, -1);
        flush();
        if (code) out.push(textNode(code, [...marks, { type: "code" }]));
        i = j + n;
        continue;
      }
      buf += fence;
      i += n;
      continue;
    }
    if (c === "*" || c === "_") {
      const double = src[i + 1] === c;
      const d = double ? c + c : c;
      const prev = i > 0 ? src[i - 1] : " ";
      const opens = !/\s/.test(src[i + d.length] ?? " ") && (c === "*" || !WORD.test(prev));
      if (opens) {
        const close = findClose(src, i + d.length, d, c);
        if (close !== -1) {
          flush();
          const mark: Mark = { type: double ? "bold" : "italic" };
          out.push(...parseInline(src.slice(i + d.length, close), [...marks, mark]));
          i = close + d.length;
          continue;
        }
      }
      buf += d;
      i += d.length;
      continue;
    }
    if (c === "[") {
      const m = /^\[([^\]\n]*)\]\(([^()\s]*)\)/.exec(src.slice(i));
      if (m && m[1]) {
        flush();
        out.push(...parseInline(m[1], [...marks, { type: "link", attrs: { href: m[2] } }]));
        i += m[0].length;
        continue;
      }
    }
    buf += c;
    i += 1;
  }
  flush();
  return out;
}

function findClose(src: string, from: number, d: string, c: string): number {
  let j = from;
  while (j < src.length) {
    if (src[j] === "\\") {
      j += 2;
      continue;
    }
    if (src[j] === "`") {
      // skip a code span
      let n = 0;
      while (src[j + n] === "`") n++;
      const k = src.indexOf("`".repeat(n), j + n);
      j = k === -1 ? j + n : k + n;
      continue;
    }
    if (src.startsWith(d, j) && !/\s/.test(src[j - 1] ?? " ")) {
      // `***`: the close of `**` is its last two
      if (d.length === 2) while (src[j + 2] === c) j++;
      const after = src[j + d.length] ?? " ";
      const okAfter = c === "*" || !WORD.test(after);
      // `**` inside `*…*` is not the close of the single one
      const single = d.length === 1 && src[j + 1] === c;
      if (okAfter && !single) return j;
      if (single) {
        j += 2;
        continue;
      }
    }
    j += 1;
  }
  return -1;
}

function escapeText(t: string, lineStart: boolean): string {
  const needsBracket = /\[[^\]]*\]\(/.test(t);
  let out = "";
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (c === "\\" && i + 1 < t.length && PUNCT.test(t[i + 1])) out += "\\\\";
    else if (c === "\\" && i + 1 === t.length) out += "\\\\";
    else if (c === "*" || c === "`") out += "\\" + c;
    else if (c === "_" && (!WORD.test(t[i - 1] ?? " ") || !WORD.test(t[i + 1] ?? " "))) out += "\\_";
    else if (c === "[" && needsBracket) out += "\\[";
    else out += c;
  }
  return lineStart ? escapeLineStart(out) : out;
}

/** A line that would read as a block construct gets a backslash. */
function escapeLineStart(line: string): string {
  if (/^(#{1,6}(\s|$)|>|[-+*](\s|$)|`{3}|~{3})/.test(line)) return "\\" + line;
  const m = /^(\d+)([.)])(\s|$)/.exec(line);
  if (m) return m[1] + "\\" + line.slice(m[1].length);
  if (/^(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) return "\\" + line;
  return line;
}

const ORDER = ["link", "bold", "italic"];

/** Text nodes → inline markdown. */
export function renderInline(nodes: Node[] | undefined): string {
  let out = "";
  let open: Mark[] = [];
  let atLineStart = true;
  const same = (a: Mark, b: Mark) => a.type === b.type && JSON.stringify(a.attrs ?? {}) === JSON.stringify(b.attrs ?? {});
  const closeMark = (m: Mark) => (m.type === "link" ? `](${String(m.attrs?.href ?? "")})` : m.type === "bold" ? "**" : "*");
  const openMark = (m: Mark) => (m.type === "link" ? "[" : m.type === "bold" ? "**" : "*");
  const closeTo = (keep: number) => {
    while (open.length > keep) out += closeMark(open.pop()!);
  };
  for (const n of nodes ?? []) {
    if (n.type === "hardBreak") {
      closeTo(0);
      out += "\n";
      atLineStart = true;
      continue;
    }
    if (n.type !== "text" || !n.text) continue;
    const marks = (n.marks ?? []).filter((m) => ORDER.includes(m.type)).sort((a, b) => ORDER.indexOf(a.type) - ORDER.indexOf(b.type));
    const isCode = (n.marks ?? []).some((m) => m.type === "code");
    let keep = 0;
    while (keep < open.length && keep < marks.length && same(open[keep], marks[keep])) keep++;
    closeTo(keep);
    for (const m of marks.slice(keep)) {
      out += openMark(m);
      open.push(m);
    }
    if (isCode) {
      const longest = Math.max(0, ...(n.text.match(/`+/g) ?? []).map((r) => r.length));
      const f = "`".repeat(longest + 1);
      const pad = n.text.includes("`") || (n.text.startsWith(" ") && n.text.endsWith(" ") && n.text.trim()) ? " " : "";
      out += f + pad + n.text + pad + f;
    } else {
      const lines = n.text.split("\n");
      out += lines.map((l, k) => escapeText(l, (k > 0 || atLineStart) && open.length === 0)).join("\n");
    }
    atLineStart = false;
  }
  closeTo(0);
  return out;
}

// ---------------------------------------------------------------- blocks

const FENCE = /^(\s{0,3})(`{3,}|~{3,})(.*)$/;
const HEADING = /^(#{1,6})(?:\s+(.*?))?\s*#*\s*$/;
const ITEM = /^(\s*)([-+*]|\d{1,9}[.)])(\s+|$)(.*)$/;
const RULE = /^\s{0,3}(-{3,}|\*{3,}|_{3,})\s*$/;

const startsBlock = (l: string) => FENCE.test(l) || /^#{1,6}(\s|$)/.test(l) || /^\s{0,3}>/.test(l) || ITEM.test(l) || RULE.test(l);

const para = (text: string): Node => {
  const content = parseInline(text);
  return content.length ? { type: "paragraph", content } : { type: "paragraph" };
};

/** Markdown (one text part) → block nodes. */
export function parseBlocks(src: string): Node[] {
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  const out: Node[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    const f = FENCE.exec(line);
    if (f) {
      const ch = f[2][0];
      const n = f[2].length;
      let j = i + 1;
      while (j < lines.length && !new RegExp(`^\\s{0,3}\\${ch}{${n},}\\s*$`).test(lines[j])) j++;
      const body = lines.slice(i + 1, j).join("\n");
      const lang = f[3].trim();
      const node: Node = { type: "codeBlock", attrs: { language: lang || null } };
      if (body) node.content = [{ type: "text", text: body }];
      out.push(node);
      i = j + 1;
      continue;
    }
    const h = HEADING.exec(line);
    if (h && /^#{1,6}(\s|$)/.test(line)) {
      const content = parseInline(h[2] ?? "");
      out.push({ type: "heading", attrs: { level: h[1].length }, ...(content.length ? { content } : {}) });
      i++;
      continue;
    }
    if (RULE.test(line)) {
      out.push({ type: "horizontalRule" });
      i++;
      continue;
    }
    if (/^\s{0,3}>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s{0,3}>/.test(lines[i])) body.push(lines[i++].replace(/^\s{0,3}> ?/, ""));
      const inner = parseBlocks(body.join("\n"));
      out.push({ type: "blockquote", content: inner.length ? inner : [{ type: "paragraph" }] });
      continue;
    }
    if (ITEM.test(line)) {
      const [list, next] = parseList(lines, i);
      out.push(list);
      i = next;
      continue;
    }
    const body: string[] = [line];
    i++;
    while (i < lines.length && lines[i].trim() && !startsBlock(lines[i])) body.push(lines[i++]);
    out.push(para(body.join("\n")));
  }
  return out;
}

function parseList(lines: string[], start: number): [Node, number] {
  const first = ITEM.exec(lines[start])!;
  const indent = first[1].length;
  const ordered = /\d/.test(first[2]);
  const items: Node[] = [];
  let i = start;
  while (i < lines.length) {
    const m = ITEM.exec(lines[i]);
    if (!m || m[1].length !== indent || /\d/.test(m[2]) !== ordered) break;
    const contentIndent = indent + m[2].length + Math.max(1, Math.min(m[3].length, 4));
    const body: string[] = [m[4]];
    i++;
    // continuation: lines indented under the item (blank lines inside if more follows)
    while (i < lines.length) {
      const l = lines[i];
      if (!l.trim()) {
        const nxt = lines[i + 1];
        if (nxt !== undefined && nxt.trim() && indentOf(nxt) >= contentIndent) {
          body.push("");
          i++;
          continue;
        }
        break;
      }
      if (indentOf(l) >= contentIndent || (indentOf(l) > indent && ITEM.test(l))) {
        body.push(l.slice(Math.min(contentIndent, indentOf(l))));
        i++;
        continue;
      }
      if (ITEM.test(l) || startsBlock(l)) break;
      body.push(l); // a lazy continuation of the item's paragraph
      i++;
    }
    const inner = parseBlocks(body.join("\n"));
    if (!inner.length || inner[0].type !== "paragraph") inner.unshift({ type: "paragraph" });
    items.push({ type: "listItem", content: inner });
    if (i < lines.length && !lines[i].trim()) {
      // a blank line between items keeps the list going
      const nxt = lines[i + 1];
      const nm = nxt !== undefined ? ITEM.exec(nxt) : null;
      if (nm && nm[1].length === indent && /\d/.test(nm[2]) === ordered) i++;
      else break;
    }
  }
  const node: Node = { type: ordered ? "orderedList" : "bulletList", content: items };
  if (ordered) {
    const s = parseInt(first[2], 10);
    node.attrs = { start: s };
  }
  return [node, i];
}

const indentOf = (l: string) => l.length - l.trimStart().length;

const indentLines = (text: string, pad: string, firstPad: string) =>
  text
    .split("\n")
    .map((l, k) => (k === 0 ? firstPad + l : l ? pad + l : l))
    .join("\n");

/** One block node → markdown. */
export function renderBlock(n: Node): string {
  switch (n.type) {
    case "paragraph":
      return renderInline(n.content);
    case "heading": {
      const level = Math.min(6, Math.max(1, Number(n.attrs?.level ?? 1)));
      const text = renderInline(n.content).replace(/\n/g, " ");
      return "#".repeat(level) + (text ? " " + text : "");
    }
    case "horizontalRule":
      return "---";
    case "codeBlock": {
      const text = (n.content ?? []).map((t) => t.text ?? "").join("");
      const longest = Math.max(0, ...text.split("\n").map((l) => /^`*/.exec(l)![0].length));
      const f = "`".repeat(Math.max(3, longest + 1));
      return f + String(n.attrs?.language ?? "") + "\n" + (text ? text + "\n" : "") + f;
    }
    case "blockquote": {
      const inner = renderBlocks(n.content ?? []);
      return inner
        .split("\n")
        .map((l) => (l ? "> " + l : ">"))
        .join("\n");
    }
    case "bulletList":
    case "orderedList": {
      const start = Number(n.attrs?.start ?? 1);
      return (n.content ?? [])
        .map((item, k) => {
          const marker = n.type === "bulletList" ? "- " : `${start + k}. `;
          const kids = item.content ?? [];
          const body = kids
            .map((c, j) => (j === 0 ? "" : c.type === "bulletList" || c.type === "orderedList" ? "\n" : "\n\n") + renderBlock(c))
            .join("");
          return indentLines(body, " ".repeat(marker.length), marker);
        })
        .join("\n");
    }
    default:
      // anything else the editor might hold: its text
      return renderInline(n.content);
  }
}

/** Block nodes → markdown, separated by blank lines (empty paragraphs dropped). */
export function renderBlocks(nodes: Node[]): string {
  return nodes
    .map(renderBlock)
    .filter((t) => t.trim() !== "")
    .join("\n\n");
}

// ---------------------------------------------------------------- the notebook

export const BLOCK = "arbBlock";

/** The file's parts (`nbdoc::Doc.parts`) → the editor's document. */
export function partsToDoc(parts: NbPart[]): Node {
  const content: Node[] = [];
  for (const p of parts) {
    if (p.kind === "text") content.push(...parseBlocks(p.text));
    else content.push({ type: BLOCK, attrs: { id: p.id }, ...(p.code ? { content: [{ type: "text", text: p.code }] } : {}) });
  }
  if (!content.length || content[content.length - 1].type !== "paragraph") content.push({ type: "paragraph" });
  return { type: "doc", content };
}

/** The fence `nbdoc::render` writes around a block's code. */
export function blockFence(id: string, code: string): string {
  const longest = Math.max(0, ...code.split("\n").map((l) => /^`*/.exec(l)![0].length));
  const f = "`".repeat(Math.max(2, longest) + 1);
  return f + (id ? `arb {#${id}}` : "arb") + "\n" + (code ? code + "\n" : "") + f;
}

/** The editor's document → the file's body (no frontmatter: the app keeps the file's). */
export function docToText(doc: Node): string {
  const parts: string[] = [];
  let run: Node[] = [];
  const flush = () => {
    const t = renderBlocks(run).replace(/^\n+|\n+$/g, "");
    if (t) parts.push(t);
    run = [];
  };
  for (const n of doc.content ?? []) {
    if (n.type === BLOCK) {
      flush();
      const code = (n.content ?? []).map((t) => t.text ?? "").join("");
      parts.push(blockFence(String(n.attrs?.id ?? ""), code));
    } else run.push(n);
  }
  flush();
  return parts.map((p) => p + "\n").join("\n");
}

/** A new block id (`nbdoc::valid_id`: 4–12 of a-z0-9-), not in `taken`. */
export function newBlockId(taken: Set<string>): string {
  const abc = "abcdefghijklmnopqrstuvwxyz0123456789";
  for (;;) {
    let id = "";
    for (let k = 0; k < 4; k++) id += abc[Math.floor(Math.random() * abc.length)];
    if (!taken.has(id)) return id;
  }
}
