// The app over the fixture (src/fixtures/board.json), for screenshots and work on the
// pages without the Rust side: `npm run dev`, then /mock.html (?screen=overview|idea|card|
// palette|terminal). Not part of the built app (vite builds index.html only).

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import fixture from "./fixtures/board.json";
import type { Board } from "./types";
import { clock } from "./App";

const now = Date.parse(fixture.now);
const MACHINE = {
  name: "deltaai",
  kind: "slurm",
  host: "dtai-login",
  account: "bjki-dtai-gh",
  arch: "aarch64",
  gpus_per_node: 4,
  cpus_per_gpu: 72,
  mem_per_gpu_gb: 110.898,
  default_partition: "ghx4",
  max_segments: 4,
  billing_per_su: 1000,
  balance_cmd: "accounts",
  prelude: [],
  env_activate: ["source /work/hdd/bjki/$USER/venvs/lm/bin/activate"],
  partitions: [
    { name: "ghx4", max_walltime: "48h", charge: 1 },
    { name: "ghx4-interactive", max_walltime: "2h", charge: 2 },
  ],
  paths: { runs: "/projects/bjki/$USER/runs", data: "/work/nvme/bjki/$USER/data", cache: "/work/nvme/bjki/$USER/cache", ckpt: "/work/hdd/bjki/$USER/ckpt", out: "/work/hdd/bjki/$USER/out" },
  root: null,
};
const CLUSTERS = `[deltaai]
kind = "slurm"
host = "dtai-login.delta.ncsa.illinois.edu"
account = "bjki-dtai-gh"
gpus_per_node = 4
cpus_per_gpu = 72
mem_per_gpu_gb = 110.898
default_partition = "ghx4"
billing_per_su = 1000

[deltaai.partitions.ghx4]
max_walltime = "48h"

[lab-box]
kind = "slurm"
host = "lab-box"
account = "<your account>"
`;
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
        return { ...fixture.platform, home: fixture.projects.find((p: { kind: string }) => p.kind === "home")?.root ?? "/Users/you/arboria" };
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
      case "clusters_read":
      case "clusters_check": {
        const text = (a.text as string | undefined) ?? CLUSTERS;
        const bad = /<[a-z]/.test(text);
        return {
          path: "/Users/you/.config/arboria/clusters.toml",
          exists: true,
          text,
          syntax_error: null,
          profiles: [
            { name: "deltaai", kind: "slurm", host: "dtai-login.delta.ncsa.illinois.edu", error: null },
            { name: "lab-box", kind: "slurm", host: "lab-box", error: bad ? "clusters.toml: [lab-box]: account is a <placeholder>; fill it in" : null },
          ],
        };
      }
      case "terminal_launchers":
        return [
          { id: "shell", label: "新终端", installed: true, group: null },
          { id: "claude", label: "Claude Code", installed: true, group: "agent" },
          { id: "codex", label: "Codex", installed: false, group: "agent" },
          { id: "ssh:deltaai", label: "登录 deltaai", installed: true, group: "ssh" },
        ];
      case "clone_dir":
        return (a.set as string | null) ?? "/Users/you/code";
      case "settings_machines":
        return {
          path: "/Users/you/.config/arboria/clusters.toml",
          syntax_error: null,
          machines: [
            { form: MACHINE, error: null },
            { form: { ...MACHINE, name: "lab-box", host: "lab-box", account: "<your account>", partitions: [{ name: "gpu", max_walltime: "24h", charge: 1 }], default_partition: "gpu" }, error: "clusters.toml: [lab-box] account is a <placeholder>; fill it in" },
          ],
        };
      case "settings_blank_machine":
        return { ...MACHINE, name: "", host: "", account: "", partitions: [{ name: "gpu", max_walltime: "24h", charge: 1 }], default_partition: "gpu" };
      case "settings_project":
        return {
          root: a.root,
          path: `${a.root}/arboria.toml`,
          project_id: "lm-3f2a",
          error: null,
          machines: ["deltaai", "lab-box"],
          override_styles: ["hydra", "argparse", "tyro"],
          form: { default_machine: "deltaai", total_su: 400, wandb_entity: "you", wandb_project: "lm", run_command: "python train.py", override_style: "hydra", observation_minutes: 10, fail_patterns: ["CUDA out of memory", "torch\\.OutOfMemoryError"] },
        };
      case "app_settings":
        return { notifications: true, clone_dir: "/Users/you/code", helper: fixture.helper };
      case "pick_project":
      case "pick_folder":
      case "open_notebook":
        return "data:text/html;charset=utf-8," + encodeURIComponent(
          '<body style="font:14px -apple-system,sans-serif;margin:0;background:#fff;color:#1a211e"><div style="padding:24px 48px"><div style="color:#888;font-size:12px">marimo · notebook.py</div><h2>warmup-long</h2><p>r011 在 1.2k 到 3k 步之间有三次尖峰，都出现在 lr 爬到 2e-4 左右的时候。</p><pre style="background:#f3f4f3;padding:12px;border-radius:6px">lab.plot(lab.runs(), "val/loss")</pre><svg width="560" height="160"><path d="M0 20 L60 60 L80 40 L120 90 L200 120 L320 135 L560 140" fill="none" stroke="#0072b2" stroke-width="2"/><path d="M0 30 L100 90 L200 118 L320 132 L560 142" fill="none" stroke="#cb5900" stroke-width="2"/></svg></div></body>',
        );
      case "notification_status":
        return "authorized";
      case "notification_test":
        return "/Applications/Arboria.app/Contents/MacOS/arboria";
      case "act":
        return (a.action as { kind: string }).kind === "logs"
          ? { output: "step 3000 | loss 2.912 | lr 2.0e-4\nstep 3100 | loss 2.905 | lr 2.0e-4\nstep 3200 | loss 2.899 | lr 2.0e-4" }
          : { output: "ok" };
      case "remove_project":
        return true;
      case "reveal":
      case "close_notebook":
      case "pty_write":
      case "pty_resize":
      case "pty_close":

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
const nav = (label: string) =>
  [...document.querySelectorAll<HTMLElement>(".side .nav")].find((e) => e.querySelector(".lbl")?.textContent === label)?.click();
setTimeout(() => {
  if (screen === "inbox") document.querySelector<HTMLElement>(".row")?.click();
  if (screen === "project") nav("lm");
  if (screen === "idea") {
    nav("warmup-long");
    setTimeout(() => [...document.querySelectorAll<HTMLElement>(".row")].find((e) => e.textContent?.includes("r012"))?.click(), 200);
  }
  if (screen === "settings") nav("设置");
  if (screen === "notebook") {
    nav("warmup-long");
    setTimeout(() => [...document.querySelectorAll<HTMLElement>(".tab")].find((e) => e.textContent === "Notebook")?.click(), 200);
  }
  if (screen === "machines" || screen === "projectsettings") {
    nav("设置");
    setTimeout(() => [...document.querySelectorAll<HTMLElement>(".settings-nav .nav")].find((e) => e.textContent === (screen === "machines" ? "机器" : "项目"))?.click(), 200);
  }
  if (screen === "palette") press("k", true);
  if (screen === "terminal") press("`", true);
  if (screen === "add") nav("添加第一个项目…");
}, 400);
