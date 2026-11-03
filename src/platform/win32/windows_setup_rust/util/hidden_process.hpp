#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace exv::setup {

struct HiddenProcessResult {
  bool started{false};
  // KI-13 (2026-09-28 review F2): true = wait hit timeout_ms; the child is still
  // running (exit_code reads STILL_ACTIVE 259 and must not be treated as failure).
  bool timed_out{false};
  std::uint32_t exit_code{0xFFFFFFFFu};
  std::string captured_stdout;
};

// CreateProcessW with CREATE_NO_WINDOW. Waits until exit if timeout_ms >= 0.
// timeout_ms < 0 means wait forever.
HiddenProcessResult RunHidden(const std::wstring &application,
                              const std::wstring &command_line,
                              int timeout_ms = 60000);

// Convenience: run taskkill.exe /IM <image> /T /F hidden.
bool TaskKillImage(const std::wstring &image_name);

}  // namespace exv::setup
