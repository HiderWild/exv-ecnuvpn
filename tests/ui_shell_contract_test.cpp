#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/renderer_assets.hpp"
#include "app/ui_shell/ui_shell_options.hpp"

#include <cassert>
#include <string>

int main() {
  using namespace ecnuvpn::ui_shell;

  assert(is_allowed_host_action("status.get"));
  assert(is_allowed_host_action("vpn.connect"));
  assert(!is_allowed_host_action("shell.unknown"));

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
  char devtools_arg[] = "--devtools";
  char *argv[] = {program, renderer_arg, renderer_url, exv_arg, exv_path,
                  devtools_arg};
  UiShellOptions options = parse_ui_shell_options(6, argv);
  assert(options.renderer_dev_server_url == "http://127.0.0.1:8288");
  assert(options.exv_path == "C:/app/bin/exv.exe");
  assert(options.enable_dev_tools);

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
