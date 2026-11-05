// "/" at the start of an empty line of text: a small menu (command block, headings,
// lists, quote). Built with DOM nodes and textContent only.

import { Extension, type Editor, type Range } from "@tiptap/core";
import Suggestion, { type SuggestionProps, type SuggestionKeyDownProps } from "@tiptap/suggestion";
import { PluginKey } from "@tiptap/pm/state";

interface Item {
  title: string;
  hint: string;
  keys: string;
  run: (ed: Editor) => void;
}

const ITEMS: Item[] = [
  { title: "命令块", hint: "/plot、lab.…", keys: "command cmd ml mingling 命令 代码 code arb", run: (ed) => ed.commands.insertArbBlock("") },
  { title: "标题", hint: "##", keys: "heading h2 bt biaoti 标题", run: (ed) => ed.chain().setNode("heading", { level: 2 }).run() },
  { title: "小标题", hint: "###", keys: "heading h3 xbt xiaobiaoti 小标题", run: (ed) => ed.chain().setNode("heading", { level: 3 }).run() },
  { title: "列表", hint: "-", keys: "list bullet lb liebiao 列表", run: (ed) => ed.chain().toggleBulletList().run() },
  { title: "编号列表", hint: "1.", keys: "ordered number bh bianhao 编号", run: (ed) => ed.chain().toggleOrderedList().run() },
  { title: "引用", hint: ">", keys: "quote yy yinyong 引用", run: (ed) => ed.chain().toggleBlockquote().run() },
];

export function filterItems(q: string): Item[] {
  const s = q.toLowerCase();
  return ITEMS.filter((it) => !s || it.title.includes(s) || it.keys.split(" ").some((k) => k.startsWith(s)));
}

function renderer() {
  let el: HTMLDivElement | null = null;
  let items: Item[] = [];
  let index = 0;
  let command: ((it: Item) => void) | null = null;
  const place = (props: SuggestionProps<Item>) => {
    const r = props.clientRect?.();
    if (!el || !r) return;
    el.style.left = `${Math.round(r.left)}px`;
    el.style.top = `${Math.round(r.bottom + 4)}px`;
  };
  const paint = () => {
    if (!el) return;
    el.textContent = "";
    if (!items.length) {
      const empty = document.createElement("div");
      empty.className = "nb-menu-empty";
      empty.textContent = "没有匹配的";
      el.append(empty);
      return;
    }
    items.forEach((it, i) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "nb-menu-item" + (i === index ? " is-active" : "");
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(i === index));
      const t = document.createElement("span");
      t.textContent = it.title;
      const h = document.createElement("span");
      h.className = "nb-menu-hint";
      h.textContent = it.hint;
      row.append(t, h);
      row.addEventListener("mousedown", (e) => {
        e.preventDefault();
        command?.(it);
      });
      row.addEventListener("mousemove", () => {
        if (index !== i) {
          index = i;
          paint();
        }
      });
      el!.append(row);
    });
  };
  return {
    onStart(props: SuggestionProps<Item>) {
      el = document.createElement("div");
      el.className = "nb-slash";
      el.setAttribute("role", "listbox");
      items = props.items;
      index = 0;
      command = props.command;
      document.body.append(el);
      place(props);
      paint();
    },
    onUpdate(props: SuggestionProps<Item>) {
      items = props.items;
      command = props.command;
      index = Math.min(index, Math.max(0, items.length - 1));
      place(props);
      paint();
    },
    onKeyDown({ event }: SuggestionKeyDownProps) {
      const n = Math.max(1, items.length);
      if (event.key === "ArrowDown") {
        index = (index + 1) % n;
        paint();
        return true;
      }
      if (event.key === "ArrowUp") {
        index = (index - 1 + n) % n;
        paint();
        return true;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        if (items[index]) command?.(items[index]);
        return true;
      }
      if (event.key === "Escape") {
        el?.remove();
        el = null;
        return true;
      }
      return false;
    },
    onExit() {
      el?.remove();
      el = null;
    },
  };
}

export const SlashMenu = Extension.create({
  name: "slashMenu",
  addProseMirrorPlugins() {
    return [
      Suggestion<Item, Item>({
        editor: this.editor,
        pluginKey: new PluginKey("slashMenu"),
        char: "/",
        startOfLine: true,
        allow: ({ state, range }: { state: Editor["state"]; range: Range }) => state.doc.resolve(range.from).parent.type.name === "paragraph",
        items: ({ query }: { query: string }) => filterItems(query),
        command: ({ editor, range, props }: { editor: Editor; range: Range; props: Item }) => {
          editor.chain().focus().deleteRange(range).run();
          props.run(editor);
        },
        render: renderer,
      }),
    ];
  },
});
