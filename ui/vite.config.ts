import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri serves the dev frontend from a fixed port and reads `dist/` for builds.
export default defineConfig({
  plugins: [svelte()],
  // Relative asset paths so they resolve under Tauri's asset protocol.
  base: "./",
  // Keep Vite's output out of the way of Tauri's own logging.
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    target: "esnext",
    emptyOutDir: true,
  },
});
