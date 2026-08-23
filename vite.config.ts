import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

// Version single source of truth is src-tauri/tauri.conf.json; injected here as
// __APP_VERSION__ (declared in src/vite-env.d.ts). vitest.config.ts duplicates
// this define because vitest does not read this file.
const appVersion = (
  JSON.parse(readFileSync("./src-tauri/tauri.conf.json", "utf8")) as {
    version: string;
  }
).version;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],

  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available.
  //    Fleet-unique port so it never collides with a sibling Tauri app's
  //    launcher port-kill (dropkick uses 1521, quickdeck 1621).
  server: {
    port: 1721,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1722,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
