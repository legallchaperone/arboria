import { describe, expect, it } from "vitest";
import fixture from "../fixtures/board.json";
import type { ProjectSummary } from "../types";
import { entries, group } from "./inbox";

const projects = fixture.projects as unknown as ProjectSummary[];

describe("inbox", () => {
  it("groups by kind in the design's order, newest first", () => {
    const gs = group(entries(projects));
    expect(gs.map((g) => g.label)).toEqual(["要签字", "失败", "跑完了", "agent 写好了"]);
    expect(gs[0].items[0].run).toBe("r014");
  });
  it("prefixes the project only when several have something", () => {
    const one = entries(projects);
    expect(one[0].meta.startsWith("lm · ")).toBe(false); // the home has nothing waiting: one project
    const two = entries([projects[0], { ...projects[0], root: "/x", name: "other" }]);
    expect(two[0].meta.startsWith("lm · ")).toBe(true);
    expect(new Set(two.map((e) => e.key)).size).toBe(two.length);
  });
  it("leaves out projects that could not be read", () => {
    const broken = { ...projects[0], root: "/gone", error: "no ledger", inbox: [] };
    expect(entries([projects[0], broken]).length).toBe(projects[0].inbox.length);
  });
});
