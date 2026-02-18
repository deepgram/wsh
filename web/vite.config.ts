import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

export default defineConfig({
  plugins: [preact()],
  build: {
    outDir: "../web-dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/ws": {
        target: "ws://127.0.0.1:8080",
        ws: true,
      },
      "/sessions": {
        target: "http://127.0.0.1:8080",
      },
      "/health": {
        target: "http://127.0.0.1:8080",
      },
      "/orch-ws": {
        target: "ws://127.0.0.1:9090",
        ws: true,
        rewrite: (path) => path.replace(/^\/orch-ws/, "/ws"),
      },
      "/orch-api": {
        target: "http://127.0.0.1:9090",
        rewrite: (path) => path.replace(/^\/orch-api/, ""),
      },
    },
  },
});
