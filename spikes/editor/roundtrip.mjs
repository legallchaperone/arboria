// Spike: Arboria notebook file format = a restricted Markdown subset + ```arb {#id} fences.
// Question: does doc -> markdown -> doc lose anything? Is the markdown form canonical (stable)?
import { Schema } from "prosemirror-model";
import { MarkdownParser, MarkdownSerializer, MarkdownSerializerState, defaultMarkdownSerializer } from "prosemirror-markdown";

// fix 1 (library gap): prosemirror-markdown only escapes "1." / "+" at line start when a space follows,
// but CommonMark also reads "1." or "+" at the end of a line as a list item.
const origEsc = MarkdownSerializerState.prototype.esc;
MarkdownSerializerState.prototype.esc = function (str, startOfLine = false) {
  // fix 6: text right after a hard break also starts a line ("---" there would become a setext underline)
  startOfLine = startOfLine || /\n[ \t]*$/.test(this.out);
  str = origEsc.call(this, str, startOfLine);
  if (startOfLine) str = str.replace(/^(=+|-+)$/, "\\$1");
  if (startOfLine) str = str.replace(/^(\s*\d+)([.)])(?=\s|$)/, "$1\\$2").replace(/^\+$/, "\\+").replace(/^([-*])$/, "\\$1");
  return str;
};
import MarkdownIt from "markdown-it";
import fc from "fast-check";

// ---------------------------------------------------------------- schema (the subset)
const schema = new Schema({
  nodes: {
    doc: { content: "block+" },
    paragraph: { content: "inline*", group: "block" },
    heading: { attrs: { level: { default: 1 } }, content: "inline*", group: "block", defining: true },
    code_block: { content: "text*", marks: "", group: "block", code: true, defining: true, attrs: { params: { default: "" } } },
    bullet_list: { content: "list_item+", group: "block", attrs: { tight: { default: true } } },
    ordered_list: { content: "list_item+", group: "block", attrs: { order: { default: 1 }, tight: { default: true } } },
    list_item: { content: "block+", defining: true },
    arb_block: { content: "text*", marks: "", group: "block", code: true, defining: true, attrs: { id: { default: null } } },
    text: { group: "inline" },
    hard_break: { inline: true, group: "inline", selectable: false },
  },
  marks: {
    em: {}, strong: {},
    code: { excludes: "_" },
    link: { attrs: { href: {} }, inclusive: false },
  },
});

// ---------------------------------------------------------------- markdown <-> doc
const md = MarkdownIt("commonmark", { html: false }).disable(["image", "blockquote", "hr", "html_block", "html_inline", "code"]);
const ARB_INFO = /^arb(?:\s+\{#([a-z0-9]{4,8})\})?\s*$/;
md.core.ruler.push("arb_fences", (st) => { for (const t of st.tokens) if (t.type === "fence" && !/^arb(\s|$)/.test(t.info)) t.type = "code_block_fence"; });

const parser = new MarkdownParser(schema, md, {
  paragraph: { block: "paragraph" },
  heading: { block: "heading", getAttrs: (t) => ({ level: +t.tag.slice(1) }) },
  bullet_list: { block: "bullet_list", getAttrs: (_, ts, i) => ({ tight: listIsTight(ts, i) }) },
  ordered_list: { block: "ordered_list", getAttrs: (t, ts, i) => ({ order: +t.attrGet("start") || 1, tight: listIsTight(ts, i) }) },
  list_item: { block: "list_item" },
  // every fence becomes an arb block; a non-arb fence keeps its text but loses its language (reported below)
  fence: { block: "arb_block", getAttrs: (t) => ({ id: (ARB_INFO.exec(t.info) || [])[1] ?? null }), noCloseToken: true },

  hardbreak: { node: "hard_break" },
  softbreak: { node: "hard_break" },
  code_block_fence: { block: "code_block", getAttrs: (t) => ({ params: t.info || "" }), noCloseToken: true }, // fix 3: keep line breaks someone typed in another editor
  em: { mark: "em" }, strong: { mark: "strong" },
  link: { mark: "link", getAttrs: (t) => ({ href: t.attrGet("href") }) },
  code_inline: { mark: "code", noCloseToken: true },
});
function listIsTight(tokens, i) {
  while (++i < tokens.length) if (tokens[i].type != "list_item_open") return tokens[i].hidden;
  return false;
}

const d = defaultMarkdownSerializer;
const serializer = new MarkdownSerializer(
  {
    paragraph: d.nodes.paragraph, code_block: d.nodes.code_block,
    // fix 4: "## foo ##" — a trailing run of # in a heading is read as its closing sequence
    heading(state, node) {
      state.write(state.repeat("#", node.attrs.level) + " ");
      state.renderInline(node, false);
      state.out = state.out.replace(/(^|[ \t])(#+)$/, (m, sp, h) => sp + "\\" + h);
      state.closeBlock(node);
    },
    // fix 2: two lists in a row would merge into one on reload unless their markers differ (CommonMark)
    bullet_list(state, node, parent, index) {
      let k = 0; for (let i = index - 1; i >= 0 && parent.child(i).type === node.type; i--) k++;
      state.renderList(node, "  ", () => (k % 2 ? "-" : "*") + " ");
    },
    ordered_list(state, node, parent, index) {
      let k = 0; for (let i = index - 1; i >= 0 && parent.child(i).type === node.type; i--) k++;
      const start = node.attrs.order || 1, maxW = String(start + node.childCount - 1).length, space = state.repeat(" ", maxW + 2);
      state.renderList(node, space, (i) => { const n = String(start + i); return state.repeat(" ", maxW - n.length) + n + (k % 2 ? ") " : ". "); });
    }, list_item: d.nodes.list_item, text: d.nodes.text, hard_break: d.nodes.hard_break,
    arb_block(state, node) {
      // fence long enough to contain any backtick run in the content
      const runs = node.textContent.match(/`{3,}/g) || [];
      const fence = "`".repeat(Math.max(3, ...runs.map((r) => r.length + 1)));
      state.write(fence + "arb" + (node.attrs.id ? ` {#${node.attrs.id}}` : "") + "\n");
      state.text(node.textContent, false);
      state.ensureNewLine();
      state.write(fence);
      state.closeBlock(node);
    },
  },
  d.marks,
);

// fix 5: Markdown cannot hold leading/trailing spaces in a paragraph or heading, so the saved form drops them
function trimBlocks(doc) {
  const json = doc.toJSON();
  (function walk(n) {
    if ((n.type === "paragraph" || n.type === "heading") && n.content) {
      const c = n.content;
      if (c[0]?.type === "text") c[0].text = c[0].text.replace(/^[ \t]+/, "");
      const l = c[c.length - 1]; if (l?.type === "text") l.text = l.text.replace(/[ \t]+$/, "");
      n.content = c.filter((x) => x.type !== "text" || x.text);
      if (!n.content.length) delete n.content;
    } else (n.content || []).forEach(walk);
  })(json);
  return schema.nodeFromJSON(json);
}
const toMd = (doc) => serializer.serialize(trimBlocks(doc));
const fromMd = (text) => parser.parse(text);

// merge adjacent text nodes with equal marks so structurally equal docs compare equal
function norm(json) {
  if (!json.content) return json;
  const out = [];
  for (const c0 of json.content) {
    const c = norm(c0);
    const last = out[out.length - 1];
    if (c.type === "text" && last && last.type === "text" && JSON.stringify(last.marks || []) === JSON.stringify(c.marks || [])) last.text += c.text;
    else out.push({ ...c });
  }
  return { ...json, content: out };
}
const same = (a, b) => JSON.stringify(norm(a.toJSON())) === JSON.stringify(norm(b.toJSON()));

// ---------------------------------------------------------------- generators
const CJK = "的一是在了不和有大这主中人上为们地个用工时要动国产以我到他会作来分生对于学下级就年阶义发成部民可出能方进同行面说种过命度革而多子后自社加小机也经力线本电高量长党得实家定深法表着水理化争现所二起政三好十战无农使性前等反体合斗路图把结第里正新开论之物从当两些还天资事队批如应形想制心样干都向变关点育重其思与间内去因件日利相由压员气业代全组数果期导平各基或月毛然问比展那它最及外没看治提五解系林者米群头意只明四道马认次文通但条较克又公孔领军流入接席位情运器并飞原油放立题质指建区验活众很教决特此常石强极土少已根共直团统式转别造切九你取西持总料连任志观调七么山程百报更见必真保热委手改管处己将修支识病象几先老光专什六型具示复安带每东增则完风回南广劳轮科北打积车计给节做务被整联步类集号列温装即毫知轴研单色坚据速防史拉世设达尔场织历花受求传口断况采精金界品判参层止边清至万确究书术状厂须离再目海交权且儿青才证低越际八试规斯近注办布门铁需走议县兵固除般引齿千胜细影济白格效置推空配刀叶率述今选养德话查差半敌始片施响收华觉备名红续均药标记难存测士身紧液派准斤角降维板许破述技消底床田势端感往神便贺村构照容非搞亚磨族火段算适讲按值美态黄易彪服早班麦削信排台声该击素张密害侯草何树肥继右属市严径螺检左页抗苏显苦英快称坏移约巴材省黑武培著河帝仅针怎植京助升王眼她抓含苗副杂普谈围食射源例致酸旧却充足短划剂宣环落首尺波承粉践府鱼随考刻靠够满夫失包住促枝局菌杆周护岩师举曲春元超负砂封换太模贫减阳扬江析亩木言球朝医校古呢稻宋听唯输滑站另卫字鼓刚写刘微略范供阿块某功套友限项余倒卷创律雨让骨远帮初皮播优占死毒圈伟季训控激找叫云互跟裂粮粒母练塞钢顶策双留误础吸阻故寸盾晚丝女散焊功株亲院冷彻弹错散商视艺灭版烈零室轻血倍缺厘泵察绝富城冲喷壤简否柱李望盘磁雄似困巩益洲脱投送奴侧润盖挥距触星松送获兴独官混纪依未突架宽冬章湿偏纹吃执阀矿寨责熟稳夺硬价努翻奇甲预职评读背协损棉侵灰虽矛厚罗泥辟告卵箱掌氧恩爱停曾溶营终纲孟钱待尽俄缩沙退陈讨奋械载胞幼哪剥迫旋征槽倒握担仍呀鲜吧卡粗介钻逐弱脚怕盐末阴丰雾冠丙街莱贝辐肠付吉渗瑞惊顿挤秒悬姆烂森糖圣凹陶词迟蚕亿矩".slice(0, 400);
const word = fc.oneof(
  { weight: 4, arbitrary: fc.string({ unit: fc.constantFrom(..."abcxyzABC0123456789"), minLength: 1, maxLength: 8 }) },
  { weight: 4, arbitrary: fc.string({ unit: fc.constantFrom(...CJK), minLength: 1, maxLength: 6 }) },
  // the dangerous ones: markdown-special characters inside prose
  { weight: 3, arbitrary: fc.string({ unit: fc.constantFrom(..."*_`[]()#>-+.!|\\<&$~=:;，。（）“”"), minLength: 1, maxLength: 4 }) },
  { weight: 1, arbitrary: fc.constantFrom("1.", "-", "#", "##", "**", "__", "```", "[x]", "`code`", "a*b*c", "r001", "val/loss", "1e-4", "2.879") },
);
const plain = fc.array(word, { minLength: 1, maxLength: 8 }).map((ws) => ws.join(" "));
const marksArb = fc.subarray(["em", "strong", "code", "link"], { maxLength: 2 });
const inlineSeg = fc.record({ text: plain, marks: marksArb });
const inline = fc.array(inlineSeg, { minLength: 1, maxLength: 4 }).map((segs) => {
  const nodes = [];
  segs.forEach((s, i) => {
    let ms = s.marks;
    if (ms.includes("code")) ms = ["code"]; // code excludes all other marks
    const marks = ms.map((m) => (m === "link" ? schema.marks.link.create({ href: "https://example.com/" + i }) : schema.marks[m].create()));
    if (i > 0) nodes.push(schema.text(" "));
    nodes.push(schema.text(s.text, marks));
  });
  return nodes;
});
const para = inline.map((c) => schema.nodes.paragraph.create(null, c));
const heading = fc.tuple(fc.integer({ min: 1, max: 3 }), inline).map(([l, c]) => schema.nodes.heading.create({ level: l }, c));
const arbLine = fc.oneof(
  fc.constantFrom("/plot r1 r2 val/loss", 'lab.svd(r2, ask="前 20 个奇异值")', "lab.run(from_=r013, set={\"warmup\": 16000})", "# 学习率大于 1e-4 的画在一起", "runs = [r for r in lab.runs() if r.lr > 1e-4]", "x = 1  # 注释里有 ``` 三个反引号"),
  plain,
);
const arb = fc.tuple(fc.stringMatching(/^[a-z0-9]{4,6}$/), fc.array(arbLine, { minLength: 1, maxLength: 3 })).map(([id, lines]) =>
  schema.nodes.arb_block.create({ id }, schema.text(lines.join("\n"))));
const listOf = (type) => fc.array(para, { minLength: 1, maxLength: 3 }).map((ps) => schema.nodes[type].create(null, ps.map((p) => schema.nodes.list_item.create(null, p))));
const block = fc.oneof({ weight: 5, arbitrary: para }, { weight: 2, arbitrary: heading }, { weight: 3, arbitrary: arb }, { weight: 1, arbitrary: listOf("bullet_list") }, { weight: 1, arbitrary: listOf("ordered_list") });
const docArb = fc.array(block, { minLength: 1, maxLength: 10 }).map((bs) => schema.nodes.doc.create(null, bs));

// ---------------------------------------------------------------- properties
const results = {};
function prop(name, arbitrary, pred, numRuns) {
  try {
    fc.assert(fc.property(arbitrary, pred), { numRuns });
    results[name] = `ok (${numRuns} 份)`;
  } catch (e) {
    results[name] = "FAIL\n" + String(e.message).split("\n").slice(0, 12).join("\n");
  }
}
const N = +(process.env.N || 1000);
prop("doc → md → doc 结构不变", docArb, (doc) => same(fromMd(toMd(doc)), doc), N);
prop("md 是规范形式（再存一次不变）", docArb, (doc) => { const a = toMd(doc); return toMd(fromMd(a)) === a; }, N);
prop("命令块的原文一字不差", docArb, (doc) => {
  const want = []; doc.descendants((n) => { if (n.type.name === "arb_block") want.push([n.attrs.id, n.textContent]); });
  const got = []; fromMd(toMd(doc)).descendants((n) => { if (n.type.name === "arb_block") got.push([n.attrs.id, n.textContent]); });
  return JSON.stringify(want) === JSON.stringify(got);
}, N);
for (const [k, v] of Object.entries(results)) console.log(`${k}: ${v}`);

// ---------------------------------------------------------------- what happens to markdown outside the subset
const outside = {
  "表格": "| a | b |\n|---|---|\n| 1 | 2 |",
  "HTML": "前面 <b>加粗</b> 后面",
  "引用": "> 引用一段",
  "分隔线": "上面\n\n---\n\n下面",
  "图片": "![图](a.png)",
  "四级标题": "#### 四级",
  "普通代码块": "```python\nprint(1)\n```",
  "缩进代码": "    indented code",
  "公式": "能量 $E=mc^2$ 守恒",
};
console.log("\n子集之外的写法（外部编辑器里写的）打开再保存后：");
for (const [k, src] of Object.entries(outside)) console.log(`- ${k}: ${JSON.stringify(src)} → ${JSON.stringify(toMd(fromMd(src)))}`);

// one worked example, for the report
const sample = `---\n`;
const example = fromMd("# warmup-long\n\n前 2k 步的尖峰可能和 *warmup* 太短有关。\n\n```arb {#k3f9}\n/plot r1 r2 val/loss\n```\n\n尖峰在 `r002` 里基本消失了。\n");
console.log("\n示例往返：\n" + toMd(example));

// ---------------------------------------------------------------- arbitrary markdown typed elsewhere
const frag = fc.constantFrom("# ", "## ", "#### ", "- ", "* ", "+ ", "1. ", "2) ", "> ", "```", "```arb {#ab12}", "```python", "\n", "\n\n", "    ", "**", "*", "_", "`", "[", "](u)", "|", "---", "\\", "<b>", "$x$", "文字", "word", "r001", " ", "1.", "#");
prop("任意 markdown：打开再存一次后稳定", fc.array(frag, { maxLength: 30 }).map((a) => a.join("")), (src) => {
  const a = toMd(fromMd(src)); return toMd(fromMd(a)) === a;
}, N);
console.log("任意 markdown：打开再存一次后稳定: " + results["任意 markdown：打开再存一次后稳定"]);
