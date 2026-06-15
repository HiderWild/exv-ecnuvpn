#pragma once
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_error.hpp"
#include <functional>
#include <map>

namespace exv::helper {

struct HelperRequestContext;

class HelperRequestDispatcher {
public:
    using Handler =
        std::function<HelperResponse(const HelperRequest&,
                                     const HelperRequestContext&)>;

    void register_handler(HelperOp op, Handler handler);
    HelperResponse dispatch(const HelperRequest& request,
                            const HelperRequestContext& context);

private:
    std::map<HelperOp, Handler> handlers_;
};

} // namespace exv::helper
