import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import UnoCSS from "@unocss/vite";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [vue(), UnoCSS()],
  server: {
    host: "::",
    proxy: {
      "/api": {
        target: "http://localhost:5174",
        changeOrigin: true
      }
    }
  },
  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        admin: resolve(__dirname, "admin.html"),
        viewer: resolve(__dirname, "viewer.html")
      }
    }
  }
});
