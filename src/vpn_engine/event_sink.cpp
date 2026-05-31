#include "vpn_engine/event_sink.hpp"

#include "vpn_engine/native_engine.hpp"

#include <fstream>

namespace ecnuvpn {
namespace vpn_engine {

JsonLinesEventSink::JsonLinesEventSink(std::filesystem::path path)
    : path_(std::move(path)) {}

void JsonLinesEventSink::emit(const VpnEngineEvent &event) {
  const std::lock_guard<std::mutex> lock(mu_);

  const auto parent = path_.parent_path();
  if (!parent.empty()) {
    std::error_code ec;
    std::filesystem::create_directories(parent, ec);
  }

  std::ofstream out(path_, std::ios::binary | std::ios::app);
  if (!out.is_open())
    return;

  std::string line = event_to_json(event).dump();
  line.push_back('\n');
  out.write(line.data(), static_cast<std::streamsize>(line.size()));
}

} // namespace vpn_engine
} // namespace ecnuvpn
