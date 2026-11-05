import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
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
