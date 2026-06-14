#pragma once
#include <memory>
#include <functional>

namespace exv::helper {

struct HelperRequest;
struct HelperResponse;

class HelperServer {
public:
    virtual ~HelperServer() = default;

    using RequestHandler = std::function<HelperResponse(const HelperRequest&)>;

    virtual bool start(RequestHandler handler) = 0;
    virtual void stop() = 0;
    virtual bool is_running() const = 0;

    // Platform factory
    static std::unique_ptr<HelperServer> create();
};

} // namespace exv::helper
