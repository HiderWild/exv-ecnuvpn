#include "app/ui_shell/core_process_manager.hpp"
#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/renderer_assets.hpp"
#include "app/ui_shell/ui_shell_options.hpp"
#include "app/ui_shell/ui_shell_runtime.hpp"
#include "app/ui_shell/ui_window.hpp"

#include <filesystem>
#include <functional>
#include <iostream>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#if defined(_WIN32)
#include <windows.h>
#elif defined(__APPLE__)
#include <mach-o/dyld.h>
#elif defined(__linux__)
#include <array>
#include <unistd.h>
#endif

namespace {

class PlatformEntrypointWindow final : public ecnuvpn::ui_shell::UiWindow {
public:
  using Runner = std::function<int(const ecnuvpn::ui_shell::UiWindowConfig &)>;

  explicit PlatformEntrypointWindow(Runner runner) : runner_(std::move(runner)) {}

  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &config) override {
    if (!runner_) {
      return 70;
    }
    return runner_(config);
  }

  void emit_event(const std::string &event_json) override {
    last_event_json_ = event_json;
  }

private:
  Runner runner_;
  ecnuvpn::ui_shell::HostMessageHandler handler_;
  std::string last_event_json_;
};

std::filesystem::path current_executable_path() {
#if defined(_WIN32)
  std::vector<wchar_t> buffer(32768);
  const DWORD length = GetModuleFileNameW(
      nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
  if (length == 0 || length >= buffer.size()) {
    return {};
  }
  return std::filesystem::path(std::wstring(buffer.data(), length));
#elif defined(__APPLE__)
  uint32_t size = 0;
  _NSGetExecutablePath(nullptr, &size);
  std::vector<char> buffer(size);
  if (_NSGetExecutablePath(buffer.data(), &size) != 0) {
    return {};
  }
  return std::filesystem::path(buffer.data());
#elif defined(__linux__)
  std::array<char, 4096> buffer{};
  const ssize_t length =
      readlink("/proc/self/exe", buffer.data(), buffer.size() - 1);
  if (length <= 0) {
    return {};
  }
  return std::filesystem::path(std::string(buffer.data(), static_cast<size_t>(length)));
#else
  return {};
#endif
}

} // namespace

#if defined(_WIN32)
namespace ecnuvpn::platform::win32::ui_shell {
int run_webview2_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}
#elif defined(__APPLE__)
namespace ecnuvpn::platform::darwin::ui_shell {
int run_wk_webview_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}
#elif defined(__linux__)
namespace ecnuvpn::platform::linux::ui_shell {
int run_webkitgtk_host(const ecnuvpn::ui_shell::UiWindowConfig &config);
}
#endif

int main(int argc, char **argv) {
  auto options = ecnuvpn::ui_shell::resolve_ui_shell_options(
      argc, argv, current_executable_path());
  const std::string validation_error =
      ecnuvpn::ui_shell::validate_ui_shell_options(options);
  if (!validation_error.empty()) {
    std::cerr << "exv-ui: " << validation_error << '\n';
    return 64;
  }

  ecnuvpn::ui_shell::UiWindowConfig config{
      ecnuvpn::ui_shell::resolve_renderer_assets(
          options.renderer_dev_server_url, options.packaged_renderer_index),
      options.exv_path,
      options.enable_dev_tools,
  };

  auto transport = ecnuvpn::ui_shell::create_core_process_transport(
      ecnuvpn::ui_shell::CoreProcessLaunch{options.exv_path, "", "", true});
  ecnuvpn::ui_shell::CoreRpcClient client(*transport);

#if defined(_WIN32)
  PlatformEntrypointWindow window(
      [](const ecnuvpn::ui_shell::UiWindowConfig &runtime_config) {
        return ecnuvpn::platform::win32::ui_shell::run_webview2_host(
            runtime_config);
      });
#elif defined(__APPLE__)
  PlatformEntrypointWindow window(
      [](const ecnuvpn::ui_shell::UiWindowConfig &runtime_config) {
        return ecnuvpn::platform::darwin::ui_shell::run_wk_webview_host(
            runtime_config);
      });
#elif defined(__linux__)
  PlatformEntrypointWindow window(
      [](const ecnuvpn::ui_shell::UiWindowConfig &runtime_config) {
        return ecnuvpn::platform::linux::ui_shell::run_webkitgtk_host(
            runtime_config);
      });
#else
  return 70;
#endif
  return ecnuvpn::ui_shell::run_ui_shell_window(window, config, client);
}
