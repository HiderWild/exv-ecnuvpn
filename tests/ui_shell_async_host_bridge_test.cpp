#include "app/ui_shell/async_host_bridge.hpp"
#include "app/ui_shell/core_rpc_client.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <condition_variable>
#include <iostream>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

class FakeTransport final : public exv::ui_shell::CoreRpcTransport {
public:
  explicit FakeTransport(std::string response)
      : response_(std::move(response)) {}

  bool write_line(const std::string &line) override {
    std::lock_guard<std::mutex> lock(mutex_);
    writes.push_back(line);
    return true;
  }

  bool read_line(std::string &line) override {
    if (block_reads) {
      std::unique_lock<std::mutex> lock(read_mutex_);
      read_cv_.wait(lock, [this] { return reads_released_; });
    }
    std::lock_guard<std::mutex> lock(mutex_);
    if (response_consumed_) {
      return false;
    }
    response_consumed_ = true;
    line = response_;
    return true;
  }

  void release_reads() {
    {
      std::lock_guard<std::mutex> lock(read_mutex_);
      reads_released_ = true;
    }
    read_cv_.notify_all();
  }

  std::vector<std::string> writes;
  bool block_reads = false;

private:
  std::string response_;
  bool response_consumed_ = false;
  bool reads_released_ = false;
  std::mutex mutex_;
  std::mutex read_mutex_;
  std::condition_variable read_cv_;
};

class PostedResponses {
public:
  void push(std::string response) {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      responses_.push_back(std::move(response));
    }
    cv_.notify_all();
  }

  bool wait_for_count(std::size_t count, std::chrono::milliseconds timeout) {
    std::unique_lock<std::mutex> lock(mutex_);
    return cv_.wait_for(lock, timeout, [&] {
      return responses_.size() >= count;
    });
  }

  std::vector<std::string> snapshot() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return responses_;
  }

private:
  mutable std::mutex mutex_;
  std::condition_variable cv_;
  std::vector<std::string> responses_;
};

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << "\n";
  return false;
}

} // namespace

int main() {
  using namespace exv::ui_shell;
  bool ok = true;

  {
    FakeTransport transport(R"({"id":44,"ok":true,"data":{"items":[]}})");
    transport.block_reads = true;
    CoreRpcClient client(transport);
    PostedResponses posted;
    AsyncHostBridge bridge(client, [&posted](std::string response) {
      posted.push(std::move(response));
    });

    const auto started = std::chrono::steady_clock::now();
    const bool accepted = bridge.accept_message(
        R"({"id":44,"action":"logs.list","payload":{}})");
    const auto elapsed = std::chrono::steady_clock::now() - started;

    ok = expect(accepted, "bridge should accept valid core requests") && ok;
    ok = expect(elapsed < std::chrono::milliseconds(100),
                "bridge should return before a blocked core response") &&
         ok;
    ok = expect(transport.writes.size() == 1,
                "bridge should forward one request to core") &&
         ok;
    ok = expect(posted.snapshot().empty(),
                "bridge should not post before core has responded") &&
         ok;

    transport.release_reads();
    ok = expect(posted.wait_for_count(1, std::chrono::milliseconds(500)),
                "bridge should post the eventual core response") &&
         ok;
    const auto responses = posted.snapshot();
    if (responses.size() == 1) {
      ok = expect(responses[0] == R"({"id":44,"ok":true,"data":{"items":[]}})",
                  "bridge should preserve response id and data") &&
           ok;
    }
  }

  {
    FakeTransport transport(R"({"id":0,"ok":true,"data":{}})");
    CoreRpcClient client(transport);
    PostedResponses posted;
    AsyncHostBridge bridge(client, [&posted](std::string response) {
      posted.push(std::move(response));
    });

    ok = expect(bridge.accept_message(
                    R"({"id":7,"action":"window.setMode","payload":{"mode":"minimal"}})"),
                "bridge should accept local window actions") &&
         ok;
    ok = expect(transport.writes.empty(),
                "local window actions should not reach core") &&
         ok;
    ok = expect(posted.wait_for_count(1, std::chrono::milliseconds(100)),
                "local window actions should post an immediate ok response") &&
         ok;
    const auto responses = posted.snapshot();
    if (responses.size() == 1) {
      ok = expect(responses[0] == R"({"id":7,"ok":true,"data":{}})",
                  "local window action response should be stable") &&
           ok;
    }
  }

  {
    FakeTransport transport(R"({"id":0,"ok":true,"data":{}})");
    CoreRpcClient client(transport);
    PostedResponses posted;
    AsyncHostBridge bridge(client, [&posted](std::string response) {
      posted.push(std::move(response));
    });

    ok = expect(bridge.accept_message(R"({"id":[],"action":"status.get"})"),
                "bridge should accept malformed renderer requests for async error posting") &&
         ok;
    ok = expect(bridge.accept_message(R"({"id":9,"action":"missing.action"})"),
                "bridge should accept unknown actions for async error posting") &&
         ok;
    ok = expect(transport.writes.empty(),
                "invalid host messages should not reach core") &&
         ok;
    ok = expect(posted.wait_for_count(2, std::chrono::milliseconds(100)),
                "invalid host messages should post stable error responses") &&
         ok;
    const auto responses = posted.snapshot();
    if (responses.size() == 2) {
      const auto invalid_id = nlohmann::json::parse(responses[0]);
      const auto unknown_action = nlohmann::json::parse(responses[1]);
      ok = expect(invalid_id.value("code", "") == "host_bridge_error",
                  "invalid request id should use host_bridge_error") &&
           ok;
      ok = expect(unknown_action.value("id", 0) == 9,
                  "unknown action response should preserve id") &&
           ok;
      ok = expect(unknown_action.value("code", "") == "unknown_action",
                  "unknown action response should use unknown_action") &&
           ok;
    }
  }

  {
    FakeTransport transport(R"({"id":55,"ok":true,"data":{"late":true}})");
    transport.block_reads = true;
    CoreRpcClient client(transport);
    PostedResponses posted;
    {
      AsyncHostBridge bridge(client, [&posted](std::string response) {
        posted.push(std::move(response));
      });
      ok = expect(bridge.accept_message(
                      R"({"id":55,"action":"logs.list","payload":{}})"),
                  "bridge should accept request before shutdown") &&
           ok;
      bridge.shutdown();
    }

    transport.release_reads();
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
    ok = expect(posted.snapshot().empty(),
                "bridge shutdown should suppress late core responses") &&
         ok;
  }

  {
    FakeTransport transport(R"({"id":66,"ok":true,"data":{"late":true}})");
    transport.block_reads = true;
    CoreRpcClient client(transport);
    PostedResponses posted;
    AsyncHostBridge bridge(
        client,
        [&posted](std::string response) { posted.push(std::move(response)); },
        std::chrono::milliseconds(30));

    ok = expect(bridge.accept_message(
                    R"({"id":66,"action":"config.import","payload":{"format":"unprotected","data":"{}"}})"),
                "bridge should accept config import requests") &&
         ok;
    ok = expect(posted.wait_for_count(1, std::chrono::milliseconds(500)),
                "bridge should post an error when core does not answer") &&
         ok;
    const auto responses = posted.snapshot();
    if (responses.size() == 1) {
      const auto timeout = nlohmann::json::parse(responses[0]);
      ok = expect(timeout.value("id", 0) == 66,
                  "timeout response should preserve the renderer request id") &&
           ok;
      ok = expect(timeout.value("ok", true) == false,
                  "timeout response should reject the renderer promise") &&
           ok;
      ok = expect(timeout.value("code", "") == "core_unresponsive",
                  "timeout response should use core_unresponsive") &&
           ok;
    }

    bridge.shutdown();
    transport.release_reads();
    client.shutdown();
  }

  return ok ? 0 : 1;
}
