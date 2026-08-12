import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  test: {
    // W3-15 Playwright specs live under e2e/ and must not be collected by vitest.
    exclude: ["**/node_modules/**", "**/e2e/**", "**/playwright-report/**", "**/test-results/**"],
  },
});
