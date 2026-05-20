#pragma once

#include <string>

namespace ecnuvpn {
namespace platform {

struct ConfigDefaults {
  bool disable_dtls = false;
  std::string useragent;
  std::string log_file;
  bool webui_enabled = true;
};

const ConfigDefaults &config_defaults();

} // namespace platform
} // namespace ecnuvpn