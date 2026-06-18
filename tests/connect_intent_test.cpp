#include "core/tunnel_controller/vpn_connect_job.hpp"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <future>
#include <iostream>
#include <mutex>
#include <string>

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

  void wait() {
    std::unique_lock<std::mutex> lock(mutex_);
    cv_.wait(lock, [&] { return open_; });
  }

private:
  std::mutex mutex_;
  std::condition_variable cv_;
  bool open_ = false;
};

bool wait_until(const std::function<bool()> &predicate,
                std::chrono::milliseconds timeout) {
  const auto deadline = std::chrono::steady_clock::now() + timeout;
  while (std::chrono::steady_clock::now() < deadline) {
    if (predicate()) {
      return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  return predicate();
}

exv::core::PendingConnectRequest request(std::string profile) {
  exv::core::PendingConnectRequest out;
  out.profile_id = std::move(profile);
  out.server = "vpn.example.edu";
  out.has_password = true;
  return out;
}

} // namespace

int main() {
  using exv::core::DesiredVpnIntent;
  using exv::core::VpnConnectJobOwner;

  bool ok = true;

  {
    VpnConnectJobOwner owner;
    Gate release;
    std::atomic<int> starts{0};

    auto state = owner.submit_connect(
        request("first"),
        [&](std::stop_token, std::uint64_t) {
          ++starts;
          release.wait();
        });

    ok = expect(state.accepted, "connect_when_idle_starts_job: accepted") && ok;
    ok = expect(state.active, "connect_when_idle_starts_job: active") && ok;
    ok = expect(!state.coalesced, "connect_when_idle_starts_job: not coalesced") && ok;
    ok = expect(state.desired_connected,
                "connect_when_idle_starts_job: desired connected") && ok;
    ok = expect(!state.job_id.empty(),
                "connect_when_idle_starts_job: job id") && ok;
    ok = expect(wait_until([&] { return starts.load() == 1; },
                           std::chrono::milliseconds(500)),
                "connect_when_idle_starts_job: run starts") && ok;
    release.open();
  }

  {
    VpnConnectJobOwner owner;
    Gate release;
    std::atomic<bool> stop_seen{false};

    auto connect = owner.submit_connect(
        request("cancel"),
        [&](std::stop_token stop, std::uint64_t) {
          while (!stop.stop_requested()) {
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
          }
          stop_seen = true;
          release.wait();
        });
    auto cancel = owner.submit_disconnect("user_cancelled_connect");

    ok = expect(connect.active, "cancel_while_connecting: connect active") && ok;
    ok = expect(cancel.accepted, "cancel_while_connecting: disconnect accepted") && ok;
    ok = expect(cancel.cancelling, "cancel_while_connecting: cancelling") && ok;
    ok = expect(cancel.user_cancelled, "cancel_while_connecting: user cancelled") && ok;
    ok = expect(!cancel.desired_connected,
                "cancel_while_connecting: desired disconnected") && ok;
    ok = expect(cancel.last_error_code.empty(),
                "cancel_while_connecting: no last error") && ok;
    ok = expect(wait_until([&] { return stop_seen.load(); },
                           std::chrono::milliseconds(500)),
                "cancel_while_connecting: stop requested") && ok;
    release.open();
  }

  {
    VpnConnectJobOwner owner;
    Gate release;
    std::atomic<int> starts{0};
    auto run = [&](std::stop_token, std::uint64_t) {
      ++starts;
      release.wait();
    };

    owner.submit_connect(request("a"), run);
    ok = expect(wait_until([&] { return starts.load() == 1; },
                           std::chrono::milliseconds(500)),
                "rapid_disconnect: first workflow starts") && ok;
    owner.submit_disconnect("user_cancelled_connect");
    owner.submit_connect(request("b"), run);
    auto latest = owner.submit_disconnect("user_cancelled_connect");

    ok = expect(latest.active, "rapid_disconnect: one active workflow") && ok;
    ok = expect(latest.cancelling, "rapid_disconnect: cancelling") && ok;
    ok = expect(!latest.desired_connected,
                "rapid_disconnect: latest intent disconnect") && ok;
    ok = expect(starts.load() == 1,
                "rapid_disconnect: no duplicate workflow while busy") && ok;
    release.open();
    ok = expect(wait_until([&] { return !owner.snapshot().active; },
                           std::chrono::milliseconds(500)),
                "rapid_disconnect: cleanup reaches idle") && ok;
    owner.reconcile_after_idle();
    ok = expect(starts.load() == 1,
                "rapid_disconnect: no reconnect after latest disconnect") && ok;
  }

  {
    VpnConnectJobOwner owner;
    Gate release_first;
    Gate release_second;
    std::atomic<int> starts{0};
    auto run = [&](std::stop_token, std::uint64_t) {
      const int index = ++starts;
      if (index == 1) {
        release_first.wait();
      } else {
        release_second.wait();
      }
    };

    owner.submit_connect(request("a"), run);
    ok = expect(wait_until([&] { return starts.load() == 1; },
                           std::chrono::milliseconds(500)),
                "rapid_reconnect: first workflow starts") && ok;
    owner.submit_disconnect("user_cancelled_connect");
    owner.submit_connect(request("b"), run);

    ok = expect(starts.load() == 1,
                "rapid_reconnect: no duplicate before cleanup") && ok;
    release_first.open();
    ok = expect(wait_until([&] { return !owner.snapshot().active; },
                           std::chrono::milliseconds(500)),
                "rapid_reconnect: first cleanup reaches idle") && ok;
    owner.reconcile_after_idle();
    ok = expect(wait_until([&] { return starts.load() == 2; },
                           std::chrono::milliseconds(500)),
                "rapid_reconnect: one newer connect starts after idle") && ok;
    release_second.open();
  }

  {
    VpnConnectJobOwner owner;
    std::atomic<int> starts{0};
    auto fail_run = [&](std::stop_token, std::uint64_t) {
      ++starts;
      throw std::runtime_error("auth_failed");
    };

    owner.submit_connect(request("fail-1"), fail_run);
    ok = expect(wait_until([&] { return !owner.snapshot().active; },
                           std::chrono::milliseconds(500)),
                "failed_job: failed job reaches idle") && ok;
    owner.reconcile_after_idle();
    ok = expect(starts.load() == 1,
                "failed_job: no retry without newer epoch") && ok;
    owner.submit_connect(request("fail-2"), fail_run);
    ok = expect(wait_until([&] { return starts.load() == 2; },
                           std::chrono::milliseconds(500)),
                "failed_job: later user epoch starts new job") && ok;
  }

  if (ok) {
    std::cout << "connect_intent_test: all assertions passed\n";
  }
  return ok ? 0 : 1;
}
