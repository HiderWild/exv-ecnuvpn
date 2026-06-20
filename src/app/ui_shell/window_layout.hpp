#pragma once

namespace ecnuvpn::ui_shell {

struct WindowBounds {
  int width = 0;
  int height = 0;
};

inline constexpr int kWindowShadowMarginPx = 12;
inline constexpr WindowBounds kAppSurfaceAdvancedWindowBounds{972, 563};
inline constexpr WindowBounds kAppSurfaceMinimalWindowBounds{302, 118};
inline constexpr WindowBounds kElectronAdvancedWindowBounds{
    kAppSurfaceAdvancedWindowBounds.width + kWindowShadowMarginPx * 2,
    kAppSurfaceAdvancedWindowBounds.height + kWindowShadowMarginPx * 2};
inline constexpr WindowBounds kElectronMinimalWindowBounds{
    kAppSurfaceMinimalWindowBounds.width + kWindowShadowMarginPx * 2,
    kAppSurfaceMinimalWindowBounds.height + kWindowShadowMarginPx * 2};

[[nodiscard]] constexpr WindowBounds advanced_window_bounds() noexcept {
  return kElectronAdvancedWindowBounds;
}

[[nodiscard]] constexpr WindowBounds minimal_window_bounds() noexcept {
  return kElectronMinimalWindowBounds;
}

} // namespace ecnuvpn::ui_shell
