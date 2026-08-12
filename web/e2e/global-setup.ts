import { chromium, type FullConfig } from "@playwright/test";

function failOrSkip(reason: string): never {
  const requireE2e = process.env.LIBRA_E2E_REQUIRE === "1" || process.env.CI === "true";
  // Explicit skip for local diagnosis only — never treat as Checkpoint C evidence.
  console.error(`skip: ${reason}`);
  if (requireE2e) {
    console.error(
      "LIBRA_E2E_REQUIRE/CI is set: refusing soft-skip. Install Playwright browsers and start the deterministic runtime (e2e/scripts/start-deterministic-runtime.sh).",
    );
    process.exit(1);
  }
  process.exit(0);
}

/**
 * Preflight: Chromium must launch, and `/api/health` on LIBRA_E2E_BASE_URL must
 * respond. Specs assert only through the browser DOM against that live server.
 */
async function globalSetup(config: FullConfig): Promise<void> {
  const baseURL = config.projects[0]?.use?.baseURL ?? process.env.LIBRA_E2E_BASE_URL;
  if (!baseURL || typeof baseURL !== "string") {
    failOrSkip("LIBRA_E2E_BASE_URL / playwright baseURL is unset");
  }

  let browser;
  try {
    browser = await chromium.launch({ headless: true });
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    failOrSkip(
      `Playwright Chromium unavailable (${detail}). Run: pnpm --dir web exec playwright install chromium`,
    );
  }

  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    const health = await page.goto(`${baseURL.replace(/\/$/, "")}/api/health`, {
      waitUntil: "domcontentloaded",
      timeout: 10_000,
    });
    const body = (await health?.text())?.trim() ?? "";
    if (!health || health.status() !== 200 || body !== "ok") {
      failOrSkip(
        `deterministic Web runtime not reachable at ${baseURL}/api/health (status=${health?.status() ?? "none"} body=${JSON.stringify(body)}). Start it with e2e/scripts/start-deterministic-runtime.sh`,
      );
    }
    await context.close();
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    failOrSkip(
      `deterministic Web runtime health check failed at ${baseURL}: ${detail}. Start it with e2e/scripts/start-deterministic-runtime.sh`,
    );
  } finally {
    await browser.close();
  }
}

export default globalSetup;
