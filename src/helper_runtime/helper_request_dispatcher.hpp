#pragma once
#include "../helper_common/helper_messages.hpp"
#include "../helper_common/helper_error.hpp"
#include <functional>
#include <map>

namespace exv::helper {

class HelperRequestDispatcher {
public:
    using Handler = std::function<HelperResponse(const HelperRequest&)>;

    void register_handler(HelperOp op, Handler handler);
    HelperResponse dispatch(const HelperRequest& request);

private:
    std::map<HelperOp, Handler> handlers_;
};

} // namespace exv::helper
