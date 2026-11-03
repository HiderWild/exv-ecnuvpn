#pragma once

#include <string_view>

namespace exv::setup {

inline constexpr wchar_t kWebView2RuntimeClientId[] =
    L"{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
inline constexpr wchar_t kWebView2OfficialDownloadUrl[] =
    L"https://developer.microsoft.com/microsoft-edge/webview2/";
inline constexpr wchar_t kWebView2MissingRuntimeMessage[] =
    L"缺少 Microsoft Edge WebView2 Evergreen Runtime，已停止安装以避免 EXV 无法启动。"
    L"请从微软官方页面安装 Evergreen Runtime 后重新运行安装程序："
    L"https://developer.microsoft.com/microsoft-edge/webview2/";
inline constexpr wchar_t kWebView2RuntimePrompt[] =
    L"检测到本机未安装 Microsoft Edge WebView2 Evergreen Runtime。\n"
    L"EXV 主界面依赖该运行时；继续安装会导致 EXV 无法启动。\n\n"
    L"请选择“是”打开微软官方 WebView2 下载页，安装 Evergreen Runtime 后重新运行本安装程序。"
    L"校园离线环境可由管理员通过微软官方 Evergreen Standalone Installer 部署。\n\n"
    L"是：打开微软官方下载页；否：退出安装。\n"
    L"https://developer.microsoft.com/microsoft-edge/webview2/";

// 仅接受 Microsoft WebView2 Evergreen Runtime 注册值 pv (REG_SZ) 的四段数值版本，
// 且版本必须大于 0.0.0.0。
[[nodiscard]] bool IsUsableWebView2RuntimeVersion(std::wstring_view version);

// 检查当前设备/用户的 Evergreen Runtime 注册状态，不下载或安装任何组件。
[[nodiscard]] bool IsWebView2EvergreenRuntimeInstalled();

}  // namespace exv::setup
