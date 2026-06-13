#pragma once

#include <cstddef>

namespace ecnuvpn {

// Subscribes to LogEventBus and renders TypedLogEvent → text line → logger::write().
// Must be instantiated once at process startup (lives for process lifetime).
class LogRenderer {
public:
  LogRenderer();
  ~LogRenderer();

  LogRenderer(const LogRenderer&) = delete;
  LogRenderer& operator=(const LogRenderer&) = delete;

private:
  size_t subscription_id_;
};

} // namespace ecnuvpn
