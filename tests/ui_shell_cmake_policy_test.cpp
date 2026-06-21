#include <iostream>
#include <fstream>
#include <sstream>
#include <string>

#ifndef EXV_SOURCE_DIR
#error "EXV_SOURCE_DIR must be defined"
#endif

namespace {

std::string read_file(const std::string &path) {
  std::ifstream input(path);
  if (!input.good()) {
    std::cerr << "failed to read " << path << "\n";
    return {};
  }
  std::ostringstream buffer;
  buffer << input.rdbuf();
  return buffer.str();
}

bool contains(const std::string &text, const std::string &needle) {
  return text.find(needle) != std::string::npos;
}

} // namespace

int main() {
  const std::string source_dir = EXV_SOURCE_DIR;
  const std::string cmake = read_file(source_dir + "/CMakeLists.txt");
  const std::string windows_build_script =
      read_file(source_dir + "/scripts/build-windows.ps1");
  const std::string macos_build_script =
      read_file(source_dir + "/scripts/build-macos.sh");
  const std::string windows_merge_prep_script =
      read_file(source_dir + "/scripts/validate-merge-prep-windows.ps1");
  const std::string macos_merge_prep_script =
      read_file(source_dir + "/scripts/validate-merge-prep-macos.sh");
  const std::string webview2_host =
      read_file(source_dir +
                "/src/platform/win32/ui_shell/webview2_host_win32.cpp");
  const std::string win32_manifest =
      read_file(source_dir +
                "/src/platform/win32/ui_shell/exv_ui_win32.manifest");
  const std::string win32_resources =
      read_file(source_dir + "/src/platform/win32/ui_shell/resource.hpp");
  const std::string win32_resource_script =
      read_file(source_dir + "/src/platform/win32/ui_shell/exv_ui_win32.rc");
  const std::string wkwebview_host =
      read_file(source_dir +
                "/src/platform/darwin/ui_shell/wk_webview_host_darwin.mm");
  const std::string webkitgtk_host =
      read_file(source_dir +
                "/src/platform/linux/ui_shell/webkitgtk_host_linux.cpp");
  const std::string core_process_manager =
      read_file(source_dir + "/src/app/ui_shell/core_process_manager.cpp");

  int failures = 0;
  const auto expect_contains = [&](const std::string &needle) {
    if (!contains(cmake, needle)) {
      std::cerr << "missing CMake UI shell policy: " << needle << "\n";
      ++failures;
    }
  };
  const auto expect_not_contains = [&](const std::string &text,
                                       const std::string &description,
                                       const std::string &needle) {
    if (contains(text, needle)) {
      std::cerr << description << " references disabled target: " << needle
                << "\n";
      ++failures;
    }
  };

  expect_contains("option(EXV_BUILD_UI_SHELL");
  expect_contains("WEBVIEW2_SDK_DIR is required");
  expect_contains("elseif(APPLE AND EXV_BUILD_UI_SHELL)");
  expect_contains("target_link_libraries(exv-ui PRIVATE \"-framework Cocoa\" \"-framework WebKit\")");
  expect_contains("WebView2Loader.dll");
  expect_contains("POST_BUILD");
  expect_contains("src/platform/win32/ui_shell/exv_ui_win32.rc");
  expect_contains("src/platform/win32/ui_shell/exv_ui_win32.manifest");
  expect_contains("WIN32_EXECUTABLE TRUE");
  expect_contains("OBJECT_DEPENDS");
  expect_contains("${CMAKE_SOURCE_DIR}/assets/icons");
  expect_contains("if(UNIX AND NOT APPLE AND EXV_BUILD_UI_SHELL)");
  expect_contains("pkg_check_modules(WEBKITGTK");
  expect_contains("webkit2gtk-4.1");
  expect_contains("gtk+-3.0");
  expect_contains("darwin_wkwebview_runtime_test");
  expect_contains("tests/darwin_wkwebview_runtime_test.cpp");
  expect_contains("target_link_libraries(darwin_wkwebview_runtime_test PRIVATE");
  expect_contains("\"-framework Cocoa\"");
  expect_contains("\"-framework WebKit\"");
  expect_contains("linux_webkitgtk_runtime_test");
  expect_contains("tests/linux_webkitgtk_runtime_test.cpp");
  expect_contains("target_compile_definitions(linux_webkitgtk_runtime_test PRIVATE");
  expect_contains("${WEBKITGTK_INCLUDE_DIRS}");
  expect_contains("${WEBKITGTK_LIBRARY_DIRS}");
  expect_contains("${WEBKITGTK_CFLAGS_OTHER}");
  expect_contains("${WEBKITGTK_LIBRARIES}");

  const auto expect_webview2_host_contains = [&](const std::string &needle) {
    if (!contains(webview2_host, needle)) {
      std::cerr << "missing Windows WebView2 host implementation marker: "
                << needle << "\n";
      ++failures;
    }
  };

  expect_webview2_host_contains("CreateCoreWebView2EnvironmentWithOptions");
  expect_webview2_host_contains("add_WebMessageReceived");
  expect_webview2_host_contains("PostWebMessageAsJson");
  expect_webview2_host_contains("WM_DPICHANGED");
  expect_webview2_host_contains("GetDpiForWindow");
  expect_webview2_host_contains("webview2_app_icon_resource_id");
  expect_webview2_host_contains("IDI_EXV_APP");
  expect_webview2_host_contains("window_class.hIcon");
  expect_webview2_host_contains("window_class.hIconSm");
  if (contains(webview2_host, "LoadIconW(nullptr")) {
    std::cerr << "Windows WebView shell must not use the default application icon\n";
    ++failures;
  }

  if (!contains(win32_manifest, "PerMonitorV2")) {
    std::cerr << "Windows WebView shell manifest must declare PerMonitorV2 DPI awareness\n";
    ++failures;
  }
  if (!contains(win32_manifest, "dpiAwareness")) {
    std::cerr << "Windows WebView shell manifest must declare dpiAwareness\n";
    ++failures;
  }
  if (!contains(win32_manifest, "requestedExecutionLevel") ||
      !contains(win32_manifest, "requireAdministrator")) {
    std::cerr << "Windows WebView shell manifest must request Administrator rights for Wintun packet I/O\n";
    ++failures;
  }
  if (!contains(win32_resources, "#define IDI_EXV_APP")) {
    std::cerr << "Windows WebView shell must define a stable EXV app icon resource id\n";
    ++failures;
  }
  if (!contains(win32_resource_script, "IDI_EXV_APP ICON \"icon.ico\"")) {
    std::cerr << "Windows WebView shell resource script must embed the EXV app icon\n";
    ++failures;
  }

  const auto expect_wkwebview_host_contains = [&](const std::string &needle) {
    if (!contains(wkwebview_host, needle)) {
      std::cerr << "missing macOS WKWebView host implementation marker: "
                << needle << "\n";
      ++failures;
    }
  };

  expect_wkwebview_host_contains("WKWebView");
  expect_wkwebview_host_contains("WKScriptMessageHandler");
  expect_wkwebview_host_contains("addScriptMessageHandler");
  expect_wkwebview_host_contains("WKUserScript");
  expect_wkwebview_host_contains("evaluateJavaScript");
  expect_wkwebview_host_contains("loadFileURL");

  const auto expect_webkitgtk_host_contains = [&](const std::string &needle) {
    if (!contains(webkitgtk_host, needle)) {
      std::cerr << "missing Linux WebKitGTK host implementation marker: "
                << needle << "\n";
      ++failures;
    }
  };

  expect_webkitgtk_host_contains("WebKitWebView");
  expect_webkitgtk_host_contains("gtk_application_window_new");
  expect_webkitgtk_host_contains("WebKitUserContentManager");
  expect_webkitgtk_host_contains("webkit_user_content_manager_register_script_message_handler");
  expect_webkitgtk_host_contains("script-message-received::exvHost");
  expect_webkitgtk_host_contains("webkit_user_script_new");
  expect_webkitgtk_host_contains("webkit_web_view_run_javascript");
  expect_webkitgtk_host_contains("webkit_web_view_load_uri");

  const auto expect_core_process_manager_contains =
      [&](const std::string &needle) {
        if (!contains(core_process_manager, needle)) {
          std::cerr << "missing UI shell core process transport marker: "
                    << needle << "\n";
          ++failures;
        }
      };

  expect_core_process_manager_contains("class PosixCoreProcessTransport");
  expect_core_process_manager_contains("move_fd_above_stdio");
  expect_core_process_manager_contains("posix_spawn_file_actions_adddup2");
  expect_core_process_manager_contains("posix_spawn(&child");
  expect_core_process_manager_contains("poll(&descriptor, 1, 0)");
  expect_core_process_manager_contains("configure_core_process_transport_signal_policy");
  expect_core_process_manager_contains("std::make_unique<PosixCoreProcessTransport>");

  expect_not_contains(windows_build_script, "scripts/build-windows.ps1",
                      "vpn_runtime_test");
  expect_not_contains(macos_build_script, "scripts/build-macos.sh",
                      "vpn_runtime_test");
  expect_not_contains(windows_merge_prep_script,
                      "scripts/validate-merge-prep-windows.ps1",
                      "vpn_runtime_test");
  expect_not_contains(macos_merge_prep_script,
                      "scripts/validate-merge-prep-macos.sh",
                      "vpn_runtime_test");
  expect_not_contains(windows_merge_prep_script,
                      "scripts/validate-merge-prep-windows.ps1",
                      "tunnel_script_contract_test");
  expect_not_contains(macos_merge_prep_script,
                      "scripts/validate-merge-prep-macos.sh",
                      "tunnel_script_contract_test");

  return failures == 0 ? 0 : 1;
}
