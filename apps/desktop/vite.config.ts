import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import pkg from "./package.json" with { type: "json" };

// Tauri serves the frontend from a fixed port in development and from bundled
// assets in release, so the build target stays deliberately boring.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  define: {
    // Surfaced in Settings → About so a bug report can name the exact build.
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 5219,
    strictPort: true,
    host: "127.0.0.1",
  },
  build: {
    target: "chrome110",
    sourcemap: false,
    minify: "esbuild",
  },
});