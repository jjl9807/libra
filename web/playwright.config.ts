import { defineConfig, devices } from "@playwright/test";

/**
 * W3-15: real-browser e2e against an already-started deterministic Web runtime.
 *
 * Does not spawn `libra code`. Start the runtime first (see
 * `e2e/scripts/start-deterministic-runtime.sh`), then:
 *
 *   LIBRA_E2E_BASE_URL=http://127.0.0.1:4410 pnpm --dir web test:e2e
 *
 * Set `LIBRA_E2E_REQUIRE=1` to fail closed when the browser or runtime is
 * missing (completion evidence). Without it, missing deps print an explicit
 * skip reason and exit 0 for local diagnosis only.
 */
const baseURL = process.env.LIBRA_E2E_BASE_URL ?? "http://127.0.0.1:4410";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  // Serial chain against one live runtime — a retry would replay mid-session
  // without restarting the fake provider / lease.
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 30_000 },
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  globalSetup: "./e2e/global-setup.ts",
  outputDir: "test-results",
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    extraHTTPHeaders: {
      // Same-origin browser traffic; Origin helps loopback write guards (W3-05).
      Origin: baseURL,
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
