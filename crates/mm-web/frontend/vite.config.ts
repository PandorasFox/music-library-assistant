import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// mm-web backend — resolve via container IP or override with MM_BACKEND env var.
const backend = process.env.MM_BACKEND ?? "http://172.18.0.5:3313";

const proxyPaths = [
  "/status",
  "/config",
  "/config-kdl",
  "/dir-config",
  "/queries",
  "/tx",
  "/auth",
  "/setup",
  "/commands",
  "/actions",
  "/build-info",
];

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      ...Object.fromEntries(proxyPaths.map((p) => [p, backend])),
      "/ws": {
        target: backend.replace(/^http/, "ws"),
        ws: true,
      },
    },
  },
});
