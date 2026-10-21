import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "tests/e2e",
  outputDir: "test-results",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [["list"], ["github"]] : [["list"]],
  use: { baseURL: "http://127.0.0.1:5173", trace: "retain-on-failure" },
  projects: [
    { name: "chromium-1440", testIgnore: /fabric-console(-[a-z-]+)?\.spec\.ts/, use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } },
    { name: "console-1440", testMatch: /fabric-console(-[a-z-]+)?\.spec\.ts/, use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 }, baseURL: "http://127.0.0.1:5174" } },
  ],
  webServer: [
    { command: "npm run dev", url: "http://127.0.0.1:5173", reuseExistingServer: true, timeout: 60_000 },
    { command: "npm run dev:console", url: "http://127.0.0.1:5174", reuseExistingServer: true, timeout: 60_000 },
  ],
});
