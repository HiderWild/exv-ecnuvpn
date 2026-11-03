/** 产品版本由根 release.json 在构建时注入。 */
const version = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "";
if (!version) throw new Error("构建时缺少产品版本，请检查 release.json 和 Vite 配置");
export const PRODUCT_VERSION: string = version;
