#include "helper/runtime/privileged_task_queue.hpp"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <exception>
#include <future>
#include <iostream>
#include <mutex>
#include <thread>

namespace {

using namespace std::chrono_literals;

bool expect(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
    }
    return condition;
}

class ManualEvent {
public:
    void signal() {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            signaled_ = true;
        }
        cv_.notify_all();
    }

    void wait() {
        std::unique_lock<std::mutex> lock(mutex_);
        cv_.wait(lock, [&] { return signaled_; });
    }

    bool wait_for(std::chrono::milliseconds timeout) {
        std::unique_lock<std::mutex> lock(mutex_);
        return cv_.wait_for(lock, timeout, [&] { return signaled_; });
    }

private:
    std::mutex mutex_;
    std::condition_variable cv_;
    bool signaled_ = false;
};

template <typename Predicate>
bool wait_until(Predicate predicate, std::chrono::milliseconds timeout) {
    const auto deadline = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < deadline) {
        if (predicate()) {
            return true;
        }
        std::this_thread::sleep_for(10ms);
    }
    return predicate();
}

int test_shutdown_drops_pending_tasks_without_running_them() {
    bool ok = true;

    exv::helper::PrivilegedTaskQueue queue;
    ManualEvent running_task_started;
    ManualEvent release_running_task;
    std::atomic<bool> pending_executed{false};
    std::atomic<bool> pending_failed{false};
    std::promise<void> pending_done;
    auto pending_done_future = pending_done.get_future();

    std::thread running_thread([&] {
        queue.run_sync("running_task", [&] {
            running_task_started.signal();
            release_running_task.wait();
        });
    });

    ok = expect(running_task_started.wait_for(500ms),
                "first privileged task should start") &&
         ok;

    std::thread pending_thread([&] {
        try {
            queue.run_sync("pending_task", [&] {
                pending_executed = true;
                return 7;
            });
        } catch (const std::exception&) {
            pending_failed = true;
        }
        pending_done.set_value();
    });

    ok = expect(wait_until([&] {
                    return queue.state().pending_jobs >= 1;
                },
                500ms),
                "second privileged task should remain pending") &&
         ok;

    std::thread shutdown_thread([&] { queue.shutdown(); });

    const auto pending_ready =
        pending_done_future.wait_for(500ms) == std::future_status::ready;
    ok = expect(pending_ready,
                "shutdown should unblock pending task callers before the "
                "running platform call completes") &&
         ok;
    ok = expect(pending_failed,
                "dropped pending task should report an exception to caller") &&
         ok;
    ok = expect(!pending_executed,
                "dropped pending task must not execute during shutdown") &&
         ok;

    release_running_task.signal();

    if (running_thread.joinable()) {
        running_thread.join();
    }
    if (pending_thread.joinable()) {
        pending_thread.join();
    }
    if (shutdown_thread.joinable()) {
        shutdown_thread.join();
    }

    return ok ? 0 : 1;
}

int test_worker_thread_run_sync_executes_reentrant_work_inline() {
    bool ok = true;
    exv::helper::PrivilegedTaskQueue queue;
    std::thread::id outer_thread;
    std::thread::id inner_thread;

    const auto result = queue.run_sync("outer_task", [&] {
        outer_thread = std::this_thread::get_id();
        return queue.run_sync("inner_task", [&] {
            inner_thread = std::this_thread::get_id();
            return 42;
        });
    });

    ok = expect(result == 42, "nested worker task should return its value") && ok;
    ok = expect(outer_thread == inner_thread,
                "nested run_sync from worker should execute inline") &&
         ok;

    return ok ? 0 : 1;
}

} // namespace

int main() {
    int failures = 0;
    failures += test_shutdown_drops_pending_tasks_without_running_them();
    failures += test_worker_thread_run_sync_executes_reentrant_work_inline();

    if (failures == 0) {
        std::cout << "privileged_task_queue_test: all tests passed\n";
        return 0;
    }

    std::cerr << "privileged_task_queue_test: " << failures
              << " test(s) FAILED\n";
    return 1;
}
