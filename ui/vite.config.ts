import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// https://vitejs.dev/config/ — `vitest/config` so the `test` block type-checks.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // The factory sound library lives in the core crate and is imported from there.
    fs: { allow: [".", "../crates/dissonant-core/assets"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: ["es2022", "chrome105", "safari14"],
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
