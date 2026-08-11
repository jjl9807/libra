/**
 * Next.js build configuration for the Libra web UI.
 *
 * Configured for static HTML export so the bundle can be served directly
 * from the Rust backend (or any static host) without a Node runtime.
 *
 * - `output: "export"` produces a fully static `out/` directory.
 * - `images.unoptimized` disables the optimizer because the export target
 *   has no Next.js image server; all `<Image>` sources must self-host.
 * - `trailingSlash: true` ensures every route resolves to `path/index.html`,
 *   which is required by most static file servers (including the embedded
 *   `axum` server used by `libra code`).
 * - `generateBuildId` is pinned to the package version so CI's
 *   `web/out` drift check stays deterministic (Next's default nanoid
 *   would rewrite `_next/static/<buildId>/` on every clean build).
 */
import type { NextConfig } from "next";

import packageJson from "./package.json";

const nextConfig: NextConfig = {
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,
  generateBuildId: async () => `libra-web-${packageJson.version}`,
};

export default nextConfig;
