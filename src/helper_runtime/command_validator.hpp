#pragma once
#include "../helper_common/helper_messages.hpp"
#include "../helper_common/helper_error.hpp"
#include <optional>
#include <set>

namespace exv::helper {

class CommandValidator {
public:
    // Validate incoming request
    std::optional<HelperError> validate(const HelperRequest& request) const;

    // Security checks
    bool is_op_allowed(HelperOp op) const;
    bool is_session_valid(const SessionId& id) const;

    // Forbidden patterns
    static bool contains_shell_injection(const std::string& input);
    static bool contains_path_traversal(const std::string& input);

private:
    // Whitelist of allowed V2 ops
    static const std::set<HelperOp> allowed_v2_ops_;
};

} // namespace exv::helper
