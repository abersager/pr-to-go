import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and serves the built files from dist/.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // For running the UI in a browser against `cargo run -p dev-server`.
    proxy: Object.fromEntries(
      ["/api", "/events", "/prtg", "/__demo"].map((p) => [p, "http://127.0.0.1:1421"]),
    ),
  },
  build: { target: "es2022", outDir: "dist" },
  // ES-module workers can code-split, so each Shiki grammar is its own
  // chunk, loaded the first time a file in that language is opened.
  worker: { format: "es" },
  test: { environment: "jsdom", exclude: ["e2e/**", "node_modules/**"] },
});
