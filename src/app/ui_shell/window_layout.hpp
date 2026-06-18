#pragma once

namespace ecnuvpn::ui_shell {

struct WindowBounds {
  int width = 0;
  int height = 0;
};

inline constexpr WindowBounds kElectronAdvancedWindowBounds{972, 563};
inline constexpr WindowBounds kElectronMinimalWindowBounds{302, 118};

[[nodiscard]] constexpr WindowBounds advanced_window_bounds() noexcept {
  return kElectronAdvancedWindowBounds;
}

[[nodiscard]] constexpr WindowBounds minimal_window_bounds() noexcept {
  return kElectronMinimalWindowBounds;
}

} // namespace ecnuvpn::ui_shell
