#include "app/ui_shell/renderer_assets.hpp"
#include "app/ui_shell/ui_shell_options.hpp"
#include "app/ui_shell/ui_window.hpp"

#include <filesystem>
#include <iostream>
#include <string>
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

#if defined(_WIN32)
  return ecnuvpn::platform::win32::ui_shell::run_webview2_host(config);
#elif defined(__APPLE__)
  return ecnuvpn::platform::darwin::ui_shell::run_wk_webview_host(config);
#elif defined(__linux__)
  return ecnuvpn::platform::linux::ui_shell::run_webkitgtk_host(config);
#else
  return 70;
#endif
}
