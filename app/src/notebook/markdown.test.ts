import { describe, expect, it } from "vitest";
import { docToText, parseBlocks, parseInline, partsToDoc, renderBlocks, renderInline } from "./markdown";
import type { NbPart } from "../types";

const round = (md: string) => renderBlocks(parseBlocks(md));

describe("notebook markdown", () => {
  it("keeps the subset as written", () => {
    for (const md of [
      "# warmup-long",
      "假设：把 warmup 拉长，`val/loss` 至少低 0.02。",
      "**粗体**、*斜体*、***两者***，还有 [链接](https://example.com)。",
      "两行\n在一段里",
      "- 一\n- 二\n  - 二点一\n- 三",
      "1. 第一\n2. 第二",
      "3. 从三开始\n4. 四",
      "> 引用\n>\n> 第二段",
      "## 种子里的思路\n\n> 想法\n> 第二行",
      "```python\nprint(1)\n```",
      "````\n```\n````",
      "---",
      "snake_case_name 和 a*b 不是格式",
      "`` a`b ``",
    ]) {
      expect(round(md), md).toBe(md.replace("a*b", "a\\*b"));
    }
  });

  it("reads marks and never HTML", () => {
    expect(parseInline("<b>x</b>")).toEqual([{ type: "text", text: "<b>x</b>" }]);
    expect(parseInline("`<script>`")).toEqual([{ type: "text", text: "<script>", marks: [{ type: "code" }] }]);
    expect(parseInline("**a** b")[0]).toEqual({ type: "text", text: "a", marks: [{ type: "bold" }] });
    expect(parseInline("[x](y)")[0].marks).toEqual([{ type: "link", attrs: { href: "y" } }]);
  });

  it("escapes what would read as markdown", () => {
    const t = (text: string) => renderInline([{ type: "text", text }]);
    expect(t("# not a heading")).toBe("\\# not a heading");
    expect(t("- not a list")).toBe("\\- not a list");
    expect(t("1. not a list")).toBe("1\\. not a list");
    expect(t("2*3")).toBe("2\\*3");
    expect(t("`tick`")).toBe("\\`tick\\`");
    expect(t("[a](b)")).toBe("\\[a](b)");
    expect(t("[a] plain")).toBe("[a] plain");
    for (const s of ["# x", "- y", "1. z", "2*3", "`q`", "[a](b)", "a\\b", "_x_"]) {
      expect(renderInline(parseInline(t(s)))).toBe(t(s));
      expect(parseInline(t(s)).map((n) => n.text).join("")).toBe(s);
    }
  });

  it("round-trips the notebook arb writes", () => {
    const parts: NbPart[] = [
      { kind: "text", text: "# exp1\n\n假设：更长的 warmup\n\n指标：val/loss\n\n来自种子 `s`。" },
      { kind: "text", text: "## 种子里的思路\n\n> 先想清楚\n>\n> 再说" },
      { kind: "text", text: "## 记录\n\n先看看已有的 run：" },
      { kind: "block", id: "k3f9", code: "/runs" },
      { kind: "block", id: "ab12", code: "x = 1\n```\ny = 2" },
      { kind: "block", id: "zz00", code: "" },
    ];
    const text = docToText(partsToDoc(parts));
    expect(text).toBe(
      "# exp1\n\n假设：更长的 warmup\n\n指标：val/loss\n\n来自种子 `s`。\n\n## 种子里的思路\n\n> 先想清楚\n>\n> 再说\n\n## 记录\n\n先看看已有的 run：\n\n" +
        "```arb {#k3f9}\n/runs\n```\n\n````arb {#ab12}\nx = 1\n```\ny = 2\n````\n\n```arb {#zz00}\n```\n",
    );
  });

  it("keeps list items with several paragraphs", () => {
    expect(round("- 一\n\n  续\n- 二")).toBe("- 一\n\n  续\n- 二");
  });
});
