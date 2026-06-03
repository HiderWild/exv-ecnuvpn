#include "helper_server.hpp"
#include "../helper_common/helper_messages.hpp"
#include <iostream>

namespace exv::helper {

class StubHelperServer : public HelperServer {
public:
    bool start(RequestHandler handler) override {
        handler_ = std::move(handler);
        running_ = true;
        std::cout << "[StubHelperServer] Started" << std::endl;
        return true;
    }

    void stop() override {
        running_ = false;
        std::cout << "[StubHelperServer] Stopped" << std::endl;
    }

    bool is_running() const override { return running_; }

private:
    RequestHandler handler_;
    bool running_ = false;
};

std::unique_ptr<HelperServer> HelperServer::create() {
    return std::make_unique<StubHelperServer>();
}

} // namespace exv::helper
