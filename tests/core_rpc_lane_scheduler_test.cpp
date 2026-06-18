#include "core/rpc/lane_scheduler.hpp"

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

class ManualGate {
public:
  void open() {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      open_ = true;
    }
    cv_.notify_all();
  }

  void wait() {
    std::unique_lock<std::mutex> lock(mutex_);
    cv_.wait(lock, [this] { return open_; });
  }

private:
  std::mutex mutex_;
  std::condition_variable cv_;
  bool open_ = false;
};

class EventLog {
public:
  void push(std::string item) {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      items_.push_back(std::move(item));
    }
    cv_.notify_all();
  }

  bool wait_for_size(std::size_t size, std::chrono::milliseconds timeout) {
    std::unique_lock<std::mutex> lock(mutex_);
    return cv_.wait_for(lock, timeout,
                        [&] { return items_.size() >= size; });
  }

  std::vector<std::string> snapshot() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return items_;
  }

private:
  mutable std::mutex mutex_;
  std::condition_variable cv_;
  std::vector<std::string> items_;
};

} // namespace

int main() {
  using exv::core_api::LaneScheduler;
  using exv::core_api::RpcLane;
  using exv::core_api::RpcRequest;
  using exv::core_api::RpcResponse;

  bool ok = true;

  {
    LaneScheduler scheduler;
    EventLog events;
    ManualGate first_can_finish;
    std::atomic<bool> first_started{false};

    RpcRequest first;
    first.action = "vpn.connect";
    first.request_id = "fifo-a";
    RpcRequest second;
    second.action = "vpn.disconnect";
    second.request_id = "fifo-b";

    ok = expect(scheduler.schedule(RpcLane::VpnControl, first,
                                   [&](const RpcRequest &) {
                                     first_started.store(true);
                                     events.push("a-start");
                                     first_can_finish.wait();
                                     events.push("a-finish");
                                     return RpcResponse{true, "{}", "", "", ""};
                                   },
                                   [](RpcResponse) {}),
                "same-lane first work should be accepted") &&
         ok;
    ok = expect(scheduler.schedule(RpcLane::VpnControl, second,
                                   [&](const RpcRequest &) {
                                     events.push("b-run");
                                     return RpcResponse{true, "{}", "", "", ""};
                                   },
                                   [](RpcResponse) {}),
                "same-lane second work should be accepted") &&
         ok;

    ok = expect(events.wait_for_size(1, std::chrono::milliseconds(500)),
                "same-lane first work should start") &&
         ok;
    ok = expect(first_started.load(), "same-lane first marker should be set") &&
         ok;
    ok = expect(events.snapshot() == std::vector<std::string>{"a-start"},
                "same-lane second work must wait behind first work") &&
         ok;
    first_can_finish.open();
    ok = expect(events.wait_for_size(3, std::chrono::milliseconds(500)),
                "same-lane queued work should drain after first release") &&
         ok;
    ok = expect((events.snapshot() ==
                 std::vector<std::string>{"a-start", "a-finish", "b-run"}),
                "same-lane work should run FIFO") &&
         ok;
    scheduler.stop();
  }

  {
    LaneScheduler scheduler;
    EventLog events;
    ManualGate vpn_can_finish;

    RpcRequest vpn;
    vpn.action = "vpn.connect";
    vpn.request_id = "vpn";
    RpcRequest logs;
    logs.action = "logs.list";
    logs.request_id = "logs";

    ok = expect(scheduler.schedule(RpcLane::VpnControl, vpn,
                                   [&](const RpcRequest &) {
                                     events.push("vpn-start");
                                     vpn_can_finish.wait();
                                     events.push("vpn-finish");
                                     return RpcResponse{true, "{}", "", "", ""};
                                   },
                                   [](RpcResponse) {}),
                "vpn lane work should be accepted") &&
         ok;
    ok = expect(events.wait_for_size(1, std::chrono::milliseconds(500)),
                "vpn lane work should start") &&
         ok;
    ok = expect(scheduler.schedule(RpcLane::Diagnostics, logs,
                                   [&](const RpcRequest &) {
                                     events.push("logs-run");
                                     return RpcResponse{true, "[]", "", "", ""};
                                   },
                                   [](RpcResponse) {}),
                "diagnostics lane work should be accepted") &&
         ok;
    ok = expect(events.wait_for_size(2, std::chrono::milliseconds(500)),
                "diagnostics lane must run while vpn lane is blocked") &&
         ok;
    ok = expect((events.snapshot() ==
                 std::vector<std::string>{"vpn-start", "logs-run"}),
                "different lanes should run concurrently") &&
         ok;
    vpn_can_finish.open();
    scheduler.stop();
  }

  {
    LaneScheduler scheduler;
    EventLog events;
    ManualGate first_can_finish;

    RpcRequest connect;
    connect.action = "vpn.connect";
    connect.request_id = "connect";
    RpcRequest update;
    update.action = "vpn.connect";
    update.request_id = "intent-update";

    ok = expect(scheduler.schedule(RpcLane::VpnControl, connect,
                                   [&](const RpcRequest &) {
                                     events.push("connect-start");
                                     first_can_finish.wait();
                                     events.push("connect-finish");
                                     return RpcResponse{true, "{}", "", "", ""};
                                   },
                                   [](RpcResponse) {}),
                "first vpn control request should be accepted") &&
         ok;
    ok = expect(scheduler.schedule(RpcLane::VpnControl, update,
                                   [&](const RpcRequest &) {
                                     events.push("intent-update");
                                     return RpcResponse{true, "{}", "", "", ""};
                                   },
                                   [](RpcResponse) {}),
                "second vpn control request should be accepted") &&
         ok;

    ok = expect(events.wait_for_size(1, std::chrono::milliseconds(500)),
                "first vpn control request should start") &&
         ok;
    ok = expect(events.snapshot() == std::vector<std::string>{"connect-start"},
                "vpn control requests should remain serialized") &&
         ok;
    first_can_finish.open();
    ok = expect(events.wait_for_size(3, std::chrono::milliseconds(500)),
                "second vpn control request should run after first") &&
         ok;
    ok = expect((events.snapshot() ==
                 std::vector<std::string>{"connect-start", "connect-finish",
                                          "intent-update"}),
                "vpn control serialization order should be stable") &&
         ok;
    scheduler.stop();
  }

  {
    LaneScheduler scheduler;
    RpcRequest req;
    req.action = "logs.list";
    req.request_id = "after-stop";
    scheduler.stop();
    ok = expect(!scheduler.schedule(RpcLane::Diagnostics, req,
                                    [](const RpcRequest &) {
                                      return RpcResponse{true, "[]", "", "", ""};
                                    },
                                    [](RpcResponse) {}),
                "scheduler should reject work after stop") &&
         ok;
  }

  if (ok) {
    std::cout << "core_rpc_lane_scheduler_test: all assertions passed\n";
  }
  return ok ? 0 : 1;
}
