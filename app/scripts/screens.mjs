// Screenshots of the app over the fixture (ADR 0004): starts `vite` on a free port,
// opens /mock.html?screen=… in Chromium (light and dark) and writes screens/*.png.
// Playwright is not a dependency of the app: this uses the one installed on the machine
// (`require('playwright')`), or PLAYWRIGHT_MODULE.

import { createRequire } from "node:module";
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const port = 5199;
const vite = spawn("node_modules/.bin/vite", ["--port", String(port), "--strictPort"], {
  stdio: ["ignore", "pipe", "inherit"],
  detached: true,
});
await new Promise((ok, fail) => {
  vite.stdout.on("data", (d) => String(d).includes("Local") && ok());
  vite.on("exit", (code) => fail(new Error(`vite exited (${code})`)));
  vite.stdout.on("data", () => undefined);
});
mkdirSync("screens", { recursive: true });
const browser = await chromium.launch();
const screens = (process.argv[2] ?? "inbox,overview,idea,card,palette,terminal").split(",");
const errors = [];
for (const scheme of ["light", "dark"]) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 2, colorScheme: scheme });
  page.on("pageerror", (e) => errors.push(`${scheme}: ${e.message}`));
  page.on("console", (m) => m.type() === "error" && errors.push(`${scheme}: ${m.text()}`));
  for (const s of screens) {
    console.log(scheme, s);
    await page.goto(`http://localhost:${port}/mock.html?screen=${s}`, { waitUntil: "domcontentloaded" });
    await page.waitForTimeout(s === "card" ? 2600 : 1200);
    await page.screenshot({ path: `screens/${s}-${scheme}.png` });
  }
  await page.close();
}
await browser.close();
process.kill(-vite.pid);
if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(`wrote ${screens.length * 2} screenshots to screens/`);
