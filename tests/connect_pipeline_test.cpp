#include "core/tunnel_controller/connect_pipeline.hpp"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <iostream>
#include <mutex>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << "\n";
  return false;
}

class Gate {
public:
  void open() {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      open_ = true;
    }
    cv_.notify_all();
  }

  bool wait_until_open(std::stop_token stop) {
    std::unique_lock<std::mutex> lock(mutex_);
    cv_.wait(lock, stop, [&] { return open_; });
    return open_;
  }

private:
  std::mutex mutex_;
  std::condition_variable_any cv_;
  bool open_ = false;
};

exv::core::ConnectBranchResult ok_result(exv::core::ConnectBranch branch) {
  exv::core::ConnectBranchResult result;
  result.branch = branch;
  result.ok = true;
  result.payload = nlohmann::json{{"branch", exv::core::connect_branch_name(branch)}};
  return result;
}

exv::core::ConnectBranchResult fail_result(exv::core::ConnectBranch branch,
                                           std::string code) {
  exv::core::ConnectBranchResult result;
  result.branch = branch;
  result.ok = false;
  result.code = std::move(code);
  result.message = "failed";
  return result;
}

} // namespace

int main() {
  using exv::core::ConnectBranch;
  using exv::core::ConnectPipeline;

  bool ok = true;

  {
    Gate backend_release;
    Gate handshake_release;
    std::atomic<bool> backend_cancel_requested{false};
    std::atomic<bool> handshake_cancel_requested{false};
    std::mutex late_mutex;
    std::vector<std::string> late_failures;

    ConnectPipeline pipeline(
        "job-first-failure",
        [&](const exv::core::ConnectBranchResult &result,
            std::string_view first_code) {
          std::lock_guard<std::mutex> lock(late_mutex);
          late_failures.push_back(result.code + ":" + std::string(first_code));
        });

    const auto start = std::chrono::steady_clock::now();
    auto result = pipeline.run(
        [&](std::stop_token stop) {
          backend_cancel_requested = stop.stop_requested();
          backend_release.wait_until_open(stop);
          backend_cancel_requested =
              backend_cancel_requested.load() || stop.stop_requested();
          return ok_result(ConnectBranch::BackendHelperReady);
        },
        [&](std::stop_token) {
          return fail_result(ConnectBranch::PlatformReady, "wintun_missing");
        },
        [&](std::stop_token stop) {
          handshake_cancel_requested = stop.stop_requested();
          handshake_release.wait_until_open(stop);
          handshake_cancel_requested =
              handshake_cancel_requested.load() || stop.stop_requested();
          return fail_result(ConnectBranch::ProtocolHandshake, "auth_failed");
        },
        std::stop_token{});
    const auto elapsed = std::chrono::steady_clock::now() - start;

    ok = expect(!result.ok, "first_failure: result fails") && ok;
    ok = expect(result.code == "wintun_missing",
                "first_failure: first branch code wins") && ok;
    ok = expect(result.first_failure_branch == "platform_ready",
                "first_failure: branch name is reported") && ok;
    ok = expect(elapsed < std::chrono::milliseconds(200),
                "first_failure: returns before slow branches release") && ok;
    const auto cancel_deadline = std::chrono::steady_clock::now() +
                                 std::chrono::milliseconds(500);
    while (std::chrono::steady_clock::now() < cancel_deadline &&
           !backend_cancel_requested.load() &&
           !handshake_cancel_requested.load()) {
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    ok = expect(backend_cancel_requested.load() ||
                    handshake_cancel_requested.load(),
                "first_failure: cancellation is requested for losing branches") && ok;

    backend_release.open();
    handshake_release.open();
    const auto deadline = std::chrono::steady_clock::now() +
                          std::chrono::milliseconds(500);
    while (std::chrono::steady_clock::now() < deadline) {
      std::lock_guard<std::mutex> lock(late_mutex);
      if (!late_failures.empty()) {
        break;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    {
      std::lock_guard<std::mutex> lock(late_mutex);
      ok = expect(late_failures.size() == 1,
                  "first_failure: late non-cancel failure is logged once") && ok;
      if (!late_failures.empty()) {
        ok = expect(late_failures[0] == "auth_failed:wintun_missing",
                    "first_failure: late failure records discarded reason") && ok;
      }
    }
  }

  {
    Gate backend_release;
    Gate platform_release;
    std::atomic<int> completed{0};

    ConnectPipeline pipeline("job-success", nullptr);
    std::thread release_backend([&] {
      std::this_thread::sleep_for(std::chrono::milliseconds(30));
      backend_release.open();
    });
    std::thread release_platform([&] {
      std::this_thread::sleep_for(std::chrono::milliseconds(60));
      platform_release.open();
    });

    auto result = pipeline.run(
        [&](std::stop_token stop) {
          backend_release.wait_until_open(stop);
          ++completed;
          return ok_result(ConnectBranch::BackendHelperReady);
        },
        [&](std::stop_token stop) {
          platform_release.wait_until_open(stop);
          ++completed;
          return ok_result(ConnectBranch::PlatformReady);
        },
        [&](std::stop_token) {
          ++completed;
          return ok_result(ConnectBranch::ProtocolHandshake);
        },
        std::stop_token{});

    release_backend.join();
    release_platform.join();
    ok = expect(result.ok, "success: all branches succeed") && ok;
    ok = expect(completed.load() == 3,
                "success: result waits for all branch results") && ok;
    ok = expect(result.backend.value("branch", "") == "backend_helper_ready",
                "success: backend payload stored") && ok;
    ok = expect(result.platform.value("branch", "") == "platform_ready",
                "success: platform payload stored") && ok;
    ok = expect(result.handshake.value("branch", "") == "protocol_handshake",
                "success: handshake payload stored") && ok;
  }

  if (ok) {
    std::cout << "connect_pipeline_test: all assertions passed\n";
  }
  return ok ? 0 : 1;
}
