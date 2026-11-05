import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

// y-monaco imports Monaco by a path that monaco-editor 0.57's exports map no longer
// resolves; point it at the same module the app uses (one Monaco instance).
const monacoApi = fileURLToPath(new URL("./node_modules/monaco-editor/esm/vs/editor/editor.api.js", import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [{ find: /^monaco-editor\/esm\/vs\/editor\/editor\.api\.js$/, replacement: monacoApi }],
  },
  server: {
    port: 5173,
    strictPort: true,
    // The browser talks to one origin; Vite forwards API calls to the Rust server.
    proxy: {
      "/api": { target: "http://127.0.0.1:8090", ws: true },
      "/healthz": "http://127.0.0.1:8090",
    },
  },
});
