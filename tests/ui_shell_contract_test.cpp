#include "app/ui_shell/close_preference.hpp"
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/renderer_assets.hpp"
#include "app/ui_shell/ui_shell_options.hpp"
#include "app/ui_shell/window_layout.hpp"

#include <cassert>
#include <filesystem>
#include <fstream>
#include <string>

int main() {
  using namespace exv::ui_shell;
  namespace fs = std::filesystem;

  if (kWindowShadowMarginPx != 0) {
    return 1;
  }

  if (kAppSurfaceAdvancedWindowBounds.width != 972 ||
      kAppSurfaceAdvancedWindowBounds.height != 563) {
    return 1;
  }

  if (kAppSurfaceMinimalWindowBounds.width != 302 ||
      kAppSurfaceMinimalWindowBounds.height != 118) {
    return 1;
  }

  if (kElectronAdvancedWindowBounds.width !=
          kAppSurfaceAdvancedWindowBounds.width + kWindowShadowMarginPx * 2 ||
      kElectronAdvancedWindowBounds.height !=
          kAppSurfaceAdvancedWindowBounds.height + kWindowShadowMarginPx * 2) {
    return 1;
  }

  if (kElectronMinimalWindowBounds.width !=
          kAppSurfaceMinimalWindowBounds.width + kWindowShadowMarginPx * 2 ||
      kElectronMinimalWindowBounds.height !=
          kAppSurfaceMinimalWindowBounds.height + kWindowShadowMarginPx * 2) {
    return 1;
  }

  assert(is_allowed_host_action("status.get"));
  assert(is_allowed_host_action("vpn.connect"));
  assert(is_allowed_host_action("vpn.authInteraction.get"));
  assert(is_allowed_host_action("vpn.authInteraction.respond"));
  assert(!is_allowed_host_action("shell.unknown"));
  if (!is_allowed_host_action("config.import")) {
    return 1;
  }
  if (!is_allowed_host_action("config.export")) {
    return 1;
  }
  if (!is_allowed_host_action("config.reset")) {
    return 1;
  }
  if (!is_allowed_host_action("key.status")) {
    return 1;
  }
  if (!is_allowed_host_action("key.reset")) {
    return 1;
  }
  if (!is_allowed_host_action("maintenance.inspectCore")) {
    return 1;
  }
  if (!is_allowed_host_action("maintenance.killStaleCore")) {
    return 1;
  }
  if (!is_allowed_host_action("window.resolveClosePrompt")) {
    return 1;
  }
  if (!is_allowed_host_action("window.setMode")) {
    return 1;
  }

  const ClosePromptResolution remembered_close =
      parse_close_prompt_resolution(
          R"({"id":4,"action":"window.resolveClosePrompt","payload":{"result":{"action":"tray","remember":true}}})");
  if (remembered_close.action != "tray" || !remembered_close.remember) {
    return 1;
  }
  const ClosePromptResolution cancelled_close =
      parse_close_prompt_resolution(
          R"({"id":5,"action":"window.resolveClosePrompt","payload":{"result":"cancel"}})");
  if (cancelled_close.action != "cancel" || cancelled_close.remember) {
    return 1;
  }
  const ClosePromptResolution invalid_close =
      parse_close_prompt_resolution(
          R"({"id":6,"action":"window.resolveClosePrompt","payload":{"result":{"action":"bogus","remember":true}}})");
  if (invalid_close.action != "cancel" || !invalid_close.remember) {
    return 1;
  }

  const fs::path close_pref_root =
      fs::temp_directory_path() / "exv-ui-shell-close-pref";
  fs::remove_all(close_pref_root);
  if (close_preference_path(close_pref_root).filename() !=
      "close-preference.json") {
    return 1;
  }
  if (read_close_preference(close_pref_root).has_value()) {
    return 1;
  }
  if (!write_close_preference(close_pref_root, "quit")) {
    return 1;
  }
  const auto stored_close_preference = read_close_preference(close_pref_root);
  if (!stored_close_preference || *stored_close_preference != "quit") {
    return 1;
  }
  if (write_close_preference(close_pref_root, "cancel")) {
    return 1;
  }
  fs::remove_all(close_pref_root);

  RendererAssets dev = resolve_renderer_assets("http://127.0.0.1:8288", "");
  assert(dev.kind == RendererAssetKind::DevServer);
  assert(dev.location == "http://127.0.0.1:8288");

  RendererAssets packaged =
      resolve_renderer_assets("", "C:/app/dist/index.html");
  assert(packaged.kind == RendererAssetKind::PackagedFile);
  assert(packaged.location == "C:/app/dist/index.html");

  char program[] = "exv-ui";
  char renderer_arg[] = "--renderer-url";
  char renderer_url[] = "http://127.0.0.1:8288";
  char exv_arg[] = "--exv";
  char exv_path[] = "C:/app/bin/exv.exe";
  char state_arg[] = "--state-dir";
  char state_dir[] = "C:/Users/Alice/AppData/Local/EXV/profile/default";
  char devtools_arg[] = "--devtools";
  char *argv[] = {program, renderer_arg, renderer_url, exv_arg, exv_path,
                  devtools_arg};
  UiShellOptions options = parse_ui_shell_options(6, argv);
  assert(options.renderer_dev_server_url == "http://127.0.0.1:8288");
  assert(options.exv_path == "C:/app/bin/exv.exe");
  assert(options.enable_dev_tools);
  assert(options.state_dir.empty());
  const std::string valid_options_error = validate_ui_shell_options(options);
  if (!valid_options_error.empty()) {
    return 1;
  }

  char *profile_argv[] = {program, renderer_arg, renderer_url, exv_arg,
                          exv_path, state_arg, state_dir};
  UiShellOptions profile_options = parse_ui_shell_options(7, profile_argv);
  if (profile_options.state_dir !=
      "C:/Users/Alice/AppData/Local/EXV/profile/default") {
    return 1;
  }
  if (!validate_ui_shell_options(profile_options).empty()) {
    return 1;
  }

  const fs::path package_root =
      fs::temp_directory_path() / "exv-ui-shell-contract-package";
  fs::remove_all(package_root);
  fs::create_directories(package_root);
  const fs::path args_file = package_root / "exv-ui.args";
  {
    std::ofstream sidecar(args_file);
    sidecar << "--exv\n"
            << "bin/exv.exe\n"
            << "\n"
            << "--renderer-index\n"
            << "webui/index.html\n";
  }

  UiShellOptions sidecar_options = parse_ui_shell_args_file(args_file);
  if (!validate_ui_shell_options(sidecar_options).empty()) {
    return 1;
  }
  if (fs::path(sidecar_options.exv_path) != package_root / "bin/exv.exe") {
    return 1;
  }
  if (fs::path(sidecar_options.packaged_renderer_index) !=
      package_root / "webui/index.html") {
    return 1;
  }
  if (!sidecar_options.state_dir.empty()) {
    return 1;
  }

  {
    std::ofstream sidecar(args_file, std::ios::trunc);
    sidecar << "--exv\n"
            << "bin/exv.exe\n"
            << "--renderer-index\n"
            << "webui/index.html\n"
            << "--state-dir\n"
            << "profile/default\n";
  }
  UiShellOptions relative_profile_sidecar_options =
      parse_ui_shell_args_file(args_file);
  if (fs::path(relative_profile_sidecar_options.state_dir) !=
      package_root / "profile/default") {
    return 1;
  }

  {
    std::ofstream sidecar(args_file, std::ios::trunc);
    sidecar << "--exv\n"
            << "bin/exv.exe\n"
            << "--renderer-index\n"
            << "webui/index.html\n";
  }

  const fs::path bin_dir = package_root / "bin";
  const fs::path webui_dir = package_root / "webui";
  fs::create_directories(bin_dir);
  fs::create_directories(webui_dir);
  std::ofstream(bin_dir / "exv.exe").close();
  std::ofstream(webui_dir / "index.html").close();
  if (!validate_packaged_ui_shell_options(sidecar_options, package_root).empty()) {
    return 1;
  }

  {
    std::ofstream sidecar(args_file, std::ios::trunc);
    sidecar << "--exv\n"
            << "../outside-exv.exe\n"
            << "--renderer-index\n"
            << "webui/index.html\n";
  }
  UiShellOptions escaping_sidecar_options = parse_ui_shell_args_file(args_file);
  if (validate_packaged_ui_shell_options(escaping_sidecar_options, package_root) !=
      "packaged --exv path escapes package root") {
    return 1;
  }

  fs::remove(bin_dir / "exv.exe");
  {
    std::ofstream sidecar(args_file, std::ios::trunc);
    sidecar << "--exv\n"
            << "bin/exv.exe\n"
            << "--renderer-index\n"
            << "webui/index.html\n";
  }
  UiShellOptions missing_target_sidecar_options = parse_ui_shell_args_file(args_file);
  if (validate_packaged_ui_shell_options(missing_target_sidecar_options,
                                         package_root) !=
      "packaged --exv path does not exist") {
    return 1;
  }
  std::ofstream(bin_dir / "exv.exe").close();

  fs::remove(args_file);
  UiShellOptions missing_sidecar_options =
      load_packaged_ui_shell_options(package_root / "exv-ui.exe");
  if (validate_ui_shell_options(missing_sidecar_options) !=
      "missing required --exv path") {
    return 1;
  }

  std::ofstream(args_file, std::ios::trunc).close();
  UiShellOptions empty_sidecar_options = parse_ui_shell_args_file(args_file);
  if (validate_ui_shell_options(empty_sidecar_options) !=
      "missing required --exv path") {
    return 1;
  }

  std::ofstream(args_file) << "--exv\n"
                           << "bin/exv.exe\n"
                           << "--renderer-index\n"
                           << "webui/index.html\n";
  UiShellOptions packaged_options =
      load_packaged_ui_shell_options(package_root / "exv-ui.exe");
  if (!validate_ui_shell_options(packaged_options).empty()) {
    return 1;
  }
  if (fs::path(packaged_options.exv_path) != package_root / "bin/exv.exe") {
    return 1;
  }
  if (fs::path(packaged_options.packaged_renderer_index) !=
      package_root / "webui/index.html") {
    return 1;
  }

  char *no_arg_argv[] = {program};
  UiShellOptions no_arg_resolved = resolve_ui_shell_options(
      1, no_arg_argv, package_root / "exv-ui.exe");
  if (!validate_ui_shell_options(no_arg_resolved).empty()) {
    return 1;
  }
  if (fs::path(no_arg_resolved.exv_path) != package_root / "bin/exv.exe") {
    return 1;
  }
  if (fs::path(no_arg_resolved.packaged_renderer_index) !=
      package_root / "webui/index.html") {
    return 1;
  }

  char *missing_exv_argv[] = {program, renderer_arg, renderer_url};
  UiShellOptions explicit_invalid_resolved = resolve_ui_shell_options(
      3, missing_exv_argv, package_root / "exv-ui.exe");
  if (validate_ui_shell_options(explicit_invalid_resolved) !=
      "missing required --exv path") {
    return 1;
  }

  UiShellOptions explicit_resolved = resolve_ui_shell_options(
      6, argv, package_root / "exv-ui.exe");
  if (explicit_resolved.renderer_dev_server_url != "http://127.0.0.1:8288") {
    return 1;
  }
  if (explicit_resolved.exv_path != "C:/app/bin/exv.exe") {
    return 1;
  }
  if (!explicit_resolved.enable_dev_tools) {
    return 1;
  }

  UiShellOptions explicit_options = parse_ui_shell_options(6, argv);
  if (explicit_options.renderer_dev_server_url != "http://127.0.0.1:8288") {
    return 1;
  }
  if (explicit_options.exv_path != "C:/app/bin/exv.exe") {
    return 1;
  }
  if (!explicit_options.enable_dev_tools) {
    return 1;
  }
  if (!validate_ui_shell_options(explicit_options).empty()) {
    return 1;
  }

  fs::remove_all(package_root);

  UiShellOptions missing_exv;
  missing_exv.renderer_dev_server_url = "http://127.0.0.1:8288";
  if (validate_ui_shell_options(missing_exv) != "missing required --exv path") {
    return 1;
  }

  UiShellOptions missing_renderer;
  missing_renderer.exv_path = "C:/app/bin/exv.exe";
  if (validate_ui_shell_options(missing_renderer) !=
      "missing required renderer URL or index path") {
    return 1;
  }

  UiShellOptions both_renderers;
  both_renderers.renderer_dev_server_url = "http://127.0.0.1:8288";
  both_renderers.packaged_renderer_index = "C:/app/dist/index.html";
  both_renderers.exv_path = "C:/app/bin/exv.exe";
  if (validate_ui_shell_options(both_renderers) !=
      "choose either --renderer-url or --renderer-index, not both") {
    return 1;
  }

  {
    bool core_invoked = false;
    const std::string forwarded = handle_host_request(
        R"({"id":7,"action":"maintenance.inspectCore","payload":{}})",
        [&](const CoreRpcRequest &request) {
          core_invoked = true;
          assert(request.action == "maintenance.inspectCore");
          CoreRpcResponse response;
          response.id = 7;
          response.ok = true;
          response.data_json = R"({"state":"normal","risk":"low"})";
          return response;
        });
    if (!core_invoked) {
      return 1;
    }
    if (forwarded !=
        R"({"id":7,"ok":true,"data":{"risk":"low","state":"normal"}})") {
      return 1;
    }
  }

  bool invoked = false;
  const std::string accepted = handle_host_request(
      R"({"id":1,"action":"status.get","payload":{"source":"test"}})",
      [&](const CoreRpcRequest &request) {
        invoked = true;
        assert(request.action == "status.get");
        assert(request.payload_json == R"({"source":"test"})");
        assert(request.request_id == "1");
        CoreRpcResponse response;
        response.id = 1;
        response.ok = true;
        response.data_json = R"({"phase":"idle"})";
        return response;
      });
  assert(invoked);
  assert(accepted == R"({"id":1,"ok":true,"data":{"phase":"idle"}})");

  const std::string rejected = handle_host_request(
      R"({"id":2,"action":"shell.unknown","payload":{}})",
      [](const CoreRpcRequest &) {
        assert(false);
        return CoreRpcResponse{};
      });
  assert(rejected ==
         R"({"id":2,"ok":false,"code":"unknown_action","message":"Unknown desktop action"})");
}
