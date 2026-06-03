#include "timing.hpp"

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

} // namespace exv::core
