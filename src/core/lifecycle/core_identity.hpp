#pragma once

#include "platform/common/core_lifecycle_contract.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace exv::core::lifecycle {

struct CoreIdentity {
    std::string core_instance_id;
    int pid = 0;
    std::string core_path;
    std::string started_at;
};

CoreIdentity make_core_identity();
nlohmann::json core_hello_payload(const CoreIdentity& identity);
bool accepts_contract_version(std::string_view requested);

} // namespace exv::core::lifecycle
