#pragma once

#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/ui_window.hpp"
#include "app/ui_shell/window_layout.hpp"

#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace ecnuvpn::platform::win32::ui_shell {

std::string dispatch_webview2_host_message(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core);

void post_webview2_host_response(
    const std::string &message_json,
    const ecnuvpn::ui_shell::CoreRpcInvoker &invoke_core,
    const std::function<void(const std::string &)> &post_response);

std::wstring webview2_renderer_uri(
    const ecnuvpn::ui_shell::RendererAssets &renderer);

std::wstring webview2_packaged_renderer_folder(
    const ecnuvpn::ui_shell::RendererAssets &renderer);

[[nodiscard]] ecnuvpn::ui_shell::WindowBounds
webview2_default_window_bounds() noexcept;

struct WebView2TrayMenuItem {
  std::wstring label;
  int command_id;
  bool separator;
};

std::vector<WebView2TrayMenuItem> webview2_tray_menu_model();
bool webview2_should_create_tray_on_start();

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webview2_window();

} // namespace ecnuvpn::platform::win32::ui_shell
