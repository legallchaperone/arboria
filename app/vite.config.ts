import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

// The fonts are the Python package's vendored copies (SIL OFL 1.1): one copy in the repo.
const fonts = fileURLToPath(new URL("../src/arboria/ui/fonts", import.meta.url));

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: { alias: { "@fonts": fonts } },
  server: { port: 5173, strictPort: true, fs: { allow: [".", fonts] } },
  build: { target: "safari16", outDir: "dist", emptyOutDir: true },
  test: { include: ["src/**/*.test.ts"] },
});
