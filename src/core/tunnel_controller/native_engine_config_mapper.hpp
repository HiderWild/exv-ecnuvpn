#pragma once

#include "core/config/config.hpp"
#include "vpn_engine/engine.hpp"

#include <string>

namespace exv::core {

exv::vpn_engine::ValidationResult
validate_native_app_config(const exv::Config &cfg);

exv::vpn_engine::ValidationResult make_native_engine_config(
    const exv::Config &cfg, const std::string &plaintext_password,
    exv::vpn_engine::VpnEngineConfig *out);

} // namespace exv::core
