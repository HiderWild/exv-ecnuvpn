#pragma once

#include "vpn_engine/engine.hpp"

#include <filesystem>
#include <mutex>

namespace exv {
namespace vpn_engine {

class EventSink {
public:
  virtual ~EventSink() = default;
  virtual void emit(const VpnEngineEvent &event) = 0;
};

class JsonLinesEventSink final : public EventSink {
public:
  explicit JsonLinesEventSink(std::filesystem::path path);
  void emit(const VpnEngineEvent &event) override;

private:
  std::filesystem::path path_;
  std::mutex mu_;
};

} // namespace vpn_engine
} // namespace exv
