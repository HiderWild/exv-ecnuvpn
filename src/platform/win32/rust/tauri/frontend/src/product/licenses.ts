// 从仓库和已安装的实际运行时依赖许可文件保留原文；不是原生发行包完整 SBOM。
import exv from "../assets/licenses/exv.txt?raw";
import vue from "../assets/licenses/vue.txt?raw";
import three from "../assets/licenses/three.txt?raw";
import tauri_mit from "../assets/licenses/tauri-mit.txt?raw";
import tauri_apache from "../assets/licenses/tauri-apache.txt?raw";

export const PRODUCT_LICENSE_NOTICES = [
  { name: "EXV", license: "MIT", source: "项目根目录 LICENSE", text: exv },
  { name: "Vue 3.5.41", license: "MIT", source: "node_modules/vue/LICENSE", text: vue },
  { name: "Three.js 0.180.0", license: "MIT", source: "node_modules/three/LICENSE", text: three },
  { name: "Tauri API 2.11.1", license: "MIT", source: "node_modules/@tauri-apps/api/LICENSE_MIT", text: tauri_mit },
  { name: "Tauri API 2.11.1", license: "Apache-2.0", source: "node_modules/@tauri-apps/api/LICENSE_APACHE-2.0", text: tauri_apache }
] as const;
