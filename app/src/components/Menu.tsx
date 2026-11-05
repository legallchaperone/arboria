// A small dropdown (the terminal's "+" and "⋯"): rows of the same height, an icon, the
// label, a hint (shortcut, 未安装) on the right; separators between groups. Esc or a click
// outside closes it; ↑↓ ⏎ work.

import { useEffect, useRef, useState, type ReactNode } from "react";
import type { GlyphName } from "../lib/format";
import { Glyph } from "./Glyph";

export interface MenuItem {
  key: string;
  label: string;
  glyph?: GlyphName;
  hint?: string;
  disabled?: boolean;
  checked?: boolean;
  /** A separator above this item. */
  divider?: boolean;
  run: () => void;
}

export function MenuButton(props: { label: ReactNode; ariaLabel: string; items: MenuItem[]; align?: "left" | "right" }) {
  const [open, setOpen] = useState(false);
  const [sel, setSel] = useState(0);
  const box = useRef<HTMLSpanElement>(null);
  const usable = props.items.map((it, i) => (it.disabled ? -1 : i)).filter((i) => i >= 0);
  useEffect(() => {
    if (!open) return;
    setSel(usable[0] ?? 0);
    const off = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", off);
    return () => window.removeEventListener("mousedown", off);
  }, [open]);
  const pick = (it: MenuItem | undefined) => {
    if (!it || it.disabled) return;
    setOpen(false);
    it.run();
  };
  return (
    <span className="app-menu" ref={box}>
      <button
        type="button"
        className={"app-icon-btn" + (open ? " is-on" : "")}
        aria-label={props.ariaLabel}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (!open) return;
          const at = usable.indexOf(sel);
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setSel(usable[Math.min(usable.length - 1, at + 1)]);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setSel(usable[Math.max(0, at - 1)]);
          } else if (e.key === "Enter") {
            e.preventDefault();
            pick(props.items[sel]);
          } else if (e.key === "Escape") {
            e.stopPropagation();
            setOpen(false);
          }
        }}
      >
        {props.label}
      </button>
      {open && (
        <div className={`app-menu-list is-${props.align ?? "right"}`} role="menu">
          {props.items.map((it, i) => (
            <div key={it.key}>
              {it.divider && <div className="app-menu-sep" role="separator" />}
              <div
                role="menuitem"
                aria-disabled={it.disabled || undefined}
                className={"app-menu-item body" + (i === sel ? " is-sel" : "") + (it.disabled ? " is-off" : "")}
                onMouseEnter={() => !it.disabled && setSel(i)}
                onClick={() => pick(it)}
              >
                <span className="app-menu-glyph">{it.checked ? <Glyph name="check" /> : it.glyph ? <Glyph name={it.glyph} /> : null}</span>
                <span className="app-menu-label">{it.label}</span>
                {it.hint && <span className="app-menu-hint small">{it.hint}</span>}
              </div>
            </div>
          ))}
        </div>
      )}
    </span>
  );
}
