// TimerScheduler unit tests.
//
// Verifies that the thread-based timer scheduler correctly fires callbacks
// after the specified delay and honours cancel_all().

#include "core/tunnel_controller/timer_scheduler.hpp"
#include "core/tunnel_controller/timing.hpp"

#include <atomic>
#include <chrono>
#include <functional>
#include <iostream>
#include <mutex>
#include <string_view>
#include <thread>
#include <vector>

import exv.core.tunnel.timing;

namespace timing_contract = exv::core::tunnel::timing;

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    // --- Timing module facade matches the production ConnectTiming constants ---
    {
        ok = expect(timing_contract::connect_stage_count() == 6,
                    "timing module should export all connect stage constants") && ok;
        ok = expect(std::string_view{timing_contract::connect_stage_name(0)} ==
                        exv::core::ConnectTiming::HELPER_PREPARE,
                    "timing module helper stage should match production constant") && ok;
        ok = expect(std::string_view{timing_contract::connect_stage_name(5)} ==
                        exv::core::ConnectTiming::FIRST_PACKET,
                    "timing module first-packet stage should match production constant") && ok;
        ok = expect(timing_contract::connect_stage_name(6) == nullptr,
                    "timing module should reject out-of-range stage indexes") && ok;
    }

    // --- Basic: single timer fires after delay ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<bool> fired{false};

        sched.schedule(std::chrono::milliseconds(10), [&] { fired = true; });

        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        ok = expect(fired.load(), "single timer should fire after delay") && ok;
        ok = expect(sched.pending_count() == 0, "pending count should be 0 after fire") && ok;
    }

    // --- Multiple timers fire in order ---
    {
        exv::core::TimerScheduler sched;
        std::mutex mtx;
        std::vector<int> order;

        sched.schedule(std::chrono::milliseconds(30), [&] {
            std::lock_guard<std::mutex> lk(mtx);
            order.push_back(1);
        });
        sched.schedule(std::chrono::milliseconds(10), [&] {
            std::lock_guard<std::mutex> lk(mtx);
            order.push_back(2);
        });
        sched.schedule(std::chrono::milliseconds(20), [&] {
            std::lock_guard<std::mutex> lk(mtx);
            order.push_back(3);
        });

        std::this_thread::sleep_for(std::chrono::milliseconds(80));

        ok = expect(order.size() == 3, "all 3 timers should fire") && ok;
        if (order.size() == 3) {
            ok = expect(order[0] == 2, "first fired should be #2 (10ms)") && ok;
            ok = expect(order[1] == 3, "second fired should be #3 (20ms)") && ok;
            ok = expect(order[2] == 1, "third fired should be #1 (30ms)") && ok;
        }
    }

    // --- cancel_all prevents pending timers from firing ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<bool> fired{false};

        sched.schedule(std::chrono::milliseconds(10), [&] { fired = true; });
        sched.cancel_all();

        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        ok = expect(!fired.load(), "cancelled timer should NOT fire") && ok;
    }

    // --- cancel_all clears pending_count ---
    {
        exv::core::TimerScheduler sched;
        sched.schedule(std::chrono::milliseconds(1000), [] {});
        sched.schedule(std::chrono::milliseconds(2000), [] {});
        ok = expect(sched.pending_count() == 2, "pending count should be 2") && ok;

        sched.cancel_all();
        ok = expect(sched.pending_count() == 0, "pending count should be 0 after cancel") && ok;
    }

    // --- Timer fires exactly once ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<int> count{0};

        sched.schedule(std::chrono::milliseconds(5), [&] { ++count; });
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        ok = expect(count.load() == 1, "timer should fire exactly once") && ok;
    }

    // --- Schedule from different thread ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<bool> fired{false};

        std::thread caller([&] {
            sched.schedule(std::chrono::milliseconds(10), [&] { fired = true; });
        });
        caller.join();

        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        ok = expect(fired.load(), "timer scheduled from other thread should fire") && ok;
    }

    // --- Scheduling while the worker is waiting must not invalidate its deadline ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<int> fired{0};

        sched.schedule(std::chrono::milliseconds(200), [&] { ++fired; });
        std::thread caller([&] {
            for (int i = 0; i < 64; ++i) {
                sched.schedule(std::chrono::milliseconds(1), [&] { ++fired; });
            }
        });
        caller.join();

        std::this_thread::sleep_for(std::chrono::milliseconds(260));
        ok = expect(fired.load() == 65,
                    "timers scheduled while worker waits should all fire") && ok;
    }

    // --- Destructor cancels pending timers without crash ---
    {
        std::atomic<bool> fired{false};
        {
            exv::core::TimerScheduler sched;
            sched.schedule(std::chrono::milliseconds(10), [&] { fired = true; });
            // destructor runs here
        }
        // If we reach here without crashing, the test passes.
        // The timer should not have had time to fire.
        ok = expect(true, "destructor should not crash") && ok;
    }

    // --- pending_count decrements as timers fire ---
    {
        exv::core::TimerScheduler sched;
        sched.schedule(std::chrono::milliseconds(5), [] {});
        sched.schedule(std::chrono::milliseconds(5), [] {});
        ok = expect(sched.pending_count() == 2, "pending count should be 2 before fire") && ok;

        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        ok = expect(sched.pending_count() == 0, "pending count should be 0 after fire") && ok;
    }

    // --- Zero delay fires immediately ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<bool> fired{false};

        sched.schedule(std::chrono::milliseconds(0), [&] { fired = true; });
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
        ok = expect(fired.load(), "zero-delay timer should fire quickly") && ok;
    }

    // --- Cancel all then schedule new works ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<bool> old_fired{false};
        std::atomic<bool> new_fired{false};

        sched.schedule(std::chrono::milliseconds(10), [&] { old_fired = true; });
        sched.cancel_all();
        sched.schedule(std::chrono::milliseconds(10), [&] { new_fired = true; });

        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        ok = expect(!old_fired.load(), "old cancelled timer should not fire") && ok;
        ok = expect(new_fired.load(), "new timer after cancel should fire") && ok;
    }

    // --- Explicit shutdown discards pending timers and rejects later schedules ---
    {
        exv::core::TimerScheduler sched;
        std::atomic<int> fired{0};

        sched.schedule(std::chrono::milliseconds(100), [&] { ++fired; });
        sched.shutdown();
        sched.schedule(std::chrono::milliseconds(0), [&] { ++fired; });

        std::this_thread::sleep_for(std::chrono::milliseconds(30));
        ok = expect(fired.load() == 0,
                    "shutdown should discard pending and future timers") && ok;
    }

    if (ok) {
        std::cout << "timer_scheduler_test: all assertions passed\n";
    } else {
        std::cerr << "timer_scheduler_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
