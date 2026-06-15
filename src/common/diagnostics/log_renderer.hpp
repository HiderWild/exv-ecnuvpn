#pragma once

namespace ecnuvpn {

// Compatibility RAII object that enables default logging for process scopes
// which need stdout log events in addition to the file sink.
class LogRenderer {
public:
  LogRenderer();
  ~LogRenderer();

  LogRenderer(const LogRenderer&) = delete;
  LogRenderer& operator=(const LogRenderer&) = delete;

private:
  bool active_ = false;
};

} // namespace ecnuvpn
