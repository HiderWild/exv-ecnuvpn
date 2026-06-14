#pragma once
// DEPRECATED: V1 legacy helper internal types.  This header is not included
// by any compilation unit and will be removed once the V1 helper protocol
// is fully decommissioned.  It must NOT depend on vpn_engine/*.

#include "core/config/config.hpp"

#include <nlohmann/json.hpp>
#include <string>

namespace ecnuvpn {
namespace helper {
namespace internal {

struct ValidationResult {
  bool ok = true;
  std::string code;
  std::string message;
};

struct WorkerStartRequest {
  unsigned int uid = 0;
  unsigned int gid = 0;
  std::string config_json;   // serialized Config (replaces SupervisorStartPayload)
  std::string password;      // DEPRECATED: V1 legacy
  int retry_limit = 0;       // DEPRECATED: V1 legacy
  std::string home;
  std::string config_dir;
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

ValidationResult
resolve_request_owner(const nlohmann::json &request,
                      const PeerIdentity &peer_identity,
                      SessionOwner *owner);

ValidationResult
validate_same_owner(const SessionOwner &owner,
                    const PeerIdentity &peer_identity);

ValidationResult
prepare_worker_start_request(const nlohmann::json &start_request,
                             unsigned int owner_uid, unsigned int owner_gid,
                             const std::string &default_home,
                             const std::string &default_config_dir,
                             nlohmann::json *worker_request);

ValidationResult
decode_worker_start_request(const nlohmann::json &worker_request,
                            WorkerStartRequest *out);

} // namespace internal
} // namespace helper
} // namespace ecnuvpn
