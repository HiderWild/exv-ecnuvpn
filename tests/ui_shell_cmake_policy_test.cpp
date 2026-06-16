#include <iostream>
#include <fstream>
#include <sstream>
#include <string>

#ifndef ECNUVPN_SOURCE_DIR
#error "ECNUVPN_SOURCE_DIR must be defined"
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
  const std::string source_dir = ECNUVPN_SOURCE_DIR;
  const std::string cmake = read_file(source_dir + "/CMakeLists.txt");
  const std::string windows_build_script =
      read_file(source_dir + "/scripts/build-windows.ps1");
  const std::string macos_build_script =
      read_file(source_dir + "/scripts/build-macos.sh");
  const std::string webview2_host =
      read_file(source_dir +
                "/src/platform/win32/ui_shell/webview2_host_win32.cpp");
  const std::string wkwebview_host =
      read_file(source_dir +
                "/src/platform/darwin/ui_shell/wk_webview_host_darwin.mm");

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

  expect_not_contains(windows_build_script, "scripts/build-windows.ps1",
                      "vpn_runtime_test");
  expect_not_contains(macos_build_script, "scripts/build-macos.sh",
                      "vpn_runtime_test");

  return failures == 0 ? 0 : 1;
}
