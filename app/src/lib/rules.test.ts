// The source rules of ADR 0004 for the app: ledger text is never rendered as HTML, the
// app's CSS uses design tokens only, and only the main window may call the commands.

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const app = join(__dirname, "..", "..");

function files(dir: string, ext: RegExp): string[] {
  return readdirSync(dir).flatMap((n: string) => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? files(p, ext) : ext.test(n) ? [p] : [];
  });
}

describe("rules", () => {
  it("never renders HTML from data", () => {
    for (const f of files(join(app, "src"), /\.tsx?$/)) {
      if (f.endsWith(".test.ts")) continue;
      const src = readFileSync(f, "utf8");
      for (const bad of ["dangerouslySetInnerHTML", ".innerHTML", "insertAdjacentHTML", "eval("]) {
        expect(src.includes(bad), `${f} uses ${bad}`).toBe(false);
      }
    }
  });
  it("styles with tokens only", () => {
    const css = readFileSync(join(app, "src/styles/app.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    expect(css.match(/#[0-9a-fA-F]{3,8}\b/g)).toBeNull();
    expect(css.match(/\b(rgb|rgba|hsl|hsla)\(/g)).toBeNull();
    for (const f of files(join(app, "src"), /\.tsx$/)) {
      expect(readFileSync(f, "utf8").match(/#[0-9a-fA-F]{6}\b/g), f).toBeNull();
    }
  });
  it("lets only the main window call the commands", () => {
    const dir = join(app, "src-tauri/capabilities");
    const caps = readdirSync(dir).map((n: string) => JSON.parse(readFileSync(join(dir, n), "utf8")));
    expect(caps.length).toBe(1);
    expect(caps[0].windows).toEqual(["main"]);
    expect(caps[0].webviews).toBeUndefined();
    expect(caps[0].remote).toBeUndefined();
    const conf = JSON.parse(readFileSync(join(app, "src-tauri/tauri.conf.json"), "utf8"));
    expect(conf.app.security.capabilities).toEqual(["main"]);
    expect(conf.app.windows.map((w: { label: string }) => w.label)).toEqual(["main"]);
    expect(conf.app.security.csp["connect-src"]).toBe("ipc: http://ipc.localhost");
    const lib = readFileSync(join(app, "src-tauri/src/lib.rs"), "utf8");
    const build = readFileSync(join(app, "src-tauri/build.rs"), "utf8");
    const listed = [...lib.matchAll(/^    "([a-z_]+)",$/gm)].map((m) => m[1]);
    expect(listed.length).toBeGreaterThan(5);
    for (const c of listed) {
      expect(caps[0].permissions).toContain(`allow-${c.replace(/_/g, "-")}`);
      expect(build).toContain(`"${c}"`);
    }
  });
});
