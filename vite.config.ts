import tailwindcss from "@tailwindcss/vite";
import { paraglideVitePlugin } from "@inlang/paraglide-js";
import { TanStackRouterVite } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import path from "path";
import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

const EMPTY_RUNTIME_SOURCEMAP = JSON.stringify({
  version: 3,
  file: "runtime.js",
  sources: [],
  names: [],
  mappings: "",
});

const EMPTY_SERVER_SOURCEMAP = JSON.stringify({
  version: 3,
  file: "server.js",
  sources: [],
  names: [],
  mappings: "",
});

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [
    paraglideVitePlugin({
      project: "./project.inlang",
      outdir: "./src/paraglide",
      strategy: ["localStorage", "preferredLanguage", "baseLocale"],
      emitTsDeclarations: true,
      // Paraglide runtime.js 内部带有 `//# sourceMappingURL=strategy.js.map`，但默认不会输出 .map 文件。
      // Vite dev 会尝试读取该 map，导致控制台出现 ENOENT 警告；这里写入一个空 map 用于消噪。
      additionalFiles: {
        "strategy.js.map": EMPTY_RUNTIME_SOURCEMAP,
        // server.js 同理（仅在被引入时会触发）
        "middleware.js.map": EMPTY_SERVER_SOURCEMAP,
      },
      outputStructure: "message-modules",
    }),
    TanStackRouterVite({ target: "react", autoCodeSplitting: true }),
    react({
      babel: {
        plugins: ["babel-plugin-react-compiler"],
      },
    }),
    tailwindcss(),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    // Tauri 桌面包本地分发，且配置页已经按路由懒加载；当前最大 chunk 属于可接受范围。
    // 将阈值调到接近现状，避免稳定可接受的大包持续污染构建输出。
    chunkSizeWarningLimit: 900,
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
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
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
