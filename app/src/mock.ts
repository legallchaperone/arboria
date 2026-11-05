// The app over the fixture (src/fixtures/board.json), for screenshots and work on the
// pages without the Rust side: `npm run dev`, then /mock.html (?screen=overview|idea|card|
// palette|terminal). Not part of the built app (vite builds index.html only).

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import fixture from "./fixtures/board.json";
import type { Board } from "./types";
import { clock } from "./App";

const now = Date.parse(fixture.now);
const started = Date.now();
clock.now = () => now + (Date.now() - started);

const boards = fixture.boards as unknown as Board[];
const internals = () => (window as unknown as { __TAURI_INTERNALS__: { runCallback: (id: number, d: unknown) => void } }).__TAURI_INTERNALS__;

mockWindows("main");
mockIPC(
  async (cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case "platform":
        return fixture.platform;
      case "projects":
        return fixture.projects;
      case "board":
        return boards.find((b) => b.project.root === a.root);
      case "card_shown":
        return boards.find((b) => b.project.root === a.root)?.cards.find((c) => c.run === a.run);
      case "approve":
        await new Promise((r) => setTimeout(r, 600));
        return { run: a.run, status: "queued", decision: "d031", jobs: ["4400014"] };
      case "helper_status":
        return fixture.helper;
      case "pty_open": {
        const ch = a.onData as { id: number };
        const text =
          "\x1b[2m~/lm\x1b[0m $ arb status --brief\r\n" +
          "lm · 2 个进行中的 idea · 1 个 run 等你签字（r014）\r\n" +
          "看板：arb ui\r\n\x1b[2m~/lm\x1b[0m $ ";
        setTimeout(() => internals().runCallback(ch.id, { index: 0, message: Array.from(new TextEncoder().encode(text)) }), 50);
        return 1;
      }
      case "pty_write":
      case "pty_resize":
      case "pty_close":
      case "open_notebook":
        return null;
      default:
        throw `mock: ${cmd} is not mocked`;
    }
  },
  { shouldMockEvents: true },
);

await import("./main");

// a screen to start on (the screenshot script)
const screen = new URLSearchParams(location.search).get("screen");
const press = (key: string, mod = false) =>
  window.dispatchEvent(new KeyboardEvent("keydown", { key, metaKey: mod, ctrlKey: mod }));
const click = (text: string, sel = "button") =>
  [...document.querySelectorAll<HTMLElement>(sel)].find((e) => e.textContent?.trim() === text)?.click();
const clickOption = (text: string) =>
  [...document.querySelectorAll<HTMLElement>("[role=option]")].find((e) => e.textContent?.includes(text))?.click();
setTimeout(() => {
  if (screen === "overview") click("全局");
  if (screen === "idea") click("warmup-long");
  if (screen === "card") clickOption("r014 待签字");
  if (screen === "palette") press("k", true);
  if (screen === "ideas") press("p", true);
  if (screen === "terminal") press("`", true);
}, 300);
