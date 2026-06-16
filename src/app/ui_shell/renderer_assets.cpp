#include "app/ui_shell/renderer_assets.hpp"

namespace ecnuvpn::ui_shell {

RendererAssets resolve_renderer_assets(const std::string &dev_server_url,
                                       const std::string &packaged_index) {
  if (!dev_server_url.empty()) {
    return {RendererAssetKind::DevServer, dev_server_url};
  }
  return {RendererAssetKind::PackagedFile, packaged_index};
}

} // namespace ecnuvpn::ui_shell
