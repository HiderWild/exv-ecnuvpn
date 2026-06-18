#pragma once
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_error.hpp"
#include <optional>
#include <set>

namespace exv::helper {

class CommandValidator {
public:
    // Validate incoming request
    std::optional<HelperError> validate(const HelperRequest& request) const;

    // Security checks
    bool is_op_allowed(HelperOp op) const;

    // Forbidden patterns
    static bool contains_shell_injection(const std::string& input);
    static bool contains_path_traversal(const std::string& input);

private:
    static const std::set<HelperOp> allowed_ops_;
};

} // namespace exv::helper
