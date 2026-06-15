#include "helper/runtime/privileged_task_queue.hpp"

#include <algorithm>
#include <sstream>

namespace exv::helper {

PrivilegedTaskQueue::PrivilegedTaskQueue()
    : worker_([this] { worker_loop(); }) {
}

PrivilegedTaskQueue::~PrivilegedTaskQueue() {
    shutdown();
}

void PrivilegedTaskQueue::shutdown() {
    std::deque<Task> dropped_tasks;
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!stopping_) {
            stopping_ = true;
            dropped_tasks.swap(tasks_);
        }
    }
    dropped_tasks.clear();
    cv_.notify_all();
    if (worker_.joinable()) {
        worker_.join();
    }
}

TaskQueueState PrivilegedTaskQueue::state() const {
    std::lock_guard<std::mutex> lock(mutex_);
    TaskQueueState state;
    state.current_job_id = current_job_id_;
    state.pending_jobs = static_cast<int>(tasks_.size());
    state.idle = state.current_job_id.empty() && state.pending_jobs == 0;
    return state;
}

void PrivilegedTaskQueue::worker_loop() {
    {
        std::lock_guard<std::mutex> lock(mutex_);
        worker_id_ = std::this_thread::get_id();
    }

    while (true) {
        Task task;
        {
            std::unique_lock<std::mutex> lock(mutex_);
            cv_.wait(lock, [&] { return stopping_ || !tasks_.empty(); });
            if (tasks_.empty()) {
                if (stopping_) {
                    return;
                }
                continue;
            }

            task = std::move(tasks_.front());
            tasks_.pop_front();
            current_job_id_ = task.job_id;
        }

        task.run();

        {
            std::lock_guard<std::mutex> lock(mutex_);
            current_job_id_.clear();
        }
        cv_.notify_all();
    }
}

bool PrivilegedTaskQueue::is_worker_thread() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return worker_id_ == std::this_thread::get_id();
}

std::string PrivilegedTaskQueue::next_job_id(std::string label) {
    if (label.empty()) {
        label = "privileged_task";
    }
    std::replace(label.begin(), label.end(), ' ', '_');

    std::ostringstream oss;
    oss << label << '-' << next_id_++;
    return oss.str();
}

} // namespace exv::helper
