#pragma once

#include <functional>
#include <nlohmann/json.hpp>
#include <stop_token>
#include <string>
#include <string_view>

namespace exv::core {

enum class ConnectBranch {
  BackendHelperReady,
  PlatformReady,
  ProtocolHandshake,
};

std::string_view connect_branch_name(ConnectBranch branch);

struct ConnectBranchResult {
  ConnectBranch branch;
  bool ok = false;
  std::string code;
  std::string message;
  nlohmann::json payload = nlohmann::json::object();
};

struct ConnectPipelineResult {
  bool ok = false;
  std::string first_failure_branch;
  std::string code;
  std::string message;
  nlohmann::json backend = nlohmann::json::object();
  nlohmann::json platform = nlohmann::json::object();
  nlohmann::json handshake = nlohmann::json::object();
};

class ConnectPipeline {
public:
  using BranchFn = std::function<ConnectBranchResult(std::stop_token)>;
  using LateFailureLogger =
      std::function<void(const ConnectBranchResult&, std::string_view first_code)>;

  ConnectPipeline(std::string job_id, LateFailureLogger logger);

  ConnectPipelineResult run(BranchFn backend_helper,
                            BranchFn platform_ready,
                            BranchFn protocol_handshake,
                            std::stop_token external_stop);

private:
  std::string job_id_;
  LateFailureLogger logger_;
};

} // namespace exv::core
