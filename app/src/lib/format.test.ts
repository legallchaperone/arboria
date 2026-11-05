import { describe, expect, it } from "vitest";
import { ago, clip, day, statusTone, su } from "./format";

const now = Date.parse("2026-11-04T12:00:00Z");

describe("format", () => {
  it("says how long ago", () => {
    expect(ago("2026-11-04T11:59:30Z", now)).toBe("刚才");
    expect(ago("2026-11-04T11:48:00Z", now)).toBe("12 分钟前");
    expect(ago("2026-11-04T10:00:00Z", now)).toBe("2 小时前");
    expect(ago("2026-11-03T09:40:00Z", now)).toBe("1 天前");
    expect(ago("2026-10-01T00:00:00Z", now)).toBe("10-01");
    expect(ago("2025-10-01T00:00:00Z", now)).toBe("2025-10-01");
    expect(ago(null, now)).toBe("");
    expect(day("2026-11-04T23:59:00Z", now)).toBe("11-04");
  });
  it("formats SU like the card", () => {
    expect(su(48)).toBe("48");
    expect(su(11.857)).toBe("11.86");
    expect(su(0.6)).toBe("0.6");
    expect(su(null)).toBe("–");
  });
  it("clips to one line", () => {
    expect(clip("abc", 5)).toBe("abc");
    expect(clip("abcdef", 4)).toBe("abc…");
    expect(clip("a\nb", 10)).toBe("a…");
    expect(clip("尖峰在 r002 里基本消失了", 6)).toBe("尖峰在 r…");
  });
  it("gives each status a glyph", () => {
    expect(statusTone("awaiting_approval")).toEqual({ glyph: "touch", tone: "warn" });
    expect(statusTone("running").tone).toBe("accent");
    expect(statusTone("failed").glyph).toBe("cross");
  });
});
