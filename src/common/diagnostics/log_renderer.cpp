#include "common/diagnostics/log_renderer.hpp"

#include "platform/common/logging/log_runtime.hpp"

namespace exv {

LogRenderer::LogRenderer() {
  platform::logging::configure_default_logging(true);
  active_ = true;
}

LogRenderer::~LogRenderer() {
  if (active_) {
    platform::logging::shutdown_default_logging();
  }
}

} // namespace exv
