#include "helper_request_dispatcher.hpp"

namespace exv::helper {

void HelperRequestDispatcher::register_handler(HelperOp op, Handler handler) {
    handlers_[op] = std::move(handler);
}

HelperResponse HelperRequestDispatcher::dispatch(const HelperRequest& request) {
    auto it = handlers_.find(request.op);
    if (it == handlers_.end()) {
        HelperResponse resp;
        resp.op = request.op;
        resp.success = false;
        resp.error_code = "unsupported_op";
        resp.error_message = "No handler registered for this operation";
        return resp;
    }
    return it->second(request);
}

} // namespace exv::helper
