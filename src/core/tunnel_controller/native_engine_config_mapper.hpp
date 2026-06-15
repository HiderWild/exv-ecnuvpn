#pragma once

#include "core/config/config.hpp"
#include "vpn_engine/engine.hpp"

#include <string>

namespace exv::core {

ecnuvpn::vpn_engine::ValidationResult
validate_native_app_config(const ecnuvpn::Config &cfg);

ecnuvpn::vpn_engine::ValidationResult make_native_engine_config(
    const ecnuvpn::Config &cfg, const std::string &plaintext_password,
    ecnuvpn::vpn_engine::VpnEngineConfig *out);

} // namespace exv::core
