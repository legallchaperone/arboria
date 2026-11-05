// The design system's 16×16 stroke glyphs (Arboria 设计系统).
import type { JSX } from "react";
import type { GlyphName, Tone } from "../lib/format";

const PATHS: Record<GlyphName, JSX.Element> = {
  check: <path d="M3.5 8.5l3 3 6-7" />,
  play: <path className="solid" d="M5 3.2v9.6l7.6-4.8z" />,
  touch: (
    <>
      <path d="M3.6 12A5.6 5.6 0 0 1 2.9 9.2V8a5.1 5.1 0 0 1 10.2 0v1.6" />
      <path d="M6.1 13.6c-.6-1.4-.9-3.1-.9-5.1a2.8 2.8 0 0 1 5.6 0c0 1.9.3 3.4.9 4.7" />
      <path d="M8 8.4c0 2.1.3 3.8.9 5.2" />
    </>
  ),
  cross: <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />,
  dash: <path d="M4.5 8h7" />,
  circle: <circle cx="8" cy="8" r="4.75" />,
  chevron: <path d="M4.5 6.5L8 10l3.5-3.5" />,
  terminal: <path d="M3 4.5l3.5 3.5L3 11.5M8 11.5h5" />,
  plus: <path d="M8 3.5v9M3.5 8h9" />,
  bell: (
    <>
      <path d="M4.5 11V7.5a3.5 3.5 0 0 1 7 0V11l1 1.5h-9z" />
      <path d="M6.8 14a1.4 1.4 0 0 0 2.4 0" />
    </>
  ),
};

const TONE: Record<Tone, string | undefined> = {
  ink: undefined,
  accent: "ab-accent",
  warn: "ab-warn",
  bad: "ab-bad",
  ok: "ab-ok",
  muted: "ab-muted",
};

export function Glyph({ name, tone = "ink" }: { name: GlyphName; tone?: Tone }) {
  return (
    <svg className={["ab-glyph", TONE[tone]].filter(Boolean).join(" ")} viewBox="0 0 16 16" aria-hidden="true">
      {PATHS[name]}
    </svg>
  );
}
