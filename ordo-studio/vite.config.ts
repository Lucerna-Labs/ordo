import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.ORDO_STUDIO_DEV_HOST;

export default defineConfig(() => ({
  plugins: [react()],
  base: "./",
  clearScreen: false,
  build: {
    rolldownOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("react") || id.includes("scheduler")) return "vendor-react";
          if (id.includes("framer-motion")) return "vendor-motion";
          if (id.includes("lucide-react")) return "vendor-icons";
          return "vendor";
        },
      },
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    // Proxy /api, /ws, /health to the local Ordo runtime so Studio can
    // fetch the control API without CORS gymnastics. ORDO_CONTROL_URL allows
    // pointing at a non-default runtime; defaults match `ordo serve`.
    proxy: {
      "/api": {
        target: process.env.ORDO_CONTROL_URL || "http://127.0.0.1:4141",
        changeOrigin: false,
      },
      "/ws": {
        target: process.env.ORDO_CONTROL_URL || "http://127.0.0.1:4141",
        changeOrigin: false,
        ws: true,
      },
      "/health": {
        target: process.env.ORDO_CONTROL_URL || "http://127.0.0.1:4141",
        changeOrigin: false,
      },
      // Auto-connect probes for local LLM providers. Ollama and LM
      // Studio bind to their default ports without CORS, so the studio
      // proxies to them via these prefixes when discovering models.
      "/proxy/ollama": {
        target: "http://localhost:11434",
        changeOrigin: true,
        rewrite: (p: string) => p.replace(/^\/proxy\/ollama/, ""),
      },
      "/proxy/lmstudio": {
        target: "http://localhost:1234",
        changeOrigin: true,
        rewrite: (p: string) => p.replace(/^\/proxy\/lmstudio/, ""),
      },
    },
  },
}));
