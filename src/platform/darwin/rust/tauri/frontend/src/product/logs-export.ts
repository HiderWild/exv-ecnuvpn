import { darwinCommandAdapter } from "./command-adapter-global";

/** 返回取消、实际保存或交给浏览器下载，避免将下载请求误报为写盘成功。 */
export async function exportLogFile(contents: string): Promise<"saved" | "download" | "cancelled"> {
  const suggestedName = `EXV-logs-${new Date().toISOString().replace(/[:.]/g, "-")}.jsonl`;
  const adapter = darwinCommandAdapter();
  if (adapter) {
    return await adapter.call<boolean>("logs_export", { suggestedName, contents }) ? "saved" : "cancelled";
  }
  // 浏览器预览（壳未注入 adapter）：走浏览器下载，不把下载请求表述为写盘成功。
  const url = URL.createObjectURL(new Blob([contents], { type: "application/x-ndjson;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = suggestedName;
  link.hidden = true;
  document.body.appendChild(link);
  try {
    link.click();
  } finally {
    link.remove();
    // 浏览器异步接管下载后再释放 URL。
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  }
  return "download";
}
