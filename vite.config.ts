import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The Tauri config is the single source of the app version, injected as
// __APP_VERSION__, and of the development URL Tauri loads, which this server
// must listen on. The Vitest config reads the version the same way because
// Vitest does not load this file.
const tauriConfig = JSON.parse(readFileSync("./src-tauri/tauri.conf.json", "utf8")) as {
  version: string;
  build: { devUrl: string };
};
const appVersion = tauriConfig.version;
const devUrl = new URL(tauriConfig.build.devUrl);

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],

  // Loaded from disk by the webview, not over a network: the default 500 kB
  // warning measures transfer cost. 2000 keeps a runaway bundle loud without
  // flagging the ten-language catalogues on every build.
  build: {
    chunkSizeWarningLimit: 2000,
  },

  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. Tauri loads the fixed development URL, so fail rather than move to
  //    another port when it is taken. The port is unique across the fleet so
  //    it never collides with a sibling app's development server.
  server: {
    port: Number(devUrl.port),
    strictPort: true,
    host: devUrl.hostname,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
