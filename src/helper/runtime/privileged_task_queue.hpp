#pragma once

#include "helper/common/helper_messages.hpp"

#include <condition_variable>
#include <cstdint>
#include <deque>
#include <functional>
#include <future>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <type_traits>
#include <utility>

namespace exv::helper {

class PrivilegedTaskQueue {
public:
    PrivilegedTaskQueue();
    ~PrivilegedTaskQueue();

    PrivilegedTaskQueue(const PrivilegedTaskQueue&) = delete;
    PrivilegedTaskQueue& operator=(const PrivilegedTaskQueue&) = delete;

    // Stops accepting new work and drops queued tasks that have not started.
    // The task currently running on the worker is allowed to finish; C++ cannot
    // safely preempt an in-flight platform call owned by that task.
    void shutdown();

    template <typename Fn>
    auto run_sync(std::string label, Fn&& fn) -> std::invoke_result_t<Fn> {
        using Result = std::invoke_result_t<Fn>;

        if (is_worker_thread()) {
            return std::forward<Fn>(fn)();
        }

        auto task = std::make_shared<std::packaged_task<Result()>>(
            std::forward<Fn>(fn));
        auto future = task->get_future();

        {
            std::lock_guard<std::mutex> lock(mutex_);
            if (stopping_) {
                throw std::runtime_error("privileged task queue is stopping");
            }
            tasks_.push_back(Task{
                next_job_id(std::move(label)),
                [task = std::move(task)] { (*task)(); },
            });
        }
        cv_.notify_one();

        if constexpr (std::is_void_v<Result>) {
            future.get();
        } else {
            return future.get();
        }
    }

    TaskQueueState state() const;

private:
    struct Task {
        std::string job_id;
        std::function<void()> run;
    };

    void worker_loop();
    bool is_worker_thread() const;
    std::string next_job_id(std::string label);

    mutable std::mutex mutex_;
    std::condition_variable cv_;
    std::deque<Task> tasks_;
    std::thread worker_;
    std::thread::id worker_id_;
    std::string current_job_id_;
    bool stopping_ = false;
    std::uint64_t next_id_ = 1;
};

} // namespace exv::helper
