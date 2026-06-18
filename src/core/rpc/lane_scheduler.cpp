#include "core/rpc/lane_scheduler.hpp"

#include <condition_variable>
#include <deque>
#include <exception>
#include <mutex>
#include <thread>
#include <utility>

namespace exv::core_api {
namespace {

RpcResponse handler_exception_response(const RpcRequest& request,
                                       const std::exception& error) {
    RpcResponse response;
    response.success = false;
    response.error_code = "handler_exception";
    response.error_message = error.what();
    response.request_id = request.request_id;
    return response;
}

RpcResponse unknown_exception_response(const RpcRequest& request) {
    RpcResponse response;
    response.success = false;
    response.error_code = "handler_exception";
    response.error_message = "Unhandled RPC handler exception";
    response.request_id = request.request_id;
    return response;
}

} // namespace

struct LaneScheduler::LaneState {
    explicit LaneState(RpcLane lane_value) : lane(lane_value) {}

    RpcLane lane;
    std::mutex mutex;
    std::condition_variable cv;
    std::deque<LaneWorkItem> queue;
    std::thread worker;
    bool stopping = false;
    bool started = false;

    bool start() {
        std::lock_guard<std::mutex> lock(mutex);
        if (started) {
            return true;
        }
        stopping = false;
        started = true;
        worker = std::thread([this] { run(); });
        return true;
    }

    void stop() {
        {
            std::lock_guard<std::mutex> lock(mutex);
            stopping = true;
        }
        cv.notify_all();
        if (worker.joinable()) {
            worker.join();
        }
        std::lock_guard<std::mutex> lock(mutex);
        started = false;
    }

    bool schedule(LaneWorkItem item) {
        {
            std::lock_guard<std::mutex> lock(mutex);
            if (stopping || !started) {
                return false;
            }
            queue.push_back(std::move(item));
        }
        cv.notify_one();
        return true;
    }

    void run() {
        for (;;) {
            LaneWorkItem item;
            {
                std::unique_lock<std::mutex> lock(mutex);
                cv.wait(lock, [this] { return stopping || !queue.empty(); });
                if (queue.empty()) {
                    if (stopping) {
                        return;
                    }
                    continue;
                }
                item = std::move(queue.front());
                queue.pop_front();
            }

            RpcResponse response;
            try {
                response = item.handler(item.request);
                if (response.request_id.empty()) {
                    response.request_id = item.request.request_id;
                }
            } catch (const std::exception& error) {
                response = handler_exception_response(item.request, error);
            } catch (...) {
                response = unknown_exception_response(item.request);
            }

            if (item.respond) {
                item.respond(std::move(response));
            }
        }
    }
};

LaneScheduler::LaneScheduler()
    : control_(std::make_unique<LaneState>(RpcLane::Control)),
      read_model_(std::make_unique<LaneState>(RpcLane::ReadModel)),
      vpn_control_(std::make_unique<LaneState>(RpcLane::VpnControl)),
      config_store_(std::make_unique<LaneState>(RpcLane::ConfigStore)),
      diagnostics_(std::make_unique<LaneState>(RpcLane::Diagnostics)),
      platform_admin_(std::make_unique<LaneState>(RpcLane::PlatformAdmin)) {}

LaneScheduler::~LaneScheduler() {
    stop();
}

bool LaneScheduler::start() {
    return control_->start() &&
           read_model_->start() &&
           vpn_control_->start() &&
           config_store_->start() &&
           diagnostics_->start() &&
           platform_admin_->start();
}

void LaneScheduler::stop() {
    control_->stop();
    read_model_->stop();
    vpn_control_->stop();
    config_store_->stop();
    diagnostics_->stop();
    platform_admin_->stop();
}

bool LaneScheduler::schedule(LaneWorkItem item) {
    return state_for(item.metadata.lane).schedule(std::move(item));
}

LaneScheduler::LaneState& LaneScheduler::state_for(RpcLane lane) {
    switch (lane) {
    case RpcLane::Control:
        return *control_;
    case RpcLane::ReadModel:
        return *read_model_;
    case RpcLane::VpnControl:
        return *vpn_control_;
    case RpcLane::ConfigStore:
        return *config_store_;
    case RpcLane::Diagnostics:
        return *diagnostics_;
    case RpcLane::PlatformAdmin:
        return *platform_admin_;
    }
    return *read_model_;
}

} // namespace exv::core_api
