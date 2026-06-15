module;

#include "base/errors/error_info.hpp"

export module exv.base.errors;

export namespace exv::base {

using ::exv::base::ErrorInfo;

} // namespace exv::base

export namespace exv::base::error_domains {

using ::exv::base::error_domains::Auth;
using ::exv::base::error_domains::Config;
using ::exv::base::error_domains::Helper;
using ::exv::base::error_domains::Packet;
using ::exv::base::error_domains::Platform;
using ::exv::base::error_domains::Transport;

} // namespace exv::base::error_domains

export namespace exv::base::error_codes {

using ::exv::base::error_codes::AuthFailed;
using ::exv::base::error_codes::HelperUnavailable;
using ::exv::base::error_codes::InvalidConfig;
using ::exv::base::error_codes::TransportClosed;
using ::exv::base::error_codes::UnknownAction;

} // namespace exv::base::error_codes
