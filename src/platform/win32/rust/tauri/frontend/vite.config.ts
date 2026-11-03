import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// 产品版本从根 release.json 读取，并检查本平台配置。
const appVersion: string = JSON.parse(readFileSync(new URL("../../../../../../release.json", import.meta.url), "utf8")).version;
const configuredVersion = JSON.parse(readFileSync(new URL("../app/tauri.conf.json", import.meta.url), "utf8")).version;
if (typeof appVersion !== "string" || !appVersion || appVersion !== configuredVersion) {
  throw new Error("产品版本与 Tauri 配置不一致，请运行 python scripts/release.py version check");
}

// Tauri 官方 dev 端口约定：devUrl 见 app/tauri.conf.json（http://localhost:1420）。
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [vue()],
  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },
  // 生产构建输出到 ../frontend/dist（tauri.conf.json frontendDist）。
  build: { target: "es2022" },
  // Tauri 生产模式经 custom protocol（http://tauri.localhost）加载 frontendDist：
  // 绝对路径 `/assets/...` 会解析到裸 localhost 导致 ERR_CONNECTION_REFUSED。
  // 相对 base 使资源从当前页面 URL 解析，不依赖 host。
  base: "./",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/app/**"] },
  },
});
