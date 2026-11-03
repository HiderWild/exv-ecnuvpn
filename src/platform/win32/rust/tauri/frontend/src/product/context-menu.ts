/**
 * S6b 全域禁用原生右键菜单：
 * 桌面 WebView 的原生 contextmenu 只提供「检查元素」等开发态入口（release 配置
 * 无 devtools 依赖），对产品只有噪声；日志页的自制条目菜单承担真实复制入口。
 * document 级监听一次 preventDefault，应用生命周期内常驻不移除；模块独立成文件
 * 是为了让 main.ts 只引入一行，不扩大入口改动面。
 */
export function installNativeContextMenuBan(target: Pick<Document, "addEventListener"> = document): void {
  target.addEventListener("contextmenu", (event: Event) => {
    event.preventDefault();
  });
}
