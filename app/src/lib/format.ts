// Small formatting helpers (pure).

/** `2026-11-02T21:24:00Z` → epoch ms (NaN when not a stamp). */
export function parseUtc(ts: string | null | undefined): number {
  if (!ts) return NaN;
  return Date.parse(ts);
}

/** "刚才", "12 分钟前", "2 小时前", "3 天前", then "10-04" (same year) or "2025-10-04". */
export function ago(ts: string | null | undefined, now: number): string {
  const t = parseUtc(ts);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return "刚才";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  if (d < 14) return `${d} 天前`;
  return day(ts!, now);
}

/** `MM-DD`, with the year when it is not `now`'s. */
export function day(ts: string, now: number): string {
  const t = new Date(parseUtc(ts));
  const n = new Date(now);
  const md = `${String(t.getUTCMonth() + 1).padStart(2, "0")}-${String(t.getUTCDate()).padStart(2, "0")}`;
  return t.getUTCFullYear() === n.getUTCFullYear() ? md : `${t.getUTCFullYear()}-${md}`;
}

/** SU like the card: 2 places, trailing zeros dropped (`48`, `11.86`, `0.6`). */
export function su(x: number | null | undefined): string {
  if (x === null || x === undefined || Number.isNaN(x)) return "–";
  const r = Math.round(x * 100) / 100;
  return Number.isInteger(r) ? String(r) : String(r);
}

/** What a run status reads as. */
export const STATUS_TEXT: Record<string, string> = {
  draft: "草稿",
  awaiting_approval: "待签字",
  approved: "已批准",
  queued: "排队中",
  running: "运行中",
  succeeded: "完成",
  failed: "失败",
  cancelled: "已取消",
  rejected: "已拒绝",
};

/** The glyph and tone of a run status. */
export function statusTone(status: string): { glyph: GlyphName; tone: Tone } {
  switch (status) {
    case "succeeded":
      return { glyph: "check", tone: "ink" };
    case "running":
    case "queued":
    case "approved":
      return { glyph: "play", tone: "accent" };
    case "awaiting_approval":
      return { glyph: "touch", tone: "warn" };
    case "failed":
      return { glyph: "cross", tone: "bad" };
    case "cancelled":
      return { glyph: "cross", tone: "muted" };
    default:
      return { glyph: "dash", tone: "muted" };
  }
}

export type Tone = "ink" | "accent" | "warn" | "bad" | "ok" | "muted";
export type GlyphName =
  | "check"
  | "play"
  | "touch"
  | "cross"
  | "dash"
  | "circle"
  | "chevron"
  | "terminal"
  | "plus"
  | "bell"
  | "sparkle"
  | "cube"
  | "server"
  | "more"
  | "folder"
  | "link"
  | "panel"
  | "search"
  | "trash"
  | "pen"
  | "merge";

/** "一句话的第一行" cut to n chars. */
export function clip(s: string | null | undefined, n: number): string {
  if (!s) return "";
  const first = s.split("\n")[0];
  const chars = [...first];
  return chars.length > n || s.includes("\n") ? chars.slice(0, n - 1).join("") + "…" : first;
}
