// 保留项目和本侧实际前端运行时依赖许可原文，不代表原生发行包完整 SBOM。
import exv from "../assets/licenses/exv.txt?raw";
import vue from "../assets/licenses/vue.txt?raw";
import three from "../assets/licenses/three.txt?raw";

export const PRODUCT_LICENSE_NOTICES = [
  { name: "EXV", license: "MIT", source: "项目根目录 LICENSE", text: exv },
  { name: "Vue 3.5.42", license: "MIT", source: "node_modules/vue/LICENSE", text: vue },
  { name: "Three.js 0.180.0", license: "MIT", source: "node_modules/three/LICENSE", text: three },
] as const;
