import { createRequire } from "node:module";
import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";

const require = createRequire(import.meta.url);

// Every server-side route the SPA calls in dev is proxied to one running
// yorishiro-hosted-server; the SPA itself is served by rsbuild on :3000.
const DEV_API_TARGET = process.env.YORISHIRO_DEV_API_TARGET || "http://localhost:8080";
const PROXIED_PREFIXES = ["/api", "/auth", "/hosted", "/setup", "/up"];

export default defineConfig({
  plugins: [pluginReact({ reactCompiler: true })],
  server: {
    host: "0.0.0.0",
    port: 3000,
    proxy: Object.fromEntries(
      PROXIED_PREFIXES.map((prefix) => [
        prefix,
        { target: DEV_API_TARGET, changeOrigin: true },
      ]),
    ),
  },
  source: {
    entry: {
      index: "./src/main.tsx",
    },
  },
  html: {
    template: "./index.html",
  },
  tools: {
    postcss: {
      postcssOptions: {
        plugins: [require("@tailwindcss/postcss")],
      },
    },
  },
  output: {
    sourceMap: {
      js: false,
      css: false,
    },
    minify: {
      js: true,
      css: true,
    },
  },
});
