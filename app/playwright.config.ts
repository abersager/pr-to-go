import { defineConfig } from "@playwright/test";

// End-to-end tests: the real UI in Chromium, the real core behind it
// (`prtg-dev`), and the fake GitHub with demo data. No Tauri needed.
export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  fullyParallel: false,
  workers: 1,
  use: { baseURL: "http://localhost:1420", viewport: { width: 1400, height: 900 } },
  webServer: [
    {
      command: "cargo run -q -p dev-server",
      cwd: "..",
      port: 1421,
      timeout: 300_000,
      reuseExistingServer: !process.env.CI,
    },
    { command: "pnpm dev", url: "http://localhost:1420", reuseExistingServer: !process.env.CI },
  ],
});
