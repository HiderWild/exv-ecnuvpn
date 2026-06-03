#pragma once

#include "vpn_engine/protocol/native_authenticator.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

nlohmann::json to_json(const NativeAuthSession &session);
ValidationResult from_json(const nlohmann::json &payload,
                           NativeAuthSession *session);
nlohmann::json summarize_native_auth_session(const NativeAuthSession &session);

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
