import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "tests/e2e",
  outputDir: "test-results",
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: { baseURL: "http://127.0.0.1:5173", trace: "retain-on-failure" },
  projects: [{ name: "chromium-1440", use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } }],
  webServer: { command: "npm run dev", url: "http://127.0.0.1:5173", reuseExistingServer: true, timeout: 60_000 },
});
