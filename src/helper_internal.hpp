#pragma once

#include "config.hpp"
#include "platform/common/vpn_supervisor_process.hpp"
#include "vpn_engine/engine.hpp"

#include <nlohmann/json.hpp>
#include <string>

namespace ecnuvpn {
namespace helper {
namespace internal {

struct WorkerStartRequest {
  unsigned int uid = 0;
  unsigned int gid = 0;
  platform::SupervisorStartPayload payload;
};

struct PeerIdentity {
  unsigned int uid = 0;
  unsigned int gid = 0;
  std::string sid;
  bool verified = false;
};

struct SessionOwner {
  unsigned int uid = 0;
  unsigned int gid = 0;
  std::string sid;
};

vpn_engine::ValidationResult
resolve_request_owner(const nlohmann::json &request,
                      const PeerIdentity &peer_identity,
                      SessionOwner *owner);

vpn_engine::ValidationResult
validate_same_owner(const SessionOwner &owner,
                    const PeerIdentity &peer_identity);

vpn_engine::ValidationResult
prepare_worker_start_request(const nlohmann::json &start_request,
                             unsigned int owner_uid, unsigned int owner_gid,
                             const std::string &default_home,
                             const std::string &default_config_dir,
                             nlohmann::json *worker_request);

vpn_engine::ValidationResult
decode_worker_start_request(const nlohmann::json &worker_request,
                            WorkerStartRequest *out);

} // namespace internal
} // namespace helper
} // namespace ecnuvpn
