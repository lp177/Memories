import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Dev server proxies API + MCP calls to the Rust backend on :8090.
export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5174,
    proxy: {
      "/api": "http://localhost:8090",
      "/mcp": "http://localhost:8090",
    },
  },
});
