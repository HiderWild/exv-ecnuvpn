#include "timing.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"

namespace exv::core {

void StageTimer::start(const std::string& stage_name) {
    stages_[stage_name].start_time = std::chrono::steady_clock::now();
    stages_[stage_name].completed = false;
    if (overall_start_ == std::chrono::steady_clock::time_point{}) {
        overall_start_ = std::chrono::steady_clock::now();
    }
}

void StageTimer::end(const std::string& stage_name) {
    auto it = stages_.find(stage_name);
    if (it != stages_.end()) {
        it->second.end_time = std::chrono::steady_clock::now();
        it->second.completed = true;
    }
}

std::chrono::milliseconds StageTimer::elapsed(const std::string& stage_name) const {
    auto it = stages_.find(stage_name);
    if (it == stages_.end()) return std::chrono::milliseconds{0};
    if (!it->second.completed) return std::chrono::milliseconds{0};
    return std::chrono::duration_cast<std::chrono::milliseconds>(
        it->second.end_time - it->second.start_time);
}

std::chrono::milliseconds StageTimer::total_elapsed() const {
    auto now = std::chrono::steady_clock::now();
    return std::chrono::duration_cast<std::chrono::milliseconds>(now - overall_start_);
}

std::map<std::string, std::chrono::milliseconds> StageTimer::all_stages() const {
    std::map<std::string, std::chrono::milliseconds> result;
    for (auto& [name, stage] : stages_) {
        if (stage.completed) {
            result[name] = std::chrono::duration_cast<std::chrono::milliseconds>(
                stage.end_time - stage.start_time);
        }
    }
    return result;
}

void StageTimer::reset() {
    stages_.clear();
    overall_start_ = {};
}

ConnectStageTimer::ConnectStageTimer(std::string scope)
    : scope_(std::move(scope)), started_(Clock::now()), last_(started_) {
    exv::observability::LogFacade::info("[connect-timing] scope=" + scope_ +
                          " stage=begin delta_ms=0 total_ms=0");
}

void ConnectStageTimer::mark(const std::string& stage,
                              const std::string& detail) {
    auto now = Clock::now();
    auto delta_ms = elapsed_ms(last_, now);
    auto total_ms = elapsed_ms(started_, now);
    last_ = now;

    std::string message = "[connect-timing] scope=" + scope_ +
                          " stage=" + stage +
                          " delta_ms=" + std::to_string(delta_ms) +
                          " total_ms=" + std::to_string(total_ms);
    if (!detail.empty())
        message += " " + detail;
    exv::observability::LogFacade::info(message);
}

void ConnectStageTimer::finish(bool ok, const std::string& detail) {
    if (finished_)
        return;
    finished_ = true;
    mark(ok ? "finish.ok" : "finish.error", detail);
}

long long ConnectStageTimer::elapsed_ms(const Clock::time_point& from,
                                         const Clock::time_point& to) {
    return std::chrono::duration_cast<std::chrono::milliseconds>(to - from)
        .count();
}

} // namespace exv::core
