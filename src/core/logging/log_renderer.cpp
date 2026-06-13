#include "log_renderer.hpp"
#include "log_event_bus.hpp"
#include "logger.hpp"

#include <ctime>
#include <iomanip>
#include <sstream>

namespace ecnuvpn {

static std::string render(const TypedLogEvent& event) {
  auto now = std::time(nullptr);
  auto* tm = std::localtime(&now);
  std::ostringstream ss;
  ss << "[" << std::put_time(tm, "%Y-%m-%d %H:%M:%S") << "] "
     << "[" << event.level << "] ";
  if (!event.component.empty()) {
    ss << "[" << event.component << "] ";
  }
  if (!event.code.empty()) {
    ss << "code=" << event.code << " ";
  }
  ss << event.message;
  for (const auto& field : event.fields) {
    ss << " " << field.first << "=" << field.second;
  }
  return ss.str();
}

LogRenderer::LogRenderer() {
  subscription_id_ = LogEventBus::instance().subscribe(
    [](const TypedLogEvent& event) {
      logger::write(event.level, render(event));
    }
  );
}

LogRenderer::~LogRenderer() {
  LogEventBus::instance().unsubscribe(subscription_id_);
}

} // namespace ecnuvpn
