import { defineConfig, devices } from "@playwright/test";

// The end-to-end test through the screens on the disposable test cluster. It is never part of
// `npm run test:e2e` or CI: scripts/test-cluster/ui-e2e.sh starts the control plane, worker,
// Studio and Console on their own ports and runs it with this configuration.
export default defineConfig({
  testDir: ".",
  // Not under test-results/: the main suite empties that directory when it starts.
  outputDir: "../../test-results-lab",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 15 * 60_000,
  expect: { timeout: 30_000 },
  reporter: [["list"]],
  // A click on something that never becomes usable fails in a minute, not at the test's timeout.
  use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 }, actionTimeout: 60_000, trace: "retain-on-failure" },
});
