#include "core/tunnel_controller/connect_pipeline.hpp"

#include <condition_variable>
#include <memory>
#include <mutex>
#include <thread>
#include <utility>
#include <vector>

namespace exv::core {
namespace {

struct PipelineSharedState {
  explicit PipelineSharedState(ConnectPipeline::LateFailureLogger logger_value)
      : logger(std::move(logger_value)) {}

  std::mutex mutex;
  std::condition_variable cv;
  std::stop_source stop_source;
  ConnectPipelineResult result;
  ConnectPipeline::LateFailureLogger logger;
  bool failed = false;
  int completed = 0;
};

void store_success_payload(ConnectPipelineResult& result,
                           const ConnectBranchResult& branch_result) {
  switch (branch_result.branch) {
  case ConnectBranch::BackendHelperReady:
    result.backend = branch_result.payload;
    break;
  case ConnectBranch::PlatformReady:
    result.platform = branch_result.payload;
    break;
  case ConnectBranch::ProtocolHandshake:
    result.handshake = branch_result.payload;
    break;
  }
}

ConnectBranchResult cancellation_result(ConnectBranch branch) {
  ConnectBranchResult result;
  result.branch = branch;
  result.ok = false;
  result.code = "cancelled";
  result.message = "Connect branch cancelled";
  return result;
}

} // namespace

std::string_view connect_branch_name(ConnectBranch branch) {
  switch (branch) {
  case ConnectBranch::BackendHelperReady:
    return "backend_helper_ready";
  case ConnectBranch::PlatformReady:
    return "platform_ready";
  case ConnectBranch::ProtocolHandshake:
    return "protocol_handshake";
  }
  return "unknown";
}

ConnectPipeline::ConnectPipeline(std::string job_id, LateFailureLogger logger)
    : job_id_(std::move(job_id)), logger_(std::move(logger)) {}

ConnectPipelineResult ConnectPipeline::run(BranchFn backend_helper,
                                           BranchFn platform_ready,
                                           BranchFn protocol_handshake,
                                           std::stop_token external_stop) {
  (void)job_id_;
  auto shared = std::make_shared<PipelineSharedState>(logger_);
  std::vector<std::thread> threads;
  threads.reserve(3);

  auto launch = [&](ConnectBranch branch, BranchFn fn) {
    threads.emplace_back([shared, branch, fn = std::move(fn),
                          external_stop]() mutable {
      ConnectBranchResult result;
      if (external_stop.stop_requested()) {
        result = cancellation_result(branch);
      } else {
        result = fn(shared->stop_source.get_token());
      }

      ConnectPipeline::LateFailureLogger late_logger;
      std::string first_code;
      bool should_log_late_failure = false;
      {
        std::lock_guard<std::mutex> lock(shared->mutex);
        ++shared->completed;
        if (!result.ok) {
          if (!shared->failed && result.code != "cancelled") {
            shared->failed = true;
            shared->result.ok = false;
            shared->result.first_failure_branch =
                std::string(connect_branch_name(result.branch));
            shared->result.code = result.code;
            shared->result.message = result.message;
            shared->stop_source.request_stop();
          } else if (shared->failed && result.code != "cancelled") {
            should_log_late_failure = true;
            late_logger = shared->logger;
            first_code = shared->result.code;
          }
        } else if (!shared->failed) {
          store_success_payload(shared->result, result);
        }
      }

      if (should_log_late_failure && late_logger) {
        late_logger(result, first_code);
      }
      shared->cv.notify_all();
    });
  };

  launch(ConnectBranch::BackendHelperReady, std::move(backend_helper));
  launch(ConnectBranch::PlatformReady, std::move(platform_ready));
  launch(ConnectBranch::ProtocolHandshake, std::move(protocol_handshake));

  {
    std::unique_lock<std::mutex> lock(shared->mutex);
    shared->cv.wait(lock, [&] {
      return shared->failed || shared->completed == 3 ||
             external_stop.stop_requested();
    });
    if (external_stop.stop_requested() && !shared->failed) {
      shared->failed = true;
      shared->result.ok = false;
      shared->result.code = "cancelled";
      shared->result.message = "Connect pipeline cancelled";
      shared->stop_source.request_stop();
    }
  }

  if (shared->failed) {
    for (auto& thread : threads) {
      if (thread.joinable()) {
        thread.detach();
      }
    }
    std::lock_guard<std::mutex> lock(shared->mutex);
    return shared->result;
  }

  for (auto& thread : threads) {
    if (thread.joinable()) {
      thread.join();
    }
  }

  std::lock_guard<std::mutex> lock(shared->mutex);
  shared->result.ok = true;
  return shared->result;
}

} // namespace exv::core
