#pragma once

#include <string>

namespace exv::ui_shell {

enum class RendererAssetKind {
  DevServer,
  PackagedFile,
};

struct RendererAssets {
  RendererAssetKind kind;
  std::string location;
};

RendererAssets resolve_renderer_assets(const std::string &dev_server_url,
                                       const std::string &packaged_index);

} // namespace exv::ui_shell
